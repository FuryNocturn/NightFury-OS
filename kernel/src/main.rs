// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! NightFury: núcleo. Solo aporta hardware (vídeo, interrupciones, PS/2, reloj, memoria);
//! todo lo demás vive en `nf-core`.
#![no_std]
#![no_main]

extern crate alloc;

mod ahci;
mod arch;
#[cfg(feature = "demo")]
mod demo;
mod boot;
mod fb;
mod paging;
mod pci;
mod ps2;
mod rtc;
mod serial;
mod uefi;
mod user;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering::*};
use fb::Fb;
use nf_core::desktop::Desktop;
use nf_core::block::{BlockDev, BLOCK};
use nf_core::files::Files;
use nf_core::fs::Nffs;
use nf_core::geom::Rect;
use nf_core::heap::Heap;
use nf_core::icons::{self, Icon};
use nf_core::input::{Event, Keyboard, Ps2Mouse};
use nf_core::platform::{DateTime, MemInfo, Platform};
use nf_core::proc::{ProcInfo, Process};
use nf_core::surface::Surface;
use nf_core::{font, NAME};
use serial::traza;

/// Trazas por COM1 al arrancar.
const TRAZA: bool = true;
/// Traza además cada byte crudo de teclado y ratón.
const TRAZA_BYTES: bool = false;
// ---------- Montón ----------

struct Global(UnsafeCell<Heap>);
// Un solo núcleo y las interrupciones no reservan memoria.
unsafe impl Sync for Global {}

static HEAP: Global = Global(UnsafeCell::new(Heap::empty()));
static HEAP_LISTO: AtomicBool = AtomicBool::new(false);

struct Asignador;

unsafe impl GlobalAlloc for Asignador {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        (*HEAP.0.get()).alloc(l.size(), l.align())
    }
    unsafe fn dealloc(&self, p: *mut u8, _l: Layout) {
        (*HEAP.0.get()).free(p)
    }
}

/// Programas que trae el sistema (los compila `tools\programas.ps1` antes que el núcleo).
const PROGRAMAS_SISTEMA: &[(&str, &[(&str, &[u8])])] = &[
    (
        "Calculadora",
        &[
            ("app.txt", include_bytes!("../../programs/calculadora/app.txt")),
            ("programa.nfx", include_bytes!("../../target/programas/Calculadora/programa.nfx")),
        ],
    ),
    (
        "Reloj",
        &[
            ("app.txt", include_bytes!("../../programs/reloj/app.txt")),
            ("programa.nfx", include_bytes!("../../target/programas/Reloj/programa.nfx")),
            ("icono.bmp", include_bytes!("../../programs/reloj/icono.bmp")),
        ],
    ),
    (
        "Paint",
        &[
            ("app.txt", include_bytes!("../../programs/paint/app.txt")),
            ("programa.nfx", include_bytes!("../../target/programas/Paint/programa.nfx")),
            ("icono.bmp", include_bytes!("../../programs/paint/icono.bmp")),
        ],
    ),
    // v0.10: las apps que antes iban dentro del núcleo.
    (
        "Bloc de notas",
        &[
            ("app.txt", include_bytes!("../../programs/bloc-notas/app.txt")),
            ("programa.nfx", include_bytes!("../../target/programas/Bloc de notas/programa.nfx")),
        ],
    ),
    (
        "Terminal",
        &[
            ("app.txt", include_bytes!("../../programs/terminal/app.txt")),
            ("programa.nfx", include_bytes!("../../target/programas/Terminal/programa.nfx")),
        ],
    ),
    (
        "Configuración",
        &[
            ("app.txt", include_bytes!("../../programs/configuracion/app.txt")),
            ("programa.nfx", include_bytes!("../../target/programas/Configuración/programa.nfx")),
        ],
    ),
    (
        "Explorador de archivos",
        &[
            ("app.txt", include_bytes!("../../programs/explorador/app.txt")),
            ("programa.nfx", include_bytes!("../../target/programas/Explorador de archivos/programa.nfx")),
        ],
    ),
];

#[global_allocator]
static ASIGNADOR: Asignador = Asignador;

// ---------- Plataforma ----------

struct Plat {
    info: boot::BootInfo,
    files: Files,
    pool: nf_core::keys::Pool,
}

fn cpu_name() -> String {
    use core::arch::x86_64::__cpuid;
    let mut bytes = [0u8; 48];
    let max = __cpuid(0x8000_0000).eax;
    let n = if max >= 0x8000_0004 {
        for i in 0..3u32 {
            let r = __cpuid(0x8000_0002 + i);
            for (j, v) in [r.eax, r.ebx, r.ecx, r.edx].iter().enumerate() {
                bytes[i as usize * 16 + j * 4..][..4].copy_from_slice(&v.to_le_bytes());
            }
        }
        48
    } else {
        let r = __cpuid(0);
        for (j, v) in [r.ebx, r.edx, r.ecx].iter().enumerate() {
            bytes[j * 4..][..4].copy_from_slice(&v.to_le_bytes());
        }
        12
    };
    let end = bytes[..n].iter().position(|&b| b == 0).unwrap_or(n);
    let mut out = String::new();
    for w in core::str::from_utf8(&bytes[..end]).unwrap_or("").split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

impl Plat {
    fn reset(&self, kind: u32) {
        if !self.info.runtime.is_null() {
            unsafe { ((*self.info.runtime).reset_system)(kind, 0, 0, core::ptr::null()) };
        }
    }
}

impl Platform for Plat {
    fn now(&mut self) -> DateTime {
        rtc::now()
    }
    fn uptime_ms(&self) -> u64 {
        arch::TICKS.load(Relaxed) * 10
    }
    fn mem(&self) -> MemInfo {
        let h = unsafe { &*HEAP.0.get() };
        MemInfo { total: self.info.ram_total, heap_total: h.total() as u64, heap_used: h.used() as u64 }
    }
    fn cpu(&self) -> String {
        cpu_name()
    }
    fn screen(&self) -> (u32, u32) {
        (self.info.width, self.info.height)
    }
    fn reboot(&mut self) {
        self.reset(uefi::RESET_COLD);
        unsafe { arch::outb(0x64, 0xFE) };
    }
    fn shutdown(&mut self) {
        self.reset(uefi::RESET_SHUTDOWN);
    }
    fn files(&mut self) -> &mut Files {
        &mut self.files
    }
    fn pool(&mut self) -> &mut nf_core::keys::Pool {
        &mut self.pool
    }
    fn spawn(&mut self, nfx: &[u8], name: &str) -> Result<Box<dyn Process>, String> {
        user::spawn(nfx, name)
    }
    fn processes(&mut self) -> Vec<ProcInfo> {
        user::lista()
    }
    fn kill_process(&mut self, id: u64) {
        user::finalizar(id)
    }
    fn run_programs(&mut self) {
        // `correr` exige las interrupciones cerradas, como en `arch::idle`.
        unsafe {
            core::arch::asm!("cli", options(nomem, nostack));
            user::correr();
            core::arch::asm!("sti", options(nomem, nostack));
        }
    }
    fn is_system_binary(&self, nfx: &[u8]) -> bool {
        PROGRAMAS_SISTEMA.iter().flat_map(|(_, items)| items.iter()).any(|(n, b)| *n == "programa.nfx" && *b == nfx)
    }
    fn entropy(&mut self) -> u64 {
        use core::arch::x86_64::{__cpuid, _rdtsc};
        // Contador de ciclos del procesador y reloj del sistema: siempre disponibles.
        #[allow(unused_unsafe)]
        let mut v = unsafe { _rdtsc() } ^ arch::TICKS.load(Relaxed).rotate_left(40);
        // RDRAND (Intel SDM vol. 2, CPUID.01H:ECX bit 30) si el procesador lo tiene.
        #[allow(unused_unsafe)]
        let has = unsafe { __cpuid(1) }.ecx & (1 << 30) != 0;
        if has {
            let r: u64;
            let ok: u8;
            unsafe { core::arch::asm!("rdrand {r}", "setc {ok}", r = out(reg) r, ok = out(reg_byte) ok, options(nomem, nostack)) };
            if ok != 0 {
                v ^= r;
            }
        }
        v
    }
}

// ---------- Disco de datos ----------

/// Sondea PCI/AHCI y ofrece el primer disco SATA que sea de NightFury o esté en blanco.
/// Cualquier otro disco se salta sin escribir nada en él.
fn elegir_disco() -> Option<Box<dyn BlockDev>> {
    let Some(abar) = pci::buscar_ahci() else {
        if TRAZA {
            traza!("NF disco: sin controlador AHCI");
        }
        return None;
    };
    let mut elegido: Option<Box<dyn BlockDev>> = None;
    for mut d in ahci::sondear(abar) {
        let mib = d.blocks() * BLOCK as u64 >> 20;
        let motivo = if Nffs::is_formatted(&mut *d) {
            "NightFury"
        } else if nf_core::cryptdev::detect(&mut *d).is_some() {
            // Cifrado total: el NFFS no se ve, pero la cabecera del último bloque es la nuestra.
            "NightFury cifrado"
        } else if Nffs::is_blank(&mut *d) {
            "en blanco"
        } else {
            ""
        };
        let ofrecer = !motivo.is_empty() && elegido.is_none();
        if TRAZA {
            let r = match (motivo, ofrecer) {
                (_, true) => "se ofrece",
                ("", _) => "se salta: contenido ajeno",
                _ => "se salta: ya hay otro disco ofrecido",
            };
            traza!("NF disco: puerto {} {} MiB {} bloques ({}) {}", d.puerto(), mib, d.blocks(), motivo, r);
        }
        if ofrecer {
            elegido = Some(d);
        }
    }
    if elegido.is_none() && TRAZA {
        traza!("NF disco: ninguno ofrecido");
    }
    elegido
}

// ---------- Núcleo ----------

#[no_mangle]
extern "C" fn kmain() -> ! {
    let info = boot::info();
    let fb = Fb::new(&info);
    if TRAZA {
        serial::init();
    }
    unsafe { (*HEAP.0.get()).init(info.heap_base as *mut u8, info.heap_size as usize) };
    HEAP_LISTO.store(true, Release);
    paging::init(info.ram_end, info.fb_base + info.stride as u64 * info.height as u64 * 4);
    arch::init_gdt();
    arch::init_idt();
    arch::init_pic();
    arch::init_pit();
    user::init_syscall();

    pantalla_de_arranque(&fb);
    let con_rueda = ps2::init();

    let disco = elegir_disco();
    let files = Files::probe(disco, rtc::now());
    if TRAZA {
        traza!("NF archivos: persistente={} formato_pendiente={}", files.persistent(), files.pending_format());
    }
    let mut plat = Plat { info, files, pool: nf_core::keys::Pool::new() };
    let (w, h) = (info.width as i32, info.height as i32);
    let mut desk = Desktop::new(w, h, &mut plat);

    if TRAZA {
        let t = plat.now();
        traza!(
            "NF arranque: {}x{} formato={} pitch={} ram={} MiB heap={} MiB cpu='{}' fecha={:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            w,
            h,
            if info.bgr { "BGR" } else { "RGB" },
            info.stride,
            info.ram_total >> 20,
            info.heap_size >> 20,
            plat.cpu(),
            t.year,
            t.month,
            t.day,
            t.hour,
            t.minute,
            t.second
        );
    }
    #[cfg(feature = "demo")]
    demo::lanzar(&mut desk, &mut plat);
    unsafe { core::arch::asm!("sti") };

    let mut teclado = Keyboard::new();
    let mut raton = Ps2Mouse::new();
    raton.set_wheel(con_rueda);
    let mut ultimo = u64::MAX;
    let mut programas_listos = false;
    loop {
        // Programas del sistema: se instalan (o actualizan) en cuanto el disco está listo.
        if !programas_listos && !plat.files.disk_locked() && !plat.files.pending_format() {
            programas_listos = true;
            let now = plat.now();
            for (dir, items) in PROGRAMAS_SISTEMA {
                if let Err(e) = plat.files.install_system_program(dir, items, now) {
                    traza!("NF programas: no se pudo instalar {}: {:?}", dir, e);
                }
            }
        }
        while let Some(b) = ps2::TECLADO.pop() {
            if TRAZA_BYTES {
                traza!("tecla byte={:#04x}", b);
            }
            if let Some(k) = teclado.feed(b) {
                desk.event(Event::Key(k), &mut plat);
            }
        }
        while let Some(b) = ps2::RATON.pop() {
            if TRAZA_BYTES {
                traza!("raton byte={:#04x}", b);
            }
            if let Some(p) = raton.feed(b) {
                desk.event(Event::Mouse(p), &mut plat);
            }
        }
        let t = arch::TICKS.load(Relaxed);
        if t != ultimo {
            ultimo = t;
            desk.event(Event::Tick(t * 10), &mut plat);
        }
        for r in desk.render(&mut plat) {
            fb.present(desk.back(), r);
        }
        arch::idle(|| !ps2::TECLADO.vacia() || !ps2::RATON.vacia() || arch::TICKS.load(Relaxed) != ultimo);
    }
}

fn pantalla_de_arranque(fb: &Fb) {
    const FONDO: u32 = 0x0B1030;
    fb.clear(FONDO);
    let (sw, sh) = fb.size();
    let (w, h) = (400.min(sw), 260.min(sh));
    let mut s = Surface::new(w, h, FONDO);
    icons::draw(&mut s, Icon::Logo, (w - 128) / 2, 30, 128, 0xFFFFFF);
    font::draw_in(&mut s, Rect::new(0, 180, w, 40), NAME, 20, true, 0xFFFFFF, font::Align::Center);
    fb.present_at(&s, s.bounds(), (sw - w) / 2, (sh - h) / 2);
}

// ---------- Fallos ----------

/// Búfer fijo en pila para formatear sin usar el montón.
struct Buf {
    b: [u8; 512],
    n: usize,
}

impl Write for Buf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &c in s.as_bytes() {
            if self.n < self.b.len() {
                self.b[self.n] = if c == b'\n' || c == b'\r' { b' ' } else { c };
                self.n += 1;
            }
        }
        Ok(())
    }
}

impl Buf {
    fn text(&self) -> &str {
        match core::str::from_utf8(&self.b[..self.n]) {
            Ok(s) => s,
            Err(e) => core::str::from_utf8(&self.b[..e.valid_up_to()]).unwrap_or(""),
        }
    }
}

static EN_FALLO: AtomicBool = AtomicBool::new(false);

fn pantalla_de_error(detalle: &str) -> ! {
    unsafe { core::arch::asm!("cli") };
    const FONDO: u32 = 0x2A1040;
    let fb = Fb::new(&boot::info());
    fb.clear(FONDO);
    // Un segundo fallo mientras se dibuja (p. ej. sin memoria): se queda el fondo.
    if HEAP_LISTO.load(Acquire) && !EN_FALLO.swap(true, AcqRel) {
        let (sw, sh) = fb.size();
        let (w, h) = (1000.min(sw), 320.min(sh));
        let mut s = Surface::new(w, h, FONDO);
        let (x, mut y) = (24, 24);
        font::draw(&mut s, x, y, "NightFury ha tenido un problema y se ha detenido.", 14, true, 0xFFFFFF);
        y += font::line_h(14) * 2;
        let por_linea = ((w - 2 * x) / font::adv('M', 9, false).max(1)).max(1) as usize;
        let mut resto = detalle;
        while !resto.is_empty() && y < h - 80 {
            let corte = resto.char_indices().nth(por_linea).map_or(resto.len(), |(i, _)| i);
            font::draw(&mut s, x, y, &resto[..corte], 9, false, 0xFFFFFF);
            resto = &resto[corte..];
            y += font::line_h(9);
        }
        font::draw(&mut s, x, h - 48, "Mantén pulsado el botón de encendido para apagar.", 9, false, 0xFFFFFF);
        fb.present_at(&s, s.bounds(), (sw - w) / 2, (sh - h) / 2);
    }
    loop {
        unsafe { core::arch::asm!("cli", "hlt") };
    }
}

#[no_mangle]
extern "C" fn fault_handler(vector: u64, frame: *const u64) -> ! {
    // Con código de error la pila empieza por él y el RIP está una posición más allá.
    let con_codigo = matches!(vector, 8 | 10 | 11 | 12 | 13 | 14 | 17 | 21 | 29 | 30);
    let rip = unsafe { *frame.add(con_codigo as usize) };
    // CS del marco: si era un programa (ring 3) solo se mata al programa; el sistema sigue.
    if unsafe { *frame.add(con_codigo as usize + 1) } & 3 == 3 {
        user::fallo(vector, rip);
    }
    let mut b = Buf { b: [0; 512], n: 0 };
    let _ = write!(b, "Excepcion {} en la direccion {:#018x}", vector, rip);
    if TRAZA {
        traza!("NF fallo: {}", b.text());
    }
    pantalla_de_error(b.text())
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut b = Buf { b: [0; 512], n: 0 };
    let _ = write!(b, "{}", info);
    if TRAZA {
        traza!("NF panico: {}", b.text());
    }
    pantalla_de_error(b.text())
}
