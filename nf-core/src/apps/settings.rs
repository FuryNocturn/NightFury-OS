//! Configuración: personalización (fondo y color de énfasis) y datos del sistema.
//! CONTRATO: `pub fn new() -> Box<dyn App>`.
use super::{App, AppKind, Ctx, MouseEv};
use crate::font::{self, Align};
use crate::geom::Rect;
use crate::icons::{self, Icon};
use crate::input::KeyEvent;
use crate::platform::Platform;
use crate::surface::Surface;
use crate::theme::{render_wallpaper, Theme, ACCENTS, WALLPAPERS};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const PANEL_W: i32 = 220;
const THUMB: (i32, i32) = (128, 72);
const TEXT: u32 = 0x1A1A1A;
const GRAY: u32 = 0x6A6A6A;
const SECTIONS: [(&str, Icon); 2] = [("Personalización", Icon::Palette), ("Acerca de", Icon::Info)];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hit {
    Nav(usize),
    Wall(u8),
    Accent(usize),
}

struct Settings {
    page: usize,
    thumbs: Vec<Surface>,
    /// Color de énfasis con el que se generó la miniatura de "Color sólido".
    thumb_accent: u32,
    hover: Option<usize>,
    /// Zonas pulsables de lo dibujado por última vez.
    hits: Vec<(Rect, Hit)>,
    /// Segundo (de `now_ms`) de la última vez que se redibujó "Acerca de".
    last_sec: u64,
}

pub fn new() -> Box<dyn App> {
    Box::new(Settings { page: 0, thumbs: Vec::new(), thumb_accent: 0, hover: None, hits: Vec::new(), last_sec: 0 })
}

fn nav_rect(i: usize) -> Rect {
    Rect::new(0, 56 + 40 * i as i32, PANEL_W, 40)
}

impl Settings {
    fn panel(&mut self, s: &mut Surface, theme: &Theme) {
        s.fill_rect(Rect::new(0, 0, PANEL_W, s.h), 0xF2F2F2);
        font::draw(s, 16, 18, "Configuración", 12, true, TEXT);
        for (i, (name, icon)) in SECTIONS.iter().enumerate() {
            let r = nav_rect(i);
            self.hits.push((r, Hit::Nav(i)));
            if self.page == i {
                s.fill_rect(r, 0xE5E5E5);
                s.fill_rect(Rect::new(0, r.y + 8, 3, r.h - 16), theme.accent);
            } else if self.hover == Some(i) {
                s.fill_rect(r, 0xEAEAEA);
            }
            icons::draw(s, *icon, 16, r.y + 10, 20, 0x444444);
            font::draw_in(s, Rect::new(48, r.y, PANEL_W - 48, r.h), name, 9, false, TEXT, Align::Left);
        }
    }

    fn personalization(&mut self, s: &mut Surface, theme: &Theme) {
        let compact = s.h < 440;
        let x0 = PANEL_W + 24;
        let avail = (s.w - x0 - if compact { 12 } else { 24 }).max(1);
        let mut y = if compact { 14 } else { 24 };
        font::draw(s, x0, y, "Personalización", 18, false, TEXT);
        y += font::line_h(18) + if compact { 4 } else { 10 };
        font::draw(s, x0, y, "Fondo", 11, true, TEXT);
        y += font::line_h(11) + 4;
        // Miniaturas de los fondos.
        let (tw, th) = THUMB;
        let per = ((avail + 8) / (tw + 8)).max(1);
        let row_h = th + 2 + font::line_h(9) + if compact { 4 } else { 8 };
        for (i, name) in WALLPAPERS.iter().enumerate() {
            let (col, row) = (i as i32 % per, i as i32 / per);
            let r = Rect::new(x0 + col * (tw + 8), y + row * row_h, tw, th);
            s.blit(&self.thumbs[i], r.x, r.y);
            let sel = theme.wallpaper as usize == i;
            if sel {
                s.frame(r.inflate(2), theme.accent);
                s.frame(r.inflate(1), theme.accent);
            }
            font::draw(s, r.x, r.bottom() + 2, name, 9, sel, TEXT);
            self.hits.push((r, Hit::Wall(i as u8)));
        }
        y += ((WALLPAPERS.len() as i32 + per - 1) / per) * row_h + if compact { 0 } else { 4 };
        font::draw(s, x0, y, "Color de énfasis", 11, true, TEXT);
        y += font::line_h(11) + 6;
        // Muestras de color.
        let (sz, gap) = if compact { (30, 6) } else { (40, 6) };
        let per = ((avail + gap) / (sz + gap)).max(1);
        for (i, &c) in ACCENTS.iter().enumerate() {
            let (col, row) = (i as i32 % per, i as i32 / per);
            let r = Rect::new(x0 + col * (sz + gap), y + row * (sz + gap), sz, sz);
            s.fill_rect(r, c);
            if c == theme.accent {
                s.frame(r.inflate(2), 0x333333);
                s.frame(r.inflate(1), 0x333333);
                font::draw_in(s, r, "✓", 14, true, 0xFFFFFF, Align::Center);
            }
            self.hits.push((r, Hit::Accent(i)));
        }
    }

    fn about(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform) {
        let compact = s.h < 440;
        let x0 = PANEL_W + 24;
        let mut y = if compact { 14 } else { 24 };
        font::draw(s, x0, y, "Acerca de", 18, false, TEXT);
        y += font::line_h(18) + if compact { 8 } else { 16 };
        icons::draw(s, Icon::Logo, x0, y, 48, theme.accent);
        let name = format!("{} {}", crate::NAME, crate::VERSION);
        font::draw_in(s, Rect::new(x0 + 60, y, (s.w - x0 - 60).max(0), 48), &name, 20, true, TEXT, Align::Left);
        y += 48 + if compact { 12 } else { 20 };
        let mem = sys.mem();
        let mb = |b: u64| b / (1 << 20);
        let up = sys.uptime_ms() / 60_000;
        let (sw, sh) = sys.screen();
        let (total, free) = sys.files().space();
        let disk = format!(
            "{} libres de {}{}",
            super::terminal::units(free),
            super::terminal::units(total),
            if sys.files().persistent() { "" } else { " (en memoria)" }
        );
        let rows = [
            ("Procesador", sys.cpu()),
            ("Memoria instalada", format!("{} MB", mb(mem.total))),
            ("Memoria del sistema", format!("{} / {} MB", mb(mem.heap_used), mb(mem.heap_total))),
            ("Pantalla", format!("{} x {}", sw, sh)),
            ("Disco C:", disk),
            ("Tipo de sistema", String::from("64 bits, arranque UEFI")),
            ("Tiempo encendido", format!("{} h {} min", up / 60, up % 60)),
        ];
        let row_h = if compact { 22 } else { 28 };
        for (label, value) in rows.iter() {
            font::draw_in(s, Rect::new(x0, y, 170, row_h), label, 9, false, GRAY, Align::Left);
            font::draw_in(s, Rect::new(x0 + 170, y, (s.w - x0 - 170).max(0), row_h), value, 9, false, TEXT, Align::Left);
            y += row_h;
        }
        y += if compact { 6 } else { 14 };
        font::draw(s, x0, y, "Sistema operativo escrito desde cero en Rust.", 9, false, GRAY);
    }
}

impl App for Settings {
    fn kind(&self) -> AppKind {
        AppKind::Settings
    }
    fn title(&self) -> String {
        String::from("Configuración")
    }
    fn min_size(&self) -> (i32, i32) {
        (520, 360)
    }

    fn draw(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, _focused: bool) {
        // Las miniaturas se generan una vez; la de "Color sólido" se rehace si cambia el énfasis.
        if self.thumbs.is_empty() {
            self.thumbs = (0..WALLPAPERS.len() as u8).map(|i| render_wallpaper(i, THUMB.0, THUMB.1, theme.accent)).collect();
            self.thumb_accent = theme.accent;
        } else if self.thumb_accent != theme.accent {
            self.thumbs[3] = render_wallpaper(3, THUMB.0, THUMB.1, theme.accent);
            self.thumb_accent = theme.accent;
        }
        let b = s.bounds();
        s.fill_rect(b, 0xFFFFFF);
        self.hits.clear();
        self.panel(s, theme);
        // Lo de la derecha no se sale de su zona.
        s.set_clip(Rect::new(PANEL_W, 0, (s.w - PANEL_W).max(0), s.h));
        if self.page == 0 {
            self.personalization(s, theme);
        } else {
            self.about(s, theme, sys);
        }
        s.reset_clip();
    }

    fn key(&mut self, _ev: &KeyEvent, _cx: &mut Ctx) {}

    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
        let at = |s: &Self, x: i32, y: i32| s.hits.iter().find(|(r, _)| r.contains(x, y)).map(|h| h.1);
        match ev {
            MouseEv::Move { x, y } => {
                let h = match at(self, x, y) {
                    Some(Hit::Nav(i)) => Some(i),
                    _ => None,
                };
                if h != self.hover {
                    self.hover = h;
                    cx.redraw = true;
                }
            }
            MouseEv::Leave => {
                if self.hover.take().is_some() {
                    cx.redraw = true;
                }
            }
            MouseEv::Up { .. } => {}
            MouseEv::Down { x, y } => match at(self, x, y) {
                Some(Hit::Nav(i)) if i != self.page => {
                    self.page = i;
                    self.hits.clear();
                    cx.redraw = true;
                }
                Some(Hit::Wall(i)) if cx.theme.wallpaper != i => {
                    cx.theme.wallpaper = i;
                    cx.theme_changed = true;
                    cx.redraw = true;
                }
                Some(Hit::Accent(i)) if cx.theme.accent != ACCENTS[i] => {
                    cx.theme.accent = ACCENTS[i];
                    cx.theme_changed = true;
                    cx.redraw = true;
                }
                _ => {}
            },
        }
    }

    fn tick(&mut self, cx: &mut Ctx) {
        // "Tiempo encendido" y memoria cambian solos: redibuja una vez por segundo.
        let sec = cx.now_ms / 1000;
        if self.page == 1 && sec != self.last_sec {
            self.last_sec = sec;
            cx.redraw = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::TestPlatform;

    struct Banco {
        app: Box<dyn App>,
        sys: TestPlatform,
        theme: Theme,
        redraw: bool,
        changed: bool,
    }

    impl Banco {
        fn new() -> Self {
            Banco { app: new(), sys: TestPlatform { size: (1280, 800), ..Default::default() }, theme: Theme::default(), redraw: false, changed: false }
        }
        fn dibuja(&mut self, w: i32, h: i32) -> Surface {
            let mut s = Surface::new(w, h, 0);
            self.app.draw(&mut s, &self.theme, &mut self.sys, true);
            s
        }
        fn ev(&mut self, ev: MouseEv) {
            let mut cx = Ctx::new(&mut self.sys, &mut self.theme, 0);
            self.app.mouse(ev, &mut cx);
            self.redraw |= cx.redraw;
            self.changed |= cx.theme_changed;
        }
        fn clic(&mut self, x: i32, y: i32) {
            self.redraw = false;
            self.changed = false;
            self.ev(MouseEv::Down { x, y });
            self.ev(MouseEv::Up { x, y });
        }
    }

    #[test]
    fn muestra_cambia_acento() {
        let mut b = Banco::new();
        b.dibuja(780, 520);
        // Posición de la muestra 5: buscada por el dibujo (color único en la zona).
        let s = b.dibuja(780, 520);
        let (x, y) = (0..s.h).flat_map(|y| (PANEL_W..s.w).map(move |x| (x, y))).find(|&(x, y)| s.px[(y * s.w + x) as usize] == ACCENTS[5]).unwrap();
        b.clic(x + 2, y + 2);
        assert_eq!(b.theme.accent, ACCENTS[5]);
        assert!(b.changed && b.redraw);
    }

    #[test]
    fn miniatura_cambia_fondo() {
        let mut b = Banco::new();
        b.dibuja(780, 520);
        // Miniatura del fondo 1: en la primera fila, segunda columna.
        b.clic(PANEL_W + 24 + 136 + 10, 24 + font::line_h(18) + 10 + font::line_h(11) + 4 + 10);
        assert_eq!(b.theme.wallpaper, 1);
        assert!(b.changed);
    }

    #[test]
    fn cambio_de_seccion() {
        let mut b = Banco::new();
        b.dibuja(780, 520);
        b.ev(MouseEv::Move { x: 50, y: 56 + 40 + 5 });
        assert!(b.redraw);
        b.clic(50, 56 + 40 + 5);
        assert!(b.redraw && !b.changed);
        let s = b.dibuja(780, 520);
        // En "Acerca de" ya no hay muestras de color en la zona de contenido.
        assert!(!s.px.iter().any(|&p| p == ACCENTS[6]));
        b.clic(50, 56 + 5);
        let s = b.dibuja(780, 520);
        assert!(s.px.iter().any(|&p| p == ACCENTS[6]));
    }

    #[test]
    fn tick_solo_en_acerca_de() {
        let mut b = Banco::new();
        b.dibuja(780, 520);
        let mut cx = Ctx::new(&mut b.sys, &mut b.theme, 5000);
        b.app.tick(&mut cx);
        assert!(!cx.redraw);
        drop(cx);
        b.clic(50, 56 + 40 + 5);
        b.dibuja(780, 520);
        let mut cx = Ctx::new(&mut b.sys, &mut b.theme, 6000);
        b.app.tick(&mut cx);
        assert!(cx.redraw);
        cx.redraw = false;
        b.app.tick(&mut cx);
        assert!(!cx.redraw);
    }

    #[test]
    fn tamanos_extremos() {
        let mut b = Banco::new();
        for pagina in 0..2 {
            b.clic(50, 56 + 40 * pagina + 5);
            for (w, h) in [(520, 360), (780, 520), (1, 1), (0, 0), (221, 400), (600, 1)] {
                b.dibuja(w, h);
            }
            b.ev(MouseEv::Leave);
        }
        // Color sólido se regenera con el nuevo énfasis sin pánico.
        b.theme.accent = ACCENTS[3];
        b.dibuja(520, 360);
    }
}
