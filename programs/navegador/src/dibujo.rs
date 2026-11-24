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
    Estrella,
    EstrellaLlena,
    Menu,
    Carpeta,
    Abajo,
    Incognito,
}

/// Direcciones (cada 36°, desde arriba) de las puntas (pares) y los huecos (impares) de la estrella.
const ESTRELLA: [(f32, f32); 10] = [(0.0, -1.0), (0.588, -0.809), (0.951, -0.309), (0.951, 0.309), (0.588, 0.809), (0.0, 1.0), (-0.588, 0.809), (-0.951, 0.309), (-0.951, -0.309), (-0.588, -0.809)];

/// Contorno cerrado de la estrella de radio `r` (los huecos, a 0,42 de `r`).
fn estrella(x: f32, y: f32, r: f32) -> [(f32, f32); 11] {
    let mut p = [(x, y); 11];
    for (i, (dx, dy)) in ESTRELLA.iter().enumerate() {
        let k = if i % 2 == 0 { r } else { r * 0.42 };
        p[i] = (x + dx * k, y + dy * k);
    }
    p[10] = p[0];
    p
}

/// Color de un sitio (siempre el mismo para el mismo host) y su inicial, para el círculo de favoritos.
pub fn color_y_letra(host: &str, titulo: &str) -> (u32, char) {
    const COLORES: [u32; 8] = [0xD93025, 0xE8710A, 0xC77700, 0x188038, 0x129EAF, 0x1A73E8, 0x9334E6, 0xD01884];
    let base = host.strip_prefix("www.").unwrap_or(host);
    let fuente = if base.is_empty() { titulo } else { base };
    let letra = fuente.chars().find(|c| c.is_alphanumeric()).map_or('?', |c| c.to_uppercase().next().unwrap_or(c));
    let h = fuente.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    (COLORES[h as usize % COLORES.len()], letra)
}

/// Círculo de color con la inicial del sitio (no hay favicons).
pub fn disco_inicial(s: &mut Surface, cx: f32, cy: f32, r: f32, host: &str, titulo: &str) {
    let (c, l) = color_y_letra(host, titulo);
    s.fill_circle(cx, cy, r, c, 255);
    let t = alloc::string::String::from(l);
    let (x, y, d) = ((cx - r) as i32, (cy - r) as i32, (r * 2.0) as i32);
    nf_core::font::draw_in(s, Rect::new(x, y, d, d), &t, if r >= 16.0 { 12 } else { 8 }, true, 0xFFFFFF, nf_core::font::Align::Center);
}

/// Parte `texto` en líneas de como mucho `ancho` px (por palabras).
pub fn envolver(texto: &str, size: u32, ancho: i32) -> alloc::vec::Vec<alloc::string::String> {
    let mut v: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
    for p in texto.split_whitespace() {
        match v.last_mut() {
            Some(l) if nf_core::font::width(&alloc::format!("{l} {p}"), size, false) <= ancho => {
                l.push(' ');
                l.push_str(p);
            }
            _ => v.push(alloc::string::String::from(p)),
        }
    }
    v
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
        Ico::Estrella | Ico::EstrellaLlena => {
            let p = estrella(x, y + 0.5, 8.0);
            s.stroke(&p, 1.5, c);
            if i == Ico::EstrellaLlena {
                // Relleno: radios gruesos desde el centro a cada vértice.
                for v in &p[..10] {
                    s.stroke(&[(x, y + 0.5), (v.0 * 0.9 + x * 0.1, v.1 * 0.9 + (y + 0.5) * 0.1)], 3.0, c);
                }
            }
        }
        Ico::Menu => {
            for k in [-5.0, 0.0, 5.0] {
                s.fill_circle(x, y + k, 1.7, c, 255);
            }
        }
        Ico::Carpeta => {
            caja(s, Rect::new(x as i32 - 7, y as i32 - 4, 15, 10), c);
            caja(s, Rect::new(x as i32 - 7, y as i32 - 6, 6, 4), c);
        }
        Ico::Abajo => s.stroke(&[(x - 3.5, y - 1.5), (x, y + 2.0), (x + 3.5, y - 1.5)], 1.5, c),
        Ico::Incognito => {
            // Sombrero y gafas.
            s.stroke(&[(x - 8.0, y - 1.0), (x + 8.0, y - 1.0)], 1.8, c);
            s.stroke(&[(x - 4.5, y - 1.0), (x - 3.0, y - 7.0), (x + 3.0, y - 7.0), (x + 4.5, y - 1.0)], 1.6, c);
            s.arc(x - 3.8, y + 4.0, 2.8, 0.0, 360.0, 1.5, c);
            s.arc(x + 3.8, y + 4.0, 2.8, 0.0, 360.0, 1.5, c);
        }
    }
}
