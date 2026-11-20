// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! UDF (ECMA-167 + OSTA UDF 2.01/2.50/2.60) de solo lectura: anclaje en el sector 256, secuencia
//! de descriptores de volumen, mapas de partición tipo 1 y de metadatos (UDF 2.50+), File Set,
//! File Entry / Extended File Entry y directorios. La imagen es entrada hostil: todo se valida
//! (checksum y CRC de cada etiqueta) y las lecturas se acotan al tamaño del volumen.
//!
//! shortcut: sin partición virtual (CD-R multisesión) ni tabla de sparing (se leen como físicas),
//! sin comprobar el campo «ubicación» de las etiquetas ni los enlaces VDP de la secuencia de volumen.
use super::fat::{same_name, split, Io};
use super::iso::read_at;
use super::part::{u16le, u32le, u64le};
use super::{FsKind, Partition, SectorDev, Volume};
use crate::fs::{Entry, FsError};
use crate::platform::DateTime;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// Tamaño máximo de una carpeta que se carga entera.
const MAXDIR: u64 = 1 << 24;
/// Tope de descriptores de extensión por archivo y de continuaciones encadenadas.
const MAXADS: usize = 1 << 16;
const MAXHOPS: usize = 64;

/// CRC-CCITT (polinomio 0x1021, inicial 0) de las etiquetas UDF.
fn crc16(d: &[u8]) -> u16 {
    let mut c = 0u16;
    for &b in d {
        c ^= (b as u16) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x1021 } else { c << 1 };
        }
    }
    c
}

/// Etiqueta de descriptor correcta: identificador, suma de comprobación y CRC.
fn tag_ok(b: &[u8], id: u16) -> bool {
    if b.len() < 16 || u16le(b, 0) != id {
        return false;
    }
    let sum = b[..16].iter().enumerate().filter(|(i, _)| *i != 4).fold(0u8, |a, (_, &x)| a.wrapping_add(x));
    let n = u16le(b, 10) as usize;
    sum == b[4] && 16 + n <= b.len() && crc16(&b[16..16 + n]) == u16le(b, 8)
}

/// Texto CS0: primer byte = compresión (8 = un byte por carácter, 16 = UTF-16 big-endian).
fn cs0(b: &[u8]) -> Result<String, FsError> {
    match b.first() {
        None => Ok(String::new()),
        Some(8) | Some(254) => Ok(b[1..].iter().map(|&c| c as char).collect()),
        Some(16) | Some(255) if b.len() % 2 == 1 => {
            Ok(char::decode_utf16(b[1..].chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]]))).map(|r| r.unwrap_or('\u{FFFD}')).collect())
        }
        _ => Err(FsError::Corrupt),
    }
}

/// dstring: el último byte es la longitud usada (incluido el byte de compresión).
fn dstring(d: &[u8]) -> Result<String, FsError> {
    let l = *d.last().ok_or(FsError::Corrupt)? as usize;
    if l >= d.len() {
        return Err(FsError::Corrupt);
    }
    cs0(&d[..l])
}

/// Marca de tiempo UDF de 12 bytes; la zona horaria se descarta (hora tal cual).
fn timestamp(b: &[u8]) -> DateTime {
    let y = u16le(b, 2) as i16;
    if y <= 0 || b[4] == 0 {
        return DateTime::default();
    }
    DateTime { year: y as u16, month: b[4].clamp(1, 12), day: b[5].clamp(1, 31), hour: b[6].min(23), minute: b[7].min(59), second: b[8].min(59) }
}

/// long_ad: (longitud, tipo, bloque lógico, referencia de partición).
fn long_ad(b: &[u8], o: usize) -> (u64, u32, u64, u16) {
    let l = u32le(b, o);
    ((l & 0x3FFF_FFFF) as u64, l >> 30, u32le(b, o + 4) as u64, u16le(b, o + 8))
}

/// Trozo de bytes del medio (desplazamiento absoluto en la partición del disco) o ceros.
#[derive(Clone)]
struct Run {
    off: u64,
    len: u64,
    zero: bool,
}

#[derive(Clone)]
enum Data {
    Emb(Vec<u8>),
    Runs(Vec<Run>),
}

#[derive(Clone)]
struct Node {
    /// Directorio / archivo normal (o tipo 0); otros tipos (enlaces, dispositivos) no se listan.
    dir: bool,
    shown: bool,
    size: u64,
    date: DateTime,
    data: Data,
}

enum Map {
    /// Partición física: primer bloque y número de bloques.
    Phys { start: u64, blocks: u64 },
    /// Partición de metadatos: sus bloques viven en los trozos del archivo de metadatos.
    Meta(Vec<Run>),
}

struct Ctx {
    io: Io,
    bs: u64,
    maps: Vec<Map>,
}

impl Ctx {
    /// Traduce `len` bytes desde el bloque `lbn` de la partición `part` a trozos del medio.
    fn resolve(&self, part: u16, lbn: u64, len: u64) -> Result<Vec<Run>, FsError> {
        match self.maps.get(part as usize).ok_or(FsError::Corrupt)? {
            Map::Phys { start, blocks } => {
                if lbn * self.bs + len > blocks * self.bs {
                    return Err(FsError::Corrupt);
                }
                Ok(alloc::vec![Run { off: (start + lbn) * self.bs, len, zero: false }])
            }
            Map::Meta(runs) => {
                let (mut skip, mut left, mut out) = (lbn * self.bs, len, Vec::new());
                for r in runs {
                    if left == 0 {
                        break;
                    }
                    if skip >= r.len {
                        skip -= r.len;
                        continue;
                    }
                    let t = (r.len - skip).min(left);
                    out.push(Run { off: r.off + skip, len: t, zero: r.zero });
                    left -= t;
                    skip = 0;
                }
                if left > 0 {
                    return Err(FsError::Corrupt);
                }
                Ok(out)
            }
        }
    }

    /// Lee exactamente `size` bytes de los trozos (el llamador ya acotó `size`).
    fn read_runs(&mut self, runs: &[Run], size: u64) -> Result<Vec<u8>, FsError> {
        let mut v = Vec::new();
        v.try_reserve_exact(size as usize).map_err(|_| FsError::NoSpace)?;
        for r in runs {
            let t = r.len.min(size - v.len() as u64);
            if t == 0 {
                break;
            }
            if r.zero {
                v.resize(v.len() + t as usize, 0);
            } else {
                v.extend(read_at(&mut self.io, r.off, t as usize)?);
            }
        }
        if v.len() as u64 != size {
            return Err(FsError::Corrupt);
        }
        Ok(v)
    }

    fn block(&mut self, part: u16, lbn: u64) -> Result<Vec<u8>, FsError> {
        let runs = self.resolve(part, lbn, self.bs)?;
        self.read_runs(&runs, self.bs)
    }

    /// Lista de descriptores de extensión (cortos o largos) → trozos, siguiendo las continuaciones.
    fn runs(&mut self, ad: &[u8], long: bool, part: u16) -> Result<Vec<Run>, FsError> {
        let (step, mut out, mut cur, mut hops, mut count) = (if long { 16 } else { 8 }, Vec::new(), ad.to_vec(), 0, 0);
        loop {
            let mut next = None;
            for c in cur.chunks_exact(step) {
                let (len, et, lbn, p) = if long { long_ad(c, 0) } else { ((u32le(c, 0) & 0x3FFF_FFFF) as u64, u32le(c, 0) >> 30, u32le(c, 4) as u64, part) };
                let (len, lbn) = if long { (len, lbn) } else { (len, u32le(c, 4) as u64) };
                if len == 0 {
                    break;
                }
                count += 1;
                if count > MAXADS || out.len() > MAXADS {
                    return Err(FsError::Corrupt);
                }
                match et {
                    0 => out.extend(self.resolve(p, lbn, len)?),
                    1 | 2 => out.push(Run { off: 0, len, zero: true }), // reservado sin grabar: ceros
                    _ => {
                        next = Some((p, lbn));
                        break;
                    }
                }
            }
            let Some((p, lbn)) = next else { return Ok(out) };
            hops += 1;
            let b = if hops > MAXHOPS { Err(FsError::Corrupt) } else { self.block(p, lbn) }?;
            let n = u32le(&b, 20) as usize;
            if !tag_ok(&b, 258) || 24 + n > b.len() {
                return Err(FsError::Corrupt);
            }
            cur = b[24..24 + n].to_vec();
        }
    }

    /// Carga el File Entry / Extended File Entry que hay en el bloque `lbn` de `part`
    /// (siguiendo hasta 8 entradas indirectas).
    fn load(&mut self, part: u16, lbn: u64) -> Result<Node, FsError> {
        let (mut part, mut lbn) = (part, lbn);
        for _ in 0..8 {
            let b = self.block(part, lbn)?;
            let id = u16le(&b, 0);
            if !matches!(id, 259 | 261 | 266) || !tag_ok(&b, id) {
                return Err(FsError::Corrupt);
            }
            if id == 259 {
                let (_, _, l, p) = long_ad(&b, 36);
                (part, lbn) = (p, l);
                continue;
            }
            return self.parse_fe(&b, id, part);
        }
        Err(FsError::Corrupt)
    }

    fn parse_fe(&mut self, b: &[u8], id: u16, part: u16) -> Result<Node, FsError> {
        // File Entry (261) o Extended File Entry (266): desplazamientos de la fecha de modificación,
        // de L_EA / L_AD y del inicio de los descriptores.
        let (mt, lea, lad, base) = if id == 261 { (84, 168, 172, 176) } else { (92, 208, 212, 216) };
        let (ftype, flags, info) = (b[27], u16le(b, 34), u64le(b, 56));
        let at = base + u32le(b, lea) as usize;
        let end = at + u32le(b, lad) as usize;
        if end > b.len() || b.len() < base + 16 {
            return Err(FsError::Corrupt);
        }
        let ad = &b[at..end];
        let data = match flags & 7 {
            3 if info <= ad.len() as u64 => Data::Emb(ad[..info as usize].to_vec()),
            0 | 1 => Data::Runs(self.runs(ad, flags & 7 == 1, part)?),
            _ => return Err(FsError::Corrupt),
        };
        Ok(Node { dir: ftype == 4, shown: matches!(ftype, 0 | 4 | 5), size: info, date: timestamp(&b[mt..mt + 12]), data })
    }

    /// Contenido de un archivo o carpeta, acotado a `cap` y al tamaño del medio.
    fn content(&mut self, n: &Node, cap: u64) -> Result<Vec<u8>, FsError> {
        if n.size > cap || n.size > self.io.len * self.io.ss as u64 {
            return Err(FsError::Corrupt);
        }
        match &n.data {
            Data::Emb(v) => Ok(v.clone()),
            Data::Runs(r) => self.read_runs(&r.clone(), n.size),
        }
    }

    /// Entradas de una carpeta: (nombre, partición, bloque de su File Entry).
    fn fids(&mut self, dir: &Node) -> Result<Vec<(String, u16, u64)>, FsError> {
        if !dir.dir {
            return Err(FsError::NotDir);
        }
        let d = self.content(dir, MAXDIR)?;
        let (mut out, mut o) = (Vec::new(), 0);
        while o < d.len() {
            if o + 38 > d.len() {
                return Err(FsError::Corrupt);
            }
            let (liu, lfi) = (u16le(&d, o + 36) as usize, d[o + 19] as usize);
            let total = (38 + liu + lfi + 3) & !3;
            if o + total > d.len() || !tag_ok(&d[o..o + total], 257) {
                return Err(FsError::Corrupt);
            }
            let f = &d[o..o + total];
            o += total;
            // Característica: 4 = borrado, 8 = padre. Sin nombre no hay nada que listar.
            if f[18] & 12 != 0 || lfi == 0 {
                continue;
            }
            let (_, _, lbn, part) = long_ad(f, 20);
            let Ok(nombre) = cs0(&f[38 + liu..38 + liu + lfi]) else { continue };
            out.push((nombre, part, lbn));
        }
        Ok(out)
    }
}

struct Udf {
    c: Ctx,
    root: Node,
    label: String,
    total: u64,
}

/// Secuencia de descriptores de volumen: particiones (la de mayor número de secuencia por
/// número de partición) y el descriptor de volumen lógico de mayor secuencia.
fn read_vds(io: &mut Io, bs: u64, loc: u32, len: u32) -> Result<(Vec<(u32, u16, u64, u64)>, Vec<u8>), FsError> {
    let (mut pds, mut lv): (Vec<(u32, u16, u64, u64)>, Option<(u32, Vec<u8>)>) = (Vec::new(), None);
    for i in 0..(len as u64 / bs).min(256) {
        let b = read_at(io, (loc as u64 + i) * bs, bs as usize)?;
        let id = u16le(&b, 0);
        if id == 8 {
            break;
        }
        if !matches!(id, 5 | 6) || !tag_ok(&b, id) {
            continue;
        }
        let vdsn = u32le(&b, 16);
        if id == 5 {
            let p = (vdsn, u16le(&b, 22), u32le(&b, 188) as u64, u32le(&b, 192) as u64);
            match pds.iter_mut().find(|q| q.1 == p.1) {
                Some(q) if vdsn > q.0 => *q = p,
                Some(_) => {}
                None => pds.push(p),
            }
        } else if lv.as_ref().map_or(true, |l| vdsn > l.0) {
            lv = Some((vdsn, b));
        }
    }
    Ok((pds, lv.ok_or(FsError::Corrupt)?.1))
}

/// Busca el anclaje (sectores 256, N-256 y N) para un tamaño de bloque dado y lee su secuencia.
fn find_vds(io: &mut Io, bs: u64) -> Result<(Vec<(u32, u16, u64, u64)>, Vec<u8>), FsError> {
    let nb = io.len * io.ss as u64 / bs;
    let mut last = FsError::Corrupt;
    for a in [256, nb.wrapping_sub(256), nb.wrapping_sub(1)] {
        if a >= nb {
            continue;
        }
        let b = match read_at(io, a * bs, bs as usize) {
            Ok(b) if tag_ok(&b, 2) => b,
            Ok(_) => continue,
            Err(e) => {
                last = e;
                continue;
            }
        };
        // Secuencia principal y de reserva.
        for o in [16, 24] {
            match read_vds(io, bs, u32le(&b, o + 4), u32le(&b, o)) {
                Ok(r) if u32le(&r.1, 212) as u64 == bs => return Ok(r),
                Ok(_) => {}
                Err(e) => last = e,
            }
        }
    }
    Err(last)
}

pub fn mount(dev: Box<dyn SectorDev>, p: &Partition) -> Result<Box<dyn Volume>, FsError> {
    let ss = dev.sector_size();
    let mut io = Io { dev, start: p.start, len: p.sectors, ss };
    let mut found = Err(FsError::Corrupt);
    for bs in [2048u64, 512, 4096, 1024] {
        if bs >= ss as u64 {
            found = find_vds(&mut io, bs).map(|r| (bs, r));
            if found.is_ok() {
                break;
            }
        }
    }
    let (bs, (pds, lv)) = found?;
    let devblocks = io.len * ss as u64 / bs;
    let mut c = Ctx { io, bs, maps: Vec::new() };
    let mut total = 0;
    // Mapas de partición del descriptor de volumen lógico.
    let (tl, nm) = (u32le(&lv, 264) as usize, u32le(&lv, 268));
    if 440 + tl > lv.len() {
        return Err(FsError::Corrupt);
    }
    let t = &lv[440..440 + tl];
    let (mut o, mut metas) = (0, Vec::new());
    for _ in 0..nm {
        if o + 2 > t.len() || (t[o + 1] as usize) < 2 || o + t[o + 1] as usize > t.len() {
            return Err(FsError::Corrupt);
        }
        let m = &t[o..o + t[o + 1] as usize];
        o += m.len();
        let num = match m[0] {
            1 if m.len() >= 6 => u16le(m, 4),
            2 if m.len() >= 48 => u16le(m, 38),
            _ => return Err(FsError::Corrupt),
        };
        let &(_, _, start, len) = pds.iter().find(|q| q.1 == num).ok_or(FsError::Corrupt)?;
        if start >= devblocks {
            return Err(FsError::Corrupt);
        }
        let blocks = len.min(devblocks - start);
        total = total.max(blocks * bs);
        if m[0] == 2 {
            if m[5..28].starts_with(b"*UDF Virtual") {
                return Err(FsError::Corrupt);
            }
            if m[5..28].starts_with(b"*UDF Metadata") {
                metas.push((c.maps.len(), u32le(m, 40) as u64, u32le(m, 44) as u64));
            }
        }
        c.maps.push(Map::Phys { start, blocks });
    }
    // La partición de metadatos se resuelve con el archivo de metadatos (o su espejo), que
    // direcciona la partición física de debajo.
    for (i, loc, mirror) in metas {
        let n = match c.load(i as u16, loc) {
            Ok(n) => n,
            Err(_) => c.load(i as u16, mirror)?,
        };
        match n.data {
            Data::Runs(r) => c.maps[i] = Map::Meta(r),
            Data::Emb(_) => return Err(FsError::Corrupt),
        }
    }
    // File Set Descriptor → carpeta raíz.
    let (_, _, lbn, part) = long_ad(&lv, 248);
    let fsd = c.block(part, lbn)?;
    if !tag_ok(&fsd, 256) {
        return Err(FsError::Corrupt);
    }
    let (_, _, rl, rp) = long_ad(&fsd, 400);
    let root = c.load(rp, rl)?;
    if !root.dir {
        return Err(FsError::Corrupt);
    }
    let label = dstring(&lv[84..212]).unwrap_or_default();
    Ok(Box::new(Udf { c, root, label, total }))
}

impl Udf {
    fn walk(&mut self, path: &str) -> Result<Node, FsError> {
        let mut cur = self.root.clone();
        for comp in split(path) {
            let (_, part, lbn) = self.c.fids(&cur)?.into_iter().find(|f| same_name(&f.0, comp)).ok_or(FsError::NotFound)?;
            cur = self.c.load(part, lbn)?;
            if !cur.shown {
                return Err(FsError::NotFound);
            }
        }
        Ok(cur)
    }
}

fn entry_of(name: String, n: &Node) -> Entry {
    Entry { name, dir: n.dir, size: if n.dir { 0 } else { n.size }, modified: n.date }
}

impl Volume for Udf {
    fn kind(&self) -> FsKind {
        FsKind::Udf
    }
    fn label(&self) -> String {
        self.label.clone()
    }
    fn space(&mut self) -> (u64, u64) {
        (self.total, 0)
    }
    fn read_only(&self) -> bool {
        true
    }
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, FsError> {
        let d = self.walk(dir)?;
        let mut out = Vec::new();
        for (name, part, lbn) in self.c.fids(&d)? {
            // Una entrada dañada no esconde el resto de la carpeta.
            let Ok(n) = self.c.load(part, lbn) else { continue };
            if n.shown {
                out.push(entry_of(name, &n));
            }
        }
        Ok(out)
    }
    fn stat(&mut self, path: &str) -> Result<Entry, FsError> {
        let n = self.walk(path)?;
        let name = split(path).last().map(|s| String::from(*s)).unwrap_or_default();
        Ok(entry_of(name, &n))
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        let n = self.walk(path)?;
        if n.dir {
            return Err(FsError::IsDir);
        }
        // shortcut: un archivo disperso mayor que el medio se rechaza como dañado, si hiciera falta se leería por trozos.
        self.c.content(&n, u64::MAX)
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
mod tests {
    use super::*;
    use crate::vol::{partitions, MemSectors};
    use alloc::vec;

    /// Pone etiqueta (id, CRC, suma) a un descriptor cuyo cuerpo empieza en el byte 16.
    fn desc(id: u16, mut b: Vec<u8>) -> Vec<u8> {
        b[0..2].copy_from_slice(&id.to_le_bytes());
        b[2..4].copy_from_slice(&3u16.to_le_bytes());
        let n = (b.len() - 16) as u16;
        let crc = crc16(&b[16..]);
        b[8..10].copy_from_slice(&crc.to_le_bytes());
        b[10..12].copy_from_slice(&n.to_le_bytes());
        b[4] = 0;
        b[4] = b[..16].iter().fold(0u8, |a, &x| a.wrapping_add(x));
        b
    }

    fn short_ad(len: u32, pos: u32) -> Vec<u8> {
        [len.to_le_bytes(), pos.to_le_bytes()].concat()
    }
    fn long_ad_b(len: u32, lbn: u32, part: u16) -> Vec<u8> {
        let mut v = [len.to_le_bytes(), lbn.to_le_bytes()].concat();
        v.extend(part.to_le_bytes());
        v.extend([0u8; 6]);
        v
    }
    fn ts() -> [u8; 12] {
        let mut t = [0u8; 12];
        t[0] = 1; // tipo 0, zona 1 minuto
        t[2..4].copy_from_slice(&2026u16.to_le_bytes());
        t[4..9].copy_from_slice(&[11, 18, 10, 30, 45]);
        t
    }
    /// File Entry (261) o Extended (266) de `ftype`, tipo de asignación `at` y descriptores `ads`.
    fn fe(ext: bool, ftype: u8, at: u16, info: u64, ads: &[u8]) -> Vec<u8> {
        let (mt, lad, base) = if ext { (92, 212, 216) } else { (84, 172, 176) };
        let mut b = vec![0u8; base];
        b[27] = ftype;
        b[34..36].copy_from_slice(&at.to_le_bytes());
        b[56..64].copy_from_slice(&info.to_le_bytes());
        b[mt..mt + 12].copy_from_slice(&ts());
        b[lad..lad + 4].copy_from_slice(&(ads.len() as u32).to_le_bytes());
        b.extend(ads);
        desc(if ext { 266 } else { 261 }, b)
    }
    fn name_cs0(s: &str) -> Vec<u8> {
        if s.chars().all(|c| (c as u32) < 256) {
            let mut v = vec![8];
            v.extend(s.chars().map(|c| c as u8));
            v
        } else {
            let mut v = vec![16];
            v.extend(s.encode_utf16().flat_map(|u| u.to_be_bytes()));
            v
        }
    }
    fn fid(name: &str, dir: bool, part: u16, lbn: u32) -> Vec<u8> {
        let n = name_cs0(name);
        let mut b = vec![0u8; 38];
        b[18] = if dir { 2 } else { 0 };
        b[19] = n.len() as u8;
        b[20..36].copy_from_slice(&long_ad_b(2048, lbn, part));
        b.extend(n);
        while b.len() % 4 != 0 {
            b.push(0);
        }
        desc(257, b)
    }
    fn parent_fid() -> Vec<u8> {
        let mut b = vec![0u8; 40];
        b[18] = 10; // padre + directorio
        desc(257, b)
    }

    struct Img {
        d: Vec<u8>,
        bs: usize,
    }
    impl Img {
        fn new(blocks: usize, bs: usize) -> Img {
            Img { d: vec![0; blocks * bs], bs }
        }
        fn put(&mut self, blk: usize, b: &[u8]) {
            self.d[blk * self.bs..blk * self.bs + b.len()].copy_from_slice(b);
        }
    }

    /// Reconocimiento (BEA01/NSR02/TEA01), anclaje, secuencia de volumen y partición física
    /// de 64 bloques en el bloque `p0`; `maps` son los mapas del LVD (bytes ya hechos).
    fn base(bs: usize, p0: usize, maps: &[u8], nmaps: u32, fsd_part: u16) -> Img {
        let mut m = Img::new(p0 + 80, bs);
        for (i, id) in [b"BEA01", b"NSR02", b"TEA01"].iter().enumerate() {
            m.d[32768 + i * 2048 + 1..32768 + i * 2048 + 6].copy_from_slice(*id);
        }
        let mut avdp = vec![0u8; 512];
        avdp[16..20].copy_from_slice(&(6 * bs as u32).to_le_bytes());
        avdp[20..24].copy_from_slice(&32u32.to_le_bytes());
        avdp[24..28].copy_from_slice(&(6 * bs as u32).to_le_bytes());
        avdp[28..32].copy_from_slice(&40u32.to_le_bytes());
        m.put(256, &desc(2, avdp));
        let mut pd = vec![0u8; 512];
        pd[16..20].copy_from_slice(&1u32.to_le_bytes());
        pd[22..24].copy_from_slice(&0u16.to_le_bytes());
        pd[188..192].copy_from_slice(&(p0 as u32).to_le_bytes());
        pd[192..196].copy_from_slice(&64u32.to_le_bytes());
        let mut lv = vec![0u8; 440];
        lv[16..20].copy_from_slice(&1u32.to_le_bytes());
        let l = name_cs0("Mi UDF");
        lv[84..84 + l.len()].copy_from_slice(&l);
        lv[84 + 127] = l.len() as u8;
        lv[212..216].copy_from_slice(&(bs as u32).to_le_bytes());
        lv[248..264].copy_from_slice(&long_ad_b(bs as u32, 0, fsd_part));
        lv[264..268].copy_from_slice(&(maps.len() as u32).to_le_bytes());
        lv[268..272].copy_from_slice(&nmaps.to_le_bytes());
        lv.extend(maps);
        let td = vec![0u8; 512];
        for base_blk in [32, 40] {
            m.put(base_blk, &desc(5, pd.clone()));
            m.put(base_blk + 1, &desc(6, lv.clone()));
            m.put(base_blk + 2, &desc(8, td.clone()));
        }
        m
    }
    fn map1(num: u16) -> Vec<u8> {
        let mut v = vec![1, 6, 0, 0];
        v.extend(num.to_le_bytes());
        v
    }
    fn fsd(root_part: u16, root_lbn: u32) -> Vec<u8> {
        let mut b = vec![0u8; 512];
        b[400..416].copy_from_slice(&long_ad_b(2048, root_lbn, root_part));
        desc(256, b)
    }

    fn pattern(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 249) as u8).collect()
    }

    /// UDF 2.01 con carpetas, un archivo de asignación corta (3 bloques, el tercero parcial),
    /// uno embebido y otro en una subcarpeta con Extended File Entry.
    fn udf201(bs: usize) -> Img {
        let p0 = 300;
        let mut m = base(bs, p0, &map1(0), 1, 0);
        let b = |lbn: usize| (p0 + lbn) * bs;
        let size = 2 * bs + 100;
        // Partición: 0 FSD, 1 raíz FE, 2 raíz datos, 3 FE subcarpeta, 4 datos subcarpeta,
        // 5 FE archivo, 6..8 datos, 9 FE embebido, 10 FE archivo en subcarpeta, 11 datos.
        let put = |m: &mut Img, lbn: usize, v: &[u8]| m.d[b(lbn)..b(lbn) + v.len()].copy_from_slice(v);
        put(&mut m, 0, &fsd(0, 1));
        let root = [parent_fid(), fid("Año.txt", false, 0, 5), fid("pequeño", false, 0, 9), fid("Carpeta ñ", true, 0, 3)].concat();
        put(&mut m, 1, &fe(false, 4, 0, root.len() as u64, &short_ad(root.len() as u32, 2)));
        put(&mut m, 2, &root);
        let sub = [parent_fid(), fid("dentro.bin", false, 0, 10)].concat();
        put(&mut m, 3, &fe(true, 4, 0, sub.len() as u64, &short_ad(sub.len() as u32, 4)));
        put(&mut m, 4, &sub);
        put(&mut m, 5, &fe(false, 5, 0, size as u64, &short_ad(size as u32, 6)));
        put(&mut m, 6, &pattern(size));
        put(&mut m, 9, &fe(false, 5, 3, 4, b"hola"));
        put(&mut m, 10, &fe(true, 5, 0, 7, &short_ad(7, 11)));
        put(&mut m, 11, b"abcdefg");
        m
    }

    fn mount_udf(img: Vec<u8>, ss: usize) -> Result<Box<dyn Volume>, FsError> {
        let mut d = MemSectors::new(0, ss);
        d.data = img;
        let p = partitions(&mut d)?.into_iter().find(|p| p.kind == FsKind::Udf).ok_or(FsError::Corrupt)?;
        super::mount(Box::new(d), &p)
    }

    fn check201(v: &mut dyn Volume, bs: usize) {
        assert_eq!((v.kind(), v.label(), v.read_only()), (FsKind::Udf, "Mi UDF".into(), true));
        assert_eq!(v.space().1, 0);
        let l = v.list("").unwrap();
        assert_eq!(l.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["Año.txt", "pequeño", "Carpeta ñ"]);
        assert_eq!((l[0].size, l[0].dir, l[2].dir), (2 * bs as u64 + 100, false, true));
        assert_eq!((l[0].modified.year, l[0].modified.month, l[0].modified.hour, l[0].modified.second), (2026, 11, 10, 45));
        assert_eq!(v.read("AÑO.TXT").unwrap(), pattern(2 * bs + 100));
        assert_eq!(v.read("PEQUEÑO").unwrap(), b"hola");
        assert_eq!(v.read("carpeta Ñ\\DENTRO.bin").unwrap(), b"abcdefg");
        assert_eq!(v.stat("carpeta ñ").unwrap().dir, true);
        assert_eq!(v.read("carpeta ñ"), Err(FsError::IsDir));
        assert_eq!(v.read("nada"), Err(FsError::NotFound));
        assert_eq!(v.list("año.txt"), Err(FsError::NotDir));
        assert_eq!(v.write("a", b"", DateTime::default()), Err(FsError::Denied));
        assert_eq!(v.mkdir("a", DateTime::default()), Err(FsError::Denied));
        assert_eq!(v.rename("a", "b"), Err(FsError::Denied));
        assert_eq!(v.delete("a"), Err(FsError::Denied));
        assert_eq!(v.flush(), Ok(()));
    }

    #[test]
    fn udf_201_blocks() {
        for (bs, ss) in [(2048, 2048), (2048, 512), (512, 512)] {
            let mut v = mount_udf(udf201(bs).d, ss).unwrap_or_else(|e| panic!("bs={bs} ss={ss}: {e:?}"));
            check201(&mut *v, bs);
        }
    }

    /// UDF 2.50: partición de metadatos (mapa tipo 2) con el archivo de metadatos en la partición
    /// física; el contenido de los archivos sigue en la física (asignación larga).
    fn udf250() -> Img {
        let (bs, p0) = (2048usize, 300usize);
        let mut maps = map1(0);
        let mut m2 = vec![2u8, 64, 0, 0];
        let mut id = vec![0u8; 32];
        id[1..1 + 23].copy_from_slice(b"*UDF Metadata Partition");
        m2.extend(id);
        m2.extend(0u16.to_le_bytes()); // número de secuencia
        m2.extend(0u16.to_le_bytes()); // partición física 0
        m2.extend(0u32.to_le_bytes()); // archivo de metadatos: bloque 0 de la física
        m2.extend(0u32.to_le_bytes());
        m2.resize(64, 0);
        maps.extend(m2);
        let mut m = base(bs, p0, &maps, 2, 1);
        let b = |lbn: usize| (p0 + lbn) * bs;
        let put = |m: &mut Img, lbn: usize, v: &[u8]| m.d[b(lbn)..b(lbn) + v.len()].copy_from_slice(v);
        // Física: 0 = FE del archivo de metadatos (bloques 8..24), 30.. datos.
        put(&mut m, 0, &fe(false, 250, 0, 16 * bs as u64, &short_ad(16 * bs as u32, 8)));
        // Metadatos (lbn 0.. → física 8..): 0 FSD, 1 raíz FE, 2 raíz, 3 FE archivo.
        put(&mut m, 8, &fsd(1, 1));
        let root = [parent_fid(), fid("datos.txt", false, 1, 3)].concat();
        put(&mut m, 9, &fe(true, 4, 0, root.len() as u64, &short_ad(root.len() as u32, 2)));
        put(&mut m, 10, &root);
        put(&mut m, 11, &fe(true, 5, 1, 5000, &long_ad_b(5000, 30, 0)));
        put(&mut m, 30, &pattern(5000));
        m
    }

    #[test]
    fn udf_250_metadata_partition() {
        let mut v = mount_udf(udf250().d, 2048).unwrap();
        assert_eq!(v.list("").unwrap().len(), 1);
        assert_eq!(v.read("DATOS.TXT").unwrap(), pattern(5000));
    }

    #[test]
    fn cs0_names() {
        assert_eq!(cs0(&[8, b'a', 0xF1]).unwrap(), "añ");
        assert_eq!(cs0(&[16, 0, b'a', 0xD8, 0x3D, 0xDE, 0x00]).unwrap(), "a\u{1F600}");
        assert_eq!(cs0(&[9, 1]), Err(FsError::Corrupt));
        assert_eq!(cs0(&[16, 0]), Err(FsError::Corrupt));
    }

    #[test]
    fn corrupt_cases() {
        let ok = udf201(2048);
        // CRC de la etiqueta del LVD roto en las dos secuencias: sin volumen lógico.
        let mut d = ok.d.clone();
        d[33 * 2048 + 30] ^= 1;
        d[41 * 2048 + 30] ^= 1;
        assert_eq!(mount_udf(d, 2048).err(), Some(FsError::Corrupt));
        // Solo la principal rota: se usa la de reserva.
        let mut d = ok.d.clone();
        d[33 * 2048 + 30] ^= 1;
        assert!(mount_udf(d, 2048).is_ok());
        // Sin anclaje.
        let mut d = ok.d.clone();
        d[256 * 2048] = 0;
        assert_eq!(mount_udf(d, 2048).err(), Some(FsError::Corrupt));
        // Archivo embebido con longitud mayor que el espacio.
        let p0 = 300;
        let mut d = ok.d.clone();
        let e = fe(false, 5, 3, 9999, b"hola");
        let o = (p0 + 9) * 2048;
        d[o..o + e.len()].copy_from_slice(&e);
        let mut v = mount_udf(d, 2048).unwrap();
        assert_eq!(v.read("pequeño"), Err(FsError::Corrupt));
        // Carpeta que se contiene a sí misma: las rutas son finitas.
        let mut d = ok.d.clone();
        let root = [parent_fid(), fid("yo", true, 0, 1)].concat();
        let o = (p0 + 2) * 2048;
        d[o..o + 2048].fill(0);
        d[o..o + root.len()].copy_from_slice(&root);
        let e = fe(false, 4, 0, root.len() as u64, &short_ad(root.len() as u32, 2));
        let o = (p0 + 1) * 2048;
        d[o..o + e.len()].copy_from_slice(&e);
        let mut v = mount_udf(d, 2048).unwrap();
        assert!(v.stat("yo\\yo\\yo\\yo").unwrap().dir);
        // Continuación de descriptores que se apunta a sí misma: tope, no bucle.
        let mut d = ok.d.clone();
        let mut aed = vec![0u8; 24];
        aed[20..24].copy_from_slice(&8u32.to_le_bytes());
        aed.extend(short_ad(8 | 3 << 30, 12));
        let aed = desc(258, aed);
        let o = (p0 + 12) * 2048;
        d[o..o + aed.len()].copy_from_slice(&aed);
        let e = fe(false, 5, 0, 10, &short_ad(8 | 3 << 30, 12));
        let o = (p0 + 5) * 2048;
        d[o..o + e.len()].copy_from_slice(&e);
        let mut v = mount_udf(d, 2048).unwrap();
        assert!(v.read("año.txt").is_err());
        // Archivo con tamaño mayor que el medio.
        let mut d = ok.d.clone();
        let e = fe(false, 5, 0, u64::MAX >> 4, &short_ad(0x3FFF_F800, 6));
        let o = (p0 + 5) * 2048;
        d[o..o + e.len()].copy_from_slice(&e);
        let mut v = mount_udf(d, 2048).unwrap();
        assert_eq!(v.read("año.txt"), Err(FsError::Corrupt));
    }

    #[test]
    fn fuzz_no_panic() {
        for img in [udf201(2048), udf250()] {
            let bs = img.bs;
            for blk in [32usize, 33, 34, 256, 300, 301, 302, 303, 304, 305, 308, 309, 310, 311, 312] {
                for i in 0..96 {
                    for x in [0xFFu8, 0x00, 0x7F] {
                        let mut d = img.d.clone();
                        d[blk * bs + i] = x;
                        if let Ok(mut v) = mount_udf(d, 2048) {
                            for p in ["", "año.txt", "carpeta ñ", "carpeta ñ\\dentro.bin", "pequeño", "datos.txt"] {
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
}
