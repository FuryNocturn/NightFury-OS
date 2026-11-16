//! Archivos tal como los ven las apps: unidad C:, rutas estilo Windows, Papelera y copias.
//! CONTRATO: las firmas públicas no cambian. Esta versión es provisional (todo devuelve error).
//!
//! Rutas absolutas canónicas: "C:\\" es la raíz; "C:\\Documentos\\nota.txt" un archivo (sin
//! barra final salvo en la raíz). No se distinguen mayúsculas.
use crate::block::BlockDev;
use crate::fs::{Entry, FsError};
use crate::platform::DateTime;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// Ubicación especial que el Explorador muestra como la Papelera (no es una ruta de disco).
pub const TRASH: &str = "Papelera";
pub const DESKTOP_DIR: &str = "C:\\Escritorio";
pub const DOCUMENTS_DIR: &str = "C:\\Documentos";
pub const PICTURES_DIR: &str = "C:\\Imágenes";
/// Archivo de configuración del sistema (texto, líneas `clave=valor`).
pub const CONFIG_FILE: &str = "C:\\Sistema\\config.txt";

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

/// Une carpeta y nombre: join("C:\\", "a") = "C:\\a"; join("C:\\a", "b") = "C:\\a\\b".
pub fn join(_dir: &str, _name: &str) -> String {
    String::new()
}

/// Carpeta que contiene a `path`; `None` para la raíz.
pub fn parent(_path: &str) -> Option<String> {
    None
}

/// Último componente de la ruta ("" para la raíz).
pub fn file_name(_path: &str) -> &str {
    ""
}

/// ¿Sirve `name` como nombre de archivo o carpeta? (no vacío, hasta 255 bytes, sin
/// `\ / : * ? " < > |` ni caracteres de control, y distinto de "." y "..").
pub fn valid_name(_name: &str) -> bool {
    false
}

/// Convierte lo que escribe el usuario en una ruta absoluta canónica: admite rutas
/// relativas a `cwd`, ".", "..", "/" como separador y letra de unidad en mayúscula o minúscula.
pub fn resolve(_cwd: &str, _input: &str) -> Result<String, FsError> {
    Err(FsError::BadPath)
}

#[derive(Default)]
pub struct Files {
    _provisional: (),
}

impl Files {
    /// Prepara la unidad C: a partir del disco encontrado al arrancar:
    /// - con NFFS → se monta (persistente);
    /// - en blanco → queda pendiente de que el usuario acepte formatearlo; mientras, C: en memoria;
    /// - con otro contenido, o sin disco → se ignora; C: en memoria.
    ///
    /// `Files::default()` es una unidad C: en memoria. Toda unidad nueva nace con las carpetas
    /// Documentos, Imágenes, Escritorio y Sistema.
    pub fn probe(_dev: Option<Box<dyn BlockDev>>, _now: DateTime) -> Files {
        Files::default()
    }

    /// ¿Se guarda en un disco de verdad?
    pub fn persistent(&self) -> bool {
        false
    }

    /// ¿Hay un disco en blanco esperando confirmación para formatearse?
    pub fn pending_format(&self) -> bool {
        false
    }

    /// Formatea el disco pendiente y pasa a usarlo, copiando lo que hubiera en la unidad en memoria.
    pub fn format_pending(&mut self, _now: DateTime) -> Result<(), FsError> {
        Err(FsError::Invalid)
    }

    /// El usuario no quiere formatear: se descarta el disco pendiente.
    pub fn decline_format(&mut self) {}

    /// Aviso para el usuario cuando la unidad no es persistente; `None` si todo es normal.
    pub fn notice(&self) -> Option<&'static str> {
        None
    }

    /// Cambia cada vez que se modifica algo; sirve para saber cuándo refrescar una vista.
    pub fn generation(&self) -> u64 {
        0
    }

    /// Entradas de una carpeta: carpetas primero, luego por nombre sin distinguir mayúsculas.
    /// No incluye el almacén interno de la Papelera.
    pub fn list(&mut self, _dir: &str) -> Result<Vec<Entry>, FsError> {
        Err(FsError::Disk)
    }

    pub fn stat(&mut self, _path: &str) -> Result<Entry, FsError> {
        Err(FsError::Disk)
    }

    pub fn exists(&mut self, _path: &str) -> bool {
        false
    }

    pub fn read(&mut self, _path: &str) -> Result<Vec<u8>, FsError> {
        Err(FsError::Disk)
    }

    /// Crea o sustituye un archivo (atómico).
    pub fn write(&mut self, _path: &str, _data: &[u8], _now: DateTime) -> Result<(), FsError> {
        Err(FsError::Disk)
    }

    pub fn mkdir(&mut self, _path: &str, _now: DateTime) -> Result<(), FsError> {
        Err(FsError::Disk)
    }

    /// Mueve o renombra. Falla con `Exists` si el destino existe.
    pub fn rename(&mut self, _from: &str, _to: &str) -> Result<(), FsError> {
        Err(FsError::Disk)
    }

    /// Copia un archivo o una carpeta entera. Falla con `Exists` si el destino existe.
    pub fn copy(&mut self, _from: &str, _to: &str, _now: DateTime) -> Result<(), FsError> {
        Err(FsError::Disk)
    }

    /// Borrado definitivo; las carpetas, con todo su contenido.
    pub fn delete(&mut self, _path: &str) -> Result<(), FsError> {
        Err(FsError::Disk)
    }

    /// Nombre libre en `dir` parecido a `name`: "nota.txt", "nota (2).txt", "nota (3).txt"...
    pub fn free_name(&mut self, _dir: &str, _name: &str) -> String {
        String::new()
    }

    /// Envía un archivo o carpeta a la Papelera.
    pub fn trash(&mut self, _path: &str, _now: DateTime) -> Result<(), FsError> {
        Err(FsError::Disk)
    }

    /// Contenido de la Papelera, lo más reciente primero.
    pub fn trash_list(&mut self) -> Result<Vec<TrashItem>, FsError> {
        Err(FsError::Disk)
    }

    /// Devuelve un elemento a su sitio (con otro nombre libre si el original está ocupado,
    /// recreando la carpeta original si ya no existe). Devuelve la ruta restaurada.
    pub fn restore(&mut self, _id: &str) -> Result<String, FsError> {
        Err(FsError::Disk)
    }

    pub fn empty_trash(&mut self) -> Result<(), FsError> {
        Err(FsError::Disk)
    }

    /// (bytes totales, bytes libres) de la unidad C:.
    pub fn space(&mut self) -> (u64, u64) {
        (0, 0)
    }
}
