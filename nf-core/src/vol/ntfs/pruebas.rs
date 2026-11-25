// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Constructores de imágenes NTFS de prueba (solo con `cargo test` o la característica `imagenes`).
use alloc::vec;
use alloc::vec::Vec;

pub const CL: usize = 4096;
/// 2023-11-14 22:13:20 UTC como FILETIME.
pub const FT: u64 = 133_444_736_000_000_000;

pub fn w16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
pub fn w32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
pub fn w64(d: &mut [u8], o: usize, v: u64) {
    d[o..o + 8].copy_from_slice(&v.to_le_bytes());
}
pub fn u16z(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|c| c.to_le_bytes()).collect()
}

/// Fixups: USN 1 y los dos últimos bytes de cada sector de 512 guardados en la matriz.
pub fn fix(b: &mut [u8], off: usize) {
    w16(b, off, 1);
    for i in 1..b.len() / 512 + 1 {
        let e = i * 512;
        (b[off + 2 * i], b[off + 2 * i + 1]) = (b[e - 2], b[e - 1]);
        (b[e - 2], b[e - 1]) = (1, 0);
    }
}

/// Atributo residente.
pub fn res(ty: u32, name: &str, id: u16, val: &[u8]) -> Vec<u8> {
    let nm = u16z(name);
    let voff = (0x18 + nm.len() + 7) & !7;
    let len = (voff + val.len() + 7) & !7;
    let mut a = vec![0u8; len];
    w32(&mut a, 0, ty);
    w32(&mut a, 4, len as u32);
    a[9] = name.encode_utf16().count() as u8;
    w16(&mut a, 0xA, 0x18);
    w16(&mut a, 0xE, id);
    w32(&mut a, 0x10, val.len() as u32);
    w16(&mut a, 0x14, voff as u16);
    a[0x18..0x18 + nm.len()].copy_from_slice(&nm);
    a[voff..voff + val.len()].copy_from_slice(val);
    a
}
/// Atributo no residente (cabecera de 0x48 bytes).
pub fn non(ty: u32, name: &str, id: u16, v0: u64, v1: u64, rl: &[u8], size: u64, flags: u16, unit: u16) -> Vec<u8> {
    let nm = u16z(name);
    let ro = (0x48 + nm.len() + 7) & !7;
    let len = (ro + rl.len() + 7) & !7;
    let mut a = vec![0u8; len];
    w32(&mut a, 0, ty);
    w32(&mut a, 4, len as u32);
    a[8] = 1;
    a[9] = name.encode_utf16().count() as u8;
    w16(&mut a, 0xA, 0x48);
    w16(&mut a, 0xC, flags);
    w16(&mut a, 0xE, id);
    w64(&mut a, 0x10, v0);
    w64(&mut a, 0x18, v1);
    w16(&mut a, 0x20, ro as u16);
    w16(&mut a, 0x22, unit);
    w64(&mut a, 0x28, (v1 + 1 - v0) * CL as u64);
    w64(&mut a, 0x30, size);
    w64(&mut a, 0x38, size);
    a[0x48..0x48 + nm.len()].copy_from_slice(&nm);
    a[ro..ro + rl.len()].copy_from_slice(rl);
    a
}
/// Runlist: (clusters, LCN) con `None` = hueco.
pub fn rl(runs: &[(u64, Option<u64>)]) -> Vec<u8> {
    let (mut o, mut prev) = (Vec::new(), 0i64);
    for &(len, lcn) in runs {
        let n = ((64 - len.leading_zeros()).div_ceil(8)).max(1) as usize;
        let mut delta = Vec::new();
        if let Some(l) = lcn {
            let d = l as i64 - prev;
            prev = l as i64;
            let k = (1..=8usize).find(|&k| (((d as u64) << (64 - 8 * k)) as i64) >> (64 - 8 * k) == d).unwrap();
            delta = d.to_le_bytes()[..k].to_vec();
        }
        o.push((delta.len() << 4 | n) as u8);
        o.extend_from_slice(&len.to_le_bytes()[..n]);
        o.extend(delta);
    }
    o.push(0);
    o
}
/// $STANDARD_INFORMATION de 72 bytes con la fecha de prueba.
pub fn si(flags: u32) -> Vec<u8> {
    let mut v = vec![0u8; 0x48];
    for o in [0, 8, 0x10, 0x18] {
        w64(&mut v, o, FT);
    }
    w32(&mut v, 0x20, flags);
    res(0x10, "", 0, &v)
}
pub fn reparse(tag: u32) -> Vec<u8> {
    let mut v = vec![0u8; 8];
    w32(&mut v, 0, tag);
    res(0xC0, "", 9, &v)
}

pub fn rec_at(n: usize) -> usize {
    4 * CL + n * 1024
}
/// Registro FILE `n` (secuencia 1) con esos atributos.
pub fn put_rec(img: &mut [u8], n: usize, flags: u16, base: u64, attrs: &[Vec<u8>]) {
    let r = &mut img[rec_at(n)..rec_at(n) + 1024];
    r.fill(0);
    r[..4].copy_from_slice(b"FILE");
    w16(r, 4, 0x30);
    w16(r, 6, 3);
    w16(r, 0x10, 1);
    w16(r, 0x14, 0x38);
    w16(r, 0x16, flags);
    w32(r, 0x1C, 1024);
    w64(r, 0x20, base);
    let mut p = 0x38;
    for a in attrs {
        r[p..p + a.len()].copy_from_slice(a);
        p += a.len();
    }
    w32(r, p, u32::MAX);
    w32(r, 0x18, p as u32 + 8);
    fix(r, 0x30);
}

/// $FILE_NAME (clave de índice).
pub fn fname(name: &str, ns: u8) -> Vec<u8> {
    let mut k = vec![0u8; 0x42 + 2 * name.encode_utf16().count()];
    w64(&mut k, 0, 5 | 5 << 48);
    k[0x40] = name.encode_utf16().count() as u8;
    k[0x41] = ns;
    k[0x42..].copy_from_slice(&u16z(name));
    k
}
/// Entrada de índice con secuencia `seq` y, si hay, subnodo.
pub fn ie(rec: u64, seq: u64, name: &str, ns: u8, sub: Option<u64>) -> Vec<u8> {
    let k = fname(name, ns);
    let elen = ((0x10 + k.len() + 7) & !7) + if sub.is_some() { 8 } else { 0 };
    let mut e = vec![0u8; elen];
    w64(&mut e, 0, rec | seq << 48);
    w16(&mut e, 8, elen as u16);
    w16(&mut e, 10, k.len() as u16);
    w16(&mut e, 12, sub.is_some() as u16);
    e[0x10..0x10 + k.len()].copy_from_slice(&k);
    if let Some(v) = sub {
        w64(&mut e, elen - 8, v);
    }
    e
}
pub fn en(rec: u64, name: &str, ns: u8) -> Vec<u8> {
    ie(rec, 1, name, ns, None)
}
/// Entrada final (sin clave), con subnodo si se indica.
pub fn last(sub: Option<u64>) -> Vec<u8> {
    let mut e = vec![0u8; if sub.is_some() { 24 } else { 16 }];
    let l = e.len() as u16;
    w16(&mut e, 8, l);
    w16(&mut e, 12, 2 | sub.is_some() as u16);
    if let Some(v) = sub {
        w64(&mut e, 16, v);
    }
    e
}
pub fn iroot(ents: &[Vec<u8>], child: bool) -> Vec<u8> {
    let body: Vec<u8> = ents.concat();
    let mut v = vec![0u8; 0x20];
    w32(&mut v, 0, 0x30);
    w32(&mut v, 4, 1);
    w32(&mut v, 8, 4096);
    v[0xC] = 1;
    w32(&mut v, 0x10, 0x10);
    w32(&mut v, 0x14, 0x10 + body.len() as u32);
    w32(&mut v, 0x18, 0x10 + body.len() as u32);
    v[0x1C] = child as u8;
    v.extend(body);
    v
}
pub fn indx(vcn: u64, ents: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = ents.concat();
    let mut b = vec![0u8; 4096];
    b[..4].copy_from_slice(b"INDX");
    w16(&mut b, 4, 0x28);
    w16(&mut b, 6, 9);
    w64(&mut b, 0x10, vcn);
    w32(&mut b, 0x18, 0x28);
    w32(&mut b, 0x1C, 0x28 + body.len() as u32);
    w32(&mut b, 0x20, 4096 - 0x18);
    b[0x40..0x40 + body.len()].copy_from_slice(&body);
    fix(&mut b, 0x28);
    b
}
pub fn dir_rec(img: &mut [u8], n: usize, flags: u32, ents: &[Vec<u8>]) {
    put_rec(img, n, 3, 0, &[si(flags), res(0x90, "$I30", 1, &iroot(ents, false))]);
}
pub fn fill(img: &mut [u8], lcn: usize, n: usize, v: u8) {
    img[lcn * CL..(lcn + n) * CL].fill(v);
}

/// Un «Windows» mínimo de 1 MiB (sectores de 512): NTFS con la carpeta `\Windows\System32`.
pub fn windows_falso() -> Vec<u8> {
    let mut d = vec![0u8; 256 * CL];
    let mut b = [0u8; 512];
    b[..3].copy_from_slice(&[0xEB, 0x52, 0x90]);
    b[3..11].copy_from_slice(b"NTFS    ");
    w16(&mut b, 0xB, 512);
    b[0xD] = 8;
    w64(&mut b, 0x28, 2047);
    w64(&mut b, 0x30, 4);
    w64(&mut b, 0x38, 2);
    b[0x40] = 0xF6;
    b[0x44] = 1;
    w64(&mut b, 0x48, 0x1122_3344_5566_7788);
    b[510] = 0x55;
    b[511] = 0xAA;
    d[..512].copy_from_slice(&b);
    d[2047 * 512..].copy_from_slice(&b);
    put_rec(&mut d, 0, 1, 0, &[non(0x80, "", 1, 0, 15, &rl(&[(16, Some(4))]), 64 * 1024, 0, 0)]);
    put_rec(&mut d, 1, 1, 0, &[non(0x80, "", 1, 0, 0, &rl(&[(1, Some(2))]), 4096, 0, 0)]);
    let mut vi = vec![0u8; 12];
    (vi[8], vi[9]) = (3, 1);
    put_rec(&mut d, 3, 1, 0, &[res(0x60, "", 1, &u16z("Windows")), res(0x70, "", 2, &vi)]);
    put_rec(&mut d, 6, 1, 0, &[non(0x80, "", 1, 0, 0, &rl(&[(1, Some(3))]), 32, 0, 0)]);
    d[3 * CL..3 * CL + 3].fill(0xFF);
    dir_rec(&mut d, 24, 0x10, &[en(25, "System32", 3), last(None)]);
    dir_rec(&mut d, 25, 0x10, &[last(None)]);
    dir_rec(&mut d, 5, 0x1000_0000, &[en(24, "Windows", 3), last(None)]);
    d.copy_within(4 * CL..5 * CL, 2 * CL);
    d
}

