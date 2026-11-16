//! Framebuffer lineal del firmware.
use crate::boot::BootInfo;
use nf_core::geom::Rect;
use nf_core::surface::Surface;

#[derive(Clone, Copy)]
pub struct Fb {
    base: *mut u32,
    w: i32,
    h: i32,
    stride: usize,
    bgr: bool,
}

impl Fb {
    pub fn new(b: &BootInfo) -> Self {
        Fb { base: b.fb_base as *mut u32, w: b.width as i32, h: b.height as i32, stride: b.stride as usize, bgr: b.bgr }
    }

    pub fn size(&self) -> (i32, i32) {
        (self.w, self.h)
    }

    fn conv(&self, c: u32) -> u32 {
        if self.bgr {
            c
        } else {
            (c & 0xFF00_FF00) | ((c & 0xFF) << 16) | ((c >> 16) & 0xFF)
        }
    }

    /// Rellena toda la pantalla de un color 0x00RRGGBB.
    pub fn clear(&self, color: u32) {
        let c = self.conv(color);
        for y in 0..self.h as usize {
            for x in 0..self.w as usize {
                unsafe { self.base.add(y * self.stride + x).write_volatile(c) };
            }
        }
    }

    /// Copia la zona `r` de `s` a la pantalla, desplazada (ox, oy).
    pub fn present_at(&self, s: &Surface, r: Rect, ox: i32, oy: i32) {
        let r = r.intersect(s.bounds()).intersect(Rect::new(-ox, -oy, self.w, self.h));
        if r.is_empty() {
            return;
        }
        for y in r.y..r.bottom() {
            let src = &s.px[(y * s.w + r.x) as usize..(y * s.w + r.right()) as usize];
            let dst = unsafe { self.base.add((y + oy) as usize * self.stride + (r.x + ox) as usize) };
            if self.bgr {
                unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len()) };
            } else {
                for (i, &c) in src.iter().enumerate() {
                    unsafe { dst.add(i).write(self.conv(c)) };
                }
            }
        }
    }

    pub fn present(&self, s: &Surface, r: Rect) {
        self.present_at(s, r, 0, 0);
    }
}
