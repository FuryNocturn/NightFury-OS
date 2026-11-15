//! Bloc de notas. Provisional: lo sustituye la app de verdad. CONTRATO: `pub fn new() -> Box<dyn App>`.
use super::{App, AppKind, Placeholder};
use alloc::boxed::Box;

pub fn new() -> Box<dyn App> {
    Box::new(Placeholder(AppKind::Notepad))
}
