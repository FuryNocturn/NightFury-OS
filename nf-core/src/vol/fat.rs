// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! FAT12, FAT16 y FAT32 con nombres largos VFAT, según la especificación pública de Microsoft
//! (fatgen103). Los datos del disco son entrada hostil: todo se valida y las cadenas de clusters
//! llevan tope contra bucles.
//!
//! Escritura segura: primero los datos, luego la cadena en la FAT (todas las copias) y al final la
//! entrada de directorio; al borrar, al revés (entrada primero y luego los clusters).
use super::part::{self, u16le, u32le};
use super::{FsKind, Partition, SectorDev, Volume};
use crate::fs::{Entry, FsError};
use crate::platform::DateTime;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Tamaño máximo de un directorio FAT (65536 entradas de 32 bytes).
const MAXDIR: usize = 1 << 21;

/// Sectores de una partición (relativos a su inicio, con límites comprobados).
pub(super) struct Io {
    pub dev: Box<dyn SectorDev>,
    pub start: u64,
    pub len: u64,
    pub ss: usize,
}

impl Io {
    pub fn read(&mut self, sec: u64, n: usize) -> Result<Vec<u8>, FsError> {
        if sec.checked_add(n as u64).map_or(true, |e| e > self.len) {
            return Err(FsError::Corrupt);
        }
        let mut b = Vec::new();
        b.try_reserve_exact(n * self.ss).map_err(|_| FsError::NoSpace)?;
        b.resize(n * self.ss, 0);
        self.dev.read(self.start + sec, &mut b)?;
        Ok(b)
    }
    pub fn write(&mut self, sec: u64, b: &[u8]) -> Result<(), FsError> {
        if sec.checked_add((b.len() / self.ss) as u64).map_or(true, |e| e > self.len) {
            return Err(FsError::Corrupt);
        }
        self.dev.write(self.start + sec, b)
    }
}

/// Lee los clusters `cl` de un área de datos que empieza en el sector `start` (juntando los
/// contiguos en una sola lectura). Comparte código con exFAT.
pub(super) fn read_cl(io: &mut Io, start: u64, spc: u64, cl: &[u32]) -> Result<Vec<u8>, FsError> {
    let cs = spc as usize * io.ss;
    let mut out = Vec::new();
    out.try_reserve_exact(cl.len() * cs).map_err(|_| FsError::NoSpace)?;
    let mut i = 0;
    while i < cl.len() {
        let mut j = i + 1;
        while j < cl.len() && cl[j] == cl[j - 1] + 1 {
            j += 1;
        }
        let sec = (cl[i] as u64).checked_sub(2).ok_or(FsError::Corrupt)? * spc + start;
        out.extend_from_slice(&io.read(sec, (j - i) * spc as usize)?);
        i = j;
    }
    Ok(out)
}

/// Escribe `data` (rellenando con ceros el último cluster) en los clusters `cl`.
pub(super) fn write_cl(io: &mut Io, start: u64, spc: u64, cl: &[u32], data: &[u8]) -> Result<(), FsError> {
    let cs = spc as usize * io.ss;
    let mut i = 0;
    while i < cl.len() {
        let mut j = i + 1;
        while j < cl.len() && cl[j] == cl[j - 1] + 1 {
            j += 1;
        }
        let mut buf = data[i * cs..(j * cs).min(data.len())].to_vec();
        buf.resize((j - i) * cs, 0);
        let sec = (cl[i] as u64).checked_sub(2).ok_or(FsError::Corrupt)? * spc + start;
        io.write(sec, &buf)?;
        i = j;
    }
    Ok(())
}

/// Escribe ceros en `n` sectores desde `lba` (absoluto del disco).
pub(super) fn zero_sectors(dev: &mut dyn SectorDev, lba: u64, n: u64) -> Result<(), FsError> {
    let ss = dev.sector_size();
    let z = vec![0u8; ss * 64];
    let mut i = 0;
    while i < n {
        let k = (n - i).min(64);
        dev.write(lba + i, &z[..k as usize * ss])?;
        i += k;
    }
    Ok(())
}

/// Una FAT (todas sus copias) con caché de sectores y escritura diferida hasta `flush`.
pub(super) struct Table {
    /// Primer sector de la copia que se lee y de cada copia que se escribe.
    pub base: u64,
    pub copies: Vec<u64>,
    pub sectors: u64,
    map: BTreeMap<u64, (Vec<u8>, bool)>,
}

impl Table {
    pub fn new(base: u64, copies: Vec<u64>, sectors: u64) -> Table {
        Table { base, copies, sectors, map: BTreeMap::new() }
    }
    fn slot(&mut self, io: &mut Io, i: u64) -> Result<&mut (Vec<u8>, bool), FsError> {
        if i >= self.sectors {
            return Err(FsError::Corrupt);
        }
        if !self.map.contains_key(&i) {
            if self.map.len() >= 256 {
                self.flush(io)?;
                self.map.clear();
            }
            let v = io.read(self.base + i, 1)?;
            self.map.insert(i, (v, false));
        }
        Ok(self.map.get_mut(&i).unwrap())
    }
    /// `n` bytes (1..=4) little-endian desde `off`.
    pub fn get(&mut self, io: &mut Io, off: u64, n: usize) -> Result<u32, FsError> {
        let ss = io.ss as u64;
        let mut v = 0u32;
        for k in 0..n as u64 {
            v |= (self.slot(io, (off + k) / ss)?.0[((off + k) % ss) as usize] as u32) << (8 * k);
        }
        Ok(v)
    }
    pub fn set(&mut self, io: &mut Io, off: u64, n: usize, v: u32) -> Result<(), FsError> {
        let ss = io.ss as u64;
        for k in 0..n as u64 {
            let s = self.slot(io, (off + k) / ss)?;
            let b = (v >> (8 * k)) as u8;
            let o = ((off + k) % ss) as usize;
            if s.0[o] != b {
                s.0[o] = b;
                s.1 = true;
            }
        }
        Ok(())
    }
    pub fn flush(&mut self, io: &mut Io) -> Result<(), FsError> {
        for (i, (v, dirty)) in self.map.iter_mut() {
            if *dirty {
                for c in &self.copies {
                    io.write(c + i, v)?;
                }
                *dirty = false;
            }
        }
        Ok(())
    }
}

// --- nombres y fechas, compartidos con exFAT ---

/// Nombre admisible en FAT y exFAT: 1..=255 unidades UTF-16, sin `"*/:<>?\|` ni controles, sin
/// terminar en espacio o punto (así tampoco valen "." ni "..").
pub(super) fn valid_name(n: &str) -> bool {
    let u = n.encode_utf16().count();
    u > 0 && u <= 255 && !n.ends_with(' ') && !n.ends_with('.') && !n.chars().any(|c| c < ' ' || "\"*/:<>?\\|".contains(c))
}

/// Mayúsculas de un carácter (ñ, acentos…) para comparar sin distinguir.
pub(super) fn upper(c: char) -> char {
    let mut u = c.to_uppercase();
    match (u.next(), u.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

pub(super) fn same_name(a: &str, b: &str) -> bool {
    a.chars().map(upper).eq(b.chars().map(upper))
}

/// Separa una ruta en componentes (sin vacíos).
pub(super) fn split(path: &str) -> Vec<&str> {
    path.split('\\').filter(|s| !s.is_empty()).collect()
}

/// Fecha y hora FAT empaquetadas (fecha en los 16 bits altos): el mismo formato que usa exFAT.
pub(super) fn pack_time(d: DateTime) -> u32 {
    let y = (d.year.clamp(1980, 2107) - 1980) as u32;
    let date = y << 9 | (d.month.clamp(1, 12) as u32) << 5 | d.day.clamp(1, 31) as u32;
    let time = (d.hour.min(23) as u32) << 11 | (d.minute.min(59) as u32) << 5 | (d.second.min(59) as u32) / 2;
    date << 16 | time
}

pub(super) fn unpack_time(v: u32) -> DateTime {
    let (date, time) = (v >> 16, v & 0xFFFF);
    DateTime {
        year: 1980 + (date >> 9) as u16,
        month: (((date >> 5) & 15) as u8).clamp(1, 12),
        day: ((date & 31) as u8).clamp(1, 31),
        hour: ((time >> 11) as u8).min(23),
        minute: (((time >> 5) & 63) as u8).min(59),
        second: (((time & 31) * 2) as u8).min(59),
    }
}

// --- geometría ---

#[derive(Clone, Copy)]
struct Geo {
    kind: FsKind,
    total: u64,
    spc: u64,
    rsvd: u64,
    nfats: u64,
    fatsz: u64,
    root_secs: u64,
    root_clus: u32,
    fsinfo: u64,
    count: u32,
    flags: u16,
}

impl Geo {
    /// Interpreta el sector de arranque (según el cálculo de clusters de la especificación, no por
    /// la etiqueta de texto).
    fn parse(s: &[u8], ss: usize) -> Result<Geo, FsError> {
        if s.len() < 512 || s[510] != 0x55 || s[511] != 0xAA || u16le(s, 11) as usize != ss {
            return Err(FsError::Corrupt);
        }
        let (spc, rsvd, nfats) = (s[13] as u64, u16le(s, 14) as u64, s[16] as u64);
        let root_ent = u16le(s, 17) as u64;
        let tot16 = u16le(s, 19) as u64;
        let total = if tot16 != 0 { tot16 } else { u32le(s, 32) as u64 };
        let fat16 = u16le(s, 22) as u64;
        let fatsz = if fat16 != 0 { fat16 } else { u32le(s, 36) as u64 };
        if !spc.is_power_of_two() || spc > 128 || rsvd == 0 || !(1..=2).contains(&nfats) || total == 0 || fatsz == 0 || s[21] < 0xF0 {
            return Err(FsError::Corrupt);
        }
        let root_secs = (root_ent * 32).div_ceil(ss as u64);
        let overhead = rsvd + nfats * fatsz + root_secs;
        if overhead >= total {
            return Err(FsError::Corrupt);
        }
        let count = (total - overhead) / spc;
        if count == 0 || count > 0x0FFF_FFF5 {
            return Err(FsError::Corrupt);
        }
        let kind = if count < 4085 {
            FsKind::Fat12
        } else if count < 65525 {
            FsKind::Fat16
        } else {
            FsKind::Fat32
        };
        let bits = match kind {
            FsKind::Fat12 => 12,
            FsKind::Fat16 => 16,
            _ => 32,
        };
        if (count + 2) * bits > fatsz * ss as u64 * 8 {
            return Err(FsError::Corrupt);
        }
        let (mut root_clus, mut fsinfo, mut flags) = (0, 0, 0);
        if kind == FsKind::Fat32 {
            root_clus = u32le(s, 44);
            fsinfo = u16le(s, 48) as u64;
            flags = u16le(s, 40);
            if root_ent != 0 || fat16 != 0 || root_clus < 2 || root_clus as u64 > count as u64 + 1 {
                return Err(FsError::Corrupt);
            }
        } else if root_ent == 0 {
            return Err(FsError::Corrupt);
        }
        Ok(Geo { kind, total, spc, rsvd, nfats, fatsz, root_secs, root_clus, fsinfo, count: count as u32, flags })
    }
    fn root_start(&self) -> u64 {
        self.rsvd + self.nfats * self.fatsz
    }
    fn data_start(&self) -> u64 {
        self.root_start() + self.root_secs
    }
}

/// ¿Es este sector 0 el arranque de un FAT válido? (lo usa la detección de particiones).
pub(super) fn detect(s0: &[u8], ss: usize) -> Option<FsKind> {
    Geo::parse(s0, ss).ok().map(|g| g.kind)
}

// --- directorios ---

/// Entrada de directorio ya interpretada (con sus entradas LFN).
#[derive(Clone)]
struct Item {
    name: String,
    /// Índice de la primera entrada (LFN incluidas) y número de entradas.
    at: usize,
    n: usize,
    /// La entrada corta tal cual está en disco.
    raw: [u8; 32],
}

impl Item {
    fn dir(&self) -> bool {
        self.raw[11] & 0x10 != 0
    }
}

/// Un directorio cargado entero en memoria. `cl` vacío = raíz fija de FAT12/16.
struct DirBuf {
    data: Vec<u8>,
    orig: Vec<u8>,
    cl: Vec<u32>,
}

fn lfn_sum(s: &[u8]) -> u8 {
    s[..11].iter().fold(0u8, |a, &b| (a >> 1 | a << 7).wrapping_add(b))
}

fn short_text(raw: &[u8]) -> String {
    let conv = |b: &[u8], lower: bool| -> String {
        let mut s: String = b.iter().map(|&c| if lower { (c as char).to_ascii_lowercase() } else { c as char }).collect();
        s.truncate(s.trim_end_matches(' ').len());
        s
    };
    let mut base = raw[..8].to_vec();
    if base[0] == 0x05 {
        base[0] = 0xE5;
    }
    let (b, e) = (conv(&base, raw[12] & 8 != 0), conv(&raw[8..11], raw[12] & 16 != 0));
    if e.is_empty() { b } else { alloc::format!("{b}.{e}") }
}

/// Caracteres válidos en un nombre corto 8.3 además de letras y cifras.
fn short_ok(c: char) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit() || "!#$%&'()-@^_`{}~".contains(c)
}

/// Nombre corto 8.3 para `name` sin chocar con `taken`. Devuelve (nombre, ¿basta sin LFN?).
fn make_short(name: &str, taken: &[[u8; 11]]) -> Result<([u8; 11], bool), FsError> {
    let taken: alloc::collections::BTreeSet<[u8; 11]> = taken.iter().copied().collect();
    let trimmed = name.trim_start_matches(['.', ' ']);
    let mut lossy = trimmed.len() != name.len();
    let (b, e) = match trimmed.rsplit_once('.') {
        Some((b, e)) => (b, e),
        None => (trimmed, ""),
    };
    let mut conv = |s: &str, max: usize| -> String {
        let mut o = String::new();
        for c in s.chars() {
            let u = upper(c);
            if u == ' ' || u == '.' {
                lossy = true;
            } else if short_ok(u) {
                o.push(u);
            } else {
                o.push('_');
                lossy = true;
            }
        }
        if o.len() > max {
            o.truncate(max);
            lossy = true;
        }
        o
    };
    let (mut base, ext) = (conv(b, 8), conv(e, 3));
    if base.is_empty() {
        base.push('_');
    }
    let fits = |base: &str| {
        let mut r = [b' '; 11];
        r[..base.len()].copy_from_slice(base.as_bytes());
        r[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
        r
    };
    let exact = |r: &[u8; 11]| short_text(&{
        let mut x = [0u8; 32];
        x[..11].copy_from_slice(r);
        x
    }) == name;
    if !lossy {
        let r = fits(&base);
        if !taken.contains(&r) {
            let ex = exact(&r);
            return Ok((r, ex));
        }
    }
    for n in 1..=999_999u32 {
        let tail = alloc::format!("~{n}");
        let keep = base.len().min(8 - tail.len());
        let r = fits(&alloc::format!("{}{}", &base[..keep], tail));
        if !taken.contains(&r) {
            return Ok((r, false));
        }
    }
    Err(FsError::NoSpace)
}

fn lfn_entries(units: &[u16], chk: u8) -> Vec<[u8; 32]> {
    let n = units.len().div_ceil(13);
    let mut out = Vec::new();
    for k in (0..n).rev() {
        let mut e = [0u8; 32];
        e[0] = (k + 1) as u8 | if k == n - 1 { 0x40 } else { 0 };
        e[11] = 0x0F;
        e[13] = chk;
        for j in 0..13 {
            let i = k * 13 + j;
            let u = if i < units.len() { units[i] } else if i == units.len() { 0 } else { 0xFFFF };
            let o = match j {
                0..=4 => 1 + 2 * j,
                5..=10 => 14 + 2 * (j - 5),
                _ => 28 + 2 * (j - 11),
            };
            e[o..o + 2].copy_from_slice(&u.to_le_bytes());
        }
        out.push(e);
    }
    out
}

/// Primer hueco de `k` entradas libres seguidas (lo que hay tras el marcador de fin cuenta libre).
fn find_run(data: &[u8], k: usize) -> Option<usize> {
    let total = data.len() / 32;
    let end = (0..total).find(|&i| data[i * 32] == 0).unwrap_or(total);
    let mut run = 0;
    for i in 0..total {
        if i >= end || data[i * 32] == 0xE5 {
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

fn new_raw(attr: u8, first: u32, size: u32, now: DateTime) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[11] = attr;
    let t = pack_time(now);
    for o in [14, 22] {
        e[o..o + 2].copy_from_slice(&(t as u16).to_le_bytes());
        e[o + 2..o + 4].copy_from_slice(&((t >> 16) as u16).to_le_bytes());
    }
    e[18..20].copy_from_slice(&((t >> 16) as u16).to_le_bytes());
    set_first(&mut e, first);
    e[28..32].copy_from_slice(&size.to_le_bytes());
    e
}

fn set_first(e: &mut [u8], c: u32) {
    e[20..22].copy_from_slice(&((c >> 16) as u16).to_le_bytes());
    e[26..28].copy_from_slice(&(c as u16).to_le_bytes());
}

// --- el volumen ---

struct Fat {
    io: Io,
    g: Geo,
    t: Table,
    cs: usize,
    free: u32,
    next: u32,
    dirty: bool,
    ro: bool,
    label: String,
    fsinfo_ok: bool,
}

pub fn mount(dev: Box<dyn SectorDev>, p: &Partition) -> Result<Box<dyn Volume>, FsError> {
    let ss = dev.sector_size();
    let ro = dev.read_only();
    let mut io = Io { dev, start: p.start, len: p.sectors, ss };
    let g = Geo::parse(&io.read(0, 1)?, ss)?;
    if g.total > p.sectors {
        return Err(FsError::Corrupt);
    }
    let active = if g.kind == FsKind::Fat32 && g.flags & 0x80 != 0 { (g.flags & 15) as u64 } else { 0 };
    let active = if active < g.nfats { active } else { 0 };
    let copies = (0..g.nfats).map(|i| g.rsvd + i * g.fatsz).collect();
    let t = Table::new(g.rsvd + active * g.fatsz, copies, g.fatsz);
    let cs = g.spc as usize * ss;
    let mut v = Fat { io, g, t, cs, free: 0, next: 2, dirty: false, ro, label: String::new(), fsinfo_ok: false };
    let clean = v.clean_bit()?;
    // FSInfo (FAT32) solo es una pista: si la última vez no se cerró bien, se recalcula.
    let mut hint = None;
    if g.kind == FsKind::Fat32 && g.fsinfo != 0 && g.fsinfo < g.rsvd {
        let f = v.io.read(g.fsinfo, 1)?;
        if u32le(&f, 0) == 0x4161_5252 && u32le(&f, 484) == 0x6141_7272 && u32le(&f, 508) == 0xAA55_0000 {
            v.fsinfo_ok = true;
            let (fc, nf) = (u32le(&f, 488), u32le(&f, 492));
            if clean && fc <= g.count {
                hint = Some(fc);
            }
            if (2..g.count + 2).contains(&nf) {
                v.next = nf;
            }
        }
    }
    v.free = match hint {
        Some(h) => h,
        None => v.count_free()?,
    };
    let root = v.root();
    let (_, label) = v.scan_dir(root)?;
    v.label = label.unwrap_or_default();
    if !ro {
        v.begin()?;
    }
    Ok(Box::new(v))
}

impl Fat {
    fn root(&self) -> u32 {
        if self.g.kind == FsKind::Fat32 { self.g.root_clus } else { 0 }
    }
    fn eoc(&self) -> u32 {
        match self.g.kind {
            FsKind::Fat12 => 0xFFF,
            FsKind::Fat16 => 0xFFFF,
            _ => 0x0FFF_FFFF,
        }
    }
    fn csec(&self, c: u32) -> u64 {
        self.g.data_start() + (c as u64 - 2) * self.g.spc
    }

    fn get(&mut self, c: u32) -> Result<u32, FsError> {
        let c64 = c as u64;
        Ok(match self.g.kind {
            FsKind::Fat12 => {
                let v = self.t.get(&mut self.io, c64 + c64 / 2, 2)?;
                if c & 1 == 1 { v >> 4 } else { v & 0xFFF }
            }
            FsKind::Fat16 => self.t.get(&mut self.io, c64 * 2, 2)?,
            _ => self.t.get(&mut self.io, c64 * 4, 4)? & 0x0FFF_FFFF,
        })
    }
    fn set(&mut self, c: u32, v: u32) -> Result<(), FsError> {
        let c64 = c as u64;
        match self.g.kind {
            FsKind::Fat12 => {
                let o = c64 + c64 / 2;
                let old = self.t.get(&mut self.io, o, 2)?;
                let new = if c & 1 == 1 { (old & 0xF) | v << 4 } else { (old & 0xF000) | v };
                self.t.set(&mut self.io, o, 2, new)
            }
            FsKind::Fat16 => self.t.set(&mut self.io, c64 * 2, 2, v),
            _ => {
                let old = self.t.get(&mut self.io, c64 * 4, 4)?;
                self.t.set(&mut self.io, c64 * 4, 4, (old & 0xF000_0000) | (v & 0x0FFF_FFFF))
            }
        }
    }

    /// Bit de "cerrado correctamente" de FAT[1] (FAT12 no lo tiene: siempre limpio).
    fn clean_bit(&mut self) -> Result<bool, FsError> {
        Ok(match self.g.kind {
            FsKind::Fat12 => true,
            FsKind::Fat16 => self.t.get(&mut self.io, 2, 2)? & 0x8000 != 0,
            _ => self.t.get(&mut self.io, 4, 4)? & 0x0800_0000 != 0,
        })
    }
    fn set_clean(&mut self, clean: bool) -> Result<(), FsError> {
        let (off, n, bit) = match self.g.kind {
            FsKind::Fat12 => return Ok(()),
            FsKind::Fat16 => (2, 2, 0x8000),
            _ => (4, 4, 0x0800_0000),
        };
        let v = self.t.get(&mut self.io, off, n)?;
        self.t.set(&mut self.io, off, n, if clean { v | bit } else { v & !bit })?;
        self.t.flush(&mut self.io)
    }
    /// Antes de cualquier cambio: solo escritura, y volumen marcado como sucio.
    fn begin(&mut self) -> Result<(), FsError> {
        if self.ro {
            return Err(FsError::Denied);
        }
        if !self.dirty {
            self.set_clean(false)?;
            self.dirty = true;
        }
        Ok(())
    }

    fn count_free(&mut self) -> Result<u32, FsError> {
        let (count, mut free) = (self.g.count as u64, 0u32);
        if self.g.kind == FsKind::Fat12 {
            for c in 2..count as u32 + 2 {
                free += (self.get(c)? == 0) as u32;
            }
            return Ok(free);
        }
        // Lectura directa por bloques (no pasa por la caché: aún no hay nada sucio).
        let w = if self.g.kind == FsKind::Fat16 { 2usize } else { 4 };
        let mut idx = 0u64;
        let mut sec = 0;
        while idx < count + 2 && sec < self.g.fatsz {
            let k = (self.g.fatsz - sec).min(64) as usize;
            let b = self.io.read(self.t.base + sec, k)?;
            for e in b.chunks_exact(w) {
                let v = if w == 2 { u16le(e, 0) as u32 } else { u32le(e, 0) & 0x0FFF_FFFF };
                free += (idx >= 2 && idx < count + 2 && v == 0) as u32;
                idx += 1;
            }
            sec += k as u64;
        }
        Ok(free)
    }

    /// Cadena de clusters desde `start`, con a lo sumo `limit` clusters. `Corrupt` si se sale del
    /// volumen, cae en un cluster libre o malo, o da vueltas.
    fn chain(&mut self, start: u32, limit: usize) -> Result<Vec<u32>, FsError> {
        let (max, eoc_min) = (self.g.count + 1, self.eoc() - 7);
        let mut v = Vec::new();
        let mut c = start;
        while v.len() < limit {
            if c < 2 || c > max || v.len() as u32 >= self.g.count {
                return Err(FsError::Corrupt);
            }
            v.push(c);
            let n = self.get(c)?;
            if n >= eoc_min {
                break;
            }
            if n == 0 || n == eoc_min - 1 {
                return Err(FsError::Corrupt);
            }
            c = n;
        }
        Ok(v)
    }

    /// Reserva `n` clusters libres y los enlaza en una cadena (en la caché de la FAT).
    fn alloc(&mut self, n: usize) -> Result<Vec<u32>, FsError> {
        if n == 0 {
            return Ok(Vec::new());
        }
        if n as u64 > self.free as u64 {
            return Err(FsError::NoSpace);
        }
        let total = self.g.count + 2;
        let mut out = Vec::with_capacity(n);
        let mut c = self.next.clamp(2, total - 1);
        for _ in 0..self.g.count {
            if self.get(c)? == 0 {
                out.push(c);
                if out.len() == n {
                    break;
                }
            }
            c = if c + 1 >= total { 2 } else { c + 1 };
        }
        if out.len() < n {
            return Err(FsError::NoSpace);
        }
        for w in out.windows(2) {
            self.set(w[0], w[1])?;
        }
        self.set(out[n - 1], self.eoc())?;
        self.free -= n as u32;
        self.next = if c + 1 >= total { 2 } else { c + 1 };
        Ok(out)
    }

    fn release(&mut self, list: &[u32]) -> Result<(), FsError> {
        for &c in list {
            // Una cadena cruzada (imagen dañada) puede nombrar dos veces el mismo cluster.
            if self.get(c)? != 0 {
                self.set(c, 0)?;
                self.free += 1;
            }
        }
        Ok(())
    }

    fn read_clusters(&mut self, cl: &[u32]) -> Result<Vec<u8>, FsError> {
        read_cl(&mut self.io, self.g.data_start(), self.g.spc, cl)
    }

    fn write_clusters(&mut self, cl: &[u32], data: &[u8]) -> Result<(), FsError> {
        write_cl(&mut self.io, self.g.data_start(), self.g.spc, cl, data)
    }

    fn load(&mut self, dir: u32) -> Result<DirBuf, FsError> {
        let (data, cl) = if dir == 0 && self.g.kind != FsKind::Fat32 {
            (self.io.read(self.g.root_start(), self.g.root_secs as usize)?, Vec::new())
        } else {
            let cl = self.chain(dir, MAXDIR / self.cs + 1)?;
            if cl.len() * self.cs > MAXDIR {
                return Err(FsError::Corrupt);
            }
            (self.read_clusters(&cl)?, cl)
        };
        Ok(DirBuf { orig: data.clone(), data, cl })
    }

    /// Escribe los sectores cambiados del directorio, tras dejar la FAT en disco.
    fn store(&mut self, d: &mut DirBuf) -> Result<(), FsError> {
        self.t.flush(&mut self.io)?;
        let ss = self.io.ss;
        for s in 0..d.data.len() / ss {
            if d.data[s * ss..(s + 1) * ss] != d.orig[s * ss..(s + 1) * ss] {
                let sec = if d.cl.is_empty() {
                    self.g.root_start() + s as u64
                } else {
                    self.csec(d.cl[s / self.g.spc as usize]) + (s as u64 % self.g.spc)
                };
                self.io.write(sec, &d.data[s * ss..(s + 1) * ss])?;
            }
        }
        d.orig = d.data.clone();
        Ok(())
    }

    /// Entradas de un directorio y su etiqueta de volumen (si la tiene).
    fn scan(&self, data: &[u8]) -> (Vec<Item>, Option<String>) {
        struct Lfn {
            start: usize,
            chk: u8,
            next: u8,
            chunks: Vec<[u16; 13]>,
        }
        let (mut items, mut label, mut lfn) = (Vec::new(), None, None::<Lfn>);
        for i in 0..data.len() / 32 {
            let e = &data[i * 32..i * 32 + 32];
            if e[0] == 0 {
                break;
            }
            if e[0] == 0xE5 {
                lfn = None;
                continue;
            }
            if e[11] & 0x3F == 0x0F {
                let (ord, mut chunk) = (e[0], [0u16; 13]);
                if ord & 0x40 != 0 && (1..=20).contains(&(ord & 0x3F)) {
                    lfn = Some(Lfn { start: i, chk: e[13], next: ord & 0x3F, chunks: Vec::new() });
                }
                for (j, u) in chunk.iter_mut().enumerate() {
                    let o = match j {
                        0..=4 => 1 + 2 * j,
                        5..=10 => 14 + 2 * (j - 5),
                        _ => 28 + 2 * (j - 11),
                    };
                    *u = u16le(e, o);
                }
                lfn = match lfn {
                    Some(mut l) if l.next > 0 && ord & 0x3F == l.next && e[13] == l.chk => {
                        l.chunks.push(chunk);
                        l.next -= 1;
                        Some(l)
                    }
                    _ => None,
                };
                continue;
            }
            if e[11] & 0x08 != 0 {
                if label.is_none() && e[11] & 0x10 == 0 {
                    label = Some(String::from(short_text(e).replace('.', "").trim_end()));
                }
                lfn = None;
                continue;
            }
            if e[0] == b'.' {
                lfn = None;
                continue;
            }
            let (name, at) = match lfn.take() {
                Some(l) if l.next == 0 && l.chk == lfn_sum(e) => {
                    let mut u = Vec::new();
                    'o: for ch in l.chunks.iter().rev() {
                        for &x in ch {
                            if x == 0 {
                                break 'o;
                            }
                            u.push(x);
                        }
                    }
                    let n: String = char::decode_utf16(u).map(|r| r.unwrap_or('\u{FFFD}')).collect();
                    if n.is_empty() { (short_text(e), i) } else { (n, l.start) }
                }
                _ => (short_text(e), i),
            };
            items.push(Item { name, at, n: i - at + 1, raw: e.try_into().unwrap() });
        }
        (items, label)
    }

    fn scan_dir(&mut self, dir: u32) -> Result<(Vec<Item>, Option<String>), FsError> {
        let b = self.load(dir)?;
        Ok(self.scan(&b.data))
    }

    fn first_of(&self, it: &Item) -> u32 {
        let lo = u16le(&it.raw, 26) as u32;
        if self.g.kind == FsKind::Fat32 { (u16le(&it.raw, 20) as u32) << 16 | lo } else { lo }
    }

    /// Clusters de cada directorio del camino (desde la raíz); el último es el directorio pedido.
    fn walk(&mut self, comps: &[&str]) -> Result<Vec<u32>, FsError> {
        let mut path = vec![self.root()];
        for c in comps {
            let (items, _) = self.scan_dir(*path.last().unwrap())?;
            let it = items.iter().find(|i| same_name(&i.name, c)).ok_or(FsError::NotFound)?;
            if !it.dir() {
                return Err(FsError::NotDir);
            }
            let f = self.first_of(it);
            if f < 2 {
                return Err(FsError::Corrupt);
            }
            path.push(f);
        }
        Ok(path)
    }

    /// Añade `name` al directorio cargado (con sus entradas LFN y un nombre corto único).
    fn add_entry(&mut self, buf: &mut DirBuf, name: &str, mut raw: [u8; 32]) -> Result<(), FsError> {
        let taken: Vec<[u8; 11]> = self.scan(&buf.data).0.iter().map(|i| i.raw[..11].try_into().unwrap()).collect();
        let (short, exact) = make_short(name, &taken)?;
        raw[..11].copy_from_slice(&short);
        raw[12] = 0;
        let lfn = if exact { Vec::new() } else { lfn_entries(&name.encode_utf16().collect::<Vec<_>>(), lfn_sum(&short)) };
        let k = lfn.len() + 1;
        let total0 = buf.data.len() / 32;
        let end = (0..total0).find(|&i| buf.data[i * 32] == 0).unwrap_or(total0);
        let at = loop {
            match find_run(&buf.data, k) {
                Some(a) => break a,
                None => self.grow(buf)?,
            }
        };
        for (j, e) in lfn.iter().chain(core::iter::once(&raw)).enumerate() {
            buf.data[(at + j) * 32..(at + j + 1) * 32].copy_from_slice(e);
        }
        let after = at + k;
        if after > end && after < buf.data.len() / 32 {
            buf.data[after * 32] = 0;
        }
        Ok(())
    }

    /// Añade un cluster a un directorio (la raíz fija de FAT12/16 no crece).
    fn grow(&mut self, buf: &mut DirBuf) -> Result<(), FsError> {
        let last = *buf.cl.last().ok_or(FsError::NoSpace)?;
        if (buf.cl.len() + 1) * self.cs > MAXDIR {
            return Err(FsError::NoSpace);
        }
        let c = self.alloc(1)?[0];
        self.set(last, c)?;
        self.io.write(self.csec(c), &vec![0; self.cs])?;
        buf.cl.push(c);
        buf.data.resize(buf.data.len() + self.cs, 0);
        buf.orig.resize(buf.orig.len() + self.cs, 0);
        Ok(())
    }

    fn entry_of(&self, it: &Item) -> Entry {
        let size = if it.dir() { 0 } else { u32le(&it.raw, 28) as u64 };
        Entry { name: it.name.clone(), dir: it.dir(), size, modified: unpack_time((u16le(&it.raw, 24) as u32) << 16 | u16le(&it.raw, 22) as u32), hidden: it.raw[11] & 0x06 != 0 }
    }

    /// (clusters de los directorios del camino, nombre final) de una ruta que no es la raíz.
    fn parent<'a>(&mut self, path: &'a str) -> Result<(Vec<u32>, &'a str), FsError> {
        let mut c = split(path);
        let name = c.pop().ok_or(FsError::Invalid)?;
        Ok((self.walk(&c)?, name))
    }

    fn lookup(&mut self, path: &str) -> Result<(DirBuf, Item), FsError> {
        let (dirs, name) = self.parent(path)?;
        let buf = self.load(*dirs.last().unwrap())?;
        let it = self.scan(&buf.data).0.into_iter().find(|i| same_name(&i.name, name)).ok_or(FsError::NotFound)?;
        Ok((buf, it))
    }
}

impl Volume for Fat {
    fn kind(&self) -> FsKind {
        self.g.kind
    }
    fn label(&self) -> String {
        self.label.clone()
    }
    fn space(&mut self) -> (u64, u64) {
        (self.g.count as u64 * self.cs as u64, self.free as u64 * self.cs as u64)
    }
    fn read_only(&self) -> bool {
        self.ro
    }

    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, FsError> {
        let d = *self.walk(&split(dir))?.last().unwrap();
        Ok(self.scan_dir(d)?.0.iter().map(|i| self.entry_of(i)).collect())
    }

    fn stat(&mut self, path: &str) -> Result<Entry, FsError> {
        if split(path).is_empty() {
            return Ok(Entry { name: String::new(), dir: true, size: 0, modified: DateTime::default(), hidden: false });
        }
        let (_, it) = self.lookup(path)?;
        Ok(self.entry_of(&it))
    }

    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        let (_, it) = self.lookup(path)?;
        if it.dir() {
            return Err(FsError::IsDir);
        }
        let size = u32le(&it.raw, 28) as usize;
        if size == 0 {
            return Ok(Vec::new());
        }
        let need = size.div_ceil(self.cs);
        if need as u64 > self.g.count as u64 {
            return Err(FsError::Corrupt);
        }
        let cl = self.chain(self.first_of(&it), need)?;
        if cl.len() != need {
            return Err(FsError::Corrupt);
        }
        let mut d = self.read_clusters(&cl)?;
        d.truncate(size);
        Ok(d)
    }

    fn write(&mut self, path: &str, data: &[u8], now: DateTime) -> Result<(), FsError> {
        let (dirs, name) = self.parent(path)?;
        if !valid_name(name) {
            return Err(FsError::BadName);
        }
        if data.len() > u32::MAX as usize {
            return Err(FsError::NoSpace);
        }
        self.begin()?;
        let mut buf = self.load(*dirs.last().unwrap())?;
        let existing = self.scan(&buf.data).0.into_iter().find(|i| same_name(&i.name, name));
        if existing.as_ref().is_some_and(|i| i.dir()) {
            return Err(FsError::IsDir);
        }
        let old = match &existing {
            Some(i) if self.first_of(i) >= 2 => self.chain(self.first_of(i), usize::MAX)?,
            _ => Vec::new(),
        };
        // Copia sobre copia: los clusters nuevos se reservan con los viejos aún intactos; solo si
        // no cabe se liberan antes los viejos (entonces un corte de luz puede dejar el archivo mezclado).
        let n = data.len().div_ceil(self.cs);
        let mut old_freed = false;
        if (self.free as usize) < n {
            // Antes de soltar los viejos, el libre exacto (el de FSInfo es una pista y puede mentir):
            // así `alloc` no puede fallar después y dejar el archivo apuntando a clusters libres.
            self.free = self.count_free()?;
            if self.free as usize + old.len() < n {
                return Err(FsError::NoSpace);
            }
            self.release(&old)?;
            old_freed = true;
        }
        let cl = self.alloc(n)?;
        let first = cl.first().copied().unwrap_or(0);
        let done = self.write_clusters(&cl, data).and_then(|_| match &existing {
            Some(it) => {
                let mut raw = it.raw;
                set_first(&mut raw, first);
                raw[28..32].copy_from_slice(&(data.len() as u32).to_le_bytes());
                let t = pack_time(now);
                raw[22..24].copy_from_slice(&(t as u16).to_le_bytes());
                raw[24..26].copy_from_slice(&((t >> 16) as u16).to_le_bytes());
                raw[11] |= 0x20;
                buf.data[(it.at + it.n - 1) * 32..(it.at + it.n) * 32].copy_from_slice(&raw);
                self.store(&mut buf)
            }
            None => {
                let r = self.add_entry(&mut buf, name, new_raw(0x20, first, data.len() as u32, now));
                r.and_then(|_| self.store(&mut buf))
            }
        });
        if let Err(e) = done {
            if !old_freed {
                let _ = self.release(&cl);
            }
            return Err(e);
        }
        if !old_freed {
            self.release(&old)?;
        }
        self.t.flush(&mut self.io)
    }

    fn mkdir(&mut self, path: &str, now: DateTime) -> Result<(), FsError> {
        let (dirs, name) = self.parent(path)?;
        if !valid_name(name) {
            return Err(FsError::BadName);
        }
        self.begin()?;
        let pd = *dirs.last().unwrap();
        let mut buf = self.load(pd)?;
        if self.scan(&buf.data).0.iter().any(|i| same_name(&i.name, name)) {
            return Err(FsError::Exists);
        }
        let c = self.alloc(1)?[0];
        let up = if pd == self.g.root_clus && self.g.kind == FsKind::Fat32 { 0 } else { pd };
        let mut body = vec![0u8; self.cs];
        for (i, (nm, cl)) in [(*b".          ", c), (*b"..         ", up)].iter().enumerate() {
            let mut e = new_raw(0x10, *cl, 0, now);
            e[..11].copy_from_slice(nm);
            body[i * 32..(i + 1) * 32].copy_from_slice(&e);
        }
        let r = self.io.write(self.csec(c), &body).and_then(|_| self.add_entry(&mut buf, name, new_raw(0x10, c, 0, now))).and_then(|_| self.store(&mut buf));
        if r.is_err() {
            let _ = self.release(&[c]);
        }
        r?;
        self.t.flush(&mut self.io)
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        let (fdirs, fname) = self.parent(from)?;
        let (tdirs, tname) = self.parent(to)?;
        if !valid_name(tname) {
            return Err(FsError::BadName);
        }
        self.begin()?;
        let (fd, td) = (*fdirs.last().unwrap(), *tdirs.last().unwrap());
        let mut fb = self.load(fd)?;
        let it = self.scan(&fb.data).0.into_iter().find(|i| same_name(&i.name, fname)).ok_or(FsError::NotFound)?;
        let first = self.first_of(&it);
        // Una carpeta no puede entrar en sí misma ni en sus descendientes.
        if it.dir() && (first < 2 || tdirs.contains(&first)) {
            return Err(if first < 2 { FsError::Corrupt } else { FsError::Invalid });
        }
        let same = fd == td;
        if same && fname == tname {
            return Ok(());
        }
        let mut tb = if same { None } else { Some(self.load(td)?) };
        let tdata = tb.as_ref().map_or(&fb.data, |b| &b.data);
        if self.scan(tdata).0.iter().any(|i| same_name(&i.name, tname) && !(same && i.at == it.at)) {
            return Err(FsError::Exists);
        }
        let kill = |b: &mut DirBuf| (it.at..it.at + it.n).for_each(|i| b.data[i * 32] = 0xE5);
        match &mut tb {
            None => {
                kill(&mut fb);
                self.add_entry(&mut fb, tname, it.raw)?;
                self.store(&mut fb)?;
            }
            Some(tb) => {
                self.add_entry(tb, tname, it.raw)?;
                self.store(tb)?;
                if it.dir() {
                    // ".." del directorio movido apunta al nuevo padre (0 = raíz).
                    let mut db = self.load(first)?;
                    let up = if td == self.g.root_clus && self.g.kind == FsKind::Fat32 { 0 } else { td };
                    // En una imagen dañada la segunda entrada puede ser un archivo: no se le cambia el cluster.
                    if db.data.get(32..44).is_some_and(|e| &e[..11] == b"..         " && e[11] & 0x10 != 0) {
                        set_first(&mut db.data[32..64], up);
                        self.store(&mut db)?;
                    }
                }
                kill(&mut fb);
                self.store(&mut fb)?;
            }
        }
        Ok(())
    }

    fn delete(&mut self, path: &str) -> Result<(), FsError> {
        self.begin()?;
        let (dirs, name) = self.parent(path)?;
        let mut buf = self.load(*dirs.last().unwrap())?;
        let it = self.scan(&buf.data).0.into_iter().find(|i| same_name(&i.name, name)).ok_or(FsError::NotFound)?;
        let first = self.first_of(&it);
        if it.dir() && first < 2 {
            return Err(FsError::Corrupt);
        }
        if it.dir() && !self.scan_dir(first)?.0.is_empty() {
            return Err(FsError::NotEmpty);
        }
        (it.at..it.at + it.n).for_each(|i| buf.data[i * 32] = 0xE5);
        self.store(&mut buf)?;
        // Si la cadena estaba rota se pierde algún cluster (queda para chkdsk), pero la entrada ya no está.
        if first >= 2 {
            if let Ok(ch) = self.chain(first, usize::MAX) {
                self.release(&ch)?;
            }
        }
        self.t.flush(&mut self.io)
    }

    fn flush(&mut self) -> Result<(), FsError> {
        if self.ro {
            return Ok(());
        }
        self.t.flush(&mut self.io)?;
        if self.fsinfo_ok {
            let mut f = self.io.read(self.g.fsinfo, 1)?;
            f[488..492].copy_from_slice(&self.free.to_le_bytes());
            f[492..496].copy_from_slice(&self.next.to_le_bytes());
            self.io.write(self.g.fsinfo, &f)?;
        }
        self.set_clean(true)?;
        self.dirty = false;
        self.io.dev.flush()
    }
}

// --- formatear ---

/// Etiqueta de volumen (hasta 11 caracteres ASCII, en mayúsculas); `None` si está vacía.
/// shortcut: solo ASCII (en disco es de 8 bits OEM); ampliar con una tabla de página de códigos.
pub(super) fn label_bytes(label: &str) -> Result<Option<[u8; 11]>, FsError> {
    if label.is_empty() {
        return Ok(None);
    }
    if label.len() > 11 || !label.chars().all(|c| c.is_ascii() && c >= ' ' && !"\"*+,./:;<=>?[\\]|".contains(c)) {
        return Err(FsError::BadName);
    }
    let mut r = [b' '; 11];
    r[..label.len()].copy_from_slice(label.to_ascii_uppercase().as_bytes());
    Ok(Some(r))
}

/// (sectores por FAT, clusters) si `kind` cabe en `total` sectores con `spc` sectores por cluster.
fn layout(kind: FsKind, total: u64, ss: u64, spc: u64) -> Option<(u64, u32)> {
    let (rsvd, root, bits) = match kind {
        FsKind::Fat32 => (32, 0, 32),
        FsKind::Fat16 => (1, 512 * 32 / ss, 16),
        _ => (1, 512 * 32 / ss, 12),
    };
    let clusters = |f: u64| total.saturating_sub(rsvd + root + 2 * f) / spc;
    let need = |c: u64| ((c + 2) * bits).div_ceil(8).div_ceil(ss);
    let mut f = need(clusters(0));
    for _ in 0..8 {
        let n = need(clusters(f));
        if n == f {
            break;
        }
        f = n;
    }
    while need(clusters(f)) > f {
        f += 1;
    }
    if rsvd + root + 2 * f >= total {
        return None;
    }
    let c = clusters(f);
    let ok = match kind {
        FsKind::Fat12 => (8..4085).contains(&c),
        FsKind::Fat16 => (4085..65525).contains(&c),
        _ => (65525..=0x0FFF_FFF5).contains(&c),
    };
    ok.then_some((f, c as u32))
}

/// Tamaño de cluster preferido (tabla de la especificación) en sectores.
fn pref_spc(kind: FsKind, bytes: u64, ss: u64) -> u64 {
    let mb = bytes >> 20;
    let cb = match kind {
        FsKind::Fat32 => match mb {
            ..=260 => 512,
            ..=8192 => 4096,
            ..=16384 => 8192,
            ..=32768 => 16384,
            _ => 32768,
        },
        FsKind::Fat16 => match mb {
            ..=4 => 512,
            ..=16 => 1024,
            ..=128 => 2048,
            ..=256 => 4096,
            ..=512 => 8192,
            ..=1024 => 16384,
            ..=2048 => 32768,
            _ => 65536,
        },
        _ => 512,
    };
    (cb / ss).max(1)
}

/// Formatea el disco entero: una partición MBR alineada a 1 MiB con FAT32 (si caben ≥ 64 MiB), si no
/// FAT16, si no FAT12. Solo se escriben las estructuras; el área de datos no se borra.
pub fn format(dev: &mut dyn SectorDev, label: &str, now: DateTime) -> Result<(), FsError> {
    let lab = label_bytes(label)?;
    let (start, total) = part::prepare(dev)?;
    let ss = dev.sector_size() as u64;
    let kinds: &[FsKind] = if total * ss >= 64 << 20 { &[FsKind::Fat32, FsKind::Fat16, FsKind::Fat12] } else { &[FsKind::Fat16, FsKind::Fat12] };
    let pick = kinds.iter().find_map(|&k| {
        let p = pref_spc(k, total * ss, ss);
        let mut cand = vec![p];
        let mut s = p;
        while s > 1 {
            s /= 2;
            cand.push(s);
        }
        s = p;
        while s < 128 {
            s *= 2;
            cand.push(s);
        }
        cand.into_iter().find_map(|spc| layout(k, total, ss, spc).map(|(f, c)| (k, spc, f, c)))
    });
    let (kind, spc, fatsz, count) = pick.ok_or(FsError::NoSpace)?;
    let f32 = kind == FsKind::Fat32;
    let (rsvd, root_ent) = if f32 { (32u64, 0u64) } else { (1, 512) };
    let root_secs = root_ent * 32 / ss;
    let data = rsvd + 2 * fatsz + root_secs;
    zero_sectors(dev, start, data + if f32 { spc } else { 0 })?;

    let vid = ((now.year as u32) << 16) ^ ((now.month as u32) << 12 | (now.day as u32) << 7 | (now.hour as u32) << 2) ^ (now.minute as u32 * 60 + now.second as u32);
    let mut b = vec![0u8; ss as usize];
    b[0..3].copy_from_slice(&[0xEB, if f32 { 0x58 } else { 0x3C }, 0x90]);
    b[3..11].copy_from_slice(b"NIGHTFRY");
    b[11..13].copy_from_slice(&(ss as u16).to_le_bytes());
    b[13] = spc as u8;
    b[14..16].copy_from_slice(&(rsvd as u16).to_le_bytes());
    b[16] = 2;
    b[17..19].copy_from_slice(&(root_ent as u16).to_le_bytes());
    if !f32 && total < 65536 {
        b[19..21].copy_from_slice(&(total as u16).to_le_bytes());
    } else {
        b[32..36].copy_from_slice(&(total as u32).to_le_bytes());
    }
    b[21] = 0xF8;
    if !f32 {
        b[22..24].copy_from_slice(&(fatsz as u16).to_le_bytes());
    } else {
        b[36..40].copy_from_slice(&(fatsz as u32).to_le_bytes());
        b[44..48].copy_from_slice(&2u32.to_le_bytes());
        b[48..50].copy_from_slice(&1u16.to_le_bytes());
        b[50..52].copy_from_slice(&6u16.to_le_bytes());
    }
    b[24..26].copy_from_slice(&63u16.to_le_bytes());
    b[26..28].copy_from_slice(&255u16.to_le_bytes());
    b[28..32].copy_from_slice(&(start as u32).to_le_bytes());
    let o = if f32 { 64 } else { 36 };
    b[o] = 0x80;
    b[o + 2] = 0x29;
    b[o + 3..o + 7].copy_from_slice(&vid.to_le_bytes());
    b[o + 7..o + 18].copy_from_slice(&lab.unwrap_or(*b"NO NAME    "));
    b[o + 18..o + 26].copy_from_slice(match kind {
        FsKind::Fat32 => b"FAT32   ",
        FsKind::Fat16 => b"FAT16   ",
        _ => b"FAT12   ",
    });
    b[510] = 0x55;
    b[511] = 0xAA;
    dev.write(start, &b)?;

    // Primeros sectores de las dos FAT: medio, fin de cadena (y la raíz en FAT32).
    let mut f = vec![0u8; ss as usize];
    let head: &[u8] = match kind {
        FsKind::Fat12 => &[0xF8, 0xFF, 0xFF],
        FsKind::Fat16 => &[0xF8, 0xFF, 0xFF, 0xFF],
        _ => &[0xF8, 0xFF, 0xFF, 0x0F, 0xFF, 0xFF, 0xFF, 0x0F, 0xFF, 0xFF, 0xFF, 0x0F],
    };
    f[..head.len()].copy_from_slice(head);
    dev.write(start + rsvd, &f)?;
    dev.write(start + rsvd + fatsz, &f)?;
    if let Some(l) = lab {
        let mut e = vec![0u8; ss as usize];
        let mut r = new_raw(0x08, 0, 0, now);
        r[..11].copy_from_slice(&l);
        e[..32].copy_from_slice(&r);
        dev.write(start + if f32 { data } else { rsvd + 2 * fatsz }, &e)?;
    }
    if f32 {
        let mut i = vec![0u8; ss as usize];
        i[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
        i[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
        i[488..492].copy_from_slice(&(count - 1).to_le_bytes());
        i[492..496].copy_from_slice(&3u32.to_le_bytes());
        i[508..512].copy_from_slice(&0xAA55_0000u32.to_le_bytes());
        dev.write(start + 1, &i)?;
        dev.write(start + 6, &b)?;
        dev.write(start + 7, &i)?;
    }
    part::write_mbr(dev, start, total, match kind {
        FsKind::Fat32 => 0x0C,
        FsKind::Fat16 => 0x0E,
        _ => 0x01,
    })?;
    dev.flush()
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::vol::{format as vformat, mount as vmount, partitions, MemSectors};
    use alloc::rc::Rc;
    use core::cell::RefCell;

    /// Disco compartido para poder desmontar y volver a montar sobre los mismos bytes.
    #[derive(Clone)]
    pub struct Shared(pub Rc<RefCell<MemSectors>>);
    impl Shared {
        pub fn new(sectors: u64, ss: usize) -> Shared {
            Shared(Rc::new(RefCell::new(MemSectors::new(sectors, ss))))
        }
    }
    impl SectorDev for Shared {
        fn sector_size(&self) -> usize {
            self.0.borrow().sector
        }
        fn sectors(&self) -> u64 {
            self.0.borrow().sectors()
        }
        fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
            self.0.borrow_mut().read(lba, buf)
        }
        fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError> {
            self.0.borrow_mut().write(lba, buf)
        }
        fn read_only(&self) -> bool {
            self.0.borrow().ro
        }
    }

    pub const NOW: DateTime = DateTime { year: 2026, month: 11, day: 18, hour: 13, minute: 45, second: 30 };

    pub fn open(d: &Shared) -> Box<dyn Volume> {
        let p = partitions(&mut d.clone()).unwrap();
        assert_eq!(p.len(), 1);
        vmount(Box::new(d.clone()), &p[0]).unwrap()
    }

    fn pattern(n: usize, seed: u32) -> Vec<u8> {
        let mut x = seed.wrapping_mul(2654435761).wrapping_add(1);
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                (x >> 24) as u8
            })
            .collect()
    }

    /// Batería común a FAT y exFAT. `dirty(disco)` dice si el bit de volumen sucio está puesto.
    pub fn suite(d: &Shared, kind: FsKind, chunk: usize, dirty: &dyn Fn(&Shared) -> Option<bool>) {
        vformat(&mut d.clone(), kind, "Mi Disco", NOW).unwrap();
        let mut v = open(d);
        assert_eq!(v.label().to_uppercase(), "MI DISCO");
        assert!(v.list("").unwrap().is_empty());
        let (total, free0) = v.space();
        assert!(free0 > 0 && free0 <= total);
        assert_ne!(dirty(d), Some(false), "tras montar en escritura debe estar sucio");

        // carpetas anidadas
        v.mkdir("a", NOW).unwrap();
        v.mkdir("a\\b", NOW).unwrap();
        v.mkdir("\\a\\b\\c", NOW).unwrap();
        assert_eq!(v.mkdir("A", NOW), Err(FsError::Exists));
        assert_eq!(v.mkdir("x\\y", NOW), Err(FsError::NotFound));
        assert_eq!(v.mkdir("a\\q*", NOW), Err(FsError::BadName));
        assert!(v.stat("a\\b").unwrap().dir);

        // pequeño y de varios clusters
        v.write("a\\hola.txt", b"hola mundo", NOW).unwrap();
        assert_eq!(v.read("a\\hola.txt").unwrap(), b"hola mundo");
        let e = v.stat("a\\hola.txt").unwrap();
        assert_eq!((e.size, e.dir, e.modified.year, e.modified.month, e.modified.day), (10, false, 2026, 11, 18));
        let big = pattern(100_000, 1);
        v.write("a\\b\\big.bin", &big, NOW).unwrap();
        assert_eq!(v.read("a\\b\\big.bin").unwrap(), big);
        v.write("vacio", b"", NOW).unwrap();
        assert_eq!(v.read("vacio").unwrap(), b"");
        assert_eq!(v.read("a"), Err(FsError::IsDir));
        assert_eq!(v.read("a\\hola.txt\\x"), Err(FsError::NotDir));
        assert_eq!(v.read("nada"), Err(FsError::NotFound));

        // sobrescribir más grande y más pequeño
        let (_, f_before) = v.space();
        let bigger = pattern(300_000, 2);
        v.write("a\\b\\big.bin", &bigger, NOW).unwrap();
        assert_eq!(v.read("a\\b\\big.bin").unwrap(), bigger);
        let (_, f_bigger) = v.space();
        assert!(f_bigger < f_before);
        v.write("a\\b\\big.bin", b"corto", NOW).unwrap();
        assert_eq!(v.read("a\\b\\big.bin").unwrap(), b"corto");
        assert!(v.space().1 > f_bigger);
        v.write("a\\b\\big.bin", b"", NOW).unwrap();
        assert_eq!(v.read("a\\b\\big.bin").unwrap(), b"");

        // nombres largos con acentos y eñes, y mayúsculas/minúsculas
        let largo = "Canción de Ñandú con un nombre bastante largo para varias entradas.txt";
        v.write(&alloc::format!("a\\{largo}"), "ñandú".as_bytes(), NOW).unwrap();
        assert_eq!(v.read("A\\CANCIÓN DE ÑANDÚ CON UN NOMBRE BASTANTE LARGO PARA VARIAS ENTRADAS.TXT").unwrap(), "ñandú".as_bytes());
        assert!(v.list("a").unwrap().iter().any(|e| e.name == largo));
        v.write("a\\Mayus.TXT", b"1", NOW).unwrap();
        v.write("a\\mayus.txt", b"22", NOW).unwrap();
        assert_eq!(v.list("a").unwrap().iter().filter(|e| e.name.eq_ignore_ascii_case("mayus.txt")).count(), 1);
        assert_eq!(v.read("A\\MAYUS.txt").unwrap(), b"22");
        v.write("a\\Archivo Uno.txt", b"u", NOW).unwrap();
        v.write("a\\Archivo Uno.txt.bak", b"u2", NOW).unwrap();
        v.write("a\\Archivo Dos.txt", b"d", NOW).unwrap();
        assert_eq!(v.read("a\\archivo dos.txt").unwrap(), b"d");
        assert_eq!(v.read("a\\archivo uno.txt").unwrap(), b"u");
        assert_eq!(v.write("a\\x:y", b"", NOW), Err(FsError::BadName));
        assert_eq!(v.write(&alloc::format!("a\\{}", "n".repeat(256)), b"", NOW), Err(FsError::BadName));
        v.write(&alloc::format!("a\\{}", "n".repeat(255)), b"", NOW).unwrap();

        // renombrar y mover
        v.rename("a\\hola.txt", "a\\adios.txt").unwrap();
        assert_eq!(v.read("a\\hola.txt"), Err(FsError::NotFound));
        assert_eq!(v.read("a\\adios.txt").unwrap(), b"hola mundo");
        v.rename("a\\adios.txt", "a\\ADIOS.txt").unwrap();
        assert!(v.list("a").unwrap().iter().any(|e| e.name == "ADIOS.txt"));
        assert_eq!(v.rename("a\\ADIOS.txt", "a\\Mayus.txt"), Err(FsError::Exists));
        v.rename("a\\ADIOS.txt", "Movido con nombre largo.txt").unwrap();
        assert_eq!(v.read("Movido con nombre largo.txt").unwrap(), b"hola mundo");
        v.write("a\\b\\c\\hijo.txt", b"hijo", NOW).unwrap();
        assert_eq!(v.rename("a\\b", "a\\b\\c\\b2"), Err(FsError::Invalid));
        v.rename("a\\b", "b2").unwrap();
        assert_eq!(v.read("b2\\c\\hijo.txt").unwrap(), b"hijo");
        assert_eq!(v.stat("a\\b"), Err(FsError::NotFound));

        // borrar
        assert_eq!(v.delete("b2"), Err(FsError::NotEmpty));
        v.delete("b2\\c\\hijo.txt").unwrap();
        v.delete("b2\\c").unwrap();
        v.delete("b2\\big.bin").unwrap();
        v.delete("b2").unwrap();
        assert_eq!(v.delete("b2"), Err(FsError::NotFound));
        v.delete("vacio").unwrap();

        // llenar hasta NoSpace y recuperar el espacio
        let (_, f_start) = v.space();
        let blob = pattern(chunk, 3);
        let mut n = 0;
        loop {
            match v.write(&alloc::format!("FILL{n}"), &blob, NOW) {
                Ok(()) => n += 1,
                Err(e) => {
                    assert_eq!(e, FsError::NoSpace);
                    break;
                }
            }
            assert!(n < 100_000);
        }
        assert!(n > 0);
        assert!(v.space().1 < chunk as u64 * 2);
        let f_full = v.space().1;
        assert_eq!(v.write("a\\no_cabe", &blob, NOW), Err(FsError::NoSpace));
        assert_eq!(v.space().1, f_full);
        assert_eq!(v.read(&alloc::format!("FILL{}", n - 1)).unwrap(), blob);
        for i in 0..n {
            v.delete(&alloc::format!("FILL{i}")).unwrap();
        }
        assert_eq!(v.space().1, f_start);

        // desmontar y montar: datos intactos y bit sucio quitado
        let (expect_big, expect_largo) = (pattern(5000, 9), "ñandú".as_bytes());
        v.write("a\\persistente.bin", &expect_big, NOW).unwrap();
        let space = v.space();
        v.flush().unwrap();
        assert_ne!(dirty(d), Some(true), "tras flush no debe estar sucio");
        drop(v);
        let mut v = open(d);
        assert_eq!(v.space(), space);
        assert_eq!(v.read("a\\persistente.bin").unwrap(), expect_big);
        assert_eq!(v.read(&alloc::format!("a\\{largo}")).unwrap(), expect_largo);
        assert_eq!(v.read("Movido con nombre largo.txt").unwrap(), b"hola mundo");
        assert!(v.stat("a\\b").is_err());
        assert!(v.stat("a").unwrap().dir);
        let names: Vec<String> = v.list("a").unwrap().into_iter().map(|e| e.name).collect();
        assert!(!names.iter().any(|n| n == "." || n == ".."));
        drop(v);

        // solo lectura: se lee, no se escribe, y no se ensucia
        d.0.borrow_mut().ro = true;
        let mut v = open(d);
        assert!(v.read_only());
        assert_eq!(v.read("a\\persistente.bin").unwrap(), expect_big);
        assert_eq!(v.write("zz", b"1", NOW), Err(FsError::Denied));
        assert_eq!(v.mkdir("zz", NOW), Err(FsError::Denied));
        assert_eq!(v.delete("a\\persistente.bin"), Err(FsError::Denied));
        assert_eq!(v.rename("a", "zz"), Err(FsError::Denied));
        v.flush().unwrap();
        d.0.borrow_mut().ro = false;
    }

    fn fat_dirty(d: &Shared) -> Option<bool> {
        let p = partitions(&mut d.clone()).unwrap()[0].clone();
        let m = d.0.borrow();
        let ss = m.sector;
        let g = Geo::parse(&m.data[p.start as usize * ss..][..512], ss).unwrap();
        let o = (p.start + g.rsvd) as usize * ss;
        match g.kind {
            FsKind::Fat12 => None,
            FsKind::Fat16 => Some(u16le(&m.data, o + 2) & 0x8000 == 0),
            _ => Some(u32le(&m.data, o + 4) & 0x0800_0000 == 0),
        }
    }

    #[test]
    fn fat12_suite() {
        let d = Shared::new(4096, 512); // 2 MiB
        suite(&d, FsKind::Fat12, 40_000, &fat_dirty);
        assert_eq!(open(&d).kind(), FsKind::Fat12);
    }

    #[test]
    fn fat16_suite() {
        let d = Shared::new(40 * 2048, 512); // 40 MiB
        suite(&d, FsKind::Fat32, 3_000_000, &fat_dirty);
        assert_eq!(open(&d).kind(), FsKind::Fat16);
    }

    #[test]
    fn fat32_suite() {
        let d = Shared::new(80 * 2048, 512); // 80 MiB
        suite(&d, FsKind::Fat32, 16_000_000, &fat_dirty);
        assert_eq!(open(&d).kind(), FsKind::Fat32);
    }

    #[test]
    fn fat16_sector_4096() {
        let d = Shared::new(70 * 256, 4096); // 70 MiB con sectores de 4096: solo caben ~17000 clusters
        suite(&d, FsKind::Fat32, 16_000_000, &fat_dirty);
        assert_eq!(open(&d).kind(), FsKind::Fat16);
    }

    #[test]
    fn short_names() {
        let none: [[u8; 11]; 0] = [];
        let (s, exact) = make_short("Hello World.text", &none).unwrap();
        assert_eq!((&s, exact), (b"HELLOW~1TEX", false));
        let (s2, _) = make_short("Hello Wonder.text", &[s]).unwrap();
        assert_eq!(&s2, b"HELLOW~2TEX");
        assert_eq!(make_short("README.TXT", &none).unwrap(), (*b"README  TXT", true));
        assert_eq!(make_short("Readme.txt", &none).unwrap(), (*b"README  TXT", false));
        assert_eq!(&make_short("ñandú.x", &none).unwrap().0, b"_AND_~1 X  ");
        assert_eq!(&make_short(".gitignore", &none).unwrap().0, b"GITIGN~1   ");
        assert_eq!(make_short("Readme.txt", &[*b"README  TXT"]).unwrap().0, *b"README~1TXT");
    }

    #[test]
    fn dates_roundtrip() {
        let d = unpack_time(pack_time(NOW));
        assert_eq!(d, NOW);
        // fecha 0 (no válida) no provoca pánico
        let z = unpack_time(0);
        assert_eq!((z.year, z.month, z.day), (1980, 1, 1));
    }

    /// Imagen FAT32 escrita byte a byte según fatgen103 (valores fijos, sin usar `format`).
    #[test]
    fn fat32_boot_sector_by_spec() {
        let mut img = vec![0u8; 70000 * 512];
        let b = &mut img[..512];
        b[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
        b[3..11].copy_from_slice(b"MSWIN4.1");
        b[11] = 0x00; // BytsPerSec = 0x0200
        b[12] = 0x02;
        b[13] = 1; // SecPerClus
        b[14] = 32; // RsvdSecCnt
        b[16] = 2; // NumFATs
        b[21] = 0xF8; // Media
        // TotSec32 = 70000 = 0x00011170
        b[32..36].copy_from_slice(&[0x70, 0x11, 0x01, 0x00]);
        // FATSz32 = 548 = 0x0224
        b[36..40].copy_from_slice(&[0x24, 0x02, 0x00, 0x00]);
        b[44] = 2; // RootClus
        b[48] = 1; // FSInfo
        b[50] = 6; // BkBootSec
        b[66] = 0x29;
        b[71..82].copy_from_slice(b"SPECDISK   ");
        b[82..90].copy_from_slice(b"FAT32   ");
        b[510] = 0x55;
        b[511] = 0xAA;
        let fat0 = 32 * 512;
        img[fat0..fat0 + 16].copy_from_slice(&[0xF8, 0xFF, 0xFF, 0x0F, 0xFF, 0xFF, 0xFF, 0x0F, 0xFF, 0xFF, 0xFF, 0x0F, 0xFF, 0xFF, 0xFF, 0x0F]);
        // cluster 2 = (32 + 2*548) = sector 1128: raíz con la etiqueta, un LFN y un archivo corto
        let root = 1128 * 512;
        img[root..root + 11].copy_from_slice(b"SPECDISK   ");
        img[root + 11] = 0x08;
        let e = root + 32;
        img[e..e + 11].copy_from_slice(b"HELLO   TXT");
        img[e + 11] = 0x20;
        img[e + 26] = 3; // cluster 3
        img[e + 28] = 5; // tamaño 5
        img[1129 * 512..1129 * 512 + 5].copy_from_slice(b"hello");
        let mut d = MemSectors::new(70000, 512);
        d.data = img;
        let mut dev = d;
        let p = partitions(&mut dev).unwrap();
        assert_eq!(p, vec![Partition { start: 0, sectors: 70000, kind: FsKind::Fat32 }]);
        let mut v = vmount(Box::new(dev), &p[0]).unwrap();
        assert_eq!(v.label(), "SPECDISK");
        assert_eq!(v.list("").unwrap().len(), 1);
        assert_eq!(v.read("hello.txt").unwrap(), b"hello");
        let (total, free) = v.space();
        assert_eq!((total, free), (68872 * 512, (68872 - 2) * 512));
    }

    #[test]
    fn hostile_chains_do_not_hang() {
        let d = Shared::new(15 * 2048, 512); // FAT16 con clusters de 1 KiB
        vformat(&mut d.clone(), FsKind::Fat16, "", NOW).unwrap();
        let mut v = open(&d);
        let data = pattern(3000, 4); // 3 clusters: 2, 3, 4 (la raíz de FAT16 es fija)
        v.write("f.bin", &data, NOW).unwrap();
        v.flush().unwrap();
        drop(v);
        let p = partitions(&mut d.clone()).unwrap()[0].clone();
        let g = Geo::parse(&d.0.borrow().data[p.start as usize * 512..][..512], 512).unwrap();
        let fat = (p.start + g.rsvd) as usize * 512;
        let patch = |c: usize, v: u16| d.0.borrow_mut().data[fat + 2 * c..fat + 2 * c + 2].copy_from_slice(&v.to_le_bytes());
        patch(4, 0xFFF7); // cluster malo dentro de la cadena
        assert_eq!(open(&d).read("f.bin"), Err(FsError::Corrupt));
        patch(4, 0xFFFF);
        patch(3, 0x7FFF); // enlace fuera del volumen
        assert_eq!(open(&d).read("f.bin"), Err(FsError::Corrupt));
        patch(3, 0); // cadena que cae en un cluster libre
        assert_eq!(open(&d).read("f.bin"), Err(FsError::Corrupt));
        patch(3, 4);
        patch(4, 2); // ciclo 2 -> 3 -> 4 -> 2
        let mut v = open(&d);
        assert_eq!(v.read("f.bin").unwrap(), data);
        v.delete("f.bin").unwrap(); // la cadena circular no cuelga el borrado
        assert!(v.list("").unwrap().is_empty());
    }

    #[test]
    fn garbage_boot_sectors_are_rejected() {
        for seed in 0..200u32 {
            let mut d = MemSectors::new(64, 512);
            d.data[..512].copy_from_slice(&pattern(512, seed));
            d.data[510] = 0x55;
            d.data[511] = 0xAA;
            let p = Partition { start: 0, sectors: 64, kind: FsKind::Fat32 };
            let _ = mount(Box::new(d), &p);
        }
    }
}
