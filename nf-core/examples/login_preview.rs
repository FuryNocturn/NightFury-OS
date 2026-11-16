//! Vista previa de las pantallas de cuentas: `cargo run -p nf-core --example login_preview -- carpeta`
//! Guarda BMP (1280x800) del asistente, de su segunda tarjeta, del inicio de sesión y del bloqueo.
use nf_core::apps::AppKind;
use nf_core::block::RamDisk;
use nf_core::desktop::Desktop;
use nf_core::files::Files;
use nf_core::input::{Event, Key, KeyEvent};
use nf_core::platform::{Platform, TestPlatform};

fn press(d: &mut Desktop, sys: &mut TestPlatform, key: Key, ch: Option<char>) {
    d.event(Event::Key(KeyEvent { key, ch, down: true, shift: false, ctrl: false, alt: false }), sys);
}

fn typ(d: &mut Desktop, sys: &mut TestPlatform, text: &str) {
    for c in text.chars() {
        press(d, sys, Key::Char(c.to_ascii_lowercase()), Some(c));
    }
}

fn guarda(dir: &str, nombre: &str, d: &mut Desktop, sys: &mut TestPlatform) {
    d.invalidate();
    d.render(sys);
    std::fs::write(format!("{dir}/{nombre}.bmp"), nf_core::bmp::encode(d.back())).unwrap();
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| String::from("login_preview"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut sys = TestPlatform { ms: 1000, size: (1280, 800), ..Default::default() };
    let now = sys.now();
    sys.files = Files::probe(Some(Box::new(RamDisk::new(4096))), now);
    sys.files.format_pending(now).unwrap();
    sys.files.logout();
    let mut d = Desktop::new(1280, 800, &mut sys);
    d.event(Event::Tick(10), &mut sys);
    // Asistente: tarjeta 1 con un error, y tarjeta 2 con una cuenta creada.
    typ(&mut d, &mut sys, "a|b");
    press(&mut d, &mut sys, Key::Enter, None);
    guarda(&dir, "1_asistente", &mut d, &mut sys);
    for _ in 0..3 {
        press(&mut d, &mut sys, Key::Backspace, None);
    }
    typ(&mut d, &mut sys, "Ana");
    press(&mut d, &mut sys, Key::Enter, None);
    typ(&mut d, &mut sys, "Luis");
    press(&mut d, &mut sys, Key::Tab, None);
    typ(&mut d, &mut sys, "12345");
    press(&mut d, &mut sys, Key::Enter, None);
    guarda(&dir, "2_asistente_otra", &mut d, &mut sys);
    // Cuentas: Ana (administradora) sin clave y Luis con clave.
    let mut sys2 = TestPlatform { ms: 1000, size: (1280, 800), ..Default::default() };
    sys2.files = Files::probe(Some(Box::new(RamDisk::new(4096))), now);
    sys2.files.format_pending(now).unwrap();
    sys2.files.create_account("Ana", None, true, [1; 16], now).unwrap();
    sys2.files.login("Ana", "").unwrap();
    sys2.files.create_account("Luis", Some("secreto1"), false, [2; 16], now).unwrap();
    sys2.files.logout();
    let mut d = Desktop::new(1280, 800, &mut sys2);
    d.event(Event::Tick(10), &mut sys2);
    guarda(&dir, "3_inicio_sin_clave", &mut d, &mut sys2);
    press(&mut d, &mut sys2, Key::Right, None);
    typ(&mut d, &mut sys2, "mala12");
    press(&mut d, &mut sys2, Key::Enter, None);
    typ(&mut d, &mut sys2, "abc");
    guarda(&dir, "4_inicio_con_clave", &mut d, &mut sys2);
    // Bloqueo con una ventana abierta debajo.
    typ(&mut d, &mut sys2, "def");
    press(&mut d, &mut sys2, Key::Backspace, None);
    press(&mut d, &mut sys2, Key::Backspace, None);
    press(&mut d, &mut sys2, Key::Backspace, None);
    press(&mut d, &mut sys2, Key::Backspace, None);
    press(&mut d, &mut sys2, Key::Backspace, None);
    press(&mut d, &mut sys2, Key::Backspace, None);
    typ(&mut d, &mut sys2, "secreto1");
    press(&mut d, &mut sys2, Key::Enter, None);
    d.launch(AppKind::Files, &mut sys2);
    d.lock(&mut sys2);
    guarda(&dir, "5_bloqueo", &mut d, &mut sys2);
}
