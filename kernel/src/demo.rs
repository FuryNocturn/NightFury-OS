// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Programas de prueba en modo usuario (feature `demo`): dos ventanas que pintan y se cierran, una
//! que lee memoria del núcleo y una que se cuelga. Son código independiente de la posición:
//! se copian sus bytes a una sección Code de un `.nfx` armado aquí mismo.
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::arch::global_asm;
use nf_core::abi::USER_BASE;
use nf_core::desktop::Desktop;
use nf_core::nfx::{self, Kind, Section};
use nf_core::platform::Platform;
use nf_core::proc::ProcApp;

const CODE: u64 = USER_BASE + 0x40_0000;
/// Búfer de imagen de la ventana demo (512 x 512 píxeles de 4 bytes).
const BSS: u64 = CODE + 0x1_0000;
const BSS_SIZE: u64 = 512 * 512 * 4;

extern "C" {
    static DEMO_A_START: u8;
    static DEMO_A_END: u8;
    static DEMO_B_START: u8;
    static DEMO_B_END: u8;
    static DEMO_C_START: u8;
    static DEMO_C_END: u8;
}

global_asm!(
    // (a) Pone título y un LOG; en EV_RESIZE pinta su contenido de un color (PRESENT); en
    // EV_CLOSE hace EXIT. rsp = STACK_TOP: el evento se recibe en la propia pila.
    ".global DEMO_A_START",
    "DEMO_A_START:",
    "lea rdi, [rip + 20f]",
    "mov esi, {tl}",
    "mov eax, 4", // SET_TITLE
    "syscall",
    "lea rdi, [rip + 22f]",
    "mov esi, {ll}",
    "mov eax, 1", // LOG
    "syscall",
    "sub rsp, 64",
    "2:",
    "mov rdi, rsp",
    "mov rsi, -1",
    "mov eax, 2", // WAIT_EVENT sin límite
    "syscall",
    "mov eax, [rsp]",
    "cmp eax, 8", // EV_RESIZE
    "je 3f",
    "cmp eax, 12", // EV_CLOSE
    "je 5f",
    "jmp 2b",
    "3:",
    "mov r12d, [rsp+4]",
    "mov r13d, [rsp+8]",
    "cmp r12d, 512",
    "jbe 4f",
    "mov r12d, 512",
    "4:",
    "cmp r13d, 512",
    "jbe 6f",
    "mov r13d, 512",
    "6:",
    "test r12d, r12d",
    "jz 2b",
    "test r13d, r13d",
    "jz 2b",
    "mov eax, r12d",
    "imul eax, r13d",
    "mov ecx, eax",
    "movabs rdi, {bss}",
    "mov eax, 0x2E7D32",
    "cld",
    "rep stosd",
    "movabs rdi, {bss}",
    "mov esi, r12d",
    "mov edx, r13d",
    "mov r10d, r12d",
    "mov eax, 3", // PRESENT
    "syscall",
    "jmp 2b",
    "5:",
    "xor edi, edi",
    "xor eax, eax", // EXIT(0)
    "syscall",
    "20:",
    ".ascii \"Demo en modo usuario\"",
    "22:",
    ".ascii \"hola desde el anillo 3\"",
    ".global DEMO_A_END",
    "DEMO_A_END:",
    // (b) Lee memoria del núcleo: debe morir con un fallo de página, no tirar el sistema.
    ".global DEMO_B_START",
    "DEMO_B_START:",
    "mov rax, qword ptr [0x100000]",
    "xor edi, edi",
    "xor eax, eax",
    "syscall",
    ".global DEMO_B_END",
    "DEMO_B_END:",
    // (c) Bucle infinito: no bloquea nada; el escritorio lo marca "No responde".
    ".global DEMO_C_START",
    "DEMO_C_START:",
    "2:",
    "jmp 2b",
    ".global DEMO_C_END",
    "DEMO_C_END:",
    bss = const BSS,
    tl = const "Demo en modo usuario".len(),
    ll = const "hola desde el anillo 3".len(),
);

fn montar(start: &u8, end: &u8, bss: u64) -> Vec<u8> {
    // Los dos símbolos delimitan bytes de código del propio núcleo, en este orden.
    let code = unsafe { core::slice::from_raw_parts(start, end as *const u8 as usize - start as *const u8 as usize) };
    let mut s = alloc::vec![Section { kind: Kind::Code, vaddr: CODE, memsz: code.len() as u64, data: code }];
    if bss > 0 {
        s.push(Section { kind: Kind::Bss, vaddr: BSS, memsz: bss, data: &[] });
    }
    nfx::encode(CODE, &s)
}

/// Abre los programas de prueba como ventanas del escritorio.
pub fn lanzar(desk: &mut Desktop, plat: &mut dyn Platform) {
    // Solo se toman las direcciones de los símbolos, no se leen como datos.
    let progs = unsafe {
        [
            ("Demo", montar(&DEMO_A_START, &DEMO_A_END, BSS_SIZE)),
            ("Demo 2", montar(&DEMO_A_START, &DEMO_A_END, BSS_SIZE)),
            ("Demo lee el núcleo", montar(&DEMO_B_START, &DEMO_B_END, 0)),
            ("Demo se cuelga", montar(&DEMO_C_START, &DEMO_C_END, 0)),
        ]
    };
    for (nombre, bytes) in progs {
        match plat.spawn(&bytes, nombre) {
            Ok(p) => desk.launch_app(Box::new(ProcApp::new(p, nombre, None, false)), plat),
            Err(e) => crate::serial::traza!("NF demo: {nombre}: {e}"),
        }
    }
}
