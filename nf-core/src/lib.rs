//! NightFury: todo lo que no toca hardware. Se prueba en el PC con `cargo test -p nf-core`.
#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod accounts;
pub mod apps;
pub mod block;
pub mod crypto;
pub mod bmp;
pub mod dialogs;
pub mod files;
pub mod fs;
pub mod desktop;
pub mod font;
pub mod geom;
pub mod glyphs;
pub mod heap;
pub mod icons;
pub mod input;
pub mod login;
pub mod math;
pub mod menu;
pub mod platform;
pub mod ribbon;
pub mod scrollbar;
pub mod surface;
pub mod sync;
pub mod theme;

/// Nombre del sistema. Único sitio donde se define (ver docs/CLEANROOM.md).
pub const NAME: &str = "NightFury";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
