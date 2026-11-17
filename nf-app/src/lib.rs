//! Runtime de un programa NightFury en modo usuario (F3, v0.8).
//!
//! Un programa es un `Box<dyn App>` (el mismo trait que las apps del sistema). Este crate pone lo
//! demás: arranque, llamadas al núcleo, asignador, panic y el bucle de eventos.
//!
//! - `bucle`: traduce eventos del núcleo a llamadas de `App` y decide cuándo redibujar. No toca
//!   hardware, así que se prueba en el PC con una `Llamadas` falsa.
//! - `rt` (solo `target_os = "none"`): `syscall` en asm, panic, asignador y `arranque`.
//!
//! Un programa entero:
//! ```ignore
//! #![no_std]
//! #![no_main]
//! nf_app::programa!(nf_core::apps::create(nf_core::apps::AppKind::Calc), None);
//! ```
#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod bucle;
#[cfg(target_os = "none")]
pub mod rt;

pub use bucle::{ejecutar, Llamadas};

/// Declara el programa: `programa!(<expresión que crea el Box<dyn App>>, <tick en ms o None>)`.
/// Crea el punto de entrada `_start`: el núcleo lo arranca con `rsp` alineada a 16, y el ABI de
/// Rust espera `rsp + 8` alineada al entrar en una función, de ahí el trampolín en asm.
#[macro_export]
macro_rules! programa {
    ($app:expr, $tick:expr) => {
        ::core::arch::global_asm!(".globl _start", "_start:", "and rsp, -16", "xor ebp, ebp", "call nf_entrada", "ud2");
        #[no_mangle]
        pub extern "C" fn nf_entrada() -> ! {
            $crate::rt::arranque(|| $app, $tick)
        }
    };
}
