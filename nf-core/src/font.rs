//! Tipografía propia de trazos. CONTRATO: estas firmas públicas no cambian.
//! `size` es la altura de las mayúsculas en píxeles (9 = texto normal de interfaz).
//!
//! Cada glifo se define como trazos (ver `glyphs.rs`), se ajusta a la rejilla de píxeles
//! (así los trazos rectos de 1 px caen exactamente en una columna o fila), se rasteriza una
//! sola vez a un mapa de opacidad y queda en caché.
use crate::geom::Rect;
use crate::glyphs::{glyph, Glyph, ACCENTS, S, UNKNOWN};
use crate::math::{ceil, clamp01, cos, floor, iround, seg_dist, sin, PI};
use crate::surface::{blend, Surface};
use crate::sync::SpinLock;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Mapa de opacidad de un glifo; (ox, oy) es la esquina superior izquierda respecto al
/// origen de la pluma (x) y a la fila de la línea base (y).
struct GlyphBmp {
    ox: i32,
    oy: i32,
    w: i32,
    h: i32,
    a: Vec<u8>,
}

// shortcut: la clave reserva 8 bits para el tamaño; tamaños > 255 comparten caché, subir a u64 si hacen falta.
static CACHE: SpinLock<BTreeMap<u32, GlyphBmp>> = SpinLock::new(BTreeMap::new());

fn scale(size: u32) -> f32 {
    size as f32 / 7.0
}

/// Grosor entero del trazo en píxeles.
fn thick(size: u32, bold: bool) -> i32 {
    let mut t = iround(size as f32 / 11.0).max(1);
    if bold {
        t += 1 + t / 3;
    }
    t
}

/// Acentos: (caracteres con acento, bases) por cada forma de `ACCENTS` (agudo, grave, diéresis, circunflejo, tilde).
const ACC_CHARS: [(&str, &str); 5] = [
    ("áéíóúÁÉÍÓÚ", "aeıouAEIOU"),
    ("àèìòùÀÈÌÒÙ", "aeıouAEIOU"),
    ("äëïöüÄËÏÖÜ", "aeıouAEIOU"),
    ("âêîôûÂÊÎÔÛ", "aeıouAEIOU"),
    ("ñÑ", "nN"),
];

/// Glifo base y acento (forma, altura) de `c`; `None` si el carácter no existe.
fn parts(c: char) -> Option<(&'static Glyph, Option<(&'static [S], f32)>)> {
    if let Some(g) = glyph(c) {
        return Some((g, None));
    }
    for (k, (con, base)) in ACC_CHARS.iter().enumerate() {
        if let Some((_, b)) = con.chars().zip(base.chars()).find(|&(a, _)| a == c) {
            let alto = if b.is_ascii_lowercase() || b == 'ı' { 5.0 } else { 7.0 };
            return Some((glyph(b)?, Some((ACCENTS[k], alto))));
        }
    }
    None
}

fn glyph_of(c: char) -> &'static Glyph {
    parts(c).map_or(&UNKNOWN, |p| p.0)
}

/// Alto de una línea de texto, en píxeles.
pub fn line_h(size: u32) -> i32 {
    let sc = scale(size);
    iround(1.5 * sc) + size as i32 + 1 + iround(2.2 * sc) + 1
}

/// Avance horizontal de un carácter, en píxeles.
pub fn adv(c: char, size: u32, bold: bool) -> i32 {
    let sc = scale(size);
    if c == ' ' {
        return iround(2.6 * sc);
    }
    iround(glyph_of(c).w * sc) + thick(size, bold) + iround(1.2 * sc).max(1)
}

/// Ancho de `text` en píxeles (suma de avances).
pub fn width(text: &str, size: u32, bold: bool) -> i32 {
    text.chars().map(|c| adv(c, size, bold)).sum()
}

/// Segmento ya en píxeles; `r` es el radio (medio grosor).
struct Seg {
    ax: f32,
    ay: f32,
    bx: f32,
    by: f32,
    r: f32,
}

/// Convierte los trazos `strokes` (desplazados (dx, dy) unidades) a segmentos en píxeles ajustados a la rejilla.
fn emit(out: &mut Vec<Seg>, strokes: &[S], dx: f32, dy: f32, sc: f32, t: i32) {
    let off = if t & 1 == 1 { 0.5 } else { 0.0 };
    let fx = |u: f32| iround((u + dx) * sc) as f32 + off;
    let fy = |u: f32| -(iround((u + dy) * sc) as f32) + off;
    let r = t as f32 / 2.0;
    for s in strokes {
        match *s {
            S::L(pts) => {
                for w in pts.windows(2) {
                    out.push(Seg { ax: fx(w[0].0), ay: fy(w[0].1), bx: fx(w[1].0), by: fy(w[1].1), r });
                }
            }
            S::D(x, y) => out.push(Seg { ax: fx(x), ay: fy(y), bx: fx(x), by: fy(y), r: 0.8 * t as f32 }),
            S::A(cx, cy, rx, ry, a0, a1) => {
                // Se ajustan los cuatro bordes de la elipse; de ahí salen centro y radios.
                let (l, rt, top, bot) = (fx(cx - rx), fx(cx + rx), fy(cy + ry), fy(cy - ry));
                let (mx, my, px, py) = ((l + rt) / 2.0, (top + bot) / 2.0, (rt - l) / 2.0, (bot - top) / 2.0);
                let rad = (a1 - a0) * (PI / 180.0);
                let n = (ceil(px.max(py) * rad / 1.2) as i32).clamp(3, 64);
                let punto = |i: i32| {
                    let a = (a0 + (a1 - a0) * i as f32 / n as f32) * (PI / 180.0);
                    (mx + px * cos(a), my - py * sin(a))
                };
                let mut p = punto(0);
                for i in 1..=n {
                    let q = punto(i);
                    out.push(Seg { ax: p.0, ay: p.1, bx: q.0, by: q.1, r });
                    p = q;
                }
            }
        }
    }
}

fn raster(c: char, size: u32, bold: bool) -> GlyphBmp {
    let sc = scale(size);
    let t = thick(size, bold);
    let mut segs = Vec::new();
    if let Some((g, acc)) = parts(c) {
        emit(&mut segs, g.s, 0.0, 0.0, sc, t);
        if let Some((a, alto)) = acc {
            emit(&mut segs, a, g.w / 2.0, alto, sc, t);
        }
    } else {
        emit(&mut segs, UNKNOWN.s, 0.0, 0.0, sc, t);
    }
    if segs.is_empty() {
        return GlyphBmp { ox: 0, oy: 0, w: 0, h: 0, a: Vec::new() };
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for s in &segs {
        let m = s.r + 1.0;
        x0 = x0.min(s.ax.min(s.bx) - m);
        y0 = y0.min(s.ay.min(s.by) - m);
        x1 = x1.max(s.ax.max(s.bx) + m);
        y1 = y1.max(s.ay.max(s.by) + m);
    }
    let (ox, oy) = (floor(x0) as i32, floor(y0) as i32);
    let (w, h) = (ceil(x1) as i32 - ox, ceil(y1) as i32 - oy);
    let mut a = alloc::vec![0u8; (w * h) as usize];
    for s in &segs {
        let m = s.r + 1.0;
        let bx0 = (floor(s.ax.min(s.bx) - m) as i32 - ox).max(0);
        let bx1 = (ceil(s.ax.max(s.bx) + m) as i32 - ox).min(w);
        let by0 = (floor(s.ay.min(s.by) - m) as i32 - oy).max(0);
        let by1 = (ceil(s.ay.max(s.by) + m) as i32 - oy).min(h);
        for j in by0..by1 {
            for i in bx0..bx1 {
                let px = (i + ox) as f32 + 0.5;
                let py = (j + oy) as f32 + 0.5;
                let cov = clamp01(s.r + 0.5 - seg_dist(px, py, s.ax, s.ay, s.bx, s.by));
                let v = (cov * 255.0 + 0.5) as u8;
                let o = (j * w + i) as usize;
                if v > a[o] {
                    a[o] = v;
                }
            }
        }
    }
    GlyphBmp { ox, oy, w, h, a }
}

/// Dibuja `text` con la esquina superior izquierda de su caja de línea en (x, y).
/// Devuelve la x final (x + ancho).
pub fn draw(s: &mut Surface, x: i32, y: i32, text: &str, size: u32, bold: bool, color: u32) -> i32 {
    let base = y + iround(1.5 * scale(size)) + size as i32;
    let clip = s.clip();
    let mut pen = x;
    let mut cache = CACHE.lock();
    for c in text.chars() {
        let key = c as u32 | (size.min(255) << 21) | ((bold as u32) << 29);
        let g = cache.entry(key).or_insert_with(|| raster(c, size, bold));
        let (gx, gy) = (pen + g.ox, base + g.oy);
        let (cx0, cx1) = (gx.max(clip.x), (gx + g.w).min(clip.right()));
        let (cy0, cy1) = (gy.max(clip.y), (gy + g.h).min(clip.bottom()));
        for yy in cy0..cy1 {
            let fila = ((yy - gy) * g.w) as usize;
            for xx in cx0..cx1 {
                let a = g.a[fila + (xx - gx) as usize] as u32;
                if a > 0 {
                    let o = (yy * s.w + xx) as usize;
                    s.px[o] = blend(s.px[o], color, a);
                }
            }
        }
        pen += adv(c, size, bold);
    }
    pen
}

/// Dibuja `text` centrado en vertical dentro de `r` y alineado en horizontal según `align`.
pub fn draw_in(s: &mut Surface, r: Rect, text: &str, size: u32, bold: bool, color: u32, align: Align) {
    let w = width(text, size, bold);
    let x = match align {
        Align::Left => r.x,
        Align::Center => r.x + (r.w - w) / 2,
        Align::Right => r.right() - w,
    };
    draw(s, x, r.y + (r.h - line_h(size)) / 2, text, size, bold, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conocido(c: char) -> bool {
        parts(c).is_some()
    }

    #[test]
    fn ascii_y_espanol_tienen_glifo() {
        for c in (32u8..=126).map(|b| b as char) {
            assert!(conocido(c), "falta {c:?}");
            assert!(adv(c, 9, false) > 0, "avance {c:?}");
        }
        for c in "áéíóúüñÁÉÍÓÚÜÑ¿¡ºª€ç".chars() {
            assert!(conocido(c), "falta {c:?}");
            assert!(adv(c, 9, false) > 0, "avance {c:?}");
        }
        assert!(!conocido('\u{3042}'));
    }

    #[test]
    fn anchos() {
        assert_eq!(width("", 9, false), 0);
        let t = "Hola, ñandú";
        let suma: i32 = t.chars().map(|c| adv(c, 12, true)).sum();
        assert_eq!(width(t, 12, true), suma);
        let d = adv('0', 9, false);
        assert!("0123456789".chars().all(|c| adv(c, 9, false) == d && adv(c, 16, true) == adv('0', 16, true)));
    }

    #[test]
    fn dibujar_con_recorte_no_falla() {
        let mut s = Surface::new(40, 20, 0);
        s.set_clip(Rect::new(5, 3, 20, 10));
        draw(&mut s, -7, -3, "Árbol ñ", 9, false, 0xFFFFFF);
        draw(&mut s, 30, 15, "Zq€", 16, true, 0xFFFFFF);
        draw(&mut s, 500, 500, "fuera", 9, false, 0xFFFFFF);
        draw(&mut s, -500, -500, "fuera", 9, false, 0xFFFFFF);
        draw_in(&mut s, Rect::new(0, 0, 10, 10), "Largo largo", 12, false, 0xFFFFFF, Align::Center);
    }

    #[test]
    fn dibujar_a_cambia_pixeles() {
        let mut s = Surface::new(30, 30, 0);
        let fin = draw(&mut s, 2, 2, "A", 9, false, 0xFFFFFF);
        assert_eq!(fin, 2 + adv('A', 9, false));
        assert!(s.px.iter().any(|&p| p != 0));
    }
}
