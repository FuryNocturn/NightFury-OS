//! Escritorio y gestor de ventanas. CONTRATO: las firmas públicas de `Desktop` no cambian
//! (las usa el núcleo). Esta versión es provisional: solo fondo y puntero.
use crate::geom::Rect;
use crate::input::Event;
use crate::platform::Platform;
use crate::surface::Surface;
use crate::theme::{self, Theme};
use alloc::vec;
use alloc::vec::Vec;

pub struct Desktop {
    w: i32,
    h: i32,
    back: Surface,
    wall: Surface,
    theme: Theme,
    mx: i32,
    my: i32,
    damage: Vec<Rect>,
}

impl Desktop {
    pub fn new(w: i32, h: i32, _sys: &mut dyn Platform) -> Self {
        let theme = Theme::default();
        Desktop {
            w,
            h,
            back: Surface::new(w, h, 0),
            wall: theme::render_wallpaper(theme.wallpaper, w, h, theme.accent),
            theme,
            mx: w / 2,
            my: h / 2,
            damage: vec![Rect::new(0, 0, w, h)],
        }
    }

    /// Atiende un evento de entrada o de tiempo.
    pub fn event(&mut self, ev: Event, _sys: &mut dyn Platform) {
        if let Event::Mouse(p) = ev {
            self.damage.push(Rect::new(self.mx, self.my, 12, 19));
            self.mx = (self.mx + p.dx).clamp(0, self.w - 1);
            self.my = (self.my + p.dy).clamp(0, self.h - 1);
            self.damage.push(Rect::new(self.mx, self.my, 12, 19));
        }
    }

    /// Recompone en la imagen final las zonas que han cambiado y devuelve esas zonas
    /// (ya recortadas a la pantalla) para que el núcleo las copie al monitor.
    pub fn render(&mut self, _sys: &mut dyn Platform) -> Vec<Rect> {
        let _ = self.theme;
        let rects: Vec<Rect> =
            self.damage.drain(..).map(|r| r.intersect(Rect::new(0, 0, self.w, self.h))).filter(|r| !r.is_empty()).collect();
        for r in &rects {
            self.back.set_clip(*r);
            self.back.blit(&self.wall, 0, 0);
            self.back.fill_rect(Rect::new(self.mx, self.my, 12, 19), 0xFFFFFF);
        }
        self.back.reset_clip();
        rects
    }

    /// Imagen final de la pantalla (0x00RRGGBB, w x h).
    pub fn back(&self) -> &Surface {
        &self.back
    }
}
