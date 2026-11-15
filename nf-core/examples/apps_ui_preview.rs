//! Vista previa de Calculadora y Configuración:
//! `cargo run --release -p nf-core --example apps_ui_preview -- carpeta`
//! Guarda BMP de varias situaciones (tamaño por defecto y mínimo).
use nf_core::apps::{self, App, AppKind, Ctx, MouseEv};
use nf_core::input::{Key, KeyEvent};
use nf_core::platform::TestPlatform;
use nf_core::surface::Surface;
use nf_core::theme::{Theme, ACCENTS};

struct Banco {
    app: Box<dyn App>,
    sys: TestPlatform,
    theme: Theme,
}

impl Banco {
    fn new(kind: AppKind) -> Self {
        let sys = TestPlatform { ms: 3 * 3_600_000 + 25 * 60_000, size: (1280, 800), ..Default::default() };
        Banco { app: apps::create(kind), sys, theme: Theme::default() }
    }

    fn dibuja(&mut self, w: i32, h: i32) -> Surface {
        let mut s = Surface::new(w, h, 0);
        self.app.draw(&mut s, &self.theme, &mut self.sys, true);
        s
    }

    fn ratón(&mut self, ev: MouseEv) {
        let mut cx = Ctx { sys: &mut self.sys, theme: &mut self.theme, now_ms: 0, redraw: false, close: false, theme_changed: false };
        self.app.mouse(ev, &mut cx);
    }

    fn clic(&mut self, x: i32, y: i32) {
        self.ratón(MouseEv::Down { x, y });
        self.ratón(MouseEv::Up { x, y });
    }

    fn tecla(&mut self, c: char) {
        let mut cx = Ctx { sys: &mut self.sys, theme: &mut self.theme, now_ms: 0, redraw: false, close: false, theme_changed: false };
        let key = if c == '\n' { Key::Enter } else { Key::Char(c) };
        let ch = if c == '\n' { None } else { Some(c) };
        self.app.key(&KeyEvent { key, ch, down: true, shift: false, ctrl: false, alt: false }, &mut cx);
    }
}

/// Centro del botón (fila, columna) de la rejilla de la calculadora para una superficie w x h.
fn boton(w: i32, h: i32, fila: i32, col: i32) -> (i32, i32) {
    let disp = (h * 17 / 100).clamp(72, 96);
    let (gw, gh) = (w - 8, h - disp - 4);
    let x0 = 4 + col * (gw + 2) / 4;
    let x1 = 4 + (col + 1) * (gw + 2) / 4 - 2;
    let y0 = disp + fila * (gh + 2) / 6;
    let y1 = disp + (fila + 1) * (gh + 2) / 6 - 2;
    ((x0 + x1) / 2, (y0 + y1) / 2)
}

fn guarda(dir: &str, nombre: &str, s: &Surface) {
    std::fs::write(format!("{dir}/{nombre}.bmp"), nf_core::bmp::encode(s)).unwrap();
}

fn main() {
    let dir = std::env::args().nth(1).expect("uso: apps_ui_preview <carpeta>");
    std::fs::create_dir_all(&dir).unwrap();

    for (nombre, (w, h)) in [("def", apps::default_size(AppKind::Calc)), ("min", (240, 340))] {
        // Operación a medias con el ratón sobre un botón (el 8).
        let mut b = Banco::new(AppKind::Calc);
        b.dibuja(w, h);
        for c in "12+3".chars() {
            b.tecla(c);
        }
        let (x, y) = boton(w, h, 2, 1);
        b.ratón(MouseEv::Move { x, y });
        guarda(&dir, &format!("calc_{nombre}_operacion"), &b.dibuja(w, h));
        // Pulsado (sin soltar) el botón "=".
        let (x, y) = boton(w, h, 5, 3);
        b.ratón(MouseEv::Down { x, y });
        guarda(&dir, &format!("calc_{nombre}_pulsado"), &b.dibuja(w, h));
        b.ratón(MouseEv::Up { x, y });
        guarda(&dir, &format!("calc_{nombre}_resultado"), &b.dibuja(w, h));
        // Número largo y negativo.
        let mut b = Banco::new(AppKind::Calc);
        b.dibuja(w, h);
        for c in "9999999999999999*9=".chars() {
            b.tecla(c);
        }
        guarda(&dir, &format!("calc_{nombre}_cientifica"), &b.dibuja(w, h));
        // Error.
        let mut b = Banco::new(AppKind::Calc);
        b.dibuja(w, h);
        for c in "5/0=".chars() {
            b.tecla(c);
        }
        guarda(&dir, &format!("calc_{nombre}_error"), &b.dibuja(w, h));
    }

    for (nombre, (w, h)) in [("def", apps::default_size(AppKind::Settings)), ("min", (520, 360))] {
        let mut b = Banco::new(AppKind::Settings);
        guarda(&dir, &format!("conf_{nombre}_personalizacion"), &b.dibuja(w, h));
        // Pasa el ratón sobre "Acerca de" y entra.
        b.dibuja(w, h);
        b.ratón(MouseEv::Move { x: 60, y: 56 + 40 + 10 });
        guarda(&dir, &format!("conf_{nombre}_hover"), &b.dibuja(w, h));
        b.clic(60, 56 + 40 + 10);
        guarda(&dir, &format!("conf_{nombre}_acerca"), &b.dibuja(w, h));
        // Vuelve, elige otro fondo y otro color: se busca el color en la imagen para pulsar encima.
        b.clic(60, 56 + 10);
        let s = b.dibuja(w, h);
        let busca = |s: &Surface, c: u32| (0..s.h).flat_map(|y| (220..s.w).map(move |x| (x, y))).find(|&(x, y)| s.px[(y * s.w + x) as usize] == c);
        let (x, y) = busca(&s, ACCENTS[6]).expect("muestra");
        b.clic(x + 3, y + 3);
        // Miniatura del fondo 2: segunda de las que caben; se pulsa en la rejilla conocida.
        let y_miniaturas = if h < 440 { 14 + 30 + 4 + 18 + 4 } else { 24 + 30 + 10 + 18 + 4 };
        let per = ((w - 244 - if h < 440 { 12 } else { 24 }) + 8) / 136;
        let (col, fila) = (2 % per, 2 / per);
        let row_h = 72 + 2 + 16 + if h < 440 { 4 } else { 8 };
        b.clic(244 + col * 136 + 20, y_miniaturas + fila * row_h + 20);
        guarda(&dir, &format!("conf_{nombre}_eleccion"), &b.dibuja(w, h));
        eprintln!("{nombre}: fondo {} acento {:06X}", b.theme.wallpaper, b.theme.accent);
    }
}
