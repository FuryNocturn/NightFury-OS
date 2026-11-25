// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Pendrive de arranque, modo portátil e instalación en disco: lógica pura sobre `SectorDev`.
//!
//! Todas las funciones reciben `abrir`, una fábrica de manejadores del mismo disco (los volúmenes
//! consumen su dispositivo, igual que `nfvol::format`). La fuente de azar (`aleatorio`) la pone el
//! llamador: aquí no hay generador. Cualquier error deja el disco con la tabla anterior válida,
//! salvo `instalar`, que empieza por la tabla nueva (un corte deja particiones sin formato).
use super::part::gpt::{self, Entrada, Particion, TIPO_ESP, TIPO_NFFS};
use super::part::guid;
use super::{nfvol, FsKind, Partition, SectorDev, Volume, Window};
use crate::fs::FsError;
use crate::keys::KeyError;
use crate::platform::DateTime;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
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
/// Carpeta de NightFury en la ESP (única cosa que se escribe en una ESP ajena) y sus archivos.
const NF_DIR: &str = "EFI\\NightFury";
pub const NF_ARRANQUE: &str = "EFI\\NightFury\\arranque.efi";
pub const NF_NUCLEO: &str = "EFI\\NightFury\\nightfury.efi";
const NF_VERSION: &str = "EFI\\NightFury\\version.txt";
/// GUID única (en texto) de la partición NFFS de esta instalación.
const NF_PARTICION: &str = "EFI\\NightFury\\particion";
const NOMBRE_ESP: &str = "NightFury ESP";
/// Hueco o partición mínimos para instalar junto a otro sistema.
const MIN_SITIO: u64 = 2048 * MIB;
/// Sitio libre que se exige a una ESP ajena antes de ver los archivos (`instalar_junto` mira el tamaño real).
const ESP_MINIMA: u64 = 24 * MIB;
const ESP_MARGEN: u64 = MIB;

/// La ESP de la instalación (el núcleo la necesita para la entrada de arranque UEFI).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Esp {
    pub unica: [u8; 16],
    pub inicio_lba: u64,
    pub sectores: u64,
    /// Número de partición en la GPT (desde 1), el que lleva el nodo Hard Drive de la entrada UEFI.
    pub numero: u32,
}

fn esp_de(e: &Entrada) -> Esp {
    Esp { unica: e.unica, inicio_lba: e.inicio_lba, sectores: e.sectores(), numero: e.numero as u32 }
}

/// GUID en texto (8-4-4-4-12, mayúsculas) como la escribe `particion`.
fn guid_texto(g: &[u8; 16]) -> String {
    let mut s = format!("{:08X}-{:04X}-{:04X}-", u32::from_le_bytes([g[0], g[1], g[2], g[3]]), u16::from_le_bytes([g[4], g[5]]), u16::from_le_bytes([g[6], g[7]]));
    for (i, b) in g[8..].iter().enumerate() {
        if i == 2 {
            s.push('-');
        }
        s += &format!("{b:02X}");
    }
    s
}

fn fs_de(e: KeyError) -> FsError {
    match e {
        KeyError::Fs(f) => f,
        _ => FsError::Invalid,
    }
}

/// Formatea FAT32 en `[inicio, inicio+sectores)` y escribe ahí las carpetas y archivos.
pub(super) fn llenar_esp(abrir: Abrir, inicio: u64, sectores: u64, ficheros: &[(&str, &[u8])], now: DateTime) -> Result<(), FsError> {
    super::format_in(&mut Window::new(abrir(), inicio, sectores), FsKind::Fat32, "NIGHTFURY", now)?;
    escribir_en_esp(abrir, inicio, sectores, ficheros, now)
}

/// Monta la partición `[inicio, inicio+sectores)`; en solo lectura si `escritura` es falso.
fn montar(abrir: Abrir, inicio: u64, sectores: u64, escritura: bool) -> Result<Box<dyn Volume>, FsError> {
    let p: Partition = gpt::como_particion(&mut *abrir(), inicio, sectores);
    let d = abrir();
    super::mount(if escritura { d } else { Box::new(Lectura(d)) }, &p)
}

/// Escribe archivos en una ESP que ya tiene sistema de archivos (crea las carpetas que falten, sobrescribe).
pub(super) fn escribir_en_esp(abrir: Abrir, inicio: u64, sectores: u64, ficheros: &[(&str, &[u8])], now: DateTime) -> Result<(), FsError> {
    let mut v = montar(abrir, inicio, sectores, true)?;
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
/// 1 MiB, `\EFI\BOOT\BOOTX64.EFI` = `efi`, el marcador `\EFI\NightFury\pendrive.txt` y, si no va vacío, el
/// menú de arranque `\EFI\NightFury\arranque.efi` (lo que el instalador copia al disco).
pub fn crear_pendrive(abrir: Abrir, efi: &[u8], arranque: &[u8], version: &str, esp_bytes: u64, now: DateTime, aleatorio: &mut dyn FnMut() -> u64) -> Result<(), FsError> {
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
    let mut f: Vec<(&str, &[u8])> = vec![(ARRANQUE, efi), (MARCA, marca.as_bytes())];
    if !arranque.is_empty() {
        f.push((NF_ARRANQUE, arranque));
    }
    llenar_esp(abrir, inicio, fin - inicio + 1, &f, now)
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
    leer_de_pen(abrir, ARRANQUE)
}

/// El menú de arranque `\EFI\NightFury\arranque.efi` del pendrive (`NotFound` en pendrives de la 0.12.6).
pub fn leer_arranque(abrir: Abrir) -> Result<Vec<u8>, FsError> {
    leer_de_pen(abrir, NF_ARRANQUE)
}

fn leer_de_pen(abrir: Abrir, ruta: &str) -> Result<Vec<u8>, FsError> {
    let mut d = abrir();
    let (_, ent) = gpt::leer_gpt(&mut *d).ok_or(FsError::Corrupt)?;
    let e = ent.first().filter(|e| e.tipo == TIPO_ESP).ok_or(FsError::NotFound)?;
    let p = gpt::como_particion(&mut *d, e.inicio_lba, e.sectores());
    drop(d);
    let mut v = super::mount(Box::new(Lectura(abrir())), &p)?;
    if v.stat(ruta)?.size > MAX_EFI {
        return Err(FsError::Invalid);
    }
    v.read(ruta)
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

/// Archivos de NightFury en la ESP, sin `particion` (que se escribe el último).
fn archivos_nf<'a>(nucleo: &'a [u8], arranque: &'a [u8], ver: &'a [u8]) -> [(&'static str, &'a [u8]); 3] {
    [(NF_ARRANQUE, arranque), (NF_NUCLEO, nucleo), (NF_VERSION, ver)]
}

/// Instalación en disco entero: GPT nueva con ESP de 256 MiB y una partición NFFS con el resto, EN BLANCO
/// (el primer arranque la formatea con el asistente de disco nuevo, que deja elegir el cifrado).
/// En la ESP: `\EFI\BOOT\BOOTX64.EFI` y `\EFI\NightFury\arranque.efi` = `arranque`, `\EFI\NightFury\nightfury.efi`
/// = `nucleo`, `version.txt` y `particion` (GUID única de la NFFS). Si `arranque` va vacío se usa `nucleo`.
pub fn instalar(abrir: Abrir, nucleo: &[u8], arranque: &[u8], version: &str, now: DateTime, aleatorio: &mut dyn FnMut() -> u64) -> Result<Esp, FsError> {
    let arranque = if arranque.is_empty() { nucleo } else { arranque };
    let mut d = abrir();
    let ss = d.sector_size();
    let ini = gpt::alinear_mib(gpt::primer_lba_usable(ss), ss);
    let esp_fin = ini + ESP_INSTALAR / ss as u64 - 1;
    let n_ini = gpt::alinear_mib(esp_fin + 1, ss);
    let n_fin = gpt::ultimo_lba_usable(d.sectors(), ss);
    if n_ini > n_fin || (n_fin - n_ini + 1) * (ss as u64) < MIN_NFFS {
        return Err(FsError::NoSpace);
    }
    let esp = Esp { unica: gpt::guid_nuevo(aleatorio), inicio_lba: ini, sectores: esp_fin - ini + 1, numero: 1 };
    let unica_nffs = gpt::guid_nuevo(aleatorio);
    let datos = Particion { tipo: TIPO_NFFS, unica: unica_nffs, nombre: "NightFury", inicio_lba: n_ini, fin_lba: n_fin };
    let sistema = Particion { tipo: TIPO_ESP, unica: esp.unica, nombre: NOMBRE_ESP, inicio_lba: ini, fin_lba: esp_fin };
    gpt::crear_gpt(&mut *d, &[sistema, datos], gpt::guid_nuevo(aleatorio))?;
    drop(d);
    let ver = format!("NightFury {version}\r\n");
    let particion = guid_texto(&unica_nffs);
    let mut f = archivos_nf(nucleo, arranque, ver.as_bytes()).to_vec();
    f.extend([(ARRANQUE, arranque), (NF_PARTICION, particion.as_bytes())]);
    llenar_esp(abrir, ini, esp.sectores, &f, now)?;
    poner_en_blanco(abrir, n_ini, n_fin - n_ini + 1)?;
    abrir().flush()?;
    Ok(esp)
}

/// Dónde se puede instalar NightFury en un disco que ya tiene otras cosas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lugar {
    /// Espacio sin asignar (la partición NFFS nueva se alinea a 1 MiB).
    Hueco,
    /// Partición de datos que se borra: `numero` es su puesto en la GPT, `fs` lo que tiene dentro.
    Particion { numero: usize, fs: FsKind, etiqueta: String },
}

/// Un sitio posible (hueco o partición) con su rango de sectores. Si `bloqueo` es `Some`, se muestra
/// pero no se puede elegir: es el motivo para enseñar al usuario.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sitio {
    pub inicio_lba: u64,
    pub sectores: u64,
    pub lugar: Lugar,
    pub bloqueo: Option<&'static str>,
}

/// Resultado de `analizar`. `esp` es la ESP del disco, si la hay; `nightfury`, si ya hay una instalación.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Analisis {
    pub bloqueo_disco: Option<&'static str>,
    pub esp: Option<Esp>,
    pub sitios: Vec<Sitio>,
    pub nightfury: bool,
}

pub(super) const GUID_MSR: &str = "E3C9E316-0B5C-4DB8-817D-F92DF00215AE";
const GUID_RECUPERACION: &str = "DE94BBA4-06D1-4D40-A16A-BFD50179D6AC";
const GUID_SWAP: &str = "0657FD6D-A4AB-43C4-84E5-0933C84B4F4F";
/// Tipos cuyo contenido no se reconoce pero que pueden ser un Linux o un disco dinámico de Windows:
/// si el formato es desconocido, se bloquean.
const GUID_PELIGROSOS: [(&str, &str); 6] = [
    ("0FC63DAF-8483-4772-8E79-3D69D8477DE4", "Formato no reconocido: podría contener Linux"),
    ("4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709", "Formato no reconocido: podría contener Linux"),
    ("E6D6D379-F507-44C2-A23C-238F2A3DF928", "Formato no reconocido: podría contener Linux"),
    ("A19D880F-05FC-4D3B-A006-743F0F84911E", "Formato no reconocido: podría contener Linux"),
    ("5808C8AA-7E8F-42E0-85D2-E1E90434CFB3", "Es un disco dinámico de Windows"),
    ("AF9B60A0-1431-4F62-BC68-3311714A69AD", "Es un disco dinámico de Windows"),
];

/// Etiqueta de la partición y por qué no se puede usar (mira dentro, en solo lectura).
fn examinar(abrir: Abrir, e: &Entrada, kind: FsKind) -> (String, Option<&'static str>) {
    let mut etiqueta = gpt::nombre(e);
    if e.tipo == TIPO_NFFS || kind == FsKind::NightFury {
        return (etiqueta, Some("NightFury ya está aquí"));
    }
    if kind == FsKind::BitLocker {
        return (etiqueta, Some("Cifrada con BitLocker"));
    }
    let (rutas, motivo): (&[&str], _) = match kind {
        FsKind::Ntfs => (&["Windows\\System32"], "Contiene Windows"),
        FsKind::Ext => (&["etc\\os-release", "usr\\lib\\os-release"], "Contiene Linux"),
        _ => (&[], ""),
    };
    if !rutas.is_empty() {
        let Ok(mut v) = montar(abrir, e.inicio_lba, e.sectores(), false) else { return (etiqueta, Some("No se puede comprobar su contenido")) };
        if !v.label().is_empty() {
            etiqueta = v.label();
        }
        let r: Vec<_> = rutas.iter().map(|r| v.stat(r)).collect();
        if r.iter().any(|x| x.is_ok()) {
            return (etiqueta, Some(motivo));
        }
        if r.iter().any(|x| *x != Err(FsError::NotFound)) {
            return (etiqueta, Some("No se puede comprobar su contenido"));
        }
    }
    let peligro = GUID_PELIGROSOS.iter().find(|(g, _)| guid(g) == e.tipo).filter(|_| kind == FsKind::Unknown);
    (etiqueta, peligro.map(|p| p.1))
}

/// Mira un disco sin escribir nada y dice dónde se podría instalar NightFury junto a lo que haya.
/// Nunca se encogen particiones. `bloqueo_disco`: no es GPT, hay BitLocker o la ESP no tiene sitio.
pub fn analizar(abrir: Abrir) -> Analisis {
    let mut a = Analisis { bloqueo_disco: None, esp: None, sitios: Vec::new(), nightfury: false };
    let mut d = abrir();
    let Some((_, ent)) = gpt::leer_gpt(&mut *d) else {
        let mut s0 = vec![0u8; d.sector_size()];
        let mbr = d.read(0, &mut s0).is_ok() && s0[510..512] == [0x55, 0xAA];
        a.bloqueo_disco = Some(if mbr { "El disco usa MBR: no es compatible" } else { "El disco no tiene tabla GPT" });
        return a;
    };
    let (ss, ultimo) = (d.sector_size(), gpt::ultimo_lba_usable(d.sectors(), d.sector_size()));
    let tipos: Vec<FsKind> = ent.iter().map(|e| gpt::como_particion(&mut *d, e.inicio_lba, e.sectores()).kind).collect();
    drop(d);
    a.nightfury = buscar_marca(abrir, &ent).is_some() || ent.iter().zip(&tipos).any(|(e, k)| e.tipo == TIPO_NFFS || *k == FsKind::NightFury);
    if tipos.contains(&FsKind::BitLocker) {
        a.bloqueo_disco = Some("El disco tiene una partición con BitLocker");
    }
    if let Some((i, e)) = ent.iter().enumerate().find(|(_, e)| e.tipo == TIPO_ESP) {
        a.esp = Some(esp_de(e));
        let libre = if matches!(tipos[i], FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32) { montar(abrir, e.inicio_lba, e.sectores(), false).map(|mut v| v.space().1).ok() } else { None };
        if a.bloqueo_disco.is_none() && libre.map_or(true, |l| l < ESP_MINIMA) {
            a.bloqueo_disco = Some(if libre.is_none() { "No se puede leer la partición EFI" } else { "La partición EFI no tiene sitio para NightFury" });
        }
    }
    let ocultos = [TIPO_ESP, guid(GUID_MSR), guid(GUID_RECUPERACION), guid(GUID_SWAP)];
    let mut orden: Vec<usize> = (0..ent.len()).collect();
    orden.sort_by_key(|&i| ent[i].inicio_lba);
    let hueco = |s: &mut Vec<Sitio>, i: u64, f: u64| {
        if f >= i && (f - i + 1) * (ss as u64) >= MIN_SITIO {
            s.push(Sitio { inicio_lba: i, sectores: f - i + 1, lugar: Lugar::Hueco, bloqueo: None });
        }
    };
    let mut cursor = gpt::alinear_mib(gpt::primer_lba_usable(ss), ss);
    for i in orden {
        let e = &ent[i];
        if e.inicio_lba > cursor {
            hueco(&mut a.sitios, cursor, e.inicio_lba - 1);
        }
        cursor = cursor.max(gpt::alinear_mib(e.fin_lba + 1, ss));
        if ocultos.contains(&e.tipo) {
            continue;
        }
        let (etiqueta, mut bloqueo) = examinar(abrir, e, tipos[i]);
        if bloqueo.is_none() && e.sectores() * (ss as u64) < MIN_SITIO {
            bloqueo = Some("Es demasiado pequeña (mínimo 2 GiB)");
        }
        if bloqueo.is_none() && a.esp.is_none() {
            bloqueo = Some("El disco no tiene partición EFI");
        }
        a.sitios.push(Sitio { inicio_lba: e.inicio_lba, sectores: e.sectores(), lugar: Lugar::Particion { numero: e.numero, fs: tipos[i], etiqueta }, bloqueo });
    }
    hueco(&mut a.sitios, cursor, ultimo);
    a
}

/// Instala NightFury junto a lo que hay en `sitio` (uno de `analizar`, que se vuelve a comprobar entero).
/// Orden: archivos en la ESP (solo `\EFI\NightFury`; si no hay ESP se crea una de 256 MiB al principio del
/// hueco con `\EFI\BOOT\BOOTX64.EFI`), luego la GPT, luego la partición en blanco y por último `particion`.
/// Las demás particiones no se tocan. Errores: `Denied` (bloqueado), `Exists` (ya instalado), `NotFound`
/// (el sitio ya no es válido), `NoSpace` (la ESP no cabe los archivos + 1 MiB o no hay entradas libres).
pub fn instalar_junto(abrir: Abrir, sitio: &Sitio, nucleo: &[u8], arranque: &[u8], version: &str, now: DateTime, aleatorio: &mut dyn FnMut() -> u64) -> Result<Esp, FsError> {
    let arranque = if arranque.is_empty() { nucleo } else { arranque };
    let a = analizar(abrir);
    if a.bloqueo_disco.is_some() {
        return Err(FsError::Denied);
    }
    if a.nightfury {
        return Err(FsError::Exists);
    }
    let s = a.sitios.iter().find(|s| s.inicio_lba == sitio.inicio_lba && s.sectores == sitio.sectores && s.lugar == sitio.lugar).ok_or(FsError::NotFound)?;
    if s.bloqueo.is_some() {
        return Err(FsError::Denied);
    }
    let mut d = abrir();
    let ss = d.sector_size();
    let n_ent = gpt::leer_gpt(&mut *d).ok_or(FsError::Corrupt)?.1.len();
    drop(d);
    let ver = format!("NightFury {version}\r\n");
    let (mut n_ini, n_fin) = (s.inicio_lba, s.inicio_lba + s.sectores - 1);
    let nueva_esp = a.esp.is_none();
    let extra = nueva_esp as usize + matches!(s.lugar, Lugar::Hueco) as usize;
    if n_ent + extra > gpt::ENTRADAS {
        return Err(FsError::NoSpace);
    }
    let mut esp = match &a.esp {
        Some(e) => {
            let total = (nucleo.len() + arranque.len() + ver.len()) as u64 + ESP_MARGEN;
            if montar(abrir, e.inicio_lba, e.sectores, false)?.space().1 < total {
                return Err(FsError::NoSpace);
            }
            escribir_en_esp(abrir, e.inicio_lba, e.sectores, &archivos_nf(nucleo, arranque, ver.as_bytes()), now)?;
            e.clone()
        }
        None => {
            let sectores = ESP_INSTALAR / ss as u64;
            n_ini = gpt::alinear_mib(s.inicio_lba + sectores, ss);
            let e = Esp { unica: gpt::guid_nuevo(aleatorio), inicio_lba: s.inicio_lba, sectores, numero: 0 };
            let mut f = archivos_nf(nucleo, arranque, ver.as_bytes()).to_vec();
            f.push((ARRANQUE, arranque));
            llenar_esp(abrir, e.inicio_lba, e.sectores, &f, now)?;
            e
        }
    };
    if nueva_esp {
        let p = Particion { tipo: TIPO_ESP, unica: esp.unica, nombre: NOMBRE_ESP, inicio_lba: esp.inicio_lba, fin_lba: esp.inicio_lba + esp.sectores - 1 };
        gpt::anadir_particion(&mut *abrir(), &p)?;
    }
    let nffs = Particion { tipo: TIPO_NFFS, unica: gpt::guid_nuevo(aleatorio), nombre: "NightFury", inicio_lba: n_ini, fin_lba: n_fin };
    match s.lugar {
        Lugar::Hueco => gpt::anadir_particion(&mut *abrir(), &nffs)?,
        Lugar::Particion { numero, .. } => gpt::reemplazar(&mut *abrir(), numero, Some(&nffs))?,
    }
    poner_en_blanco(abrir, n_ini, n_fin - n_ini + 1)?;
    if nueva_esp {
        esp.numero = gpt::leer_gpt(&mut *abrir()).and_then(|(_, e)| e.iter().find(|e| e.unica == esp.unica).map(|e| e.numero)).ok_or(FsError::Corrupt)? as u32;
    }
    escribir_en_esp(abrir, esp.inicio_lba, esp.sectores, &[(NF_PARTICION, guid_texto(&nffs.unica).as_bytes())], now)?;
    abrir().flush()?;
    Ok(esp)
}

/// Primera ESP que lleva `\EFI\NightFury\particion` y la NFFS cuya GUID única dice (`None` si ya no existe).
fn buscar_marca<'a>(abrir: Abrir, ent: &'a [Entrada]) -> Option<(&'a Entrada, Option<&'a Entrada>)> {
    for e in ent.iter().filter(|e| e.tipo == TIPO_ESP) {
        let Ok(mut v) = montar(abrir, e.inicio_lba, e.sectores(), false) else { continue };
        if v.stat(NF_PARTICION).map_or(true, |s| s.size > 64) {
            continue;
        }
        let Ok(t) = v.read(NF_PARTICION) else { continue };
        let t = String::from_utf8_lossy(&t);
        return Some((e, ent.iter().find(|n| n.tipo == TIPO_NFFS && guid_texto(&n.unica).eq_ignore_ascii_case(t.trim()))));
    }
    None
}

/// La NFFS de NightFury en este disco: (primer sector, sectores). La que nombra `particion` en la ESP; si
/// ningún disco tiene ese archivo (instalaciones de la 0.12.6 o menos), la primera NFFS.
pub fn particion_nightfury(abrir: Abrir) -> Option<(u64, u64)> {
    let (_, ent) = gpt::leer_gpt(&mut *abrir())?;
    let n = match buscar_marca(abrir, &ent) {
        Some((_, n)) => n,
        None => ent.iter().find(|e| e.tipo == TIPO_NFFS),
    };
    n.map(|e| (e.inicio_lba, e.sectores()))
}

/// Lo que `quitar` ha borrado: la ESP donde estaba NightFury (para quitar su `Boot####`: su GUID única y el
/// número de partición salen en la entrada UEFI), la NFFS (`None` si ya no estaba) y si se borró la ESP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quitado {
    pub esp: Esp,
    pub nffs: Option<(u64, u64)>,
    pub esp_borrada: bool,
}

fn borrar_arbol(v: &mut dyn Volume, dir: &str) -> Result<(), FsError> {
    for e in v.list(dir)? {
        let ruta = format!("{dir}\\{}", e.name);
        if e.dir {
            borrar_arbol(v, &ruta)?;
        } else {
            v.delete(&ruta)?;
        }
    }
    v.delete(dir)
}

/// ¿La ESP es una que creó NightFury? Se llama «NightFury ESP» y en `\EFI` solo hay BOOT y NightFury.
fn esp_nuestra(abrir: Abrir, e: &Entrada) -> bool {
    gpt::nombre(e) == NOMBRE_ESP
        && montar(abrir, e.inicio_lba, e.sectores(), false)
            .and_then(|mut v| v.list("EFI"))
            .is_ok_and(|l| l.iter().all(|x| x.name.eq_ignore_ascii_case("BOOT") || x.name.eq_ignore_ascii_case("NightFury")))
}

/// Quita NightFury: borra la entrada GPT de su NFFS (queda hueco) y pone en blanco su cabecera, borra
/// `\EFI\NightFury` de la ESP y, si la ESP era nuestra, la borra también de la GPT. Nunca toca otras
/// particiones ni otras carpetas de la ESP. Busca la instalación por `particion`, o (0.12.6) por la ESP
/// llamada «NightFury ESP» y la primera NFFS. `NotFound` si no hay ninguna. Quitar la entrada UEFI
/// `Boot####` es cosa del núcleo.
pub fn quitar(abrir: Abrir) -> Result<Quitado, FsError> {
    let (_, ent) = gpt::leer_gpt(&mut *abrir()).ok_or(FsError::Corrupt)?;
    let (e, nffs) = match buscar_marca(abrir, &ent) {
        Some(x) => x,
        None => {
            let e = ent.iter().find(|e| e.tipo == TIPO_ESP && gpt::nombre(e) == NOMBRE_ESP).ok_or(FsError::NotFound)?;
            (e, ent.iter().find(|e| e.tipo == TIPO_NFFS))
        }
    };
    let nuestra = esp_nuestra(abrir, e);
    if let Some(n) = nffs {
        gpt::reemplazar(&mut *abrir(), n.numero, None)?;
        poner_en_blanco(abrir, n.inicio_lba, n.sectores())?;
    }
    if nuestra {
        gpt::reemplazar(&mut *abrir(), e.numero, None)?;
    } else {
        let mut v = montar(abrir, e.inicio_lba, e.sectores(), true)?;
        match borrar_arbol(&mut *v, NF_DIR) {
            Ok(()) | Err(FsError::NotFound) => {}
            Err(x) => return Err(x),
        }
        v.flush()?;
    }
    abrir().flush()?;
    Ok(Quitado { esp: esp_de(e), nffs: nffs.map(|n| (n.inicio_lba, n.sectores())), esp_borrada: nuestra })
}

/// Valor de una variable `Boot####` (EFI_LOAD_OPTION, UEFI 2.x 3.1.3) que arranca `ruta` de la ESP de la
/// instalación: activa, descripción «NightFury», y la ruta = nodo Hard Drive (número de partición real de la
/// ESP en la GPT, firma = su GUID) + nodo File Path + nodo fin.
pub fn opcion_arranque(esp: &Esp, ruta: &str) -> Vec<u8> {
    let ruta: Vec<u8> = ruta.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
    let mut lista = Vec::new();
    lista.extend([4, 1, 42, 0]);
    lista.extend(esp.numero.to_le_bytes());
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

    fn arranque() -> Vec<u8> {
        (0..200_000usize).map(|i| (i.wrapping_mul(7) ^ (i >> 5)) as u8).collect()
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
            crear_pendrive(&mut pen.abrir(), &efi(), b"menu", "0.12.6", ESP_PENDRIVE, NOW, &mut azar()).unwrap();
            assert!(es_pendrive(&mut pen.abrir()), "ss={ss}");
            assert_eq!(portatil(&mut pen.abrir()), None);
            let antes = pen.0.borrow().data.clone();
            assert!(es_pendrive(&mut pen.abrir()));
            assert!(pen.0.borrow().data == antes, "es_pendrive no escribe (ni marca la FAT como sucia)");
            assert_eq!(leer_efi(&mut pen.abrir()).unwrap(), efi());
            assert_eq!(leer_arranque(&mut pen.abrir()).unwrap(), b"menu");
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

            let arr = arranque();
            let esp = instalar(&mut d.abrir(), &efi(), &arr, "0.12.6", NOW, &mut azar()).unwrap();
            assert_eq!(esp.numero, 1);
            let (_, e) = gpt::leer_gpt(&mut d.clone()).unwrap();
            assert_eq!(e.len(), 2, "solo ESP + NFFS");
            assert_eq!((e[0].tipo, e[0].unica, e[0].inicio_lba, e[0].sectores()), (TIPO_ESP, esp.unica, esp.inicio_lba, esp.sectores));
            assert_eq!(e[0].sectores() * ss as u64, ESP_INSTALAR);
            assert_eq!(e[1].tipo, TIPO_NFFS);
            assert_ne!(e[0].unica, e[1].unica);
            for (ruta, esperado) in [(ARRANQUE, arr.clone()), (NF_ARRANQUE, arr.clone()), (NF_NUCLEO, efi())] {
                assert_eq!(leer(&d, e[0].inicio_lba, e[0].sectores(), ruta), esperado, "{ruta}");
            }
            assert_eq!(leer(&d, e[0].inicio_lba, e[0].sectores(), NF_PARTICION), guid_texto(&e[1].unica).as_bytes());
            assert_eq!(particion_nightfury(&mut d.abrir()), Some((e[1].inicio_lba, e[1].sectores())));
            // Sin `arranque` el núcleo hace de BOOTX64 (como hasta ahora).
            let solo = Disco::nuevo(300, ss);
            instalar(&mut solo.abrir(), &efi(), &[], "x", NOW, &mut azar()).unwrap();
            let (_, es) = gpt::leer_gpt(&mut solo.clone()).unwrap();
            assert_eq!(leer(&solo, es[0].inicio_lba, es[0].sectores(), NF_ARRANQUE), efi());
            // La NFFS queda en blanco: el primer arranque la ve como disco nuevo y la formatea el asistente.
            let dev = || Box::new(nfvol::SectorBlocks { dev: Box::new(d.clone()), start: e[1].inicio_lba, sectors: e[1].sectores() });
            assert!(crate::fs::Nffs::is_blank(&mut *dev()));
            let mut f = crate::files::Files::probe(Some(dev()), NOW);
            assert!(f.pending_format());
            f.format_pending(NOW).unwrap();
            assert!(f.persistent());
            // Un disco demasiado pequeño no se toca.
            let chico = Disco::nuevo(260, ss);
            assert_eq!(instalar(&mut chico.abrir(), &efi(), &[], "x", NOW, &mut azar()), Err(FsError::NoSpace));
            assert!(chico.0.borrow().data.iter().all(|&b| b == 0));
            // Quitarla (0.12.7): sin tocar nada más, queda la GPT vacía y válida.
            let q = quitar(&mut d.abrir()).unwrap();
            assert_eq!((q.esp, q.nffs.is_some(), q.esp_borrada), (esp, true, true));
            assert!(gpt::leer_gpt(&mut d.clone()).unwrap().1.is_empty());
            assert_eq!(quitar(&mut d.abrir()), Err(FsError::NotFound));
            assert_eq!(particion_nightfury(&mut d.abrir()), None);
        }
    }

    #[test]
    fn entrada_de_arranque_uefi() {
        let esp = Esp { unica: [0xAB; 16], inicio_lba: 2048, sectores: 524_288, numero: 3 };
        let o = opcion_arranque(&esp, "\\EFI\\NightFury\\nightfury.efi");
        assert_eq!(&o[..4], &1u32.to_le_bytes());
        let lista_len = u16::from_le_bytes([o[4], o[5]]) as usize;
        let desc = 6 + 2 * ("NightFury".len() + 1);
        assert_eq!(&o[6..8], &[b'N', 0]);
        assert_eq!(&o[desc - 2..desc], &[0, 0]);
        assert_eq!(o.len(), desc + lista_len);
        let hd = &o[desc..];
        assert_eq!(&hd[..4], &[4, 1, 42, 0]);
        assert_eq!(&hd[4..8], &3u32.to_le_bytes());
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

    // --- Instalar junto a otro sistema (0.12.7) ---

    /// Los discos de estas pruebas son de ~2,3 GiB: de uno en uno, para no pedir gigas de golpe.
    static GRANDE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    const MS: &[u8] = b"MZ cargador del otro sistema";
    const BOOTMGR: &str = "EFI\\Microsoft\\Boot\\bootmgfw.efi";

    fn disco(tipo: crate::vol::imagenes::Prueba, mib: u64, ss: usize) -> Disco {
        let d = Disco::nuevo(mib, ss);
        crate::vol::imagenes::crear(&mut d.abrir(), tipo, MS, NOW, &mut azar()).unwrap();
        d
    }

    /// Resumen de los bytes de una partición (FNV por palabras: rápido sobre decenas de MiB).
    fn huella(d: &Disco, e: &Entrada) -> u64 {
        let ss = d.0.borrow().sector;
        d.0.borrow().data[e.inicio_lba as usize * ss..][..e.sectores() as usize * ss].chunks_exact(8).fold(0xCBF2_9CE4_8422_2325u64, |h, c| (h ^ u64::from_le_bytes(c.try_into().unwrap())).wrapping_mul(0x0100_0000_01B3))
    }

    fn existe(d: &Disco, e: &Entrada, ruta: &str) -> bool {
        let p = gpt::como_particion(&mut d.clone(), e.inicio_lba, e.sectores());
        crate::vol::mount(Box::new(Lectura(Box::new(d.clone()))), &p).unwrap().stat(ruta).is_ok()
    }

    /// Las dos cabeceras GPT valen por separado (CRC): se anula una y se lee con la otra.
    fn gpt_valida(d: &Disco) {
        let n = d.0.borrow().sectors();
        let ss = d.0.borrow().sector;
        for lba in [1, n - 1] {
            let guardado = d.0.borrow().data[lba as usize * ss..][..ss].to_vec();
            d.0.borrow_mut().data[lba as usize * ss..][..ss].fill(0);
            assert!(gpt::leer_gpt(&mut d.clone()).is_some(), "la otra cabecera no basta (lba {lba})");
            d.0.borrow_mut().data[lba as usize * ss..][..ss].copy_from_slice(&guardado);
        }
    }

    fn entradas(d: &Disco) -> Vec<Entrada> {
        gpt::leer_gpt(&mut d.clone()).unwrap().1
    }

    #[test]
    fn junto_a_windows_en_hueco_y_quitar() {
        use crate::vol::imagenes::Prueba;
        let _g = GRANDE.lock().unwrap_or_else(|e| e.into_inner());
        for ss in [512usize, 4096] {
            let d = disco(Prueba::Windows, 2300, ss);
            let antes = entradas(&d);
            assert_eq!(antes.len(), 3);
            let a = analizar(&mut d.abrir());
            assert_eq!((a.bloqueo_disco, a.nightfury), (None, false));
            assert_eq!(a.esp, Some(Esp { unica: antes[0].unica, inicio_lba: antes[0].inicio_lba, sectores: antes[0].sectores(), numero: 1 }));
            // ESP y MSR no salen; Windows sale bloqueada; queda un hueco alineado a 1 MiB.
            assert_eq!(a.sitios.len(), 2, "{:?}", a.sitios);
            assert_eq!((a.sitios[0].bloqueo, &a.sitios[0].lugar), (Some("Contiene Windows"), &Lugar::Particion { numero: 3, fs: FsKind::Ntfs, etiqueta: "Windows".into() }));
            let hueco = a.sitios[1].clone();
            assert_eq!((hueco.lugar.clone(), hueco.bloqueo), (Lugar::Hueco, None));
            assert_eq!(hueco.inicio_lba % (MIB / ss as u64), 0);
            assert!(hueco.sectores * ss as u64 >= MIN_SITIO);
            let (h_msr, h_win) = (huella(&d, &antes[1]), huella(&d, &antes[2]));

            // Windows no se elige ni se inventa un sitio.
            assert_eq!(instalar_junto(&mut d.abrir(), &a.sitios[0], &efi(), &[], "x", NOW, &mut azar()), Err(FsError::Denied));
            assert_eq!(instalar_junto(&mut d.abrir(), &Sitio { inicio_lba: hueco.inicio_lba + 1, ..hueco.clone() }, &efi(), &[], "x", NOW, &mut azar()), Err(FsError::NotFound));
            assert_eq!(entradas(&d), antes);

            let arr = arranque();
            let esp = instalar_junto(&mut d.abrir(), &hueco, &efi(), &arr, "0.12.7", NOW, &mut azar()).unwrap();
            assert_eq!((esp.numero, esp.unica), (1, antes[0].unica));
            let ahora = entradas(&d);
            assert_eq!(ahora.len(), 4);
            // Las tres de antes, byte a byte (nombre, atributos, GUID); la nueva es la NFFS del hueco.
            assert_eq!(ahora[..3], antes[..]);
            let n = &ahora[3];
            assert_eq!((n.numero, n.tipo, gpt::nombre(n), n.inicio_lba, n.sectores()), (4, TIPO_NFFS, "NightFury".into(), hueco.inicio_lba, hueco.sectores));
            gpt_valida(&d);
            assert_eq!((huella(&d, &antes[1]), huella(&d, &antes[2])), (h_msr, h_win), "las demás particiones no se tocan");
            // En la ESP ajena solo \EFI\NightFury (nada de \EFI\BOOT) y Microsoft sigue igual.
            let (i, c) = (antes[0].inicio_lba, antes[0].sectores());
            assert!(!existe(&d, &antes[0], ARRANQUE));
            assert_eq!(leer(&d, i, c, NF_ARRANQUE), arr);
            assert_eq!(leer(&d, i, c, NF_NUCLEO), efi());
            assert_eq!(leer(&d, i, c, NF_VERSION), b"NightFury 0.12.7\r\n");
            assert_eq!(leer(&d, i, c, NF_PARTICION), guid_texto(&n.unica).as_bytes());
            assert_eq!(leer(&d, i, c, BOOTMGR), MS);
            assert_eq!(particion_nightfury(&mut d.abrir()), Some((n.inicio_lba, n.sectores())));
            // La NFFS queda en blanco, lista para el asistente de disco nuevo.
            let dev = || Box::new(nfvol::SectorBlocks { dev: Box::new(d.clone()), start: n.inicio_lba, sectors: n.sectores() });
            assert!(crate::fs::Nffs::is_blank(&mut *dev()));
            // Ya instalado: no se vuelve a instalar.
            assert!(analizar(&mut d.abrir()).nightfury);
            assert_eq!(instalar_junto(&mut d.abrir(), &hueco, &efi(), &[], "x", NOW, &mut azar()), Err(FsError::Exists));

            // Quitar: la NFFS se va, \EFI\NightFury también, y la ESP de Windows sigue siendo suya.
            let q = quitar(&mut d.abrir()).unwrap();
            assert_eq!((q.esp.clone(), q.nffs, q.esp_borrada), (esp, Some((n.inicio_lba, n.sectores())), false));
            assert_eq!(entradas(&d), antes);
            gpt_valida(&d);
            assert_eq!((huella(&d, &antes[1]), huella(&d, &antes[2])), (h_msr, h_win), "Windows intacto tras instalar y quitar");
            assert!(!existe(&d, &antes[0], NF_DIR));
            assert_eq!(leer(&d, i, c, BOOTMGR), MS);
            assert_eq!(particion_nightfury(&mut d.abrir()), None);
            assert_eq!(quitar(&mut d.abrir()), Err(FsError::NotFound));
            let c = analizar(&mut d.abrir());
            assert_eq!((c.nightfury, c.sitios.len(), c.sitios[1].lugar.clone()), (false, 2, Lugar::Hueco));
        }
    }

    #[test]
    fn junto_a_windows_en_particion_de_datos() {
        use crate::vol::imagenes::Prueba;
        let _g = GRANDE.lock().unwrap_or_else(|e| e.into_inner());
        for ss in [512usize, 4096] {
            let d = disco(Prueba::WindowsDatos, 2250, ss);
            let antes = entradas(&d);
            assert_eq!(antes.len(), 4);
            let a = analizar(&mut d.abrir());
            assert_eq!(a.bloqueo_disco, None);
            assert_eq!(a.sitios.len(), 2, "sin hueco de 2 GiB: {:?}", a.sitios);
            assert_eq!(a.sitios[0].bloqueo, Some("Contiene Windows"));
            let datos = a.sitios[1].clone();
            assert_eq!((datos.bloqueo, datos.lugar.clone()), (None, Lugar::Particion { numero: 4, fs: FsKind::Fat32, etiqueta: "Datos".into() }));
            let (h_msr, h_win) = (huella(&d, &antes[1]), huella(&d, &antes[2]));
            let esp = instalar_junto(&mut d.abrir(), &datos, &efi(), &[], "0.12.7", NOW, &mut azar()).unwrap();
            assert_eq!(esp.numero, 1);
            let ahora = entradas(&d);
            // Mismo puesto y mismo rango: solo cambian tipo, GUID y nombre.
            assert_eq!(ahora[..3], antes[..3]);
            assert_eq!((ahora[3].numero, ahora[3].tipo, gpt::nombre(&ahora[3]), ahora[3].inicio_lba, ahora[3].fin_lba), (4, TIPO_NFFS, "NightFury".into(), antes[3].inicio_lba, antes[3].fin_lba));
            assert_ne!(ahora[3].unica, antes[3].unica);
            gpt_valida(&d);
            assert_eq!((huella(&d, &antes[1]), huella(&d, &antes[2])), (h_msr, h_win));
            // Sin `arranque`, el núcleo hace de arranque.efi.
            assert_eq!(leer(&d, antes[0].inicio_lba, antes[0].sectores(), NF_ARRANQUE), efi());
            let dev = || Box::new(nfvol::SectorBlocks { dev: Box::new(d.clone()), start: ahora[3].inicio_lba, sectors: ahora[3].sectores() });
            assert!(crate::fs::Nffs::is_blank(&mut *dev()));
            let mut f = crate::files::Files::probe(Some(dev()), NOW);
            assert!(f.pending_format());
            f.format_pending(NOW).unwrap();
            assert_eq!(particion_nightfury(&mut d.abrir()), Some((ahora[3].inicio_lba, ahora[3].sectores())));
            // Quitar la deja como hueco: Windows y el resto, tal cual.
            let q = quitar(&mut d.abrir()).unwrap();
            assert_eq!((q.nffs, q.esp_borrada), (Some((ahora[3].inicio_lba, ahora[3].sectores())), false));
            assert_eq!(entradas(&d), antes[..3]);
            assert_eq!((huella(&d, &antes[1]), huella(&d, &antes[2])), (h_msr, h_win));
            gpt_valida(&d);
        }
    }

    #[test]
    fn junto_bloqueos_y_esp() {
        use crate::vol::imagenes::Prueba;
        let _g = GRANDE.lock().unwrap_or_else(|e| e.into_inner());
        let ss = 512;
        // Linux: ext con /etc/os-release bloqueada, hueco libre.
        let l = disco(Prueba::Linux, 2300, ss);
        let a = analizar(&mut l.abrir());
        assert_eq!((a.bloqueo_disco, a.sitios.len()), (None, 2));
        assert_eq!((a.sitios[0].bloqueo, a.sitios[1].lugar.clone()), (Some("Contiene Linux"), Lugar::Hueco));
        // BitLocker en cualquier partición: el disco no se toca. MBR y disco vacío tampoco.
        let b = disco(Prueba::BitLocker, 300, ss);
        let a = analizar(&mut b.abrir());
        assert_eq!(a.bloqueo_disco, Some("El disco tiene una partición con BitLocker"));
        assert_eq!(instalar_junto(&mut b.abrir(), &Sitio { inicio_lba: 2048, sectores: 4_000_000, lugar: Lugar::Hueco, bloqueo: None }, &efi(), &[], "x", NOW, &mut azar()), Err(FsError::Denied));
        let m = disco(Prueba::Mbr, 64, ss);
        assert_eq!(analizar(&mut m.abrir()).bloqueo_disco, Some("El disco usa MBR: no es compatible"));
        assert_eq!(analizar(&mut Disco::nuevo(64, ss).abrir()).bloqueo_disco, Some("El disco no tiene tabla GPT"));

        // ESP sin sitio para los archivos: NoSpace sin tocar nada; casi llena: bloqueo del disco.
        let d = disco(Prueba::Windows, 2300, ss);
        let (antes, a) = (entradas(&d), analizar(&mut d.abrir()));
        let hueco = a.sitios[1].clone();
        let grande = vec![7u8; 50 << 20];
        assert_eq!(instalar_junto(&mut d.abrir(), &hueco, &grande, &grande, "x", NOW, &mut azar()), Err(FsError::NoSpace));
        assert_eq!(entradas(&d), antes);
        assert!(!existe(&d, &antes[0], NF_DIR));
        let mut v = montar(&mut d.abrir(), antes[0].inicio_lba, antes[0].sectores(), true).unwrap();
        v.write("relleno.bin", &vec![1u8; 80 << 20], NOW).unwrap();
        v.flush().unwrap();
        drop(v);
        let a = analizar(&mut d.abrir());
        assert_eq!(a.bloqueo_disco, Some("La partición EFI no tiene sitio para NightFury"));
        assert_eq!(instalar_junto(&mut d.abrir(), &hueco, &efi(), &[], "x", NOW, &mut azar()), Err(FsError::Denied));
        assert_eq!(entradas(&d), antes);

        // Sin ESP: se crea una de 256 MiB al principio del hueco, con BOOTX64 propio; quitar la borra.
        let d = disco(Prueba::Windows, 2300, ss);
        gpt::reemplazar(&mut d.clone(), 1, None).unwrap();
        let (antes, a) = (entradas(&d), analizar(&mut d.abrir()));
        assert_eq!((a.bloqueo_disco, a.esp.clone()), (None, None));
        assert_eq!(a.sitios.iter().map(|s| s.bloqueo).collect::<Vec<_>>(), [Some("Contiene Windows"), None]);
        let hueco = a.sitios[1].clone();
        let h_win = huella(&d, &antes[1]);
        let arr = arranque();
        let esp = instalar_junto(&mut d.abrir(), &hueco, &efi(), &arr, "0.12.7", NOW, &mut azar()).unwrap();
        let ahora = entradas(&d);
        assert_eq!(ahora.len(), 4);
        let e = ahora.iter().find(|e| e.unica == esp.unica).unwrap();
        assert_eq!((e.numero as u32, e.tipo, gpt::nombre(e), e.inicio_lba, e.sectores() * ss as u64), (esp.numero, TIPO_ESP, "NightFury ESP".into(), hueco.inicio_lba, ESP_INSTALAR));
        assert_eq!(leer(&d, e.inicio_lba, e.sectores(), ARRANQUE), arr);
        assert_eq!(leer(&d, e.inicio_lba, e.sectores(), NF_NUCLEO), efi());
        let n = ahora.iter().find(|e| e.tipo == TIPO_NFFS).unwrap();
        assert_eq!((n.inicio_lba, n.fin_lba), (hueco.inicio_lba + ESP_INSTALAR / ss as u64, hueco.inicio_lba + hueco.sectores - 1));
        assert_eq!(leer(&d, e.inicio_lba, e.sectores(), NF_PARTICION), guid_texto(&n.unica).as_bytes());
        assert_eq!(huella(&d, &antes[1]), h_win);
        gpt_valida(&d);
        let q = quitar(&mut d.abrir()).unwrap();
        assert_eq!((q.esp, q.esp_borrada), (esp, true));
        assert_eq!(entradas(&d), antes);
        gpt_valida(&d);
    }

    #[test]
    fn quitar_instalacion_de_la_0_12_6_sin_particion() {
        let d = Disco::nuevo(300, 512);
        instalar(&mut d.abrir(), &efi(), &[], "0.12.6", NOW, &mut azar()).unwrap();
        let e = entradas(&d);
        // Como la 0.12.6: sin el archivo `particion`; se encuentra por el nombre de la ESP y la primera NFFS.
        let mut v = montar(&mut d.abrir(), e[0].inicio_lba, e[0].sectores(), true).unwrap();
        v.delete(NF_PARTICION).unwrap();
        v.flush().unwrap();
        drop(v);
        assert_eq!(particion_nightfury(&mut d.abrir()), Some((e[1].inicio_lba, e[1].sectores())));
        let q = quitar(&mut d.abrir()).unwrap();
        assert_eq!((q.nffs, q.esp_borrada), (Some((e[1].inicio_lba, e[1].sectores())), true));
        assert!(entradas(&d).is_empty());
    }

    #[test]
    fn particion_nightfury_elige_por_guid() {
        let d = Disco::nuevo(80, 512);
        let m = MIB / 512;
        let p = |k: u64, tipo, nombre| Particion { tipo, unica: [k as u8 + 1; 16], nombre, inicio_lba: 2048 + k * 20 * m, fin_lba: 2048 + k * 20 * m + 19 * m };
        gpt::crear_gpt(&mut d.clone(), &[p(0, TIPO_ESP, "EFI"), p(1, TIPO_NFFS, "a"), p(2, TIPO_NFFS, "b")], [9; 16]).unwrap();
        let e = entradas(&d);
        assert_eq!(particion_nightfury(&mut d.abrir()), Some((e[1].inicio_lba, e[1].sectores())), "sin archivo: la primera NFFS");
        llenar_esp(&mut d.abrir(), e[0].inicio_lba, e[0].sectores(), &[(NF_PARTICION, guid_texto(&e[2].unica).to_lowercase().as_bytes())], NOW).unwrap();
        assert_eq!(particion_nightfury(&mut d.abrir()), Some((e[2].inicio_lba, e[2].sectores())));
        // El archivo apunta a una NFFS que ya no está: no se adivina otra.
        gpt::reemplazar(&mut d.clone(), 3, None).unwrap();
        assert_eq!(particion_nightfury(&mut d.abrir()), None);
        assert_eq!(guid_texto(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]), "04030201-0605-0807-090A-0B0C0D0E0F10");
    }

    #[test]
    fn ntfs_y_ext_sin_sistema_se_ofrecen() {
        use crate::vol::imagenes::Prueba;
        let u16z = |s: &str| s.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
        // Sin \Windows ni os-release dentro se examina el contenido y no hay bloqueo por «contiene» ni por
        // «no se puede comprobar»: solo el de tamaño (el disco de prueba es pequeño).
        for (tipo, puesto, de, a) in [(Prueba::Windows, 2, u16z("Windows"), u16z("Wxndows")), (Prueba::Linux, 1, b"os-release".to_vec(), b"xs-release".to_vec())] {
            let d = disco(tipo, 300, 512);
            let ini = entradas(&d)[puesto].inicio_lba as usize * 512;
            let mut m = d.0.borrow_mut();
            let zona = &mut m.data[ini..ini + (1 << 20)];
            let mut n = 0;
            for k in 0..zona.len() - de.len() {
                if zona[k..k + de.len()] == de[..] {
                    zona[k..k + a.len()].copy_from_slice(&a);
                    n += 1;
                }
            }
            drop(m);
            assert!(n > 0);
            assert_eq!(analizar(&mut d.abrir()).sitios[0].bloqueo, Some("Es demasiado pequeña (mínimo 2 GiB)"), "{tipo:?}");
        }
    }
}
