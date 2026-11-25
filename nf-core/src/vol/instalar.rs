// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Pendrive de arranque, modo portátil e instalación en disco: lógica pura sobre `SectorDev`.
//!
//! Todas las funciones reciben `abrir`, una fábrica de manejadores del mismo disco (los volúmenes
//! consumen su dispositivo, igual que `nfvol::format`). La fuente de azar (`aleatorio`) la pone el
//! llamador: aquí no hay generador. Cualquier error deja el disco con la tabla anterior válida,
//! salvo `instalar`, que empieza por la tabla nueva (un corte deja particiones sin formato).
use super::part::gpt::{self, Entrada, Particion, TIPO_ESP, TIPO_NFFS};
use super::{nfvol, FsKind, SectorDev, Window};
use crate::fs::FsError;
use crate::keys::KeyError;
use crate::platform::DateTime;
use alloc::boxed::Box;
use alloc::format;
use alloc::vec;
use alloc::vec::Vec;

/// Fábrica de manejadores del disco.
pub type Abrir<'a> = &'a mut dyn FnMut() -> Box<dyn SectorDev>;

const MIB: u64 = 1 << 20;
/// Tamaño de la ESP del pendrive de arranque: deja el resto del pendrive para el modo portátil.
pub const ESP_PENDRIVE: u64 = 64 * MIB;
/// Tamaño de la ESP de una instalación.
pub const ESP_INSTALAR: u64 = 256 * MIB;
/// Mínimo útil para la partición NFFS (el NFFS pide 64 bloques; menos de esto no merece la pena).
const MIN_NFFS: u64 = 16 * MIB;
const MARCA: &str = "EFI\\NightFury\\pendrive.txt";
const ARRANQUE: &str = "EFI\\BOOT\\BOOTX64.EFI";

/// La ESP recién creada por `instalar` (el núcleo la necesita para la entrada de arranque UEFI).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Esp {
    pub unica: [u8; 16],
    pub inicio_lba: u64,
    pub sectores: u64,
}

fn fs_de(e: KeyError) -> FsError {
    match e {
        KeyError::Fs(f) => f,
        _ => FsError::Invalid,
    }
}

/// Formatea FAT32 en `[inicio, inicio+sectores)` y escribe ahí las carpetas y archivos.
fn llenar_esp(abrir: Abrir, inicio: u64, sectores: u64, ficheros: &[(&str, &[u8])], now: DateTime) -> Result<(), FsError> {
    super::format_in(&mut Window::new(abrir(), inicio, sectores), FsKind::Fat32, "NIGHTFURY", now)?;
    let mut d = abrir();
    let p = gpt::como_particion(&mut *d, inicio, sectores);
    let mut v = super::mount(d, &p)?;
    for (ruta, datos) in ficheros {
        // Crea las carpetas intermedias que falten.
        for (i, c) in ruta.char_indices().filter(|&(_, c)| c == '\\') {
            let _ = c;
            if v.stat(&ruta[..i]).is_err() {
                v.mkdir(&ruta[..i], now)?;
            }
        }
        v.write(ruta, datos, now)?;
    }
    v.flush()
}

/// Pendrive de arranque: GPT con una ESP FAT32 de `esp_bytes` (o hasta el final si no cabe) desde
/// 1 MiB, `\EFI\BOOT\BOOTX64.EFI` = `efi` y el marcador `\EFI\NightFury\pendrive.txt`.
pub fn crear_pendrive(abrir: Abrir, efi: &[u8], version: &str, esp_bytes: u64, now: DateTime, aleatorio: &mut dyn FnMut() -> u64) -> Result<(), FsError> {
    let mut d = abrir();
    let ss = d.sector_size();
    let inicio = gpt::alinear_mib(gpt::primer_lba_usable(ss), ss);
    let fin = (inicio + esp_bytes / ss as u64 - 1).min(gpt::ultimo_lba_usable(d.sectors(), ss));
    if fin < inicio || (fin - inicio + 1) * (ss as u64) < 8 * MIB {
        return Err(FsError::NoSpace);
    }
    let (disco, unica) = (gpt::guid_nuevo(aleatorio), gpt::guid_nuevo(aleatorio));
    gpt::crear_gpt(&mut *d, &[Particion { tipo: TIPO_ESP, unica, nombre: "NightFury ESP", inicio_lba: inicio, fin_lba: fin }], disco)?;
    drop(d);
    let marca = format!("NightFury pendrive {version}\r\n");
    llenar_esp(abrir, inicio, fin - inicio + 1, &[(ARRANQUE, efi), (MARCA, marca.as_bytes())], now)
}

/// Manejador que solo lee: montar una FAT en escritura la marca como sucia, y mirar si es un pendrive no debe tocarlo.
struct Lectura(Box<dyn SectorDev>);

impl SectorDev for Lectura {
    fn sector_size(&self) -> usize {
        self.0.sector_size()
    }
    fn sectors(&self) -> u64 {
        self.0.sectors()
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        self.0.read(lba, buf)
    }
    fn write(&mut self, _: u64, _: &[u8]) -> Result<(), FsError> {
        Err(FsError::Denied)
    }
    fn read_only(&self) -> bool {
        true
    }
}

/// ¿GPT válida cuya primera partición es una ESP FAT con el marcador de pendrive NightFury? No escribe nada.
pub fn es_pendrive(abrir: Abrir) -> bool {
    let mut d = abrir();
    let Some((_, ent)) = gpt::leer_gpt(&mut *d) else { return false };
    let Some(e) = ent.first().filter(|e| e.tipo == TIPO_ESP) else { return false };
    let p = gpt::como_particion(&mut *d, e.inicio_lba, e.sectores());
    if !matches!(p.kind, FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32) {
        return false;
    }
    drop(d);
    super::mount(Box::new(Lectura(abrir())), &p).and_then(|mut v| v.read(MARCA)).is_ok_and(|b| b.starts_with(b"NightFury pendrive"))
}

/// La partición NFFS del modo portátil, si el disco ya la tiene: (primer, último sector).
pub fn portatil(abrir: Abrir) -> Option<(u64, u64)> {
    let (_, ent) = gpt::leer_gpt(&mut *abrir())?;
    ent.iter().find(|e: &&Entrada| e.tipo == TIPO_NFFS).map(|e| (e.inicio_lba, e.fin_lba))
}

/// Formatea NFFS en `[inicio, fin]`.
fn formatear_nffs(abrir: Abrir, inicio: u64, fin: u64, now: DateTime) -> Result<(), FsError> {
    nfvol::format_in(Box::new(Window::new(abrir(), inicio, fin - inicio + 1)), "NightFury", None, now).map(|_| ()).map_err(fs_de)
}

/// Modo portátil: en un pendrive NightFury añade la partición 2 (NFFS) en el hueco tras la ESP y la
/// formatea. Mueve la GPT de respaldo al final real del disco (la imagen pudo grabarse en un pendrive
/// mayor). `NoSpace` si no queda hueco de al menos 16 MiB; `Exists` si ya la tiene.
pub fn crear_portatil(abrir: Abrir, now: DateTime, aleatorio: &mut dyn FnMut() -> u64) -> Result<(u64, u64), FsError> {
    if !es_pendrive(abrir) {
        return Err(FsError::Invalid);
    }
    if portatil(abrir).is_some() {
        return Err(FsError::Exists);
    }
    let mut d = abrir();
    let ss = d.sector_size() as u64;
    let (inicio, fin) = gpt::hueco_final(&mut *d).filter(|&(i, f)| (f - i + 1) * ss >= MIN_NFFS).ok_or(FsError::NoSpace)?;
    drop(d);
    // Primero el formato, luego la tabla: si algo falla, el pendrive queda como estaba.
    formatear_nffs(abrir, inicio, fin, now)?;
    let nueva = Particion { tipo: TIPO_NFFS, unica: gpt::guid_nuevo(aleatorio), nombre: "NightFury", inicio_lba: inicio, fin_lba: fin };
    gpt::anadir_particion(&mut *abrir(), &nueva)?;
    Ok((inicio, fin))
}

/// Tamaño máximo que se lee del `BOOTX64.EFI` del pendrive (el de verdad pesa ~10 MiB).
const MAX_EFI: u64 = 64 * MIB;

/// `\EFI\BOOT\BOOTX64.EFI` de la ESP (primera partición) del pendrive de arranque. No escribe nada.
pub fn leer_efi(abrir: Abrir) -> Result<Vec<u8>, FsError> {
    let mut d = abrir();
    let (_, ent) = gpt::leer_gpt(&mut *d).ok_or(FsError::Corrupt)?;
    let e = ent.first().filter(|e| e.tipo == TIPO_ESP).ok_or(FsError::NotFound)?;
    let p = gpt::como_particion(&mut *d, e.inicio_lba, e.sectores());
    drop(d);
    let mut v = super::mount(Box::new(Lectura(abrir())), &p)?;
    if v.stat(ARRANQUE)?.size > MAX_EFI {
        return Err(FsError::Invalid);
    }
    v.read(ARRANQUE)
}

/// Deja a cero los 2 MiB del principio y del final de la partición (con margen sobre los 256 bloques de
/// 4 KiB que mira `Nffs::is_blank`): así el primer arranque la ve como disco nuevo y ofrece el asistente
/// (cifrado y cuentas). shortcut: el resto conserva lo que hubiera; NFFS solo lee lo que ha escrito él.
fn poner_en_blanco(abrir: Abrir, inicio: u64, sectores: u64) -> Result<(), FsError> {
    let mut w = Window::new(abrir(), inicio, sectores);
    let m = MIB / w.sector_size() as u64;
    let cero = vec![0u8; MIB as usize];
    for base in [0, sectores - 2 * m] {
        for k in 0..2 {
            w.write(base + k * m, &cero)?;
        }
    }
    w.flush()
}

/// Instalación en disco entero: GPT nueva con ESP de 256 MiB (`BOOTX64.EFI` y `\EFI\NightFury\nightfury.efi`,
/// ambos = `efi`, y `version.txt`) y una partición NFFS con el resto, EN BLANCO (el primer arranque la formatea
/// con el asistente de disco nuevo, que deja elegir el cifrado).
pub fn instalar(abrir: Abrir, efi: &[u8], version: &str, now: DateTime, aleatorio: &mut dyn FnMut() -> u64) -> Result<Esp, FsError> {
    let mut d = abrir();
    let ss = d.sector_size();
    let ini = gpt::alinear_mib(gpt::primer_lba_usable(ss), ss);
    let esp_fin = ini + ESP_INSTALAR / ss as u64 - 1;
    let n_ini = gpt::alinear_mib(esp_fin + 1, ss);
    let n_fin = gpt::ultimo_lba_usable(d.sectors(), ss);
    if n_ini > n_fin || (n_fin - n_ini + 1) * (ss as u64) < MIN_NFFS {
        return Err(FsError::NoSpace);
    }
    let esp = Esp { unica: gpt::guid_nuevo(aleatorio), inicio_lba: ini, sectores: esp_fin - ini + 1 };
    let datos = Particion { tipo: TIPO_NFFS, unica: gpt::guid_nuevo(aleatorio), nombre: "NightFury", inicio_lba: n_ini, fin_lba: n_fin };
    let sistema = Particion { tipo: TIPO_ESP, unica: esp.unica, nombre: "NightFury ESP", inicio_lba: ini, fin_lba: esp_fin };
    gpt::crear_gpt(&mut *d, &[sistema, datos], gpt::guid_nuevo(aleatorio))?;
    drop(d);
    let ver = format!("NightFury {version}\r\n");
    llenar_esp(abrir, ini, esp.sectores, &[(ARRANQUE, efi), ("EFI\\NightFury\\nightfury.efi", efi), ("EFI\\NightFury\\version.txt", ver.as_bytes())], now)?;
    poner_en_blanco(abrir, n_ini, n_fin - n_ini + 1)?;
    abrir().flush()?;
    Ok(esp)
}

/// Valor de una variable `Boot####` (EFI_LOAD_OPTION, UEFI 2.x 3.1.3) que arranca `ruta` de la ESP recién
/// creada: activa, descripción «NightFury», y la ruta = nodo Hard Drive (partición 1 de la GPT, firma = la
/// GUID de la ESP) + nodo File Path + nodo fin.
pub fn opcion_arranque(esp: &Esp, ruta: &str) -> Vec<u8> {
    let ruta: Vec<u8> = ruta.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
    let mut lista = Vec::new();
    lista.extend([4, 1, 42, 0]);
    lista.extend(1u32.to_le_bytes());
    lista.extend(esp.inicio_lba.to_le_bytes());
    lista.extend(esp.sectores.to_le_bytes());
    lista.extend(esp.unica);
    lista.extend([2, 2]); // formato GPT, firma GUID
    lista.extend([4, 4]);
    lista.extend(((4 + ruta.len()) as u16).to_le_bytes());
    lista.extend(ruta);
    lista.extend([0x7F, 0xFF, 4, 0]);
    let mut o = Vec::new();
    o.extend(1u32.to_le_bytes()); // LOAD_OPTION_ACTIVE
    o.extend((lista.len() as u16).to_le_bytes());
    o.extend("NightFury".encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
    o.extend(lista);
    o
}

/// Nuevo valor de `BootOrder`: `n` el primero y los demás como estaban (sin repetirlo).
pub fn orden_con(actual: &[u8], n: u16) -> Vec<u8> {
    let mut o = n.to_le_bytes().to_vec();
    o.extend(actual.chunks_exact(2).filter(|c| *c != n.to_le_bytes()).flatten());
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vol::Partition;
    use alloc::vec::Vec;
    use crate::vol::{nfvol::Opened, partitions, MemSectors};
    use alloc::rc::Rc;
    use core::cell::RefCell;

    const NOW: DateTime = DateTime { year: 2026, month: 11, day: 25, hour: 12, minute: 0, second: 0 };

    /// Disco en memoria compartido: cada manejador ve lo que escriben los demás.
    #[derive(Clone)]
    struct Disco(Rc<RefCell<MemSectors>>);
    impl Disco {
        fn nuevo(mib: u64, ss: usize) -> Disco {
            Disco(Rc::new(RefCell::new(MemSectors::new(mib * MIB / ss as u64, ss))))
        }
        fn abrir(&self) -> impl FnMut() -> Box<dyn SectorDev> + '_ {
            || Box::new(self.clone())
        }
    }
    impl SectorDev for Disco {
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
    }

    fn azar() -> impl FnMut() -> u64 {
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        }
    }

    fn efi() -> Vec<u8> {
        (0..300_000usize).map(|i| (i.wrapping_mul(31) ^ (i >> 8)) as u8).collect()
    }

    fn leer(d: &Disco, inicio: u64, sectores: u64, ruta: &str) -> Vec<u8> {
        let p = gpt::como_particion(&mut d.clone(), inicio, sectores);
        crate::vol::mount(Box::new(d.clone()), &p).unwrap().read(ruta).unwrap()
    }

    fn nffs_monta(d: &Disco, i: u64, f: u64) {
        let p = Partition { start: i, sectors: f - i + 1, kind: FsKind::NightFury };
        assert_eq!(partitions(&mut d.clone()).unwrap().iter().filter(|q| q.start == i && q.kind == FsKind::NightFury).count(), 1);
        let Opened::Ready(mut v) = nfvol::open(Box::new(d.clone()), &p).unwrap() else { panic!("cifrado") };
        v.write("hola.txt", b"hola", NOW).unwrap();
        assert_eq!(v.read("hola.txt").unwrap(), b"hola");
    }

    #[test]
    fn pendrive_de_arranque_y_modo_portatil() {
        for ss in [512usize, 4096] {
            let pen = Disco::nuevo(66, ss);
            crear_pendrive(&mut pen.abrir(), &efi(), "0.12.6", ESP_PENDRIVE, NOW, &mut azar()).unwrap();
            assert!(es_pendrive(&mut pen.abrir()), "ss={ss}");
            assert_eq!(portatil(&mut pen.abrir()), None);
            let antes = pen.0.borrow().data.clone();
            assert!(es_pendrive(&mut pen.abrir()));
            assert!(pen.0.borrow().data == antes, "es_pendrive no escribe (ni marca la FAT como sucia)");
            assert_eq!(leer_efi(&mut pen.abrir()).unwrap(), efi());
            let (_, e) = gpt::leer_gpt(&mut pen.clone()).unwrap();
            assert_eq!(e.len(), 1);
            assert_eq!(leer(&pen, e[0].inicio_lba, e[0].sectores(), ARRANQUE), efi());
            assert!(leer(&pen, e[0].inicio_lba, e[0].sectores(), MARCA).starts_with(b"NightFury pendrive 0.12.6"));
            // En la imagen sin hueco no hay sitio para el modo portátil.
            assert_eq!(crear_portatil(&mut pen.abrir(), NOW, &mut azar()), Err(FsError::NoSpace));

            // La misma imagen grabada en un pendrive mayor (la GPT de respaldo queda mal puesta).
            let grande = Disco::nuevo(256, ss);
            grande.0.borrow_mut().data[..pen.0.borrow().data.len()].copy_from_slice(&pen.0.borrow().data);
            let (i, f) = crear_portatil(&mut grande.abrir(), NOW, &mut azar()).unwrap();
            assert_eq!(portatil(&mut grande.abrir()), Some((i, f)));
            assert_eq!(f, gpt::ultimo_lba_usable(grande.0.borrow().sectors(), ss));
            assert!(es_pendrive(&mut grande.abrir()));
            assert_eq!(crear_portatil(&mut grande.abrir(), NOW, &mut azar()), Err(FsError::Exists));
            // Cabecera de respaldo al final real; la del tamaño viejo ya no existe.
            let n = grande.0.borrow().sectors();
            assert_eq!(&grande.0.borrow().data[(n as usize - 1) * ss..][..8], b"EFI PART");
            let viejo = (pen.0.borrow().sectors() as usize - 1) * ss;
            assert_ne!(&grande.0.borrow().data[viejo..][..8], b"EFI PART");
            let (_, e) = gpt::leer_gpt(&mut grande.clone()).unwrap();
            assert_eq!(e.len(), 2);
            assert_eq!(partitions(&mut grande.clone()).unwrap().len(), 1, "la ESP no se lista como unidad de datos");
            nffs_monta(&grande, i, f);
            // La ESP sigue entera tras tocar la tabla.
            assert_eq!(leer(&grande, e[0].inicio_lba, e[0].sectores(), ARRANQUE), efi());
        }
    }

    #[test]
    fn instalar_en_disco_con_tabla_previa() {
        for ss in [512usize, 4096] {
            let d = Disco::nuevo(1024, ss);
            // Una partición NTFS falsa que ocupa todo.
            let ini = gpt::alinear_mib(gpt::primer_lba_usable(ss), ss);
            let ultimo = gpt::ultimo_lba_usable(d.0.borrow().sectors(), ss);
            let basic = [0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7];
            gpt::crear_gpt(&mut d.clone(), &[Particion { tipo: basic, unica: [7; 16], nombre: "Windows", inicio_lba: ini, fin_lba: ultimo }], [9; 16]).unwrap();
            d.0.borrow_mut().data[ini as usize * ss + 3..][..8].copy_from_slice(b"NTFS    ");
            d.0.borrow_mut().data[ini as usize * ss + 510..][..2].copy_from_slice(&[0x55, 0xAA]);
            assert_eq!(partitions(&mut d.clone()).unwrap()[0].kind, FsKind::Ntfs);

            let esp = instalar(&mut d.abrir(), &efi(), "0.12.6", NOW, &mut azar()).unwrap();
            let (_, e) = gpt::leer_gpt(&mut d.clone()).unwrap();
            assert_eq!(e.len(), 2, "solo ESP + NFFS");
            assert_eq!((e[0].tipo, e[0].unica, e[0].inicio_lba, e[0].sectores()), (TIPO_ESP, esp.unica, esp.inicio_lba, esp.sectores));
            assert_eq!(e[0].sectores() * ss as u64, ESP_INSTALAR);
            assert_eq!(e[1].tipo, TIPO_NFFS);
            assert_ne!(e[0].unica, e[1].unica);
            for ruta in [ARRANQUE, "EFI\\NightFury\\nightfury.efi"] {
                assert_eq!(leer(&d, e[0].inicio_lba, e[0].sectores(), ruta), efi(), "{ruta}");
            }
            // La NFFS queda en blanco: el primer arranque la ve como disco nuevo y la formatea el asistente.
            let dev = || Box::new(nfvol::SectorBlocks { dev: Box::new(d.clone()), start: e[1].inicio_lba, sectors: e[1].sectores() });
            assert!(crate::fs::Nffs::is_blank(&mut *dev()));
            let mut f = crate::files::Files::probe(Some(dev()), NOW);
            assert!(f.pending_format());
            f.format_pending(NOW).unwrap();
            assert!(f.persistent());
            // Un disco demasiado pequeño no se toca.
            let chico = Disco::nuevo(260, ss);
            assert_eq!(instalar(&mut chico.abrir(), &efi(), "x", NOW, &mut azar()), Err(FsError::NoSpace));
            assert!(chico.0.borrow().data.iter().all(|&b| b == 0));
        }
    }

    #[test]
    fn entrada_de_arranque_uefi() {
        let esp = Esp { unica: [0xAB; 16], inicio_lba: 2048, sectores: 524_288 };
        let o = opcion_arranque(&esp, "\\EFI\\NightFury\\nightfury.efi");
        assert_eq!(&o[..4], &1u32.to_le_bytes());
        let lista_len = u16::from_le_bytes([o[4], o[5]]) as usize;
        let desc = 6 + 2 * ("NightFury".len() + 1);
        assert_eq!(&o[6..8], &[b'N', 0]);
        assert_eq!(&o[desc - 2..desc], &[0, 0]);
        assert_eq!(o.len(), desc + lista_len);
        let hd = &o[desc..];
        assert_eq!(&hd[..4], &[4, 1, 42, 0]);
        assert_eq!(&hd[4..8], &1u32.to_le_bytes());
        assert_eq!(&hd[8..16], &2048u64.to_le_bytes());
        assert_eq!(&hd[16..24], &524_288u64.to_le_bytes());
        assert_eq!(&hd[24..40], &[0xAB; 16]);
        assert_eq!(&hd[40..42], &[2, 2]);
        let fp = &hd[42..];
        let n = u16::from_le_bytes([fp[2], fp[3]]) as usize;
        assert_eq!(&fp[..2], &[4, 4]);
        assert_eq!(n, 4 + 2 * ("\\EFI\\NightFury\\nightfury.efi".len() + 1));
        assert_eq!(&fp[n..], &[0x7F, 0xFF, 4, 0]);
        assert_eq!(&fp[n - 2..n], &[0, 0]);
    }

    #[test]
    fn orden_de_arranque() {
        assert_eq!(orden_con(&[], 5), [5, 0]);
        assert_eq!(orden_con(&[3, 0, 5, 0, 1, 0], 5), [5, 0, 3, 0, 1, 0]);
        assert_eq!(orden_con(&[3, 0, 7], 1), [1, 0, 3, 0]);
    }

    #[test]
    fn crear_portatil_exige_pendrive() {
        let d = Disco::nuevo(64, 512);
        assert!(!es_pendrive(&mut d.abrir()));
        assert_eq!(crear_portatil(&mut d.abrir(), NOW, &mut azar()), Err(FsError::Invalid));
    }
}
