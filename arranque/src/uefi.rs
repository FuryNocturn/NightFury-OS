// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Definiciones mínimas de UEFI 2.x (solo lo que usa el menú de arranque), escritas a mano de la especificación.
#![no_std]
use core::ffi::c_void;

pub type Status = usize;
pub type Handle = *mut c_void;
pub type Event = *mut c_void;
pub const EFI_NOT_FOUND: Status = 0x8000_0000_0000_000E;
pub const EFI_BUFFER_TOO_SMALL: Status = 0x8000_0000_0000_0005;

#[repr(C)]
pub struct Guid {
    pub d1: u32,
    pub d2: u16,
    pub d3: u16,
    pub d4: [u8; 8],
}

pub const SIMPLE_FILE_SYSTEM_GUID: Guid =
    Guid { d1: 0x964e5b22, d2: 0x6459, d3: 0x11d2, d4: [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b] };
pub const DEVICE_PATH_GUID: Guid =
    Guid { d1: 0x09576e91, d2: 0x6d3f, d3: 0x11d2, d4: [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b] };
pub const GLOBAL_VARIABLE_GUID: Guid =
    Guid { d1: 0x8be4df61, d2: 0x93ca, d3: 0x11d2, d4: [0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b, 0x8c] };

/// NON_VOLATILE | BOOTSERVICE_ACCESS | RUNTIME_ACCESS.
pub const VAR_NV_BS_RT: u32 = 0x7;
pub const RESET_COLD: u32 = 0;
pub const RESET_SHUTDOWN: u32 = 2;
pub const EVT_TIMER: u32 = 0x8000_0000;
pub const TIMER_PERIODIC: u32 = 1;
pub const TPL_APPLICATION: usize = 4;
pub const POOL_LOADER_DATA: u32 = 2;
/// LocateHandleBuffer: ByProtocol.
pub const BY_PROTOCOL: u32 = 2;
pub const FILE_MODE_READ: u64 = 1;
pub const FILE_DIRECTORY: u64 = 0x10;

#[repr(C)]
pub struct TableHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
}

#[repr(C)]
pub struct InputKey {
    pub scan_code: u16,
    pub unicode_char: u16,
}

#[repr(C)]
pub struct SimpleTextInput {
    pub reset: usize,
    pub read_key_stroke: extern "efiapi" fn(*mut SimpleTextInput, *mut InputKey) -> Status,
    pub wait_for_key: Event,
}

#[repr(C)]
pub struct SimpleTextOutput {
    pub reset: usize,
    pub output_string: extern "efiapi" fn(*mut SimpleTextOutput, *const u16) -> Status,
    pub test_string: extern "efiapi" fn(*mut SimpleTextOutput, *const u16) -> Status,
    /// QueryMode, SetMode.
    _modo: [usize; 2],
    pub set_attribute: extern "efiapi" fn(*mut SimpleTextOutput, usize) -> Status,
    pub clear_screen: extern "efiapi" fn(*mut SimpleTextOutput) -> Status,
    _set_cursor_position: usize,
    pub enable_cursor: extern "efiapi" fn(*mut SimpleTextOutput, u8) -> Status,
}

#[repr(C)]
pub struct SystemTable {
    pub hdr: TableHeader,
    pub firmware_vendor: *const u16,
    pub firmware_revision: u32,
    pub console_in_handle: Handle,
    pub con_in: *mut SimpleTextInput,
    pub console_out_handle: Handle,
    pub con_out: *mut SimpleTextOutput,
    pub standard_error_handle: Handle,
    pub std_err: *mut SimpleTextOutput,
    pub runtime_services: *mut RuntimeServices,
    pub boot_services: *mut BootServices,
}

/// Todos los punteros en el orden de la especificación (los que no se usan, sin tipo).
#[repr(C)]
pub struct BootServices {
    pub hdr: TableHeader,
    /// RaiseTPL, RestoreTPL, AllocatePages, FreePages, GetMemoryMap.
    _tpl_y_paginas: [usize; 5],
    pub allocate_pool: extern "efiapi" fn(u32, usize, *mut *mut u8) -> Status,
    pub free_pool: extern "efiapi" fn(*mut u8) -> Status,
    pub create_event: extern "efiapi" fn(u32, usize, usize, *mut c_void, *mut Event) -> Status,
    pub set_timer: extern "efiapi" fn(Event, u32, u64) -> Status,
    pub wait_for_event: extern "efiapi" fn(usize, *mut Event, *mut usize) -> Status,
    _signal_event: usize,
    pub close_event: extern "efiapi" fn(Event) -> Status,
    pub check_event: extern "efiapi" fn(Event) -> Status,
    /// Install, Reinstall y UninstallProtocolInterface.
    _protocolos: [usize; 3],
    pub handle_protocol: extern "efiapi" fn(Handle, *const Guid, *mut *mut c_void) -> Status,
    /// Reserved, RegisterProtocolNotify, LocateHandle, LocateDevicePath, InstallConfigurationTable.
    _localizar: [usize; 5],
    pub load_image: extern "efiapi" fn(u8, Handle, *const u8, *const c_void, usize, *mut Handle) -> Status,
    pub start_image: extern "efiapi" fn(Handle, *mut usize, *mut *mut u16) -> Status,
    /// Exit, UnloadImage, ExitBootServices, GetNextMonotonicCount, Stall.
    _salir_y_tiempo: [usize; 5],
    pub set_watchdog_timer: extern "efiapi" fn(usize, u64, usize, *const u16) -> Status,
    /// ConnectController, DisconnectController, OpenProtocol, CloseProtocol, OpenProtocolInformation, ProtocolsPerHandle.
    _controladores: [usize; 6],
    pub locate_handle_buffer: extern "efiapi" fn(u32, *const Guid, *mut c_void, *mut usize, *mut *mut Handle) -> Status,
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

#[repr(C)]
pub struct SimpleFileSystem {
    pub revision: u64,
    pub open_volume: extern "efiapi" fn(*mut SimpleFileSystem, *mut *mut File) -> Status,
}

#[repr(C)]
pub struct File {
    pub revision: u64,
    pub open: extern "efiapi" fn(*mut File, *mut *mut File, *const u16, u64, u64) -> Status,
    pub close: extern "efiapi" fn(*mut File) -> Status,
    _delete: usize,
    pub read: extern "efiapi" fn(*mut File, *mut usize, *mut u8) -> Status,
}
