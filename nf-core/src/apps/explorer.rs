//! Explorador de archivos. Provisional: lo sustituye la app de verdad.
//! CONTRATO: `pub fn new() -> Box<dyn App>`.
use super::{App, AppKind, Ctx, MouseEv};
use crate::input::KeyEvent;
use crate::platform::Platform;
use crate::surface::Surface;
use crate::theme::Theme;
use alloc::boxed::Box;
use alloc::string::String;

pub fn new() -> Box<dyn App> {
    Box::new(Explorer)
}

struct Explorer;

impl App for Explorer {
    fn kind(&self) -> AppKind {
        AppKind::Files
    }
    fn title(&self) -> String {
        String::from("Explorador de archivos")
    }
    fn min_size(&self) -> (i32, i32) {
        (420, 260)
    }
    fn draw(&mut self, s: &mut Surface, _theme: &Theme, _sys: &mut dyn Platform, _focused: bool) {
        let b = s.bounds();
        s.fill_rect(b, 0xFFFFFF);
    }
    fn key(&mut self, _ev: &KeyEvent, _cx: &mut Ctx) {}
    fn mouse(&mut self, _ev: MouseEv, _cx: &mut Ctx) {}
    fn tick(&mut self, _cx: &mut Ctx) {}
}
