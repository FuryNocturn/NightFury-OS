// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! EFI mínimo de prueba: hace de «Windows/Linux falso» para probar el menú de arranque en QEMU.
#![no_std]
#![no_main]

use core::ffi::c_void;
use efi::{InputKey, SystemTable};

fn texto(t: &str, buf: &mut [u16; 80]) -> *const u16 {
    for (d, c) in buf.iter_mut().zip(t.encode_utf16().chain([0])) {
        *d = c;
    }
    buf.as_ptr()
}

#[no_mangle]
pub extern "efiapi" fn efi_main(_imagen: *mut c_void, st: *mut SystemTable) -> usize {
    let st = unsafe { &*st };
    let (sal, ent) = (st.con_out, st.con_in);
    let mut buf = [0u16; 80];
    (unsafe { &*sal }.clear_screen)(sal);
    (unsafe { &*sal }.output_string)(sal, texto("Sistema de prueba arrancado. Pulsa una tecla para volver.", &mut buf));
    let (bs, mut k) = (unsafe { &*st.boot_services }, InputKey { scan_code: 0, unicode_char: 0 });
    let mut ev = unsafe { &*ent }.wait_for_key;
    let mut i = 0;
    (bs.wait_for_event)(1, &mut ev, &mut i);
    (unsafe { &*ent }.read_key_stroke)(ent, &mut k);
    0
}

#[panic_handler]
fn panico(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
