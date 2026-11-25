// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Entrada de arranque UEFI «NightFury» tras instalar: variable `Boot####` y primer puesto en `BootOrder`
//! (UEFI 2.x, 3.1.3). Los Runtime Services se llaman como `ResetSystem`: sin `SetVirtualAddressMap`, con
//! la dirección física de la tabla y las interrupciones cerradas.
use crate::uefi::*;
use alloc::format;
use alloc::vec::Vec;
use nf_core::vol::instalar::{self, Esp};

/// Mayor `BootOrder` que se lee (512 entradas): uno más largo no se toca.
const MAX_ORDEN: usize = 1024;
/// Números `Boot####` que se prueban hasta dar con uno libre.
const MAX_NUM: u16 = 0x100;
/// Una entrada de arranque ocupa unos cientos de bytes; más de esto no es una entrada nuestra.
const MAX_VAR: usize = 512;
/// Inicio de la descripción en EFI_LOAD_OPTION: Attributes (u32) + FilePathListLength (u16).
const DESC: usize = 6;

/// ¿Es una entrada nuestra de una instalación anterior? (descripción «NightFury»). Se reutiliza en vez de
/// crear otra al reinstalar.
fn es_nuestra(v: &[u8]) -> bool {
    let d = nombre("NightFury");
    v.len() >= DESC + d.len() * 2 && d.iter().enumerate().all(|(i, c)| v[DESC + i * 2..DESC + i * 2 + 2] == c.to_le_bytes())
}

fn nombre(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn leer(rt: &RuntimeServices, n: &[u16], buf: &mut [u8]) -> (Status, usize) {
    let mut len = buf.len();
    let st = (rt.get_variable)(n.as_ptr(), &GLOBAL_VARIABLE_GUID, core::ptr::null_mut(), &mut len, buf.as_mut_ptr().cast());
    (st, len)
}

fn escribir(rt: &RuntimeServices, n: &[u16], v: &[u8]) -> Status {
    (rt.set_variable)(n.as_ptr(), &GLOBAL_VARIABLE_GUID, VAR_NV_BS_RT, v.len(), v.as_ptr().cast())
}

fn sin_interrupciones<T>(f: impl FnOnce() -> T) -> T {
    let rflags: u64;
    unsafe { core::arch::asm!("pushfq", "pop {}", "cli", out(reg) rflags) };
    let r = f();
    if rflags & (1 << 9) != 0 {
        unsafe { core::arch::asm!("sti") };
    }
    r
}

/// Crea `Boot####` (primer número libre) que arranca `ruta` (relativa a la raíz, como `EFI\NightFury\arranque.efi`)
/// de la ESP recién instalada y la pone la primera en `BootOrder`. `Err` con el motivo si el firmware no lo
/// permite: en una ESP propia la ruta por defecto (`\EFI\BOOT\BOOTX64.EFI`) sigue valiendo.
pub fn crear(rt: *mut RuntimeServices, esp: &Esp, ruta: &str) -> Result<(), &'static str> {
    let ruta = format!("\\{ruta}");
    if rt.is_null() {
        return Err("el firmware no da servicios de ejecucion");
    }
    // La tabla la dejó el firmware y sigue ahí (memoria de tipo Runtime Services).
    let rt = unsafe { &*rt };
    sin_interrupciones(|| {
        let (mut libre, mut nuestra) = (None, None);
        let mut v = [0u8; MAX_VAR];
        for n in 0..MAX_NUM {
            match leer(rt, &nombre(&format!("Boot{n:04X}")), &mut v) {
                (EFI_NOT_FOUND, _) => libre = libre.or(Some(n)),
                (0, len) if es_nuestra(&v[..len.min(MAX_VAR)]) => {
                    nuestra = Some(n);
                    break;
                }
                (0 | EFI_BUFFER_TOO_SMALL, _) => {}
                _ => return Err("no se pueden leer las variables UEFI"),
            }
        }
        let n = nuestra.or(libre).ok_or("no hay un numero de entrada de arranque libre")?;
        if escribir(rt, &nombre(&format!("Boot{n:04X}")), &instalar::opcion_arranque(esp, &ruta)) != 0 {
            return Err("el firmware no deja crear la entrada de arranque");
        }
        let mut buf = [0u8; MAX_ORDEN];
        let (st, len) = leer(rt, &nombre("BootOrder"), &mut buf);
        let actual = match st {
            0 => &buf[..len.min(MAX_ORDEN)],
            EFI_NOT_FOUND => &buf[..0],
            _ => return Err("no se puede leer BootOrder"),
        };
        if escribir(rt, &nombre("BootOrder"), &instalar::orden_con(actual, n)) != 0 {
            return Err("no se pudo poner NightFury el primero en BootOrder");
        }
        Ok(())
    })
}
