//! NightFury: núcleo. Provisional hasta que se escriba el arranque.
#![no_std]
#![no_main]

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[no_mangle]
pub extern "efiapi" fn efi_main(_image: *mut core::ffi::c_void, _st: *mut core::ffi::c_void) -> usize {
    0
}
