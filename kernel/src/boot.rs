// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Entrada UEFI: elige el modo de vídeo, reserva la memoria, sale de los servicios de
//! arranque y salta al núcleo con pila propia.
use crate::uefi::*;
use core::ffi::c_void;
use core::ptr::{null, null_mut};

/// Lo que el arranque le deja al núcleo.
#[derive(Clone, Copy)]
pub struct BootInfo {
    pub fb_base: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bgr: bool,
    pub heap_base: u64,
    pub heap_size: u64,
    pub ram_total: u64,
    /// Fin (exclusivo) de la RAM más alta según el mapa de memoria.
    pub ram_end: u64,
    pub runtime: *mut RuntimeServices,
    /// Dirección de la RSDP de ACPI (0 si el firmware no la da).
    pub rsdp: u64,
}

static mut BOOT: BootInfo = BootInfo {
    fb_base: 0,
    width: 0,
    height: 0,
    stride: 0,
    bgr: true,
    heap_base: 0,
    heap_size: 0,
    ram_total: 0,
    ram_end: 0,
    runtime: null_mut(),
    rsdp: 0,
};

pub fn info() -> BootInfo {
    // Se escribe una sola vez en el arranque, antes de `kmain`.
    unsafe { core::ptr::addr_of!(BOOT).read() }
}

const MIB: u64 = 1 << 20;
const STACK_SIZE: u64 = MIB;

fn print(st: &SystemTable, msg: &str) {
    let mut buf = [0u16; 128];
    for (i, b) in msg.bytes().take(126).enumerate() {
        buf[i] = b as u16;
    }
    if !st.con_out.is_null() {
        unsafe { ((*st.con_out).output_string)(st.con_out, buf.as_ptr()) };
    }
}

/// Modo de vídeo utilizable (formato RGB o BGR): devuelve su resolución.
fn usable(gop: *mut Gop, mode: u32) -> Option<(u32, u32)> {
    let mut size = 0usize;
    let mut mi: *mut ModeInfo = null_mut();
    unsafe {
        if ((*gop).query_mode)(gop, mode, &mut size, &mut mi) != 0 || mi.is_null() {
            return None;
        }
        let m = &*mi;
        (m.pixel_format <= 1).then_some((m.horizontal_resolution, m.vertical_resolution))
    }
}

#[no_mangle]
pub extern "efiapi" fn efi_main(image: *mut c_void, st: *mut SystemTable) -> usize {
    let st = unsafe { &*st };
    let bs = unsafe { &*st.boot_services };
    (bs.set_watchdog_timer)(0, 0, 0, null());

    // Vídeo.
    let mut gop: *mut Gop = null_mut();
    let s = (bs.locate_protocol)(&GOP_GUID, null_mut(), &mut gop as *mut _ as *mut *mut c_void);
    if s != 0 || gop.is_null() {
        print(st, "NightFury: no hay salida de video (GOP).\r\n");
        return EFI_ERROR_UNSUPPORTED;
    }
    unsafe {
        let cur = (*(*gop).mode).mode;
        let keep = matches!(usable(gop, cur), Some((w, _)) if w >= 1024);
        if !keep {
            let mut best: Option<(u32, u64)> = None;
            for m in 0..(*(*gop).mode).max_mode {
                if let Some((w, h)) = usable(gop, m) {
                    let area = w as u64 * h as u64;
                    if w <= 1920 && h <= 1080 && best.map_or(true, |(_, a)| area > a) {
                        best = Some((m, area));
                    }
                }
            }
            match best {
                Some((m, _)) => {
                    if m != cur {
                        ((*gop).set_mode)(gop, m);
                    }
                }
                None => {
                    print(st, "NightFury: ningun modo de video compatible.\r\n");
                    return EFI_ERROR_UNSUPPORTED;
                }
            }
        }
    }
    let (fb_base, width, height, stride, bgr) = unsafe {
        let mode = &*(*gop).mode;
        let i = &*mode.info;
        (mode.frame_buffer_base, i.horizontal_resolution, i.vertical_resolution, i.pixels_per_scan_line, i.pixel_format == 1)
    };

    // Memoria del sistema: pila (primer MiB) y montón (el resto).
    let mut base = 0u64;
    let mut size = 0u64;
    for mib in [1024u64, 512, 256, 128, 64, 32] {
        let mut addr = 0u64;
        if (bs.allocate_pages)(0, 2, (mib * MIB / 4096) as usize, &mut addr) == 0 {
            base = addr;
            size = mib * MIB;
            break;
        }
    }
    if size == 0 {
        print(st, "NightFury: no hay memoria suficiente.\r\n");
        return EFI_ERROR_UNSUPPORTED;
    }
    const MAP_PAGES: usize = 64;
    let mut map_addr = 0u64;
    if (bs.allocate_pages)(0, 2, MAP_PAGES, &mut map_addr) != 0 {
        print(st, "NightFury: no hay memoria para el mapa.\r\n");
        return EFI_ERROR_UNSUPPORTED;
    }

    // RSDP de ACPI 2.0 (tabla de configuración con su GUID); la memoria de las tablas ACPI sobrevive.
    let rsdp = (0..st.number_of_table_entries)
        .map(|i| unsafe { st.configuration_table.cast::<u8>().add(i * 24) })
        .find(|&e| unsafe { e.cast::<Guid>().read_unaligned() } == ACPI20_GUID)
        .map_or(0, |e| unsafe { e.add(16).cast::<u64>().read_unaligned() });

    // Salida de los servicios de arranque (el mapa puede cambiar, así que se reintenta).
    let mut ram_total = 0u64;
    let mut ram_end = 0u64;
    let mut exited = false;
    for _ in 0..3 {
        let mut map_size = MAP_PAGES * 4096;
        let (mut key, mut dsize, mut dver) = (0usize, 0usize, 0u32);
        if (bs.get_memory_map)(&mut map_size, map_addr as *mut u8, &mut key, &mut dsize, &mut dver) != 0 || dsize == 0 {
            continue;
        }
        if (bs.exit_boot_services)(image, key) == 0 {
            // Descriptor: tipo u32 en el desplazamiento 0, número de páginas u64 en el 24.
            for off in (0..map_size).step_by(dsize) {
                let d = (map_addr as usize + off) as *const u8;
                let (ty, start, pages) = unsafe { (d.cast::<u32>().read_unaligned(), d.add(8).cast::<u64>().read_unaligned(), d.add(24).cast::<u64>().read_unaligned()) };
                if matches!(ty, 1..=7 | 9 | 10 | 14) {
                    ram_total += pages * 4096;
                    ram_end = ram_end.max(start + pages * 4096);
                }
            }
            exited = true;
            break;
        }
    }
    if !exited {
        print(st, "NightFury: no se pudo salir de los servicios de arranque.\r\n");
        return EFI_ERROR_UNSUPPORTED;
    }

    unsafe {
        core::arch::asm!("cli");
        core::ptr::addr_of_mut!(BOOT).write(BootInfo {
            fb_base,
            width,
            height,
            stride,
            bgr,
            heap_base: base + STACK_SIZE,
            heap_size: size - STACK_SIZE,
            ram_total,
            ram_end,
            runtime: st.runtime_services,
            rsdp,
        });
        let top = (base + STACK_SIZE) & !15;
        core::arch::asm!(
            "mov rsp, {top}",
            "sub rsp, 32",
            "call {k}",
            top = in(reg) top,
            k = sym crate::kmain,
            options(noreturn)
        );
    }
}
