// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! PNG (W3C / ISO 15948): todos los tipos de color, profundidades 1/2/4/8/16, paleta y tRNS, Adam7 y
//! los cinco filtros. Los bloques traen CRC-32; los auxiliares desconocidos se ignoran. APNG: solo el
//! primer fotograma (el de los `IDAT`). Las muestras de 16 bits se pasan a 8.
use super::inflate::{crc32, Inflate, Wrapper};
use super::{comprobar, ImgError, Limits};
use crate::bmp::Picture;
use alloc::vec;
use alloc::vec::Vec;

pub const FIRMA: [u8; 8] = [0x89, b'P', b'N', b'G', 13, 10, 26, 10];

/// (x0, y0, salto x, salto y) de cada pasada Adam7.
const ADAM7: [(u32, u32, u32, u32); 7] = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];
const UNA_PASADA: [(u32, u32, u32, u32); 1] = [(0, 0, 1, 1)];

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

struct Cab {
    w: u32,
    h: u32,
    prof: u8,
    color: u8,
    entrelazado: bool,
}

impl Cab {
    fn canales(&self) -> u32 {
        match self.color {
            2 => 3,
            4 => 2,
            6 => 4,
            _ => 1,
        }
    }
}

/// Ancho y alto del `IHDR`, sin decodificar nada más.
pub fn dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.get(12..16)? != b"IHDR" {
        return None;
    }
    let (w, h) = (be32(data.get(16..20)?), be32(data.get(20..24)?));
    (w != 0 && h != 0 && w < 1 << 31 && h < 1 << 31).then_some((w, h))
}

fn cabecera(d: &[u8]) -> Result<Cab, ImgError> {
    if d.len() != 13 {
        return Err(ImgError::Formato);
    }
    let c = Cab { w: be32(d), h: be32(&d[4..]), prof: d[8], color: d[9], entrelazado: d[12] == 1 };
    let ok = match c.color {
        0 => matches!(c.prof, 1 | 2 | 4 | 8 | 16),
        3 => matches!(c.prof, 1 | 2 | 4 | 8),
        2 | 4 | 6 => matches!(c.prof, 8 | 16),
        _ => false,
    };
    if !ok || d[10] != 0 || d[11] != 0 || d[12] > 1 || c.w == 0 || c.h == 0 || c.w >= 1 << 31 || c.h >= 1 << 31 {
        return Err(ImgError::Formato);
    }
    Ok(c)
}

/// Tamaño de una pasada: (ancho, alto, bytes por fila).
fn pasada(c: &Cab, p: (u32, u32, u32, u32)) -> (u32, u32, usize) {
    let pw = (c.w.saturating_sub(p.0) + p.2 - 1) / p.2;
    let ph = (c.h.saturating_sub(p.1) + p.3 - 1) / p.3;
    (pw, ph, (pw as usize * (c.canales() * c.prof as u32) as usize).div_ceil(8))
}

pub fn decode(data: &[u8], lim: Limits) -> Result<Picture, ImgError> {
    if !data.starts_with(&FIRMA) {
        return Err(ImgError::Formato);
    }
    let mut pos = FIRMA.len();
    let mut cab: Option<Cab> = None;
    let mut paleta: Vec<u32> = Vec::new();
    let mut clave: Option<[u16; 3]> = None;
    let mut inf: Option<Inflate> = None;
    let mut crudo: Vec<u8> = Vec::new();
    let (mut hecho, mut visto_idat, mut iend) = (false, false, false);
    let mut esperado = 0usize;
    while pos < data.len() {
        let cab_ok = data.get(pos..pos + 8).ok_or(ImgError::Truncado)?;
        let len = be32(cab_ok) as usize;
        if len > i32::MAX as usize {
            return Err(ImgError::Formato);
        }
        let fin = pos + 8 + len;
        let tipo = &cab_ok[4..8];
        let cuerpo = data.get(pos + 8..fin).ok_or(ImgError::Truncado)?;
        let crc = data.get(fin..fin + 4).ok_or(ImgError::Truncado)?;
        if crc32(&data[pos + 4..fin]) != be32(crc) {
            return Err(ImgError::Formato);
        }
        pos = fin + 4;
        if cab.is_none() && tipo != b"IHDR" {
            return Err(ImgError::Formato);
        }
        match tipo {
            b"IHDR" => {
                if cab.is_some() {
                    return Err(ImgError::Formato);
                }
                let c = cabecera(cuerpo)?;
                comprobar(c.w, c.h, &lim)?;
                let pasadas: &[_] = if c.entrelazado { &ADAM7 } else { &UNA_PASADA };
                for &p in pasadas {
                    let (pw, ph, rb) = pasada(&c, p);
                    if pw > 0 && ph > 0 {
                        esperado = esperado.saturating_add((rb + 1).saturating_mul(ph as usize));
                    }
                }
                if esperado > lim.max_bytes {
                    return Err(ImgError::Limite);
                }
                inf = Some(Inflate::new(Wrapper::Zlib, esperado));
                cab = Some(c);
            }
            b"PLTE" => {
                let c = cab.as_ref().ok_or(ImgError::Formato)?;
                if visto_idat || c.color == 0 || c.color == 4 || cuerpo.is_empty() || cuerpo.len() % 3 != 0 || cuerpo.len() / 3 > 1 << c.prof.min(8) {
                    return Err(ImgError::Formato);
                }
                paleta = cuerpo.chunks(3).map(|p| 0xFF00_0000 | (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32).collect();
            }
            b"tRNS" => {
                let c = cab.as_ref().ok_or(ImgError::Formato)?;
                let v = |i: usize| (cuerpo[2 * i] as u16) << 8 | cuerpo[2 * i + 1] as u16;
                match (c.color, cuerpo.len()) {
                    (3, n) if n <= paleta.len() => {
                        for (p, &a) in paleta.iter_mut().zip(cuerpo) {
                            *p = (*p & 0xFF_FFFF) | (a as u32) << 24;
                        }
                    }
                    (0, 2) => clave = Some([v(0), 0, 0]),
                    (2, 6) => clave = Some([v(0), v(1), v(2)]),
                    (4 | 6, _) => {} // no debería venir; se ignora
                    _ => return Err(ImgError::Formato),
                }
            }
            b"IDAT" => {
                visto_idat = true;
                if !hecho {
                    if let Some(i) = inf.as_mut() {
                        hecho = i.push(cuerpo, &mut crudo)?;
                    }
                }
            }
            b"IEND" => {
                iend = true;
                break;
            }
            // crítico desconocido: no se puede decodificar bien; los auxiliares (APNG incluido) se ignoran
            [b, ..] if b & 0x20 == 0 => return Err(ImgError::NoSoportado),
            _ => {}
        }
    }
    let c = cab.ok_or(ImgError::Truncado)?;
    if !iend || !hecho || crudo.len() < esperado {
        return Err(ImgError::Truncado);
    }
    if c.color == 3 && paleta.is_empty() {
        return Err(ImgError::Formato);
    }
    convertir(&c, &mut crudo, &paleta, clave)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (a as i32, b as i32, c as i32);
    let p = ia + ib - ic;
    let (pa, pb, pc) = ((p - ia).abs(), (p - ib).abs(), (p - ic).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Deshace el filtro de una fila (`previa` = la fila anterior ya sin filtro, o ceros).
fn quitar_filtro(f: u8, fila: &mut [u8], previa: &[u8], bpp: usize) -> Result<(), ImgError> {
    for i in 0..fila.len() {
        let a = if i >= bpp { fila[i - bpp] } else { 0 };
        let (b, c) = (previa[i], if i >= bpp { previa[i - bpp] } else { 0 });
        let suma = match f {
            0 => 0,
            1 => a,
            2 => b,
            3 => ((a as u16 + b as u16) / 2) as u8,
            4 => paeth(a, b, c),
            _ => return Err(ImgError::Formato),
        };
        fila[i] = fila[i].wrapping_add(suma);
    }
    Ok(())
}

fn convertir(c: &Cab, crudo: &mut [u8], paleta: &[u32], clave: Option<[u16; 3]>) -> Result<Picture, ImgError> {
    let (w, h) = (c.w as usize, c.h as usize);
    let mut argb = vec![0u32; w * h];
    let bpp = ((c.canales() * c.prof as u32) as usize / 8).max(1);
    let pasadas: &[_] = if c.entrelazado { &ADAM7 } else { &UNA_PASADA };
    let mut off = 0usize;
    for &p in pasadas {
        let (pw, ph, rb) = pasada(c, p);
        if pw == 0 || ph == 0 {
            continue;
        }
        let mut previa = vec![0u8; rb];
        for r in 0..ph as usize {
            let f = *crudo.get(off).ok_or(ImgError::Truncado)?;
            let fila = crudo.get_mut(off + 1..off + 1 + rb).ok_or(ImgError::Truncado)?;
            quitar_filtro(f, fila, &previa, bpp)?;
            previa.copy_from_slice(fila);
            off += 1 + rb;
            let y = p.1 as usize + r * p.3 as usize;
            for i in 0..pw as usize {
                let x = p.0 as usize + i * p.2 as usize;
                argb[y * w + x] = pixel(c, fila, i, paleta, clave)?;
            }
        }
    }
    Ok(Picture { w: c.w as i32, h: c.h as i32, argb })
}

/// Muestra `i` de la fila (valor crudo: hasta 16 bits).
fn muestra(fila: &[u8], i: usize, prof: u8) -> u16 {
    match prof {
        16 => (fila[2 * i] as u16) << 8 | fila[2 * i + 1] as u16,
        8 => fila[i] as u16,
        _ => {
            let por_byte = 8 / prof as usize;
            let desp = 8 - prof as usize * (i % por_byte + 1);
            ((fila[i / por_byte] >> desp) & ((1u16 << prof) - 1) as u8) as u16
        }
    }
}

/// Muestra de 16 bits o menos, llevada a 0..=255.
fn a8(v: u16, prof: u8) -> u32 {
    match prof {
        16 => (v >> 8) as u32,
        8 => v as u32,
        _ => v as u32 * 255 / ((1u32 << prof) - 1),
    }
}

fn pixel(c: &Cab, fila: &[u8], i: usize, paleta: &[u32], clave: Option<[u16; 3]>) -> Result<u32, ImgError> {
    let p = c.prof;
    let m = |k: usize| muestra(fila, i * c.canales() as usize + k, p);
    Ok(match c.color {
        0 => {
            let v = m(0);
            let g = a8(v, p);
            let a = if clave.is_some_and(|k| k[0] == v) { 0 } else { 255 };
            a << 24 | g * 0x01_0101
        }
        2 => {
            let (r, g, b) = (m(0), m(1), m(2));
            let a = if clave == Some([r, g, b]) { 0 } else { 255 };
            a << 24 | a8(r, p) << 16 | a8(g, p) << 8 | a8(b, p)
        }
        3 => *paleta.get(m(0) as usize).ok_or(ImgError::Formato)?,
        4 => a8(m(1), p) << 24 | a8(m(0), p) * 0x01_0101,
        _ => a8(m(3), p) << 24 | a8(m(0), p) << 16 | a8(m(1), p) << 8 | a8(m(2), p),
    })
}
