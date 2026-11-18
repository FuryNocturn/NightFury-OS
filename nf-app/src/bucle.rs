// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Bucle de eventos de un programa: traduce `WireEvent` a llamadas de `App` y decide cuándo
//! redibujar. No toca hardware: todo lo del núcleo entra por `Llamadas` (la real está en `rt`, la
//! falsa en las pruebas).
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use nf_core::abi::*;
use nf_core::apps::{App, Ctx};
use nf_core::dialogs::{Answer, DialogResult};
use nf_core::files::{self, Files};
use nf_core::keys::Pool;
use nf_core::platform::{DateTime, MemInfo, Platform};
use nf_core::surface::Surface;
use nf_core::sync::SpinLock;
use nf_core::theme::Theme;

/// Etiqueta con la que llega a `App::dialog_result` el resultado de `guardar_como`.
pub const TAG_GUARDADO: u32 = u32::MAX;

/// Guardado pedido por la app y aún sin enviar al núcleo; el bucle lo recoge tras cada evento.
static PENDIENTE: SpinLock<Option<(String, Vec<u8>)>> = SpinLock::new(None);
/// Hay un guardado pedido o esperando respuesta (el núcleo atiende uno a la vez).
static EN_CURSO: AtomicBool = AtomicBool::new(false);

/// Pide al sistema guardar `datos` con el nombre propuesto `nombre` (sin `\`, `/` ni `:`): el
/// usuario elige dónde en el diálogo Guardar y el programa nunca ve la ruta. `false` si no se
/// puede pedir (nombre no válido, más de `SAVE_MAX` bytes o ya hay otro guardado en curso).
/// El resultado llega después a `App::dialog_result` con `tag == TAG_GUARDADO`:
/// `Path { path: Some(nombre) }` = guardado, `Path { path: None }` = cancelado y
/// `Answer { answer: No }` = error (sin permiso, disco lleno…).
// shortcut: un guardado a la vez y estado global (un programa = un hilo); si hace falta más, ids por petición.
pub fn guardar_como(nombre: &str, datos: &[u8]) -> bool {
    if !files::valid_name(nombre) || nombre.len() > 255 || datos.len() as u64 > SAVE_MAX {
        return false;
    }
    if EN_CURSO.swap(true, Ordering::AcqRel) {
        return false;
    }
    *PENDIENTE.lock() = Some((String::from(nombre), datos.to_vec()));
    true
}

/// `EV_SAVED` (`a` = 1, 0 o -1) como resultado de diálogo para la app.
fn resultado_guardado(a: i32, nombre: String) -> DialogResult {
    match a {
        1 => DialogResult::Path { tag: TAG_GUARDADO, path: Some(nombre) },
        0 => DialogResult::Path { tag: TAG_GUARDADO, path: None },
        _ => DialogResult::Answer { tag: TAG_GUARDADO, answer: Answer::No },
    }
}

/// `s` recortado a `max` bytes sin partir un carácter UTF-8 (el núcleo rechaza texto inválido).
pub fn cortar(s: &str, max: usize) -> &str {
    let mut n = max.min(s.len());
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    &s[..n]
}

/// Lo que el bucle necesita del núcleo (una llamada del ABI cada método).
pub trait Llamadas {
    /// `WAIT_EVENT`.
    fn esperar(&mut self, timeout_ms: u64) -> WireEvent;
    /// `PRESENT` con `stride = w`.
    fn presentar(&mut self, px: &[u32], w: i32, h: i32);
    fn titulo(&mut self, t: &str);
    fn tam_min(&mut self, w: i32, h: i32);
    fn fecha(&mut self) -> DateTime;
    fn log(&mut self, t: &str);
    /// `SAVE_AS`: `>= 0` pedido (la respuesta llega como `EV_SAVED`), `< 0` rechazado.
    fn guardar(&mut self, nombre: &str, datos: &[u8]) -> i64;
}

/// `Platform` de un programa. El reloj y la fecha los refresca el bucle en cada evento.
/// shortcut: los programas ven una unidad C: en memoria vacía (no el disco), hasta la v0.10.
struct PlataformaPrograma {
    ms: u64,
    fecha: DateTime,
    files: Files,
    pool: Pool,
    semilla: u64,
}

impl Platform for PlataformaPrograma {
    fn now(&mut self) -> DateTime {
        self.fecha
    }
    fn uptime_ms(&self) -> u64 {
        self.ms
    }
    fn mem(&self) -> MemInfo {
        MemInfo { total: MAX_MEM, heap_total: MAX_MEM, heap_used: 0 }
    }
    fn cpu(&self) -> String {
        String::from("Procesador x86-64")
    }
    fn screen(&self) -> (u32, u32) {
        (1280, 720)
    }
    // Un programa no puede apagar ni reiniciar el equipo.
    fn reboot(&mut self) {}
    fn shutdown(&mut self) {}
    fn files(&mut self) -> &mut Files {
        &mut self.files
    }
    /// shortcut: xorshift sobre el reloj; vale para sales y azar de juegos, no para llaves.
    fn entropy(&mut self) -> u64 {
        let mut x = self.semilla ^ self.ms.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.semilla = x;
        x
    }
    fn pool(&mut self) -> &mut Pool {
        &mut self.pool
    }
}

/// Estado del bucle entre eventos.
struct Bucle {
    app: Box<dyn App>,
    plat: PlataformaPrograma,
    theme: Theme,
    size: (i32, i32),
    focused: bool,
    title: String,
    min: (i32, i32),
    dirty: bool,
    surf: Surface,
    tick_ms: Option<u64>,
    next_tick: Option<u64>,
    now: u64,
    /// Nombre del guardado enviado al núcleo y aún sin respuesta.
    guardando: Option<String>,
}

impl Bucle {
    fn timeout(&self) -> u64 {
        self.next_tick.map_or(u64::MAX, |t| t.saturating_sub(self.now))
    }

    /// Título y tamaño mínimo si han cambiado.
    fn avisar(&mut self, c: &mut impl Llamadas) {
        let t = self.app.title();
        if t != self.title {
            c.titulo(&t);
            self.title = t;
        }
        let m = self.app.min_size();
        if m != self.min {
            c.tam_min(m.0, m.1);
            self.min = m;
        }
    }

    /// Entrega un resultado de diálogo a la app. `true` si ella pide cerrar.
    fn entregar(&mut self, r: DialogResult) -> bool {
        let mut cx = Ctx::new(&mut self.plat, &mut self.theme, self.now);
        self.app.dialog_result(r, &mut cx);
        self.dirty |= cx.redraw | cx.theme_changed;
        cx.close
    }

    /// Atiende un evento. `Some(código)` si el programa debe terminar.
    fn evento(&mut self, c: &mut impl Llamadas, ev: &WireEvent) -> Option<i32> {
        self.now = ev.now_ms;
        self.plat.ms = ev.now_ms;
        self.plat.fecha = c.fecha();
        if self.next_tick.is_none() {
            self.next_tick = self.tick_ms.map(|t| ev.now_ms + t);
        }
        // Estado que manda el escritorio: no llega a la app, solo cambia lo que se dibuja.
        match ev.kind {
            EV_THEME => {
                let t = Theme { accent: ev.d, wallpaper: ev.a as u8 };
                self.dirty |= t != self.theme;
                self.theme = t;
            }
            EV_RESIZE => {
                let s = (ev.a.clamp(0, MAX_DIM as i32), ev.b.clamp(0, MAX_DIM as i32));
                self.dirty |= s != self.size;
                self.size = s;
            }
            EV_FOCUS => {
                self.dirty |= (ev.a != 0) != self.focused;
                self.focused = ev.a != 0;
            }
            _ => {}
        }
        let mut cerrar = false;
        if ev.kind == EV_SAVED {
            if let Some(n) = self.guardando.take() {
                EN_CURSO.store(false, Ordering::Release);
                cerrar |= self.entregar(resultado_guardado(ev.a, n));
            }
        }
        let mut cx = Ctx::new(&mut self.plat, &mut self.theme, ev.now_ms);
        match ev.kind {
            EV_KEY => {
                if let Some(k) = ev.to_key() {
                    self.app.key(&k, &mut cx);
                }
            }
            EV_NONE | EV_TIMER => {
                self.app.tick(&mut cx);
                self.next_tick = self.tick_ms.map(|t| ev.now_ms + t);
            }
            EV_CLOSE => cerrar = self.app.request_close(&mut cx),
            _ => {
                if let Some(m) = ev.to_mouse() {
                    self.app.mouse(m, &mut cx);
                }
            }
        }
        cerrar |= cx.close;
        self.dirty |= cx.redraw | cx.theme_changed;
        // v0.8: los programas no tienen diálogos, menús, abrir ni arrastrar; se avisa y se ignora.
        for (pedido, nombre) in [
            (cx.dialog.is_some(), "dialog"),
            (cx.menu.is_some(), "menu"),
            (cx.open.is_some() || cx.open_new.is_some() || cx.open_terminal.is_some(), "open"),
            (cx.drag.is_some(), "drag"),
        ] {
            if pedido {
                c.log(&alloc::format!("cx.{nombre} no existe para programas (v0.8): ignorado"));
            }
        }
        // Guardado pedido durante este evento: se envía al núcleo; si lo rechaza, la app lo sabe ya.
        let pedido = PENDIENTE.lock().take();
        if let Some((nombre, datos)) = pedido {
            if c.guardar(&nombre, &datos) >= 0 {
                self.guardando = Some(nombre);
            } else {
                EN_CURSO.store(false, Ordering::Release);
                cerrar |= self.entregar(resultado_guardado(-1, nombre));
            }
        }
        if cerrar {
            return Some(0);
        }
        self.avisar(c);
        if self.dirty && self.size.0 > 0 && self.size.1 > 0 {
            self.dirty = false;
            if (self.surf.w, self.surf.h) != self.size {
                self.surf = Surface::new(self.size.0, self.size.1, 0xFFFFFF);
            }
            self.app.draw(&mut self.surf, &self.theme, &mut self.plat, self.focused);
            c.presentar(&self.surf.px, self.size.0, self.size.1);
        }
        None
    }
}

/// Ejecuta el programa hasta que termine; devuelve el código de salida (el llamador hace `EXIT`).
/// `tick_ms`: cada cuánto llamar a `App::tick` (`None` = nunca, el programa duerme entre eventos).
pub fn ejecutar(c: &mut impl Llamadas, app: Box<dyn App>, tick_ms: Option<u64>) -> i32 {
    let mut b = Bucle {
        app,
        plat: PlataformaPrograma { ms: 0, fecha: DateTime::default(), files: Files::default(), pool: Pool::new(), semilla: 0x2545_F491_4F6C_DD1D },
        theme: Theme::default(),
        size: (0, 0),
        focused: false,
        title: String::new(),
        min: (-1, -1),
        dirty: false,
        surf: Surface::empty(),
        tick_ms,
        next_tick: None,
        now: 0,
        guardando: None,
    };
    b.avisar(c);
    loop {
        let t = b.timeout();
        let ev = c.esperar(t);
        if let Some(code) = b.evento(c, &ev) {
            return code;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::rc::Rc;
    use alloc::vec::Vec;
    use core::cell::RefCell;
    use nf_core::apps::{AppKind, MouseEv};
    use nf_core::input::KeyEvent;

    /// Núcleo falso: entrega los eventos de la cola (al acabar, pide cerrar) y apunta lo pedido.
    #[derive(Default)]
    struct Falso {
        cola: Vec<WireEvent>,
        timeouts: Vec<u64>,
        frames: Vec<(i32, i32, u32)>,
        titulos: Vec<String>,
        mins: Vec<(i32, i32)>,
        logs: Vec<String>,
        guardados: Vec<(String, Vec<u8>)>,
        rechaza_guardar: bool,
    }
    impl Llamadas for Falso {
        fn esperar(&mut self, t: u64) -> WireEvent {
            self.timeouts.push(t);
            if self.cola.is_empty() {
                WireEvent::new(EV_CLOSE, 0)
            } else {
                self.cola.remove(0)
            }
        }
        fn presentar(&mut self, px: &[u32], w: i32, h: i32) {
            assert_eq!(px.len(), (w * h) as usize);
            self.frames.push((w, h, px[0]));
        }
        fn titulo(&mut self, t: &str) {
            self.titulos.push(t.into());
        }
        fn tam_min(&mut self, w: i32, h: i32) {
            self.mins.push((w, h));
        }
        fn fecha(&mut self) -> DateTime {
            DateTime { year: 2026, ..Default::default() }
        }
        fn log(&mut self, t: &str) {
            self.logs.push(t.into());
        }
        fn guardar(&mut self, n: &str, d: &[u8]) -> i64 {
            self.guardados.push((n.into(), d.into()));
            if self.rechaza_guardar {
                E_BUSY
            } else {
                0
            }
        }
    }

    /// Los guardados son estado global: los tests del bucle no pueden correr a la vez.
    static SERIE: SpinLock<()> = SpinLock::new(());

    #[derive(Default)]
    struct Visto {
        teclas: u32,
        ratones: Vec<MouseEv>,
        ticks: u32,
        draws: Vec<(i32, i32, bool, u32)>,
        titulo: String,
        cierra: bool,
    }
    struct Prueba(Rc<RefCell<Visto>>);
    impl App for Prueba {
        fn kind(&self) -> AppKind {
            AppKind::Program
        }
        fn title(&self) -> String {
            self.0.borrow().titulo.clone()
        }
        fn min_size(&self) -> (i32, i32) {
            (100, 50)
        }
        fn draw(&mut self, s: &mut Surface, th: &Theme, _: &mut dyn Platform, f: bool) {
            self.0.borrow_mut().draws.push((s.w, s.h, f, th.accent));
            s.px[0] = 0x123456;
        }
        fn key(&mut self, _: &KeyEvent, cx: &mut Ctx) {
            let mut v = self.0.borrow_mut();
            v.teclas += 1;
            v.titulo = alloc::format!("teclas {}", v.teclas);
            cx.redraw = true;
            cx.open = Some(String::from("C:\\"));
        }
        fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
            self.0.borrow_mut().ratones.push(ev);
            cx.close = matches!(ev, MouseEv::RightDown { .. });
        }
        fn tick(&mut self, cx: &mut Ctx) {
            self.0.borrow_mut().ticks += 1;
            cx.redraw = true;
        }
        fn request_close(&mut self, _: &mut Ctx) -> bool {
            self.0.borrow().cierra
        }
    }

    fn ev(kind: u32, a: i32, b: i32, now: u64) -> WireEvent {
        WireEvent { kind, a, b, now_ms: now, ..Default::default() }
    }
    fn arranque() -> Vec<WireEvent> {
        vec![WireEvent { kind: EV_THEME, a: 2, d: 0xABCDEF, ..Default::default() }, ev(EV_RESIZE, 200, 100, 0), ev(EV_FOCUS, 1, 0, 0)]
    }

    #[test]
    fn arranque_dibuja_una_vez_y_traduce_eventos() {
        let _g = SERIE.lock();
        let visto = Rc::new(RefCell::new(Visto::default()));
        let mut c = Falso { cola: arranque(), ..Default::default() };
        c.cola.push(WireEvent { kind: EV_KEY, a: 0, d: 'a' as u32, ..Default::default() });
        c.cola.push(ev(EV_MOUSE_MOVE, 3, 4, 5));
        c.cola.push(ev(EV_MOUSE_RIGHT, 1, 1, 6));
        let code = ejecutar(&mut c, Box::new(Prueba(visto.clone())), None);
        assert_eq!(code, 0);
        assert_eq!(c.mins, vec![(100, 50)]);
        // Un dibujo por cada evento que lo pide: el primero (tema + tamaño + foco juntos no son
        // uno: cada evento llega por separado, pero con tamaño 0 aún no se dibuja) y la tecla.
        let v = visto.borrow();
        assert_eq!(v.draws, vec![(200, 100, false, 0xABCDEF), (200, 100, true, 0xABCDEF), (200, 100, true, 0xABCDEF)]);
        assert_eq!(c.frames, vec![(200, 100, 0x123456); 3]);
        assert_eq!(v.teclas, 1);
        assert_eq!(v.ratones, vec![MouseEv::Move { x: 3, y: 4 }, MouseEv::RightDown { x: 1, y: 1 }]);
        // El título nuevo se avisa una vez; cx.open se ignora con una línea de log.
        assert_eq!(c.titulos, vec![String::from("teclas 1")]);
        assert_eq!(c.logs.len(), 1);
        assert!(c.logs[0].contains("open"));
    }

    #[test]
    fn tick_con_reloj_y_cierre() {
        let _g = SERIE.lock();
        let visto = Rc::new(RefCell::new(Visto::default()));
        let mut c = Falso { cola: arranque(), ..Default::default() };
        c.cola.push(ev(EV_NONE, 0, 0, 1000));
        c.cola.push(ev(EV_MOUSE_MOVE, 0, 0, 1030));
        c.cola.push(ev(EV_TIMER, 0, 0, 1100));
        // Sin `cierra`, EV_CLOSE no cierra; con él sí. Aquí cierra el Close del final.
        visto.borrow_mut().cierra = true;
        ejecutar(&mut c, Box::new(Prueba(visto.clone())), Some(100));
        assert_eq!(visto.borrow().ticks, 2);
        // Esperas: antes del primer evento no hay reloj (sin límite); luego 100 hasta el tick de
        // 1000 (siguiente a 1100), tras el movimiento a 1030 quedan 70 y tras el tick de 1100, 100.
        let t = &c.timeouts;
        assert_eq!(t[0], u64::MAX);
        assert_eq!(t[t.len() - 4..], [100, 100, 70, 100]);
    }

    #[test]
    fn cortar_no_parte_caracteres() {
        assert_eq!(cortar("abc", 10), "abc");
        assert_eq!(cortar("añb", 2), "a");
        assert_eq!(cortar("añb", 3), "añ");
        assert_eq!(cortar("abc", 0), "");
    }

    #[test]
    fn sin_tick_duerme_y_close_rechazado_sigue() {
        let _g = SERIE.lock();
        let visto = Rc::new(RefCell::new(Visto::default()));
        let mut c = Falso { cola: arranque(), ..Default::default() };
        c.cola.push(ev(EV_CLOSE, 0, 0, 0));
        c.cola.push(ev(EV_FOCUS, 0, 0, 0));
        // La app no quiere cerrar (cierra = false) hasta que cx.close lo decida: aquí EV_CLOSE
        // falso la deja viva y el cierre final del Falso también, así que acabamos con el clic.
        c.cola.push(ev(EV_MOUSE_RIGHT, 0, 0, 0));
        ejecutar(&mut c, Box::new(Prueba(visto.clone())), None);
        assert!(c.timeouts.iter().all(|&t| t == u64::MAX));
        // Perder el foco redibuja.
        assert_eq!(visto.borrow().draws.last().map(|d| d.2), Some(false));
        assert_eq!(visto.borrow().ratones.len(), 1);
    }

    /// App que guarda con cada tecla y apunta lo que recibe.
    #[derive(Default)]
    struct Guarda {
        resultados: Vec<DialogResult>,
        segundo: Vec<bool>,
    }
    struct GuardaApp(Rc<RefCell<Guarda>>);
    impl App for GuardaApp {
        fn kind(&self) -> AppKind {
            AppKind::Program
        }
        fn title(&self) -> String {
            String::new()
        }
        fn min_size(&self) -> (i32, i32) {
            (1, 1)
        }
        fn draw(&mut self, _: &mut Surface, _: &Theme, _: &mut dyn Platform, _: bool) {}
        fn key(&mut self, _: &KeyEvent, _: &mut Ctx) {
            let ok = guardar_como("dibujo.bmp", b"BM..");
            // Con el primero en curso, otro igual se rechaza.
            let otro = guardar_como("otro.bmp", b"x");
            let mut g = self.0.borrow_mut();
            g.segundo.push(ok);
            g.segundo.push(otro);
        }
        fn mouse(&mut self, _: MouseEv, _: &mut Ctx) {}
        fn tick(&mut self, _: &mut Ctx) {}
        fn dialog_result(&mut self, r: DialogResult, _: &mut Ctx) {
            self.0.borrow_mut().resultados.push(r);
        }
    }

    #[test]
    fn guardar_como_ida_y_vuelta() {
        let _g = SERIE.lock();
        assert!(!guardar_como(r"a\b", b"x") && !guardar_como("", b"x") && !guardar_como("a:b", b"x"), "el nombre no es una ruta");
        let g = Rc::new(RefCell::new(Guarda::default()));
        let tecla = WireEvent { kind: EV_KEY, a: 0, d: 'g' as u32, ..Default::default() };
        let saved = |a| WireEvent { kind: EV_SAVED, a, ..Default::default() };
        let mut c = Falso { cola: arranque(), ..Default::default() };
        // Guardado, cancelado, y otra vez guardado tras el cancelado (el estado se libera).
        c.cola.extend([tecla, saved(1), tecla, saved(0), tecla, saved(-1)]);
        ejecutar(&mut c, Box::new(GuardaApp(g.clone())), None);
        assert_eq!(c.guardados.len(), 3, "solo el primero de cada tecla llega al núcleo");
        assert_eq!(c.guardados[0], (String::from("dibujo.bmp"), b"BM..".to_vec()));
        let g = g.borrow();
        assert_eq!(g.segundo, [true, false, true, false, true, false]);
        let tag = TAG_GUARDADO;
        assert_eq!(
            g.resultados,
            [
                DialogResult::Path { tag, path: Some(String::from("dibujo.bmp")) },
                DialogResult::Path { tag, path: None },
                DialogResult::Answer { tag, answer: Answer::No },
            ]
        );
        // Un EV_SAVED que nadie esperaba no llega a la app.
        drop(g);
        let g = Rc::new(RefCell::new(Guarda::default()));
        let mut c = Falso { cola: arranque(), ..Default::default() };
        c.cola.push(saved(1));
        ejecutar(&mut c, Box::new(GuardaApp(g.clone())), None);
        assert!(g.borrow().resultados.is_empty());
        // El núcleo lo rechaza: la app recibe el error enseguida y puede pedir otro.
        let g = Rc::new(RefCell::new(Guarda::default()));
        let mut c = Falso { cola: arranque(), rechaza_guardar: true, ..Default::default() };
        c.cola.extend([tecla, tecla]);
        ejecutar(&mut c, Box::new(GuardaApp(g.clone())), None);
        assert_eq!(g.borrow().resultados.len(), 2);
        assert_eq!(g.borrow().segundo, [true, false, true, false]);
        assert!(guardar_como("libre.bmp", b"x"), "nada se queda en curso");
        // Lo pedido y no enviado (la prueba no pasó por el bucle) se limpia para no dejar rastro.
        PENDIENTE.lock().take();
        EN_CURSO.store(false, Ordering::Release);
    }
}
