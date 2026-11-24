// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Archivos tal como los ven las apps: unidad C:, rutas estilo Windows, Papelera y copias.
//! CONTRATO: las firmas públicas no cambian.
//!
//! Rutas absolutas canónicas: "C:\\" es la raíz; "C:\\Documentos\\nota.txt" un archivo (sin
//! barra final salvo en la raíz). No se distinguen mayúsculas.
//!
//! Estructura (v0.5.0): `C:\Usuarios\<nombre>` (seis carpetas estándar, `.config` y `.papelera`,
//! las dos últimas ocultas en los listados) y `C:\Sistema\{config,temp,lib,drivers,logs}`.
//!
//! Cada usuario tiene su Papelera en `<home>\.papelera`: cada elemento borrado se mueve allí con
//! un número como nombre y un índice de texto (`indice`) guarda su ruta original y su fecha.
//! Al enviar se escribe primero el índice y luego se mueve el elemento; al restaurar o vaciar,
//! al revés: un corte solo puede dejar líneas de índice sin elemento, que se ignoran.
//!
//! Permisos (aquí pasan todas las apps y el Terminal): un usuario normal no entra en la carpeta de
//! otro ni escribe en `C:\Sistema`; un administrador sí. Los errores de acceso son `FsError::Invalid`.
//!
//! Migración: al montar un disco con la estructura antigua (`Documentos`, `Imágenes`, `Escritorio`,
//! `Sistema\config.txt`, `$Papelera` en la raíz) o al formatear el disco pendiente, queda un
//! archivo de estado (`Sistema\config\migracion`) y lo antiguo lo adopta la primera cuenta. Solo se
//! mueve (renombrar, atómico), nunca se sobrescribe ni se borra; si se corta, el siguiente arranque
//! continúa donde se quedó.
use crate::accounts::{self, Account, AccountError};
use crate::block::{BlockDev, RamDisk};
use crate::cryptdev::{self, Core, CryptDev, Header, State};
use crate::crypto::{wipe, Pbkdf2Paso};
use crate::fs::{same, Entry, FsError, Nffs};
use crate::homecrypt::{self, HState, KeyFile, Keys};
use crate::drives::{DriveAction, DriveInfo, FormatRequest, Locked};
use crate::keys::{self, KeyError, Pool};
use crate::platform::DateTime;
use crate::vol::{LockedDrive, Volume};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use core::cell::RefCell;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Ubicación especial que el Explorador muestra como la Papelera (no es una ruta de disco).
pub const TRASH: &str = "Papelera";
/// Ubicación especial que el Explorador muestra como «Este equipo» (las unidades; no es una ruta).
pub const COMPUTER: &str = "Este equipo";

// ---- Estructura del disco (v0.5.0). Ningún otro módulo escribe estas rutas a mano. ----
pub const USERS_DIR: &str = "C:\\Usuarios";
pub const SYSTEM_DIR: &str = "C:\\Sistema";
/// Programas instalados (F3, v0.8): uno por carpeta. Solo los administradores escriben aquí.
pub const PROGRAMS_DIR: &str = "C:\\Programas";
pub const SYS_CONFIG_DIR: &str = "C:\\Sistema\\config";
pub const SYS_TEMP_DIR: &str = "C:\\Sistema\\temp";
pub const SYS_LIB_DIR: &str = "C:\\Sistema\\lib";
pub const SYS_DRIVERS_DIR: &str = "C:\\Sistema\\drivers";
pub const SYS_LOGS_DIR: &str = "C:\\Sistema\\logs";
/// Cuentas de usuario (formato en `accounts.rs`).
pub const ACCOUNTS_FILE: &str = "C:\\Sistema\\config\\cuentas";
/// Particiones internas que NightFury formateó y puede escribir (una línea `serie|inicio`).
const UNITS_FILE: &str = "C:\\Sistema\\unidades.cfg";
pub const LOG_FILE: &str = "C:\\Sistema\\logs\\sistema.log";
/// Cuenta de la unidad en memoria (sin disco no hay cuentas que guardar).
pub const DEFAULT_USER: &str = "Usuario";

/// Carpetas estándar de cada usuario.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Std {
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Music,
    Videos,
}

impl Std {
    pub const ALL: [Std; 6] = [Std::Desktop, Std::Documents, Std::Downloads, Std::Pictures, Std::Music, Std::Videos];

    /// Nombre de la carpeta (y el que se muestra).
    pub fn name(self) -> &'static str {
        match self {
            Std::Desktop => "Escritorio",
            Std::Documents => "Documentos",
            Std::Downloads => "Descargas",
            Std::Pictures => "Imágenes",
            Std::Music => "Música",
            Std::Videos => "Vídeos",
        }
    }
}

const USERS: &str = "Usuarios";
const SYSTEM: &str = "Sistema";
const PROGRAMS: &str = "Programas";
const SYS_SUBS: [&str; 5] = ["config", "temp", "lib", "drivers", "logs"];
const HOME_CONFIG: &str = ".config";
const HOME_TRASH: &str = ".papelera";
const INDEX_NAME: &str = "indice";
/// Estructura antigua: Papelera de la raíz y carpetas del usuario.
const LEGACY_STORE: &str = "$Papelera";
const LEGACY_DIRS: [&str; 3] = ["Documentos", "Imágenes", "Escritorio"];
/// Archivos internos de la migración y de los cambios de nombre de cuenta.
const VERSION_FILE: &str = "C:\\Sistema\\config\\version";
const STATE_FILE: &str = "C:\\Sistema\\config\\migracion";
const RENAME_FILE: &str = "C:\\Sistema\\config\\renombrar";
/// Archivos de llaves de las carpetas cifradas (uno por cuenta), relativo a la raíz.
const KEYS_DIR: &str = "Sistema\\config\\llaves";
/// Sitio de espera de lo suelto de `Sistema` que había en la estructura antigua.
const OLD_DIR: &str = "C:\\Sistema\\config\\antiguo";
/// Tamaño máximo del registro.
const LOG_MAX: usize = 64 * 1024;
/// Bloques del disco en memoria (32 MiB: caben los programas y las fuentes del sistema, ~4,5 MiB).
const MEMORY_BLOCKS: u64 = 8192;

const NOTICE_NO_DISK: &str = "No hay disco: los archivos se guardan solo en la memoria y se perderán al apagar.";
const NOTICE_IGNORED: &str =
    "El disco tiene otro contenido y se ha ignorado para no dañarlo: los archivos se guardan solo en la memoria y se perderán al apagar.";
const NOTICE_DAMAGED: &str =
    "El disco de NightFury está dañado y no se ha tocado: los archivos se guardan solo en la memoria y se perderán al apagar.";
const NOTICE_LOCKED: &str =
    "Disco cifrado: escribe su clave para abrirlo. Mientras, los archivos se guardan solo en la memoria y se perderán al apagar.";
const NOTICE_LOCKED_SKIPPED: &str =
    "Disco cifrado sin abrir: no se ha tocado y los archivos se guardan solo en la memoria y se perderán al apagar.";
const NOTICE_NEW: &str =
    "Disco sin formato: mientras no se prepare, los archivos se guardan solo en la memoria y se perderán al apagar.";

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TrashItem {
    /// Identificador para `restore`.
    pub id: String,
    pub name: String,
    /// Ruta absoluta que tenía antes de borrarse.
    pub original: String,
    pub dir: bool,
    pub size: u64,
    pub deleted: DateTime,
}

fn is_root(p: &str) -> bool {
    let b = p.as_bytes();
    (b.len() == 3 && b[1] == b':' && b[2] == b'\\') || (b.len() == 2 && b[1] == b':')
}

/// Une carpeta y nombre: join("C:\\", "a") = "C:\\a"; join("C:\\a", "b") = "C:\\a\\b".
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('\\') {
        format!("{dir}{name}")
    } else {
        format!("{dir}\\{name}")
    }
}

/// Carpeta que contiene a `path`; `None` para la raíz.
pub fn parent(path: &str) -> Option<String> {
    if is_root(path) {
        return None;
    }
    match path.rfind('\\') {
        Some(i) if i > 2 => Some(String::from(&path[..i])),
        Some(_) => Some(format!("{}:\\", path.chars().next()?)),
        None => None,
    }
}

/// Último componente de la ruta ("" para la raíz).
pub fn file_name(path: &str) -> &str {
    if is_root(path) {
        return "";
    }
    match path.rfind('\\') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// ¿Sirve `name` como nombre de archivo o carpeta? (no vacío, hasta 255 bytes, sin
/// `\ / : * ? " < > |` ni caracteres de control, y distinto de "." y "..").
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.chars().any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
}

/// Quita la letra de unidad si la hay. Devuelve (resto, letra en mayúscula). Que la unidad exista
/// lo comprueba cada operación de `Files`, no la ruta.
fn strip_drive(s: &str) -> Result<(&str, Option<char>), FsError> {
    let b = s.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        return Ok((&s[2..], Some(b[0].to_ascii_uppercase() as char)));
    }
    Ok((s, None))
}

fn apply<'a>(stack: &mut Vec<&'a str>, s: &'a str) -> Result<(), FsError> {
    for c in s.split(['\\', '/']) {
        match c {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            _ if c.contains(':') => return Err(FsError::BadPath),
            _ if !valid_name(c) => return Err(FsError::BadName),
            _ => stack.push(c),
        }
    }
    Ok(())
}

/// Convierte lo que escribe el usuario en una ruta absoluta canónica: admite rutas
/// relativas a `cwd`, ".", "..", "/" como separador y letra de unidad en mayúscula o minúscula.
pub fn resolve(cwd: &str, input: &str) -> Result<String, FsError> {
    let (rest, drive) = strip_drive(input)?;
    let (cwd_rest, cwd_drive) = strip_drive(cwd)?;
    let mut stack = Vec::new();
    if drive.is_none() && !rest.starts_with(['\\', '/']) {
        apply(&mut stack, cwd_rest)?;
    }
    apply(&mut stack, rest)?;
    let mut out = format!("{}:\\", drive.or(cwd_drive).unwrap_or('C'));
    out.push_str(&stack.join("\\"));
    Ok(out)
}

/// Ruta canónica (válida y fuera del almacén de la Papelera) de lo que pide una app.
fn canon(path: &str) -> Result<String, FsError> {
    let c = resolve("C:\\", path)?;
    // La Papelera de la estructura antigua (huérfana hasta que una cuenta la adopta) no se toca.
    if is_c(&c) && same(first(rel(&c)), LEGACY_STORE) {
        return Err(FsError::BadPath);
    }
    Ok(c)
}

/// ¿Está `path` dentro de `C:\Sistema` (o es esa carpeta)? Para que las apps pidan confirmación.
pub fn in_system(path: &str) -> bool {
    resolve("C:\\", path).is_ok_and(|c| is_c(&c) && same(first(rel(&c)), SYSTEM))
}

/// ¿Es una carpeta que no se puede eliminar, mover ni renombrar? (raíz, `Usuarios`, `Sistema` y
/// sus subcarpetas, la carpeta de cada cuenta y sus carpetas estándar, `.config` y `.papelera`). En
/// las demás unidades solo lo es la raíz.
pub fn is_protected(path: &str) -> bool {
    resolve("C:\\", path).is_ok_and(|c| protected(rel(&c)) && (is_c(&c) || rel(&c).is_empty()))
}

/// ¿Es una ruta canónica de la unidad C:?
fn is_c(canon: &str) -> bool {
    canon.starts_with('C')
}

/// Letra de una ruta canónica que no es de C: (unidad extraíble).
fn ext(canon: &str) -> Option<char> {
    (!is_c(canon)).then(|| canon.as_bytes()[0] as char)
}

/// Ruta relativa a la raíz de una ruta canónica.
fn rel(canon: &str) -> &str {
    &canon[3..]
}

fn first(rel: &str) -> &str {
    rel.split('\\').next().unwrap_or("")
}

fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        String::from(name)
    } else {
        format!("{dir}\\{name}")
    }
}

fn comps(rel: &str) -> Vec<&str> {
    rel.split('\\').filter(|c| !c.is_empty()).collect()
}

fn is_home_sub(name: &str) -> bool {
    same(name, HOME_CONFIG) || same(name, HOME_TRASH) || Std::ALL.iter().any(|s| same(name, s.name()))
}

/// No se pueden borrar, mover ni renombrar: la raíz, `Usuarios`, `Sistema` y sus subcarpetas, la
/// carpeta de cada cuenta y las suyas estándar, `.config` y `.papelera`. Solo mira la ruta.
fn protected(rel: &str) -> bool {
    let c = comps(rel);
    let Some(&top) = c.first() else { return true };
    if same(top, USERS) {
        return c.len() <= 2 || (c.len() == 3 && is_home_sub(c[2]));
    }
    (same(top, PROGRAMS) && c.len() == 1) || same(top, SYSTEM) && (c.len() == 1 || (c.len() == 2 && SYS_SUBS.iter().any(|s| same(s, c[1]))))
}

fn mk(fs: &mut Nffs, path: &str, now: DateTime) {
    if fs.stat(path).is_err() {
        let _ = fs.mkdir(path, now);
    }
}

/// `Usuarios` y `Sistema` con sus subcarpetas.
fn ensure_structure(fs: &mut Nffs, now: DateTime) {
    mk(fs, USERS, now);
    mk(fs, SYSTEM, now);
    mk(fs, PROGRAMS, now);
    for s in SYS_SUBS {
        mk(fs, &join_rel(SYSTEM, s), now);
    }
}

/// La carpeta completa de una cuenta.
fn ensure_home(fs: &mut Nffs, name: &str, now: DateTime) {
    let home = join_rel(USERS, name);
    mk(fs, &home, now);
    for s in Std::ALL {
        mk(fs, &join_rel(&home, s.name()), now);
    }
    mk(fs, &join_rel(&home, HOME_CONFIG), now);
    mk(fs, &join_rel(&home, HOME_TRASH), now);
}

fn new_memory(now: DateTime) -> Nffs {
    let mut fs = Nffs::format(Box::new(RamDisk::new(MEMORY_BLOCKS)), now).expect("disco en memoria");
    ensure_structure(&mut fs, now);
    ensure_home(&mut fs, DEFAULT_USER, now);
    let _ = fs.write(rel(VERSION_FILE), b"2\n", now);
    fs
}

/// ¿Quedan restos de la estructura antigua? (carpetas del usuario o Papelera en la raíz, o
/// archivos sueltos en `Sistema`).
fn legacy_items(fs: &mut Nffs) -> bool {
    let root = fs.list("").unwrap_or_default();
    let sys = fs.list(SYSTEM).unwrap_or_default();
    root.iter().any(|e| same(&e.name, LEGACY_STORE) || LEGACY_DIRS.iter().any(|d| same(&e.name, d)))
        || sys.iter().any(|e| !SYS_SUBS.iter().any(|s| same(s, &e.name)))
}

/// Sustituye el prefijo de `orig` según `pairs` (solo si coincide por componentes).
fn rewrite(orig: &str, pairs: &[(String, String)]) -> String {
    let lo = lower(orig);
    for (from, to) in pairs {
        let lf = lower(from);
        if lo == lf || (lo.starts_with(&lf) && lo.as_bytes()[lf.len()] == b'\\') {
            // `lower` puede cambiar la longitud en bytes: se cuenta por caracteres.
            let rest: String = orig.chars().skip(lf.chars().count()).collect();
            return format!("{to}{rest}");
        }
    }
    String::from(orig)
}

/// Copia todo el contenido de `src` a `dst` (las carpetas que ya existan se respetan).
fn copy_all(src: &mut Nffs, dst: &mut Nffs) -> Result<(), FsError> {
    let mut work = vec![String::new()];
    while let Some(p) = work.pop() {
        for e in src.list(&p)? {
            let q = join_rel(&p, &e.name);
            if e.dir {
                if dst.stat(&q).is_err() {
                    dst.mkdir(&q, e.modified)?;
                }
                work.push(q);
            } else {
                let data = src.read(&q)?;
                dst.write(&q, &data, e.modified)?;
            }
        }
    }
    Ok(())
}

/// Cifrado total del disco: dispositivo cifrado a la espera de su clave, o capa de cifrado en uso.
#[derive(Default)]
struct DiskState {
    locked: Option<(Box<dyn BlockDev>, Header)>,
    core: Option<Rc<RefCell<Core>>>,
}

pub struct Files {
    fs: Nffs,
    persistent: bool,
    pending: Option<Box<dyn BlockDev>>,
    notice: Option<&'static str>,
    generation: u64,
    clip: Option<(Vec<String>, bool)>,
    undo: Option<(Vec<Undo>, &'static str)>,
    /// Sesión abierta: (nombre de la cuenta, es administrador).
    user: Option<(String, bool)>,
    disk: DiskState,
    /// Cifrado de la carpeta de la sesión abierta (llaves, estado y conversión).
    hc: Home,
    /// Unidades extraíbles D:, E:… (v0.11).
    xd: XDrives,
    /// Raíces de confianza ya montadas (`raices::store`); se rehace al añadir o quitar una.
    roots: Option<alloc::sync::Arc<crate::x509::RootStore>>,
    /// Zona horaria y peticiones de hora (v0.12.3).
    hora: crate::hora::Estado,
}

/// Una unidad conectada: abierta (`vol`) o cifrada esperando contraseña (`locked`).
struct XDrive {
    letter: char,
    disk: u32,
    /// Primer sector de la partición en su disco (identifica la partición de un disco interno).
    part: u64,
    /// `false` en las particiones de los discos internos (SATA, NVMe).
    removable: bool,
    device: String,
    vol: Option<Box<dyn Volume>>,
    locked: Option<LockedDrive>,
}

/// Desbloqueo en curso: el PBKDF2 de la unidad `letter`, que se avanza con `unlock_step`.
struct UnlockJob {
    letter: char,
    recovery: bool,
    kdf: Pbkdf2Paso,
}

#[derive(Default)]
struct XDrives {
    list: Vec<XDrive>,
    actions: Vec<DriveAction>,
    result: Option<Result<Option<String>, FsError>>,
    job: Option<UnlockJob>,
    unlocked: Option<Result<(), KeyError>>,
}

/// Línea de `unidades.cfg`: la serie sin saltos de línea ni `|`.
fn units_line(serie: &str, start: u64) -> String {
    let s: String = serie.trim().chars().map(|c| if c.is_control() || c == '|' { '_' } else { c }).collect();
    format!("{s}|{start}")
}

impl Default for Files {
    /// Unidad C: en memoria con las carpetas iniciales.
    fn default() -> Files {
        Files::memory(NOTICE_NO_DISK, None, DateTime::default())
    }
}

impl Files {
    fn memory(notice: &'static str, pending: Option<Box<dyn BlockDev>>, now: DateTime) -> Files {
        Files { fs: new_memory(now), persistent: false, pending, notice: Some(notice), generation: 0, clip: None, undo: None, user: Some((String::from(DEFAULT_USER), true)), disk: DiskState::default(), hc: Home::default(), xd: XDrives::default(), roots: None, hora: Default::default() }
    }

    /// Prepara la unidad C: a partir del disco encontrado al arrancar:
    /// - con NFFS → se monta (persistente);
    /// - en blanco → queda pendiente de que el usuario acepte formatearlo; mientras, C: en memoria;
    /// - con otro contenido, o sin disco → se ignora; C: en memoria.
    ///
    /// `Files::default()` es una unidad C: en memoria. Toda unidad nace con `Usuarios` y `Sistema`
    /// (la de memoria, además con la carpeta de `DEFAULT_USER` y su sesión abierta). Al montar un
    /// disco: crea lo que falte, vacía `Sistema\temp`, prepara la migración si el disco es antiguo
    /// y, si ya hay cuentas, la completa.
    pub fn probe(dev: Option<Box<dyn BlockDev>>, now: DateTime) -> Files {
        let Some(mut dev) = dev else {
            return Files::memory(NOTICE_NO_DISK, None, now);
        };
        // Disco cifrado (cabecera íntegra en el último bloque): no se toca hasta `unlock_disk`. Si el
        // NFFS ocupa todo el dispositivo, el último bloque es suyo y no una cabecera.
        if let Some(h) = cryptdev::detect(&mut *dev) {
            if Nffs::volume_blocks_of(&mut *dev) != Some(dev.blocks()) {
                let mut f = Files::memory(NOTICE_LOCKED, None, now);
                f.disk.locked = Some((dev, h));
                return f;
            }
        }
        if Nffs::is_formatted(&mut *dev) {
            // Con NFFS dañado no se formatea nunca: se deja el disco como está.
            return match Nffs::mount(dev) {
                Ok(fs) => {
                    let mut f = Files { fs, persistent: true, pending: None, notice: None, generation: 0, clip: None, undo: None, user: None, disk: DiskState::default(), hc: Home::default(), xd: XDrives::default(), roots: None, hora: Default::default() };
                    f.startup(now);
                    f
                }
                Err(_) => Files::memory(NOTICE_DAMAGED, None, now),
            };
        }
        if Nffs::is_blank(&mut *dev) {
            Files::memory(NOTICE_NEW, Some(dev), now)
        } else {
            Files::memory(NOTICE_IGNORED, None, now)
        }
    }

    /// Para herramientas (nfdisk): trabaja sobre un volumen NFFS ya montado, tal cual: sin arranque
    /// (no crea ni migra nada), sin sesión (hay que usar `login`) y con las reglas de siempre.
    pub fn on_volume(fs: Nffs) -> Files {
        Files { fs, persistent: true, pending: None, notice: None, generation: 0, clip: None, undo: None, user: None, disk: DiskState::default(), hc: Home::default(), xd: XDrives::default(), roots: None, hora: Default::default() }
    }

    /// ¿Se guarda en un disco de verdad?
    pub fn persistent(&self) -> bool {
        self.persistent
    }

    /// ¿Hay un disco en blanco esperando confirmación para formatearse?
    pub fn pending_format(&self) -> bool {
        self.pending.is_some()
    }

    /// Formatea el disco pendiente y pasa a usarlo, copiando lo que hubiera en la unidad en memoria.
    /// Después NO hay sesión (`user()` es `None`) y `needs_setup()` es `true`: la carpeta de
    /// `DEFAULT_USER` que venía de la memoria la adopta la primera cuenta que se cree.
    pub fn format_pending(&mut self, now: DateTime) -> Result<(), FsError> {
        let dev = self.pending.take().ok_or(FsError::Invalid)?;
        // Se dejan sin usar los dos últimos bloques: sitio para el cifrado total.
        let total = dev.blocks().saturating_sub(cryptdev::RESERVED);
        self.format_on(dev, total, now)
    }

    /// Como `format_pending`, pero el disco nace con el cifrado total ya puesto (sin conversión):
    /// se escribe la cabecera y el sistema de archivos se crea ya cifrado. Devuelve el código de
    /// recuperación del disco. Para el asistente del primer arranque.
    pub fn format_pending_encrypted(&mut self, password: &str, pool: &mut Pool, now: DateTime) -> Result<String, KeyError> {
        if password.chars().count() < accounts::MIN_PASSWORD {
            return Err(KeyError::Weak);
        }
        if !pool.ready() {
            return Err(KeyError::NotReady);
        }
        let mut dev = self.pending.take().ok_or(KeyError::Unavailable)?;
        if dev.blocks() < 66 {
            self.pending = Some(dev);
            return Err(KeyError::NoRoom);
        }
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
        if let Err(e) = cryptdev::write_header(&mut *dev, &hdr) {
            wipe(&mut master);
            self.pending = Some(dev);
            return Err(cryptdev::disk_err(e));
        }
        let core = Rc::new(RefCell::new(Core::new(dev, hdr, &master)));
        wipe(&mut master);
        let crypt: Box<dyn BlockDev> = Box::new(CryptDev::new(core.clone()));
        let total = crypt.blocks();
        self.format_on(crypt, total, now)?;
        self.disk.core = Some(core);
        self.log("Disco formateado con cifrado total", now);
        Ok(code)
    }

    /// Crea el sistema de archivos en `dev` y pasa a usarlo (ver `format_pending`).
    fn format_on(&mut self, dev: Box<dyn BlockDev>, total: u64, now: DateTime) -> Result<(), FsError> {
        let mut fs = Nffs::format_with(dev, total, now)?;
        ensure_structure(&mut fs, now);
        fs.write(rel(VERSION_FILE), b"2\n", now)?;
        // El estado va antes de la copia: si se corta, lo que llegó al disco no queda sin dueño.
        fs.write(rel(STATE_FILE), b"memoria\n", now)?;
        copy_all(&mut self.fs, &mut fs)?;
        self.fs = fs;
        self.persistent = true;
        self.notice = None;
        self.user = None;
        self.clip = None;
        self.undo = None;
        self.generation += 1;
        self.log("Disco formateado; lo de la memoria queda pendiente de la primera cuenta", now);
        Ok(())
    }

    /// El usuario no quiere formatear: se descarta el disco pendiente.
    pub fn decline_format(&mut self) {
        if self.pending.take().is_some() {
            self.notice = Some(NOTICE_IGNORED);
        }
    }

    /// Aviso para el usuario cuando la unidad no es persistente; `None` si todo es normal.
    pub fn notice(&self) -> Option<&'static str> {
        self.notice
    }

    /// Cambia cada vez que se modifica algo; sirve para saber cuándo refrescar una vista.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Como `list`, pero sin lo oculto o de sistema salvo que el usuario marcara «Elementos
    /// ocultos». Es lo que enseñan el Explorador, el escritorio y los diálogos; copiar, mover y
    /// borrar usan `list` para no dejarse nada.
    pub fn list_view(&mut self, dir: &str) -> Result<Vec<Entry>, FsError> {
        let mut v = self.list(dir)?;
        if v.iter().any(|e| e.hidden) && !self.show_hidden() {
            v.retain(|e| !e.hidden);
        }
        Ok(v)
    }

    /// Entradas de una carpeta: carpetas primero, luego por nombre sin distinguir mayúsculas.
    /// No incluye la Papelera antigua de la raíz ni, dentro de la carpeta de un usuario, lo que
    /// empieza por punto (`.config`, `.papelera`). En `Usuarios`, un usuario normal solo ve la suya.
    pub fn list(&mut self, dir: &str) -> Result<Vec<Entry>, FsError> {
        let c = canon(dir)?;
        if let Some(l) = ext(&c) {
            let mut v = self.vol(l)?.list(rel(&c))?;
            self.a_local(l, &mut v);
            v.sort_by_cached_key(|e| (!e.dir, lower(&e.name)));
            return Ok(v);
        }
        self.guard(rel(&c), false)?;
        let mut v = self.r_list(rel(&c))?;
        let depth = comps(rel(&c)).len();
        if depth == 0 {
            v.retain(|e| !same(&e.name, LEGACY_STORE));
        } else if same(first(rel(&c)), USERS) {
            if depth == 1 && !self.is_admin() {
                let me = self.user().map(String::from);
                v.retain(|e| me.as_deref().is_some_and(|m| same(m, &e.name)));
            } else if depth == 2 {
                v.retain(|e| !e.name.starts_with('.'));
            }
        }
        v.sort_by_cached_key(|e| (!e.dir, e.name.chars().flat_map(char::to_lowercase).collect::<String>()));
        Ok(v)
    }

    pub fn stat(&mut self, path: &str) -> Result<Entry, FsError> {
        let c = canon(path)?;
        if let Some(l) = ext(&c) {
            return self.v_stat(l, rel(&c));
        }
        self.guard(rel(&c), false)?;
        self.r_stat(rel(&c))
    }

    pub fn exists(&mut self, path: &str) -> bool {
        self.stat(path).is_ok()
    }

    pub fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        let c = canon(path)?;
        if let Some(l) = ext(&c) {
            return if rel(&c).is_empty() { Err(FsError::IsDir) } else { self.vol(l)?.read(rel(&c)) };
        }
        self.guard(rel(&c), false)?;
        self.r_read(rel(&c))
    }

    /// Crea o sustituye un archivo (atómico).
    pub fn write(&mut self, path: &str, data: &[u8], now: DateTime) -> Result<(), FsError> {
        let c = canon(path)?;
        if rel(&c).is_empty() {
            return Err(FsError::IsDir);
        }
        if let Some(l) = ext(&c) {
            self.generation += 1;
            return self.vol(l)?.write(rel(&c), data, now);
        }
        self.guard(rel(&c), true)?;
        self.generation += 1;
        self.r_write(rel(&c), data, now)
    }

    pub fn mkdir(&mut self, path: &str, now: DateTime) -> Result<(), FsError> {
        let c = canon(path)?;
        if rel(&c).is_empty() {
            return Err(FsError::Exists);
        }
        if let Some(l) = ext(&c) {
            self.generation += 1;
            return self.vol(l)?.mkdir(rel(&c), now);
        }
        self.guard(rel(&c), true)?;
        self.generation += 1;
        self.r_mkdir(rel(&c), now)
    }

    /// Mueve o renombra. Falla con `Exists` si el destino existe. En las unidades extraíbles solo
    /// dentro de la misma unidad (`Invalid` entre dos: para eso está `transfer`, que copia y borra).
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        let (f, t) = (canon(from)?, canon(to)?);
        if ext(&f).is_some() || ext(&t).is_some() {
            return self.rename_x(&f, &t);
        }
        if protected(rel(&f)) {
            return Err(FsError::Invalid);
        }
        self.guard(rel(&f), true)?;
        if rel(&t).is_empty() {
            return Err(FsError::Exists);
        }
        self.guard(rel(&t), true)?;
        self.generation += 1;
        self.r_rename(rel(&f), rel(&t))
    }

    /// Copia un archivo o una carpeta entera. Falla con `Exists` si el destino existe.
    pub fn copy(&mut self, from: &str, to: &str, now: DateTime) -> Result<(), FsError> {
        let (f, t) = (canon(from)?, canon(to)?);
        if ext(&f).is_some() || ext(&t).is_some() {
            return self.copy_x(&f, &t, now);
        }
        let (f, t) = (rel(&f), rel(&t));
        self.guard(f, false)?;
        self.guard(t, true)?;
        let src = self.r_stat(f)?;
        let inside = f.is_empty() || {
            let (lf, lt) = (lower(f), lower(t));
            lt.len() > lf.len() && lt.starts_with(&lf) && lt.as_bytes()[lf.len()] == b'\\'
        };
        if src.dir && inside {
            return Err(FsError::Invalid);
        }
        if t.is_empty() || self.r_stat(t).is_ok() {
            return Err(FsError::Exists);
        }
        self.cross_ok(f, t)?;
        self.generation += 1;
        let r = self.copy_tree(f, t, now, false);
        if r.is_err() {
            let _ = self.rm_tree(t);
        }
        r
    }

    /// Copia el árbol `from` en `to` (rutas relativas; cada lado pasa por su cifrado si lo tiene).
    /// `keep`: conserva las fechas del origen (mover entre carpeta cifrada y fuera de ella).
    fn copy_tree(&mut self, from: &str, to: &str, now: DateTime, keep: bool) -> Result<(), FsError> {
        let mut work = vec![(String::from(from), String::from(to))];
        while let Some((s, d)) = work.pop() {
            let st = self.r_stat(&s)?;
            let when = if keep { st.modified } else { now };
            if st.dir {
                self.r_mkdir(&d, when)?;
                for e in self.r_list(&s)? {
                    work.push((join_rel(&s, &e.name), join_rel(&d, &e.name)));
                }
            } else {
                let data = self.r_read(&s)?;
                self.r_write(&d, &data, when)?;
            }
        }
        Ok(())
    }

    /// Borra `rel` y todo lo que contiene (sin protecciones; las comprueban quienes llaman). En una
    /// carpeta cifrada borra también lo que no se ve (entradas en claro de más o con nombre alterado).
    fn rm_tree(&mut self, r: &str) -> Result<(), FsError> {
        if !self.enc_route(r, false)? {
            return self.raw_rm_tree(r);
        }
        let res = (|| {
            let f = self.find_enc(r)?;
            let i = f.item.ok_or(FsError::NotFound)?;
            self.raw_rm_tree(&join_rel(&f.parent, &i.raw))?;
            match &i.alt {
                Some(a) => self.raw_rm_tree(&join_rel(&f.parent, a)),
                None => Ok(()),
            }
        })();
        self.hc.cache.clear();
        res
    }

    fn raw_rm_tree(&mut self, rel: &str) -> Result<(), FsError> {
        let mut order = Vec::new();
        let mut stack = vec![String::from(rel)];
        while let Some(p) = stack.pop() {
            if self.fs.stat(&p)?.dir {
                for e in self.fs.list(&p)? {
                    stack.push(join_rel(&p, &e.name));
                }
            }
            order.push(p);
        }
        // Los hijos van siempre detrás de su carpeta: al revés se borran antes que ella.
        for p in order.iter().rev() {
            self.fs.remove(p)?;
        }
        Ok(())
    }

    /// Borrado definitivo; las carpetas, con todo su contenido.
    pub fn delete(&mut self, path: &str) -> Result<(), FsError> {
        let c = canon(path)?;
        if is_protected(&c) {
            return Err(FsError::Invalid);
        }
        if let Some(l) = ext(&c) {
            self.generation += 1;
            return self.v_rm_tree(l, rel(&c));
        }
        self.guard(rel(&c), true)?;
        self.generation += 1;
        self.rm_tree(rel(&c))
    }

    fn free_rel(&mut self, dir: &str, name: &str) -> String {
        self.free_in(None, dir, name)
    }

    /// Nombre libre en `dir` (relativo) de C: (`l` = `None`) o de la unidad extraíble `l`.
    fn free_in(&mut self, l: Option<char>, dir: &str, name: &str) -> String {
        let taken = |s: &mut Files, n: &str| match l {
            Some(l) => s.v_stat(l, &join_rel(dir, n)).is_ok(),
            None => s.r_stat(&join_rel(dir, n)).is_ok(),
        };
        if !taken(self, name) {
            return String::from(name);
        }
        let (stem, ext) = match name.rfind('.') {
            Some(i) if i > 0 => name.split_at(i),
            _ => (name, ""),
        };
        let mut n = 2;
        loop {
            let cand = format!("{stem} ({n}){ext}");
            if !taken(self, &cand) {
                return cand;
            }
            n += 1;
        }
    }

    /// Nombre libre en `dir` parecido a `name`: "nota.txt", "nota (2).txt", "nota (3).txt"...
    pub fn free_name(&mut self, dir: &str, name: &str) -> String {
        match canon(dir) {
            Ok(c) => self.free_in(ext(&c), rel(&c), name),
            Err(_) => String::from(name),
        }
    }

    // ---- Papelera (una por usuario, en `<home>\.papelera`) ----

    /// Almacén de la Papelera del usuario de la sesión (relativo a la raíz); `None` sin sesión.
    fn store(&self) -> Option<String> {
        self.user().map(|u| join_rel(&join_rel(USERS, u), HOME_TRASH))
    }

    fn read_index_at(&mut self, store: &str) -> Vec<(String, DateTime, String)> {
        let text = self.r_read(&join_rel(store, INDEX_NAME)).ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
        text.lines()
            .filter_map(|l| {
                let mut p = l.splitn(3, '|');
                let (id, date, orig) = (p.next()?, p.next()?, p.next()?);
                let mut d = date.split('-').map(|x| x.parse::<u16>().ok());
                let mut n = || d.next().flatten();
                let dt = DateTime {
                    year: n()?,
                    month: n()? as u8,
                    day: n()? as u8,
                    hour: n()? as u8,
                    minute: n()? as u8,
                    second: n()? as u8,
                };
                Some((String::from(id), dt, String::from(orig)))
            })
            .collect()
    }

    fn write_index_at(&mut self, store: &str, idx: &[(String, DateTime, String)], now: DateTime) -> Result<(), FsError> {
        let mut s = String::new();
        for (id, d, orig) in idx {
            s.push_str(&format!(
                "{id}|{}-{}-{}-{}-{}-{}|{orig}\n",
                d.year, d.month, d.day, d.hour, d.minute, d.second
            ));
        }
        if self.r_stat(store).is_err() {
            self.r_mkdir(store, now)?;
        }
        self.r_write(&join_rel(store, INDEX_NAME), s.as_bytes(), now)
    }

    /// Siguiente identificador libre de un almacén (mayor del índice y de los elementos, más uno).
    fn next_id(&mut self, store: &str, idx: &[(String, DateTime, String)]) -> String {
        let mut next = idx.iter().filter_map(|(id, _, _)| id.parse::<u64>().ok()).max().unwrap_or(0);
        for e in self.r_list(store).unwrap_or_default() {
            next = next.max(e.name.parse::<u64>().unwrap_or(0));
        }
        format!("{:08}", next + 1)
    }

    /// Envía un archivo o carpeta a la Papelera del usuario de la sesión (también lo que está
    /// fuera de su carpeta, p. ej. en `C:\` si es administrador). Las unidades extraíbles no tienen
    /// Papelera: allí borra definitivamente (como Windows).
    pub fn trash(&mut self, path: &str, now: DateTime) -> Result<(), FsError> {
        let c = canon(path)?;
        if ext(&c).is_some() {
            return self.delete(&c);
        }
        let r = rel(&c);
        if protected(r) {
            return Err(FsError::Invalid);
        }
        self.guard(r, true)?;
        // Lo que ya está en una Papelera no se vuelve a enviar a ella.
        if comps(r).iter().any(|x| same(x, HOME_TRASH)) {
            return Err(FsError::Invalid);
        }
        let store = self.store().ok_or(FsError::Invalid)?;
        self.r_stat(r)?;
        self.generation += 1;
        let mut idx = self.read_index_at(&store);
        let id = self.next_id(&store, &idx);
        idx.push((id.clone(), now, c.clone()));
        self.write_index_at(&store, &idx, now)?;
        if let Err(e) = self.r_rename(r, &join_rel(&store, &id)) {
            idx.pop();
            let _ = self.write_index_at(&store, &idx, now);
            return Err(e);
        }
        Ok(())
    }

    /// Contenido de la Papelera del usuario de la sesión, lo más reciente primero (vacío sin sesión).
    pub fn trash_list(&mut self) -> Result<Vec<TrashItem>, FsError> {
        let Some(store) = self.store() else { return Ok(Vec::new()) };
        let mut out = Vec::new();
        for (id, deleted, original) in self.read_index_at(&store) {
            if let Ok(e) = self.r_stat(&join_rel(&store, &id)) {
                out.push(TrashItem {
                    name: String::from(file_name(&original)),
                    id,
                    original,
                    dir: e.dir,
                    size: e.size,
                    deleted,
                });
            }
        }
        out.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(out)
    }

    /// Devuelve un elemento a su sitio (con otro nombre libre si el original está ocupado,
    /// recreando la carpeta original si ya no existe). Devuelve la ruta restaurada.
    pub fn restore(&mut self, id: &str) -> Result<String, FsError> {
        let store = self.store().ok_or(FsError::Invalid)?;
        let mut idx = self.read_index_at(&store);
        let pos = idx.iter().position(|(i, _, _)| i == id).ok_or(FsError::NotFound)?;
        let item = join_rel(&store, id);
        self.r_stat(&item)?;
        let (deleted, original) = (idx[pos].1, idx[pos].2.clone());
        let r = original.get(3..).unwrap_or("");
        // Un índice manipulado no puede sacar nada a un sitio donde el usuario no puede escribir.
        self.guard(r, true)?;
        self.generation += 1;
        let (dir, name) = r.rsplit_once('\\').unwrap_or(("", r));
        let mut cur = String::new();
        for c in dir.split('\\').filter(|c| !c.is_empty()) {
            cur = join_rel(&cur, c);
            match self.r_stat(&cur) {
                Ok(e) if !e.dir => return Err(FsError::NotDir),
                Ok(_) => {}
                Err(FsError::NotFound) => self.r_mkdir(&cur, deleted)?,
                Err(e) => return Err(e),
            }
        }
        let dest = join_rel(dir, &self.free_rel(dir, name));
        self.r_rename(&item, &dest)?;
        idx.remove(pos);
        self.write_index_at(&store, &idx, deleted)?;
        Ok(format!("C:\\{dest}"))
    }

    pub fn empty_trash(&mut self) -> Result<(), FsError> {
        let store = self.store().ok_or(FsError::Invalid)?;
        self.generation += 1;
        for e in self.r_list(&store)? {
            if e.name != INDEX_NAME {
                self.rm_tree(&join_rel(&store, &e.name))?;
            }
        }
        self.write_index_at(&store, &[], DateTime::default())
    }

    /// (bytes totales, bytes libres) de la unidad C:.
    pub fn space(&mut self) -> (u64, u64) {
        self.fs.space()
    }

    // ---- Portapapeles y deshacer (compartidos por el Explorador y el escritorio) ----

    /// Primera ruta del portapapeles de archivos y si es "cortar".
    pub fn clip(&self) -> Option<(&str, bool)> {
        self.clip.as_ref().and_then(|(p, cut)| p.first().map(|p| (p.as_str(), *cut)))
    }

    /// Todas las rutas del portapapeles (vacío si no hay nada) y si es "cortar".
    pub fn clip_paths(&self) -> (&[String], bool) {
        match &self.clip {
            Some((p, cut)) => (p.as_slice(), *cut),
            None => (&[], false),
        }
    }

    pub fn set_clip(&mut self, path: &str, cut: bool) {
        self.clip = Some((vec![String::from(path)], cut));
    }

    /// Como `set_clip` con varias rutas. Con una lista vacía, vacía el portapapeles.
    pub fn set_clip_many(&mut self, paths: &[String], cut: bool) {
        self.clip = if paths.is_empty() { None } else { Some((paths.to_vec(), cut)) };
    }

    /// Pega el portapapeles en `dir` (con otro nombre libre si ya existe). Devuelve la primera ruta
    /// nueva. Cortar y pegar en la misma carpeta no hace nada. Tras pegar un "cortar" se vacía.
    pub fn paste(&mut self, dir: &str, now: DateTime) -> Result<String, FsError> {
        self.paste_all(dir, now)?.into_iter().next().ok_or(FsError::NotFound)
    }

    /// Como `paste`, pero devuelve todas las rutas resultantes. Se deshace como un solo lote.
    pub fn paste_all(&mut self, dir: &str, now: DateTime) -> Result<Vec<String>, FsError> {
        let (src, cut) = self.clip.clone().ok_or(FsError::NotFound)?;
        if cut {
            self.clip = None;
        }
        self.transfer(&src, dir, cut, now)
    }

    /// Mueve (`mv`) o copia `paths` a la carpeta `dir`, con nombre libre si ya existe; se deshace
    /// como un solo lote ("Mover" o "Copiar"). Mover a la carpeta donde ya está no hace nada.
    /// Si algo falla se para ahí: lo ya hecho se queda y se puede deshacer.
    pub fn transfer(&mut self, paths: &[String], dir: &str, mv: bool, now: DateTime) -> Result<Vec<String>, FsError> {
        let mut out = Vec::new();
        let mut done = Vec::new();
        let mut err = None;
        for src in paths {
            if mv && parent(src).is_some_and(|p| lower(&p) == lower(dir)) {
                out.push(src.clone());
                continue;
            }
            let dest = join(dir, &self.free_name(dir, file_name(src)));
            let cross = mv && letter(src) != letter(&dest);
            let r = if cross { self.move_across(src, &dest, now) } else if mv { self.rename(src, &dest) } else { self.copy(src, &dest, now) };
            if let Err(e) = r {
                err = Some(e);
                break;
            }
            // shortcut: mover entre unidades no se puede deshacer; al nacer de copiar y borrar, volver sería otra copia larga. Si hace falta, `Undo::MoveAcross`.
            if !cross {
                done.push(if mv { Undo::Move { from: src.clone(), to: dest.clone() } } else { Undo::Created(dest.clone()) });
            }
            out.push(dest);
        }
        if !done.is_empty() {
            self.undo = Some((done, if mv { "Mover" } else { "Copiar" }));
        }
        err.map_or(Ok(out), Err)
    }

    /// Como `rename`, pero se puede deshacer ("Cambiar nombre").
    pub fn user_rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        self.rename(from, to)?;
        self.undo = Some((vec![Undo::Move { from: String::from(from), to: String::from(to) }], "Cambiar nombre"));
        Ok(())
    }

    /// Como `trash`, pero se puede deshacer ("Eliminar").
    pub fn user_trash(&mut self, path: &str, now: DateTime) -> Result<(), FsError> {
        self.user_trash_many(&[String::from(path)], now)
    }

    /// Envía varias rutas a la Papelera; se deshace como un solo lote ("Eliminar").
    /// Si algo falla se para ahí: lo ya hecho se queda y se puede deshacer.
    pub fn user_trash_many(&mut self, paths: &[String], now: DateTime) -> Result<(), FsError> {
        let mut done = Vec::new();
        let mut err = None;
        for p in paths {
            if let Err(e) = self.trash(p, now) {
                err = Some(e);
                break;
            }
            // shortcut: lo borrado de una unidad extraíble no va a la Papelera y no se deshace.
            if letter(p) == 'C' {
                done.push(Undo::Trashed(p.clone()));
            }
        }
        if !done.is_empty() {
            self.undo = Some((done, "Eliminar"));
        }
        err.map_or(Ok(()), Err)
    }

    /// Crea una carpeta (`dir == true`) o un archivo vacío; se puede deshacer ("Nuevo").
    pub fn user_create(&mut self, path: &str, dir: bool, now: DateTime) -> Result<(), FsError> {
        if dir {
            self.mkdir(path, now)?;
        } else if self.exists(path) {
            return Err(FsError::Exists);
        } else {
            self.write(path, &[], now)?;
        }
        self.undo = Some((vec![Undo::Created(String::from(path))], "Nuevo"));
        Ok(())
    }

    /// Nombre de la última operación que se puede deshacer: "Cambiar nombre", "Mover", "Copiar",
    /// "Nuevo" o "Eliminar". Para el texto "Deshacer <nombre>".
    pub fn undo_label(&self) -> Option<&'static str> {
        self.undo.as_ref().map(|u| u.1)
    }

    /// Deshace la última operación (un solo nivel, el lote entero). Lo creado o copiado va a la
    /// Papelera. Si un paso falla sigue con los demás y devuelve el primer error.
    pub fn undo(&mut self, now: DateTime) -> Result<(), FsError> {
        let (steps, _) = self.undo.take().ok_or(FsError::NotFound)?;
        let mut res = Ok(());
        for u in steps.into_iter().rev() {
            let r = match u {
                Undo::Move { from, to } => self.rename(&to, &from),
                Undo::Created(p) => self.trash(&p, now),
                Undo::Trashed(orig) => {
                    // trash_list da lo más reciente primero.
                    match self.trash_list()?.into_iter().find(|t| lower(&t.original) == lower(&orig)) {
                        Some(item) => self.restore(&item.id).map(|_| ()),
                        None => Err(FsError::NotFound),
                    }
                }
            };
            if res.is_ok() {
                res = r;
            }
        }
        res
    }

    // ---- Extensiones de nombre de archivo (Explorador y escritorio) ----

    /// `true` si el usuario pidió ver los elementos ocultos (`ocultos=si` en `explorer_prefs()`).
    pub fn show_hidden(&mut self) -> bool {
        let prefs = self.explorer_prefs();
        self.read(&prefs).is_ok_and(|d| String::from_utf8_lossy(&d).lines().any(|l| l.trim() == "ocultos=si"))
    }

    /// `false` si el usuario ocultó las extensiones (`extensiones=no` en `explorer_prefs()`).
    pub fn show_ext(&mut self) -> bool {
        let prefs = self.explorer_prefs();
        match self.read(&prefs) {
            Ok(d) => !String::from_utf8_lossy(&d).lines().any(|l| l.trim() == "extensiones=no"),
            Err(_) => true,
        }
    }
}

/// Nombre que se muestra: sin la extensión si `show_ext` es `false` (solo archivos; un nombre
/// que empieza por punto o no tiene punto se deja igual).
pub fn display_name(name: &str, is_dir: bool, show_ext: bool) -> &str {
    if show_ext || is_dir {
        return name;
    }
    match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    }
}

enum Undo {
    Move { from: String, to: String },
    Created(String),
    Trashed(String),
}

// ---- Cuentas, sesión y rutas del usuario (v0.5.0) ----
//
// CONTRATO. Semántica final:
// - Unidad en memoria (sin disco): hay una sesión abierta de `DEFAULT_USER`, administrador, y
//   no hay archivo de cuentas. `needs_setup()` es `false`.
// - Disco montado: sin sesión hasta `login`. Si no hay ninguna cuenta, `needs_setup()` es `true`
//   y `create_account` crea la primera, que siempre es administradora.
// - Las rutas del usuario salen SIEMPRE de `home`, `std_dir`, `config_file` y `explorer_prefs`.
// - Las funciones de este bloque funcionan sin sesión (acceden al disco sin pasar por los permisos).
impl Files {
    /// Cuentas guardadas (vacío si no hay archivo).
    pub fn accounts(&mut self) -> Vec<Account> {
        match self.fs.read(rel(ACCOUNTS_FILE)) {
            Ok(d) => accounts::parse(&String::from_utf8_lossy(&d)),
            Err(_) => Vec::new(),
        }
    }

    fn save_accounts(&mut self, list: &[Account], now: DateTime) -> Result<(), AccountError> {
        mk(&mut self.fs, rel(SYS_CONFIG_DIR), now);
        self.generation += 1;
        Ok(self.fs.write(rel(ACCOUNTS_FILE), accounts::serialize(list).as_bytes(), now)?)
    }

    /// Hay disco pero ninguna cuenta: toca el asistente inicial.
    pub fn needs_setup(&mut self) -> bool {
        self.persistent && self.accounts().is_empty()
    }

    /// Nombre de la cuenta con la sesión abierta.
    pub fn user(&self) -> Option<&str> {
        self.user.as_ref().map(|u| u.0.as_str())
    }

    pub fn is_admin(&self) -> bool {
        self.user.as_ref().is_some_and(|u| u.1)
    }

    /// Abre la sesión de `name` si la clave es correcta (vacía para una cuenta sin clave).
    pub fn login(&mut self, name: &str, password: &str) -> Result<(), AccountError> {
        let a = self.accounts().into_iter().find(|a| same(&a.name, name)).ok_or(AccountError::NotFound)?;
        if !a.verify(password) {
            return Err(AccountError::WrongPassword);
        }
        // Carpeta cifrada: la llave tiene que abrirse con esta misma clave; si no, no hay sesión.
        let mut hc = Home::default();
        match self.read_keyfile(&a.name) {
            None => {}
            Some(kf) => {
                let kf = kf?;
                let mut m = kf.open_pass(password).ok_or(AccountError::WrongPassword)?;
                hc.keys = Some(Box::new(Keys::new(&m)));
                hc.state = Some(kf.state);
                wipe(&mut m);
            }
        }
        // Por si la carpeta falta (p. ej. un corte al crear la cuenta).
        self.ensure_home_for(&a.name, DateTime::default());
        self.hc = hc;
        if self.hc.state.is_some_and(|s| s != HState::Encrypted) {
            self.hc.total = self.pending(&a.name);
        }
        self.user = Some((a.name, a.admin));
        self.clip = None;
        self.undo = None;
        self.generation += 1;
        Ok(())
    }

    /// Cierra la sesión (en la unidad en memoria no hace nada: no hay a qué volver).
    pub fn logout(&mut self) {
        if self.persistent {
            self.user = None;
            self.hc = Home::default();
            self.clip = None;
            self.undo = None;
            self.generation += 1;
        }
    }

    /// Comprueba la clave de la sesión abierta (para desbloquear).
    pub fn verify_password(&mut self, password: &str) -> bool {
        let Some(name) = self.user().map(String::from) else { return false };
        if !self.persistent {
            return password.is_empty();
        }
        if !self.accounts().iter().find(|a| same(&a.name, &name)).is_some_and(|a| a.verify(password)) {
            return false;
        }
        // Con la llave borrada (bloqueo), la clave buena la vuelve a abrir; si no abre, sigue cerrada.
        if self.hc.state.is_some() && self.hc.keys.is_none() {
            return self.reload_keys(&name, password);
        }
        true
    }

    /// ¿Tiene clave la cuenta de la sesión abierta?
    pub fn has_password(&mut self) -> bool {
        let Some(name) = self.user().map(String::from) else { return false };
        self.accounts().iter().find(|a| same(&a.name, &name)).is_some_and(|a| a.has_password())
    }

    /// Crea una cuenta y su carpeta completa. La primera es siempre administradora y no exige
    /// sesión; después solo puede crear cuentas un administrador. La primera cuenta adopta lo
    /// que hubiera pendiente de la estructura antigua o de la memoria (ver la cabecera).
    /// `salt`: `accounts::salt_from` con `Platform::entropy`.
    pub fn create_account(&mut self, name: &str, password: Option<&str>, admin: bool, salt: [u8; 16], now: DateTime) -> Result<(), AccountError> {
        let mut list = self.accounts();
        if !list.is_empty() && !self.is_admin() {
            return Err(AccountError::NotAdmin);
        }
        if !accounts::valid_user_name(name) {
            return Err(AccountError::BadName);
        }
        if list.iter().any(|a| same(&a.name, name)) {
            return Err(AccountError::Exists);
        }
        let first = list.is_empty();
        // Una carpeta conservada de una cuenta eliminada no se hereda: la cuenta nueva vería sus
        // archivos. (La primera cuenta sí puede coincidir con la carpeta de la unidad en memoria.)
        if !first && self.fs.stat(rel(&join(USERS_DIR, name))).is_ok() {
            return Err(AccountError::Exists);
        }
        let mut a = Account::new(name, admin || first);
        a.set_password(password, salt)?;
        // Primero la carpeta: un corte deja una carpeta sin cuenta, que es inofensiva.
        ensure_home(&mut self.fs, name, now);
        list.push(a);
        self.save_accounts(&list, now)?;
        self.log(&format!("Cuenta creada: {name}"), now);
        if first {
            self.run_adopt(now);
        }
        Ok(())
    }

    /// Elimina una cuenta (solo un administrador; no la de la sesión abierta ni la última
    /// administradora). `keep_files == false` borra su carpeta para siempre; `true` la conserva.
    pub fn delete_account(&mut self, name: &str, keep_files: bool, now: DateTime) -> Result<(), AccountError> {
        if !self.is_admin() {
            return Err(AccountError::NotAdmin);
        }
        if self.user().is_some_and(|u| same(u, name)) {
            return Err(AccountError::InUse);
        }
        let mut list = self.accounts();
        let i = list.iter().position(|a| same(&a.name, name)).ok_or(AccountError::NotFound)?;
        if list[i].admin && list.iter().filter(|a| a.admin).count() == 1 {
            return Err(AccountError::LastAdmin);
        }
        let gone = list.remove(i);
        self.save_accounts(&list, now)?;
        if !keep_files {
            self.rm_tree(&join_rel(USERS, &gone.name))?;
        }
        // Sin su archivo de llaves, una carpeta conservada y cifrada queda ilegible para siempre.
        let _ = self.fs.remove(&keys_path(&gone.name));
        self.log(&format!("Cuenta eliminada: {}{}", gone.name, if keep_files { " (carpeta conservada)" } else { " (con sus archivos)" }), now);
        Ok(())
    }

    /// Cambia el nombre de una cuenta y el de su carpeta (solo un administrador). Si es la de la
    /// sesión abierta, la sesión pasa a llamarse igual. Si se corta, el siguiente arranque lo termina.
    pub fn rename_account(&mut self, old: &str, new: &str, now: DateTime) -> Result<(), AccountError> {
        if !self.is_admin() {
            return Err(AccountError::NotAdmin);
        }
        if !accounts::valid_user_name(new) {
            return Err(AccountError::BadName);
        }
        let list = self.accounts();
        let old = list.iter().find(|a| same(&a.name, old)).ok_or(AccountError::NotFound)?.name.clone();
        if old == new {
            return Ok(());
        }
        if !same(&old, new) && (list.iter().any(|a| same(&a.name, new)) || self.fs.stat(&join_rel(USERS, new)).is_ok()) {
            return Err(AccountError::Exists);
        }
        self.fs.write(rel(RENAME_FILE), format!("{old}|{new}\n").as_bytes(), now)?;
        self.resume_rename(now)?;
        self.hc.cache.clear();
        if let Some(u) = self.user.as_mut().filter(|u| same(&u.0, &old)) {
            u.0 = String::from(new);
        }
        self.generation += 1;
        self.log(&format!("Cuenta renombrada: {old} -> {new}"), now);
        Ok(())
    }

    /// Hace administradora o normal a una cuenta (solo un administrador; debe quedar uno).
    pub fn set_admin(&mut self, name: &str, admin: bool, now: DateTime) -> Result<(), AccountError> {
        if !self.is_admin() {
            return Err(AccountError::NotAdmin);
        }
        let mut list = self.accounts();
        let i = list.iter().position(|a| same(&a.name, name)).ok_or(AccountError::NotFound)?;
        if list[i].admin && !admin && list.iter().filter(|a| a.admin).count() == 1 {
            return Err(AccountError::LastAdmin);
        }
        list[i].admin = admin;
        if let Some(u) = self.user.as_mut().filter(|u| same(&u.0, name)) {
            u.1 = admin;
        }
        self.save_accounts(&list, now)
    }

    /// Cambia, pone (`new = Some`) o quita (`new = None`) la clave de la sesión abierta.
    /// `old` es la clave actual (vacía si no tenía).
    pub fn set_password(&mut self, old: &str, new: Option<&str>, salt: [u8; 16], now: DateTime) -> Result<(), AccountError> {
        let name = self.user().map(String::from).ok_or(AccountError::NotFound)?;
        let mut list = self.accounts();
        let a = list.iter_mut().find(|a| same(&a.name, &name)).ok_or(AccountError::NotFound)?;
        if !a.verify(old) {
            return Err(AccountError::WrongPassword);
        }
        // Con la carpeta cifrada no se puede quitar la clave (la llave dejaría de estar protegida).
        let encrypted = self.hc.state.is_some();
        if encrypted && new.is_none() {
            return Err(AccountError::Encrypted);
        }
        a.set_password(new, salt)?;
        if !encrypted {
            return self.save_accounts(&list, now);
        }
        // La MISMA llave, envuelta con la clave nueva. Primero las dos juntas, luego la cuenta y
        // al final solo la nueva: un corte en medio nunca deja la carpeta sin una clave que abra.
        let kf = match self.read_keyfile(&name) {
            Some(r) => r?,
            None => return Err(AccountError::Fs(FsError::Corrupt)),
        };
        let mut master = kf.open_pass(old).ok_or(AccountError::WrongPassword)?;
        let keys = Keys::new(&master);
        let slot = keys::Slot::wrap(&master, new.unwrap_or("").as_bytes(), salt, homecrypt::iv_from_salt(&salt));
        wipe(&mut master);
        let current = kf.pass.iter().find(|s| s.unwrap(old.as_bytes()).is_some_and(|mut m| {
            wipe(&mut m);
            true
        }));
        let mut both: Vec<keys::Slot> = current.cloned().into_iter().collect();
        both.push(slot.clone());
        self.write_keyfile(&name, &KeyFile::new(kf.state, both, kf.recovery.clone()), &keys, now)?;
        self.save_accounts(&list, now)?;
        Ok(self.write_keyfile(&name, &KeyFile::new(kf.state, vec![slot], kf.recovery), &keys, now)?)
    }

    /// Carpeta del usuario de la sesión abierta: `C:\Usuarios\<nombre>`. Sin sesión (pantalla de
    /// inicio, asistente) devuelve `C:\Usuarios`, que no es de nadie: las rutas que salen de ella
    /// no existen y los permisos impiden escribir.
    pub fn home(&self) -> String {
        match self.user() {
            Some(u) => join(USERS_DIR, u),
            None => String::from(USERS_DIR),
        }
    }

    /// Carpeta estándar del usuario: `C:\Usuarios\<nombre>\Documentos`, etc.
    pub fn std_dir(&self, d: Std) -> String {
        join(&self.home(), d.name())
    }

    /// Configuración del usuario (`fondo=`, `enfasis=`, `iconos=`, `orden=`): `<home>\.config\config.txt`.
    pub fn config_file(&self) -> String {
        join(&join(&self.home(), HOME_CONFIG), "config.txt")
    }

    /// Preferencias del Explorador del usuario: `<home>\.config\explorador.txt`.
    pub fn explorer_prefs(&self) -> String {
        join(&join(&self.home(), HOME_CONFIG), "explorador.txt")
    }

    /// Instala o actualiza un programa del sistema en `C:\Programas\<folder>` (lo llama el núcleo
    /// al tener el disco listo, sin sesión; no pasa por los permisos). Solo escribe los archivos
    /// que cambian. Devuelve `true` si escribió algo.
    pub fn install_system_program(&mut self, folder: &str, items: &[(&str, &[u8])], now: DateTime) -> Result<bool, FsError> {
        if folder.is_empty() || folder.contains(['\\', '/', ':']) || folder.starts_with('.') {
            return Err(FsError::Invalid);
        }
        mk(&mut self.fs, PROGRAMS, now);
        let dir = join_rel(PROGRAMS, folder);
        mk(&mut self.fs, &dir, now);
        let mut changed = false;
        // Marca de programa del sistema (`programs::SYSTEM_MARK`): no se desinstala ni se reemplaza.
        let mark = join_rel(&dir, ".sistema");
        if self.fs.stat(&mark).is_err() {
            self.fs.write(&mark, b"", now)?;
        }
        for (name, data) in items {
            let p = join_rel(&dir, name);
            if self.fs.read(&p).ok().as_deref() != Some(*data) {
                self.fs.write(&p, data, now)?;
                changed = true;
            }
        }
        if changed {
            // El escritorio rehace sus iconos y el menú Inicio.
            self.generation += 1;
            self.log(&format!("Programa del sistema instalado: {folder}"), now);
        }
        Ok(changed)
    }

    /// Instala (o actualiza) ficheros que trae el sistema en `C:\Sistema\<sub>` (v0.12.4: las fuentes).
    /// Solo escribe los que cambian. Devuelve si cambió alguno.
    pub fn install_system_files(&mut self, sub: &str, items: &[(&str, &[u8])], now: DateTime) -> Result<bool, FsError> {
        if sub.is_empty() || sub.contains(['\\', '/', ':']) || sub.starts_with('.') {
            return Err(FsError::Invalid);
        }
        let dir = join_rel(SYSTEM, sub);
        mk(&mut self.fs, &dir, now);
        let mut changed = false;
        for (name, data) in items {
            let p = join_rel(&dir, name);
            if self.fs.read(&p).ok().as_deref() != Some(*data) {
                self.fs.write(&p, data, now)?;
                changed = true;
            }
        }
        Ok(changed)
    }
}

// ---- Permisos, registro, arranque y migración (internos salvo `log`) ----
impl Files {
    /// Permisos de la sesión sobre una ruta relativa canónica (ver la cabecera). `Denied` si no.
    fn guard(&self, r: &str, write: bool) -> Result<(), FsError> {
        let c = comps(r);
        let Some(&top) = c.first() else { return Ok(()) };
        let admin = self.is_admin();
        let denied = if same(top, USERS) {
            // Crear o tocar algo directamente en `Usuarios` o en la carpeta de una cuenta es cosa del sistema.
            let mine = c.len() >= 2 && self.user().is_some_and(|u| same(u, c[1]));
            (c.len() <= 2 && write) || (c.len() >= 2 && !mine && !admin)
        } else if same(top, SYSTEM) {
            let accounts_file = c.len() >= 3 && same(c[1], "config") && same(c[2], "cuentas");
            // Las llaves de las carpetas cifradas solo las toca el sistema, ni siquiera un administrador.
            let keys_dir = c.len() >= 3 && same(c[1], "config") && same(c[2], "llaves");
            keys_dir || (!admin && (write || accounts_file))
        } else if same(top, PROGRAMS) {
            write && !admin
        } else {
            write && self.user.is_none()
        };
        if denied {
            Err(FsError::Denied)
        } else {
            Ok(())
        }
    }

    /// Añade una línea con fecha a `C:\Sistema\logs\sistema.log` (se queda en ~64 KB: lo más viejo
    /// se descarta). Nunca pases claves. Funciona sin sesión y no puede fallar hacia fuera.
    pub fn log(&mut self, text: &str, now: DateTime) {
        let text: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        let line = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {text}\n",
            now.year, now.month, now.day, now.hour, now.minute, now.second
        );
        let mut data = self.fs.read(rel(LOG_FILE)).unwrap_or_default();
        data.extend_from_slice(line.as_bytes());
        if data.len() > LOG_MAX {
            let cut = data.len() - LOG_MAX;
            let start = data[cut..].iter().position(|&b| b == b'\n').map_or(data.len(), |i| cut + i + 1);
            data.drain(..start);
        }
        mk(&mut self.fs, rel(SYS_LOGS_DIR), now);
        let _ = self.fs.write(rel(LOG_FILE), &data, now);
    }

    /// v0.12.1: permisos de red de los programas (`netperm`). Los lee y escribe el sistema (aviso y
    /// Configuración) sin pasar por los permisos de la cuenta; los programas no llegan aquí.
    pub fn net_perms(&mut self) -> Vec<crate::netperm::Perm> {
        crate::netperm::parse(&self.fs.read(rel(crate::netperm::PERMS_FILE)).unwrap_or_default())
    }
    pub fn set_net_perms(&mut self, perms: &[crate::netperm::Perm], now: DateTime) -> Result<(), FsError> {
        mk(&mut self.fs, rel(SYS_CONFIG_DIR), now);
        let r = self.fs.write(rel(crate::netperm::PERMS_FILE), &crate::netperm::render(perms), now);
        if r.is_ok() {
            self.generation += 1;
        }
        r
    }

    /// v0.12.3: zona horaria y «ajustar sola» (`hora`). Cualquiera puede verlos; se leen una vez y se recuerdan.
    pub fn time_cfg(&mut self) -> crate::hora::Ajuste {
        if self.hora.ajuste.is_none() {
            self.hora.ajuste = Some(crate::hora::parse(&self.fs.read(rel(crate::hora::FILE)).unwrap_or_default()));
        }
        self.hora.ajuste.unwrap_or_default()
    }

    /// Configuración → Fecha y hora: guarda la zona y el modo automático. Solo un administrador.
    pub fn set_time_cfg(&mut self, a: crate::hora::Ajuste, now: DateTime) -> Result<(), FsError> {
        if !self.is_admin() || a.zona as usize >= crate::hora::ZONAS.len() {
            return Err(FsError::Denied);
        }
        mk(&mut self.fs, rel(SYS_CONFIG_DIR), now);
        self.fs.write(rel(crate::hora::FILE), &crate::hora::render(&a), now)?;
        self.hora.ajuste = Some(a);
        self.generation += 1;
        self.log("Fecha y hora: ajustes cambiados en Configuración", now);
        Ok(())
    }

    /// Configuración → Fecha y hora: pide poner el reloj a `t` (hora local). Solo un administrador; lo hace el escritorio.
    pub fn set_clock(&mut self, t: DateTime) -> Result<(), FsError> {
        if !self.is_admin() {
            return Err(FsError::Denied);
        }
        if !crate::hora::valida(t) {
            return Err(FsError::Invalid);
        }
        self.hora.poner = Some(t);
        Ok(())
    }

    /// Configuración → «Sincronizar ahora». Solo un administrador; lo hace el escritorio con internet.
    pub fn time_sync(&mut self) -> Result<(), FsError> {
        if !self.is_admin() {
            return Err(FsError::Denied);
        }
        self.hora.sync_pedida = true;
        Ok(())
    }

    pub fn time_sync_info(&mut self) -> crate::hora::SyncInfo {
        self.hora.info
    }

    /// Lo que el escritorio tiene que hacer con la hora (lo consume): reloj a poner y si hay que sincronizar.
    pub fn hora_pendiente(&mut self) -> (Option<DateTime>, bool) {
        (self.hora.poner.take(), core::mem::take(&mut self.hora.sync_pedida))
    }

    pub fn hora_sync_info(&mut self, info: crate::hora::SyncInfo) {
        self.hora.info = info;
    }

    /// v0.12.2: raíces de confianza propias (DER). Las lee el sistema; cualquiera puede verlas.
    pub fn custom_roots(&mut self) -> Vec<Vec<u8>> {
        crate::raices::custom(&mut self.fs)
    }

    /// Almacén completo (Mozilla + propias) para validar HTTPS, sin releer el disco en cada conexión.
    pub fn root_store(&mut self) -> alloc::sync::Arc<crate::x509::RootStore> {
        crate::raices::store(&mut self.fs, &mut self.roots)
    }

    /// Configuración → Certificados: añade una raíz propia. Solo un administrador.
    pub fn add_root(&mut self, der: Vec<u8>, now: DateTime) -> Result<(), FsError> {
        if !self.is_admin() {
            return Err(FsError::Denied);
        }
        crate::raices::add(&mut self.fs, &mut self.roots, &der, now)?;
        self.generation += 1;
        self.log("Certificados: raíz añadida en Configuración", now);
        Ok(())
    }

    /// Configuración → Certificados: quita una raíz propia por su huella. Solo un administrador.
    pub fn remove_root(&mut self, sha: [u8; 32], now: DateTime) -> Result<(), FsError> {
        if !self.is_admin() {
            return Err(FsError::Denied);
        }
        crate::raices::remove(&mut self.fs, &mut self.roots, &sha)?;
        self.generation += 1;
        self.log("Certificados: raíz quitada en Configuración", now);
        Ok(())
    }

    /// Configuración → Red: cambia (o, con los dos a `None`, olvida) los permisos de un programa ya
    /// apuntado. Solo un administrador; un programa que no está es `NotFound`.
    pub fn set_net_perm(&mut self, hash: [u8; 32], out: Option<bool>, inn: Option<bool>, now: DateTime) -> Result<(), FsError> {
        if !self.is_admin() {
            return Err(FsError::Invalid);
        }
        let mut perms = self.net_perms();
        let i = perms.iter().position(|p| p.hash == hash).ok_or(FsError::NotFound)?;
        let name = alloc::format!("«{}»", perms[i].name);
        if out.is_none() && inn.is_none() {
            perms.remove(i);
        } else {
            perms[i].out = out;
            perms[i].inn = inn;
        }
        self.set_net_perms(&perms, now)?;
        self.log(&alloc::format!("Red: permisos de {name} cambiados en Configuración"), now);
        Ok(())
    }

    /// Al montar un disco: estructura, migración pendiente y limpieza. Nunca falla hacia fuera;
    /// lo que no pueda hacer ahora lo reintenta el siguiente arranque.
    fn startup(&mut self, now: DateTime) {
        ensure_structure(&mut self.fs, now);
        if self.fs.stat(rel(VERSION_FILE)).is_err() {
            // Primera vez con la estructura nueva: ¿era un disco antiguo?
            let legacy = legacy_items(&mut self.fs);
            if !legacy || self.fs.write(rel(STATE_FILE), b"antigua\n", now).is_ok() {
                if legacy {
                    self.log("Disco con la estructura antigua: su contenido pasará a la primera cuenta", now);
                }
                let _ = self.fs.write(rel(VERSION_FILE), b"2\n", now);
            }
        }
        if self.resume_rename(now).is_err() {
            self.log("No se pudo terminar de renombrar una cuenta; se reintentará", now);
        }
        self.stage_old(now);
        // `Sistema\temp` se vacía en cada arranque (solo su contenido).
        for e in self.fs.list(rel(SYS_TEMP_DIR)).unwrap_or_default() {
            let _ = self.rm_tree(&join_rel(rel(SYS_TEMP_DIR), &e.name));
        }
        for a in self.accounts() {
            self.ensure_home_for(&a.name, now);
        }
        self.run_adopt(now);
        self.log("Arranque", now);
    }

    fn state_has(&mut self, what: &str) -> bool {
        self.fs.read(rel(STATE_FILE)).is_ok_and(|b| String::from_utf8_lossy(&b).lines().any(|l| l.trim() == what))
    }

    /// Estructura antigua: lo suelto de `Sistema` (config.txt, explorador.txt...) pasa a un sitio de espera.
    fn stage_old(&mut self, now: DateTime) {
        if !self.state_has("antigua") {
            return;
        }
        for e in self.fs.list(SYSTEM).unwrap_or_default() {
            if SYS_SUBS.iter().any(|s| same(s, &e.name)) {
                continue;
            }
            mk(&mut self.fs, rel(OLD_DIR), now);
            let dest = join_rel(rel(OLD_DIR), &self.free_rel(rel(OLD_DIR), &e.name));
            match self.fs.rename(&join_rel(SYSTEM, &e.name), &dest) {
                Ok(()) => self.log(&format!("Movido a un lado: Sistema\\{} -> {}", e.name, dest), now),
                Err(_) => self.log(&format!("No se pudo mover Sistema\\{}", e.name), now),
            }
        }
    }

    /// Completa la adopción pendiente si ya hay cuentas; anota el resultado.
    fn run_adopt(&mut self, now: DateTime) {
        if let Err(e) = self.adopt(now) {
            self.log(&format!("Migración interrumpida: {}; continuará en el próximo arranque", e.message()), now);
        }
    }

    /// Mueve a la primera cuenta lo que quedó huérfano: la estructura antigua (carpetas, ajustes y
    /// Papelera) y/o lo que había en la memoria. Cada movimiento es un renombrado atómico y se
    /// puede repetir: tras un corte, continúa donde se quedó. Al terminar quita el archivo de estado.
    fn adopt(&mut self, now: DateTime) -> Result<(), FsError> {
        let (old, mem) = (self.state_has("antigua"), self.state_has("memoria"));
        if !old && !mem {
            return Ok(());
        }
        let Some(first) = self.accounts().into_iter().next() else { return Ok(()) };
        ensure_home(&mut self.fs, &first.name, now);
        let home = join_rel(USERS, &first.name);
        let abs_home = format!("C:\\{home}");
        let trash = join_rel(&home, HOME_TRASH);
        if old {
            let mut pairs = Vec::new();
            for n in LEGACY_DIRS {
                let to = join_rel(&home, n);
                self.merge(n, &to)?;
                self.log(&format!("Migrado: {n} -> {to}"), now);
                pairs.push((format!("C:\\{n}"), format!("C:\\{to}")));
            }
            for f in ["config.txt", "explorador.txt"] {
                let to = join_rel(&join_rel(&home, HOME_CONFIG), f);
                self.merge(&join_rel(rel(OLD_DIR), f), &to)?;
            }
            self.adopt_trash(LEGACY_STORE, &trash, &pairs, now)?;
            let _ = self.fs.remove(rel(OLD_DIR)); // solo si quedó vacía
            self.log("Migrado: ajustes y Papelera antiguos", now);
        }
        if mem && !same(DEFAULT_USER, &first.name) {
            let src = join_rel(USERS, DEFAULT_USER);
            if self.fs.stat(&src).is_ok() {
                for e in self.fs.list(&src)? {
                    if !same(&e.name, HOME_TRASH) {
                        self.merge(&join_rel(&src, &e.name), &join_rel(&home, &e.name))?;
                    }
                }
                let pairs = [(format!("C:\\{src}"), abs_home)];
                self.adopt_trash(&join_rel(&src, HOME_TRASH), &trash, &pairs, now)?;
                let _ = self.fs.remove(&src); // solo si quedó vacía
                self.log(&format!("Migrado: lo de la memoria -> {home}"), now);
            }
        }
        self.fs.remove(rel(STATE_FILE))?;
        self.log(&format!("Migración completa; propietaria: {}", first.name), now);
        self.generation += 1;
        Ok(())
    }

    /// Mueve `src` a `dst` sin pisar nada. Si `dst` no existe, se renombra. Si ambos son
    /// carpetas, el contenido de `src` pasa uno a uno a `dst` (con nombre libre si choca) y `src`
    /// se quita cuando queda vacía. Si `dst` existe y no son dos carpetas, `src` va a un nombre libre.
    fn merge(&mut self, src: &str, dst: &str) -> Result<(), FsError> {
        let s = match self.fs.stat(src) {
            Ok(s) => s,
            Err(FsError::NotFound) => return Ok(()), // ya hecho
            Err(e) => return Err(e),
        };
        let (dir, name) = dst.rsplit_once('\\').unwrap_or(("", dst));
        match self.fs.stat(dst) {
            Err(FsError::NotFound) => return self.fs.rename(src, dst),
            Err(e) => return Err(e),
            Ok(d) if !(s.dir && d.dir) => {
                let n = self.free_rel(dir, name);
                return self.fs.rename(src, &join_rel(dir, &n));
            }
            Ok(_) => {}
        }
        for e in self.fs.list(src)? {
            let n = self.free_rel(dst, &e.name);
            self.fs.rename(&join_rel(src, &e.name), &join_rel(dst, &n))?;
        }
        self.fs.remove(src)
    }

    /// Pasa los elementos de una Papelera (`src`) a otra (`dst`) con identificadores nuevos y las
    /// rutas originales reescritas según `pairs`, para que Restaurar siga funcionando. Por cada
    /// elemento: índice de destino, mover, quitar la línea de origen; un corte solo deja líneas
    /// sin elemento, que se ignoran.
    fn adopt_trash(&mut self, src: &str, dst: &str, pairs: &[(String, String)], now: DateTime) -> Result<(), FsError> {
        if self.fs.stat(src).is_err() {
            return Ok(());
        }
        let mut idx = self.read_index_at(src);
        while !idx.is_empty() {
            let (id, date, orig) = idx[0].clone();
            let item = join_rel(src, &id);
            if self.fs.stat(&item).is_ok() {
                let mut d = self.read_index_at(dst);
                let nid = self.next_id(dst, &d);
                d.push((nid.clone(), date, rewrite(&orig, pairs)));
                self.write_index_at(dst, &d, now)?;
                self.fs.rename(&item, &join_rel(dst, &nid))?;
            }
            idx.remove(0);
            self.write_index_at(src, &idx, now)?;
        }
        // Si no queda nada más que el índice, se quitan. Si hay elementos sin línea de índice, se dejan.
        if self.fs.list(src)?.iter().all(|e| e.name == INDEX_NAME) {
            let _ = self.fs.remove(&join_rel(src, INDEX_NAME));
            let _ = self.fs.remove(src);
        }
        Ok(())
    }

    /// Termina un cambio de nombre de cuenta empezado (carpeta y línea de cuentas), si lo hay.
    fn resume_rename(&mut self, now: DateTime) -> Result<(), FsError> {
        let Ok(raw) = self.fs.read(rel(RENAME_FILE)) else { return Ok(()) };
        let text = String::from_utf8_lossy(&raw).into_owned();
        if let Some((old, new)) = text.trim().split_once('|') {
            let (ro, rn) = (join_rel(USERS, old), join_rel(USERS, new));
            if self.fs.stat(&ro).is_ok() && (same(old, new) || self.fs.stat(&rn).is_err()) {
                self.fs.rename(&ro, &rn)?;
            }
            // El archivo de llaves va con la cuenta (si hay y el sitio está libre).
            if !same(old, new) && self.fs.stat(&keys_path(old)).is_ok() && self.fs.stat(&keys_path(new)).is_err() {
                self.fs.rename(&keys_path(old), &keys_path(new))?;
            }
            let mut list = self.accounts();
            if !list.iter().any(|a| same(&a.name, new) && !same(old, new)) {
                if let Some(a) = list.iter_mut().find(|a| same(&a.name, old)) {
                    a.name = String::from(new);
                    self.save_accounts(&list, now).map_err(|_| FsError::Disk)?;
                }
            }
        }
        self.fs.remove(rel(RENAME_FILE))
    }
}


// ---- Cifrado (v0.6.0). CONTRATO: firmas fijas; ver docs/PLAN-estructura.md, entrega 2. ----
//
// Cifrado total: todo el disco con una llave maestra que abre la CLAVE DEL DISCO (o su código
// de recuperación). Un disco cifrado se detecta al arrancar y queda "bloqueado" (C: en memoria
// mientras tanto) hasta `unlock_disk`. Activarlo o quitarlo convierte el disco en el sitio, por
// pasos (`convert_step` desde `tick`), y se reanuda solo si se corta.
//
// Cifrado de usuario: la carpeta de una cuenta con clave se cifra con su propia llave maestra,
// que abre la clave de la cuenta (`login`) o su código de recuperación. Las apps no notan nada:
// leen y escriben por las mismas rutas.
//
// Las funciones que generan llaves reciben un `Pool` que debe estar `ready()` (pantalla del
// ratón) y devuelven el código de recuperación, que se muestra UNA vez y no se guarda en claro.
impl Files {
    /// Hay un disco cifrado esperando su clave (mientras, C: está en memoria).
    pub fn disk_locked(&self) -> bool {
        self.disk.locked.is_some()
    }

    /// Abre el disco cifrado con la clave del disco (`recovery == false`) o con su código de
    /// recuperación. Tras abrirlo no hay sesión: sigue el inicio de sesión normal. Si hay una
    /// conversión a medias, queda en curso. Con una clave mala no se escribe nada.
    pub fn unlock_disk(&mut self, secret: &str, recovery: bool, now: DateTime) -> Result<(), KeyError> {
        let (dev, hdr) = self.disk.locked.take().ok_or(KeyError::NotEncrypted)?;
        let core = match cryptdev::open(dev, hdr, secret, recovery) {
            Ok(c) => Rc::new(RefCell::new(c)),
            Err((dev, hdr, e)) => {
                self.disk.locked = Some((dev, hdr));
                return Err(e);
            }
        };
        // Con la clave buena ya se puede escribir: rehace el bloque que quedó a medias y, si la
        // conversión estaba terminada, la remata. Si algo falla, el disco vuelve a quedar cerrado.
        let rec = core.borrow_mut().recover().map_err(cryptdev::disk_err);
        let ready = rec.and_then(|()| Nffs::mount(Box::new(CryptDev::new(core.clone()))).map_err(|_| KeyError::Fs(FsError::Corrupt)));
        match ready {
            Ok(fs) => {
                self.fs = fs;
                self.persistent = true;
                self.notice = None;
                self.user = None;
                self.clip = None;
                self.undo = None;
                self.generation += 1;
                self.disk.core = Some(core);
                self.startup(now);
                self.log("Disco cifrado abierto", now);
                Ok(())
            }
            Err(e) => {
                let hdr = core.borrow().header().clone();
                let dev = core.borrow_mut().take_dev();
                if let Some(dev) = dev {
                    self.disk.locked = Some((dev, hdr));
                }
                Err(e)
            }
        }
    }

    /// El usuario prefiere seguir sin abrir el disco cifrado (archivos solo en memoria). Suelta el
    /// dispositivo sin tocarlo.
    pub fn skip_locked_disk(&mut self) {
        if self.disk.locked.take().is_some() {
            self.notice = Some(NOTICE_LOCKED_SKIPPED);
        }
    }

    /// El disco en uso está cifrado (o se está cifrando).
    pub fn disk_encrypted(&self) -> bool {
        self.disk.core.is_some()
    }

    /// Conversión del disco en curso: (bloques hechos, total, `true` = cifrando).
    pub fn disk_conversion(&self) -> Option<(u64, u64, bool)> {
        self.disk.core.as_ref()?.borrow().progress()
    }

    /// Activa el cifrado total (solo administrador). Devuelve el código de recuperación y deja
    /// una conversión en curso. En un disco antiguo (sin los dos bloques reservados del final)
    /// hace falta que esos bloques estén libres: si no, `NoRoom`.
    pub fn encrypt_disk(&mut self, password: &str, pool: &mut Pool, now: DateTime) -> Result<String, KeyError> {
        if !self.is_admin() {
            return Err(KeyError::NotAdmin);
        }
        if password.chars().count() < accounts::MIN_PASSWORD {
            return Err(KeyError::Weak);
        }
        if !pool.ready() {
            return Err(KeyError::NotReady);
        }
        if let Some(c) = &self.disk.core {
            let busy = c.borrow().progress().is_some();
            return Err(if busy { KeyError::Busy } else { KeyError::AlreadyEncrypted });
        }
        if !self.persistent {
            return Err(KeyError::Unavailable);
        }
        let (n, vol) = (self.fs.dev_blocks(), self.fs.volume_blocks());
        if n < 66 || vol > n {
            return Err(KeyError::NoRoom);
        }
        if vol > n - cryptdev::RESERVED {
            let r = self.fs.shrink_tail(vol - (n - cryptdev::RESERVED));
            r.map_err(|e| if e == FsError::Invalid { KeyError::NoRoom } else { KeyError::Fs(e) })?;
        }
        let mut master = pool.master();
        let (mut s1, mut i1, mut s2, mut i2) = ([0u8; 16], [0u8; 16], [0u8; 16], [0u8; 16]);
        pool.take(&mut s1);
        pool.take(&mut i1);
        pool.take(&mut s2);
        pool.take(&mut i2);
        let code = keys::recovery_code(pool);
        let mut rsecret = keys::normalize_recovery(&code).unwrap_or_default().into_bytes();
        let hdr = Header {
            state: State::Encrypting,
            next: 0,
            pending: false,
            disk: keys::Slot::wrap(&master, password.as_bytes(), s1, i1),
            recovery: keys::Slot::wrap(&master, &rsecret, s2, i2),
        };
        wipe(&mut rsecret);
        // La cabecera llega al disco antes de cifrar nada; después el volumen pasa a la capa de cifrado.
        let (mut core, mut failed) = (None, None);
        self.fs.swap_dev(|mut dev| match cryptdev::write_header(&mut *dev, &hdr) {
            Ok(()) => {
                let c = Rc::new(RefCell::new(Core::new(dev, hdr.clone(), &master)));
                core = Some(c.clone());
                Box::new(CryptDev::new(c))
            }
            Err(e) => {
                failed = Some(e);
                dev
            }
        });
        wipe(&mut master);
        for b in [&mut s1, &mut i1, &mut s2, &mut i2] {
            wipe(b);
        }
        if let Some(e) = failed {
            return Err(cryptdev::disk_err(e));
        }
        self.disk.core = core;
        self.log("Cifrado total activado", now);
        Ok(code)
    }

    /// Quita el cifrado total (solo administrador; pide la clave del disco). Deja una conversión en curso.
    pub fn decrypt_disk(&mut self, password: &str, _now: DateTime) -> Result<(), KeyError> {
        if !self.is_admin() {
            return Err(KeyError::NotAdmin);
        }
        let core = self.disk.core.clone().ok_or(KeyError::NotEncrypted)?;
        if core.borrow().progress().is_some() {
            return Err(KeyError::Busy);
        }
        let hdr = core.borrow().header().clone();
        wipe(&mut cryptdev::unlock(&hdr, password, false)?);
        let r = core.borrow_mut().start_decrypt();
        r.map_err(cryptdev::disk_err)
    }

    /// Avanza la conversión del disco como mucho `blocks` bloques. `Ok(true)` cuando ha terminado.
    /// Al terminar de descifrar, el volumen vuelve a usar el dispositivo directo.
    pub fn convert_step(&mut self, blocks: u32, now: DateTime) -> Result<bool, KeyError> {
        let Some(core) = self.disk.core.clone() else { return Ok(true) };
        let was = core.borrow().progress().is_some();
        let done = core.borrow_mut().step(blocks).map_err(cryptdev::disk_err)?;
        if done && was {
            let decrypted = core.borrow().header().state == State::Decrypting;
            if decrypted {
                let dev = core.borrow_mut().take_dev().ok_or(KeyError::Unavailable)?;
                self.fs.swap_dev(|_| dev);
                self.disk.core = None;
                self.log("Cifrado total quitado", now);
            } else {
                self.log("Cifrado total: conversión terminada", now);
            }
        }
        Ok(done)
    }

    /// Cambia la clave del disco (solo administrador). No recifra datos: vuelve a envolver la
    /// misma llave maestra con sal e iv nuevos.
    pub fn change_disk_password(&mut self, old: &str, new: &str, pool: &mut Pool, now: DateTime) -> Result<(), KeyError> {
        if !self.is_admin() {
            return Err(KeyError::NotAdmin);
        }
        let core = self.disk.core.clone().ok_or(KeyError::NotEncrypted)?;
        if new.chars().count() < accounts::MIN_PASSWORD {
            return Err(KeyError::Weak);
        }
        if !pool.ready() {
            return Err(KeyError::NotReady);
        }
        let hdr = core.borrow().header().clone();
        let mut master = cryptdev::unlock(&hdr, old, false)?;
        let (mut salt, mut iv) = ([0u8; 16], [0u8; 16]);
        pool.take(&mut salt);
        pool.take(&mut iv);
        let slot = keys::Slot::wrap(&master, new.as_bytes(), salt, iv);
        wipe(&mut master);
        let r = core.borrow_mut().set_slots(Some(slot), None);
        r.map_err(cryptdev::disk_err)?;
        self.log("Clave del disco cambiada", now);
        Ok(())
    }

    /// Código de recuperación nuevo para el disco (invalida el anterior). Pide la clave del disco.
    pub fn new_disk_recovery(&mut self, password: &str, pool: &mut Pool, now: DateTime) -> Result<String, KeyError> {
        if !self.is_admin() {
            return Err(KeyError::NotAdmin);
        }
        let core = self.disk.core.clone().ok_or(KeyError::NotEncrypted)?;
        if !pool.ready() {
            return Err(KeyError::NotReady);
        }
        let hdr = core.borrow().header().clone();
        let mut master = cryptdev::unlock(&hdr, password, false)?;
        let (mut salt, mut iv) = ([0u8; 16], [0u8; 16]);
        pool.take(&mut salt);
        pool.take(&mut iv);
        let code = keys::recovery_code(pool);
        let mut secret = keys::normalize_recovery(&code).unwrap_or_default().into_bytes();
        let slot = keys::Slot::wrap(&master, &secret, salt, iv);
        wipe(&mut secret);
        wipe(&mut master);
        let r = core.borrow_mut().set_slots(None, Some(slot));
        r.map_err(cryptdev::disk_err)?;
        self.log("Código de recuperación del disco renovado", now);
        Ok(code)
    }

    /// La carpeta de la cuenta `name` está cifrada (o se está cifrando). Solo mira si existe su
    /// archivo de llaves: funciona sin sesión y no deriva nada.
    pub fn account_encrypted(&mut self, name: &str) -> bool {
        self.fs.stat(&keys_path(name)).is_ok()
    }

    /// La carpeta de la sesión abierta está cifrada.
    pub fn home_encrypted(&mut self) -> bool {
        self.hc.state.is_some()
    }

    /// Cifra la carpeta de la sesión abierta. `password` debe ser la clave actual de la cuenta
    /// (sin clave no hay cifrado). Devuelve el código de recuperación y deja una conversión en curso.
    pub fn encrypt_home(&mut self, password: &str, pool: &mut Pool, now: DateTime) -> Result<String, KeyError> {
        let name = self.user().map(String::from).filter(|_| self.persistent).ok_or(KeyError::Unavailable)?;
        if let Some(s) = self.hc.state {
            return Err(if s == HState::Encrypted { KeyError::AlreadyEncrypted } else { KeyError::Busy });
        }
        if self.fs.stat(&keys_path(&name)).is_ok() {
            return Err(KeyError::AlreadyEncrypted);
        }
        if !self.accounts().iter().find(|a| same(&a.name, &name)).is_some_and(|a| a.has_password() && a.verify(password)) {
            return Err(KeyError::WrongSecret);
        }
        if !pool.ready() {
            return Err(KeyError::NotReady);
        }
        // Un nombre que no cabría cifrado se avisa ahora, antes de tocar nada.
        if !self.names_fit(&name) {
            return Err(KeyError::Fs(FsError::BadName));
        }
        let mut master = pool.master();
        let (mut s1, mut i1, mut s2, mut i2) = ([0u8; 16], [0u8; 16], [0u8; 16], [0u8; 16]);
        pool.take(&mut s1);
        pool.take(&mut i1);
        pool.take(&mut s2);
        pool.take(&mut i2);
        let code = keys::recovery_code(pool);
        let mut rsecret = keys::normalize_recovery(&code).unwrap_or_default().into_bytes();
        let kf = KeyFile::new(
            HState::Encrypting,
            vec![keys::Slot::wrap(&master, password.as_bytes(), s1, i1)],
            keys::Slot::wrap(&master, &rsecret, s2, i2),
        );
        let hkeys = Keys::new(&master);
        wipe(&mut master);
        wipe(&mut rsecret);
        for b in [&mut s1, &mut i1, &mut s2, &mut i2] {
            wipe(b);
        }
        // Las llaves llegan al disco antes de cifrar nada.
        self.write_keyfile(&name, &kf, &hkeys, now)?;
        self.hc = Home { keys: Some(Box::new(hkeys)), state: Some(HState::Encrypting), ..Home::default() };
        self.hc.total = self.pending(&name);
        self.generation += 1;
        self.log(&format!("Cifrado de la carpeta de {name} activado"), now);
        Ok(code)
    }

    /// Cifra la carpeta de la cuenta `name` recién creada y termina la conversión en el acto
    /// (una carpeta nueva casi no tiene nada). Funciona sin sesión o con sesión de administrador;
    /// `password` es la clave de esa cuenta. Devuelve su código de recuperación. Para el asistente.
    pub fn encrypt_new_home(&mut self, name: &str, password: &str, pool: &mut Pool, now: DateTime) -> Result<String, KeyError> {
        if !self.persistent {
            return Err(KeyError::Unavailable);
        }
        if self.user.is_some() && !self.is_admin() {
            return Err(KeyError::NotAdmin);
        }
        let acc = self.accounts().into_iter().find(|a| same(&a.name, name)).ok_or(KeyError::Unavailable)?;
        let own = self.user().is_some_and(|u| same(u, &acc.name));
        let prev_user = self.user.take();
        let prev_hc = core::mem::take(&mut self.hc);
        self.user = Some((acc.name.clone(), acc.admin));
        let private = self.has_private_data(&acc.name);
        let r = self.encrypt_home(password, pool, now).and_then(|code| {
            self.hc.fresh = !private;
            while !self.home_convert_step(64, now)? {}
            self.hc.fresh = false;
            Ok(code)
        });
        // Si la sesión era la de esa misma cuenta, se queda con sus llaves; si no, vuelve a la de antes.
        if !own {
            self.hc = prev_hc;
        }
        self.user = prev_user;
        self.generation += 1;
        r
    }

    /// ¿Hay en la carpeta de `name` algún archivo con contenido, fuera de `.config`? (lectura en
    /// bruto, antes de cifrar).
    fn has_private_data(&mut self, name: &str) -> bool {
        let home = join_rel(USERS, name);
        let mut work = vec![home.clone()];
        while let Some(d) = work.pop() {
            let Ok(list) = self.fs.list(&d) else { continue };
            for e in list {
                if d == home && same(&e.name, HOME_CONFIG) {
                    continue;
                }
                let p = join_rel(&d, &e.name);
                if e.dir {
                    work.push(p);
                } else if e.size > 0 {
                    return true;
                }
            }
        }
        false
    }

    /// Quita el cifrado de la carpeta de la sesión abierta. Deja una conversión en curso.
    pub fn decrypt_home(&mut self, password: &str, now: DateTime) -> Result<(), KeyError> {
        let name = self.user().map(String::from).ok_or(KeyError::Unavailable)?;
        match self.hc.state {
            None => return Err(KeyError::NotEncrypted),
            Some(HState::Encrypted) => {}
            Some(_) => return Err(KeyError::Busy),
        }
        if self.hc.keys.is_none() {
            return Err(KeyError::Unavailable);
        }
        let kf = match self.read_keyfile(&name) {
            Some(Ok(k)) => k,
            _ => return Err(KeyError::Fs(FsError::Corrupt)),
        };
        let mut master = kf.open_pass(password).ok_or(KeyError::WrongSecret)?;
        wipe(&mut master);
        let hkeys = self.hc.keys.take().ok_or(KeyError::Unavailable)?;
        let r = self.write_keyfile(&name, &KeyFile::new(HState::Decrypting, kf.pass, kf.recovery), &hkeys, now);
        self.hc.keys = Some(hkeys);
        r?;
        self.hc.state = Some(HState::Decrypting);
        self.hc.conv = None;
        self.hc.done = 0;
        self.hc.total = self.pending(&name);
        self.generation += 1;
        Ok(())
    }

    /// Conversión de la carpeta de la sesión en curso: (archivos hechos, total, `true` = cifrando).
    /// El total es aproximado (crece si se crean archivos mientras tanto) pero nunca baja.
    pub fn home_conversion(&self) -> Option<(u64, u64, bool)> {
        match self.hc.state {
            Some(HState::Encrypting) => Some((self.hc.done, self.hc.total.max(self.hc.done), true)),
            Some(HState::Decrypting) => Some((self.hc.done, self.hc.total.max(self.hc.done), false)),
            _ => None,
        }
    }

    /// Avanza la conversión de la carpeta como mucho `files` archivos o carpetas. `Ok(true)` cuando
    /// ha terminado. Cada elemento se convierte de forma que un corte no pierde nada: lo nuevo se
    /// escribe entero (atómico) antes de quitar lo viejo, y al reanudar lo ya hecho se reconoce solo.
    pub fn home_convert_step(&mut self, files: u32, now: DateTime) -> Result<bool, KeyError> {
        let Some(state) = self.hc.state else { return Ok(true) };
        if state == HState::Encrypted {
            return Ok(true);
        }
        let name = self.user().map(String::from).ok_or(KeyError::Unavailable)?;
        if self.hc.keys.is_none() {
            return Err(KeyError::Unavailable);
        }
        self.generation += 1;
        let r = self.conv_run(&name, state == HState::Encrypting, files.max(1), now);
        self.hc.cache.clear();
        match r {
            Ok(false) => Ok(false),
            Ok(true) => self.conv_finish(&name, now).map(|()| true),
            Err(e) => {
                // Se vuelve a recorrer desde el principio: lo hecho se reconoce solo.
                self.hc.conv = None;
                Err(KeyError::Fs(e))
            }
        }
    }

    /// Código de recuperación nuevo para la carpeta de la sesión (invalida el anterior).
    pub fn new_home_recovery(&mut self, password: &str, pool: &mut Pool, now: DateTime) -> Result<String, KeyError> {
        let name = self.user().map(String::from).ok_or(KeyError::Unavailable)?;
        let state = self.hc.state.ok_or(KeyError::NotEncrypted)?;
        if self.hc.keys.is_none() {
            return Err(KeyError::Unavailable);
        }
        if !pool.ready() {
            return Err(KeyError::NotReady);
        }
        let kf = match self.read_keyfile(&name) {
            Some(Ok(k)) => k,
            _ => return Err(KeyError::Fs(FsError::Corrupt)),
        };
        let mut master = kf.open_pass(password).ok_or(KeyError::WrongSecret)?;
        let (mut salt, mut iv) = ([0u8; 16], [0u8; 16]);
        pool.take(&mut salt);
        pool.take(&mut iv);
        let code = keys::recovery_code(pool);
        let mut secret = keys::normalize_recovery(&code).unwrap_or_default().into_bytes();
        let slot = keys::Slot::wrap(&master, &secret, salt, iv);
        let hkeys = Keys::new(&master);
        wipe(&mut secret);
        wipe(&mut master);
        self.write_keyfile(&name, &KeyFile::new(state, kf.pass, slot), &hkeys, now)?;
        self.log(&format!("Código de recuperación de la carpeta de {name} renovado"), now);
        Ok(code)
    }

    /// Sin sesión: con el código de recuperación de la cuenta `name` le pone una clave nueva
    /// (y vuelve a envolver su llave). No abre la sesión. Con un código malo no cambia nada.
    pub fn recover_account(&mut self, name: &str, recovery: &str, new_password: &str, pool: &mut Pool, now: DateTime) -> Result<(), KeyError> {
        let mut list = self.accounts();
        let i = list.iter().position(|a| same(&a.name, name)).ok_or(KeyError::NotEncrypted)?;
        let acc_name = list[i].name.clone();
        let kf = match self.read_keyfile(&acc_name) {
            None => return Err(KeyError::NotEncrypted),
            Some(Ok(k)) => k,
            Some(Err(_)) => return Err(KeyError::Fs(FsError::Corrupt)),
        };
        if new_password.chars().count() < accounts::MIN_PASSWORD {
            return Err(KeyError::Weak);
        }
        if !pool.ready() {
            return Err(KeyError::NotReady);
        }
        let mut code = keys::normalize_recovery(recovery).ok_or(KeyError::WrongSecret)?.into_bytes();
        let opened = core::str::from_utf8(&code).ok().and_then(|c| kf.open_recovery(c));
        wipe(&mut code);
        let mut master = opened.ok_or(KeyError::WrongSecret)?;
        let (mut salt, mut iv, mut asalt) = ([0u8; 16], [0u8; 16], [0u8; 16]);
        pool.take(&mut salt);
        pool.take(&mut iv);
        pool.take(&mut asalt);
        let slot = keys::Slot::wrap(&master, new_password.as_bytes(), salt, iv);
        let hkeys = Keys::new(&master);
        wipe(&mut master);
        list[i].set_password(Some(new_password), asalt).map_err(|_| KeyError::Weak)?;
        // Primero la llave (la vieja clave deja de abrirla) y luego la cuenta. Si se corta entre
        // las dos, el código de recuperación sigue sirviendo para repetirlo.
        self.write_keyfile(&acc_name, &KeyFile::new(kf.state, vec![slot], kf.recovery), &hkeys, now)?;
        self.save_accounts(&list, now).map_err(|e| match e {
            AccountError::Fs(f) => KeyError::Fs(f),
            _ => KeyError::Fs(FsError::Disk),
        })?;
        self.log(&format!("Clave de {acc_name} restablecida con el código de recuperación"), now);
        Ok(())
    }

    /// Bloqueo de pantalla: borra de la memoria la llave de la carpeta de la sesión. Mientras,
    /// leer o escribir en ella falla con `Denied`. `verify_password` con la clave correcta la recupera.
    pub fn lock_keys(&mut self) {
        self.hc.keys = None;
        self.hc.conv = None;
        self.hc.cache.clear();
    }
}

/// Bloques que cuenta como una unidad de progreso la limpieza del espacio libre.
const SCRUB_CHUNK: u64 = 256;

/// Pone a cero los bloques LIBRES del volumen NFFS de `dev` desde `*cursor`, mirando como mucho
/// `max`. Al cifrar una carpeta, lo viejo en claro queda en bloques liberados (datos y metadatos);
/// esto los borra de verdad. Lee el mapa de bloques confirmado en cada llamada (el superbloque
/// válido de mayor generación, con su suma comprobada), así que solo toca lo que nadie usa y cada
/// llamada es válida aunque entre dos de ellas se haya escrito algo. `Ok(true)` al llegar al final.
/// shortcut: formato de NFFS leído aquí a mano (fs.rs no ofrece el mapa); si cambia el formato, mover esto a fs.rs.
fn scrub_free(dev: &mut dyn BlockDev, cursor: &mut u64, max: u64) -> Result<bool, FsError> {
    use crate::block::BLOCK;
    fn fnv(data: &[u8]) -> u64 {
        data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
    }
    let le64 = |b: &[u8], o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    let le32 = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let mut best: Option<(u64, [u8; BLOCK])> = None;
    for slot in 0..2u64 {
        let mut b = [0u8; BLOCK];
        dev.read(slot, &mut b)?;
        if &b[..4] == b"NFFS" && le32(&b, 4) == 1 && le64(&b, 56) == fnv(&b[..56]) && best.as_ref().is_none_or(|x| le64(&b, 8) > x.0) {
            best = Some((le64(&b, 8), b));
        }
    }
    let (_, sb) = best.ok_or(FsError::Corrupt)?;
    let (total, count, first, len, sum) = (le64(&sb, 16), le32(&sb, 28) as usize, le64(&sb, 32), le32(&sb, 40) as usize, le64(&sb, 48));
    let bm = total.div_ceil(8) as usize;
    if total > dev.blocks() || count == 0 || len < bm || len > count * (BLOCK - 8) {
        return Err(FsError::Corrupt);
    }
    let mut meta = Vec::with_capacity(len);
    let (mut b, mut buf) = (first, [0u8; BLOCK]);
    for _ in 0..count {
        if b < 2 || b >= total {
            return Err(FsError::Corrupt);
        }
        dev.read(b, &mut buf)?;
        meta.extend_from_slice(&buf[8..]);
        b = le64(&buf, 0);
    }
    meta.truncate(len);
    if b != 0 || fnv(&meta) != sum {
        return Err(FsError::Corrupt);
    }
    let end = (*cursor).saturating_add(max).min(total);
    while *cursor < end {
        let n = *cursor;
        *cursor += 1;
        if n < 2 || meta[(n / 8) as usize] >> (n % 8) & 1 == 1 {
            continue;
        }
        dev.read(n, &mut buf)?;
        if buf.iter().any(|&x| x != 0) {
            dev.write(n, &[0u8; BLOCK])?;
        }
    }
    dev.flush()?;
    Ok(*cursor >= total)
}

/// Archivo de llaves de una cuenta (ruta relativa a la raíz).
fn keys_path(name: &str) -> String {
    join_rel(KEYS_DIR, name)
}

/// Lo más largo que puede ser un nombre en claro para que cifrado quepa en un nombre de NFFS.
const MAX_PLAIN_NAME: usize = 142;

/// Lo que se ve de una entrada de una carpeta cifrada: nombre en claro, nombre en disco y de qué
/// tipo es (cifrada o en claro, solo mientras se convierte).
#[derive(Clone)]
struct Item {
    name: String,
    raw: String,
    enc: bool,
    dir: bool,
    size: u64,
    modified: DateTime,
    /// La otra variante del mismo nombre (resto de un corte en plena conversión): sobra.
    alt: Option<String>,
}

struct Found {
    /// Carpeta que contiene el elemento, como está en disco.
    parent: String,
    /// Último componente tal como se pidió.
    name: String,
    item: Option<Item>,
}

struct Frame {
    path: String,
    rest: Option<Vec<String>>,
}

/// Estado del cifrado de la carpeta de la sesión: llaves (solo en memoria), estado y conversión.
#[derive(Default)]
struct Home {
    keys: Option<Box<Keys>>,
    state: Option<HState>,
    /// Recorrido de la conversión en curso (se rehace si se corta o falla).
    conv: Option<Vec<Frame>>,
    done: u64,
    total: u64,
    /// Limpieza del espacio libre al terminar de cifrar: siguiente bloque por mirar.
    scrub: Option<u64>,
    /// Carpeta recién creada sin datos propios: al terminar de cifrar no hace falta limpiar el
    /// espacio libre (no hubo nada suyo en claro en el disco).
    fresh: bool,
    counter: u32,
    /// Entradas de carpetas ya listadas (por ruta en disco, en minúsculas); se vacía al modificar.
    cache: BTreeMap<String, Vec<Item>>,
}

// ---- Carpeta de usuario cifrada: acceso transparente (v0.6.0) ----
//
// Las rutas que ven las apps son siempre en claro. `r_*` hacen lo de `fs.*` pero, dentro de una
// carpeta cifrada, traducen nombres y contenido. Sin la llave en memoria, `Denied`.
impl Files {
    fn read_keyfile(&mut self, name: &str) -> Option<Result<KeyFile, AccountError>> {
        match self.fs.read(&keys_path(name)) {
            Err(FsError::NotFound) => None,
            Err(e) => Some(Err(AccountError::Fs(e))),
            Ok(b) => Some(KeyFile::parse(&String::from_utf8_lossy(&b)).ok_or(AccountError::Fs(FsError::Corrupt))),
        }
    }

    fn write_keyfile(&mut self, name: &str, kf: &KeyFile, k: &Keys, now: DateTime) -> Result<(), FsError> {
        mk(&mut self.fs, rel(SYS_CONFIG_DIR), now);
        mk(&mut self.fs, KEYS_DIR, now);
        self.fs.write(&keys_path(name), kf.to_text(k).as_bytes(), now)
    }

    /// Carpeta completa de la cuenta; si está cifrada solo su carpeta (el resto ya existe cifrado
    /// o en conversión y crearlo en claro dejaría duplicados).
    fn ensure_home_for(&mut self, name: &str, now: DateTime) {
        if self.fs.stat(&keys_path(name)).is_ok() {
            mk(&mut self.fs, &join_rel(USERS, name), now);
        } else {
            ensure_home(&mut self.fs, name, now);
        }
    }

    /// Vuelve a abrir la llave de la sesión (tras un bloqueo) con la clave de la cuenta.
    fn reload_keys(&mut self, name: &str, password: &str) -> bool {
        let Some(Ok(kf)) = self.read_keyfile(name) else { return false };
        let Some(mut m) = kf.open_pass(password) else { return false };
        self.hc.keys = Some(Box::new(Keys::new(&m)));
        wipe(&mut m);
        self.hc.cache.clear();
        true
    }

    /// ¿Esta ruta (relativa canónica) va por el cifrado de la carpeta de la sesión? `listing`: se
    /// van a ver los nombres de la propia carpeta del usuario (sus hijos van cifrados).
    /// `Denied` si la carpeta es de una cuenta cifrada y no se tiene su llave (la de otro usuario,
    /// aunque sea administrador; la propia con la llave borrada).
    fn enc_route(&mut self, r: &str, listing: bool) -> Result<bool, FsError> {
        let c = comps(r);
        if c.len() < 2 || !same(c[0], USERS) || (c.len() == 2 && !listing) {
            return Ok(false);
        }
        if self.user().is_some_and(|u| same(u, c[1])) && self.hc.state.is_some() {
            return if self.hc.keys.is_some() { Ok(true) } else { Err(FsError::Denied) };
        }
        if self.fs.stat(&keys_path(c[1])).is_ok() {
            return Err(FsError::Denied);
        }
        Ok(false)
    }

    /// Entradas de una carpeta en disco (`raw_dir`) vistas desde dentro de la carpeta cifrada.
    fn items(&mut self, raw_dir: &str) -> Result<Vec<Item>, FsError> {
        let key = lower(raw_dir);
        if let Some(v) = self.hc.cache.get(&key) {
            return Ok(v.clone());
        }
        let state = self.hc.state.ok_or(FsError::Denied)?;
        let list = self.fs.list(raw_dir)?;
        let keys = self.hc.keys.as_ref().ok_or(FsError::Denied)?;
        let prefer_enc = state != HState::Decrypting;
        let mut out: Vec<Item> = Vec::new();
        for e in list {
            let (name, enc) = if homecrypt::has_name_prefix(&e.name) {
                match homecrypt::decode_name(keys, &e.name) {
                    Some(n) => (n, true),
                    None => continue, // nombre alterado o de otra llave: no se toma por válido
                }
            } else if state == HState::Encrypted {
                continue; // en una carpeta ya cifrada, lo que está en claro no es nuestro
            } else {
                (e.name.clone(), false)
            };
            let size = if e.dir {
                0
            } else if enc {
                homecrypt::plain_len(e.size)
            } else {
                e.size
            };
            let item = Item { name, raw: e.name, enc, dir: e.dir, size, modified: e.modified, alt: None };
            match out.iter_mut().find(|i| same(&i.name, &item.name)) {
                None => out.push(item),
                Some(p) if p.enc != item.enc => {
                    // Las dos variantes a la vez: manda la del tipo que se está escribiendo.
                    if item.enc == prefer_enc {
                        let old = core::mem::replace(p, item);
                        p.alt = Some(old.raw);
                    } else {
                        p.alt = Some(item.raw);
                    }
                }
                Some(_) => {}
            }
        }
        self.hc.cache.insert(key, out.clone());
        Ok(out)
    }

    /// Resuelve una ruta lógica dentro de una carpeta cifrada (al menos `Usuarios\x\y`).
    fn find_enc(&mut self, r: &str) -> Result<Found, FsError> {
        let c = comps(r);
        let mut raw = join_rel(USERS, c[1]);
        for comp in &c[2..c.len() - 1] {
            match self.items(&raw)?.into_iter().find(|i| same(&i.name, comp)) {
                Some(i) if i.dir => raw = join_rel(&raw, &i.raw),
                Some(_) => return Err(FsError::NotDir),
                None => return Err(FsError::NotFound),
            }
        }
        let last = c[c.len() - 1];
        let item = self.items(&raw)?.into_iter().find(|i| same(&i.name, last));
        Ok(Found { parent: raw, name: String::from(last), item })
    }

    fn r_list(&mut self, r: &str) -> Result<Vec<Entry>, FsError> {
        if !self.enc_route(r, true)? {
            return self.fs.list(r);
        }
        let c = comps(r);
        let raw = if c.len() == 2 {
            join_rel(USERS, c[1])
        } else {
            let f = self.find_enc(r)?;
            match f.item {
                None => return Err(FsError::NotFound),
                Some(i) if !i.dir => return Err(FsError::NotDir),
                Some(i) => join_rel(&f.parent, &i.raw),
            }
        };
        Ok(self.items(&raw)?.into_iter().map(|i| Entry { name: i.name, dir: i.dir, size: i.size, modified: i.modified, hidden: false }).collect())
    }

    fn r_stat(&mut self, r: &str) -> Result<Entry, FsError> {
        if !self.enc_route(r, false)? {
            return self.fs.stat(r);
        }
        let i = self.find_enc(r)?.item.ok_or(FsError::NotFound)?;
        Ok(Entry { name: i.name, dir: i.dir, size: i.size, modified: i.modified, hidden: false })
    }

    fn r_read(&mut self, r: &str) -> Result<Vec<u8>, FsError> {
        if !self.enc_route(r, false)? {
            return self.fs.read(r);
        }
        let f = self.find_enc(r)?;
        let i = f.item.ok_or(FsError::NotFound)?;
        if i.dir {
            return Err(FsError::IsDir);
        }
        let raw = self.fs.read(&join_rel(&f.parent, &i.raw))?;
        if !i.enc {
            return Ok(raw);
        }
        // Nunca se devuelve nada sin verificar.
        let keys = self.hc.keys.as_ref().ok_or(FsError::Denied)?;
        homecrypt::open(keys, &raw).ok_or(FsError::Corrupt)
    }

    /// Nombre en disco de un elemento nuevo (o renombrado) de tipo `enc` en una carpeta cifrada.
    fn raw_name(&self, name: &str, enc: bool) -> Result<String, FsError> {
        if enc {
            let keys = self.hc.keys.as_ref().ok_or(FsError::Denied)?;
            homecrypt::encode_name(keys, name).ok_or(FsError::BadName)
        } else if homecrypt::has_name_prefix(name) {
            Err(FsError::BadName)
        } else {
            Ok(String::from(name))
        }
    }

    fn r_write(&mut self, r: &str, data: &[u8], now: DateTime) -> Result<(), FsError> {
        if !self.enc_route(r, false)? {
            return self.fs.write(r, data, now);
        }
        let res = self.w_enc(r, data, now);
        self.hc.cache.clear();
        res
    }

    fn w_enc(&mut self, r: &str, data: &[u8], now: DateTime) -> Result<(), FsError> {
        let f = self.find_enc(r)?;
        if f.item.as_ref().is_some_and(|i| i.dir) {
            return Err(FsError::IsDir);
        }
        // Lo nuevo va en el tipo de la conversión en curso: cifrado, salvo mientras se descifra.
        let enc = self.hc.state != Some(HState::Decrypting);
        let name = f.item.as_ref().map_or(f.name.clone(), |i| i.name.clone());
        let raw_name = self.raw_name(&name, enc)?;
        self.hc.counter = self.hc.counter.wrapping_add(1);
        let bytes = if enc {
            let keys = self.hc.keys.as_ref().ok_or(FsError::Denied)?;
            homecrypt::seal(keys, homecrypt::make_sal(&now, self.hc.counter), data)
        } else {
            data.to_vec()
        };
        self.fs.write(&join_rel(&f.parent, &raw_name), &bytes, now)?;
        // Escrito entero lo nuevo, lo viejo de otro tipo sobra (si el borrado se corta, manda lo nuevo).
        if let Some(i) = &f.item {
            for old in [i.alt.as_ref(), (i.enc != enc).then_some(&i.raw)].into_iter().flatten() {
                let _ = self.fs.remove(&join_rel(&f.parent, old));
            }
        }
        Ok(())
    }

    fn r_mkdir(&mut self, r: &str, now: DateTime) -> Result<(), FsError> {
        if !self.enc_route(r, false)? {
            return self.fs.mkdir(r, now);
        }
        let res = (|| {
            let f = self.find_enc(r)?;
            if f.item.is_some() {
                return Err(FsError::Exists);
            }
            let raw_name = self.raw_name(&f.name, self.hc.state != Some(HState::Decrypting))?;
            self.fs.mkdir(&join_rel(&f.parent, &raw_name), now)
        })();
        self.hc.cache.clear();
        res
    }

    fn r_rename(&mut self, f: &str, t: &str) -> Result<(), FsError> {
        let (ef, et) = (self.enc_route(f, false)?, self.enc_route(t, false)?);
        match (ef, et) {
            (false, false) => self.fs.rename(f, t),
            (true, true) => {
                let res = self.rn_enc(f, t);
                self.hc.cache.clear();
                res
            }
            _ => {
                self.cross_ok(f, t)?;
                self.mv_cross(f, t)
            }
        }
    }

    fn rn_enc(&mut self, f: &str, t: &str) -> Result<(), FsError> {
        let src = self.find_enc(f)?;
        let item = src.item.clone().ok_or(FsError::NotFound)?;
        let dst = self.find_enc(t)?;
        if let Some(d) = &dst.item {
            let itself = same(&dst.parent, &src.parent) && same(&d.name, &item.name);
            if !itself {
                return Err(FsError::Exists);
            }
        }
        // Un elemento conserva su tipo (cifrado o en claro) al cambiar de nombre o de sitio.
        let to = join_rel(&dst.parent, &self.raw_name(&dst.name, item.enc)?);
        let from = join_rel(&src.parent, &item.raw);
        self.fs.rename(&from, &to)?;
        if let Some(a) = &item.alt {
            let _ = self.fs.remove(&join_rel(&src.parent, a));
        }
        Ok(())
    }

    /// Mover entre una carpeta cifrada y fuera de ella: copia (cifrando o descifrando) y borra el origen.
    fn mv_cross(&mut self, f: &str, t: &str) -> Result<(), FsError> {
        self.r_stat(f)?;
        if self.r_stat(t).is_ok() {
            return Err(FsError::Exists);
        }
        if let Err(e) = self.copy_tree(f, t, DateTime::default(), true) {
            let _ = self.rm_tree(t);
            return Err(e);
        }
        self.rm_tree(f)
    }

    /// Entre una carpeta cifrada y la de otro usuario (aunque no esté cifrada) no se pasa nada.
    fn cross_ok(&mut self, f: &str, t: &str) -> Result<(), FsError> {
        let (ef, et) = (self.enc_route(f, false)?, self.enc_route(t, false)?);
        if ef != et {
            let c = comps(if ef { t } else { f });
            if c.len() >= 2 && same(c[0], USERS) {
                return Err(FsError::Denied);
            }
        }
        Ok(())
    }

    // ---- Conversión ----

    /// Recorre `name` (en disco) sin entrar en lo que no es de la carpeta; llama a `f` con cada entrada.
    fn walk_raw(&mut self, name: &str, mut f: impl FnMut(&Entry)) {
        let mut stack = vec![join_rel(USERS, name)];
        while let Some(p) = stack.pop() {
            for e in self.fs.list(&p).unwrap_or_default() {
                f(&e);
                if e.dir {
                    stack.push(join_rel(&p, &e.name));
                }
            }
        }
    }

    /// ¿Todos los nombres de la carpeta caben cifrados y ninguno usa el prefijo reservado?
    fn names_fit(&mut self, name: &str) -> bool {
        let mut ok = true;
        self.walk_raw(name, |e| ok &= e.name.len() <= MAX_PLAIN_NAME && !homecrypt::has_name_prefix(&e.name));
        ok
    }

    /// Elementos que faltan por convertir en la carpeta de `name` según el estado de la sesión.
    fn pending(&mut self, name: &str) -> u64 {
        let Some(keys) = self.hc.keys.take() else { return 0 };
        let to_enc = self.hc.state == Some(HState::Encrypting);
        let mut n = 0;
        self.walk_raw(name, |e| {
            let is_enc = homecrypt::has_name_prefix(&e.name);
            if is_enc && homecrypt::decode_name(&keys, &e.name).is_none() {
                return; // nombre alterado: no es nuestro
            }
            n += u64::from(is_enc != to_enc);
        });
        self.hc.keys = Some(keys);
        n
    }

    fn conv_run(&mut self, name: &str, to_enc: bool, mut budget: u32, now: DateTime) -> Result<bool, FsError> {
        if to_enc && self.hc.scrub.is_some() && self.hc.conv.is_none() {
            return self.scrub_phase(budget as u64 * SCRUB_CHUNK); // ya está todo convertido
        }
        let mut stack = match self.hc.conv.take() {
            Some(s) => s,
            None => {
                // Primera vez o tras un fallo/reinicio: se recorre desde el principio.
                let left = self.pending(name);
                self.hc.total = self.hc.total.max(self.hc.done + left);
                vec![Frame { path: join_rel(USERS, name), rest: None }]
            }
        };
        let res = self.conv_loop(&mut stack, to_enc, &mut budget, now);
        if res == Ok(false) {
            self.hc.conv = Some(stack);
        }
        // Todo convertido: lo claro que quedó en bloques ya libres (datos y metadatos viejos) se borra.
        if res == Ok(true) && to_enc && !self.hc.fresh {
            return self.scrub_phase(budget.max(1) as u64 * SCRUB_CHUNK);
        }
        res
    }

    /// Una pasada de limpieza de como mucho `blocks` bloques. `Ok(true)` al terminar el volumen.
    fn scrub_phase(&mut self, blocks: u64) -> Result<bool, FsError> {
        let vol = self.fs.volume_blocks();
        let mut cur = match self.hc.scrub {
            Some(c) => c,
            None => {
                self.hc.total += vol.div_ceil(SCRUB_CHUNK);
                0
            }
        };
        let from = cur;
        let mut res = Ok(false);
        self.fs.swap_dev(|mut dev| {
            res = scrub_free(&mut *dev, &mut cur, blocks);
            dev
        });
        self.hc.scrub = Some(cur);
        self.hc.done += (cur - from).div_ceil(SCRUB_CHUNK);
        res
    }

    fn conv_loop(&mut self, stack: &mut Vec<Frame>, to_enc: bool, budget: &mut u32, now: DateTime) -> Result<bool, FsError> {
        loop {
            let Some(top) = stack.last_mut() else { return Ok(true) };
            if *budget == 0 {
                return Ok(false);
            }
            let path = top.path.clone();
            let Some(rest) = top.rest.as_mut() else {
                let mut names: Vec<String> = self.fs.list(&path)?.into_iter().map(|e| e.name).collect();
                names.reverse();
                top.rest = Some(names);
                continue;
            };
            let Some(rn) = rest.pop() else {
                // Carpeta recorrida: ahora se convierte ella misma (la raíz de la cuenta no cambia).
                stack.pop();
                if !stack.is_empty() && self.conv_dir(&path, to_enc)? {
                    *budget -= 1;
                    self.hc.done += 1;
                }
                continue;
            };
            let full = join_rel(&path, &rn);
            let e = match self.fs.stat(&full) {
                Ok(e) => e,
                Err(FsError::NotFound) => continue,
                Err(e) => return Err(e),
            };
            let is_enc = homecrypt::has_name_prefix(&rn);
            if is_enc && self.hc.keys.as_ref().and_then(|k| homecrypt::decode_name(k, &rn)).is_none() {
                continue; // nombre alterado: se deja como está
            }
            if e.dir {
                stack.push(Frame { path: full, rest: None });
            } else if is_enc != to_enc {
                if to_enc {
                    self.conv_file_enc(&path, &rn, &e, now)?;
                } else {
                    self.conv_file_dec(&path, &rn, &e)?;
                }
                *budget -= 1;
                self.hc.done += 1;
            }
        }
    }

    /// Cambia el nombre de una carpeta ya recorrida al del tipo que toca. `true` si lo cambió.
    fn conv_dir(&mut self, path: &str, to_enc: bool) -> Result<bool, FsError> {
        let (parent, name) = path.rsplit_once('\\').unwrap_or(("", path));
        let keys = self.hc.keys.as_ref().ok_or(FsError::Denied)?;
        let new = match (to_enc, homecrypt::has_name_prefix(name)) {
            (true, false) => homecrypt::encode_name(keys, name).ok_or(FsError::BadName)?,
            (false, true) => homecrypt::decode_name(keys, name).ok_or(FsError::Corrupt)?,
            _ => return Ok(false),
        };
        let to = join_rel(parent, &new);
        if self.fs.stat(&to).is_ok() {
            return Err(FsError::Exists);
        }
        self.fs.rename(path, &to)?;
        Ok(true)
    }

    fn conv_file_enc(&mut self, parent: &str, rn: &str, e: &Entry, now: DateTime) -> Result<(), FsError> {
        let keys = self.hc.keys.as_ref().ok_or(FsError::Denied)?;
        let src = join_rel(parent, rn);
        let dst = join_rel(parent, &homecrypt::encode_name(keys, rn).ok_or(FsError::BadName)?);
        if self.fs.stat(&dst).is_ok() {
            // Ya hay una copia cifrada entera (un corte entre escribirla y quitar la vieja): solo
            // se quita la vieja si la cifrada se verifica.
            let raw = self.fs.read(&dst)?;
            if homecrypt::open(keys, &raw).is_none() {
                return Err(FsError::Corrupt);
            }
            return self.fs.remove(&src);
        }
        let data = self.fs.read(&src)?;
        self.hc.counter = self.hc.counter.wrapping_add(1);
        let sealed = homecrypt::seal(keys, homecrypt::make_sal(&now, self.hc.counter), &data);
        self.fs.write(&dst, &sealed, e.modified)?;
        self.fs.remove(&src)
    }

    fn conv_file_dec(&mut self, parent: &str, rn: &str, e: &Entry) -> Result<(), FsError> {
        let keys = self.hc.keys.as_ref().ok_or(FsError::Denied)?;
        let src = join_rel(parent, rn);
        let dst = join_rel(parent, &homecrypt::decode_name(keys, rn).ok_or(FsError::Corrupt)?);
        if self.fs.stat(&dst).is_ok() {
            // Ya está en claro (escrito entero antes del corte): la cifrada sobra.
            return self.fs.remove(&src);
        }
        let raw = self.fs.read(&src)?;
        let data = homecrypt::open(keys, &raw).ok_or(FsError::Corrupt)?;
        self.fs.write(&dst, &data, e.modified)?;
        self.fs.remove(&src)
    }

    /// La conversión recorrió todo: el estado pasa a ser el definitivo.
    fn conv_finish(&mut self, name: &str, now: DateTime) -> Result<(), KeyError> {
        let to_enc = self.hc.state == Some(HState::Encrypting);
        if to_enc {
            let kf = match self.read_keyfile(name) {
                Some(Ok(k)) => k,
                _ => return Err(KeyError::Fs(FsError::Corrupt)),
            };
            let keys = self.hc.keys.take().ok_or(KeyError::Unavailable)?;
            let r = self.write_keyfile(name, &KeyFile::new(HState::Encrypted, kf.pass, kf.recovery), &keys, now);
            self.hc.keys = Some(keys);
            r?;
            self.hc.state = Some(HState::Encrypted);
            self.hc.conv = None;
        } else {
            self.fs.remove(&keys_path(name))?;
            self.hc = Home::default();
            self.log(&format!("Cifrado de la carpeta de {name} quitado"), now);
        }
        self.generation += 1;
        Ok(())
    }
}

fn lower(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}

// ---- Unidades extraíbles D:, E:… (contrato v0.11; ver `drives`) ----

/// Letra de unidad de lo que escribe una app (`C` si la ruta no vale).
fn letter(path: &str) -> char {
    resolve("C:\\", path).map_or('C', |c| c.as_bytes()[0] as char)
}

impl Files {
    /// Volumen abierto de la unidad `l`: `BadPath` si no existe, `Denied` si está cifrada sin abrir.
    fn vol(&mut self, l: char) -> Result<&mut dyn Volume, FsError> {
        let d = self.xd.list.iter_mut().find(|d| d.letter == l).ok_or(FsError::BadPath)?;
        match d.vol.as_mut() {
            Some(v) => Ok(&mut **v),
            None => Err(FsError::Denied),
        }
    }

    fn v_stat(&mut self, l: char, r: &str) -> Result<Entry, FsError> {
        let v = self.vol(l)?;
        if r.is_empty() {
            return Ok(Entry { name: String::new(), dir: true, size: 0, modified: DateTime::default(), hidden: false });
        }
        let mut e = [v.stat(r)?];
        self.a_local(l, &mut e);
        let [e] = e;
        Ok(e)
    }

    /// NTFS y ext guardan las fechas en UTC: se pasan a la hora local de la zona (FAT y exFAT ya van en local).
    fn a_local(&mut self, l: char, es: &mut [Entry]) {
        if matches!(self.vol(l).map(|v| v.kind()), Ok(crate::vol::FsKind::Ntfs | crate::vol::FsKind::Ext)) {
            let z = self.time_cfg().zona();
            for e in es.iter_mut().filter(|e| e.modified.year != 0) {
                e.modified = crate::hora::utc_a_local(e.modified, z);
            }
        }
    }

    /// Borra `r` y todo lo que contiene en la unidad `l` (las comprueba quien llama).
    fn v_rm_tree(&mut self, l: char, r: &str) -> Result<(), FsError> {
        let v = self.vol(l)?;
        let mut order = Vec::new();
        let mut stack = vec![String::from(r)];
        while let Some(p) = stack.pop() {
            if v.stat(&p)?.dir {
                for e in v.list(&p)? {
                    stack.push(join_rel(&p, &e.name));
                }
            }
            order.push(p);
        }
        for p in order.iter().rev() {
            v.delete(p)?;
        }
        Ok(())
    }

    /// `rename` cuando alguna ruta es de una unidad extraíble (canónicas).
    fn rename_x(&mut self, f: &str, t: &str) -> Result<(), FsError> {
        for p in [f, t] {
            if let Some(l) = ext(p) {
                self.vol(l)?;
            }
        }
        let l = ext(f).filter(|l| ext(t) == Some(*l)).ok_or(FsError::Invalid)?;
        let (rf, rt) = (lower(rel(f)), lower(rel(t)));
        if rf.is_empty() {
            return Err(FsError::Invalid);
        }
        if rt.is_empty() {
            return Err(FsError::Exists);
        }
        if rt.len() > rf.len() && rt.starts_with(&rf) && rt.as_bytes()[rf.len()] == b'\\' {
            return Err(FsError::Invalid);
        }
        if rf != rt && self.v_stat(l, rel(t)).is_ok() {
            return Err(FsError::Exists);
        }
        self.generation += 1;
        self.vol(l)?.rename(rel(f), rel(t))
    }

    /// `copy` cuando alguna ruta es de una unidad extraíble (canónicas): recorre el árbol leyendo de
    /// un lado y escribiendo en el otro con las operaciones públicas, así cada lado aplica sus reglas.
    fn copy_x(&mut self, f: &str, t: &str, now: DateTime) -> Result<(), FsError> {
        let src = self.stat(f)?;
        let (lf, lt) = (lower(f), lower(t));
        let inside = lt.len() > lf.len() && lt.starts_with(&lf) && (is_root(f) || lt.as_bytes()[lf.len()] == b'\\');
        if src.dir && inside {
            return Err(FsError::Invalid);
        }
        if is_root(t) || self.stat(t).is_ok() {
            return Err(FsError::Exists);
        }
        self.generation += 1;
        let r = self.copy_nodes(f, t, now);
        if r.is_err() {
            let _ = self.delete(t);
        }
        r
    }

    fn copy_nodes(&mut self, f: &str, t: &str, now: DateTime) -> Result<(), FsError> {
        let mut work = vec![(String::from(f), String::from(t))];
        while let Some((s, d)) = work.pop() {
            if self.stat(&s)?.dir {
                self.mkdir(&d, now)?;
                for e in self.list(&s)? {
                    work.push((join(&s, &e.name), join(&d, &e.name)));
                }
            } else {
                let data = self.read(&s)?;
                self.write(&d, &data, now)?;
            }
        }
        Ok(())
    }

    /// Mover entre dos unidades distintas: copia y luego borra el origen. Si el borrado falla se
    /// deja la copia (borrarla podría perder lo que quede del origen).
    fn move_across(&mut self, from: &str, to: &str, now: DateTime) -> Result<(), FsError> {
        let (f, t) = (canon(from)?, canon(to)?);
        if is_protected(&f) {
            return Err(FsError::Invalid);
        }
        if ext(&f).is_none() {
            self.guard(rel(&f), true)?;
        }
        self.copy_x(&f, &t, now)?;
        self.delete(&f)
    }

    fn free_letter(&self) -> Option<char> {
        ('D'..='Z').find(|l| self.xd.list.iter().all(|d| d.letter != *l))
    }

    /// Conecta un volumen montado de la partición del disco `disk` en la primera letra libre
    /// (D..Z). `None` si no quedan letras.
    pub fn attach(&mut self, disk: u32, device: &str, vol: Box<dyn Volume>) -> Option<char> {
        self.attach_as(disk, device, vol, true)
    }

    /// Como `attach`, diciendo si la unidad es extraíble (USB) o una partición de un disco interno.
    /// Las internas no se expulsan; si no son de NightFury, el núcleo las monta en solo lectura.
    pub fn attach_as(&mut self, disk: u32, device: &str, vol: Box<dyn Volume>, removable: bool) -> Option<char> {
        self.attach_part(disk, 0, device, vol, removable)
    }

    /// Como `attach_as`, diciendo en qué sector empieza la partición (para formatearla sola).
    pub fn attach_part(&mut self, disk: u32, part: u64, device: &str, vol: Box<dyn Volume>, removable: bool) -> Option<char> {
        let letter = self.free_letter()?;
        self.xd.list.push(XDrive { letter, disk, part, removable, device: String::from(device), vol: Some(vol), locked: None });
        self.generation += 1;
        Some(letter)
    }

    /// Como `attach`, para un volumen NightFury cifrado que espera contraseña.
    pub fn attach_locked(&mut self, locked: Locked) -> Option<char> {
        let letter = self.free_letter()?;
        self.xd.list.push(XDrive { letter, disk: locked.disk, part: locked.part, removable: locked.removable, device: locked.device, vol: None, locked: Some(locked.drive) });
        self.generation += 1;
        Some(letter)
    }

    /// El disco `disk` se ha quitado: sus letras desaparecen sin escribir nada (ya no está).
    pub fn detach(&mut self, disk: u32) {
        let n = self.xd.list.len();
        self.xd.list.retain(|d| d.disk != disk);
        if self.xd.list.len() != n {
            self.generation += 1;
            self.forget_gone();
        }
    }

    /// Olvida deshacer y portapapeles que nombren una letra que ya no está: si otro pendrive
    /// recibe esa letra, Ctrl+Z o pegar actuarían sobre sus archivos.
    fn forget_gone(&mut self) {
        let list = &self.xd.list;
        let gone = |p: &String| p.chars().next().is_some_and(|c| !c.eq_ignore_ascii_case(&'C') && !list.iter().any(|d| d.letter.eq_ignore_ascii_case(&c)));
        if self.clip.as_ref().is_some_and(|(v, _)| v.iter().any(gone)) {
            self.clip = None;
        }
        let stale = |u: &Undo| match u {
            Undo::Move { from, to } => gone(from) || gone(to),
            Undo::Created(p) | Undo::Trashed(p) => gone(p),
        };
        if self.undo.as_ref().is_some_and(|(v, _)| v.iter().any(stale)) {
            self.undo = None;
        }
    }

    /// C: y las unidades conectadas, por letra.
    pub fn drives(&mut self) -> Vec<DriveInfo> {
        let (total, free) = self.fs.space();
        let mut v = vec![DriveInfo { letter: 'C', label: String::new(), device: String::new(), kind: crate::vol::FsKind::NightFury, total, free, read_only: false, removable: false, locked: false }];
        let mut rest: Vec<DriveInfo> = self
            .xd
            .list
            .iter_mut()
            .map(|d| match d.vol.as_mut() {
                Some(vol) => {
                    let (total, free) = vol.space();
                    DriveInfo { letter: d.letter, label: vol.label(), device: d.device.clone(), kind: vol.kind(), total, free, read_only: vol.read_only(), removable: d.removable, locked: false }
                }
                None => DriveInfo { letter: d.letter, label: String::new(), device: d.device.clone(), kind: crate::vol::FsKind::NightFury, total: 0, free: 0, read_only: false, removable: d.removable, locked: true },
            })
            .collect();
        rest.sort_by_key(|d| d.letter);
        v.append(&mut rest);
        v
    }

    /// Aviso del volumen de la unidad `letter` (p. ej. un ext con el diario a medias), si lo hay.
    pub fn drive_notice(&self, letter: char) -> Option<&'static str> {
        let letter = letter.to_ascii_uppercase();
        self.xd.list.iter().find(|d| d.letter == letter)?.vol.as_ref()?.notice()
    }

    /// ¿Escribe NightFury en la partición interna (`serie` del disco, sector `start`) tras reiniciar?
    /// Lo dice `C:\Sistema\unidades.cfg`: una línea `serie|inicio` por partición que NightFury formateó.
    pub fn internal_writable(&mut self, serie: &str, start: u64) -> bool {
        // Sin número de serie no se distingue un disco de otro: nunca se da por nuestro.
        if serie.trim().is_empty() {
            return false;
        }
        let line = units_line(serie, start);
        self.fs.read(rel(UNITS_FILE)).is_ok_and(|b| String::from_utf8_lossy(&b).lines().any(|l| l == line))
    }

    /// Apunta (`on`) o quita la partición interna de `unidades.cfg`. Sin sesión y sin pasar por `guard`
    /// (lo llama el núcleo); no hace nada si ya está como se pide.
    pub fn set_internal_writable(&mut self, serie: &str, start: u64, on: bool, now: DateTime) -> Result<(), FsError> {
        let line = units_line(serie, start);
        let old = self.fs.read(rel(UNITS_FILE)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        if old.lines().any(|l| l == line) == on {
            return Ok(());
        }
        let mut new: String = old.lines().filter(|l| *l != line).map(|l| format!("{l}\n")).collect();
        if on {
            new += &format!("{line}\n");
        }
        Ok(self.fs.write(rel(UNITS_FILE), new.as_bytes(), now)?)
    }

    /// Abre una unidad cifrada con su contraseña o con el código de recuperación (de golpe: el
    /// PBKDF2 congela todo unos segundos; la interfaz usa `begin_unlock`).
    pub fn unlock_drive(&mut self, letter: char, secret: &str, recovery: bool) -> Result<(), KeyError> {
        self.unlock_with(letter, |l| l.unlock(secret, recovery))
    }

    fn unlock_with(
        &mut self,
        letter: char,
        f: impl FnOnce(LockedDrive) -> Result<Box<dyn Volume>, (LockedDrive, KeyError)>,
    ) -> Result<(), KeyError> {
        let letter = letter.to_ascii_uppercase();
        let d = self.xd.list.iter_mut().find(|d| d.letter == letter).ok_or(FsError::BadPath)?;
        let locked = d.locked.take().ok_or(KeyError::NotEncrypted)?;
        match f(locked) {
            Ok(vol) => {
                d.vol = Some(vol);
                self.generation += 1;
                Ok(())
            }
            Err((locked, e)) => {
                d.locked = Some(locked);
                Err(e)
            }
        }
    }

    /// Empieza a abrir una unidad cifrada sin bloquear: `unlock_step` avanza el trabajo y
    /// `unlock_result` da el final. El secreto no se guarda (solo el PBKDF2 en marcha). Un trabajo
    /// anterior sin terminar se descarta.
    pub fn begin_unlock(&mut self, letter: char, secret: &str, recovery: bool) -> Result<(), KeyError> {
        let letter = letter.to_ascii_uppercase();
        let d = self.xd.list.iter().find(|d| d.letter == letter).ok_or(FsError::BadPath)?;
        let kdf = d.locked.as_ref().ok_or(KeyError::NotEncrypted)?.unlock_paso(secret, recovery)?;
        self.xd.unlocked = None;
        self.xd.job = Some(UnlockJob { letter, recovery, kdf });
        Ok(())
    }

    /// Avanza hasta `iters` vueltas el desbloqueo en curso; `true` mientras falte trabajo. Al
    /// acabar deja el resultado para `unlock_result` (y la unidad abierta si era buena).
    pub fn unlock_step(&mut self, iters: u32) -> bool {
        let Some(j) = self.xd.job.as_mut() else { return false };
        if j.kdf.step(iters) {
            return true;
        }
        let Some(mut j) = self.xd.job.take() else { return false };
        let mut dk = j.kdf.take();
        let r = self.unlock_with(j.letter, |l| l.unlock_con(&mut dk, j.recovery));
        wipe(&mut dk);
        self.xd.unlocked = Some(r);
        false
    }

    /// Resultado del último desbloqueo, una sola vez.
    pub fn unlock_result(&mut self) -> Option<Result<(), KeyError>> {
        self.xd.unlocked.take()
    }

    /// Pide expulsar, formatear o cifrar el disco de la unidad `letter` (solo programas del sistema;
    /// lo comprueba el servicio). Lo ejecuta el núcleo con `take_drive_actions`. Se escribe lo
    /// pendiente de todas las unidades de ese disco y sus letras desaparecen enseguida.
    pub fn request_drive_action(&mut self, letter: char, req: FormatRequest) -> Result<(), FsError> {
        let letter = letter.to_ascii_uppercase();
        if letter == 'C' {
            return Err(FsError::Invalid);
        }
        let x = self.xd.list.iter().find(|d| d.letter == letter).ok_or(FsError::BadPath)?;
        let (disk, part, internal) = (x.disk, x.part, !x.removable);
        let eject = req == FormatRequest::Eject;
        // Un disco interno nunca se expulsa, y formatear o cifrar toca solo su partición (las demás siguen montadas).
        if internal && eject {
            return Err(FsError::Denied);
        }
        let mine = |d: &XDrive| d.disk == disk && (!internal || d.part == part);
        for d in self.xd.list.iter_mut().filter(|d| mine(d)) {
            if let Some(v) = d.vol.as_mut() {
                // Las internas ajenas (NTFS, ext…) son de solo lectura y se pueden formatear igualmente.
                if !eject && !internal && v.read_only() {
                    return Err(FsError::Denied);
                }
            }
        }
        for d in self.xd.list.iter_mut().filter(|d| mine(d)) {
            if let Some(v) = d.vol.as_mut() {
                // Al formatear se pierde todo igualmente; al expulsar, un fallo impide quitarla.
                if let (Err(e), true) = (v.flush(), eject) {
                    return Err(e);
                }
            }
        }
        self.xd.list.retain(|d| !mine(d));
        self.generation += 1;
        self.forget_gone();
        self.xd.result = None;
        self.xd.actions.push(match req {
            FormatRequest::Eject => DriveAction::Eject { disk },
            FormatRequest::Format { kind, label } => DriveAction::Format { disk, part, kind, label },
            FormatRequest::Encrypt { label, password } => DriveAction::Encrypt { disk, part, label, password },
        });
        Ok(())
    }

    /// Acciones pendientes, para el núcleo.
    pub fn take_drive_actions(&mut self) -> Vec<DriveAction> {
        core::mem::take(&mut self.xd.actions)
    }

    /// El núcleo terminó la última acción: `Ok(Some(código))` al cifrar, `Ok(None)` en el resto.
    pub fn set_drive_result(&mut self, r: Result<Option<String>, FsError>) {
        self.xd.result = Some(r);
    }

    /// Resultado de la última acción, una sola vez (lo consulta el programa que la pidió).
    pub fn drive_result(&mut self) -> Option<Result<Option<String>, FsError>> {
        self.xd.result.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{DiskError, BLOCK};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn primer_arranque_cifrado_sin_conversion() {
        let n = DateTime::default();
        let mut pool = crate::keys::Pool::new();
        for i in 0..crate::keys::POOL_EVENTS_NEEDED {
            pool.add_event(i as i32 * 7, 3, i as u64, i as u64 * 7919);
        }
        let disk = Disk::new(4096);
        let mut f = Files::probe(Some(Box::new(disk.clone())), n);
        assert!(f.pending_format());
        assert_eq!(f.format_pending_encrypted("corta", &mut pool, n), Err(KeyError::Weak));
        assert!(f.pending_format(), "si falla, el disco sigue pendiente");
        let code = f.format_pending_encrypted("clave-del-disco", &mut pool, n).unwrap();
        assert!(f.disk_encrypted() && f.disk_conversion().is_none(), "nace cifrado, sin conversión");
        assert!(f.needs_setup() && f.user().is_none());
        // Primera cuenta con carpeta cifrada, sin sesión (como en el asistente).
        f.create_account("Ana", Some("clave-de-ana"), true, [1; 16], n).unwrap();
        assert_eq!(f.encrypt_new_home("Ana", "mala-clave", &mut pool, n), Err(KeyError::WrongSecret));
        let ana = f.encrypt_new_home("Ana", "clave-de-ana", &mut pool, n).unwrap();
        assert!(f.user().is_none() && f.account_encrypted("Ana") && f.home_conversion().is_none());
        // Con la sesión de Ana (administradora), otra cuenta y su carpeta.
        f.login("Ana", "clave-de-ana").unwrap();
        f.create_account("Luis", Some("clave-de-luis"), false, [2; 16], n).unwrap();
        let luis = f.encrypt_new_home("Luis", "clave-de-luis", &mut pool, n).unwrap();
        assert_eq!(f.user(), Some("Ana"), "la sesión no cambia");
        assert!(f.home_encrypted() && f.account_encrypted("Luis"));
        let doc = format!("{}\\nota-secreta-de-ana.txt", f.std_dir(Std::Documents));
        f.write(&doc, b"CONTENIDO-PRIVADO-DE-ANA-0123456789", n).unwrap();
        assert_ne!([&code, &ana, &luis][0], &ana);
        drop(f);
        let raw = disk.0.borrow().clone();
        for s in [&b"CONTENIDO-PRIVADO-DE-ANA"[..], b"nota-secreta", b"Usuarios", b"Documentos", b"Luis|"] {
            assert!(!raw.windows(s.len()).any(|w| w == s), "en claro: {}", String::from_utf8_lossy(s));
        }
        // Al volver a arrancar: disco bloqueado; con su clave abre y cada cuenta ve lo suyo.
        let mut f = Files::probe(Some(Box::new(disk.clone())), n);
        assert!(f.disk_locked());
        f.unlock_disk(&code, true, n).unwrap();
        f.login("Ana", "clave-de-ana").unwrap();
        assert_eq!(f.read(&doc).unwrap(), b"CONTENIDO-PRIVADO-DE-ANA-0123456789");
        f.logout();
        f.login("Luis", "clave-de-luis").unwrap();
        assert_eq!(f.read(&doc), Err(FsError::Denied));
        // El código de recuperación de Luis le pone una clave nueva.
        f.logout();
        f.recover_account("Luis", &luis, "otra-clave-luis", &mut pool, n).unwrap();
        f.login("Luis", "otra-clave-luis").unwrap();
        // Un usuario normal no cifra la carpeta de otro desde su sesión.
        assert_eq!(f.encrypt_new_home("Ana", "clave-de-ana", &mut pool, n), Err(KeyError::NotAdmin));
    }

    #[test]
    fn cuenta_nueva_no_hereda_carpeta_conservada() {
        let n = DateTime::default();
        let disk = Disk::new(2048);
        drop(Nffs::format(Box::new(disk.clone()), n).unwrap());
        let mut f = Files::probe(Some(Box::new(disk)), n);
        f.create_account("Ana", None, true, [1; 16], n).unwrap();
        f.login("Ana", "").unwrap();
        f.create_account("Luis", None, false, [2; 16], n).unwrap();
        f.write("C:\\Usuarios\\Luis\\Documentos\\privado.txt", b"x", n).unwrap();
        f.delete_account("Luis", true, n).unwrap();
        assert!(f.exists("C:\\Usuarios\\Luis\\Documentos\\privado.txt"), "la carpeta se conserva");
        assert_eq!(f.create_account("luis", None, false, [3; 16], n), Err(AccountError::Exists));
        assert_eq!(f.accounts().len(), 1);
    }

    #[test]
    fn cuentas_y_sesion() {
        let n = DateTime::default();
        // En memoria: sesión de la cuenta por defecto, sin asistente.
        let mut m = Files::default();
        assert_eq!(m.user(), Some(DEFAULT_USER));
        assert!(m.is_admin() && !m.needs_setup() && m.verify_password(""));
        m.logout();
        assert_eq!(m.user(), Some(DEFAULT_USER));
        // Con disco: sin sesión y con asistente hasta crear la primera cuenta.
        let disk = Disk::new(2048);
        let fs = Nffs::format(Box::new(disk.clone()), n).unwrap();
        drop(fs);
        let mut f = Files::probe(Some(Box::new(disk.clone())), n);
        assert!(f.persistent() && f.user().is_none() && f.needs_setup());
        assert_eq!(f.create_account("a|b", None, false, [1; 16], n), Err(AccountError::BadName));
        f.create_account("Ana", Some("secreta1"), false, [1; 16], n).unwrap();
        assert!(!f.needs_setup() && f.accounts()[0].admin, "la primera cuenta es administradora");
        // Sin sesión de administrador no se crean más.
        assert_eq!(f.create_account("Luis", None, false, [2; 16], n), Err(AccountError::NotAdmin));
        assert_eq!(f.login("Ana", "mala"), Err(AccountError::WrongPassword));
        assert_eq!(f.login("Nadie", ""), Err(AccountError::NotFound));
        f.login("ana", "secreta1").unwrap();
        assert_eq!(f.user(), Some("Ana"));
        assert!(f.is_admin() && f.has_password() && f.verify_password("secreta1") && !f.verify_password(""));
        f.create_account("Luis", None, false, [2; 16], n).unwrap();
        assert_eq!(f.create_account("LUIS", None, false, [2; 16], n), Err(AccountError::Exists));
        // Administradores: debe quedar uno; no se elimina la sesión abierta.
        assert_eq!(f.set_admin("Ana", false, n), Err(AccountError::LastAdmin));
        assert_eq!(f.delete_account("Ana", true, n), Err(AccountError::InUse));
        f.set_admin("Luis", true, n).unwrap();
        f.set_admin("Ana", false, n).unwrap();
        assert!(!f.is_admin());
        assert_eq!(f.delete_account("Luis", true, n), Err(AccountError::NotAdmin));
        // Cambio de clave propio.
        assert_eq!(f.set_password("mala", Some("otraclave"), [3; 16], n), Err(AccountError::WrongPassword));
        assert_eq!(f.set_password("secreta1", Some("abc"), [3; 16], n), Err(AccountError::WeakPassword));
        f.set_password("secreta1", Some("otraclave"), [3; 16], n).unwrap();
        f.logout();
        assert!(f.user().is_none());
        assert_eq!(f.login("Ana", "secreta1"), Err(AccountError::WrongPassword));
        // Las cuentas sobreviven a un reinicio; Luis entra sin clave y puede eliminar a Ana.
        drop(f);
        let mut f = Files::probe(Some(Box::new(disk.clone())), n);
        assert_eq!(f.accounts().len(), 2);
        f.login("Luis", "").unwrap();
        f.delete_account("Ana", true, n).unwrap();
        assert_eq!(f.delete_account("Luis", true, n), Err(AccountError::InUse));
        assert_eq!(f.accounts().len(), 1);
        let raw = disk.0.borrow();
        assert!(!raw.windows(8).any(|w| w == b"secreta1") && !raw.windows(9).any(|w| w == b"otraclave"), "la clave no se guarda");
    }

    #[test]
    fn lotes_y_extensiones() {
        let mut f = Files::default();
        let n = DateTime::default();
        let doc = |x: &str| format!("C:\\Usuarios\\Usuario\\Documentos\\{x}");
        for x in ["a.txt", "b.txt", "c.txt"] {
            f.write(&doc(x), b"x", n).unwrap();
        }
        let all = vec![doc("a.txt"), doc("b.txt"), doc("c.txt")];
        // Eliminar varios y deshacer el lote entero.
        f.user_trash_many(&all, n).unwrap();
        assert!(all.iter().all(|p| !f.exists(p)));
        f.undo(n).unwrap();
        assert!(all.iter().all(|p| f.exists(p)) && f.undo_label().is_none());
        // Copiar varios con el portapapeles.
        f.set_clip_many(&all, false);
        assert_eq!(f.clip_paths().0.len(), 3);
        assert_eq!(f.clip(), Some((all[0].as_str(), false)));
        let out = f.paste_all("C:\\Usuarios\\Usuario\\Escritorio", n).unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(f.undo_label(), Some("Copiar"));
        f.undo(n).unwrap();
        assert!(out.iter().all(|p| !f.exists(p)));
        // Mover varios y deshacer.
        let out = f.transfer(&all, "C:\\Usuarios\\Usuario\\Escritorio", true, n).unwrap();
        assert!(all.iter().all(|p| !f.exists(p)) && out.iter().all(|p| f.exists(p)));
        assert_eq!(f.undo_label(), Some("Mover"));
        f.undo(n).unwrap();
        assert!(all.iter().all(|p| f.exists(p)));
        // Si uno falla, lo anterior queda hecho y se puede deshacer.
        let bad = vec![doc("a.txt"), doc("no.txt"), doc("b.txt")];
        assert!(f.user_trash_many(&bad, n).is_err());
        assert!(!f.exists(&doc("a.txt")) && f.exists(&doc("b.txt")));
        f.undo(n).unwrap();
        assert!(f.exists(&doc("a.txt")));
        f.set_clip_many(&[], true);
        assert!(f.clip().is_none());
        // Extensiones.
        assert!(f.show_ext());
        let prefs = f.explorer_prefs();
        f.write(&prefs, b"vista=lista\nextensiones=no\n", n).unwrap();
        assert!(!f.show_ext());
        assert_eq!(display_name("nota.txt", false, false), "nota");
        assert_eq!(display_name("nota.txt", false, true), "nota.txt");
        assert_eq!(display_name("v1.2", true, false), "v1.2");
        assert_eq!(display_name(".oculto", false, false), ".oculto");
        assert_eq!(display_name("sin", false, false), "sin");
    }

    #[test]
    fn portapapeles_y_deshacer() {
        let mut f = Files::default();
        let n = DateTime::default();
        f.write("C:\\Usuarios\\Usuario\\Documentos\\a.txt", b"hola", n).unwrap();
        assert_eq!(f.undo_label(), None);
        // Copiar y pegar en la misma carpeta: nombre libre; deshacer lo manda a la Papelera.
        f.set_clip("C:\\Usuarios\\Usuario\\Documentos\\a.txt", false);
        let p = f.paste("C:\\Usuarios\\Usuario\\Documentos", n).unwrap();
        assert_eq!(p, "C:\\Usuarios\\Usuario\\Documentos\\a (2).txt");
        assert_eq!(f.undo_label(), Some("Copiar"));
        f.undo(n).unwrap();
        assert!(!f.exists(&p) && f.undo_label().is_none());
        // Cortar y pegar en otra carpeta; deshacer lo devuelve.
        f.set_clip("C:\\Usuarios\\Usuario\\Documentos\\a.txt", true);
        let p = f.paste("C:\\Usuarios\\Usuario\\Escritorio", n).unwrap();
        assert!(f.exists(&p) && !f.exists("C:\\Usuarios\\Usuario\\Documentos\\a.txt") && f.clip().is_none());
        assert_eq!(f.undo_label(), Some("Mover"));
        f.undo(n).unwrap();
        assert!(f.exists("C:\\Usuarios\\Usuario\\Documentos\\a.txt"));
        // Cortar y pegar en la misma carpeta no hace nada.
        f.set_clip("C:\\Usuarios\\Usuario\\Documentos\\a.txt", true);
        assert_eq!(f.paste("c:\\usuarios\\usuario\\documentos", n).unwrap(), "C:\\Usuarios\\Usuario\\Documentos\\a.txt");
        assert!(f.undo_label().is_none());
        // Cambiar nombre, eliminar y crear.
        f.user_rename("C:\\Usuarios\\Usuario\\Documentos\\a.txt", "C:\\Usuarios\\Usuario\\Documentos\\b.txt").unwrap();
        f.undo(n).unwrap();
        assert!(f.exists("C:\\Usuarios\\Usuario\\Documentos\\a.txt"));
        f.user_trash("C:\\Usuarios\\Usuario\\Documentos\\a.txt", n).unwrap();
        assert_eq!(f.undo_label(), Some("Eliminar"));
        f.undo(n).unwrap();
        assert_eq!(f.read("C:\\Usuarios\\Usuario\\Documentos\\a.txt").unwrap(), b"hola");
        f.user_create("C:\\Usuarios\\Usuario\\Documentos\\n.txt", false, n).unwrap();
        assert_eq!(f.user_create("C:\\Usuarios\\Usuario\\Documentos\\n.txt", false, n), Err(FsError::Exists));
        f.user_create("C:\\Usuarios\\Usuario\\Documentos\\c", true, n).unwrap();
        f.undo(n).unwrap();
        assert!(!f.exists("C:\\Usuarios\\Usuario\\Documentos\\c") && f.exists("C:\\Usuarios\\Usuario\\Documentos\\n.txt"));
        assert_eq!(f.undo(n), Err(FsError::NotFound));
    }

    fn d(n: u8) -> DateTime {
        DateTime { year: 2026, month: 11, day: n, hour: 9, minute: 8, second: 7 }
    }

    /// Disco en memoria que sigue vivo tras entregárselo a `Files`.
    #[derive(Clone)]
    struct Disk(Rc<RefCell<Vec<u8>>>);
    impl Disk {
        fn new(blocks: usize) -> Disk {
            Disk(Rc::new(RefCell::new(vec![0; blocks * BLOCK])))
        }
    }
    impl BlockDev for Disk {
        fn blocks(&self) -> u64 {
            (self.0.borrow().len() / BLOCK) as u64
        }
        fn read(&mut self, n: u64, buf: &mut [u8]) -> Result<(), DiskError> {
            if n >= self.blocks() || buf.len() != BLOCK {
                return Err(DiskError::Range);
            }
            buf.copy_from_slice(&self.0.borrow()[n as usize * BLOCK..][..BLOCK]);
            Ok(())
        }
        fn write(&mut self, n: u64, buf: &[u8]) -> Result<(), DiskError> {
            if n >= self.blocks() || buf.len() != BLOCK {
                return Err(DiskError::Range);
            }
            self.0.borrow_mut()[n as usize * BLOCK..][..BLOCK].copy_from_slice(buf);
            Ok(())
        }
    }

    impl Files {
        /// Existe `rel` (ruta relativa a la raíz) en el disco, sin pasar por los permisos.
        fn exists_raw(&mut self, rel: &str) -> bool {
            self.fs.stat(rel).is_ok()
        }
    }

    /// Crea la cuenta `name` si no hay ninguna, y abre su sesión (como administradora).
    fn enter(f: &mut Files, name: &str) {
        if f.accounts().is_empty() {
            f.create_account(name, None, true, [7; 16], d(1)).unwrap();
        }
        f.login(name, "").unwrap();
    }

    fn names(f: &mut Files, dir: &str) -> Vec<String> {
        f.list(dir).unwrap().into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn path_helpers() {
        assert_eq!(join("C:\\", "a"), "C:\\a");
        assert_eq!(join("C:\\a", "b"), "C:\\a\\b");
        assert_eq!(join("C:\\a\\", "b"), "C:\\a\\b");
        assert_eq!(parent("C:\\"), None);
        assert_eq!(parent("C:\\a"), Some("C:\\".into()));
        assert_eq!(parent("C:\\a\\b"), Some("C:\\a".into()));
        assert_eq!(parent("C:\\a\\b\\c.txt"), Some("C:\\a\\b".into()));
        assert_eq!(file_name("C:\\"), "");
        assert_eq!(file_name("C:\\a"), "a");
        assert_eq!(file_name("C:\\a\\b.txt"), "b.txt");
        assert_eq!(file_name("C:\\Usuarios\\Usuario\\Imágenes\\ñ"), "ñ");
    }

    #[test]
    fn valid_names() {
        for ok in ["a", "nota.txt", "Ñandú", "a b", "a.b.c", ".oculto", "a-b_c(2)", "日本語", "x."] {
            assert!(valid_name(ok), "{ok:?}");
        }
        for bad in [
            "", ".", "..", "a\\b", "a/b", "a:b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b", "a\tb", "a\nb", "a\u{7f}b",
            "a\u{0}b",
        ] {
            assert!(!valid_name(bad), "{bad:?}");
        }
        assert!(valid_name(&"a".repeat(255)));
        assert!(!valid_name(&"a".repeat(256)));
        assert!(valid_name(&format!("{}x", "ñ".repeat(127))));
        assert!(!valid_name(&"ñ".repeat(128)));
    }

    #[test]
    fn resolve_cases() {
        let r = |cwd: &str, i: &str| resolve(cwd, i);
        assert_eq!(r("C:\\", "C:\\x"), Ok("C:\\x".into()));
        assert_eq!(r("C:\\", "c:/x"), Ok("C:\\x".into()));
        assert_eq!(r("C:\\a", "b"), Ok("C:\\a\\b".into()));
        assert_eq!(r("C:\\a", "b\\c"), Ok("C:\\a\\b\\c".into()));
        assert_eq!(r("C:\\a\\b", ".."), Ok("C:\\a".into()));
        assert_eq!(r("C:\\a\\b", "..\\.."), Ok("C:\\".into()));
        assert_eq!(r("C:\\a\\b", "..\\..\\.."), Ok("C:\\".into()));
        assert_eq!(r("C:\\", ".."), Ok("C:\\".into()));
        assert_eq!(r("C:\\", "C:\\.."), Ok("C:\\".into()));
        assert_eq!(r("C:\\a", "."), Ok("C:\\a".into()));
        assert_eq!(r("C:\\a", "./b/./c"), Ok("C:\\a\\b\\c".into()));
        assert_eq!(r("C:\\a", "C:\\\\x\\\\\\y\\"), Ok("C:\\x\\y".into()));
        assert_eq!(r("C:\\a", "\\x"), Ok("C:\\x".into()));
        assert_eq!(r("C:\\a", "/x"), Ok("C:\\x".into()));
        assert_eq!(r("C:\\a", "C:"), Ok("C:\\".into()));
        assert_eq!(r("C:\\a", "C:\\"), Ok("C:\\".into()));
        assert_eq!(r("C:\\a", "c:"), Ok("C:\\".into()));
        assert_eq!(r("C:\\a", ""), Ok("C:\\a".into()));
        assert_eq!(r("C:\\a", "b\\..\\c"), Ok("C:\\a\\c".into()));
        assert_eq!(r("C:\\", "Imágenes\\Ñu"), Ok("C:\\Imágenes\\Ñu".into()));
        // Cualquier letra vale en la ruta (que la unidad exista lo mira cada operación) y se conserva.
        assert_eq!(r("C:\\", "D:\\x"), Ok("D:\\x".into()));
        assert_eq!(r("C:\\", "z:x"), Ok("Z:\\x".into()));
        assert_eq!(r("C:\\", "x\\a:b"), Err(FsError::BadPath));
        assert_eq!(r("D:\\", "x"), Ok("D:\\x".into()));
        assert_eq!(r("D:\\a", "..\\b"), Ok("D:\\b".into()));
        assert_eq!(r("D:\\a", "\\x"), Ok("D:\\x".into()));
        assert_eq!(r("D:\\a", "c:x"), Ok("C:\\x".into()));
        assert_eq!(parent("D:\\a"), Some("D:\\".into()));
        assert_eq!(parent("D:\\a\\b"), Some("D:\\a".into()));
        assert_eq!(parent("D:\\"), None);
        assert_eq!(r("C:\\", "a*b"), Err(FsError::BadName));
        assert_eq!(r("C:\\", &"a".repeat(300)), Err(FsError::BadName));
        assert_eq!(r("C:\\", "C:\\a\\?"), Err(FsError::BadName));
    }

    #[test]
    fn default_drive_has_initial_folders() {
        let mut f = Files::default();
        assert!(!f.persistent());
        assert!(!f.pending_format());
        assert!(f.notice().is_some());
        assert_eq!(names(&mut f, "C:\\"), ["Programas", "Sistema", "Usuarios"]);
        for s in Std::ALL {
            assert!(f.stat(&f.std_dir(s)).unwrap().dir);
        }
        for p in ["C:\\Usuarios", "C:\\Usuarios\\Usuario\\.config", "C:\\Sistema\\temp", "C:\\Sistema\\logs", "C:\\Sistema", "c:/usuarios/usuario/documentos", "c:/usuarios/usuario/imágenes"] {
            assert!(f.stat(p).unwrap().dir, "{p}");
        }
        assert!(f.exists("C:\\"));
        let (total, free) = f.space();
        assert_eq!(total, 32 << 20);
        assert!(free > 3 << 20);
        // el almacén de la Papelera no se ve ni se toca
        for p in ["C:\\$Papelera", "c:\\$PAPELERA\\indice", "C:\\$Papelera\\x"] {
            assert_eq!(f.list(p), Err(FsError::BadPath), "{p}");
            assert_eq!(f.stat(p), Err(FsError::BadPath));
            assert_eq!(f.read(p), Err(FsError::BadPath));
            assert_eq!(f.write(p, b"x", d(1)), Err(FsError::BadPath));
            assert_eq!(f.mkdir(p, d(1)), Err(FsError::BadPath));
            assert_eq!(f.delete(p), Err(FsError::BadPath));
            assert_eq!(f.trash(p, d(1)), Err(FsError::BadPath));
            assert_eq!(f.rename(p, "C:\\z"), Err(FsError::BadPath));
            assert_eq!(f.rename("C:\\Usuarios\\Usuario\\Documentos", p), Err(FsError::BadPath));
            assert_eq!(f.copy("C:\\Usuarios\\Usuario\\Documentos", p, d(1)), Err(FsError::BadPath));
            assert_eq!(f.copy(p, "C:\\z", d(1)), Err(FsError::BadPath));
            assert!(!f.exists(p));
        }
        assert_eq!(f.write("C:\\Usuarios\\Usuario\\Documentos\\..\\..\\..\\$Papelera\\x", b"1", d(1)), Err(FsError::BadPath));
    }

    #[test]
    fn list_order_and_basic_io() {
        let mut f = Files::default();
        f.write("C:\\Usuarios\\Usuario\\Documentos\\b.txt", b"b", d(1)).unwrap();
        f.write("C:\\Usuarios\\Usuario\\Documentos\\A.txt", b"aaa", d(2)).unwrap();
        f.write("C:\\Usuarios\\Usuario\\Documentos\\ñu.txt", b"n", d(3)).unwrap();
        f.write("C:\\Usuarios\\Usuario\\Documentos\\zeta", b"z", d(3)).unwrap();
        f.mkdir("C:\\Usuarios\\Usuario\\Documentos\\Zona", d(4)).unwrap();
        f.mkdir("C:\\Usuarios\\Usuario\\Documentos\\carpeta", d(4)).unwrap();
        assert_eq!(names(&mut f, "C:\\Usuarios\\Usuario\\Documentos"), ["carpeta", "Zona", "A.txt", "b.txt", "zeta", "ñu.txt"]);
        let e = f.stat("c:/usuarios/usuario/documentos/a.TXT").unwrap();
        assert_eq!((e.name.as_str(), e.size, e.modified, e.dir), ("A.txt", 3, d(2), false));
        assert_eq!(f.read("C:\\Usuarios\\Usuario\\Documentos\\A.txt").unwrap(), b"aaa");
        assert_eq!(f.write("C:\\", b"x", d(1)), Err(FsError::IsDir));
        assert_eq!(f.mkdir("C:\\", d(1)), Err(FsError::Exists));
        assert_eq!(f.mkdir("C:\\Usuarios\\Usuario\\Documentos", d(1)), Err(FsError::Exists));
        assert_eq!(f.read("C:\\nada"), Err(FsError::NotFound));
        assert_eq!(f.read("D:\\x"), Err(FsError::BadPath));
        assert_eq!(f.write("C:\\a*", b"x", d(1)), Err(FsError::BadName));
    }

    #[test]
    fn copy_and_delete_are_recursive() {
        let mut f = Files::default();
        f.mkdir("C:\\Usuarios\\Usuario\\Documentos\\p", d(1)).unwrap();
        f.mkdir("C:\\Usuarios\\Usuario\\Documentos\\p\\q", d(1)).unwrap();
        f.mkdir("C:\\Usuarios\\Usuario\\Documentos\\p\\vacia", d(1)).unwrap();
        f.write("C:\\Usuarios\\Usuario\\Documentos\\p\\a.txt", b"A", d(1)).unwrap();
        f.write("C:\\Usuarios\\Usuario\\Documentos\\p\\q\\b.bin", &vec![7u8; 10_000], d(1)).unwrap();
        f.copy("C:\\Usuarios\\Usuario\\Documentos\\p", "C:\\Usuarios\\Usuario\\Escritorio\\copia", d(5)).unwrap();
        assert_eq!(f.read("C:\\Usuarios\\Usuario\\Escritorio\\copia\\q\\b.bin").unwrap(), vec![7u8; 10_000]);
        assert_eq!(f.read("C:\\Usuarios\\Usuario\\Escritorio\\copia\\a.txt").unwrap(), b"A");
        assert!(f.stat("C:\\Usuarios\\Usuario\\Escritorio\\copia\\vacia").unwrap().dir);
        assert!(f.exists("C:\\Usuarios\\Usuario\\Documentos\\p\\a.txt"));
        // archivo suelto
        f.copy("C:\\Usuarios\\Usuario\\Documentos\\p\\a.txt", "C:\\Usuarios\\Usuario\\Documentos\\a2.txt", d(5)).unwrap();
        assert_eq!(f.read("C:\\Usuarios\\Usuario\\Documentos\\a2.txt").unwrap(), b"A");
        // errores
        assert_eq!(f.copy("C:\\Usuarios\\Usuario\\Documentos\\p", "C:\\Usuarios\\Usuario\\Escritorio\\copia", d(5)), Err(FsError::Exists));
        assert_eq!(f.copy("C:\\Usuarios\\Usuario\\Documentos\\p", "C:\\Usuarios\\Usuario\\Documentos\\p\\q\\dentro", d(5)), Err(FsError::Invalid));
        assert_eq!(f.copy("C:\\Usuarios\\Usuario\\Documentos\\p", "C:\\USUARIOS\\USUARIO\\DOCUMENTOS\\P\\dentro", d(5)), Err(FsError::Invalid));
        assert!(!f.exists("C:\\Usuarios\\Usuario\\Documentos\\p\\q\\dentro"));
        assert_eq!(f.copy("C:\\", "C:\\Usuarios\\Usuario\\Escritorio\\todo", d(5)), Err(FsError::Invalid));
        assert_eq!(f.copy("C:\\nada", "C:\\x", d(5)), Err(FsError::NotFound));
        assert_eq!(f.copy("C:\\Usuarios\\Usuario\\Documentos\\a2.txt", "C:\\nada\\x", d(5)), Err(FsError::NotFound));
        assert!(!f.exists("C:\\nada"));
        // un nombre parecido no cuenta como "dentro"
        f.copy("C:\\Usuarios\\Usuario\\Documentos\\p", "C:\\Usuarios\\Usuario\\Documentos\\p2", d(5)).unwrap();
        // borrado
        let (_, free) = f.space();
        f.delete("C:\\Usuarios\\Usuario\\Escritorio\\copia").unwrap();
        assert!(!f.exists("C:\\Usuarios\\Usuario\\Escritorio\\copia"));
        assert!(f.space().1 > free);
        assert_eq!(f.delete("C:\\Usuarios\\Usuario\\Escritorio\\copia"), Err(FsError::NotFound));
        f.delete("C:\\Usuarios\\Usuario\\Documentos\\a2.txt").unwrap();
        assert!(f.exists("C:\\Usuarios\\Usuario\\Documentos\\p2\\q\\b.bin"));
    }

    #[test]
    fn copy_that_runs_out_of_space_leaves_nothing() {
        let mut f = Files::default();
        f.write("C:\\Usuarios\\Usuario\\Documentos\\grande", &vec![1u8; 20_000_000], d(1)).unwrap();
        f.mkdir("C:\\Usuarios\\Usuario\\Documentos\\c", d(1)).unwrap();
        f.write("C:\\Usuarios\\Usuario\\Documentos\\c\\g", &vec![2u8; 9_600_000], d(1)).unwrap();
        let (_, free) = f.space();
        assert_eq!(f.copy("C:\\Usuarios\\Usuario\\Documentos\\c", "C:\\Usuarios\\Usuario\\Escritorio\\c", d(2)), Err(FsError::NoSpace));
        assert!(!f.exists("C:\\Usuarios\\Usuario\\Escritorio\\c"));
        assert_eq!(f.space().1, free);
    }

    #[test]
    fn free_name_cases() {
        let mut f = Files::default();
        let dir = "C:\\Usuarios\\Usuario\\Documentos";
        assert_eq!(f.free_name(dir, "nota.txt"), "nota.txt");
        f.write("C:\\Usuarios\\Usuario\\Documentos\\nota.txt", b"1", d(1)).unwrap();
        assert_eq!(f.free_name(dir, "nota.txt"), "nota (2).txt");
        assert_eq!(f.free_name(dir, "NOTA.TXT"), "NOTA (2).TXT");
        f.write("C:\\Usuarios\\Usuario\\Documentos\\nota (2).txt", b"1", d(1)).unwrap();
        f.write("C:\\Usuarios\\Usuario\\Documentos\\nota (3).txt", b"1", d(1)).unwrap();
        assert_eq!(f.free_name(dir, "nota.txt"), "nota (4).txt");
        f.mkdir("C:\\Usuarios\\Usuario\\Documentos\\carpeta", d(1)).unwrap();
        assert_eq!(f.free_name(dir, "carpeta"), "carpeta (2)");
        f.mkdir("C:\\Usuarios\\Usuario\\Documentos\\carpeta (2)", d(1)).unwrap();
        assert_eq!(f.free_name(dir, "carpeta"), "carpeta (3)");
        f.write("C:\\Usuarios\\Usuario\\Documentos\\.oculto", b"1", d(1)).unwrap();
        assert_eq!(f.free_name(dir, ".oculto"), ".oculto (2)");
        f.write("C:\\Usuarios\\Usuario\\Documentos\\a.tar.gz", b"1", d(1)).unwrap();
        assert_eq!(f.free_name(dir, "a.tar.gz"), "a.tar (2).gz");
        assert_eq!(f.free_name("C:\\Usuarios\\Usuario\\Documentos\\nada", "x"), "x");
        assert_eq!(f.free_name("D:\\", "x"), "x");
    }

    #[test]
    fn rename_and_protected_folders() {
        let mut f = Files::default();
        f.write("C:\\Usuarios\\Usuario\\Documentos\\a.txt", b"1", d(1)).unwrap();
        f.rename("C:\\Usuarios\\Usuario\\Documentos\\a.txt", "C:\\Usuarios\\Usuario\\Escritorio\\b.txt").unwrap();
        assert_eq!(f.read("C:\\Usuarios\\Usuario\\Escritorio\\b.txt").unwrap(), b"1");
        assert_eq!(f.rename("C:\\Usuarios\\Usuario\\Documentos\\a.txt", "C:\\x"), Err(FsError::NotFound));
        f.write("C:\\Usuarios\\Usuario\\Documentos\\c.txt", b"2", d(1)).unwrap();
        assert_eq!(f.rename("C:\\Usuarios\\Usuario\\Documentos\\c.txt", "C:\\Usuarios\\Usuario\\Escritorio\\B.TXT"), Err(FsError::Exists));
        assert_eq!(f.rename("C:\\Usuarios\\Usuario\\Documentos", "C:\\Usuarios\\Usuario\\Documentos\\x"), Err(FsError::Invalid));
        f.rename("c:/usuarios/usuario/escritorio/b.txt", "C:\\Usuarios\\Usuario\\Escritorio\\B.txt").unwrap();
        // raíz y Sistema
        for p in ["C:\\", "C:\\Sistema", "c:\\sistema", "C:\\Usuarios\\Usuario\\..\\..\\Sistema"] {
            assert_eq!(f.delete(p), Err(FsError::Invalid), "{p}");
            assert_eq!(f.rename(p, "C:\\otro"), Err(FsError::Invalid), "{p}");
            assert_eq!(f.trash(p, d(1)), Err(FsError::Invalid), "{p}");
        }
        assert_eq!(f.rename("C:\\Usuarios\\Usuario\\Documentos", "C:\\"), Err(FsError::Invalid));
        assert!(f.exists("C:\\Sistema"));
        // dentro de Sistema sí se puede trabajar
        f.write("C:\\Sistema\\temp\\x.txt", b"fondo=1\n", d(1)).unwrap();
        f.delete("C:\\Sistema\\temp\\x.txt").unwrap();
    }

    #[test]
    fn generation_changes_only_on_modification() {
        let mut f = Files::default();
        let g0 = f.generation();
        let _ = f.list("C:\\");
        let _ = f.stat("C:\\Usuarios\\Usuario\\Documentos");
        let _ = f.read("C:\\nada");
        let _ = f.trash_list();
        assert_eq!(f.generation(), g0);
        let mut last = g0;
        let mut step = |f: &mut Files| {
            assert!(f.generation() > last);
            last = f.generation();
        };
        f.write("C:\\Usuarios\\Usuario\\Documentos\\a", b"1", d(1)).unwrap();
        step(&mut f);
        f.mkdir("C:\\Usuarios\\Usuario\\Documentos\\m", d(1)).unwrap();
        step(&mut f);
        f.rename("C:\\Usuarios\\Usuario\\Documentos\\a", "C:\\Usuarios\\Usuario\\Documentos\\b").unwrap();
        step(&mut f);
        f.copy("C:\\Usuarios\\Usuario\\Documentos\\b", "C:\\Usuarios\\Usuario\\Documentos\\c", d(1)).unwrap();
        step(&mut f);
        f.trash("C:\\Usuarios\\Usuario\\Documentos\\c", d(1)).unwrap();
        step(&mut f);
        let id = f.trash_list().unwrap()[0].id.clone();
        f.restore(&id).unwrap();
        step(&mut f);
        f.trash("C:\\Usuarios\\Usuario\\Documentos\\c", d(1)).unwrap();
        step(&mut f);
        f.empty_trash().unwrap();
        step(&mut f);
        f.delete("C:\\Usuarios\\Usuario\\Documentos\\b").unwrap();
        step(&mut f);
    }

    #[test]
    fn trash_full_cycle() {
        let mut f = Files::default();
        assert!(f.trash_list().unwrap().is_empty());
        f.write("C:\\Usuarios\\Usuario\\Documentos\\nota.txt", b"hola", d(1)).unwrap();
        f.mkdir("C:\\Usuarios\\Usuario\\Documentos\\carp", d(1)).unwrap();
        f.write("C:\\Usuarios\\Usuario\\Documentos\\carp\\x.bin", &vec![9u8; 5000], d(1)).unwrap();
        f.trash("C:\\Usuarios\\Usuario\\Documentos\\nota.txt", d(10)).unwrap();
        f.trash("C:\\Usuarios\\Usuario\\Documentos\\carp", d(11)).unwrap();
        // desaparecen del sitio y de la raíz no aparece el almacén
        assert!(!f.exists("C:\\Usuarios\\Usuario\\Documentos\\nota.txt"));
        assert!(!f.exists("C:\\Usuarios\\Usuario\\Documentos\\carp"));
        assert_eq!(names(&mut f, "C:\\"), ["Programas", "Sistema", "Usuarios"]);
        let items = f.trash_list().unwrap();
        assert_eq!(items.len(), 2);
        // lo más reciente primero
        assert_eq!(items[0].name, "carp");
        assert_eq!(items[0].original, "C:\\Usuarios\\Usuario\\Documentos\\carp");
        assert!(items[0].dir);
        assert_eq!(items[0].deleted, d(11));
        assert_eq!(items[1].name, "nota.txt");
        assert_eq!((items[1].dir, items[1].size, items[1].deleted), (false, 4, d(10)));
        assert_ne!(items[0].id, items[1].id);
        // restaurar
        let p = f.restore(&items[1].id).unwrap();
        assert_eq!(p, "C:\\Usuarios\\Usuario\\Documentos\\nota.txt");
        assert_eq!(f.read(&p).unwrap(), b"hola");
        assert_eq!(f.trash_list().unwrap().len(), 1);
        assert_eq!(f.restore(&items[1].id), Err(FsError::NotFound));
        assert_eq!(f.restore("inventado"), Err(FsError::NotFound));
        // conflicto de nombre
        f.trash(&p, d(12)).unwrap();
        f.write(&p, b"nueva", d(13)).unwrap();
        let id = f.trash_list().unwrap()[0].id.clone();
        assert_eq!(f.restore(&id).unwrap(), "C:\\Usuarios\\Usuario\\Documentos\\nota (2).txt");
        assert_eq!(f.read("C:\\Usuarios\\Usuario\\Documentos\\nota (2).txt").unwrap(), b"hola");
        assert_eq!(f.read(&p).unwrap(), b"nueva");
        // la carpeta original ya no existe: se recrea
        f.mkdir("C:\\Usuarios\\Usuario\\Escritorio\\a", d(1)).unwrap();
        f.mkdir("C:\\Usuarios\\Usuario\\Escritorio\\a\\b", d(1)).unwrap();
        f.write("C:\\Usuarios\\Usuario\\Escritorio\\a\\b\\f.txt", b"f", d(1)).unwrap();
        f.trash("C:\\Usuarios\\Usuario\\Escritorio\\a\\b\\f.txt", d(14)).unwrap();
        f.delete("C:\\Usuarios\\Usuario\\Escritorio\\a").unwrap();
        let id = f.trash_list().unwrap()[0].id.clone();
        assert_eq!(f.restore(&id).unwrap(), "C:\\Usuarios\\Usuario\\Escritorio\\a\\b\\f.txt");
        assert_eq!(f.read("C:\\Usuarios\\Usuario\\Escritorio\\a\\b\\f.txt").unwrap(), b"f");
        // la carpeta con contenido se restaura entera
        let id = f.trash_list().unwrap()[0].id.clone();
        assert_eq!(f.trash_list().unwrap()[0].name, "carp");
        assert_eq!(f.restore(&id).unwrap(), "C:\\Usuarios\\Usuario\\Documentos\\carp");
        assert_eq!(f.read("C:\\Usuarios\\Usuario\\Documentos\\carp\\x.bin").unwrap(), vec![9u8; 5000]);
        assert!(f.trash_list().unwrap().is_empty());
        // vaciar
        f.trash("C:\\Usuarios\\Usuario\\Documentos\\carp", d(15)).unwrap();
        f.trash("C:\\Usuarios\\Usuario\\Documentos\\nota (2).txt", d(15)).unwrap();
        assert_eq!(f.trash_list().unwrap().len(), 2);
        let (_, free) = f.space();
        f.empty_trash().unwrap();
        assert!(f.trash_list().unwrap().is_empty());
        assert!(f.space().1 > free);
        f.empty_trash().unwrap();
        // se sigue pudiendo usar
        f.trash("C:\\Usuarios\\Usuario\\Documentos\\nota.txt", d(16)).unwrap();
        assert_eq!(f.trash_list().unwrap().len(), 1);
        assert_eq!(f.trash("C:\\Usuarios\\Usuario\\Documentos\\nota.txt", d(16)), Err(FsError::NotFound));
        assert_eq!(f.trash("C:\\Usuarios\\Usuario\\Documentos\\ya-no-esta", d(16)), Err(FsError::NotFound));
    }

    #[test]
    fn trash_survives_remount_and_cuts_between_steps_are_harmless() {
        let disk = Disk::new(1024);
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(1));
        f.format_pending(d(1)).unwrap();
        enter(&mut f, DEFAULT_USER);
        f.write("C:\\Usuarios\\Usuario\\Documentos\\a.txt", b"A", d(2)).unwrap();
        f.trash("C:\\Usuarios\\Usuario\\Documentos\\a.txt", d(3)).unwrap();
        drop(f);
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(4));
        assert!(f.persistent());
        enter(&mut f, DEFAULT_USER);
        let items = f.trash_list().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].deleted, d(3));
        // una línea de índice sin elemento (corte entre los dos pasos) se ignora
        f.fs.write("Usuarios\\Usuario\\.papelera\\indice", b"00000007|2026-11-3-1-2-3|C:\\Usuarios\\Usuario\\Documentos\\fantasma\n", d(5)).unwrap();
        assert!(f.trash_list().unwrap().is_empty());
        f.write("C:\\Usuarios\\Usuario\\Documentos\\b.txt", b"B", d(5)).unwrap();
        f.trash("C:\\Usuarios\\Usuario\\Documentos\\b.txt", d(6)).unwrap();
        let items = f.trash_list().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(f.restore(&items[0].id).unwrap(), "C:\\Usuarios\\Usuario\\Documentos\\b.txt");
    }

    #[test]
    fn probe_without_disk() {
        let mut f = Files::probe(None, d(1));
        assert!(!f.persistent() && !f.pending_format());
        assert!(f.notice().is_some());
        assert_eq!(names(&mut f, "C:\\"), ["Programas", "Sistema", "Usuarios"]);
        assert_eq!(f.format_pending(d(1)), Err(FsError::Invalid));
    }

    #[test]
    fn probe_with_nffs_is_persistent() {
        let disk = Disk::new(1024);
        let mut fs = Nffs::format(Box::new(disk.clone()), d(1)).unwrap();
        fs.mkdir("Mio", d(1)).unwrap();
        fs.write("Mio\\x.txt", b"x", d(1)).unwrap();
        drop(fs);
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(2));
        assert!(f.persistent());
        assert!(!f.pending_format());
        assert_eq!(f.notice(), None);
        assert_eq!(f.read("C:\\Mio\\x.txt").unwrap(), b"x");
        // asegura las carpetas iniciales
        assert_eq!(names(&mut f, "C:\\"), ["Mio", "Programas", "Sistema", "Usuarios"]);
        enter(&mut f, DEFAULT_USER);
        f.write("C:\\Usuarios\\Usuario\\Documentos\\n.txt", b"dato", d(3)).unwrap();
        drop(f);
        let mut f = Files::probe(Some(Box::new(disk)), d(4));
        enter(&mut f, DEFAULT_USER);
        assert_eq!(f.read("C:\\Usuarios\\Usuario\\Documentos\\n.txt").unwrap(), b"dato");
        assert_eq!(f.stat("C:\\Usuarios\\Usuario\\Documentos\\n.txt").unwrap().modified, d(3));
    }

    #[test]
    fn probe_with_blank_disk_is_pending_and_decline_works() {
        let disk = Disk::new(1024);
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(1));
        assert!(!f.persistent());
        assert!(f.pending_format());
        let notice_new = f.notice().unwrap();
        f.write("C:\\Usuarios\\Usuario\\Documentos\\a.txt", b"mem", d(1)).unwrap();
        f.decline_format();
        assert!(!f.pending_format());
        assert!(!f.persistent());
        assert!(f.notice().is_some());
        assert_eq!(f.format_pending(d(2)), Err(FsError::Invalid));
        assert_eq!(f.read("C:\\Usuarios\\Usuario\\Documentos\\a.txt").unwrap(), b"mem");
        // el disco no se ha tocado
        assert!(disk.0.borrow().iter().all(|&b| b == 0));
        assert_ne!(notice_new, "");
    }

    #[test]
    fn probe_with_other_content_is_ignored_and_untouched() {
        let disk = Disk::new(1024);
        disk.0.borrow_mut()[510] = 0x55;
        disk.0.borrow_mut()[511] = 0xaa; // como un sector de arranque de otro sistema
        let before = disk.0.borrow().clone();
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(1));
        assert!(!f.persistent() && !f.pending_format());
        assert!(f.notice().is_some());
        f.write("C:\\Usuarios\\Usuario\\Documentos\\a.txt", b"mem", d(1)).unwrap();
        assert_eq!(f.format_pending(d(1)), Err(FsError::Invalid));
        assert_eq!(*disk.0.borrow(), before);
        // contenido en medio del disco, pero cabeceras en blanco: sigue contando como en blanco
        let disk = Disk::new(1024);
        disk.0.borrow_mut()[600 * BLOCK] = 1;
        assert!(Files::probe(Some(Box::new(disk)), d(1)).pending_format());
    }

    #[test]
    fn probe_with_damaged_nffs_never_formats() {
        let disk = Disk::new(1024);
        let fs = Nffs::format(Box::new(disk.clone()), d(1)).unwrap();
        drop(fs);
        disk.0.borrow_mut()[2 * BLOCK + 100] ^= 0xff; // metadatos (bloque 2) rotos, superbloque válido
        assert_eq!(Nffs::mount(Box::new(disk.clone())).err(), Some(FsError::Corrupt));
        let before = disk.0.borrow().clone();
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(2));
        assert!(!f.persistent());
        assert!(!f.pending_format());
        assert!(f.notice().is_some());
        assert_eq!(f.format_pending(d(2)), Err(FsError::Invalid));
        f.write("C:\\Usuarios\\Usuario\\Documentos\\a", b"1", d(2)).unwrap();
        assert_eq!(*disk.0.borrow(), before, "no se debe escribir nada en un disco dañado");
    }

    #[test]
    fn notices_are_different_for_each_case() {
        let none = Files::probe(None, d(1));
        let ignored = {
            let disk = Disk::new(1024);
            disk.0.borrow_mut()[0] = 1;
            Files::probe(Some(Box::new(disk)), d(1))
        };
        let damaged = {
            let disk = Disk::new(1024);
            drop(Nffs::format(Box::new(disk.clone()), d(1)).unwrap());
            disk.0.borrow_mut()[2 * BLOCK + 50] ^= 1;
            Files::probe(Some(Box::new(disk)), d(1))
        };
        let new = Files::probe(Some(Box::new(Disk::new(1024))), d(1));
        let all = [none.notice(), ignored.notice(), damaged.notice(), new.notice()];
        for (i, a) in all.iter().enumerate() {
            let a = a.unwrap();
            assert!(!a.is_empty());
            for b in &all[i + 1..] {
                assert_ne!(Some(a), *b);
            }
        }
    }

    #[test]
    fn format_pending_keeps_what_was_in_memory() {
        let disk = Disk::new(2048);
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(1));
        assert!(f.pending_format());
        f.write("C:\\Usuarios\\Usuario\\Documentos\\a.txt", b"uno", d(2)).unwrap();
        f.mkdir("C:\\Usuarios\\Usuario\\Documentos\\sub", d(3)).unwrap();
        f.write("C:\\Usuarios\\Usuario\\Documentos\\sub\\b.bin", &vec![5u8; 70_000], d(4)).unwrap();
        f.write("C:\\Usuarios\\Usuario\\Escritorio\\ñ.txt", b"n", d(5)).unwrap();
        let cfg = f.config_file();
        f.write(&cfg, b"fondo=2\n", d(6)).unwrap();
        f.write("C:\\Usuarios\\Usuario\\Documentos\\borrado.txt", b"papelera", d(7)).unwrap();
        f.trash("C:\\Usuarios\\Usuario\\Documentos\\borrado.txt", d(8)).unwrap();
        let g = f.generation();
        f.format_pending(d(9)).unwrap();
        assert!(f.generation() > g);
        assert!(f.persistent() && !f.pending_format() && f.notice().is_none());
        // Sin sesión: asistente pendiente y nada de lo de la memoria al alcance.
        assert!(f.user().is_none() && f.needs_setup());
        assert_eq!(f.read("C:\\Usuarios\\Usuario\\Documentos\\a.txt"), Err(FsError::Denied));
        // La primera cuenta (con otro nombre) adopta la carpeta de la memoria.
        f.create_account("Ana", None, false, [1; 16], d(10)).unwrap();
        assert!(!f.exists_raw("Usuarios\\Usuario"), "la carpeta de la memoria ya no está");
        assert!(!f.exists_raw("Sistema\\config\\migracion"));
        f.login("Ana", "").unwrap();
        let ana = "C:\\Usuarios\\Ana";
        assert_eq!(f.read(&format!("{ana}\\Documentos\\a.txt")).unwrap(), b"uno");
        assert_eq!(f.read(&format!("{ana}\\Documentos\\sub\\b.bin")).unwrap(), vec![5u8; 70_000]);
        assert_eq!(f.read(&format!("{ana}\\Escritorio\\ñ.txt")).unwrap(), b"n");
        assert_eq!(f.read(&f.config_file()).unwrap(), b"fondo=2\n");
        assert_eq!(f.stat(&format!("{ana}\\Documentos\\a.txt")).unwrap().modified, d(2));
        assert_eq!(f.stat(&format!("{ana}\\Documentos\\sub")).unwrap().modified, d(3));
        let items = f.trash_list().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].original, format!("{ana}\\Documentos\\borrado.txt"));
        assert_eq!(items[0].deleted, d(8));
        assert_eq!(f.restore(&items[0].id).unwrap(), format!("{ana}\\Documentos\\borrado.txt"));
        // y está de verdad en el disco
        drop(f);
        let mut f = Files::probe(Some(Box::new(disk)), d(11));
        f.login("Ana", "").unwrap();
        assert_eq!(f.read(&format!("{ana}\\Documentos\\sub\\b.bin")).unwrap().len(), 70_000);
        assert_eq!(f.read(&format!("{ana}\\Documentos\\borrado.txt")).unwrap(), b"papelera");
    }

    #[test]
    fn format_pending_on_tiny_disk_fails_and_keeps_memory_drive() {
        let mut f = Files::probe(Some(Box::new(Disk::new(100))), d(1));
        assert!(f.pending_format());
        f.write("C:\\Usuarios\\Usuario\\Documentos\\gran.bin", &vec![1u8; 2_000_000], d(2)).unwrap();
        assert_eq!(f.format_pending(d(3)), Err(FsError::NoSpace));
        assert!(!f.persistent());
        assert_eq!(f.read("C:\\Usuarios\\Usuario\\Documentos\\gran.bin").unwrap().len(), 2_000_000);
        let mut f = Files::probe(Some(Box::new(Disk::new(100))), d(1));
        f.write("C:\\a.txt", b"1", d(1)).unwrap();
        f.format_pending(d(2)).unwrap();
        assert_eq!(f.read("C:\\a.txt").unwrap(), b"1");
    }

    // ---- v0.5.0: estructura, permisos, cuentas y migración ----

    use std::cell::Cell;

    const ANA: &str = "C:\\Usuarios\\Ana";

    /// Disco que deja de escribir tras `left` bloques (un corte de corriente).
    struct Cut {
        disk: Disk,
        left: Rc<Cell<u32>>,
        failed: Rc<Cell<bool>>,
    }
    impl BlockDev for Cut {
        fn blocks(&self) -> u64 {
            self.disk.blocks()
        }
        fn read(&mut self, n: u64, buf: &mut [u8]) -> Result<(), DiskError> {
            self.disk.read(n, buf)
        }
        fn write(&mut self, n: u64, buf: &[u8]) -> Result<(), DiskError> {
            if self.left.get() == 0 {
                self.failed.set(true);
                return Err(DiskError::Io);
            }
            self.left.set(self.left.get() - 1);
            self.disk.write(n, buf)
        }
    }

    /// Imagen con la estructura de la v0.4: carpetas en la raíz, ajustes en Sistema y Papelera con dos elementos.
    fn old_disk() -> Disk {
        let disk = Disk::new(2048);
        let mut fs = Nffs::format(Box::new(disk.clone()), d(1)).unwrap();
        for p in ["Documentos", "Documentos\\sub", "Imágenes", "Escritorio", "Sistema", "$Papelera", "$Papelera\\00000002"] {
            fs.mkdir(p, d(1)).unwrap();
        }
        fs.write("Documentos\\a.txt", b"A", d(2)).unwrap();
        fs.write("Documentos\\sub\\b.bin", &vec![3u8; 9000], d(2)).unwrap();
        fs.write("Imágenes\\f.bmp", b"BM", d(2)).unwrap();
        fs.write("Escritorio\\e.txt", b"E", d(2)).unwrap();
        fs.write("Sistema\\config.txt", b"fondo=3\n", d(2)).unwrap();
        fs.write("Sistema\\explorador.txt", b"vista=lista\n", d(2)).unwrap();
        fs.write("$Papelera\\00000001", b"borrado", d(2)).unwrap();
        fs.write("$Papelera\\00000002\\x.txt", b"X", d(2)).unwrap();
        fs.write(
            "$Papelera\\indice",
            b"00000001|2026-11-8-1-2-3|C:\\Documentos\\borrado.txt\n00000002|2026-11-9-1-2-3|C:\\Escritorio\\carp\n",
            d(2),
        )
        .unwrap();
        disk
    }

    /// Comprueba que `Ana` tiene todo lo de `old_disk` y que no queda nada antiguo.
    fn check_migrated(f: &mut Files) {
        f.login("Ana", "").unwrap();
        assert_eq!(f.read(&format!("{ANA}\\Documentos\\a.txt")).unwrap(), b"A");
        assert_eq!(f.read(&format!("{ANA}\\Documentos\\sub\\b.bin")).unwrap(), vec![3u8; 9000]);
        assert_eq!(f.read(&format!("{ANA}\\Imágenes\\f.bmp")).unwrap(), b"BM");
        assert_eq!(f.read(&format!("{ANA}\\Escritorio\\e.txt")).unwrap(), b"E");
        assert_eq!(f.read(&f.config_file()).unwrap(), b"fondo=3\n");
        assert_eq!(f.read(&f.explorer_prefs()).unwrap(), b"vista=lista\n");
        for old in ["Documentos", "Imágenes", "Escritorio", "$Papelera", "Sistema\\config.txt", "Sistema\\explorador.txt", "Sistema\\config\\migracion", "Sistema\\config\\antiguo"] {
            assert!(!f.exists_raw(old), "{old}");
        }
        // La Papelera antigua se restaura a las rutas nuevas.
        let items = f.trash_list().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].original, format!("{ANA}\\Escritorio\\carp"));
        assert_eq!(items[1].original, format!("{ANA}\\Documentos\\borrado.txt"));
        assert_eq!(f.restore(&items[0].id).unwrap(), format!("{ANA}\\Escritorio\\carp"));
        assert_eq!(f.read(&format!("{ANA}\\Escritorio\\carp\\x.txt")).unwrap(), b"X");
        assert_eq!(f.restore(&items[1].id).unwrap(), format!("{ANA}\\Documentos\\borrado.txt"));
        assert_eq!(f.read(&format!("{ANA}\\Documentos\\borrado.txt")).unwrap(), b"borrado");
        assert!(f.trash_list().unwrap().is_empty());
    }

    /// Disco formateado y montado, sin cuentas.
    fn fresh(disk: &Disk) -> Files {
        drop(Nffs::format(Box::new(disk.clone()), d(1)).unwrap());
        Files::probe(Some(Box::new(disk.clone())), d(1))
    }

    #[test]
    fn estructura_inicial_en_memoria_y_en_disco() {
        let mut m = Files::default();
        assert_eq!(names(&mut m, "C:\\"), ["Programas", "Sistema", "Usuarios"]);
        assert_eq!(names(&mut m, "C:\\Sistema"), ["config", "drivers", "lib", "logs", "temp"]);
        assert_eq!(names(&mut m, "C:\\Usuarios"), [DEFAULT_USER]);
        assert_eq!(m.home(), "C:\\Usuarios\\Usuario");
        let h = m.home();
        assert_eq!(names(&mut m, &h), ["Descargas", "Documentos", "Escritorio", "Imágenes", "Música", "Vídeos"]);
        assert_eq!(m.config_file(), "C:\\Usuarios\\Usuario\\.config\\config.txt");
        assert_eq!(m.explorer_prefs(), "C:\\Usuarios\\Usuario\\.config\\explorador.txt");
        // Las carpetas con punto no se listan pero se leen y escriben por ruta.
        let cfg = m.config_file();
        m.write(&cfg, b"fondo=1\n", d(1)).unwrap();
        assert_eq!(m.read(&cfg).unwrap(), b"fondo=1\n");
        assert!(m.stat("C:\\Usuarios\\Usuario\\.papelera").unwrap().dir);
        // En disco: sin cuentas ni carpetas de usuario, y sin sesión `home` es `C:\Usuarios`.
        let disk = Disk::new(1024);
        drop(Nffs::format(Box::new(disk.clone()), d(1)).unwrap());
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(2));
        assert_eq!(names(&mut f, "C:\\"), ["Programas", "Sistema", "Usuarios"]);
        assert_eq!(f.home(), "C:\\Usuarios");
        assert!(f.user().is_none() && f.needs_setup());
        assert_eq!(f.write("C:\\x.txt", b"1", d(2)), Err(FsError::Denied), "sin sesión no se escribe");
        assert_eq!(f.write("C:\\Usuarios\\x.txt", b"1", d(2)), Err(FsError::Denied));
        assert_eq!(f.read("C:\\Sistema\\config\\cuentas"), Err(FsError::Denied));
        // Cada cuenta crea su carpeta completa; la sesión da las rutas.
        f.create_account("Ana", None, true, [1; 16], d(3)).unwrap();
        f.create_account("Ana", None, true, [1; 16], d(3)).unwrap_err();
        f.login("Ana", "").unwrap();
        assert_eq!(f.home(), ANA);
        assert_eq!(f.std_dir(Std::Music), format!("{ANA}\\Música"));
        assert_eq!(names(&mut f, ANA), ["Descargas", "Documentos", "Escritorio", "Imágenes", "Música", "Vídeos"]);
        for p in [".config", ".papelera"] {
            assert!(f.stat(&format!("{ANA}\\{p}")).unwrap().dir);
        }
        assert!(f.exists_raw("Usuarios\\Ana\\.config") && f.exists_raw("Usuarios\\Ana\\.papelera"));
        assert_eq!(f.write(&f.config_file(), b"a", d(3)), Ok(()));
        assert_eq!(f.write(&f.explorer_prefs(), b"b", d(3)), Ok(()));
    }

    #[test]
    fn papelera_por_usuario() {
        let disk = Disk::new(2048);
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(1));
        f.format_pending(d(1)).unwrap();
        f.create_account("Ana", None, true, [1; 16], d(1)).unwrap();
        f.create_account("Luis", None, true, [2; 16], d(1)).unwrap_err(); // sin sesión
        f.login("Ana", "").unwrap();
        f.create_account("Luis", None, false, [2; 16], d(1)).unwrap();
        f.write(&format!("{ANA}\\Documentos\\a.txt"), b"A", d(2)).unwrap();
        f.write("C:\\suelto.txt", b"S", d(2)).unwrap();
        f.trash(&format!("{ANA}\\Documentos\\a.txt"), d(3)).unwrap();
        f.trash("C:\\suelto.txt", d(3)).unwrap(); // fuera de su carpeta: va a la Papelera de quien lo borra
        assert_eq!(f.trash_list().unwrap().len(), 2);
        assert!(f.exists_raw("Usuarios\\Ana\\.papelera\\indice"));
        f.logout();
        assert!(f.trash_list().unwrap().is_empty(), "sin sesión no hay Papelera");
        assert_eq!(f.empty_trash(), Err(FsError::Invalid));
        f.login("Luis", "").unwrap();
        assert!(f.trash_list().unwrap().is_empty(), "Luis no ve lo de Ana");
        f.write("C:\\Usuarios\\Luis\\Documentos\\l.txt", b"L", d(4)).unwrap();
        f.trash("C:\\Usuarios\\Luis\\Documentos\\l.txt", d(4)).unwrap();
        assert_eq!(f.trash_list().unwrap().len(), 1);
        // Lo que ya está en una Papelera no se vuelve a enviar a otra.
        let id = f.trash_list().unwrap()[0].id.clone();
        assert_eq!(f.trash("C:\\Usuarios\\Luis\\.papelera\\".to_string().as_str(), d(4)), Err(FsError::Invalid));
        assert_eq!(f.trash(&format!("C:\\Usuarios\\Luis\\.papelera\\{id}"), d(4)), Err(FsError::Invalid));
        // Un índice manipulado no saca nada a la carpeta de otro.
        f.fs.write("Usuarios\\Luis\\.papelera\\indice", format!("{id}|2026-11-3-1-2-3|{ANA}\\Documentos\\mala.txt\n").as_bytes(), d(5)).unwrap();
        assert_eq!(f.restore(&id), Err(FsError::Denied));
        f.empty_trash().unwrap();
        assert!(f.trash_list().unwrap().is_empty());
        f.login("Ana", "").unwrap();
        assert_eq!(f.trash_list().unwrap().len(), 2, "vaciar la de Luis no toca la de Ana");
        let items = f.trash_list().unwrap();
        assert_eq!(f.restore(&items[0].id).unwrap(), "C:\\suelto.txt");
        assert_eq!(f.restore(&items[1].id).unwrap(), format!("{ANA}\\Documentos\\a.txt"));
    }

    #[test]
    fn permisos_entre_usuarios() {
        let disk = Disk::new(2048);
        let mut f = fresh(&disk);
        f.create_account("Ana", None, true, [1; 16], d(1)).unwrap();
        f.login("Ana", "").unwrap();
        f.create_account("Luis", None, false, [2; 16], d(1)).unwrap();
        // El administrador entra y escribe en la carpeta de otro y en Sistema.
        f.write("C:\\Usuarios\\Luis\\Documentos\\l.txt", b"L", d(2)).unwrap();
        assert_eq!(names(&mut f, "C:\\Usuarios"), ["Ana", "Luis"]);
        assert!(f.write("C:\\Sistema\\temp\\t.txt", b"t", d(2)).is_ok());
        assert!(in_system("c:/sistema/temp/t.txt") && in_system("C:\\Sistema") && !in_system("C:\\Usuarios\\Ana"));
        f.write(&format!("{ANA}\\Documentos\\a.txt"), b"A", d(2)).unwrap();
        // Un usuario normal solo ve y usa lo suyo.
        f.login("Luis", "").unwrap();
        assert!(!f.is_admin());
        assert_eq!(names(&mut f, "C:\\Usuarios"), ["Luis"]);
        let a = format!("{ANA}\\Documentos\\a.txt");
        assert_eq!(f.read(&a), Err(FsError::Denied));
        assert_eq!(f.stat(ANA), Err(FsError::Denied));
        assert_eq!(f.list(ANA).err(), Some(FsError::Denied));
        assert_eq!(f.write(&a, b"x", d(3)), Err(FsError::Denied));
        assert_eq!(f.mkdir(&format!("{ANA}\\Documentos\\n"), d(3)), Err(FsError::Denied));
        assert_eq!(f.delete(&a), Err(FsError::Denied));
        assert_eq!(f.trash(&a, d(3)), Err(FsError::Denied));
        assert_eq!(f.rename(&a, "C:\\Usuarios\\Luis\\Documentos\\a.txt"), Err(FsError::Denied));
        assert_eq!(f.copy(&a, "C:\\Usuarios\\Luis\\Documentos\\a.txt", d(3)), Err(FsError::Denied));
        assert_eq!(f.copy("C:\\Usuarios\\Luis\\Documentos\\l.txt", &format!("{ANA}\\Documentos\\l.txt"), d(3)), Err(FsError::Denied));
        assert!(f.transfer(&[String::from("C:\\Usuarios\\Luis\\Documentos\\l.txt")], &format!("{ANA}\\Escritorio"), true, d(3)).is_err());
        assert_eq!(f.read("C:\\Usuarios\\Luis\\Documentos\\l.txt").unwrap(), b"L");
        assert!(f.user_rename(&a, &format!("{ANA}\\Documentos\\b.txt")).is_err());
        assert!(f.user_trash(&a, d(3)).is_err() && f.user_create(&format!("{ANA}\\Documentos\\z"), true, d(3)).is_err());
        // Lo suyo sí, y Sistema se lee pero no se escribe; las cuentas no se leen.
        f.write("C:\\Usuarios\\Luis\\Documentos\\m.txt", b"M", d(3)).unwrap();
        f.trash("C:\\Usuarios\\Luis\\Documentos\\m.txt", d(3)).unwrap();
        assert!(f.read("C:\\Sistema\\temp\\t.txt").is_ok());
        assert_eq!(f.write("C:\\Sistema\\temp\\u.txt", b"u", d(3)), Err(FsError::Denied));
        assert_eq!(f.delete("C:\\Sistema\\temp\\t.txt"), Err(FsError::Denied));
        assert_eq!(f.mkdir("C:\\Sistema\\nueva", d(3)), Err(FsError::Denied));
        assert_eq!(f.rename("C:\\Sistema\\temp\\t.txt", "C:\\Usuarios\\Luis\\t.txt"), Err(FsError::Denied));
        assert_eq!(f.read(ACCOUNTS_FILE), Err(FsError::Denied));
        // Programas: un usuario normal los ve pero no instala ni cambia nada.
        f.install_system_program("Calc", &[("app.txt", b"nombre=Calc\n")], d(3)).unwrap();
        assert!(f.read("C:\\Programas\\Calc\\app.txt").is_ok());
        assert_eq!(f.write("C:\\Programas\\Calc\\programa.nfx", b"x", d(3)), Err(FsError::Denied));
        assert_eq!(f.mkdir("C:\\Programas\\Malo", d(3)), Err(FsError::Denied));
        assert_eq!(f.delete("C:\\Programas\\Calc\\app.txt"), Err(FsError::Denied));
        assert!(f.user_trash("C:\\Programas", d(3)).is_err());
        // Ficheros del sistema (fuentes): se instalan con sesión normal, se leen, no se tocan, y solo se reescriben si cambian.
        assert_eq!(f.install_system_files("fuentes", &[("a.ttf", b"A")], d(3)), Ok(true));
        assert_eq!(f.install_system_files("fuentes", &[("a.ttf", b"A")], d(3)), Ok(false));
        assert_eq!(f.read("C:\\Sistema\\fuentes\\a.ttf").as_deref(), Ok(&b"A"[..]));
        assert_eq!(f.write("C:\\Sistema\\fuentes\\a.ttf", b"x", d(3)), Err(FsError::Denied));
        assert_eq!(f.install_system_files("..\\config", &[], d(3)), Err(FsError::Invalid));
        // Nadie crea carpetas sueltas dentro de `Usuarios`, ni siquiera un administrador.
        f.login("Ana", "").unwrap();
        assert_eq!(f.mkdir("C:\\Usuarios\\Falsa", d(3)), Err(FsError::Denied));
        assert!(f.read(ACCOUNTS_FILE).is_ok());
        f.mkdir("C:\\Programas\\Nuevo", d(3)).unwrap();
        assert!(f.user_trash("C:\\Programas", d(3)).is_err(), "la carpeta Programas no se borra");
    }

    #[test]
    fn proteccion_de_carpetas_por_todas_las_vias() {
        let mut f = Files::default();
        let h = f.home();
        let mut prot: Vec<String> = ["C:\\", "C:\\Usuarios", "C:\\Sistema", h.as_str()].iter().map(|s| s.to_string()).collect();
        for s in SYS_SUBS {
            prot.push(format!("C:\\Sistema\\{s}"));
        }
        for s in Std::ALL {
            prot.push(f.std_dir(s));
        }
        prot.push(format!("{h}\\.config"));
        prot.push(format!("{h}\\.papelera"));
        let n = d(1);
        let dest = f.std_dir(Std::Documents);
        for p in &prot {
            assert!(is_protected(p), "{p}");
            assert_eq!(f.delete(p), Err(FsError::Invalid), "delete {p}");
            assert_eq!(f.rename(p, "C:\\otro"), Err(FsError::Invalid), "rename {p}");
            assert_eq!(f.trash(p, n), Err(FsError::Invalid), "trash {p}");
            assert!(f.user_rename(p, "C:\\otro").is_err() && f.user_trash(p, n).is_err(), "user_ {p}");
            assert!(f.user_trash_many(&[p.clone()], n).is_err());
            assert!(f.transfer(&[p.clone()], &dest, true, n).is_err() || p == &dest, "transfer {p}");
            assert!(f.exists(p), "{p}");
            // Tampoco como destino de una sobrescritura.
            f.write("C:\\Usuarios\\Usuario\\Documentos\\x", b"x", n).unwrap();
            assert!(f.rename("C:\\Usuarios\\Usuario\\Documentos\\x", p).is_err(), "destino {p}");
            f.delete("C:\\Usuarios\\Usuario\\Documentos\\x").unwrap();
        }
        // Lo de dentro sí se puede mover y borrar.
        assert!(!is_protected(&format!("{h}\\Documentos\\a")) && !is_protected("C:\\Sistema\\config\\cuentas"));
        assert!(!is_protected(&format!("{h}\\.config\\config.txt")));
        // La carpeta de otra cuenta también está protegida (aunque no exista como cuenta).
        assert!(is_protected("C:\\Usuarios\\Otra") && is_protected("C:\\Usuarios\\Otra\\Vídeos"));
    }

    #[test]
    fn migracion_completa_y_repetida() {
        let disk = old_disk();
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(5));
        assert!(f.persistent() && f.needs_setup() && f.user().is_none());
        // Sin cuenta, lo antiguo espera donde estaba y lo suelto de Sistema, apartado.
        assert!(f.exists_raw("Documentos\\a.txt") && f.exists_raw("$Papelera\\indice"));
        assert!(f.exists_raw("Sistema\\config\\antiguo\\config.txt") && !f.exists_raw("Sistema\\config.txt"));
        assert!(f.exists_raw("Sistema\\config\\migracion"));
        assert_eq!(names(&mut f, "C:\\"), ["Documentos", "Escritorio", "Imágenes", "Programas", "Sistema", "Usuarios"]);
        // Reiniciar sin cuenta no cambia nada (idempotente).
        drop(f);
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(6));
        assert!(f.exists_raw("Documentos\\a.txt") && f.exists_raw("Sistema\\config\\antiguo\\explorador.txt"));
        f.create_account("Ana", None, false, [1; 16], d(7)).unwrap();
        check_migrated(&mut f);
        // Y repetir el arranque tras migrar no vuelve a tocar nada ni resucita carpetas antiguas.
        drop(f);
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(9));
        f.login("Ana", "").unwrap();
        f.mkdir("C:\\Documentos", d(9)).unwrap();
        f.write("C:\\Documentos\\mio.txt", b"m", d(9)).unwrap();
        drop(f);
        let mut f = Files::probe(Some(Box::new(disk)), d(10));
        f.login("Ana", "").unwrap();
        assert!(f.exists("C:\\Documentos\\mio.txt"), "una carpeta creada después no se vuelve a migrar");
        assert!(!f.exists(&format!("{ANA}\\Documentos\\mio.txt")));
        let log = String::from_utf8(f.read(LOG_FILE).unwrap()).unwrap();
        assert!(log.contains("Migración completa") && log.contains("2026-11-07"), "{log}");
    }

    #[test]
    fn migracion_con_nombres_repetidos_en_destino() {
        let disk = old_disk();
        {
            // Ana ya existe y ya tiene un a.txt y un config.txt propios.
            let mut fs = Nffs::mount(Box::new(disk.clone())).unwrap();
            fs.mkdir("Usuarios", d(1)).unwrap();
            fs.mkdir("Sistema\\config", d(1)).unwrap();
            fs.write("Sistema\\config\\cuentas", b"Ana|admin|50||\n", d(1)).unwrap();
            for p in ["Usuarios\\Ana", "Usuarios\\Ana\\Documentos", "Usuarios\\Ana\\.config"] {
                fs.mkdir(p, d(1)).unwrap();
            }
            fs.write("Usuarios\\Ana\\Documentos\\a.txt", b"NUEVO", d(1)).unwrap();
            fs.mkdir("Usuarios\\Ana\\Documentos\\sub", d(1)).unwrap();
            fs.write("Usuarios\\Ana\\.config\\config.txt", b"propio", d(1)).unwrap();
        }
        let mut f = Files::probe(Some(Box::new(disk)), d(2));
        f.login("Ana", "").unwrap();
        assert_eq!(f.read(&format!("{ANA}\\Documentos\\a.txt")).unwrap(), b"NUEVO");
        assert_eq!(f.read(&format!("{ANA}\\Documentos\\a (2).txt")).unwrap(), b"A");
        assert_eq!(f.read(&format!("{ANA}\\Documentos\\sub\\b.bin")).is_err(), true, "la carpeta vieja no se mezcla con la nueva");
        assert_eq!(f.read(&format!("{ANA}\\Documentos\\sub (2)\\b.bin")).unwrap().len(), 9000);
        assert_eq!(f.read(&f.config_file()).unwrap(), b"propio");
        assert_eq!(f.read(&format!("{ANA}\\.config\\config (2).txt")).unwrap(), b"fondo=3\n");
        assert_eq!(f.trash_list().unwrap().len(), 2);
    }

    #[test]
    fn migracion_cortada_en_cada_paso_y_reanudada() {
        let snapshot = old_disk().0.borrow().clone();
        let mut cuts = 0;
        for n in 0..4000u32 {
            let disk = Disk(Rc::new(RefCell::new(snapshot.clone())));
            let failed = Rc::new(Cell::new(false));
            {
                let dev = Cut { disk: disk.clone(), left: Rc::new(Cell::new(n)), failed: failed.clone() };
                let mut f = Files::probe(Some(Box::new(dev)), d(1));
                if f.persistent() && f.accounts().is_empty() {
                    let _ = f.create_account("Ana", None, true, [1; 16], d(2));
                }
            }
            if failed.get() {
                cuts += 1;
            }
            // Siguiente arranque, ya sin cortes: termina igual, haya o no cuenta.
            let mut f = Files::probe(Some(Box::new(disk.clone())), d(3));
            assert!(f.persistent(), "n={n}");
            if f.accounts().is_empty() {
                f.create_account("Ana", None, true, [1; 16], d(4)).unwrap();
            }
            check_migrated(&mut f);
            // Y un arranque más, sin tocar nada.
            drop(f);
            let mut f = Files::probe(Some(Box::new(disk)), d(5));
            f.login("Ana", "").unwrap();
            assert!(!f.exists_raw("Sistema\\config\\migracion"), "n={n}");
            if !failed.get() {
                break;
            }
        }
        assert!(cuts > 30, "se probaron pocos cortes: {cuts}");
    }

    #[test]
    fn format_pending_cortado_falla_sin_perder_la_memoria() {
        let disk = Disk::new(2048);
        let failed = Rc::new(Cell::new(false));
        let mut f = Files::probe(Some(Box::new(Cut { disk: disk.clone(), left: Rc::new(Cell::new(60)), failed: failed.clone() })), d(1));
        f.write("C:\\Usuarios\\Usuario\\Documentos\\a.txt", b"uno", d(2)).unwrap();
        f.write("C:\\Usuarios\\Usuario\\Documentos\\grande.bin", &vec![1u8; 300_000], d(2)).unwrap();
        assert!(f.format_pending(d(3)).is_err() && failed.get(), "el corte tiene que ocurrir durante la copia");
        assert!(!f.persistent());
        assert_eq!(f.read("C:\\Usuarios\\Usuario\\Documentos\\a.txt").unwrap(), b"uno");
    }

    #[test]
    fn rename_y_delete_de_cuentas() {
        let disk = Disk::new(2048);
        let mut f = fresh(&disk);
        f.create_account("Ana", None, true, [1; 16], d(1)).unwrap();
        f.login("Ana", "").unwrap();
        f.create_account("Luis", None, false, [2; 16], d(1)).unwrap();
        f.create_account("Eva", None, false, [3; 16], d(1)).unwrap();
        f.write("C:\\Usuarios\\Luis\\Documentos\\l.txt", b"L", d(2)).unwrap();
        f.write("C:\\Usuarios\\Eva\\Documentos\\e.txt", b"E", d(2)).unwrap();
        assert_eq!(f.rename_account("Luis", "Eva", d(3)), Err(AccountError::Exists));
        assert_eq!(f.rename_account("Luis", "a|b", d(3)), Err(AccountError::BadName));
        assert_eq!(f.rename_account("Nadie", "Pepe", d(3)), Err(AccountError::NotFound));
        f.rename_account("luis", "Pepe", d(3)).unwrap();
        assert_eq!(f.read("C:\\Usuarios\\Pepe\\Documentos\\l.txt").unwrap(), b"L");
        assert!(!f.exists("C:\\Usuarios\\Luis") && f.accounts().iter().any(|a| a.name == "Pepe"));
        assert_eq!(f.login("Luis", ""), Err(AccountError::NotFound));
        // La propia cuenta: la sesión pasa a llamarse igual y las rutas cambian con ella.
        f.write(&format!("{ANA}\\Documentos\\a.txt"), b"A", d(3)).unwrap();
        f.rename_account("Ana", "Anita", d(4)).unwrap();
        assert_eq!(f.user(), Some("Anita"));
        assert_eq!(f.home(), "C:\\Usuarios\\Anita");
        assert_eq!(f.read("C:\\Usuarios\\Anita\\Documentos\\a.txt").unwrap(), b"A");
        // Un usuario normal no renombra.
        f.login("Pepe", "").unwrap();
        assert_eq!(f.rename_account("Pepe", "Otro", d(5)), Err(AccountError::NotAdmin));
        // Un cambio de nombre cortado (el diario escrito, nada más) se termina al arrancar.
        f.login("Anita", "").unwrap();
        f.fs.write("Sistema\\config\\renombrar", b"Pepe|Pablo\n", d(5)).unwrap();
        drop(f);
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(6));
        let nombres: Vec<String> = f.accounts().into_iter().map(|a| a.name).collect();
        assert_eq!(nombres, ["Anita", "Pablo", "Eva"]);
        assert!(f.exists_raw("Usuarios\\Pablo\\Documentos\\l.txt") && !f.exists_raw("Usuarios\\Pepe") && !f.exists_raw("Sistema\\config\\renombrar"));
        // Eliminar: conservando la carpeta, y con sus archivos.
        f.login("Anita", "").unwrap();
        f.delete_account("Pablo", true, d(7)).unwrap();
        assert!(f.exists_raw("Usuarios\\Pablo\\Documentos\\l.txt") && f.accounts().len() == 2);
        f.delete_account("Eva", false, d(7)).unwrap();
        assert!(!f.exists_raw("Usuarios\\Eva") && f.accounts().len() == 1);
        assert_eq!(f.delete_account("Eva", false, d(7)), Err(AccountError::NotFound));
        // Una carpeta conservada que nadie adopta sigue ahí tras reiniciar.
        drop(f);
        let mut f = Files::probe(Some(Box::new(disk)), d(8));
        assert!(f.exists_raw("Usuarios\\Pablo\\Documentos\\l.txt"));
        let log = String::from_utf8(f.fs.read("Sistema\\logs\\sistema.log").unwrap()).unwrap();
        assert!(log.contains("Cuenta renombrada: Luis -> Pepe") && log.contains("Cuenta eliminada: Eva (con sus archivos)"));
        assert!(!log.contains("secreta"));
    }

    #[test]
    fn temporales_vaciados_al_montar() {
        let disk = Disk::new(1024);
        let mut f = fresh(&disk);
        f.create_account("Ana", None, true, [1; 16], d(1)).unwrap();
        f.login("Ana", "").unwrap();
        f.mkdir("C:\\Sistema\\temp\\sub", d(1)).unwrap();
        f.write("C:\\Sistema\\temp\\sub\\t.txt", b"t", d(1)).unwrap();
        f.write("C:\\Sistema\\temp\\u.txt", b"u", d(1)).unwrap();
        f.write("C:\\Sistema\\lib\\keep.txt", b"k", d(1)).unwrap();
        drop(f);
        let mut f = Files::probe(Some(Box::new(disk)), d(2));
        assert!(f.fs.list("Sistema\\temp").unwrap().is_empty());
        assert!(f.exists_raw("Sistema\\temp") && f.exists_raw("Sistema\\lib\\keep.txt"));
    }

    #[test]
    fn registro_recortado_sin_claves() {
        let mut f = Files::default();
        for i in 0..3000 {
            f.log(&format!("linea numero {i} con relleno de texto para que ocupe\nbastante"), d(1));
        }
        let data = f.read(LOG_FILE).unwrap();
        assert!(data.len() <= LOG_MAX && data.len() > LOG_MAX - 200, "{}", data.len());
        let text = String::from_utf8(data).unwrap();
        assert!(text.starts_with("2026-11-01 09:08:07 linea numero "), "empieza en una línea entera");
        assert!(text.trim_end().ends_with("numero 2999 con relleno de texto para que ocupe bastante"));
        assert_eq!(text.lines().count(), text.matches('\n').count());
        // Las claves no llegan al registro al crear cuentas ni cambiar la clave.
        let disk = Disk::new(1024);
        let mut f = fresh(&disk);
        f.create_account("Ana", Some("secreta1"), true, [1; 16], d(1)).unwrap();
        f.login("Ana", "secreta1").unwrap();
        f.set_password("secreta1", Some("otraclave"), [2; 16], d(1)).unwrap();
        let log = String::from_utf8(f.fs.read("Sistema\\logs\\sistema.log").unwrap()).unwrap();
        assert!(log.contains("Cuenta creada: Ana") && !log.contains("secreta1") && !log.contains("otraclave"));
    }

    // ---- Cifrado total del disco ----

    /// Disco en memoria compartido con cuenta de escrituras y corte de luz simulado: a partir de
    /// la escritura `limit` las escrituras se descartan (sin error, como un corte real).
    #[derive(Clone)]
    struct Cdisk {
        data: Rc<RefCell<Vec<u8>>>,
        writes: Rc<std::cell::Cell<usize>>,
        limit: Rc<std::cell::Cell<usize>>,
    }
    impl Cdisk {
        fn new(blocks: usize) -> Cdisk {
            Cdisk::from_bytes(vec![0; blocks * BLOCK])
        }
        fn from_bytes(v: Vec<u8>) -> Cdisk {
            Cdisk { data: Rc::new(RefCell::new(v)), writes: Rc::new(std::cell::Cell::new(0)), limit: Rc::new(std::cell::Cell::new(usize::MAX)) }
        }
        fn bytes(&self) -> Vec<u8> {
            self.data.borrow().clone()
        }
        fn writes(&self) -> usize {
            self.writes.get()
        }
    }
    impl BlockDev for Cdisk {
        fn blocks(&self) -> u64 {
            (self.data.borrow().len() / BLOCK) as u64
        }
        fn read(&mut self, n: u64, buf: &mut [u8]) -> Result<(), DiskError> {
            if n >= self.blocks() || buf.len() != BLOCK {
                return Err(DiskError::Range);
            }
            buf.copy_from_slice(&self.data.borrow()[n as usize * BLOCK..][..BLOCK]);
            Ok(())
        }
        fn write(&mut self, n: u64, buf: &[u8]) -> Result<(), DiskError> {
            if n >= self.blocks() || buf.len() != BLOCK {
                return Err(DiskError::Range);
            }
            let w = self.writes.get();
            self.writes.set(w + 1);
            if w < self.limit.get() {
                self.data.borrow_mut()[n as usize * BLOCK..][..BLOCK].copy_from_slice(buf);
            }
            Ok(())
        }
    }

    fn ready_pool() -> Pool {
        let mut p = Pool::new();
        for i in 0..crate::keys::POOL_EVENTS_NEEDED as i32 {
            p.add_event(i, i * 5, i as u64, i as u64 * 11);
        }
        p
    }

    const KEY: &str = "clave-del-disco";
    const SECRET_NAME: &str = "informe-secreto-confidencial.txt";
    const SECRET_TEXT: &str = "CONTENIDO-CONFIDENCIAL-DE-PRUEBA-0123456789";
    const DOCS: &str = "C:\\Usuarios\\Ana\\Documentos\\";

    fn secret_body(i: usize) -> Vec<u8> {
        SECRET_TEXT.repeat(30 + i * 40).into_bytes()
    }

    /// Disco nuevo de `blocks` bloques, formateado, con la cuenta Ana y unos archivos.
    fn populated_disk(blocks: usize) -> (Cdisk, Files) {
        let disk = Cdisk::new(blocks);
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(1));
        f.format_pending(d(1)).unwrap();
        enter(&mut f, "Ana");
        f.write(&format!("{DOCS}{SECRET_NAME}"), &secret_body(0), d(2)).unwrap();
        f.mkdir(&format!("{DOCS}carpeta-reservada-secreta"), d(2)).unwrap();
        f.write(&format!("{DOCS}carpeta-reservada-secreta\\segundo.txt"), &secret_body(1), d(2)).unwrap();
        (disk, f)
    }

    fn check_files(f: &mut Files) {
        assert_eq!(f.read(&format!("{DOCS}{SECRET_NAME}")).unwrap(), secret_body(0));
        assert_eq!(f.read(&format!("{DOCS}carpeta-reservada-secreta\\segundo.txt")).unwrap(), secret_body(1));
    }

    fn contains(hay: &[u8], needle: &str) -> bool {
        hay.windows(needle.len()).any(|w| w == needle.as_bytes())
    }

    fn finish_conversion(f: &mut Files, step: u32) {
        let mut guard = 0;
        while !f.convert_step(step, d(3)).unwrap() {
            guard += 1;
            assert!(guard < 10_000);
        }
    }

    /// Reinicio: un `Files` nuevo sobre los bytes que hay en el disco.
    fn reboot(disk: &Cdisk) -> (Cdisk, Files) {
        let after = Cdisk::from_bytes(disk.bytes());
        let f = Files::probe(Some(Box::new(after.clone())), d(4));
        (after, f)
    }

    #[test]
    fn new_disks_reserve_two_blocks_and_old_ones_still_mount() {
        let (disk, mut f) = populated_disk(300);
        assert_eq!(f.fs.volume_blocks(), 298);
        assert_eq!(f.fs.dev_blocks(), 300);
        assert!(disk.bytes()[298 * BLOCK..].iter().all(|&b| b == 0));
        // Disco antiguo: NFFS en todos los bloques.
        let old = Cdisk::new(300);
        drop(Nffs::format(Box::new(old.clone()), d(1)).unwrap());
        let mut g = Files::probe(Some(Box::new(old.clone())), d(2));
        assert!(g.persistent() && !g.disk_locked());
        assert_eq!(g.fs.volume_blocks(), 300);
        enter(&mut g, "Ana");
        check_files(&mut f);
    }

    #[test]
    fn encrypt_roundtrip_with_key_and_recovery_and_nothing_in_clear() {
        let (disk, mut f) = populated_disk(300);
        let mut pool = ready_pool();
        let code = f.encrypt_disk(KEY, &mut pool, d(2)).unwrap();
        assert!(f.disk_encrypted());
        assert_eq!(f.disk_conversion().map(|c| (c.0, c.1, c.2)), Some((0, 298, true)));
        // Uso durante la conversión, por debajo y por encima del progreso.
        assert!(!f.convert_step(100, d(3)).unwrap());
        check_files(&mut f);
        let during = format!("{DOCS}durante-la-conversion-secreta.txt");
        f.write(&during, &secret_body(2), d(3)).unwrap();
        assert!(!f.convert_step(60, d(3)).unwrap());
        assert_eq!(f.read(&during).unwrap(), secret_body(2));
        // Mientras convierte no deja cambiar nada más ni activarlo otra vez.
        assert_eq!(f.encrypt_disk(KEY, &mut pool, d(3)), Err(KeyError::Busy));
        assert_eq!(f.decrypt_disk(KEY, d(3)), Err(KeyError::Busy));
        finish_conversion(&mut f, 50);
        assert_eq!(f.disk_conversion(), None);
        assert!(f.disk_encrypted());
        check_files(&mut f);
        assert_eq!(f.encrypt_disk(KEY, &mut pool, d(3)), Err(KeyError::AlreadyEncrypted));
        // En la imagen en bruto no hay nada en claro, ni en el bloque de trabajo.
        let raw = disk.bytes();
        for needle in [SECRET_TEXT, SECRET_NAME, "carpeta-reservada-secreta", "durante-la-conversion", "Usuarios", "Documentos", "NFFS"] {
            assert!(!contains(&raw, needle), "en claro: {needle}");
        }
        assert!(raw[298 * BLOCK..299 * BLOCK].iter().all(|&b| b == 0));
        let log = String::from_utf8(f.fs.read("Sistema\\logs\\sistema.log").unwrap()).unwrap();
        assert!(log.contains("Cifrado total activado"));
        assert!(!log.contains(KEY) && !log.contains(&code) && !log.contains(&code.replace('-', "")));

        // Reinicio: bloqueado, sin tocar el disco, clave mala deja el disco intacto byte a byte.
        let (after, mut g) = reboot(&disk);
        assert!(g.disk_locked() && !g.persistent() && g.notice().is_some());
        assert!(!g.pending_format());
        let snapshot = after.bytes();
        let w = after.writes();
        assert_eq!(g.unlock_disk("clave-mala", false, d(5)), Err(KeyError::WrongSecret));
        assert_eq!(g.unlock_disk(KEY, true, d(5)), Err(KeyError::WrongSecret));
        assert_eq!(g.unlock_disk("ABCDE-ABCDE-ABCDE-ABCDE-ABCDE-ABCDE-ABCDE-ABCDE", true, d(5)), Err(KeyError::WrongSecret));
        assert_eq!(after.bytes(), snapshot);
        assert_eq!(after.writes(), w);
        assert!(g.disk_locked());
        // Clave buena.
        g.unlock_disk(KEY, false, d(5)).unwrap();
        assert!(!g.disk_locked() && g.persistent() && g.disk_encrypted() && g.user().is_none());
        g.login("Ana", "").unwrap();
        check_files(&mut g);
        assert_eq!(g.read(&during).unwrap(), secret_body(2));
        // Con el código de recuperación (también escrito en minúsculas y sin guiones).
        for variant in [code.clone(), code.to_lowercase().replace('-', " ")] {
            let (_, mut h) = reboot(&disk);
            assert!(h.disk_locked());
            h.unlock_disk(&variant, true, d(5)).unwrap();
            h.login("Ana", "").unwrap();
            check_files(&mut h);
        }
        // Y la clave del disco no sirve como código de recuperación ni al revés.
        let (_, mut h) = reboot(&disk);
        assert_eq!(h.unlock_disk(&code, false, d(5)), Err(KeyError::WrongSecret));
    }

    #[test]
    fn use_during_decrypt_and_decrypt_leaves_a_normal_nffs() {
        let (disk, mut f) = populated_disk(300);
        let mut pool = ready_pool();
        f.encrypt_disk(KEY, &mut pool, d(2)).unwrap();
        finish_conversion(&mut f, 80);
        assert_eq!(f.decrypt_disk("otra-clave", d(3)), Err(KeyError::WrongSecret));
        f.decrypt_disk(KEY, d(3)).unwrap();
        assert_eq!(f.disk_conversion().map(|c| (c.0, c.2)), Some((0, false)));
        assert!(!f.convert_step(120, d(3)).unwrap());
        check_files(&mut f);
        let mid = format!("{DOCS}a-mitad-de-descifrado.txt");
        f.write(&mid, &secret_body(3), d(3)).unwrap();
        assert!(!f.convert_step(60, d(3)).unwrap());
        assert_eq!(f.read(&mid).unwrap(), secret_body(3));
        // Cortado a mitad y reiniciado: sigue pidiendo la clave y se puede terminar.
        let (after, mut g) = reboot(&disk);
        assert!(g.disk_locked());
        g.unlock_disk(KEY, false, d(5)).unwrap();
        assert!(g.disk_conversion().is_some_and(|c| !c.2 && c.0 > 0));
        g.login("Ana", "").unwrap();
        check_files(&mut g);
        finish_conversion(&mut g, 70);
        assert!(!g.disk_encrypted() && g.disk_conversion().is_none());
        assert_eq!(g.read(&mid).unwrap(), secret_body(3));
        // La imagen vuelve a ser un NFFS normal: monta sin clave y no queda cabecera.
        let raw = Cdisk::from_bytes(after.bytes());
        assert!(cryptdev::detect(&mut raw.clone()).is_none());
        assert!(raw.bytes()[298 * BLOCK..].iter().all(|&b| b == 0));
        let (_, mut h) = reboot(&after);
        assert!(!h.disk_locked() && h.persistent() && !h.disk_encrypted());
        h.login("Ana", "").unwrap();
        check_files(&mut h);
        assert_eq!(h.read(&mid).unwrap(), secret_body(3));
        // Y se puede volver a cifrar.
        h.encrypt_disk(KEY, &mut pool, d(6)).unwrap();
    }

    /// Corta la luz en CADA escritura (desde `limit0` hasta que acaba la conversión), reinicia, abre
    /// con la clave, termina la conversión y comprueba que todo está íntegro.
    fn cut_everywhere(decrypting: bool) {
        let (disk, mut f) = populated_disk(70);
        let mut pool = ready_pool();
        if decrypting {
            f.encrypt_disk(KEY, &mut pool, d(2)).unwrap();
            finish_conversion(&mut f, 100);
        }
        let base = disk.bytes();
        // Una ejecución de toda la operación; con `limit` la luz se va tras esa escritura.
        let run = |limit: Option<usize>| -> (Cdisk, usize) {
            let dk = Cdisk::from_bytes(base.clone());
            let mut g = Files::probe(Some(Box::new(dk.clone())), d(4));
            let mut pool = ready_pool();
            if decrypting {
                g.unlock_disk(KEY, false, d(4)).unwrap();
            }
            g.login("Ana", "").unwrap();
            let start = dk.writes();
            if let Some(k) = limit {
                dk.limit.set(start + k);
            }
            if decrypting {
                g.decrypt_disk(KEY, d(5)).unwrap();
            } else {
                g.encrypt_disk(KEY, &mut pool, d(5)).unwrap();
            }
            finish_conversion(&mut g, 1);
            let n = dk.writes() - start;
            (dk, n)
        };
        let (_, total) = run(None);
        assert!(total > 2 * 68, "{total}");
        for k in 0..=total {
            let (dk, _) = run(Some(k));
            let (after, mut g) = reboot(&dk);
            if g.disk_locked() {
                // Cifrado (o a medias): se abre con la clave y se termina lo que falte.
                g.unlock_disk(KEY, false, d(6)).unwrap();
                g.login("Ana", "").unwrap();
                check_files(&mut g);
                let mut guard = 0;
                while !g.convert_step(7, d(6)).unwrap() {
                    guard += 1;
                    assert!(guard < 1000);
                }
                check_files(&mut g);
                if !decrypting {
                    assert!(g.disk_encrypted(), "k={k}");
                    let raw = after.bytes();
                    for needle in [SECRET_TEXT, SECRET_NAME] {
                        assert!(!contains(&raw, needle), "k={k}: {needle}");
                    }
                    assert!(raw[68 * BLOCK..69 * BLOCK].iter().all(|&b| b == 0), "k={k}");
                } else if !g.disk_encrypted() {
                    // Descifrado del todo: la imagen es un NFFS normal, sin cabecera, que monta sin clave.
                    let (_, mut fin) = reboot(&after);
                    assert!(!fin.disk_locked() && fin.persistent(), "k={k}");
                    fin.login("Ana", "").unwrap();
                    check_files(&mut fin);
                    assert!(cryptdev::detect(&mut after.clone()).is_none(), "k={k}");
                } else {
                    // Solo si se perdió la escritura que empezaba a descifrar: sigue cifrado del todo.
                    assert!(g.disk_conversion().is_none(), "k={k}");
                }
            } else {
                // Plano (antes de empezar o ya descifrado): monta sin clave.
                assert!(g.persistent(), "k={k}");
                g.login("Ana", "").unwrap();
                check_files(&mut g);
            }
        }
    }

    #[test]
    fn power_cut_at_every_write_while_encrypting_the_disk() {
        cut_everywhere(false);
    }

    #[test]
    fn power_cut_at_every_write_while_decrypting_the_disk() {
        cut_everywhere(true);
    }

    #[test]
    fn permissions_and_errors_of_disk_encryption() {
        let (_, mut f) = populated_disk(300);
        let mut pool = ready_pool();
        assert_eq!(f.encrypt_disk("abc", &mut pool, d(2)), Err(KeyError::Weak));
        assert_eq!(f.encrypt_disk(KEY, &mut Pool::new(), d(2)), Err(KeyError::NotReady));
        assert_eq!(f.decrypt_disk(KEY, d(2)), Err(KeyError::NotEncrypted));
        assert_eq!(f.change_disk_password(KEY, "nueva-clave", &mut pool, d(2)), Err(KeyError::NotEncrypted));
        assert_eq!(f.new_disk_recovery(KEY, &mut pool, d(2)), Err(KeyError::NotEncrypted));
        assert_eq!(f.unlock_disk(KEY, false, d(2)), Err(KeyError::NotEncrypted));
        assert!(f.convert_step(5, d(2)).unwrap());
        // Un usuario normal no puede.
        f.create_account("Beto", None, false, [9; 16], d(2)).unwrap();
        f.logout();
        f.login("Beto", "").unwrap();
        assert_eq!(f.encrypt_disk(KEY, &mut pool, d(2)), Err(KeyError::NotAdmin));
        assert!(!f.disk_encrypted());
        f.logout();
        f.login("Ana", "").unwrap();
        f.encrypt_disk(KEY, &mut pool, d(2)).unwrap();
        finish_conversion(&mut f, 100);
        f.logout();
        f.login("Beto", "").unwrap();
        assert_eq!(f.decrypt_disk(KEY, d(2)), Err(KeyError::NotAdmin));
        assert_eq!(f.change_disk_password(KEY, "nueva-clave", &mut pool, d(2)), Err(KeyError::NotAdmin));
        assert_eq!(f.new_disk_recovery(KEY, &mut pool, d(2)), Err(KeyError::NotAdmin));
        // Sin disco persistente.
        let mut m = Files::default();
        assert_eq!(m.encrypt_disk(KEY, &mut pool, d(2)), Err(KeyError::Unavailable));
        // Sin sesión tampoco.
        let (_, mut g) = populated_disk(300);
        g.logout();
        assert_eq!(g.encrypt_disk(KEY, &mut pool, d(2)), Err(KeyError::NotAdmin));
    }

    #[test]
    fn change_password_and_new_recovery_invalidate_the_old_ones() {
        let (disk, mut f) = populated_disk(300);
        let mut pool = ready_pool();
        let code = f.encrypt_disk(KEY, &mut pool, d(2)).unwrap();
        finish_conversion(&mut f, 100);
        let body_before: Vec<u8> = disk.bytes()[..298 * BLOCK].to_vec();
        assert_eq!(f.change_disk_password("incorrecta", "otra-nueva", &mut pool, d(3)), Err(KeyError::WrongSecret));
        assert_eq!(f.change_disk_password(KEY, "corta", &mut pool, d(3)), Err(KeyError::Weak));
        assert_eq!(f.change_disk_password(KEY, "otra-nueva", &mut Pool::new(), d(3)), Err(KeyError::NotReady));
        f.change_disk_password(KEY, "otra-nueva", &mut pool, d(3)).unwrap();
        // Los datos cifrados no cambian (salvo el registro), solo la cabecera.
        let raw = disk.bytes();
        let changed = (0..298).filter(|&i| raw[i * BLOCK..(i + 1) * BLOCK] != body_before[i * BLOCK..(i + 1) * BLOCK]).count();
        assert!(changed < 20, "{changed}");
        let (ga, mut g) = reboot(&disk);
        assert_eq!(g.unlock_disk(KEY, false, d(5)), Err(KeyError::WrongSecret));
        g.unlock_disk("otra-nueva", false, d(5)).unwrap();
        g.login("Ana", "").unwrap();
        check_files(&mut g);
        // El código de recuperación sigue valiendo tras cambiar la clave.
        let (_, mut h) = reboot(&disk);
        h.unlock_disk(&code, true, d(5)).unwrap();
        // Código nuevo: el viejo deja de valer.
        let new_code = g.new_disk_recovery("otra-nueva", &mut pool, d(6)).unwrap();
        assert_ne!(new_code, code);
        assert_eq!(g.new_disk_recovery(KEY, &mut pool, d(6)), Err(KeyError::WrongSecret));
        let (_, mut h) = reboot(&ga);
        assert_eq!(h.unlock_disk(&code, true, d(7)), Err(KeyError::WrongSecret));
        h.unlock_disk(&new_code, true, d(7)).unwrap();
        h.login("Ana", "").unwrap();
        check_files(&mut h);
        // Cambiar la clave a mitad de una conversión también vale.
        let (_, mut c) = populated_disk(300);
        c.encrypt_disk(KEY, &mut pool, d(2)).unwrap();
        assert!(!c.convert_step(50, d(2)).unwrap());
        c.change_disk_password(KEY, "otra-nueva", &mut pool, d(3)).unwrap();
        finish_conversion(&mut c, 40);
        check_files(&mut c);
    }

    #[test]
    fn old_disk_needs_free_last_blocks() {
        // Disco antiguo con todo el espacio: con los dos últimos bloques libres se activa.
        let old = Cdisk::new(200);
        drop(Nffs::format(Box::new(old.clone()), d(1)).unwrap());
        let mut f = Files::probe(Some(Box::new(old.clone())), d(2));
        assert_eq!(f.fs.volume_blocks(), 200);
        enter(&mut f, "Ana");
        f.write(&format!("{DOCS}{SECRET_NAME}"), &secret_body(0), d(2)).unwrap();
        f.mkdir(&format!("{DOCS}carpeta-reservada-secreta"), d(2)).unwrap();
        f.write(&format!("{DOCS}carpeta-reservada-secreta\\segundo.txt"), &secret_body(1), d(2)).unwrap();
        let before = old.bytes();
        let mut pool = ready_pool();
        // Con el último bloque ocupado: NoRoom y el disco no cambia.
        f.fs.mark_used_for_test(199);
        assert_eq!(f.encrypt_disk(KEY, &mut pool, d(3)), Err(KeyError::NoRoom));
        assert_eq!(old.bytes(), before);
        assert!(!f.disk_encrypted());
        // Con el penúltimo también.
        let mut f = Files::probe(Some(Box::new(Cdisk::from_bytes(before.clone()))), d(2));
        f.login("Ana", "").unwrap();
        f.fs.mark_used_for_test(198);
        assert_eq!(f.encrypt_disk(KEY, &mut pool, d(3)), Err(KeyError::NoRoom));
        // Libres: se reduce el volumen y se cifra.
        let disk = Cdisk::from_bytes(before);
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(2));
        f.login("Ana", "").unwrap();
        f.encrypt_disk(KEY, &mut pool, d(3)).unwrap();
        assert_eq!(f.fs.volume_blocks(), 198);
        finish_conversion(&mut f, 64);
        check_files(&mut f);
        let (_, mut g) = reboot(&disk);
        assert!(g.disk_locked());
        g.unlock_disk(KEY, false, d(5)).unwrap();
        g.login("Ana", "").unwrap();
        check_files(&mut g);
        assert!(!contains(&disk.bytes(), SECRET_TEXT));
    }

    #[test]
    fn skip_locked_disk_writes_nothing() {
        let (disk, mut f) = populated_disk(300);
        let mut pool = ready_pool();
        f.encrypt_disk(KEY, &mut pool, d(2)).unwrap();
        finish_conversion(&mut f, 100);
        let (after, mut g) = reboot(&disk);
        let (snap, w) = (after.bytes(), after.writes());
        assert!(g.disk_locked());
        g.skip_locked_disk();
        assert!(!g.disk_locked() && !g.persistent() && g.notice().is_some());
        // C: sigue en memoria y se puede usar sin tocar el disco.
        g.write("C:\\Usuarios\\Usuario\\Documentos\\a.txt", b"mem", d(5)).unwrap();
        assert_eq!(g.unlock_disk(KEY, false, d(5)), Err(KeyError::NotEncrypted));
        assert_eq!(after.bytes(), snap);
        assert_eq!(after.writes(), w);
        // Inspeccionar sin clave tampoco escribe.
        let (a2, g2) = reboot(&disk);
        assert!(g2.disk_locked());
        assert_eq!(a2.writes(), 0);
    }

    #[test]
    fn altered_header_or_garbage_is_not_an_encrypted_disk_and_is_not_written() {
        let (disk, mut f) = populated_disk(300);
        let mut pool = ready_pool();
        f.encrypt_disk(KEY, &mut pool, d(2)).unwrap();
        finish_conversion(&mut f, 100);
        let good = disk.bytes();
        let last = 299 * BLOCK;
        let used = good[last..last + BLOCK].iter().position(|&b| b == 0).unwrap();
        // Cualquier byte alterado de la cabecera: ya no es un disco cifrado y se ignora sin escribir.
        for i in (0..used).step_by(37).chain([0, 1, 8, used - 1]) {
            let mut bad = good.clone();
            bad[last + i] ^= 0x40;
            let dk = Cdisk::from_bytes(bad.clone());
            let g = Files::probe(Some(Box::new(dk.clone())), d(4));
            assert!(!g.disk_locked() && !g.persistent() && !g.pending_format(), "byte {i}");
            assert_eq!(dk.writes(), 0);
            assert_eq!(dk.bytes(), bad);
        }
        // Basura en el último bloque de un disco por lo demás en blanco: no está en blanco ni es cifrado.
        let mut junk = vec![0u8; 300 * BLOCK];
        junk[last..last + 64].copy_from_slice(&[0xa5; 64]);
        let dk = Cdisk::from_bytes(junk.clone());
        let g = Files::probe(Some(Box::new(dk.clone())), d(4));
        assert!(!g.disk_locked() && !g.pending_format() && !g.persistent());
        assert_eq!((dk.writes(), dk.bytes()), (0, junk));
        // Una cabecera válida con otro contenido delante sigue siendo de un disco cifrado (se abre
        // con clave); y un NFFS antiguo cuyo último bloque es suyo no se toma por cifrado.
        let mut text = vec![0u8; 100 * BLOCK];
        let old = Cdisk::from_bytes(std::mem::take(&mut text));
        drop(Nffs::format(Box::new(old.clone()), d(1)).unwrap());
        let mut bytes = old.bytes();
        bytes[99 * BLOCK..].copy_from_slice(&good[last..last + BLOCK]);
        let g = Files::probe(Some(Box::new(Cdisk::from_bytes(bytes))), d(4));
        assert!(!g.disk_locked() && g.persistent());
    }

    #[test]
    fn blank_means_all_zeros_even_at_the_very_end() {
        let mut v = vec![0u8; 300 * BLOCK];
        v[300 * BLOCK - 1] = 1;
        let dk = Cdisk::from_bytes(v.clone());
        let g = Files::probe(Some(Box::new(dk.clone())), d(4));
        assert!(!g.pending_format() && !g.persistent());
        assert_eq!((dk.writes(), dk.bytes()), (0, v));
        let g = Files::probe(Some(Box::new(Cdisk::new(300))), d(4));
        assert!(g.pending_format());
    }

    // ---- Cifrado de la carpeta de usuario ----

    const PW: &str = "secreta1";
    const H: &str = "C:\\Usuarios\\Ana";
    const NEEDLES: [&str; 9] = [
        SECRET_TEXT,
        SECRET_NAME,
        "carpeta-reservada-secreta",
        "segundo.txt",
        "nota-del-escritorio-privada",
        "CUERPO-ESCRITORIO-PRIVADO",
        "CONFIG-PRIVADA-XYZ",
        "para-la-papelera-secreta",
        "nombre-largo-subcarpeta",
    ];

    /// Disco con Ana (administradora, con clave) y la sesión abierta, sin archivos.
    fn hdisk(blocks: usize) -> (Cdisk, Files) {
        let disk = Cdisk::new(blocks);
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(1));
        f.format_pending(d(1)).unwrap();
        f.create_account("Ana", Some(PW), true, [1; 16], d(1)).unwrap();
        f.login("Ana", PW).unwrap();
        (disk, f)
    }

    /// Ana con archivos, subcarpetas, configuración y algo en la Papelera.
    fn fill(f: &mut Files) {
        f.write(&format!("{H}\\Documentos\\{SECRET_NAME}"), &secret_body(0), d(2)).unwrap();
        f.mkdir(&format!("{H}\\Documentos\\carpeta-reservada-secreta"), d(2)).unwrap();
        f.write(&format!("{H}\\Documentos\\carpeta-reservada-secreta\\segundo.txt"), &secret_body(1), d(3)).unwrap();
        f.mkdir(&format!("{H}\\Documentos\\carpeta-reservada-secreta\\nombre-largo-subcarpeta"), d(3)).unwrap();
        f.write(&format!("{H}\\Escritorio\\nota-del-escritorio-privada.txt"), b"CUERPO-ESCRITORIO-PRIVADO", d(4)).unwrap();
        let cfg = f.config_file();
        f.write(&cfg, b"fondo=CONFIG-PRIVADA-XYZ\n", d(5)).unwrap();
        f.write(&format!("{H}\\Documentos\\para-la-papelera-secreta.txt"), &secret_body(2), d(6)).unwrap();
        f.trash(&format!("{H}\\Documentos\\para-la-papelera-secreta.txt"), d(6)).unwrap();
        f.write(&format!("{H}\\Documentos\\vacio.txt"), b"", d(6)).unwrap();
    }

    /// Foto lógica de todo lo de Ana (nombres, tipos, tamaños, fechas y contenido), incluidas
    /// `.config` y `.papelera`.
    fn snap(f: &mut Files) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![String::from("Usuarios\\Ana")];
        while let Some(p) = stack.pop() {
            let mut es = f.r_list(&p).unwrap();
            es.sort_by_key(|e| lower(&e.name));
            for e in es {
                let q = join_rel(&p, &e.name);
                let body = if e.dir {
                    stack.push(q.clone());
                    String::new()
                } else {
                    crate::crypto::to_hex(&crate::crypto::sha256(&f.r_read(&q).unwrap()))
                };
                out.push(format!("{q}|{}|{}|{:?}|{body}", e.dir, e.size, e.modified));
            }
        }
        out.sort();
        out
    }

    fn finish_home(f: &mut Files, step: u32) {
        let mut guard = 0;
        while !f.home_convert_step(step, d(8)).unwrap() {
            guard += 1;
            assert!(guard < 20_000);
        }
    }

    fn raw_has(disk: &Cdisk, needles: &[&str]) -> Option<String> {
        let raw = disk.bytes();
        needles.iter().find(|n| contains(&raw, n)).map(|n| n.to_string())
    }

    #[test]
    fn home_roundtrip_nothing_in_clear_and_same_after_relogin() {
        let (disk, mut f) = hdisk(400);
        fill(&mut f);
        let before = snap(&mut f);
        assert!(raw_has(&disk, &NEEDLES).is_some());
        let mut pool = ready_pool();
        assert!(!f.home_encrypted() && !f.account_encrypted("Ana"));
        let code = f.encrypt_home(PW, &mut pool, d(7)).unwrap();
        assert!(f.home_encrypted() && f.account_encrypted("ana"));
        let (done, total, enc) = f.home_conversion().unwrap();
        assert!(done == 0 && total >= 10 && enc);
        // A medias: todo se sigue viendo igual y el progreso solo sube.
        let mut last = 0;
        for _ in 0..6 {
            assert!(!f.home_convert_step(2, d(8)).unwrap());
            assert_eq!(snap(&mut f), before);
            let (dn, tot, _) = f.home_conversion().unwrap();
            assert!(dn >= last && tot >= dn);
            last = dn;
        }
        finish_home(&mut f, 3);
        assert_eq!(f.home_conversion(), None);
        assert_eq!(snap(&mut f), before);
        assert_eq!(f.stat(&format!("{H}\\Documentos\\vacio.txt")).unwrap().size, 0);
        assert_eq!(f.stat(&format!("{H}\\Documentos\\{SECRET_NAME}")).unwrap().size, secret_body(0).len() as u64);
        // En la imagen en bruto no hay ni contenido ni nombres, ni en `.config` ni en la Papelera.
        assert_eq!(raw_has(&disk, &NEEDLES), None);
        assert_eq!(raw_has(&disk, &["Documentos", "Escritorio", ".papelera", ".config", "Descargas"]), None);
        // El archivo de llaves no tiene ni la clave ni el código; el registro tampoco.
        let kf = String::from_utf8(f.fs.read("Sistema\\config\\llaves\\Ana").unwrap()).unwrap();
        assert!(kf.starts_with("estado=cifrado\n") && !kf.contains(PW) && !kf.contains(&code));
        let log = String::from_utf8(f.fs.read("Sistema\\logs\\sistema.log").unwrap()).unwrap();
        assert!(log.contains("Cifrado de la carpeta de Ana activado") && !log.contains(PW) && !log.contains(&code.replace('-', "")));
        // Papelera dentro de la carpeta cifrada.
        let items = f.trash_list().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "para-la-papelera-secreta.txt");
        // Cierra la sesión y vuelve: mismas cosas; con la clave mala no hay sesión.
        f.logout();
        assert!(f.hc.keys.is_none());
        assert_eq!(f.login("Ana", "mala-clave"), Err(AccountError::WrongPassword));
        assert!(f.user().is_none());
        f.login("Ana", PW).unwrap();
        assert_eq!(snap(&mut f), before);
        // Reinicio completo.
        let (after, mut g) = reboot(&disk);
        assert!(g.account_encrypted("Ana"));
        // Ni el arranque crea carpetas en claro dentro de la carpeta cifrada.
        let names: Vec<String> = g.fs.list("Usuarios\\Ana").unwrap().into_iter().map(|e| e.name).collect();
        assert!(names.len() == 8 && names.iter().all(|n| n.starts_with("$E") || n.starts_with("$e")), "{names:?}");
        assert_eq!(g.read(&format!("{H}\\Documentos\\{SECRET_NAME}")), Err(FsError::Denied));
        g.login("Ana", PW).unwrap();
        assert_eq!(snap(&mut g), before);
        assert_eq!(raw_has(&after, &NEEDLES), None);
        let id = g.trash_list().unwrap()[0].id.clone();
        let back = g.restore(&id).unwrap();
        assert_eq!(g.read(&back).unwrap(), secret_body(2));
    }

    /// Operaciones que se aplican igual a una carpeta normal y a otra cifrada (o a medias).
    fn script(f: &mut Files, i: usize) {
        let docs = format!("{H}\\Documentos");
        let desk = format!("{H}\\Escritorio");
        match i {
            0 => f.write(&format!("{docs}\\nuevo.txt"), b"nuevo contenido", d(10)).unwrap(),
            1 => f.write(&format!("{docs}\\{SECRET_NAME}"), b"reescrito", d(11)).unwrap(),
            2 => f.mkdir(&format!("{docs}\\otra"), d(12)).unwrap(),
            3 => f.rename(&format!("{docs}\\nuevo.txt"), &format!("{docs}\\otra\\Movido.TXT")).unwrap(),
            4 => f.rename(&format!("{docs}\\carpeta-reservada-secreta"), &format!("{desk}\\carpeta-renombrada")).unwrap(),
            5 => f.copy(&format!("{desk}\\carpeta-renombrada"), &format!("{docs}\\copia"), d(13)).unwrap(),
            6 => f.delete(&format!("{docs}\\copia\\segundo.txt")).unwrap(),
            7 => f.user_trash(&format!("{docs}\\otra\\movido.txt"), d(14)).unwrap(),
            8 => f.undo(d(15)).unwrap(),
            9 => {
                f.set_clip(&format!("{desk}\\nota-del-escritorio-privada.txt"), false);
                f.paste(&docs, d(16)).unwrap();
            }
            10 => assert_eq!(f.free_name(&docs, "nota-del-escritorio-privada.txt"), "nota-del-escritorio-privada (2).txt"),
            11 => {
                f.transfer(&[format!("{docs}\\copia")], &desk, true, d(17)).unwrap();
            }
            12 => f.user_rename(&format!("{desk}\\copia"), &format!("{desk}\\COPIA2")).unwrap(),
            13 => f.user_create(&format!("{docs}\\creado.txt"), false, d(18)).unwrap(),
            14 => {
                let t = f.trash_list().unwrap();
                f.restore(&t[0].id).unwrap();
            }
            15 => f.write(&format!("{docs}\\final.txt"), &secret_body(4), d(19)).unwrap(),
            16 => f.empty_trash().unwrap(),
            _ => unreachable!(),
        }
    }
    const SCRIPT_LEN: usize = 17;

    #[test]
    fn every_operation_works_inside_an_encrypted_home_same_as_outside() {
        // Gemelo sin cifrar para comparar.
        let (_, mut plain) = hdisk(400);
        fill(&mut plain);
        let (disk, mut enc) = hdisk(400);
        fill(&mut enc);
        let mut pool = ready_pool();
        enc.encrypt_home(PW, &mut pool, d(7)).unwrap();
        finish_home(&mut enc, 5);
        assert_eq!(snap(&mut enc), snap(&mut plain));
        for i in 0..SCRIPT_LEN {
            script(&mut plain, i);
            script(&mut enc, i);
            assert_eq!(snap(&mut enc), snap(&mut plain), "paso {i}");
            assert_eq!(enc.list(&format!("{H}\\Escritorio")).unwrap(), plain.list(&format!("{H}\\Escritorio")).unwrap());
            assert_eq!(enc.undo_label(), plain.undo_label());
        }
        // Mayúsculas y minúsculas da igual en las rutas.
        assert!(enc.exists(&format!("{}\\DOCUMENTOS\\FINAL.TXT", H.to_uppercase())));
        assert_eq!(enc.read("c:/usuarios/ana/documentos/final.txt").unwrap(), secret_body(4));
        // Cosas inexistentes y errores iguales.
        for p in ["Documentos\\no-existe.txt", "Documentos\\final.txt\\x", "NoExiste\\a.txt"] {
            assert_eq!(enc.read(&format!("{H}\\{p}")), plain.read(&format!("{H}\\{p}")), "{p}");
        }
        assert_eq!(enc.list(&format!("{H}\\Documentos\\final.txt")).err(), Some(FsError::NotDir));
        assert_eq!(enc.read(&format!("{H}\\Documentos")), Err(FsError::IsDir));
        assert_eq!(enc.mkdir(&format!("{H}\\Documentos"), d(1)), Err(FsError::Exists));
        assert_eq!(enc.write(&format!("{H}\\Documentos"), b"x", d(1)), Err(FsError::IsDir));
        assert_eq!(enc.rename(&format!("{H}\\Documentos\\final.txt"), &format!("{H}\\Escritorio\\COPIA2")), Err(FsError::Exists));
        assert_eq!(enc.rename(&format!("{H}\\Escritorio\\COPIA2"), &format!("{H}\\Escritorio\\copia2\\dentro")), Err(FsError::Invalid));
        // Las carpetas protegidas siguen protegidas.
        assert_eq!(enc.rename(&format!("{H}\\Documentos"), &format!("{H}\\Docs")), Err(FsError::Invalid));
        assert_eq!(enc.delete(&format!("{H}\\.papelera")), Err(FsError::Invalid));
        // El nombre de lo escrito no se ve en la imagen.
        assert_eq!(raw_has(&disk, &["final.txt", "creado.txt", "COPIA2", "Movido", "carpeta-renombrada"]), None);
    }

    #[test]
    fn mixed_state_works_while_encrypting_and_while_decrypting() {
        for decrypting in [false, true] {
            let (_, mut plain) = hdisk(400);
            fill(&mut plain);
            let (_, mut enc) = hdisk(400);
            fill(&mut enc);
            let mut pool = ready_pool();
            enc.encrypt_home(PW, &mut pool, d(7)).unwrap();
            if decrypting {
                finish_home(&mut enc, 50);
                enc.decrypt_home(PW, d(8)).unwrap();
            }
            for i in 0..SCRIPT_LEN {
                // Un paso de conversión (de dos en dos) entre operación y operación.
                if !enc.home_convert_step(2, d(9)).unwrap() {
                    assert!(enc.home_conversion().is_some());
                }
                script(&mut plain, i);
                script(&mut enc, i);
                assert_eq!(snap(&mut enc), snap(&mut plain), "decrypting={decrypting} paso {i}");
            }
            finish_home(&mut enc, 4);
            assert_eq!(snap(&mut enc), snap(&mut plain));
            assert_eq!(enc.home_encrypted(), !decrypting);
        }
    }

    /// Ruta en disco (relativa) de un elemento de una carpeta cifrada, dada su ruta en claro.
    fn raw_of(f: &mut Files, path: &str) -> String {
        let c = canon(path).unwrap();
        let found = f.find_enc(rel(&c)).unwrap();
        join_rel(&found.parent, &found.item.unwrap().raw)
    }

    fn encrypted_ana(blocks: usize) -> (Cdisk, Files) {
        let (disk, mut f) = hdisk(blocks);
        fill(&mut f);
        f.encrypt_home(PW, &mut ready_pool(), d(7)).unwrap();
        finish_home(&mut f, 50);
        (disk, f)
    }

    fn home_cut_everywhere(decrypting: bool) {
        let (disk, mut f) = hdisk(160);
        fill(&mut f);
        if decrypting {
            f.encrypt_home(PW, &mut ready_pool(), d(7)).unwrap();
            finish_home(&mut f, 50);
        }
        let before = snap(&mut f);
        let base = disk.bytes();
        let run = |limit: Option<usize>| -> (Cdisk, usize) {
            let dk = Cdisk::from_bytes(base.clone());
            let mut g = Files::probe(Some(Box::new(dk.clone())), d(4));
            g.login("Ana", PW).unwrap();
            let start = dk.writes();
            if let Some(k) = limit {
                dk.limit.set(start + k);
            }
            if decrypting {
                g.decrypt_home(PW, d(5)).unwrap();
            } else {
                g.encrypt_home(PW, &mut ready_pool(), d(5)).unwrap();
            }
            // Pasa lo que pase en memoria: lo que importa es la imagen que queda.
            for _ in 0..5000 {
                if !matches!(g.home_convert_step(1, d(5)), Ok(false)) {
                    break;
                }
            }
            let n = dk.writes() - start;
            (dk, n)
        };
        let (_, total) = run(None);
        assert!(total > 30, "{total}");
        for k in 0..=total {
            let (dk, _) = run(Some(k));
            let (after, mut g) = reboot(&dk);
            g.login("Ana", PW).unwrap_or_else(|e| panic!("k={k}: {e:?}"));
            assert_eq!(snap(&mut g), before, "k={k}");
            let started = g.account_encrypted("Ana");
            finish_home(&mut g, 7);
            assert_eq!(snap(&mut g), before, "k={k} tras terminar");
            if !decrypting && started {
                assert!(g.home_encrypted(), "k={k}");
                assert_eq!(raw_has(&after, &NEEDLES), None, "k={k}");
            }
            if decrypting && !g.home_encrypted() {
                assert!(!g.account_encrypted("Ana"), "k={k}");
                // Sin cifrado: lo hay en claro en el disco.
                let names: Vec<String> = g.fs.list("Usuarios\\Ana").unwrap().into_iter().map(|e| e.name).collect();
                assert!(names.iter().any(|n| n == "Documentos") && !names.iter().any(|n| n.starts_with("$e") || n.starts_with("$E")), "k={k} {names:?}");
            }
            // Y tras otro reinicio sigue igual.
            let (_, mut h) = reboot(&after);
            h.login("Ana", PW).unwrap();
            assert_eq!(snap(&mut h), before, "k={k} reinicio");
        }
    }

    #[test]
    fn power_cut_at_every_write_while_encrypting_the_home() {
        home_cut_everywhere(false);
    }

    #[test]
    fn power_cut_at_every_write_while_decrypting_the_home() {
        home_cut_everywhere(true);
    }

    #[test]
    fn altered_files_and_names_are_never_trusted() {
        let (_, mut f) = encrypted_ana(400);
        let p = format!("{H}\\Documentos\\{SECRET_NAME}");
        let rawp = raw_of(&mut f, &p);
        let orig = f.fs.read(&rawp).unwrap();
        let h = homecrypt::HEADER;
        let mut cases: Vec<Vec<u8>> = Vec::new();
        for i in [0, 5, 6, 14, h + 5, orig.len() - 1] {
            let mut b = orig.clone();
            b[i] ^= 1;
            cases.push(b);
        }
        cases.push(orig[..orig.len() - 1].to_vec());
        cases.push(orig[..10].to_vec());
        cases.push(orig[..h].to_vec());
        cases.push(Vec::new());
        let mut longer = orig.clone();
        longer.push(0);
        cases.push(longer);
        cases.push(secret_body(0)); // el mismo contenido pero en claro
        cases.push(vec![0; orig.len()]);
        for (i, bad) in cases.iter().enumerate() {
            f.fs.write(&rawp, bad, d(9)).unwrap();
            assert_eq!(f.read(&p), Err(FsError::Corrupt), "caso {i}");
            // Lo que no verifica tampoco se copia ni se mueve a otro sitio.
            assert!(f.copy(&p, "C:\\copia-mala.txt", d(9)).is_err());
            assert!(!f.exists("C:\\copia-mala.txt"));
        }
        f.fs.write(&rawp, &orig, d(2)).unwrap();
        assert_eq!(f.read(&p).unwrap(), secret_body(0));
        // Nombre alterado, de otra llave, o en claro: no aparece.
        let dir = format!("{H}\\Documentos\\carpeta-reservada-secreta");
        let raw_dir = raw_of(&mut f, &dir);
        let file_raw = raw_of(&mut f, &format!("{dir}\\segundo.txt"));
        let leaf = file_raw.rsplit('\\').next().unwrap().to_string();
        let mut bad = leaf.clone().into_bytes();
        let last = bad.len() - 2;
        bad[last] = if bad[last] == b'A' { b'B' } else { b'A' };
        f.fs.rename(&file_raw, &join_rel(&raw_dir, &String::from_utf8(bad).unwrap())).unwrap();
        f.hc.cache.clear();
        assert_eq!(f.read(&format!("{dir}\\segundo.txt")), Err(FsError::NotFound));
        assert_eq!(names(&mut f, &dir), ["nombre-largo-subcarpeta"]);
        let other = Keys::new(&[9; 64]);
        let foreign = homecrypt::encode_name(&other, "intruso.txt").unwrap();
        f.fs.write(&join_rel(&raw_dir, &foreign), b"x", d(9)).unwrap();
        f.fs.write(&join_rel(&raw_dir, "intruso-en-claro.txt"), b"y", d(9)).unwrap();
        f.hc.cache.clear();
        assert_eq!(names(&mut f, &dir), ["nombre-largo-subcarpeta"]);
        assert_eq!(f.read(&format!("{dir}\\intruso-en-claro.txt")), Err(FsError::NotFound));
        // Borrar la carpeta se lleva también lo que no se ve.
        f.delete(&dir).unwrap();
        assert!(!f.fs.stat(&raw_dir).is_ok());
    }

    #[test]
    fn without_the_key_everything_is_denied() {
        let (_, mut f) = encrypted_ana(500);
        f.create_account("Boss", None, true, [5; 16], d(9)).unwrap();
        f.create_account("Luis", Some("luis-clave"), false, [6; 16], d(9)).unwrap();
        let p = format!("{H}\\Documentos\\{SECRET_NAME}");
        let docs = format!("{H}\\Documentos");
        for (u, pw) in [("Luis", "luis-clave"), ("Boss", "")] {
            f.login(u, pw).unwrap();
            assert_eq!(f.read(&p), Err(FsError::Denied), "{u}");
            assert_eq!(f.stat(&p), Err(FsError::Denied));
            assert!(!f.exists(&p));
            assert_eq!(f.list(&docs).err(), Some(FsError::Denied));
            assert_eq!(f.list(H).err(), Some(FsError::Denied));
            assert_eq!(f.write(&format!("{docs}\\x.txt"), b"x", d(9)), Err(FsError::Denied));
            assert_eq!(f.mkdir(&format!("{docs}\\x"), d(9)), Err(FsError::Denied));
            assert_eq!(f.delete(&p), Err(FsError::Denied));
            assert_eq!(f.trash(&p, d(9)), Err(FsError::Denied));
            assert_eq!(f.rename(&p, "C:\\x.txt"), Err(FsError::Denied));
            assert_eq!(f.copy(&p, "C:\\x.txt", d(9)), Err(FsError::Denied));
            assert!(f.transfer(&[p.clone()], "C:\\", false, d(9)).is_err());
            assert_eq!(f.read("C:\\Sistema\\config\\llaves\\Ana"), Err(FsError::Denied));
            assert_eq!(f.write("C:\\Sistema\\config\\llaves\\Ana", b"x", d(9)).is_err(), true);
        }
        // Sin sesión.
        f.logout();
        assert_eq!(f.read(&p), Err(FsError::Denied));
        assert_eq!(f.list(&docs).err(), Some(FsError::Denied));
        assert!(f.hc.keys.is_none() && f.hc.state.is_none());
        // Bloqueo: la llave se va y solo la clave correcta la devuelve.
        f.login("Ana", PW).unwrap();
        assert_eq!(f.read(&p).unwrap(), secret_body(0));
        f.lock_keys();
        assert!(f.hc.keys.is_none());
        assert_eq!(f.read(&p), Err(FsError::Denied));
        assert_eq!(f.write(&p, b"x", d(9)), Err(FsError::Denied));
        assert_eq!(f.list(&docs).err(), Some(FsError::Denied));
        assert!(f.home_encrypted());
        assert!(!f.verify_password("mala-clave"));
        assert_eq!(f.read(&p), Err(FsError::Denied));
        assert!(f.verify_password(PW));
        assert_eq!(f.read(&p).unwrap(), secret_body(0));
        // Una cuenta ajena sin cifrar sigue siendo normal para quien tiene permiso.
        assert!(f.list("C:\\Usuarios\\Luis").is_ok());
    }

    #[test]
    fn password_change_keeps_the_same_key_and_removing_it_is_refused() {
        let (disk, mut f) = encrypted_ana(400);
        let before = snap(&mut f);
        let p = format!("{H}\\Documentos\\{SECRET_NAME}");
        let rawp = raw_of(&mut f, &p);
        let raw_before = f.fs.read(&rawp).unwrap();
        let kf_before = f.fs.read("Sistema\\config\\llaves\\Ana").unwrap();
        assert_eq!(f.set_password(PW, None, [3; 16], d(9)), Err(AccountError::Encrypted));
        assert_eq!(f.set_password("mala", Some("clave-nueva"), [3; 16], d(9)), Err(AccountError::WrongPassword));
        assert_eq!(f.set_password(PW, Some("abc"), [3; 16], d(9)), Err(AccountError::WeakPassword));
        assert_eq!(f.fs.read("Sistema\\config\\llaves\\Ana").unwrap(), kf_before);
        f.set_password(PW, Some("clave-nueva-2"), [3; 16], d(9)).unwrap();
        // No se ha recifrado nada.
        assert_eq!(f.fs.read(&rawp).unwrap(), raw_before);
        let kf = String::from_utf8(f.fs.read("Sistema\\config\\llaves\\Ana").unwrap()).unwrap();
        assert_eq!(kf.matches("clave=").count(), 1);
        assert!(KeyFile::parse(&kf).unwrap().open_pass(PW).is_none());
        f.logout();
        assert_eq!(f.login("Ana", PW), Err(AccountError::WrongPassword));
        f.login("Ana", "clave-nueva-2").unwrap();
        assert_eq!(snap(&mut f), before);
        let (_, mut g) = reboot(&disk);
        g.login("Ana", "clave-nueva-2").unwrap();
        assert_eq!(snap(&mut g), before);
    }

    #[test]
    fn password_change_cut_at_every_write_never_locks_the_folder() {
        let (disk, mut f) = encrypted_ana(300);
        let before = snap(&mut f);
        let base = disk.bytes();
        let run = |limit: Option<usize>| -> (Cdisk, usize) {
            let dk = Cdisk::from_bytes(base.clone());
            let mut g = Files::probe(Some(Box::new(dk.clone())), d(4));
            g.login("Ana", PW).unwrap();
            let start = dk.writes();
            if let Some(k) = limit {
                dk.limit.set(start + k);
            }
            let _ = g.set_password(PW, Some("clave-nueva-2"), [3; 16], d(5));
            (dk.clone(), dk.writes() - start)
        };
        let (_, total) = run(None);
        for k in 0..=total {
            let (dk, _) = run(Some(k));
            let (_, mut g) = reboot(&dk);
            // Una de las dos claves abre siempre, y con ella se ve todo.
            if g.login("Ana", "clave-nueva-2").is_err() {
                g.login("Ana", PW).unwrap_or_else(|e| panic!("k={k}: {e:?}"));
            }
            assert_eq!(snap(&mut g), before, "k={k}");
        }
    }

    #[test]
    fn recovery_code_resets_the_password_and_new_code_replaces_the_old() {
        let (disk, mut f) = hdisk(400);
        fill(&mut f);
        let code = f.encrypt_home(PW, &mut ready_pool(), d(7)).unwrap();
        finish_home(&mut f, 50);
        let before = snap(&mut f);
        // El código nuevo invalida el anterior.
        assert_eq!(f.new_home_recovery("mala", &mut ready_pool(), d(8)), Err(KeyError::WrongSecret));
        assert_eq!(f.new_home_recovery(PW, &mut Pool::new(), d(8)), Err(KeyError::NotReady));
        let code2 = f.new_home_recovery(PW, &mut ready_pool(), d(8)).unwrap();
        assert_ne!(code, code2);
        f.logout();
        // Sin sesión.
        let mut pool = ready_pool();
        assert_eq!(f.recover_account("Ana", &code, "clave-recuperada", &mut pool, d(9)), Err(KeyError::WrongSecret));
        assert_eq!(f.recover_account("Ana", "ABCDE-ABCDE", "clave-recuperada", &mut pool, d(9)), Err(KeyError::WrongSecret));
        assert_eq!(f.recover_account("Ana", &code2, "abc", &mut pool, d(9)), Err(KeyError::Weak));
        assert_eq!(f.recover_account("Ana", &code2, "clave-recuperada", &mut Pool::new(), d(9)), Err(KeyError::NotReady));
        assert_eq!(f.recover_account("Nadie", &code2, "clave-recuperada", &mut pool, d(9)), Err(KeyError::NotEncrypted));
        // Nada ha cambiado con los intentos fallidos.
        f.login("Ana", PW).unwrap();
        assert_eq!(snap(&mut f), before);
        f.logout();
        let kf = f.fs.read("Sistema\\config\\llaves\\Ana").unwrap();
        let snapshot_accounts = f.fs.read(rel(ACCOUNTS_FILE)).unwrap();
        // Bien (minúsculas y espacios también valen).
        f.recover_account("Ana", &code2.to_lowercase().replace('-', " "), "clave-recuperada", &mut pool, d(9)).unwrap();
        assert!(f.user().is_none());
        assert_ne!(f.fs.read("Sistema\\config\\llaves\\Ana").unwrap(), kf);
        assert_ne!(f.fs.read(rel(ACCOUNTS_FILE)).unwrap(), snapshot_accounts);
        assert_eq!(f.login("Ana", PW), Err(AccountError::WrongPassword));
        f.login("Ana", "clave-recuperada").unwrap();
        assert_eq!(snap(&mut f), before);
        // El código sigue valiendo para repetirlo, y el de antes ya no.
        f.logout();
        assert_eq!(f.recover_account("Ana", &code, "otra-clave-mas", &mut pool, d(9)), Err(KeyError::WrongSecret));
        f.recover_account("Ana", &code2, "otra-clave-mas", &mut pool, d(9)).unwrap();
        let (_, mut g) = reboot(&disk);
        g.login("Ana", "otra-clave-mas").unwrap();
        assert_eq!(snap(&mut g), before);
        // Una cuenta sin cifrar no tiene qué recuperar.
        g.create_account("Luis", Some("luis-clave"), false, [6; 16], d(9)).unwrap();
        g.logout();
        assert_eq!(g.recover_account("Luis", &code2, "otra-clave-mas", &mut pool, d(9)), Err(KeyError::NotEncrypted));
        // Y el log no lleva claves ni códigos.
        let log = String::from_utf8(g.fs.read("Sistema\\logs\\sistema.log").unwrap()).unwrap();
        for s in [PW, "clave-recuperada", "otra-clave-mas"].into_iter().chain([code.as_str(), code2.as_str()]) {
            assert!(!log.contains(s) && !log.contains(&s.replace('-', "")));
        }
    }

    #[test]
    fn rename_and_delete_of_an_encrypted_account() {
        let (disk, mut f) = encrypted_ana(500);
        f.create_account("Boss", None, true, [5; 16], d(9)).unwrap();
        let before = snap(&mut f);
        // Ana (administradora) se cambia el nombre: la llave sigue funcionando.
        f.rename_account("Ana", "Ana2", d(9)).unwrap();
        assert!(f.account_encrypted("Ana2") && !f.account_encrypted("Ana"));
        assert_eq!(f.read(&format!("C:\\Usuarios\\Ana2\\Documentos\\{SECRET_NAME}")).unwrap(), secret_body(0));
        f.logout();
        f.login("Ana2", PW).unwrap();
        let after: Vec<String> = snap_of(&mut f, "Ana2");
        assert_eq!(after.len(), before.len());
        let (_, mut g) = reboot(&disk);
        g.login("Ana2", PW).unwrap();
        assert_eq!(snap_of(&mut g, "Ana2").len(), before.len());
        // Boss elimina la cuenta conservando la carpeta: queda ilegible; sin la carpeta, no queda nada.
        g.logout();
        g.login("Boss", "").unwrap();
        g.delete_account("Ana2", true, d(10)).unwrap();
        assert!(!g.account_encrypted("Ana2") && g.fs.stat("Usuarios\\Ana2").is_ok());
        let names: Vec<String> = g.fs.list("Usuarios\\Ana2").unwrap().into_iter().map(|e| e.name).collect();
        assert!(names.iter().all(|n| n.starts_with("$e")), "{names:?}");
        assert_eq!(g.create_account("Ana2", None, false, [1; 16], d(11)), Err(AccountError::Exists));
        assert_eq!(raw_has(&disk, &NEEDLES), None);
        // Cuenta cifrada con eliminación completa.
        let (_, mut h) = encrypted_ana(500);
        h.create_account("Boss", None, true, [5; 16], d(9)).unwrap();
        h.logout();
        h.login("Boss", "").unwrap();
        h.delete_account("Ana", false, d(10)).unwrap();
        assert!(h.fs.stat("Usuarios\\Ana").is_err() && !h.account_encrypted("Ana"));
    }

    fn snap_of(f: &mut Files, user: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![format!("Usuarios\\{user}")];
        while let Some(p) = stack.pop() {
            for e in f.r_list(&p).unwrap() {
                let q = join_rel(&p, &e.name);
                if e.dir {
                    stack.push(q.clone());
                }
                out.push(q);
            }
        }
        out
    }

    #[test]
    fn errors_of_home_encryption() {
        // Sin disco persistente o sin sesión.
        let mut mem = Files::default();
        assert_eq!(mem.encrypt_home(PW, &mut ready_pool(), d(1)), Err(KeyError::Unavailable));
        assert!(!mem.home_encrypted() && !mem.account_encrypted("Usuario"));
        assert_eq!(mem.decrypt_home(PW, d(1)), Err(KeyError::NotEncrypted));
        assert_eq!(mem.new_home_recovery(PW, &mut ready_pool(), d(1)), Err(KeyError::NotEncrypted));
        assert_eq!(mem.home_convert_step(5, d(1)), Ok(true));
        assert_eq!(mem.home_conversion(), None);
        let (_, mut f) = hdisk(400);
        fill(&mut f);
        f.create_account("Luis", None, false, [6; 16], d(1)).unwrap();
        // Clave mala, sin clave, sin mover el ratón.
        assert_eq!(f.encrypt_home("mala", &mut ready_pool(), d(2)), Err(KeyError::WrongSecret));
        assert_eq!(f.encrypt_home("", &mut ready_pool(), d(2)), Err(KeyError::WrongSecret));
        assert_eq!(f.encrypt_home(PW, &mut Pool::new(), d(2)), Err(KeyError::NotReady));
        assert!(!f.account_encrypted("Ana"));
        assert_eq!(f.decrypt_home(PW, d(2)), Err(KeyError::NotEncrypted));
        f.logout();
        assert_eq!(f.encrypt_home(PW, &mut ready_pool(), d(2)), Err(KeyError::Unavailable));
        f.login("Luis", "").unwrap();
        assert_eq!(f.encrypt_home("", &mut ready_pool(), d(2)), Err(KeyError::WrongSecret));
        f.login("Ana", PW).unwrap();
        // Un nombre que no cabría cifrado: se avisa antes de empezar y no se toca nada.
        let long = format!("{H}\\Documentos\\{}", "a".repeat(143));
        f.write(&long, b"x", d(2)).unwrap();
        assert_eq!(f.encrypt_home(PW, &mut ready_pool(), d(2)), Err(KeyError::Fs(FsError::BadName)));
        assert!(!f.account_encrypted("Ana"));
        f.delete(&long).unwrap();
        // En marcha: ocupado; terminado: ya cifrada.
        f.encrypt_home(PW, &mut ready_pool(), d(3)).unwrap();
        assert_eq!(f.encrypt_home(PW, &mut ready_pool(), d(3)), Err(KeyError::Busy));
        assert_eq!(f.decrypt_home(PW, d(3)), Err(KeyError::Busy));
        finish_home(&mut f, 50);
        assert_eq!(f.encrypt_home(PW, &mut ready_pool(), d(3)), Err(KeyError::AlreadyEncrypted));
        assert_eq!(f.decrypt_home("mala", d(3)), Err(KeyError::WrongSecret));
        assert!(f.home_encrypted());
        // Nombres: el más largo que cabe, sí; uno más, no; y tampoco el prefijo reservado.
        let ok = format!("{H}\\Documentos\\{}", "b".repeat(142));
        f.write(&ok, b"y", d(4)).unwrap();
        assert_eq!(f.read(&ok).unwrap(), b"y");
        for n in [143usize, 200] {
            let bad = format!("{H}\\Documentos\\{}", "c".repeat(n));
            assert_eq!(f.write(&bad, b"y", d(4)), Err(FsError::BadName));
            assert_eq!(f.mkdir(&bad, d(4)), Err(FsError::BadName));
            assert_eq!(f.rename(&ok, &bad), Err(FsError::BadName));
        }
        assert_eq!(f.write(&format!("{H}\\Documentos\\$ereservado.txt"), b"y", d(4)), Err(FsError::BadName));
        assert!(f.exists(&ok));
        // Descifrar a medias y cancelar no existe, pero sí sigue usable.
        f.decrypt_home(PW, d(5)).unwrap();
        assert_eq!(f.decrypt_home(PW, d(5)), Err(KeyError::Busy));
        assert_eq!(f.write(&format!("{H}\\Documentos\\$ereservado.txt"), b"y", d(4)), Err(FsError::BadName));
        finish_home(&mut f, 50);
        assert!(!f.home_encrypted() && !f.account_encrypted("Ana"));
    }

    #[test]
    fn move_and_copy_between_encrypted_home_and_outside_and_between_users() {
        let (disk, mut f) = hdisk(700);
        f.create_account("Luis", None, false, [6; 16], d(1)).unwrap();
        f.write("C:\\Usuarios\\Luis\\Documentos\\de-luis.txt", b"L", d(1)).unwrap();
        fill(&mut f);
        f.mkdir("C:\\Fuera", d(1)).unwrap();
        f.encrypt_home(PW, &mut ready_pool(), d(7)).unwrap();
        finish_home(&mut f, 50);
        let p = format!("{H}\\Documentos\\{SECRET_NAME}");
        let dir = format!("{H}\\Documentos\\carpeta-reservada-secreta");
        // Sacar (descifra) conservando la fecha.
        f.rename(&p, "C:\\Fuera\\sacado.txt").unwrap();
        assert!(!f.exists(&p));
        assert_eq!(f.read("C:\\Fuera\\sacado.txt").unwrap(), secret_body(0));
        assert_eq!(f.stat("C:\\Fuera\\sacado.txt").unwrap().modified, d(2));
        // Meter (cifra): en disco el archivo lleva firma y cabecera.
        f.write("C:\\Fuera\\entra.txt", b"TEXTO-QUE-ENTRA", d(3)).unwrap();
        f.rename("C:\\Fuera\\entra.txt", &format!("{H}\\Documentos\\entra.txt")).unwrap();
        let rp = raw_of(&mut f, &format!("{H}\\Documentos\\entra.txt"));
        let raw = f.fs.read(&rp).unwrap();
        assert!(raw.starts_with(b"NFH1") && raw.len() == 15 + homecrypt::HEADER && !contains(&raw, "TEXTO-QUE-ENTRA"));
        assert_eq!(f.stat(&format!("{H}\\Documentos\\entra.txt")).unwrap().modified, d(3));
        // Copiar una carpeta entera hacia fuera y de vuelta.
        f.copy(&dir, "C:\\Fuera\\copia", d(9)).unwrap();
        assert_eq!(f.read("C:\\Fuera\\copia\\segundo.txt").unwrap(), secret_body(1));
        assert!(f.exists("C:\\Fuera\\copia\\nombre-largo-subcarpeta"));
        f.copy("C:\\Fuera\\copia", &format!("{H}\\Escritorio\\otra-copia"), d(9)).unwrap();
        assert_eq!(f.read(&format!("{H}\\Escritorio\\otra-copia\\segundo.txt")).unwrap(), secret_body(1));
        // Mover con transfer y deshacer.
        f.transfer(&[format!("{H}\\Escritorio\\otra-copia")], "C:\\Fuera", true, d(10)).unwrap();
        assert!(!f.exists(&format!("{H}\\Escritorio\\otra-copia")) && f.exists("C:\\Fuera\\otra-copia\\segundo.txt"));
        f.undo(d(10)).unwrap();
        assert_eq!(f.read(&format!("{H}\\Escritorio\\otra-copia\\segundo.txt")).unwrap(), secret_body(1));
        // Destino ocupado.
        assert_eq!(f.rename(&format!("{H}\\Documentos\\entra.txt"), "C:\\Fuera\\sacado.txt"), Err(FsError::Exists));
        // Entre usuarios no se pasa nada, ni de una carpeta cifrada a la de otro ni al revés.
        let luis = "C:\\Usuarios\\Luis\\Documentos";
        assert_eq!(f.copy(&format!("{H}\\Documentos\\entra.txt"), &format!("{luis}\\x.txt"), d(11)), Err(FsError::Denied));
        assert_eq!(f.rename(&format!("{H}\\Documentos\\entra.txt"), &format!("{luis}\\x.txt")), Err(FsError::Denied));
        assert_eq!(f.copy(&format!("{luis}\\de-luis.txt"), &format!("{H}\\Documentos\\l.txt"), d(11)), Err(FsError::Denied));
        assert_eq!(f.rename(&format!("{luis}\\de-luis.txt"), &format!("{H}\\Documentos\\l.txt")), Err(FsError::Denied));
        assert!(f.transfer(&[format!("{luis}\\de-luis.txt")], &format!("{H}\\Documentos"), true, d(11)).is_err());
        assert!(f.exists(&format!("{luis}\\de-luis.txt")) && !f.exists(&format!("{H}\\Documentos\\l.txt")));
        // Al revés, sigue siendo cosa de cada cual: Luis no llega.
        f.login("Luis", "").unwrap();
        assert_eq!(f.copy(&format!("{luis}\\de-luis.txt"), &format!("{H}\\Documentos\\l.txt"), d(11)), Err(FsError::Denied));
        let _ = disk;
    }

    #[test]
    fn same_content_gives_different_ciphertext_and_names_are_stable() {
        let (_, mut f) = encrypted_ana(400);
        let (a, b) = (format!("{H}\\Documentos\\a.txt"), format!("{H}\\Documentos\\b.txt"));
        f.write(&a, b"contenido repetido", d(2)).unwrap();
        f.write(&b, b"contenido repetido", d(3)).unwrap();
        let (ra, rb) = (raw_of(&mut f, &a), raw_of(&mut f, &b));
        let (ca, cb) = (f.fs.read(&ra).unwrap(), f.fs.read(&rb).unwrap());
        assert_ne!(ca[4..], cb[4..]);
        assert_ne!(ca[homecrypt::HEADER..], cb[homecrypt::HEADER..]);
        // Reescribir lo mismo, aunque sea en el mismo instante, cambia el cifrado.
        f.write(&a, b"contenido repetido", d(2)).unwrap();
        assert_ne!(f.fs.read(&ra).unwrap(), ca);
        assert_eq!(f.read(&a).unwrap(), b"contenido repetido");
        // El nombre en disco no depende del momento: mismo nombre, mismo nombre cifrado.
        let r1 = raw_of(&mut f, &format!("{H}\\Documentos\\{SECRET_NAME}"));
        f.logout();
        f.login("Ana", PW).unwrap();
        assert_eq!(raw_of(&mut f, &format!("{H}\\Documentos\\{SECRET_NAME}")), r1);
        // Cada archivo nuevo tiene sal distinta, así que contenidos vacíos también.
        f.write(&format!("{H}\\Documentos\\v1"), b"", d(2)).unwrap();
        f.write(&format!("{H}\\Documentos\\v2"), b"", d(2)).unwrap();
        let (v1, v2) = (raw_of(&mut f, &format!("{H}\\Documentos\\v1")), raw_of(&mut f, &format!("{H}\\Documentos\\v2")));
        assert_ne!(f.fs.read(&v1).unwrap(), f.fs.read(&v2).unwrap());
    }

    #[test]
    fn works_together_with_whole_disk_encryption() {
        let key = "clave-del-disco";
        let (disk, mut f) = hdisk(400);
        fill(&mut f);
        let before = snap(&mut f);
        // Carpeta cifrada primero y luego el disco entero.
        f.encrypt_home(PW, &mut ready_pool(), d(7)).unwrap();
        finish_home(&mut f, 50);
        f.encrypt_disk(key, &mut ready_pool(), d(8)).unwrap();
        // Con las dos conversiones en marcha a la vez se puede seguir usando todo.
        assert!(!f.convert_step(100, d(9)).unwrap());
        f.write(&format!("{H}\\Documentos\\mientras.txt"), b"mientras convierte", d(9)).unwrap();
        finish_conversion(&mut f, 60);
        assert_eq!(f.read(&format!("{H}\\Documentos\\mientras.txt")).unwrap(), b"mientras convierte");
        f.delete(&format!("{H}\\Documentos\\mientras.txt")).unwrap();
        assert_eq!(snap(&mut f), before);
        let (after, mut g) = reboot(&disk);
        assert!(g.disk_locked());
        g.unlock_disk(key, false, d(10)).unwrap();
        g.login("Ana", PW).unwrap();
        assert_eq!(snap(&mut g), before);
        // Quitar el cifrado del disco deja la carpeta como estaba: cifrada.
        g.decrypt_disk(key, d(11)).unwrap();
        finish_conversion(&mut g, 60);
        assert!(!g.disk_encrypted() && g.home_encrypted());
        assert_eq!(snap(&mut g), before);
        assert_eq!(raw_has(&after, &NEEDLES), None);
        let (_, mut h) = reboot(&after);
        assert!(!h.disk_locked());
        h.login("Ana", PW).unwrap();
        assert_eq!(snap(&mut h), before);
        // Y al revés: disco cifrado y después la carpeta, con la limpieza pasando por la capa de cifrado.
        let (disk2, mut f2) = hdisk(400);
        fill(&mut f2);
        f2.encrypt_disk(key, &mut ready_pool(), d(8)).unwrap();
        finish_conversion(&mut f2, 60);
        f2.encrypt_home(PW, &mut ready_pool(), d(9)).unwrap();
        finish_home(&mut f2, 50);
        assert_eq!(snap(&mut f2), before);
        f2.decrypt_disk(key, d(11)).unwrap();
        finish_conversion(&mut f2, 60);
        assert_eq!(raw_has(&disk2, &NEEDLES), None);
        assert_eq!(snap(&mut f2), before);
    }
}

#[cfg(test)]
mod drive_tests {
    use super::*;
    use crate::drives::FormatKind;
    use crate::drives::Locked;
    use crate::vol::{self, FsKind, MemSectors, SectorDev};
    use std::cell::RefCell;
    use std::rc::Rc;

    const NOW: DateTime = DateTime { year: 2026, month: 1, day: 1, hour: 0, minute: 0, second: 0 };

    /// Disco en memoria compartido: se puede volver a montar tras expulsarlo.
    #[derive(Clone)]
    struct Shared(Rc<RefCell<MemSectors>>);
    impl SectorDev for Shared {
        fn sector_size(&self) -> usize {
            self.0.borrow().sector_size()
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

    fn stick(kind: FsKind, label: &str) -> Shared {
        let mut m = MemSectors::new(131072, 512);
        vol::format(&mut m, kind, label, NOW).unwrap();
        Shared(Rc::new(RefCell::new(m)))
    }

    fn mount(s: &Shared) -> Box<dyn Volume> {
        let mut d = s.clone();
        let p = vol::partitions(&mut d).unwrap();
        vol::mount(Box::new(s.clone()), &p[0]).unwrap()
    }

    fn two() -> (Files, char, char) {
        let mut f = Files::default();
        let d = f.attach(1, "Pendrive", mount(&stick(FsKind::Fat32, "FOTOS"))).unwrap();
        let e = f.attach(2, "Disco", mount(&stick(FsKind::ExFat, ""))).unwrap();
        (f, d, e)
    }

    fn names(f: &mut Files, dir: &str) -> Vec<String> {
        f.list(dir).unwrap().into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn ocultos_y_unidades_internas() {
        let s = stick(FsKind::Fat32, "");
        let mut v = mount(&s);
        v.write("visto.txt", b"1", NOW).unwrap();
        v.write("oculto.txt", b"2", NOW).unwrap();
        v.flush().unwrap();
        drop(v);
        let mut f = Files::default();
        // Atributo «oculto» (0x02) puesto a mano en la entrada corta de la raíz.
        {
            let mut m = s.0.borrow_mut();
            let i = m.data.windows(11).position(|w| w == b"OCULTO  TXT").unwrap();
            m.data[i + 11] |= 0x02;
        }
        let d = f.attach_as(1, "Disco interno", Box::new(vol::ReadOnly(mount(&s))), false).unwrap();
        let path = alloc::format!("{d}:\\");
        let view = |f: &mut Files| f.list_view(&path).unwrap().into_iter().map(|e| e.name).collect::<Vec<_>>();
        assert_eq!(view(&mut f), ["visto.txt"]);
        assert_eq!(names(&mut f, &path).len(), 2, "list (copiar, mover) lo ve todo");
        let prefs = f.explorer_prefs();
        f.write(&prefs, b"ocultos=si\n", NOW).unwrap();
        assert_eq!(view(&mut f).len(), 2);
        // Interna y de solo lectura: no es extraíble y no se escribe.
        let info = f.drives().into_iter().find(|i| i.letter == d).unwrap();
        assert!(!info.removable && info.read_only);
        assert_eq!(f.write(&alloc::format!("{d}:\\x.txt"), b"x", NOW), Err(FsError::Denied));
    }

    #[test]
    fn letras_lectura_y_escritura() {
        let (mut f, d, e) = two();
        assert_eq!((d, e), ('D', 'E'));
        let g = f.generation();
        f.mkdir("D:\\Fotos", NOW).unwrap();
        f.write("d:\\Fotos\\a.txt", b"hola", NOW).unwrap();
        assert!(f.generation() > g);
        assert_eq!(f.read("D:\\fotos\\A.TXT").unwrap(), b"hola");
        assert_eq!(names(&mut f, "D:\\"), ["Fotos"]);
        assert!(f.stat("D:\\Fotos").unwrap().dir);
        assert!(f.stat("D:\\").unwrap().dir);
        assert_eq!(f.stat("D:\\nada"), Err(FsError::NotFound));
        assert_eq!(f.list("F:\\"), Err(FsError::BadPath));
        assert_eq!(f.write("D:\\", b"x", NOW), Err(FsError::IsDir));
        assert_eq!(f.free_name("D:\\Fotos", "a.txt"), "a (2).txt");
        assert_eq!(f.free_name("D:\\Fotos", "b.txt"), "b.txt");
        assert!(!is_protected("D:\\Usuarios") && is_protected("D:\\") && !in_system("D:\\Sistema"));
        let info = f.drives();
        assert_eq!(info.iter().map(|i| i.letter).collect::<Vec<_>>(), ['C', 'D', 'E']);
        assert_eq!(info[1].label, "FOTOS");
        assert_eq!(info[1].device, "Pendrive");
        assert!(info[1].removable && !info[0].removable && info[1].total > info[1].free);
        // Una cuenta normal también escribe en las extraíbles.
        f.create_account("Ana", None, false, [1; 16], NOW).unwrap();
        f.login("Ana", "").unwrap();
        f.write("D:\\Fotos\\ana.txt", b"x", NOW).unwrap();
        assert_eq!(f.read("D:\\Fotos\\ana.txt").unwrap(), b"x");
    }

    #[test]
    fn copiar_y_mover_entre_unidades() {
        let (mut f, ..) = two();
        let docs = f.std_dir(Std::Documents);
        let src = join(&docs, "Carpeta");
        f.mkdir(&src, NOW).unwrap();
        f.mkdir(&join(&src, "Sub"), NOW).unwrap();
        f.write(&join(&src, "a.txt"), b"uno", NOW).unwrap();
        f.write(&join(&src, "Sub\\b.txt"), b"dos", NOW).unwrap();
        // C: -> D:
        f.copy(&src, "D:\\Carpeta", NOW).unwrap();
        assert_eq!(f.read("D:\\Carpeta\\Sub\\b.txt").unwrap(), b"dos");
        assert_eq!(f.copy(&src, "D:\\Carpeta", NOW), Err(FsError::Exists));
        // D: -> E: y E: -> C:
        f.copy("D:\\Carpeta", "E:\\Copia", NOW).unwrap();
        let back = join(&docs, "Vuelta");
        f.copy("E:\\Copia", &back, NOW).unwrap();
        assert_eq!(f.read(&join(&back, "a.txt")).unwrap(), b"uno");
        // dentro de sí misma
        assert_eq!(f.copy("D:\\Carpeta", "D:\\Carpeta\\Sub\\x", NOW), Err(FsError::Invalid));
        assert_eq!(f.copy("D:\\", "D:\\y", NOW), Err(FsError::Invalid));
        // mover entre unidades: copia y borra el origen
        let r = f.transfer(&[String::from("D:\\Carpeta")], "E:\\", true, NOW).unwrap();
        assert_eq!(r, ["E:\\Carpeta"]);
        assert!(!f.exists("D:\\Carpeta") && f.exists("E:\\Carpeta\\Sub\\b.txt"));
        // mover dentro de la misma unidad: renombrar, y se deshace
        f.mkdir("E:\\Otra", NOW).unwrap();
        f.transfer(&[String::from("E:\\Carpeta")], "E:\\Otra", true, NOW).unwrap();
        assert!(f.exists("E:\\Otra\\Carpeta\\a.txt") && !f.exists("E:\\Carpeta"));
        f.undo(NOW).unwrap();
        assert!(f.exists("E:\\Carpeta\\a.txt"));
        // copiar a C:
        let r = f.transfer(&[String::from("E:\\Carpeta")], &docs, false, NOW).unwrap();
        assert_eq!(r.len(), 1);
        // pegar con el portapapeles, de C: a D:
        f.set_clip(&src, false);
        assert_eq!(f.paste_all("D:\\", NOW).unwrap(), ["D:\\Carpeta"]);
        // rename entre unidades: no vale
        assert_eq!(f.rename("D:\\Carpeta", "E:\\z"), Err(FsError::Invalid));
        assert_eq!(f.rename(&src, "D:\\z"), Err(FsError::Invalid));
        f.rename("D:\\Carpeta", "D:\\Nueva").unwrap();
        assert_eq!(f.rename("D:\\Nueva", "D:\\Nueva\\Sub\\q"), Err(FsError::Invalid));
        assert_eq!(f.rename("D:\\", "D:\\q"), Err(FsError::Invalid));
    }

    #[test]
    fn borrar_es_definitivo_y_la_papelera_no_se_toca() {
        let (mut f, ..) = two();
        f.mkdir("D:\\x", NOW).unwrap();
        f.write("D:\\x\\a.txt", b"a", NOW).unwrap();
        f.user_trash_many(&[String::from("D:\\x")], NOW).unwrap();
        assert!(!f.exists("D:\\x") && f.trash_list().unwrap().is_empty() && f.undo_label().is_none());
        assert_eq!(f.delete("D:\\"), Err(FsError::Invalid));
        f.user_create("D:\\nuevo.txt", false, NOW).unwrap();
        assert!(f.exists("D:\\nuevo.txt"));
        f.undo(NOW).unwrap();
        assert!(!f.exists("D:\\nuevo.txt"));
    }

    #[test]
    fn quitar_y_reutilizar_letras() {
        let (mut f, ..) = two();
        f.write("D:\\a.txt", b"a", NOW).unwrap();
        let g = f.generation();
        f.detach(1);
        assert!(f.generation() > g);
        assert_eq!(f.read("D:\\a.txt"), Err(FsError::BadPath));
        assert_eq!(f.list("D:\\"), Err(FsError::BadPath));
        assert_eq!(f.drives().len(), 2);
        // La letra libre más baja se reutiliza.
        assert_eq!(f.attach(3, "Otro", mount(&stick(FsKind::Fat32, ""))), Some('D'));
        assert_eq!(f.attach(4, "Otro", mount(&stick(FsKind::Fat32, ""))), Some('F'));
        f.detach(99);
    }

    #[test]
    fn quitar_una_unidad_olvida_deshacer_y_portapapeles() {
        let mut f = Files::default();
        f.attach(1, "A", mount(&stick(FsKind::Fat32, ""))).unwrap();
        f.user_create("D:\\x.txt", false, NOW).unwrap();
        f.set_clip_many(&[String::from("D:\\x.txt")], true);
        // Otra unidad en E: no cuenta.
        f.attach(2, "B", mount(&stick(FsKind::Fat32, ""))).unwrap();
        f.detach(2);
        assert!(f.undo_label().is_some() && !f.clip_paths().0.is_empty());
        // Se quita A y otro pendrive con su propio D:\x.txt recibe la letra D:.
        f.detach(1);
        assert_eq!(f.attach(3, "B", mount(&stick(FsKind::Fat32, ""))), Some('D'));
        f.write("D:\\x.txt", b"de B", NOW).unwrap();
        assert_eq!(f.undo_label(), None);
        assert!(f.clip_paths().0.is_empty());
        assert_eq!(f.read("D:\\x.txt").unwrap(), b"de B");
        // Lo de C: no se olvida al quitar una unidad.
        f.user_create("C:\\y.txt", false, NOW).unwrap();
        f.request_drive_action('D', FormatRequest::Eject).unwrap();
        assert_eq!(f.undo_label(), Some("Nuevo"));
    }

    #[test]
    fn expulsar_escribe_lo_pendiente_y_formatear_encola() {
        let s = stick(FsKind::Fat32, "X");
        let mut f = Files::default();
        f.attach(7, "Pendrive", mount(&s));
        f.write("D:\\dato.txt", b"importante", NOW).unwrap();
        assert_eq!(f.request_drive_action('C', FormatRequest::Eject), Err(FsError::Invalid));
        assert_eq!(f.request_drive_action('Q', FormatRequest::Eject), Err(FsError::BadPath));
        f.request_drive_action('d', FormatRequest::Eject).unwrap();
        assert_eq!(f.take_drive_actions(), [DriveAction::Eject { disk: 7 }]);
        assert!(f.take_drive_actions().is_empty());
        assert_eq!(f.list("D:\\"), Err(FsError::BadPath));
        // Lo escrito llegó al disco: otro montaje lo ve.
        let mut v = mount(&s);
        assert_eq!(v.read("dato.txt").unwrap(), b"importante");
        // El disco expulsado se puede volver a conectar y formatear.
        f.attach(7, "Pendrive", v).unwrap();
        f.request_drive_action('D', FormatRequest::Format { kind: FormatKind::ExFat, label: String::from("NUEVA") }).unwrap();
        assert_eq!(f.take_drive_actions(), [DriveAction::Format { disk: 7, part: 0, kind: FormatKind::ExFat, label: String::from("NUEVA") }]);
        assert_eq!(f.drive_result(), None);
        f.set_drive_result(Ok(Some(String::from("codigo"))));
        assert_eq!(f.drive_result(), Some(Ok(Some(String::from("codigo")))));
        assert_eq!(f.drive_result(), None);
    }

    /// Dos particiones internas del mismo disco: una de solo lectura (ajena) y otra NightFury.
    fn internas() -> (Files, char, char) {
        let mut f = Files::default();
        let a = f.attach_part(INTERNO, 2048, "Disco", Box::new(vol::ReadOnly(mount(&stick(FsKind::Fat32, "A")))), false).unwrap();
        let b = f.attach_part(INTERNO, 900_000, "Disco", mount(&stick(FsKind::ExFat, "B")), false).unwrap();
        (f, a, b)
    }
    const INTERNO: u32 = 1 << 31;

    #[test]
    fn internas_no_se_expulsan_y_formatear_toca_solo_su_particion() {
        let (mut f, a, b) = internas();
        assert_eq!(f.request_drive_action(a, FormatRequest::Eject), Err(FsError::Denied));
        assert_eq!(f.request_drive_action(b, FormatRequest::Eject), Err(FsError::Denied));
        assert!(f.take_drive_actions().is_empty());
        assert_eq!(f.drives().len(), 3, "denegar no desmonta nada");
        // La de solo lectura (Windows, Linux) sí se puede formatear; la otra partición sigue montada.
        f.request_drive_action(a, FormatRequest::Format { kind: FormatKind::ExFat, label: String::from("N") }).unwrap();
        assert_eq!(f.take_drive_actions(), [DriveAction::Format { disk: INTERNO, part: 2048, kind: FormatKind::ExFat, label: String::from("N") }]);
        let l: Vec<char> = f.drives().iter().map(|d| d.letter).collect();
        assert_eq!(l, ['C', b]);
        f.request_drive_action(b, FormatRequest::Encrypt { label: String::new(), password: String::from("secreto12") }).unwrap();
        assert_eq!(f.take_drive_actions(), [DriveAction::Encrypt { disk: INTERNO, part: 900_000, label: String::new(), password: String::from("secreto12") }]);
        // En USB, expulsar sigue valiendo.
        let mut g = Files::default();
        let d = g.attach(1, "USB", mount(&stick(FsKind::Fat32, ""))).unwrap();
        g.request_drive_action(d, FormatRequest::Eject).unwrap();
    }

    #[test]
    fn unidades_cfg_apunta_particiones_y_las_apps_no_lo_leen() {
        let mut f = Files::default();
        assert!(!f.internal_writable("SER|1", 2048));
        f.set_internal_writable("SER|1", 2048, true, NOW).unwrap();
        f.set_internal_writable("SER|1", 2048, true, NOW).unwrap();
        f.set_internal_writable("SER|1", 4096, true, NOW).unwrap();
        f.set_internal_writable("OTRO", 2048, true, NOW).unwrap();
        assert!(f.internal_writable("SER|1", 2048) && f.internal_writable("SER|1", 4096) && f.internal_writable("OTRO", 2048));
        assert!(!f.internal_writable("SER|1", 1) && !f.internal_writable("SER", 2048));
        f.set_internal_writable("SER|1", 2048, false, NOW).unwrap();
        assert!(!f.internal_writable("SER|1", 2048) && f.internal_writable("SER|1", 4096));
        assert_eq!(f.read("C:\\Sistema\\unidades.cfg").unwrap().iter().filter(|&&c| c == b'\n').count(), 2);
        // Un programa de terceros no lo lee ni lo escribe (el servicio bloquea C:\Sistema).
        assert!(crate::service::blocked(false, "C:\\Sistema\\unidades.cfg"));
    }

    /// Disco cifrado en memoria con contraseña `clave-buena`: la unidad cerrada y su código de recuperación.
    fn cifrada() -> (Files, char, String) {
        let s = Shared(Rc::new(RefCell::new(MemSectors::new((16 << 20) / 512, 512))));
        let mut pool = Pool::new();
        for i in 0..keys::POOL_EVENTS_NEEDED as i32 {
            pool.add_event(i, i * 3, i as u64, i as u64 * 7);
        }
        let code = vol::nfvol::format(Box::new(s.clone()), "Sec", Some(("clave-buena", &mut pool)), NOW).unwrap().unwrap();
        let p = vol::partitions(&mut s.clone()).unwrap();
        let vol::nfvol::Opened::Locked(drive) = vol::nfvol::open(Box::new(s.clone()), &p[0]).unwrap() else { panic!("debía estar cerrada") };
        let mut f = Files::default();
        let l = f.attach_locked(Locked { disk: 1, part: 0, removable: true, device: String::from("USB"), drive }).unwrap();
        (f, l, code)
    }

    /// Avanza el desbloqueo en trozos pequeños hasta que acaba.
    fn por_pasos(f: &mut Files) -> Result<(), KeyError> {
        let mut n = 0;
        while f.unlock_step(7) {
            n += 1;
            assert!(n < 100_000);
        }
        f.unlock_result().unwrap()
    }

    #[test]
    fn desbloqueo_por_pasos() {
        let (mut f, l, _) = cifrada();
        assert!(f.drives().iter().any(|d| d.letter == l && d.locked));
        // Sin trabajo no hay nada que avanzar ni resultado.
        assert!(!f.unlock_step(10) && f.unlock_result().is_none());
        // Contraseña mala: error y sigue cerrada; un código mal tecleado ni empieza.
        f.begin_unlock(l, "otra-clave", false).unwrap();
        assert_eq!(por_pasos(&mut f), Err(KeyError::WrongSecret));
        assert_eq!(f.begin_unlock(l, "AAAAA", true), Err(KeyError::WrongSecret));
        assert_eq!(f.begin_unlock('Q', "x", false), Err(KeyError::Fs(FsError::BadPath)));
        assert!(f.drives().iter().any(|d| d.letter == l && d.locked));
        // Un trabajo a medias se descarta al empezar otro; no queda resultado viejo.
        f.begin_unlock(l, "otra-clave", false).unwrap();
        assert!(f.unlock_step(3));
        f.begin_unlock(l, "clave-buena", false).unwrap();
        assert!(f.unlock_result().is_none());
        let gen = f.generation();
        assert_eq!(por_pasos(&mut f), Ok(()));
        assert_ne!(f.generation(), gen);
        assert!(f.drives().iter().any(|d| d.letter == l && !d.locked));
        assert_eq!(f.begin_unlock(l, "clave-buena", false), Err(KeyError::NotEncrypted));
        // Con el código de recuperación.
        let (mut f, l, code) = cifrada();
        f.begin_unlock(l, &code.to_lowercase().replace('-', ""), true).unwrap();
        assert_eq!(por_pasos(&mut f), Ok(()));
        assert!(f.drives().iter().any(|d| d.letter == l && !d.locked));
    }

    #[test]
    fn solo_lectura_y_unidad_desconocida() {
        let s = stick(FsKind::Fat32, "RO");
        let mut f = Files::default();
        s.0.borrow_mut().ro = true;
        f.attach(1, "Lector", mount(&s));
        assert_eq!(f.write("D:\\a", b"x", NOW), Err(FsError::Denied));
        assert_eq!(f.request_drive_action('D', FormatRequest::Format { kind: FormatKind::Fat32, label: String::new() }), Err(FsError::Denied));
        assert_eq!(f.unlock_drive('D', "x", false), Err(KeyError::NotEncrypted));
        assert_eq!(f.unlock_drive('Q', "x", false), Err(KeyError::Fs(FsError::BadPath)));
    }
}
