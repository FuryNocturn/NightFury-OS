//! Vista previa del escritorio: simula escenas y guarda BMP en la carpeta indicada.
//! Uso: cargo run --release -p nf-core --example preview -- carpeta
use nf_core::apps::AppKind;
use nf_core::desktop::Desktop;
use nf_core::geom::Rect;
use nf_core::input::{Event, Key, KeyEvent};
use nf_core::platform::TestPlatform;

const W: i32 = 1280;
const H: i32 = 800;

fn center(r: Rect) -> (i32, i32) {
    (r.x + r.w / 2, r.y + r.h / 2)
}

struct Shots {
    dir: String,
    d: Desktop,
    sys: TestPlatform,
}

impl Shots {
    fn save(&mut self, name: &str) {
        self.d.render(&mut self.sys);
        let inc = self.d.back().px.clone();
        let path = format!("{}/{name}.bmp", self.dir);
        std::fs::write(&path, nf_core::bmp::encode(self.d.back())).unwrap();
        // Comprobación: el redibujado completo debe dar la misma imagen.
        self.d.invalidate();
        self.d.render(&mut self.sys);
        let ok = inc == self.d.back().px;
        eprintln!("{path}: incremental == completo: {ok}");
        assert!(ok);
    }
    fn mv(&mut self, x: i32, y: i32) {
        self.d.mouse_to(x, y, &mut self.sys);
        self.d.render(&mut self.sys);
    }
    fn click(&mut self, p: (i32, i32)) {
        self.mv(p.0, p.1);
        self.d.left_button(true, &mut self.sys);
        self.d.left_button(false, &mut self.sys);
    }
    fn key(&mut self, key: Key) {
        let ev = KeyEvent { key, ch: None, down: true, shift: false, ctrl: false, alt: false };
        self.d.event(Event::Key(ev), &mut self.sys);
    }
}

fn main() {
    let dir = std::env::args().nth(1).expect("falta la carpeta de salida");
    std::fs::create_dir_all(&dir).unwrap();
    let mut sys = TestPlatform { size: (W as u32, H as u32), ..Default::default() };
    let d = Desktop::new(W, H, &mut sys);
    let mut s = Shots { dir, d, sys };

    // 1. Escritorio vacío.
    s.mv(900, 400);
    s.save("1_vacio");

    // 2. Menú de inicio con el ratón sobre un mosaico.
    s.key(Key::Super);
    s.mv(450, 350);
    s.save("2_inicio");

    // 3. Menú de apagado.
    s.click((24, 800 - 40 - 24));
    s.mv(80, 800 - 40 - 48 - 20);
    s.save("3_apagado");
    s.key(Key::Escape);
    s.key(Key::Escape);

    // 4. Tres ventanas solapadas, ratón sobre cerrar de la activa.
    for k in [AppKind::Notepad, AppKind::Calc, AppKind::Terminal] {
        s.d.launch(k, &mut s.sys);
    }
    let r = *s.d.window_rects().last().unwrap();
    s.mv(r.right() - 24, r.y + 16);
    s.save("4_ventanas");

    // 5. Ventana maximizada (doble clic en la barra de título).
    let t = (r.x + 200, r.y + 10);
    s.click(t);
    s.click(t);
    s.mv(600, 300);
    s.save("5_maximizada");
    s.click((600, 10));
    s.click((600, 10));

    // 6. Arrastrar y redimensionar.
    let r = *s.d.window_rects().last().unwrap();
    s.mv(r.x + 200, r.y + 10);
    s.d.left_button(true, &mut s.sys);
    for i in 1..=10 {
        s.mv(r.x + 200 + i * 10, r.y + 10 + i * 6);
    }
    s.d.left_button(false, &mut s.sys);
    let r = *s.d.window_rects().last().unwrap();
    s.mv(r.right() - 2, r.bottom() - 2);
    s.d.left_button(true, &mut s.sys);
    for i in 1..=10 {
        s.mv(r.right() - 2 + i * 12, r.bottom() - 2 + i * 4);
    }
    s.d.left_button(false, &mut s.sys);
    s.save("6_arrastre");
    let _ = center(r);
}
