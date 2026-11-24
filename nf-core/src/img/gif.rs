// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! GIF 87a/89a: paleta global y local, LZW de hasta 12 bits, entrelazado y transparencia (Graphic
//! Control Extension). `decode` devuelve el primer fotograma compuesto sobre el lienzo lógico (lo que
//! queda fuera es transparente).
//!
//! Para la animación (v0.13): `leer_fotograma` ya entrega cada fotograma con su retardo y su método de
//! descarte; solo falta repetirlo y componer con `componer` sobre el lienzo anterior.
use super::{comprobar, ImgError, Limits};
use crate::bmp::Picture;
use alloc::vec;
use alloc::vec::Vec;

pub fn dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let b = data.get(6..10)?;
    let (w, h) = (u16::from_le_bytes([b[0], b[1]]) as u32, u16::from_le_bytes([b[2], b[3]]) as u32);
    (w != 0 && h != 0).then_some((w, h))
}

struct Lector<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Lector<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], ImgError> {
        let fin = self.pos.checked_add(n).ok_or(ImgError::Truncado)?;
        let r = self.d.get(self.pos..fin).ok_or(ImgError::Truncado)?;
        self.pos = fin;
        Ok(r)
    }
    fn u8(&mut self) -> Result<u8, ImgError> {
        Ok(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> Result<u32, ImgError> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]) as u32)
    }
    /// Sub-bloques de datos hasta el terminador, concatenados.
    fn subbloques(&mut self) -> Result<Vec<u8>, ImgError> {
        let mut v = Vec::new();
        loop {
            let n = self.u8()? as usize;
            if n == 0 {
                return Ok(v);
            }
            v.extend_from_slice(self.bytes(n)?);
        }
    }
}

/// Un fotograma ya decodificado (índices de paleta), con lo necesario para componerlo.
#[allow(dead_code)] // retardo y descarte los usará la animación (v0.13)
struct Fotograma {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    indices: Vec<u8>,
    paleta: Vec<u32>,
    transparente: Option<u8>,
    /// Centésimas de segundo.
    retardo: u32,
    descarte: u8,
}

struct Pantalla {
    w: u32,
    h: u32,
    paleta: Vec<u32>,
}

fn leer_paleta(l: &mut Lector, bits: u8) -> Result<Vec<u32>, ImgError> {
    let n = 1usize << (bits + 1);
    Ok(l.bytes(n * 3)?.chunks(3).map(|c| 0xFF00_0000 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32).collect())
}

fn cabecera(l: &mut Lector, lim: &Limits) -> Result<Pantalla, ImgError> {
    let sig = l.bytes(6)?;
    if sig != b"GIF87a" && sig != b"GIF89a" {
        return Err(ImgError::Formato);
    }
    let (w, h) = (l.u16()?, l.u16()?);
    let emp = l.u8()?;
    l.bytes(2)?; // color de fondo y proporción: no se usan (el fondo es transparente)
    comprobar(w, h, lim)?;
    let paleta = if emp & 0x80 != 0 { leer_paleta(l, emp & 7)? } else { Vec::new() };
    Ok(Pantalla { w, h, paleta })
}

/// El siguiente fotograma; `None` al llegar al final (`;`).
fn leer_fotograma(l: &mut Lector, pan: &Pantalla, lim: &Limits) -> Result<Option<Fotograma>, ImgError> {
    let (mut transp, mut retardo, mut descarte) = (None, 0, 0);
    loop {
        match l.u8()? {
            0x3B => return Ok(None),
            0x21 => {
                let etiqueta = l.u8()?;
                let datos = l.subbloques()?;
                if etiqueta == 0xF9 && datos.len() >= 4 {
                    descarte = datos[0] >> 2 & 7;
                    retardo = u16::from_le_bytes([datos[1], datos[2]]) as u32;
                    transp = (datos[0] & 1 != 0).then_some(datos[3]);
                }
            }
            0x2C => {
                let (x, y, w, h) = (l.u16()?, l.u16()?, l.u16()?, l.u16()?);
                let emp = l.u8()?;
                let paleta = if emp & 0x80 != 0 { leer_paleta(l, emp & 7)? } else { pan.paleta.clone() };
                let n = comprobar(w, h, lim)?;
                let min = l.u8()?;
                if !(1..=8).contains(&min) {
                    return Err(ImgError::Formato);
                }
                let datos = l.subbloques()?;
                let lineal = lzw(&datos, min, n)?;
                let indices = if emp & 0x40 != 0 { desentrelazar(&lineal, w as usize, h as usize) } else { lineal };
                return Ok(Some(Fotograma { x, y, w, h, indices, paleta, transparente: transp, retardo, descarte }));
            }
            _ => return Err(ImgError::Formato),
        }
    }
}

/// Las filas llegan en cuatro pasadas (cada 8 desde 0, cada 8 desde 4, cada 4 desde 2, cada 2 desde 1).
fn desentrelazar(lin: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; lin.len()];
    let mut fila = 0;
    for (ini, paso) in [(0, 8), (4, 8), (2, 4), (1, 2)] {
        for y in (ini..h).step_by(paso) {
            out[y * w..(y + 1) * w].copy_from_slice(&lin[fila * w..(fila + 1) * w]);
            fila += 1;
        }
    }
    out
}

/// LZW de GIF (códigos de 2 a 12 bits, LSB primero) hasta `n` índices.
fn lzw(datos: &[u8], min: u8, n: usize) -> Result<Vec<u8>, ImgError> {
    let (clear, eoi) = (1u16 << min, (1u16 << min) + 1);
    let mut prefijo = [0u16; 4096];
    let mut sufijo = [0u8; 4096];
    let mut pila = [0u8; 4097];
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let (mut sig, mut tam, mut previo) = (eoi + 1, min as u32 + 1, None::<u16>);
    let (mut acum, mut nacum, mut pos) = (0u32, 0u32, 0usize);
    while out.len() < n {
        while nacum < tam {
            acum |= (*datos.get(pos).ok_or(ImgError::Truncado)? as u32) << nacum;
            pos += 1;
            nacum += 8;
        }
        let cod = (acum & ((1 << tam) - 1)) as u16;
        acum >>= tam;
        nacum -= tam;
        if cod == clear {
            (sig, tam, previo) = (eoi + 1, min as u32 + 1, None);
            continue;
        }
        if cod == eoi {
            break;
        }
        // cadena de `cod` (al revés) en la pila
        let mut cima = 0;
        let mut c = cod;
        let extra = match previo {
            Some(p) if cod == sig && sig < 4096 => {
                c = p; // caso KwKwK: la cadena anterior más su primer carácter
                true
            }
            Some(_) if cod >= sig => return Err(ImgError::Formato),
            None if cod >= clear => return Err(ImgError::Formato),
            _ => false,
        };
        while c >= clear {
            pila[cima] = sufijo[c as usize];
            cima += 1;
            c = prefijo[c as usize];
        }
        pila[cima] = c as u8;
        cima += 1;
        let primero = pila[cima - 1];
        for i in (0..cima).rev() {
            if out.len() < n {
                out.push(pila[i]);
            }
        }
        if extra && out.len() < n {
            out.push(primero);
        }
        if let Some(p) = previo {
            if sig < 4096 {
                prefijo[sig as usize] = p;
                sufijo[sig as usize] = primero;
                sig += 1;
                if sig == 1 << tam && tam < 12 {
                    tam += 1;
                }
            }
        }
        previo = Some(cod);
    }
    if out.len() < n {
        return Err(ImgError::Truncado);
    }
    Ok(out)
}

/// Pinta el fotograma sobre el lienzo (lo que cae fuera se recorta; el índice transparente no pinta).
fn componer(lienzo: &mut [u32], pan: &Pantalla, f: &Fotograma) {
    for j in 0..f.h {
        let cy = f.y + j;
        if cy >= pan.h {
            break;
        }
        for i in 0..f.w {
            let cx = f.x + i;
            if cx >= pan.w {
                break;
            }
            let idx = f.indices[(j * f.w + i) as usize];
            if Some(idx) != f.transparente {
                if let (Some(&c), Some(d)) = (f.paleta.get(idx as usize), lienzo.get_mut((cy * pan.w + cx) as usize)) {
                    *d = c;
                }
            }
        }
    }
}

pub fn decode(data: &[u8], lim: Limits) -> Result<Picture, ImgError> {
    let mut l = Lector { d: data, pos: 0 };
    let pan = cabecera(&mut l, &lim)?;
    let mut lienzo = vec![0u32; pan.w as usize * pan.h as usize];
    // shortcut: solo el primer fotograma; para animar (v0.13) bucle con `leer_fotograma` y respetar `descarte`
    let f = leer_fotograma(&mut l, &pan, &lim)?.ok_or(ImgError::Formato)?;
    componer(&mut lienzo, &pan, &f);
    l.u8()?; // tras el fotograma tiene que seguir algo (otro bloque o el final `;`): si no, el archivo está cortado
    Ok(Picture { w: pan.w as i32, h: pan.h as i32, argb: lienzo })
}
