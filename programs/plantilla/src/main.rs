//! PLANTILLA de programa NightFury. Copia esta carpeta, cambia el nombre en `Cargo.toml` y en
//! `app.txt`, y reescribe `Plantilla` a tu gusto. Ver docs/KIT-programas.md.
//!
//! Un programa es una `App`: el sistema te avisa de cada tecla, clic y cambio de tamaño, y tú
//! dibujas todo el contenido de la ventana en `draw`. Todo lo demás (arranque, eventos, memoria,
//! cerrar) lo pone `nf-app`.
#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use nf_core::apps::{App, AppKind, Ctx, MouseEv};
use nf_core::font::{self, Align};
use nf_core::geom::Rect;
use nf_core::input::{Key, KeyEvent};
use nf_core::platform::Platform;
use nf_core::surface::Surface;
use nf_core::theme::Theme;

struct Plantilla {
    clics: u32,
}

impl App for Plantilla {
    /// Los programas siempre son `AppKind::Program`.
    fn kind(&self) -> AppKind {
        AppKind::Program
    }

    /// Título de la ventana. Si cambia entre eventos, el sistema lo actualiza solo.
    fn title(&self) -> String {
        String::from("Plantilla")
    }

    /// Tamaño mínimo del contenido de la ventana.
    fn min_size(&self) -> (i32, i32) {
        (240, 120)
    }

    /// Dibuja TODO el contenido (`s.w` x `s.h`). Se llama al cambiar el tamaño, el foco o el tema,
    /// y cuando pides redibujar con `cx.redraw = true`.
    fn draw(&mut self, s: &mut Surface, theme: &Theme, _sys: &mut dyn Platform, _focused: bool) {
        s.fill_rect(Rect::new(0, 0, s.w, s.h), 0xFFFFFF);
        // `theme.accent` es el color de énfasis que el usuario eligió en Configuración.
        s.fill_rect(Rect::new(0, 0, s.w, 6), theme.accent);
        let r = Rect::new(0, 0, s.w, s.h);
        font::draw_in(s, r, "Hola desde NightFury", 14, true, 0x1A1A1A, Align::Center);
        let r = Rect::new(0, s.h / 2 + 20, s.w, 24);
        font::draw_in(s, r, &format!("Clics: {}  (Espacio suma, Esc cierra)", self.clics), 10, false, 0x6A6A6A, Align::Center);
    }

    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
        match ev.key {
            Key::Char(' ') => {
                self.clics += 1;
                cx.redraw = true;
            }
            // Para cerrar la ventana desde el programa: `cx.close = true`.
            Key::Escape => cx.close = true,
            _ => {}
        }
    }

    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
        // `x`, `y` son relativos a la esquina superior izquierda del contenido.
        if let MouseEv::Down { .. } = ev {
            self.clics += 1;
            cx.redraw = true;
        }
    }

    /// Solo se llama si pides tick en `programa!` (el segundo argumento); aquí no hace falta.
    fn tick(&mut self, _cx: &mut Ctx) {}
}

// Declara el programa: cómo crear la app y cada cuántos ms quieres `tick` (`None` = nunca).
nf_app::programa!(Box::new(Plantilla { clics: 0 }), None);
