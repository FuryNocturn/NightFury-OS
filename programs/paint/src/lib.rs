// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Paint: lienzo de 800×500 con lápiz, pincel, goma, línea, rectángulo, relleno y cuentagotas.
//! Programa en modo usuario (ver `main.rs`); la lógica vive aquí para probarla y fotografiarla.
#![cfg_attr(not(test), no_std)]
extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use nf_app::TAG_GUARDADO;
use nf_core::apps::{App, AppKind, Ctx, MouseEv};
use nf_core::bmp;
use nf_core::dialogs::{Answer, DialogResult};
use nf_core::font::{self, Align};
use nf_core::geom::Rect;
use nf_core::input::{Key, KeyEvent};
use nf_core::platform::Platform;
use nf_core::scrollbar::{Metrics, ScrollBar, SB_W};
use nf_core::surface::{blend, darken, Surface};
use nf_core::theme::Theme;

/// Tamaño del dibujo.
pub const CW: i32 = 800;
pub const CH: i32 = 500;
const TOP_H: i32 = 40;
const TOOL_W: i32 = 92;
const PAL_H: i32 = 64;
const MARGEN: i32 = 8;
const UNDO_MAX: usize = 10;
const GROSORES: [i32; 4] = [1, 3, 5, 8];
const INK: u32 = 0x1A1A1A;
const MUTED: u32 = 0x6B6B6B;
const LINE: u32 = 0xDADADA;
const FONDO_VISTA: u32 = 0xCFD4DB;

/// Paleta propia de 16 colores: fila de oscuros y fila de claros.
pub const PALETA: [u32; 16] = [
    0x000000, 0x5B5F66, 0xB3261E, 0xE8710A, 0xF2B01E, 0x2E9E4F, 0x1A8FA6, 0x1F5FD0, //
    0xFFFFFF, 0xC4C8CF, 0x8B5A2B, 0xE23B7A, 0xFFE27A, 0xA6E3A1, 0x8AD8E8, 0x7C4DDB,
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Herr {
    Lapiz,
    Pincel,
    Goma,
    Linea,
    Rectangulo,
    Relleno,
    Cuentagotas,
}

const HERRS: [Herr; 7] = [Herr::Lapiz, Herr::Pincel, Herr::Goma, Herr::Linea, Herr::Rectangulo, Herr::Relleno, Herr::Cuentagotas];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Estado {
    Nuevo,
    Modificado,
    Guardando,
    Guardado,
    Error,
}

/// Lo que queda pendiente tras contestar "¿guardar los cambios?".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Accion {
    Nuevo,
    Cerrar,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ui {
    Herr(Herr),
    Grosor(usize),
    Color(usize),
    Nuevo,
    Guardar,
    Deshacer,
    PGuardar,
    PDescartar,
    PCancelar,
}

struct Trazo {
    herr: Herr,
    ini: (i32, i32),
    ult: (i32, i32),
}

/// Guarda el dibujo (nombre propuesto y BMP); `false` si no se puede pedir.
type Guardador = fn(&str, &[u8]) -> bool;

pub struct Paint {
    lienzo: Vec<u32>,
    deshacer: Vec<Vec<u32>>,
    herr: Herr,
    /// Herramienta a la que vuelve el cuentagotas tras coger un color.
    previa: Herr,
    color: u32,
    grosor: usize,
    trazo: Option<Trazo>,
    estado: Estado,
    /// Estado anterior a pedir guardar, por si el usuario cancela.
    antes: Estado,
    pregunta: Option<Accion>,
    tras_guardar: Option<Accion>,
    guardador: Guardador,
    size: (i32, i32),
    sx: i32,
    sy: i32,
    sbv: ScrollBar,
    sbh: ScrollBar,
    hover: Option<Ui>,
    abajo: Option<Ui>,
}

/// App lista para `nf_app::programa!`.
pub fn crear() -> Box<dyn App> {
    Box::new(Paint::new(nf_app::guardar_como))
}

impl Paint {
    pub fn new(guardador: Guardador) -> Self {
        Paint {
            lienzo: vec![0xFFFFFF; (CW * CH) as usize],
            deshacer: Vec::new(),
            herr: Herr::Lapiz,
            previa: Herr::Lapiz,
            color: PALETA[0],
            grosor: 1,
            trazo: None,
            estado: Estado::Nuevo,
            antes: Estado::Nuevo,
            pregunta: None,
            tras_guardar: None,
            guardador,
            size: (0, 0),
            sx: 0,
            sy: 0,
            sbv: ScrollBar::new(false),
            sbh: ScrollBar::new(true),
            hover: None,
            abajo: None,
        }
    }

    // ------------------------------------------------------------ geometría

    fn vista(&self) -> Rect {
        Rect::new(TOOL_W, TOP_H, (self.size.0 - TOOL_W).max(0), (self.size.1 - TOP_H - PAL_H).max(0))
    }

    /// ¿Hacen falta barra vertical y horizontal?
    fn barras(&self) -> (bool, bool) {
        let v = self.vista();
        let (mut bv, mut bh) = (false, false);
        for _ in 0..2 {
            bv = CH + 2 * MARGEN > v.h - if bh { SB_W } else { 0 };
            bh = CW + 2 * MARGEN > v.w - if bv { SB_W } else { 0 };
        }
        (bv, bh)
    }

    /// Zona donde se ve el dibujo (la vista sin las barras).
    fn area(&self) -> Rect {
        let (bv, bh) = self.barras();
        let v = self.vista();
        Rect::new(v.x, v.y, (v.w - if bv { SB_W } else { 0 }).max(0), (v.h - if bh { SB_W } else { 0 }).max(0))
    }

    fn metricas(&self) -> (Metrics, Metrics) {
        let (a, v) = (self.area(), self.vista());
        (
            Metrics { rect: Rect::new(v.right() - SB_W, v.y, SB_W, a.h), total: CH + 2 * MARGEN, page: a.h, pos: self.sy, line: 20 },
            Metrics { rect: Rect::new(v.x, v.bottom() - SB_W, a.w, SB_W), total: CW + 2 * MARGEN, page: a.w, pos: self.sx, line: 20 },
        )
    }

    fn ajusta(&mut self) {
        let (mv, mh) = self.metricas();
        self.sy = self.sy.clamp(0, mv.max());
        self.sx = self.sx.clamp(0, mh.max());
    }

    /// Esquina del lienzo en coordenadas de la ventana.
    fn origen(&self) -> (i32, i32) {
        let a = self.area();
        (a.x + MARGEN - self.sx, a.y + MARGEN - self.sy)
    }

    fn al_lienzo(&self, x: i32, y: i32) -> (i32, i32) {
        let (ox, oy) = self.origen();
        (x - ox, y - oy)
    }

    /// Zonas pulsables; con el aviso abierto solo valen sus botones.
    fn zonas(&self) -> Vec<(Ui, Rect, bool)> {
        if self.pregunta.is_some() {
            self.zonas_aviso()
        } else {
            self.zonas_base()
        }
    }

    fn zonas_aviso(&self) -> Vec<(Ui, Rect, bool)> {
        let p = self.panel();
        [Ui::PGuardar, Ui::PDescartar, Ui::PCancelar].into_iter().enumerate().map(|(i, u)| (u, Rect::new(p.x + 16 + i as i32 * 120, p.bottom() - 48, 112, 32), true)).collect()
    }

    fn zonas_base(&self) -> Vec<(Ui, Rect, bool)> {
        let (w, h) = self.size;
        let mut v = Vec::new();
        v.push((Ui::Nuevo, Rect::new(8, 6, 80, 28), true));
        v.push((Ui::Guardar, Rect::new(96, 6, 80, 28), true));
        v.push((Ui::Deshacer, Rect::new(184, 6, 88, 28), !self.deshacer.is_empty()));
        for i in 0..GROSORES.len() {
            v.push((Ui::Grosor(i), Rect::new(w - 8 - (4 - i as i32) * 44, 6, 40, 28), true));
        }
        for (i, hh) in HERRS.iter().enumerate() {
            let (c, f) = (i as i32 % 2, i as i32 / 2);
            v.push((Ui::Herr(*hh), Rect::new(4 + c * 44, TOP_H + 8 + f * 44, 40, 40), true));
        }
        for i in 0..PALETA.len() {
            let (c, f) = (i as i32 % 8, i as i32 / 8);
            v.push((Ui::Color(i), Rect::new(80 + c * 30, h - PAL_H + 4 + f * 30, 26, 26), true));
        }
        v
    }

    fn ui_en(&self, x: i32, y: i32) -> Option<Ui> {
        self.zonas().into_iter().find(|(_, r, on)| *on && r.contains(x, y)).map(|(u, ..)| u)
    }

    /// Recuadro del aviso "¿Guardar los cambios?".
    fn panel(&self) -> Rect {
        let (w, h) = self.size;
        let (pw, ph) = (376.min(w).max(0), 150.min(h).max(0));
        Rect::new((w - pw) / 2, (h - ph) / 2, pw, ph)
    }

    // ------------------------------------------------------------ dibujo sobre el lienzo

    fn pone(&mut self, x: i32, y: i32, c: u32) {
        if (0..CW).contains(&x) && (0..CH).contains(&y) {
            self.lienzo[(y * CW + x) as usize] = c;
        }
    }

    /// Sello de diámetro `d` centrado en (x, y): redondo, o cuadrado para la goma.
    fn sello(&mut self, x: i32, y: i32, d: i32, c: u32, cuadrado: bool) {
        let o = (d - 1) / 2;
        for j in 0..d {
            for i in 0..d {
                let (a, b) = (2 * i - (d - 1), 2 * j - (d - 1));
                if cuadrado || a * a + b * b <= d * d {
                    self.pone(x - o + i, y - o + j, c);
                }
            }
        }
    }

    /// Diámetro y forma del sello de una herramienta con el grosor elegido.
    fn pincel(&self, h: Herr) -> (i32, bool) {
        let g = GROSORES[self.grosor];
        match h {
            Herr::Pincel => (2 * g + 3, false),
            Herr::Goma => (2 * g + 4, true),
            _ => (g, false),
        }
    }

    /// Línea de Bresenham de (x0, y0) a (x1, y1) con el sello dado.
    fn linea(&mut self, (mut x0, mut y0): (i32, i32), (x1, y1): (i32, i32), d: i32, c: u32, cuadrado: bool) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let mut err = dx + dy;
        loop {
            self.sello(x0, y0, d, c, cuadrado);
            if (x0, y0) == (x1, y1) {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    fn rectangulo(&mut self, (x0, y0): (i32, i32), (x1, y1): (i32, i32), d: i32, c: u32) {
        for (a, b) in [((x0, y0), (x1, y0)), ((x1, y0), (x1, y1)), ((x1, y1), (x0, y1)), ((x0, y1), (x0, y0))] {
            self.linea(a, b, d, c, false);
        }
    }

    /// Relleno por líneas: cambia la zona de un solo color que contiene (x, y).
    fn rellena(&mut self, x: i32, y: i32, nuevo: u32) {
        if !(0..CW).contains(&x) || !(0..CH).contains(&y) {
            return;
        }
        let viejo = self.lienzo[(y * CW + x) as usize];
        if viejo == nuevo {
            return;
        }
        let mut pila = vec![(x, y)];
        while let Some((x, y)) = pila.pop() {
            if self.lienzo[(y * CW + x) as usize] != viejo {
                continue;
            }
            let (mut a, mut b) = (x, x);
            while a > 0 && self.lienzo[(y * CW + a - 1) as usize] == viejo {
                a -= 1;
            }
            while b < CW - 1 && self.lienzo[(y * CW + b + 1) as usize] == viejo {
                b += 1;
            }
            for i in a..=b {
                self.lienzo[(y * CW + i) as usize] = nuevo;
            }
            for yy in [y - 1, y + 1] {
                if !(0..CH).contains(&yy) {
                    continue;
                }
                // Una semilla por tramo contiguo de la fila vecina.
                let mut dentro = false;
                for i in a..=b {
                    let es = self.lienzo[(yy * CW + i) as usize] == viejo;
                    if es && !dentro {
                        pila.push((i, yy));
                    }
                    dentro = es;
                }
            }
        }
    }

    // ------------------------------------------------------------ acciones

    /// Guarda el estado actual para Deshacer (hasta `UNDO_MAX` pasos) y marca que hay cambios.
    fn guarda_paso(&mut self) {
        if self.deshacer.len() == UNDO_MAX {
            self.deshacer.remove(0);
        }
        self.deshacer.push(self.lienzo.clone());
        self.estado = Estado::Modificado;
    }

    fn deshacer(&mut self) {
        if let Some(l) = self.deshacer.pop() {
            self.lienzo = l;
            self.estado = Estado::Modificado;
        }
    }

    fn sucio(&self) -> bool {
        !matches!(self.estado, Estado::Nuevo | Estado::Guardado)
    }

    fn nuevo(&mut self) {
        self.lienzo.fill(0xFFFFFF);
        self.deshacer.clear();
        self.estado = Estado::Nuevo;
        self.trazo = None;
        self.sx = 0;
        self.sy = 0;
    }

    /// Pide a Nuevo o Cerrar; con cambios sin guardar pregunta antes.
    fn pide(&mut self, a: Accion, cx: &mut Ctx) {
        if self.sucio() && self.estado != Estado::Guardando {
            self.pregunta = Some(a);
        } else if self.estado != Estado::Guardando {
            self.hace(a, cx);
        }
    }

    fn hace(&mut self, a: Accion, cx: &mut Ctx) {
        match a {
            Accion::Nuevo => self.nuevo(),
            Accion::Cerrar => cx.close = true,
        }
    }

    /// Manda el dibujo a guardar como BMP de 24 bits; el resultado llega por `dialog_result`.
    fn guardar(&mut self, tras: Option<Accion>) {
        if self.estado == Estado::Guardando {
            return;
        }
        let mut s = Surface::new(CW, CH, 0);
        s.px.copy_from_slice(&self.lienzo);
        let datos = bmp::encode(&s);
        if (self.guardador)("dibujo.bmp", &datos) {
            self.antes = self.estado;
            self.estado = Estado::Guardando;
            self.tras_guardar = tras;
        } else {
            self.estado = Estado::Error;
        }
    }

    fn activa(&mut self, u: Ui, cx: &mut Ctx) {
        match u {
            Ui::Herr(h) => {
                if h == Herr::Cuentagotas && self.herr != Herr::Cuentagotas {
                    self.previa = self.herr;
                }
                self.herr = h;
            }
            Ui::Grosor(i) => self.grosor = i,
            Ui::Color(i) => self.color = PALETA[i],
            Ui::Nuevo => self.pide(Accion::Nuevo, cx),
            Ui::Guardar => self.guardar(None),
            Ui::Deshacer => self.deshacer(),
            Ui::PGuardar => {
                if let Some(a) = self.pregunta.take() {
                    self.guardar(Some(a));
                }
            }
            Ui::PDescartar => {
                if let Some(a) = self.pregunta.take() {
                    self.hace(a, cx);
                }
            }
            Ui::PCancelar => self.pregunta = None,
        }
    }

    fn empieza(&mut self, x: i32, y: i32) {
        let p = self.al_lienzo(x, y);
        let (d, cuad) = self.pincel(self.herr);
        match self.herr {
            Herr::Cuentagotas => {
                if (0..CW).contains(&p.0) && (0..CH).contains(&p.1) {
                    self.color = self.lienzo[(p.1 * CW + p.0) as usize];
                }
                self.herr = self.previa;
            }
            Herr::Relleno => {
                self.guarda_paso();
                self.rellena(p.0, p.1, self.color);
            }
            h @ (Herr::Lapiz | Herr::Pincel | Herr::Goma) => {
                self.guarda_paso();
                let c = if h == Herr::Goma { 0xFFFFFF } else { self.color };
                self.sello(p.0, p.1, d, c, cuad);
                self.trazo = Some(Trazo { herr: h, ini: p, ult: p });
            }
            h => {
                self.guarda_paso();
                self.trazo = Some(Trazo { herr: h, ini: p, ult: p });
                self.pinta_forma(h, p, p);
            }
        }
    }

    /// Línea o rectángulo de `ini` a `fin` sobre el estado anterior al trazo (vista previa).
    fn pinta_forma(&mut self, h: Herr, ini: (i32, i32), fin: (i32, i32)) {
        if let Some(base) = self.deshacer.last() {
            self.lienzo.copy_from_slice(base);
        }
        let (d, _) = self.pincel(h);
        let c = self.color;
        if h == Herr::Linea {
            self.linea(ini, fin, d, c, false);
        } else {
            self.rectangulo(ini, fin, d, c);
        }
    }

    fn continua(&mut self, x: i32, y: i32) {
        let p = self.al_lienzo(x, y);
        let Some(t) = self.trazo.take() else { return };
        let (d, cuad) = self.pincel(t.herr);
        match t.herr {
            Herr::Lapiz | Herr::Pincel | Herr::Goma => {
                let c = if t.herr == Herr::Goma { 0xFFFFFF } else { self.color };
                self.linea(t.ult, p, d, c, cuad);
            }
            h => self.pinta_forma(h, t.ini, p),
        }
        self.trazo = Some(Trazo { ult: p, ..t });
    }

    fn titulo(&self) -> &'static str {
        match self.estado {
            Estado::Nuevo => "Sin título - Paint",
            Estado::Modificado => "Sin título* - Paint",
            Estado::Guardando => "Guardando… - Paint",
            Estado::Guardado => "Guardado - Paint",
            Estado::Error => "No se pudo guardar - Paint",
        }
    }
}

impl App for Paint {
    fn kind(&self) -> AppKind {
        AppKind::Program
    }
    fn title(&self) -> String {
        String::from(self.titulo())
    }
    fn min_size(&self) -> (i32, i32) {
        (620, 420)
    }

    fn draw(&mut self, s: &mut Surface, theme: &Theme, _sys: &mut dyn Platform, _focused: bool) {
        self.size = (s.w, s.h);
        self.ajusta();
        s.fill_rect(s.bounds(), 0xF3F3F3);
        let (w, h) = (s.w, s.h);
        // Barra superior.
        s.fill_rect(Rect::new(0, 0, w, TOP_H), 0xFFFFFF);
        s.fill_rect(Rect::new(0, TOP_H - 1, w, 1), LINE);
        font::draw_in(s, Rect::new(w - 8 - 4 * 44 - 70, 6, 64, 28), "Grosor", 9, false, MUTED, Align::Right);
        // Panel de herramientas y paleta.
        s.fill_rect(Rect::new(0, TOP_H, TOOL_W, h - TOP_H - PAL_H), 0xF8F8F8);
        s.fill_rect(Rect::new(TOOL_W - 1, TOP_H, 1, h - TOP_H - PAL_H), LINE);
        s.fill_rect(Rect::new(0, h - PAL_H, w, PAL_H), 0xFFFFFF);
        s.fill_rect(Rect::new(0, h - PAL_H, w, 1), LINE);
        s.fill_rect(Rect::new(12, h - PAL_H + 8, 52, 48), 0x9A9A9A);
        s.fill_rect(Rect::new(13, h - PAL_H + 9, 50, 46), self.color);
        // Lienzo.
        self.dibuja_lienzo(s);
        let (bv, bh) = self.barras();
        let (mv, mh) = self.metricas();
        if bv {
            self.sbv.draw(s, &mv, theme);
        }
        if bh {
            self.sbh.draw(s, &mh, theme);
        }
        if bv && bh {
            let v = self.vista();
            s.fill_rect(Rect::new(v.right() - SB_W, v.bottom() - SB_W, SB_W, SB_W), 0xF0F0F0);
        }
        // Botones; el aviso, si lo hay, va encima de todo.
        for (u, r, on) in self.zonas_base() {
            self.dibuja_boton(s, theme, u, r, on);
        }
        if let Some(a) = self.pregunta {
            self.dibuja_aviso(s, theme, a);
            for (u, r, on) in self.zonas_aviso() {
                self.dibuja_boton(s, theme, u, r, on);
            }
        }
    }

    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
        if self.pregunta.is_some() {
            match ev.key {
                Key::Escape => self.activa(Ui::PCancelar, cx),
                Key::Enter => self.activa(Ui::PGuardar, cx),
                _ => return,
            }
            cx.redraw = true;
            return;
        }
        match ev.key {
            Key::Char('z') if ev.ctrl => self.deshacer(),
            Key::Char('s') if ev.ctrl => self.guardar(None),
            Key::Char('n') if ev.ctrl => self.pide(Accion::Nuevo, cx),
            _ => return,
        }
        cx.redraw = true;
    }

    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
        // Barras de desplazamiento (no mientras se dibuja).
        if self.pregunta.is_none() && self.trazo.is_none() {
            let (bv, bh) = self.barras();
            let (mv, mh) = self.metricas();
            if bv {
                let e = self.sbv.mouse(ev, &mv, cx.now_ms);
                if let Some(p) = e.pos {
                    self.sy = p;
                }
                cx.redraw |= e.redraw;
                if e.captured {
                    return;
                }
            }
            if bh {
                let e = self.sbh.mouse(ev, &mh, cx.now_ms);
                if let Some(p) = e.pos {
                    self.sx = p;
                }
                cx.redraw |= e.redraw;
                if e.captured {
                    return;
                }
            }
        }
        let (hover, abajo) = match ev {
            MouseEv::Down { x, y } => {
                let u = self.ui_en(x, y);
                if u.is_none() && self.pregunta.is_none() && self.area().contains(x, y) {
                    self.empieza(x, y);
                    cx.redraw = true;
                }
                (u, u)
            }
            MouseEv::Move { x, y } => {
                if self.trazo.is_some() {
                    self.continua(x, y);
                    cx.redraw = true;
                }
                (self.ui_en(x, y), self.abajo)
            }
            MouseEv::Up { x, y } => {
                self.trazo = None;
                let u = self.ui_en(x, y);
                if let (Some(u), true) = (u, u == self.abajo) {
                    self.activa(u, cx);
                    cx.redraw = true;
                }
                (u, None)
            }
            MouseEv::Wheel { dz, .. } => {
                if self.pregunta.is_none() {
                    self.sy += dz * 40;
                    self.ajusta();
                    cx.redraw = true;
                }
                return;
            }
            MouseEv::Leave => (None, self.abajo),
            MouseEv::RightDown { .. } => return,
        };
        if (hover, abajo) != (self.hover, self.abajo) {
            cx.redraw = true;
        }
        self.hover = hover;
        self.abajo = abajo;
    }

    fn tick(&mut self, cx: &mut Ctx) {
        let (mv, mh) = self.metricas();
        if let Some(p) = self.sbv.tick(&mv, cx.now_ms) {
            self.sy = p;
            cx.redraw = true;
        }
        if let Some(p) = self.sbh.tick(&mh, cx.now_ms) {
            self.sx = p;
            cx.redraw = true;
        }
    }

    fn dialog_result(&mut self, r: DialogResult, cx: &mut Ctx) {
        match r {
            DialogResult::Path { tag: TAG_GUARDADO, path: Some(_) } => {
                self.estado = Estado::Guardado;
                if let Some(a) = self.tras_guardar.take() {
                    self.hace(a, cx);
                }
            }
            DialogResult::Path { tag: TAG_GUARDADO, path: None } => {
                self.estado = self.antes;
                self.tras_guardar = None;
            }
            DialogResult::Answer { tag: TAG_GUARDADO, answer: Answer::No } => {
                self.estado = Estado::Error;
                self.tras_guardar = None;
            }
            _ => return,
        }
        cx.redraw = true;
    }

    fn request_close(&mut self, cx: &mut Ctx) -> bool {
        if self.estado == Estado::Guardando {
            return false;
        }
        if self.sucio() {
            self.pregunta = Some(Accion::Cerrar);
            cx.redraw = true;
            return false;
        }
        true
    }
}

impl Paint {
    fn dibuja_lienzo(&self, s: &mut Surface) {
        let a = self.area().intersect(s.bounds());
        let v = self.vista().intersect(s.bounds());
        s.fill_rect(v, FONDO_VISTA);
        if a.w <= 0 || a.h <= 0 {
            return;
        }
        let old = s.clip();
        s.set_clip(old.intersect(a));
        let (ox, oy) = self.origen();
        s.fill_rect(Rect::new(ox + 3, oy + 3, CW, CH), 0xAEB4BD);
        s.fill_rect(Rect::new(ox - 1, oy - 1, CW + 2, CH + 2), 0x7A7F87);
        let (x0, x1) = ((a.x - ox).max(0), (a.right() - ox).min(CW));
        if x0 < x1 {
            for y in (a.y - oy).max(0)..(a.bottom() - oy).min(CH) {
                let d = ((oy + y) * s.w + ox + x0) as usize;
                let o = (y * CW + x0) as usize;
                s.px[d..d + (x1 - x0) as usize].copy_from_slice(&self.lienzo[o..o + (x1 - x0) as usize]);
            }
        }
        s.set_clip(old);
    }

    fn dibuja_boton(&self, s: &mut Surface, theme: &Theme, u: Ui, r: Rect, on: bool) {
        let hot = self.hover == Some(u) && on;
        let activo = match u {
            Ui::Herr(h) => h == self.herr,
            Ui::Grosor(i) => i == self.grosor,
            Ui::Color(i) => PALETA[i] == self.color,
            _ => false,
        };
        match u {
            Ui::Color(i) => {
                s.fill_rect(r, if activo { theme.accent } else { 0x9A9A9A });
                s.fill_rect(Rect::new(r.x + 1, r.y + 1, r.w - 2, r.h - 2), PALETA[i]);
                if activo {
                    s.frame(Rect::new(r.x - 1, r.y - 1, r.w + 2, r.h + 2), theme.accent);
                }
            }
            Ui::PGuardar | Ui::PDescartar | Ui::PCancelar => {
                let principal = u == Ui::PGuardar;
                let base = if principal { theme.accent } else { 0xE6E6E6 };
                let fondo = if self.abajo == Some(u) && hot { darken(base, 40) } else if hot { darken(base, 18) } else { base };
                s.fill_rect(r, fondo);
                let t = match u {
                    Ui::PGuardar => "Guardar",
                    Ui::PDescartar => "No guardar",
                    _ => "Cancelar",
                };
                font::draw_in(s, r, t, 10, false, if principal { 0xFFFFFF } else { INK }, Align::Center);
            }
            _ => {
                let fondo = if activo {
                    blend(0xFFFFFF, theme.accent, 60)
                } else if hot && self.abajo == Some(u) {
                    0xCFCFCF
                } else if hot {
                    0xE5E5E5
                } else if matches!(u, Ui::Nuevo | Ui::Guardar | Ui::Deshacer) {
                    0xEDEDED
                } else {
                    0xF8F8F8
                };
                s.fill_rect(r, if on { fondo } else { 0xF3F3F3 });
                if activo {
                    s.frame(r, theme.accent);
                }
                match u {
                    Ui::Herr(h) => icono(s, h, r.x + 8, r.y + 8),
                    Ui::Grosor(i) => {
                        let g = GROSORES[i];
                        s.fill_rect(Rect::new(r.x + 8, r.y + (r.h - g) / 2, r.w - 16, g), INK);
                    }
                    Ui::Nuevo => font::draw_in(s, r, "Nuevo", 10, false, INK, Align::Center),
                    Ui::Guardar => font::draw_in(s, r, "Guardar", 10, false, INK, Align::Center),
                    Ui::Deshacer => font::draw_in(s, r, "Deshacer", 10, false, if on { INK } else { 0xA8A8A8 }, Align::Center),
                    _ => {}
                }
            }
        }
    }

    /// Aviso propio: oscurece la ventana y pregunta si se guardan los cambios.
    fn dibuja_aviso(&self, s: &mut Surface, theme: &Theme, a: Accion) {
        for p in s.px.iter_mut() {
            *p = blend(*p, 0x000000, 90);
        }
        let p = self.panel();
        s.fill_rect(p, 0xFFFFFF);
        s.frame(p, theme.accent);
        font::draw_in(s, Rect::new(p.x + 16, p.y + 14, p.w - 32, 24), "¿Quieres guardar los cambios?", 11, true, INK, Align::Left);
        let t = if a == Accion::Nuevo { "Si empiezas un dibujo nuevo, se perderá el actual." } else { "Si cierras Paint, se perderá el dibujo actual." };
        font::draw_in(s, Rect::new(p.x + 16, p.y + 46, p.w - 32, 40), t, 9, false, MUTED, Align::Left);
    }
}

/// Icono de una herramienta en un cuadrado de 24×24 con esquina en (x, y).
fn icono(s: &mut Surface, h: Herr, x: i32, y: i32) {
    let p = |u: f32, v: f32| (x as f32 + u, y as f32 + v);
    match h {
        Herr::Lapiz => {
            s.stroke(&[p(5.0, 19.0), p(17.0, 7.0)], 5.0, 0xF2B01E);
            s.stroke(&[p(18.0, 6.0), p(20.0, 4.0)], 5.0, 0xE07A7A);
            s.stroke(&[p(3.5, 20.5), p(6.0, 18.0)], 3.0, 0x3A3F47);
        }
        Herr::Pincel => {
            s.stroke(&[p(20.0, 4.0), p(12.0, 12.0)], 3.0, 0x8B5A2B);
            s.stroke(&[p(12.0, 12.0), p(9.5, 14.5)], 4.0, 0xB8BCC4);
            s.fill_circle(x as f32 + 7.0, y as f32 + 17.0, 3.8, 0x1F5FD0, 255);
        }
        Herr::Goma => {
            s.stroke(&[p(6.0, 17.0), p(17.0, 6.0)], 8.0, 0xF4A6A0);
            s.stroke(&[p(6.0, 17.0), p(10.0, 13.0)], 8.0, 0x8B9099);
            s.fill_rect(Rect::new(x + 3, y + 21, 18, 2), 0x8B9099);
        }
        Herr::Linea => {
            s.stroke(&[p(4.0, 20.0), p(20.0, 4.0)], 2.0, INK);
            s.fill_circle(x as f32 + 4.0, y as f32 + 20.0, 2.4, INK, 255);
            s.fill_circle(x as f32 + 20.0, y as f32 + 4.0, 2.4, INK, 255);
        }
        Herr::Rectangulo => {
            s.frame(Rect::new(x + 3, y + 5, 18, 14), INK);
            s.frame(Rect::new(x + 4, y + 6, 16, 12), INK);
        }
        Herr::Relleno => {
            // Cubo inclinado con un asa y una gota que cae.
            s.stroke(&[p(6.0, 9.0), p(12.0, 3.0)], 2.0, 0x5B5F66);
            s.stroke(&[p(4.0, 11.0), p(12.0, 19.0), p(18.0, 13.0), p(10.0, 5.0), p(4.0, 11.0)], 3.0, 0x5B5F66);
            s.stroke(&[p(6.0, 12.0), p(12.0, 18.0)], 3.0, 0x1F5FD0);
            s.fill_circle(x as f32 + 20.0, y as f32 + 17.0, 2.6, 0x1F5FD0, 255);
        }
        Herr::Cuentagotas => {
            s.stroke(&[p(17.0, 7.0), p(10.0, 14.0)], 4.0, 0x8AD8E8);
            s.stroke(&[p(18.0, 6.0), p(21.0, 3.0)], 6.0, 0x3A3F47);
            s.stroke(&[p(10.0, 14.0), p(5.0, 19.5)], 2.0, 0x5B5F66);
            s.fill_circle(x as f32 + 4.0, y as f32 + 21.0, 1.8, 0x1F5FD0, 255);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nf_core::platform::TestPlatform;

    fn falso(_: &str, _: &[u8]) -> bool {
        true
    }
    fn ocupado(_: &str, _: &[u8]) -> bool {
        false
    }

    fn app() -> Paint {
        let mut p = Paint::new(falso);
        p.size = (920, 620);
        p
    }

    fn con<R>(f: impl FnOnce(&mut Ctx) -> R) -> R {
        let mut sys = TestPlatform::default();
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        f(&mut cx)
    }

    fn px(p: &Paint, x: i32, y: i32) -> u32 {
        p.lienzo[(y * CW + x) as usize]
    }

    /// Arrastra con el ratón del punto (x0, y0) al (x1, y1) en coordenadas del LIENZO.
    fn arrastra(p: &mut Paint, a: (i32, i32), b: (i32, i32)) {
        let (ox, oy) = p.origen();
        con(|cx| {
            p.mouse(MouseEv::Down { x: ox + a.0, y: oy + a.1 }, cx);
            p.mouse(MouseEv::Move { x: ox + b.0, y: oy + b.1 }, cx);
            p.mouse(MouseEv::Up { x: ox + b.0, y: oy + b.1 }, cx);
        });
    }

    fn pulsa(p: &mut Paint, u: Ui) {
        let r = p.zonas().into_iter().find(|z| z.0 == u).unwrap().1;
        let (x, y) = (r.x + r.w / 2, r.y + r.h / 2);
        con(|cx| {
            p.mouse(MouseEv::Down { x, y }, cx);
            p.mouse(MouseEv::Up { x, y }, cx);
        });
    }

    fn tecla(p: &mut Paint, k: Key, ctrl: bool) {
        con(|cx| p.key(&KeyEvent { key: k, ch: None, down: true, shift: false, ctrl, alt: false }, cx));
    }

    #[test]
    fn lapiz_linea_y_goma() {
        let mut p = app();
        p.grosor = 0;
        arrastra(&mut p, (10, 10), (20, 10));
        assert!((10..=20).all(|x| px(&p, x, 10) == 0), "el lápiz une los puntos");
        assert_eq!(px(&p, 15, 11), 0xFFFFFF);
        pulsa(&mut p, Ui::Herr(Herr::Goma));
        assert_eq!(p.herr, Herr::Goma);
        arrastra(&mut p, (12, 10), (14, 10));
        assert_eq!(px(&p, 13, 10), 0xFFFFFF, "la goma borra a blanco");
        assert_eq!(px(&p, 20, 10), 0, "y no toca lo de lejos");
        // Línea: vista previa y resultado final sin restos intermedios.
        pulsa(&mut p, Ui::Herr(Herr::Linea));
        pulsa(&mut p, Ui::Color(2));
        let (ox, oy) = p.origen();
        con(|cx| {
            p.mouse(MouseEv::Down { x: ox + 100, y: oy + 100 }, cx);
            p.mouse(MouseEv::Move { x: ox + 100, y: oy + 200 }, cx);
            p.mouse(MouseEv::Move { x: ox + 200, y: oy + 100 }, cx);
            p.mouse(MouseEv::Up { x: ox + 200, y: oy + 100 }, cx);
        });
        assert_eq!(px(&p, 150, 100), PALETA[2]);
        assert_eq!(px(&p, 100, 150), 0xFFFFFF, "la posición intermedia se borró");
    }

    #[test]
    fn rectangulo_con_grosor_y_pincel() {
        let mut p = app();
        pulsa(&mut p, Ui::Grosor(2));
        pulsa(&mut p, Ui::Herr(Herr::Rectangulo));
        arrastra(&mut p, (50, 50), (150, 120));
        assert_eq!(px(&p, 100, 50), 0);
        assert_eq!(px(&p, 50, 85), 0);
        assert_eq!(px(&p, 100, 85), 0xFFFFFF, "solo el contorno");
        pulsa(&mut p, Ui::Herr(Herr::Pincel));
        pulsa(&mut p, Ui::Grosor(0));
        arrastra(&mut p, (300, 300), (300, 300));
        assert_eq!(px(&p, 300, 300), 0);
        assert_eq!(px(&p, 302, 300), 0, "el pincel es más grueso que el lápiz");
        assert_eq!(px(&p, 305, 300), 0xFFFFFF);
    }

    #[test]
    fn relleno_cuentagotas_y_limites() {
        let mut p = app();
        pulsa(&mut p, Ui::Grosor(0));
        pulsa(&mut p, Ui::Herr(Herr::Rectangulo));
        arrastra(&mut p, (10, 10), (60, 60));
        pulsa(&mut p, Ui::Color(3));
        pulsa(&mut p, Ui::Herr(Herr::Relleno));
        arrastra(&mut p, (30, 30), (30, 30));
        assert_eq!((px(&p, 11, 11), px(&p, 59, 59), px(&p, 10, 30)), (PALETA[3], PALETA[3], 0), "rellena dentro y respeta el borde");
        assert_eq!(px(&p, 70, 70), 0xFFFFFF, "no se sale del recuadro");
        // Rellenar fuera cuesta un paso de deshacer pero no cambia nada; un relleno del mismo color tampoco.
        pulsa(&mut p, Ui::Color(0));
        pulsa(&mut p, Ui::Herr(Herr::Cuentagotas));
        assert_eq!(p.previa, Herr::Relleno);
        arrastra(&mut p, (30, 30), (30, 30));
        assert_eq!(p.color, PALETA[3], "coge el color de debajo");
        assert_eq!(p.herr, Herr::Relleno, "y vuelve a la herramienta anterior");
        // Relleno fuera del lienzo: no falla.
        p.rellena(-5, 9999, 0);
    }

    #[test]
    fn deshacer_diez_pasos() {
        let mut p = app();
        p.grosor = 0;
        for i in 0..12 {
            arrastra(&mut p, (10 + i * 5, 10), (10 + i * 5, 10));
        }
        assert_eq!(p.deshacer.len(), UNDO_MAX);
        tecla(&mut p, Key::Char('z'), true);
        assert_eq!(px(&p, 10 + 11 * 5, 10), 0xFFFFFF, "Ctrl+Z quita el último");
        assert_eq!(px(&p, 10 + 10 * 5, 10), 0);
        for _ in 0..20 {
            pulsa_si_activo(&mut p);
        }
        assert!(p.deshacer.is_empty());
        assert_eq!(px(&p, 15, 10), 0, "los dos primeros puntos ya no se pueden deshacer");
        assert_eq!(px(&p, 20, 10), 0xFFFFFF);
        assert!(p.ui_en(200, 20).is_none(), "Deshacer apagado cuando no hay nada");
    }

    fn pulsa_si_activo(p: &mut Paint) {
        if !p.deshacer.is_empty() {
            pulsa(p, Ui::Deshacer);
        }
    }

    #[test]
    fn nuevo_pregunta_si_hay_cambios() {
        let mut p = app();
        pulsa(&mut p, Ui::Nuevo);
        assert!(p.pregunta.is_none(), "sin cambios no pregunta");
        arrastra(&mut p, (10, 10), (30, 30));
        assert_eq!(p.title(), "Sin título* - Paint");
        pulsa(&mut p, Ui::Nuevo);
        assert_eq!(p.pregunta, Some(Accion::Nuevo));
        // Con el aviso abierto, dibujar no hace nada; Cancelar lo cierra y deja el dibujo.
        let n = p.deshacer.len();
        arrastra(&mut p, (100, 100), (120, 120));
        assert_eq!(p.deshacer.len(), n);
        pulsa(&mut p, Ui::PCancelar);
        assert!(p.pregunta.is_none() && px(&p, 10, 10) == 0);
        // No guardar: lienzo en blanco.
        pulsa(&mut p, Ui::Nuevo);
        pulsa(&mut p, Ui::PDescartar);
        assert!(p.lienzo.iter().all(|&c| c == 0xFFFFFF));
        assert_eq!((p.title().as_str(), p.deshacer.len()), ("Sin título - Paint", 0));
    }

    #[test]
    fn guardar_bmp_y_titulo() {
        let mut p = app();
        arrastra(&mut p, (10, 10), (30, 30));
        tecla(&mut p, Key::Char('s'), true);
        assert_eq!(p.title(), "Guardando… - Paint");
        // Cancelar el diálogo deja las cosas como estaban.
        con(|cx| p.dialog_result(DialogResult::Path { tag: TAG_GUARDADO, path: None }, cx));
        assert_eq!(p.title(), "Sin título* - Paint");
        // Guardar de verdad y luego Nuevo no pregunta.
        p.guardar(None);
        con(|cx| p.dialog_result(DialogResult::Path { tag: TAG_GUARDADO, path: Some(String::from("dibujo.bmp")) }, cx));
        assert_eq!(p.title(), "Guardado - Paint");
        pulsa(&mut p, Ui::Nuevo);
        assert!(p.pregunta.is_none());
        // Error al guardar.
        arrastra(&mut p, (10, 10), (30, 30));
        p.guardar(None);
        con(|cx| p.dialog_result(DialogResult::Answer { tag: TAG_GUARDADO, answer: Answer::No }, cx));
        assert_eq!(p.title(), "No se pudo guardar - Paint");
        // Si el sistema no acepta la petición, también es error.
        p.guardador = ocupado;
        p.guardar(None);
        assert_eq!(p.estado, Estado::Error);
    }

    #[test]
    fn el_bmp_guardado_es_el_dibujo() {
        fn comprueba(nombre: &str, datos: &[u8]) -> bool {
            assert_eq!(nombre, "dibujo.bmp");
            assert_eq!(&datos[..2], b"BM");
            assert_eq!(u32::from_le_bytes([datos[18], datos[19], datos[20], datos[21]]), CW as u32);
            assert_eq!(u32::from_le_bytes([datos[22], datos[23], datos[24], datos[25]]), CH as u32);
            assert_eq!(u16::from_le_bytes([datos[28], datos[29]]), 24);
            assert_eq!(datos.len(), 54 + (CW as usize * 3).div_ceil(4) * 4 * CH as usize);
            // Primera fila guardada = la de abajo del dibujo (la esquina inferior izquierda: azul 1F5FD0).
            assert_eq!(&datos[54..57], &[0xD0, 0x5F, 0x1F]);
            true
        }
        let mut p = Paint::new(comprueba);
        p.size = (920, 620);
        p.lienzo[((CH - 1) * CW) as usize] = 0x1F5FD0;
        p.guardar(None);
        assert_eq!(p.estado, Estado::Guardando);
    }

    #[test]
    fn cerrar_con_cambios_pregunta() {
        let mut p = app();
        assert!(con(|cx| p.request_close(cx)), "limpio: se cierra");
        arrastra(&mut p, (10, 10), (30, 30));
        assert!(!con(|cx| p.request_close(cx)));
        assert_eq!(p.pregunta, Some(Accion::Cerrar));
        // Guardar y cerrar: cierra cuando el guardado termina bien.
        pulsa(&mut p, Ui::PGuardar);
        assert_eq!((p.estado, p.tras_guardar), (Estado::Guardando, Some(Accion::Cerrar)));
        assert!(!con(|cx| p.request_close(cx)), "mientras guarda no se cierra");
        let cerrado = con(|cx| {
            p.dialog_result(DialogResult::Path { tag: TAG_GUARDADO, path: Some(String::from("dibujo.bmp")) }, cx);
            cx.close
        });
        assert!(cerrado);
        // No guardar y cerrar.
        let mut p = app();
        arrastra(&mut p, (10, 10), (30, 30));
        con(|cx| p.request_close(cx));
        let cerrado = con(|cx| {
            let r = p.zonas().into_iter().find(|z| z.0 == Ui::PDescartar).unwrap().1;
            p.mouse(MouseEv::Down { x: r.x + 3, y: r.y + 3 }, cx);
            p.mouse(MouseEv::Up { x: r.x + 3, y: r.y + 3 }, cx);
            cx.close
        });
        assert!(cerrado);
    }

    #[test]
    fn barras_de_desplazamiento_y_tamanos_raros() {
        let mut p = app();
        let mut sys = TestPlatform::default();
        let th = Theme::default();
        // 920×620 cabe sin barras; más pequeño, con ellas.
        let mut s = Surface::new(920, 620, 0);
        p.draw(&mut s, &th, &mut sys, true);
        assert_eq!(p.barras(), (false, false));
        let mut s = Surface::new(640, 440, 0);
        p.draw(&mut s, &th, &mut sys, true);
        assert_eq!(p.barras(), (true, true));
        con(|cx| p.mouse(MouseEv::Wheel { x: 200, y: 200, dz: 100 }, cx));
        assert_eq!(p.sy, p.metricas().0.max(), "la rueda se para en el final");
        for (w, h) in [(620, 420), (1, 1), (0, 0), (100, 900), (2000, 100)] {
            let mut s = Surface::new(w, h, 0);
            p.draw(&mut s, &th, &mut sys, true);
            con(|cx| {
                p.mouse(MouseEv::Down { x: 5, y: 5 }, cx);
                p.mouse(MouseEv::Move { x: 400, y: 300 }, cx);
                p.mouse(MouseEv::Up { x: 400, y: 300 }, cx);
                p.tick(cx);
            });
        }
        // Dibujar con el lienzo desplazado cae donde apunta el ratón.
        let mut p = app();
        let mut s = Surface::new(640, 440, 0);
        p.draw(&mut s, &th, &mut sys, true);
        p.sx = 100;
        p.sy = 50;
        p.grosor = 0;
        let a = p.area();
        con(|cx| {
            p.mouse(MouseEv::Down { x: a.x + 20, y: a.y + 20 }, cx);
            p.mouse(MouseEv::Up { x: a.x + 20, y: a.y + 20 }, cx);
        });
        assert_eq!(px(&p, 100 + 20 - MARGEN, 50 + 20 - MARGEN), 0);
    }
}
