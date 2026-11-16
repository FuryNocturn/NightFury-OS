//! Controlador PS/2 (8042): teclado y ratón por interrupciones, con colas sin cerrojo.
use crate::arch::{inb, outb};
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering::*};

const DATO: u16 = 0x60;
const CMD: u16 = 0x64;
const LIMITE: u32 = 100_000;

/// Cola circular de 256 bytes: la interrupción escribe, el bucle principal lee.
pub struct Cola {
    buf: [AtomicU8; 256],
    cab: AtomicUsize,
    cola: AtomicUsize,
}

#[allow(clippy::declare_interior_mutable_const)]
const CERO: AtomicU8 = AtomicU8::new(0);

impl Cola {
    const fn new() -> Self {
        Cola { buf: [CERO; 256], cab: AtomicUsize::new(0), cola: AtomicUsize::new(0) }
    }
    fn push(&self, b: u8) {
        let c = self.cab.load(Relaxed);
        if c.wrapping_sub(self.cola.load(Acquire)) >= 256 {
            return; // llena: se pierde el byte
        }
        self.buf[c % 256].store(b, Relaxed);
        self.cab.store(c.wrapping_add(1), Release);
    }
    pub fn pop(&self) -> Option<u8> {
        let t = self.cola.load(Relaxed);
        if t == self.cab.load(Acquire) {
            return None;
        }
        let b = self.buf[t % 256].load(Relaxed);
        self.cola.store(t.wrapping_add(1), Release);
        Some(b)
    }
    pub fn vacia(&self) -> bool {
        self.cola.load(Relaxed) == self.cab.load(Acquire)
    }
}

pub static TECLADO: Cola = Cola::new();
pub static RATON: Cola = Cola::new();

fn espera_escribir() {
    for _ in 0..LIMITE {
        if unsafe { inb(CMD) } & 2 == 0 {
            return;
        }
    }
}

fn espera_leer() -> bool {
    for _ in 0..LIMITE {
        if unsafe { inb(CMD) } & 1 != 0 {
            return true;
        }
    }
    false
}

fn orden(c: u8) {
    espera_escribir();
    unsafe { outb(CMD, c) };
}

fn dato(d: u8) {
    espera_escribir();
    unsafe { outb(DATO, d) };
}

fn vaciar() {
    for _ in 0..64 {
        if unsafe { inb(CMD) } & 1 == 0 {
            break;
        }
        unsafe { inb(DATO) };
    }
}

fn al_raton(b: u8) {
    orden(0xD4);
    dato(b);
    if espera_leer() {
        unsafe { inb(DATO) }; // respuesta 0xFA
    }
}

/// Con las interrupciones cerradas.
pub fn init() {
    orden(0xAD);
    orden(0xA7);
    vaciar();
    orden(0x20);
    let cfg = if espera_leer() { unsafe { inb(DATO) } } else { 0 };
    let cfg = (cfg | 0x43) & !0x30;
    orden(0x60);
    dato(cfg);
    orden(0xAE);
    orden(0xA8);
    // Teclado: activar el envío de teclas (0xF4) y leer su confirmación.
    dato(0xF4);
    if espera_leer() {
        unsafe { inb(DATO) };
    }
    al_raton(0xF6);
    al_raton(0xF4);
    vaciar();
}

/// Manejador de las IRQ 1 y 12: vacía el 8042 hacia la cola que toque.
pub fn irq() {
    for _ in 0..16 {
        let st = unsafe { inb(CMD) };
        if st & 1 == 0 {
            break;
        }
        let b = unsafe { inb(DATO) };
        if st & 0x20 != 0 {
            RATON.push(b);
        } else {
            TECLADO.push(b);
        }
    }
}
