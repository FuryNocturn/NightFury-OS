// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Configuración PCI por el mecanismo de puertos 0xCF8 (dirección) y 0xCFC (datos).
use crate::arch::{inl, outl};

const DIR: u16 = 0xCF8;
const DATOS: u16 = 0xCFC;

fn dir(bus: u8, dev: u8, fun: u8, reg: u8) -> u32 {
    0x8000_0000 | (bus as u32) << 16 | (dev as u32) << 11 | (fun as u32) << 8 | (reg & 0xFC) as u32
}

pub fn leer(bus: u8, dev: u8, fun: u8, reg: u8) -> u32 {
    unsafe {
        outl(DIR, dir(bus, dev, fun, reg));
        inl(DATOS)
    }
}

pub fn escribir(bus: u8, dev: u8, fun: u8, reg: u8, v: u32) {
    unsafe {
        outl(DIR, dir(bus, dev, fun, reg));
        outl(DATOS, v);
    }
}

/// Todas las funciones PCI de una clase (`clase << 16 | subclase << 8 | interfaz`), como (bus, dispositivo, función).
pub fn buscar(codigo: u32) -> alloc::vec::Vec<(u8, u8, u8)> {
    let mut v = alloc::vec::Vec::new();
    for bus in 0..=255u8 {
        for dev in 0..32u8 {
            if leer(bus, dev, 0, 0) & 0xFFFF == 0xFFFF {
                continue;
            }
            let multi = (leer(bus, dev, 0, 0x0C) >> 16) & 0x80 != 0;
            for fun in 0..(if multi { 8 } else { 1 }) {
                if leer(bus, dev, fun, 0) & 0xFFFF != 0xFFFF && leer(bus, dev, fun, 0x08) >> 8 == codigo {
                    v.push((bus, dev, fun));
                }
            }
        }
    }
    v
}

/// Busca el primer controlador AHCI (clase 0x01, subclase 0x06, interfaz 0x01), activa
/// espacio de memoria y bus master y devuelve la dirección base de BAR5 (ABAR).
pub fn buscar_ahci() -> Option<u64> {
    for bus in 0..=255u8 {
        for dev in 0..32u8 {
            if leer(bus, dev, 0, 0) & 0xFFFF == 0xFFFF {
                continue;
            }
            let multi = (leer(bus, dev, 0, 0x0C) >> 16) & 0x80 != 0;
            for fun in 0..(if multi { 8 } else { 1 }) {
                if leer(bus, dev, fun, 0) & 0xFFFF == 0xFFFF {
                    continue;
                }
                if leer(bus, dev, fun, 0x08) >> 8 != 0x01_06_01 {
                    continue;
                }
                let bar = leer(bus, dev, fun, 0x24);
                if bar & 1 != 0 || bar & !0xF == 0 {
                    continue; // BAR5 debe ser de memoria y estar asignado
                }
                let cmd = leer(bus, dev, fun, 0x04);
                escribir(bus, dev, fun, 0x04, cmd | 0b110);
                return Some((bar & !0xF) as u64);
            }
        }
    }
    None
}
