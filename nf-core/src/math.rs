// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Matemáticas mínimas: `core` no trae raíz, seno ni redondeo.

pub const PI: f32 = 3.141_592_7;

pub fn floor(x: f32) -> f32 {
    let i = x as i32 as f32;
    if i > x { i - 1.0 } else { i }
}

pub fn ceil(x: f32) -> f32 {
    let i = x as i32 as f32;
    if i < x { i + 1.0 } else { i }
}

pub fn iround(x: f32) -> i32 {
    floor(x + 0.5) as i32
}

pub fn clamp01(x: f32) -> f32 {
    if x < 0.0 { 0.0 } else if x > 1.0 { 1.0 } else { x }
}

/// Raíz cuadrada por Newton (error relativo < 1e-6).
pub fn sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut g = f32::from_bits(0x1fbd_1df5 + (x.to_bits() >> 1));
    for _ in 0..3 {
        g = 0.5 * (g + x / g);
    }
    g
}

pub fn sqrt64(x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut g = f64::from_bits((x.to_bits() >> 1) + 0x1ff8_0000_0000_0000);
    for _ in 0..6 {
        g = 0.5 * (g + x / g);
    }
    g
}

pub fn sin(x: f32) -> f32 {
    let mut x = x - 2.0 * PI * floor(x / (2.0 * PI) + 0.5);
    if x > PI / 2.0 {
        x = PI - x;
    } else if x < -PI / 2.0 {
        x = -PI - x;
    }
    let x2 = x * x;
    x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0 * (1.0 - x2 / 72.0))))
}

pub fn cos(x: f32) -> f32 {
    sin(x + PI / 2.0)
}

/// Distancia del punto `p` al segmento `a`-`b`.
pub fn seg_dist(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let dx = bx - ax;
    let dy = by - ay;
    let l2 = dx * dx + dy * dy;
    let t = if l2 <= 1e-9 { 0.0 } else { clamp01(((px - ax) * dx + (py - ay) * dy) / l2) };
    let cx = ax + t * dx - px;
    let cy = ay + t * dy - py;
    sqrt(cx * cx + cy * cy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqrt_ok() {
        for v in [0.25f32, 1.0, 2.0, 9.0, 12345.0] {
            assert!((sqrt(v) - v.sqrt()).abs() < 1e-3 * v.sqrt().max(1.0));
        }
        assert!((sqrt64(2.0) - 2f64.sqrt()).abs() < 1e-12);
        assert!((sqrt64(1e10) - 1e5).abs() < 1e-6);
    }

    #[test]
    fn trig_ok() {
        let mut a = -10.0f32;
        while a < 10.0 {
            assert!((sin(a) - a.sin()).abs() < 1e-3, "sin {a}");
            assert!((cos(a) - a.cos()).abs() < 1e-3, "cos {a}");
            a += 0.37;
        }
    }

    #[test]
    fn rounding() {
        assert_eq!(floor(-0.5), -1.0);
        assert_eq!(ceil(1.2), 2.0);
        assert_eq!(iround(2.5), 3);
        assert_eq!(iround(-1.4), -1);
    }
}
