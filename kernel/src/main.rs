//! NightFury: núcleo. Solo aporta hardware (vídeo, interrupciones, PS/2, reloj, memoria);
//! todo lo demás vive en `nf-core`.
#![no_std]
#![no_main]

extern crate alloc;

mod ahci;
mod arch;
mod boot;
mod fb;
mod pci;
mod ps2;
mod rtc;
mod serial;
mod uefi;

use alloc::boxed::Box;
use alloc::string::String;
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

#[global_allocator]
static ASIGNADOR: Asignador = Asignador;

// ---------- Plataforma ----------

struct Plat {
    info: boot::BootInfo,
    files: Files,
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
    arch::init_gdt();
    arch::init_idt();
    arch::init_pic();
    arch::init_pit();

    pantalla_de_arranque(&fb);
    let con_rueda = ps2::init();

    let disco = elegir_disco();
    let files = Files::probe(disco, rtc::now());
    if TRAZA {
        traza!("NF archivos: persistente={} formato_pendiente={}", files.persistent(), files.pending_format());
    }
    let mut plat = Plat { info, files };
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
    unsafe { core::arch::asm!("sti") };

    let mut teclado = Keyboard::new();
    let mut raton = Ps2Mouse::new();
    raton.set_wheel(con_rueda);
    let mut ultimo = u64::MAX;
    loop {
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
