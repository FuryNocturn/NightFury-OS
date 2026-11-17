// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Controlador SATA (AHCI) por sondeo, sin interrupciones. Una orden cada vez, en la ranura 0.
use crate::serial::traza;
use alloc::alloc::{alloc_zeroed, Layout};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};
use nf_core::block::{BlockDev, DiskError, BLOCK};

const SECTORES_BLOQUE: u64 = (BLOCK / 512) as u64;
/// Vueltas máximas de espera; la espera es por contador porque el arranque va sin interrupciones.
const ESPERA: u32 = 20_000_000;

/// Un disco SATA de sectores de 512 bytes, ya con su motor de órdenes en marcha.
pub struct Disco {
    puerto: u32,
    base: usize, // registros del puerto
    lista: *mut u8,
    tabla: *mut u8,
    buf: *mut u8, // búfer intermedio de BLOCK bytes alineado a 4096
    sectores: u64,
}

/// Memoria propia alineada y a cero.
fn pedir(tam: usize, alin: usize) -> *mut u8 {
    unsafe { alloc_zeroed(Layout::from_size_align(tam, alin).unwrap()) }
}

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}

fn esperar(mut cond: impl FnMut() -> bool) -> bool {
    for _ in 0..ESPERA {
        if cond() {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

// Registros del puerto
const CLB: usize = 0x00;
const CLBU: usize = 0x04;
const FB: usize = 0x08;
const FBU: usize = 0x0C;
const IS: usize = 0x10;
const IE: usize = 0x14;
const CMD: usize = 0x18;
const TFD: usize = 0x20;
const SIG: usize = 0x24;
const SSTS: usize = 0x28;
const SERR: usize = 0x30;
const CI: usize = 0x38;

const CMD_ST: u32 = 1;
const CMD_FRE: u32 = 1 << 4;
const CMD_FR: u32 = 1 << 14;
const CMD_CR: u32 = 1 << 15;

impl Disco {
    /// Lanza una orden de 48 bits. `datos` = bytes a transferir (0 = sin datos).
    fn orden(&mut self, cmd: u8, lba: u64, sectores: u16, datos: usize, escribe: bool) -> Result<(), DiskError> {
        let p = self.base;
        if !esperar(|| rd(p + TFD) & 0x88 == 0) {
            return Err(DiskError::Io);
        }
        unsafe {
            // Cabecera de la ranura 0: FIS de 5 palabras, escritura, 1 PRDT (si hay datos) y tabla.
            let h = self.lista as *mut u32;
            let prdt = (datos != 0) as u32;
            write_volatile(h, 5 | (escribe as u32) << 6 | prdt << 16);
            write_volatile(h.add(1), 0);
            write_volatile(h.add(2), self.tabla as u64 as u32);
            write_volatile(h.add(3), (self.tabla as u64 >> 32) as u32);
            // FIS Register Host to Device.
            let f = self.tabla;
            core::ptr::write_bytes(f, 0, 64);
            *f = 0x27;
            *f.add(1) = 0x80;
            *f.add(2) = cmd;
            for i in 0..3 {
                *f.add(4 + i) = (lba >> (8 * i)) as u8;
                *f.add(8 + i) = (lba >> (24 + 8 * i)) as u8;
            }
            *f.add(7) = 0x40;
            *f.add(12) = sectores as u8;
            *f.add(13) = (sectores >> 8) as u8;
            if datos != 0 {
                let e = self.tabla.add(0x80) as *mut u32;
                write_volatile(e, self.buf as u64 as u32);
                write_volatile(e.add(1), (self.buf as u64 >> 32) as u32);
                write_volatile(e.add(2), 0);
                write_volatile(e.add(3), (datos - 1) as u32);
            }
        }
        wr(p + IS, 0xFFFF_FFFF);
        wr(p + SERR, 0xFFFF_FFFF);
        wr(p + CI, 1);
        let hecho = esperar(|| rd(p + CI) & 1 == 0 || rd(p + IS) & (1 << 30) != 0);
        if !hecho || rd(p + IS) & (1 << 30) != 0 || rd(p + TFD) & 1 != 0 {
            // Se detiene el motor para que no quede una orden colgada y se reinicia.
            wr(p + CMD, rd(p + CMD) & !CMD_ST);
            esperar(|| rd(p + CMD) & CMD_CR == 0);
            wr(p + SERR, 0xFFFF_FFFF);
            wr(p + IS, 0xFFFF_FFFF);
            wr(p + CMD, rd(p + CMD) | CMD_ST);
            return Err(DiskError::Io);
        }
        Ok(())
    }

    fn bloque(&mut self, n: u64, escribe: bool) -> Result<(), DiskError> {
        let c = if escribe { 0x35 } else { 0x25 };
        self.orden(c, n * SECTORES_BLOQUE, SECTORES_BLOQUE as u16, BLOCK, escribe)
    }

    pub fn puerto(&self) -> u32 {
        self.puerto
    }
}

impl BlockDev for Disco {
    fn blocks(&self) -> u64 {
        self.sectores / SECTORES_BLOQUE
    }
    fn read(&mut self, n: u64, buf: &mut [u8]) -> Result<(), DiskError> {
        if n >= self.blocks() || buf.len() != BLOCK {
            return Err(DiskError::Range);
        }
        self.bloque(n, false)?;
        buf.copy_from_slice(unsafe { core::slice::from_raw_parts(self.buf, BLOCK) });
        Ok(())
    }
    fn write(&mut self, n: u64, buf: &[u8]) -> Result<(), DiskError> {
        if n >= self.blocks() || buf.len() != BLOCK {
            return Err(DiskError::Range);
        }
        unsafe { core::slice::from_raw_parts_mut(self.buf, BLOCK) }.copy_from_slice(buf);
        self.bloque(n, true)
    }
    fn flush(&mut self) -> Result<(), DiskError> {
        self.orden(0xEA, 0, 0, 0, false)
    }
}

/// Prepara un puerto con disco SATA; `None` (con traza) si no sirve.
fn iniciar(hba: usize, p: u32, c64: bool) -> Option<Disco> {
    let b = hba + 0x100 + p as usize * 0x80;
    if rd(b + SIG) != 0x0000_0101 || rd(b + SSTS) & 0xF != 3 {
        return None;
    }
    // Parar el motor de órdenes.
    wr(b + CMD, rd(b + CMD) & !(CMD_ST | CMD_FRE));
    if !esperar(|| rd(b + CMD) & (CMD_CR | CMD_FR) == 0) {
        traza!("AHCI puerto {}: el motor no se detiene, descartado", p);
        return None;
    }
    let lista = pedir(1024, 1024);
    let fis = pedir(256, 256);
    let tabla = pedir(0x80 + 16, 128);
    let buf = pedir(BLOCK, 4096);
    if lista.is_null() || fis.is_null() || tabla.is_null() || buf.is_null() {
        traza!("AHCI puerto {}: sin memoria, descartado", p);
        return None;
    }
    let alto = [lista, fis, tabla, buf].iter().any(|&x| x as u64 >> 32 != 0);
    if alto && !c64 {
        traza!("AHCI puerto {}: memoria por encima de 4 GiB y el controlador es de 32 bits, descartado", p);
        return None;
    }
    wr(b + CLB, lista as u64 as u32);
    wr(b + CLBU, (lista as u64 >> 32) as u32);
    wr(b + FB, fis as u64 as u32);
    wr(b + FBU, (fis as u64 >> 32) as u32);
    wr(b + SERR, 0xFFFF_FFFF);
    wr(b + IS, 0xFFFF_FFFF);
    wr(b + IE, 0);
    wr(b + CMD, rd(b + CMD) | CMD_FRE);
    wr(b + CMD, rd(b + CMD) | CMD_ST);
    let mut d = Disco { puerto: p, base: b, lista, tabla, buf, sectores: 0 };
    // IDENTIFY DEVICE
    if d.orden(0xEC, 0, 0, 512, false).is_err() {
        traza!("AHCI puerto {}: IDENTIFY falla, descartado", p);
        return None;
    }
    let w: Vec<u16> = (0..256).map(|i| unsafe { read_volatile((d.buf as *const u16).add(i)) }).collect();
    if w[83] & (1 << 10) == 0 {
        traza!("AHCI puerto {}: sin LBA de 48 bits, descartado", p);
        return None;
    }
    d.sectores = (w[100] as u64) | (w[101] as u64) << 16 | (w[102] as u64) << 32 | (w[103] as u64) << 48;
    if w[106] & 0xC000 == 0x4000 && w[106] & (1 << 12) != 0 {
        traza!("AHCI puerto {}: sectores lógicos que no son de 512 bytes, descartado", p);
        return None;
    }
    if d.blocks() == 0 {
        traza!("AHCI puerto {}: disco demasiado pequeño, descartado", p);
        return None;
    }
    Some(d)
}

/// Todos los discos SATA utilizables del controlador, en orden de puerto.
pub fn sondear(abar: u64) -> Vec<Box<Disco>> {
    let hba = abar as usize;
    let mut v = Vec::new();
    let cap = rd(hba);
    wr(hba + 0x04, rd(hba + 0x04) | 1 << 31);
    let pi = rd(hba + 0x0C);
    for p in 0..32 {
        if pi & (1 << p) != 0 {
            if let Some(d) = iniciar(hba, p, cap & (1 << 31) != 0) {
                v.push(Box::new(d));
            }
        }
    }
    v
}
