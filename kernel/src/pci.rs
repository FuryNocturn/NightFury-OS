// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Configuración PCI por el mecanismo de puertos 0xCF8 (dirección) y 0xCFC (datos).
use crate::arch::{inl, outl};

const DIR: u16 = 0xCF8;
const DATOS: u16 = 0xCFC;

fn dir(bus: u8, dev: u8, fun: u8, reg: u8) -> u32 {
    0x8000_0000 | (bus as u32) << 16 | (dev as u32) << 11 | (fun as u32) << 8 | (reg & 0xFC) as u32
}

pub fn leer(bus: u8, dev: u8, fun: u8, reg: u8) -> u32 {
    unsafe {
        outl(DIR, dir(bus, dev, fun, reg));
        inl(DATOS)
    }
}

pub fn escribir(bus: u8, dev: u8, fun: u8, reg: u8, v: u32) {
    unsafe {
        outl(DIR, dir(bus, dev, fun, reg));
        outl(DATOS, v);
    }
}

/// Todas las funciones PCI de una clase (`clase << 16 | subclase << 8 | interfaz`), como (bus, dispositivo, función).
pub fn buscar(codigo: u32) -> alloc::vec::Vec<(u8, u8, u8)> {
    let mut v = alloc::vec::Vec::new();
    for bus in 0..=255u8 {
        for dev in 0..32u8 {
            if leer(bus, dev, 0, 0) & 0xFFFF == 0xFFFF {
                continue;
            }
            let multi = (leer(bus, dev, 0, 0x0C) >> 16) & 0x80 != 0;
            for fun in 0..(if multi { 8 } else { 1 }) {
                if leer(bus, dev, fun, 0) & 0xFFFF != 0xFFFF && leer(bus, dev, fun, 0x08) >> 8 == codigo {
                    v.push((bus, dev, fun));
                }
            }
        }
    }
    v
}

/// Todos los controladores AHCI (clase 0x01, subclase 0x06, interfaz 0x01): activa espacio de
/// memoria y bus master y devuelve la dirección base de BAR5 (ABAR) de cada uno.
pub fn buscar_ahci() -> alloc::vec::Vec<u64> {
    let mut v = alloc::vec::Vec::new();
    for (bus, dev, fun) in buscar(0x01_06_01) {
        let bar = leer(bus, dev, fun, 0x24);
        if bar & 1 != 0 || bar & !0xF == 0 {
            continue; // BAR5 debe ser de memoria y estar asignado
        }
        let cmd = leer(bus, dev, fun, 0x04);
        escribir(bus, dev, fun, 0x04, cmd | 0b110);
        v.push((bar & !0xF) as u64);
    }
    v
}

/// BAR `n` de memoria (de 32 o 64 bits) como dirección; `None` si es de E/S, no está asignado o
/// queda fuera de lo que el mapeo del núcleo puede direccionar.
pub fn bar(bus: u8, dev: u8, fun: u8, n: u8) -> Option<u64> {
    let r = 0x10 + 4 * n;
    let b = leer(bus, dev, fun, r);
    let mut base = (b & !0xF) as u64;
    if (b >> 1) & 3 == 2 && n < 5 {
        base |= (leer(bus, dev, fun, r + 4) as u64) << 32;
    }
    (b & 1 == 0 && base != 0 && base < 1 << 46).then_some(base)
}

/// Activa espacio de memoria y bus master y desactiva las interrupciones INTx (todo va por sondeo).
pub fn activar(bus: u8, dev: u8, fun: u8) {
    let cmd = leer(bus, dev, fun, 0x04);
    escribir(bus, dev, fun, 0x04, (cmd & 0xFFFF) | 0x406);
}

/// Desplazamientos de la lista de capacidades (espacio de 256 bytes), con tope por si está en bucle.
pub fn capacidades(bus: u8, dev: u8, fun: u8) -> alloc::vec::Vec<u8> {
    let mut v = alloc::vec::Vec::new();
    if leer(bus, dev, fun, 0x04) >> 16 & 0x10 == 0 {
        return v;
    }
    let mut p = (leer(bus, dev, fun, 0x34) & 0xFC) as u8;
    while p >= 0x40 && v.len() < 48 {
        v.push(p);
        p = (leer(bus, dev, fun, p) >> 8 & 0xFC) as u8;
    }
    v
}

// ---------- Espacio de configuración extendido (ECAM, por la tabla MCFG de ACPI) ----------

/// Tabla ACPI en `p` con esa firma y un tamaño razonable; `None` si no.
unsafe fn tabla_acpi(p: usize, firma: &[u8; 4]) -> Option<usize> {
    if p == 0 || core::slice::from_raw_parts(p as *const u8, 4) != firma {
        return None;
    }
    let len = ((p + 4) as *const u32).read_unaligned();
    (36..1 << 20).contains(&len).then_some(p)
}

/// Dirección en el ECAM del registro `reg` (0..4095) de bus/dev/fun, ya mapeada sin caché.
/// `None` si el firmware no da MCFG o el bus no está en ningún rango.
fn ecam(bus: u8, dev: u8, fun: u8, reg: u16) -> Option<usize> {
    let rsdp = crate::boot::info().rsdp as usize;
    if rsdp == 0 || reg >= 4096 {
        return None;
    }
    // Las tablas son del firmware: se valida firma y tamaño y los bucles tienen tope.
    unsafe {
        let r = rsdp as *const u8;
        let (raiz, ancho, firma) = if r.add(15).read() >= 2 {
            (r.add(24).cast::<u64>().read_unaligned() as usize, 8, b"XSDT")
        } else {
            (r.add(16).cast::<u32>().read_unaligned() as usize, 4, b"RSDT")
        };
        let raiz = tabla_acpi(raiz, firma)?;
        let n = (((raiz + 4) as *const u32).read_unaligned() as usize - 36) / ancho;
        for i in 0..n.min(64) {
            let e = (raiz + 36 + i * ancho) as *const u8;
            let t = if ancho == 8 { e.cast::<u64>().read_unaligned() as usize } else { e.cast::<u32>().read_unaligned() as usize };
            let Some(t) = tabla_acpi(t, b"MCFG") else { continue };
            let n = (((t + 4) as *const u32).read_unaligned() as usize).saturating_sub(44) / 16;
            for j in 0..n.min(16) {
                let m = (t + 44 + j * 16) as *const u8;
                let (base, seg, ini, fin) = (m.cast::<u64>().read_unaligned(), m.add(8).cast::<u16>().read_unaligned(), m.add(10).read(), m.add(11).read());
                if seg == 0 && (ini..=fin).contains(&bus) && base < 1 << 46 {
                    let a = base + (((bus - ini) as u64) << 20 | (dev as u64) << 15 | (fun as u64) << 12);
                    // Mapeo de 2 MiB sin caché alrededor de la página de este dispositivo.
                    return crate::paging::mapear_mmio(a, 4096).then_some((a | reg as u64) as usize);
                }
            }
        }
        None
    }
}

/// Capacidad extendida PCIe `id`: desplazamiento de su cabecera. `None` si no hay ECAM o no existe.
pub fn cap_ext(bus: u8, dev: u8, fun: u8, id: u16) -> Option<u16> {
    let mut p = 0x100u16;
    for _ in 0..64 {
        let h = leer_ext(bus, dev, fun, p)?;
        if h == 0 || h == !0 {
            return None;
        }
        if h as u16 == id {
            return Some(p);
        }
        p = (h >> 20 & 0xFFC) as u16;
        if p < 0x100 {
            return None;
        }
    }
    None
}

pub fn leer_ext(bus: u8, dev: u8, fun: u8, reg: u16) -> Option<u32> {
    // Dirección ya mapeada por `ecam`, dentro de la página de configuración del dispositivo.
    Some(unsafe { (ecam(bus, dev, fun, reg & !3)? as *const u32).read_volatile() })
}

pub fn escribir_ext(bus: u8, dev: u8, fun: u8, reg: u16, v: u32) -> bool {
    match ecam(bus, dev, fun, reg & !3) {
        Some(a) => unsafe { (a as *mut u32).write_volatile(v) },
        None => return false,
    }
    true
}
