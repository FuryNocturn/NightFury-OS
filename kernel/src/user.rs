// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Programas en modo usuario (ring 3) y multitarea (v0.9): cada programa es una tarea con su
//! espacio de direcciones, su pila de núcleo y su área `fxsave`; la tarea 0 es el bucle del
//! núcleo (escritorio) sobre la pila de arranque. Un solo procesador.
//!
//! Reglas del reparto:
//! - La tarea 0 nunca se interrumpe para dar paso a un programa (el montón y el escritorio no son
//!   reentrantes): solo cede en `arch::idle` (`correr`). Por eso un programa solo está en ring 3 o
//!   en una llamada mientras la tarea 0 duerme dentro de `correr`.
//! - Un programa vuelve a la tarea 0 cuando el temporizador lo pilla en ring 3 (`tick_en_usuario`,
//!   sigue listo), cuando duerme en `WAIT_EVENT` o cuando termina o falla. Las llamadas corren con
//!   IF=0 y no se expropian.
//! - Todos los cambios de tarea pasan por `nf_switch` con IF=0 y cargan antes el CR3 del destino.
//!
//! Siempre se vuelve al programa con `iretq` (no `sysretq`): así un `rcx` no canónico no puede
//! provocar el fallo conocido de `sysret` en modo núcleo; además se exige que el RIP y el RSP
//! guardados sean de la zona del programa antes de reentrar.
use crate::arch::{self, TICKS};
use crate::paging::{self, alloc_pages, ADDR, NX, P, U, W};
use alloc::alloc::{dealloc, Layout};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::{asm, global_asm};
use core::fmt::Write;
use core::ptr::{addr_of_mut, null_mut};
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering::*};
use nf_core::abi::*;
use nf_core::nfx::{self, NfxError};
use nf_core::proc::{dispatch, Kill, ProcInfo, Process, Regions, Status, Sys, SysState, UserMem};

const KSTACK: usize = 32 << 10;
const EFER: u32 = 0xC000_0080;
const STAR: u32 = 0xC000_0081;
const LSTAR: u32 = 0xC000_0082;
const FMASK: u32 = 0xC000_0084;
/// Selectores: código/datos de usuario con RPL 3 (los que deja `sysret`).
const USER_CS: u64 = 0x23;
const USER_SS: u64 = 0x1B;
/// RFLAGS que se conservan del programa: CF PF AF ZF SF DF OF (IF siempre 1, TF/AC/IOPL nunca).
const FLAGS_USER: u64 = 0xCD5;

/// Registros del programa tal como los deja `syscall_entry` y los espera `nf_user_return`
/// (de abajo arriba de la pila: orden de `pop`, y al final el marco de `iretq`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Regs {
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rbp: u64,
    rdi: u64,
    rsi: u64,
    rdx: u64,
    rcx: u64,
    rbx: u64,
    rax: u64,
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}
const _: () = assert!(core::mem::size_of::<Regs>() == 160);

/// Área de `fxsave`/`fxrstor`: 512 bytes alineados a 16.
#[repr(C, align(16))]
struct Fx([u8; 512]);

// ---------- Estado global (un solo núcleo) ----------
// SEGURIDAD de todo este bloque: solo lo tocan la tarea 0 y el programa que ella ha puesto en
// marcha (nunca a la vez: la tarea 0 duerme dentro de `correr` mientras corre un programa) y las
// interrupciones con IF=0. Por eso no hace falta bloqueo.
#[no_mangle]
static mut NF_URSP: u64 = 0;
#[no_mangle]
static mut NF_KTOP: u64 = 0;
/// Pila de núcleo guardada de la tarea 0 y su estado de coma flotante.
static mut T0_KSP: u64 = 0;
static mut T0_FX: Fx = Fx([0; 512]);
/// Tabla de procesos: un `Box` por programa (su dirección no cambia aunque crezca el `Vec`). Una
/// entrada vive hasta que el escritorio suelta su `Handle`, aunque el programa ya haya terminado
/// (así `SysState` sigue accesible).
static mut PROCS: Vec<Box<Proc>> = Vec::new();
/// Programa en marcha (null = tarea 0).
static mut CUR: *mut Proc = null_mut();
/// Dónde empieza la búsqueda del siguiente programa listo (turno rotatorio).
static RR: AtomicUsize = AtomicUsize::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

extern "sysv64" {
    /// Cambio de tarea: guarda el contexto actual (registros del núcleo y `fxsave` en `old_fx`),
    /// deja su pila en `*old_rsp` y sigue con la tarea de `new_rsp` y `new_fx`. IF=0 al llamar.
    fn nf_switch(old_rsp: *mut u64, new_rsp: u64, old_fx: *mut Fx, new_fx: *mut Fx);
    /// `iretq` a ring 3 con los registros de `regs` (no vuelve).
    fn nf_to_user(regs: *mut Regs) -> !;
    /// Cola de `nf_to_user`; también es la dirección de retorno del marco inicial de un programa.
    fn nf_user_return();
    fn syscall_entry();
}

global_asm!(
    ".global nf_switch",
    "nf_switch:",
    "push rbp", "push rbx", "push r12", "push r13", "push r14", "push r15",
    "fxsave64 [rdx]",
    "fxrstor64 [rcx]",
    "mov [rdi], rsp",
    "mov rsp, rsi",
    "pop r15", "pop r14", "pop r13", "pop r12", "pop rbx", "pop rbp",
    "ret",
    ".global nf_to_user",
    "nf_to_user:",
    "mov rsp, rdi",
    ".global nf_user_return",
    "nf_user_return:",
    "pop r15", "pop r14", "pop r13", "pop r12", "pop r11", "pop r10", "pop r9", "pop r8",
    "pop rbp", "pop rdi", "pop rsi", "pop rdx", "pop rcx", "pop rbx", "pop rax",
    "iretq",
    // Entrada de `syscall`: rcx = rip, r11 = rflags. Un solo núcleo: el rsp del programa va a una
    // variable global. Guarda el marco de `Regs` en lo alto de la pila de núcleo del programa y
    // sigue en Rust (`syscall_main`), que nunca vuelve aquí.
    ".global syscall_entry",
    "syscall_entry:",
    "mov [rip + NF_URSP], rsp",
    "mov rsp, [rip + NF_KTOP]",
    "push 0x1B",
    "push qword ptr [rip + NF_URSP]",
    "push r11",
    "push 0x23",
    "push rcx",
    "push rax", "push rbx", "push rcx", "push rdx", "push rsi", "push rdi", "push rbp",
    "push r8", "push r9", "push r10", "push r11", "push r12", "push r13", "push r14", "push r15",
    "cld",
    "mov rdi, rsp", // alineada a 16: el marco son 160 bytes bajo lo alto de una pila alineada
    "call syscall_main",
    "ud2",
);

/// Activa `syscall`/`sysret` (tras `init_gdt` y `paging::init`).
pub fn init_syscall() {
    // MSR documentados de AMD64: EFER.SCE, STAR[47:32] = CS del núcleo (SS = +8), STAR[63:48] = base
    // de `sysret` (SS = +8, CS = +16), FMASK limpia IF, DF, TF y AC al entrar.
    unsafe {
        paging::wrmsr(EFER, paging::rdmsr(EFER) | 1);
        paging::wrmsr(STAR, 0x10 << 48 | 0x08 << 32);
        paging::wrmsr(LSTAR, syscall_entry as *const () as u64);
        paging::wrmsr(FMASK, 0x200 | 0x400 | 0x100 | 0x40000);
    }
}

// ---------- Procesos ----------

/// Espacio de direcciones del programa: tablas, zonas y los bloques que hay que devolver.
struct Mem {
    /// PML4 propia (primer elemento de `bloques`).
    pml4: *mut u64,
    /// Todo lo que hay que devolver al montón: tablas, imagen, pila, montón del programa.
    bloques: Vec<(*mut u8, Layout)>,
    regions: Regions,
}

struct Proc {
    id: u64,
    name: String,
    /// Lo comparte con el escritorio (`Handle::state`).
    st: SysState,
    mm: Mem,
    /// Pila de núcleo (null una vez liberada) y la pila guardada mientras la tarea no corre.
    kstack: *mut u8,
    ksp: u64,
    fx: Box<Fx>,
    /// Dormido en `WAIT_EVENT`: dónde escribir el evento y el plazo.
    wait: Option<(u64, Option<u64>)>,
    /// Tics (10 ms) que el temporizador ha pillado al programa en ring 3.
    cpu_ticks: u64,
}

fn kstack_layout() -> Layout {
    Layout::from_size_align(KSTACK, 16).unwrap()
}

fn procs() -> &'static mut Vec<Box<Proc>> {
    // Ver "SEGURIDAD" del estado global: nunca se usa a la vez desde dos sitios.
    unsafe { &mut *addr_of_mut!(PROCS) }
}

fn find(id: u64) -> Option<&'static mut Proc> {
    procs().iter_mut().find(|p| p.id == id).map(|p| &mut **p)
}

/// El programa en marcha. Solo se llama desde ring 3 (interrupción, excepción, llamada) o desde
/// código que corre en su tarea.
fn cur() -> &'static mut Proc {
    // `CUR` no es nulo mientras corre un programa; se pone antes del cambio de tarea.
    unsafe { &mut *CUR }
}

impl Mem {
    /// `n` páginas a cero del montón, anotadas para devolverlas.
    fn alloc(&mut self, n: usize) -> Option<*mut u8> {
        let (p, l) = alloc_pages(n);
        if p.is_null() {
            return None;
        }
        self.bloques.push((p, l));
        Some(p)
    }

    /// Mapea la página `va` (de la zona del programa) al marco `pa` con `flags`. `false` si ya
    /// estaba mapeada, no es de la zona o no hay memoria para las tablas.
    fn map_page(&mut self, va: u64, pa: u64, flags: u64) -> bool {
        if !(USER_BASE..USER_TOP).contains(&va) || va % PAGE != 0 {
            return false;
        }
        let idx = [(va >> 39) & 511, (va >> 30) & 511, (va >> 21) & 511, (va >> 12) & 511];
        let mut t = self.pml4;
        for &i in &idx[..3] {
            // `t` apunta a una tabla de 512 entradas propia del proceso e `i < 512`.
            let e = unsafe { t.add(i as usize) };
            if unsafe { *e } & P == 0 {
                let Some(n) = self.alloc(1) else { return false };
                unsafe { *e = n as u64 | P | W | U };
            }
            t = unsafe { (*e & ADDR) as *mut u64 };
        }
        // Mismo argumento: tabla propia de 512 entradas.
        let e = unsafe { t.add(idx[3] as usize) };
        if unsafe { *e } & P != 0 {
            return false;
        }
        unsafe { *e = pa | flags | P | U };
        true
    }

    /// Reserva y mapea `[start, end)` (alineados) a cero con esos permisos; `data` se copia al principio.
    fn map_range(&mut self, start: u64, end: u64, data: &[u8], writable: bool, exec: bool) -> bool {
        let pages = ((end - start) / PAGE) as usize;
        let Some(mem) = self.alloc(pages) else { return false };
        // `data` cabe: la sección validada tiene `data.len() <= memsz <= pages * PAGE`.
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), mem, data.len()) };
        let flags = if writable { W } else { 0 } | if exec { 0 } else { NX };
        for i in 0..pages as u64 {
            if !self.map_page(start + i * PAGE, mem as u64 + i * PAGE, flags) {
                return false;
            }
        }
        self.regions.add(start, end, writable);
        true
    }

    /// Devuelve al montón tablas y páginas. Solo con el CR3 del núcleo cargado; idempotente.
    fn release(&mut self) {
        for (p, l) in self.bloques.drain(..) {
            // Vienen de `alloc_pages` con ese mismo layout y no se liberan dos veces (`drain`).
            unsafe { dealloc(p, l) };
        }
        self.pml4 = null_mut();
        self.regions = Regions::default();
    }
}

impl UserMem for Mem {
    fn check(&self, addr: u64, len: u64, write: bool) -> bool {
        paging::cr3() == self.pml4 as u64 && self.regions.check(addr, len, write)
    }

    fn read(&mut self, addr: u64, buf: &mut [u8]) -> bool {
        // Con CR3 de otro espacio no se tocaría el programa: se niega en vez de leer cualquier cosa.
        if paging::cr3() != self.pml4 as u64 || !self.regions.check(addr, buf.len() as u64, false) {
            return false;
        }
        // Rango validado: mapeado en este espacio y dentro de la zona del programa.
        unsafe { core::ptr::copy_nonoverlapping(addr as *const u8, buf.as_mut_ptr(), buf.len()) };
        true
    }

    fn write(&mut self, addr: u64, data: &[u8]) -> bool {
        if paging::cr3() != self.pml4 as u64 || !self.regions.check(addr, data.len() as u64, true) {
            return false;
        }
        // Igual que en `read`, y la zona admite escritura.
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), addr as *mut u8, data.len()) };
        true
    }

    fn map_zeroed(&mut self, start: u64, end: u64) -> bool {
        start % PAGE == 0 && end % PAGE == 0 && start >= HEAP_BASE && self.map_range(start, end, &[], true, false)
    }
}

impl Proc {
    fn new(name: &str) -> Result<Proc, String> {
        let sin_mem = || String::from("Sin memoria para cargar el programa");
        // Estado de coma flotante limpio: FCW por defecto y MXCSR con las excepciones enmascaradas.
        let mut fx = Box::new(Fx([0; 512]));
        fx.0[0..2].copy_from_slice(&0x037Fu16.to_le_bytes());
        fx.0[24..28].copy_from_slice(&0x1F80u32.to_le_bytes());
        // Montón del núcleo inicializado; tamaño y alineación válidos.
        let kstack = unsafe { alloc::alloc::alloc(kstack_layout()) };
        if kstack.is_null() {
            return Err(sin_mem());
        }
        let mut p = Proc {
            id: NEXT_ID.fetch_add(1, Relaxed),
            name: String::from(name),
            st: SysState::new(0),
            mm: Mem { pml4: null_mut(), bloques: Vec::new(), regions: Regions::default() },
            kstack,
            ksp: 0,
            fx,
            wait: None,
            cpu_ticks: 0,
        };
        let pml4 = p.mm.alloc(1).ok_or_else(sin_mem)? as *mut u64;
        // Se comparten las entradas del núcleo (sin bit U: solo supervisor); la 32 queda a cero.
        // Ambas tablas son de 512 entradas y no se solapan.
        unsafe { core::ptr::copy_nonoverlapping(paging::kernel_cr3() as *const u64, pml4, 512) };
        p.mm.pml4 = pml4;
        Ok(p)
    }

    fn ktop(&self) -> u64 {
        self.kstack as u64 + KSTACK as u64
    }

    /// Prepara el primer arranque: en lo alto de la pila de núcleo, los registros de ring 3 y
    /// debajo un marco de `nf_switch` que "retorna" a `nf_user_return`.
    fn marco_inicial(&mut self, entry: u64) {
        let regs = (self.ktop() - core::mem::size_of::<Regs>() as u64) as *mut Regs;
        // Pila de núcleo propia y recién reservada; caben los 160 bytes de `Regs` y los 7 qwords de debajo.
        unsafe {
            regs.write(Regs { rip: entry, rcx: entry, rsp: STACK_TOP, rflags: 0x202, r11: 0x202, cs: USER_CS, ss: USER_SS, ..Default::default() });
            let sw = (regs as *mut u64).sub(7);
            for i in 0..6 {
                sw.add(i).write(0); // r15 r14 r13 r12 rbx rbp
            }
            sw.add(6).write(nf_user_return as *const () as u64);
            self.ksp = sw as u64;
        }
    }

    /// Libera la pila de núcleo. Solo si no se está ejecutando sobre ella.
    fn libera_kstack(&mut self) {
        if !self.kstack.is_null() {
            // Viene de `alloc` con ese layout y se anula para no liberarla dos veces.
            unsafe { dealloc(self.kstack, kstack_layout()) };
            self.kstack = null_mut();
        }
    }

    fn vivo(&self) -> bool {
        self.st.status == Status::Alive
    }

    /// ¿Puede correr ahora? Listo = no duerme en `WAIT_EVENT`, o duerme pero ya tiene algo que recibir.
    fn listo(&self, now: u64) -> bool {
        self.vivo()
            && match self.wait {
                None => true,
                Some((_, d)) => !self.st.queue.is_empty() || self.st.wake_at(d).is_some_and(|w| now >= w),
            }
    }

    fn mem(&self) -> u64 {
        if self.vivo() {
            self.st.fixed_mem + self.st.brk - HEAP_BASE
        } else {
            0
        }
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        // Solo se suelta desde la tarea 0 (CR3 del núcleo) y con el programa parado.
        self.mm.release();
        self.libera_kstack();
    }
}

/// Las líneas de `LOG` salen por el puerto serie.
fn saca_log(st: &mut SysState) {
    for l in st.out.log.drain(..) {
        if crate::TRAZA {
            crate::serial::traza!("NF prog: {}", l);
        }
    }
}

fn traza_fin(p: &Proc) {
    if crate::TRAZA {
        crate::serial::traza!("NF proc: programa {} terminado: {:?}", p.id, p.st.status);
    }
}

// ---------- Cambios de tarea ----------

/// Tarea 0 → siguiente programa listo (por turnos). Lo llama `arch::idle` con IF=0. Devuelve
/// `true` si ha corrido alguno (la tarea 0 ha recuperado el control), `false` si no había ninguno.
pub fn correr() -> bool {
    let now = TICKS.load(Relaxed) * 10;
    let v = procs();
    let n = v.len();
    let ini = RR.load(Relaxed);
    let Some(i) = (0..n).map(|k| (ini + k) % n).find(|&i| v[i].listo(now)) else { return false };
    RR.store(i + 1, Relaxed);
    let p: *mut Proc = &mut *v[i];
    // `p` es un programa vivo y parado; la tarea 0 es la única que corre. Cargamos su espacio y
    // sus pilas antes de saltar; las interrupciones siguen cerradas hasta su `iretq`.
    unsafe {
        CUR = p;
        NF_KTOP = (*p).ktop();
        arch::set_rsp0((*p).ktop());
        paging::set_cr3((*p).mm.pml4 as u64);
        nf_switch(addr_of_mut!(T0_KSP), (*p).ksp, addr_of_mut!(T0_FX), addr_of_mut!(*(*p).fx));
    }
    // De vuelta en la tarea 0 (quien nos devolvió el control cargó el CR3 del núcleo). Los
    // programas que han terminado sobre su pila de núcleo la dejan para ahora.
    for p in procs().iter_mut().filter(|p| !p.vivo()) {
        p.libera_kstack();
    }
    true
}

/// Programa actual → tarea 0, guardando su contexto; vuelve cuando la tarea 0 lo reanude.
/// Siempre con IF=0 (interrupción, llamada o excepción).
fn a_tarea0() {
    // El estado del programa se lee de `CUR` aquí y no se pasa como `&mut`: la tarea 0 lo toca
    // mientras tanto. `T0_KSP` lo dejó `correr`.
    unsafe {
        let p = CUR;
        CUR = null_mut();
        paging::set_cr3(paging::kernel_cr3());
        nf_switch(addr_of_mut!((*p).ksp), T0_KSP, addr_of_mut!(*(*p).fx), addr_of_mut!(T0_FX));
    }
}

/// Lo llama el temporizador cuando interrumpe a un programa en ring 3: lo anota y devuelve el
/// procesador a la tarea 0 (el programa sigue listo y entrará de nuevo por turnos).
pub fn tick_en_usuario() {
    if unsafe { CUR }.is_null() {
        return;
    }
    // shortcut: el tiempo de CPU se cuenta por muestreo (un tic por cada vez que el temporizador
    // pilla al programa), no con el reloj fino; cuando haga falta exactitud, leer el TSC al entrar y salir.
    cur().cpu_ticks += 1;
    a_tarea0();
}

/// Termina el programa actual con ese estado: libera su memoria al momento (con el CR3 del
/// núcleo) y pasa a la tarea 0 para siempre. La pila de núcleo, en la que seguimos, la libera
/// `correr` al volver.
fn morir(estado: Status) -> ! {
    // Se cambia a la tabla del núcleo antes de devolver las del programa; la pila de núcleo está
    // en el montón y se ve en cualquier espacio.
    unsafe { paging::set_cr3(paging::kernel_cr3()) };
    let p = cur();
    p.st.status = estado;
    p.wait = None;
    saca_log(&mut p.st);
    traza_fin(p);
    p.mm.release();
    a_tarea0();
    // Un programa terminado no está listo, así que la tarea 0 no lo vuelve a entrar.
    unreachable!("un programa terminado ha vuelto a correr")
}

/// Excepción de la CPU en ring 3: el programa muere con el motivo y el sistema sigue.
pub fn fallo(vector: u64, rip: u64) -> ! {
    let cr2: u64;
    // Leer CR2 no tiene efectos.
    unsafe { asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack, preserves_flags)) };
    let mut s = String::new();
    let _ = match vector {
        0 => write!(s, "división por cero (rip {rip:#x})"),
        6 => write!(s, "instrucción no válida (rip {rip:#x})"),
        13 => write!(s, "protección general (rip {rip:#x})"),
        14 => write!(s, "fallo de página en {cr2:#x} (rip {rip:#x})"),
        _ => write!(s, "excepción {vector} del procesador (rip {rip:#x})"),
    };
    // Ring 3 solo ocurre con un programa en marcha (`CUR` válido).
    morir(Status::Killed(Kill::Fault(s)))
}

/// Duerme al programa actual hasta tener algo que entregarle en `ptr` y lo escribe. `false` si ya
/// no cabe en su memoria. No guarda en pila nada que posea memoria (se perdería al matarlo dormido).
fn esperar(ptr: u64, deadline: Option<u64>) -> bool {
    loop {
        let p = cur();
        let now = TICKS.load(Relaxed) * 10;
        p.st.now_ms = now;
        if let Some(e) = p.st.next_event(now, deadline) {
            p.wait = None;
            return p.mm.write(ptr, &e.to_bytes());
        }
        p.wait = Some((ptr, deadline));
        a_tarea0();
    }
}

/// Vuelve a ring 3 con los registros de `regs` tras comprobarlos: RIP en la zona del programa,
/// RSP canónico, RFLAGS saneado y selectores de usuario.
unsafe fn a_usuario(regs: *mut Regs) -> ! {
    let r = &mut *regs;
    if !(USER_BASE..USER_TOP).contains(&r.rip) || r.rsp >> 47 != 0 {
        morir(Status::Killed(Kill::Fault(String::from("dirección de retorno no válida"))));
    }
    r.rcx = r.rip;
    r.r11 = r.rflags;
    r.rflags = r.rflags & FLAGS_USER | 0x202;
    r.cs = USER_CS;
    r.ss = USER_SS;
    // Pila, TSS y espacio ya son los de este programa; IF vuelve a 1 con el `iretq`.
    nf_to_user(regs)
}

/// Atiende una `syscall` (la llama `syscall_entry`, con IF=0 y `regs` en lo alto de la pila de núcleo).
#[no_mangle]
extern "sysv64" fn syscall_main(regs: *mut Regs) -> ! {
    // `regs` es el marco que acaba de guardar `syscall_entry`; se accede siempre por el puntero
    // porque la tarea puede dormir y la 0 correr entre medias.
    let (nr, args) = unsafe { ((*regs).rax, [(*regs).rdi, (*regs).rsi, (*regs).rdx, (*regs).r10, (*regs).r8]) };
    let p = cur();
    p.st.now_ms = TICKS.load(Relaxed) * 10;
    // Leer el reloj CMOS son muchos accesos a puertos: solo cuando el programa lo pide.
    if nr == SYS_DATETIME {
        p.st.datetime = crate::rtc::now();
    }
    let sys = dispatch(nr, args, &mut p.mm, &mut p.st);
    saca_log(&mut p.st);
    match sys {
        Sys::Ret(v) => unsafe { (*regs).rax = v as u64 },
        Sys::Exit(c) => morir(Status::Exited(c)),
        Sys::Wait { ptr, timeout_ms } => {
            let now = TICKS.load(Relaxed) * 10;
            let deadline = (timeout_ms != u64::MAX).then(|| now.saturating_add(timeout_ms));
            if !esperar(ptr, deadline) {
                morir(Status::Killed(Kill::Fault(String::from("el evento no cabe en su memoria"))));
            }
            unsafe { (*regs).rax = 0 };
        }
    }
    unsafe { a_usuario(regs) }
}

// ---------- Lo que ve el escritorio ----------

/// Asa del escritorio a un programa de la tabla. Soltarla lo termina y lo quita de la tabla.
struct Handle(u64);

/// Termina el programa si vive y libera todo, pila de núcleo incluida. Solo desde la tarea 0: el
/// programa está dormido o expropiado, nunca en medio de una llamada ni con su espacio cargado.
fn matar(p: &mut Proc) {
    if p.vivo() {
        p.st.status = Status::Killed(Kill::Ended);
        saca_log(&mut p.st);
        traza_fin(p);
    }
    p.wait = None;
    p.mm.release();
    p.libera_kstack();
}

impl Process for Handle {
    fn id(&self) -> u64 {
        self.0
    }
    fn state(&mut self) -> &mut SysState {
        // La entrada existe mientras exista el `Handle` (solo su `Drop` la quita).
        &mut find(self.0).expect("proceso fuera de la tabla").st
    }
    fn kill(&mut self) {
        if let Some(p) = find(self.0) {
            matar(p);
        }
    }
    fn mem(&self) -> u64 {
        find(self.0).map_or(0, |p| p.mem())
    }
    fn cpu_ms(&self) -> u64 {
        find(self.0).map_or(0, |p| p.cpu_ticks * 10)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.kill();
        procs().retain(|p| p.id != self.0);
    }
}

/// Programas vivos, para el Administrador de tareas.
pub fn lista() -> Vec<ProcInfo> {
    let now = TICKS.load(Relaxed) * 10;
    procs()
        .iter()
        .filter(|p| p.vivo())
        .map(|p| ProcInfo { id: p.id, name: p.name.clone(), mem: p.mem(), cpu_ms: p.cpu_ticks * 10, responding: !p.st.not_responding(now) })
        .collect()
}

pub fn finalizar(id: u64) {
    if let Some(p) = find(id) {
        matar(p);
    }
}

/// Carga un `.nfx`, lo deja listo para correr y devuelve su asa.
pub fn spawn(file: &[u8], name: &str) -> Result<Box<dyn Process>, String> {
    if !paging::NX_OK.load(Relaxed) {
        return Err(String::from("Este procesador no admite NX: no se pueden ejecutar programas"));
    }
    let img = nfx::parse(file).map_err(|e| {
        String::from(match e {
            NfxError::NotNfx => "No es un programa de NightFury",
            NfxError::Version => "Programa hecho para otra versión del sistema",
            NfxError::Invalid => "Programa dañado",
            NfxError::TooBig => "El programa necesita demasiada memoria",
        })
    })?;
    if !(USER_BASE..USER_TOP).contains(&img.entry) {
        return Err(String::from("Programa dañado"));
    }
    let mut p = Box::new(Proc::new(name)?);
    let sin_mem = || String::from("Sin memoria para cargar el programa");
    let mut fixed = STACK_SIZE;
    for s in &img.sections {
        let (start, end) = (s.vaddr, (s.vaddr + s.memsz).div_ceil(PAGE) * PAGE);
        fixed += end - start;
        if !p.mm.map_range(start, end, s.data, s.kind.writable(), s.kind.executable()) {
            return Err(sin_mem());
        }
    }
    // Pila: debajo de STACK_TOP; las páginas de guarda de abajo y de arriba quedan sin mapear.
    if !p.mm.map_range(STACK_TOP - STACK_SIZE, STACK_TOP, &[], true, false) {
        return Err(sin_mem());
    }
    p.st = SysState::new(fixed);
    p.marco_inicial(img.entry);
    let id = p.id;
    procs().push(p);
    Ok(Box::new(Handle(id)))
}
