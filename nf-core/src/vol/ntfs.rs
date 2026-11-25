// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! NTFS (Windows) en solo lectura.
//!
//! Sala limpia: se escribe solo a partir de `F:\IdeaProjects\especificaciones\ntfs\ESPECIFICACION.md`.
//!
//! CONTRATO (v0.11.1): `mount` devuelve un `Volume` cuyo `read_only()` es `true` y cuyas escrituras
//! dan `FsError::Denied`. Rutas como en `vol` (relativas a la raíz, con `\`, sin distinguir
//! mayúsculas). `Entry::hidden` marca lo oculto o de sistema.
//!
//! La imagen es entrada hostil: la geometría se valida al montar, todo registro lleva sus fixups
//! comprobados, las runlists se validan contra el volumen y toda lectura va con tope. Las carpetas se
//! recorren de forma lineal (cada bloque INDX asignado, una vez), así que un árbol con ciclos no
//! cuelga nada. No se sigue ningún punto de reanálisis ni se descifra EFS.
//!
//! shortcut: no se reproduce el $LogFile (solo se avisa si el volumen no se cerró bien); los flujos con
//! nombre no salen; WOF/dedup/nube se tratan como reanálisis (`Denied`) sin leerlos; la caché de
//! registros es pequeña y no hay caché de unidades comprimidas (cada unidad se descomprime una vez por
//! lectura); la búsqueda por nombre enumera la carpeta (O(n)) en vez de bajar por el árbol.
use super::fat::{split, upper, Io};
use super::iso::read_at;
use super::part::{u16le, u32le, u64le};
use super::{FsKind, Partition, SectorDev, Volume};
use crate::fs::{Entry, FsError};
use crate::platform::DateTime;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

/// Topes: bytes de una carpeta que se recorre, entradas devueltas, archivo leído entero, componentes
/// de una ruta, registros en caché, tramos por runlist, registros de extensión por archivo y bytes de
/// una lista de atributos.
const MAXDIR: u64 = 1 << 25;
const MAXENT: usize = 1 << 18;
const MAXFILE: u64 = 1 << 30;
const MAXCOMP: usize = 256;
const CACHE: usize = 64;
const MAXRUNS: usize = 1_000_000;
const MAXEXT: usize = 1024;
const MAXLIST: u64 = 1 << 23;
/// Una unidad de compresión descomprimida no pasa de 1 MiB; un trozo LZNT1 da 4096 bytes.
const MAXUNIT: u64 = 1 << 20;
const MASK: u64 = (1 << 48) - 1;
const ROOT: u64 = 5;
/// "$I30" en UTF-16.
const I30: [u16; 4] = [0x24, 0x49, 0x33, 0x30];
const MSG_SUCIO: &str = "Windows no cerró bien esta unidad (o está en inicio rápido): puede que falten los últimos cambios.";
const MSG_COPIA: &str = "Esta unidad NTFS tenía dañado su arranque o su tabla principal: se usó una copia.";

/// Descompresión LZNT1, pura: bytes comprimidos a bytes, con tope de salida.
mod lznt1 {
    use crate::fs::FsError;
    use alloc::vec::Vec;

    /// Descomprime una secuencia de trozos (cada uno da hasta 4096 bytes). Termina en una cabecera 0 o
    /// al agotar `src`. Error si algo no cuadra o la salida pasa de `max`.
    pub fn decode(src: &[u8], max: usize) -> Result<Vec<u8>, FsError> {
        let bad = || FsError::Corrupt;
        let (mut out, mut pos) = (Vec::new(), 0usize);
        while pos + 2 <= src.len() {
            let h = u16::from_le_bytes([src[pos], src[pos + 1]]) as usize;
            if h == 0 {
                break;
            }
            // Bits 12 a 14: siempre 3.
            if (h >> 12) & 7 != 3 {
                return Err(bad());
            }
            let end = pos + 2 + (h & 0xFFF) + 1;
            if end > src.len() {
                return Err(bad());
            }
            pos += 2;
            let start = out.len();
            if h & 0x8000 == 0 {
                out.extend_from_slice(&src[pos..end]);
                pos = end;
            }
            while pos < end {
                let flags = src[pos];
                pos += 1;
                for bit in 0..8 {
                    if pos >= end {
                        break;
                    }
                    if flags >> bit & 1 == 0 {
                        out.push(src[pos]);
                        pos += 1;
                    } else {
                        if pos + 2 > end {
                            return Err(bad());
                        }
                        let r = u16::from_le_bytes([src[pos], src[pos + 1]]) as usize;
                        pos += 2;
                        let p = out.len() - start;
                        if p == 0 {
                            return Err(bad());
                        }
                        // Bits del desplazamiento: techo de log2(p), entre 4 y 12.
                        let ob = (usize::BITS - (p - 1).leading_zeros()).max(4);
                        let lb = 16 - ob;
                        let (len, disp) = ((r & ((1 << lb) - 1)) + 3, (r >> lb) + 1);
                        if disp > p || p + len > 4096 {
                            return Err(bad());
                        }
                        for _ in 0..len {
                            out.push(out[out.len() - disp]);
                        }
                    }
                    if out.len() - start > 4096 {
                        return Err(bad());
                    }
                }
            }
            if out.len() > max {
                return Err(bad());
            }
        }
        Ok(out)
    }
}

/// Tramo de una runlist: `lcn` es `None` en un hueco (lee como ceros).
#[derive(Clone, Copy)]
struct Run {
    vcn: u64,
    len: u64,
    lcn: Option<u64>,
}

/// Contenido no residente: tramos ordenados y contiguos desde el VCN 0, tamaño de datos, tamaño
/// inicializado y, si está comprimido, el log2 de clusters por unidad (0 = sin compresión).
#[derive(Clone, Default)]
struct Stream {
    runs: Vec<Run>,
    size: u64,
    init: u64,
    unit: u32,
}

enum Val {
    Res(Vec<u8>),
    Non(Stream),
}

/// Atributo lógico: sus fragmentos ya están fusionados.
struct Attr {
    ty: u32,
    name: Vec<u16>,
    flags: u16,
    val: Val,
}

struct File {
    num: u64,
    flags: u16,
    attrs: Vec<Attr>,
}

impl File {
    fn attr(&self, ty: u32, name: &[u16]) -> Option<&Attr> {
        self.attrs.iter().find(|a| a.ty == ty && a.name == name)
    }
    fn data(&self) -> Option<&Attr> {
        self.attr(0x80, &[])
    }
    fn is_dir(&self) -> bool {
        self.flags & 2 != 0
    }
    /// (modificación en FILETIME, atributos de Windows) de $STANDARD_INFORMATION.
    fn si(&self) -> (u64, u32) {
        match self.attr(0x10, &[]) {
            Some(Attr { val: Val::Res(v), .. }) if v.len() >= 0x24 => (u64le(v, 8), u32le(v, 0x20)),
            _ => (0, 0),
        }
    }
    fn reparse(&self) -> bool {
        self.attr(0xC0, &[]).is_some() || self.si().1 & 0x400 != 0
    }
    fn encrypted(&self) -> bool {
        self.si().1 & 0x4000 != 0 || self.data().map_or(false, |a| a.flags & 0x4000 != 0)
    }
    fn size(&self) -> u64 {
        match self.data() {
            Some(Attr { val: Val::Res(v), .. }) => v.len() as u64,
            Some(Attr { val: Val::Non(s), .. }) => s.size,
            None => 0,
        }
    }
}

/// Fragmento de atributo tal como está en un registro (aún sin fusionar).
#[derive(Clone)]
struct Part {
    ty: u32,
    name: Vec<u16>,
    id: u16,
    flags: u16,
    vcn0: u64,
    vcn1: u64,
    size: u64,
    init: u64,
    unit: u16,
    val: Option<Vec<u8>>,
    runs: Vec<Run>,
}

struct Geo {
    cl: u64,
    rec: usize,
    clusters: u64,
    mft: u64,
    mirr: u64,
}

fn u16s(b: &[u8]) -> Vec<u16> {
    b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
}

/// Nombre para mostrar: con pérdida si hay sustitutos sueltos y sin caracteres de control.
fn show(b: &[u16]) -> String {
    char::decode_utf16(b.iter().copied()).map(|r| r.unwrap_or('\u{FFFD}')).map(|c| if c < ' ' { '?' } else { c }).collect()
}

/// FILETIME (100 ns desde 1601) a fecha y hora civil; 0 queda como fecha vacía.
fn date(t: u64) -> DateTime {
    if t == 0 {
        return DateTime::default();
    }
    let s = (t / 10_000_000) as i64 - 11_644_473_600;
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

/// Tamaño codificado en un s8 del arranque: positivo = clusters, negativo = 2^(-v) bytes.
fn boot_size(v: i8, cl: u64) -> Option<u64> {
    let v = v as i32;
    if v > 0 {
        cl.checked_mul(v as u64)
    } else if v > -32 {
        Some(1u64 << -v)
    } else {
        None
    }
}

/// Valida el sector de arranque. `pbytes` es el tamaño de la partición en bytes.
fn parse_boot(s: &[u8], pbytes: u64) -> Result<Geo, FsError> {
    let bad = || FsError::Corrupt;
    if s.len() < 512 || &s[3..11] != b"NTFS    " {
        return Err(bad());
    }
    let bps = u16le(s, 0xB) as u64;
    if !bps.is_power_of_two() || !(512..=4096).contains(&bps) {
        return Err(bad());
    }
    let raw = s[0xD] as u32;
    let spc = match raw {
        1..=128 if raw.is_power_of_two() => raw as u64,
        129..=255 if 256 - raw <= 16 => 1u64 << (256 - raw),
        _ => return Err(bad()),
    };
    let cl = bps * spc;
    if cl > 2 << 20 {
        return Err(bad());
    }
    // Campos que en NTFS valen cero.
    if u16le(s, 0xE) != 0 || s[0x10] != 0 || u16le(s, 0x11) != 0 || u16le(s, 0x13) != 0 || u16le(s, 0x16) != 0 || u32le(s, 0x20) != 0 {
        return Err(bad());
    }
    let total = u64le(s, 0x28);
    if total == 0 {
        return Err(bad());
    }
    // Un dispositivo más pequeño que el volumen lo recorta.
    let usable = total.checked_mul(bps).ok_or_else(bad)?.min(pbytes);
    let clusters = usable / cl;
    let (mft, mirr) = (u64le(s, 0x30) as i64, u64le(s, 0x38) as i64);
    if mft <= 0 || mirr <= 0 || mft == mirr || mft as u64 >= clusters || mirr as u64 >= clusters {
        return Err(bad());
    }
    let rec = boot_size(s[0x40] as i8, cl).ok_or_else(bad)?;
    if !rec.is_power_of_two() || !(512..=4096).contains(&rec) {
        return Err(bad());
    }
    Ok(Geo { cl, rec: rec as usize, clusters, mft: mft as u64, mirr: mirr as u64 })
}

/// Aplica los fixups de un registro FILE o un bloque INDX (los dos últimos bytes de cada sector de
/// 512 vuelven a su valor). `min` es el final de la cabecera fija.
fn fixup(b: &mut [u8], min: usize) -> Result<(), FsError> {
    let (off, cnt) = (u16le(b, 4) as usize, u16le(b, 6) as usize);
    if b.len() % 512 != 0 || off % 2 != 0 || off < min || cnt != b.len() / 512 + 1 || off + cnt * 2 > b.len() {
        return Err(FsError::Corrupt);
    }
    let usn = [b[off], b[off + 1]];
    for i in 1..cnt {
        let e = i * 512;
        if b[e - 2..e] != usn {
            return Err(FsError::Corrupt);
        }
        (b[e - 2], b[e - 1]) = (b[off + 2 * i], b[off + 2 * i + 1]);
    }
    Ok(())
}

/// Valida un registro FILE recién leído (con sus fixups ya aplicados). Un registro libre o a ceros
/// es `NotFound` (referencia obsoleta).
fn check_rec(b: &mut [u8]) -> Result<(), FsError> {
    let bad = FsError::Corrupt;
    if b.len() < 512 {
        return Err(bad);
    }
    if b[..4] == [0; 4] {
        return Err(FsError::NotFound);
    }
    if &b[..4] != b"FILE" {
        return Err(bad);
    }
    fixup(b, 0x2A)?;
    let (used, alloc, first) = (u32le(b, 0x18) as usize, u32le(b, 0x1C) as usize, u16le(b, 0x14) as usize);
    let mend = u16le(b, 4) as usize + 2 * u16le(b, 6) as usize;
    if alloc != b.len() || used > alloc || used % 8 != 0 || first % 8 != 0 || first < mend || first + 8 > used {
        return Err(bad);
    }
    if u16le(b, 0x16) & 1 == 0 {
        return Err(FsError::NotFound);
    }
    Ok(())
}

/// Entero little-endian de hasta 8 bytes.
fn le(b: &[u8]) -> u64 {
    b.iter().rev().fold(0, |a, &x| a << 8 | x as u64)
}

/// Decodifica una runlist que empieza en el VCN `vcn0`. Cada tramo real debe caber en `total` clusters.
fn runs(d: &[u8], vcn0: u64, total: u64) -> Result<Vec<Run>, FsError> {
    let bad = || FsError::Corrupt;
    let (mut out, mut p, mut vcn, mut lcn) = (Vec::new(), 0usize, vcn0, 0i64);
    while p < d.len() && d[p] != 0 {
        let (n, m) = ((d[p] & 15) as usize, (d[p] >> 4) as usize);
        if n == 0 || n > 8 || m > 8 || p + 1 + n + m > d.len() {
            return Err(bad());
        }
        let len = le(&d[p + 1..p + 1 + n]);
        if len == 0 || out.len() >= MAXRUNS {
            return Err(bad());
        }
        let real = if m == 0 {
            None
        } else {
            // Desplazamiento con signo, relativo al último tramo real.
            let sh = 64 - 8 * m as u32;
            let delta = ((le(&d[p + 1 + n..p + 1 + n + m]) << sh) as i64) >> sh;
            lcn = lcn.checked_add(delta).filter(|&l| l >= 0).ok_or_else(bad)?;
            if (lcn as u64).checked_add(len).map_or(true, |e| e > total) {
                return Err(bad());
            }
            Some(lcn as u64)
        };
        out.push(Run { vcn, len, lcn: real });
        vcn = vcn.checked_add(len).ok_or_else(bad)?;
        p += 1 + n + m;
    }
    Ok(out)
}

/// Recorre los atributos de un registro ya validado, uno por uno, con sus tamaños comprobados.
fn parse_attrs(r: &[u8], total: u64) -> Result<Vec<Part>, FsError> {
    let bad = || FsError::Corrupt;
    let used = u32le(r, 0x18) as usize;
    let (mut p, mut last, mut out) = (u16le(r, 0x14) as usize, 0u32, Vec::new());
    loop {
        if p + 8 > used {
            return Err(bad());
        }
        let ty = u32le(r, p);
        if ty == u32::MAX {
            return Ok(out);
        }
        let len = u32le(r, p + 4) as usize;
        if ty == 0 || ty % 16 != 0 || ty > 0x100 || ty < last || len < 0x18 || len % 8 != 0 || len > used - p {
            return Err(bad());
        }
        last = ty;
        let a = &r[p..p + len];
        let (nl, no) = (a[9] as usize, u16le(a, 0xA) as usize);
        if no + 2 * nl > len {
            return Err(bad());
        }
        let mut q = Part { ty, name: u16s(&a[no..no + 2 * nl]), id: u16le(a, 0xE), flags: u16le(a, 0xC), vcn0: 0, vcn1: 0, size: 0, init: 0, unit: 0, val: None, runs: Vec::new() };
        match a[8] {
            0 => {
                let (vl, vo) = (u32le(a, 0x10) as usize, u16le(a, 0x14) as usize);
                if vo.checked_add(vl).map_or(true, |e| e > len) {
                    return Err(bad());
                }
                q.val = Some(a[vo..vo + vl].to_vec());
            }
            1 => {
                if len < 0x40 {
                    return Err(bad());
                }
                let ro = u16le(a, 0x20) as usize;
                if ro < 0x40 || ro > len {
                    return Err(bad());
                }
                (q.vcn0, q.vcn1, q.unit) = (u64le(a, 0x10), u64le(a, 0x18), u16le(a, 0x22));
                q.size = u64le(a, 0x30);
                q.init = u64le(a, 0x38).min(q.size);
                if q.flags & 1 != 0 && q.unit > 6 {
                    return Err(bad());
                }
                // La suma de los tramos tiene que ser VCN final - VCN inicial + 1 (-1 con 0: vacío).
                let want = if q.vcn1 == u64::MAX && q.vcn0 == 0 {
                    0
                } else if q.vcn1 >= q.vcn0 {
                    q.vcn1 - q.vcn0 + 1
                } else {
                    return Err(bad());
                };
                q.runs = runs(&a[ro..], q.vcn0, total)?;
                if q.runs.last().map_or(0, |l| l.vcn + l.len - q.vcn0) != want {
                    return Err(bad());
                }
            }
            _ => return Err(bad()),
        }
        out.push(q);
        p += len;
    }
}

/// Junta los fragmentos (de la base y de las extensiones) en atributos lógicos: los no residentes se
/// ordenan por VCN, deben encadenar sin huecos y llevan los tamaños del primero.
fn merge(mut parts: Vec<Part>) -> Result<Vec<Attr>, FsError> {
    let bad = || FsError::Corrupt;
    parts.sort_by(|a, b| (a.ty, &a.name, a.vcn0).cmp(&(b.ty, &b.name, b.vcn0)));
    let (mut out, mut it) = (Vec::new(), parts.into_iter().peekable());
    while let Some(f) = it.next() {
        let same = |n: &Part| n.ty == f.ty && n.name == f.name;
        let val = match f.val {
            // Un residente repetido se ignora.
            Some(v) => {
                while it.peek().map_or(false, |n| same(n)) {
                    it.next();
                }
                Val::Res(v)
            }
            None => {
                if f.vcn0 != 0 {
                    return Err(bad());
                }
                let (mut rs, mut next) = (f.runs.clone(), f.vcn1.wrapping_add(1));
                while it.peek().map_or(false, |n| same(n)) {
                    let n = it.next().ok_or_else(bad)?;
                    if n.val.is_some() || n.vcn0 != next || rs.len() + n.runs.len() > MAXRUNS {
                        return Err(bad());
                    }
                    next = n.vcn1.wrapping_add(1);
                    rs.extend_from_slice(&n.runs);
                }
                Val::Non(Stream { runs: rs, size: f.size, init: f.init, unit: if f.flags & 1 != 0 { f.unit as u32 } else { 0 } })
            }
        };
        out.push(Attr { ty: f.ty, name: f.name, flags: f.flags, val });
    }
    Ok(out)
}

/// Tramo (con su LCN ya desplazado) que contiene el cluster virtual `vcn`: `Ok(None)` si no hay
/// ninguno que lo cubra y `max` es el VCN hasta donde sigue sin cobertura.
fn piece(runs: &[Run], vcn: u64) -> Option<(Option<u64>, u64)> {
    let r = runs.get(runs.partition_point(|r| r.vcn + r.len <= vcn))?;
    (r.vcn <= vcn).then(|| (r.lcn.map(|l| l + (vcn - r.vcn)), r.vcn + r.len))
}

/// Lee `[pos, pos+len)` de un flujo no residente, recortado al tamaño de datos. Los huecos y lo que
/// pasa del tamaño inicializado son ceros.
fn stream_read(io: &mut Io, cl: u64, s: &Stream, pos: u64, len: usize) -> Result<Vec<u8>, FsError> {
    let end = pos.checked_add(len as u64).ok_or(FsError::Corrupt)?.min(s.size);
    if pos >= end {
        return Ok(Vec::new());
    }
    let n = (end - pos) as usize;
    let mut out = Vec::new();
    out.try_reserve_exact(n).map_err(|_| FsError::NoSpace)?;
    out.resize(n, 0);
    if s.unit != 0 {
        return comp_read(io, cl, s, pos, end, out);
    }
    let lim = end.min(s.init);
    let mut p = pos;
    while p < lim {
        let (lcn, vend) = piece(&s.runs, p / cl).ok_or(FsError::Corrupt)?;
        let left = vend.checked_mul(cl).ok_or(FsError::Corrupt)? - p;
        let take = (lim - p).min(left).min(1 << 22);
        if let Some(l) = lcn {
            let off = l.checked_mul(cl).and_then(|o| o.checked_add(p % cl)).ok_or(FsError::Corrupt)?;
            let d = read_at(io, off, take as usize)?;
            out[(p - pos) as usize..][..take as usize].copy_from_slice(&d);
        }
        p += take;
    }
    Ok(out)
}

/// Flujo comprimido: se descomprime cada unidad que toca el rango.
fn comp_read(io: &mut Io, cl: u64, s: &Stream, pos: u64, end: u64, mut out: Vec<u8>) -> Result<Vec<u8>, FsError> {
    let uc = 1u64 << s.unit;
    let ub = uc.checked_mul(cl).filter(|&b| b <= MAXUNIT).ok_or(FsError::Corrupt)?;
    for u in pos / ub..=(end - 1) / ub {
        let unit = comp_unit(io, cl, s, u * uc, uc, ub as usize)?;
        let (a, b) = (pos.max(u * ub), end.min((u + 1) * ub));
        out[(a - pos) as usize..(b - pos) as usize].copy_from_slice(&unit[(a - u * ub) as usize..(b - u * ub) as usize]);
    }
    if s.init < end {
        let z = s.init.max(pos);
        out[(z - pos) as usize..].fill(0);
    }
    Ok(out)
}

/// Una unidad de compresión (`uc` clusters desde el VCN `v0`, `ub` bytes): hueco entero, sin
/// comprimir o comprimida (clusters al principio y hueco hasta el final).
fn comp_unit(io: &mut Io, cl: u64, s: &Stream, v0: u64, uc: u64, ub: usize) -> Result<Vec<u8>, FsError> {
    let (mut v, mut k, mut hole, mut buf) = (v0, 0u64, false, Vec::new());
    while v < v0 + uc {
        // Lo que no cubre ningún tramo cuenta como hueco.
        let (lcn, to) = match piece(&s.runs, v) {
            Some((l, e)) => (l, e.min(v0 + uc)),
            None => (None, s.runs.get(s.runs.partition_point(|r| r.vcn <= v)).map_or(u64::MAX, |r| r.vcn).min(v0 + uc)),
        };
        match lcn {
            Some(l) if !hole => {
                let n = to - v;
                buf.extend_from_slice(&read_at(io, l.checked_mul(cl).ok_or(FsError::Corrupt)?, (n * cl) as usize)?);
                k += n;
            }
            Some(_) => return Err(FsError::Corrupt),
            None => hole = true,
        }
        v = to;
    }
    if k == uc {
        return Ok(buf);
    }
    let mut out = if k == 0 { Vec::new() } else { lznt1::decode(&buf, ub)? };
    out.resize(ub, 0);
    Ok(out)
}

/// Entrada de un índice de carpeta ($FILE_NAME) ya resumida.
struct Ie {
    rec: u64,
    seq: u16,
    ns: u8,
    name: Vec<u16>,
}

/// Resume una entrada de índice; `None` si su clave no es un $FILE_NAME entero.
fn parse_ie(e: &[u8]) -> Option<Ie> {
    let kl = u16le(e, 10) as usize;
    if kl < 0x42 || 0x10 + kl > e.len() {
        return None;
    }
    let k = &e[0x10..0x10 + kl];
    let nl = k[0x40] as usize;
    if nl == 0 || 0x42 + 2 * nl > kl {
        return None;
    }
    let r = u64le(e, 0);
    Some(Ie { rec: r & MASK, seq: (r >> 48) as u16, ns: k[0x41], name: u16s(&k[0x42..0x42 + 2 * nl]) })
}

/// Recorre las entradas de un nodo de índice (cabecera de nodo en `hdr`) llamando a `f`; `Ok(false)`
/// si `f` pidió parar. Valida el nodo y cada entrada.
fn node(buf: &[u8], hdr: usize, f: &mut dyn FnMut(&[u8]) -> bool) -> Result<bool, FsError> {
    let bad = || FsError::Corrupt;
    if hdr + 16 > buf.len() {
        return Err(bad());
    }
    let (first, used, alloc) = (u32le(buf, hdr) as usize, u32le(buf, hdr + 4) as usize, u32le(buf, hdr + 8) as usize);
    if first % 8 != 0 || first.checked_add(16).map_or(true, |x| x > used) || used > alloc || hdr.checked_add(alloc).map_or(true, |x| x > buf.len()) {
        return Err(bad());
    }
    let (end, mut p) = (hdr + used, hdr + first);
    loop {
        if p + 16 > end {
            return Err(bad());
        }
        let (elen, kl, fl) = (u16le(buf, p + 8) as usize, u16le(buf, p + 10) as usize, u16le(buf, p + 12));
        if elen % 8 != 0 || elen < if fl & 1 != 0 { 24 } else { 16 } || p + elen > end || (fl & 2 == 0 && 0x10 + kl > elen) {
            return Err(bad());
        }
        if fl & 2 != 0 {
            return Ok(true);
        }
        if !f(&buf[p..p + elen]) {
            return Ok(false);
        }
        p += elen;
    }
}

/// ¿Es `n` un nombre que se puede mostrar y alcanzar por ruta?
fn name_ok(n: &[u16]) -> bool {
    n != [0x2E] && n != [0x2E, 0x2E] && !n.iter().any(|&c| c == 0 || c == 0x5C || c == 0x2F)
}

struct Ntfs {
    io: Io,
    cl: u64,
    rec: usize,
    clusters: u64,
    mft: Stream,
    nrec: u64,
    /// Tabla $UpCase (65536 entradas) o vacía si no se pudo cargar.
    up: Rc<Vec<u16>>,
    label: String,
    notice: Option<&'static str>,
    free: Option<u64>,
    cache: BTreeMap<u64, Rc<Vec<u8>>>,
}

impl Ntfs {
    /// Registro `n` de la MFT con los fixups aplicados y validado, con una caché pequeña.
    fn rec(&mut self, n: u64) -> Result<Rc<Vec<u8>>, FsError> {
        if let Some(r) = self.cache.get(&n) {
            return Ok(r.clone());
        }
        if n >= self.nrec {
            return Err(FsError::NotFound);
        }
        let mut b = stream_read(&mut self.io, self.cl, &self.mft, n * self.rec as u64, self.rec)?;
        if b.len() != self.rec {
            return Err(FsError::Corrupt);
        }
        check_rec(&mut b)?;
        if self.cache.len() >= CACHE {
            self.cache.clear();
        }
        let r = Rc::new(b);
        self.cache.insert(n, r.clone());
        Ok(r)
    }

    /// Contenido entero de un atributo (hasta `max` bytes).
    fn read_val(&mut self, a: &Attr, max: u64) -> Result<Vec<u8>, FsError> {
        match &a.val {
            Val::Res(v) => Ok(v.clone()),
            Val::Non(s) if s.size > max => Err(FsError::NoSpace),
            Val::Non(s) => stream_read(&mut self.io, self.cl, s, 0, s.size as usize),
        }
    }

    /// Carga el archivo `n` con todos sus atributos (también los repartidos por $ATTRIBUTE_LIST).
    /// Con `seq`, la referencia obsoleta es `NotFound`.
    fn file(&mut self, n: u64, seq: Option<u16>) -> Result<File, FsError> {
        let r = self.rec(n)?;
        let rseq = u16le(&r, 0x10);
        // Una extensión no se abre directamente.
        if u64le(&r, 0x20) != 0 || seq.map_or(false, |s| s != rseq) {
            return Err(FsError::NotFound);
        }
        let mut parts = parse_attrs(&r, self.clusters)?;
        let lst = parts.iter().find(|p| p.ty == 0x20 && p.name.is_empty()).map(|l| (l.val.clone(), Stream { runs: l.runs.clone(), size: l.size, init: l.init, unit: 0 }));
        if let Some((val, st)) = lst {
            let list = match val {
                Some(v) => v,
                None if st.size > MAXLIST => return Err(FsError::Corrupt),
                None => stream_read(&mut self.io, self.cl, &st, 0, st.size as usize)?,
            };
            self.extend(n, rseq, &list, &mut parts)?;
        }
        Ok(File { num: n, flags: u16le(&r, 0x16), attrs: merge(parts)? })
    }

    /// Añade a `parts` los fragmentos que la lista de atributos reparte en registros de extensión.
    fn extend(&mut self, n: u64, bseq: u16, list: &[u8], parts: &mut Vec<Part>) -> Result<(), FsError> {
        let bad = || FsError::Corrupt;
        let mut exts: BTreeMap<u64, Vec<Part>> = BTreeMap::new();
        let (mut p, mut count) = (0usize, 0usize);
        while p + 0x1A <= list.len() {
            count += 1;
            let len = u16le(list, p + 4) as usize;
            if count > 65536 || len < 0x1A || len % 8 != 0 || len > list.len() - p {
                return Err(bad());
            }
            let (ty, nl, no) = (u32le(list, p), list[p + 6] as usize, list[p + 7] as usize);
            if no + 2 * nl > len {
                return Err(bad());
            }
            let (vcn, rf, id) = (u64le(list, p + 8), u64le(list, p + 0x10), u16le(list, p + 0x18));
            let (rn, rs) = (rf & MASK, (rf >> 48) as u16);
            if rn != n {
                if !exts.contains_key(&rn) {
                    if exts.len() >= MAXEXT {
                        return Err(bad());
                    }
                    let er = self.rec(rn)?;
                    // Debe ser una extensión de este archivo y la referencia no estar obsoleta.
                    if u64le(&er, 0x20) != n | (bseq as u64) << 48 || u16le(&er, 0x10) != rs {
                        return Err(bad());
                    }
                    exts.insert(rn, parse_attrs(&er, self.clusters)?);
                }
                let name = u16s(&list[p + no..p + no + 2 * nl]);
                let q = exts[&rn].iter().find(|q| q.ty == ty && q.name == name && q.id == id && (q.val.is_some() || q.vcn0 == vcn));
                parts.push(q.ok_or_else(bad)?.clone());
            }
            p += len;
        }
        Ok(())
    }

    /// Recorre las entradas de una carpeta (todos los bloques INDX asignados, una vez cada uno),
    /// sin los metadatos de la raíz, sin la entrada de la propia carpeta y sin nombres imposibles.
    fn each(&mut self, d: &File, f: &mut dyn FnMut(Ie) -> bool) -> Result<(), FsError> {
        let bad = || FsError::Corrupt;
        let Some(Attr { val: Val::Res(root), .. }) = d.attr(0x90, &I30) else { return Err(bad()) };
        if root.len() < 0x20 || u32le(root, 0) != 0x30 || u32le(root, 4) != 1 {
            return Err(bad());
        }
        let bs = u32le(root, 8) as u64;
        if !bs.is_power_of_two() || !(512..=65536).contains(&bs) {
            return Err(bad());
        }
        let num = d.num;
        let mut cb = |e: &[u8]| match parse_ie(e) {
            Some(ie) if ie.rec != num && !(num == ROOT && ie.rec < 24) && name_ok(&ie.name) => f(ie),
            _ => true,
        };
        if !node(root, 0x10, &mut cb)? || root[0x1C] & 1 == 0 {
            return Ok(());
        }
        let Some(Attr { val: Val::Non(al), .. }) = d.attr(0xA0, &I30) else { return Err(bad()) };
        if al.size > MAXDIR || al.unit != 0 {
            return Err(FsError::NoSpace);
        }
        let bm = match d.attr(0xB0, &I30) {
            Some(a) => self.read_val(a, 1 << 20)?,
            None => return Err(bad()),
        };
        for b in 0..al.size / bs {
            if bm.get((b / 8) as usize).map_or(true, |x| x >> (b % 8) & 1 == 0) {
                continue;
            }
            let mut blk = stream_read(&mut self.io, self.cl, al, b * bs, bs as usize)?;
            if blk.len() != bs as usize || &blk[..4] != b"INDX" {
                return Err(bad());
            }
            fixup(&mut blk, 0x28)?;
            // El VCN cuenta clusters, o unidades de 512 bytes si el cluster es mayor que el bloque.
            if u64le(&blk, 0x10) != b * bs / if self.cl <= bs { self.cl } else { 512 } {
                return Err(bad());
            }
            if !node(&blk, 0x18, &mut cb)? {
                break;
            }
        }
        Ok(())
    }

    /// Mayúsculas de una unidad UTF-16: la tabla $UpCase del volumen o, sin ella, la de Unicode.
    fn upcase(up: &[u16], c: u16) -> u16 {
        match up.get(c as usize) {
            Some(&u) => u,
            None => char::from_u32(c as u32).map(|ch| upper(ch) as u32).filter(|&u| u <= 0xFFFF).map_or(c, |u| u as u16),
        }
    }

    /// Resuelve una ruta a su archivo y al nombre tal como está en disco. No entra en reanálisis.
    fn resolve(&mut self, path: &str) -> Result<(File, String), FsError> {
        let comps = split(path);
        if path.len() > 4096 || comps.len() > MAXCOMP {
            return Err(FsError::BadPath);
        }
        let up = self.up.clone();
        let (mut cur, mut name) = (self.file(ROOT, None)?, String::new());
        for c in comps {
            if c == "." || c == ".." {
                return Err(FsError::BadPath);
            }
            if !cur.is_dir() {
                return Err(FsError::NotDir);
            }
            if cur.reparse() {
                return Err(FsError::Denied);
            }
            let want: Vec<u16> = c.encode_utf16().collect();
            let mut hit = None;
            self.each(&cur, &mut |e| {
                // Los nombres POSIX distinguen mayúsculas; el resto no.
                let eq = e.name.len() == want.len() && e.name.iter().zip(&want).all(|(&a, &b)| if e.ns == 0 { a == b } else { Self::upcase(&up, a) == Self::upcase(&up, b) });
                if eq {
                    hit = Some(e);
                }
                !eq
            })?;
            let e = hit.ok_or(FsError::NotFound)?;
            name = show(&e.name);
            cur = self.file(e.rec, Some(e.seq))?;
        }
        Ok((cur, name))
    }

    fn entry(f: &File, name: String) -> Entry {
        let (t, fl) = f.si();
        Entry { name, dir: f.is_dir(), size: if f.is_dir() { 0 } else { f.size() }, modified: date(t), hidden: fl & 6 != 0 || f.num < 24 }
    }

    /// Clusters libres según $Bitmap (0 si no se puede leer): se cuenta una vez, la primera vez.
    fn count_free(&mut self) -> u64 {
        let Ok(f) = self.file(6, None) else { return 0 };
        let Some(Attr { val: Val::Non(s), .. }) = f.data() else { return 0 };
        let need = self.clusters.div_ceil(8);
        if s.size < need {
            return 0;
        }
        let (mut free, mut pos) = (0u64, 0u64);
        while pos < need {
            let n = (need - pos).min(1 << 20) as usize;
            match stream_read(&mut self.io, self.cl, s, pos, n) {
                Ok(b) if b.len() == n => {
                    for (i, &x) in b.iter().enumerate() {
                        // En el último byte solo cuentan los bits de clusters que existen.
                        let valid = (self.clusters - (pos + i as u64) * 8).min(8) as u32;
                        free += (!x & (0xFFu16 >> (8 - valid)) as u8).count_ones() as u64;
                    }
                }
                _ => return 0,
            }
            pos += n as u64;
        }
        free
    }

    /// Lee la MFT a partir de su registro 0 en el cluster `lcn` (el principal o su espejo).
    fn boot_mft(&mut self, lcn: u64) -> Result<(), FsError> {
        let off = lcn.checked_mul(self.cl).ok_or(FsError::Corrupt)?;
        let mut b = read_at(&mut self.io, off, self.rec)?;
        check_rec(&mut b)?;
        let d = parse_attrs(&b, self.clusters)?.into_iter().find(|p| p.ty == 0x80 && p.name.is_empty() && p.val.is_none() && p.vcn0 == 0).ok_or(FsError::Corrupt)?;
        // Primer fragmento: basta para localizar las extensiones de la propia MFT.
        self.mft = Stream { runs: d.runs, size: d.size, init: d.init, unit: 0 };
        self.nrec = d.size / self.rec as u64;
        self.cache.clear();
        self.cache.insert(0, Rc::new(b));
        let f = self.file(0, None)?;
        let Some(Attr { val: Val::Non(s), .. }) = f.data() else { return Err(FsError::Corrupt) };
        let nrec = s.size / self.rec as u64;
        if nrec == 0 || nrec > 1 << 32 {
            return Err(FsError::Corrupt);
        }
        (self.mft, self.nrec) = (s.clone(), nrec);
        Ok(())
    }

    /// ¿Dice el $LogFile que el volumen no se cerró limpio? Manda la página de reinicio más reciente.
    fn log_unclean(&mut self) -> bool {
        let Ok(f) = self.file(2, None) else { return false };
        let Some(Attr { val: Val::Non(s), .. }) = f.data() else { return false };
        let Ok(b) = stream_read(&mut self.io, self.cl, s, 0, 8192) else { return false };
        let mut best: Option<(u64, bool)> = None;
        for pg in b.chunks(4096).filter(|p| p.len() >= 0x40 && (&p[..4] == b"RSTR" || &p[..4] == b"CHKD")) {
            let ra = u16le(pg, 0x18) as usize;
            if ra + 0x10 > pg.len() {
                continue;
            }
            let (lsn, clean) = (u64le(pg, ra), u16le(pg, ra + 0xE) & 2 != 0);
            if best.map_or(true, |(l, _)| lsn > l) {
                best = Some((lsn, clean));
            }
        }
        best.map_or(false, |(_, clean)| !clean)
    }

    /// ¿Está hibernado el sistema? (`hiberfil.sys` empieza por "hibr"/"HIBR").
    fn hibernated(&mut self) -> bool {
        let Ok((f, _)) = self.resolve("hiberfil.sys") else { return false };
        let Some(Attr { val: Val::Non(s), .. }) = f.data() else { return false };
        matches!(stream_read(&mut self.io, self.cl, s, 0, 4).as_deref(), Ok(b"hibr") | Ok(b"HIBR"))
    }
}

impl Volume for Ntfs {
    fn kind(&self) -> FsKind {
        FsKind::Ntfs
    }
    fn label(&self) -> String {
        self.label.clone()
    }
    fn space(&mut self) -> (u64, u64) {
        let total = self.clusters.saturating_mul(self.cl);
        let free = match self.free {
            Some(f) => f,
            None => {
                let f = self.count_free();
                self.free = Some(f);
                f
            }
        };
        (total, free.saturating_mul(self.cl).min(total))
    }
    fn read_only(&self) -> bool {
        true
    }
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, FsError> {
        let (d, _) = self.resolve(dir)?;
        if !d.is_dir() {
            return Err(FsError::NotDir);
        }
        if d.reparse() {
            return Err(FsError::Denied);
        }
        let (mut v, mut full) = (Vec::new(), false);
        self.each(&d, &mut |e| {
            full = v.len() >= MAXENT;
            if !full {
                v.push(e);
            }
            !full
        })?;
        if full {
            return Err(FsError::NoSpace);
        }
        // Un nombre corto DOS sobra si la misma carpeta ya tiene el largo del mismo archivo.
        let long: BTreeSet<u64> = v.iter().filter(|e| e.ns != 2).map(|e| e.rec).collect();
        v.retain(|e| e.ns != 2 || !long.contains(&e.rec));
        // Una entrada con el registro dañado u obsoleto se salta: no tumba la carpeta.
        Ok(v.into_iter().filter_map(|e| self.file(e.rec, Some(e.seq)).ok().map(|f| Self::entry(&f, show(&e.name)))).collect())
    }
    fn stat(&mut self, path: &str) -> Result<Entry, FsError> {
        let (f, name) = self.resolve(path)?;
        Ok(Self::entry(&f, name))
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        let (f, _) = self.resolve(path)?;
        if f.is_dir() {
            return Err(FsError::IsDir);
        }
        if f.reparse() || f.encrypted() {
            return Err(FsError::Denied);
        }
        match f.data() {
            None => Ok(Vec::new()),
            Some(a) => self.read_val(a, MAXFILE),
        }
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
        self.notice
    }
}

/// Monta la partición `p` de `dev`. `Err(FsError::Corrupt)` si no es válida.
pub fn mount(dev: Box<dyn SectorDev>, p: &Partition) -> Result<Box<dyn Volume>, FsError> {
    let ss = dev.sector_size();
    let mut io = Io { dev, start: p.start, len: p.sectors, ss };
    let pbytes = p.sectors.checked_mul(ss as u64).ok_or(FsError::Corrupt)?;
    let mut warn = None;
    let g = match read_at(&mut io, 0, 512).and_then(|s| parse_boot(&s, pbytes)) {
        Ok(g) => g,
        Err(_) => {
            // La copia del arranque está en el último sector.
            warn = Some(MSG_COPIA);
            let s = read_at(&mut io, pbytes.checked_sub(ss as u64).ok_or(FsError::Corrupt)?, 512)?;
            parse_boot(&s, pbytes)?
        }
    };
    let mut n = Ntfs { io, cl: g.cl, rec: g.rec, clusters: g.clusters, mft: Stream::default(), nrec: 0, up: Rc::new(Vec::new()), label: String::new(), notice: warn, free: None, cache: BTreeMap::new() };
    // La MFT: la principal y, si falla, su espejo.
    if n.boot_mft(g.mft).is_err() {
        n.boot_mft(g.mirr).map_err(|_| FsError::Corrupt)?;
        n.notice = Some(MSG_COPIA);
    }
    // La raíz tiene que ser una carpeta legible.
    match n.file(ROOT, None) {
        Ok(r) if r.is_dir() && r.attr(0x90, &I30).is_some() => {}
        _ => return Err(FsError::Corrupt),
    }
    // Lo demás (mayúsculas, etiqueta, estado) es opcional: si falta, se sigue sin ello.
    if let Ok(f) = n.file(10, None) {
        if let Some(Ok(b)) = f.data().map(|a| n.read_val(a, 1 << 17)) {
            if b.len() == 1 << 17 {
                n.up = Rc::new(u16s(&b));
            }
        }
    }
    let mut dirty = false;
    if let Ok(f) = n.file(3, None) {
        if let Some(Attr { val: Val::Res(v), .. }) = f.attr(0x60, &[]) {
            n.label = show(&u16s(&v[..v.len().min(256)]));
        }
        if let Some(Attr { val: Val::Res(v), .. }) = f.attr(0x70, &[]) {
            dirty = v.len() >= 0xC && u16le(v, 0xA) & 1 != 0;
        }
    }
    if dirty || n.log_unclean() || n.hibernated() {
        n.notice = Some(MSG_SUCIO);
    }
    Ok(Box::new(n))
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

    #[derive(Default, Clone, Copy)]
    struct Opt {
        vol_flags: u16,
        log_dirty: bool,
        hiber: bool,
        upcase: bool,
    }

    /// Contenido esperado de los archivos grandes de la imagen.
    fn big() -> Vec<u8> {
        [vec![0x11; CL], vec![0x22; CL], vec![0; CL], vec![0x44; CL - 100]].concat()
    }
    fn comp() -> Vec<u8> {
        let mut e = vec![0x20u8; 4096];
        e.extend(b"ABC");
        e.resize(65536 * 2, 0);
        for c in 0..16 {
            e.extend(vec![c as u8 + 1; CL]);
        }
        e.truncate(3 * 65536 - 1000);
        e
    }
    fn lista() -> Vec<u8> {
        let mut e: Vec<u8> = (0..6).flat_map(|k| vec![0x61 + k as u8; CL]).collect();
        e.truncate(6 * CL - 10);
        e
    }

    /// Imagen de 1 MiB: sector 512, cluster 4096, registro 1024, 255 clusters (los 2047 sectores
    /// del arranque), MFT de 64 registros en el LCN 4.
    fn build(o: Opt) -> Vec<u8> {
        let mut d = vec![0u8; 256 * CL];
        // Arranque y su copia en el último sector.
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
        // Registros de sistema.
        put_rec(&mut d, 0, 1, 0, &[non(0x80, "", 1, 0, 15, &rl(&[(16, Some(4))]), 64 * 1024, 0, 0)]);
        put_rec(&mut d, 1, 1, 0, &[non(0x80, "", 1, 0, 0, &rl(&[(1, Some(2))]), 4096, 0, 0)]);
        put_rec(&mut d, 2, 1, 0, &[non(0x80, "", 1, 0, 1, &rl(&[(2, Some(20))]), 8192, 0, 0)]);
        let mut vi = vec![0u8; 12];
        (vi[8], vi[9]) = (3, 1);
        w16(&mut vi, 0xA, o.vol_flags);
        put_rec(&mut d, 3, 1, 0, &[res(0x60, "", 1, &u16z("Etiqueta ñ")), res(0x70, "", 2, &vi)]);
        put_rec(&mut d, 6, 1, 0, &[non(0x80, "", 1, 0, 0, &rl(&[(1, Some(3))]), 32, 0, 0)]);
        d[3 * CL..3 * CL + 3].fill(0xFF);
        if o.upcase {
            put_rec(&mut d, 10, 1, 0, &[non(0x80, "", 1, 0, 31, &rl(&[(32, Some(100))]), 131072, 0, 0)]);
            for c in 0..65536usize {
                let u = if (0x61..=0x7A).contains(&c) { c - 32 } else { c };
                w16(&mut d, 100 * CL + 2 * c, u as u16);
            }
        }
        // $LogFile: dos páginas de reinicio; manda la de LSN mayor (la segunda).
        for (pg, lsn, flags) in [(0usize, 5u64, if o.log_dirty { 2u16 } else { 0 }), (1, 7, if o.log_dirty { 0 } else { 2 })] {
            let at = 20 * CL + pg * 4096;
            d[at..at + 4].copy_from_slice(b"RSTR");
            w16(&mut d, at + 0x18, 0x30);
            w64(&mut d, at + 0x30, lsn);
            w16(&mut d, at + 0x3E, flags);
        }
        // Archivos de usuario.
        put_rec(&mut d, 24, 1, 0, &[si(0x20), res(0x80, "", 1, b"Hello NTFS")]);
        // Fragmentado, con un hueco y el último cluster a medias.
        put_rec(&mut d, 25, 1, 0, &[si(0x20), non(0x80, "", 1, 0, 3, &rl(&[(2, Some(24)), (1, None), (1, Some(28))]), 4 * CL as u64 - 100, 0x8000, 0)]);
        fill(&mut d, 24, 1, 0x11);
        fill(&mut d, 25, 1, 0x22);
        fill(&mut d, 28, 1, 0x44);
        // Tamaño inicializado menor que el de datos: el segundo cluster tiene datos pero lee ceros.
        let mut init = non(0x80, "", 1, 0, 1, &rl(&[(2, Some(30))]), 8192, 0, 0);
        w64(&mut init, 0x38, 4096);
        put_rec(&mut d, 26, 1, 0, &[si(0x20), init]);
        fill(&mut d, 30, 1, 0xAA);
        fill(&mut d, 31, 1, 0xFF);
        // Comprimido: unidad 0 con LZNT1, unidad 1 hueco, unidad 2 sin comprimir.
        let c = rl(&[(1, Some(32)), (15, None), (16, None), (16, Some(40))]);
        put_rec(&mut d, 27, 1, 0, &[si(0x820), non(0x80, "", 1, 0, 47, &c, 3 * 65536 - 1000, 1, 4)]);
        d[32 * CL..32 * CL + 12].copy_from_slice(&[0x03, 0xB0, 0x02, 0x20, 0xFC, 0x0F, 0x03, 0xB0, 0x00, 0x41, 0x42, 0x43]);
        for k in 0..16 {
            fill(&mut d, 40 + k, 1, k as u8 + 1);
        }
        // Atributo repartido en tres registros (28 base, 29 y 30 extensiones).
        let mut al = Vec::new();
        for (vcn, rec, id) in [(0u64, 28u64, 1u16), (2, 29, 2), (4, 30, 3)] {
            let mut e = vec![0u8; 0x20];
            w32(&mut e, 0, 0x80);
            w16(&mut e, 4, 0x20);
            e[7] = 0x1A;
            w64(&mut e, 8, vcn);
            w64(&mut e, 0x10, rec | 1 << 48);
            w16(&mut e, 0x18, id);
            al.extend(e);
        }
        put_rec(&mut d, 28, 1, 0, &[si(0x20), res(0x20, "", 5, &al), non(0x80, "", 1, 0, 1, &rl(&[(2, Some(60))]), 6 * CL as u64 - 10, 0, 0)]);
        put_rec(&mut d, 29, 1, 28 | 1 << 48, &[non(0x80, "", 2, 2, 3, &rl(&[(2, Some(62))]), 0, 0, 0)]);
        put_rec(&mut d, 30, 1, 28 | 1 << 48, &[non(0x80, "", 3, 4, 5, &rl(&[(2, Some(64))]), 0, 0, 0)]);
        for k in 0..6 {
            fill(&mut d, 60 + k, 1, 0x61 + k as u8);
        }
        // Ocultos, de sistema, cifrado, reanálisis.
        put_rec(&mut d, 31, 1, 0, &[si(0x22), res(0x80, "", 1, b"x")]);
        put_rec(&mut d, 32, 1, 0, &[si(0x24), res(0x80, "", 1, b"s")]);
        dir_rec(&mut d, 33, 0x06, &[last(None)]);
        put_rec(&mut d, 34, 1, 0, &[si(0x4020), res(0x80, "", 1, b"enc")]);
        put_rec(&mut d, 35, 3, 0, &[si(0x400), res(0x90, "$I30", 1, &iroot(&[last(None)], false)), reparse(0xA000_0003)]);
        put_rec(&mut d, 36, 1, 0, &[si(0x420), res(0x80, "", 1, b"x"), reparse(0xA000_000C)]);
        put_rec(&mut d, 37, 1, 0, &[si(0x20), res(0x80, "", 1, b"doc")]);
        put_rec(&mut d, 38, 1, 0, &[si(0x20), res(0x80, "", 1, b"dos")]);
        dir_rec(&mut d, 39, 0x10, &[en(24, "uno.txt", 3), en(31, "dos.txt", 3), last(None)]);
        put_rec(&mut d, 42, 1, 0, &[si(0x20), res(0x80, "", 1, b"n")]);
        // Carpeta con tres bloques INDX: el 2.º tiene un subnodo que se apunta a sí mismo y el 3.º
        // no está asignado (basura que no debe leerse).
        put_rec(&mut d, 40, 3, 0, &[si(0x10), res(0x90, "$I30", 1, &iroot(&[last(Some(0))], true)), non(0xA0, "$I30", 2, 0, 2, &rl(&[(3, Some(141))]), 3 * CL as u64, 0, 0), res(0xB0, "$I30", 3, &[0b011])]);
        let mut b0: Vec<Vec<u8>> = (0..40).map(|i| en(24, &format!("f{i:02}.txt"), 1)).collect();
        b0.push(last(None));
        d[141 * CL..142 * CL].copy_from_slice(&indx(0, &b0));
        d[142 * CL..143 * CL].copy_from_slice(&indx(1, &[ie(31, 1, "m.txt", 1, Some(1)), en(25, "z.txt", 1), last(None)]));
        fill(&mut d, 143, 1, 0xEE);
        // Raíz: metadatos (no salen), la propia raíz, un 8.3, una referencia obsoleta y un registro libre.
        let mut r = vec![en(0, "$MFT", 3), en(6, "$Bitmap", 3), en(11, "$Extend", 3), en(5, "raiz", 1)];
        r.extend([en(24, "hello.txt", 3), en(25, "big.bin", 3), en(26, "init.bin", 3), en(27, "comp.bin", 3), en(28, "attrlist.dat", 3)]);
        r.extend([en(31, "oculto.txt", 3), en(32, "sistema.sys", 3), en(33, "System Volume Information", 3), en(34, "secreto.txt", 3)]);
        r.extend([en(35, "union", 3), en(36, "simbolico.txt", 3), en(37, "Documento largo.txt", 1), en(37, "DOCUME~1.TXT", 2), en(38, "SOLO~1.TXT", 2)]);
        r.extend([en(39, "Pequena", 3), en(40, "Carpeta", 3), en(42, "ñandú.txt", 1), ie(24, 9, "viejo.txt", 3, None), en(50, "fantasma.txt", 3)]);
        if o.hiber {
            put_rec(&mut d, 41, 1, 0, &[si(0x6), non(0x80, "", 1, 0, 0, &rl(&[(1, Some(22))]), 4096, 0, 0)]);
            d[22 * CL..22 * CL + 4].copy_from_slice(b"hibr");
            r.push(en(41, "hiberfil.sys", 3));
        }
        r.push(last(None));
        d[140 * CL..141 * CL].copy_from_slice(&indx(0, &r));
        put_rec(&mut d, 5, 3, 0, &[si(0x1000_0000), res(0x90, "$I30", 1, &iroot(&[last(Some(0))], true)), non(0xA0, "$I30", 2, 0, 0, &rl(&[(1, Some(140))]), 4096, 0, 0), res(0xB0, "$I30", 3, &[1])]);
        // Espejo de los cuatro primeros registros.
        d.copy_within(4 * CL..5 * CL, 2 * CL);
        d
    }

    fn part() -> Partition {
        Partition { start: 0, sectors: 2048, kind: FsKind::Ntfs }
    }
    fn mnt(d: Vec<u8>) -> Result<Box<dyn Volume>, FsError> {
        mount(Box::new(MemSectors { data: d, sector: 512, ro: false }), &part())
    }
    fn names(v: &mut dyn Volume, dir: &str) -> Vec<String> {
        v.list(dir).unwrap().into_iter().map(|e| e.name).collect()
    }
    fn dt(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> DateTime {
        DateTime { year, month, day, hour, minute, second }
    }

    #[test]
    fn lznt1_vectores() {
        assert_eq!(lznt1::decode(&[0x03, 0xB0, 0x02, 0x20, 0xFC, 0x0F], 4096).unwrap(), vec![0x20; 4096]);
        assert_eq!(lznt1::decode(&[0x03, 0xB0, 0x00, 0x41, 0x42, 0x43], 16).unwrap(), b"ABC");
        assert_eq!(lznt1::decode(&[0x05, 0xB0, 0x08, 0x41, 0x42, 0x43, 0x03, 0x20], 16).unwrap(), b"ABCABCABC");
        // Cabecera 0: nada; trozo sin comprimir: los bytes tal cual.
        assert_eq!(lznt1::decode(&[0, 0, 1, 2, 3], 16).unwrap(), b"");
        assert_eq!(lznt1::decode(&[0x03, 0x30, 1, 2, 3, 4], 16).unwrap(), [1, 2, 3, 4]);
        // Desplazamiento mayor que lo ya producido, salida por encima de 4096, firma mala, trozo
        // que se sale del búfer, salida por encima del tope y referencia sin nada producido antes.
        assert!(lznt1::decode(&[0x03, 0xB0, 0x02, 0x41, 0x00, 0x20], 16).is_err());
        assert!(lznt1::decode(&[0x03, 0xB0, 0x02, 0x20, 0xFD, 0x0F], 8192).is_err());
        assert!(lznt1::decode(&[0x03, 0x80, 0x00, 0x41, 0x42, 0x43], 16).is_err());
        assert!(lznt1::decode(&[0x0F, 0xB0, 0x00, 0x41], 16).is_err());
        assert!(lznt1::decode(&[0x03, 0xB0, 0x00, 0x41, 0x42, 0x43], 2).is_err());
        assert!(lznt1::decode(&[0x03, 0xB0, 0x01, 0x00, 0x00, 0x00], 16).is_err());
    }

    #[test]
    fn runlists() {
        let r = runs(&[0x21, 0x18, 0x34, 0x56, 0x00], 0, 1 << 20).unwrap();
        assert_eq!((r.len(), r[0].len, r[0].lcn), (1, 0x18, Some(0x5634)));
        let r = runs(&[0x21, 0x10, 0x00, 0x10, 0x01, 0x20, 0x11, 0x08, 0xF0, 0x00], 5, 1 << 20).unwrap();
        let v: Vec<_> = r.iter().map(|x| (x.vcn, x.len, x.lcn)).collect();
        assert_eq!(v, [(5, 0x10, Some(0x1000)), (0x15, 0x20, None), (0x35, 8, Some(0xFF0))]);
        // Longitud 0, LCN negativo, tramo fuera del volumen, 0 bytes de longitud, más de 8 bytes de
        // desplazamiento, runlist truncada y longitud que no cabe.
        for bad in [&[0x11, 0x00, 0x05, 0][..], &[0x11, 0x01, 0xFF, 0], &[0x11, 0x10, 0x7F, 0], &[0x10, 0x01, 0], &[0x91, 0x01], &[0x21, 0x01], &[0x18, 1, 1, 1, 1, 1, 1, 1, 0xFF, 0]] {
            assert!(runs(bad, 0, 100).is_err(), "{bad:?}");
        }
        // Un hueco enorme es válido (archivo disperso), pero la suma de VCN no puede desbordar.
        let huge = [0x08, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
        assert!(runs(&huge, 0, 10).is_ok());
        assert!(runs(&huge, 1, 10).is_err());
    }

    #[test]
    fn fechas() {
        assert_eq!(date(FT), dt(2023, 11, 14, 22, 13, 20));
        assert_eq!(date(116_444_736_000_000_000), dt(1970, 1, 1, 0, 0, 0));
        assert_eq!(date(0), DateTime::default());
        assert!(date(u64::MAX).year > 50000);
    }

    #[test]
    fn arranque() {
        let mut b = [0u8; 512];
        b[3..11].copy_from_slice(b"NTFS    ");
        w16(&mut b, 0xB, 512);
        b[0xD] = 0xF4;
        w64(&mut b, 0x28, 1 << 20);
        w64(&mut b, 0x30, 4);
        w64(&mut b, 0x38, 2);
        b[0x40] = 0xF6;
        let g = parse_boot(&b, 1 << 30).unwrap();
        assert_eq!((g.cl, g.rec), (2 << 20, 1024));
        b[0x40] = 0xF4;
        assert_eq!(parse_boot(&b, 1 << 30).unwrap().rec, 4096);
        // Positivo = clusters por registro: 2 clusters de 512.
        (b[0xD], b[0x40]) = (1, 2);
        assert_eq!(parse_boot(&b, 1 << 30).unwrap().rec, 1024);
        // Dispositivo menor que el volumen: se recorta.
        assert_eq!(parse_boot(&b, 100 * 512).unwrap().clusters, 100);
        let fallos: [&dyn Fn(&mut [u8; 512]); 14] = [
            &|b| b[0xD] = 0xF3,
            &|b| b[0xD] = 0,
            &|b| b[0xD] = 3,
            &|b| w16(b, 0xB, 100),
            &|b| w16(b, 0xE, 1),
            &|b| w32(b, 0x20, 1),
            &|b| w64(b, 0x28, 0),
            &|b| w64(b, 0x38, 4),
            &|b| w64(b, 0x30, 0),
            &|b| w64(b, 0x30, u64::MAX),
            &|b| b[0x40] = 0,
            &|b| b[0x40] = 0xE0,
            &|b| b[0x40] = 0xF3,
            &|b| b[3] = b'X',
        ];
        for f in fallos {
            let mut c = b;
            f(&mut c);
            assert!(parse_boot(&c, 1 << 30).is_err());
        }
    }

    #[test]
    fn imagen_principal() {
        // `partitions` la reconoce y `vol::mount` la monta.
        // Dentro de un MBR (un NTFS sin tabla de particiones no lo reconoce `part::fs_boot`).
        let mut data = vec![0u8; 64 * 512];
        data[446 + 4] = 7;
        w32(&mut data, 446 + 8, 64);
        w32(&mut data, 446 + 12, 2048);
        data[510] = 0x55;
        data[511] = 0xAA;
        data.extend(build(Opt::default()));
        let mut m = MemSectors { data, sector: 512, ro: false };
        let ps = partitions(&mut m).unwrap();
        assert_eq!((ps[0].kind, ps[0].start), (FsKind::Ntfs, 64));
        let mut v = crate::vol::mount(Box::new(m), &ps[0]).unwrap();
        assert_eq!((v.kind(), v.label(), v.read_only(), v.notice()), (FsKind::Ntfs, "Etiqueta ñ".into(), true, None));
        assert_eq!(v.space(), (255 * 4096, 231 * 4096));
        // Sin metadatos de la raíz, ni la propia raíz, ni el 8.3 que acompaña a un largo, ni la
        // referencia obsoleta ni el registro libre; el 8.3 suelto sí sale.
        assert_eq!(
            names(&mut *v, ""),
            ["hello.txt", "big.bin", "init.bin", "comp.bin", "attrlist.dat", "oculto.txt", "sistema.sys", "System Volume Information", "secreto.txt", "union", "simbolico.txt", "Documento largo.txt", "SOLO~1.TXT", "Pequena", "Carpeta", "ñandú.txt"]
        );
        let es = v.list("\\").unwrap();
        let get = |n: &str| es.iter().find(|e| e.name == n).unwrap().clone();
        let h = get("hello.txt");
        assert_eq!((h.dir, h.size, h.hidden, h.modified), (false, 10, false, dt(2023, 11, 14, 22, 13, 20)));
        for n in ["oculto.txt", "sistema.sys", "System Volume Information"] {
            assert!(get(n).hidden, "{n}");
        }
        assert!(get("Carpeta").dir && !get("Carpeta").hidden && get("Carpeta").size == 0);
        // El tamaño es el del registro, no el del índice (que aquí es 0).
        assert_eq!((get("big.bin").size, get("comp.bin").size), (4 * 4096 - 100, 3 * 65536 - 1000));
        // Lectura: residente, fragmentado con hueco, inicializado corto, comprimido, repartido.
        assert_eq!(v.read("hello.txt").unwrap(), b"Hello NTFS");
        assert_eq!(v.read("HELLO.TXT").unwrap(), b"Hello NTFS");
        assert_eq!(v.read("big.bin").unwrap(), big());
        assert_eq!(v.read("init.bin").unwrap(), [vec![0xAA; 4096], vec![0; 4096]].concat());
        assert!(v.read("comp.bin").unwrap() == comp());
        assert!(v.read("attrlist.dat").unwrap() == lista());
        assert_eq!(v.read("Documento largo.txt").unwrap(), b"doc");
        assert_eq!(v.read("DOCUME~1.TXT").unwrap(), b"doc");
        // Carpetas: solo índice raíz, y con tres bloques (cada nombre una vez, sin leer el libre).
        assert_eq!(names(&mut *v, "Pequena"), ["uno.txt", "dos.txt"]);
        assert_eq!(v.read("Pequena\\UNO.txt").unwrap(), b"Hello NTFS");
        let c = names(&mut *v, "Carpeta");
        assert_eq!((c.len(), c[0].as_str(), c[39].as_str(), c[40].as_str(), c[41].as_str()), (42, "f00.txt", "f39.txt", "m.txt", "z.txt"));
        assert_eq!(v.stat("Carpeta\\z.txt").unwrap().size, 4 * 4096 - 100);
        assert!(v.stat("").unwrap().dir);
        // Cifrado y reanálisis: se ven, pero no se leen ni se entra.
        assert_eq!(v.stat("secreto.txt").unwrap().size, 3);
        assert_eq!(v.read("secreto.txt"), Err(FsError::Denied));
        assert_eq!(v.read("simbolico.txt"), Err(FsError::Denied));
        assert!(v.stat("union").unwrap().dir);
        assert_eq!(v.list("union").map(|_| ()), Err(FsError::Denied));
        assert_eq!(v.read("union\\x"), Err(FsError::Denied));
        assert_eq!(v.list("System Volume Information").unwrap(), []);
        // Errores de ruta.
        assert_eq!(v.read("nada"), Err(FsError::NotFound));
        assert_eq!(v.read("$MFT"), Err(FsError::NotFound));
        assert_eq!(v.read("viejo.txt"), Err(FsError::NotFound));
        assert_eq!(v.list("hello.txt").map(|_| ()), Err(FsError::NotDir));
        assert_eq!(v.read("hello.txt\\x"), Err(FsError::NotDir));
        assert_eq!(v.read("Carpeta"), Err(FsError::IsDir));
        assert_eq!(v.read("Carpeta\\..\\hello.txt"), Err(FsError::BadPath));
        // Solo lectura.
        let now = dt(2026, 1, 1, 0, 0, 0);
        assert_eq!(v.write("x", b"x", now), Err(FsError::Denied));
        assert_eq!(v.mkdir("x", now), Err(FsError::Denied));
        assert_eq!(v.rename("hello.txt", "y"), Err(FsError::Denied));
        assert_eq!(v.delete("hello.txt"), Err(FsError::Denied));
        assert_eq!(v.flush(), Ok(()));
    }

    #[test]
    fn mayusculas_del_volumen() {
        // Sin $UpCase vale la conversión de Unicode (ñ = Ñ); con la tabla de la prueba, que solo
        // cambia ASCII, no.
        assert_eq!(mnt(build(Opt::default())).unwrap().read("ÑANDÚ.TXT").unwrap(), b"n");
        let mut v = mnt(build(Opt { upcase: true, ..Opt::default() })).unwrap();
        assert_eq!(v.read("ÑANDÚ.TXT"), Err(FsError::NotFound));
        assert_eq!(v.read("ñandú.txt").unwrap(), b"n");
        assert_eq!(v.read("HELLO.TXT").unwrap(), b"Hello NTFS");
        // Una tabla de tamaño distinto no se usa.
        let mut d = build(Opt { upcase: true, ..Opt::default() });
        w64(&mut d, rec_at(10) + 0x38 + 0x30, 131071);
        assert_eq!(mnt(d).unwrap().read("ÑANDÚ.TXT").unwrap(), b"n");
    }

    #[test]
    fn estado_del_volumen() {
        let sucio = Some(MSG_SUCIO);
        assert_eq!(mnt(build(Opt { vol_flags: 1, ..Opt::default() })).unwrap().notice(), sucio);
        assert_eq!(mnt(build(Opt { vol_flags: 0x8000, ..Opt::default() })).unwrap().notice(), None);
        assert_eq!(mnt(build(Opt { log_dirty: true, ..Opt::default() })).unwrap().notice(), sucio);
        let mut v = mnt(build(Opt { hiber: true, ..Opt::default() })).unwrap();
        assert_eq!(v.notice(), sucio);
        assert!(v.list("").unwrap().iter().any(|e| e.name == "hiberfil.sys" && e.hidden));
        // Un $LogFile todo a 0xFF cuenta como limpio.
        let mut d = build(Opt { log_dirty: true, ..Opt::default() });
        d[20 * CL..22 * CL].fill(0xFF);
        assert_eq!(mnt(d).unwrap().notice(), None);
    }

    #[test]
    fn copias_de_seguridad() {
        // Arranque dañado: se usa la copia del último sector.
        let mut d = build(Opt::default());
        d[3] = b'X';
        let mut v = mnt(d).unwrap();
        assert_eq!(v.notice(), Some(MSG_COPIA));
        assert_eq!(v.read("hello.txt").unwrap(), b"Hello NTFS");
        // Las dos copias dañadas, o campos reservados distintos de cero: no monta.
        let mut d = build(Opt::default());
        d[3] = b'X';
        d[2047 * 512 + 3] = b'X';
        assert!(mnt(d).is_err());
        let mut d = build(Opt::default());
        w16(&mut d, 0xE, 1);
        w16(&mut d, 2047 * 512 + 0xE, 1);
        assert!(mnt(d).is_err());
        // Registro 0 de la MFT dañado: se carga desde el espejo.
        let mut d = build(Opt::default());
        d[rec_at(0)] = b'X';
        let mut v = mnt(d).unwrap();
        assert_eq!(v.notice(), Some(MSG_COPIA));
        assert_eq!(v.read("hello.txt").unwrap(), b"Hello NTFS");
        // Si el espejo también está roto, no monta; y un disco a ceros tampoco.
        let mut d = build(Opt::default());
        d[rec_at(0)] = b'X';
        d[2 * CL] = b'X';
        assert!(mnt(d).is_err());
        assert!(mnt(vec![0; 256 * CL]).is_err());
    }

    #[test]
    fn registros_y_carpetas_danados() {
        // Fixup roto en un registro: solo ese archivo desaparece.
        let mut d = build(Opt::default());
        d[rec_at(24) + 510] = 0x77;
        let mut v = mnt(d).unwrap();
        assert!(!names(&mut *v, "").contains(&"hello.txt".to_string()));
        assert_eq!(v.read("hello.txt"), Err(FsError::Corrupt));
        assert_eq!(v.read("big.bin").unwrap(), big());
        // Longitud de atributo 0 (bucle), no múltiplo de 8 o que pasa de lo usado.
        for l in [0u32, 0x61, 0x10000] {
            let mut d = build(Opt::default());
            w32(&mut d, rec_at(24) + 0x3C, l);
            let mut v = mnt(d).unwrap();
            assert_eq!(v.read("hello.txt"), Err(FsError::Corrupt));
            assert_eq!(v.read("big.bin").unwrap(), big());
        }
        // Registro libre y secuencia distinta: la entrada se ignora.
        let mut d = build(Opt::default());
        w16(&mut d, rec_at(25) + 0x16, 0);
        w16(&mut d, rec_at(26) + 0x10, 2);
        let mut v = mnt(d).unwrap();
        assert_eq!((v.read("big.bin"), v.read("init.bin")), (Err(FsError::NotFound), Err(FsError::NotFound)));
        assert!(!names(&mut *v, "").contains(&"big.bin".to_string()));
        // Runlist con LCN negativo: error al leer, no al listar.
        let mut d = build(Opt::default());
        let at = d[rec_at(25)..rec_at(25) + 1024].windows(3).position(|w| w == [0x11, 0x02, 0x18]).unwrap();
        d[rec_at(25) + at + 2] = 0xFF;
        let mut v = mnt(d).unwrap();
        assert_eq!(v.read("big.bin"), Err(FsError::Corrupt));
        assert!(names(&mut *v, "").contains(&"attrlist.dat".to_string()));
        // Extensión de otro archivo (referencia a la base): el archivo da error, lo demás sigue.
        let mut d = build(Opt::default());
        w64(&mut d, rec_at(29) + 0x20, 24 | 1 << 48);
        let mut v = mnt(d).unwrap();
        assert_eq!(v.read("attrlist.dat"), Err(FsError::Corrupt));
        assert_eq!(v.read("big.bin").unwrap(), big());
        // Bloque INDX con VCN distinto, firma rota o nodo mal: la carpeta da error, la raíz sigue.
        let fallos: [&dyn Fn(&mut Vec<u8>); 6] = [
            &|d| w64(d, 142 * CL + 0x10, 7),
            &|d| d[142 * CL] = b'X',
            &|d| w32(d, 142 * CL + 0x1C, 0x7000),
            &|d| w32(d, 142 * CL + 0x18, 5),
            &|d| w16(d, 142 * CL + 0x40 + 8, 0),
            &|d| d[142 * CL + 510] = 9,
        ];
        for f in fallos {
            let mut d = build(Opt::default());
            f(&mut d);
            let mut v = mnt(d).unwrap();
            assert_eq!(v.list("Carpeta").map(|_| ()), Err(FsError::Corrupt));
            assert_eq!(v.read("hello.txt").unwrap(), b"Hello NTFS");
        }
        // Compresión con LZNT1 dañado: error de lectura, no pánico.
        let mut d = build(Opt::default());
        d[32 * CL + 6] = 0x03;
        d[32 * CL + 7] = 0x80;
        assert_eq!(mnt(d).unwrap().read("comp.bin"), Err(FsError::Corrupt));
    }

    #[test]
    fn unidad_de_compresion_con_hueco_en_medio() {
        let mut io = Io { dev: Box::new(MemSectors::new(64, 512)), start: 0, len: 64, ss: 512 };
        let run = |vcn, len, lcn| Run { vcn, len, lcn };
        let mut s = Stream { runs: vec![run(0, 1, Some(1)), run(1, 2, None), run(3, 1, Some(3)), run(4, 12, None)], size: 65536, init: 65536, unit: 4 };
        assert_eq!(comp_unit(&mut io, 4096, &s, 0, 16, 65536).map(|_| ()), Err(FsError::Corrupt));
        // Clusters tras un hueco al principio: corrupto; hueco entero: ceros.
        s.runs = vec![run(0, 4, None), run(4, 1, Some(5)), run(5, 11, None)];
        assert!(comp_unit(&mut io, 4096, &s, 0, 16, 65536).is_err());
        s.runs = vec![run(0, 16, None)];
        assert_eq!(comp_unit(&mut io, 4096, &s, 0, 16, 65536).unwrap(), vec![0; 65536]);
        // Una unidad que no cabe en el tope no se lee.
        s.unit = 6;
        assert!(stream_read(&mut io, 1 << 16, &s, 0, 10).is_err());
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
        // Cada byte de las cabeceras clave (arranque, registros, índices), a valores extremos.
        let base = build(Opt { upcase: true, hiber: true, ..Opt::default() });
        let mut campos: Vec<usize> = (0..0x48).collect();
        for r in [0, 5, 25, 27, 28, 40] {
            campos.extend((0..0x100).map(|o| rec_at(r) + 4 + o));
        }
        campos.extend((0..0x60).map(|o| 140 * CL + o).chain((0..0x60).map(|o| 142 * CL + o)));
        let mut montados = 0;
        for &o in &campos {
            for v in [0u8, 0xFF, 0x7F, 0x80, 1] {
                let mut d = base.clone();
                d[o] = v;
                if let Ok(mut vol) = mnt(d) {
                    montados += 1;
                    recorrer(&mut *vol, "", 0, &mut 0);
                }
            }
        }
        assert!(montados > campos.len(), "solo {montados} montadas");
        // Mutaciones al azar de los metadatos.
        let mut x = 0x9E37_79B9u32;
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        let zonas = [(0usize, 512usize), (4 * CL, 43 * 1024), (20 * CL, 8192), (140 * CL, 4 * CL), (2 * CL, 4096)];
        let mut montados = 0;
        for n in 0..1500usize {
            let mut d = base.clone();
            for _ in 0..1 + n % 10 {
                let (a, l) = zonas[rnd() as usize % zonas.len()];
                d[a + rnd() as usize % l] = rnd() as u8;
            }
            if let Ok(mut vol) = mnt(d) {
                montados += 1;
                recorrer(&mut *vol, "", 0, &mut 0);
            }
        }
        assert!(montados > 500, "solo {montados} montadas");
    }
}
