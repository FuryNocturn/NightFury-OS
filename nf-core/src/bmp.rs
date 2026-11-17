// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Codificador BMP (24 bits) para volcar una superficie a un archivo en las vistas previas del PC.
use crate::surface::Surface;
use alloc::vec::Vec;

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
}
