// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! USB: controladores xHCI (`xhci.rs`), enumeración, hubs y clases (`dev.rs`).
//!
//! Todo va por sondeo y de forma bloqueante con tiempos de espera: no hay interrupciones del
//! controlador. Interfaz con `main.rs`: `init()` y `poll()`.
//!
//! Para añadir un driver de clase (p. ej. almacenamiento masivo): una variante en `Uso`, su caso en
//! `Uso::de` (qué interfaz reclama), en `dev::completada` (qué hace al terminar una transferencia) y,
//! si hace falta preparar algo al configurar, en `dev::preparar`. El controlador no se toca: ofrece
//! `Xhci::control`, `Xhci::normal` (bulk/interrupción) y `Xhci::esperar`.
mod dev;
mod xhci;

use crate::arch::{inb, outb};
use crate::{paging, pci};
use crate::serial::traza;
use alloc::alloc::{alloc_zeroed, dealloc, Layout};
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use nf_core::input::MousePacket;
use nf_core::usbhid::BootKeyboard;
use xhci::Xhci;

/// Memoria para DMA: alineada y a cero. El núcleo es de mapeo identidad: físico = virtual.
pub struct Dma {
    p: *mut u8,
    l: Layout,
}

impl Dma {
    pub fn nuevo(tam: usize, alin: usize) -> Option<Dma> {
        let l = Layout::from_size_align(tam.max(1), alin).ok()?;
        let p = unsafe { alloc_zeroed(l) };
        (!p.is_null()).then_some(Dma { p, l })
    }
    pub fn dir(&self) -> u64 {
        self.p as u64
    }
    pub fn ptr<T>(&self) -> *mut T {
        self.p as *mut T
    }
}

impl Drop for Dma {
    fn drop(&mut self) {
        unsafe { dealloc(self.p, self.l) }
    }
}

/// Qué hace el núcleo con un endpoint (decidido por la clase de su interfaz).
pub enum Uso {
    Nada,
    Teclado(BootKeyboard),
    Raton,
    Hub,
}

impl Uso {
    /// Clase, subclase y protocolo de una interfaz -> driver que la reclama.
    fn de(clase: u8, sub: u8, proto: u8) -> Option<Uso> {
        match (clase, sub, proto) {
            (3, 1, 1) => Some(Uso::Teclado(BootKeyboard::new())),
            (3, 1, 2) => Some(Uso::Raton),
            (9, _, _) => Some(Uso::Hub),
            _ => None,
        }
    }
}

// ---------- Tiempo ----------
// Antes de `sti` no corre el temporizador del sistema, así que se lee el contador del PIT (canal 0,
// modo 3: baja de 2 en 2 desde 11932 y da la vuelta cada 5 ms). Si no se consulta con más
// frecuencia que cada 5 ms se pierde tiempo: los plazos salen más largos, nunca más cortos.

static PIT_ANT: AtomicU32 = AtomicU32::new(0);
static PIT_ACUM: AtomicU64 = AtomicU64::new(0);

pub fn ahora_us() -> u64 {
    let c = unsafe {
        outb(0x43, 0);
        inb(0x40) as u32 | (inb(0x40) as u32) << 8
    };
    let ant = PIT_ANT.swap(c, Relaxed);
    let d = if c <= ant { ant - c } else { ant + 11932 - c } as u64;
    // Cada unidad es medio ciclo de 1,193182 MHz.
    (PIT_ACUM.fetch_add(d, Relaxed) + d) * 500_000 / 1_193_182
}

pub struct Plazo(u64);

impl Plazo {
    pub fn ms(n: u64) -> Plazo {
        Plazo(ahora_us() + n * 1000)
    }
    pub fn vencido(&self) -> bool {
        ahora_us() >= self.0
    }
}

pub fn espera_ms(n: u64) {
    let p = Plazo::ms(n);
    while !p.vencido() {
        core::hint::spin_loop();
    }
}

/// Espera hasta `ms` a que se cumpla `c`. `false` si se agota el plazo.
pub fn esperar(ms: u64, mut c: impl FnMut() -> bool) -> bool {
    let p = Plazo::ms(ms);
    loop {
        if c() {
            return true;
        }
        if p.vencido() {
            return c();
        }
        core::hint::spin_loop();
    }
}

// ---------- Interfaz con main.rs ----------

struct Estado(UnsafeCell<Vec<Xhci>>);
// Un solo núcleo; solo `init` y `poll` (bucle principal, nunca una interrupción) lo tocan.
unsafe impl Sync for Estado {}
static USB: Estado = Estado(UnsafeCell::new(Vec::new()));

/// Intel Panther Point/Lynx Point/Wildcat Point...: sus puertos USB 2/3 se reparten entre EHCI y xHCI
/// con registros de configuración PCI propios (0xD0/0xD8; máscaras en 0xD4/0xDC).
const INTEL_CONMUTADOR: [u16; 6] = [0x1E31, 0x8C31, 0x9C31, 0x8CB1, 0x9CB1, 0x8D31];

/// Con las interrupciones cerradas. Devuelve cuántos controladores xHCI han arrancado.
pub fn init() -> usize {
    let v = unsafe { &mut *USB.0.get() };
    for (bus, dev, fun) in pci::buscar(0x0C_03_30) {
        let id = pci::leer(bus, dev, fun, 0);
        let b0 = pci::leer(bus, dev, fun, 0x10);
        let mut base = (b0 & !0xF) as u64;
        if (b0 >> 1) & 3 == 2 {
            base |= (pci::leer(bus, dev, fun, 0x14) as u64) << 32;
        }
        // Se mapea 1 MiB desde la BAR sin caché (paging.rs solo cubre la RAM y los primeros 4 GiB).
        if b0 & 1 != 0 || base == 0 || base >= 1 << 46 || !paging::mapear_mmio(base, 0x10_0000) {
            traza!("NF usb: {:02x}:{:02x}.{} {:04x}:{:04x} BAR0 inutilizable ({:#x}), se salta", bus, dev, fun, id & 0xFFFF, id >> 16, base);
            continue;
        }
        let cmd = pci::leer(bus, dev, fun, 0x04);
        pci::escribir(bus, dev, fun, 0x04, cmd | 0b110);
        if id & 0xFFFF == 0x8086 && INTEL_CONMUTADOR.contains(&((id >> 16) as u16)) {
            pci::escribir(bus, dev, fun, 0xD8, pci::leer(bus, dev, fun, 0xDC));
            pci::escribir(bus, dev, fun, 0xD0, pci::leer(bus, dev, fun, 0xD4));
        }
        match Xhci::nuevo(v.len(), base) {
            Some(x) => {
                traza!("NF usb: controlador {} en {:02x}:{:02x}.{} {:04x}:{:04x} base={:#x}", v.len(), bus, dev, fun, id & 0xFFFF, id >> 16, base);
                v.push(x);
            }
            None => traza!("NF usb: {:02x}:{:02x}.{} {:04x}:{:04x} no arranca", bus, dev, fun, id & 0xFFFF, id >> 16),
        }
    }
    // Con el controlador ya en marcha, se enumera lo que haya conectado.
    for x in v.iter_mut() {
        dev::explorar(x);
    }
    v.len()
}

/// Procesa eventos, reencola transferencias y deja bytes set 1 del teclado y paquetes del ratón.
// shortcut: sondeo sin MSI, pasar a MSI/APIC cuando llegue la red
pub fn poll(now_ms: u64, teclado: &mut Vec<u8>, raton: &mut Vec<MousePacket>) {
    for x in unsafe { &mut *USB.0.get() }.iter_mut() {
        dev::servir(x, now_ms, teclado, raton);
    }
}
