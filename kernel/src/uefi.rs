//! Definiciones mínimas de UEFI, escritas a mano a partir de la especificación UEFI.
use core::ffi::c_void;

pub type Status = usize;
pub const EFI_ERROR_UNSUPPORTED: Status = 0x8000_0000_0000_0003;

#[repr(C)]
pub struct Guid {
    pub d1: u32,
    pub d2: u16,
    pub d3: u16,
    pub d4: [u8; 8],
}

pub const GOP_GUID: Guid =
    Guid { d1: 0x9042a9de, d2: 0x23dc, d3: 0x4a38, d4: [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a] };

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
    _pool_events_protocols_images: [usize; 21],
    pub exit_boot_services: extern "efiapi" fn(*mut c_void, usize) -> Status,
    _monotonic_stall: [usize; 2],
    pub set_watchdog_timer: extern "efiapi" fn(usize, u64, usize, *const u16) -> Status,
    _controllers_and_handles: [usize; 7],
    pub locate_protocol: extern "efiapi" fn(*const Guid, *mut c_void, *mut *mut c_void) -> Status,
}

#[repr(C)]
pub struct RuntimeServices {
    pub hdr: TableHeader,
    _time_variables: [usize; 10],
    pub reset_system: extern "efiapi" fn(u32, usize, usize, *const c_void),
}

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
