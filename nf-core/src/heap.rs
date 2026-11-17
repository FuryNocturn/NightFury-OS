// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Asignador de memoria: lista de bloques libres ordenada por dirección, primer hueco
//! que sirve, con fusión de vecinos al liberar.
use core::ptr::null_mut;

#[repr(C)]
struct Free {
    size: usize,
    next: *mut Free,
}

/// Cabecera delante de cada bloque entregado: [inicio del bloque, tamaño del bloque].
const HDR: usize = 16;

pub struct Heap {
    head: *mut Free,
    total: usize,
    used: usize,
}

unsafe impl Send for Heap {}

impl Heap {
    pub const fn empty() -> Self {
        Heap { head: null_mut(), total: 0, used: 0 }
    }

    /// # Safety
    /// `base..base+size` debe ser memoria válida, exclusiva de este montón.
    pub unsafe fn init(&mut self, base: *mut u8, size: usize) {
        let start = (base as usize + 15) & !15;
        let size = (size - (start - base as usize)) & !15;
        let f = start as *mut Free;
        (*f).size = size;
        (*f).next = null_mut();
        self.head = f;
        self.total = size;
        self.used = 0;
    }

    pub fn total(&self) -> usize {
        self.total
    }
    pub fn used(&self) -> usize {
        self.used
    }

    /// # Safety
    /// El montón debe estar inicializado. `align` debe ser potencia de dos.
    pub unsafe fn alloc(&mut self, size: usize, align: usize) -> *mut u8 {
        let size = ((size + 15) & !15).max(16);
        let align = align.max(16);
        let mut prev: *mut *mut Free = &mut self.head;
        let mut cur = self.head;
        while !cur.is_null() {
            let start = cur as usize;
            let bsize = (*cur).size;
            let next = (*cur).next;
            let payload = (start + HDR + align - 1) & !(align - 1);
            let need = payload - start + size;
            if need <= bsize {
                let rem = bsize - need;
                let taken = if rem >= 32 {
                    let nf = (start + need) as *mut Free;
                    (*nf).size = rem;
                    (*nf).next = next;
                    *prev = nf;
                    need
                } else {
                    *prev = next;
                    bsize
                };
                let h = (payload - HDR) as *mut usize;
                *h = start;
                *h.add(1) = taken;
                self.used += taken;
                return payload as *mut u8;
            }
            prev = &mut (*cur).next;
            cur = next;
        }
        null_mut()
    }

    /// # Safety
    /// `p` debe venir de `alloc` de este montón y no haberse liberado ya.
    pub unsafe fn free(&mut self, p: *mut u8) {
        let h = (p as usize - HDR) as *const usize;
        let start = *h;
        let size = *h.add(1);
        self.used -= size;
        let mut prev: *mut Free = null_mut();
        let mut cur = self.head;
        while !cur.is_null() && (cur as usize) < start {
            prev = cur;
            cur = (*cur).next;
        }
        let blk = start as *mut Free;
        (*blk).size = size;
        (*blk).next = cur;
        if !cur.is_null() && start + size == cur as usize {
            (*blk).size += (*cur).size;
            (*blk).next = (*cur).next;
        }
        if prev.is_null() {
            self.head = blk;
        } else if prev as usize + (*prev).size == start {
            (*prev).size += (*blk).size;
            (*prev).next = (*blk).next;
        } else {
            (*prev).next = blk;
        }
    }

    /// Número de bloques libres (para pruebas y diagnóstico).
    pub fn free_blocks(&self) -> usize {
        let mut n = 0;
        let mut cur = self.head;
        while !cur.is_null() {
            n += 1;
            cur = unsafe { (*cur).next };
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_free_coalesce() {
        let mut mem = vec![0u64; 1 << 16];
        let bytes = mem.len() * 8;
        let mut h = Heap::empty();
        unsafe {
            h.init(mem.as_mut_ptr() as *mut u8, bytes);
            let mut live: Vec<(*mut u8, usize, u8)> = Vec::new();
            let mut seed = 12345u32;
            for round in 0..4000u32 {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let r = seed >> 8;
                if live.len() < 60 && r % 3 != 0 {
                    let size = (r % 900 + 1) as usize;
                    let align = 1usize << (r % 8);
                    let p = h.alloc(size, align);
                    assert!(!p.is_null());
                    assert_eq!(p as usize % align, 0);
                    let tag = round as u8;
                    core::ptr::write_bytes(p, tag, size);
                    live.push((p, size, tag));
                } else if !live.is_empty() {
                    let (p, size, tag) = live.swap_remove(r as usize % live.len());
                    for i in 0..size {
                        assert_eq!(*p.add(i), tag, "memoria pisada");
                    }
                    h.free(p);
                }
            }
            for (p, _, _) in live {
                h.free(p);
            }
        }
        assert_eq!(h.used(), 0);
        assert_eq!(h.free_blocks(), 1);
    }

    #[test]
    fn out_of_memory_returns_null() {
        let mut mem = vec![0u64; 64];
        let mut h = Heap::empty();
        unsafe {
            h.init(mem.as_mut_ptr() as *mut u8, 512);
            assert!(h.alloc(4096, 8).is_null());
            let p = h.alloc(100, 8);
            assert!(!p.is_null());
            h.free(p);
        }
        assert_eq!(h.used(), 0);
    }
}
