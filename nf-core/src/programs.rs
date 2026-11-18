// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Programas instalados en `C:\Programas` (F3, v0.8): cada uno en su carpeta con `app.txt` y
//! `programa.nfx`. CONTRATO: `Program`, `parse_manifest`, `installed`, `open`, y desde la v0.9
//! `check_source`, `install` y `uninstall` (los usa Configuración > Aplicaciones).
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
use crate::fsapi::FilesApi;
use crate::fs::FsError;
use crate::bmp::{self, Picture};
use crate::icons::Icon;
use crate::platform::{DateTime, Platform};
use crate::proc::ProcApp;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub const MANIFEST: &str = "app.txt";
pub const BINARY: &str = "programa.nfx";
/// Icono propio opcional: BMP de 24 o 32 bits, cuadrado, hasta 256×256 (`bmp::decode`).
pub const ICON: &str = "icono.bmp";
/// Marca de los programas que trae el sistema (no se desinstalan ni se reemplazan).
pub const SYSTEM_MARK: &str = ".sistema";
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
    /// Viene con el sistema (`.sistema` en su carpeta).
    pub system: bool,
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
    let mut p = Program { dir: String::from(dir), name: String::new(), version: String::new(), author: String::new(), icon: Icon::Logo, size: (480, 360), picture: None, system: false };
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
            p.system = fs.stat(&files::join(&p.dir, SYSTEM_MARK)).is_ok();
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
    Some(Program { dir: String::from(dir), name: String::from(name), version: String::new(), author: String::new(), icon: Icon::Logo, size: (480, 360), picture: None, system: false })
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

/// Comprueba una carpeta con un programa para instalar: `app.txt` con nombre válido y un
/// `programa.nfx` que se puede cargar. Devuelve el programa (con `dir` = la carpeta de origen).
pub fn check_source(fs: &mut dyn FilesApi, src: &str) -> Result<Program, String> {
    if src.to_lowercase().starts_with(&PROGRAMS_DIR.to_lowercase()) {
        return Err(String::from("Esa carpeta ya está dentro de C:\\Programas"));
    }
    let txt = fs.read(&files::join(src, MANIFEST)).map_err(|_| String::from("La carpeta no tiene app.txt: no es un programa de NightFury"))?;
    let p = parse_manifest(src, &String::from_utf8_lossy(&txt)).ok_or_else(|| String::from("El app.txt no tiene la línea nombre="))?;
    if !files::valid_name(&p.name) {
        return Err(format!("«{}» no vale como nombre de carpeta", p.name));
    }
    let bin = p.binary();
    let st = fs.stat(&bin).map_err(|_| String::from("Falta programa.nfx"))?;
    if st.dir || st.size > MAX_NFX {
        return Err(String::from("programa.nfx no es válido"));
    }
    let data = fs.read(&bin).map_err(fs_msg)?;
    crate::nfx::parse(&data).map_err(|e| {
        String::from(match e {
            crate::nfx::NfxError::Version => "El programa es para otra versión del sistema",
            crate::nfx::NfxError::TooBig => "El programa necesita demasiada memoria",
            _ => "programa.nfx está dañado",
        })
    })?;
    Ok(p)
}

/// `Some(es_del_sistema)` si ya hay un programa instalado con ese nombre.
pub fn installed_as(fs: &mut dyn FilesApi, name: &str) -> Option<bool> {
    let dir = files::join(PROGRAMS_DIR, name);
    fs.stat(&dir).ok().filter(|e| e.dir)?;
    Some(fs.stat(&files::join(&dir, SYSTEM_MARK)).is_ok())
}

/// Copia el programa `src` (de `check_source`) a `C:\Programas\<nombre>`, reemplazando el que
/// hubiera con ese nombre salvo si es del sistema. Solo copia `app.txt`, `programa.nfx` e
/// `icono.bmp`. Necesita una sesión de administrador (lo hacen cumplir los permisos).
pub fn install(fs: &mut dyn FilesApi, src: &Program, now: DateTime) -> Result<String, String> {
    if installed_as(fs, &src.name) == Some(true) {
        return Err(format!("«{}» es un programa del sistema y no se puede reemplazar", src.name));
    }
    let dest = files::join(PROGRAMS_DIR, &src.name);
    if fs.stat(&dest).is_err() {
        fs.mkdir(&dest, now).map_err(install_msg)?;
    }
    for name in [MANIFEST, BINARY, ICON] {
        let from = files::join(&src.dir, name);
        let to = files::join(&dest, name);
        match fs.read(&from) {
            Ok(data) => fs.write(&to, &data, now).map_err(install_msg)?,
            // Sin icono en el origen: fuera también el de una versión anterior.
            Err(_) if name == ICON => {
                let _ = fs.delete(&to);
            }
            Err(e) => return Err(install_msg(e)),
        }
    }
    fs.log(&format!("Programa instalado: {}", src.name), now);
    Ok(dest)
}

/// Manda el programa a la Papelera (los del sistema no se desinstalan).
pub fn uninstall(fs: &mut dyn FilesApi, p: &Program, now: DateTime) -> Result<(), String> {
    if p.system || installed_as(fs, &p.name) == Some(true) {
        return Err(format!("«{}» viene con el sistema y no se puede desinstalar", p.name));
    }
    fs.user_trash_many(&[p.dir.clone()], now).map_err(install_msg)?;
    fs.log(&format!("Programa desinstalado: {}", p.name), now);
    Ok(())
}

fn install_msg(e: FsError) -> String {
    match e {
        FsError::Denied => String::from("Solo un administrador puede instalar o desinstalar programas"),
        e => String::from(e.message()),
    }
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
    use crate::platform::TestPlatform;

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
        assert!(p.system, "los instalados por el sistema llevan la marca");
    }

    fn nfx_valido() -> Vec<u8> {
        use crate::nfx::{encode, Kind, Section};
        let b = crate::abi::USER_BASE + 0x40_0000;
        encode(b, &[Section { kind: Kind::Code, vaddr: b, memsz: 4, data: &[0x90; 4] }])
    }

    #[test]
    fn instalar_y_desinstalar() {
        let mut sys = TestPlatform::default();
        let n = DateTime::default();
        let f = sys.files();
        let src = "C:\\Usuarios\\Usuario\\Descargas\\Juego";
        f.mkdir(src, n).unwrap();
        assert!(check_source(f, src).unwrap_err().contains("app.txt"));
        f.write(&files::join(src, MANIFEST), b"nombre=Juego\nversion=1.0\n", n).unwrap();
        assert!(check_source(f, src).unwrap_err().contains("programa.nfx"));
        f.write(&files::join(src, BINARY), b"NFX1 roto", n).unwrap();
        assert!(check_source(f, src).unwrap_err().contains("dañado"));
        f.write(&files::join(src, BINARY), &nfx_valido(), n).unwrap();
        f.write(&files::join(src, "otro.txt"), b"no se copia", n).unwrap();
        let p = check_source(f, src).unwrap();
        assert_eq!(installed_as(f, "Juego"), None);
        assert_eq!(install(f, &p, n).unwrap(), "C:\\Programas\\Juego");
        assert_eq!(installed_as(f, "Juego"), Some(false));
        assert!(f.stat("C:\\Programas\\Juego\\otro.txt").is_err(), "solo los archivos del programa");
        // Reemplazar con una versión nueva sin icono quita el icono viejo.
        f.write("C:\\Programas\\Juego\\icono.bmp", b"BM", n).unwrap();
        f.write(&files::join(src, MANIFEST), b"nombre=Juego\nversion=2.0\n", n).unwrap();
        let p2 = check_source(f, src).unwrap();
        install(f, &p2, n).unwrap();
        assert!(f.stat("C:\\Programas\\Juego\\icono.bmp").is_err());
        let v = installed(&mut sys);
        let j = v.iter().find(|p| p.name == "Juego").unwrap().clone();
        assert_eq!(j.version, "2.0");
        assert!(!j.system);
        // Los del sistema no se reemplazan ni se desinstalan.
        let f = sys.files();
        f.install_system_program("Calculadora", &[(MANIFEST, b"nombre=Calculadora\n"), (BINARY, &nfx_valido())], n).unwrap();
        f.write(&files::join(src, MANIFEST), b"nombre=Calculadora\n", n).unwrap();
        let p3 = check_source(f, src).unwrap();
        assert!(install(f, &p3, n).unwrap_err().contains("del sistema"));
        let calc = installed(&mut sys).into_iter().find(|p| p.name == "Calculadora").unwrap();
        assert!(uninstall(sys.files(), &calc, n).is_err());
        // Desinstalar: a la Papelera.
        uninstall(sys.files(), &j, n).unwrap();
        assert!(installed(&mut sys).iter().all(|p| p.name != "Juego"));
        // Desde dentro de C:\Programas no se instala.
        assert!(check_source(sys.files(), "C:\\Programas\\Calculadora").is_err());
    }
}
