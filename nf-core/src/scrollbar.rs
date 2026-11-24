// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Barra de desplazamiento compartida (v0.4.0). Ver docs/PLAN-cinta.md.
//!
//! La barra solo guarda el estado de la interacción; la geometría y la posición las pasa la app
//! en cada llamada con `Metrics`. Si `total <= page` no hay nada que desplazar: las apps no la
//! dibujan ni le pasan eventos.
use crate::apps::MouseEv;
use crate::geom::Rect;
use crate::surface::{darken, Surface};
use crate::theme::Theme;

/// Ancho (o alto, si es horizontal) de la barra.
pub const SB_W: i32 = 17;
const MIN_THUMB: i32 = 24;
/// Primera repetición al mantener pulsado, y las siguientes.
const REPEAT_FIRST_MS: u64 = 400;
const REPEAT_MS: u64 = 50;

/// Geometría y posición, en las unidades de la app (líneas o píxeles).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Metrics {
    /// Dónde está la barra (alto libre para vertical, ancho libre para horizontal).
    pub rect: Rect,
    /// Tamaño total del contenido.
    pub total: i32,
    /// Tamaño visible.
    pub page: i32,
    /// Posición actual (primer elemento visible).
    pub pos: i32,
    /// Paso de una flecha.
    pub line: i32,
}

impl Metrics {
    /// Posición máxima: `max(total - page, 0)`.
    pub fn max(&self) -> i32 {
        (self.total - self.page).max(0)
    }
    /// ¿Hace falta barra?
    pub fn needed(&self) -> bool {
        self.total > self.page
    }
    fn clamp(&self, p: i32) -> i32 {
        p.clamp(0, self.max())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Part {
    Back,
    Fwd,
    Thumb,
    PageBack,
    PageFwd,
}

/// Resultado de atender un evento.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SbEvent {
    /// Nueva posición, si ha cambiado.
    pub pos: Option<i32>,
    /// Hay que volver a dibujar.
    pub redraw: bool,
    /// El evento era de la barra: la app no debe tratarlo.
    pub captured: bool,
}

pub struct ScrollBar {
    horizontal: bool,
    hover: Option<Part>,
    /// Arrastrando el tirador: distancia del agarre al inicio del tirador.
    drag: Option<i32>,
    /// Flecha o pista mantenida y momento de la próxima repetición.
    hold: Option<(Part, u64)>,
    /// Última posición del ratón a lo largo de la barra (para la pista mantenida).
    along: i32,
}

impl ScrollBar {
    pub const fn new(horizontal: bool) -> ScrollBar {
        ScrollBar { horizontal, hover: None, drag: None, hold: None, along: 0 }
    }

    /// Arrastrando el tirador o manteniendo una flecha o la pista.
    pub fn active(&self) -> bool {
        self.drag.is_some() || self.hold.is_some()
    }

    /// (inicio, largo) de la barra sobre su eje.
    fn axis(&self, m: &Metrics) -> (i32, i32) {
        if self.horizontal {
            (m.rect.x, m.rect.w)
        } else {
            (m.rect.y, m.rect.h)
        }
    }

    /// Largo de cada botón de flecha.
    fn btn(&self, m: &Metrics) -> i32 {
        let (_, len) = self.axis(m);
        SB_W.min(len / 2).max(0)
    }

    /// Pista entre botones, relativa al inicio de la barra: (inicio, largo).
    fn track(&self, m: &Metrics) -> (i32, i32) {
        let (_, len) = self.axis(m);
        let b = self.btn(m);
        (b, (len - 2 * b).max(0))
    }

    /// Tirador relativo al inicio de la barra: (inicio, largo).
    pub fn thumb(&self, m: &Metrics) -> (i32, i32) {
        let (ts, tl) = self.track(m);
        let total = m.total.max(1) as i64;
        let len = ((tl as i64 * m.page.max(0) as i64 / total) as i32).max(MIN_THUMB).min(tl);
        let max = m.max();
        let room = tl - len;
        let off = if max == 0 { 0 } else { (room as i64 * m.clamp(m.pos) as i64 / max as i64) as i32 };
        (ts + off, len)
    }

    fn part_at(&self, m: &Metrics, x: i32, y: i32) -> Option<Part> {
        if !m.rect.contains(x, y) {
            return None;
        }
        Some(self.part_along(m, self.along_of(m, x, y)))
    }

    fn part_along(&self, m: &Metrics, a: i32) -> Part {
        let (_, len) = self.axis(m);
        let b = self.btn(m);
        let (ts, tl) = self.thumb(m);
        if a < b {
            Part::Back
        } else if a >= len - b {
            Part::Fwd
        } else if a < ts {
            Part::PageBack
        } else if a >= ts + tl {
            Part::PageFwd
        } else {
            Part::Thumb
        }
    }

    fn step(&self, m: &Metrics, p: Part) -> i32 {
        let line = m.line.max(1);
        let page = m.page.max(1);
        m.clamp(match p {
            Part::Back => m.pos - line,
            Part::Fwd => m.pos + line,
            Part::PageBack => m.pos - page,
            Part::PageFwd => m.pos + page,
            Part::Thumb => m.pos,
        })
    }

    /// Posición que corresponde a tener el ratón en `a` arrastrando el tirador.
    fn drag_pos(&self, m: &Metrics, a: i32, grab: i32) -> i32 {
        let (ts, tl) = self.track(m);
        let (_, len) = self.thumb(m);
        let room = (tl - len).max(0);
        if room == 0 {
            return 0;
        }
        let off = (a - grab - ts).clamp(0, room);
        m.clamp(((off as i64 * m.max() as i64 + room as i64 / 2) / room as i64) as i32)
    }

    fn along_of(&self, m: &Metrics, x: i32, y: i32) -> i32 {
        let (start, _) = self.axis(m);
        (if self.horizontal { x } else { y }) - start
    }

    fn reset(&mut self) -> bool {
        let was = self.hover.is_some() || self.active();
        self.hover = None;
        self.drag = None;
        self.hold = None;
        was
    }

    fn moved(&self, m: &Metrics, p: i32) -> Option<i32> {
        (p != m.pos).then_some(p)
    }

    pub fn mouse(&mut self, ev: MouseEv, m: &Metrics, now_ms: u64) -> SbEvent {
        if !m.needed() {
            let redraw = self.reset();
            return SbEvent { redraw, ..SbEvent::default() };
        }
        let mut out = SbEvent::default();
        match ev {
            MouseEv::Down { x, y } => {
                let Some(p) = self.part_at(m, x, y) else { return out };
                let a = self.along_of(m, x, y);
                self.along = a;
                out.captured = true;
                out.redraw = true;
                if p == Part::Thumb {
                    self.drag = Some(a - self.thumb(m).0);
                } else {
                    out.pos = self.moved(m, self.step(m, p));
                    self.hold = Some((p, now_ms + REPEAT_FIRST_MS));
                }
            }
            MouseEv::Move { x, y } => {
                let a = self.along_of(m, x, y);
                self.along = a;
                if let Some(grab) = self.drag {
                    out.captured = true;
                    out.redraw = true;
                    out.pos = self.moved(m, self.drag_pos(m, a, grab));
                } else {
                    let h = self.part_at(m, x, y);
                    out.captured = h.is_some() || self.hold.is_some();
                    if h != self.hover {
                        self.hover = h;
                        out.redraw = true;
                    }
                }
            }
            MouseEv::Up { .. } => {
                if self.active() {
                    self.drag = None;
                    self.hold = None;
                    out.captured = true;
                    out.redraw = true;
                }
            }
            // El arrastre sigue si el ratón sale: el escritorio sigue enviando Move y Up hasta soltar.
            MouseEv::Leave => {
                if self.hover.take().is_some() {
                    out.redraw = true;
                }
            }
            MouseEv::RightDown { x, y } => out.captured = m.rect.contains(x, y),
            MouseEv::Wheel { .. } => {}
        }
        out
    }

    /// Repetición al mantener pulsada una flecha o la pista. Devuelve la nueva posición.
    pub fn tick(&mut self, m: &Metrics, now_ms: u64) -> Option<i32> {
        let (p, next) = self.hold?;
        if !m.needed() {
            self.hold = None;
            return None;
        }
        if now_ms < next {
            return None;
        }
        self.hold = Some((p, now_ms + REPEAT_MS));
        // La pista deja de repetirse cuando el tirador alcanza al ratón.
        if matches!(p, Part::PageBack | Part::PageFwd) && self.part_along(m, self.along) != p {
            return None;
        }
        self.moved(m, self.step(m, p))
    }

    pub fn draw(&self, s: &mut Surface, m: &Metrics, _theme: &Theme) {
        let r = m.rect;
        if r.is_empty() {
            return;
        }
        s.fill_rect(r, 0xF0F0F0);
        if !m.needed() {
            return;
        }
        let (start, len) = self.axis(m);
        let b = self.btn(m);
        let pressed = |p: Part| (p == Part::Thumb && self.drag.is_some()) || matches!(self.hold, Some((q, _)) if q == p);
        let shade = |base: u32, p: Part| {
            if pressed(p) {
                darken(base, 90)
            } else if self.hover == Some(p) {
                darken(base, 30)
            } else {
                base
            }
        };
        // Tramo [a, a+l) del eje, con margen `inset` a cada lado del eje cruzado.
        let seg = |a: i32, l: i32, inset: i32| {
            if self.horizontal {
                Rect::new(start + a, r.y + inset, l, r.h - 2 * inset)
            } else {
                Rect::new(r.x + inset, start + a, r.w - 2 * inset, l)
            }
        };
        for (p, a) in [(Part::Back, 0), (Part::Fwd, len - b)] {
            let face = seg(a, b, 0);
            if pressed(p) || self.hover == Some(p) {
                s.fill_rect(face, shade(0xE4E4E4, p));
            }
            self.arrow(s, face, p == Part::Back, if pressed(p) { 0x000000 } else { 0x505050 });
        }
        let (ts, tl) = self.thumb(m);
        if tl > 0 {
            s.fill_rect(seg(ts, tl, 2), shade(0xC0C0C0, Part::Thumb));
        }
    }

    /// Triángulo de flecha centrado en `b`, apuntando al inicio (`back`) o al final.
    fn arrow(&self, s: &mut Surface, b: Rect, back: bool, c: u32) {
        let (cx, cy) = (b.x + b.w / 2, b.y + b.h / 2);
        for i in 0..4 {
            // Fila i desde la punta: ancho 1, 3, 5, 7.
            let off = if back { i - 2 } else { 1 - i };
            let r = if self.horizontal {
                Rect::new(cx + off, cy - i, 1, 2 * i + 1)
            } else {
                Rect::new(cx - i, cy + off, 2 * i + 1, 1)
            };
            s.fill_rect(r, c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pos: i32) -> Metrics {
        // Barra vertical en x=100, y=0, alto 200; 100 líneas de las que se ven 20.
        Metrics { rect: Rect::new(100, 0, SB_W, 200), total: 100, page: 20, pos, line: 1 }
    }
    fn down(x: i32, y: i32) -> MouseEv {
        MouseEv::Down { x, y }
    }
    fn up() -> MouseEv {
        MouseEv::Up { x: 0, y: 0 }
    }

    #[test]
    fn flecha_mueve_una_linea() {
        let mut sb = ScrollBar::new(false);
        let e = sb.mouse(down(105, 195), &m(10), 0);
        assert!(e.captured && e.redraw);
        assert_eq!(e.pos, Some(11));
        let e = sb.mouse(up(), &m(11), 10);
        assert!(e.captured && !sb.active());
        assert_eq!(sb.mouse(down(105, 5), &m(10), 20).pos, Some(9));
    }

    #[test]
    fn pista_mueve_una_pagina() {
        let mut sb = ScrollBar::new(false);
        assert_eq!(sb.mouse(down(105, 150), &m(0), 0).pos, Some(20));
        sb.mouse(up(), &m(20), 1);
        assert_eq!(sb.mouse(down(105, 20), &m(80), 2).pos, Some(60));
    }

    #[test]
    fn limites() {
        let mut sb = ScrollBar::new(false);
        assert_eq!(sb.mouse(down(105, 5), &m(0), 0).pos, None);
        sb.mouse(up(), &m(0), 0);
        assert_eq!(sb.mouse(down(105, 195), &m(79), 0).pos, Some(80));
        sb.mouse(up(), &m(80), 0);
        assert_eq!(sb.mouse(down(105, 195), &m(80), 0).pos, None);
        // Una pos fuera de rango no rompe el dibujo.
        let (ts, tl) = sb.thumb(&m(5000));
        assert_eq!(ts + tl, 200 - SB_W);
    }

    #[test]
    fn arrastre_proporcional_y_sigue_fuera() {
        let mut sb = ScrollBar::new(false);
        let (ts, tl) = sb.thumb(&m(0));
        assert_eq!(ts, SB_W);
        assert!(tl >= MIN_THUMB);
        let e = sb.mouse(down(105, ts + 3), &m(0), 0);
        assert!(e.captured && e.pos.is_none() && sb.active());
        // Con el ratón muy fuera de la barra, el tirador llega al final y al principio.
        assert_eq!(sb.mouse(MouseEv::Move { x: 300, y: 5000 }, &m(0), 1).pos, Some(80));
        assert_eq!(sb.mouse(MouseEv::Move { x: -50, y: -50 }, &m(80), 2).pos, Some(0));
        // Leave no suelta; Up sí.
        sb.mouse(MouseEv::Leave, &m(0), 3);
        assert!(sb.active());
        sb.mouse(up(), &m(0), 4);
        assert!(!sb.active());
        // La mitad del recorrido útil es la mitad de max.
        let room = (200 - 2 * SB_W) - tl;
        sb.mouse(down(105, ts + 1), &m(0), 5);
        assert_eq!(sb.mouse(MouseEv::Move { x: 105, y: ts + 1 + room / 2 }, &m(0), 6).pos, Some(40));
    }

    #[test]
    fn repeticion_al_mantener() {
        let mut sb = ScrollBar::new(false);
        assert_eq!(sb.mouse(down(105, 195), &m(0), 1000).pos, Some(1));
        assert_eq!(sb.tick(&m(1), 1100), None);
        assert_eq!(sb.tick(&m(1), 1400), Some(2));
        assert_eq!(sb.tick(&m(2), 1430), None);
        assert_eq!(sb.tick(&m(2), 1450), Some(3));
        sb.mouse(up(), &m(3), 1460);
        assert_eq!(sb.tick(&m(3), 2000), None);
    }

    #[test]
    fn pista_se_para_al_llegar_el_tirador() {
        let mut sb = ScrollBar::new(false);
        assert_eq!(sb.mouse(down(105, 100), &m(0), 0).pos, Some(20));
        let (mut pos, mut t) = (20, 400);
        for _ in 0..20 {
            if let Some(p) = sb.tick(&m(pos), t) {
                pos = p;
            }
            t += 50;
        }
        // Se detiene cuando el tirador cubre el ratón, lejos del final (80).
        assert!(pos > 20 && pos < 80);
        let (ts, tl) = sb.thumb(&m(pos));
        assert!(ts <= 100 && ts + tl > 100);
    }

    #[test]
    fn sin_barra_si_todo_cabe() {
        let mut sb = ScrollBar::new(false);
        let mm = Metrics { total: 20, ..m(0) };
        assert_eq!(sb.mouse(down(105, 195), &mm, 0), SbEvent::default());
        assert!(!sb.active());
        assert_eq!(sb.tick(&mm, 10_000), None);
    }

    #[test]
    fn horizontal_y_fuera_no_captura() {
        let mut sb = ScrollBar::new(true);
        let mm = Metrics { rect: Rect::new(0, 50, 200, SB_W), total: 100, page: 20, pos: 0, line: 1 };
        assert_eq!(sb.mouse(down(195, 55), &mm, 0).pos, Some(1));
        sb.mouse(up(), &mm, 1);
        let e = sb.mouse(down(50, 20), &mm, 2);
        assert!(!e.captured && e.pos.is_none());
    }

    #[test]
    fn hover_y_dibujo() {
        let mut sb = ScrollBar::new(false);
        let e = sb.mouse(MouseEv::Move { x: 105, y: 5 }, &m(0), 0);
        assert!(e.redraw && e.captured);
        assert!(!sb.mouse(MouseEv::Move { x: 106, y: 6 }, &m(0), 1).redraw);
        assert!(sb.mouse(MouseEv::Leave, &m(0), 2).redraw);
        let mut s = Surface::new(200, 200, 0xFFFFFF);
        let th = Theme { accent: 0x2D7DD2, wallpaper: 0, letra: 0 };
        sb.draw(&mut s, &m(0), &th);
        assert_eq!(s.px[(100 * 200 + 99) as usize], 0xFFFFFF);
        assert_ne!(s.px[(100 * 200 + 108) as usize], 0xFFFFFF);
    }
}
