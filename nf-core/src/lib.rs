// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! NightFury: todo lo que no toca hardware. Se prueba en el PC con `cargo test -p nf-core`.
#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod abi;
pub mod accounts;
pub mod apps;
pub mod block;
pub mod cipher;
pub mod cryptdev;
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
pub mod homecrypt;
pub mod icons;
pub mod input;
pub mod keys;
pub mod login;
pub mod math;
pub mod menu;
pub mod nfx;
pub mod platform;
pub mod proc;
pub mod programs;
pub mod ribbon;
pub mod scrollbar;
pub mod surface;
pub mod sync;
pub mod theme;
pub mod wire;

/// Nombre del sistema. Único sitio donde se define (ver docs/CLEANROOM.md).
pub const NAME: &str = "NightFury";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Crédito obligatorio (GPL-3.0 §7(b), ver NOTICE): los derivados lo conservan visible en "Acerca de".
pub const CREDIT: &str = "Basado en NightFury-OS de FuryNocturn";
pub const CREDIT_URL: &str = "https://github.com/FuryNocturn/NightFury-OS";
pub const LICENSE_LINE: &str = "Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn). Licencia GPL-3.0.";
