// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! nfprog: herramienta del PC para los programas del sistema.
//!
//! - `nfprog iconos [programs]`: dibuja con nf-core el `icono.bmp` (32×32, BMP de 24 bits, magenta
//!   FF00FF = transparente) de Reloj y Paint y lo deja en `programs/<nombre>/`.
//! - `nfprog capturas <carpeta>`: guarda en BMP el contenido de Reloj, Paint y el Administrador de
//!   tareas en varias situaciones (para revisar el aspecto sin arrancar el sistema).
use nf_core::apps::{self, App, AppKind, Ctx, MouseEv};
use nf_core::bmp;
use nf_core::files::Files;
use nf_core::keys::Pool;
use nf_core::platform::{DateTime, MemInfo, Platform, TestPlatform};
use nf_core::proc::ProcInfo;
use nf_core::surface::Surface;
use nf_core::theme::Theme;
use std::process::ExitCode;

const MAGENTA: u32 = 0xFF00FF;

// ---------------------------------------------------------------- iconos

/// Dibuja `f` sobre dos fondos distintos y deduce de ahí el color y la opacidad reales de cada
/// píxel: el resultado tiene bordes duros (opaco o magenta) y sin cercos rosados.
fn icono(f: impl Fn(&mut Surface)) -> Surface {
    let mut a = Surface::new(32, 32, 0xFF00FF);
    let mut b = Surface::new(32, 32, 0x00FF00);
    f(&mut a);
    f(&mut b);
    let mut out = Surface::new(32, 32, MAGENTA);
    for i in 0..out.px.len() {
        let canal = |c: u32, sh: u32| ((c >> sh) & 0xFF) as f32;
        // Con fondos (255, 0, 255) y (0, 255, 0): R_a - R_b = (1 - alfa) * 255.
        let alfa = 1.0 - (canal(a.px[i], 16) - canal(b.px[i], 16)) / 255.0;
        if alfa < 0.5 {
            continue;
        }
        let r = ((canal(a.px[i], 16) - (1.0 - alfa) * 255.0) / alfa).clamp(0.0, 255.0) as u32;
        let g = (canal(a.px[i], 8) / alfa).clamp(0.0, 255.0) as u32;
        let bl = ((canal(a.px[i], 0) - (1.0 - alfa) * 255.0) / alfa).clamp(0.0, 255.0) as u32;
        let c = r << 16 | g << 8 | bl;
        // Nada del dibujo puede confundirse con el color transparente.
        out.px[i] = if c == MAGENTA { 0xFE00FE } else { c };
    }
    out
}

fn icono_reloj(s: &mut Surface) {
    let (c, azul, tinta) = (16.0, 0x2F7BE0, 0x2B2F36);
    s.fill_circle(c, c, 14.5, azul, 255);
    s.fill_circle(c, c, 11.8, 0xFFFFFF, 255);
    // Marcas de las 12, 3, 6 y 9, y el resto en puntos.
    for h in 0..12 {
        let a = h as f32 * std::f32::consts::PI / 6.0;
        let (sn, cs) = a.sin_cos();
        let largo = if h % 3 == 0 { 3.0 } else { 1.0 };
        let (r0, r1) = (10.8 - largo, 10.8);
        s.stroke(&[(c + sn * r0, c - cs * r0), (c + sn * r1, c - cs * r1)], if h % 3 == 0 { 1.8 } else { 1.0 }, 0x8A9099);
    }
    // Las 10:10.
    s.stroke(&[(c, c), (c - 4.6, c - 3.4)], 2.6, tinta);
    s.stroke(&[(c, c), (c + 5.0, c - 6.6)], 2.0, tinta);
    s.fill_circle(c, c, 1.9, 0xD94141, 255);
}

fn icono_paint(s: &mut Surface) {
    // Hoja con marco, trazos de color y un pincel que los acaba de dejar.
    s.fill_rect(nf_core::geom::Rect::new(3, 5, 22, 23), 0x8A9099);
    s.fill_rect(nf_core::geom::Rect::new(4, 6, 20, 21), 0xFFFFFF);
    s.stroke(&[(6.5, 22.0), (10.0, 14.0), (13.0, 20.0), (16.5, 12.0)], 2.6, 0xD94141);
    s.stroke(&[(6.5, 10.0), (10.5, 8.5), (14.0, 11.0)], 2.4, 0xF2B01E);
    s.stroke(&[(10.0, 25.0), (16.0, 24.0), (21.5, 20.0)], 2.4, 0x3CB371);
    s.stroke(&[(29.0, 3.0), (19.0, 13.0)], 3.4, 0x8B5A2B);
    s.stroke(&[(19.0, 13.0), (16.6, 15.4)], 4.0, 0xB8BCC4);
    s.fill_circle(14.6, 17.4, 3.2, 0x2F7BE0, 255);
}

fn iconos(dir: &str) -> Result<(), String> {
    for (nombre, f) in [("reloj", icono_reloj as fn(&mut Surface)), ("paint", icono_paint)] {
        let ruta = format!("{dir}/{nombre}/icono.bmp");
        std::fs::write(&ruta, bmp::encode(&icono(f))).map_err(|e| format!("{ruta}: {e}"))?;
        println!("{ruta}");
    }
    Ok(())
}

// ---------------------------------------------------------------- capturas

/// Plataforma de pruebas con una lista de programas falsa (para el Administrador de tareas).
struct ConProcesos {
    base: TestPlatform,
    procs: Vec<ProcInfo>,
}

impl Platform for ConProcesos {
    fn now(&mut self) -> DateTime {
        self.base.now()
    }
    fn uptime_ms(&self) -> u64 {
        self.base.uptime_ms()
    }
    fn mem(&self) -> MemInfo {
        self.base.mem()
    }
    fn cpu(&self) -> String {
        self.base.cpu()
    }
    fn screen(&self) -> (u32, u32) {
        self.base.screen()
    }
    fn reboot(&mut self) {}
    fn shutdown(&mut self) {}
    fn files(&mut self) -> &mut Files {
        self.base.files()
    }
    fn entropy(&mut self) -> u64 {
        self.base.entropy()
    }
    fn pool(&mut self) -> &mut Pool {
        self.base.pool()
    }
    fn processes(&mut self) -> Vec<ProcInfo> {
        self.procs.clone()
    }
}

/// Una app con su plataforma y su tema, a la que se le mandan eventos como haría el escritorio.
struct Banco {
    app: Box<dyn App>,
    sys: ConProcesos,
    th: Theme,
}

impl Banco {
    fn new(app: Box<dyn App>, procs: Vec<ProcInfo>) -> Self {
        Banco { app, sys: ConProcesos { base: TestPlatform { size: (1280, 800), ..Default::default() }, procs }, th: Theme::default() }
    }

    fn ratón(&mut self, ev: MouseEv, ms: u64) {
        self.sys.base.ms = ms;
        let mut cx = Ctx::new(&mut self.sys, &mut self.th, ms);
        self.app.mouse(ev, &mut cx);
    }

    fn clic(&mut self, p: (i32, i32), ms: u64) {
        self.ratón(MouseEv::Down { x: p.0, y: p.1 }, ms);
        self.ratón(MouseEv::Up { x: p.0, y: p.1 }, ms);
    }

    /// Arrastra por los puntos dados (pulsa en el primero, suelta en el último).
    fn trazo(&mut self, pts: &[(i32, i32)]) {
        self.ratón(MouseEv::Down { x: pts[0].0, y: pts[0].1 }, 0);
        for p in &pts[1..] {
            self.ratón(MouseEv::Move { x: p.0, y: p.1 }, 0);
        }
        let u = pts[pts.len() - 1];
        self.ratón(MouseEv::Up { x: u.0, y: u.1 }, 0);
    }

    fn tick(&mut self, ms: u64) {
        self.sys.base.ms = ms;
        let mut cx = Ctx::new(&mut self.sys, &mut self.th, ms);
        self.app.tick(&mut cx);
    }

    fn foto(&mut self, w: i32, h: i32, ms: u64) -> Surface {
        self.sys.base.ms = ms;
        let mut s = Surface::new(w, h, 0);
        self.app.draw(&mut s, &self.th, &mut self.sys, true);
        s
    }
}

fn guarda(dir: &str, nombre: &str, s: &Surface) -> Result<(), String> {
    let ruta = format!("{dir}/{nombre}.bmp");
    std::fs::write(&ruta, bmp::encode(s)).map_err(|e| format!("{ruta}: {e}"))?;
    println!("{ruta}");
    Ok(())
}

fn capturas_reloj(dir: &str) -> Result<(), String> {
    let mut b = Banco::new(reloj_app::crear(), vec![]);
    guarda(dir, "reloj-reloj", &b.foto(420, 360, 0))?;
    // Cronómetro: arranca, dos vueltas, y se queda corriendo.
    b.clic((210, 22), 0);
    b.clic((80, 174), 1000);
    b.clic((210, 174), 13_340);
    b.clic((210, 174), 27_810);
    b.tick(41_230);
    guarda(dir, "reloj-cronometro", &b.foto(420, 360, 41_230))?;
    // Temporizador: 5:00 → 2:00 y en marcha.
    b.clic((350, 22), 0);
    guarda(dir, "reloj-temporizador-parado", &b.foto(420, 360, 0))?;
    for _ in 0..3 {
        b.clic((50, 212), 0);
    }
    b.clic((145, 274), 100_000);
    b.tick(100_000);
    guarda(dir, "reloj-temporizador-marcha", &b.foto(420, 360, 160_500))?;
    // Acabado: las dos mitades del parpadeo.
    b.tick(100_000 + 120_000 + 1);
    guarda(dir, "reloj-temporizador-fin-a", &b.foto(420, 360, 220_100))?;
    guarda(dir, "reloj-temporizador-fin-b", &b.foto(420, 360, 220_100 + 400))?;
    Ok(())
}

fn capturas_paint(dir: &str) -> Result<(), String> {
    let mut b = Banco::new(paint_app::crear(), vec![]);
    guarda(dir, "paint-vacio", &b.foto(920, 620, 0))?;
    // Posiciones de la ventana de 920×620: lienzo en (100, 48), herramientas y colores abajo.
    let lienzo = |x: i32, y: i32| (100 + x, 48 + y);
    let herr = |i: i32| (4 + (i % 2) * 44 + 20, 40 + 8 + (i / 2) * 44 + 20);
    let color = |i: i32| (80 + (i % 8) * 30 + 13, 620 - 64 + 4 + (i / 8) * 30 + 13);
    let grosor = |i: i32| (920 - 8 - (4 - i) * 44 + 20, 20);
    let (lapiz, pincel, linea, rect, relleno) = (herr(0), herr(1), herr(3), herr(4), herr(5));
    // Suelo y casa.
    b.clic(rect, 0);
    b.clic(color(5), 0);
    b.clic(grosor(1), 0);
    b.trazo(&[lienzo(-5, 400), lienzo(805, 505)]);
    b.clic(relleno, 0);
    b.clic(color(13), 0);
    b.clic(lienzo(400, 450), 0);
    b.clic(rect, 0);
    b.clic(color(0), 0);
    b.trazo(&[lienzo(200, 250), lienzo(380, 400)]);
    b.clic(relleno, 0);
    b.clic(color(12), 0);
    b.clic(lienzo(290, 330), 0);
    b.clic(linea, 0);
    b.clic(color(2), 0);
    b.trazo(&[lienzo(180, 250), lienzo(290, 165)]);
    b.trazo(&[lienzo(290, 165), lienzo(400, 250)]);
    b.trazo(&[lienzo(180, 250), lienzo(400, 250)]);
    b.clic(relleno, 0);
    b.clic(color(3), 0);
    b.clic(lienzo(290, 220), 0);
    // Puerta, ventana.
    b.clic(rect, 0);
    b.clic(color(10), 0);
    b.clic(grosor(0), 0);
    b.trazo(&[lienzo(270, 330), lienzo(310, 400)]);
    b.clic(color(7), 0);
    b.trazo(&[lienzo(220, 280), lienzo(255, 315)]);
    b.trazo(&[lienzo(325, 280), lienzo(360, 315)]);
    // Sol y árbol con el pincel.
    b.clic(pincel, 0);
    b.clic(grosor(3), 0);
    b.clic(color(4), 0);
    let sol: Vec<_> = (0..=16).map(|i| {
        let a = i as f32 * std::f32::consts::PI / 8.0;
        lienzo(650 + (a.cos() * 22.0) as i32, 90 + (a.sin() * 22.0) as i32)
    }).collect();
    b.trazo(&sol);
    b.trazo(&[lienzo(650, 90), lienzo(652, 92)]);
    b.clic(color(10), 0);
    b.clic(grosor(1), 0);
    b.trazo(&[lienzo(560, 410), lienzo(560, 320)]);
    b.clic(color(5), 0);
    b.clic(grosor(3), 0);
    let copa: Vec<_> = (0..=20).map(|i| {
        let a = i as f32 * std::f32::consts::PI / 10.0;
        lienzo(560 + (a.cos() * 38.0) as i32, 285 + (a.sin() * 34.0) as i32)
    }).collect();
    b.trazo(&copa);
    b.trazo(&[lienzo(530, 285), lienzo(590, 285), lienzo(560, 262), lienzo(560, 312)]);
    // Firma a lápiz.
    b.clic(lapiz, 0);
    b.clic(grosor(0), 0);
    b.clic(color(0), 0);
    b.trazo(&[lienzo(40, 30), lienzo(50, 55), lienzo(60, 30), lienzo(70, 55), lienzo(80, 30), lienzo(95, 52)]);
    guarda(dir, "paint-dibujo", &b.foto(920, 620, 0))?;
    // Aviso de Nuevo con cambios.
    b.clic((8 + 40, 20), 0);
    guarda(dir, "paint-aviso", &b.foto(920, 620, 0))?;
    // Ventana más pequeña: aparecen las barras de desplazamiento.
    b.clic((464, 353), 0); // «No guardar»
    guarda(dir, "paint-barras", &b.foto(640, 440, 0))?;
    Ok(())
}

fn capturas_tareas(dir: &str) -> Result<(), String> {
    let p = |id, nombre: &str, mb: u64, cpu, ok| ProcInfo { id, name: nombre.into(), mem: mb * 1_048_576 + 300_000, cpu_ms: cpu, responding: ok };
    let mut b = Banco::new(apps::create(AppKind::TaskManager), vec![p(1, "Calculadora", 3, 0, true), p(2, "Paint", 38, 0, true), p(3, "Reloj", 4, 0, true), p(4, "Plantilla", 3, 0, false)]);
    guarda(dir, "tareas-inicial", &b.foto(560, 400, 0))?;
    // Un segundo después: reparto de CPU, y se selecciona Paint.
    for (i, cpu) in [20, 640, 70, 0].into_iter().enumerate() {
        b.sys.procs[i].cpu_ms = cpu;
    }
    b.tick(1000);
    b.clic((120, 28 + 26 + 10), 1000);
    guarda(dir, "tareas-lista", &b.foto(560, 400, 1000))?;
    b.sys.procs.clear();
    b.tick(2000);
    guarda(dir, "tareas-sin-programas", &b.foto(560, 400, 2000))?;
    // El icono del Administrador a 16, 24, 32 y 64 px.
    let mut s = Surface::new(160, 80, 0x2B2B2B);
    for (i, z) in [16, 24, 32, 64].into_iter().enumerate() {
        nf_core::icons::draw(&mut s, nf_core::icons::Icon::Tareas, 6 + [0, 24, 56, 94][i], 8, z, 0xFFFFFF);
    }
    guarda(dir, "tareas-icono", &s)?;
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let r = match args.get(1).map(String::as_str) {
        Some("iconos") => iconos(args.get(2).map_or("programs", String::as_str)),
        Some("capturas") => match args.get(2) {
            Some(d) => std::fs::create_dir_all(d).map_err(|e| e.to_string()).and_then(|_| capturas_reloj(d)).and_then(|_| capturas_paint(d)).and_then(|_| capturas_tareas(d)),
            None => Err(String::from("uso: nfprog capturas <carpeta>")),
        },
        _ => Err(String::from("uso: nfprog iconos [programs] | nfprog capturas <carpeta>")),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("nfprog: {e}");
            ExitCode::FAILURE
        }
    }
}
