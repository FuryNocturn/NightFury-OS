//! Vista previa de Calculadora y Configuración:
//! `cargo run --release -p nf-core --example apps_ui_preview -- carpeta`
//! Guarda BMP de varias situaciones (tamaño por defecto y mínimo).
use nf_core::apps::{self, App, AppKind, Ctx, MouseEv};
use nf_core::block::RamDisk;
use nf_core::files::Files;
use nf_core::input::{Key, KeyEvent};
use nf_core::keys::{Pool, POOL_EVENTS_NEEDED};
use nf_core::platform::{DateTime, TestPlatform};
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
        let mut cx = Ctx::new(&mut self.sys, &mut self.theme, 0);
        self.app.mouse(ev, &mut cx);
    }

    fn clic(&mut self, x: i32, y: i32) {
        self.ratón(MouseEv::Down { x, y });
        self.ratón(MouseEv::Up { x, y });
    }

    fn tecla(&mut self, c: char) {
        let mut cx = Ctx::new(&mut self.sys, &mut self.theme, 0);
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
        b.ratón(MouseEv::Move { x: 60, y: 56 + 120 + 10 });
        guarda(&dir, &format!("conf_{nombre}_hover"), &b.dibuja(w, h));
        b.clic(60, 56 + 120 + 10);
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

    // Cuentas: sin disco (aviso), como administradora con dos cuentas más y como usuario normal.
    for (nombre, (w, h)) in [("def", apps::default_size(AppKind::Settings)), ("min", (520, 360))] {
        let mut b = Banco::new(AppKind::Settings);
        b.dibuja(w, h);
        b.clic(60, 56 + 40 + 10);
        guarda(&dir, &format!("conf_{nombre}_cuentas_sin_disco"), &b.dibuja(w, h));
        for quien in ["Ana", "Luis"] {
            let mut b = Banco::new(AppKind::Settings);
            b.sys.files = con_cuentas(quien);
            b.dibuja(w, h);
            b.clic(60, 56 + 40 + 10);
            guarda(&dir, &format!("conf_{nombre}_cuentas_{}", quien.to_lowercase()), &b.dibuja(w, h));
        }
    }

    // Disco: administradora (sin cifrar y cifrando al 37 %), usuario normal y sin disco.
    for (nombre, (w, h)) in [("def", apps::default_size(AppKind::Settings)), ("min", (520, 360))] {
        let mut b = Banco::new(AppKind::Settings);
        b.dibuja(w, h);
        b.clic(60, 56 + 80 + 10);
        guarda(&dir, &format!("conf_{nombre}_disco_sin_disco"), &b.dibuja(w, h));
        for quien in ["Ana", "Luis"] {
            let mut b = Banco::new(AppKind::Settings);
            b.sys.files = con_cuentas(quien);
            b.dibuja(w, h);
            b.clic(60, 56 + 80 + 10);
            guarda(&dir, &format!("conf_{nombre}_disco_{}", quien.to_lowercase()), &b.dibuja(w, h));
        }
        let mut b = Banco::new(AppKind::Settings);
        b.sys.files = con_cuentas("Ana");
        cifrando(&mut b.sys.files, 37);
        b.dibuja(w, h);
        b.clic(60, 56 + 80 + 10);
        guarda(&dir, &format!("conf_{nombre}_disco_cifrando"), &b.dibuja(w, h));
        // Tras terminar la conversión: los tres botones de un disco cifrado.
        while !b.sys.files.convert_step(500, DateTime::default()).unwrap() {}
        guarda(&dir, &format!("conf_{nombre}_disco_cifrado"), &b.dibuja(w, h));
    }
}

/// Activa el cifrado total y avanza la conversión hasta el `porcentaje` indicado.
fn cifrando(f: &mut Files, porcentaje: u64) {
    let mut pool = Pool::new();
    for i in 0..POOL_EVENTS_NEEDED as i32 {
        pool.add_event(i, i * 5, i as u64, i as u64 * 11);
    }
    f.encrypt_disk("clave-del-disco-9!", &mut pool, DateTime::default()).unwrap();
    let (_, total, _) = f.disk_conversion().unwrap();
    f.convert_step((total * porcentaje).div_ceil(100) as u32, DateTime::default()).unwrap();
}

/// Disco en memoria con la administradora Ana y el usuario normal Luis (más la normal Marta);
/// la sesión es la de `quien`.
fn con_cuentas(quien: &str) -> Files {
    let n = DateTime::default();
    let mut f = Files::probe(Some(Box::new(RamDisk::new(2048))), n);
    f.format_pending(n).unwrap();
    f.create_account("Ana", Some("secreta1"), true, [1; 16], n).unwrap();
    f.login("Ana", "secreta1").unwrap();
    f.create_account("Luis", None, false, [2; 16], n).unwrap();
    f.create_account("Marta", Some("clavemarta"), true, [3; 16], n).unwrap();
    f.logout();
    f.login(quien, if quien == "Ana" { "secreta1" } else { "" }).unwrap();
    f
}
