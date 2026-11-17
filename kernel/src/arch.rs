// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! x86_64: puertos, GDT, IDT, PIC 8259, temporizador PIT 8254 y rutinas de entrada.
use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

pub static TICKS: AtomicU64 = AtomicU64::new(0);

pub const VEC_TIMER: usize = 32;
pub const VEC_TECLADO: usize = 33;
pub const VEC_RATON: usize = 44;

/// # Safety
/// Escribir en un puerto de E/S puede tener cualquier efecto sobre el hardware.
#[inline]
pub unsafe fn outb(port: u16, v: u8) {
    asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack, preserves_flags));
}

/// # Safety
/// Leer un puerto de E/S puede tener efectos secundarios en el hardware.
#[inline]
pub unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack, preserves_flags));
    v
}

/// # Safety
/// Escribir en un puerto de E/S puede tener cualquier efecto sobre el hardware.
#[inline]
pub unsafe fn outl(port: u16, v: u32) {
    asm!("out dx, eax", in("dx") port, in("eax") v, options(nomem, nostack, preserves_flags));
}

/// # Safety
/// Leer un puerto de E/S puede tener efectos secundarios en el hardware.
#[inline]
pub unsafe fn inl(port: u16) -> u32 {
    let v: u32;
    asm!("in eax, dx", in("dx") port, out("eax") v, options(nomem, nostack, preserves_flags));
    v
}

// ---------- Rutinas de entrada ----------

/// Rutina de una IRQ: guarda los registros y el estado SSE, llama al manejador y vuelve.
/// 5 qwords de la CPU + 9 registros = 112 bytes, y 544 más: la pila queda alineada a 16.
macro_rules! irq_stub {
    ($name:literal, $handler:literal) => {
        global_asm!(concat!(
            ".global ", $name, "\n",
            $name, ":\n",
            "push rax\npush rcx\npush rdx\npush rsi\npush rdi\n",
            "push r8\npush r9\npush r10\npush r11\n",
            "sub rsp, 544\n",
            "fxsave64 [rsp+32]\n",
            "cld\n",
            "lea rdi, [rsp+616]\n", // marco de la CPU (rip, cs, ...): solo lo mira el temporizador
            "call ", $handler, "\n",
            "fxrstor64 [rsp+32]\n",
            "add rsp, 544\n",
            "pop r11\npop r10\npop r9\npop r8\npop rdi\npop rsi\npop rdx\npop rcx\npop rax\n",
            "iretq\n",
        ));
    };
}

irq_stub!("irq_timer_stub", "timer_handler");
irq_stub!("irq_teclado_stub", "teclado_handler");
irq_stub!("irq_raton_stub", "raton_handler");

global_asm!(".global iret_stub\niret_stub:\niretq\n");

macro_rules! exc_stubs {
    ($($n:literal)*) => {
        $(
            global_asm!(concat!(
                "exc_stub_", $n, ":\n",
                "mov ecx, ", $n, "\n",
                "mov rdx, rsp\n",
                "and rsp, -16\n",
                "sub rsp, 32\n",
                "call fault_handler\n",
                "ud2\n",
            ));
        )*
        global_asm!(concat!(
            ".data\n.balign 8\n.global exc_stub_table\nexc_stub_table:\n",
            $(".quad exc_stub_", $n, "\n",)*
            ".text\n",
        ));
    };
}

exc_stubs!(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31);

extern "C" {
    static exc_stub_table: [u64; 32];
    fn irq_timer_stub();
    fn irq_teclado_stub();
    fn irq_raton_stub();
    fn iret_stub();
}

// ---------- Manejadores (los llaman las rutinas de arriba) ----------

#[no_mangle]
extern "sysv64" fn timer_handler(frame: *const u64) {
    TICKS.fetch_add(1, Relaxed);
    eoi(0);
    // frame[1] = CS interrumpido: si era un programa (ring 3), mira si lleva demasiado sin esperar.
    // La rutina de entrada nos pasa un marco válido (5 qwords de la CPU).
    if unsafe { *frame.add(1) } & 3 == 3 {
        crate::user::tick_en_usuario();
    }
}

#[no_mangle]
extern "C" fn teclado_handler() {
    crate::ps2::irq();
    eoi(1);
}

#[no_mangle]
extern "C" fn raton_handler() {
    crate::ps2::irq();
    eoi(12);
}

/// Fin de interrupción.
fn eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            outb(0xA0, 0x20);
        }
        outb(0x20, 0x20);
    }
}

// ---------- GDT ----------

#[repr(C, packed)]
struct Ptr {
    limit: u16,
    base: u64,
}

/// Nula, código y datos del núcleo (0x08, 0x10), datos y código de usuario (0x18, 0x20; con
/// `sysret` salen como 0x1B y 0x23) y el TSS (0x28, ocupa dos entradas).
static mut GDT: [u64; 7] = [0, 0x00AF_9A00_0000_FFFF, 0x00CF_9200_0000_FFFF, 0x00CF_F200_0000_FFFF, 0x00AF_FA00_0000_FFFF, 0, 0];

#[repr(C, align(16))]
struct Tss([u32; 26]);
static mut TSS: Tss = Tss([0; 26]);

/// Pila aparte para el fallo doble (IST1), por si la pila del núcleo está rota.
#[repr(C, align(16))]
struct PilaFd([u8; 16384]);
static mut PILA_FD: PilaFd = PilaFd([0; 16384]);

/// Pila del núcleo para cuando un programa entra por una interrupción o excepción (TSS.RSP0).
pub fn set_rsp0(v: u64) {
    // Un solo núcleo; solo se llama antes de entrar al programa (interrupciones cerradas).
    unsafe {
        let t = &mut (*core::ptr::addr_of_mut!(TSS)).0;
        t[1] = v as u32;
        t[2] = (v >> 32) as u32;
    }
}

pub fn init_gdt() {
    // Arranque, un solo núcleo: nadie más toca la GDT ni el TSS.
    unsafe {
        let tss = core::ptr::addr_of_mut!(TSS) as u64;
        let t = &mut (*core::ptr::addr_of_mut!(TSS)).0;
        let top = core::ptr::addr_of_mut!(PILA_FD) as u64 + 16384;
        t[9] = top as u32;
        t[10] = (top >> 32) as u32;
        t[25] = 104 << 16; // sin mapa de E/S
        let g = &mut *core::ptr::addr_of_mut!(GDT);
        // TSS de 64 bits libre (tipo 0x89), límite 103.
        g[5] = 103 | (tss & 0xFFFF) << 16 | (tss >> 16 & 0xFF) << 32 | 0x89 << 40 | (tss >> 24 & 0xFF) << 56;
        g[6] = tss >> 32;
    }
    let p = Ptr { limit: (7 * 8 - 1) as u16, base: core::ptr::addr_of!(GDT) as u64 };
    unsafe {
        asm!(
            "lgdt [{p}]",
            "push 0x08",
            "lea rax, [rip + 2f]",
            "push rax",
            "retfq",
            "2:",
            "mov ax, 0x10",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            "mov fs, ax",
            "mov gs, ax",
            p = in(reg) &p,
            out("rax") _,
        );
        asm!("ltr ax", in("ax") 0x28u16, options(nomem, nostack));
    }
}

// ---------- IDT ----------

#[repr(C)]
#[derive(Clone, Copy)]
struct Entry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

static mut IDT: [Entry; 256] =
    [Entry { offset_low: 0, selector: 0, ist: 0, type_attr: 0, offset_mid: 0, offset_high: 0, reserved: 0 }; 256];

fn entry(addr: u64) -> Entry {
    Entry {
        offset_low: addr as u16,
        selector: 0x08,
        ist: 0,
        type_attr: 0x8E,
        offset_mid: (addr >> 16) as u16,
        offset_high: (addr >> 32) as u32,
        reserved: 0,
    }
}

pub fn init_idt() {
    unsafe {
        let idt = &mut *core::ptr::addr_of_mut!(IDT);
        for e in idt.iter_mut() {
            *e = entry(iret_stub as *const () as u64);
        }
        for (i, e) in idt.iter_mut().enumerate().take(32) {
            *e = entry(exc_stub_table[i]);
        }
        idt[8].ist = 1; // fallo doble
        idt[VEC_TIMER] = entry(irq_timer_stub as *const () as u64);
        idt[VEC_TECLADO] = entry(irq_teclado_stub as *const () as u64);
        idt[VEC_RATON] = entry(irq_raton_stub as *const () as u64);
        let p = Ptr { limit: (256 * 16 - 1) as u16, base: idt.as_ptr() as u64 };
        asm!("lidt [{p}]", p = in(reg) &p);
    }
}

// ---------- PIC y PIT ----------

fn espera_io() {
    unsafe { outb(0x80, 0) };
}

pub fn init_pic() {
    unsafe {
        outb(0x20, 0x11);
        espera_io();
        outb(0xA0, 0x11);
        espera_io();
        outb(0x21, 0x20);
        espera_io();
        outb(0xA1, 0x28);
        espera_io();
        outb(0x21, 4);
        espera_io();
        outb(0xA1, 2);
        espera_io();
        outb(0x21, 1);
        espera_io();
        outb(0xA1, 1);
        espera_io();
        outb(0x21, 0xF8);
        outb(0xA1, 0xEF);
    }
}

/// Temporizador a 100 Hz.
pub fn init_pit() {
    unsafe {
        outb(0x43, 0x36);
        outb(0x40, (11932u16 & 0xFF) as u8);
        outb(0x40, (11932u16 >> 8) as u8);
    }
}

/// Espera a la próxima interrupción si no hay trabajo pendiente.
pub fn idle(hay_trabajo: impl Fn() -> bool) {
    unsafe {
        asm!("cli", options(nomem, nostack));
        if hay_trabajo() {
            asm!("sti", options(nomem, nostack));
        } else {
            asm!("sti", "hlt", options(nomem, nostack));
        }
    }
}
