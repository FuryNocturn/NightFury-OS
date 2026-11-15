//! Tipografía propia de trazos. CONTRATO: estas firmas públicas no cambian.
//! `size` es la altura de las mayúsculas en píxeles (9 = texto normal de interfaz).
use crate::geom::Rect;
use crate::surface::Surface;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Alto de una línea de texto, en píxeles.
pub fn line_h(size: u32) -> i32 {
    size as i32 + 7
}

/// Avance horizontal de un carácter, en píxeles.
pub fn adv(_c: char, size: u32, _bold: bool) -> i32 {
    size as i32
}

/// Ancho de `text` en píxeles (suma de avances).
pub fn width(text: &str, size: u32, bold: bool) -> i32 {
    text.chars().map(|c| adv(c, size, bold)).sum()
}

/// Dibuja `text` con la esquina superior izquierda de su caja de línea en (x, y).
/// Devuelve la x final (x + ancho).
pub fn draw(_s: &mut Surface, x: i32, _y: i32, text: &str, size: u32, bold: bool, _color: u32) -> i32 {
    x + width(text, size, bold)
}

/// Dibuja `text` centrado en vertical dentro de `r` y alineado en horizontal según `align`.
pub fn draw_in(s: &mut Surface, r: Rect, text: &str, size: u32, bold: bool, color: u32, align: Align) {
    let w = width(text, size, bold);
    let x = match align {
        Align::Left => r.x,
        Align::Center => r.x + (r.w - w) / 2,
        Align::Right => r.right() - w,
    };
    draw(s, x, r.y + (r.h - line_h(size)) / 2, text, size, bold, color);
}
