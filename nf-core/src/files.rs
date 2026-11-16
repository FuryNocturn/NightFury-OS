//! Archivos tal como los ven las apps: unidad C:, rutas estilo Windows, Papelera y copias.
//! CONTRATO: las firmas públicas no cambian.
//!
//! Rutas absolutas canónicas: "C:\\" es la raíz; "C:\\Documentos\\nota.txt" un archivo (sin
//! barra final salvo en la raíz). No se distinguen mayúsculas.
//!
//! La Papelera vive en la carpeta oculta `C:\$Papelera`: cada elemento borrado se mueve allí con
//! un número como nombre y un índice de texto (`indice`) guarda su ruta original y su fecha.
//! Al enviar se escribe primero el índice y luego se mueve el elemento; al restaurar o vaciar,
//! al revés: un corte solo puede dejar líneas de índice sin elemento, que se ignoran.
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

/// Almacén interno de la Papelera (relativo a la raíz) y su índice.
const STORE: &str = "$Papelera";
const INDEX: &str = "$Papelera\\indice";
const SYSTEM: &str = "Sistema";
const FOLDERS: [&str; 4] = ["Documentos", "Imágenes", "Escritorio", SYSTEM];
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
    if same(first(rel(&c)), STORE) {
        return Err(FsError::BadPath);
    }
    Ok(c)
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

/// La raíz y `C:\Sistema` no se pueden borrar, mover ni renombrar.
fn protected(rel: &str) -> bool {
    rel.is_empty() || same(rel, SYSTEM)
}

fn init_folders(fs: &mut Nffs, now: DateTime) {
    for f in FOLDERS.iter().chain([&STORE]) {
        if fs.stat(f).is_err() {
            let _ = fs.mkdir(f, now);
        }
    }
}

fn new_memory(now: DateTime) -> Nffs {
    let mut fs = Nffs::format(Box::new(RamDisk::new(MEMORY_BLOCKS)), now).expect("disco en memoria");
    init_folders(&mut fs, now);
    fs
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
    /// `Files::default()` es una unidad C: en memoria. Toda unidad nueva nace con las carpetas
    /// Documentos, Imágenes, Escritorio y Sistema.
    pub fn probe(dev: Option<Box<dyn BlockDev>>, now: DateTime) -> Files {
        let Some(mut dev) = dev else {
            return Files::memory(NOTICE_NO_DISK, None, now);
        };
        if Nffs::is_formatted(&mut *dev) {
            // Con NFFS dañado no se formatea nunca: se deja el disco como está.
            return match Nffs::mount(dev) {
                Ok(mut fs) => {
                    init_folders(&mut fs, now);
                    Files { fs, persistent: true, pending: None, notice: None, generation: 0, clip: None, undo: None, user: None }
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
    pub fn format_pending(&mut self, now: DateTime) -> Result<(), FsError> {
        let dev = self.pending.take().ok_or(FsError::Invalid)?;
        let mut fs = Nffs::format(dev, now)?;
        init_folders(&mut fs, now);
        copy_all(&mut self.fs, &mut fs)?;
        self.fs = fs;
        self.persistent = true;
        self.notice = None;
        self.generation += 1;
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
    /// No incluye el almacén interno de la Papelera.
    pub fn list(&mut self, dir: &str) -> Result<Vec<Entry>, FsError> {
        let c = canon(dir)?;
        let mut v = self.fs.list(rel(&c))?;
        if rel(&c).is_empty() {
            v.retain(|e| !same(&e.name, STORE));
        }
        v.sort_by_cached_key(|e| (!e.dir, e.name.chars().flat_map(char::to_lowercase).collect::<String>()));
        Ok(v)
    }

    pub fn stat(&mut self, path: &str) -> Result<Entry, FsError> {
        let c = canon(path)?;
        self.fs.stat(rel(&c))
    }

    pub fn exists(&mut self, path: &str) -> bool {
        self.stat(path).is_ok()
    }

    pub fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        let c = canon(path)?;
        self.fs.read(rel(&c))
    }

    /// Crea o sustituye un archivo (atómico).
    pub fn write(&mut self, path: &str, data: &[u8], now: DateTime) -> Result<(), FsError> {
        let c = canon(path)?;
        if rel(&c).is_empty() {
            return Err(FsError::IsDir);
        }
        self.generation += 1;
        self.fs.write(rel(&c), data, now)
    }

    pub fn mkdir(&mut self, path: &str, now: DateTime) -> Result<(), FsError> {
        let c = canon(path)?;
        if rel(&c).is_empty() {
            return Err(FsError::Exists);
        }
        self.generation += 1;
        self.fs.mkdir(rel(&c), now)
    }

    /// Mueve o renombra. Falla con `Exists` si el destino existe.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        let (f, t) = (canon(from)?, canon(to)?);
        if protected(rel(&f)) {
            return Err(FsError::Invalid);
        }
        if rel(&t).is_empty() {
            return Err(FsError::Exists);
        }
        self.generation += 1;
        self.fs.rename(rel(&f), rel(&t))
    }

    /// Copia un archivo o una carpeta entera. Falla con `Exists` si el destino existe.
    pub fn copy(&mut self, from: &str, to: &str, now: DateTime) -> Result<(), FsError> {
        let (f, t) = (canon(from)?, canon(to)?);
        let (f, t) = (rel(&f), rel(&t));
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

    // ---- Papelera ----

    fn read_index(&mut self) -> Vec<(String, DateTime, String)> {
        let text = self.fs.read(INDEX).ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
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

    fn write_index(&mut self, idx: &[(String, DateTime, String)], now: DateTime) -> Result<(), FsError> {
        let mut s = String::new();
        for (id, d, orig) in idx {
            s.push_str(&format!(
                "{id}|{}-{}-{}-{}-{}-{}|{orig}\n",
                d.year, d.month, d.day, d.hour, d.minute, d.second
            ));
        }
        if self.fs.stat(STORE).is_err() {
            self.fs.mkdir(STORE, now)?;
        }
        self.fs.write(INDEX, s.as_bytes(), now)
    }

    /// Envía un archivo o carpeta a la Papelera.
    pub fn trash(&mut self, path: &str, now: DateTime) -> Result<(), FsError> {
        let c = canon(path)?;
        let r = rel(&c);
        if protected(r) {
            return Err(FsError::Invalid);
        }
        self.fs.stat(r)?;
        self.generation += 1;
        let mut idx = self.read_index();
        let mut next = idx.iter().filter_map(|(id, _, _)| id.parse::<u64>().ok()).max().unwrap_or(0);
        for e in self.fs.list(STORE).unwrap_or_default() {
            next = next.max(e.name.parse::<u64>().unwrap_or(0));
        }
        let id = format!("{:08}", next + 1);
        idx.push((id.clone(), now, c.clone()));
        self.write_index(&idx, now)?;
        if let Err(e) = self.fs.rename(r, &join_rel(STORE, &id)) {
            idx.pop();
            let _ = self.write_index(&idx, now);
            return Err(e);
        }
        Ok(())
    }

    /// Contenido de la Papelera, lo más reciente primero.
    pub fn trash_list(&mut self) -> Result<Vec<TrashItem>, FsError> {
        let mut out = Vec::new();
        for (id, deleted, original) in self.read_index() {
            if let Ok(e) = self.fs.stat(&join_rel(STORE, &id)) {
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
        let mut idx = self.read_index();
        let pos = idx.iter().position(|(i, _, _)| i == id).ok_or(FsError::NotFound)?;
        let item = join_rel(STORE, id);
        self.fs.stat(&item)?;
        let (deleted, original) = (idx[pos].1, idx[pos].2.clone());
        self.generation += 1;
        let r = original.get(3..).unwrap_or("");
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
        self.write_index(&idx, deleted)?;
        Ok(format!("C:\\{dest}"))
    }

    pub fn empty_trash(&mut self) -> Result<(), FsError> {
        self.generation += 1;
        for e in self.fs.list(STORE)? {
            if e.name != "indice" {
                self.rm_tree(&join_rel(STORE, &e.name))?;
            }
        }
        self.write_index(&[], DateTime::default())
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

    /// `false` si el usuario ocultó las extensiones (`extensiones=no` en `EXPLORER_PREFS`).
    pub fn show_ext(&mut self) -> bool {
        match self.read(EXPLORER_PREFS) {
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
// CONTRATO. Semántica final (la estructura del disco la completa `files.rs`):
// - Unidad en memoria (sin disco): hay una sesión abierta de `DEFAULT_USER`, administrador, y
//   no hay archivo de cuentas. `needs_setup()` es `false`.
// - Disco montado: sin sesión hasta `login`. Si no hay ninguna cuenta, `needs_setup()` es `true`
//   y `create_account` crea la primera, que siempre es administradora.
// - Las rutas del usuario salen SIEMPRE de `home`, `std_dir`, `config_file` y `explorer_prefs`.
impl Files {
    /// Cuentas guardadas (vacío si no hay archivo).
    pub fn accounts(&mut self) -> Vec<Account> {
        match self.read(ACCOUNTS_FILE) {
            Ok(d) => accounts::parse(&String::from_utf8_lossy(&d)),
            Err(_) => Vec::new(),
        }
    }

    fn save_accounts(&mut self, list: &[Account], now: DateTime) -> Result<(), AccountError> {
        if !self.exists(SYS_CONFIG_DIR) {
            self.mkdir(SYS_CONFIG_DIR, now)?;
        }
        Ok(self.write(ACCOUNTS_FILE, accounts::serialize(list).as_bytes(), now)?)
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

    /// Crea una cuenta. La primera es siempre administradora y no exige sesión; después solo
    /// puede crear cuentas un administrador. `salt`: `accounts::salt_from` con `Platform::entropy`.
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
        let mut a = Account::new(name, admin || list.is_empty());
        a.set_password(password, salt)?;
        list.push(a);
        self.save_accounts(&list, now)
    }

    /// Elimina una cuenta (solo un administrador; no la de la sesión abierta ni la última
    /// administradora). `keep_files == false` borra su carpeta para siempre.
    pub fn delete_account(&mut self, name: &str, keep_files: bool, now: DateTime) -> Result<(), AccountError> {
        let _ = keep_files; // la carpeta del usuario llega con la estructura nueva del disco
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
        list.remove(i);
        self.save_accounts(&list, now)
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

    /// Carpeta del usuario de la sesión abierta: `C:\Usuarios\<nombre>`.
    /// shortcut: hasta que llegue la estructura nueva devuelve la raíz (estructura antigua).
    pub fn home(&self) -> String {
        String::from("C:\\")
    }

    /// Carpeta estándar del usuario: `C:\Usuarios\<nombre>\Documentos`, etc.
    pub fn std_dir(&self, d: Std) -> String {
        join(&self.home(), d.name())
    }

    /// Configuración del usuario (`fondo=`, `enfasis=`, `iconos=`, `orden=`).
    /// shortcut: ruta antigua hasta que llegue la estructura nueva (`<home>\.config\config.txt`).
    pub fn config_file(&self) -> String {
        String::from(CONFIG_FILE)
    }

    /// Preferencias del Explorador del usuario (`<home>\.config\explorador.txt`).
    pub fn explorer_prefs(&self) -> String {
        String::from(EXPLORER_PREFS)
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
        let doc = |x: &str| format!("C:\\Documentos\\{x}");
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
        let out = f.paste_all("C:\\Escritorio", n).unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(f.undo_label(), Some("Copiar"));
        f.undo(n).unwrap();
        assert!(out.iter().all(|p| !f.exists(p)));
        // Mover varios y deshacer.
        let out = f.transfer(&all, "C:\\Escritorio", true, n).unwrap();
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
        f.write(EXPLORER_PREFS, b"vista=lista\nextensiones=no\n", n).unwrap();
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
        f.write("C:\\Documentos\\a.txt", b"hola", n).unwrap();
        assert_eq!(f.undo_label(), None);
        // Copiar y pegar en la misma carpeta: nombre libre; deshacer lo manda a la Papelera.
        f.set_clip("C:\\Documentos\\a.txt", false);
        let p = f.paste("C:\\Documentos", n).unwrap();
        assert_eq!(p, "C:\\Documentos\\a (2).txt");
        assert_eq!(f.undo_label(), Some("Copiar"));
        f.undo(n).unwrap();
        assert!(!f.exists(&p) && f.undo_label().is_none());
        // Cortar y pegar en otra carpeta; deshacer lo devuelve.
        f.set_clip("C:\\Documentos\\a.txt", true);
        let p = f.paste("C:\\Escritorio", n).unwrap();
        assert!(f.exists(&p) && !f.exists("C:\\Documentos\\a.txt") && f.clip().is_none());
        assert_eq!(f.undo_label(), Some("Mover"));
        f.undo(n).unwrap();
        assert!(f.exists("C:\\Documentos\\a.txt"));
        // Cortar y pegar en la misma carpeta no hace nada.
        f.set_clip("C:\\Documentos\\a.txt", true);
        assert_eq!(f.paste("C:\\documentos", n).unwrap(), "C:\\Documentos\\a.txt");
        assert!(f.undo_label().is_none());
        // Cambiar nombre, eliminar y crear.
        f.user_rename("C:\\Documentos\\a.txt", "C:\\Documentos\\b.txt").unwrap();
        f.undo(n).unwrap();
        assert!(f.exists("C:\\Documentos\\a.txt"));
        f.user_trash("C:\\Documentos\\a.txt", n).unwrap();
        assert_eq!(f.undo_label(), Some("Eliminar"));
        f.undo(n).unwrap();
        assert_eq!(f.read("C:\\Documentos\\a.txt").unwrap(), b"hola");
        f.user_create("C:\\Documentos\\n.txt", false, n).unwrap();
        assert_eq!(f.user_create("C:\\Documentos\\n.txt", false, n), Err(FsError::Exists));
        f.user_create("C:\\Documentos\\c", true, n).unwrap();
        f.undo(n).unwrap();
        assert!(!f.exists("C:\\Documentos\\c") && f.exists("C:\\Documentos\\n.txt"));
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
        assert_eq!(file_name("C:\\Imágenes\\ñ"), "ñ");
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
        assert_eq!(names(&mut f, "C:\\"), ["Documentos", "Escritorio", "Imágenes", "Sistema"]);
        for p in [DOCUMENTS_DIR, DESKTOP_DIR, PICTURES_DIR, "C:\\Sistema", "c:/documentos", "C:\\imágenes"] {
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
            assert_eq!(f.rename("C:\\Documentos", p), Err(FsError::BadPath));
            assert_eq!(f.copy("C:\\Documentos", p, d(1)), Err(FsError::BadPath));
            assert_eq!(f.copy(p, "C:\\z", d(1)), Err(FsError::BadPath));
            assert!(!f.exists(p));
        }
        assert_eq!(f.write("C:\\Documentos\\..\\$Papelera\\x", b"1", d(1)), Err(FsError::BadPath));
    }

    #[test]
    fn list_order_and_basic_io() {
        let mut f = Files::default();
        f.write("C:\\Documentos\\b.txt", b"b", d(1)).unwrap();
        f.write("C:\\Documentos\\A.txt", b"aaa", d(2)).unwrap();
        f.write("C:\\Documentos\\ñu.txt", b"n", d(3)).unwrap();
        f.write("C:\\Documentos\\zeta", b"z", d(3)).unwrap();
        f.mkdir("C:\\Documentos\\Zona", d(4)).unwrap();
        f.mkdir("C:\\Documentos\\carpeta", d(4)).unwrap();
        assert_eq!(names(&mut f, "C:\\Documentos"), ["carpeta", "Zona", "A.txt", "b.txt", "zeta", "ñu.txt"]);
        let e = f.stat("c:/documentos/a.TXT").unwrap();
        assert_eq!((e.name.as_str(), e.size, e.modified, e.dir), ("A.txt", 3, d(2), false));
        assert_eq!(f.read("C:\\Documentos\\A.txt").unwrap(), b"aaa");
        assert_eq!(f.write("C:\\", b"x", d(1)), Err(FsError::IsDir));
        assert_eq!(f.mkdir("C:\\", d(1)), Err(FsError::Exists));
        assert_eq!(f.mkdir("C:\\Documentos", d(1)), Err(FsError::Exists));
        assert_eq!(f.read("C:\\nada"), Err(FsError::NotFound));
        assert_eq!(f.read("D:\\x"), Err(FsError::BadPath));
        assert_eq!(f.write("C:\\a*", b"x", d(1)), Err(FsError::BadName));
    }

    #[test]
    fn copy_and_delete_are_recursive() {
        let mut f = Files::default();
        f.mkdir("C:\\Documentos\\p", d(1)).unwrap();
        f.mkdir("C:\\Documentos\\p\\q", d(1)).unwrap();
        f.mkdir("C:\\Documentos\\p\\vacia", d(1)).unwrap();
        f.write("C:\\Documentos\\p\\a.txt", b"A", d(1)).unwrap();
        f.write("C:\\Documentos\\p\\q\\b.bin", &vec![7u8; 10_000], d(1)).unwrap();
        f.copy("C:\\Documentos\\p", "C:\\Escritorio\\copia", d(5)).unwrap();
        assert_eq!(f.read("C:\\Escritorio\\copia\\q\\b.bin").unwrap(), vec![7u8; 10_000]);
        assert_eq!(f.read("C:\\Escritorio\\copia\\a.txt").unwrap(), b"A");
        assert!(f.stat("C:\\Escritorio\\copia\\vacia").unwrap().dir);
        assert!(f.exists("C:\\Documentos\\p\\a.txt"));
        // archivo suelto
        f.copy("C:\\Documentos\\p\\a.txt", "C:\\Documentos\\a2.txt", d(5)).unwrap();
        assert_eq!(f.read("C:\\Documentos\\a2.txt").unwrap(), b"A");
        // errores
        assert_eq!(f.copy("C:\\Documentos\\p", "C:\\Escritorio\\copia", d(5)), Err(FsError::Exists));
        assert_eq!(f.copy("C:\\Documentos\\p", "C:\\Documentos\\p\\q\\dentro", d(5)), Err(FsError::Invalid));
        assert_eq!(f.copy("C:\\Documentos\\p", "C:\\DOCUMENTOS\\P\\dentro", d(5)), Err(FsError::Invalid));
        assert!(!f.exists("C:\\Documentos\\p\\q\\dentro"));
        assert_eq!(f.copy("C:\\", "C:\\Escritorio\\todo", d(5)), Err(FsError::Invalid));
        assert_eq!(f.copy("C:\\nada", "C:\\x", d(5)), Err(FsError::NotFound));
        assert_eq!(f.copy("C:\\Documentos\\a2.txt", "C:\\nada\\x", d(5)), Err(FsError::NotFound));
        assert!(!f.exists("C:\\nada"));
        // un nombre parecido no cuenta como "dentro"
        f.copy("C:\\Documentos\\p", "C:\\Documentos\\p2", d(5)).unwrap();
        // borrado
        let (_, free) = f.space();
        f.delete("C:\\Escritorio\\copia").unwrap();
        assert!(!f.exists("C:\\Escritorio\\copia"));
        assert!(f.space().1 > free);
        assert_eq!(f.delete("C:\\Escritorio\\copia"), Err(FsError::NotFound));
        f.delete("C:\\Documentos\\a2.txt").unwrap();
        assert!(f.exists("C:\\Documentos\\p2\\q\\b.bin"));
    }

    #[test]
    fn copy_that_runs_out_of_space_leaves_nothing() {
        let mut f = Files::default();
        f.write("C:\\Documentos\\grande", &vec![1u8; 2_500_000], d(1)).unwrap();
        f.mkdir("C:\\Documentos\\c", d(1)).unwrap();
        f.write("C:\\Documentos\\c\\g", &vec![2u8; 1_200_000], d(1)).unwrap();
        let (_, free) = f.space();
        assert_eq!(f.copy("C:\\Documentos\\c", "C:\\Escritorio\\c", d(2)), Err(FsError::NoSpace));
        assert!(!f.exists("C:\\Escritorio\\c"));
        assert_eq!(f.space().1, free);
    }

    #[test]
    fn free_name_cases() {
        let mut f = Files::default();
        let dir = "C:\\Documentos";
        assert_eq!(f.free_name(dir, "nota.txt"), "nota.txt");
        f.write("C:\\Documentos\\nota.txt", b"1", d(1)).unwrap();
        assert_eq!(f.free_name(dir, "nota.txt"), "nota (2).txt");
        assert_eq!(f.free_name(dir, "NOTA.TXT"), "NOTA (2).TXT");
        f.write("C:\\Documentos\\nota (2).txt", b"1", d(1)).unwrap();
        f.write("C:\\Documentos\\nota (3).txt", b"1", d(1)).unwrap();
        assert_eq!(f.free_name(dir, "nota.txt"), "nota (4).txt");
        f.mkdir("C:\\Documentos\\carpeta", d(1)).unwrap();
        assert_eq!(f.free_name(dir, "carpeta"), "carpeta (2)");
        f.mkdir("C:\\Documentos\\carpeta (2)", d(1)).unwrap();
        assert_eq!(f.free_name(dir, "carpeta"), "carpeta (3)");
        f.write("C:\\Documentos\\.oculto", b"1", d(1)).unwrap();
        assert_eq!(f.free_name(dir, ".oculto"), ".oculto (2)");
        f.write("C:\\Documentos\\a.tar.gz", b"1", d(1)).unwrap();
        assert_eq!(f.free_name(dir, "a.tar.gz"), "a.tar (2).gz");
        assert_eq!(f.free_name("C:\\Documentos\\nada", "x"), "x");
        assert_eq!(f.free_name("D:\\", "x"), "x");
    }

    #[test]
    fn rename_and_protected_folders() {
        let mut f = Files::default();
        f.write("C:\\Documentos\\a.txt", b"1", d(1)).unwrap();
        f.rename("C:\\Documentos\\a.txt", "C:\\Escritorio\\b.txt").unwrap();
        assert_eq!(f.read("C:\\Escritorio\\b.txt").unwrap(), b"1");
        assert_eq!(f.rename("C:\\Documentos\\a.txt", "C:\\x"), Err(FsError::NotFound));
        f.write("C:\\Documentos\\c.txt", b"2", d(1)).unwrap();
        assert_eq!(f.rename("C:\\Documentos\\c.txt", "C:\\Escritorio\\B.TXT"), Err(FsError::Exists));
        assert_eq!(f.rename("C:\\Documentos", "C:\\Documentos\\x"), Err(FsError::Invalid));
        f.rename("c:/escritorio/b.txt", "C:\\Escritorio\\B.txt").unwrap();
        // raíz y Sistema
        for p in ["C:\\", "C:\\Sistema", "c:\\sistema", "C:\\Documentos\\..\\Sistema"] {
            assert_eq!(f.delete(p), Err(FsError::Invalid), "{p}");
            assert_eq!(f.rename(p, "C:\\otro"), Err(FsError::Invalid), "{p}");
            assert_eq!(f.trash(p, d(1)), Err(FsError::Invalid), "{p}");
        }
        assert_eq!(f.rename("C:\\Documentos", "C:\\"), Err(FsError::Exists));
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
        let _ = f.stat("C:\\Documentos");
        let _ = f.read("C:\\nada");
        let _ = f.trash_list();
        assert_eq!(f.generation(), g0);
        let mut last = g0;
        let mut step = |f: &mut Files| {
            assert!(f.generation() > last);
            last = f.generation();
        };
        f.write("C:\\Documentos\\a", b"1", d(1)).unwrap();
        step(&mut f);
        f.mkdir("C:\\Documentos\\m", d(1)).unwrap();
        step(&mut f);
        f.rename("C:\\Documentos\\a", "C:\\Documentos\\b").unwrap();
        step(&mut f);
        f.copy("C:\\Documentos\\b", "C:\\Documentos\\c", d(1)).unwrap();
        step(&mut f);
        f.trash("C:\\Documentos\\c", d(1)).unwrap();
        step(&mut f);
        let id = f.trash_list().unwrap()[0].id.clone();
        f.restore(&id).unwrap();
        step(&mut f);
        f.trash("C:\\Documentos\\c", d(1)).unwrap();
        step(&mut f);
        f.empty_trash().unwrap();
        step(&mut f);
        f.delete("C:\\Documentos\\b").unwrap();
        step(&mut f);
    }

    #[test]
    fn trash_full_cycle() {
        let mut f = Files::default();
        assert!(f.trash_list().unwrap().is_empty());
        f.write("C:\\Documentos\\nota.txt", b"hola", d(1)).unwrap();
        f.mkdir("C:\\Documentos\\carp", d(1)).unwrap();
        f.write("C:\\Documentos\\carp\\x.bin", &vec![9u8; 5000], d(1)).unwrap();
        f.trash("C:\\Documentos\\nota.txt", d(10)).unwrap();
        f.trash("C:\\Documentos\\carp", d(11)).unwrap();
        // desaparecen del sitio y de la raíz no aparece el almacén
        assert!(!f.exists("C:\\Documentos\\nota.txt"));
        assert!(!f.exists("C:\\Documentos\\carp"));
        assert_eq!(names(&mut f, "C:\\"), ["Documentos", "Escritorio", "Imágenes", "Sistema"]);
        let items = f.trash_list().unwrap();
        assert_eq!(items.len(), 2);
        // lo más reciente primero
        assert_eq!(items[0].name, "carp");
        assert_eq!(items[0].original, "C:\\Documentos\\carp");
        assert!(items[0].dir);
        assert_eq!(items[0].deleted, d(11));
        assert_eq!(items[1].name, "nota.txt");
        assert_eq!((items[1].dir, items[1].size, items[1].deleted), (false, 4, d(10)));
        assert_ne!(items[0].id, items[1].id);
        // restaurar
        let p = f.restore(&items[1].id).unwrap();
        assert_eq!(p, "C:\\Documentos\\nota.txt");
        assert_eq!(f.read(&p).unwrap(), b"hola");
        assert_eq!(f.trash_list().unwrap().len(), 1);
        assert_eq!(f.restore(&items[1].id), Err(FsError::NotFound));
        assert_eq!(f.restore("inventado"), Err(FsError::NotFound));
        // conflicto de nombre
        f.trash(&p, d(12)).unwrap();
        f.write(&p, b"nueva", d(13)).unwrap();
        let id = f.trash_list().unwrap()[0].id.clone();
        assert_eq!(f.restore(&id).unwrap(), "C:\\Documentos\\nota (2).txt");
        assert_eq!(f.read("C:\\Documentos\\nota (2).txt").unwrap(), b"hola");
        assert_eq!(f.read(&p).unwrap(), b"nueva");
        // la carpeta original ya no existe: se recrea
        f.mkdir("C:\\Escritorio\\a", d(1)).unwrap();
        f.mkdir("C:\\Escritorio\\a\\b", d(1)).unwrap();
        f.write("C:\\Escritorio\\a\\b\\f.txt", b"f", d(1)).unwrap();
        f.trash("C:\\Escritorio\\a\\b\\f.txt", d(14)).unwrap();
        f.delete("C:\\Escritorio\\a").unwrap();
        let id = f.trash_list().unwrap()[0].id.clone();
        assert_eq!(f.restore(&id).unwrap(), "C:\\Escritorio\\a\\b\\f.txt");
        assert_eq!(f.read("C:\\Escritorio\\a\\b\\f.txt").unwrap(), b"f");
        // la carpeta con contenido se restaura entera
        let id = f.trash_list().unwrap()[0].id.clone();
        assert_eq!(f.trash_list().unwrap()[0].name, "carp");
        assert_eq!(f.restore(&id).unwrap(), "C:\\Documentos\\carp");
        assert_eq!(f.read("C:\\Documentos\\carp\\x.bin").unwrap(), vec![9u8; 5000]);
        assert!(f.trash_list().unwrap().is_empty());
        // vaciar
        f.trash("C:\\Documentos\\carp", d(15)).unwrap();
        f.trash("C:\\Documentos\\nota (2).txt", d(15)).unwrap();
        assert_eq!(f.trash_list().unwrap().len(), 2);
        let (_, free) = f.space();
        f.empty_trash().unwrap();
        assert!(f.trash_list().unwrap().is_empty());
        assert!(f.space().1 > free);
        f.empty_trash().unwrap();
        // se sigue pudiendo usar
        f.trash("C:\\Documentos\\nota.txt", d(16)).unwrap();
        assert_eq!(f.trash_list().unwrap().len(), 1);
        assert_eq!(f.trash("C:\\Documentos\\nota.txt", d(16)), Err(FsError::NotFound));
        assert_eq!(f.trash("C:\\Documentos\\ya-no-esta", d(16)), Err(FsError::NotFound));
    }

    #[test]
    fn trash_survives_remount_and_cuts_between_steps_are_harmless() {
        let disk = Disk::new(1024);
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(1));
        f.format_pending(d(1)).unwrap();
        f.write("C:\\Documentos\\a.txt", b"A", d(2)).unwrap();
        f.trash("C:\\Documentos\\a.txt", d(3)).unwrap();
        drop(f);
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(4));
        assert!(f.persistent());
        let items = f.trash_list().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].deleted, d(3));
        // una línea de índice sin elemento (corte entre los dos pasos) se ignora
        f.fs.write(INDEX, b"00000007|2026-11-3-1-2-3|C:\\Documentos\\fantasma\n", d(5)).unwrap();
        assert!(f.trash_list().unwrap().is_empty());
        f.write("C:\\Documentos\\b.txt", b"B", d(5)).unwrap();
        f.trash("C:\\Documentos\\b.txt", d(6)).unwrap();
        let items = f.trash_list().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(f.restore(&items[0].id).unwrap(), "C:\\Documentos\\b.txt");
    }

    #[test]
    fn probe_without_disk() {
        let mut f = Files::probe(None, d(1));
        assert!(!f.persistent() && !f.pending_format());
        assert!(f.notice().is_some());
        assert_eq!(names(&mut f, "C:\\"), ["Documentos", "Escritorio", "Imágenes", "Sistema"]);
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
        assert_eq!(names(&mut f, "C:\\"), ["Documentos", "Escritorio", "Imágenes", "Mio", "Sistema"]);
        f.write("C:\\Documentos\\n.txt", b"dato", d(3)).unwrap();
        drop(f);
        let mut f = Files::probe(Some(Box::new(disk)), d(4));
        assert_eq!(f.read("C:\\Documentos\\n.txt").unwrap(), b"dato");
        assert_eq!(f.stat("C:\\Documentos\\n.txt").unwrap().modified, d(3));
    }

    #[test]
    fn probe_with_blank_disk_is_pending_and_decline_works() {
        let disk = Disk::new(1024);
        let mut f = Files::probe(Some(Box::new(disk.clone())), d(1));
        assert!(!f.persistent());
        assert!(f.pending_format());
        let notice_new = f.notice().unwrap();
        f.write("C:\\Documentos\\a.txt", b"mem", d(1)).unwrap();
        f.decline_format();
        assert!(!f.pending_format());
        assert!(!f.persistent());
        assert!(f.notice().is_some());
        assert_eq!(f.format_pending(d(2)), Err(FsError::Invalid));
        assert_eq!(f.read("C:\\Documentos\\a.txt").unwrap(), b"mem");
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
        f.write("C:\\Documentos\\a.txt", b"mem", d(1)).unwrap();
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
        f.write("C:\\Documentos\\a", b"1", d(2)).unwrap();
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
        f.write("C:\\Documentos\\a.txt", b"uno", d(2)).unwrap();
        f.mkdir("C:\\Documentos\\sub", d(3)).unwrap();
        f.write("C:\\Documentos\\sub\\b.bin", &vec![5u8; 70_000], d(4)).unwrap();
        f.write("C:\\Escritorio\\ñ.txt", b"n", d(5)).unwrap();
        f.write("C:\\Sistema\\config.txt", b"fondo=2\n", d(6)).unwrap();
        f.write("C:\\Documentos\\borrado.txt", b"papelera", d(7)).unwrap();
        f.trash("C:\\Documentos\\borrado.txt", d(8)).unwrap();
        let g = f.generation();
        f.format_pending(d(9)).unwrap();
        assert!(f.generation() > g);
        assert!(f.persistent());
        assert!(!f.pending_format());
        assert_eq!(f.notice(), None);
        assert_eq!(f.read("C:\\Documentos\\a.txt").unwrap(), b"uno");
        assert_eq!(f.read("C:\\Documentos\\sub\\b.bin").unwrap(), vec![5u8; 70_000]);
        assert_eq!(f.read("C:\\Escritorio\\ñ.txt").unwrap(), b"n");
        assert_eq!(f.read(CONFIG_FILE).unwrap(), b"fondo=2\n");
        assert_eq!(f.stat("C:\\Documentos\\a.txt").unwrap().modified, d(2));
        assert_eq!(f.stat("C:\\Documentos\\sub").unwrap().modified, d(3));
        let items = f.trash_list().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].original, "C:\\Documentos\\borrado.txt");
        assert_eq!(items[0].deleted, d(8));
        assert_eq!(f.restore(&items[0].id).unwrap(), "C:\\Documentos\\borrado.txt");
        // y está de verdad en el disco
        drop(f);
        let mut f = Files::probe(Some(Box::new(disk)), d(10));
        assert!(f.persistent());
        assert_eq!(f.read("C:\\Documentos\\sub\\b.bin").unwrap().len(), 70_000);
        assert_eq!(f.read("C:\\Documentos\\borrado.txt").unwrap(), b"papelera");
        assert_eq!(f.read(CONFIG_FILE).unwrap(), b"fondo=2\n");
    }

    #[test]
    fn format_pending_on_tiny_disk_fails_and_keeps_memory_drive() {
        let mut f = Files::probe(Some(Box::new(Disk::new(100))), d(1));
        assert!(f.pending_format());
        f.write("C:\\Documentos\\gran.bin", &vec![1u8; 2_000_000], d(2)).unwrap();
        assert_eq!(f.format_pending(d(3)), Err(FsError::NoSpace));
        assert!(!f.persistent());
        assert_eq!(f.read("C:\\Documentos\\gran.bin").unwrap().len(), 2_000_000);
        let mut f = Files::probe(Some(Box::new(Disk::new(100))), d(1));
        f.write("C:\\a.txt", b"1", d(1)).unwrap();
        f.format_pending(d(2)).unwrap();
        assert_eq!(f.read("C:\\a.txt").unwrap(), b"1");
    }
}
