//! Parte del runtime que habla con el núcleo de verdad: `syscall`, asignador, panic y arranque.
//! Solo se compila para `x86_64-unknown-none`; lo demás (el bucle) se prueba en el PC.
use crate::bucle::{cortar, ejecutar, Llamadas};
use alloc::boxed::Box;
use core::alloc::{GlobalAlloc, Layout};
use core::fmt::Write;
use nf_core::abi::*;
use nf_core::apps::App;
use nf_core::heap::Heap;
use nf_core::platform::DateTime;
use nf_core::sync::SpinLock;

/// Llamada al núcleo: `rax` = número, `rdi, rsi, rdx, r10, r8` = argumentos (contrato en `abi`).
fn syscall(nr: u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> i64 {
    let r: i64;
    // SAFETY: `syscall` entra en el núcleo, que valida todos los punteros (E_FAULT) y conserva los
    // registros salvo rax, rcx y r11, que se declaran como destruidos. No usa la pila del programa.
    unsafe {
        core::arch::asm!("syscall", inlateout("rax") nr => r, in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d, in("r8") e, lateout("rcx") _, lateout("r11") _, options(nostack));
    }
    r
}

fn log(t: &str) {
    let t = cortar(t, LOG_MAX);
    syscall(SYS_LOG, t.as_ptr() as u64, t.len() as u64, 0, 0, 0);
}

fn salir(code: i32) -> ! {
    syscall(SYS_EXIT, code as u64, 0, 0, 0, 0);
    // EXIT no vuelve; si algo fallara, mejor parado que ejecutando basura.
    loop {
        core::hint::spin_loop();
    }
}

// ---------- Asignador ----------

/// Cada ampliación del montón (`MEM_GROW`) es un `Heap` nuevo. Se empieza sin nada y se pide de
/// 4 en 4 MiB al hacer falta (el límite del programa es `MAX_MEM`, 256 MiB).
const CHUNK: usize = 4 << 20;
const MAX_CHUNKS: usize = 64;

struct Montones {
    heaps: [Heap; MAX_CHUNKS],
    /// (inicio, longitud) de cada uno, para saber a cuál devolver un bloque.
    rangos: [(usize, usize); MAX_CHUNKS],
    n: usize,
}

struct Asignador(SpinLock<Montones>);

// SAFETY: `GlobalAlloc` pide Sync; el SpinLock serializa el acceso y el programa tiene un solo hilo.
unsafe impl GlobalAlloc for Asignador {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let mut m = self.0.lock();
        let n = m.n;
        for h in m.heaps[..n].iter_mut() {
            // SAFETY: cada Heap de `..n` se inicializó con memoria que es solo suya.
            let p = h.alloc(l.size(), l.align());
            if !p.is_null() {
                return p;
            }
        }
        if n == MAX_CHUNKS {
            return core::ptr::null_mut();
        }
        // Cabecera (16) + alineación + redondeo del Heap caben en `+ l.align() + 64`.
        let Some(need) = l.size().checked_add(l.align() + 64) else { return core::ptr::null_mut() };
        let bytes = need.max(CHUNK).div_ceil(PAGE as usize) * PAGE as usize;
        let base = syscall(SYS_MEM_GROW, bytes as u64, 0, 0, 0, 0);
        if base < 0 {
            return core::ptr::null_mut();
        }
        // SAFETY: MEM_GROW entrega `bytes` de memoria nueva a cero que nadie más usa.
        m.heaps[n].init(base as *mut u8, bytes);
        m.rangos[n] = (base as usize, bytes);
        m.n = n + 1;
        m.heaps[n].alloc(l.size(), l.align())
    }

    unsafe fn dealloc(&self, p: *mut u8, _l: Layout) {
        let mut m = self.0.lock();
        let a = p as usize;
        let n = m.n;
        if let Some(i) = m.rangos[..n].iter().position(|&(s, len)| a >= s && a < s + len) {
            // SAFETY: `p` salió de `alloc` de este Heap (lo dice su rango) y no se ha liberado.
            m.heaps[i].free(p);
        }
    }
}

#[global_allocator]
static ASIGNADOR: Asignador = Asignador(SpinLock::new(Montones { heaps: [const { Heap::empty() }; MAX_CHUNKS], rangos: [(0, 0); MAX_CHUNKS], n: 0 }));

// ---------- Panic ----------

/// Texto en la pila (sin asignar: el panic puede venir de un fallo del propio asignador).
struct Buf {
    b: [u8; 512],
    n: usize,
}

impl Write for Buf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let s = cortar(s, self.b.len() - self.n);
        self.b[self.n..self.n + s.len()].copy_from_slice(s.as_bytes());
        self.n += s.len();
        Ok(())
    }
}

#[panic_handler]
fn panico(info: &core::panic::PanicInfo) -> ! {
    let mut b = Buf { b: [0; 512], n: 0 };
    let _ = write!(b, "panic: {info}");
    // `Buf` solo recibe trozos de `&str` completos, así que es UTF-8 válido.
    log(core::str::from_utf8(&b.b[..b.n]).unwrap_or("panic"));
    salir(101)
}

// ---------- Llamadas reales ----------

struct Nucleo;

impl Llamadas for Nucleo {
    fn esperar(&mut self, timeout_ms: u64) -> WireEvent {
        let mut ev = WireEvent::default();
        syscall(SYS_WAIT_EVENT, &mut ev as *mut WireEvent as u64, timeout_ms, 0, 0, 0);
        ev
    }
    fn presentar(&mut self, px: &[u32], w: i32, h: i32) {
        syscall(SYS_PRESENT, px.as_ptr() as u64, w as u64, h as u64, w as u64, 0);
    }
    fn titulo(&mut self, t: &str) {
        let t = cortar(t, TITLE_MAX);
        syscall(SYS_SET_TITLE, t.as_ptr() as u64, t.len() as u64, 0, 0, 0);
    }
    fn tam_min(&mut self, w: i32, h: i32) {
        syscall(SYS_SET_MIN_SIZE, w.max(0) as u64, h.max(0) as u64, 0, 0, 0);
    }
    fn fecha(&mut self) -> DateTime {
        let mut b = [0u8; 8];
        syscall(SYS_DATETIME, b.as_mut_ptr() as u64, 0, 0, 0, 0);
        DateTime { year: u16::from_le_bytes([b[0], b[1]]), month: b[2], day: b[3], hour: b[4], minute: b[5], second: b[6] }
    }
    fn log(&mut self, t: &str) {
        log(t);
    }
}

/// Lo llama `programa!` desde `_start`: ejecuta la app y termina con su código.
pub fn arranque(crear: impl FnOnce() -> Box<dyn App>, tick_ms: Option<u64>) -> ! {
    let code = ejecutar(&mut Nucleo, crear(), tick_ms);
    salir(code)
}
