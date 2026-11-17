// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Programas en modo usuario (ring 3): espacio de direcciones propio, entrada y salida con
//! `syscall`/`iretq`, y el `Process` que usa el escritorio.
//!
//! Modelo (v0.8, sin planificador): `run` entra al programa con `nf_run_user`, que guarda el
//! contexto del núcleo; cada `syscall`, excepción o aviso de cuelgue vuelve a ese contexto
//! (`nf_leave`) con un código. `run` atiende la llamada con `dispatch` y reentra, o devuelve el
//! control al escritorio cuando el programa espera, termina o lo matan.
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
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use nf_core::abi::*;
use nf_core::nfx::{self, NfxError};
use nf_core::proc::{dispatch, Kill, Process, Regions, RunEnd, Sys, SysState, UserMem};

const KSTACK: usize = 32 << 10;
/// Un programa que lleva tanto sin esperar (ticks de 10 ms = 2 s) se considera colgado.
const HUNG_TICKS: u64 = 200;
const EFER: u32 = 0xC000_0080;
const STAR: u32 = 0xC000_0081;
const LSTAR: u32 = 0xC000_0082;
const FMASK: u32 = 0xC000_0084;
/// Selectores: código/datos de usuario con RPL 3 (los que deja `sysret`).
const USER_CS: u64 = 0x23;
const USER_SS: u64 = 0x1B;
/// RFLAGS que se conservan del programa: CF PF AF ZF SF DF OF (IF siempre 1, TF/AC/IOPL nunca).
const FLAGS_USER: u64 = 0xCD5;

/// Registros del programa tal como los deja `syscall_entry` y los espera `nf_run_user`
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
// Los usan las rutinas de ensamblador de abajo por su nombre.
#[no_mangle]
static mut NF_KRSP: u64 = 0;
#[no_mangle]
static mut NF_URSP: u64 = 0;
#[no_mangle]
static mut NF_KTOP: u64 = 0;
#[no_mangle]
static mut NF_UFX: u64 = 0;
#[no_mangle]
static mut NF_KFX: u64 = 0;
static mut KFX: Fx = Fx([0; 512]);

/// Hay un programa ejecutándose en ring 3 (o a punto de).
static IN_USER: AtomicBool = AtomicBool::new(false);
/// Tick en el que empezó el `run` actual.
static RUN_START: AtomicU64 = AtomicU64::new(0);
/// Datos del fallo de CPU en ring 3 (los rellena `fallo`, los lee `run`).
static F_VEC: AtomicU64 = AtomicU64::new(0);
static F_RIP: AtomicU64 = AtomicU64::new(0);
static F_CR2: AtomicU64 = AtomicU64::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

// Motivos con los que `nf_run_user` vuelve al núcleo.
const WHY_SYSCALL: u64 = 0;
const WHY_FAULT: u64 = 1;
const WHY_HUNG: u64 = 2;

extern "sysv64" {
    /// Guarda el contexto del núcleo y salta al programa con los registros de `regs` y el estado
    /// de coma flotante `ufx`; el estado del núcleo va a `kfx`. Vuelve (con el motivo) cuando
    /// alguien llama a `nf_leave`.
    fn nf_run_user(regs: *mut Regs, ufx: *mut Fx, kfx: *mut Fx) -> u64;
    fn nf_leave(why: u64) -> !;
    fn syscall_entry();
}

global_asm!(
    ".global nf_run_user",
    "nf_run_user:",
    "push rbp", "push rbx", "push r12", "push r13", "push r14", "push r15",
    "mov [rip + NF_KRSP], rsp",
    "mov [rip + NF_UFX], rsi",
    "mov [rip + NF_KFX], rdx",
    "cli",
    "fxsave64 [rdx]",
    "fxrstor64 [rsi]",
    "mov rsp, rdi",
    "pop r15", "pop r14", "pop r13", "pop r12", "pop r11", "pop r10", "pop r9", "pop r8",
    "pop rbp", "pop rdi", "pop rsi", "pop rdx", "pop rcx", "pop rbx", "pop rax",
    "iretq",
    // Vuelve al contexto del núcleo guardado en `nf_run_user`; rdi = motivo.
    ".global nf_leave",
    "nf_leave:",
    "cld",
    "mov rax, [rip + NF_UFX]",
    "fxsave64 [rax]",
    "mov rax, [rip + NF_KFX]",
    "fxrstor64 [rax]",
    "mov rax, rdi",
    "mov rsp, [rip + NF_KRSP]",
    "pop r15", "pop r14", "pop r13", "pop r12", "pop rbx", "pop rbp",
    "ret",
    // Entrada de `syscall`: rcx = rip, r11 = rflags. Un solo núcleo: el rsp del programa va a una
    // variable global. Guarda el marco de `Regs` en lo alto de la pila de núcleo del proceso.
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
    "xor edi, edi",
    "jmp nf_leave",
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

/// Lo llama el temporizador cuando interrumpe a un programa: si lleva 2 s sin esperar, lo corta.
pub fn tick_en_usuario() {
    if IN_USER.load(Relaxed) && TICKS.load(Relaxed).wrapping_sub(RUN_START.load(Relaxed)) >= HUNG_TICKS {
        // Estamos en la pila del núcleo del proceso, con el CPU en ring 0 y `IN_USER` verdadero:
        // el contexto guardado por `nf_run_user` es válido.
        unsafe { nf_leave(WHY_HUNG) }
    }
}

/// Excepción de la CPU en ring 3: anota qué pasó y vuelve al núcleo (nunca sigue en el programa).
pub fn fallo(vector: u64, rip: u64) -> ! {
    let cr2: u64;
    // Leer CR2 no tiene efectos.
    unsafe { asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack, preserves_flags)) };
    F_VEC.store(vector, Relaxed);
    F_RIP.store(rip, Relaxed);
    F_CR2.store(cr2, Relaxed);
    // `IN_USER` es verdadero (ring 3 solo ocurre dentro de `nf_run_user`), así que el contexto es válido.
    unsafe { nf_leave(WHY_FAULT) }
}

fn mensaje_fallo() -> String {
    let (v, rip, cr2) = (F_VEC.load(Relaxed), F_RIP.load(Relaxed), F_CR2.load(Relaxed));
    let mut s = String::new();
    let _ = match v {
        0 => write!(s, "división por cero (rip {rip:#x})"),
        6 => write!(s, "instrucción no válida (rip {rip:#x})"),
        13 => write!(s, "protección general (rip {rip:#x})"),
        14 => write!(s, "fallo de página en {cr2:#x} (rip {rip:#x})"),
        _ => write!(s, "excepción {v} del procesador (rip {rip:#x})"),
    };
    s
}

// ---------- Proceso ----------

struct Proc {
    id: u64,
    entry: u64,
    /// PML4 propia (primer elemento de `bloques`).
    pml4: *mut u64,
    /// Todo lo que hay que devolver al montón: tablas, imagen, pila, montón del programa.
    bloques: Vec<(*mut u8, Layout)>,
    regions: Regions,
    kstack: *mut u8,
    fx: Box<Fx>,
    started: bool,
    /// Evento de la primera ejecución: se entrega en la primera espera.
    pending_ev: Option<WireEvent>,
    /// Dónde escribir el evento de la espera en curso.
    wait_ptr: Option<u64>,
    done: bool,
}

fn kstack_layout() -> Layout {
    Layout::from_size_align(KSTACK, 16).unwrap()
}

impl Proc {
    fn new(entry: u64) -> Result<Proc, String> {
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
            entry,
            pml4: core::ptr::null_mut(),
            bloques: Vec::new(),
            regions: Regions::default(),
            kstack,
            fx,
            started: false,
            pending_ev: None,
            wait_ptr: None,
            done: false,
        };
        let pml4 = p.alloc(1).ok_or_else(sin_mem)? as *mut u64;
        // Se comparten las entradas del núcleo (sin bit U: solo supervisor); la 32 queda a cero.
        // Ambas tablas son de 512 entradas y no se solapan.
        unsafe { core::ptr::copy_nonoverlapping(paging::kernel_cr3() as *const u64, pml4, 512) };
        p.pml4 = pml4;
        Ok(p)
    }

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

    fn regs(&self) -> *mut Regs {
        (self.kstack as u64 + KSTACK as u64 - core::mem::size_of::<Regs>() as u64) as *mut Regs
    }

    fn ktop(&self) -> u64 {
        self.kstack as u64 + KSTACK as u64
    }

    /// Devuelve al montón todo lo del proceso. Solo con CR3 del núcleo.
    fn release(&mut self) {
        self.done = true;
        for (p, l) in self.bloques.drain(..) {
            // Vienen de `alloc_pages`/`alloc` con ese mismo layout y no se liberan dos veces.
            unsafe { dealloc(p, l) };
        }
        if !self.kstack.is_null() {
            unsafe { dealloc(self.kstack, kstack_layout()) };
            self.kstack = core::ptr::null_mut();
        }
        self.regions = Regions::default();
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        if !self.done {
            // Si su espacio estuviera activo no se podría soltar; `run` siempre vuelve al del núcleo.
            self.release();
        }
    }
}

impl UserMem for Proc {
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

impl Process for Proc {
    fn run(&mut self, ev: &WireEvent, st: &mut SysState) -> RunEnd {
        if self.done {
            return RunEnd::Exited(-1);
        }
        let regs = self.regs();
        // Espacio y pila de núcleo de este proceso (un solo núcleo: variables globales).
        unsafe {
            paging::set_cr3(self.pml4 as u64);
            NF_KTOP = self.ktop();
        }
        arch::set_rsp0(self.ktop());
        RUN_START.store(TICKS.load(Relaxed), Relaxed);
        if !self.started {
            self.started = true;
            self.pending_ev = Some(*ev);
            // Entrada: punto de entrada, pila arriba del todo, IF=1. `regs` es el hueco reservado en la pila de núcleo.
            unsafe { regs.write(Regs { rip: self.entry, rsp: STACK_TOP, rflags: 0x202, ..Default::default() }) };
        } else if let Some(ptr) = self.wait_ptr.take() {
            if !self.write(ptr, &ev.to_bytes()) {
                return self.fin(RunEnd::Killed(Kill::Fault(String::from("el evento no cabe en su memoria"))), st);
            }
            unsafe { (*regs).rax = 0 };
        }
        let end = loop {
            // Comprobaciones antes de `iretq`: RIP en la zona del programa, RSP canónico.
            let r = unsafe { &mut *regs };
            if !(USER_BASE..USER_TOP).contains(&r.rip) || r.rsp >> 47 != 0 {
                break RunEnd::Killed(Kill::Fault(String::from("dirección de retorno no válida")));
            }
            r.rcx = r.rip;
            r.r11 = r.rflags;
            r.rflags = r.rflags & FLAGS_USER | 0x202;
            r.cs = USER_CS;
            r.ss = USER_SS;
            IN_USER.store(true, Release);
            // Pila, TSS y espacio ya preparados; `regs` y `fx` son de este proceso.
            let why = unsafe { nf_run_user(regs, &mut *self.fx, core::ptr::addr_of_mut!(KFX)) };
            IN_USER.store(false, Release);
            match why {
                WHY_SYSCALL => {
                    let r = unsafe { &mut *regs };
                    st.now_ms = TICKS.load(Relaxed) * 10;
                    let args = [r.rdi, r.rsi, r.rdx, r.r10, r.r8];
                    let sys = dispatch(r.rax, args, self, st);
                    saca_log(st);
                    match sys {
                        Sys::Ret(v) => r.rax = v as u64,
                        Sys::Exit(c) => break RunEnd::Exited(c),
                        Sys::Wait { ptr, timeout_ms } => {
                            if let Some(e) = self.pending_ev.take() {
                                if !self.write(ptr, &e.to_bytes()) {
                                    break RunEnd::Killed(Kill::Fault(String::from("el evento no cabe en su memoria")));
                                }
                                r.rax = 0;
                            } else {
                                self.wait_ptr = Some(ptr);
                                let deadline = (timeout_ms != u64::MAX).then(|| st.now_ms.saturating_add(timeout_ms));
                                break RunEnd::Waiting { deadline };
                            }
                        }
                    }
                }
                WHY_FAULT => break RunEnd::Killed(Kill::Fault(mensaje_fallo())),
                _ => break RunEnd::Killed(Kill::Hung),
            }
        };
        self.fin(end, st)
    }
}

impl Proc {
    /// Vuelve al espacio del núcleo; si el programa ha terminado o lo han matado, lo libera.
    fn fin(&mut self, end: RunEnd, st: &mut SysState) -> RunEnd {
        // CR3 del núcleo antes de soltar nada; las interrupciones se habían cerrado en `nf_run_user`.
        unsafe {
            paging::set_cr3(paging::kernel_cr3());
            asm!("sti", options(nomem, nostack));
        }
        saca_log(st);
        if !matches!(end, RunEnd::Waiting { .. }) {
            if crate::TRAZA {
                crate::serial::traza!("NF proc: programa {} terminado: {:?}", self.id, end);
            }
            self.release();
        }
        end
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

/// Carga un `.nfx` y devuelve el proceso y su memoria fija (imagen + pila).
pub fn spawn(file: &[u8]) -> Result<(Box<dyn Process>, u64), String> {
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
    let mut p = Proc::new(img.entry)?;
    let sin_mem = || String::from("Sin memoria para cargar el programa");
    let mut fixed = STACK_SIZE;
    for s in &img.sections {
        let (start, end) = (s.vaddr, (s.vaddr + s.memsz).div_ceil(PAGE) * PAGE);
        fixed += end - start;
        if !p.map_range(start, end, s.data, s.kind.writable(), s.kind.executable()) {
            return Err(sin_mem());
        }
    }
    // Pila: debajo de STACK_TOP; las páginas de guarda de abajo y de arriba quedan sin mapear.
    if !p.map_range(STACK_TOP - STACK_SIZE, STACK_TOP, &[], true, false) {
        return Err(sin_mem());
    }
    Ok((Box::new(p), fixed))
}
