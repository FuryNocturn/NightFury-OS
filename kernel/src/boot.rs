// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Entrada UEFI: elige el modo de vídeo, reserva la memoria, sale de los servicios de
//! arranque y salta al núcleo con pila propia.
use crate::uefi::*;
use core::ffi::c_void;
use core::ptr::{null, null_mut};

/// Se arrancó desde un USB: nodo Hard Drive de la ruta del dispositivo (la partición de la imagen), si lo hay.
#[derive(Clone, Copy)]
pub struct ArranqueUsb {
    pub particion: u32,
    pub inicio: u64,
    pub tamano: u64,
    /// Firma GUID de la partición (en el orden de la GPT); `None` si el nodo no la trae o no hay nodo.
    pub guid: Option<[u8; 16]>,
}

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
    /// `Some` si la imagen se cargó desde un dispositivo USB.
    pub usb: Option<ArranqueUsb>,
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
    usb: None,
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

/// ¿Se cargó esta imagen desde un USB? Mira la ruta del dispositivo (UEFI 2.x, «Device Path Protocol»): un nodo
/// Messaging (3) USB (5), USB Class (0xF) o USB WWID (0x10). Cualquier fallo cuenta como «no».
fn arranque_usb(image: *mut c_void, bs: &BootServices) -> Option<ArranqueUsb> {
    let mut li: *mut c_void = null_mut();
    if (bs.handle_protocol)(image, &LOADED_IMAGE_GUID, &mut li) != 0 || li.is_null() {
        return None;
    }
    // EFI_LOADED_IMAGE_PROTOCOL: Revision (u32) en 0, ParentHandle en 8, SystemTable en 16, DeviceHandle en 24.
    let dev = unsafe { li.cast::<u8>().add(24).cast::<*mut c_void>().read_unaligned() };
    let mut dp: *mut c_void = null_mut();
    if dev.is_null() || (bs.handle_protocol)(dev, &DEVICE_PATH_GUID, &mut dp) != 0 || dp.is_null() {
        return None;
    }
    let (mut usb, mut hd) = (false, None);
    let mut p = dp.cast::<u8>();
    // Tope de nodos: una ruta rota no puede dejar el arranque dando vueltas.
    for _ in 0..64 {
        let (t, s, l) = unsafe { (p.read(), p.add(1).read(), u16::from_le_bytes([p.add(2).read(), p.add(3).read()]) as usize) };
        if t == 0x7F || t == 0xFF || l < 4 {
            break;
        }
        match (t, s) {
            (3, 0x05 | 0x0F | 0x10) => usb = true,
            // Hard Drive: partición u32 en 4, inicio u64 en 8, tamaño u64 en 16, firma de 16 bytes en 24 y tipo de firma en 41 (2 = GUID).
            (4, 1) if l >= 42 => unsafe {
                let mut g = [0u8; 16];
                core::ptr::copy_nonoverlapping(p.add(24), g.as_mut_ptr(), 16);
                let guid = (p.add(41).read() == 2).then_some(g);
                hd = Some(ArranqueUsb { particion: p.add(4).cast::<u32>().read_unaligned(), inicio: p.add(8).cast::<u64>().read_unaligned(), tamano: p.add(16).cast::<u64>().read_unaligned(), guid });
            },
            _ => {}
        }
        p = unsafe { p.add(l) };
    }
    usb.then(|| hd.unwrap_or(ArranqueUsb { particion: 0, inicio: 0, tamano: 0, guid: None }))
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

    let usb = arranque_usb(image, bs);

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
            usb,
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
