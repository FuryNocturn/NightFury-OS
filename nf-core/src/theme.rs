//! Tema y fondos de escritorio propios. CONTRATO: `Theme`, `ACCENTS`, `WALLPAPERS` y la
//! firma de `render_wallpaper` no cambian.
use crate::surface::Surface;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Theme {
    /// Color de énfasis 0xRRGGBB.
    pub accent: u32,
    /// Índice en `WALLPAPERS`.
    pub wallpaper: u8,
}

/// Paleta de colores de énfasis que ofrece Configuración. El primero es el de por defecto.
pub const ACCENTS: [u32; 12] = [
    0x1877D4, 0x0E8A8A, 0x2E8B57, 0x6B8E23, 0xC98A0B, 0xE0701A, 0xD13B3B, 0xC2398F, 0x8A4FD6, 0x5A5FD6, 0x5D6B7A,
    0x3A3F47,
];

/// Nombres de los fondos disponibles, por índice.
pub const WALLPAPERS: [&str; 4] = ["Noche", "Aurora", "Amanecer", "Color sólido"];

impl Default for Theme {
    fn default() -> Self {
        Theme { accent: ACCENTS[0], wallpaper: 0 }
    }
}

/// Genera el fondo `id` a tamaño w x h. El fondo 3 usa `accent`.
pub fn render_wallpaper(_id: u8, w: i32, h: i32, accent: u32) -> Surface {
    Surface::new(w, h, crate::surface::darken(accent, 120))
}
