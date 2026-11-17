// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Superficie de píxeles 0x00RRGGBB con recorte y primitivas de dibujo suavizadas.
//! Convención de coordenadas reales: el píxel (i, j) cubre [i, i+1) x [j, j+1); su centro es (i+0.5, j+0.5).
use crate::geom::Rect;
use crate::math::{ceil, clamp01, cos, floor, seg_dist, sin, sqrt, PI};
use alloc::vec;
use alloc::vec::Vec;

pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

/// Mezcla `src` sobre `dst` con opacidad `a` (0..=255).
pub fn blend(dst: u32, src: u32, a: u32) -> u32 {
    if a >= 255 {
        return src & 0xFF_FFFF;
    }
    if a == 0 {
        return dst;
    }
    let a = a + (a >> 7);
    let ia = 256 - a;
    let rb = (((src & 0xFF00FF) * a + (dst & 0xFF00FF) * ia) >> 8) & 0xFF00FF;
    let g = (((src & 0x00FF00) * a + (dst & 0x00FF00) * ia) >> 8) & 0x00FF00;
    rb | g
}

pub fn lighten(c: u32, amount: u32) -> u32 {
    blend(c, 0xFFFFFF, amount)
}

pub fn darken(c: u32, amount: u32) -> u32 {
    blend(c, 0, amount)
}

pub struct Surface {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
    clip: Rect,
}

impl Surface {
    pub fn new(w: i32, h: i32, color: u32) -> Self {
        let w = w.max(0);
        let h = h.max(0);
        Surface { w, h, px: vec![color; (w * h) as usize], clip: Rect::new(0, 0, w, h) }
    }

    /// Superficie vacía, sin reservar memoria.
    pub fn empty() -> Self {
        Surface { w: 0, h: 0, px: Vec::new(), clip: Rect::default() }
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.w, self.h)
    }
    pub fn clip(&self) -> Rect {
        self.clip
    }
    /// Limita todo el dibujo posterior a `r` (recortado a la superficie).
    pub fn set_clip(&mut self, r: Rect) {
        self.clip = r.intersect(self.bounds());
    }
    pub fn reset_clip(&mut self) {
        self.clip = self.bounds();
    }

    pub fn fill_rect(&mut self, r: Rect, c: u32) {
        let r = r.intersect(self.clip);
        if r.is_empty() {
            return;
        }
        for y in r.y..r.bottom() {
            let o = (y * self.w + r.x) as usize;
            self.px[o..o + r.w as usize].fill(c);
        }
    }

    pub fn blend_rect(&mut self, r: Rect, c: u32, a: u32) {
        let r = r.intersect(self.clip);
        if r.is_empty() {
            return;
        }
        for y in r.y..r.bottom() {
            let o = (y * self.w + r.x) as usize;
            for p in &mut self.px[o..o + r.w as usize] {
                *p = blend(*p, c, a);
            }
        }
    }

    /// Contorno de 1 píxel por dentro de `r`.
    pub fn frame(&mut self, r: Rect, c: u32) {
        self.fill_rect(Rect::new(r.x, r.y, r.w, 1), c);
        self.fill_rect(Rect::new(r.x, r.bottom() - 1, r.w, 1), c);
        self.fill_rect(Rect::new(r.x, r.y, 1, r.h), c);
        self.fill_rect(Rect::new(r.right() - 1, r.y, 1, r.h), c);
    }

    pub fn blend_px(&mut self, x: i32, y: i32, c: u32, a: u32) {
        if self.clip.contains(x, y) {
            let o = (y * self.w + x) as usize;
            self.px[o] = blend(self.px[o], c, a);
        }
    }

    /// Copia opaca de `src` con su esquina en (dx, dy).
    pub fn blit(&mut self, src: &Surface, dx: i32, dy: i32) {
        let r = Rect::new(dx, dy, src.w, src.h).intersect(self.clip);
        if r.is_empty() {
            return;
        }
        for y in r.y..r.bottom() {
            let so = ((y - dy) * src.w + (r.x - dx)) as usize;
            let o = (y * self.w + r.x) as usize;
            self.px[o..o + r.w as usize].copy_from_slice(&src.px[so..so + r.w as usize]);
        }
    }

    /// Degradado vertical de `top` a `bottom` dentro de `r`.
    pub fn vgradient(&mut self, r: Rect, top: u32, bottom: u32) {
        for i in 0..r.h {
            let a = if r.h > 1 { (i * 255 / (r.h - 1)) as u32 } else { 0 };
            self.fill_rect(Rect::new(r.x, r.y + i, r.w, 1), blend(top, bottom, a));
        }
    }

    /// Disco suavizado. `alpha` es la opacidad máxima (0..=255).
    pub fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, c: u32, alpha: u32) {
        let b = Rect::new(floor(cx - r - 1.0) as i32, floor(cy - r - 1.0) as i32, ceil(2.0 * r + 3.0) as i32, ceil(2.0 * r + 3.0) as i32)
            .intersect(self.clip);
        for y in b.y..b.bottom() {
            for x in b.x..b.right() {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let cov = clamp01(r + 0.5 - sqrt(dx * dx + dy * dy));
                if cov > 0.0 {
                    let o = (y * self.w + x) as usize;
                    self.px[o] = blend(self.px[o], c, (cov * alpha as f32) as u32);
                }
            }
        }
    }

    /// Disco (cx, cy, r) al que se le resta el disco (ox, oy, or): una luna creciente.
    #[allow(clippy::too_many_arguments)]
    pub fn crescent(&mut self, cx: f32, cy: f32, r: f32, ox: f32, oy: f32, or: f32, c: u32) {
        let b = Rect::new(floor(cx - r - 1.0) as i32, floor(cy - r - 1.0) as i32, ceil(2.0 * r + 3.0) as i32, ceil(2.0 * r + 3.0) as i32)
            .intersect(self.clip);
        for y in b.y..b.bottom() {
            for x in b.x..b.right() {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let c1 = clamp01(r + 0.5 - sqrt((fx - cx) * (fx - cx) + (fy - cy) * (fy - cy)));
                let c2 = clamp01(or + 0.5 - sqrt((fx - ox) * (fx - ox) + (fy - oy) * (fy - oy)));
                let cov = c1 * (1.0 - c2);
                if cov > 0.0 {
                    let o = (y * self.w + x) as usize;
                    self.px[o] = blend(self.px[o], c, (cov * 255.0) as u32);
                }
            }
        }
    }

    /// Línea quebrada suavizada de grosor `t` píxeles.
    pub fn stroke(&mut self, pts: &[(f32, f32)], t: f32, c: u32) {
        let half = t / 2.0;
        for s in pts.windows(2) {
            let (ax, ay) = s[0];
            let (bx, by) = s[1];
            let x0 = floor(ax.min(bx) - half - 1.0) as i32;
            let y0 = floor(ay.min(by) - half - 1.0) as i32;
            let x1 = ceil(ax.max(bx) + half + 1.0) as i32;
            let y1 = ceil(ay.max(by) + half + 1.0) as i32;
            let b = Rect::new(x0, y0, x1 - x0, y1 - y0).intersect(self.clip);
            for y in b.y..b.bottom() {
                for x in b.x..b.right() {
                    let d = seg_dist(x as f32 + 0.5, y as f32 + 0.5, ax, ay, bx, by);
                    let cov = clamp01(half + 0.5 - d);
                    if cov > 0.0 {
                        let o = (y * self.w + x) as usize;
                        self.px[o] = blend(self.px[o], c, (cov * 255.0) as u32);
                    }
                }
            }
        }
    }

    /// Arco de circunferencia de `a0` a `a1` grados (antihorario, 0 = derecha, 90 = arriba).
    #[allow(clippy::too_many_arguments)]
    pub fn arc(&mut self, cx: f32, cy: f32, r: f32, a0: f32, a1: f32, t: f32, c: u32) {
        let n = ((a1 - a0).abs() / 360.0 * 2.0 * PI * r / 2.0) as usize + 8;
        let mut pts = Vec::with_capacity(n + 1);
        for i in 0..=n {
            let a = (a0 + (a1 - a0) * i as f32 / n as f32) * PI / 180.0;
            pts.push((cx + r * cos(a), cy - r * sin(a)));
        }
        self.stroke(&pts, t, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_limits() {
        assert_eq!(blend(0x102030, 0xFFFFFF, 255), 0xFFFFFF);
        assert_eq!(blend(0x102030, 0xFFFFFF, 0), 0x102030);
        let m = blend(0x000000, 0xFFFFFF, 128);
        assert!((m & 0xFF) > 0x70 && (m & 0xFF) < 0x90);
    }

    #[test]
    fn clipping_never_panics() {
        let mut s = Surface::new(20, 10, 0);
        s.fill_rect(Rect::new(-5, -5, 100, 100), 0x112233);
        assert!(s.px.iter().all(|&p| p == 0x112233));
        s.set_clip(Rect::new(2, 2, 4, 4));
        s.fill_rect(Rect::new(-50, -50, 200, 200), 0xFF);
        assert_eq!(s.px[0], 0x112233);
        assert_eq!(s.px[(2 * 20 + 2) as usize], 0xFF);
        s.reset_clip();
        s.fill_circle(19.0, 9.0, 30.0, 0xFFFFFF, 255);
        s.stroke(&[(-10.0, -10.0), (40.0, 30.0)], 3.0, 0xFF0000);
        s.arc(10.0, 5.0, 40.0, 0.0, 360.0, 2.0, 0x00FF00);
        let src = Surface::new(8, 8, 0xABCDEF);
        s.blit(&src, 16, 6);
        s.blit(&src, -4, -4);
        assert_eq!(s.px[0], 0xABCDEF);
    }
}
