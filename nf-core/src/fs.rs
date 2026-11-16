//! NFFS: sistema de archivos propio de NightFury. CONTRATO: `FsError`, `Entry` y las firmas
//! públicas de `Nffs` no cambian. Esta versión es provisional (todo devuelve error).
//!
//! Las rutas de `Nffs` son relativas a la raíz del volumen, con `\` como separador y sin letra
//! de unidad: "" es la raíz, "Documentos\\nota.txt" un archivo. Los nombres no distinguen
//! mayúsculas pero las conservan.
use crate::block::BlockDev;
use crate::platform::DateTime;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FsError {
    NotFound,
    Exists,
    NotDir,
    IsDir,
    NotEmpty,
    NoSpace,
    /// Nombre vacío, demasiado largo o con caracteres no permitidos.
    BadName,
    /// Ruta mal formada o unidad desconocida.
    BadPath,
    /// Fallo del disco.
    Disk,
    /// El disco no tiene un NFFS válido.
    Corrupt,
    /// Operación no permitida (p. ej. mover una carpeta dentro de sí misma).
    Invalid,
}

impl FsError {
    /// Mensaje para el usuario.
    pub fn message(&self) -> &'static str {
        match self {
            FsError::NotFound => "No se encuentra el archivo o la carpeta.",
            FsError::Exists => "Ya existe un elemento con ese nombre.",
            FsError::NotDir => "No es una carpeta.",
            FsError::IsDir => "Es una carpeta, no un archivo.",
            FsError::NotEmpty => "La carpeta no está vacía.",
            FsError::NoSpace => "No queda espacio en el disco.",
            FsError::BadName => "El nombre no es válido.",
            FsError::BadPath => "La ruta no es válida.",
            FsError::Disk => "Error de disco.",
            FsError::Corrupt => "El disco está dañado o no tiene formato.",
            FsError::Invalid => "La operación no se puede hacer.",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    /// Tamaño en bytes (0 en carpetas).
    pub size: u64,
    pub modified: DateTime,
}

pub struct Nffs {
    #[allow(dead_code)]
    dev: Box<dyn BlockDev>,
}

impl Nffs {
    /// ¿Tiene `dev` un NFFS (firma y superbloque válidos)?
    pub fn is_formatted(_dev: &mut dyn BlockDev) -> bool {
        false
    }

    /// ¿Está `dev` en blanco? (primeros y últimos 256 bloques a cero). Solo un disco en blanco
    /// puede formatearse; uno con cualquier otro contenido se ignora siempre.
    pub fn is_blank(_dev: &mut dyn BlockDev) -> bool {
        false
    }

    /// Crea un NFFS vacío en `dev` y lo monta.
    pub fn format(_dev: Box<dyn BlockDev>, _now: DateTime) -> Result<Nffs, FsError> {
        Err(FsError::Disk)
    }

    /// Monta un NFFS existente.
    pub fn mount(_dev: Box<dyn BlockDev>) -> Result<Nffs, FsError> {
        Err(FsError::Corrupt)
    }

    /// Entradas de una carpeta, sin orden garantizado.
    pub fn list(&mut self, _path: &str) -> Result<Vec<Entry>, FsError> {
        Err(FsError::Disk)
    }

    pub fn stat(&mut self, _path: &str) -> Result<Entry, FsError> {
        Err(FsError::Disk)
    }

    /// Contenido completo de un archivo.
    pub fn read(&mut self, _path: &str) -> Result<Vec<u8>, FsError> {
        Err(FsError::Disk)
    }

    /// Crea el archivo o sustituye su contenido de forma atómica: tras un corte queda el
    /// contenido viejo o el nuevo, nunca una mezcla.
    pub fn write(&mut self, _path: &str, _data: &[u8], _now: DateTime) -> Result<(), FsError> {
        Err(FsError::Disk)
    }

    pub fn mkdir(&mut self, _path: &str, _now: DateTime) -> Result<(), FsError> {
        Err(FsError::Disk)
    }

    /// Borra un archivo o una carpeta vacía.
    pub fn remove(&mut self, _path: &str) -> Result<(), FsError> {
        Err(FsError::Disk)
    }

    /// Mueve o renombra dentro del volumen. Falla con `Exists` si el destino existe.
    pub fn rename(&mut self, _from: &str, _to: &str) -> Result<(), FsError> {
        Err(FsError::Disk)
    }

    /// (bytes totales, bytes libres).
    pub fn space(&mut self) -> (u64, u64) {
        (0, 0)
    }

    /// Comprueba la coherencia del volumen entero.
    pub fn check(&mut self) -> Result<(), FsError> {
        Err(FsError::Disk)
    }

    /// Devuelve el dispositivo (para inspeccionarlo en pruebas y herramientas).
    pub fn into_dev(self) -> Box<dyn BlockDev> {
        self.dev
    }
}
