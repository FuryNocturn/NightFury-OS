// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Unidades extraíbles con el formato propio (NFFS), en claro o cifradas con `cryptdev`.
//!
//! CONTRATO (v0.11): `SectorBlocks`, `open`, `Opened`, `LockedDrive::unlock` y `format` no cambian
//! de firma.
//!
//! Disposición de una unidad: tabla MBR con una partición de tipo `PTYPE`; dentro, el NFFS de
//! siempre en bloques de 4096 bytes. Cifrada, el NFFS ocupa `N-2` bloques cifrados con `cryptdev`
//! (sin conversión: nace cifrada) y los dos últimos son el de trabajo y la cabecera. La etiqueta
//! es el contenido del archivo oculto `.nflabel` de la raíz.
use super::{part, FsKind, Partition, SectorDev, Volume};
use crate::block::{BlockDev, DiskError, BLOCK};
use crate::crypto::{wipe, Pbkdf2Paso};
use crate::cryptdev::{self, Core, CryptDev, Header, State};
use crate::fs::{same, Entry, FsError, Nffs};
use crate::keys::{self, KeyError, Pool};
use crate::platform::DateTime;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

/// Tipo de partición MBR (0xDA: «datos sin sistema de archivos»; ningún otro sistema la monta solo).
const PTYPE: u8 = 0xDA;
/// La etiqueta del volumen vive en este archivo de la raíz: `list` no lo enseña y desde fuera ni
/// se lee, ni se escribe, ni se borra.
const LABEL_FILE: &str = ".nflabel";
/// Longitud máxima de la etiqueta, en caracteres.
const LABEL_MAX: usize = 32;

/// Bloques de 4096 bytes (`BlockDev`) sobre un rango de sectores de un `SectorDev`
/// (la partición `start..start+sectors`). Los bloques que no caben enteros al final no se usan.
pub struct SectorBlocks {
    pub dev: Box<dyn SectorDev>,
    pub start: u64,
    pub sectors: u64,
}

impl SectorBlocks {
    /// Sectores por bloque (0 si el tamaño de sector no divide a 4096: entonces no hay bloques).
    fn per_block(&self) -> u64 {
        let ss = self.dev.sector_size() as u64;
        if ss == 0 || BLOCK as u64 % ss != 0 {
            0
        } else {
            BLOCK as u64 / ss
        }
    }

    /// Primer sector del bloque `n` si el bloque y el búfer son válidos.
    fn lba(&self, n: u64, len: usize) -> Result<u64, DiskError> {
        if len != BLOCK || n >= self.blocks() {
            return Err(DiskError::Range);
        }
        Ok(self.start + n * self.per_block())
    }
}

impl BlockDev for SectorBlocks {
    fn blocks(&self) -> u64 {
        match self.per_block() {
            0 => 0,
            spb => self.sectors.min(self.dev.sectors().saturating_sub(self.start)) / spb,
        }
    }
    fn read(&mut self, n: u64, buf: &mut [u8]) -> Result<(), DiskError> {
        let lba = self.lba(n, buf.len())?;
        self.dev.read(lba, buf).map_err(|_| DiskError::Io)
    }
    fn write(&mut self, n: u64, buf: &[u8]) -> Result<(), DiskError> {
        let lba = self.lba(n, buf.len())?;
        self.dev.write(lba, buf).map_err(|_| DiskError::Io)
    }
    fn flush(&mut self) -> Result<(), DiskError> {
        self.dev.flush().map_err(|_| DiskError::Io)
    }
}

/// Resultado de abrir una partición `FsKind::NightFury`.
pub enum Opened {
    Ready(Box<dyn Volume>),
    /// Cifrada: hace falta la contraseña (o el código de recuperación).
    Locked(LockedDrive),
}

/// Volumen NightFury cifrado, todavía cerrado.
pub struct LockedDrive {
    dev: Box<dyn BlockDev>,
    hdr: Header,
    ro: bool,
}

impl LockedDrive {
    /// Abre con la contraseña o, si `recovery`, con el código de recuperación. Si falla, devuelve
    /// la unidad para poder reintentar.
    pub fn unlock(self, secret: &str, recovery: bool) -> Result<Box<dyn Volume>, (LockedDrive, KeyError)> {
        let LockedDrive { dev, hdr, ro } = self;
        match cryptdev::open(dev, hdr, secret, recovery) {
            Ok(c) => Self::montar(c, ro),
            Err((dev, hdr, e)) => Err((LockedDrive { dev, hdr, ro }, e)),
        }
    }

    /// Empieza el `unlock` por pasos: el PBKDF2 que hay que avanzar (`Pbkdf2Paso::step`) antes de `unlock_con`.
    pub fn unlock_paso(&self, secret: &str, recovery: bool) -> Result<Pbkdf2Paso, KeyError> {
        cryptdev::unlock_paso(&self.hdr, secret, recovery)
    }

    /// Termina el `unlock` por pasos con la clave derivada (`Pbkdf2Paso::take`); la borra.
    pub fn unlock_con(self, dk: &mut [u8], recovery: bool) -> Result<Box<dyn Volume>, (LockedDrive, KeyError)> {
        let LockedDrive { dev, hdr, ro } = self;
        match cryptdev::open_con(dev, hdr, dk, recovery) {
            Ok(c) => Self::montar(c, ro),
            Err((dev, hdr, e)) => Err((LockedDrive { dev, hdr, ro }, e)),
        }
    }

    fn montar(core: Core, ro: bool) -> Result<Box<dyn Volume>, (LockedDrive, KeyError)> {
        let core = Rc::new(RefCell::new(core));
        // Con la clave buena ya se puede escribir: rehace el bloque que quedó a medias o remata la
        // conversión. Si algo falla, la unidad vuelve a quedar cerrada.
        // shortcut: una unidad a medio convertir se monta tal cual y la conversión no sigue; aquí solo se crean unidades cifradas del todo.
        let rec = core.borrow_mut().recover().map_err(cryptdev::disk_err);
        let r = rec.and_then(|()| {
            Nffs::mount(Box::new(CryptDev::new(core.clone()))).map_err(|_| KeyError::Fs(FsError::Corrupt))
        });
        match r {
            Ok(fs) => Ok(Box::new(NfVolume::new(fs, ro))),
            Err(e) => {
                let hdr = core.borrow().header().clone();
                let dev = core.borrow_mut().take_dev().unwrap_or_else(|| Box::new(NoDev));
                Err((LockedDrive { dev, hdr, ro }, e))
            }
        }
    }
}

/// Dispositivo vacío: solo por si `take_dev` no devolviera el de verdad (no puede pasar aquí).
struct NoDev;
impl BlockDev for NoDev {
    fn blocks(&self) -> u64 {
        0
    }
    fn read(&mut self, _: u64, _: &mut [u8]) -> Result<(), DiskError> {
        Err(DiskError::Io)
    }
    fn write(&mut self, _: u64, _: &[u8]) -> Result<(), DiskError> {
        Err(DiskError::Io)
    }
}

/// `path` es el archivo de la etiqueta (en la raíz, sin distinguir mayúsculas).
fn is_label(path: &str) -> bool {
    let mut c = path.split('\\').filter(|c| !c.is_empty());
    matches!((c.next(), c.next()), (Some(n), None) if same(n, LABEL_FILE))
}

fn guard(path: &str) -> Result<(), FsError> {
    if is_label(path) {
        Err(FsError::Denied)
    } else {
        Ok(())
    }
}

/// Un NFFS de unidad extraíble como `Volume`.
struct NfVolume {
    fs: Nffs,
    label: String,
    ro: bool,
}

impl NfVolume {
    fn new(mut fs: Nffs, ro: bool) -> NfVolume {
        let label = fs.read(LABEL_FILE).ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
        NfVolume { fs, label, ro }
    }
}

impl Volume for NfVolume {
    fn kind(&self) -> FsKind {
        FsKind::NightFury
    }
    fn label(&self) -> String {
        self.label.clone()
    }
    fn space(&mut self) -> (u64, u64) {
        self.fs.space()
    }
    fn read_only(&self) -> bool {
        self.ro
    }
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, FsError> {
        let mut v = self.fs.list(dir)?;
        if dir.split('\\').all(str::is_empty) {
            v.retain(|e| !same(&e.name, LABEL_FILE));
        }
        Ok(v)
    }
    fn stat(&mut self, path: &str) -> Result<Entry, FsError> {
        if is_label(path) {
            return Err(FsError::NotFound);
        }
        self.fs.stat(path)
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        if is_label(path) {
            return Err(FsError::NotFound);
        }
        self.fs.read(path)
    }
    fn write(&mut self, path: &str, data: &[u8], now: DateTime) -> Result<(), FsError> {
        guard(path)?;
        self.fs.write(path, data, now)
    }
    fn mkdir(&mut self, path: &str, now: DateTime) -> Result<(), FsError> {
        guard(path)?;
        self.fs.mkdir(path, now)
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        guard(from)?;
        guard(to)?;
        self.fs.rename(from, to)
    }
    fn delete(&mut self, path: &str) -> Result<(), FsError> {
        guard(path)?;
        self.fs.remove(path)
    }
    fn flush(&mut self) -> Result<(), FsError> {
        // Cada operación del NFFS confirma y hace `flush` del dispositivo; nada queda pendiente.
        Ok(())
    }
}

/// Abre la partición `p` (de tipo `NightFury`) del disco.
pub fn open(dev: Box<dyn SectorDev>, p: &Partition) -> Result<Opened, FsError> {
    let ro = dev.read_only();
    let mut blocks: Box<dyn BlockDev> = Box::new(SectorBlocks { dev, start: p.start, sectors: p.sectors });
    // Cabecera de cifrado íntegra en el último bloque. Si el NFFS ocupa todo el dispositivo, ese
    // bloque es suyo y no una cabecera (igual que en `Files::probe`).
    if let Some(hdr) = cryptdev::detect(&mut *blocks) {
        if Nffs::volume_blocks_of(&mut *blocks) != Some(blocks.blocks()) {
            return Ok(Opened::Locked(LockedDrive { dev: blocks, hdr, ro }));
        }
    }
    Ok(Opened::Ready(Box::new(NfVolume::new(Nffs::mount(blocks)?, ro))))
}

/// Borra el disco entero y crea una partición MBR con NFFS dentro; con `encrypt`, cifrada con esa
/// contraseña. La etiqueta se guarda dentro del volumen. Devuelve el código de recuperación si se
/// cifró.
pub fn format(dev: Box<dyn SectorDev>, label: &str, encrypt: Option<(&str, &mut Pool)>, now: DateTime) -> Result<Option<String>, KeyError> {
    format_at(dev, label, encrypt, now, true)
}

/// Como `format`, pero el dispositivo es ya una partición: el volumen ocupa todo y no se toca la tabla.
pub fn format_in(dev: Box<dyn SectorDev>, label: &str, encrypt: Option<(&str, &mut Pool)>, now: DateTime) -> Result<Option<String>, KeyError> {
    format_at(dev, label, encrypt, now, false)
}

fn format_at(dev: Box<dyn SectorDev>, label: &str, encrypt: Option<(&str, &mut Pool)>, now: DateTime, table: bool) -> Result<Option<String>, KeyError> {
    if label.chars().count() > LABEL_MAX || label.chars().any(|c| c.is_control()) {
        return Err(KeyError::Fs(FsError::BadName));
    }
    if let Some((password, pool)) = &encrypt {
        if password.chars().count() < crate::accounts::MIN_PASSWORD {
            return Err(KeyError::Weak);
        }
        if !pool.ready() {
            return Err(KeyError::NotReady);
        }
    }
    let mut dev = dev;
    let (start, sectors) = if table { part::prepare(&mut *dev)? } else { part::whole(&mut *dev)? };
    let mut blocks = SectorBlocks { dev, start, sectors };
    let n = blocks.blocks();
    // Mínimos: el NFFS pide 64 bloques y el cifrado reserva dos más.
    if n < if encrypt.is_some() { 66 } else { 64 } {
        return Err(if encrypt.is_some() { KeyError::NoRoom } else { KeyError::Fs(FsError::NoSpace) });
    }
    // Borra firmas viejas: el primer bloque (el NFFS lo reescribe) y los dos últimos (trabajo y cabecera).
    let zero = [0u8; BLOCK];
    for b in [0, n - 2, n - 1] {
        blocks.write(b, &zero).map_err(cryptdev::disk_err)?;
    }
    // La tabla de particiones va antes de mover el disco al volumen.
    if table {
        part::write_mbr(&mut *blocks.dev, start, sectors, PTYPE)?;
    }
    let (dev, code): (Box<dyn BlockDev>, Option<String>) = match encrypt {
        None => (Box::new(blocks), None),
        Some((password, pool)) => {
            let mut master = pool.master();
            let (mut s1, mut i1, mut s2, mut i2) = ([0u8; 16], [0u8; 16], [0u8; 16], [0u8; 16]);
            pool.take(&mut s1);
            pool.take(&mut i1);
            pool.take(&mut s2);
            pool.take(&mut i2);
            let code = keys::recovery_code(pool);
            let mut rsecret = keys::normalize_recovery(&code).unwrap_or_default().into_bytes();
            let hdr = Header {
                state: State::Encrypted,
                next: 0,
                pending: false,
                disk: keys::Slot::wrap(&master, password.as_bytes(), s1, i1),
                recovery: keys::Slot::wrap(&master, &rsecret, s2, i2),
            };
            wipe(&mut rsecret);
            for b in [&mut s1, &mut i1, &mut s2, &mut i2] {
                wipe(b);
            }
            let w = cryptdev::write_header(&mut blocks, &hdr);
            let core = Rc::new(RefCell::new(Core::new(Box::new(blocks), hdr, &master)));
            wipe(&mut master);
            w.map_err(cryptdev::disk_err)?;
            (Box::new(CryptDev::new(core)), Some(code))
        }
    };
    let mut fs = Nffs::format(dev, now)?;
    fs.write(LABEL_FILE, label.as_bytes(), now)?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vol::{partitions, MemSectors};
    use alloc::vec;

    const NOW: DateTime = DateTime { year: 2026, month: 11, day: 18, hour: 12, minute: 0, second: 0 };

    fn pool() -> Pool {
        let mut p = Pool::new();
        for i in 0..keys::POOL_EVENTS_NEEDED as i32 {
            p.add_event(i, i * 3, i as u64, i as u64 * 7);
        }
        p
    }

    /// Disco en memoria que sobrevive a los volúmenes (cada uno recibe un manejador nuevo).
    #[derive(Clone)]
    struct Pen(Rc<RefCell<MemSectors>>);

    impl Pen {
        /// 16 MiB de basura (el formato debe borrarla).
        fn nuevo(ss: usize) -> Pen {
            let mut m = MemSectors::new((16 << 20) / ss as u64, ss);
            m.data.fill(0xA5);
            Pen(Rc::new(RefCell::new(m)))
        }
        fn dev(&self) -> Box<dyn SectorDev> {
            Box::new(self.clone())
        }
        fn abrir(&self) -> Opened {
            let p = partitions(&mut self.clone()).unwrap();
            assert_eq!(p.len(), 1);
            assert_eq!(p[0].kind, FsKind::NightFury);
            open(self.dev(), &p[0]).unwrap()
        }
    }

    impl SectorDev for Pen {
        fn sector_size(&self) -> usize {
            self.0.borrow().sector
        }
        fn sectors(&self) -> u64 {
            self.0.borrow().sectors()
        }
        fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
            self.0.borrow_mut().read(lba, buf)
        }
        fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError> {
            self.0.borrow_mut().write(lba, buf)
        }
        fn read_only(&self) -> bool {
            self.0.borrow().ro
        }
    }

    fn listar(v: &mut dyn Volume, dir: &str) -> Vec<String> {
        let mut n: Vec<String> = v.list(dir).unwrap().into_iter().map(|e| e.name).collect();
        n.sort();
        n
    }

    fn usar(v: &mut dyn Volume) {
        v.mkdir("Fotos", NOW).unwrap();
        v.write("Fotos\\a.txt", b"hola", NOW).unwrap();
        v.write("grande.bin", &vec![7u8; 300_000], NOW).unwrap();
        v.rename("Fotos\\a.txt", "Fotos\\b.txt").unwrap();
        assert_eq!(v.read("fotos\\B.TXT").unwrap(), b"hola");
        assert_eq!(listar(v, ""), ["Fotos", "grande.bin"], "la etiqueta no se enseña");
        assert_eq!(listar(v, "\\"), ["Fotos", "grande.bin"]);
        assert!(v.read(LABEL_FILE).is_err() && v.stat(".NFLABEL").is_err());
        assert_eq!(v.write(".NFLABEL", b"x", NOW), Err(FsError::Denied));
        assert_eq!(v.delete(".nflabel"), Err(FsError::Denied));
        assert_eq!(v.rename("grande.bin", ".nflabel"), Err(FsError::Denied));
        assert_eq!(v.delete("Fotos"), Err(FsError::NotEmpty));
        v.delete("Fotos\\b.txt").unwrap();
        v.delete("Fotos").unwrap();
        v.mkdir("Docs", NOW).unwrap();
        v.write("Docs\\x.txt", b"persistente", NOW).unwrap();
        v.flush().unwrap();
    }

    fn comprobar_persistencia(v: &mut dyn Volume) {
        assert_eq!(listar(v, ""), ["Docs", "grande.bin"]);
        assert_eq!(v.read("Docs\\x.txt").unwrap(), b"persistente");
        assert_eq!(v.read("grande.bin").unwrap(), vec![7u8; 300_000]);
    }

    #[test]
    fn en_claro_sectores_512_y_4096() {
        for ss in [512, 4096] {
            let pen = Pen::nuevo(ss);
            assert_eq!(format(pen.dev(), "MiUSB", None, NOW), Ok(None));
            let Opened::Ready(mut v) = pen.abrir() else { panic!("ss={ss}: debía estar abierto") };
            assert_eq!((v.kind(), v.label(), v.read_only()), (FsKind::NightFury, "MiUSB".into(), false));
            let (total, libre) = v.space();
            assert!(total >= 15 << 20 && libre > 10 << 20, "{total} {libre}");
            usar(v.as_mut());
            drop(v);
            let Opened::Ready(mut v) = pen.abrir() else { panic!() };
            assert_eq!(v.label(), "MiUSB");
            comprobar_persistencia(v.as_mut());
        }
    }

    #[test]
    fn cifrado_contrasena_y_recuperacion() {
        for ss in [512, 4096] {
            let pen = Pen::nuevo(ss);
            let code = format(pen.dev(), "Secreto", Some(("clave-buena", &mut pool())), NOW).unwrap().expect("código de recuperación");
            let Opened::Locked(d) = pen.abrir() else { panic!("ss={ss}: debía estar cerrada") };
            // Contraseña mala: devuelve la unidad; un código mal tecleado también.
            let (d, e) = d.unlock("otra-clave", false).err().unwrap();
            assert_eq!(e, KeyError::WrongSecret);
            let (d, e) = d.unlock("AAAAA", true).err().unwrap();
            assert_eq!(e, KeyError::WrongSecret);
            let mut v = d.unlock("clave-buena", false).ok().unwrap();
            assert_eq!((v.kind(), v.label()), (FsKind::NightFury, "Secreto".into()));
            usar(v.as_mut());
            drop(v);
            // Nada de lo escrito está en claro en el disco.
            let raw = pen.0.borrow().data.clone();
            assert!(!raw.windows(11).any(|w| w == b"persistente") && !raw.windows(7).any(|w| w == b"Secreto"));
            // Se vuelve a cerrar y abre con el código de recuperación (en minúsculas y sin guiones también).
            let Opened::Locked(d) = pen.abrir() else { panic!() };
            let mut v = d.unlock(&code.to_lowercase().replace('-', ""), true).ok().unwrap();
            comprobar_persistencia(v.as_mut());
        }
    }

    #[test]
    fn formato_rechaza_lo_invalido_sin_tocar_el_disco() {
        let pen = Pen::nuevo(512);
        let antes = pen.0.borrow().data.clone();
        assert_eq!(format(pen.dev(), "x", Some(("corta", &mut pool())), NOW), Err(KeyError::Weak));
        assert_eq!(format(pen.dev(), "x", Some(("clave-buena", &mut Pool::new())), NOW), Err(KeyError::NotReady));
        assert_eq!(format(pen.dev(), &"a".repeat(33), None, NOW), Err(KeyError::Fs(FsError::BadName)));
        assert_eq!(format(pen.dev(), "a\nb", None, NOW), Err(KeyError::Fs(FsError::BadName)));
        assert!(pen.0.borrow().data == antes);
        // Disco de solo lectura.
        pen.0.borrow_mut().ro = true;
        assert_eq!(format(pen.dev(), "x", None, NOW), Err(KeyError::Fs(FsError::Denied)));
        // Disco diminuto.
        let mut p = pool();
        let mini = MemSectors::new(100, 512);
        assert!(format(Box::new(mini), "x", Some(("clave-buena", &mut p)), NOW).is_err());
    }

    #[test]
    fn solo_lectura_se_nota() {
        let pen = Pen::nuevo(512);
        format(pen.dev(), "RO", None, NOW).unwrap();
        pen.0.borrow_mut().ro = true;
        let Opened::Ready(v) = pen.abrir() else { panic!() };
        assert!(v.read_only());
    }

    #[test]
    fn sector_blocks_limites() {
        let pen = Pen::nuevo(512);
        let mut b = SectorBlocks { dev: pen.dev(), start: 10, sectors: 100 };
        assert_eq!(b.blocks(), 12); // 100 sectores de 512 = 12 bloques y media
        let mut buf = [0u8; BLOCK];
        assert!(b.write(11, &[9u8; BLOCK]).is_ok() && b.read(11, &mut buf).is_ok() && buf == [9u8; BLOCK]);
        assert_eq!(b.read(12, &mut buf), Err(DiskError::Range));
        assert_eq!(b.read(0, &mut buf[..100]), Err(DiskError::Range));
        // El bloque 11 empieza en el sector 10 + 88 del disco.
        assert_eq!(pen.0.borrow().data[98 * 512], 9);
    }
}
