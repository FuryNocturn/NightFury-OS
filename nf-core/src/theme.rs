// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Tema y fondos de escritorio propios. CONTRATO: `Theme`, `ACCENTS`, `WALLPAPERS` y la
//! firma de `render_wallpaper` no cambian.
use crate::geom::Rect;
use crate::math::{clamp01, floor, sin, sqrt, PI};
use crate::surface::{darken, Surface};
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Theme {
    /// Color de énfasis 0xRRGGBB.
    pub accent: u32,
    /// Índice en `WALLPAPERS`.
    pub wallpaper: u8,
}

/// Paleta de colores de énfasis que ofrece Configuración. El primero es el de por defecto.
pub const ACCENTS: [u32; 12] = [
    0x1877D4, 0x0E8A8A, 0x2E8B57, 0x6B8E23, 0xC98A0B, 0xE0701A, 0xD13B3B, 0xC2398F, 0x8A4FD6, 0x5A5FD6, 0x5D6B7A,
    0x3A3F47,
];

/// Nombres de los fondos disponibles, por índice.
pub const WALLPAPERS: [&str; 4] = ["Noche", "Aurora", "Amanecer", "Color sólido"];

impl Default for Theme {
    fn default() -> Self {
        Theme { accent: ACCENTS[0], wallpaper: 0 }
    }
}

type Rgb = [f32; 3];

/// Halo circular: mezcla hacia `col` con fuerza `k` en el centro, cae con el cuadrado hasta `r`.
struct Glow {
    cx: f32,
    cy: f32,
    r: f32,
    col: Rgb,
    k: f32,
}

/// Matriz de tramado ordenado 4x4.
const BAYER: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

/// Pasa un color en coma flotante (0..255) a 8 bits con tramado, para evitar el bandeado.
fn dither(v: Rgb, x: i32, y: i32) -> u32 {
    let t = BAYER[((y & 3) * 4 + (x & 3)) as usize] as f32 / 16.0 + 1.0 / 32.0;
    let ch = |c: f32| ((c + t) as u32).min(255);
    (ch(v[0]) << 16) | (ch(v[1]) << 8) | ch(v[2])
}

/// Color del degradado vertical definido por `stops` (posición 0..1, color) en la posición `t`.
fn ramp(stops: &[(f32, Rgb)], t: f32) -> Rgb {
    let mut prev = stops[0];
    for &(p, c) in stops {
        if t <= p {
            let f = if p > prev.0 { clamp01((t - prev.0) / (p - prev.0)) } else { 1.0 };
            return [0, 1, 2].map(|i| prev.1[i] + (c[i] - prev.1[i]) * f);
        }
        prev = (p, c);
    }
    prev.1
}

/// Generador pseudoaleatorio propio (LCG de 64 bits) con semilla fija: los fondos son deterministas.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
}

/// Bandas de aurora: (centro vertical, amplitud, frecuencia, fase, anchura), en fracciones de h.
const BANDS: [(f32, f32, f32, f32, f32); 3] =
    [(0.30, 0.07, 1.3, 0.4, 0.16), (0.42, 0.06, 1.9, 2.1, 0.12), (0.22, 0.05, 2.6, 4.0, 0.10)];

/// Cielo: degradado por filas + halos + (opcional) bandas de aurora, en coma flotante y con tramado.
fn paint_sky(s: &mut Surface, stops: &[(f32, Rgb)], glows: &[Glow], aurora: bool) {
    let (w, h) = (s.w, s.h);
    let (wf, hf) = (w as f32, h as f32);
    // Centro de cada banda en cada columna (solo la aurora lo usa).
    let mut yc: Vec<[f32; 3]> = Vec::new();
    if aurora {
        for x in 0..w {
            let u = (x as f32 + 0.5) / wf;
            yc.push(BANDS.map(|(c, a, f, p, _)| c + a * sin(u * 2.0 * PI * f + p) + 0.3 * a * sin(u * 2.0 * PI * f * 3.1 + p * 2.0)));
        }
    }
    for y in 0..h {
        let v = (y as f32 + 0.5) / hf;
        let base = ramp(stops, v);
        for x in 0..w {
            let mut c = base;
            for g in glows {
                let (dx, dy) = (x as f32 + 0.5 - g.cx, y as f32 + 0.5 - g.cy);
                let d2 = dx * dx + dy * dy;
                if d2 < g.r * g.r {
                    let f = 1.0 - sqrt(d2) / g.r;
                    let a = g.k * f * f;
                    for i in 0..3 {
                        c[i] += (g.col[i] - c[i]) * a;
                    }
                }
            }
            if aurora {
                let u = (x as f32 + 0.5) / wf;
                for (i, b) in BANDS.iter().enumerate() {
                    let dn = (yc[x as usize][i] - v) / b.4; // >0: por encima del centro de la banda
                    if dn > -1.0 && dn < 1.0 {
                        let q = 1.0 - dn * dn;
                        // Rayos verticales suaves; más brillo hacia el borde inferior.
                        let rays = 0.86 + 0.14 * sin(u * 2.0 * PI * (38.0 + 11.0 * i as f32) + v * 7.0);
                        let a = 0.62 * q * q * rays * (0.65 + 0.35 * clamp01(0.5 - dn * 0.5));
                        // Verde abajo, violeta arriba.
                        let k = clamp01(0.5 + 0.5 * dn + 0.25 * i as f32 - 0.25);
                        let ac = [0.18 + 0.44 * k, 1.0 - 0.72 * k, 0.52 + 0.43 * k];
                        for j in 0..3 {
                            c[j] += (ac[j] * 255.0 - c[j]) * a;
                        }
                    }
                }
            }
            s.px[(y * w + x) as usize] = dither(c, x, y);
        }
    }
}

fn stars(s: &mut Surface, seed: u64, limit_y: f32) {
    let (w, h) = (s.w, s.h);
    let mut rng = Rng(seed);
    let n = (w as i64 * h as i64 / 1200).max(6);
    let ymax = ((h as f32 * limit_y) as u32).max(1);
    for _ in 0..n {
        let x = (rng.next() % w as u32) as i32;
        let y = (rng.next() % ymax) as i32;
        let a = 90 + rng.next() % 166;
        let big = rng.next() % 14 == 0 && w >= 400;
        s.blend_px(x, y, 0xEAF0FF, a);
        if big {
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                s.blend_px(x + dx, y + dy, 0xEAF0FF, a / 3);
            }
        }
    }
}

/// Silueta de colinas: borde superior suavizado y relleno hasta abajo.
fn hills(s: &mut Surface, base: f32, amp: f32, ph: f32, col: u32) {
    let (w, h) = (s.w, s.h);
    for x in 0..w {
        let u = (x as f32 + 0.5) / w as f32;
        let t = (base + amp * (0.6 * sin(u * 2.0 * PI * 1.3 + ph) + 0.4 * sin(u * 2.0 * PI * 3.1 + ph * 2.3))) * h as f32;
        let yi = floor(t);
        s.blend_px(x, yi as i32, col, ((1.0 - (t - yi)) * 255.0) as u32);
        s.fill_rect(Rect::new(x, yi as i32 + 1, 1, h), col);
    }
}

fn c3(r: f32, g: f32, b: f32) -> Rgb {
    [r * 255.0, g * 255.0, b * 255.0]
}

/// Genera el fondo `id` a tamaño w x h. El fondo 3 usa `accent`.
pub fn render_wallpaper(id: u8, w: i32, h: i32, accent: u32) -> Surface {
    let mut s = Surface::new(w, h, 0);
    if s.px.is_empty() {
        return s;
    }
    let (wf, hf) = (s.w as f32, s.h as f32);
    match id {
        0 => {
            let (mx, my, mr) = (wf * 0.80, hf * 0.20, hf * 0.065);
            let sky = [(0.0, c3(0.015, 0.025, 0.10)), (0.5, c3(0.06, 0.06, 0.25)), (0.8, c3(0.33, 0.14, 0.48)), (1.0, c3(0.22, 0.10, 0.36))];
            paint_sky(&mut s, &sky, &[Glow { cx: mx, cy: my, r: mr * 5.5, col: c3(0.55, 0.62, 1.0), k: 0.55 }], false);
            stars(&mut s, 0x4E46_5F31, 0.7);
            s.crescent(mx, my, mr, mx + mr * 0.5, my - mr * 0.35, mr * 0.85, 0xF6F1DC);
            hills(&mut s, 0.80, 0.045, 0.7, 0x2A1E52);
            hills(&mut s, 0.90, 0.035, 3.1, 0x0C0B1E);
        }
        1 => {
            let sky = [(0.0, c3(0.01, 0.05, 0.09)), (0.6, c3(0.02, 0.12, 0.14)), (1.0, c3(0.03, 0.09, 0.12))];
            paint_sky(&mut s, &sky, &[], true);
            stars(&mut s, 0x4E46_5F32, 0.6);
            hills(&mut s, 0.93, 0.03, 1.9, 0x07100F);
        }
        2 => {
            let (sx, sy, sr) = (wf * 0.32, hf * 0.79, hf * 0.05);
            let sky = [
                (0.0, c3(0.04, 0.08, 0.32)),
                (0.45, c3(0.22, 0.18, 0.55)),
                (0.72, c3(0.85, 0.40, 0.52)),
                (0.86, c3(1.0, 0.58, 0.30)),
                (1.0, c3(1.0, 0.70, 0.36)),
            ];
            paint_sky(&mut s, &sky, &[Glow { cx: sx, cy: sy, r: hf * 0.6, col: c3(1.0, 0.78, 0.45), k: 0.75 }], false);
            s.fill_circle(sx, sy, sr, 0xFFEBB8, 255);
            hills(&mut s, 0.84, 0.04, 0.4, 0x3A2450);
            hills(&mut s, 0.92, 0.03, 2.6, 0x1A1030);
        }
        _ => s.fill_rect(Rect::new(0, 0, w, h), darken(accent, 120)),
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallpapers_size_deterministic_small() {
        for id in 0..WALLPAPERS.len() as u8 {
            for (w, h) in [(1, 1), (16, 9), (128, 72), (320, 200)] {
                let a = render_wallpaper(id, w, h, ACCENTS[0]);
                let b = render_wallpaper(id, w, h, ACCENTS[0]);
                assert_eq!((a.w, a.h), (w, h));
                assert_eq!(a.px.len(), (w * h) as usize);
                assert_eq!(a.px, b.px, "fondo {id} no determinista");
            }
        }
    }

    #[test]
    fn solid_uses_accent() {
        let s = render_wallpaper(3, 4, 4, 0x1877D4);
        assert!(s.px.iter().all(|&p| p == s.px[0]));
        assert_ne!(s.px[0], render_wallpaper(3, 4, 4, 0xD13B3B).px[0]);
    }
}
