// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Constructores de imágenes ext de prueba (solo con `cargo test` o la característica `imagenes`).
use alloc::vec;
use alloc::vec::Vec;

pub fn w16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
pub fn w32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
pub fn words(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
/// Inodo en el byte `at` (tiempos 0x6553F100 = 2023-11-14 22:13:20).
pub fn inode(d: &mut [u8], at: usize, mode: u16, flags: u32, size: u32, blk: &[u8]) {
    w16(d, at, mode);
    w32(d, at + 4, size);
    for o in [8, 0xC, 0x10] {
        w32(d, at + o, 0x6553_F100);
    }
    w16(d, at + 0x1A, 1);
    w32(d, at + 0x20, flags);
    d[at + 0x28..at + 0x28 + blk.len()].copy_from_slice(blk);
}
/// Raíz de extents (o nodo) con `max` entradas posibles.
pub fn eroot(depth: u16, max: u16, ents: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0u8; 12];
    w16(&mut b, 0, 0xF30A);
    w16(&mut b, 2, ents.len() as u16);
    w16(&mut b, 4, max);
    w16(&mut b, 6, depth);
    ents.iter().for_each(|e| b.extend_from_slice(e));
    b
}
pub fn leaf(l: u32, len: u16, p: u32) -> Vec<u8> {
    let mut e = vec![0u8; 12];
    w32(&mut e, 0, l);
    w16(&mut e, 4, len);
    w32(&mut e, 8, p);
    e
}
pub fn idx(l: u32, child: u32) -> Vec<u8> {
    let mut e = vec![0u8; 12];
    w32(&mut e, 0, l);
    w32(&mut e, 4, child);
    e
}
/// Bloque de carpeta: la última entrada llega hasta el final.
pub fn dir(d: &mut [u8], at: usize, size: usize, ents: &[(u32, &str)]) {
    let mut p = at;
    for (k, (ino, name)) in ents.iter().enumerate() {
        let rec = if k == ents.len() - 1 { at + size - p } else { (8 + name.len() + 3) & !3 };
        w32(d, p, *ino);
        w16(d, p + 4, rec as u16);
        d[p + 6] = name.len() as u8;
        d[p + 7] = 1;
        d[p + 8..p + 8 + name.len()].copy_from_slice(name.as_bytes());
        p += rec;
    }
}
pub fn fill(d: &mut [u8], blk: usize, v: u8) {
    d[blk * 1024..(blk + 1) * 1024].fill(v);
}

/// Un «Linux» mínimo de 64 KiB (ext2, bloques de 1 KiB) con `/etc/os-release`.
pub fn linux_falso() -> Vec<u8> {
    let mut d = vec![0u8; 64 * 1024];
    let s = 1024;
    for (o, v) in [(0, 16u32), (4, 64), (0xC, 38), (0x10, 2), (0x14, 1), (0x20, 8192), (0x24, 8192), (0x28, 16), (0x4C, 1), (0x54, 11), (0x60, 2)] {
        w32(&mut d, s + o, v);
    }
    for (o, v) in [(0x38, 0xEF53u16), (0x3A, 1), (0x3C, 1), (0x58, 128)] {
        w16(&mut d, s + o, v);
    }
    d[s + 0x78..s + 0x7D].copy_from_slice(b"Linux");
    for (o, v) in [(0, 3u32), (4, 4), (8, 5)] {
        w32(&mut d, 2048 + o, v);
    }
    let at = |n: usize| 5 * 1024 + (n - 1) * 128;
    let txt = b"NAME=\"Linux de prueba\"\n";
    inode(&mut d, at(2), 0x41ED, 0, 1024, &words(&[7]));
    inode(&mut d, at(11), 0x41ED, 0, 1024, &words(&[8]));
    inode(&mut d, at(12), 0x81A4, 0, txt.len() as u32, &words(&[9]));
    dir(&mut d, 7 * 1024, 1024, &[(2, "."), (2, ".."), (11, "etc")]);
    dir(&mut d, 8 * 1024, 1024, &[(11, "."), (2, ".."), (12, "os-release")]);
    d[9 * 1024..9 * 1024 + txt.len()].copy_from_slice(txt);
    d
}

