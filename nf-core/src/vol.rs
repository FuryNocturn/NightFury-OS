// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Volúmenes ajenos (pendrives, discos USB, CD/DVD): sectores, particiones y sistemas de archivos
//! FAT12/16/32, exFAT, ISO9660 y UDF.
//!
//! CONTRATO (v0.11): `SectorDev`, `MemSectors`, `Volume`, `FsKind`, `Partition`, `partitions`,
//! `mount` y `format` no cambian de firma. Las implementaciones van en los submódulos.
//!
//! Rutas dentro de un volumen: relativas a su raíz, con `\` como separador y sin letra de unidad:
//! `""` o `"\\"` es la raíz; `"Fotos\\a.jpg"` un archivo. Los nombres se comparan sin distinguir
//! mayúsculas (como Windows) y se conservan tal como se escribieron.
use crate::fs::{Entry, FsError};
use crate::platform::DateTime;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Disco por sectores (lo que da un pendrive: 512 bytes, 2048 en un CD, a veces 4096).
pub trait SectorDev {
    /// Tamaño de sector en bytes (potencia de dos, 512..=4096).
    fn sector_size(&self) -> usize;
    /// Número de sectores.
    fn sectors(&self) -> u64;
    /// Lee `buf.len() / sector_size()` sectores desde `lba`. `buf.len()` es múltiplo del sector.
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError>;
    /// Escribe sectores desde `lba`. `Err(FsError::Denied)` si el soporte es de solo lectura.
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError>;
    /// Asegura que lo escrito ha llegado al soporte.
    fn flush(&mut self) -> Result<(), FsError> {
        Ok(())
    }
    /// Soporte de solo lectura (CD, interruptor de protección).
    fn read_only(&self) -> bool {
        false
    }
}

/// Disco por sectores en memoria (tests y pruebas).
pub struct MemSectors {
    pub data: Vec<u8>,
    pub sector: usize,
    pub ro: bool,
}

impl MemSectors {
    pub fn new(sectors: u64, sector: usize) -> Self {
        MemSectors { data: vec![0; sectors as usize * sector], sector, ro: false }
    }
}

impl SectorDev for MemSectors {
    fn sector_size(&self) -> usize {
        self.sector
    }
    fn sectors(&self) -> u64 {
        (self.data.len() / self.sector) as u64
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        let o = lba as usize * self.sector;
        if buf.len() % self.sector != 0 || o + buf.len() > self.data.len() {
            return Err(FsError::Disk);
        }
        buf.copy_from_slice(&self.data[o..o + buf.len()]);
        Ok(())
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError> {
        if self.ro {
            return Err(FsError::Denied);
        }
        let o = lba as usize * self.sector;
        if buf.len() % self.sector != 0 || o + buf.len() > self.data.len() {
            return Err(FsError::Disk);
        }
        self.data[o..o + buf.len()].copy_from_slice(buf);
        Ok(())
    }
    fn read_only(&self) -> bool {
        self.ro
    }
}

/// Sistema de archivos reconocido.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FsKind {
    Fat12,
    Fat16,
    Fat32,
    ExFat,
    Iso9660,
    Udf,
    /// El formato propio (NFFS), en claro o cifrado: lo monta `files`, no este módulo.
    NightFury,
    /// HFS+, APFS…: no se monta.
    Unknown,
    /// NTFS (Windows), en solo lectura (v0.11.1).
    Ntfs,
    /// ext2, ext3 y ext4 (Linux), en solo lectura (v0.11.1).
    Ext,
    /// Cifrada con BitLocker: se ve bloqueada y no se abre.
    BitLocker,
}

/// Partición encontrada en el disco (o el disco entero si no hay tabla).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Partition {
    /// Primer sector (del disco) y número de sectores.
    pub start: u64,
    pub sectors: u64,
    /// Lo que hay dentro, mirando el sector de arranque o el superbloque.
    pub kind: FsKind,
}

/// Un sistema de archivos montado.
pub trait Volume {
    fn kind(&self) -> FsKind;
    /// Etiqueta del volumen ("" si no tiene).
    fn label(&self) -> String;
    /// (bytes totales, bytes libres).
    fn space(&mut self) -> (u64, u64);
    fn read_only(&self) -> bool;
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, FsError>;
    fn stat(&mut self, path: &str) -> Result<Entry, FsError>;
    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError>;
    /// Crea o sobrescribe un archivo (la carpeta padre debe existir).
    fn write(&mut self, path: &str, data: &[u8], now: DateTime) -> Result<(), FsError>;
    fn mkdir(&mut self, path: &str, now: DateTime) -> Result<(), FsError>;
    /// Renombra o mueve dentro del mismo volumen.
    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError>;
    /// Borra un archivo o una carpeta vacía.
    fn delete(&mut self, path: &str) -> Result<(), FsError>;
    /// Escribe lo pendiente y deja el volumen limpio (antes de expulsar).
    fn flush(&mut self) -> Result<(), FsError>;
    /// Aviso para mostrar al conectar la unidad (p. ej. un ext con el diario a medias).
    fn notice(&self) -> Option<&'static str> {
        None
    }
}

/// Lee la tabla de particiones (GPT, si no MBR; si no hay, el disco entero es una partición) y
/// reconoce el sistema de archivos de cada una. Las particiones vacías o de tipo 0 no salen.
pub fn partitions(dev: &mut dyn SectorDev) -> Result<Vec<Partition>, FsError> {
    part::partitions(dev)
}

/// Monta la partición `p` de `dev`. `Err(FsError::Corrupt)` si el formato es `NightFury` o
/// `Unknown` o está dañado. Los de solo lectura (ISO, UDF) devuelven `Denied` al escribir.
pub fn mount(dev: Box<dyn SectorDev>, p: &Partition) -> Result<Box<dyn Volume>, FsError> {
    match p.kind {
        FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32 => fat::mount(dev, p),
        FsKind::ExFat => exfat::mount(dev, p),
        FsKind::Iso9660 => iso::mount(dev, p),
        FsKind::Udf => udf::mount(dev, p),
        FsKind::Ntfs => ntfs::mount(dev, p),
        FsKind::Ext => ext::mount(dev, p),
        FsKind::BitLocker => Ok(Box::new(Sealed { kind: FsKind::BitLocker, bytes: p.sectors * dev.sector_size() as u64 })),
        FsKind::NightFury | FsKind::Unknown => Err(FsError::Corrupt),
    }
}

/// Formatea el disco entero con una sola partición MBR del tipo pedido (`Fat32` o `ExFat`; con
/// `Fat32` en discos de menos de 64 MiB se usa FAT16 o FAT12) y la etiqueta dada.
pub fn format(dev: &mut dyn SectorDev, kind: FsKind, label: &str, now: DateTime) -> Result<(), FsError> {
    match kind {
        FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32 => fat::format(dev, label, now),
        FsKind::ExFat => exfat::format(dev, label, now),
        _ => Err(FsError::Invalid),
    }
}

/// Formatea el dispositivo entero, que ya es una partición (p. ej. una `Window`), sin crear ni tocar
/// ninguna tabla de particiones. Mismos formatos que `format`.
pub fn format_in(dev: &mut dyn SectorDev, kind: FsKind, label: &str, now: DateTime) -> Result<(), FsError> {
    match kind {
        FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32 => fat::format_in(dev, label, now),
        FsKind::ExFat => exfat::format_in(dev, label, now),
        _ => Err(FsError::Invalid),
    }
}

/// Ajusta el tipo de la partición que empieza en `start` a lo que contiene (ver `part::set_type`).
pub fn set_partition_type(dev: &mut dyn SectorDev, start: u64, kind: FsKind) -> Result<(), FsError> {
    part::set_type(dev, start, kind)
}

/// Ventana `[start, start+sectors)` de otro dispositivo: nada de lo que se lea o escriba por ella
/// puede salirse de la partición. Así formatear una partición interna no puede tocar las demás.
pub struct Window {
    dev: Box<dyn SectorDev>,
    start: u64,
    sectors: u64,
}

impl Window {
    pub fn new(dev: Box<dyn SectorDev>, start: u64, sectors: u64) -> Window {
        let sectors = sectors.min(dev.sectors().saturating_sub(start));
        Window { dev, start, sectors }
    }
    fn check(&self, lba: u64, len: usize) -> Result<(), FsError> {
        let ss = self.dev.sector_size();
        if len % ss != 0 || lba.checked_add((len / ss) as u64).map_or(true, |x| x > self.sectors) {
            return Err(FsError::Disk);
        }
        Ok(())
    }
}

impl SectorDev for Window {
    fn sector_size(&self) -> usize {
        self.dev.sector_size()
    }
    fn sectors(&self) -> u64 {
        self.sectors
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        self.check(lba, buf.len())?;
        self.dev.read(self.start + lba, buf)
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError> {
        self.check(lba, buf.len())?;
        self.dev.write(self.start + lba, buf)
    }
    fn flush(&mut self) -> Result<(), FsError> {
        self.dev.flush()
    }
    fn read_only(&self) -> bool {
        self.dev.read_only()
    }
}

/// Unidad que se ve pero no se abre (BitLocker): sin contenido y sin escritura.
struct Sealed {
    kind: FsKind,
    bytes: u64,
}

impl Volume for Sealed {
    fn kind(&self) -> FsKind {
        self.kind
    }
    fn label(&self) -> String {
        String::new()
    }
    fn space(&mut self) -> (u64, u64) {
        (self.bytes, 0)
    }
    fn read_only(&self) -> bool {
        true
    }
    fn list(&mut self, _: &str) -> Result<Vec<Entry>, FsError> {
        Err(FsError::Denied)
    }
    fn stat(&mut self, path: &str) -> Result<Entry, FsError> {
        if path.trim_matches('\\').is_empty() {
            return Ok(Entry { name: String::new(), dir: true, size: 0, modified: DateTime::default(), hidden: false });
        }
        Err(FsError::Denied)
    }
    fn read(&mut self, _: &str) -> Result<Vec<u8>, FsError> {
        Err(FsError::Denied)
    }
    fn write(&mut self, _: &str, _: &[u8], _: DateTime) -> Result<(), FsError> {
        Err(FsError::Denied)
    }
    fn mkdir(&mut self, _: &str, _: DateTime) -> Result<(), FsError> {
        Err(FsError::Denied)
    }
    fn rename(&mut self, _: &str, _: &str) -> Result<(), FsError> {
        Err(FsError::Denied)
    }
    fn delete(&mut self, _: &str) -> Result<(), FsError> {
        Err(FsError::Denied)
    }
    fn flush(&mut self) -> Result<(), FsError> {
        Ok(())
    }
}

/// Envoltorio de solo lectura: las particiones internas que no son de NightFury se montan así, para
/// que ningún fallo pueda escribir en el Windows o el Linux del PC.
pub struct ReadOnly(pub Box<dyn Volume>);

impl Volume for ReadOnly {
    fn kind(&self) -> FsKind {
        self.0.kind()
    }
    fn label(&self) -> String {
        self.0.label()
    }
    fn space(&mut self) -> (u64, u64) {
        self.0.space()
    }
    fn read_only(&self) -> bool {
        true
    }
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, FsError> {
        self.0.list(dir)
    }
    fn notice(&self) -> Option<&'static str> {
        self.0.notice()
    }
    fn stat(&mut self, path: &str) -> Result<Entry, FsError> {
        self.0.stat(path)
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        self.0.read(path)
    }
    fn write(&mut self, _: &str, _: &[u8], _: DateTime) -> Result<(), FsError> {
        Err(FsError::Denied)
    }
    fn mkdir(&mut self, _: &str, _: DateTime) -> Result<(), FsError> {
        Err(FsError::Denied)
    }
    fn rename(&mut self, _: &str, _: &str) -> Result<(), FsError> {
        Err(FsError::Denied)
    }
    fn delete(&mut self, _: &str) -> Result<(), FsError> {
        Err(FsError::Denied)
    }
    fn flush(&mut self) -> Result<(), FsError> {
        Ok(())
    }
}

pub mod exfat;
pub mod ext;
pub mod fat;
pub mod instalar;
pub mod iso;
pub mod nfvol;
pub mod ntfs;
pub use nfvol::LockedDrive;
pub mod part;
pub mod udf;

#[cfg(test)]
mod tests {
    use super::*;

    /// Imagen real por ruta (`NF_IMG`): monta la primera partición y lee `Carpeta\Sub\texto.txt`.
    #[test]
    #[ignore]
    fn imagen_real() {
        extern crate std;
        let Ok(path) = std::env::var("NF_IMG") else { return };
        let mut m = MemSectors::new(0, 2048);
        m.data = std::fs::read(path).unwrap();
        let p = partitions(&mut m).unwrap();
        std::println!("particiones: {:?}", p);
        let mut v = mount(Box::new(m), &p[0]).unwrap();
        std::println!("{:?} «{}» raíz: {:?}", v.kind(), v.label(), v.list("").unwrap().iter().map(|e| e.name.clone()).collect::<Vec<_>>());
        // ISO9660 sin Joliet solo tiene nombres 8.3: ahí el nombre largo no existe.
        match v.read(r"carpeta\SUB\Texto con ñ y acentos áé.txt") {
            Ok(t) => std::println!("texto: {}", core::str::from_utf8(&t).unwrap()),
            Err(e) => std::println!("sin nombre largo: {:?} {:?}", e, v.list(r"carpeta\sub").unwrap()),
        }
        assert_eq!(v.read("grande.bin").unwrap().len(), 300_000);
    }

    /// Recorre y modifica todo lo que se pueda de un volumen dañado: lo único que se exige es que no
    /// haya pánico ni bucle sin fin (un pendrive es entrada no fiable).
    fn recorrer(v: &mut dyn Volume, now: DateTime) {
        let _ = (v.label(), v.space());
        let mut pend = alloc::vec![String::new()];
        let mut vistos = 0;
        while let Some(d) = pend.pop() {
            let Ok(es) = v.list(&d) else { continue };
            for e in es.into_iter().take(64) {
                vistos += 1;
                if vistos > 400 {
                    return;
                }
                let ruta = if d.is_empty() { e.name.clone() } else { alloc::format!("{d}\\{}", e.name) };
                let _ = v.stat(&ruta);
                if e.dir {
                    if d.matches('\\').count() < 6 {
                        pend.push(ruta);
                    }
                } else {
                    let _ = v.read(&ruta);
                }
            }
        }
        let _ = v.write("nuevo.txt", b"hola", now);
        let _ = v.mkdir("Dir nueva", now);
        let _ = v.write("Dir nueva\\dentro con nombre largo.txt", &[7; 5000], now);
        let _ = v.rename("nuevo.txt", "renombrado.txt");
        let _ = v.delete("Carpeta\\a.txt");
        let _ = v.delete("Carpeta");
        let _ = v.flush();
    }

    /// Disco en memoria que sigue siendo accesible después de dárselo a un volumen.
    struct Comp(alloc::rc::Rc<core::cell::RefCell<MemSectors>>);
    impl SectorDev for Comp {
        fn sector_size(&self) -> usize {
            512
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
    }

    /// Formatear dentro de una ventana deja intacto todo el disco fuera de ella (tabla y otras particiones).
    #[test]
    fn formatear_en_ventana_no_toca_lo_demas() {
        let now = DateTime { year: 2026, month: 11, day: 20, hour: 2, minute: 0, second: 0 };
        let (ini, len) = (4_000u64, 143_360u64);
        for k in 0..3 {
            let mut m = MemSectors::new(160_000, 512);
            m.data.fill(0xA5);
            let disco = alloc::rc::Rc::new(core::cell::RefCell::new(m));
            let antes = disco.borrow().data.clone();
            let mut w = Window::new(Box::new(Comp(disco.clone())), ini, len);
            let kind = match k {
                0 => {
                    format_in(&mut w, FsKind::Fat32, "FAT", now).unwrap();
                    FsKind::Fat32
                }
                1 => {
                    format_in(&mut w, FsKind::ExFat, "EXF", now).unwrap();
                    FsKind::ExFat
                }
                _ => {
                    nfvol::format_in(Box::new(w), "NF", None, now).unwrap();
                    FsKind::NightFury
                }
            };
            let d = disco.borrow();
            let (a, b) = (ini as usize * 512, (ini + len) as usize * 512);
            assert!(d.data[..a] == antes[..a] && d.data[b..] == antes[b..], "{kind:?}: escribió fuera de la ventana");
            assert!(d.data[a..b] != antes[a..b]);
            drop(d);
            // Se monta como partición y funciona.
            let p = Partition { start: ini, sectors: len, kind };
            assert_eq!(partitions(&mut Window::new(Box::new(Comp(disco.clone())), ini, len)).unwrap()[0].kind, kind);
            let dev: Box<dyn SectorDev> = Box::new(Comp(disco.clone()));
            let mut v = match kind {
                FsKind::NightFury => match nfvol::open(dev, &p).unwrap() {
                    nfvol::Opened::Ready(v) => v,
                    _ => panic!("cifrado"),
                },
                _ => mount(dev, &p).unwrap(),
            };
            v.write("a.txt", b"hola", now).unwrap();
            assert_eq!(v.read("a.txt").unwrap(), b"hola");
        }
        // La ventana rechaza lo que se sale.
        let mut w = Window::new(Box::new(MemSectors::new(100, 512)), 10, 20);
        assert_eq!(w.sectors(), 20);
        assert_eq!(w.write(19, &[0; 1024]), Err(FsError::Disk));
        assert_eq!(w.read(20, &mut [0; 512]), Err(FsError::Disk));
    }

    #[test]
    fn imagenes_danadas_no_hacen_caer() {
        let now = DateTime { year: 2026, month: 11, day: 19, hour: 2, minute: 0, second: 0 };
        let mut x = 0x9E37_79B9u32;
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        // (formato, MiB, vueltas): FAT12/16 en discos pequeños, FAT32 desde 64 MiB y exFAT.
        for (kind, mib, vueltas) in [(FsKind::Fat32, 4u64, 300), (FsKind::Fat32, 16, 200), (FsKind::Fat32, 64, 60), (FsKind::ExFat, 8, 300)] {
            let disco = alloc::rc::Rc::new(core::cell::RefCell::new(MemSectors::new(mib << 11, 512)));
            format(&mut Comp(disco.clone()), kind, "PRUEBA", now).unwrap();
            let p = partitions(&mut Comp(disco.clone())).unwrap();
            {
                let mut v = mount(Box::new(Comp(disco.clone())), &p[0]).unwrap();
                v.mkdir("Carpeta", now).unwrap();
                v.mkdir("Carpeta\\Sub", now).unwrap();
                for i in 0..20 {
                    v.write(&alloc::format!("Carpeta\\archivo con nombre largo {i}.txt"), &alloc::vec![i as u8; 700 * i], now).unwrap();
                }
                v.write("Carpeta\\a.txt", b"a", now).unwrap();
                v.write("Carpeta\\Sub\\b.bin", &[1; 40_000], now).unwrap();
                v.flush().unwrap();
            }
            let base = disco.borrow().data.clone();
            // Los metadatos (arranque, FAT/bitmap, directorios) están al principio del volumen.
            let zona = base.len().min(1 << 20);
            let mut montados = 0;
            for n in 0..vueltas {
                let mut d = MemSectors { data: base.clone(), sector: 512, ro: false };
                for _ in 0..1 + n % 24 {
                    let i = rnd() as usize % zona;
                    d.data[i] = rnd() as u8;
                }
                let Ok(ps) = partitions(&mut d) else { continue };
                for p in ps.iter().take(4) {
                    let dev = MemSectors { data: d.data.clone(), sector: 512, ro: false };
                    if let Ok(mut v) = mount(Box::new(dev), p) {
                        montados += 1;
                        recorrer(&mut *v, now);
                    }
                }
            }
            // La mayoría de los daños caen en datos: si casi nada monta, la prueba no prueba nada.
            assert!(montados > vueltas / 3, "{kind:?} {mib} MiB: solo {montados} de {vueltas}");
        }
    }
}
