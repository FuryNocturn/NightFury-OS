// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Lector y rasterizador TrueType propios (tablas head, hhea, maxp, cmap, hmtx, loca, glyf, kern, OS/2, name),
//! escritos a partir de la especificación pública OpenType. La fuente es entrada hostil (ficheros del usuario):
//! todo acceso a bytes se comprueba y no hay pánicos; los límites de puntos, profundidad y tamaño son constantes.
//! shortcut: sin hinting (los glifos salen con el suavizado tal cual), sin GPOS ni contornos CFF, sin variaciones;
//! mejorar si el texto pequeño se ve borroso.
use crate::math::{ceil, clamp01, floor, iround, sqrt};
use crate::surface::Surface;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontError {
    /// Los bytes no son una fuente TrueType válida.
    Formato,
    /// Es una fuente válida pero de un tipo que no leemos (CFF/.otf, sin cmap útil).
    NoSoportado,
    /// Supera algún límite de seguridad.
    Limite,
}

const MAX_TABLAS: u16 = 256;
const MAX_PUNTOS: usize = 10_000;
const MAX_PROFUNDIDAD: u32 = 8;
/// Glifos cargados por raster (corta los compuestos que se referencian en cascada).
const MAX_CARGAS: u32 = 128;
const MAX_LADO: f32 = 512.0;
const MAX_PX: f32 = 2048.0;
/// tan(12°): inclinación de la cursiva sintética.
const INCLINACION: f32 = 0.2126;

#[derive(Clone, Copy)]
struct Tabla {
    off: usize,
    len: usize,
}

fn bytes(d: &[u8], o: usize, n: usize) -> Option<&[u8]> {
    d.get(o..o.checked_add(n)?)
}
fn rd16(d: &[u8], o: usize) -> Option<u16> {
    bytes(d, o, 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}
fn rdi16(d: &[u8], o: usize) -> Option<i16> {
    rd16(d, o).map(|v| v as i16)
}
fn rd32(d: &[u8], o: usize) -> Option<u32> {
    bytes(d, o, 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Lector secuencial sobre los bytes de un glifo.
struct Lector<'a> {
    d: &'a [u8],
    p: usize,
}
impl Lector<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.d.get(self.p)?;
        self.p += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        let v = rd16(self.d, self.p)?;
        self.p += 2;
        Some(v)
    }
    fn i16(&mut self) -> Option<i16> {
        self.u16().map(|v| v as i16)
    }
    /// Número F2Dot14.
    fn f2(&mut self) -> Option<f32> {
        self.i16().map(|v| v as f32 / 16384.0)
    }
}

#[derive(Clone, Copy)]
struct Pt {
    x: f32,
    y: f32,
    on: bool,
}

/// Puntos de un glifo (unidades de fuente) y el índice del último punto de cada contorno.
#[derive(Default)]
struct Contornos {
    pts: Vec<Pt>,
    ends: Vec<usize>,
}

pub struct Font {
    data: Vec<u8>,
    upem: u16,
    n_glyphs: u16,
    n_hmetrics: u16,
    loca_largo: bool,
    glyf: Tabla,
    loca: Tabla,
    hmtx: Tabla,
    kern: Option<Tabla>,
    name: Option<Tabla>,
    cmap_off: usize,
    cmap_fmt: u16,
    asc: i16,
    desc: i16,
    gap: i16,
    bold: bool,
    italic: bool,
}

#[derive(Clone, Copy, Default)]
pub struct Raster {
    /// Desplazamiento subpíxel horizontal, 0..1.
    pub x_offset: f32,
    /// Negrita sintética.
    pub bold: bool,
    /// Cursiva sintética (~12°).
    pub italic: bool,
}

#[derive(Clone, Default)]
pub struct GlyphBitmap {
    pub w: i32,
    pub h: i32,
    /// Columna del primer píxel respecto al origen del glifo.
    pub left: i32,
    /// Fila superior, medida desde la línea base hacia arriba.
    pub top: i32,
    /// Cobertura 0..255, fila a fila.
    pub cov: Vec<u8>,
}

impl Font {
    pub fn parse(data: Vec<u8>) -> Result<Font, FontError> {
        Self::abrir(data, 0)
    }

    pub fn parse_collection(data: Vec<u8>, index: u32) -> Result<Font, FontError> {
        Self::abrir(data, index)
    }

    fn abrir(data: Vec<u8>, index: u32) -> Result<Font, FontError> {
        let f = FontError::Formato;
        let base = if rd32(&data, 0).ok_or(f)? == 0x7474_6366 {
            // 'ttcf': colección
            if index >= rd32(&data, 8).ok_or(f)? {
                return Err(f);
            }
            rd32(&data, 12usize.saturating_add((index as usize).saturating_mul(4))).ok_or(f)? as usize
        } else if index == 0 {
            0
        } else {
            return Err(f);
        };
        match rd32(&data, base).ok_or(f)? {
            0x0001_0000 | 0x7472_7565 => {}
            0x4F54_544F => return Err(FontError::NoSoportado),
            _ => return Err(f),
        }
        let n = rd16(&data, base + 4).ok_or(f)?;
        if n > MAX_TABLAS {
            return Err(FontError::Limite);
        }
        let tabla = |tag: &[u8; 4]| -> Option<Tabla> {
            (0..n as usize).find_map(|i| {
                let r = base + 12 + 16 * i;
                if bytes(&data, r, 4)? != tag {
                    return None;
                }
                let (off, len) = (rd32(&data, r + 8)? as usize, rd32(&data, r + 12)? as usize);
                (off.checked_add(len)? <= data.len()).then_some(Tabla { off, len })
            })
        };
        let Some(glyf) = tabla(b"glyf") else {
            return Err(if tabla(b"CFF ").is_some() || tabla(b"CFF2").is_some() { FontError::NoSoportado } else { f });
        };
        let (head, hhea, maxp, cmap) = (tabla(b"head").ok_or(f)?, tabla(b"hhea").ok_or(f)?, tabla(b"maxp").ok_or(f)?, tabla(b"cmap").ok_or(f)?);
        let (loca, hmtx) = (tabla(b"loca").ok_or(f)?, tabla(b"hmtx").ok_or(f)?);
        if head.len < 54 || hhea.len < 36 || maxp.len < 6 || cmap.len < 4 {
            return Err(f);
        }
        let upem = rd16(&data, head.off + 18).ok_or(f)?;
        let n_glyphs = rd16(&data, maxp.off + 4).ok_or(f)?;
        let n_hmetrics = rd16(&data, hhea.off + 34).ok_or(f)?;
        if upem == 0 || n_glyphs == 0 || n_hmetrics == 0 {
            return Err(f);
        }
        let loca_largo = rdi16(&data, head.off + 50).ok_or(f)? != 0;
        let mac = rd16(&data, head.off + 44).ok_or(f)?;
        let (mut asc, mut desc, mut gap) =
            (rdi16(&data, hhea.off + 4).ok_or(f)?, rdi16(&data, hhea.off + 6).ok_or(f)?, rdi16(&data, hhea.off + 8).ok_or(f)?);
        let (mut bold, mut italic) = (mac & 1 != 0, mac & 2 != 0);
        if let Some(os2) = tabla(b"OS/2").filter(|t| t.len >= 74) {
            let sel = rd16(&data, os2.off + 62).ok_or(f)?;
            bold = sel & 0x20 != 0;
            italic = sel & 1 != 0;
            if sel & 0x80 != 0 {
                (asc, desc, gap) = (
                    rdi16(&data, os2.off + 68).ok_or(f)?,
                    rdi16(&data, os2.off + 70).ok_or(f)?,
                    rdi16(&data, os2.off + 72).ok_or(f)?,
                );
            }
        }
        // Subtabla cmap: (3,10) > (0,4|6) > (3,1) > (0,x); formatos 4, 6 y 12.
        let mut mejor: Option<(u8, usize, u16)> = None;
        for i in 0..rd16(&data, cmap.off + 2).ok_or(f)? as usize {
            let r = cmap.off + 4 + 8 * i;
            let (plat, enc) = (rd16(&data, r).ok_or(f)?, rd16(&data, r + 2).ok_or(f)?);
            let Some(sub) = cmap.off.checked_add(rd32(&data, r + 4).ok_or(f)? as usize) else { continue };
            let Some(fmt) = rd16(&data, sub) else { continue };
            let prio = match (plat, enc) {
                (3, 10) => 4,
                (0, 4) | (0, 6) => 3,
                (3, 1) => 2,
                (0, _) => 1,
                _ => 0,
            };
            if prio > 0 && matches!(fmt, 4 | 6 | 12) && mejor.map_or(true, |m| prio > m.0) {
                mejor = Some((prio, sub, fmt));
            }
        }
        let (_, cmap_off, cmap_fmt) = mejor.ok_or(FontError::NoSoportado)?;
        let (name, kern) = (tabla(b"name"), tabla(b"kern"));
        Ok(Font {
            data, upem, n_glyphs, n_hmetrics, loca_largo, glyf, loca, hmtx, kern, name, cmap_off, cmap_fmt,
            asc, desc, gap, bold, italic,
        })
    }

    pub fn units_per_em(&self) -> u16 {
        self.upem
    }
    /// OS/2 typo si la fuente pide USE_TYPO_METRICS; si no, hhea.
    pub fn ascender(&self) -> i16 {
        self.asc
    }
    pub fn descender(&self) -> i16 {
        self.desc
    }
    pub fn line_gap(&self) -> i16 {
        self.gap
    }
    pub fn is_bold(&self) -> bool {
        self.bold
    }
    pub fn is_italic(&self) -> bool {
        self.italic
    }

    /// Glifo del carácter; 0 (.notdef) si la fuente no lo tiene.
    pub fn glyph_index(&self, ch: char) -> u16 {
        let c = ch as u32;
        let g = match self.cmap_fmt {
            4 => self.cmap4(c),
            6 => self.cmap6(c),
            _ => self.cmap12(c),
        };
        g.filter(|&g| g < self.n_glyphs as u32).unwrap_or(0) as u16
    }

    fn cmap4(&self, c: u32) -> Option<u32> {
        if c > 0xFFFF {
            return None;
        }
        let (d, o) = (&self.data[..], self.cmap_off);
        let seg2 = rd16(d, o + 6)? as usize;
        let n = seg2 / 2;
        let fin = o + 14;
        let ini = fin + seg2 + 2;
        let delta = ini + seg2;
        let ro = delta + seg2;
        // primer segmento cuyo final es >= c
        let (mut lo, mut hi) = (0, n);
        while lo < hi {
            let m = (lo + hi) / 2;
            if (rd16(d, fin + 2 * m)? as u32) < c { lo = m + 1 } else { hi = m }
        }
        if lo >= n {
            return None;
        }
        let ini_seg = rd16(d, ini + 2 * lo)? as u32;
        if ini_seg > c {
            return None;
        }
        let dl = rd16(d, delta + 2 * lo)? as u32;
        let r = rd16(d, ro + 2 * lo)? as usize;
        let g = if r == 0 {
            c
        } else {
            match rd16(d, ro + 2 * lo + r + 2 * (c - ini_seg) as usize)? {
                0 => return None,
                g => g as u32,
            }
        };
        Some((g + dl) & 0xFFFF)
    }

    fn cmap6(&self, c: u32) -> Option<u32> {
        let (d, o) = (&self.data[..], self.cmap_off);
        let primero = rd16(d, o + 6)? as u32;
        let cuenta = rd16(d, o + 8)? as u32;
        let i = c.checked_sub(primero).filter(|&i| i < cuenta)?;
        rd16(d, o + 10 + 2 * i as usize).map(u32::from)
    }

    fn cmap12(&self, c: u32) -> Option<u32> {
        let (d, o) = (&self.data[..], self.cmap_off);
        let n = rd32(d, o + 12)? as usize;
        let (mut lo, mut hi) = (0, n);
        while lo < hi {
            let m = (lo + hi) / 2;
            if rd32(d, o + 16 + 12 * m + 4)? < c { lo = m + 1 } else { hi = m }
        }
        let g = o + 16 + 12 * lo; // lo == n falla en la lectura
        let ini = rd32(d, g)?;
        if ini > c {
            return None;
        }
        rd32(d, g + 8)?.checked_add(c - ini)
    }

    /// Avance horizontal en unidades de fuente (0 si el glifo no existe).
    pub fn advance(&self, gid: u16) -> u16 {
        if gid >= self.n_glyphs || self.n_hmetrics as usize * 4 > self.hmtx.len {
            return 0;
        }
        let i = gid.min(self.n_hmetrics - 1) as usize;
        rd16(&self.data, self.hmtx.off + 4 * i).unwrap_or(0)
    }

    /// Ajuste de pareja de la tabla `kern` (formato 0 horizontal); suma las subtablas.
    pub fn kern(&self, a: u16, b: u16) -> i16 {
        self.kern_suma(a, b).unwrap_or(0).clamp(i16::MIN as i32, i16::MAX as i32) as i16
    }

    fn kern_suma(&self, a: u16, b: u16) -> Option<i32> {
        let (d, t) = (&self.data[..], self.kern?);
        let clave = (a as u32) << 16 | b as u32;
        let (mut o, mut suma) = (t.off + 4, 0i32);
        for _ in 0..rd16(d, t.off + 2)?.min(16) {
            let len = rd16(d, o + 2)? as usize;
            if rd16(d, o + 4)? == 1 {
                // cobertura: formato 0 (byte alto) con la bandera «horizontal»
                let pares = o + 14;
                let (mut lo, mut hi) = (0, rd16(d, o + 6)? as usize);
                while lo < hi {
                    let m = (lo + hi) / 2;
                    let k = (rd16(d, pares + 6 * m)? as u32) << 16 | rd16(d, pares + 6 * m + 2)? as u32;
                    if k == clave {
                        suma += rdi16(d, pares + 6 * m + 4)? as i32;
                        break;
                    } else if k < clave {
                        lo = m + 1
                    } else {
                        hi = m
                    }
                }
            }
            if len < 6 {
                break;
            }
            o += len;
        }
        Some(suma)
    }

    /// Nombre de familia (name id 1), preferentemente inglés de Windows.
    pub fn family_name(&self) -> Option<String> {
        let (d, t) = (&self.data[..], self.name?);
        let cuerda = t.off + rd16(d, t.off + 4)? as usize;
        let mut mejor: Option<(u8, usize, usize)> = None;
        for i in 0..rd16(d, t.off + 2)? as usize {
            let r = t.off + 6 + 12 * i;
            if rd16(d, r + 6)? != 1 {
                continue;
            }
            let prio = match rd16(d, r)? {
                3 => if rd16(d, r + 4)? == 0x409 { 3 } else { 2 },
                0 => 1,
                _ => continue,
            };
            if mejor.map_or(true, |m| prio > m.0) {
                mejor = Some((prio, cuerda + rd16(d, r + 10)? as usize, rd16(d, r + 8)? as usize));
            }
        }
        let (_, o, len) = mejor?;
        let u = bytes(d, o, len)?.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]]));
        Some(char::decode_utf16(u).map(|r| r.unwrap_or('\u{FFFD}')).collect())
    }

    /// Bytes del glifo dentro de `glyf` (vacío si no tiene contorno).
    fn glifo(&self, gid: u16) -> Option<&[u8]> {
        if gid >= self.n_glyphs {
            return None;
        }
        let (d, i) = (&self.data[..], gid as usize);
        let (a, b) = if self.loca_largo {
            if (i + 2) * 4 > self.loca.len {
                return None;
            }
            (rd32(d, self.loca.off + 4 * i)? as usize, rd32(d, self.loca.off + 4 * i + 4)? as usize)
        } else {
            if (i + 2) * 2 > self.loca.len {
                return None;
            }
            (rd16(d, self.loca.off + 2 * i)? as usize * 2, rd16(d, self.loca.off + 2 * i + 2)? as usize * 2)
        };
        if b <= a {
            return Some(&[]);
        }
        if b > self.glyf.len {
            return None;
        }
        bytes(d, self.glyf.off + a, b - a)
    }

    fn cargar(&self, gid: u16, prof: u32, cargas: &mut u32, out: &mut Contornos) -> Option<()> {
        *cargas += 1;
        if prof > MAX_PROFUNDIDAD || *cargas > MAX_CARGAS {
            return None;
        }
        let g = self.glifo(gid)?;
        if g.is_empty() {
            return Some(());
        }
        let mut r = Lector { d: g, p: 0 };
        let nc = r.i16()?;
        r.p = 10;
        if nc >= 0 { Self::simple(&mut r, nc as usize, out) } else { self.compuesto(r, prof, cargas, out) }
    }

    fn simple(r: &mut Lector, nc: usize, out: &mut Contornos) -> Option<()> {
        let base = out.pts.len();
        let mut n = 0;
        for _ in 0..nc {
            let e = r.u16()? as usize;
            if e < n {
                return None; // los finales de contorno deben crecer
            }
            n = e + 1;
            out.ends.push(base + e);
        }
        if base + n > MAX_PUNTOS {
            return None;
        }
        let instr = r.u16()? as usize;
        r.p = r.p.checked_add(instr)?;
        let mut fl = Vec::with_capacity(n);
        while fl.len() < n {
            let f = r.u8()?;
            fl.push(f);
            if f & 8 != 0 {
                for _ in 0..r.u8()? {
                    if fl.len() >= n {
                        return None;
                    }
                    fl.push(f);
                }
            }
        }
        for f in &fl {
            out.pts.push(Pt { x: 0.0, y: 0.0, on: f & 1 != 0 });
        }
        // x (bits 1 y 4) y luego y (bits 2 y 5): byte sin signo con signo en el bit 4/5, «igual al anterior» o palabra
        for (eje, corto, igual) in [(0, 2u8, 16u8), (1, 4, 32)] {
            let mut v = 0i32;
            for (p, f) in out.pts[base..].iter_mut().zip(&fl) {
                if f & corto != 0 {
                    let d = r.u8()? as i32;
                    v += if f & igual != 0 { d } else { -d };
                } else if f & igual == 0 {
                    v += r.i16()? as i32;
                }
                if eje == 0 { p.x = v as f32 } else { p.y = v as f32 }
            }
        }
        Some(())
    }

    fn compuesto(&self, mut r: Lector, prof: u32, cargas: &mut u32, out: &mut Contornos) -> Option<()> {
        let inicio = out.pts.len();
        loop {
            let fl = r.u16()?;
            let gid = r.u16()?;
            let (mut a1, mut a2) = if fl & 1 != 0 { (r.u16()? as i32, r.u16()? as i32) } else { (r.u8()? as i32, r.u8()? as i32) };
            if fl & 2 != 0 {
                // desplazamiento con signo
                (a1, a2) = if fl & 1 != 0 { (a1 as u16 as i16 as i32, a2 as u16 as i16 as i32) } else { (a1 as u8 as i8 as i32, a2 as u8 as i8 as i32) };
            }
            let (mut m00, mut m01, mut m10, mut m11) = (1.0, 0.0, 0.0, 1.0);
            if fl & 8 != 0 {
                m00 = r.f2()?;
                m11 = m00;
            } else if fl & 0x40 != 0 {
                (m00, m11) = (r.f2()?, r.f2()?);
            } else if fl & 0x80 != 0 {
                (m00, m01, m10, m11) = (r.f2()?, r.f2()?, r.f2()?, r.f2()?);
            }
            let ini = out.pts.len();
            self.cargar(gid, prof + 1, cargas, out)?;
            for p in &mut out.pts[ini..] {
                (p.x, p.y) = (m00 * p.x + m10 * p.y, m01 * p.x + m11 * p.y);
            }
            let (dx, dy) = if fl & 2 != 0 {
                (a1 as f32, a2 as f32)
            } else {
                // emparejamiento de puntos: punto a1 del padre sobre punto a2 del hijo
                let p = out.pts[inicio..ini].get(usize::try_from(a1).ok()?)?;
                let c = out.pts.get(ini.checked_add(usize::try_from(a2).ok()?)?)?;
                (p.x - c.x, p.y - c.y)
            };
            for p in &mut out.pts[ini..] {
                p.x += dx;
                p.y += dy;
            }
            if fl & 0x20 == 0 {
                return Some(());
            }
        }
    }

    /// Mapa de cobertura del glifo a `px` píxeles por em. `None` si no tiene contorno (espacio), si el glifo
    /// está corrupto o si el mapa pasaría de 512 px de lado.
    pub fn raster(&self, gid: u16, px: f32, opts: Raster) -> Option<GlyphBitmap> {
        if !(px > 0.0) || px > MAX_PX {
            return None;
        }
        let mut c = Contornos::default();
        self.cargar(gid, 0, &mut 0, &mut c)?;
        if c.pts.is_empty() {
            return None;
        }
        let esc = px / self.upem as f32;
        let inc = if opts.italic { INCLINACION } else { 0.0 };
        let negrita = if opts.bold { px / 24.0 } else { 0.0 };
        let xo = clamp01(opts.x_offset);
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in &mut c.pts {
            let y = p.y * esc;
            (p.x, p.y) = (p.x * esc + inc * y + xo, y);
            (x0, x1, y0, y1) = (x0.min(p.x), x1.max(p.x), y0.min(p.y), y1.max(p.y));
        }
        let (x0, x1, y0, y1) = (floor(x0), ceil(x1 + negrita), floor(y0), ceil(y1));
        if x1 - x0 > MAX_LADO || y1 - y0 > MAX_LADO || x1 <= x0 || y1 <= y0 {
            return None;
        }
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let paso = w + 2;
        let mut acc = vec![0f32; paso * h];
        for p in &mut c.pts {
            (p.x, p.y) = (p.x - x0, y1 - p.y);
        }
        recorrer(&c, &mut |ax, ay, bx, by| {
            linea(&mut acc, paso, h, ax, ay, bx, by);
            if negrita > 0.0 {
                // negrita: segunda copia desplazada; las áreas se suman y la cobertura se recorta a 1 (unión)
                linea(&mut acc, paso, h, ax + negrita, ay, bx + negrita, by);
            }
        });
        let mut cov = Vec::with_capacity(w * h);
        for fila in acc.chunks_exact(paso) {
            let mut s = 0.0f32;
            for &a in &fila[..w] {
                s += a;
                cov.push((clamp01(if s < 0.0 { -s } else { s }) * 255.0 + 0.5) as u8);
            }
        }
        Some(GlyphBitmap { w: w as i32, h: h as i32, left: x0 as i32, top: y1 as i32, cov })
    }
}

/// Recorre los contornos y emite segmentos rectos (las curvas cuadráticas se aplanan).
fn recorrer(c: &Contornos, emit: &mut impl FnMut(f32, f32, f32, f32)) {
    let mut ini = 0;
    for &e in &c.ends {
        let Some(p) = c.pts.get(ini..=e) else { break };
        contorno(p, emit);
        ini = e + 1;
    }
}

type P = (f32, f32);

fn contorno(p: &[Pt], emit: &mut impl FnMut(f32, f32, f32, f32)) {
    let n = p.len();
    if n < 2 {
        return;
    }
    let xy = |q: &Pt| (q.x, q.y);
    // se empieza en un punto de la curva; si todos son de control, en el punto medio del primero y el último
    let (inicio, desde, cuenta) = match p.iter().position(|q| q.on) {
        Some(k) => (xy(&p[k]), k + 1, n - 1),
        None => (((p[0].x + p[n - 1].x) / 2.0, (p[0].y + p[n - 1].y) / 2.0), 0, n),
    };
    let (mut cur, mut ctrl): (P, Option<P>) = (inicio, None);
    for j in 0..cuenta {
        let q = &p[(desde + j) % n];
        if q.on {
            match ctrl.take() {
                Some(c) => cuadratica(cur, c, xy(q), emit),
                None => emit(cur.0, cur.1, q.x, q.y),
            }
            cur = xy(q);
        } else {
            if let Some(c) = ctrl {
                let m = ((c.0 + q.x) / 2.0, (c.1 + q.y) / 2.0);
                cuadratica(cur, c, m, emit);
                cur = m;
            }
            ctrl = Some(xy(q));
        }
    }
    match ctrl {
        Some(c) => cuadratica(cur, c, inicio, emit),
        None => emit(cur.0, cur.1, inicio.0, inicio.1),
    }
}

fn cuadratica(a: P, c: P, b: P, emit: &mut impl FnMut(f32, f32, f32, f32)) {
    // error de aplanado = |a-2c+b| / (4 n²); tolerancia 0,1 px
    let (dx, dy) = (a.0 - 2.0 * c.0 + b.0, a.1 - 2.0 * c.1 + b.1);
    let n = (ceil(sqrt(sqrt(dx * dx + dy * dy) * 2.5)) as i32).clamp(1, 32);
    let mut prev = a;
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        let q = if i == n { b } else { (u * u * a.0 + 2.0 * u * t * c.0 + t * t * b.0, u * u * a.1 + 2.0 * u * t * c.1 + t * t * b.1) };
        emit(prev.0, prev.1, q.0, q.1);
        prev = q;
    }
}

/// Acumula en `acc` (filas de `paso` celdas) el área con signo del segmento: sumando cada fila de izquierda a
/// derecha se obtiene la cobertura (regla nonzero aproximada: se recorta |suma| a 1).
fn linea(acc: &mut [f32], paso: usize, h: usize, x0: f32, y0: f32, x1: f32, y1: f32) {
    if y0 == y1 {
        return;
    }
    let (dir, xa, ya, xb, yb) = if y0 < y1 { (1.0, x0, y0, x1, y1) } else { (-1.0, x1, y1, x0, y0) };
    let (ya_c, yb_c) = (ya.max(0.0), yb.min(h as f32));
    if ya_c >= yb_c {
        return;
    }
    let k = (xb - xa) / (yb - ya);
    let mut x = xa + (ya_c - ya) * k;
    for r in floor(ya_c) as usize..ceil(yb_c) as usize {
        let dy = yb_c.min((r + 1) as f32) - ya_c.max(r as f32);
        if dy <= 0.0 {
            continue;
        }
        let xn = x + k * dy;
        if let Some(fila) = acc.get_mut(r * paso..(r + 1) * paso) {
            celdas(fila, x, xn, dy * dir);
        }
        x = xn;
    }
}

/// Reparte entre las celdas de una fila un trozo de segmento que va de `xa` a `xb` con altura con signo `d`.
fn celdas(fila: &mut [f32], xa: f32, xb: f32, d: f32) {
    let max = (fila.len() - 2) as f32;
    let (lo, hi) = if xa < xb { (xa, xb) } else { (xb, xa) };
    let (lo, hi) = (lo.clamp(0.0, max), hi.clamp(0.0, max));
    if hi - lo < 1e-6 {
        let c = floor(lo) as usize;
        let f = lo - c as f32;
        fila[c] += d * (1.0 - f);
        fila[c + 1] += d * f;
        return;
    }
    for c in floor(lo) as usize..ceil(hi) as usize {
        // parte del trozo que cae en la celda c; su x media reparte el área entre la celda y las siguientes
        let (l, r) = (lo.max(c as f32), hi.min((c + 1) as f32));
        let dd = d * (r - l) / (hi - lo);
        let f = (l + r) / 2.0 - c as f32;
        fila[c] += dd * (1.0 - f);
        fila[c + 1] += dd * f;
    }
}

/// Caché de mapas de glifos: (fuente, glifo, tamaño×4, banderas) → mapa. Se vacía entera al pasar del tope.
pub struct TextCache {
    mapas: BTreeMap<(u32, u16, u32, u8), GlyphBitmap>,
    bytes: usize,
}

const TOPE_CACHE: usize = 4 * 1024 * 1024;

impl TextCache {
    pub fn new() -> Self {
        TextCache { mapas: BTreeMap::new(), bytes: 0 }
    }

    /// Mapa del glifo (vacío si no hay nada que dibujar). `px4` es el tamaño en cuartos de píxel y `sub` el
    /// subpíxel en cuartos.
    fn mapa(&mut self, id: u32, font: &Font, gid: u16, px4: u32, sub: u8, bold: bool, italic: bool) -> &GlyphBitmap {
        let clave = (id, gid, px4, sub << 2 | (italic as u8) << 1 | bold as u8);
        if !self.mapas.contains_key(&clave) {
            let o = Raster { x_offset: sub as f32 / 4.0, bold, italic };
            let m = font.raster(gid, px4 as f32 / 4.0, o).unwrap_or_default();
            if self.bytes + m.cov.len() + 64 > TOPE_CACHE {
                self.mapas.clear();
                self.bytes = 0;
            }
            self.bytes += m.cov.len() + 64;
            self.mapas.insert(clave, m);
        }
        &self.mapas[&clave]
    }
}

impl Default for TextCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Ancho en píxeles del texto con avances y kern.
pub fn measure(font: &Font, px: f32, text: &str) -> f32 {
    let esc = px / font.units_per_em() as f32;
    let (mut x, mut prev) = (0.0, 0u16);
    for ch in text.chars() {
        let g = font.glyph_index(ch);
        x += (font.advance(g) as f32 + if prev != 0 { font.kern(prev, g) as f32 } else { 0.0 }) * esc;
        prev = g;
    }
    x
}

/// Dibuja `text` con la línea base en `baseline` y devuelve la x final. Respeta el clip de `s`; el subpíxel
/// horizontal lo decide la posición del lápiz (se ignora `opts.x_offset`).
/// shortcut: el subpíxel y el tamaño se cuantizan a cuartos de píxel; afinar si se nota en textos grandes.
pub fn draw_text(
    s: &mut Surface, cache: &mut TextCache, font_id: u32, font: &Font, x: f32, baseline: f32, px: f32, color: u32,
    opts: Raster, text: &str,
) -> f32 {
    let esc = px / font.units_per_em() as f32;
    let px4 = iround(px * 4.0).max(1) as u32;
    let by = iround(baseline);
    let (mut x, mut prev) = (x, 0u16);
    for ch in text.chars() {
        let g = font.glyph_index(ch);
        if prev != 0 {
            x += font.kern(prev, g) as f32 * esc;
        }
        let (mut ix, mut q) = (floor(x), iround((x - floor(x)) * 4.0));
        if q >= 4 {
            (ix, q) = (ix + 1.0, 0);
        }
        let m = cache.mapa(font_id, font, g, px4, q as u8, opts.bold, opts.italic);
        for fila in 0..m.h {
            for col in 0..m.w {
                let a = m.cov[(fila * m.w + col) as usize];
                if a != 0 {
                    s.blend_px(ix as i32 + m.left + col, by - m.top + fila, color, a as u32);
                }
            }
        }
        x += font.advance(g) as f32 * esc;
        prev = g;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::Surface;

    fn p16(v: &mut Vec<u8>, x: u16) {
        v.extend_from_slice(&x.to_be_bytes());
    }
    fn p32(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_be_bytes());
    }
    fn par(v: &mut Vec<u8>) {
        if v.len() % 2 == 1 {
            v.push(0);
        }
    }

    /// Cuadrado (100,0)-(600,500): puntos y deltas con palabras y banderas «igual al anterior».
    fn cuadrado(en_curva: bool) -> Vec<u8> {
        let mut g = Vec::new();
        for v in [1u16, 100, 0, 600, 500, 3, 0] {
            p16(&mut g, v); // contornos, caja, final del contorno, instrucciones
        }
        let on = en_curva as u8;
        g.extend_from_slice(&[on | 0x20, on | 0x10, on | 0x20, on | 0x10]);
        for v in [100u16, 500] {
            p16(&mut g, v); // x: p0 y p2
        }
        for v in [500u16, (-500i16) as u16] {
            p16(&mut g, v); // y: p1 y p3
        }
        par(&mut g);
        g
    }

    fn compuesto(partes: &[(u16, u16, &[u8])]) -> Vec<u8> {
        let mut g = Vec::new();
        for v in [0xFFFFu16, 0, 0, 1000, 500] {
            p16(&mut g, v);
        }
        for (i, (fl, gid, resto)) in partes.iter().enumerate() {
            p16(&mut g, fl | if i + 1 < partes.len() { 0x20 } else { 0 });
            p16(&mut g, *gid);
            g.extend_from_slice(resto);
        }
        par(&mut g);
        g
    }

    /// Fuente mínima: glifos 0 (vacío), 1 (cuadrado), 2 (dos cuadrados), 3 (cuadrado a media escala), 4 (todo fuera de curva).
    fn fuente() -> Vec<u8> {
        let glifos = [
            Vec::new(),
            cuadrado(true),
            compuesto(&[(3, 1, &[0, 0, 0, 0]), (3, 1, &[0x02, 0xBC, 0, 0])]),
            compuesto(&[(0x0A, 1, &[0, 0, 0x20, 0x00])]),
            cuadrado(false),
        ];
        let (mut glyf, mut loca) = (Vec::new(), Vec::new());
        for g in &glifos {
            p16(&mut loca, (glyf.len() / 2) as u16);
            glyf.extend_from_slice(g);
        }
        p16(&mut loca, (glyf.len() / 2) as u16);
        let mut head = vec![0u8; 54];
        head[18..20].copy_from_slice(&1000u16.to_be_bytes());
        head[44..46].copy_from_slice(&1u16.to_be_bytes());
        let mut hhea = vec![0u8; 36];
        hhea[4..6].copy_from_slice(&800u16.to_be_bytes());
        hhea[6..8].copy_from_slice(&(-200i16 as u16).to_be_bytes());
        hhea[8..10].copy_from_slice(&100u16.to_be_bytes());
        hhea[34..36].copy_from_slice(&3u16.to_be_bytes());
        let mut maxp = vec![0u8; 6];
        maxp[4..6].copy_from_slice(&5u16.to_be_bytes());
        let mut hmtx = Vec::new();
        for a in [500u16, 700, 1400] {
            p16(&mut hmtx, a);
            p16(&mut hmtx, 0);
        }
        hmtx.extend_from_slice(&[0; 4]);
        let mut cmap = Vec::new();
        for v in [0u16, 1, 3, 1] {
            p16(&mut cmap, v);
        }
        p32(&mut cmap, 12);
        for v in [4u16, 0, 0, 6, 0, 0, 0] {
            p16(&mut cmap, v); // formato, longitud (se rellena), idioma, segCountX2, búsqueda
        }
        for v in [68u16, 69, 0xFFFF, 0, 65, 69, 0xFFFF, (1u16.wrapping_sub(65)), 0, 1, 0, 4, 0, 2, 0] {
            p16(&mut cmap, v);
        }
        let l = (cmap.len() - 12) as u16;
        cmap[14..16].copy_from_slice(&l.to_be_bytes());
        let mut kern = Vec::new();
        for v in [0u16, 1, 0, 20, 1, 1, 6, 0, 0, 1, 2, (-50i16) as u16] {
            p16(&mut kern, v);
        }
        let mut name = Vec::new();
        for v in [0u16, 1, 18, 3, 1, 0x409, 1, 12, 0] {
            p16(&mut name, v);
        }
        for u in "Prueba".encode_utf16() {
            p16(&mut name, u);
        }
        let mut os2 = vec![0u8; 78];
        os2[62..64].copy_from_slice(&0xA1u16.to_be_bytes());
        os2[68..70].copy_from_slice(&900u16.to_be_bytes());
        os2[70..72].copy_from_slice(&(-250i16 as u16).to_be_bytes());
        os2[72..74].copy_from_slice(&50u16.to_be_bytes());
        let mut tablas: Vec<(&[u8; 4], Vec<u8>)> = vec![
            (b"OS/2", os2), (b"cmap", cmap), (b"glyf", glyf), (b"head", head), (b"hhea", hhea), (b"hmtx", hmtx),
            (b"kern", kern), (b"loca", loca), (b"maxp", maxp), (b"name", name),
        ];
        tablas.sort_by_key(|t| *t.0);
        let mut f = Vec::new();
        p32(&mut f, 0x0001_0000);
        p16(&mut f, tablas.len() as u16);
        f.extend_from_slice(&[0; 6]);
        let mut off = 12 + 16 * tablas.len();
        for (tag, d) in &tablas {
            f.extend_from_slice(*tag);
            p32(&mut f, 0);
            p32(&mut f, off as u32);
            p32(&mut f, d.len() as u32);
            off += (d.len() + 3) & !3;
        }
        for (_, d) in &tablas {
            f.extend_from_slice(d);
            while f.len() % 4 != 0 {
                f.push(0);
            }
        }
        f
    }

    fn abrir() -> Font {
        Font::parse(fuente()).expect("la fuente de prueba debe cargar")
    }

    fn fila(m: &GlyphBitmap, y: i32) -> &[u8] {
        &m.cov[(y * m.w) as usize..((y + 1) * m.w) as usize]
    }

    #[test]
    fn metricas_y_cmap() {
        let f = abrir();
        assert_eq!(f.units_per_em(), 1000);
        assert_eq!((f.ascender(), f.descender(), f.line_gap()), (900, -250, 50)); // OS/2 con USE_TYPO_METRICS
        assert!(f.is_bold() && f.is_italic());
        assert_eq!(f.family_name().as_deref(), Some("Prueba"));
        assert_eq!((f.glyph_index('A'), f.glyph_index('D'), f.glyph_index('E')), (1, 4, 2));
        assert_eq!((f.glyph_index('Z'), f.glyph_index('€'), f.glyph_index('😀')), (0, 0, 0));
        assert_eq!((f.advance(0), f.advance(1), f.advance(2), f.advance(4), f.advance(99)), (500, 700, 1400, 1400, 0));
        assert_eq!((f.kern(1, 2), f.kern(2, 1)), (-50, 0));
        assert!((measure(&f, 10.0, "AB") - 20.5).abs() < 1e-3);
    }

    #[test]
    fn cuadrado_cubre_lo_esperado() {
        let f = abrir();
        // 10 px: el cuadrado ocupa x 1..6, y 0..5 sobre la línea base, alineado a píxeles
        let m = f.raster(1, 10.0, Raster::default()).unwrap();
        assert_eq!((m.w, m.h, m.left, m.top), (5, 5, 1, 5));
        assert!(m.cov.iter().all(|&c| c == 255));
        // 15 px: x 1,5..9 e y 0..7,5: la columna izquierda y la fila superior valen la mitad
        let m = f.raster(1, 15.0, Raster::default()).unwrap();
        assert_eq!((m.w, m.h, m.left, m.top), (8, 8, 1, 8));
        assert!((120..=136).contains(&fila(&m, 4)[0]));
        assert!((120..=136).contains(&fila(&m, 0)[3]));
        assert_eq!(fila(&m, 4)[3], 255);
        assert_eq!(fila(&m, 4)[7], 255);
        // el subpíxel mueve el borde
        let m = f.raster(1, 10.0, Raster { x_offset: 0.5, ..Default::default() }).unwrap();
        assert!((126..=130).contains(&fila(&m, 2)[0]));
        assert!(f.raster(0, 10.0, Raster::default()).is_none()); // sin contorno
        assert!(f.raster(1, 5000.0, Raster::default()).is_none());
        assert!(f.raster(1, 1200.0, Raster::default()).is_none()); // más de 512 px
    }

    #[test]
    fn compuestos() {
        let f = abrir();
        let m = f.raster(2, 10.0, Raster::default()).unwrap();
        assert_eq!((m.w, m.left), (12, 1));
        assert_eq!((fila(&m, 2)[0], fila(&m, 2)[6], fila(&m, 2)[7], fila(&m, 2)[11]), (255, 0, 255, 255));
        // escala 0,5 con 20 px: x 1..6, y 0..5
        let m = f.raster(3, 20.0, Raster::default()).unwrap();
        assert_eq!((m.w, m.h, m.left, m.top), (5, 5, 1, 5));
        assert!(m.cov.iter().all(|&c| c == 255));
    }

    #[test]
    fn curvas_negrita_cursiva() {
        let f = abrir();
        let m = f.raster(4, 10.0, Raster::default()).unwrap();
        assert_eq!(fila(&m, 2)[2], 255);
        assert!(fila(&m, 4)[0] < 128); // esquina recortada por la curva
        let n = f.raster(1, 10.0, Raster { bold: true, ..Default::default() }).unwrap();
        assert!(n.w > 5 && fila(&n, 2).iter().all(|&c| c > 0));
        let i = f.raster(1, 10.0, Raster { italic: true, ..Default::default() }).unwrap();
        assert!(i.w > 5 && fila(&i, 0)[0] < 60 && fila(&i, 4)[0] > 200);
    }

    #[test]
    fn dibuja_con_clip_y_cache() {
        let f = abrir();
        let mut s = Surface::new(20, 12, 0);
        let mut c = TextCache::new();
        let x = draw_text(&mut s, &mut c, 7, &f, 0.0, 8.0, 10.0, 0xFFFFFF, Raster::default(), "A");
        assert!((x - 7.0).abs() < 1e-4);
        assert_eq!((s.px[5 * 20 + 3], s.px[5 * 20], s.px[11 * 20 + 3]), (0xFFFFFF, 0, 0));
        s.set_clip(crate::geom::Rect::new(0, 0, 3, 12));
        s.px.fill(0);
        draw_text(&mut s, &mut c, 7, &f, 0.0, 8.0, 10.0, 0xFFFFFF, Raster::default(), "A");
        assert_eq!((s.px[5 * 20 + 2], s.px[5 * 20 + 3]), (0xFFFFFF, 0));
        assert_eq!(c.mapas.len(), 1);
    }

    fn ejercitar(f: &Font) {
        for g in 0..7 {
            let _ = (f.advance(g), f.kern(g, 1), f.raster(g, 12.0, Raster { bold: true, italic: true, x_offset: 0.5 }));
        }
        for ch in ['A', 'E', 'Z', '€', '😀'] {
            let _ = f.glyph_index(ch);
        }
        let _ = (f.family_name(), f.ascender(), measure(f, 12.0, "ABE"));
    }

    #[test]
    fn datos_corruptos_no_hacen_panico() {
        let bien = fuente();
        for n in 0..bien.len() {
            if let Ok(f) = Font::parse(bien[..n].to_vec()) {
                ejercitar(&f);
            }
        }
        for i in 0..bien.len() {
            for v in [0x00u8, 0xFF, 0x7F, bien[i] ^ 0x80] {
                let mut d = bien.clone();
                d[i] = v;
                if let Ok(f) = Font::parse(d) {
                    ejercitar(&f);
                }
            }
        }
        assert!(Font::parse(Vec::new()).is_err());
        assert!(Font::parse_collection(bien, 1).is_err());
    }

    #[test]
    fn otf_no_soportado() {
        let mut d = fuente();
        d[..4].copy_from_slice(b"OTTO");
        assert_eq!(Font::parse(d).err(), Some(FontError::NoSoportado));
    }

    #[test]
    #[ignore]
    fn fuente_del_sistema() {
        for ruta in ["C:\\Windows\\Fonts\\arial.ttf", "C:\\Windows\\Fonts\\segoeui.ttf"] {
            let Ok(d) = std::fs::read(ruta) else { continue };
            let f = Font::parse(d).expect("fuente del sistema");
            for ch in "Ñandú".chars() {
                let g = f.glyph_index(ch);
                assert!(g != 0, "falta {ch}");
                assert!(f.raster(g, 16.0, Raster::default()).is_some());
            }
            return;
        }
    }
}
