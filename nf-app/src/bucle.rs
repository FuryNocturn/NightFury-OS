//! Bucle de eventos de un programa: traduce `WireEvent` a llamadas de `App` y decide cuándo
//! redibujar. No toca hardware: todo lo del núcleo entra por `Llamadas` (la real está en `rt`, la
//! falsa en las pruebas).
use alloc::boxed::Box;
use alloc::string::String;
use nf_core::abi::*;
use nf_core::apps::{App, Ctx};
use nf_core::files::Files;
use nf_core::keys::Pool;
use nf_core::platform::{DateTime, MemInfo, Platform};
use nf_core::surface::Surface;
use nf_core::theme::Theme;

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
        let mut cx = Ctx::new(&mut self.plat, &mut self.theme, ev.now_ms);
        let mut cerrar = false;
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
    }

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
}
