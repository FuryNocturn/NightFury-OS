// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Informes HID de arranque (USB) traducidos a lo que ya entiende el sistema.
//!
//! CONTRATO (v0.11): `BootKeyboard::{new, feed, tick}` y `boot_mouse` no cambian de firma.
//! - El teclado USB se traduce a bytes de scan code set 1 (con prefijos `0xE0` y los códigos de
//!   soltar `| 0x80`), que se pasan al mismo `input::Keyboard` que usa el PS/2: así la
//!   distribución española y las teclas muertas son las mismas.
//! - USB no repite teclas solo (el PS/2 sí): `tick` genera la repetición de la última tecla
//!   pulsada (retardo 500 ms, luego cada 33 ms), como haría el teclado PS/2.
use crate::input::MousePacket;
use alloc::vec::Vec;

pub struct BootKeyboard {
    prev: [u8; 8],
    /// Última tecla con repetición: (uso HID, próximo instante en ms).
    repeat: Option<(u8, u64)>,
}

impl BootKeyboard {
    pub fn new() -> Self {
        BootKeyboard { prev: [0; 8], repeat: None }
    }

    /// Informe de arranque de 8 bytes (modificadores, reservado, 6 teclas). Añade a `out` los bytes
    /// set 1 de lo que se pulsó y soltó desde el informe anterior. Informes de "rollover" (todas las
    /// teclas a 0x01) se ignoran.
    pub fn feed(&mut self, report: &[u8], now_ms: u64, out: &mut Vec<u8>) {
        let _ = (report, now_ms, out, &self.prev, &self.repeat);
        todo!("v0.11: encargo A")
    }

    /// Repetición automática de la tecla mantenida.
    pub fn tick(&mut self, now_ms: u64, out: &mut Vec<u8>) {
        let _ = (now_ms, out);
        todo!("v0.11: encargo A")
    }
}

impl Default for BootKeyboard {
    fn default() -> Self {
        Self::new()
    }
}

/// Informe de arranque de ratón (botones, dx, dy y, si viene, rueda) a `MousePacket`.
/// `dy` USB es positivo hacia abajo: se entrega con el mismo convenio que `Ps2Mouse`.
pub fn boot_mouse(report: &[u8]) -> Option<MousePacket> {
    let _ = report;
    todo!("v0.11: encargo A")
}
