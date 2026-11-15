//! Vista previa del Bloc de notas y el Terminal:
//! `cargo run --release -p nf-core --example apps_text_preview -- carpeta`
//! Escribe un BMP por app y tamaño (por defecto y mínimo) en la carpeta indicada.
use nf_core::apps::{self, App, AppKind, Ctx};
use nf_core::bmp;
use nf_core::input::{Key, KeyEvent};
use nf_core::platform::TestPlatform;
use nf_core::surface::Surface;
use nf_core::theme::Theme;

fn tecla(key: Key, ch: Option<char>) -> KeyEvent {
    KeyEvent { key, ch, down: true, shift: false, ctrl: false, alt: false }
}

fn escribe(app: &mut dyn App, texto: &str, cx: &mut Ctx) {
    for c in texto.chars() {
        let ev = match c {
            '\n' => tecla(Key::Enter, Some('\n')),
            '\t' => tecla(Key::Tab, None),
            _ => tecla(Key::Char(c), Some(c)),
        };
        app.key(&ev, cx);
    }
}

fn guarda(dir: &str, nombre: &str, app: &mut dyn App, w: i32, h: i32, sys: &mut TestPlatform) {
    let mut s = Surface::new(w, h, 0);
    app.draw(&mut s, &Theme::default(), sys, true);
    let ruta = format!("{}/{}.bmp", dir, nombre);
    std::fs::write(&ruta, bmp::encode(&s)).expect("no se pudo escribir el BMP");
    println!("{}", ruta);
}

fn main() {
    let dir = std::env::args().nth(1).expect("uso: apps_text_preview <carpeta>");
    std::fs::create_dir_all(&dir).unwrap();
    let mut sys = TestPlatform { ms: 3_723_000, size: (1280, 720), ..Default::default() };
    let mut th = Theme::default();

    let mut nota = apps::create(AppKind::Notepad);
    let mut term = apps::create(AppKind::Terminal);
    {
        let mut cx = Ctx { sys: &mut sys, theme: &mut th, now_ms: 0, redraw: false, close: false, theme_changed: false };
        escribe(
            &mut *nota,
            "Canción de otoño\nEl veloz murciélago hindú comía feliz cardillo y kiwi.\n¿Qué tal, ñandú? ¡Ñu, niño, año, pingüino!\n\tCon tabulador y acentos: áéíóú ÁÉÍÓÚ ü\nUna línea muy larga para comprobar el desplazamiento horizontal del cursor, que debe seguir visible sin cortarse.",
            &mut cx,
        );
        for o in ["ayuda", "info", "memoria", "tiempo", "eco Hola, mundo ñ", "patata"] {
            escribe(&mut *term, &format!("{o}\n"), &mut cx);
        }
    }
    // Cursor al final de una línea larga: comprueba el desplazamiento horizontal.
    let (w, h) = apps::default_size(AppKind::Notepad);
    guarda(&dir, "notepad_desplazado", &mut *nota, w, h, &mut sys);
    {
        let mut cx = Ctx { sys: &mut sys, theme: &mut th, now_ms: 0, redraw: false, close: false, theme_changed: false };
        for _ in 0..4 {
            nota.key(&tecla(Key::Up, None), &mut cx);
        }
        nota.key(&tecla(Key::Home, None), &mut cx);
    }
    for (kind, nombre) in [(AppKind::Notepad, "notepad"), (AppKind::Terminal, "terminal")] {
        let app: &mut dyn App = if kind == AppKind::Notepad { &mut *nota } else { &mut *term };
        let (w, h) = apps::default_size(kind);
        guarda(&dir, &format!("{}_grande", nombre), app, w, h, &mut sys);
        let (mw, mh) = app.min_size();
        guarda(&dir, &format!("{}_pequeno", nombre), app, mw, mh, &mut sys);
    }
}
