//! Apps del sistema. CONTRATO: `AppKind`, `APPS`, `MouseEv`, `Ctx`, `App` y las firmas de
//! `create`/`default_size` no cambian.
use crate::icons::Icon;
use crate::input::KeyEvent;
use crate::platform::Platform;
use crate::surface::Surface;
use crate::theme::Theme;
use alloc::boxed::Box;
use alloc::string::String;

mod calc;
mod notepad;
mod settings;
mod terminal;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppKind {
    Notepad,
    Calc,
    Settings,
    Terminal,
}

/// Apps instaladas, en orden alfabético: (tipo, nombre visible, icono).
pub const APPS: [(AppKind, &str, Icon); 4] = [
    (AppKind::Notepad, "Bloc de notas", Icon::Notepad),
    (AppKind::Calc, "Calculadora", Icon::Calc),
    (AppKind::Settings, "Configuración", Icon::Settings),
    (AppKind::Terminal, "Terminal", Icon::Terminal),
];

/// Eventos de ratón, en coordenadas relativas al área de contenido de la ventana.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseEv {
    Down { x: i32, y: i32 },
    Up { x: i32, y: i32 },
    Move { x: i32, y: i32 },
    /// El puntero ha salido del contenido.
    Leave,
}

/// Lo que una app puede usar y pedir al atender un evento.
pub struct Ctx<'a> {
    pub sys: &'a mut dyn Platform,
    /// La app puede cambiarlo; si lo hace, pone `theme_changed = true`.
    pub theme: &'a mut Theme,
    pub now_ms: u64,
    /// La app pide que se vuelva a dibujar su contenido.
    pub redraw: bool,
    /// La app pide cerrar su ventana.
    pub close: bool,
    pub theme_changed: bool,
}

pub trait App {
    fn kind(&self) -> AppKind;
    fn title(&self) -> String;
    /// Tamaño mínimo del área de contenido.
    fn min_size(&self) -> (i32, i32);
    /// Dibuja TODO el contenido en `s` (s.w x s.h es el área de contenido actual).
    /// Debe recordar el tamaño si lo necesita para atender el ratón.
    fn draw(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, focused: bool);
    /// Solo se llama con teclas pulsadas (`down == true`).
    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx);
    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx);
    /// Se llama unas 100 veces por segundo; `cx.now_ms` es el reloj.
    fn tick(&mut self, cx: &mut Ctx);
}

/// Tamaño inicial del área de contenido de cada app.
pub fn default_size(kind: AppKind) -> (i32, i32) {
    match kind {
        AppKind::Notepad => (640, 420),
        AppKind::Calc => (320, 470),
        AppKind::Settings => (780, 520),
        AppKind::Terminal => (680, 400),
    }
}

pub fn create(kind: AppKind) -> Box<dyn App> {
    match kind {
        AppKind::Notepad => notepad::new(),
        AppKind::Calc => calc::new(),
        AppKind::Settings => settings::new(),
        AppKind::Terminal => terminal::new(),
    }
}
