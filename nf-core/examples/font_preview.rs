//! Vista previa de la tipografía: `cargo run -p nf-core --example font_preview -- salida.bmp`
//! Dibuja sobre fondo blanco y oscuro y amplía x3 para ver el detalle a tamaño 9.
use nf_core::font::{draw, line_h};
use nf_core::surface::Surface;

const ZOOM: i32 = 3;

fn bloque(s: &mut Surface, y0: i32, fondo: u32, tinta: u32) -> i32 {
    let mut y = y0;
    let frases = [
        "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        "abcdefghijklmnopqrstuvwxyz",
        "0123456789  . , : ; ! ¡ ? ¿ - _ + = * / \\ | ( ) [ ] { } < > ' \" ` ´ ^ ~ ¨",
        "# $ % & @ º ª · ¬ € ç Ç × ÷ − ± ² √ ← → ↑ ↓ › ‹ ° … ✓",
        "áéíóú ÁÉÍÓÚ àèìòù ÀÈÌÒÙ äëïöü ÄËÏÖÜ âêîôû ÂÊÎÔÛ ñ Ñ",
        "El veloz murciélago hindú comía feliz cardillo y kiwi. ¿Qué tal? ¡Ñandú! 0123456789",
        "Bloc de notas",
        "Configuración",
    ];
    s.fill_rect(nf_core::geom::Rect::new(0, y0, s.w, 4000), fondo);
    for (size, bold) in [(9, false), (9, true), (12, false), (16, false), (26, true)] {
        for f in frases {
            // Los tamaños grandes solo con las frases cortas y el alfabeto.
            if size > 16 && f.chars().count() > 30 && !f.starts_with("El veloz") {
                continue;
            }
            if size > 16 && f.starts_with("El veloz") {
                continue;
            }
            draw(s, 6, y, f, size, bold, tinta);
            y += line_h(size) + 1;
        }
        y += 6;
    }
    y
}

fn main() {
    let ruta = std::env::args().nth(1).expect("uso: font_preview <salida.bmp>");
    let mut s = Surface::new(1100, 2000, 0xFFFFFF);
    let y1 = bloque(&mut s, 0, 0xFFFFFF, 0x101010);
    let y2 = bloque(&mut s, y1, 0x20242C, 0xE8E8E8);
    // Recorte y ampliación x3 (nitidez a tamaño 9).
    let (w, h) = (s.w, y2);
    let mut g = Surface::new(w * ZOOM, h * ZOOM, 0);
    for y in 0..h * ZOOM {
        for x in 0..w * ZOOM {
            g.px[(y * g.w + x) as usize] = s.px[((y / ZOOM) * s.w + x / ZOOM) as usize];
        }
    }
    std::fs::write(ruta, nf_core::bmp::encode(&g)).unwrap();
}
