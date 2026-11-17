// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Programas instalados en `C:\Programas` (F3, v0.8): cada uno en su carpeta con `app.txt` y
//! `programa.nfx`. CONTRATO: `Program`, `parse_manifest`, `installed`, `open`.
//!
//! `app.txt` (UTF-8, una clave por línea, `#` = comentario):
//! ```text
//! nombre=Calculadora
//! version=1.0
//! autor=NightFury
//! icono=calc          # app, calc, notas, terminal, ajustes, carpeta
//! ancho=320
//! alto=470
//! ```
use crate::files::{self, PROGRAMS_DIR};
use crate::fs::FsError;
use crate::bmp::{self, Picture};
use crate::icons::Icon;
use crate::platform::Platform;
use crate::proc::ProcApp;
use alloc::string::String;
use alloc::vec::Vec;

pub const MANIFEST: &str = "app.txt";
pub const BINARY: &str = "programa.nfx";
/// Icono propio opcional: BMP de 24 o 32 bits, cuadrado, hasta 256×256 (`bmp::decode`).
pub const ICON: &str = "icono.bmp";
/// Tamaño máximo de un `.nfx` que se carga (lo que cabe en memoria de sobra).
pub const MAX_NFX: u64 = 64 << 20;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Program {
    /// Carpeta del programa (ruta absoluta).
    pub dir: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub icon: Icon,
    /// Tamaño inicial del contenido.
    pub size: (i32, i32),
    /// Icono propio (`icono.bmp` de su carpeta, v0.9). Si está, se dibuja en vez de `icon`.
    pub picture: Option<Picture>,
}

impl Program {
    pub fn binary(&self) -> String {
        files::join(&self.dir, BINARY)
    }
}

fn icon_named(s: &str) -> Icon {
    match s {
        "calc" => Icon::Calc,
        "notas" => Icon::Notepad,
        "terminal" => Icon::Terminal,
        "ajustes" => Icon::Settings,
        "carpeta" => Icon::Folder,
        // shortcut: sin icono propio (`icono.nfi`) se usa el logo; llega con la v0.9.
        _ => Icon::Logo,
    }
}

/// Lee un `app.txt`. `None` si falta `nombre`. Lo demás tiene valores por defecto.
pub fn parse_manifest(dir: &str, text: &str) -> Option<Program> {
    let mut p = Program { dir: String::from(dir), name: String::new(), version: String::new(), author: String::new(), icon: Icon::Logo, size: (480, 360), picture: None };
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        let dim = |v: &str| v.parse::<i32>().ok().filter(|n| (100..=4000).contains(n));
        match k.trim() {
            "nombre" => p.name = v.chars().filter(|c| !c.is_control()).take(64).collect(),
            "version" => p.version = v.chars().take(32).collect(),
            "autor" => p.author = v.chars().take(64).collect(),
            "icono" => p.icon = icon_named(v),
            "ancho" => p.size.0 = dim(v).unwrap_or(p.size.0),
            "alto" => p.size.1 = dim(v).unwrap_or(p.size.1),
            _ => {}
        }
    }
    (!p.name.is_empty()).then_some(p)
}

/// Programas de `C:\Programas` con `app.txt` válido y `programa.nfx`, por nombre.
pub fn installed(sys: &mut dyn Platform) -> Vec<Program> {
    let fs = sys.files();
    let mut v = Vec::new();
    for e in fs.list(PROGRAMS_DIR).unwrap_or_default() {
        if !e.dir {
            continue;
        }
        let dir = files::join(PROGRAMS_DIR, &e.name);
        let Ok(txt) = fs.read(&files::join(&dir, MANIFEST)) else { continue };
        let Some(p) = parse_manifest(&dir, &String::from_utf8_lossy(&txt)) else { continue };
        if fs.stat(&p.binary()).is_ok_and(|s| !s.dir) {
            let mut p = p;
            let pic = files::join(&p.dir, ICON);
            if fs.stat(&pic).is_ok_and(|s| s.size <= 1 << 20) {
                p.picture = fs.read(&pic).ok().and_then(|b| bmp::decode(&b));
            }
            v.push(p);
        }
    }
    v.sort_by_key(|p| p.name.to_lowercase());
    v
}

/// El programa al que pertenece `path` (su `programa.nfx`, o un `.nfx` suelto sin `app.txt`).
pub fn for_binary(sys: &mut dyn Platform, path: &str) -> Option<Program> {
    if !path.to_lowercase().ends_with(".nfx") {
        return None;
    }
    let (dir, file) = path.rsplit_once('\\')?;
    if let Some(p) = installed(sys).into_iter().find(|p| p.binary().eq_ignore_ascii_case(path)) {
        return Some(p);
    }
    let name = file.rsplit_once('.').map_or(file, |(n, _)| n);
    Some(Program { dir: String::from(dir), name: String::from(name), version: String::new(), author: String::new(), icon: Icon::Logo, size: (480, 360), picture: None })
}

/// Carga el `.nfx` de `path` y lo arranca. `Err` con el motivo para enseñarlo al usuario.
pub fn open(sys: &mut dyn Platform, p: &Program, path: &str) -> Result<ProcApp, String> {
    let fs = sys.files();
    let st = fs.stat(path).map_err(fs_msg)?;
    if st.size > MAX_NFX {
        return Err(String::from("El programa es demasiado grande"));
    }
    let data = fs.read(path).map_err(fs_msg)?;
    let proc = sys.spawn(&data, &p.name)?;
    Ok(ProcApp::new(proc, &p.name, Some(p.icon)))
}

fn fs_msg(e: FsError) -> String {
    String::from(match e {
        FsError::Denied => "No tienes permiso para abrir este programa",
        FsError::NotFound => "No se encuentra el programa",
        _ => "No se pudo leer el programa",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{DateTime, TestPlatform};

    #[test]
    fn manifiesto() {
        let p = parse_manifest("C:\\Programas\\Calc", "# hola\nnombre = Calculadora \nicono=calc\nancho=320\nalto=99999\nraro=1\n").unwrap();
        assert_eq!((p.name.as_str(), p.icon, p.size), ("Calculadora", Icon::Calc, (320, 360)));
        assert_eq!(parse_manifest("x", "version=1\n"), None);
    }

    #[test]
    fn lista_y_permisos() {
        let mut sys = TestPlatform::default();
        let n = DateTime::default();
        let f = sys.files();
        f.install_system_program("Calculadora", &[(MANIFEST, b"nombre=Calculadora\nicono=calc\n"), (BINARY, b"NFX1")], n).unwrap();
        assert!(!f.install_system_program("Calculadora", &[(BINARY, b"NFX1")], n).unwrap(), "sin cambios no escribe");
        f.install_system_program("Roto", &[(MANIFEST, b"nombre=Roto\n")], n).unwrap();
        assert_eq!(f.install_system_program("..\\x", &[], n), Err(FsError::Invalid));
        let v = installed(&mut sys);
        assert_eq!(v.len(), 1, "sin programa.nfx no cuenta");
        assert_eq!(v[0].binary(), "C:\\Programas\\Calculadora\\programa.nfx");
        let p = for_binary(&mut sys, "C:\\Programas\\Calculadora\\programa.nfx").unwrap();
        assert_eq!(p.name, "Calculadora");
        assert_eq!(for_binary(&mut sys, "C:\\x\\juego.nfx").unwrap().name, "juego");
        assert_eq!(for_binary(&mut sys, "C:\\x\\juego.txt"), None);
        // La plataforma de pruebas no ejecuta programas: el motivo llega al usuario.
        assert!(open(&mut sys, &p, &p.binary()).is_err());
    }
}
