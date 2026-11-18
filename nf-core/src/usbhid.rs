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

/// Uso HID (página 0x07) -> scan code set 1. Bit 7 = tecla extendida (prefijo 0xE0); 0 = sin tecla.
/// La 0x31 (`\`) y la 0x32 (`#` no US) comparten código en PS/2; la 0x64 es la tecla `<>`.
const SET1: [u8; 0x66] = {
    let mut t = [0u8; 0x66];
    let letras = *b"\x1E\x30\x2E\x20\x12\x21\x22\x23\x17\x24\x25\x26\x32\x31\x18\x19\x10\x13\x1F\x14\x16\x2F\x11\x2D\x15\x2C";
    let mut i = 0;
    while i < 26 {
        t[4 + i] = letras[i];
        i += 1;
    }
    i = 0;
    while i < 10 {
        t[0x1E + i] = 2 + i as u8; // 1..9, 0 = 0x0B
        t[0x3A + i] = 0x3B + i as u8; // F1..F10
        i += 1;
    }
    // 0x28..=0x39: Intro, Esc, Retroceso, Tab, Espacio, - = [ ] \ #, ; ' ` , . /, Bloq Mayús
    let b = [0x1C, 0x01, 0x0E, 0x0F, 0x39, 0x0C, 0x0D, 0x1A, 0x1B, 0x2B, 0x2B, 0x27, 0x28, 0x29, 0x33, 0x34, 0x35, 0x3A];
    i = 0;
    while i < b.len() {
        t[0x28 + i] = b[i];
        i += 1;
    }
    t[0x44] = 0x57; // F11
    t[0x45] = 0x58; // F12
    t[0x47] = 0x46; // Bloq Despl
    // Insert, Inicio, RePág, Supr, Fin, AvPág, derecha, izquierda, abajo, arriba (extendidas)
    let e = [0x52, 0x47, 0x49, 0x53, 0x4F, 0x51, 0x4D, 0x4B, 0x50, 0x48];
    i = 0;
    while i < e.len() {
        t[0x49 + i] = 0x80 | e[i];
        i += 1;
    }
    t[0x53] = 0x45; // Bloq Num
    t[0x54] = 0x80 | 0x35; // numérico /
    t[0x55] = 0x37; // numérico *
    t[0x56] = 0x4A; // numérico -
    t[0x57] = 0x4E; // numérico +
    t[0x58] = 0x80 | 0x1C; // numérico Intro
    let kp = [0x4F, 0x50, 0x51, 0x4B, 0x4C, 0x4D, 0x47, 0x48, 0x49, 0x52, 0x53]; // 1..9, 0, .
    i = 0;
    while i < kp.len() {
        t[0x59 + i] = kp[i];
        i += 1;
    }
    t[0x64] = 0x56; // `<>` del teclado español
    t[0x65] = 0x80 | 0x5D; // Menú
    t
};

/// Bits del byte de modificadores -> (extendida, código set 1): Ctrl, Mayús, Alt, Win; izq. y der.
const MODS: [(bool, u8); 8] =
    [(false, 0x1D), (false, 0x2A), (false, 0x38), (true, 0x5B), (true, 0x1D), (false, 0x36), (true, 0x38), (true, 0x5C)];

fn emite(out: &mut Vec<u8>, ext: bool, code: u8, make: bool) {
    if ext {
        out.push(0xE0);
    }
    out.push(if make { code } else { code | 0x80 });
}

fn emite_uso(out: &mut Vec<u8>, uso: u8, make: bool) {
    if let Some(&c) = SET1.get(uso as usize).filter(|&&c| c != 0) {
        emite(out, c & 0x80 != 0, c & 0x7F, make);
    }
}

const RETARDO_MS: u64 = 500;
const REPETIR_MS: u64 = 33;

impl BootKeyboard {
    pub fn new() -> Self {
        BootKeyboard { prev: [0; 8], repeat: None }
    }

    /// Informe de arranque de 8 bytes (modificadores, reservado, 6 teclas). Añade a `out` los bytes
    /// set 1 de lo que se pulsó y soltó desde el informe anterior. Informes de "rollover" (todas las
    /// teclas a 0x01) se ignoran.
    pub fn feed(&mut self, report: &[u8], now_ms: u64, out: &mut Vec<u8>) {
        if report.len() < 8 || report[2..8].iter().all(|&k| k == 1) {
            return;
        }
        let (old, new) = (self.prev, &report[..8]);
        // Modificadores primero (Mayús antes de la letra), luego las teclas.
        for (i, &(ext, code)) in MODS.iter().enumerate() {
            let (a, b) = (old[0] >> i & 1, new[0] >> i & 1);
            if a != b {
                emite(out, ext, code, b == 1);
            }
        }
        for &k in old[2..8].iter().filter(|&&k| k > 3 && !new[2..8].contains(&k)) {
            emite_uso(out, k, false);
            if matches!(self.repeat, Some((r, _)) if r == k) {
                self.repeat = None;
            }
        }
        for &k in new[2..8].iter().filter(|&&k| k > 3 && !old[2..8].contains(&k)) {
            emite_uso(out, k, true);
            // Solo repite una tecla que existe en la tabla.
            self.repeat = SET1.get(k as usize).filter(|&&c| c != 0).map(|_| (k, now_ms + RETARDO_MS));
        }
        self.prev.copy_from_slice(new);
    }

    /// Repetición automática de la tecla mantenida.
    pub fn tick(&mut self, now_ms: u64, out: &mut Vec<u8>) {
        if let Some((k, next)) = &mut self.repeat {
            if now_ms >= *next {
                emite_uso(out, *k, true);
                *next = now_ms + REPETIR_MS; // sin ráfaga de recuperación si el sondeo se retrasó
            }
        }
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
    if report.len() < 3 {
        return None;
    }
    // La rueda USB positiva es "hacia delante" (contenido arriba): nuestro dz positivo es hacia abajo.
    let dz = report.get(3).map_or(0, |&w| -(w as i8 as i32));
    Some(MousePacket { dx: report[1] as i8 as i32, dy: report[2] as i8 as i32, buttons: report[0] & 7, dz })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Key, Keyboard};

    fn rep(m: u8, k: &[u8]) -> [u8; 8] {
        let mut r = [0u8; 8];
        r[0] = m;
        r[2..2 + k.len()].copy_from_slice(k);
        r
    }
    fn feed(kb: &mut BootKeyboard, r: [u8; 8], t: u64) -> Vec<u8> {
        let mut o = Vec::new();
        kb.feed(&r, t, &mut o);
        o
    }
    fn tick(kb: &mut BootKeyboard, t: u64) -> Vec<u8> {
        let mut o = Vec::new();
        kb.tick(t, &mut o);
        o
    }

    #[test]
    fn letra_y_soltar() {
        let mut kb = BootKeyboard::new();
        assert_eq!(feed(&mut kb, rep(0, &[0x04]), 0), [0x1E]);
        assert_eq!(feed(&mut kb, rep(0, &[]), 10), [0x9E]);
    }

    #[test]
    fn tabla_completa() {
        // Cada letra, dígito y F1..F12 tiene código; los extremos de la tabla también.
        for u in (0x04..=0x45).chain(0x47..=0x47).chain(0x49..=0x65) {
            assert_ne!(SET1[u], 0, "uso {:#x}", u);
        }
        assert_eq!([SET1[0x1D], SET1[0x1E], SET1[0x27], SET1[0x2C], SET1[0x45]], [0x2C, 0x02, 0x0B, 0x39, 0x58]);
    }

    #[test]
    fn modificadores_antes_que_tecla() {
        let mut kb = BootKeyboard::new();
        // Mayús izq. (bit 1) + b; después AltGr (bit 6, extendida) y Win der. (bit 7).
        assert_eq!(feed(&mut kb, rep(2, &[0x05]), 0), [0x2A, 0x30]);
        assert_eq!(feed(&mut kb, rep(0x42, &[0x05]), 1), [0xE0, 0x38]);
        assert_eq!(feed(&mut kb, rep(0xC2, &[0x05]), 2), [0xE0, 0x5C]);
        assert_eq!(feed(&mut kb, rep(0, &[]), 3), [0xAA, 0xE0, 0xB8, 0xE0, 0xDC, 0xB0]);
    }

    #[test]
    fn extendidas_y_especiales() {
        let mut kb = BootKeyboard::new();
        assert_eq!(feed(&mut kb, rep(0, &[0x52]), 0), [0xE0, 0x48]); // flecha arriba
        assert_eq!(feed(&mut kb, rep(0, &[0x58]), 1), [0xE0, 0xC8, 0xE0, 0x1C]); // Intro numérico
        assert_eq!(feed(&mut kb, rep(0, &[0x64, 0x32]), 2), [0xE0, 0x9C, 0x56, 0x2B]); // `<>` y ç
        assert_eq!(feed(&mut kb, rep(0, &[0x59]), 3), [0xD6, 0xAB, 0x4F]);
    }

    #[test]
    fn rollover_se_ignora() {
        let mut kb = BootKeyboard::new();
        feed(&mut kb, rep(0, &[0x04]), 0);
        assert!(feed(&mut kb, rep(0, &[1, 1, 1, 1, 1, 1]), 5).is_empty());
        // El estado anterior se conserva: soltar 'a' sigue generando su break.
        assert_eq!(feed(&mut kb, rep(0, &[]), 6), [0x9E]);
    }

    #[test]
    fn repeticion() {
        let mut kb = BootKeyboard::new();
        feed(&mut kb, rep(0, &[0x04]), 1000);
        assert!(tick(&mut kb, 1499).is_empty());
        assert_eq!(tick(&mut kb, 1500), [0x1E]);
        assert!(tick(&mut kb, 1520).is_empty());
        assert_eq!(tick(&mut kb, 1533), [0x1E]);
        // Una segunda tecla toma la repetición (con su retardo); al soltarla se cancela.
        feed(&mut kb, rep(0, &[0x04, 0x05]), 1540);
        assert!(tick(&mut kb, 2039).is_empty());
        assert_eq!(tick(&mut kb, 2040), [0x30]);
        feed(&mut kb, rep(0, &[0x04]), 2100);
        assert!(tick(&mut kb, 9000).is_empty());
        // Los modificadores no repiten.
        let mut kb = BootKeyboard::new();
        feed(&mut kb, rep(2, &[]), 0);
        assert!(tick(&mut kb, 9000).is_empty());
    }

    #[test]
    fn con_el_teclado_de_entrada() {
        let mut kb = BootKeyboard::new();
        let mut k = Keyboard::new();
        let mut evs = Vec::new();
        for (r, t) in [(rep(0, &[0x64]), 0), (rep(0, &[]), 1), (rep(2, &[0x64]), 2), (rep(0, &[0x52]), 3)] {
            for b in feed(&mut kb, r, t) {
                evs.extend(k.feed(b));
            }
        }
        assert_eq!(evs.iter().find(|e| e.down && e.ch.is_some()).unwrap().ch, Some('<'));
        assert!(evs.iter().any(|e| e.ch == Some('>')));
        assert!(evs.iter().any(|e| e.key == Key::Up && e.down));
    }

    #[test]
    fn raton() {
        assert_eq!(boot_mouse(&[1, 5, 0xFD]), Some(MousePacket { dx: 5, dy: -3, buttons: 1, dz: 0 }));
        assert_eq!(boot_mouse(&[0xFF, 0xFE, 4, 1]), Some(MousePacket { dx: -2, dy: 4, buttons: 7, dz: -1 }));
        assert_eq!(boot_mouse(&[0, 0, 0, 0xFD]).unwrap().dz, 3);
        assert_eq!(boot_mouse(&[0, 0]), None);
    }
}
