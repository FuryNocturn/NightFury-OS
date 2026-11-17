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
