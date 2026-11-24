// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Bucle de eventos de un programa: traduce `WireEvent` a llamadas de `App` y decide cuándo
//! redibujar. No toca hardware: todo lo del núcleo entra por `Llamadas` (la real está en `rt`, la
//! falsa en las pruebas).
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};
use nf_core::abi::*;
use nf_core::apps::{App, Ctx};
use nf_core::proc::lleva_datos;
use nf_core::request::Request;
use nf_core::wire::{decode, encode};
use nf_core::dialogs::{Answer, DialogResult};
use nf_core::files::{self, Files};
use nf_core::fsapi::FilesApi;
use nf_core::keys::Pool;
use nf_core::platform::{DateTime, MemInfo, Platform};
use nf_core::net::NetApi;
use nf_core::service::{Call, RemoteFiles, RemoteNet, Transport};
use nf_core::wire::Wire;
use nf_core::surface::Surface;
use nf_core::sync::SpinLock;
use nf_core::letra;
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
    /// `SERVICE` + `TAKE_REPLY`: manda la petición y devuelve la respuesta (vacía si el núcleo la
    /// rechaza). El programa se para hasta que el escritorio contesta.
    fn servicio(&mut self, peticion: &[u8]) -> Vec<u8>;
    /// `REQUEST`: petición ya codificada con `wire`; `< 0` rechazada.
    fn peticion(&mut self, msg: &[u8]) -> i64;
    /// `EVENT_DATA`: los `len` bytes de datos del último evento (vacío si falla).
    fn datos_evento(&mut self, len: usize) -> Vec<u8>;
}

/// Lo que la app ha dejado pedido en `cx` como peticiones al escritorio.
fn pedidos(cx: &mut Ctx) -> Vec<Request> {
    let mut v = Vec::new();
    v.extend(cx.open.take().map(Request::Open));
    v.extend(cx.open_new.take().map(Request::OpenNew));
    v.extend(cx.open_terminal.take().map(Request::OpenTerminal));
    v.extend(cx.dialog.take().map(Request::Dialog));
    v.extend(cx.menu.take().map(Request::Menu));
    v.extend(cx.drag.take().map(|d| Request::Drag(d.paths)));
    if cx.theme_changed {
        v.push(Request::SetTheme(*cx.theme));
    }
    v
}

fn enviar(c: &mut impl Llamadas, v: Vec<Request>) {
    for r in v {
        let e = c.peticion(&encode(&r));
        if e < 0 {
            c.log(&alloc::format!("REQUEST rechazada ({e})"));
        }
    }
}

/// Las `Llamadas` compartidas entre el bucle y la plataforma del programa (que las necesita para
/// pedir al sistema archivos y datos del equipo). Un solo hilo: el `RefCell` nunca se pide dos veces.
#[derive(Clone)]
struct Red<'a>(Rc<RefCell<&'a mut dyn Llamadas>>);

impl Llamadas for Red<'_> {
    fn esperar(&mut self, timeout_ms: u64) -> WireEvent {
        self.0.borrow_mut().esperar(timeout_ms)
    }
    fn presentar(&mut self, px: &[u32], w: i32, h: i32) {
        self.0.borrow_mut().presentar(px, w, h)
    }
    fn titulo(&mut self, t: &str) {
        self.0.borrow_mut().titulo(t)
    }
    fn tam_min(&mut self, w: i32, h: i32) {
        self.0.borrow_mut().tam_min(w, h)
    }
    fn fecha(&mut self) -> DateTime {
        self.0.borrow_mut().fecha()
    }
    fn log(&mut self, t: &str) {
        self.0.borrow_mut().log(t)
    }
    fn guardar(&mut self, nombre: &str, datos: &[u8]) -> i64 {
        self.0.borrow_mut().guardar(nombre, datos)
    }
    fn servicio(&mut self, peticion: &[u8]) -> Vec<u8> {
        self.0.borrow_mut().servicio(peticion)
    }
    fn peticion(&mut self, msg: &[u8]) -> i64 {
        self.0.borrow_mut().peticion(msg)
    }
    fn datos_evento(&mut self, len: usize) -> Vec<u8> {
        self.0.borrow_mut().datos_evento(len)
    }
}

impl Transport for Red<'_> {
    fn call(&mut self, req: &[u8]) -> Vec<u8> {
        self.servicio(req)
    }
}

/// `Platform` de un programa. El reloj y la fecha los refresca el bucle en cada evento. Los
/// archivos (`fs`) y los datos del equipo se piden al sistema por `SERVICE`; si no contesta, valen
/// los valores neutros de `RemoteFiles::ask`.
struct PlataformaPrograma<'a> {
    ms: u64,
    fecha: DateTime,
    /// Solo para `Platform::files`, que un programa no tiene: una unidad C: vacía en memoria.
    files: Files,
    fs: RemoteFiles<Red<'a>>,
    net: Red<'a>,
    /// La red: se pide al sistema por `SERVICE` (solo la tienen los programas del sistema).
    remota: RemoteNet<Red<'a>>,
    pool: Pool,
    semilla: u64,
}

impl PlataformaPrograma<'_> {
    fn pedir<R: Wire>(&self, c: Call, malo: R) -> R {
        RemoteFiles(self.net.clone()).ask(c, malo)
    }
}

impl Platform for PlataformaPrograma<'_> {
    fn now(&mut self) -> DateTime {
        self.fecha
    }
    fn uptime_ms(&self) -> u64 {
        self.ms
    }
    fn mem(&self) -> MemInfo {
        self.pedir(Call::Mem, MemInfo { total: MAX_MEM, heap_total: MAX_MEM, heap_used: 0 })
    }
    fn cpu(&self) -> String {
        self.pedir(Call::Cpu, String::from("Procesador x86-64"))
    }
    fn screen(&self) -> (u32, u32) {
        self.pedir(Call::Screen, (1280, 720))
    }
    // Reiniciar y apagar los hace el sistema, y solo si el programa es de los suyos.
    fn reboot(&mut self) {
        self.pedir(Call::Reboot, ())
    }
    fn shutdown(&mut self) {
        self.pedir(Call::Shutdown, ())
    }
    fn files(&mut self) -> &mut Files {
        &mut self.files
    }
    fn fs(&mut self) -> &mut dyn FilesApi {
        &mut self.fs
    }
    /// La entropía la da el sistema; si no contesta, xorshift sobre el reloj.
    // shortcut: el respaldo local no es criptográfico (vale para azar de juegos, no para llaves); basta mientras `SERVICE` no falle.
    fn entropy(&mut self) -> u64 {
        let mut x = self.semilla ^ self.ms.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.semilla = x;
        self.pedir(Call::Entropy, x)
    }
    fn pool(&mut self) -> &mut Pool {
        &mut self.pool
    }
    fn net(&mut self) -> Option<&mut dyn NetApi> {
        Some(&mut self.remota)
    }
}

/// Estado del bucle entre eventos.
struct Bucle<'a> {
    app: Box<dyn App>,
    plat: PlataformaPrograma<'a>,
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

impl Bucle<'_> {
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
    fn entregar(&mut self, c: &mut impl Llamadas, r: DialogResult) -> bool {
        let mut cx = Ctx::new(&mut self.plat, &mut self.theme, self.now);
        self.app.dialog_result(r, &mut cx);
        self.dirty |= cx.redraw | cx.theme_changed;
        let (cerrar, v) = (cx.close, pedidos(&mut cx));
        enviar(c, v);
        cerrar
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
                let t = Theme { accent: ev.d, wallpaper: ev.a as u8, letra: ev.b as u8 };
                // Cada programa carga la letra por su cuenta (también la primera vez, aunque el tema coincida).
                letra::aplicar(&mut self.plat.fs, t.letra);
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
                cerrar |= self.entregar(c, resultado_guardado(ev.a, n));
            }
        }
        // Los datos del evento hay que pedirlos antes de la llamada siguiente al núcleo.
        let datos = if lleva_datos(ev.kind) { c.datos_evento(ev.d as usize) } else { Vec::new() };
        let mut respuesta = None;
        let mut cx = Ctx::new(&mut self.plat, &mut self.theme, ev.now_ms);
        match ev.kind {
            EV_DIALOG => {
                if let Ok(r) = decode::<DialogResult>(&datos) {
                    self.app.dialog_result(r, &mut cx);
                }
            }
            EV_MENU => self.app.menu_result(ev.a as u32, ev.b as u32, &mut cx),
            EV_DROP => {
                if let Ok(p) = decode::<Vec<String>>(&datos) {
                    self.app.drop_files(ev.a, ev.b, &p, ev.c != 0, &mut cx);
                }
            }
            EV_OPEN => {
                if let Ok(p) = decode::<String>(&datos) {
                    self.app.open_path(&p, &mut cx);
                }
            }
            EV_GAMEPAD => {
                if let Some((id, st)) = ev.to_gamepad() {
                    self.app.gamepad(id, &st, &mut cx);
                }
            }
            EV_GAMEPADS => {
                if let Ok(l) = decode::<Vec<nf_core::gamepad::PadInfo>>(&datos) {
                    self.app.gamepads(&l, &mut cx);
                }
            }
            EV_DRAG_OVER => {
                if let Ok(p) = decode::<Vec<String>>(&datos) {
                    let pos = (ev.c != 0).then_some((ev.a, ev.b));
                    let ok = self.app.drag_over(pos, &p, &mut cx);
                    // Al salir o terminar el escritorio no espera respuesta.
                    respuesta = pos.map(|_| Request::DragAnswer(ok));
                }
            }
            EV_DRAG_SOURCE => respuesta = Some(Request::DragSourceAnswer(self.app.drag_source(ev.a, ev.b, &mut cx))),
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
        // La respuesta de arrastre va primero: el escritorio la está esperando.
        let mut v: Vec<Request> = respuesta.into_iter().collect();
        v.extend(pedidos(&mut cx));
        enviar(c, v);
        // Guardado pedido durante este evento: se envía al núcleo; si lo rechaza, la app lo sabe ya.
        let pedido = PENDIENTE.lock().take();
        if let Some((nombre, datos)) = pedido {
            if c.guardar(&nombre, &datos) >= 0 {
                self.guardando = Some(nombre);
            } else {
                EN_CURSO.store(false, Ordering::Release);
                cerrar |= self.entregar(c, resultado_guardado(-1, nombre));
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
    let c: &mut dyn Llamadas = c;
    let mut red = Red(Rc::new(RefCell::new(c)));
    let mut b = Bucle {
        app,
        plat: PlataformaPrograma {
            ms: 0,
            fecha: DateTime::default(),
            files: Files::default(),
            fs: RemoteFiles(red.clone()),
            net: red.clone(),
            remota: RemoteNet(red.clone()),
            pool: Pool::new(),
            semilla: 0x2545_F491_4F6C_DD1D,
        },
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
    b.avisar(&mut red);
    loop {
        let t = b.timeout();
        let ev = red.esperar(t);
        if let Some(code) = b.evento(&mut red, &ev) {
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
    use nf_core::apps::{AppKind, DragRequest, MouseEv};
    use nf_core::dialogs::DialogRequest;
    use nf_core::input::KeyEvent;
    use nf_core::menu::{MenuItem, MenuRequest};

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
        /// Peticiones recibidas, ya decodificadas.
        pedidos: Vec<Request>,
        /// Datos de los eventos con datos, en el orden en que se piden.
        datos: Vec<Vec<u8>>,
        /// Peticiones de `SERVICE` recibidas y lo que se contesta a todas.
        servicios: Vec<Vec<u8>>,
        respuesta: Vec<u8>,
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
        fn peticion(&mut self, m: &[u8]) -> i64 {
            self.pedidos.push(decode(m).unwrap());
            0
        }
        fn datos_evento(&mut self, len: usize) -> Vec<u8> {
            let d = self.datos.remove(0);
            assert_eq!(d.len(), len);
            d
        }
        fn servicio(&mut self, peticion: &[u8]) -> Vec<u8> {
            self.servicios.push(peticion.into());
            self.respuesta.clone()
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
        // El título nuevo se avisa una vez; cx.open sale como petición.
        assert_eq!(c.titulos, vec![String::from("teclas 1")]);
        assert!(c.logs.is_empty());
        assert_eq!(c.pedidos, vec![Request::Open(String::from("C:\\"))]);
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

    /// App que apunta lo que le llega y pide cosas con cada tecla.
    #[derive(Default)]
    struct Datos {
        vistos: Vec<String>,
    }
    struct DatosApp(Rc<RefCell<Datos>>);
    impl App for DatosApp {
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
        fn key(&mut self, _: &KeyEvent, cx: &mut Ctx) {
            cx.dialog = Some(DialogRequest::Input { tag: 4, title: String::from("t"), label: String::new(), value: String::new() });
            cx.menu = Some(MenuRequest { tag: 5, x: 1, y: 2, items: vec![MenuItem::new(1, "a")] });
            cx.drag = Some(DragRequest { paths: vec![String::from("C:\\a")] });
            cx.open_new = Some(String::from("C:\\b"));
            cx.open_terminal = Some(String::from("C:\\c"));
            cx.theme.accent = 0x010203;
            cx.theme_changed = true;
        }
        fn mouse(&mut self, _: MouseEv, _: &mut Ctx) {}
        fn tick(&mut self, _: &mut Ctx) {}
        fn dialog_result(&mut self, r: DialogResult, _: &mut Ctx) {
            self.0.borrow_mut().vistos.push(alloc::format!("dialogo {r:?}"));
        }
        fn menu_result(&mut self, tag: u32, id: u32, _: &mut Ctx) {
            self.0.borrow_mut().vistos.push(alloc::format!("menu {tag} {id}"));
        }
        fn drop_files(&mut self, x: i32, y: i32, p: &[String], copy: bool, _: &mut Ctx) {
            self.0.borrow_mut().vistos.push(alloc::format!("drop {x} {y} {p:?} {copy}"));
        }
        fn open_path(&mut self, p: &str, _: &mut Ctx) {
            self.0.borrow_mut().vistos.push(alloc::format!("open {p}"));
        }
        fn gamepad(&mut self, id: u32, s: &nf_core::gamepad::PadState, _: &mut Ctx) {
            self.0.borrow_mut().vistos.push(alloc::format!("mando {id} {} {}", s.buttons, s.axes[0]));
        }
        fn gamepads(&mut self, l: &[nf_core::gamepad::PadInfo], _: &mut Ctx) {
            self.0.borrow_mut().vistos.push(alloc::format!("mandos {l:?}"));
        }
        fn drag_over(&mut self, pos: Option<(i32, i32)>, p: &[String], _: &mut Ctx) -> bool {
            self.0.borrow_mut().vistos.push(alloc::format!("over {pos:?} {p:?}"));
            pos.is_some_and(|(x, _)| x > 0)
        }
        fn drag_source(&mut self, x: i32, y: i32, _: &mut Ctx) -> Option<Vec<String>> {
            self.0.borrow_mut().vistos.push(alloc::format!("origen {x} {y}"));
            (x > 0).then(|| vec![String::from("C:\\o")])
        }
    }

    #[test]
    fn eventos_con_datos_y_peticiones() {
        let _g = SERIE.lock();
        let d = Rc::new(RefCell::new(Datos::default()));
        let mut c = Falso { cola: arranque(), ..Default::default() };
        let ponlo = |c: &mut Falso, kind: u32, a, b, cc, data: Vec<u8>| {
            c.cola.push(WireEvent { kind, a, b, c: cc, d: data.len() as u32, ..Default::default() });
            c.datos.push(data);
        };
        let rutas = vec![String::from("C:\\x")];
        ponlo(&mut c, EV_DIALOG, 0, 0, 0, encode(&DialogResult::Text { tag: 4, value: Some(String::from("v")) }));
        c.cola.push(ev(EV_MENU, 5, 9, 0));
        ponlo(&mut c, EV_DROP, 3, 4, 1, encode(&rutas));
        ponlo(&mut c, EV_OPEN, 0, 0, 0, encode(&String::from("C:\\z")));
        ponlo(&mut c, EV_DRAG_OVER, 10, 2, 1, encode(&rutas));
        ponlo(&mut c, EV_DRAG_OVER, 0, 0, 0, encode(&rutas));
        c.cola.push(ev(EV_DRAG_SOURCE, 7, 8, 0));
        c.cola.push(ev(EV_DRAG_SOURCE, 0, 0, 0));
        let st = nf_core::gamepad::PadState { buttons: 5, axes: [-7, 0, 0, 0, 0, 0] };
        c.cola.push(WireEvent::gamepad(3, &st, 0));
        let lista = vec![nf_core::gamepad::PadInfo { id: 3, kind: nf_core::gamepad::PadKind::Generic, name: String::from("M") }];
        ponlo(&mut c, EV_GAMEPADS, 0, 0, 0, encode(&lista));
        // Un mensaje roto no llega a la app ni la tumba.
        ponlo(&mut c, EV_DROP, 0, 0, 0, vec![1, 2]);
        ponlo(&mut c, EV_GAMEPADS, 0, 0, 0, vec![9, 9]);
        c.cola.push(WireEvent { kind: EV_KEY, a: 0, d: 'a' as u32, ..Default::default() });
        ejecutar(&mut c, Box::new(DatosApp(d.clone())), None);
        assert_eq!(
            d.borrow().vistos,
            [
                "dialogo Text { tag: 4, value: Some(\"v\") }",
                "menu 5 9",
                "drop 3 4 [\"C:\\\\x\"] true",
                "open C:\\z",
                "over Some((10, 2)) [\"C:\\\\x\"]",
                "over None [\"C:\\\\x\"]",
                "origen 7 8",
                "origen 0 0",
                "mando 3 5 -7",
                "mandos [PadInfo { id: 3, kind: Generic, name: \"M\" }]",
            ]
        );
        // Respuestas de arrastre (ninguna al salir), y lo que pidió la tecla, tras ellas.
        let r = &c.pedidos;
        assert_eq!(r[0], Request::DragAnswer(true));
        assert_eq!(r[1], Request::DragSourceAnswer(Some(vec![String::from("C:\\o")])));
        assert_eq!(r[2], Request::DragSourceAnswer(None));
        assert!(matches!(r[3], Request::OpenNew(_)) && matches!(r[4], Request::OpenTerminal(_)));
        assert!(matches!(r[5], Request::Dialog(_)) && matches!(r[6], Request::Menu(_)) && matches!(r[7], Request::Drag(_)));
        assert_eq!(r[8], Request::SetTheme(Theme { accent: 0x010203, wallpaper: 2, letra: 0 }));
        assert_eq!(r.len(), 9);
        assert!(c.datos.is_empty());
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

    /// App que, con cada tecla, pide al sistema un archivo y datos del equipo, y lo apunta.
    struct PideApp(Rc<RefCell<Vec<(bool, (u32, u32))>>>);
    impl App for PideApp {
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
        fn key(&mut self, _: &KeyEvent, cx: &mut Ctx) {
            let existe = cx.sys.fs().exists(r"C:\a.txt");
            self.0.borrow_mut().push((existe, cx.sys.screen()));
        }
        fn mouse(&mut self, _: MouseEv, _: &mut Ctx) {}
        fn tick(&mut self, _: &mut Ctx) {}
    }

    #[test]
    fn archivos_y_equipo_van_por_el_servicio() {
        let _g = SERIE.lock();
        let visto = Rc::new(RefCell::new(Vec::new()));
        let tecla = WireEvent { kind: EV_KEY, a: 0, d: 'p' as u32, ..Default::default() };
        // Con respuesta: `0` + valor. Todas las llamadas reciben los mismos bytes, que para un
        // `bool` (1) valen y para la pantalla (8 bytes) no: ahí sale el valor neutro.
        let mut c = Falso { cola: arranque(), respuesta: vec![0, 1], ..Default::default() };
        c.cola.push(tecla);
        ejecutar(&mut c, Box::new(PideApp(visto.clone())), None);
        assert_eq!(*visto.borrow(), [(true, (1280, 720))]);
        // Las lecturas de la letra del sistema (al llegar el tema) no cuentan.
        let pedidas: Vec<Call> = c.servicios.iter().map(|r| nf_core::wire::decode(r).unwrap()).filter(|c| !matches!(c, Call::Read { path } if path.contains("fuentes"))).collect();
        assert!(matches!(&pedidas[0], Call::Exists { path } if path == r"C:\a.txt"));
        assert!(matches!(pedidas[1], Call::Screen));
        // Sin respuesta (el núcleo la rechaza): valores neutros, sin pánico.
        visto.borrow_mut().clear();
        let mut c = Falso { cola: arranque(), ..Default::default() };
        c.cola.push(tecla);
        ejecutar(&mut c, Box::new(PideApp(visto.clone())), None);
        assert_eq!(*visto.borrow(), [(false, (1280, 720))]);
    }

    #[test]
    fn configuracion_pide_cambiar_el_fondo() {
        let _g = SERIE.lock();
        let mut c = Falso::default();
        c.cola.push(WireEvent { kind: EV_THEME, a: 0, d: 0x1E78D7, ..Default::default() });
        c.cola.push(ev(EV_RESIZE, 780, 520, 0));
        c.cola.push(ev(EV_FOCUS, 1, 0, 0));
        c.cola.push(ev(EV_MOUSE_MOVE, 444, 120, 1));
        c.cola.push(ev(EV_MOUSE_DOWN, 444, 120, 2));
        c.cola.push(ev(EV_MOUSE_UP, 444, 120, 3));
        ejecutar(&mut c, nf_core::apps::create(AppKind::Settings), None);
        assert!(c.pedidos.iter().any(|r| matches!(r, Request::SetTheme(t) if t.wallpaper == 1)), "{:?}", c.pedidos);
    }
}
