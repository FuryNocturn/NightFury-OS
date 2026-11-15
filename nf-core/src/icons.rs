//! Iconos propios dibujados por código. CONTRATO: `Icon` y la firma de `draw` no cambian.
//! Rejilla lógica de 16 unidades; `k = size / 16`. Los rectángulos se ajustan a píxeles enteros.
use crate::geom::Rect;
use crate::math::{cos, iround, sin, PI};
use crate::surface::{rgb, Surface};

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

/// Rectángulo en unidades de rejilla (x0,y0)-(x1,y1), con bordes redondeados a píxel y mínimo 1 px.
fn cell(s: &mut Surface, o: (i32, i32), k: f32, r: (f32, f32, f32, f32), c: u32) {
    let (x0, y0) = (iround(r.0 * k), iround(r.1 * k));
    let (x1, y1) = (iround(r.2 * k).max(x0 + 1), iround(r.3 * k).max(y0 + 1));
    s.fill_rect(Rect::new(o.0 + x0, o.1 + y0, x1 - x0, y1 - y0), c);
}

/// Dibuja `icon` en el cuadrado de lado `size` con esquina en (x, y).
/// `fg` es el color de los iconos de un solo tono (Settings, Power, Logo, Info).
pub fn draw(s: &mut Surface, icon: Icon, x: i32, y: i32, size: i32, fg: u32) {
    if size <= 0 {
        return;
    }
    // Todo el dibujo queda recortado al cuadrado del icono.
    let old = s.clip();
    s.set_clip(old.intersect(Rect::new(x, y, size, size)));
    let k = size as f32 / 16.0;
    let o = (x, y);
    // Punto de la rejilla en coordenadas de superficie.
    let p = |u: f32, v: f32| (x as f32 + u * k, y as f32 + v * k);
    // Grosor de trazo en píxeles, nunca menor de 1,3 para que se vea a 16 px.
    let tw = |u: f32| (u * k).max(1.3);
    match icon {
        Icon::Notepad => {
            cell(s, o, k, (3.0, 1.0, 13.0, 15.0), rgb(0x9A, 0xA0, 0xA6));
            cell(s, o, k, (4.0, 2.0, 12.0, 14.0), 0xFFFFFF);
            cell(s, o, k, (4.0, 2.0, 12.0, 4.0), rgb(0x2F, 0x7B, 0xE0));
            for (i, yy) in [6.0f32, 8.0, 10.0, 12.0].iter().enumerate() {
                let x1 = if i == 3 { 8.0 } else { 11.0 };
                cell(s, o, k, (5.0, *yy, x1, yy + 1.0), rgb(0xA8, 0xAD, 0xB3));
            }
        }
        Icon::Calc => {
            cell(s, o, k, (2.0, 1.0, 14.0, 16.0), rgb(0x3C, 0x40, 0x43));
            cell(s, o, k, (3.0, 3.0, 13.0, 6.0), rgb(0xD6, 0xE6, 0xDA));
            for r in 0..3 {
                for c in 0..3 {
                    let (kx, ky) = (3.0 + c as f32 * 3.5, 7.0 + r as f32 * 3.0);
                    let col = if r == 2 && c == 2 { rgb(0xF2, 0x8B, 0x30) } else { rgb(0xC9, 0xCD, 0xD2) };
                    cell(s, o, k, (kx, ky, kx + 2.5, ky + 2.0), col);
                }
            }
        }
        Icon::Terminal => {
            cell(s, o, k, (1.0, 2.0, 15.0, 14.0), rgb(0xB4, 0xB8, 0xBD));
            cell(s, o, k, (2.0, 3.0, 14.0, 13.0), rgb(0x0C, 0x0C, 0x0C));
            cell(s, o, k, (2.0, 3.0, 14.0, 5.0), rgb(0x5C, 0x60, 0x66));
            let t = tw(1.1);
            let (a, b, c) = (p(3.8, 6.8), p(6.4, 8.8), p(3.8, 10.8));
            s.stroke(&[a, b, c], t, 0xFFFFFF);
            s.stroke(&[p(8.0, 11.0), p(11.5, 11.0)], t, 0xFFFFFF);
        }
        Icon::Settings => {
            let (cx, cy) = p(8.0, 8.0);
            s.arc(cx, cy, 3.7 * k, 0.0, 360.0, tw(2.6), fg);
            for i in 0..8 {
                let a = (i as f32 * 45.0 + 22.5) * PI / 180.0;
                let (c, sn) = (cos(a), sin(a));
                let t = tw(2.4);
                s.stroke(&[(cx + c * 4.4 * k, cy + sn * 4.4 * k), (cx + c * 5.9 * k, cy + sn * 5.9 * k)], t, fg);
            }
        }
        Icon::Power => {
            let (cx, cy) = p(8.0, 8.8);
            s.arc(cx, cy, 4.8 * k, 125.0, 415.0, tw(1.7), fg);
            s.stroke(&[p(8.0, 2.4), p(8.0, 8.0)], tw(1.7), fg);
        }
        Icon::Logo => {
            let (cx, cy) = p(8.0, 8.0);
            let (ox, oy) = p(11.2, 4.8);
            s.crescent(cx, cy, 7.2 * k, ox, oy, 5.9 * k, fg);
        }
        Icon::Info => {
            let (cx, cy) = p(8.0, 8.0);
            s.arc(cx, cy, 6.3 * k, 0.0, 360.0, tw(1.5), fg);
            let (dx, dy) = p(8.0, 4.9);
            s.fill_circle(dx, dy, (0.95 * k).max(0.9), fg, 255);
            s.stroke(&[p(8.0, 7.3), p(8.0, 11.6)], tw(1.8), fg);
        }
        Icon::Palette => {
            // Paleta de pintor: disco beige con agujero del pulgar y cuatro gotas de color.
            let (cx, cy) = p(8.0, 8.0);
            s.fill_circle(cx, cy, 7.0 * k, rgb(0xE6, 0xCF, 0xA3), 255);
            let (hx, hy) = p(11.0, 11.6);
            s.fill_circle(hx, hy, 1.5 * k, rgb(0x7A, 0x66, 0x44), 255);
            let dots = [(4.6, 7.2, 0xE0_4A_4A), (6.6, 3.9, 0xF2_B0_1E), (10.4, 4.3, 0x2F_7B_E0), (4.6, 10.8, 0x3C_B3_71)];
            for (u, v, c) in dots {
                let (dx, dy) = p(u, v);
                s.fill_circle(dx, dy, (1.45 * k).max(1.2), c, 255);
            }
        }
    }
    s.set_clip(old);
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Icon; 8] =
        [Icon::Notepad, Icon::Calc, Icon::Terminal, Icon::Settings, Icon::Power, Icon::Logo, Icon::Info, Icon::Palette];
    const SENT: u32 = 0x123456;

    #[test]
    fn draws_inside_its_square_only() {
        for icon in ALL {
            for size in [16, 24, 32, 40] {
                let mut s = Surface::new(size + 20, size + 20, SENT);
                draw(&mut s, icon, 10, 10, size, 0xFFFFFF);
                let (mut dentro, mut fuera) = (false, false);
                for j in 0..s.h {
                    for i in 0..s.w {
                        let changed = s.px[(j * s.w + i) as usize] != SENT;
                        if (10..10 + size).contains(&i) && (10..10 + size).contains(&j) {
                            dentro |= changed;
                        } else {
                            fuera |= changed;
                        }
                    }
                }
                assert!(dentro, "{icon:?} {size}: no dibuja nada");
                assert!(!fuera, "{icon:?} {size}: escribe fuera");
            }
        }
    }

    #[test]
    fn edges_do_not_panic() {
        let mut s = Surface::new(10, 10, 0);
        for icon in ALL {
            draw(&mut s, icon, -8, -8, 16, 0xFFFFFF);
            draw(&mut s, icon, 5, 5, 40, 0xFFFFFF);
            draw(&mut s, icon, 0, 0, 0, 0xFFFFFF);
        }
    }
}
