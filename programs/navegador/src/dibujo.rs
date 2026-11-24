// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Iconos y formas de la ventana, dibujados por código (sin tipografías ni imágenes).
use nf_core::geom::Rect;
use nf_core::surface::Surface;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Ico {
    Atras,
    Adelante,
    Recargar,
    Detener,
    Cerrar,
    Mas,
    Candado,
    Aviso,
    Lupa,
    Globo,
}

/// Rectángulo con las esquinas redondeadas (radio 3).
pub fn caja(s: &mut Surface, r: Rect, c: u32) {
    s.fill_rect(Rect::new(r.x + 2, r.y, r.w - 4, r.h), c);
    s.fill_rect(Rect::new(r.x + 1, r.y + 1, r.w - 2, r.h - 2), c);
    s.fill_rect(Rect::new(r.x, r.y + 2, r.w, r.h - 4), c);
}

/// Cápsula (extremos redondos).
pub fn capsula(s: &mut Surface, r: Rect, c: u32) {
    let rad = r.h as f32 / 2.0;
    s.fill_rect(Rect::new(r.x + r.h / 2, r.y, r.w - r.h, r.h), c);
    s.fill_circle(r.x as f32 + rad, r.y as f32 + rad, rad, c, 255);
    s.fill_circle(r.right() as f32 - rad, r.y as f32 + rad, rad, c, 255);
}

/// Dibuja un icono de unos 16 px centrado en (`x`, `y`).
pub fn icono(s: &mut Surface, i: Ico, x: f32, y: f32, c: u32) {
    match i {
        Ico::Atras | Ico::Adelante => {
            let d = if i == Ico::Atras { -1.0 } else { 1.0 };
            s.stroke(&[(x - 6.0 * d, y), (x + 6.0 * d, y)], 2.0, c);
            s.stroke(&[(x + 0.5 * d, y - 6.0), (x + 6.5 * d, y), (x + 0.5 * d, y + 6.0)], 2.0, c);
        }
        Ico::Recargar => {
            s.arc(x, y, 6.0, 40.0, 330.0, 2.0, c);
            s.stroke(&[(x + 9.0, y - 5.0), (x + 4.6, y - 3.8), (x + 5.5, y - 8.5)], 2.0, c);
        }
        Ico::Detener => {
            s.stroke(&[(x - 5.0, y - 5.0), (x + 5.0, y + 5.0)], 2.0, c);
            s.stroke(&[(x - 5.0, y + 5.0), (x + 5.0, y - 5.0)], 2.0, c);
        }
        Ico::Cerrar => {
            s.stroke(&[(x - 3.5, y - 3.5), (x + 3.5, y + 3.5)], 1.6, c);
            s.stroke(&[(x - 3.5, y + 3.5), (x + 3.5, y - 3.5)], 1.6, c);
        }
        Ico::Mas => {
            s.stroke(&[(x - 6.0, y), (x + 6.0, y)], 2.0, c);
            s.stroke(&[(x, y - 6.0), (x, y + 6.0)], 2.0, c);
        }
        Ico::Candado => {
            s.arc(x, y - 1.0, 3.4, 0.0, 180.0, 1.6, c);
            s.stroke(&[(x - 3.4, y - 1.0), (x - 3.4, y + 1.0)], 1.6, c);
            s.stroke(&[(x + 3.4, y - 1.0), (x + 3.4, y + 1.0)], 1.6, c);
            caja(s, Rect::new(x as i32 - 5, y as i32, 10, 7), c);
        }
        Ico::Aviso => {
            s.stroke(&[(x, y - 6.5), (x + 6.5, y + 5.0), (x - 6.5, y + 5.0), (x, y - 6.5)], 1.6, c);
            s.stroke(&[(x, y - 2.5), (x, y + 1.0)], 1.6, c);
            s.fill_circle(x, y + 3.0, 0.9, c, 255);
        }
        Ico::Lupa => {
            s.arc(x - 1.5, y - 1.5, 5.5, 0.0, 360.0, 1.8, c);
            s.stroke(&[(x + 2.6, y + 2.6), (x + 7.0, y + 7.0)], 2.0, c);
        }
        Ico::Globo => {
            s.arc(x, y, 6.0, 0.0, 360.0, 1.3, c);
            s.stroke(&[(x - 6.0, y), (x + 6.0, y)], 1.2, c);
            s.stroke(&[(x, y - 6.0), (x, y + 6.0)], 1.2, c);
        }
    }
}
