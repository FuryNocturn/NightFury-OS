// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Entrada de arranque UEFI «NightFury» tras instalar: variable `Boot####` y primer puesto en `BootOrder`
//! (UEFI 2.x, 3.1.3). Los Runtime Services se llaman como `ResetSystem`: sin `SetVirtualAddressMap`, con
//! la dirección física de la tabla y las interrupciones cerradas.
use crate::uefi::*;
use alloc::format;
use alloc::vec::Vec;
use nf_core::arranque::{self, Ajustes, InfoArranque};
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

fn leer(rt: &RuntimeServices, n: &[u16], guid: &Guid, buf: &mut [u8]) -> (Status, usize) {
    let mut len = buf.len();
    let st = (rt.get_variable)(n.as_ptr(), guid, core::ptr::null_mut(), &mut len, buf.as_mut_ptr().cast());
    (st, len)
}

fn escribir(rt: &RuntimeServices, n: &[u16], guid: &Guid, v: &[u8]) -> Status {
    (rt.set_variable)(n.as_ptr(), guid, VAR_NV_BS_RT, v.len(), v.as_ptr().cast())
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
            match leer(rt, &nombre(&format!("Boot{n:04X}")), &GLOBAL_VARIABLE_GUID, &mut v) {
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
        if escribir(rt, &nombre(&format!("Boot{n:04X}")), &GLOBAL_VARIABLE_GUID, &instalar::opcion_arranque(esp, &ruta)) != 0 {
            return Err("el firmware no deja crear la entrada de arranque");
        }
        let mut buf = [0u8; MAX_ORDEN];
        let (st, len) = leer(rt, &nombre("BootOrder"), &GLOBAL_VARIABLE_GUID, &mut buf);
        let actual = match st {
            0 => &buf[..len.min(MAX_ORDEN)],
            EFI_NOT_FOUND => &buf[..0],
            _ => return Err("no se puede leer BootOrder"),
        };
        if escribir(rt, &nombre("BootOrder"), &GLOBAL_VARIABLE_GUID, &instalar::orden_con(actual, n)) != 0 {
            return Err("no se pudo poner NightFury el primero en BootOrder");
        }
        Ok(())
    })
}

/// Quita la entrada `Boot####` «NightFury» de la ESP `esp` (la que lleva su GUID única en el nodo Hard Drive;
/// una «NightFury» cuyo destino no se puede leer también) y la saca de `BootOrder` sin perder las demás.
/// Una «NightFury» que apunta a otra ESP se deja: es de otra instalación. `Err` con el motivo si no la hay
/// o el firmware no deja: el quitado del disco ya está hecho.
pub fn quitar(rt: *mut RuntimeServices, esp: &Esp) -> Result<(), &'static str> {
    if rt.is_null() {
        return Err("el firmware no da servicios de ejecucion");
    }
    // La tabla la dejó el firmware y sigue ahí (ver `crear`).
    let rt = unsafe { &*rt };
    sin_interrupciones(|| {
        let (mut por_guid, mut sin_destino) = (None, None);
        let mut v = [0u8; MAX_VAR];
        for n in 0..MAX_NUM {
            if let (0, len) = leer(rt, &nombre(&format!("Boot{n:04X}")), &GLOBAL_VARIABLE_GUID, &mut v) {
                let v = &v[..len.min(MAX_VAR)];
                if es_nuestra(v) {
                    match instalar::unica_de_opcion(v) {
                        Some(g) if g == esp.unica => por_guid = por_guid.or(Some(n)),
                        None => sin_destino = sin_destino.or(Some(n)),
                        Some(_) => {}
                    }
                }
            }
        }
        let n = por_guid.or(sin_destino).ok_or("no hay entrada de arranque de NightFury para esta instalacion")?;
        if escribir(rt, &nombre(&format!("Boot{n:04X}")), &GLOBAL_VARIABLE_GUID, &[]) != 0 {
            return Err("el firmware no deja borrar la entrada de arranque");
        }
        let mut buf = [0u8; MAX_ORDEN];
        match leer(rt, &nombre("BootOrder"), &GLOBAL_VARIABLE_GUID, &mut buf) {
            (0, len) if len <= MAX_ORDEN => {
                if escribir(rt, &nombre("BootOrder"), &GLOBAL_VARIABLE_GUID, &instalar::orden_sin(&buf[..len], n)) != 0 {
                    return Err("no se pudo quitar la entrada de BootOrder");
                }
            }
            (EFI_NOT_FOUND, _) => {}
            _ => return Err("no se puede leer BootOrder"),
        }
        Ok(())
    })
}

// ---------- Menú de arranque (v0.12.7): lo que lee y cambia Configuración ----------

const GUID_NF: Guid = Guid { d1: arranque::GUID_NF.0, d2: arranque::GUID_NF.1, d3: arranque::GUID_NF.2, d4: arranque::GUID_NF.3 };

fn servicios<'a>(rt: *mut RuntimeServices) -> Result<&'a RuntimeServices, &'static str> {
    // La tabla la dejó el firmware y sigue ahí (ver `crear`).
    if rt.is_null() { Err(arranque::ERRORES[0]) } else { Ok(unsafe { &*rt }) }
}

/// Variable propia de hasta `max` bytes; `None` si no existe o no cabe.
fn leer_nf(rt: &RuntimeServices, n: &str, max: usize) -> Option<Vec<u8>> {
    let mut v = alloc::vec![0u8; max];
    let (st, len) = sin_interrupciones(|| leer(rt, &nombre(n), &GUID_NF, &mut v));
    v.truncate(len.min(max));
    (st == 0).then_some(v)
}

/// Número de nuestra entrada `Boot####` (la de descripción «NightFury»).
fn buscar_nuestra(rt: &RuntimeServices) -> Option<u16> {
    let mut v = [0u8; MAX_VAR];
    (0..MAX_NUM).find(|n| matches!(leer(rt, &nombre(&format!("Boot{n:04X}")), &GLOBAL_VARIABLE_GUID, &mut v), (0, len) if es_nuestra(&v[..len.min(MAX_VAR)])))
}

/// `None` si no se arrancó desde `arranque.efi` (no hay `NfArranqueLista`) o no hay servicios de ejecución.
pub fn arranque_leer(rt: *mut RuntimeServices) -> Option<InfoArranque> {
    let rt = servicios(rt).ok()?;
    let entradas = arranque::lista_de_bytes(&leer_nf(rt, arranque::VARIABLE_LISTA, arranque::MAX_LISTA)?)?;
    let ajustes = leer_nf(rt, arranque::VARIABLE, arranque::MAX_AJUSTES).map_or_else(Ajustes::default, |b| Ajustes::de_bytes(&b));
    let primero = sin_interrupciones(|| {
        let mut buf = [0u8; MAX_ORDEN];
        let (st, len) = leer(rt, &nombre("BootOrder"), &GLOBAL_VARIABLE_GUID, &mut buf);
        st == 0 && len >= 2 && buscar_nuestra(rt).is_some_and(|n| buf[..2] == n.to_le_bytes())
    });
    Some(InfoArranque { ajustes, entradas, primero })
}

pub fn arranque_guardar(rt: *mut RuntimeServices, a: &Ajustes) -> Result<(), &'static str> {
    arranque::validar(a)?;
    let rt = servicios(rt)?;
    let b = a.a_bytes();
    if sin_interrupciones(|| escribir(rt, &nombre(arranque::VARIABLE), &GUID_NF, &b)) != 0 {
        return Err(arranque::ERRORES[3]);
    }
    Ok(())
}

/// Pone nuestra entrada la primera de `BootOrder`, sin repetir ni perder las demás.
pub fn arranque_primero(rt: *mut RuntimeServices) -> Result<(), &'static str> {
    let rt = servicios(rt)?;
    sin_interrupciones(|| {
        let n = buscar_nuestra(rt).ok_or(arranque::ERRORES[4])?;
        let mut buf = [0u8; MAX_ORDEN];
        let (st, len) = leer(rt, &nombre("BootOrder"), &GLOBAL_VARIABLE_GUID, &mut buf);
        let actual = match st {
            0 => &buf[..len.min(MAX_ORDEN)],
            EFI_NOT_FOUND => &buf[..0],
            _ => return Err(arranque::ERRORES[5]),
        };
        if escribir(rt, &nombre("BootOrder"), &GLOBAL_VARIABLE_GUID, &instalar::orden_con(actual, n)) != 0 {
            return Err(arranque::ERRORES[5]);
        }
        Ok(())
    })
}
