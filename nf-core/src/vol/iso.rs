// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! ISO9660 (ECMA-119) de solo lectura, con Joliet (descriptor suplementario UCS-2) preferido y
//! Rock Ridge mínimo (solo nombres largos, entrada NM). La imagen es entrada hostil: todo
//! se valida y las lecturas se acotan al tamaño del volumen.
use super::fat::{same_name, split, Io};
use super::part::{u16le, u32le};
use super::{FsKind, Partition, SectorDev, Volume};
use crate::fs::{Entry, FsError};
use crate::platform::DateTime;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// Tamaño máximo de una carpeta que se carga entera.
const MAXDIR: u64 = 1 << 24;

/// Lee `len` bytes desde el byte `off` de la partición (sin alinear a sectores). Compartido con UDF.
pub(super) fn read_at(io: &mut Io, off: u64, len: usize) -> Result<Vec<u8>, FsError> {
    if len == 0 {
        return Ok(Vec::new());
    }
    let ss = io.ss as u64;
    let end = off.checked_add(len as u64).ok_or(FsError::Corrupt)?;
    let first = off / ss;
    let mut b = io.read(first, ((end + ss - 1) / ss - first) as usize)?;
    b.drain(..(off - first * ss) as usize);
    b.truncate(len);
    Ok(b)
}

/// Fecha de 7 bytes (años desde 1900); la zona horaria se descarta: se muestra la hora tal cual.
fn date7(b: &[u8]) -> DateTime {
    if b.iter().all(|&x| x == 0) {
        return DateTime::default();
    }
    DateTime { year: 1900 + b[0] as u16, month: b[1].clamp(1, 12), day: b[2].clamp(1, 31), hour: b[3].min(23), minute: b[4].min(59), second: b[5].min(59) }
}

/// Texto UCS-2 big-endian (Joliet).
fn ucs2(b: &[u8]) -> Result<String, FsError> {
    if b.len() % 2 != 0 {
        return Err(FsError::Corrupt);
    }
    Ok(char::decode_utf16(b.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]]))).map(|r| r.unwrap_or('\u{FFFD}')).collect())
}

#[derive(Clone)]
struct Rec {
    name: String,
    dir: bool,
    size: u64,
    date: DateTime,
    /// Extensiones (primer sector lógico, bytes). Una carpeta tiene una sola.
    ext: Vec<(u32, u32)>,
}

/// Interpreta un registro de directorio (`r` es exactamente el registro). Devuelve también si
/// continúa en otro registro (multi-extent). Nombre vacío = "." o "..".
fn record(r: &[u8], joliet: bool) -> Result<(Rec, bool), FsError> {
    if r.len() < 34 {
        return Err(FsError::Corrupt);
    }
    let n = r[32] as usize;
    if n == 0 || 33 + n > r.len() {
        return Err(FsError::Corrupt);
    }
    let raw = &r[33..33 + n];
    let flags = r[25];
    let dir = flags & 2 != 0;
    let len = u32le(r, 10);
    let name = if n == 1 && raw[0] < 2 {
        String::new()
    } else if joliet {
        let s = ucs2(raw)?;
        clean(s, dir)
    } else {
        // Rock Ridge: el área de uso del sistema sigue al nombre (con un relleno si n es par).
        let mut nm = Vec::new();
        let mut o = 33 + n + (1 - n % 2);
        while o + 5 <= r.len() {
            let l = r[o + 2] as usize;
            if l < 4 || o + l > r.len() {
                break;
            }
            // flags: 1 = continúa, 2 = "." y 4 = ".." (esos no son nombres)
            if &r[o..o + 2] == b"NM" && l >= 5 && r[o + 4] & 6 == 0 {
                nm.extend_from_slice(&r[o + 5..o + l]);
            }
            o += l;
        }
        if nm.is_empty() {
            clean(raw.iter().map(|&c| c as char).collect(), dir)
        } else {
            String::from_utf8_lossy(&nm).into_owned()
        }
    };
    let rec = Rec { name, dir, size: if dir { 0 } else { len as u64 }, date: date7(&r[18..25]), ext: alloc::vec![(u32le(r, 2), len)] };
    Ok((rec, flags & 0x80 != 0 && !dir))
}

/// Quita ";versión" y el punto final de los nombres de archivo ISO.
fn clean(mut s: String, dir: bool) -> String {
    if !dir {
        if let Some(i) = s.rfind(';') {
            s.truncate(i);
        }
        if s.ends_with('.') && s.len() > 1 {
            s.pop();
        }
    }
    s
}

struct Iso {
    io: Io,
    /// Tamaño del bloque lógico y del volumen (bytes).
    bs: u64,
    vol: u64,
    joliet: bool,
    label: String,
    root: Rec,
}

pub fn mount(dev: Box<dyn SectorDev>, p: &Partition) -> Result<Box<dyn Volume>, FsError> {
    let ss = dev.sector_size();
    let mut io = Io { dev, start: p.start, len: p.sectors, ss };
    let (mut prim, mut sup) = (None, None);
    // Descriptores de 2048 bytes desde el 16 hasta el terminador (255), con tope.
    for i in 0..64u64 {
        let d = read_at(&mut io, 32768 + i * 2048, 2048)?;
        if &d[1..6] != b"CD001" || d[6] != 1 {
            break;
        }
        match d[0] {
            1 if prim.is_none() => prim = Some(d),
            // Secuencias de escape de Joliet: %/@ (nivel 1), %/C (2) y %/E (3).
            2 if sup.is_none() && matches!(&d[88..91], b"%/@" | b"%/C" | b"%/E") => sup = Some(d),
            255 => break,
            _ => {}
        }
    }
    let joliet = sup.is_some();
    let v = sup.or(prim).ok_or(FsError::Corrupt)?;
    let bs = u16le(&v, 128) as u64;
    if !matches!(bs, 512 | 1024 | 2048 | 4096) {
        return Err(FsError::Corrupt);
    }
    let vol = (u32le(&v, 80) as u64 * bs).min(io.len * ss as u64);
    let label = if joliet { ucs2(&v[40..72])? } else { v[40..72].iter().map(|&c| c as char).collect() };
    let (root, _) = record(&v[156..190], joliet)?;
    let (lba, len) = root.ext[0];
    if !root.dir || len == 0 || len as u64 > MAXDIR || lba as u64 * bs + len as u64 > vol {
        return Err(FsError::Corrupt);
    }
    Ok(Box::new(Iso { io, bs, vol, joliet, label: label.trim_end_matches([' ', '\0']).into(), root }))
}

impl Iso {
    fn entries(&mut self, d: &Rec) -> Result<Vec<Rec>, FsError> {
        if !d.dir {
            return Err(FsError::NotDir);
        }
        let (lba, len) = d.ext[0];
        let off = lba as u64 * self.bs;
        if len == 0 || len as u64 > MAXDIR || off + len as u64 > self.vol {
            return Err(FsError::Corrupt);
        }
        let b = read_at(&mut self.io, off, len as usize)?;
        let bs = self.bs as usize;
        let (mut out, mut pend, mut o): (Vec<Rec>, Option<Rec>, usize) = (Vec::new(), None, 0);
        while o < b.len() {
            let l = b[o] as usize;
            if l == 0 {
                o = (o / bs + 1) * bs; // los registros no cruzan sectores: el resto del bloque es relleno
                continue;
            }
            if o + l > b.len() || o % bs + l > bs {
                return Err(FsError::Corrupt);
            }
            let (r, more) = record(&b[o..o + l], self.joliet)?;
            o += l;
            if r.name.is_empty() {
                continue;
            }
            // Archivos grandes: varios registros seguidos con el mismo nombre.
            let cur = match pend.take() {
                Some(mut p) if p.name == r.name => {
                    p.size = p.size.checked_add(r.size).ok_or(FsError::Corrupt)?;
                    p.ext.extend_from_slice(&r.ext);
                    p
                }
                Some(p) => {
                    out.push(p);
                    r
                }
                None => r,
            };
            if more {
                pend = Some(cur);
            } else {
                out.push(cur);
            }
        }
        out.extend(pend);
        Ok(out)
    }

    fn walk(&mut self, path: &str) -> Result<Rec, FsError> {
        let mut cur = self.root.clone();
        for c in split(path) {
            cur = self.entries(&cur)?.into_iter().find(|e| same_name(&e.name, c)).ok_or(FsError::NotFound)?;
        }
        Ok(cur)
    }
}

fn entry_of(r: &Rec) -> Entry {
    Entry { name: r.name.clone(), dir: r.dir, size: r.size, modified: r.date }
}

impl Volume for Iso {
    fn kind(&self) -> FsKind {
        FsKind::Iso9660
    }
    fn label(&self) -> String {
        self.label.clone()
    }
    fn space(&mut self) -> (u64, u64) {
        (self.vol, 0)
    }
    fn read_only(&self) -> bool {
        true
    }
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, FsError> {
        let d = self.walk(dir)?;
        Ok(self.entries(&d)?.iter().map(entry_of).collect())
    }
    fn stat(&mut self, path: &str) -> Result<Entry, FsError> {
        Ok(entry_of(&self.walk(path)?))
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        let r = self.walk(path)?;
        if r.dir {
            return Err(FsError::IsDir);
        }
        if r.size > self.vol {
            return Err(FsError::Corrupt);
        }
        let mut out = Vec::new();
        out.try_reserve_exact(r.size as usize).map_err(|_| FsError::NoSpace)?;
        for &(lba, len) in &r.ext {
            let off = lba as u64 * self.bs;
            if off + len as u64 > self.vol {
                return Err(FsError::Corrupt);
            }
            out.extend(read_at(&mut self.io, off, len as usize)?);
        }
        Ok(out)
    }
    fn write(&mut self, _: &str, _: &[u8], _: DateTime) -> Result<(), FsError> {
        Err(FsError::Denied)
    }
    fn mkdir(&mut self, _: &str, _: DateTime) -> Result<(), FsError> {
        Err(FsError::Denied)
    }
    fn rename(&mut self, _: &str, _: &str) -> Result<(), FsError> {
        Err(FsError::Denied)
    }
    fn delete(&mut self, _: &str) -> Result<(), FsError> {
        Err(FsError::Denied)
    }
    fn flush(&mut self) -> Result<(), FsError> {
        Ok(())
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::vol::{partitions, MemSectors};
    use alloc::vec;

    const BS: usize = 2048;

    fn rec(name: &[u8], lba: u32, len: u32, flags: u8, su: &[u8]) -> Vec<u8> {
        let mut l = 33 + name.len() + su.len();
        let pad = name.len() % 2 == 0;
        l += pad as usize;
        let mut r = vec![0u8; l];
        r[0] = l as u8;
        r[2..6].copy_from_slice(&lba.to_le_bytes());
        r[10..14].copy_from_slice(&len.to_le_bytes());
        r[18..25].copy_from_slice(&[126, 11, 18, 10, 30, 45, 4]); // 2026-11-18 10:30:45
        r[25] = flags;
        r[32] = name.len() as u8;
        r[33..33 + name.len()].copy_from_slice(name);
        let at = 33 + name.len() + pad as usize;
        r[at..].copy_from_slice(su);
        r
    }

    fn put(d: &mut [u8], block: usize, b: &[u8]) {
        d[block * BS..(block + 1) * BS].fill(0);
        d[block * BS..block * BS + b.len()].copy_from_slice(b);
    }

    fn dir(me: u32, parent: u32, kids: &[Vec<u8>]) -> Vec<u8> {
        let mut v = rec(&[0], me, BS as u32, 2, &[]);
        v.extend(rec(&[1], parent, BS as u32, 2, &[]));
        for k in kids {
            v.extend(k);
        }
        v
    }

    fn vd(kind: u8, label: &[u8], root: &[u8], escape: &[u8]) -> Vec<u8> {
        let mut v = vec![0u8; BS];
        v[0] = kind;
        v[1..6].copy_from_slice(b"CD001");
        v[6] = 1;
        v[88..88 + escape.len()].copy_from_slice(escape);
        v[40..40 + label.len()].copy_from_slice(label);
        v[80..84].copy_from_slice(&64u32.to_le_bytes());
        v[128..130].copy_from_slice(&2048u16.to_le_bytes());
        v[156..156 + root.len()].copy_from_slice(root);
        v
    }

    fn u16be(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
    }

    /// Imagen de 64 bloques. Datos: HELLO (5000 bytes) en 30..32, BIG en dos extensiones
    /// (34: 2048 bytes, 36: 100 bytes), NOTA (10 bytes) en 38.
    fn image(joliet: bool, rr: bool) -> Vec<u8> {
        let mut d = vec![0u8; 64 * BS];
        for i in 0..3 {
            d[30 * BS + i * BS..30 * BS + i * BS + BS].fill(b'a' + i as u8);
        }
        d[30 * BS..30 * BS + 5000].iter_mut().enumerate().for_each(|(i, b)| *b = (i % 251) as u8);
        d[34 * BS..36 * BS].fill(7);
        d[36 * BS..36 * BS + 100].fill(9);
        d[38 * BS..38 * BS + 10].copy_from_slice(b"hola nota!");
        let nm = |s: &str| {
            let mut v = vec![b'N', b'M', 5 + s.len() as u8, 1, 0];
            v.extend(s.as_bytes());
            v
        };
        let (root, sub) = (20u32, 21u32);
        let prim_root = dir(root, root, &[
            rec(b"HELLO.TXT;1", 30, 5000, 0, &if rr { nm("Hola largo.txt") } else { Vec::new() }),
            rec(b"BIG.", 34, 2048, 0x80, &[]),
            rec(b"BIG.", 36, 100, 0, &[]),
            rec(b"DOCS", sub, BS as u32, 2, &[]),
        ]);
        let prim_sub = dir(sub, root, &[rec(b"NOTA.TXT;1", 38, 10, 0, &[])]);
        put(&mut d, 20, &prim_root);
        put(&mut d, 21, &prim_sub);
        let rr_rec = rec(&[0], root, BS as u32, 2, &[]);
        put(&mut d, 16, &vd(1, b"MI_DISCO        ", &rr_rec, &[]));
        if joliet {
            let (jroot, jsub) = (22u32, 23u32);
            let jr = dir(jroot, jroot, &[
                rec(&u16be("Año nuevo.txt;1"), 30, 5000, 0, &[]),
                rec(&u16be("big.bin"), 34, 2048, 0x80, &[]),
                rec(&u16be("big.bin"), 36, 100, 0, &[]),
                rec(&u16be("Documentación"), jsub, BS as u32, 2, &[]),
            ]);
            let js = dir(jsub, jroot, &[rec(&u16be("Ñandú.txt"), 38, 10, 0, &[])]);
            put(&mut d, 22, &jr);
            put(&mut d, 23, &js);
            let jroot_rec = rec(&[0], jroot, BS as u32, 2, &[]);
            put(&mut d, 17, &vd(2, &u16be("Disco Ñ"), &jroot_rec, b"%/E"));
            put(&mut d, 18, &vd(255, &[], &[], &[]));
        } else {
            put(&mut d, 17, &vd(255, &[], &[], &[]));
        }
        d
    }

    fn mount_img(data: Vec<u8>, ss: usize) -> Result<Box<dyn Volume>, FsError> {
        let mut m = MemSectors::new(0, ss);
        m.data = data;
        let p = partitions(&mut m)?;
        let p = p.into_iter().find(|p| p.kind == FsKind::Iso9660).ok_or(FsError::Corrupt)?;
        super::mount(Box::new(m), &p)
    }

    fn check_tree(v: &mut dyn Volume, hello: &str, big: &str, docs: &str, nota: &str) {
        let l = v.list("").unwrap();
        assert_eq!(l.len(), 3, "{:?}", l);
        let h = v.stat(hello).unwrap();
        assert_eq!((h.size, h.dir), (5000, false));
        assert_eq!((h.modified.year, h.modified.month, h.modified.day, h.modified.hour, h.modified.second), (2026, 11, 18, 10, 45));
        let data = v.read(hello).unwrap();
        assert_eq!(data.len(), 5000);
        assert_eq!(data[4999], (4999 % 251) as u8);
        let b = v.read(big).unwrap();
        assert_eq!((b.len(), b[0], b[2047], b[2048], b[2147]), (2148, 7, 7, 9, 9));
        assert_eq!(v.stat(docs).unwrap().dir, true);
        assert_eq!(v.read(nota).unwrap(), b"hola nota!");
        assert_eq!(v.read(docs), Err(FsError::IsDir));
        assert_eq!(v.read("nada"), Err(FsError::NotFound));
        assert_eq!(v.list(hello), Err(FsError::NotDir));
    }

    #[test]
    fn primary_only() {
        let mut v = mount_img(image(false, false), 2048).unwrap();
        assert_eq!((v.kind(), v.label(), v.read_only()), (FsKind::Iso9660, "MI_DISCO".into(), true));
        assert_eq!(v.space(), (64 * 2048, 0));
        check_tree(&mut *v, "hello.txt", "big", "docs", "Docs\\nota.TXT");
        assert_eq!(v.write("a", b"", DateTime::default()), Err(FsError::Denied));
        assert_eq!(v.mkdir("a", DateTime::default()), Err(FsError::Denied));
        assert_eq!(v.rename("a", "b"), Err(FsError::Denied));
        assert_eq!(v.delete("a"), Err(FsError::Denied));
        assert_eq!(v.flush(), Ok(()));
    }

    #[test]
    fn rock_ridge_name() {
        let mut v = mount_img(image(false, true), 2048).unwrap();
        assert!(v.list("").unwrap().iter().any(|e| e.name == "Hola largo.txt"));
    }

    #[test]
    fn joliet_and_sector_sizes() {
        for ss in [2048usize, 512] {
            let mut v = mount_img(image(true, false), ss).unwrap();
            assert_eq!(v.label(), "Disco Ñ");
            check_tree(&mut *v, "año NUEVO.txt", "BIG.BIN", "documentación", "documentación\\ñandú.TXT");
        }
    }

    #[test]
    fn hostile() {
        // Una carpeta que se apunta a sí misma: las rutas son finitas, no hay bucle.
        let mut d = image(false, false);
        let r = dir(20, 20, &[rec(b"A", 20, BS as u32, 2, &[])]);
        put(&mut d, 20, &r);
        let mut v = mount_img(d, 2048).unwrap();
        assert!(v.stat(r"a\a\a\a\a").unwrap().dir);
        // Registro demasiado corto, longitud de carpeta enorme, extensión fuera del volumen.
        for edit in 0..3 {
            let mut d = image(false, false);
            match edit {
                0 => d[20 * BS + 68] = 10,
                1 => d[16 * BS + 156 + 10..16 * BS + 156 + 14].copy_from_slice(&u32::MAX.to_le_bytes()),
                _ => d[16 * BS + 156 + 2..16 * BS + 156 + 6].copy_from_slice(&9999u32.to_le_bytes()),
            }
            let res = mount_img(d, 2048).and_then(|mut v| v.list("").map(|_| ()));
            assert!(matches!(res, Err(FsError::Corrupt)), "edit {edit}: {res:?}");
        }
        // Archivo con tamaño mayor que el volumen: Corrupt, sin reservar.
        let mut d = image(false, false);
        let r = dir(20, 20, &[rec(b"F", 30, 0xF000_0000, 0, &[])]);
        put(&mut d, 20, &r);
        let mut v = mount_img(d, 2048).unwrap();
        assert_eq!(v.read("f"), Err(FsError::Corrupt));
        // Carpeta de longitud 0.
        let mut d = image(false, false);
        let r = dir(20, 20, &[rec(b"Z", 21, 0, 2, &[])]);
        put(&mut d, 20, &r);
        let mut v = mount_img(d, 2048).unwrap();
        assert_eq!(v.list("z"), Err(FsError::Corrupt));
    }

    #[test]
    fn fuzz_no_panic() {
        for joliet in [false, true] {
            let base = image(joliet, true);
            for blk in [16usize, 17, 18, 20, 21, 22, 23] {
                for i in 0..160 {
                    for x in [0xFFu8, 0x00, 0x7F] {
                        let mut d = base.clone();
                        d[blk * BS + i] = x;
                        if let Ok(mut v) = mount_img(d, 2048) {
                            for p in ["", "docs", "documentación", "hello.txt", "big", "docs\\nota.txt"] {
                                let _ = v.list(p);
                                let _ = v.stat(p);
                                let _ = v.read(p);
                            }
                        }
                    }
                }
            }
        }
    }

    /// ISO real opcional: `NF_ISO=ruta cargo test -p nf-core --lib iso_real -- --ignored`.
    #[test]
    #[ignore]
    fn iso_real() {
        extern crate std;
        let Ok(path) = std::env::var("NF_ISO") else { return };
        let mut m = MemSectors::new(0, 2048);
        m.data = std::fs::read(path).unwrap();
        let p = partitions(&mut m).unwrap();
        let mut v = super::mount(Box::new(m), &p[0]).unwrap();
        let l = v.list("").unwrap();
        assert!(!l.is_empty());
        std::println!("{} {:?}", v.label(), l.iter().map(|e| e.name.clone()).collect::<Vec<_>>());
    }
}
