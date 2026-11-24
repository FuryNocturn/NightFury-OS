// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Pintado de la lista de dibujo y consultas sobre ella (enlaces, selección de texto, anclas).
use crate::estilo::{Lista, DECO_SUBRAYA, DECO_TACHA};
use crate::maqueta::{EstadoImg, Item, Texto};
use crate::{Documento, Fuentes};
use alloc::string::String;
use nf_core::geom::Rect;
use nf_core::surface::Surface;

fn arriba(t: &Texto) -> i32 {
    t.y - t.asc
}

fn abajo(t: &Texto) -> i32 {
    t.y + t.desc
}

impl Documento {
    /// Pinta la parte visible (el recorte de `s`) desplazada `scroll_y` px.
    pub fn pintar(&self, s: &mut Surface, scroll_y: i32, f: &mut dyn Fuentes) {
        let Some(res) = &self.res else { return };
        let clip = s.clip();
        let (v0, v1) = (scroll_y.saturating_add(clip.y), scroll_y.saturating_add(clip.bottom()));
        for it in &res.items {
            match it {
                Item::Caja { r, fondo, borde, color } => {
                    if r.bottom() < v0 || r.y > v1 {
                        continue;
                    }
                    let r = Rect::new(r.x, r.y - scroll_y, r.w, r.h);
                    if let Some(c) = fondo {
                        s.fill_rect(r, *c);
                    }
                    let [bt, br, bb, bl] = *borde;
                    if bt > 0 {
                        s.fill_rect(Rect::new(r.x, r.y, r.w, bt), color[0]);
                    }
                    if bb > 0 {
                        s.fill_rect(Rect::new(r.x, r.bottom() - bb, r.w, bb), color[2]);
                    }
                    if bl > 0 {
                        s.fill_rect(Rect::new(r.x, r.y + bt, bl, r.h - bt - bb), color[3]);
                    }
                    if br > 0 {
                        s.fill_rect(Rect::new(r.right() - br, r.y + bt, br, r.h - bt - bb), color[1]);
                    }
                }
                Item::Texto(t) => {
                    if abajo(t) + 4 < v0 || arriba(t) > v1 {
                        continue;
                    }
                    let y = (t.y - scroll_y) as f32;
                    f.dibujar(s, t.letra, t.px, t.x as f32, y, t.color, &t.s);
                    let grosor = ((t.px / 14.0) as i32).max(1);
                    if t.deco & DECO_SUBRAYA != 0 {
                        s.fill_rect(Rect::new(t.x, t.y - scroll_y + (t.desc / 3).max(1), t.w, grosor), t.color);
                    }
                    if t.deco & DECO_TACHA != 0 {
                        s.fill_rect(Rect::new(t.x, t.y - scroll_y - t.asc * 3 / 10, t.w, grosor), t.color);
                    }
                }
                Item::Imagen { r, img, .. } => {
                    if r.bottom() < v0 || r.y > v1 {
                        continue;
                    }
                    if let Some(EstadoImg::Lista(p)) = self.imgs.get(*img as usize).map(|i| &i.estado) {
                        nf_core::img::draw_scaled(p, s, r.x, r.y - scroll_y, r.w, r.h);
                    }
                }
                Item::Vineta { cx, cy, r, tipo, color } => {
                    if cy + r < v0 || cy - r > v1 {
                        continue;
                    }
                    let cy = cy - scroll_y;
                    match tipo {
                        Lista::Circulo => s.arc(*cx as f32, cy as f32, *r as f32 - 0.5, 0.0, 360.0, 1.0, *color),
                        Lista::Cuadrado => s.fill_rect(Rect::new(cx - r, cy - r, 2 * r, 2 * r), *color),
                        _ => s.fill_circle(*cx as f32, cy as f32, *r as f32, *color, 255),
                    }
                }
            }
        }
    }

    /// URL absoluta (con su `#fragmento`, si lo tiene) del enlace bajo el punto, en coordenadas del documento.
    pub fn enlace_en(&self, x: i32, y_doc: i32) -> Option<String> {
        let res = self.res.as_ref()?;
        for it in res.items.iter().rev() {
            let enlace = match it {
                Item::Texto(t) if t.x <= x && x < t.x + t.w && arriba(t) <= y_doc && y_doc < abajo(t) => t.enlace,
                Item::Imagen { r, enlace, .. } if r.contains(x, y_doc) => *enlace,
                _ => continue,
            };
            if enlace != 0 {
                return res.enlaces.get(enlace as usize).cloned();
            }
        }
        None
    }

    /// Texto entre dos puntos del documento (el de arrastre al seleccionar), en orden de lectura.
    // shortcut: los saltos de línea del párrafo se copian como saltos; el corte dentro de un trozo se
    // reparte en proporción a su ancho (sin medir letra a letra). Afinar si hace falta precisión.
    pub fn texto_en(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> String {
        let Some(res) = &self.res else { return String::new() };
        let ((ax, ay), (bx, by)) = if (y0, x0) <= (y1, x1) { ((x0, y0), (x1, y1)) } else { ((x1, y1), (x0, y0)) };
        let textos: alloc::vec::Vec<&Texto> = res.items.iter().filter_map(|i| if let Item::Texto(t) = i { Some(t) } else { None }).collect();
        // primer trozo que no queda del todo antes del punto A y último que no queda del todo después del B
        let antes = |t: &Texto| abajo(t) <= ay || (ay >= arriba(t) && t.x + t.w <= ax);
        let despues = |t: &Texto| arriba(t) > by || (by < abajo(t) && t.x >= bx);
        let Some(ini) = textos.iter().position(|t| !antes(t)) else { return String::new() };
        let Some(fin) = textos.iter().rposition(|t| !despues(t)) else { return String::new() };
        if ini > fin {
            return String::new();
        }
        // posición (en letras) del punto dentro de un trozo
        let corte = |t: &Texto, px: i32, py: i32| -> Option<usize> {
            if py < arriba(t) || py >= abajo(t) || t.w <= 0 {
                return None;
            }
            let n = t.s.chars().count() as i64;
            Some(((px - t.x).clamp(0, t.w) as i64 * n / t.w as i64) as usize)
        };
        let mut r = String::new();
        for k in ini..=fin {
            let t = textos[k];
            let n = t.s.chars().count();
            let desde = if k == ini { corte(t, ax, ay).unwrap_or(0) } else { 0 };
            let hasta = if k == fin { corte(t, bx, by).unwrap_or(n) } else { n };
            if hasta > desde {
                if k > ini {
                    let p = textos[k - 1];
                    if t.y != p.y {
                        while r.ends_with(' ') {
                            r.pop();
                        }
                        r.push('\n');
                    } else if t.x - (p.x + p.w) > 2 && !r.ends_with(' ') {
                        r.push(' ');
                    }
                }
                r.extend(t.s.chars().skip(desde).take(hasta - desde));
            }
        }
        r
    }

    /// Posición vertical del elemento con ese `id` (o `<a name>`), en coordenadas del documento.
    pub fn ancla(&self, id: &str) -> Option<i32> {
        self.res.as_ref()?.anclas.iter().find(|(k, _)| k == id).map(|&(_, y)| y)
    }
}
