// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! ext2, ext3 y ext4 (Linux) en solo lectura.
//!
//! Sala limpia: se escribe solo a partir de `F:\IdeaProjects\especificaciones\ext4\ESPECIFICACION.md`.
//!
//! CONTRATO (v0.11.1): `mount` devuelve un `Volume` cuyo `read_only()` es `true` y cuyas escrituras
//! dan `FsError::Denied`. Rutas como en `vol` (relativas a la raíz, con `\`, sin distinguir
//! mayúsculas). `Entry::hidden` marca lo oculto o de sistema.
//!
//! La imagen es entrada hostil: la geometría se valida al montar, todo bloque se comprueba contra el
//! volumen, los árboles de extents se recorren con profundidad exacta y rangos anidados, y los bucles
//! (carpetas, enlaces, rutas) llevan tope. Los metadatos se leen por demanda con una caché pequeña.
//!
//! shortcut: no se verifican las sumas (crc32c/crc16) de ninguna estructura, ni se aplica el diario
//! (solo se avisa); sin comparación casefold ni descifrado. Con bigalloc se lee por bloques tal cual.
//! Un archivo mayor de `MAXFILE`, o una carpeta de más de `MAXDIR`/`MAXENT`, da error en vez de leerse
//! por trozos: súbelo o lee por trozos si hiciera falta.
use super::fat::{same_name, split, Io};
use super::iso::read_at;
use super::part::{u16le, u32le};
use super::{FsKind, Partition, SectorDev, Volume};
use crate::fs::{Entry, FsError};
use crate::platform::DateTime;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

/// Tope de bytes de una carpeta que se recorre, de entradas que se devuelven y de un archivo que se lee.
const MAXDIR: u64 = 1 << 25;
const MAXENT: usize = 1 << 18;
const MAXFILE: u64 = 1 << 30;
/// Enlaces simbólicos seguidos en una misma resolución, y componentes pendientes de una ruta.
const MAXLINKS: u32 = 8;
const MAXCOMP: usize = 1024;
/// Bloques de metadatos en caché.
const CACHE: usize = 64;
/// Banderas incompatibles que se entienden (el resto, incluida compression, journal_dev y dirdata, se rechaza).
const INCOMPAT_OK: u32 = 0x2 | 0x4 | 0x10 | 0x40 | 0x80 | 0x100 | 0x200 | 0x400 | 0x2000 | 0x4000 | 0x8000 | 0x10000 | 0x20000;
const IN_FILETYPE: u32 = 0x2;
const IN_RECOVER: u32 = 0x4;
const IN_META_BG: u32 = 0x10;
const IN_64BIT: u32 = 0x80;
const IN_LARGEDIR: u32 = 0x4000;
const IN_CASEFOLD: u32 = 0x20000;
/// Banderas de inodo.
const F_ENCRYPT: u32 = 0x800;
const F_INDEX: u32 = 0x1000;
const F_EXTENTS: u32 = 0x80000;
const F_EA_INODE: u32 = 0x20_0000;
const F_INLINE: u32 = 0x1000_0000;
const F_CASEFOLD: u32 = 0x4000_0000;

struct Sb {
    bs: u64,
    total: u64,
    free: u64,
    first_data: u64,
    bpg: u64,
    ipg: u64,
    inodes: u64,
    isz: u64,
    dsz: u64,
    first_ino: u64,
    first_meta_bg: u64,
    backup: [u64; 2],
    compat: u32,
    incompat: u32,
    ro: u32,
    state: u16,
    label: [u8; 16],
}

/// Lee y valida el superbloque (1024 bytes). Todo lo que no cuadra es `Corrupt`.
fn parse_sb(s: &[u8]) -> Result<Sb, FsError> {
    let bad = || FsError::Corrupt;
    if s.len() < 1024 || u16le(s, 0x38) != 0xEF53 {
        return Err(bad());
    }
    let rev = u32le(s, 0x4C);
    let lb = u32le(s, 0x18);
    let (compat, incompat, ro) = (u32le(s, 0x5C), u32le(s, 0x60), u32le(s, 0x64));
    if rev > 1 || lb > 6 || incompat & !INCOMPAT_OK != 0 {
        return Err(bad());
    }
    let bs = 1024u64 << lb;
    let big = ro & 0x200 != 0;
    let bpg = u32le(s, 0x20) as u64;
    if big {
        let lc = u32le(s, 0x1C);
        let cpg = u32le(s, 0x24) as u64;
        if lc < lb || lc > 19 || bpg != cpg << (lc - lb) || cpg > 8 * bs || cpg % 8 != 0 {
            return Err(bad());
        }
    } else if bpg > 8 * bs {
        return Err(bad());
    }
    if bpg < 8 || bpg % 8 != 0 {
        return Err(bad());
    }
    let isz = if rev == 0 { 128 } else { u16le(s, 0x58) as u64 };
    let first_ino = if rev == 0 { 11 } else { u32le(s, 0x54) as u64 };
    if !isz.is_power_of_two() || isz < 128 || isz > bs || first_ino < 11 {
        return Err(bad());
    }
    let (ipb, ipg) = (bs / isz, u32le(s, 0x28) as u64);
    if ipg == 0 || ipg % 8 != 0 || ipg > 8 * bs || ipg < ipb || ipg % ipb != 0 {
        return Err(bad());
    }
    let is64 = incompat & IN_64BIT != 0;
    let dsz = if is64 { u16le(s, 0xFE) as u64 } else { 32 };
    if is64 && (!dsz.is_power_of_two() || !(64..=1024).contains(&dsz)) {
        return Err(bad());
    }
    let hi = |o: usize| if is64 { (u32le(s, o) as u64) << 32 } else { 0 };
    let total = u32le(s, 4) as u64 | hi(0x150);
    let first_data = u32le(s, 0x14) as u64;
    // 1 con bloque de 1 KiB sin bigalloc, 0 en los demás casos.
    if first_data != (bs == 1024 && !big) as u64 || total <= first_data {
        return Err(bad());
    }
    let groups = (total - first_data).div_ceil(bpg);
    let dpb = bs / dsz;
    if groups > u32::MAX as u64 - dpb || groups * ipg != u32le(s, 0) as u64 || u16le(s, 0xCE) as u64 > bs / 4 {
        return Err(bad());
    }
    if incompat & IN_META_BG != 0 && u32le(s, 0x104) as u64 > groups.div_ceil(dpb) {
        return Err(bad());
    }
    let mut label = [0u8; 16];
    label.copy_from_slice(&s[0x78..0x88]);
    Ok(Sb {
        bs,
        total,
        free: u32le(s, 0xC) as u64 | hi(0x158),
        first_data,
        bpg,
        ipg,
        inodes: groups * ipg,
        isz,
        dsz,
        first_ino,
        first_meta_bg: u32le(s, 0x104) as u64,
        backup: [u32le(s, 0x24C) as u64, u32le(s, 0x250) as u64],
        compat,
        incompat,
        ro,
        state: u16le(s, 0x3A),
        label,
    })
}

impl Sb {
    /// ¿Lleva el grupo `g` una copia del superbloque (y de los descriptores)?
    fn has_sb(&self, g: u64) -> bool {
        if g == 0 {
            return true;
        }
        if self.compat & 0x200 != 0 {
            return g == self.backup[0] || g == self.backup[1];
        }
        if self.ro & 1 == 0 {
            return true;
        }
        g % 2 == 1
            && [3u64, 5, 7].iter().any(|&b| {
                let mut n = g;
                while n % b == 0 {
                    n /= b;
                }
                n == 1
            })
    }

    /// Bloque donde está el bloque `i` de descriptores de grupo.
    fn gdt_block(&self, i: u64) -> Result<u64, FsError> {
        let sb_blk = (self.bs == 1024) as u64;
        if self.incompat & IN_META_BG == 0 || i < self.first_meta_bg {
            return Ok(sb_blk + 1 + i);
        }
        let g0 = i.checked_mul(self.bs / self.dsz).ok_or(FsError::Corrupt)?;
        let first = g0.checked_mul(self.bpg).and_then(|x| x.checked_add(self.first_data)).ok_or(FsError::Corrupt)?;
        // El grupo 0 de un bigalloc de 1 KiB empieza en el bloque 0 pero su superbloque está en el 1.
        Ok(first + self.has_sb(g0) as u64 + (g0 == 0 && self.bs == 1024 && self.first_data == 0) as u64)
    }
}

/// Segundos Unix (con signo) a fecha y hora civil.
fn date(s: i64) -> DateTime {
    let (days, t) = (s.div_euclid(86400), s.rem_euclid(86400));
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    DateTime { year: y.clamp(1, 65535) as u16, month: m as u8, day: d as u8, hour: (t / 3600) as u8, minute: (t % 3600 / 60) as u8, second: (t % 60) as u8 }
}

/// Nombre para mostrar y comparar: con pérdida si no es UTF-8 y sin caracteres de control.
fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).chars().map(|c| if c < ' ' { '?' } else { c }).collect()
}

struct Inode {
    n: u64,
    mode: u16,
    flags: u32,
    size: u64,
    links: u16,
    mtime: DateTime,
    block: [u8; 60],
    /// Cuerpo del inodo tras `i_extra_isize` (atributos extendidos).
    xa: Vec<u8>,
}

impl Inode {
    fn ty(&self) -> u16 {
        self.mode & 0xF000
    }
    fn is_dir(&self) -> bool {
        self.ty() == 0x4000
    }
    fn is_reg(&self) -> bool {
        self.ty() == 0x8000
    }
    fn is_lnk(&self) -> bool {
        self.ty() == 0xA000
    }
    /// Lo que se muestra: carpetas, archivos y enlaces (dispositivos, tuberías y sockets no).
    fn shown(&self) -> bool {
        self.is_dir() || self.is_reg() || self.is_lnk()
    }
    fn entry(&self, name: String) -> Entry {
        Entry { name, dir: self.is_dir(), size: if self.is_dir() { 0 } else { self.size }, modified: self.mtime, hidden: false }
    }
}

/// Valor del atributo `system.data` (contenido en línea que pasa de 60 bytes) en el cuerpo del inodo.
fn inline_val(xa: &[u8]) -> Result<&[u8], FsError> {
    if xa.len() < 4 || u32le(xa, 0) != 0xEA02_0000 {
        return Err(FsError::Corrupt);
    }
    let r = &xa[4..];
    let (mut p, mut hit) = (0usize, None);
    for _ in 0..4096 {
        if p + 4 > r.len() || u32le(r, p) == 0 {
            break;
        }
        let nl = r[p] as usize;
        let esz = 16 + ((nl + 3) & !3);
        if p + esz > r.len() {
            return Err(FsError::Corrupt);
        }
        if r[p + 1] == 7 && &r[p + 16..p + 16 + nl] == b"data" {
            hit = Some((u16le(r, p + 2) as usize, u32le(r, p + 8) as usize, u32le(r, p + 4)));
        }
        p += esz;
    }
    let (vo, vl, vi) = hit.ok_or(FsError::Corrupt)?;
    // El valor debe quedar tras la lista (salvo vacío) y dentro de la región; en otro inodo no se lee.
    if vi != 0 || (vl > 0 && vo < p) || vo.checked_add(vl).map_or(true, |e| e > r.len()) {
        return Err(FsError::Corrupt);
    }
    Ok(&r[vo..vo + vl])
}

/// Contenido en línea: los 60 bytes de `i_block` y el valor de `system.data`, hasta `i_size`.
fn inline_data(i: &Inode) -> Result<Vec<u8>, FsError> {
    let v = inline_val(&i.xa)?;
    if i.size > 60 + v.len() as u64 {
        return Err(FsError::Corrupt);
    }
    let mut o = i.block.to_vec();
    o.extend_from_slice(v);
    o.truncate(i.size as usize);
    Ok(o)
}

/// Cabecera de un nodo de extents: (entradas, profundidad).
fn ext_head(n: &[u8]) -> Result<(usize, u16), FsError> {
    if n.len() < 24 || u16le(n, 0) != 0xF30A {
        return Err(FsError::Corrupt);
    }
    let (e, m, d) = (u16le(n, 2) as usize, u16le(n, 4) as usize, u16le(n, 6));
    if m == 0 || m > (n.len() - 12) / 12 || e > m || d > 5 {
        return Err(FsError::Corrupt);
    }
    Ok((e, d))
}

struct Ext {
    io: Io,
    sb: Sb,
    cache: BTreeMap<u64, Rc<Vec<u8>>>,
}

impl Ext {
    fn boff(&self, blk: u64) -> Result<u64, FsError> {
        blk.checked_mul(self.sb.bs).ok_or(FsError::Corrupt)
    }

    /// Bloque físico válido: dentro del volumen.
    fn chk(&self, n: u64) -> Result<u64, FsError> {
        if n < self.sb.first_data || n >= self.sb.total {
            return Err(FsError::Corrupt);
        }
        Ok(n)
    }

    /// Un bloque sin caché (carpetas y datos, que se leen una vez).
    fn blk_raw(&mut self, n: u64) -> Result<Vec<u8>, FsError> {
        let off = self.boff(self.chk(n)?)?;
        read_at(&mut self.io, off, self.sb.bs as usize)
    }

    /// Un bloque de metadatos, con una caché pequeña.
    fn blk(&mut self, n: u64) -> Result<Rc<Vec<u8>>, FsError> {
        if let Some(b) = self.cache.get(&n) {
            return Ok(b.clone());
        }
        let b = Rc::new(self.blk_raw(n)?);
        if self.cache.len() >= CACHE {
            self.cache.clear();
        }
        self.cache.insert(n, b.clone());
        Ok(b)
    }

    /// Primer bloque de la tabla de inodos del grupo `g`, validada.
    fn itable(&mut self, g: u64) -> Result<u64, FsError> {
        let dpb = self.sb.bs / self.sb.dsz;
        let b = self.blk(self.sb.gdt_block(g / dpb)?)?;
        let o = ((g % dpb) * self.sb.dsz) as usize;
        let mut t = u32le(&b, o + 8) as u64;
        if self.sb.dsz >= 64 {
            t |= (u32le(&b, o + 0x28) as u64) << 32;
        }
        // Inodos sin inicializar: solo es fiable con gdt_csum o metadata_csum.
        if u16le(&b, o + 0x12) & 1 != 0 && self.sb.ro & 0x410 != 0 {
            return Err(FsError::NotFound);
        }
        let n = (self.sb.ipg * self.sb.isz).div_ceil(self.sb.bs);
        if t < self.sb.first_data || t.checked_add(n).map_or(true, |e| e > self.sb.total) {
            return Err(FsError::Corrupt);
        }
        Ok(t)
    }

    fn read_inode(&mut self, n: u64) -> Result<Inode, FsError> {
        if n == 0 || n > self.sb.inodes {
            return Err(FsError::NotFound);
        }
        let (bs, isz) = (self.sb.bs, self.sb.isz);
        let t = self.itable((n - 1) / self.sb.ipg)?;
        let pos = (n - 1) % self.sb.ipg * isz;
        let b = self.blk(t + pos / bs)?;
        let o = (pos % bs) as usize;
        let r = &b[o..o + isz as usize];
        let (mode, flags) = (u16le(r, 0), u32le(r, 0x20));
        if mode == 0 {
            return Err(FsError::NotFound);
        }
        let extra = if isz > 128 { u16le(r, 0x80) as usize } else { 0 };
        if extra % 4 != 0 || 128 + extra > isz as usize || !matches!(mode & 0xF000, 0x1000 | 0x2000 | 0x4000 | 0x6000 | 0x8000 | 0xA000 | 0xC000) {
            return Err(FsError::Corrupt);
        }
        // Combinaciones imposibles.
        if (flags & F_INLINE != 0 && flags & F_EXTENTS != 0)
            || (flags & F_CASEFOLD != 0 && self.sb.incompat & IN_CASEFOLD == 0)
            || (flags & F_INDEX != 0 && self.sb.compat & 0x20 == 0 && self.sb.ro & 0x400 != 0)
        {
            return Err(FsError::Corrupt);
        }
        let mut size = u32le(r, 4) as u64;
        if mode & 0xF000 == 0x8000 || self.sb.incompat & IN_LARGEDIR != 0 {
            size |= (u32le(r, 0x6C) as u64) << 32;
        }
        if size >= 1 << 63 {
            return Err(FsError::Corrupt);
        }
        let mut secs = u32le(r, 0x10) as i32 as i64;
        if extra >= 12 {
            secs += ((u32le(r, 0x88) & 3) as i64) << 32;
        }
        let mut block = [0u8; 60];
        block.copy_from_slice(&r[0x28..0x64]);
        Ok(Inode {
            n,
            mode,
            flags,
            size,
            links: u16le(r, 0x1A),
            mtime: date(secs),
            block,
            xa: if isz > 128 { r[128 + extra..].to_vec() } else { Vec::new() },
        })
    }

    /// Inodo alcanzado desde una entrada de carpeta: no reservado, no borrado, no de atributos.
    fn entry_inode(&mut self, n: u64) -> Result<Inode, FsError> {
        if n != 2 && n < self.sb.first_ino {
            return Err(FsError::NotFound);
        }
        let i = self.read_inode(n)?;
        if i.links == 0 || i.flags & F_EA_INODE != 0 {
            return Err(FsError::NotFound);
        }
        Ok(i)
    }

    /// Bloque físico del bloque lógico `l` de un archivo (`None` = ceros: hueco o extent sin
    /// inicializar) y cuántos bloques seguidos comparten ese resultado (siempre ≥ 1).
    fn map(&mut self, i: &Inode, l: u64) -> Result<(Option<u64>, u64), FsError> {
        if l >= (1 << 32) - 1 {
            return Ok((None, 1 << 32));
        }
        if i.flags & F_EXTENTS != 0 {
            self.ext_map(&i.block, l)
        } else {
            self.ptr_map(&i.block, l)
        }
    }

    /// Punteros directos e indirectos (hasta triple): como mucho 3 lecturas, sin bucles posibles.
    fn ptr_map(&mut self, w: &[u8], l: u64) -> Result<(Option<u64>, u64), FsError> {
        let a = self.sb.bs / 4;
        if l < 12 {
            let p = u32le(w, l as usize * 4) as u64;
            return Ok((if p == 0 { None } else { Some(self.chk(p)?) }, 1));
        }
        let mut r = l - 12;
        for lvl in 1..=3u32 {
            let span = a.pow(lvl);
            if r >= span {
                r -= span;
                continue;
            }
            let mut cur = u32le(w, (11 + lvl as usize) * 4) as u64;
            for s in 0..=lvl {
                let cover = a.pow(lvl - s);
                if cur == 0 {
                    return Ok((None, cover - r % cover));
                }
                let c = self.chk(cur)?;
                if s == lvl {
                    return Ok((Some(c), 1));
                }
                let b = self.blk(c)?;
                cur = u32le(&b, ((r / (cover / a)) % a) as usize * 4) as u64;
            }
        }
        Ok((None, 1 << 32))
    }

    /// Árbol de extents. Cada nodo se valida entero: magia, profundidad exacta (padre − 1, sin
    /// ciclos), orden estricto, sin solapes y dentro del rango que le deja su índice.
    fn ext_map(&mut self, root: &[u8], l: u64) -> Result<(Option<u64>, u64), FsError> {
        let mut cur = Rc::new(root.to_vec());
        let (mut lo, mut hi, mut want) = (0u64, 1u64 << 32, None::<u16>);
        loop {
            let (e, d) = ext_head(&cur)?;
            if want.map_or(false, |w| w != d) || (d > 0 && e == 0) {
                return Err(FsError::Corrupt);
            }
            let (mut floor, mut hit, mut next, mut has_next) = (lo, None, hi, false);
            for k in 0..e {
                let o = 12 + 12 * k;
                let s = u32le(&cur, o) as u64;
                let (len, uninit, tgt);
                if d == 0 {
                    let raw = u16le(&cur, o + 4) as u64;
                    (len, uninit) = if raw > 32768 { (raw - 32768, true) } else { (raw, false) };
                    tgt = u32le(&cur, o + 8) as u64 | (u16le(&cur, o + 6) as u64) << 32;
                    if len == 0 || tgt < self.sb.first_data || tgt.checked_add(len).map_or(true, |x| x > self.sb.total) {
                        return Err(FsError::Corrupt);
                    }
                } else {
                    (len, uninit) = (1, false);
                    tgt = u32le(&cur, o + 4) as u64 | (u16le(&cur, o + 8) as u64) << 32;
                }
                if s < floor || s + len > hi {
                    return Err(FsError::Corrupt);
                }
                floor = s + len;
                if s <= l {
                    hit = Some((s, len, uninit, tgt));
                } else if !has_next {
                    (next, has_next) = (s, true);
                }
            }
            if d == 0 {
                return Ok(match hit {
                    Some((s, len, u, t)) if l < s + len => (if u { None } else { Some(t + l - s) }, s + len - l),
                    _ => (None, next - l),
                });
            }
            let Some((s, _, _, child)) = hit else { return Ok((None, next - l)) };
            cur = self.blk(child)?;
            (lo, hi, want) = (s, next, Some(d - 1));
        }
    }

    /// Contenido entero de un archivo o de un enlace lento.
    fn file_bytes(&mut self, i: &Inode) -> Result<Vec<u8>, FsError> {
        if i.flags & F_INLINE != 0 {
            return inline_data(i);
        }
        if i.size > MAXFILE {
            return Err(FsError::NoSpace);
        }
        let (bs, size) = (self.sb.bs, i.size);
        let mut out = Vec::new();
        out.try_reserve_exact(size as usize).map_err(|_| FsError::NoSpace)?;
        out.resize(size as usize, 0);
        let (nb, chunk) = (size.div_ceil(bs), ((4 << 20) / bs).max(1));
        let mut l = 0;
        while l < nb {
            let (p, run) = self.map(i, l)?;
            let n = run.min(nb - l);
            if let Some(p) = p {
                let mut done = 0;
                while done < n {
                    let c = (n - done).min(chunk);
                    let off = self.boff(self.chk(p + done)?)?;
                    let b = read_at(&mut self.io, off, (c * bs) as usize)?;
                    let (from, to) = ((l + done) * bs, ((l + done + c) * bs).min(size));
                    out[from as usize..to as usize].copy_from_slice(&b[..(to - from) as usize]);
                    done += c;
                }
            }
            l += n;
        }
        Ok(out)
    }

    /// Entradas de un área de carpeta (un bloque o un trozo en línea). `false` si `f` pidió parar.
    /// Un registro dañado abandona el resto del área, no la carpeta.
    fn area(&self, b: &[u8], f: &mut dyn FnMut(u64, &[u8]) -> bool) -> bool {
        let mut p = 0usize;
        while p + 8 <= b.len() {
            let raw = u16le(b, p + 4) as usize;
            let r = if self.sb.bs != 65536 {
                raw
            } else if raw == 0 || raw == 65535 {
                65536
            } else {
                (raw & 0xFFFC) | ((raw & 3) << 16)
            };
            let (ino, nl, end) = (u32le(b, p) as u64, b[p + 6] as usize, p + r);
            if r < 12 || r % 4 != 0 || r < (8 + nl + 3) & !3 || end > b.len() || (end < b.len() && b.len() - end < 12) || ino > self.sb.inodes {
                break;
            }
            if ino != 0 {
                let name = &b[p + 8..p + 8 + nl];
                // Sin `filetype` la longitud del nombre ocupa 16 bits: más de 255 no vale.
                let bad = name.iter().any(|&c| c == 0 || c == b'/') || (end == b.len() && name == b".") || (self.sb.incompat & IN_FILETYPE == 0 && b[p + 7] != 0);
                if bad {
                    break;
                }
                if !f(ino, name) {
                    return false;
                }
            }
            p = end;
        }
        true
    }

    /// Recorre una carpeta en el orden de sus bloques (con "." y ".."), sin cargarla entera.
    fn scan(&mut self, d: &Inode, f: &mut dyn FnMut(u64, &[u8]) -> bool) -> Result<(), FsError> {
        if !d.is_dir() {
            return Err(FsError::NotDir);
        }
        if d.flags & F_ENCRYPT != 0 {
            return Err(FsError::Denied);
        }
        if d.flags & F_INLINE != 0 {
            // "." y ".." son sintéticos; después vienen los 56 bytes de i_block y el valor de system.data.
            if !f(d.n, b".") || !f(u32le(&d.block, 0) as u64, b"..") || !self.area(&d.block[4..], f) {
                return Ok(());
            }
            self.area(inline_val(&d.xa)?, f);
            return Ok(());
        }
        if d.size > MAXDIR {
            return Err(FsError::Corrupt);
        }
        let nb = d.size.div_ceil(self.sb.bs);
        let mut l = 0;
        while l < nb {
            let (p, run) = self.map(d, l)?;
            let k = run.min(nb - l);
            if let Some(p) = p {
                for j in 0..k {
                    let b = self.blk_raw(p + j)?;
                    if !self.area(&b, f) {
                        return Ok(());
                    }
                }
            }
            l += k;
        }
        Ok(())
    }

    /// Destino de un enlace simbólico (rápido en `i_block`, en línea o lento en bloques).
    fn link_target(&mut self, i: &Inode) -> Result<Vec<u8>, FsError> {
        if i.flags & F_ENCRYPT != 0 {
            return Err(FsError::Denied);
        }
        if i.size == 0 || i.size > 4095 {
            return Err(FsError::Corrupt);
        }
        if i.flags & (F_INLINE | F_EXTENTS) == 0 && i.size < 60 {
            let t = &i.block[..i.size as usize];
            return if t.contains(&0) { Err(FsError::Corrupt) } else { Ok(t.to_vec()) };
        }
        self.file_bytes(i)
    }

    /// Resuelve una ruta. Los enlaces relativos que quedan dentro de la unidad se siguen (hasta
    /// `MAXLINKS`); los absolutos o que suben por encima de la raíz dan `Denied`. Si `follow` es
    /// falso no se sigue un enlace en el último componente. Devuelve el inodo y su nombre.
    fn resolve(&mut self, path: &str, follow: bool) -> Result<(Inode, String), FsError> {
        let mut todo: Vec<(String, bool)> = split(path).iter().rev().map(|c| (String::from(*c), false)).collect();
        if path.len() > 4096 || todo.len() > MAXCOMP {
            return Err(FsError::BadPath);
        }
        let mut stack = alloc::vec![2u64];
        let (mut cur, mut name, mut hops) = (self.entry_inode(2)?, String::new(), 0);
        while let Some((c, from_link)) = todo.pop() {
            if !cur.is_dir() {
                return Err(FsError::NotDir);
            }
            if c == "." {
                continue;
            }
            if c == ".." {
                if stack.len() > 1 {
                    stack.pop();
                } else if from_link {
                    return Err(FsError::Denied);
                }
                cur = self.entry_inode(stack.last().copied().unwrap_or(2))?;
                continue;
            }
            let mut found = None;
            self.scan(&cur, &mut |n, nm| {
                let s = show(nm);
                if nm != b"." && nm != b".." && same_name(&s, &c) {
                    found = Some((n, s));
                    return false;
                }
                true
            })?;
            let (n, s) = found.ok_or(FsError::NotFound)?;
            let ni = self.entry_inode(n)?;
            if ni.is_lnk() && (follow || !todo.is_empty()) {
                hops += 1;
                if hops > MAXLINKS {
                    return Err(FsError::Invalid);
                }
                let t = self.link_target(&ni)?;
                if t.first() == Some(&b'/') {
                    return Err(FsError::Denied);
                }
                todo.extend(String::from_utf8_lossy(&t).split('/').rev().filter(|x| !x.is_empty()).map(|x| (String::from(x), true)));
                if todo.len() > MAXCOMP {
                    return Err(FsError::BadPath);
                }
                continue;
            }
            if ni.is_dir() {
                stack.push(n);
            }
            (cur, name) = (ni, s);
        }
        Ok((cur, name))
    }
}

impl Volume for Ext {
    fn kind(&self) -> FsKind {
        FsKind::Ext
    }
    fn label(&self) -> String {
        let l = &self.sb.label;
        show(&l[..l.iter().position(|&c| c == 0).unwrap_or(16)])
    }
    fn space(&mut self) -> (u64, u64) {
        let total = self.sb.total.saturating_mul(self.sb.bs);
        (total, self.sb.free.saturating_mul(self.sb.bs).min(total))
    }
    fn read_only(&self) -> bool {
        true
    }
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, FsError> {
        let (d, _) = self.resolve(dir, true)?;
        let mut v: Vec<(u64, String)> = Vec::new();
        let mut full = false;
        self.scan(&d, &mut |n, nm| {
            if nm != b"." && nm != b".." {
                full = v.len() >= MAXENT;
                v.push((n, show(nm)));
            }
            !full
        })?;
        if full {
            return Err(FsError::NoSpace);
        }
        // Una entrada con el inodo dañado o de otro tipo se salta: no tumba la carpeta.
        Ok(v.into_iter().filter_map(|(n, s)| self.entry_inode(n).ok().filter(|i| i.shown()).map(|i| i.entry(s))).collect())
    }
    fn stat(&mut self, path: &str) -> Result<Entry, FsError> {
        let (i, name) = self.resolve(path, false)?;
        if !i.shown() {
            return Err(FsError::NotFound);
        }
        Ok(i.entry(name))
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        let (i, _) = self.resolve(path, true)?;
        if i.is_dir() {
            return Err(FsError::IsDir);
        }
        if !i.is_reg() {
            return Err(FsError::NotFound);
        }
        if i.flags & F_ENCRYPT != 0 {
            return Err(FsError::Denied);
        }
        self.file_bytes(&i)
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
    fn notice(&self) -> Option<&'static str> {
        // Necesita recuperación del diario, o no se desmontó limpio / tiene errores registrados.
        if self.sb.incompat & IN_RECOVER != 0 || self.sb.state & 1 == 0 || self.sb.state & 2 != 0 {
            return Some("La unidad no se cerró bien en Linux: pueden faltar los últimos cambios.");
        }
        None
    }
}

/// Monta la partición `p` de `dev`. `Err(FsError::Corrupt)` si no es válida.
pub fn mount(dev: Box<dyn SectorDev>, p: &Partition) -> Result<Box<dyn Volume>, FsError> {
    let ss = dev.sector_size();
    let mut io = Io { dev, start: p.start, len: p.sectors, ss };
    let sb = parse_sb(&read_at(&mut io, 1024, 1024)?)?;
    let mut e = Ext { io, sb, cache: BTreeMap::new() };
    // La raíz (inodo 2) tiene que ser una carpeta legible.
    match e.entry_inode(2) {
        Ok(r) if r.is_dir() => Ok(Box::new(e)),
        _ => Err(FsError::Corrupt),
    }
}

#[cfg(any(test, feature = "imagenes"))]
pub mod pruebas;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vol::{partitions, MemSectors};
    use alloc::format;
    use alloc::vec;

    use super::pruebas::*;

    /// Imagen A del anexo 4.1: ext2 de 1 KiB, 64 bloques, inodos de 128, indirección simple y huecos.
    fn img_a() -> Vec<u8> {
        let mut d = vec![0u8; 64 * 1024];
        let s = 1024;
        for (o, v) in [(0, 16u32), (4, 64), (0xC, 38), (0x10, 2), (0x14, 1), (0x20, 8192), (0x24, 8192), (0x28, 16), (0x4C, 1), (0x54, 11), (0x60, 2)] {
            w32(&mut d, s + o, v);
        }
        for (o, v) in [(0x38, 0xEF53u16), (0x3A, 1), (0x3C, 1), (0x58, 128)] {
            w16(&mut d, s + o, v);
        }
        d[s + 0x78..s + 0x80].copy_from_slice(b"TESTEXT2");
        for (o, v) in [(0, 3u32), (4, 4), (8, 5)] {
            w32(&mut d, 2048 + o, v);
        }
        let at = |n: usize| 5 * 1024 + (n - 1) * 128;
        inode(&mut d, at(2), 0x41ED, 0, 1024, &words(&[7]));
        inode(&mut d, at(11), 0x81A4, 0, 13, &words(&[8]));
        inode(&mut d, at(12), 0xA1FF, 0, 9, b"hello.txt");
        let mut big: Vec<u32> = (9..=20).collect();
        big.push(21);
        inode(&mut d, at(13), 0x81A4, 0, 14 * 1024, &words(&big));
        inode(&mut d, at(14), 0x81A4, 0, 3072, &words(&[24, 0, 25]));
        dir(&mut d, 7 * 1024, 1024, &[(2, "."), (2, ".."), (11, "hello.txt"), (12, "link"), (13, "big"), (14, "sparse")]);
        d[8 * 1024..8 * 1024 + 13].copy_from_slice(b"Hello, ext2!\n");
        for i in 0..12 {
            fill(&mut d, 9 + i, i as u8 + 1);
        }
        w32(&mut d, 21 * 1024, 22);
        w32(&mut d, 21 * 1024 + 4, 23);
        fill(&mut d, 22, 0x0D);
        fill(&mut d, 23, 0x0E);
        fill(&mut d, 24, 0xAA);
        fill(&mut d, 25, 0xBB);
        d
    }

    // Disposición de la imagen B (como la del anexo, con 32 inodos para tener sitio para más casos).
    const ROOT: u32 = 13;
    const D1: u32 = 14;
    const D2: u32 = 15;
    const LEAF: u32 = 16;
    const DEEP: u32 = 17;
    const HD: u32 = 20;
    const LOOP: u32 = 22;
    /// Inicio del inodo `n` de la imagen B.
    fn at4(n: usize) -> usize {
        5 * 1024 + (n - 1) * 256
    }

    /// Imagen B: ext4 de 1 KiB, extents, 64bit, inline_data, inodos de 256.
    fn img_b() -> Vec<u8> {
        let mut d = vec![0u8; 128 * 1024];
        let s = 1024;
        for (o, v) in [(0, 32u32), (4, 128), (0x14, 1), (0x20, 8192), (0x24, 8192), (0x28, 32), (0x4C, 1), (0x54, 11), (0x60, 0x80C2), (0x64, 0x42)] {
            w32(&mut d, s + o, v);
        }
        for (o, v) in [(0x38, 0xEF53u16), (0x3A, 1), (0x58, 256), (0xFE, 64), (0x15C, 32)] {
            w16(&mut d, s + o, v);
        }
        d[s + 0x78..s + 0x80].copy_from_slice(b"TESTEXT4");
        for (o, v) in [(0, 3u32), (4, 4), (8, 5)] {
            w32(&mut d, 2048 + o, v);
        }
        let i4 = |d: &mut Vec<u8>, n: usize, mode: u16, flags: u32, size: u32, blk: &[u8]| {
            inode(d, at4(n), mode, flags, size, blk);
            w16(d, at4(n) + 0x80, 32);
        };
        const X: u32 = 0x80000;
        i4(&mut d, 2, 0x41ED, X, 1024, &eroot(0, 4, &[leaf(0, 1, ROOT)]));
        i4(&mut d, 11, 0x81A4, X, 4096, &eroot(0, 4, &[leaf(0, 1, D1), leaf(3, 32769, D2)]));
        i4(&mut d, 12, 0x81A4, X, 3072, &eroot(1, 4, &[idx(0, LEAF)]));
        i4(&mut d, 13, 0x81A4, 0x1000_0000, 10, b"inline-ok\n");
        i4(&mut d, 14, 0x81A4, 0x1000_0000, 70, &[b'A'; 60]);
        let mut idir = vec![0u8; 60];
        w32(&mut idir, 0, 2);
        w32(&mut idir, 4, 13);
        w16(&mut idir, 8, 56);
        idir[10] = 1;
        idir[11] = 1;
        idir[12] = b'a';
        i4(&mut d, 15, 0x41ED, 0x1000_0000, 60, &idir);
        i4(&mut d, 16, 0xA1FF, 0, 7, b"ext.bin");
        i4(&mut d, 17, 0xA1FF, 0, 8, b"/ext.bin");
        i4(&mut d, 18, 0xA1FF, 0, 4, b"../x");
        i4(&mut d, 19, 0x81A4, 0x800 | X, 5, &eroot(0, 4, &[leaf(0, 1, D1)]));
        i4(&mut d, 20, 0x41ED, 0x1000 | X, 2048, &eroot(0, 4, &[leaf(0, 2, HD)]));
        i4(&mut d, 21, 0x81A4, X, 5000, &eroot(1, 4, &[idx(0, LOOP)]));
        i4(&mut d, 22, 0xA1FF, 0, 4, b"self");
        i4(&mut d, 23, 0xA1FF, 0, 4, b"hdir");
        i4(&mut d, 24, 0x2000 | 0o644, 0, 0, &[]);
        // system.data de los inodos en línea (en el byte 160 del inodo).
        for (n, val) in [(13usize, &b""[..]), (14, &[b'B'; 10][..]), (15, &b""[..])] {
            let r = at4(n) + 160;
            w32(&mut d, r, 0xEA02_0000);
            d[r + 4] = 4;
            d[r + 5] = 7;
            w16(&mut d, r + 6, if val.is_empty() { 0 } else { 80 });
            w32(&mut d, r + 12, val.len() as u32);
            d[r + 20..r + 24].copy_from_slice(b"data");
            d[r + 84..r + 84 + val.len()].copy_from_slice(val);
        }
        let root = [(2, "."), (2, ".."), (11, "ext.bin"), (12, "deep.bin"), (13, "inl.txt"), (14, "inl70.txt"), (15, "idir"), (16, "rel"), (17, "abs"), (18, "out"), (19, "enc.txt"), (20, "hdir"), (21, "loop"), (22, "self"), (23, "dlnk"), (24, "dev")];
        dir(&mut d, ROOT as usize * 1024, 1024, &root);
        fill(&mut d, D1 as usize, 0x11);
        fill(&mut d, D2 as usize, 0xFF);
        let l = eroot(0, 84, &[leaf(0, 2, DEEP), leaf(2, 1, DEEP + 2)]);
        d[LEAF as usize * 1024..LEAF as usize * 1024 + l.len()].copy_from_slice(&l);
        for k in 0..3 {
            fill(&mut d, DEEP as usize + k, 0x21 + k as u8);
        }
        dir(&mut d, HD as usize * 1024, 1024, &[(20, "."), (2, "..")]);
        dir(&mut d, (HD as usize + 1) * 1024, 1024, &[(13, "in_h")]);
        let l = eroot(1, 84, &[idx(0, LOOP)]);
        d[LOOP as usize * 1024..LOOP as usize * 1024 + l.len()].copy_from_slice(&l);
        d
    }

    fn mnt(d: Vec<u8>) -> Result<Box<dyn Volume>, FsError> {
        let n = d.len() as u64 / 512;
        mount(Box::new(MemSectors { data: d, sector: 512, ro: false }), &Partition { start: 0, sectors: n, kind: FsKind::Ext })
    }
    fn names(v: &mut dyn Volume, dir: &str) -> Vec<String> {
        v.list(dir).unwrap().into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn imagen_a_ext2() {
        // `partitions` la reconoce y `vol::mount` la monta.
        let mut m = MemSectors { data: img_a(), sector: 512, ro: false };
        let ps = partitions(&mut m).unwrap();
        assert_eq!(ps[0].kind, FsKind::Ext);
        let mut v = crate::vol::mount(Box::new(m), &ps[0]).unwrap();
        assert_eq!((v.kind(), v.label(), v.read_only(), v.notice()), (FsKind::Ext, "TESTEXT2".into(), true, None));
        assert_eq!(v.space(), (64 * 1024, 38 * 1024));
        assert_eq!(names(&mut *v, ""), ["hello.txt", "link", "big", "sparse"]);
        assert_eq!(v.read("hello.txt").unwrap(), b"Hello, ext2!\n");
        assert_eq!(v.read("HELLO.txt").unwrap(), b"Hello, ext2!\n");
        assert_eq!(v.read("\\link").unwrap(), b"Hello, ext2!\n");
        let e = v.stat("hello.txt").unwrap();
        assert_eq!((e.size, e.dir, e.hidden), (13, false, false));
        assert_eq!(e.modified, DateTime { year: 2023, month: 11, day: 14, hour: 22, minute: 13, second: 20 });
        assert!(v.stat("").unwrap().dir);
        let mut big = Vec::new();
        for i in 0..12u8 {
            big.extend_from_slice(&[i + 1; 1024]);
        }
        big.extend_from_slice(&[0x0D; 1024]);
        big.extend_from_slice(&[0x0E; 1024]);
        assert_eq!(v.read("big").unwrap(), big);
        let sp = v.read("sparse").unwrap();
        assert_eq!((sp.len(), sp[0], sp[1023], sp[1024], sp[2047], sp[2048]), (3072, 0xAA, 0xAA, 0, 0, 0xBB));
        assert_eq!(v.read("nada"), Err(FsError::NotFound));
        assert_eq!(v.read("hello.txt\\x"), Err(FsError::NotDir));
        assert_eq!(v.write("a", b"x", DateTime::default()), Err(FsError::Denied));
        assert_eq!(v.mkdir("a", DateTime::default()), Err(FsError::Denied));
        assert_eq!(v.rename("hello.txt", "b"), Err(FsError::Denied));
        assert_eq!(v.delete("hello.txt"), Err(FsError::Denied));
        assert_eq!(v.flush(), Ok(()));
    }

    #[test]
    fn imagen_a_rechazos() {
        assert!(mnt(img_a()).is_ok());
        let cambia = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut d = img_a();
            f(&mut d);
            assert_eq!(mnt(d).err(), Some(FsError::Corrupt));
        };
        cambia(&|d| w16(d, 1024 + 0x38, 0xEF54));
        cambia(&|d| w32(d, 1024 + 0x18, 7));
        cambia(&|d| w32(d, 1024, 17));
        cambia(&|d| w32(d, 1024 + 0x28, 12));
        cambia(&|d| w32(d, 1024 + 0x4C, 2));
        cambia(&|d| w32(d, 1024 + 0x60, 0x1));
        cambia(&|d| w32(d, 1024 + 0x60, 0x8));
        cambia(&|d| w32(d, 1024 + 0x60, 0x1000));
        cambia(&|d| w32(d, 1024 + 0x60, 0x8_0000));
        cambia(&|d| w32(d, 1024 + 0x14, 0));
        // Compatibles y ro_compat desconocidas no impiden montar.
        let mut d = img_a();
        w32(&mut d, 1024 + 0x5C, 0x8000_0000);
        w32(&mut d, 1024 + 0x64, 0x8000_0000);
        assert!(mnt(d).is_ok());
    }

    #[test]
    fn imagen_b_ext4() {
        let mut v = mnt(img_b()).unwrap();
        assert_eq!((v.label(), v.notice()), ("TESTEXT4".into(), None));
        let l = names(&mut *v, "");
        // El dispositivo (inodo 24) no sale; "." y ".." tampoco.
        assert_eq!(l, ["ext.bin", "deep.bin", "inl.txt", "inl70.txt", "idir", "rel", "abs", "out", "enc.txt", "hdir", "loop", "self", "dlnk"]);
        // Extent inicializado, huecos y extent sin inicializar (el bloque físico tiene 0xFF).
        let e = v.read("ext.bin").unwrap();
        assert_eq!(e.len(), 4096);
        assert!(e[..1024].iter().all(|&b| b == 0x11) && e[1024..].iter().all(|&b| b == 0));
        // Nodo índice.
        let dp = v.read("deep.bin").unwrap();
        assert_eq!((dp.len(), dp[0], dp[1024], dp[2048], dp[3071]), (3072, 0x21, 0x22, 0x23, 0x23));
        // Datos en línea: solo i_block, y con el valor de system.data.
        assert_eq!(v.read("inl.txt").unwrap(), b"inline-ok\n");
        let mut a70 = vec![b'A'; 60];
        a70.extend_from_slice(&[b'B'; 10]);
        assert_eq!(v.read("inl70.txt").unwrap(), a70);
        // Carpeta en línea: "." y ".." sintéticos y "a" (inodo 13).
        assert_eq!(names(&mut *v, "idir"), ["a"]);
        assert_eq!(v.read("idir\\a").unwrap(), b"inline-ok\n");
        // Carpeta con índice: el bloque 0 no da entradas y el 1 sí.
        assert_eq!(names(&mut *v, "hdir"), ["in_h"]);
        assert_eq!(v.read("hdir\\in_h").unwrap(), b"inline-ok\n");
        assert!(v.stat("idir").unwrap().dir);
        assert_eq!(v.stat("dev"), Err(FsError::NotFound));
    }

    #[test]
    fn imagen_b_rechazos_y_aviso() {
        for bit in [0x1u32, 0x8, 0x1000, 0x1000_0000] {
            let mut d = img_b();
            w32(&mut d, 1024 + 0x60, 0x80C2 | bit);
            assert_eq!(mnt(d).err(), Some(FsError::Corrupt), "{bit:#x}");
        }
        let mut d = img_b();
        w32(&mut d, 1024 + 0x60, 0x80C2 | 0x4);
        let v = mnt(d).unwrap();
        assert_eq!(v.notice(), Some("La unidad no se cerró bien en Linux: pueden faltar los últimos cambios."));
        let mut d = img_b();
        w16(&mut d, 1024 + 0x3A, 0);
        assert!(mnt(d).unwrap().notice().is_some());
        // Profundidad o magia erróneas en el nodo hoja: deep.bin falla, el resto sigue.
        let malos: [&dyn Fn(&mut Vec<u8>); 2] = [&|d| w16(d, LEAF as usize * 1024 + 6, 1), &|d| w16(d, LEAF as usize * 1024, 0xF30B)];
        for f in malos {
            let mut d = img_b();
            f(&mut d);
            let mut v = mnt(d).unwrap();
            assert_eq!(v.read("deep.bin"), Err(FsError::Corrupt));
            assert_eq!(v.read("inl.txt").unwrap(), b"inline-ok\n");
        }
        // Árbol que se referencia a sí mismo (profundidad igual en padre e hijo).
        assert_eq!(mnt(img_b()).unwrap().read("loop"), Err(FsError::Corrupt));
    }

    #[test]
    fn enlaces_y_cifrado() {
        let mut v = mnt(img_b()).unwrap();
        assert_eq!(v.read("rel").unwrap().len(), 4096);
        // Un enlace simbólico se lista como archivo, con el tamaño del destino escrito.
        let e = v.stat("rel").unwrap();
        assert_eq!((e.dir, e.size), (false, 7));
        // Absolutos o que salen de la raíz: se ven, pero no se leen.
        assert_eq!(v.read("abs"), Err(FsError::Denied));
        assert_eq!(v.read("out"), Err(FsError::Denied));
        assert!(v.stat("abs").is_ok() && v.stat("out").is_ok());
        // Bucle: tope de saltos.
        assert_eq!(v.read("self"), Err(FsError::Invalid));
        // Enlace relativo a una carpeta, atravesado por la ruta.
        assert_eq!(v.read("dlnk\\in_h").unwrap(), b"inline-ok\n");
        // Cifrado: se lista, no se abre.
        assert_eq!(v.stat("enc.txt").unwrap().size, 5);
        assert_eq!(v.read("enc.txt"), Err(FsError::Denied));
        let mut d = img_b();
        w32(&mut d, at4(20) + 0x20, 0x800 | 0x1000 | 0x80000);
        let mut v = mnt(d).unwrap();
        assert!(v.stat("hdir").unwrap().dir);
        assert_eq!(v.list("hdir"), Err(FsError::Denied));
    }

    #[test]
    fn fechas() {
        assert_eq!(date(0), DateTime { year: 1970, month: 1, day: 1, hour: 0, minute: 0, second: 0 });
        assert_eq!(date(1_700_000_000), DateTime { year: 2023, month: 11, day: 14, hour: 22, minute: 13, second: 20 });
        // Época extendida: base 0x80000000 (negativa como i32) con época 1 son 2^31 s reales.
        assert_eq!(date(-0x8000_0000 + (1 << 32)), DateTime { year: 2038, month: 1, day: 19, hour: 3, minute: 14, second: 8 });
        assert_eq!(date(-1), DateTime { year: 1969, month: 12, day: 31, hour: 23, minute: 59, second: 59 });
        assert_eq!(date(i32::MIN as i64).year, 1901);
    }

    /// Recorre y lee todo lo que se pueda: solo se exige que no haya pánico ni bucle sin fin.
    fn recorrer(v: &mut dyn Volume, dir: &str, prof: u32, vistos: &mut u32) {
        if prof > 4 || *vistos > 300 {
            return;
        }
        let _ = (v.label(), v.space(), v.notice());
        let Ok(es) = v.list(dir) else { return };
        for e in es.into_iter().take(32) {
            *vistos += 1;
            let p = if dir.is_empty() { e.name.clone() } else { format!("{dir}\\{}", e.name) };
            let _ = v.stat(&p);
            if e.dir {
                recorrer(v, &p, prof + 1, vistos);
            } else {
                let _ = v.read(&p);
            }
        }
    }

    #[test]
    fn imagenes_danadas_no_hacen_caer() {
        // Casos concretos: registro de 0, registro que se sale del bloque, nombre más largo que el
        // registro, inodo fuera de rango, tamaños imposibles, extra_isize impar, extent fuera del volumen.
        let root = ROOT as usize * 1024;
        let casos: [&dyn Fn(&mut Vec<u8>); 9] = [
            &|d| w16(d, root + 4, 0),
            &|d| w16(d, root + 12 + 4, 4000),
            &|d| d[root + 24 + 6] = 255,
            &|d| w32(d, root + 24, 9999),
            &|d| w16(d, root + 24 + 4, 7),
            &|d| w32(d, at4(11) + 4, u32::MAX),
            &|d| w32(d, at4(11) + 0x6C, u32::MAX),
            &|d| w16(d, at4(11) + 0x80, 33),
            &|d| w32(d, at4(11) + 0x28 + 12 + 12 + 8, 0xFFFF_FFF0),
        ];
        for f in casos {
            let mut d = img_b();
            f(&mut d);
            if let Ok(mut v) = mnt(d) {
                recorrer(&mut *v, "", 0, &mut 0);
            }
        }
        // Registro de 65535 en un bloque de 4096 (no cabe): se abandona el resto del bloque.
        let a = img_a();
        let sb = parse_sb(&a[1024..2048]).unwrap();
        let e = Ext { io: Io { dev: Box::new(MemSectors { data: a, sector: 512, ro: false }), start: 0, len: 128, ss: 512 }, sb: Sb { bs: 65536, ..sb }, cache: BTreeMap::new() };
        let mut b = vec![0u8; 4096];
        w32(&mut b, 0, 2);
        w16(&mut b, 4, 65535);
        let mut n = 0;
        e.area(&b, &mut |_, _| {
            n += 1;
            true
        });
        assert_eq!(n, 0);
        // Mutaciones al azar de los metadatos de las dos imágenes.
        let mut x = 0x9E37_79B9u32;
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        for (base, zona) in [(img_a(), 26 * 1024), (img_b(), 24 * 1024)] {
            let mut montados = 0;
            for n in 0..600 {
                let mut d = base.clone();
                for _ in 0..1 + n % 12 {
                    let i = rnd() as usize % zona;
                    d[i] = rnd() as u8;
                }
                if let Ok(mut v) = mnt(d) {
                    montados += 1;
                    recorrer(&mut *v, "", 0, &mut 0);
                }
            }
            assert!(montados > 100, "solo {montados} montadas");
        }
    }
}
