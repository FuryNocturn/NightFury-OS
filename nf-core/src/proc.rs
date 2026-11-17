//! Programas en modo usuario vistos desde el escritorio (F3, v0.8). CONTRATO: `Regions`,
//! `UserMem`, `SysState`, `Outbox`, `Sys`, `dispatch`, `Process`, `RunEnd`, `Kill` y `ProcApp`.
//!
//! Reparto: el núcleo crea el proceso (tablas de páginas, carga del `.nfx`) e implementa
//! `Process`; cada `syscall` que no sea de espera la atiende llamando a `dispatch`. `ProcApp` es
//! la ventana del escritorio: convierte cada evento de `App` en un `WireEvent`, se lo pasa al
//! programa con `Process::run` y aplica lo que el programa pidió (imagen, título, cerrar).
use crate::abi::*;
use crate::apps::{App, AppKind, Ctx, MouseEv};
use crate::icons::Icon;
use crate::input::KeyEvent;
use crate::platform::{DateTime, Platform};
use crate::surface::Surface;
use crate::theme::Theme;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ---------- Memoria del programa ----------

/// Zonas mapeadas de un programa, para validar punteros antes de tocarlos.
#[derive(Clone, Default, Debug)]
pub struct Regions {
    list: Vec<(u64, u64, bool)>,
}

impl Regions {
    /// Añade `[start, end)` (lectura, y escritura si `writable`). No comprueba solapes: el
    /// núcleo solo añade zonas que acaba de mapear.
    pub fn add(&mut self, start: u64, end: u64, writable: bool) {
        if start < end {
            self.list.push((start, end, writable));
        }
    }

    /// ¿`[addr, addr+len)` está entero dentro de la zona del programa y mapeado (y con escritura
    /// si `write`)? `len = 0` vale si `addr` está en la zona del programa.
    pub fn check(&self, addr: u64, len: u64, write: bool) -> bool {
        let Some(end) = addr.checked_add(len) else { return false };
        if addr < USER_BASE || end > USER_TOP {
            return false;
        }
        let mut cur = addr;
        while cur < end {
            match self.list.iter().find(|&&(s, e, w)| s <= cur && cur < e && (w || !write)) {
                Some(&(_, e, _)) => cur = e,
                None => return false,
            }
        }
        true
    }
}

/// Acceso a la memoria del programa desde el núcleo. Cada método comprueba `Regions` primero y
/// devuelve `false` (sin tocar nada) si el rango no es válido.
pub trait UserMem {
    /// ¿Se puede leer (o escribir, con `write`) `[addr, addr+len)` entero? No toca nada.
    fn check(&self, addr: u64, len: u64, write: bool) -> bool;
    fn read(&mut self, addr: u64, buf: &mut [u8]) -> bool;
    fn write(&mut self, addr: u64, data: &[u8]) -> bool;
    /// Mapea páginas nuevas a cero con lectura y escritura en `[start, end)` (alineados a página).
    /// `false` si no hay memoria.
    fn map_zeroed(&mut self, start: u64, end: u64) -> bool;
}

// ---------- Estado de las llamadas ----------

/// Lo que el programa ha pedido desde la última entrega; `ProcApp` lo recoge y lo vacía.
#[derive(Default)]
pub struct Outbox {
    /// Última imagen entregada con `PRESENT`.
    pub frame: Option<Surface>,
    pub title: Option<String>,
    pub min_size: Option<(i32, i32)>,
    /// Líneas de `LOG`; el núcleo las saca por el puerto serie y las vacía.
    pub log: Vec<String>,
}

/// Estado de un programa que necesitan las llamadas.
#[derive(Default)]
pub struct SysState {
    /// Final del montón (empieza en `HEAP_BASE`).
    pub brk: u64,
    /// Memoria de la imagen y la pila, para el límite `MAX_MEM`.
    pub fixed_mem: u64,
    pub now_ms: u64,
    pub datetime: DateTime,
    /// `SET_TIMER`: cuándo toca el siguiente `EV_TIMER`.
    pub timer_at: Option<u64>,
    pub out: Outbox,
}

impl SysState {
    pub fn new(fixed_mem: u64) -> Self {
        SysState { brk: HEAP_BASE, fixed_mem, ..Default::default() }
    }
}

/// Resultado de `dispatch`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sys {
    /// Volver al programa con este valor en `rax`.
    Ret(i64),
    /// `WAIT_EVENT`: el puntero ya está validado para escribir `WIRE_EVENT_SIZE` bytes. El
    /// núcleo vuelve a quien llamó a `run`; el siguiente `run` escribe ahí el evento y vuelve al
    /// programa con `rax = 0`.
    Wait { ptr: u64, timeout_ms: u64 },
    Exit(i32),
}

/// Atiende una llamada. `args` = `rdi, rsi, rdx, r10, r8`.
pub fn dispatch(nr: u64, args: [u64; 5], mem: &mut dyn UserMem, st: &mut SysState) -> Sys {
    let [a0, a1, a2, a3, _] = args;
    let r = match nr {
        SYS_EXIT => return Sys::Exit(a0 as i32),
        SYS_WAIT_EVENT => {
            let mut probe = [0u8; WIRE_EVENT_SIZE];
            // Se valida leyendo y reescribiendo lo mismo: así el fallo se ve ahora y no al entregar.
            if !mem.read(a0, &mut probe) || !mem.write(a0, &probe) {
                E_FAULT
            } else {
                return Sys::Wait { ptr: a0, timeout_ms: a1 };
            }
        }
        SYS_LOG => match read_text(mem, a0, a1, LOG_MAX) {
            Ok(t) => {
                st.out.log.push(t);
                0
            }
            Err(e) => e,
        },
        SYS_SET_TITLE => match read_text(mem, a0, a1, TITLE_MAX) {
            Ok(t) => {
                st.out.title = Some(t);
                0
            }
            Err(e) => e,
        },
        SYS_PRESENT => present(mem, st, a0, a1, a2, a3),
        SYS_MEM_GROW => mem_grow(mem, st, a0),
        SYS_UPTIME => st.now_ms as i64,
        SYS_DATETIME => {
            let d = st.datetime;
            let y = d.year.to_le_bytes();
            let b = [y[0], y[1], d.month, d.day, d.hour, d.minute, d.second, 0];
            if mem.write(a0, &b) {
                0
            } else {
                E_FAULT
            }
        }
        SYS_SET_TIMER => {
            st.timer_at = if a0 == 0 { None } else { Some(st.now_ms.saturating_add(a0)) };
            0
        }
        SYS_SET_MIN_SIZE => {
            if a0 > MAX_DIM || a1 > MAX_DIM {
                E_INVAL
            } else {
                st.out.min_size = Some((a0 as i32, a1 as i32));
                0
            }
        }
        _ => E_NOSYS,
    };
    Sys::Ret(r)
}

fn read_text(mem: &mut dyn UserMem, ptr: u64, len: u64, max: usize) -> Result<String, i64> {
    if len > max as u64 {
        return Err(E_INVAL);
    }
    let mut b = vec![0u8; len as usize];
    if !mem.read(ptr, &mut b) {
        return Err(E_FAULT);
    }
    String::from_utf8(b).map_err(|_| E_INVAL)
}

fn present(mem: &mut dyn UserMem, st: &mut SysState, ptr: u64, w: u64, h: u64, stride: u64) -> i64 {
    if w > MAX_DIM || h > MAX_DIM || stride < w || stride > MAX_DIM {
        return E_INVAL;
    }
    // Antes de reservar nada en el núcleo: todas las filas tienen que estar en su memoria.
    if h > 0 && !(0..h).all(|y| ptr.checked_add(y * stride * 4).is_some_and(|a| mem.check(a, w * 4, false))) {
        return E_FAULT;
    }
    let mut s = Surface::new(w as i32, h as i32, 0xFFFFFF);
    let mut row = vec![0u8; w as usize * 4];
    for y in 0..h {
        // No desborda: stride, h <= MAX_DIM.
        let Some(addr) = ptr.checked_add(y * stride * 4) else { return E_FAULT };
        if !mem.read(addr, &mut row) {
            return E_FAULT;
        }
        let line = &mut s.px[(y * w) as usize..((y + 1) * w) as usize];
        for (p, c) in line.iter_mut().zip(row.chunks_exact(4)) {
            *p = u32::from_le_bytes([c[0], c[1], c[2], c[3]]) & 0xFF_FFFF;
        }
    }
    st.out.frame = Some(s);
    0
}

fn mem_grow(mem: &mut dyn UserMem, st: &mut SysState, bytes: u64) -> i64 {
    let old = st.brk;
    if bytes == 0 {
        return old as i64;
    }
    let Some(add) = bytes.checked_add(PAGE - 1).map(|v| v / PAGE * PAGE) else { return E_NOMEM };
    let Some(new) = old.checked_add(add) else { return E_NOMEM };
    let used = (new - HEAP_BASE).saturating_add(st.fixed_mem);
    if used > MAX_MEM || new > STACK_TOP - STACK_SIZE || !mem.map_zeroed(old, new) {
        return E_NOMEM;
    }
    st.brk = new;
    old as i64
}

// ---------- Proceso ----------

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Kill {
    /// Fallo del procesador en modo usuario (p. ej. "fallo de página en 0x…").
    Fault(String),
    /// Ha usado el procesador demasiado tiempo sin volver a esperar.
    Hung,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RunEnd {
    /// Espera el siguiente evento (`WAIT_EVENT`). `deadline` = cuándo vence la espera
    /// (`now_ms + timeout_ms`), o `None` sin límite.
    Waiting { deadline: Option<u64> },
    Exited(i32),
    /// El núcleo lo ha terminado; su memoria ya está liberada.
    Killed(Kill),
}

/// Un programa cargado, visto desde el escritorio. Lo implementa el núcleo (modo usuario real)
/// y las pruebas (un programa simulado).
pub trait Process {
    /// Entrega `ev` como resultado de la espera pendiente y ejecuta el programa hasta que vuelve
    /// a esperar, termina o lo matan. La primera vez arranca desde el punto de entrada y entrega
    /// `ev` en su primera espera. Usa `st` y `dispatch` para las llamadas. Tras `Exited` o
    /// `Killed` no se vuelve a llamar.
    fn run(&mut self, ev: &WireEvent, st: &mut SysState) -> RunEnd;
}

/// Ventana de un programa dentro del escritorio.
pub struct ProcApp {
    proc: Box<dyn Process>,
    st: SysState,
    name: String,
    title: String,
    icon: Option<Icon>,
    min: (i32, i32),
    frame: Option<Surface>,
    /// Último tamaño, foco y tema enviados al programa.
    sent_size: Option<(i32, i32)>,
    sent_focus: Option<bool>,
    sent_theme: Option<Theme>,
    deadline: Option<u64>,
    /// `Some` cuando el programa ha terminado (código) o lo han matado.
    end: Option<RunEnd>,
    close_asked: bool,
    /// Cerrar en el próximo evento con `Ctx` (cuando el programa termina dentro de `draw`).
    close_pending: bool,
}

impl ProcApp {
    pub fn new(proc: Box<dyn Process>, fixed_mem: u64, name: &str, icon: Option<Icon>) -> Self {
        ProcApp {
            proc,
            st: SysState::new(fixed_mem),
            name: String::from(name),
            title: String::from(name),
            icon,
            min: (120, 60),
            frame: None,
            sent_size: None,
            sent_focus: None,
            sent_theme: None,
            deadline: None,
            end: None,
            close_asked: false,
            close_pending: false,
        }
    }

    /// Cómo terminó el programa (`None` si sigue vivo).
    pub fn ended(&self) -> Option<&RunEnd> {
        self.end.as_ref()
    }

    /// Entrega un evento. Devuelve `true` si el programa ha terminado.
    fn deliver(&mut self, mut ev: WireEvent, sys: &mut dyn Platform) -> bool {
        if self.end.is_some() {
            return true;
        }
        self.st.now_ms = sys.uptime_ms();
        self.st.datetime = sys.now();
        ev.now_ms = self.st.now_ms;
        let r = self.proc.run(&ev, &mut self.st);
        if let Some(f) = self.st.out.frame.take() {
            self.frame = Some(f);
        }
        if let Some(t) = self.st.out.title.take() {
            self.title = t;
        }
        if let Some(m) = self.st.out.min_size.take() {
            self.min = m;
        }
        match r {
            RunEnd::Waiting { deadline } => {
                self.deadline = deadline;
                false
            }
            other => {
                self.end = Some(other);
                true
            }
        }
    }

    /// Envía tema, tamaño y foco si han cambiado.
    fn sync(&mut self, w: i32, h: i32, theme: &Theme, focused: bool, sys: &mut dyn Platform) -> bool {
        let mut done = false;
        if self.sent_theme != Some(*theme) {
            self.sent_theme = Some(*theme);
            let ev = WireEvent { kind: EV_THEME, a: theme.wallpaper as i32, d: theme.accent, ..Default::default() };
            done |= self.deliver(ev, sys);
        }
        if self.sent_size != Some((w, h)) {
            self.sent_size = Some((w, h));
            done |= self.deliver(WireEvent { kind: EV_RESIZE, a: w, b: h, ..Default::default() }, sys);
        }
        if self.sent_focus != Some(focused) {
            self.sent_focus = Some(focused);
            done |= self.deliver(WireEvent { kind: EV_FOCUS, a: focused as i32, ..Default::default() }, sys);
        }
        done
    }

    fn after(&mut self, ended: bool, cx: &mut Ctx) {
        if ended || self.close_pending {
            cx.close = true;
        }
        cx.redraw = true;
    }
}

impl App for ProcApp {
    fn kind(&self) -> AppKind {
        AppKind::Program
    }
    fn title(&self) -> String {
        self.title.clone()
    }
    fn icon(&self) -> Option<Icon> {
        self.icon
    }
    fn min_size(&self) -> (i32, i32) {
        self.min
    }
    fn draw(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, focused: bool) {
        if self.sync(s.w, s.h, theme, focused, sys) {
            self.close_pending = true;
        }
        s.fill_rect(crate::geom::Rect::new(0, 0, s.w, s.h), 0xFFFFFF);
        if let Some(f) = &self.frame {
            s.blit(f, 0, 0);
        }
    }
    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
        let e = self.deliver(WireEvent::key(ev, 0), cx.sys);
        self.after(e, cx);
    }
    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
        let e = self.deliver(WireEvent::mouse(ev, 0), cx.sys);
        self.after(e, cx);
    }
    fn tick(&mut self, cx: &mut Ctx) {
        if self.close_pending || self.end.is_some() {
            cx.close = true;
            return;
        }
        let now = cx.now_ms;
        if self.st.timer_at.is_some_and(|t| now >= t) {
            self.st.timer_at = None;
            let e = self.deliver(WireEvent::new(EV_TIMER, 0), cx.sys);
            self.after(e, cx);
        } else if self.deadline.is_some_and(|t| now >= t) {
            self.deadline = None;
            let e = self.deliver(WireEvent::new(EV_NONE, 0), cx.sys);
            self.after(e, cx);
        }
    }
    fn request_close(&mut self, cx: &mut Ctx) -> bool {
        // Segunda vez (el programa no se ha cerrado) o ya terminado: se cierra la ventana sin más.
        if self.close_asked || self.end.is_some() {
            return true;
        }
        self.close_asked = true;
        let e = self.deliver(WireEvent::new(EV_CLOSE, 0), cx.sys);
        self.after(e, cx);
        e
    }
}

impl ProcApp {
    /// Nombre del programa (de su `app.txt`), aunque el título cambie.
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::platform::TestPlatform;

    /// Memoria de programa simulada, byte a byte (lo no escrito vale 0).
    pub struct FakeMem {
        pub regions: Regions,
        pub bytes: alloc::collections::BTreeMap<u64, u8>,
    }

    impl FakeMem {
        pub fn new() -> Self {
            FakeMem { regions: Regions::default(), bytes: Default::default() }
        }
    }

    impl UserMem for FakeMem {
        fn check(&self, addr: u64, len: u64, write: bool) -> bool {
            self.regions.check(addr, len, write)
        }
        fn read(&mut self, addr: u64, buf: &mut [u8]) -> bool {
            if !self.regions.check(addr, buf.len() as u64, false) {
                return false;
            }
            for (i, b) in buf.iter_mut().enumerate() {
                *b = self.bytes.get(&(addr + i as u64)).copied().unwrap_or(0);
            }
            true
        }
        fn write(&mut self, addr: u64, data: &[u8]) -> bool {
            if !self.regions.check(addr, data.len() as u64, true) {
                return false;
            }
            for (i, b) in data.iter().enumerate() {
                self.bytes.insert(addr + i as u64, *b);
            }
            true
        }
        fn map_zeroed(&mut self, start: u64, end: u64) -> bool {
            // Simulación: solo valida; el contenido se crea a ceros al tocarlo.
            if end - start > 64 << 20 {
                return false;
            }
            self.regions.add(start, end, true);
            true
        }
    }

    const D: u64 = USER_BASE + 0x10_0000;

    fn mem() -> FakeMem {
        let mut m = FakeMem::new();
        m.regions.add(D, D + 0x4000, true);
        m.regions.add(D + 0x4000, D + 0x5000, false);
        m
    }

    #[test]
    fn regiones() {
        let m = mem();
        assert!(m.regions.check(D, 0x5000, false));
        assert!(!m.regions.check(D, 0x5000, true), "la última página es de solo lectura");
        assert!(m.regions.check(D + 0x3FFF, 1, true));
        assert!(!m.regions.check(D + 0x4FFF, 2, false), "se sale por el final");
        assert!(!m.regions.check(D - 1, 1, false));
        assert!(!m.regions.check(u64::MAX - 1, 4, false), "desbordamiento");
        assert!(!m.regions.check(0x1000, 0, false), "dirección del núcleo, aunque sea de 0 bytes");
        assert!(m.regions.check(D + 10, 0, true));
    }

    #[test]
    fn llamadas_validan_punteros() {
        let mut m = mem();
        let mut st = SysState::new(0x10_0000);
        // Título y registro.
        m.write(D, "Hola ñ".as_bytes());
        assert_eq!(dispatch(SYS_SET_TITLE, [D, 7, 0, 0, 0], &mut m, &mut st), Sys::Ret(0));
        assert_eq!(st.out.title.as_deref(), Some("Hola ñ"));
        assert_eq!(dispatch(SYS_SET_TITLE, [0x1000, 4, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_FAULT));
        assert_eq!(dispatch(SYS_SET_TITLE, [D, 9999, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_INVAL));
        m.write(D, &[0xFF, 0xFE]);
        assert_eq!(dispatch(SYS_LOG, [D, 2, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_INVAL), "UTF-8 roto");
        // Espera: el búfer tiene que poder escribirse.
        assert_eq!(dispatch(SYS_WAIT_EVENT, [D, 5, 0, 0, 0], &mut m, &mut st), Sys::Wait { ptr: D, timeout_ms: 5 });
        assert_eq!(dispatch(SYS_WAIT_EVENT, [D + 0x4000, 5, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_FAULT));
        assert_eq!(dispatch(SYS_WAIT_EVENT, [D + 0x3FF0, 5, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_FAULT), "medio fuera");
        // Hora en zona de solo lectura: falla.
        assert_eq!(dispatch(SYS_DATETIME, [D + 0x4000, 0, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_FAULT));
        assert_eq!(dispatch(SYS_DATETIME, [D, 0, 0, 0, 0], &mut m, &mut st), Sys::Ret(0));
        assert_eq!(dispatch(99, [0; 5], &mut m, &mut st), Sys::Ret(E_NOSYS));
        assert_eq!(dispatch(SYS_EXIT, [3, 0, 0, 0, 0], &mut m, &mut st), Sys::Exit(3));
    }

    #[test]
    fn present_copia_la_imagen() {
        let mut m = mem();
        let mut st = SysState::new(0);
        // 2x2 con stride 3.
        let px: [u32; 6] = [1, 2, 0xAA, 3, 0xFF00_0004, 0xBB];
        let b: Vec<u8> = px.iter().flat_map(|p| p.to_le_bytes()).collect();
        m.write(D, &b);
        assert_eq!(dispatch(SYS_PRESENT, [D, 2, 2, 3, 0], &mut m, &mut st), Sys::Ret(0));
        assert_eq!(st.out.frame.as_ref().unwrap().px, vec![1, 2, 3, 4]);
        assert_eq!(dispatch(SYS_PRESENT, [D, 3, 2, 2, 0], &mut m, &mut st), Sys::Ret(E_INVAL), "stride < w");
        assert_eq!(dispatch(SYS_PRESENT, [D, 9000, 1, 9000, 0], &mut m, &mut st), Sys::Ret(E_INVAL));
        assert_eq!(dispatch(SYS_PRESENT, [D, 64, 100, 64, 0], &mut m, &mut st), Sys::Ret(E_FAULT), "se sale de lo mapeado");
        assert_eq!(dispatch(SYS_PRESENT, [u64::MAX - 8, 2, 2, 2, 0], &mut m, &mut st), Sys::Ret(E_FAULT));
    }

    #[test]
    fn montón_con_limite() {
        let mut m = FakeMem::new();
        let mut st = SysState::new(2 << 20);
        assert_eq!(dispatch(SYS_MEM_GROW, [0; 5], &mut m, &mut st), Sys::Ret(HEAP_BASE as i64));
        assert_eq!(dispatch(SYS_MEM_GROW, [10, 0, 0, 0, 0], &mut m, &mut st), Sys::Ret(HEAP_BASE as i64));
        assert_eq!(st.brk, HEAP_BASE + PAGE);
        assert!(m.write(HEAP_BASE + 100, &[1]));
        assert!(!m.write(HEAP_BASE + PAGE, &[1]));
        assert_eq!(dispatch(SYS_MEM_GROW, [MAX_MEM, 0, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_NOMEM));
        assert_eq!(dispatch(SYS_MEM_GROW, [u64::MAX, 0, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_NOMEM));
        assert_eq!(st.brk, HEAP_BASE + PAGE, "los fallos no mueven el final");
    }

    /// Programa simulado: guarda los eventos que recibe y reacciona como le digan.
    struct Fake {
        seen: alloc::rc::Rc<core::cell::RefCell<Vec<WireEvent>>>,
        mem: FakeMem,
    }

    impl Process for Fake {
        fn run(&mut self, ev: &WireEvent, st: &mut SysState) -> RunEnd {
            self.seen.borrow_mut().push(*ev);
            match ev.kind {
                EV_RESIZE => {
                    let w = ev.a.min(4) as u64;
                    let px = vec![0x11u8; (w * 4) as usize];
                    self.mem.write(D, &px);
                    assert_eq!(dispatch(SYS_PRESENT, [D, w, 1, w, 0], &mut self.mem, st), Sys::Ret(0));
                    RunEnd::Waiting { deadline: None }
                }
                EV_KEY => {
                    self.mem.write(D, b"Nuevo");
                    dispatch(SYS_SET_TITLE, [D, 5, 0, 0, 0], &mut self.mem, st);
                    dispatch(SYS_SET_TIMER, [50, 0, 0, 0, 0], &mut self.mem, st);
                    RunEnd::Waiting { deadline: None }
                }
                EV_MOUSE_DOWN => RunEnd::Killed(Kill::Fault(String::from("fallo de página"))),
                EV_CLOSE => RunEnd::Exited(0),
                _ => RunEnd::Waiting { deadline: None },
            }
        }
    }

    fn fake() -> (ProcApp, alloc::rc::Rc<core::cell::RefCell<Vec<WireEvent>>>) {
        let seen = alloc::rc::Rc::new(core::cell::RefCell::new(Vec::new()));
        let p = Fake { seen: seen.clone(), mem: mem() };
        (ProcApp::new(Box::new(p), 0, "Prueba", None), seen)
    }

    #[test]
    fn ventana_de_programa() {
        let (mut app, seen) = fake();
        let mut sys = TestPlatform::default();
        let mut theme = Theme::default();
        let mut s = Surface::new(10, 5, 0);
        app.draw(&mut s, &theme, &mut sys, true);
        let kinds: Vec<u32> = seen.borrow().iter().map(|e| e.kind).collect();
        assert_eq!(kinds, [EV_THEME, EV_RESIZE, EV_FOCUS]);
        assert_eq!(seen.borrow()[1].a, 10);
        assert_eq!(&s.px[..5], &[0x111111, 0x111111, 0x111111, 0x111111, 0xFFFFFF], "imagen del programa y fondo");
        // Mismo tamaño y foco: no se reenvía nada.
        app.draw(&mut s, &theme, &mut sys, true);
        assert_eq!(seen.borrow().len(), 3);
        // Tecla: cambia el título y pide temporizador.
        let k = KeyEvent { key: crate::input::Key::Char('a'), ch: Some('a'), down: true, shift: false, ctrl: false, alt: false };
        let mut cx = Ctx::new(&mut sys, &mut theme, 0);
        app.key(&k, &mut cx);
        assert!(cx.redraw && !cx.close);
        assert_eq!(app.title(), "Nuevo");
        drop(cx);
        sys.ms = 10;
        let mut cx = Ctx::new(&mut sys, &mut theme, 10);
        app.tick(&mut cx);
        assert_eq!(seen.borrow().len(), 4, "aún no toca el temporizador");
        drop(cx);
        sys.ms = 60;
        let mut cx = Ctx::new(&mut sys, &mut theme, 60);
        app.tick(&mut cx);
        assert_eq!(seen.borrow().last().unwrap().kind, EV_TIMER);
        // Cerrar: el programa sale por sí mismo.
        assert!(app.request_close(&mut cx), "ha salido al pedírselo: se cierra ya");
        assert_eq!(app.ended(), Some(&RunEnd::Exited(0)));
    }

    #[test]
    fn programa_que_falla_cierra_su_ventana() {
        let (mut app, _) = fake();
        let mut sys = TestPlatform::default();
        let mut theme = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut theme, 0);
        app.mouse(MouseEv::Down { x: 1, y: 1 }, &mut cx);
        assert!(cx.close);
        assert!(matches!(app.ended(), Some(RunEnd::Killed(Kill::Fault(_)))));
        // Ya no se le entrega nada más y cerrar no espera.
        assert!(app.request_close(&mut cx));
    }
}
