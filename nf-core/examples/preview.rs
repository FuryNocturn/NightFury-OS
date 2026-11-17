// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Vista previa del escritorio: simula escenas y guarda BMP en la carpeta indicada.
//! Uso: cargo run --release -p nf-core --example preview -- carpeta
use nf_core::apps::{App, AppKind, Ctx, MouseEv};
use nf_core::desktop::Desktop;
use nf_core::dialogs::{Buttons, DialogRequest};
use nf_core::files::{self, Std};
use nf_core::geom::Rect;
use nf_core::input::{Event, Key, KeyEvent};
use nf_core::menu::{MenuItem, MenuRequest};
use nf_core::platform::{Platform, TestPlatform};
use nf_core::surface::Surface;
use nf_core::theme::Theme;

/// App de muestra: el botón derecho abre un menú contextual de ejemplo.
struct MenuDemo;

impl App for MenuDemo {
    fn kind(&self) -> AppKind {
        AppKind::Files
    }
    fn title(&self) -> String {
        "Menú de ejemplo".into()
    }
    fn min_size(&self) -> (i32, i32) {
        (200, 200)
    }
    fn draw(&mut self, s: &mut Surface, _t: &Theme, _sys: &mut dyn Platform, _f: bool) {
        let b = s.bounds();
        s.fill_rect(b, 0xFFFFFF);
    }
    fn key(&mut self, _ev: &KeyEvent, _cx: &mut Ctx) {}
    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
        if let MouseEv::RightDown { x, y } = ev {
            let ver = vec![MenuItem::new(1, "Iconos grandes").checked(true), MenuItem::new(2, "Lista"), MenuItem::new(3, "Detalles")];
            let ord = vec![MenuItem::new(4, "Nombre"), MenuItem::new(5, "Tamaño"), MenuItem::new(6, "Tipo")];
            let nuevo = vec![MenuItem::new(7, "Carpeta"), MenuItem::new(8, "Documento de texto")];
            cx.menu = Some(MenuRequest {
                tag: 1,
                x,
                y,
                items: vec![
                    MenuItem::submenu("Ver", ver),
                    MenuItem::submenu("Ordenar por", ord),
                    MenuItem::new(9, "Actualizar"),
                    MenuItem::sep(),
                    MenuItem::new(10, "Pegar").disabled(true),
                    MenuItem::new(11, "Deshacer Cambiar nombre").shortcut("Ctrl+Z"),
                    MenuItem::sep(),
                    MenuItem::submenu("Nuevo", nuevo),
                    MenuItem::sep(),
                    MenuItem::new(12, "Personalizar").bold(),
                ],
            });
        }
    }
    fn tick(&mut self, _cx: &mut Ctx) {}
}

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
    let now = sys.now();
    let desk = sys.files.std_dir(Std::Desktop);
    sys.files.mkdir(&files::join(&desk, "Proyectos"), now).unwrap();
    sys.files.write(&files::join(&desk, "notas.txt"), b"hola", now).unwrap();
    sys.files.write(&files::join(&desk, "datos.bin"), b"1234", now).unwrap();
    let d = Desktop::new(W, H, &mut sys);
    let mut s = Shots { dir, d, sys };

    // 1. Escritorio vacío.
    s.mv(900, 400);
    s.save("1_vacio");

    // 1b. Menú del fondo con el submenú Ver abierto; 1c. iconos grandes; 1d. menú de un archivo.
    s.mv(700, 300);
    s.d.right_button(true, &mut s.sys);
    s.d.right_button(false, &mut s.sys);
    let it = s.d.menu().expect("menú del fondo").item_rect(0);
    s.mv(it.x + 20, it.y + 10);
    let sr = s.d.menu().and_then(|m| m.sub_item_rect(0)).expect("submenú Ver");
    s.mv(sr.x + 20, sr.y + 10);
    s.save("1b_menu_fondo");
    s.click((sr.x + 20, sr.y + 10));
    s.mv(900, 600);
    s.save("1c_iconos_grandes");
    s.mv(6 + 100 + 50, 6 + 40);
    s.d.right_button(true, &mut s.sys);
    s.d.right_button(false, &mut s.sys);
    let m = s.d.menu().expect("menú del archivo");
    let it = m.item_rect(1);
    s.mv(it.x + 20, it.y + 10);
    s.save("1d_menu_archivo");
    s.key(Key::Escape);
    // Vuelve a iconos medianos para el resto de escenas.
    s.mv(700, 300);
    s.d.right_button(true, &mut s.sys);
    s.d.right_button(false, &mut s.sys);
    let it = s.d.menu().unwrap().item_rect(0);
    s.mv(it.x + 20, it.y + 10);
    let sr = s.d.menu().and_then(|m| m.sub_item_rect(1)).unwrap();
    s.click((sr.x + 20, sr.y + 10));

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

    // 6b. Menú contextual con submenú abierto.
    s.d.launch_app(Box::new(MenuDemo), &mut s.sys);
    let r = *s.d.window_rects().last().unwrap();
    s.mv(r.x + 80, r.y + 80);
    s.d.right_button(true, &mut s.sys);
    s.d.right_button(false, &mut s.sys);
    let m = s.d.menu().expect("menú abierto");
    let it = m.item_rect(0);
    s.mv(it.x + 20, it.y + 10);
    let sr = s.d.menu().and_then(|m| m.sub_item_rect(1)).expect("submenú");
    s.mv(sr.x + 20, sr.y + 10);
    s.save("6b_menu_contextual");
    s.key(Key::Escape);

    // 7. Diálogo "Guardar como" con archivos y una carpeta, y un mensaje de pregunta.
    let now = s.sys.now();
    let dir = s.sys.files.std_dir(Std::Documents);
    s.sys.files.mkdir(&files::join(&dir, "Proyectos"), now).unwrap();
    s.sys.files.write(&files::join(&dir, "nota.txt"), b"hola", now).unwrap();
    s.sys.files.write(&files::join(&dir, "datos.bin"), b"1234", now).unwrap();
    s.d.request_dialog(DialogRequest::Save { tag: 1, title: "Guardar como".into(), dir, name: "nota".into() }, &mut s.sys);
    s.mv(640, 400);
    s.key(Key::Tab);
    s.key(Key::Down);
    s.save("7_dialogo_guardar");
    let text = "Se ha encontrado un disco sin formato. ¿Quieres prepararlo para NightFury?".into();
    s.d.request_dialog(DialogRequest::Message { tag: 2, title: "Disco nuevo".into(), text, buttons: Buttons::YesNo }, &mut s.sys);
    s.mv(660, 420);
    s.save("8_dialogo_mensaje");
}
