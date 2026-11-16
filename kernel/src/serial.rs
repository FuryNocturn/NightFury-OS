//! Puerto serie COM1, solo para trazas de depuración.
use crate::arch::{inb, outb};
use core::fmt::Write;

const COM1: u16 = 0x3F8;

pub fn init() {
    unsafe {
        outb(COM1 + 1, 0);
        outb(COM1 + 3, 0x80);
        outb(COM1, 1);
        outb(COM1 + 1, 0);
        outb(COM1 + 3, 3);
        outb(COM1 + 2, 0xC7);
        outb(COM1 + 4, 0x0B);
    }
}

pub struct Com1;

impl Write for Com1 {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes() {
            for _ in 0..100_000 {
                if unsafe { inb(COM1 + 5) } & 0x20 != 0 {
                    break;
                }
            }
            unsafe { outb(COM1, b) };
        }
        Ok(())
    }
}

/// Escribe una línea por el serie.
macro_rules! traza {
    ($($a:tt)*) => {{
        let _ = core::fmt::Write::write_fmt(&mut $crate::serial::Com1, format_args!($($a)*));
        let _ = core::fmt::Write::write_str(&mut $crate::serial::Com1, "\n");
    }};
}
pub(crate) use traza;
