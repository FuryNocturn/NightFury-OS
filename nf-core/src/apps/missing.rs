// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Ventana que sale si falta el programa de una app del sistema (v0.10: el núcleo ya no las lleva
//! dentro). No debería verse: el sistema reinstala sus programas al arrancar.
use super::{name, App, AppKind, Ctx, MouseEv};
use crate::font;
use crate::input::KeyEvent;
use crate::platform::Platform;
use crate::surface::Surface;
use crate::theme::Theme;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;

struct Missing(AppKind);

pub fn new(kind: AppKind) -> Box<dyn App> {
    Box::new(Missing(kind))
}

impl App for Missing {
    fn kind(&self) -> AppKind {
        self.0
    }
    fn title(&self) -> String {
        String::from(name(self.0))
    }
    fn min_size(&self) -> (i32, i32) {
        (300, 80)
    }
    fn draw(&mut self, s: &mut Surface, _theme: &Theme, _sys: &mut dyn Platform, _focused: bool) {
        s.fill_rect(crate::geom::Rect::new(0, 0, s.w, s.h), 0xFFFFFF);
        font::draw(s, 20, 24, &format!("No se encuentra el programa «{}».", name(self.0)), 10, true, 0x1A1A1A);
        font::draw(s, 20, 50, "Debería estar en C:\\Programas. Reinicia el equipo para reinstalarlo.", 9, false, 0x555555);
    }
    fn key(&mut self, _ev: &KeyEvent, _cx: &mut Ctx) {}
    fn mouse(&mut self, _ev: MouseEv, _cx: &mut Ctx) {}
    fn tick(&mut self, _cx: &mut Ctx) {}
}
