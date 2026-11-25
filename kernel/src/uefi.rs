// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Definiciones mínimas de UEFI, escritas a mano a partir de la especificación UEFI.
use core::ffi::c_void;

pub type Status = usize;
pub const EFI_ERROR_UNSUPPORTED: Status = 0x8000_0000_0000_0003;

#[repr(C)]
#[derive(PartialEq)]
pub struct Guid {
    pub d1: u32,
    pub d2: u16,
    pub d3: u16,
    pub d4: [u8; 8],
}

pub const GOP_GUID: Guid =
    Guid { d1: 0x9042a9de, d2: 0x23dc, d3: 0x4a38, d4: [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a] };

/// EFI_LOADED_IMAGE_PROTOCOL: de ella sale el dispositivo desde el que se cargó esta imagen.
pub const LOADED_IMAGE_GUID: Guid =
    Guid { d1: 0x5b1b31a1, d2: 0x9562, d3: 0x11d2, d4: [0x8e, 0x3f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b] };

pub const DEVICE_PATH_GUID: Guid =
    Guid { d1: 0x09576e91, d2: 0x6d3f, d3: 0x11d2, d4: [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b] };

pub const ACPI20_GUID: Guid =
    Guid { d1: 0x8868e871, d2: 0xe4f1, d3: 0x11d3, d4: [0xbc, 0x22, 0x00, 0x80, 0xc7, 0x3c, 0x88, 0x81] };

#[repr(C)]
pub struct TableHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
}

#[repr(C)]
pub struct SimpleTextOutput {
    pub reset: usize,
    pub output_string: extern "efiapi" fn(*mut SimpleTextOutput, *const u16) -> Status,
}

#[repr(C)]
pub struct SystemTable {
    pub hdr: TableHeader,
    pub firmware_vendor: *const u16,
    pub firmware_revision: u32,
    pub console_in_handle: *mut c_void,
    pub con_in: *mut c_void,
    pub console_out_handle: *mut c_void,
    pub con_out: *mut SimpleTextOutput,
    pub standard_error_handle: *mut c_void,
    pub std_err: *mut c_void,
    pub runtime_services: *mut RuntimeServices,
    pub boot_services: *mut BootServices,
    pub number_of_table_entries: usize,
    pub configuration_table: *mut c_void,
}

/// Solo los punteros que se usan; el resto, como relleno en el orden de la especificación.
#[repr(C)]
pub struct BootServices {
    pub hdr: TableHeader,
    _raise_restore_tpl: [usize; 2],
    pub allocate_pages: extern "efiapi" fn(u32, u32, usize, *mut u64) -> Status,
    _free_pages: usize,
    pub get_memory_map: extern "efiapi" fn(*mut usize, *mut u8, *mut usize, *mut usize, *mut u32) -> Status,
    _pool_y_eventos: [usize; 11],
    pub handle_protocol: extern "efiapi" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Status,
    _protocolos_e_imagenes: [usize; 9],
    pub exit_boot_services: extern "efiapi" fn(*mut c_void, usize) -> Status,
    _monotonic_stall: [usize; 2],
    pub set_watchdog_timer: extern "efiapi" fn(usize, u64, usize, *const u16) -> Status,
    _controllers_and_handles: [usize; 7],
    pub locate_protocol: extern "efiapi" fn(*const Guid, *mut c_void, *mut *mut c_void) -> Status,
}

#[repr(C)]
pub struct RuntimeServices {
    pub hdr: TableHeader,
    /// GetTime, SetTime, GetWakeupTime, SetWakeupTime, SetVirtualAddressMap, ConvertPointer.
    _tiempo_y_mapa: [usize; 6],
    pub get_variable: extern "efiapi" fn(*const u16, *const Guid, *mut u32, *mut usize, *mut c_void) -> Status,
    _get_next_variable_name: usize,
    pub set_variable: extern "efiapi" fn(*const u16, *const Guid, u32, usize, *const c_void) -> Status,
    _get_next_high_monotonic_count: usize,
    pub reset_system: extern "efiapi" fn(u32, usize, usize, *const c_void),
}

/// GUID de las variables globales de UEFI (BootOrder, Boot####).
pub const GLOBAL_VARIABLE_GUID: Guid =
    Guid { d1: 0x8be4df61, d2: 0x93ca, d3: 0x11d2, d4: [0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b, 0x8c] };
/// NON_VOLATILE | BOOTSERVICE_ACCESS | RUNTIME_ACCESS.
pub const VAR_NV_BS_RT: u32 = 0x7;
pub const EFI_NOT_FOUND: Status = 0x8000_0000_0000_000E;
pub const EFI_BUFFER_TOO_SMALL: Status = 0x8000_0000_0000_0005;

pub const RESET_COLD: u32 = 0;
pub const RESET_SHUTDOWN: u32 = 2;

#[repr(C)]
pub struct ModeInfo {
    pub version: u32,
    pub horizontal_resolution: u32,
    pub vertical_resolution: u32,
    pub pixel_format: u32,
    pub pixel_information: [u32; 4],
    pub pixels_per_scan_line: u32,
}

#[repr(C)]
pub struct GopMode {
    pub max_mode: u32,
    pub mode: u32,
    pub info: *mut ModeInfo,
    pub size_of_info: usize,
    pub frame_buffer_base: u64,
    pub frame_buffer_size: usize,
}

#[repr(C)]
pub struct Gop {
    pub query_mode: extern "efiapi" fn(*mut Gop, u32, *mut usize, *mut *mut ModeInfo) -> Status,
    pub set_mode: extern "efiapi" fn(*mut Gop, u32) -> Status,
    _blt: usize,
    pub mode: *mut GopMode,
}
