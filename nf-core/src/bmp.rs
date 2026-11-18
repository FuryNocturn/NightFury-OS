// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! BMP: codificador (24 bits) para guardar superficies, y lector de iconos de programa (v0.9).
use crate::surface::{blend, Surface};
use alloc::vec;
use alloc::vec::Vec;

/// Imagen pequeña con transparencia (iconos de programa, `icono.bmp`). Píxeles 0xAARRGGBB.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Picture {
    pub w: i32,
    pub h: i32,
    pub argb: Vec<u32>,
}

/// Lado máximo de un icono que se acepta.
pub const PICTURE_MAX: i32 = 256;

impl Picture {
    /// Dibuja la imagen escalada a `size`×`size` (vecino más próximo) con su transparencia.
    pub fn draw(&self, s: &mut Surface, x: i32, y: i32, size: i32) {
        let clip = s.clip();
        for dy in 0..size {
            for dx in 0..size {
                let (px, py) = (x + dx, y + dy);
                if !clip.contains(px, py) {
                    continue;
                }
                let c = self.argb[((dy * self.h / size) * self.w + dx * self.w / size) as usize];
                let a = c >> 24;
                if a == 0 {
                    continue;
                }
                let i = (py * s.w + px) as usize;
                s.px[i] = if a == 255 { c & 0xFF_FFFF } else { blend(s.px[i], c & 0xFF_FFFF, a) };
            }
        }
    }
}

/// Lee un BMP sin comprimir de 24 o 32 bits (como los que guarda Paint) de hasta `PICTURE_MAX`.
/// En 24 bits el magenta puro (FF00FF) es transparente; en 32 bits manda el canal alfa (si es todo
/// cero, la imagen se toma opaca). `None` si no es un BMP que se entienda.
pub fn decode(b: &[u8]) -> Option<Picture> {
    let u16_at = |i: usize| Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]));
    let u32_at = |i: usize| Some(u32::from_le_bytes([*b.get(i)?, *b.get(i + 1)?, *b.get(i + 2)?, *b.get(i + 3)?]));
    if b.get(0..2)? != b"BM" {
        return None;
    }
    let off = u32_at(10)? as usize;
    let w = u32_at(18)? as i32;
    let raw_h = u32_at(22)? as i32;
    let bpp = u16_at(28)?;
    let comp = u32_at(30)?;
    let h = raw_h.checked_abs()?;
    if !(1..=PICTURE_MAX).contains(&w) || !(1..=PICTURE_MAX).contains(&h) || u16_at(26)? != 1 {
        return None;
    }
    // 0 = sin comprimir; 3 = máscaras de bits, que en 32 bits Paint usa con el orden habitual BGRA.
    if !matches!((bpp, comp), (24, 0) | (32, 0) | (32, 3)) {
        return None;
    }
    let bytes = bpp as usize / 8;
    let row = (w as usize * bytes + 3) & !3;
    let data = b.get(off..off.checked_add(row * h as usize)?)?;
    let mut argb = vec![0u32; (w * h) as usize];
    let mut any_alpha = false;
    for y in 0..h as usize {
        // Positivo = de abajo arriba.
        let src = if raw_h > 0 { h as usize - 1 - y } else { y };
        for x in 0..w as usize {
            let p = &data[src * row + x * bytes..][..bytes];
            let rgb = (p[2] as u32) << 16 | (p[1] as u32) << 8 | p[0] as u32;
            let a = if bytes == 4 { p[3] as u32 } else if rgb == 0xFF00FF { 0 } else { 255 };
            any_alpha |= bytes == 4 && a != 0;
            argb[y * w as usize + x] = a << 24 | rgb;
        }
    }
    if bytes == 4 && !any_alpha {
        argb.iter_mut().for_each(|c| *c |= 0xFF00_0000);
    }
    Some(Picture { w, h, argb })
}

pub fn encode(s: &Surface) -> Vec<u8> {
    let row = ((s.w * 3 + 3) & !3) as usize;
    let size = 54 + row * s.h as usize;
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&s.w.to_le_bytes());
    out.extend_from_slice(&s.h.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]);
    for y in (0..s.h).rev() {
        let mut n = 0;
        for x in 0..s.w {
            let p = s.px[(y * s.w + x) as usize];
            out.extend_from_slice(&[p as u8, (p >> 8) as u8, (p >> 16) as u8]);
            n += 3;
        }
        while n < row {
            out.push(0);
            n += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn size_and_header() {
        let s = crate::surface::Surface::new(3, 2, 0x112233);
        let b = super::encode(&s);
        assert_eq!(b.len(), 54 + 12 * 2);
        assert_eq!(&b[0..2], b"BM");
        assert_eq!(&b[54..57], &[0x33, 0x22, 0x11]);
    }

    #[test]
    fn icono_de_ida_y_vuelta() {
        let mut s = crate::surface::Surface::new(3, 2, 0x112233);
        s.px[0] = 0xFF00FF;
        s.px[5] = 0xABCDEF;
        let p = super::decode(&super::encode(&s)).unwrap();
        assert_eq!((p.w, p.h), (3, 2));
        assert_eq!(p.argb[0] >> 24, 0, "magenta = transparente");
        assert_eq!(p.argb[1], 0xFF11_2233);
        assert_eq!(p.argb[5], 0xFFAB_CDEF, "filas en su orden");
        // Al dibujar, lo transparente deja ver el fondo.
        let mut dst = crate::surface::Surface::new(6, 4, 0x000000);
        p.draw(&mut dst, 0, 0, 3);
        assert_eq!(dst.px[0], 0x000000);
        assert_eq!(dst.px[1], 0x112233);
    }

    #[test]
    fn icono_roto_o_raro() {
        let s = crate::surface::Surface::new(4, 4, 0);
        let b = super::encode(&s);
        assert!(super::decode(&b[..60]).is_none(), "datos cortados");
        assert!(super::decode(b"XX").is_none());
        let mut big = b.clone();
        big[18..22].copy_from_slice(&1000u32.to_le_bytes());
        assert!(super::decode(&big).is_none(), "demasiado grande");
        let mut rle = b.clone();
        rle[30] = 1;
        assert!(super::decode(&rle).is_none(), "comprimido");
        let mut neg = b;
        neg[22..26].copy_from_slice(&(-4i32).to_le_bytes());
        assert!(super::decode(&neg).is_some(), "de arriba abajo");
    }
}
