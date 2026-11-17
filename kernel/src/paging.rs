//! Tablas de páginas: las del núcleo (identidad con páginas de 2 MiB, solo supervisor) y las
//! piezas para que cada programa tenga las suyas (`user.rs`).
use alloc::alloc::{alloc_zeroed, Layout};
use core::arch::asm;
use core::arch::x86_64::{__cpuid, __cpuid_count};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};

pub const P: u64 = 1;
pub const W: u64 = 2;
pub const U: u64 = 4;
pub const NX: u64 = 1 << 63;
const PS: u64 = 1 << 7;
pub const ADDR: u64 = 0x000F_FFFF_FFFF_F000;
const GIB: u64 = 1 << 30;
const EFER: u32 = 0xC000_0080;

/// PML4 del núcleo (la que se carga en CR3 cuando no corre ningún programa).
static KERNEL_CR3: AtomicU64 = AtomicU64::new(0);
/// El procesador sabe de NX (y ya está activado): sin él no se pueden cargar programas.
pub static NX_OK: AtomicBool = AtomicBool::new(false);

/// # Safety
/// Leer un MSR inexistente da #GP.
pub unsafe fn rdmsr(m: u32) -> u64 {
    let (lo, hi): (u32, u32);
    asm!("rdmsr", in("ecx") m, out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags));
    (hi as u64) << 32 | lo as u64
}

/// # Safety
/// Escribir un MSR puede cambiar el modo de la CPU.
pub unsafe fn wrmsr(m: u32, v: u64) {
    asm!("wrmsr", in("ecx") m, in("eax") v as u32, in("edx") (v >> 32) as u32, options(nomem, nostack, preserves_flags));
}

/// # Safety
/// `cr3` debe apuntar a una PML4 que mapee el código y la pila actuales.
pub unsafe fn set_cr3(v: u64) {
    asm!("mov cr3, {}", in(reg) v, options(nostack));
}

pub fn cr3() -> u64 {
    let v: u64;
    // Leer CR3 no tiene efectos.
    unsafe { asm!("mov {}, cr3", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

pub fn kernel_cr3() -> u64 {
    KERNEL_CR3.load(Relaxed)
}

/// `n` páginas contiguas a cero, alineadas a 4 KiB, del montón del núcleo (físico = virtual).
/// Nulo si no hay memoria.
pub fn alloc_pages(n: usize) -> (*mut u8, Layout) {
    let l = Layout::from_size_align(n * 4096, 4096).unwrap();
    // El montón está inicializado antes de llamar a nada de este módulo.
    (unsafe { alloc_zeroed(l) }, l)
}

/// Crea las tablas del núcleo y las activa. `fin_ram` y `fin_fb` en bytes físicos.
/// Las BAR de AHCI son de 32 bits (`pci.rs`), así que el mínimo de 4 GiB ya las cubre.
pub fn init(fin_ram: u64, fin_fb: u64) {
    // shortcut: con más de 512 GiB de RAM el resto queda sin mapear (una sola PDPT), cuando haga falta.
    let gibs = fin_ram.max(fin_fb).max(4 * GIB).div_ceil(GIB).min(512);
    let (pml4, _) = alloc_pages(1);
    let (pdpt, _) = alloc_pages(1);
    assert!(!pml4.is_null() && !pdpt.is_null(), "sin memoria para las tablas de páginas");
    // Las tablas son páginas recién reservadas, de uso exclusivo; `g < 512` e `i < 512` caben.
    unsafe {
        (pml4 as *mut u64).write(pdpt as u64 | P | W);
        for g in 0..gibs {
            let (pd, _) = alloc_pages(1);
            assert!(!pd.is_null(), "sin memoria para las tablas de páginas");
            for i in 0..512u64 {
                (pd as *mut u64).add(i as usize).write(g * GIB + i * (2 << 20) | P | W | PS);
            }
            (pdpt as *mut u64).add(g as usize).write(pd as u64 | P | W);
        }
        // NX (CPUID 0x8000_0001 EDX bit 20) antes de que ninguna tabla lo use.
        if __cpuid(0x8000_0000).eax >= 0x8000_0001 && __cpuid(0x8000_0001).edx & (1 << 20) != 0 {
            wrmsr(EFER, rdmsr(EFER) | 1 << 11);
            NX_OK.store(true, Relaxed);
        }
        // SMEP (CPUID 7.0 EBX bit 7): el núcleo no ejecuta páginas de usuario por error. Sin PGE
        // (bit 7 de CR4) para que cargar CR3 vacíe también las entradas globales de UEFI.
        let mut cr4: u64;
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        cr4 &= !(1 << 7);
        if __cpuid(0).eax >= 7 && __cpuid_count(7, 0).ebx & (1 << 7) != 0 {
            cr4 |= 1 << 20;
        }
        asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack, preserves_flags));
        KERNEL_CR3.store(pml4 as u64, Relaxed);
        set_cr3(pml4 as u64);
    }
}
