// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Reloj CMOS (MC146818).
use crate::arch::{inb, outb};
use nf_core::platform::DateTime;

fn leer(reg: u8) -> u8 {
    unsafe {
        outb(0x70, reg);
        inb(0x71)
    }
}

pub fn now() -> DateTime {
    // Espera (con límite) a que no haya una actualización en curso.
    for _ in 0..100_000 {
        if leer(0x0A) & 0x80 == 0 {
            break;
        }
    }
    let (s, mi, h, d, mo, y) = (leer(0), leer(2), leer(4), leer(7), leer(8), leer(9));
    let cfg = leer(0x0B);
    let bin = cfg & 4 != 0;
    let h24 = cfg & 2 != 0;
    let pm = !h24 && h & 0x80 != 0;
    let conv = |v: u8| if bin { v } else { (v >> 4) * 10 + (v & 15) };
    let mut hour = conv(h & 0x7F);
    if !h24 {
        hour %= 12;
        if pm {
            hour += 12;
        }
    }
    DateTime { year: 2000 + conv(y) as u16, month: conv(mo), day: conv(d), hour, minute: conv(mi), second: conv(s) }
}

fn escribir(reg: u8, v: u8) {
    unsafe {
        outb(0x70, reg);
        outb(0x71, v);
    }
}

/// Pone el reloj (hora local) respetando el formato BCD/binario y 12/24 h que ya tiene. El bit SET
/// (7 del registro 0x0B) lo para mientras se escribe, para que no se actualice a medias.
pub fn poner(t: DateTime) {
    let cfg = leer(0x0B);
    let (bin, h24) = (cfg & 4 != 0, cfg & 2 != 0);
    let enc = |v: u8| if bin { v } else { (v / 10) << 4 | (v % 10) };
    let h = if h24 { enc(t.hour) } else { enc(if t.hour % 12 == 0 { 12 } else { t.hour % 12 }) | if t.hour >= 12 { 0x80 } else { 0 } };
    escribir(0x0B, cfg | 0x80);
    for (reg, v) in [(0, enc(t.second)), (2, enc(t.minute)), (4, h), (7, enc(t.day)), (8, enc(t.month)), (9, enc((t.year % 100) as u8))] {
        escribir(reg, v);
    }
    escribir(0x0B, cfg & !0x80);
}
