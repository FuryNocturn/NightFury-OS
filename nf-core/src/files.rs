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
use crate::fs::{same, Entry, FsError, Nffs};
use crate::platform::DateTime;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Ubicación especial que el Explorador muestra como la Papelera (no es una ruta de disco).
pub const TRASH: &str = "Papelera";
// Rutas de la estructura ANTIGUA: siguen aquí mientras otros módulos las usan; en la nueva no
// existen (usa `Files::std_dir`, `config_file` y `explorer_prefs`). No llevan `#[deprecated]`
// para no llenar de avisos de compilación a esos módulos.
pub const DESKTOP_DIR: &str = "C:\\Escritorio";
pub const DOCUMENTS_DIR: &str = "C:\\Documentos";
pub const PICTURES_DIR: &str = "C:\\Imágenes";
/// Archivo de configuración del sistema (texto, líneas `clave=valor`).
pub const CONFIG_FILE: &str = "C:\\Sistema\\config.txt";
/// Preferencias del Explorador: `vista=`, `orden=`, `desc=`, `extensiones=`.
pub const EXPLORER_PREFS: &str = "C:\\Sistema\\explorador.txt";

// ---- Estructura del disco (v0.5.0). Ningún otro módulo escribe estas rutas a mano. ----
pub const USERS_DIR: &str = "C:\\Usuarios";
pub const SYSTEM_DIR: &str = "C:\\Sistema";
pub const SYS_CONFIG_DIR: &str = "C:\\Sistema\\config";
pub const SYS_TEMP_DIR: &str = "C:\\Sistema\\temp";
pub const SYS_LIB_DIR: &str = "C:\\Sistema\\lib";
pub const SYS_DRIVERS_DIR: &str = "C:\\Sistema\\drivers";
pub const SYS_LOGS_DIR: &str = "C:\\Sistema\\logs";
/// Cuentas de usuario (formato en `accounts.rs`).
pub const ACCOUNTS_FILE: &str = "C:\\Sistema\\config\\cuentas";
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
/// Sitio de espera de lo suelto de `Sistema` que había en la estructura antigua.
const OLD_DIR: &str = "C:\\Sistema\\config\\antiguo";
/// Tamaño máximo del registro.
const LOG_MAX: usize = 64 * 1024;
/// Bloques del disco en memoria (4 MiB).
const MEMORY_BLOCKS: u64 = 1024;

const NOTICE_NO_DISK: &str = "No hay disco: los archivos se guardan solo en la memoria y se perderán al apagar.";
const NOTICE_IGNORED: &str =
    "El disco tiene otro contenido y se ha ignorado para no dañarlo: los archivos se guardan solo en la memoria y se perderán al apagar.";
const NOTICE_DAMAGED: &str =
    "El disco de NightFury está dañado y no se ha tocado: los archivos se guardan solo en la memoria y se perderán al apagar.";
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
        Some(_) => Some(String::from("C:\\")),
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

/// Quita la letra de unidad si la hay. Devuelve (resto, tenía_unidad).
fn strip_drive(s: &str) -> Result<(&str, bool), FsError> {
    let b = s.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        if b[0].to_ascii_uppercase() != b'C' {
            return Err(FsError::BadPath);
        }
        return Ok((&s[2..], true));
    }
    Ok((s, false))
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
    let mut stack = Vec::new();
    if !drive && !rest.starts_with(['\\', '/']) {
        apply(&mut stack, strip_drive(cwd)?.0)?;
    }
    apply(&mut stack, rest)?;
    let mut out = String::from("C:\\");
    out.push_str(&stack.join("\\"));
    Ok(out)
}

/// Ruta canónica (válida y fuera del almacén de la Papelera) de lo que pide una app.
fn canon(path: &str) -> Result<String, FsError> {
    let c = resolve("C:\\", path)?;
    // La Papelera de la estructura antigua (huérfana hasta que una cuenta la adopta) no se toca.
    if same(first(rel(&c)), LEGACY_STORE) {
        return Err(FsError::BadPath);
    }
    Ok(c)
}

/// ¿Está `path` dentro de `C:\Sistema` (o es esa carpeta)? Para que las apps pidan confirmación.
pub fn in_system(path: &str) -> bool {
    resolve("C:\\", path).is_ok_and(|c| same(first(rel(&c)), SYSTEM))
}

/// ¿Es una carpeta que no se puede eliminar, mover ni renombrar? (raíz, `Usuarios`, `Sistema` y
/// sus subcarpetas, la carpeta de cada cuenta y sus carpetas estándar, `.config` y `.papelera`).
pub fn is_protected(path: &str) -> bool {
    resolve("C:\\", path).is_ok_and(|c| protected(rel(&c)))
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
    same(top, SYSTEM) && (c.len() == 1 || (c.len() == 2 && SYS_SUBS.iter().any(|s| same(s, c[1]))))
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
}

impl Default for Files {
    /// Unidad C: en memoria con las carpetas iniciales.
    fn default() -> Files {
        Files::memory(NOTICE_NO_DISK, None, DateTime::default())
    }
}

impl Files {
    fn memory(notice: &'static str, pending: Option<Box<dyn BlockDev>>, now: DateTime) -> Files {
        Files { fs: new_memory(now), persistent: false, pending, notice: Some(notice), generation: 0, clip: None, undo: None, user: Some((String::from(DEFAULT_USER), true)) }
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
        if Nffs::is_formatted(&mut *dev) {
            // Con NFFS dañado no se formatea nunca: se deja el disco como está.
            return match Nffs::mount(dev) {
                Ok(fs) => {
                    let mut f = Files { fs, persistent: true, pending: None, notice: None, generation: 0, clip: None, undo: None, user: None };
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
        let mut fs = Nffs::format(dev, now)?;
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

    /// Entradas de una carpeta: carpetas primero, luego por nombre sin distinguir mayúsculas.
    /// No incluye la Papelera antigua de la raíz ni, dentro de la carpeta de un usuario, lo que
    /// empieza por punto (`.config`, `.papelera`). En `Usuarios`, un usuario normal solo ve la suya.
    pub fn list(&mut self, dir: &str) -> Result<Vec<Entry>, FsError> {
        let c = canon(dir)?;
        self.guard(rel(&c), false)?;
        let mut v = self.fs.list(rel(&c))?;
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
        self.guard(rel(&c), false)?;
        self.fs.stat(rel(&c))
    }

    pub fn exists(&mut self, path: &str) -> bool {
        self.stat(path).is_ok()
    }

    pub fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        let c = canon(path)?;
        self.guard(rel(&c), false)?;
        self.fs.read(rel(&c))
    }

    /// Crea o sustituye un archivo (atómico).
    pub fn write(&mut self, path: &str, data: &[u8], now: DateTime) -> Result<(), FsError> {
        let c = canon(path)?;
        if rel(&c).is_empty() {
            return Err(FsError::IsDir);
        }
        self.guard(rel(&c), true)?;
        self.generation += 1;
        self.fs.write(rel(&c), data, now)
    }

    pub fn mkdir(&mut self, path: &str, now: DateTime) -> Result<(), FsError> {
        let c = canon(path)?;
        if rel(&c).is_empty() {
            return Err(FsError::Exists);
        }
        self.guard(rel(&c), true)?;
        self.generation += 1;
        self.fs.mkdir(rel(&c), now)
    }

    /// Mueve o renombra. Falla con `Exists` si el destino existe.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        let (f, t) = (canon(from)?, canon(to)?);
        if protected(rel(&f)) {
            return Err(FsError::Invalid);
        }
        self.guard(rel(&f), true)?;
        if rel(&t).is_empty() {
            return Err(FsError::Exists);
        }
        self.guard(rel(&t), true)?;
        self.generation += 1;
        self.fs.rename(rel(&f), rel(&t))
    }

    /// Copia un archivo o una carpeta entera. Falla con `Exists` si el destino existe.
    pub fn copy(&mut self, from: &str, to: &str, now: DateTime) -> Result<(), FsError> {
        let (f, t) = (canon(from)?, canon(to)?);
        let (f, t) = (rel(&f), rel(&t));
        self.guard(f, false)?;
        self.guard(t, true)?;
        let src = self.fs.stat(f)?;
        let inside = f.is_empty() || {
            let (lf, lt) = (lower(f), lower(t));
            lt.len() > lf.len() && lt.starts_with(&lf) && lt.as_bytes()[lf.len()] == b'\\'
        };
        if src.dir && inside {
            return Err(FsError::Invalid);
        }
        if t.is_empty() || self.fs.stat(t).is_ok() {
            return Err(FsError::Exists);
        }
        self.generation += 1;
        let r = self.copy_tree(f, t, now);
        if r.is_err() {
            let _ = self.rm_tree(t);
        }
        r
    }

    fn copy_tree(&mut self, from: &str, to: &str, now: DateTime) -> Result<(), FsError> {
        let mut work = vec![(String::from(from), String::from(to))];
        while let Some((s, d)) = work.pop() {
            if self.fs.stat(&s)?.dir {
                self.fs.mkdir(&d, now)?;
                for e in self.fs.list(&s)? {
                    work.push((join_rel(&s, &e.name), join_rel(&d, &e.name)));
                }
            } else {
                let data = self.fs.read(&s)?;
                self.fs.write(&d, &data, now)?;
            }
        }
        Ok(())
    }

    /// Borra `rel` y todo lo que contiene (sin protecciones; las comprueban quienes llaman).
    fn rm_tree(&mut self, rel: &str) -> Result<(), FsError> {
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
        if protected(rel(&c)) {
            return Err(FsError::Invalid);
        }
        self.guard(rel(&c), true)?;
        self.generation += 1;
        self.rm_tree(rel(&c))
    }

    fn free_rel(&mut self, dir: &str, name: &str) -> String {
        if self.fs.stat(&join_rel(dir, name)).is_err() {
            return String::from(name);
        }
        let (stem, ext) = match name.rfind('.') {
            Some(i) if i > 0 => name.split_at(i),
            _ => (name, ""),
        };
        let mut n = 2;
        loop {
            let cand = format!("{stem} ({n}){ext}");
            if self.fs.stat(&join_rel(dir, &cand)).is_err() {
                return cand;
            }
            n += 1;
        }
    }

    /// Nombre libre en `dir` parecido a `name`: "nota.txt", "nota (2).txt", "nota (3).txt"...
    pub fn free_name(&mut self, dir: &str, name: &str) -> String {
        match canon(dir) {
            Ok(c) => self.free_rel(rel(&c), name),
            Err(_) => String::from(name),
        }
    }

    // ---- Papelera (una por usuario, en `<home>\.papelera`) ----

    /// Almacén de la Papelera del usuario de la sesión (relativo a la raíz); `None` sin sesión.
    fn store(&self) -> Option<String> {
        self.user().map(|u| join_rel(&join_rel(USERS, u), HOME_TRASH))
    }

    fn read_index_at(&mut self, store: &str) -> Vec<(String, DateTime, String)> {
        let text = self.fs.read(&join_rel(store, INDEX_NAME)).ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
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
        if self.fs.stat(store).is_err() {
            self.fs.mkdir(store, now)?;
        }
        self.fs.write(&join_rel(store, INDEX_NAME), s.as_bytes(), now)
    }

    /// Siguiente identificador libre de un almacén (mayor del índice y de los elementos, más uno).
    fn next_id(&mut self, store: &str, idx: &[(String, DateTime, String)]) -> String {
        let mut next = idx.iter().filter_map(|(id, _, _)| id.parse::<u64>().ok()).max().unwrap_or(0);
        for e in self.fs.list(store).unwrap_or_default() {
            next = next.max(e.name.parse::<u64>().unwrap_or(0));
        }
        format!("{:08}", next + 1)
    }

    /// Envía un archivo o carpeta a la Papelera del usuario de la sesión (también lo que está
    /// fuera de su carpeta, p. ej. en `C:\` si es administrador).
    pub fn trash(&mut self, path: &str, now: DateTime) -> Result<(), FsError> {
        let c = canon(path)?;
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
        self.fs.stat(r)?;
        self.generation += 1;
        let mut idx = self.read_index_at(&store);
        let id = self.next_id(&store, &idx);
        idx.push((id.clone(), now, c.clone()));
        self.write_index_at(&store, &idx, now)?;
        if let Err(e) = self.fs.rename(r, &join_rel(&store, &id)) {
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
            if let Ok(e) = self.fs.stat(&join_rel(&store, &id)) {
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
        self.fs.stat(&item)?;
        let (deleted, original) = (idx[pos].1, idx[pos].2.clone());
        let r = original.get(3..).unwrap_or("");
        // Un índice manipulado no puede sacar nada a un sitio donde el usuario no puede escribir.
        self.guard(r, true)?;
        self.generation += 1;
        let (dir, name) = r.rsplit_once('\\').unwrap_or(("", r));
        let mut cur = String::new();
        for c in dir.split('\\').filter(|c| !c.is_empty()) {
            cur = join_rel(&cur, c);
            match self.fs.stat(&cur) {
                Ok(e) if !e.dir => return Err(FsError::NotDir),
                Ok(_) => {}
                Err(FsError::NotFound) => self.fs.mkdir(&cur, deleted)?,
                Err(e) => return Err(e),
            }
        }
        let dest = join_rel(dir, &self.free_rel(dir, name));
        self.fs.rename(&item, &dest)?;
        idx.remove(pos);
        self.write_index_at(&store, &idx, deleted)?;
        Ok(format!("C:\\{dest}"))
    }

    pub fn empty_trash(&mut self) -> Result<(), FsError> {
        let store = self.store().ok_or(FsError::Invalid)?;
        self.generation += 1;
        for e in self.fs.list(&store)? {
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
            let r = if mv { self.rename(src, &dest) } else { self.copy(src, &dest, now) };
            if let Err(e) = r {
                err = Some(e);
                break;
            }
            done.push(if mv { Undo::Move { from: src.clone(), to: dest.clone() } } else { Undo::Created(dest.clone()) });
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
            done.push(Undo::Trashed(p.clone()));
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
        // Por si la carpeta falta (p. ej. un corte al crear la cuenta).
        ensure_home(&mut self.fs, &a.name, DateTime::default());
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
        self.accounts().iter().find(|a| same(&a.name, &name)).is_some_and(|a| a.verify(password))
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
        a.set_password(new, salt)?;
        self.save_accounts(&list, now)
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
            !admin && (write || accounts_file)
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
            ensure_home(&mut self.fs, &a.name, now);
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


fn lower(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{DiskError, BLOCK};
    use std::cell::RefCell;
    use std::rc::Rc;

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
        assert_eq!(r("C:\\", "D:\\x"), Err(FsError::BadPath));
        assert_eq!(r("C:\\", "z:x"), Err(FsError::BadPath));
        assert_eq!(r("C:\\", "a:b"), Err(FsError::BadPath));
        assert_eq!(r("C:\\", "x\\a:b"), Err(FsError::BadPath));
        assert_eq!(r("D:\\", "x"), Err(FsError::BadPath));
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
        assert_eq!(names(&mut f, "C:\\"), ["Sistema", "Usuarios"]);
        for s in Std::ALL {
            assert!(f.stat(&f.std_dir(s)).unwrap().dir);
        }
        for p in ["C:\\Usuarios", "C:\\Usuarios\\Usuario\\.config", "C:\\Sistema\\temp", "C:\\Sistema\\logs", "C:\\Sistema", "c:/usuarios/usuario/documentos", "c:/usuarios/usuario/imágenes"] {
            assert!(f.stat(p).unwrap().dir, "{p}");
        }
        assert!(f.exists("C:\\"));
        let (total, free) = f.space();
        assert_eq!(total, 4 << 20);
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
        f.write("C:\\Usuarios\\Usuario\\Documentos\\grande", &vec![1u8; 2_500_000], d(1)).unwrap();
        f.mkdir("C:\\Usuarios\\Usuario\\Documentos\\c", d(1)).unwrap();
        f.write("C:\\Usuarios\\Usuario\\Documentos\\c\\g", &vec![2u8; 1_200_000], d(1)).unwrap();
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
        f.write(CONFIG_FILE, b"fondo=1\n", d(1)).unwrap();
        f.delete(CONFIG_FILE).unwrap();
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
        assert_eq!(names(&mut f, "C:\\"), ["Sistema", "Usuarios"]);
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
        assert_eq!(names(&mut f, "C:\\"), ["Sistema", "Usuarios"]);
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
        assert_eq!(names(&mut f, "C:\\"), ["Mio", "Sistema", "Usuarios"]);
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
        assert_eq!(names(&mut m, "C:\\"), ["Sistema", "Usuarios"]);
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
        assert_eq!(names(&mut f, "C:\\"), ["Sistema", "Usuarios"]);
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
        // Nadie crea carpetas sueltas dentro de `Usuarios`, ni siquiera un administrador.
        f.login("Ana", "").unwrap();
        assert_eq!(f.mkdir("C:\\Usuarios\\Falsa", d(3)), Err(FsError::Denied));
        assert!(f.read(ACCOUNTS_FILE).is_ok());
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
        assert_eq!(names(&mut f, "C:\\"), ["Documentos", "Escritorio", "Imágenes", "Sistema", "Usuarios"]);
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
}
