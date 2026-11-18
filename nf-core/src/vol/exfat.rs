// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! exFAT según la especificación publicada por Microsoft (2019): región de arranque con suma de
//! comprobación, mapa de bits de asignación, tabla de mayúsculas, conjuntos de entradas
//! File/Stream/Name con su suma y su hash, archivos contiguos (NoFatChain) y fragmentados (FAT).
//! Los datos del disco son entrada hostil: todo se valida y las cadenas llevan tope.
//!
//! Escritura segura: datos, mapa de bits y FAT primero; la entrada de directorio al final. Al
//! borrar, la entrada primero y luego los clusters.
use super::fat::{pack_time, read_cl, split, unpack_time, upper, valid_name, write_cl, zero_sectors, Io, Table};
use super::part::{self, u16le, u32le, u64le};
use super::{FsKind, Partition, SectorDev, Volume};
use crate::fs::{Entry, FsError};
use crate::platform::DateTime;
use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Tamaño máximo que se carga de un directorio.
/// shortcut: 8 MiB (~260 000 entradas); la especificación admite 256 MiB, subir si hiciera falta.
const MAXDIR: usize = 8 << 20;
const EOC: u32 = 0xFFFF_FFFF;

// --- sumas de la especificación ---

/// Suma de 32 bits (rotación a la derecha y suma), con bytes saltados opcionales.
fn sum32(data: &[u8], skip: &[usize]) -> u32 {
    let mut c = 0u32;
    for (i, &b) in data.iter().enumerate() {
        if !skip.contains(&i) {
            c = c.rotate_right(1).wrapping_add(b as u32);
        }
    }
    c
}

fn sum16(data: impl Iterator<Item = u8>, skip: &[usize]) -> u16 {
    let mut c = 0u16;
    for (i, b) in data.enumerate() {
        if !skip.contains(&i) {
            c = c.rotate_right(1).wrapping_add(b as u16);
        }
    }
    c
}

/// Lee la región de arranque en `at` (principal 0, respaldo 12) y comprueba su suma. Devuelve el sector 0.
fn read_boot(io: &mut Io, at: u64) -> Result<Vec<u8>, FsError> {
    let b = io.read(at, 12)?;
    let ss = io.ss;
    let sum = sum32(&b[..11 * ss], &[106, 107, 112]);
    if b[11 * ss..].chunks_exact(4).all(|w| u32le(w, 0) == sum) {
        Ok(b[..ss].to_vec())
    } else {
        Err(FsError::Corrupt)
    }
}

// --- directorios ---

/// Directorio: dónde empieza, si es contiguo (sin FAT), su longitud y dónde está su conjunto de
/// entradas en el directorio padre (la raíz no tiene).
#[derive(Clone, Copy)]
struct Dir {
    first: u32,
    nofat: bool,
    len: u64,
    at: usize,
    root: bool,
}

struct DirBuf {
    data: Vec<u8>,
    orig: Vec<u8>,
    cl: Vec<u32>,
}

#[derive(Clone)]
struct Item {
    name: String,
    /// Índice de la primera entrada del conjunto y número de entradas.
    at: usize,
    n: usize,
    /// El conjunto de entradas entero, tal cual está en disco.
    set: Vec<u8>,
}

impl Item {
    fn attr(&self) -> u16 {
        u16le(&self.set, 4)
    }
    fn dir(&self) -> bool {
        self.attr() & 0x10 != 0
    }
    fn nofat(&self) -> bool {
        self.set[33] & 2 != 0
    }
    fn first(&self) -> u32 {
        u32le(&self.set, 52)
    }
    fn size(&self) -> u64 {
        u64le(&self.set, 56)
    }
    fn valid(&self) -> u64 {
        u64le(&self.set, 40)
    }
}

/// Primer hueco de `k` entradas libres seguidas (lo que hay tras el marcador de fin cuenta libre).
fn find_run(data: &[u8], k: usize) -> Option<usize> {
    let total = data.len() / 32;
    let end = (0..total).find(|&i| data[i * 32] == 0).unwrap_or(total);
    let mut run = 0;
    for i in 0..total {
        if i >= end || data[i * 32] & 0x80 == 0 {
            run += 1;
            if run == k {
                return Some(i + 1 - k);
            }
        } else {
            run = 0;
        }
    }
    None
}

struct Geo {
    spc: u64,
    cs: usize,
    heap: u64,
    count: u32,
    root: u32,
}

struct Ex {
    io: Io,
    t: Table,
    g: Geo,
    /// Tabla de mayúsculas expandida (índice = unidad UTF-16).
    up: Vec<u16>,
    /// Mapa de bits de asignación (bit n = cluster n+2), en sectores enteros.
    bm: Vec<u8>,
    bm_cl: Vec<u32>,
    bm_dirty: BTreeSet<usize>,
    free: u32,
    flags: u16,
    dirty: bool,
    ro: bool,
    label: String,
}

pub fn mount(dev: Box<dyn SectorDev>, p: &Partition) -> Result<Box<dyn Volume>, FsError> {
    let ss = dev.sector_size();
    let ro = dev.read_only();
    let mut io = Io { dev, start: p.start, len: p.sectors, ss };
    let b = match read_boot(&mut io, 0) {
        Ok(b) => b,
        Err(_) => read_boot(&mut io, 12)?,
    };
    let (vol_len, fat_off, fat_len, heap) = (u64le(&b, 72), u32le(&b, 80) as u64, u32le(&b, 84) as u64, u32le(&b, 88) as u64);
    let (count, root) = (u32le(&b, 92), u32le(&b, 96));
    let (shift, sshift) = (b[108] as u32, b[109] as u32);
    if &b[3..11] != b"EXFAT   " || b[11..64].iter().any(|&x| x != 0) || b[510] != 0x55 || b[511] != 0xAA || b[105] != 1 {
        return Err(FsError::Corrupt);
    }
    if shift > 12 || 1usize << shift != ss || sshift > 25 - shift || b[110] != 1 {
        return Err(FsError::Corrupt);
    }
    let spc = 1u64 << sshift;
    if vol_len > p.sectors || fat_off < 24 || fat_off + fat_len > heap || count == 0 || count > 0xFFFF_FFF5 {
        return Err(FsError::Corrupt);
    }
    if (count as u64 + 2) * 4 > fat_len * ss as u64 || heap + count as u64 * spc > vol_len || root < 2 || root > count + 1 {
        return Err(FsError::Corrupt);
    }
    let g = Geo { spc, cs: spc as usize * ss, heap, count, root };
    let t = Table::new(fat_off, vec![fat_off], fat_len);
    let mut v = Ex { io, t, g, up: Vec::new(), bm: Vec::new(), bm_cl: Vec::new(), bm_dirty: BTreeSet::new(), free: 0, flags: u16le(&b, 106), dirty: false, ro, label: String::new() };

    // La raíz lleva el mapa de bits (0x81), la tabla de mayúsculas (0x82) y la etiqueta (0x83).
    let rd = v.root_dir();
    let rb = v.load(rd)?;
    let (mut bmap, mut upc) = (None, None);
    for e in rb.data.chunks_exact(32) {
        match e[0] {
            0 => break,
            0x81 if e[1] & 1 == 0 => bmap = Some((u32le(e, 20), u64le(e, 24))),
            0x82 => upc = Some((u32le(e, 4), u32le(e, 20), u64le(e, 24))),
            0x83 if e[1] <= 11 => {
                let u: Vec<u16> = (0..e[1] as usize).map(|i| u16le(e, 2 + 2 * i)).collect();
                v.label = char::decode_utf16(u).map(|r| r.unwrap_or('\u{FFFD}')).collect();
            }
            _ => {}
        }
    }
    let (bfirst, blen) = bmap.ok_or(FsError::Corrupt)?;
    let need = (count as u64).div_ceil(8);
    if blen < need || need > 32 << 20 {
        return Err(FsError::Corrupt);
    }
    v.bm_cl = v.chain_of(bfirst, false, blen)?;
    let mut bm = v.read_cl(&v.bm_cl.clone())?;
    bm.truncate(need.div_ceil(ss as u64) as usize * ss);
    v.bm = bm;
    let used: u32 = (0..count as usize).filter(|&i| v.bm[i / 8] >> (i % 8) & 1 == 1).count() as u32;
    v.free = count - used;

    let (usum, ufirst, ulen) = upc.ok_or(FsError::Corrupt)?;
    if ulen == 0 || ulen > 1 << 18 || ulen % 2 != 0 {
        return Err(FsError::Corrupt);
    }
    let cl = v.chain_of(ufirst, false, ulen)?;
    let mut raw = v.read_cl(&cl)?;
    raw.truncate(ulen as usize);
    if sum32(&raw, &[]) != usum {
        return Err(FsError::Corrupt);
    }
    // Descomprime: 0xFFFF seguido de n significa "los n siguientes se quedan igual".
    let words: Vec<u16> = raw.chunks_exact(2).map(|w| u16le(w, 0)).collect();
    let mut w = 0;
    while w < words.len() {
        if words[w] == 0xFFFF {
            let n = *words.get(w + 1).ok_or(FsError::Corrupt)? as usize;
            for _ in 0..n {
                v.up.push(v.up.len() as u16);
            }
            w += 2;
        } else {
            v.up.push(words[w]);
            w += 1;
        }
        if v.up.len() > 65536 {
            return Err(FsError::Corrupt);
        }
    }
    if !ro {
        v.begin()?;
    }
    Ok(Box::new(v))
}

impl Ex {
    fn root_dir(&self) -> Dir {
        Dir { first: self.g.root, nofat: false, len: 0, at: 0, root: true }
    }
    fn csec(&self, c: u32) -> u64 {
        self.g.heap + (c as u64 - 2) * self.g.spc
    }
    fn read_cl(&mut self, cl: &[u32]) -> Result<Vec<u8>, FsError> {
        read_cl(&mut self.io, self.g.heap, self.g.spc, cl)
    }
    fn write_cl(&mut self, cl: &[u32], data: &[u8]) -> Result<(), FsError> {
        write_cl(&mut self.io, self.g.heap, self.g.spc, cl, data)
    }

    fn fold(&self, u: u16) -> u16 {
        self.up.get(u as usize).copied().unwrap_or(u)
    }
    fn same(&self, a: &str, b: &str) -> bool {
        a.encode_utf16().map(|u| self.fold(u)).eq(b.encode_utf16().map(|u| self.fold(u)))
    }
    fn name_hash(&self, name: &str) -> u16 {
        sum16(name.encode_utf16().flat_map(|u| self.fold(u).to_le_bytes()), &[])
    }

    // --- FAT y mapa de bits ---

    /// Cadena de la FAT desde `first`, con a lo sumo `limit` clusters.
    fn chain(&mut self, first: u32, limit: usize) -> Result<Vec<u32>, FsError> {
        let mut v = Vec::new();
        let mut c = first;
        while v.len() < limit {
            if c < 2 || c > self.g.count + 1 || v.len() as u32 >= self.g.count {
                return Err(FsError::Corrupt);
            }
            v.push(c);
            let n = self.t.get(&mut self.io, c as u64 * 4, 4)?;
            if n >= 0xFFFF_FFF8 {
                break;
            }
            if n == 0 || n == 0xFFFF_FFF7 {
                return Err(FsError::Corrupt);
            }
            c = n;
        }
        Ok(v)
    }

    /// Clusters de un archivo o directorio de `size` bytes: contiguos (NoFatChain) o por la FAT.
    fn chain_of(&mut self, first: u32, nofat: bool, size: u64) -> Result<Vec<u32>, FsError> {
        let n = size.div_ceil(self.g.cs as u64);
        if n == 0 {
            return Ok(Vec::new());
        }
        if n > self.g.count as u64 {
            return Err(FsError::Corrupt);
        }
        if nofat {
            if first < 2 || first as u64 + n > self.g.count as u64 + 2 {
                return Err(FsError::Corrupt);
            }
            Ok((first..first + n as u32).collect())
        } else {
            let v = self.chain(first, n as usize)?;
            if (v.len() as u64) < n { Err(FsError::Corrupt) } else { Ok(v) }
        }
    }

    fn bit(&self, i: usize) -> bool {
        self.bm[i / 8] >> (i % 8) & 1 == 1
    }
    fn set_bit(&mut self, i: usize, v: bool) {
        if v {
            self.bm[i / 8] |= 1 << (i % 8);
        } else {
            self.bm[i / 8] &= !(1 << (i % 8));
        }
        self.bm_dirty.insert(i / 8 / self.io.ss);
    }

    /// Reserva `n` clusters: un hueco contiguo si lo hay (sin FAT) y si no sueltos, enlazados en la FAT.
    fn alloc(&mut self, n: usize) -> Result<(Vec<u32>, bool), FsError> {
        if n == 0 {
            return Ok((Vec::new(), true));
        }
        if n as u64 > self.free as u64 {
            return Err(FsError::NoSpace);
        }
        let count = self.g.count as usize;
        let (mut run, mut start, mut found, mut i) = (0usize, 0usize, None, 0usize);
        while i < count {
            if i % 8 == 0 && self.bm[i / 8] == 0xFF {
                run = 0;
                i += 8;
                continue;
            }
            if !self.bit(i) {
                if run == 0 {
                    start = i;
                }
                run += 1;
                if run == n {
                    found = Some(start);
                    break;
                }
            } else {
                run = 0;
            }
            i += 1;
        }
        let (idx, contig): (Vec<usize>, bool) = match found {
            Some(s) => ((s..s + n).collect(), true),
            None => ((0..count).filter(|&i| !self.bit(i)).take(n).collect(), false),
        };
        if idx.len() < n {
            return Err(FsError::NoSpace);
        }
        let cl: Vec<u32> = idx.iter().map(|&i| i as u32 + 2).collect();
        if !contig {
            for w in cl.windows(2) {
                self.t.set(&mut self.io, w[0] as u64 * 4, 4, w[1])?;
            }
            self.t.set(&mut self.io, cl[n - 1] as u64 * 4, 4, EOC)?;
        }
        for &i in &idx {
            self.set_bit(i, true);
        }
        self.free -= n as u32;
        Ok((cl, contig))
    }

    fn release(&mut self, list: &[u32]) {
        for &c in list {
            self.set_bit(c as usize - 2, false);
        }
        self.free += list.len() as u32;
    }

    /// Deja en disco el mapa de bits y la FAT (antes de escribir la entrada de directorio).
    fn sync(&mut self) -> Result<(), FsError> {
        let (ss, cs) = (self.io.ss, self.g.cs);
        for i in core::mem::take(&mut self.bm_dirty) {
            let off = i * ss;
            let c = *self.bm_cl.get(off / cs).ok_or(FsError::Corrupt)?;
            let sec = self.csec(c) + (off % cs / ss) as u64;
            let sector = self.bm[off..off + ss].to_vec();
            self.io.write(sec, &sector)?;
        }
        self.t.flush(&mut self.io)
    }

    fn write_flags(&mut self) -> Result<(), FsError> {
        let mut b = self.io.read(0, 1)?;
        b[106..108].copy_from_slice(&self.flags.to_le_bytes());
        self.io.write(0, &b)
    }
    /// Antes de cualquier cambio: solo escritura, y volumen marcado como sucio (VolumeDirty).
    fn begin(&mut self) -> Result<(), FsError> {
        if self.ro {
            return Err(FsError::Denied);
        }
        if !self.dirty {
            self.flags |= 2;
            self.write_flags()?;
            self.dirty = true;
        }
        Ok(())
    }

    // --- directorios ---

    fn load(&mut self, d: Dir) -> Result<DirBuf, FsError> {
        let cl = if d.nofat { self.chain_of(d.first, true, d.len)? } else { self.chain(d.first, usize::MAX)? };
        if cl.is_empty() || cl.len() * self.g.cs > MAXDIR {
            return Err(FsError::Corrupt);
        }
        let data = self.read_cl(&cl)?;
        Ok(DirBuf { orig: data.clone(), data, cl })
    }

    fn store(&mut self, b: &mut DirBuf) -> Result<(), FsError> {
        self.sync()?;
        let (ss, spc) = (self.io.ss, self.g.spc as usize);
        for s in 0..b.data.len() / ss {
            if b.data[s * ss..(s + 1) * ss] != b.orig[s * ss..(s + 1) * ss] {
                let sec = self.csec(b.cl[s / spc]) + (s % spc) as u64;
                self.io.write(sec, &b.data[s * ss..(s + 1) * ss])?;
            }
        }
        b.orig = b.data.clone();
        Ok(())
    }

    /// Conjuntos de entradas (archivos y carpetas) de un directorio. Los conjuntos con suma o
    /// estructura incorrectas se ignoran (no se pueden usar, pero tampoco se pisan).
    fn scan(&self, data: &[u8]) -> Vec<Item> {
        let total = data.len() / 32;
        let (mut out, mut i) = (Vec::new(), 0);
        while i < total {
            let e = &data[i * 32..i * 32 + 32];
            if e[0] == 0 {
                break;
            }
            let sc = e[1] as usize;
            if e[0] != 0x85 || !(2..=18).contains(&sc) || i + sc >= total {
                i += 1;
                continue;
            }
            let set = &data[i * 32..(i + 1 + sc) * 32];
            let (s, nl) = (&set[32..64], set[35] as usize);
            if sum16(set.iter().copied(), &[2, 3]) != u16le(set, 2) || s[0] != 0xC0 || nl == 0 || nl.div_ceil(15) != sc - 1 || (2..=sc).any(|k| set[k * 32] != 0xC1) {
                i += 1;
                continue;
            }
            let units: Vec<u16> = (0..nl).map(|k| u16le(set, (2 + k / 15) * 32 + 2 + 2 * (k % 15))).collect();
            let name: String = char::decode_utf16(units).map(|r| r.unwrap_or('\u{FFFD}')).collect();
            out.push(Item { name, at: i, n: 1 + sc, set: set.to_vec() });
            i += 1 + sc;
        }
        out
    }

    fn dir_of(&self, it: &Item) -> Result<Dir, FsError> {
        if it.first() < 2 {
            return Err(FsError::Corrupt);
        }
        Ok(Dir { first: it.first(), nofat: it.nofat(), len: it.size(), at: it.at, root: false })
    }

    /// Directorios del camino (desde la raíz); el último es el pedido.
    fn walk(&mut self, comps: &[&str]) -> Result<Vec<Dir>, FsError> {
        let mut path = vec![self.root_dir()];
        for c in comps {
            let b = self.load(*path.last().unwrap())?;
            let it = self.scan(&b.data).into_iter().find(|i| self.same(&i.name, c)).ok_or(FsError::NotFound)?;
            if !it.dir() {
                return Err(FsError::NotDir);
            }
            path.push(self.dir_of(&it)?);
        }
        Ok(path)
    }

    fn parent<'a>(&mut self, path: &'a str) -> Result<(Vec<Dir>, &'a str), FsError> {
        let mut c = split(path);
        let name = c.pop().ok_or(FsError::Invalid)?;
        Ok((self.walk(&c)?, name))
    }

    fn lookup(&mut self, path: &str) -> Result<Item, FsError> {
        let (dirs, name) = self.parent(path)?;
        let b = self.load(*dirs.last().unwrap())?;
        self.scan(&b.data).into_iter().find(|i| self.same(&i.name, name)).ok_or(FsError::NotFound)
    }

    /// Conjunto de entradas de un archivo o carpeta.
    fn build_set(&self, name: &str, attr: u16, t: u32, first: u32, size: u64, valid: u64, nofat: bool) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let nn = units.len().div_ceil(15);
        let mut s = vec![0u8; (2 + nn) * 32];
        s[0] = 0x85;
        s[1] = (1 + nn) as u8;
        s[4..6].copy_from_slice(&attr.to_le_bytes());
        for o in [8, 12, 16] {
            s[o..o + 4].copy_from_slice(&t.to_le_bytes());
        }
        s[32] = 0xC0;
        s[33] = if first == 0 { 0 } else { 1 | if nofat { 2 } else { 0 } };
        s[35] = units.len() as u8;
        s[36..38].copy_from_slice(&self.name_hash(name).to_le_bytes());
        s[40..48].copy_from_slice(&valid.to_le_bytes());
        s[52..56].copy_from_slice(&first.to_le_bytes());
        s[56..64].copy_from_slice(&size.to_le_bytes());
        for k in 0..nn {
            s[(2 + k) * 32] = 0xC1;
            for j in 0..15 {
                let u = units.get(k * 15 + j).copied().unwrap_or(0);
                s[(2 + k) * 32 + 2 + 2 * j..][..2].copy_from_slice(&u.to_le_bytes());
            }
        }
        let c = sum16(s.iter().copied(), &[2, 3]);
        s[2..4].copy_from_slice(&c.to_le_bytes());
        s
    }

    /// Cambia nombre, o datos y fechas, de un conjunto ya existente y recalcula sus sumas.
    fn rebuild(&self, it: &Item, name: &str) -> Vec<u8> {
        let mut s = self.build_set(name, it.attr(), 0, it.first(), it.size(), it.valid(), it.nofat());
        s[8..20].copy_from_slice(&it.set[8..20]);
        let c = sum16(s.iter().copied(), &[2, 3]);
        s[2..4].copy_from_slice(&c.to_le_bytes());
        s
    }

    /// Añade un conjunto de entradas al directorio `dirs.last()`; si hay que ampliarlo, lo amplía.
    fn add_set(&mut self, dirs: &mut [Dir], buf: &mut DirBuf, set: &[u8]) -> Result<(), FsError> {
        let k = set.len() / 32;
        let total0 = buf.data.len() / 32;
        let end = (0..total0).find(|&i| buf.data[i * 32] == 0).unwrap_or(total0);
        let mut grew = false;
        let at = loop {
            match find_run(&buf.data, k) {
                Some(a) => break a,
                None => {
                    self.grow(dirs.last_mut().unwrap(), buf)?;
                    grew = true;
                }
            }
        };
        buf.data[at * 32..(at + k) * 32].copy_from_slice(set);
        let after = at + k;
        if after > end && after < buf.data.len() / 32 {
            buf.data[after * 32] = 0;
        }
        if grew {
            self.store(buf)?;
            self.persist(dirs)?;
        }
        Ok(())
    }

    /// Añade un cluster al directorio (siempre por la FAT: si era contiguo, se enlaza entero).
    fn grow(&mut self, d: &mut Dir, buf: &mut DirBuf) -> Result<(), FsError> {
        let (cs, last) = (self.g.cs, *buf.cl.last().ok_or(FsError::Corrupt)?);
        if buf.cl.len() * cs + cs > MAXDIR {
            return Err(FsError::NoSpace);
        }
        let (c, _) = self.alloc(1)?;
        let c = c[0];
        if d.nofat {
            for w in buf.cl.windows(2) {
                self.t.set(&mut self.io, w[0] as u64 * 4, 4, w[1])?;
            }
        }
        self.t.set(&mut self.io, last as u64 * 4, 4, c)?;
        self.t.set(&mut self.io, c as u64 * 4, 4, EOC)?;
        self.io.write(self.csec(c), &vec![0; cs])?;
        d.nofat = false;
        d.len = (buf.cl.len() * cs + cs) as u64;
        buf.cl.push(c);
        buf.data.resize(buf.data.len() + cs, 0);
        buf.orig.resize(buf.orig.len() + cs, 0);
        Ok(())
    }

    /// Escribe en el padre la nueva longitud y modo de FAT del último directorio del camino.
    fn persist(&mut self, dirs: &[Dir]) -> Result<(), FsError> {
        let k = dirs.len() - 1;
        if dirs[k].root {
            return Ok(());
        }
        let mut pb = self.load(dirs[k - 1])?;
        let at = dirs[k].at * 32;
        let sc = pb.data[at + 1] as usize;
        if pb.data[at] != 0x85 || sc < 2 {
            return Err(FsError::Corrupt);
        }
        let s = &mut pb.data[at + 32..at + 64];
        s[1] = 1;
        s[8..16].copy_from_slice(&dirs[k].len.to_le_bytes());
        s[24..32].copy_from_slice(&dirs[k].len.to_le_bytes());
        let c = sum16(pb.data[at..at + (1 + sc) * 32].iter().copied(), &[2, 3]);
        pb.data[at + 2..at + 4].copy_from_slice(&c.to_le_bytes());
        self.store(&mut pb)
    }

    fn entry_of(&self, it: &Item) -> Entry {
        let size = if it.dir() { 0 } else { it.size() };
        Entry { name: it.name.clone(), dir: it.dir(), size, modified: unpack_time(u32le(&it.set, 12)) }
    }
}

impl Volume for Ex {
    fn kind(&self) -> FsKind {
        FsKind::ExFat
    }
    fn label(&self) -> String {
        self.label.clone()
    }
    fn space(&mut self) -> (u64, u64) {
        (self.g.count as u64 * self.g.cs as u64, self.free as u64 * self.g.cs as u64)
    }
    fn read_only(&self) -> bool {
        self.ro
    }

    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, FsError> {
        let d = *self.walk(&split(dir))?.last().unwrap();
        let b = self.load(d)?;
        Ok(self.scan(&b.data).iter().map(|i| self.entry_of(i)).collect())
    }

    fn stat(&mut self, path: &str) -> Result<Entry, FsError> {
        if split(path).is_empty() {
            return Ok(Entry { name: String::new(), dir: true, size: 0, modified: DateTime::default() });
        }
        let it = self.lookup(path)?;
        Ok(self.entry_of(&it))
    }

    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        let it = self.lookup(path)?;
        if it.dir() {
            return Err(FsError::IsDir);
        }
        let (size, valid) = (it.size(), it.valid());
        if valid > size {
            return Err(FsError::Corrupt);
        }
        let cl = self.chain_of(it.first(), it.nofat(), size)?;
        let mut d = self.read_cl(&cl)?;
        d.truncate(size as usize);
        d[valid as usize..].fill(0); // lo que pasa de ValidDataLength se lee como ceros
        Ok(d)
    }

    fn write(&mut self, path: &str, data: &[u8], now: DateTime) -> Result<(), FsError> {
        let (mut dirs, name) = self.parent(path)?;
        if !valid_name(name) {
            return Err(FsError::BadName);
        }
        self.begin()?;
        let mut buf = self.load(*dirs.last().unwrap())?;
        let existing = self.scan(&buf.data).into_iter().find(|i| self.same(&i.name, name));
        if existing.as_ref().is_some_and(|i| i.dir()) {
            return Err(FsError::IsDir);
        }
        let old = match &existing {
            Some(i) if i.first() >= 2 => self.chain_of(i.first(), i.nofat(), i.size())?,
            _ => Vec::new(),
        };
        // Copia sobre copia (ver fat.rs): solo si no cabe se liberan antes los clusters viejos.
        let n = data.len().div_ceil(self.g.cs);
        let mut old_freed = false;
        if (self.free as usize) < n {
            if self.free as usize + old.len() < n {
                return Err(FsError::NoSpace);
            }
            self.release(&old);
            old_freed = true;
        }
        let (cl, contig) = self.alloc(n)?;
        let first = cl.first().copied().unwrap_or(0);
        let t = pack_time(now);
        let done = self.write_cl(&cl, data).and_then(|_| self.sync()).and_then(|_| match &existing {
            Some(it) => {
                let mut set = self.build_set(&it.name, it.attr() | 0x20, t, first, data.len() as u64, data.len() as u64, contig);
                set[8..12].copy_from_slice(&it.set[8..12]); // la fecha de creación se conserva
                let c = sum16(set.iter().copied(), &[2, 3]);
                set[2..4].copy_from_slice(&c.to_le_bytes());
                buf.data[it.at * 32..(it.at + it.n) * 32].copy_from_slice(&set);
                self.store(&mut buf)
            }
            None => {
                let set = self.build_set(name, 0x20, t, first, data.len() as u64, data.len() as u64, contig);
                self.add_set(&mut dirs, &mut buf, &set).and_then(|_| self.store(&mut buf))
            }
        });
        if let Err(e) = done {
            if !old_freed {
                self.release(&cl);
            }
            return Err(e);
        }
        if !old_freed {
            self.release(&old);
        }
        self.sync()
    }

    fn mkdir(&mut self, path: &str, now: DateTime) -> Result<(), FsError> {
        let (mut dirs, name) = self.parent(path)?;
        if !valid_name(name) {
            return Err(FsError::BadName);
        }
        self.begin()?;
        let mut buf = self.load(*dirs.last().unwrap())?;
        if self.scan(&buf.data).iter().any(|i| self.same(&i.name, name)) {
            return Err(FsError::Exists);
        }
        let (c, _) = self.alloc(1)?;
        let c = c[0];
        let cs = self.g.cs as u64;
        let set = self.build_set(name, 0x10, pack_time(now), c, cs, cs, false);
        let r = self
            .t
            .set(&mut self.io, c as u64 * 4, 4, EOC)
            .and_then(|_| self.io.write(self.csec(c), &vec![0; cs as usize]))
            .and_then(|_| self.sync())
            .and_then(|_| self.add_set(&mut dirs, &mut buf, &set))
            .and_then(|_| self.store(&mut buf));
        if r.is_err() {
            self.release(&[c]);
        }
        r?;
        self.sync()
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        let (fdirs, fname) = self.parent(from)?;
        let (mut tdirs, tname) = self.parent(to)?;
        if !valid_name(tname) {
            return Err(FsError::BadName);
        }
        self.begin()?;
        let (fd, td) = (*fdirs.last().unwrap(), *tdirs.last().unwrap());
        let mut fb = self.load(fd)?;
        let it = self.scan(&fb.data).into_iter().find(|i| self.same(&i.name, fname)).ok_or(FsError::NotFound)?;
        // Una carpeta no puede entrar en sí misma ni en sus descendientes.
        if it.dir() && tdirs.iter().any(|d| !d.root && d.first == it.first()) {
            return Err(FsError::Invalid);
        }
        let same = fd.first == td.first;
        if same && fname == tname {
            return Ok(());
        }
        let mut tb = if same { None } else { Some(self.load(td)?) };
        let tdata = tb.as_ref().map_or(&fb.data, |b| &b.data);
        if self.scan(tdata).iter().any(|i| self.same(&i.name, tname) && !(same && i.at == it.at)) {
            return Err(FsError::Exists);
        }
        let set = self.rebuild(&it, tname);
        match &mut tb {
            None => {
                (it.at..it.at + it.n).for_each(|i| fb.data[i * 32] &= 0x7F);
                let mut fdirs = fdirs;
                self.add_set(&mut fdirs, &mut fb, &set)?;
                self.store(&mut fb)
            }
            Some(tb) => {
                self.add_set(&mut tdirs, tb, &set)?;
                self.store(tb)?;
                // El destino pudo ampliarse y con ello cambiar el origen en disco: se recarga.
                let mut fb = self.load(fd)?;
                let it = self.scan(&fb.data).into_iter().find(|i| self.same(&i.name, fname) && i.first() == it.first()).ok_or(FsError::Corrupt)?;
                (it.at..it.at + it.n).for_each(|i| fb.data[i * 32] &= 0x7F);
                self.store(&mut fb)
            }
        }
    }

    fn delete(&mut self, path: &str) -> Result<(), FsError> {
        let (dirs, name) = self.parent(path)?;
        self.begin()?;
        let mut buf = self.load(*dirs.last().unwrap())?;
        let it = self.scan(&buf.data).into_iter().find(|i| self.same(&i.name, name)).ok_or(FsError::NotFound)?;
        if it.dir() {
            let cb = self.load(self.dir_of(&it)?)?;
            if !self.scan(&cb.data).is_empty() {
                return Err(FsError::NotEmpty);
            }
        }
        (it.at..it.at + it.n).for_each(|i| buf.data[i * 32] &= 0x7F);
        self.store(&mut buf)?;
        // Si la cadena estaba rota se pierde algún cluster, pero la entrada ya no está.
        if it.first() >= 2 {
            if let Ok(cl) = self.chain_of(it.first(), it.nofat(), it.size()) {
                self.release(&cl);
            }
        }
        self.sync()
    }

    fn flush(&mut self) -> Result<(), FsError> {
        if self.ro {
            return Ok(());
        }
        self.sync()?;
        if self.dirty {
            self.flags &= !2;
            self.write_flags()?;
            self.dirty = false;
        }
        self.io.dev.flush()
    }
}

// --- formatear ---

/// Tamaño de cluster por defecto según el tamaño del volumen (valores habituales de Windows).
fn cluster_bytes(bytes: u64) -> u64 {
    if bytes <= 256 << 20 {
        4096
    } else if bytes <= 32 << 30 {
        32768
    } else {
        131072
    }
}

/// Tabla de mayúsculas comprimida (0xFFFF + n = "n unidades sin cambio"), generada con las
/// mayúsculas de Unicode del plano básico.
fn upcase_table() -> Vec<u8> {
    let map = |c: u32| -> u16 {
        match char::from_u32(c).map(|ch| upper(ch) as u32) {
            Some(u) if u <= 0xFFFF => u as u16,
            _ => c as u16,
        }
    };
    let last = (0..=0xFFFFu32).rev().find(|&c| map(c) != c as u16).unwrap_or(0);
    let (mut words, mut c) = (Vec::<u16>::new(), 0u32);
    while c <= last {
        if map(c) == c as u16 {
            let mut n = 0;
            while c + n <= last && map(c + n) == (c + n) as u16 {
                n += 1;
            }
            if n > 2 {
                words.extend_from_slice(&[0xFFFF, n as u16]);
            } else {
                words.extend((0..n).map(|k| (c + k) as u16));
            }
            c += n;
        } else {
            words.push(map(c));
            c += 1;
        }
    }
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// Formatea el disco entero: una partición MBR tipo 0x07 alineada a 1 MiB que lo ocupa, con exFAT.
pub fn format(dev: &mut dyn SectorDev, label: &str, now: DateTime) -> Result<(), FsError> {
    let units: Vec<u16> = label.encode_utf16().collect();
    if units.len() > 11 || label.chars().any(|c| c < ' ' || "\"*/:<>?\\|".contains(c)) {
        return Err(FsError::BadName);
    }
    let (start, total) = part::prepare(dev)?;
    let ss = dev.sector_size() as u64;
    let cb = cluster_bytes(total * ss).max(ss);
    let spc = cb / ss;
    let up = upcase_table();
    // Sectores de la FAT: dependen de los clusters, que dependen de dónde empieza el montón.
    let heap_for = |f: u64| (24 + f).div_ceil(spc) * spc;
    let count_for = |f: u64| total.saturating_sub(heap_for(f)) / spc;
    let need = |c: u64| ((c + 2) * 4).div_ceil(ss);
    let mut f = need(count_for(0));
    while need(count_for(f)) > f {
        f += 1;
    }
    let (heap, count) = (heap_for(f), count_for(f));
    let bmb = count.div_ceil(8);
    let (bmc, upc) = (bmb.div_ceil(cb), (up.len() as u64).div_ceil(cb));
    let used = bmc + upc + 1;
    if count < used + 8 || count > 0xFFFF_FFF5 {
        return Err(FsError::NoSpace);
    }
    let (up_first, root) = (2 + bmc, 2 + bmc + upc);
    let at = |dev: &mut dyn SectorDev, sec: u64, b: &[u8]| dev.write(start + sec, b);

    zero_sectors(dev, start, 24 + f)?;
    let vid = ((now.year as u32) << 16) ^ ((now.month as u32) << 12 | (now.day as u32) << 7 | (now.hour as u32) << 2) ^ (now.minute as u32 * 60 + now.second as u32);
    let mut b = vec![0u8; ss as usize];
    b[0..3].copy_from_slice(&[0xEB, 0x76, 0x90]);
    b[3..11].copy_from_slice(b"EXFAT   ");
    b[64..72].copy_from_slice(&start.to_le_bytes());
    b[72..80].copy_from_slice(&total.to_le_bytes());
    b[80..84].copy_from_slice(&24u32.to_le_bytes());
    b[84..88].copy_from_slice(&(f as u32).to_le_bytes());
    b[88..92].copy_from_slice(&(heap as u32).to_le_bytes());
    b[92..96].copy_from_slice(&(count as u32).to_le_bytes());
    b[96..100].copy_from_slice(&(root as u32).to_le_bytes());
    b[100..104].copy_from_slice(&vid.to_le_bytes());
    b[104..106].copy_from_slice(&0x0100u16.to_le_bytes());
    b[108] = ss.trailing_zeros() as u8;
    b[109] = spc.trailing_zeros() as u8;
    b[110] = 1;
    b[111] = 0x80;
    b[112] = (used * 100 / count) as u8;
    b[510] = 0x55;
    b[511] = 0xAA;
    // Sectores 1..=8: firma de arranque extendida; 9 y 10 a cero; 11: la suma repetida.
    let mut region = b.clone();
    for i in 1..11 {
        let mut e = vec![0u8; ss as usize];
        if i <= 8 {
            e[ss as usize - 2..].copy_from_slice(&[0x55, 0xAA]);
        }
        region.extend_from_slice(&e);
    }
    let sum = sum32(&region, &[106, 107, 112]).to_le_bytes();
    region.extend(sum.iter().cycle().take(ss as usize));
    at(dev, 0, &region)?;
    at(dev, 12, &region)?;

    // FAT: medio, fin de cadena, y las cadenas del mapa de bits, la tabla y la raíz.
    let mut fat: Vec<u32> = vec![0xFFFF_FFF8, EOC];
    for first in [2, up_first, root] {
        let n = if first == 2 { bmc } else if first == up_first { upc } else { 1 };
        fat.extend((1..=n).map(|k| if k == n { EOC } else { (first + k) as u32 }));
    }
    let mut fb: Vec<u8> = fat.iter().flat_map(|v| v.to_le_bytes()).collect();
    fb.resize(fb.len().div_ceil(ss as usize) * ss as usize, 0);
    at(dev, 24, &fb)?;

    let mut bm = vec![0u8; (bmc * cb) as usize];
    for i in 0..used as usize {
        bm[i / 8] |= 1 << (i % 8);
    }
    at(dev, heap, &bm)?;
    let mut ub = up.clone();
    ub.resize((upc * cb) as usize, 0);
    at(dev, heap + bmc * spc, &ub)?;
    let mut rb = vec![0u8; cb as usize];
    let mut k = 0;
    if !units.is_empty() {
        rb[0] = 0x83;
        rb[1] = units.len() as u8;
        for (j, u) in units.iter().enumerate() {
            rb[2 + 2 * j..4 + 2 * j].copy_from_slice(&u.to_le_bytes());
        }
        k = 1;
    }
    rb[k * 32] = 0x81;
    rb[k * 32 + 20..k * 32 + 24].copy_from_slice(&2u32.to_le_bytes());
    rb[k * 32 + 24..k * 32 + 32].copy_from_slice(&bmb.to_le_bytes());
    rb[(k + 1) * 32] = 0x82;
    rb[(k + 1) * 32 + 4..(k + 1) * 32 + 8].copy_from_slice(&sum32(&up, &[]).to_le_bytes());
    rb[(k + 1) * 32 + 20..(k + 1) * 32 + 24].copy_from_slice(&(up_first as u32).to_le_bytes());
    rb[(k + 1) * 32 + 24..(k + 1) * 32 + 32].copy_from_slice(&(up.len() as u64).to_le_bytes());
    at(dev, heap + (root - 2) * spc, &rb)?;
    part::write_mbr(dev, start, total, 0x07)?;
    dev.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vol::fat::tests::{open, suite, Shared, NOW};
    use crate::vol::{format as vformat, partitions, MemSectors};

    fn dirty(d: &Shared) -> Option<bool> {
        let p = partitions(&mut d.clone()).unwrap()[0].clone();
        let m = d.0.borrow();
        let o = p.start as usize * m.sector + 106;
        Some(u16le(&m.data, o) & 2 != 0)
    }

    #[test]
    fn exfat_suite() {
        let d = Shared::new(16 * 2048, 512); // 16 MiB
        suite(&d, FsKind::ExFat, 1_000_000, &dirty);
        assert_eq!(open(&d).kind(), FsKind::ExFat);
    }

    #[test]
    fn exfat_suite_4096() {
        let d = Shared::new(16 * 256, 4096);
        suite(&d, FsKind::ExFat, 1_000_000, &dirty);
    }

    #[test]
    fn upcase_table_roundtrip() {
        let d = Shared::new(16 * 2048, 512);
        vformat(&mut d.clone(), FsKind::ExFat, "ñ", NOW).unwrap();
        let mut v = open(&d);
        assert_eq!(v.label(), "ñ");
        v.write("Ñandú.txt", b"x", NOW).unwrap();
        assert_eq!(v.read("ñANDÚ.TXT").unwrap(), b"x");
        assert_eq!(v.write("ñandú.TXT", b"yy", NOW), Ok(()));
        assert_eq!(v.list("").unwrap().len(), 1);
    }

    /// Archivos contiguos (NoFatChain) y fragmentados (con FAT).
    #[test]
    fn contiguous_and_fragmented() {
        let d = Shared::new(8 * 2048, 512); // 8 MiB: clusters de 4 KiB
        vformat(&mut d.clone(), FsKind::ExFat, "", NOW).unwrap();
        let mut v = open(&d);
        v.mkdir("d", NOW).unwrap();
        // Llena el disco con archivos de un cluster y borra uno de cada dos: solo quedan huecos de 1.
        let mut n = 0usize;
        while v.write(&alloc::format!("d\\f{n}"), &[n as u8; 4096], NOW).is_ok() {
            n += 1;
        }
        assert!(n > 100);
        for i in (0..n).step_by(2) {
            v.delete(&alloc::format!("d\\f{i}")).unwrap();
        }
        let data: Vec<u8> = (0..40_000u32).map(|x| (x * 7) as u8).collect();
        v.write("frag.bin", &data, NOW).unwrap();
        assert_eq!(v.read("frag.bin").unwrap(), data);
        v.flush().unwrap();
        drop(v);
        // Montada de nuevo: el fragmentado va por la FAT y los pequeños son contiguos.
        let mut v = open(&d);
        assert_eq!(v.read("frag.bin").unwrap(), data);
        assert_eq!(v.read("d\\f1").unwrap(), [1u8; 4096]);
        for i in (1..n).step_by(2) {
            assert_eq!(v.read(&alloc::format!("d\\f{i}")).unwrap(), [i as u8; 4096]);
        }
        drop(v);
        let p = partitions(&mut d.clone()).unwrap()[0].clone();
        // Sin dependencias de los internos: el conjunto de "frag.bin" tiene NoFatChain a 0.
        let mut m = match mount(Box::new(d.clone()), &p) {
            Ok(v) => v,
            Err(e) => panic!("{e:?}"),
        };
        assert_eq!(m.read("frag.bin").unwrap(), data);
        m.write("liso.bin", &data[..4096], NOW).unwrap();
        m.flush().unwrap();
        let raw = d.0.borrow();
        // Busca los conjuntos en la raíz: el primer byte 0x85 seguido de 0xC0 con flags.
        let find = |name: &str| -> u8 {
            let nb: Vec<u8> = name.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
            let i = raw.data.windows(nb.len()).position(|w| w == &nb[..]).unwrap();
            // El nombre empieza 2 bytes dentro de la entrada 0xC1, que va justo detrás de la 0xC0.
            raw.data[i - 2 - 32 + 1]
        };
        assert_eq!(find("frag.bin") & 2, 0, "fragmentado: con FAT");
        assert_eq!(find("liso.bin") & 2, 2, "contiguo: NoFatChain");
    }

    /// Imagen exFAT escrita byte a byte según la especificación (sumas calculadas aparte).
    #[test]
    fn exfat_boot_sector_by_spec() {
        let ss = 512usize;
        let mut img = vec![0u8; 4096 * ss];
        let b = &mut img[..ss];
        b[0..3].copy_from_slice(&[0xEB, 0x76, 0x90]);
        b[3..11].copy_from_slice(b"EXFAT   ");
        b[72..80].copy_from_slice(&[0x00, 0x10, 0, 0, 0, 0, 0, 0]); // VolumeLength = 4096
        b[80..84].copy_from_slice(&[24, 0, 0, 0]); // FatOffset
        b[84..88].copy_from_slice(&[8, 0, 0, 0]); // FatLength
        b[88..92].copy_from_slice(&[32, 0, 0, 0]); // ClusterHeapOffset
        b[92..96].copy_from_slice(&[0xFC, 0x01, 0, 0]); // ClusterCount = 508
        b[96..100].copy_from_slice(&[4, 0, 0, 0]); // FirstClusterOfRootDirectory
        b[100..104].copy_from_slice(&[0x78, 0x56, 0x34, 0x12]);
        b[104..106].copy_from_slice(&[0x00, 0x01]); // 1.00
        b[108] = 9; // 512 bytes por sector
        b[109] = 3; // 8 sectores por cluster
        b[110] = 1;
        b[111] = 0x80;
        b[510] = 0x55;
        b[511] = 0xAA;
        for i in 1..=8 {
            img[i * ss + ss - 2..(i + 1) * ss].copy_from_slice(&[0x55, 0xAA]);
        }
        for w in img[11 * ss..12 * ss].chunks_exact_mut(4) {
            w.copy_from_slice(&0x8A9C_58A8u32.to_le_bytes()); // suma de la región de arranque
        }
        // FAT: medio, fin, y bitmap(2), mayúsculas(3) y raíz(4) con un cluster cada uno.
        let fat = 24 * ss;
        for (i, v) in [0xFFFF_FFF8u32, 0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF_FFFF].iter().enumerate() {
            img[fat + 4 * i..fat + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        let cluster = |c: usize| (32 + (c - 2) * 8) * ss;
        img[cluster(2)] = 0x0F; // clusters 2,3,4,5 ocupados
        // Tabla de mayúsculas: 97 unidades sin cambio y luego A..Z para 'a'..'z'.
        let u = cluster(3);
        img[u..u + 4].copy_from_slice(&[0xFF, 0xFF, 0x61, 0x00]);
        for i in 0..26 {
            img[u + 4 + 2 * i] = 0x41 + i as u8;
        }
        let r = cluster(4);
        img[r] = 0x81; // mapa de bits: cluster 2, 64 bytes
        img[r + 20] = 2;
        img[r + 24] = 64;
        img[r + 32] = 0x82; // mayúsculas: suma, cluster 3, 56 bytes
        img[r + 32 + 4..r + 32 + 8].copy_from_slice(&0xE394_9402u32.to_le_bytes());
        img[r + 32 + 20] = 3;
        img[r + 32 + 24] = 56;
        // "Hola.txt": 8 bytes en el cluster 5, contiguo (NoFatChain)
        let f = r + 64;
        img[f] = 0x85;
        img[f + 1] = 2;
        img[f + 2..f + 4].copy_from_slice(&0xD750u16.to_le_bytes());
        img[f + 4] = 0x20;
        for o in [8, 12, 16] {
            img[f + o..f + o + 4].copy_from_slice(&0x5D72_6DAFu32.to_le_bytes());
        }
        img[f + 32] = 0xC0;
        img[f + 33] = 0x03;
        img[f + 35] = 8;
        img[f + 36..f + 38].copy_from_slice(&0x2940u16.to_le_bytes());
        img[f + 40] = 8;
        img[f + 52] = 5;
        img[f + 56] = 8;
        img[f + 64] = 0xC1;
        for (i, ch) in "Hola.txt".bytes().enumerate() {
            img[f + 66 + 2 * i] = ch;
        }
        img[cluster(5)..cluster(5) + 8].copy_from_slice(b"exfat ok");
        let mut dev = MemSectors::new(4096, 512);
        dev.data = img;
        let p = partitions(&mut dev).unwrap();
        assert_eq!(p, vec![Partition { start: 0, sectors: 4096, kind: FsKind::ExFat }]);
        let mut v = mount(Box::new(dev), &p[0]).unwrap();
        assert_eq!(v.kind(), FsKind::ExFat);
        assert_eq!(v.space(), (508 * 4096, 504 * 4096));
        let l = v.list("").unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].name.as_str(), l[0].size, l[0].dir), ("Hola.txt", 8, false));
        assert_eq!((l[0].modified.year, l[0].modified.month, l[0].modified.day, l[0].modified.hour, l[0].modified.minute), (2026, 11, 18, 13, 45));
        assert_eq!(v.read("HOLA.TXT").unwrap(), b"exfat ok");
        // y se puede seguir escribiendo sobre ella
        v.write("nuevo.txt", b"1234", NOW).unwrap();
        assert_eq!(v.read("nuevo.txt").unwrap(), b"1234");
    }

    #[test]
    fn corrupt_images_are_rejected_not_panicking() {
        let d = Shared::new(8 * 2048, 512);
        vformat(&mut d.clone(), FsKind::ExFat, "X", NOW).unwrap();
        let p = partitions(&mut d.clone()).unwrap()[0].clone();
        let good = d.0.borrow().data.clone();
        // Cualquier byte de la región de arranque cambiado: o se monta por el respaldo o da Corrupt.
        for off in (0..512).step_by(7) {
            let mut m = MemSectors::new(8 * 2048, 512);
            m.data = good.clone();
            m.data[p.start as usize * 512 + off] ^= 0xFF;
            let _ = mount(Box::new(m), &p);
        }
        // Daña la raíz (bytes al azar): no hay pánico ni bucle.
        let heap_root = {
            let m = d.0.borrow();
            let b = &m.data[p.start as usize * 512..];
            (p.start as usize + u32le(b, 88) as usize + (u32le(b, 96) as usize - 2) * 8) * 512
        };
        for seed in 0..64usize {
            let mut m = MemSectors::new(8 * 2048, 512);
            m.data = good.clone();
            for k in 0..40 {
                let o = heap_root + (seed * 131 + k * 17) % 4096;
                m.data[o] = (seed * 31 + k * 7) as u8;
            }
            if let Ok(mut v) = mount(Box::new(m), &p) {
                let _ = v.list("");
                let _ = v.read("a");
                let _ = v.write("b", b"1", NOW);
                let _ = v.delete("b");
            }
        }
    }
}
