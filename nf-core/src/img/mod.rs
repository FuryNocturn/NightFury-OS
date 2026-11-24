// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Decodificadores propios de imágenes (PNG, JPEG, GIF) y de DEFLATE/zlib/gzip, hechos desde las normas
//! públicas (RFC 1950-1952, PNG, ITU-T T.81 + JFIF, GIF89a). La entrada es siempre hostil (imágenes de
//! webs): nada entra en pánico, los índices se comprueban y los tamaños se limitan antes de reservar.
use crate::bmp::Picture;
use crate::surface::{blend, Surface};

pub mod gif;
pub mod inflate;
pub mod jpeg;
pub mod png;

#[cfg(test)]
pub(crate) mod tests;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImgError {
    /// Datos que no cumplen la norma.
    Formato,
    /// Válido, pero con una variante que no se implementa.
    NoSoportado,
    /// Supera `Limits` (imagen o salida descomprimida demasiado grande).
    Limite,
    /// El archivo acaba antes de tiempo.
    Truncado,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Png,
    Jpeg,
    Gif,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_pixels: u64,
    /// Bytes de salida descomprimida (y de memoria de trabajo) como mucho.
    pub max_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_pixels: 16_000_000, max_bytes: 64 << 20 }
    }
}

/// Reconoce el formato por sus primeros bytes.
pub fn sniff(data: &[u8]) -> Option<Kind> {
    if data.starts_with(&png::FIRMA) {
        Some(Kind::Png)
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Kind::Jpeg)
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some(Kind::Gif)
    } else {
        None
    }
}

pub fn decode(data: &[u8], lim: Limits) -> Result<Picture, ImgError> {
    match sniff(data) {
        Some(Kind::Png) => png::decode(data, lim),
        Some(Kind::Jpeg) => jpeg::decode(data, lim),
        Some(Kind::Gif) => gif::decode(data, lim),
        None => Err(ImgError::Formato),
    }
}

/// Ancho y alto sin decodificar (para maquetar antes de tener la imagen entera).
pub fn dimensions(data: &[u8]) -> Option<(u32, u32)> {
    match sniff(data)? {
        Kind::Png => png::dimensions(data),
        Kind::Jpeg => jpeg::dimensions(data),
        Kind::Gif => gif::dimensions(data),
    }
}

/// `w`×`h` píxeles dentro de los límites; devuelve cuántos son.
pub(crate) fn comprobar(w: u32, h: u32, lim: &Limits) -> Result<usize, ImgError> {
    if w == 0 || h == 0 {
        return Err(ImgError::Formato);
    }
    let n = w as u64 * h as u64;
    // la imagen final (4 bytes por píxel) también cuenta
    if n > lim.max_pixels || n * 4 > lim.max_bytes as u64 {
        return Err(ImgError::Limite);
    }
    Ok(n as usize)
}

// ---- dibujo escalado ----

/// Muestras de un eje: píxeles de origen `[a, b]` y peso `f` (0..=256) del segundo en el modo bilineal;
/// en el modo caja (al reducir) se promedian todos de `a` a `b` inclusive.
struct Toma {
    a: i64,
    b: i64,
    f: i64,
    caja: bool,
}

fn toma(d: i64, n_dst: i64, n_src: i64) -> Toma {
    // i128: con rectángulos enormes (hostiles) i64 se desbordaría
    let (d, n_dst, n_src) = (d as i128, n_dst as i128, n_src as i128);
    if n_src > n_dst {
        let a = d * n_src / n_dst;
        let b = (((d + 1) * n_src + n_dst - 1) / n_dst - 1).max(a).min(n_src - 1);
        return Toma { a: a as i64, b: b as i64, f: 0, caja: true };
    }
    // centro del píxel de destino en coordenadas de origen, en 1/256
    let s = (((2 * d + 1) * n_src * 128) / n_dst - 128) as i64;
    let n_src = n_src as i64;
    let a = (s >> 8).clamp(0, n_src - 1);
    let b = ((s >> 8) + 1).clamp(0, n_src - 1);
    let f = if s < 0 || s >> 8 >= n_src - 1 { 0 } else { s & 255 };
    Toma { a, b, f, caja: false }
}

fn pesos(t: &Toma, mut f: impl FnMut(i64, i64)) {
    if t.caja {
        for i in t.a..=t.b {
            f(i, 1);
        }
    } else {
        f(t.a, 256 - t.f);
        if t.f != 0 {
            f(t.b, t.f);
        }
    }
}

/// Dibuja `p` escalada al rectángulo (`x`, `y`, `w`, `h`): bilineal al ampliar, promedio de área al
/// reducir; el alfa se mezcla con `blend` y se respeta `s.clip()`.
// shortcut: el promedio de área usa píxeles enteros (sin pesos fraccionarios en los bordes); suficiente salvo reducciones enormes
pub fn draw_scaled(p: &Picture, s: &mut Surface, x: i32, y: i32, w: i32, h: i32) {
    if w <= 0 || h <= 0 || p.w <= 0 || p.h <= 0 || p.argb.len() < (p.w as usize).saturating_mul(p.h as usize) {
        return;
    }
    let clip = s.clip();
    let (x0, y0) = (clip.x.max(x), clip.y.max(y));
    let (x1, y1) = (clip.right().min(x.saturating_add(w)), clip.bottom().min(y.saturating_add(h)));
    for py in y0..y1 {
        let ty = toma(py as i64 - y as i64, h as i64, p.h as i64);
        for px in x0..x1 {
            let tx = toma(px as i64 - x as i64, w as i64, p.w as i64);
            let (mut sa, mut sr, mut sg, mut sb, mut sw) = (0u64, 0u64, 0u64, 0u64, 0u64);
            pesos(&ty, |iy, wy| {
                pesos(&tx, |ix, wx| {
                    let c = p.argb[(iy * p.w as i64 + ix) as usize];
                    let wt = (wx * wy) as u64;
                    let a = (c >> 24) as u64 * wt;
                    sa += a;
                    sr += (c >> 16 & 255) as u64 * a;
                    sg += (c >> 8 & 255) as u64 * a;
                    sb += (c & 255) as u64 * a;
                    sw += wt;
                })
            });
            if sa == 0 {
                continue;
            }
            let (r, g, b) = ((sr / sa) as u32, (sg / sa) as u32, (sb / sa) as u32);
            let a = (sa / sw) as u32;
            let i = (py * s.w + px) as usize;
            s.px[i] = blend(s.px[i], r << 16 | g << 8 | b, a);
        }
    }
}
