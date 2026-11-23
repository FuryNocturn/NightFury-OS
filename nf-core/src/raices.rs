// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! v0.12.2: almacén de raíces de confianza del sistema (HTTPS).
//!
//! Son las de Mozilla (dentro del sistema, no se pueden cambiar) más las que añade un administrador
//! desde Configuración → Red → Certificados, que valen para todas las cuentas. Cada raíz propia es un
//! `<SHA-256 en hex>.der` en `C:\Sistema\config\raices`.
//!
//! Para la conexión HTTPS: `Files::root_store()` da el `Arc<RootStore>` ya montado. Se guarda en
//! memoria y solo se rehace al añadir o quitar una raíz, no en cada conexión.
use crate::crypto::{sha256, to_hex};
use crate::fs::{FsError, Nffs};
use crate::platform::DateTime;
use crate::x509::{pem_or_der, Cert, CertInfo, RootStore};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

pub const DIR: &str = "C:\\Sistema\\config\\raices";
/// Paquete de Mozilla: `[u16 BE longitud][DER]…`.
const MOZILLA: &[u8] = include_bytes!("../data/raices/raices.bin");
/// Tope de raíces propias (la carpeta no crece sin límite).
const MAX_CUSTOM: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootError {
    /// No es un certificado (ni PEM ni DER).
    NoCert,
    /// El PEM trae varios: se añaden de uno en uno.
    Varios,
    /// No es de una autoridad (cA) ni está autofirmado.
    NoCa,
}

impl RootError {
    pub fn mensaje(&self) -> &'static str {
        match self {
            RootError::NoCert => "El archivo no es un certificado.",
            RootError::Varios => "El archivo trae varios certificados. Añádelos de uno en uno.",
            RootError::NoCa => "El certificado no es de una autoridad de certificación.",
        }
    }
}

/// Comprueba el archivo elegido (PEM o DER). Devuelve el DER y sus datos para enseñarlos.
pub fn check(bytes: &[u8]) -> Result<(Vec<u8>, CertInfo), RootError> {
    if bytes.len() > 64 * 1024 {
        return Err(RootError::NoCert);
    }
    let mut v = pem_or_der(bytes);
    if v.len() > 1 {
        return Err(RootError::Varios);
    }
    let der = v.pop().ok_or(RootError::NoCert)?;
    let c = Cert::parse(&der).map_err(|_| RootError::NoCert)?;
    if !c.is_ca() && !c.is_self_issued() {
        return Err(RootError::NoCa);
    }
    Ok((der, c.info()))
}

/// Cuántas raíces trae el paquete de Mozilla (sin leerlas una a una).
pub fn mozilla_count() -> usize {
    let (mut b, mut n) = (MOZILLA, 0);
    while let [h, l, rest @ ..] = b {
        let len = u16::from_be_bytes([*h, *l]) as usize;
        if len == 0 || len > rest.len() {
            break;
        }
        n += 1;
        b = &rest[len..];
    }
    n
}

fn rel(canon: &str) -> &str {
    &canon[3..]
}

fn file(sha: &[u8; 32]) -> String {
    alloc::format!("{}\\{}.der", rel(DIR), to_hex(sha))
}

/// DER de las raíces propias. Un archivo ilegible o que no es un certificado se salta.
pub fn custom(fs: &mut Nffs) -> Vec<Vec<u8>> {
    let Ok(list) = fs.list(rel(DIR)) else { return Vec::new() };
    let mut out = Vec::new();
    for e in list.iter().filter(|e| !e.dir).take(MAX_CUSTOM) {
        if let Ok(d) = fs.read(&alloc::format!("{}\\{}", rel(DIR), e.name)) {
            if Cert::parse(&d).is_ok() {
                out.push(d);
            }
        }
    }
    out
}

/// Las raíces propias con sus datos.
pub fn list(fs: &mut Nffs) -> Vec<CertInfo> {
    custom(fs).iter().filter_map(|d| Cert::parse(d).ok()).map(|c| c.info()).collect()
}

/// Guarda una raíz (el DER que dio `check`). `Exists` si ya está (propia o de Mozilla).
pub fn add(fs: &mut Nffs, cache: &mut Option<Arc<RootStore>>, der: &[u8], now: DateTime) -> Result<(), FsError> {
    let c = Cert::parse(der).map_err(|_| FsError::Invalid)?;
    if !c.is_ca() && !c.is_self_issued() {
        return Err(FsError::Invalid);
    }
    let sha = sha256(der);
    if store(fs, cache).list().iter().any(|i| i.sha256 == sha) {
        return Err(FsError::Exists);
    }
    if custom(fs).len() >= MAX_CUSTOM {
        return Err(FsError::NoSpace);
    }
    for d in ["Sistema", "Sistema\\config", rel(DIR)] {
        if fs.stat(d).is_err() {
            fs.mkdir(d, now)?;
        }
    }
    fs.write(&file(&sha), der, now)?;
    *cache = None;
    Ok(())
}

/// Quita una raíz propia por su huella. Las de Mozilla no se pueden quitar.
pub fn remove(fs: &mut Nffs, cache: &mut Option<Arc<RootStore>>, sha: &[u8; 32]) -> Result<(), FsError> {
    fs.remove(&file(sha))?;
    *cache = None;
    Ok(())
}

/// El almacén completo (Mozilla + propias), de la caché si no ha cambiado nada.
pub fn store(fs: &mut Nffs, cache: &mut Option<Arc<RootStore>>) -> Arc<RootStore> {
    if let Some(s) = cache {
        return s.clone();
    }
    let mut s = RootStore::new();
    let _ = s.load_bundle(MOZILLA);
    for d in custom(fs) {
        let _ = s.add_der(&d);
    }
    let s = Arc::new(s);
    *cache = Some(s.clone());
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::RamDisk;

    const RAIZ: &[u8] = include_bytes!("x509/testdata/raiz.der");
    const INTER: &[u8] = include_bytes!("x509/testdata/inter.der");
    const HOJA: &[u8] = include_bytes!("x509/testdata/hoja.der");
    const AUTO: &[u8] = include_bytes!("x509/testdata/autofirmada.der");

    fn fs() -> Nffs {
        Nffs::format(alloc::boxed::Box::new(RamDisk::new(512)), DateTime::default()).unwrap()
    }

    fn pem(d: &[u8]) -> String {
        let t = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::from("-----BEGIN CERTIFICATE-----\n");
        for c in d.chunks(3) {
            let v = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                s.push(if i <= c.len() { t[(v >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
            }
        }
        s.push_str("\n-----END CERTIFICATE-----\n");
        s
    }

    #[test]
    fn comprueba_el_archivo() {
        assert_eq!(check(RAIZ).unwrap().0, RAIZ);
        assert!(check(AUTO).is_ok(), "autofirmado vale aunque no sea CA");
        assert_eq!(check(HOJA).unwrap_err(), RootError::NoCa);
        assert_eq!(check(b"hola").unwrap_err(), RootError::NoCert);
        assert_eq!(check(pem(RAIZ).as_bytes()).unwrap().0, RAIZ);
        let dos = alloc::format!("{}{}", pem(RAIZ), pem(INTER));
        assert_eq!(check(dos.as_bytes()).unwrap_err(), RootError::Varios);
    }

    #[test]
    fn anade_duplicado_quita_y_recarga_la_cache() {
        let (mut f, mut cache, n) = (fs(), None, DateTime::default());
        let base = store(&mut f, &mut cache).len();
        assert_eq!(base, mozilla_count());
        assert!(list(&mut f).is_empty());
        add(&mut f, &mut cache, RAIZ, n).unwrap();
        assert!(cache.is_none(), "añadir invalida la caché");
        let s = store(&mut f, &mut cache);
        assert_eq!(s.len(), base + 1);
        assert!(Arc::ptr_eq(&s, &store(&mut f, &mut cache)), "sin cambios, la misma caché");
        assert_eq!(list(&mut f)[0].sha256, sha256(RAIZ));
        assert_eq!(add(&mut f, &mut cache, RAIZ, n), Err(FsError::Exists));
        assert_eq!(add(&mut f, &mut cache, HOJA, n), Err(FsError::Invalid));
        assert_eq!(add(&mut f, &mut cache, b"basura", n), Err(FsError::Invalid));
        // Una de Mozilla tampoco entra como propia.
        let moz = &MOZILLA[2..2 + u16::from_be_bytes([MOZILLA[0], MOZILLA[1]]) as usize];
        assert_eq!(add(&mut f, &mut cache, moz, n), Err(FsError::Exists));
        // Un archivo roto en la carpeta se salta.
        f.write(&alloc::format!("{}\\roto.der", rel(DIR)), b"xx", n).unwrap();
        assert_eq!(custom(&mut f).len(), 1);
        remove(&mut f, &mut cache, &sha256(RAIZ)).unwrap();
        assert!(cache.is_none());
        assert_eq!(store(&mut f, &mut cache).len(), base);
        assert_eq!(remove(&mut f, &mut cache, &sha256(RAIZ)), Err(FsError::NotFound));
    }
}
