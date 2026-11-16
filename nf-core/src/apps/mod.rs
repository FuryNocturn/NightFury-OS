//! Apps del sistema. CONTRATO: `AppKind`, `APPS`, `MouseEv`, `Ctx`, `App` y las firmas de
//! `create`/`default_size` no cambian.
use crate::dialogs::{DialogRequest, DialogResult};
use crate::icons::Icon;
use crate::input::KeyEvent;
use crate::menu::MenuRequest;
use crate::platform::Platform;
use crate::surface::Surface;
use crate::theme::Theme;
use alloc::boxed::Box;
use alloc::string::String;

mod calc;
mod explorer;
mod notepad;
mod settings;
mod terminal;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppKind {
    Notepad,
    Calc,
    Settings,
    /// Explorador de archivos.
    Files,
    Terminal,
}

/// Apps instaladas, en orden alfabético: (tipo, nombre visible, icono).
pub const APPS: [(AppKind, &str, Icon); 5] = [
    (AppKind::Notepad, "Bloc de notas", Icon::Notepad),
    (AppKind::Calc, "Calculadora", Icon::Calc),
    (AppKind::Settings, "Configuración", Icon::Settings),
    (AppKind::Files, "Explorador de archivos", Icon::Folder),
    (AppKind::Terminal, "Terminal", Icon::Terminal),
];

/// Eventos de ratón, en coordenadas relativas al área de contenido de la ventana.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseEv {
    Down { x: i32, y: i32 },
    Up { x: i32, y: i32 },
    Move { x: i32, y: i32 },
    /// Botón derecho pulsado (para el menú contextual).
    RightDown { x: i32, y: i32 },
    /// Rueda: `dz` en muescas, positivo = hacia abajo (el contenido sube).
    Wheel { x: i32, y: i32, dz: i32 },
    /// El puntero ha salido del contenido.
    Leave,
}

/// Lo que una app puede usar y pedir al atender un evento. Se crea con `Ctx::new`.
/// Los archivos se usan con `cx.sys.files()`.
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
    /// La app pide un cuadro de diálogo modal; la respuesta llega por `App::dialog_result`.
    pub dialog: Option<DialogRequest>,
    /// La app pide abrir una ruta: una carpeta (o `files::TRASH`) se abre en el Explorador y
    /// un archivo en el Bloc de notas (`App::open_path` de una ventana nueva).
    pub open: Option<String>,
    /// La app pide abrir una carpeta en una ventana NUEVA del Explorador.
    pub open_new: Option<String>,
    /// La app pide un menú contextual; la elección llega por `App::menu_result`.
    pub menu: Option<MenuRequest>,
    /// La app pide abrir un Terminal nuevo situado en esta carpeta (`App::open_path`).
    pub open_terminal: Option<String>,
}

impl<'a> Ctx<'a> {
    pub fn new(sys: &'a mut dyn Platform, theme: &'a mut Theme, now_ms: u64) -> Self {
        Ctx { sys, theme, now_ms, redraw: false, close: false, theme_changed: false, dialog: None, open: None, open_new: None, menu: None, open_terminal: None }
    }
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
    /// Respuesta al diálogo pedido con `cx.dialog`.
    fn dialog_result(&mut self, _result: DialogResult, _cx: &mut Ctx) {}
    /// Elemento elegido en el menú pedido con `cx.menu` (si se cierra sin elegir no se llama).
    fn menu_result(&mut self, _tag: u32, _id: u32, _cx: &mut Ctx) {}
    /// Abre una ruta absoluta ("C:\\..." o `files::TRASH`) en esta ventana.
    fn open_path(&mut self, _path: &str, _cx: &mut Ctx) {}
    /// El usuario quiere cerrar la ventana. `true` = cerrar ya. `false` = la app se ocupa
    /// (p. ej. pide un diálogo de "cambios sin guardar") y pondrá `cx.close` cuando toque.
    fn request_close(&mut self, _cx: &mut Ctx) -> bool {
        true
    }
}

/// Tamaño inicial del área de contenido de cada app.
pub fn default_size(kind: AppKind) -> (i32, i32) {
    match kind {
        AppKind::Notepad => (640, 420),
        AppKind::Calc => (320, 470),
        AppKind::Settings => (780, 520),
        AppKind::Files => (1040, 540),
        AppKind::Terminal => (680, 400),
    }
}

pub fn create(kind: AppKind) -> Box<dyn App> {
    match kind {
        AppKind::Notepad => notepad::new(),
        AppKind::Calc => calc::new(),
        AppKind::Settings => settings::new(),
        AppKind::Files => explorer::new(),
        AppKind::Terminal => terminal::new(),
    }
}
