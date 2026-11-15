//! Iconos propios dibujados por código. CONTRATO: `Icon` y la firma de `draw` no cambian.
use crate::geom::Rect;
use crate::surface::Surface;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Notepad,
    Calc,
    Terminal,
    Settings,
    Power,
    /// Logo del sistema (luna creciente).
    Logo,
    Info,
    Palette,
}

/// Dibuja `icon` en el cuadrado de lado `size` con esquina en (x, y).
/// `fg` es el color de los iconos de un solo tono (Settings, Power, Logo, Info).
pub fn draw(s: &mut Surface, _icon: Icon, x: i32, y: i32, size: i32, fg: u32) {
    s.frame(Rect::new(x, y, size, size), fg);
}
