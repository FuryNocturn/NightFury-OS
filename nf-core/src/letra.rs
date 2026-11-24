// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Qué letra usa el sistema (v0.12.5). `Theme::letra` es el valor que viaja del escritorio a los
//! programas: `LIBERATION` (por defecto), `TRAZOS` (la letra de trazos de `font`) o `n >= 2`, la
//! `n - 2`-ésima fuente de `lista`. `aplicar` carga y registra la que toque en `font`; cada proceso
//! (el núcleo y cada programa del KIT) lo llama por su cuenta cuando le llega el tema.
use crate::font;
use crate::fsapi::FilesApi;
use crate::sync::SpinLock;
use crate::ttf::Font;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

pub const LIBERATION: u8 = 0;
pub const TRAZOS: u8 = 1;
const NADA: u8 = 255;
/// Fuentes de Windows que se ofrecen como mucho (el índice cabe en un `u8`).
const MAX_LISTA: usize = 200;
const DIR: &str = "C:\\Sistema\\fuentes\\";
const LIBERATION_FICHEROS: [&str; 3] = ["LiberationSans-Regular.ttf", "LiberationSans-Bold.ttf", "LiberationMono-Regular.ttf"];

/// Lo último que se aplicó con éxito en este proceso.
static ACTUAL: AtomicU8 = AtomicU8::new(NADA);
static MONO_INTENTADA: AtomicBool = AtomicBool::new(false);
/// Bytes de Liberation (sans, negrita, mono) que trae el propio ejecutable (solo el núcleo), para no
/// depender del disco (puede estar bloqueado en el inicio de sesión).
static INCLUIDAS: SpinLock<Option<[&'static [u8]; 3]>> = SpinLock::new(None);

/// El núcleo entrega los bytes de Liberation incluidos y la deja registrada.
pub fn incluidas(sans: &'static [u8], neg: &'static [u8], mono: &'static [u8]) {
    *INCLUIDAS.lock() = Some([sans, neg, mono]);
    aplicar_liberation(None);
}

fn bytes_liberation(fs: &mut Option<&mut dyn FilesApi>, i: usize) -> Option<Vec<u8>> {
    match *INCLUIDAS.lock() {
        Some(b) => Some(b[i].to_vec()),
        None => fs.as_mut()?.read(&format!("{DIR}{}", LIBERATION_FICHEROS[i])).ok(),
    }
}

fn aplicar_liberation(mut fs: Option<&mut dyn FilesApi>) -> bool {
    let Some(sans) = bytes_liberation(&mut fs, 0).and_then(|b| Font::parse(b).ok()) else { return false };
    let neg = bytes_liberation(&mut fs, 1).and_then(|b| Font::parse(b).ok());
    font::set_ttf(sans, neg);
    ACTUAL.store(LIBERATION, Ordering::Relaxed);
    true
}

#[cfg(test)]
pub(crate) fn aplicar_liberacion_sin_fuentes() -> bool {
    INCLUIDAS.lock().is_none() && aplicar_liberation(None)
}

/// Unidades desbloqueadas donde mirar `\Windows\Fonts`.
fn unidades(fs: &mut dyn FilesApi) -> Vec<char> {
    fs.drives().iter().filter(|d| !d.locked).map(|d| d.letter).collect()
}

/// Ficheros `.ttf` de `\Windows\Fonts` de cualquier unidad, sin repetir y por orden alfabético.
pub fn lista(fs: &mut dyn FilesApi) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for u in unidades(fs) {
        if let Ok(es) = fs.list(&format!("{u}:\\Windows\\Fonts")) {
            v.extend(es.into_iter().filter(|e| !e.dir && e.name.to_ascii_lowercase().ends_with(".ttf")).map(|e| e.name));
        }
    }
    v.sort_by_key(|n| n.to_lowercase());
    v.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    v.truncate(MAX_LISTA);
    v
}

/// Nombre con el que se guarda la elección en la configuración.
pub fn nombre(fs: &mut dyn FilesApi, letra: u8) -> String {
    match letra {
        LIBERATION => String::from("liberation"),
        TRAZOS => String::from("trazos"),
        n => lista(fs).get(n as usize - 2).cloned().unwrap_or_else(|| String::from("liberation")),
    }
}

/// Lo contrario de `nombre`; lo que no se reconoce (o ya no existe) es Liberation.
pub fn indice(fs: &mut dyn FilesApi, nombre: &str) -> u8 {
    match nombre.trim() {
        "trazos" => TRAZOS,
        "liberation" => LIBERATION,
        n => lista(fs).iter().position(|f| f.eq_ignore_ascii_case(n)).map_or(LIBERATION, |i| i as u8 + 2),
    }
}

fn leer_windows(fs: &mut dyn FilesApi, fichero: &str) -> Option<Font> {
    for u in unidades(fs) {
        if let Some(f) = fs.read(&format!("{u}:\\Windows\\Fonts\\{fichero}")).ok().and_then(|d| Font::parse(d).ok()) {
            return Some(f);
        }
    }
    None
}

/// Registra la letra `letra` en este proceso. Si no se puede cargar (fichero ausente o fuente rota) no
/// cambia nada: sin ninguna anterior, se queda la letra de trazos. Devuelve si la letra cambió (hay que
/// redibujar aunque el resto del tema sea el mismo).
pub fn aplicar(fs: &mut dyn FilesApi, letra: u8) -> bool {
    if ACTUAL.load(Ordering::Relaxed) == letra {
        return false;
    }
    match letra {
        TRAZOS => {
            font::quitar_ttf();
            ACTUAL.store(TRAZOS, Ordering::Relaxed);
            true
        }
        LIBERATION => aplicar_liberation(Some(fs)),
        n => {
            let Some(nombre) = lista(fs).get(n as usize - 2).cloned() else { return false };
            let Some(f) = leer_windows(fs, &nombre) else { return false };
            // shortcut: sin variante en negrita la fabrica `ttf` (más tosca); buscar la `bd` si molesta.
            font::set_ttf(f, None);
            ACTUAL.store(n, Ordering::Relaxed);
            true
        }
    }
}

/// La Terminal pide su letra monoespaciada (Liberation Mono) la primera vez que dibuja.
pub fn asegurar_mono(fs: &mut dyn FilesApi) {
    if font::hay_mono() || ACTUAL.load(Ordering::Relaxed) == TRAZOS || MONO_INTENTADA.swap(true, Ordering::Relaxed) {
        return;
    }
    if let Some(f) = bytes_liberation(&mut Some(fs), 2).and_then(|b| Font::parse(b).ok()) {
        font::set_mono(f);
    }
}
