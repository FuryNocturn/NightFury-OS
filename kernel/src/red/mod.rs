// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Tarjetas de red de cable (v0.12), por sondeo y sin interrupciones. Cada controlador implementa
//! `nf_core::net::NetDev`; la pila (`nf_core::net::Net`) no sabe cuál es.
//!
//! CONTRATO: `sondear` no cambia de firma.
use crate::serial::traza;
use crate::{pci, usb};
use alloc::boxed::Box;
use core::ptr::{read_volatile, write_volatile};
use nf_core::net::NetDev;

mod intel;
mod realtek;
mod virtio;

/// Lectura y escritura MMIO de las tarjetas, sobre regiones que cada controlador ha mapeado.
pub(super) fn rd<T: Copy>(a: usize) -> T {
    unsafe { read_volatile(a as *const T) }
}
pub(super) fn wr<T: Copy>(a: usize, v: T) {
    unsafe { write_volatile(a as *mut T, v) }
}

/// Plazo para que una tarjeta sin cable enlace antes de pasar a la siguiente.
const ESPERA_ENLACE_MS: u64 = 3_000;

/// Busca en el PCI las tarjetas compatibles, inicia la primera que tenga cable (si ninguna lo tiene,
/// la primera que arranque) y la devuelve. `None` si no hay ninguna.
pub fn sondear() -> Option<Box<dyn NetDev>> {
    let mut sin_cable: Option<Box<dyn NetDev>> = None;
    for (bus, dev, fun) in pci::buscar(0x02_00_00) {
        let id = pci::leer(bus, dev, fun, 0);
        let (v, d) = (id as u16, (id >> 16) as u16);
        let iniciar = match () {
            _ if virtio::soporta(v, d) => virtio::iniciar,
            _ if intel::soporta(v, d) => intel::iniciar,
            _ if realtek::soporta(v, d) => realtek::iniciar,
            _ => continue,
        };
        let Some(mut tarjeta) = iniciar(bus, dev, fun) else {
            traza!("NF red: {:02x}:{:02x}.{} {:04x}:{:04x} no arranca, se salta", bus, dev, fun, v, d);
            continue;
        };
        if usb::esperar(ESPERA_ENLACE_MS, || tarjeta.link().is_some()) {
            return Some(tarjeta);
        }
        traza!("NF red: {} sin enlace", tarjeta.name());
        sin_cable.get_or_insert(tarjeta);
    }
    sin_cable
}
