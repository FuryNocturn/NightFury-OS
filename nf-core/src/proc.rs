// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Programas en modo usuario vistos desde el escritorio (F3, v0.9). CONTRATO: `Regions`,
//! `UserMem`, `SysState` (y sus `post`, `next_event`, `wake_at`, `not_responding`), `Outbox`,
//! `Sys`, `dispatch`, `Process`, `Status`, `Kill`, `ProcInfo` y `ProcApp`.
//!
//! Reparto (v0.9, multitarea): el núcleo crea el proceso e implementa `Process`; el programa corre
//! por su cuenta y cada `syscall` la atiende `dispatch`. `WAIT_EVENT` la resuelve el núcleo con
//! `SysState::next_event`: si no hay nada, el programa se duerme hasta `wake_at` o hasta que llegue
//! un evento. `ProcApp` es la ventana: deja eventos en la cola (`SysState::post`) y en cada tick
//! recoge lo que el programa pidió (imagen, título, guardar) y su `status`.
use crate::abi::*;
use crate::apps::{App, AppKind, Ctx, DragRequest, MouseEv};
use crate::dialogs::{Answer, Buttons, DialogRequest, DialogResult};
use crate::files::{self, Std};
use crate::geom::Rect;
use crate::icons::Icon;
use crate::input::KeyEvent;
use crate::platform::{DateTime, Platform};
use crate::request::Request;
use crate::service::serve;
use crate::surface::{blend, Surface};
use crate::theme::Theme;
use crate::wire::{decode, encode};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::format;
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
    /// `SAVE_AS`: nombre propuesto y contenido. La ventana enseña el diálogo Guardar.
    pub save: Option<(String, Vec<u8>)>,
    /// `REQUEST`: peticiones ya decodificadas, hasta `REQUESTS_MAX`. `ProcApp` las atiende en orden.
    pub requests: VecDeque<Request>,
}

/// Peticiones `REQUEST` que pueden esperar a que el escritorio las recoja; las demás dan `E_BUSY`.
pub const REQUESTS_MAX: usize = 64;
/// Los programas pueden pedir diálogos con cualquier etiqueta salvo las que tienen este bit: el
/// escritorio lo usa para distinguir los diálogos del programa de los suyos.
const TAG_PROGRAMA: u32 = 1 << 31;
/// Una petición `SERVICE` espera respuesta: tiempo que `ProcApp::tick` dedica como máximo a atender
/// peticiones seguidas antes de devolver el turno al escritorio.
const SERVICE_SLICE_MS: u64 = 8;

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
    /// Eventos que el escritorio ha dejado y el programa no ha recogido (`now_ms` = cuándo llegó).
    pub queue: VecDeque<WireEvent>,
    /// Datos de los eventos de `queue` que los llevan (`lleva_datos`), en el mismo orden.
    pub payload: VecDeque<Vec<u8>>,
    /// Datos del último evento entregado (`EVENT_DATA`).
    pub event_data: Vec<u8>,
    pub status: Status,
    /// `SERVICE` sin atender (el programa está parado esperando) y la respuesta que el escritorio
    /// deja para despertarlo; `TAKE_REPLY` la recoge.
    pub service: Option<Vec<u8>>,
    pub reply: Option<Vec<u8>>,
}

/// ¿Este evento lleva datos (su `d` es la longitud)?
pub fn lleva_datos(kind: u32) -> bool {
    matches!(kind, EV_DIALOG | EV_DRAG_OVER | EV_DROP | EV_OPEN)
}

/// Eventos que caben en la cola de un programa; lo que llega con la cola llena se descarta.
pub const QUEUE_MAX: usize = 256;

impl SysState {
    pub fn new(fixed_mem: u64) -> Self {
        SysState { brk: HEAP_BASE, fixed_mem, ..Default::default() }
    }

    /// Deja un evento para el programa. Los movimientos de ratón seguidos se juntan en uno.
    pub fn post(&mut self, ev: WireEvent) {
        if let Some(last) = self.queue.back_mut() {
            if last.kind == EV_MOUSE_MOVE && ev.kind == EV_MOUSE_MOVE {
                *last = WireEvent { now_ms: last.now_ms, ..ev };
                return;
            }
        }
        // shortcut: con la cola llena se pierde lo nuevo; si molesta, descartar solo movimientos.
        if self.queue.len() < QUEUE_MAX {
            self.queue.push_back(ev);
        }
    }

    /// Deja un evento con datos (`lleva_datos`); `d` pasa a ser su longitud.
    pub fn post_data(&mut self, mut ev: WireEvent, data: Vec<u8>) {
        if self.queue.len() < QUEUE_MAX {
            ev.d = data.len() as u32;
            self.queue.push_back(ev);
            self.payload.push_back(data);
        }
    }

    /// Lo siguiente que recibe un `WAIT_EVENT` en el instante `now` con ese plazo: un evento de la
    /// cola, `EV_TIMER` si ha vencido el temporizador, `EV_NONE` si ha vencido el plazo. `None` =
    /// hay que dormir (hasta `wake_at` o hasta que llegue un evento).
    pub fn next_event(&mut self, now: u64, deadline: Option<u64>) -> Option<WireEvent> {
        self.event_data.clear();
        if let Some(e) = self.queue.pop_front() {
            if lleva_datos(e.kind) {
                self.event_data = self.payload.pop_front().unwrap_or_default();
            }
            return Some(e);
        }
        if self.timer_at.is_some_and(|t| now >= t) {
            self.timer_at = None;
            return Some(WireEvent::new(EV_TIMER, now));
        }
        deadline.filter(|&d| now >= d).map(|_| WireEvent::new(EV_NONE, now))
    }

    /// Cuándo hay que despertar a un programa dormido en `WAIT_EVENT` aunque no lleguen eventos.
    pub fn wake_at(&self, deadline: Option<u64>) -> Option<u64> {
        match (self.timer_at, deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// ¿Lleva `HUNG_MS` o más sin recoger el evento más viejo?
    pub fn not_responding(&self, now: u64) -> bool {
        self.queue.front().is_some_and(|e| now.saturating_sub(e.now_ms) >= HUNG_MS)
    }
}

/// Resultado de `dispatch`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sys {
    /// Volver al programa con este valor en `rax`.
    Ret(i64),
    /// `WAIT_EVENT`: el puntero ya está validado para escribir `WIRE_EVENT_SIZE` bytes. El núcleo
    /// usa `SysState::next_event` (plazo = `now + timeout_ms` salvo `u64::MAX`); escribe ahí el
    /// evento y vuelve con `rax = 0`, o duerme al programa hasta tener uno.
    Wait { ptr: u64, timeout_ms: u64 },
    /// `SERVICE`: la petición ya está en `SysState::service`. El núcleo duerme al programa hasta que
    /// haya `SysState::reply` y vuelve con `rax` = su longitud.
    WaitReply,
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
        SYS_SAVE_AS => save_as(mem, st, a0, a1, a2, a3),
        SYS_REQUEST => request(mem, st, a0, a1),
        SYS_EVENT_DATA => {
            if (st.event_data.len() as u64) > a1 {
                E_INVAL
            } else if mem.write(a0, &st.event_data) {
                st.event_data.len() as i64
            } else {
                E_FAULT
            }
        }
        SYS_SERVICE => match service(mem, st, a0, a1) {
            Ok(()) => return Sys::WaitReply,
            Err(e) => e,
        },
        SYS_TAKE_REPLY => take_reply(mem, st, a0, a1),
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

fn save_as(mem: &mut dyn UserMem, st: &mut SysState, ptr: u64, len: u64, name_ptr: u64, name_len: u64) -> i64 {
    if st.out.save.is_some() {
        return E_BUSY;
    }
    if len > SAVE_MAX {
        return E_INVAL;
    }
    let name = match read_text(mem, name_ptr, name_len, 255) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if !files::valid_name(&name) {
        return E_INVAL;
    }
    // Validado antes de reservar en el núcleo.
    if !mem.check(ptr, len, false) {
        return E_FAULT;
    }
    let mut data = vec![0u8; len as usize];
    if !mem.read(ptr, &mut data) {
        return E_FAULT;
    }
    st.out.save = Some((name, data));
    0
}

fn request(mem: &mut dyn UserMem, st: &mut SysState, ptr: u64, len: u64) -> i64 {
    if st.out.requests.len() >= REQUESTS_MAX {
        return E_BUSY;
    }
    if len > REQUEST_MAX {
        return E_INVAL;
    }
    // Validado antes de reservar en el núcleo.
    if !mem.check(ptr, len, false) {
        return E_FAULT;
    }
    let mut b = vec![0u8; len as usize];
    if !mem.read(ptr, &mut b) {
        return E_FAULT;
    }
    let Ok(mut r) = decode::<Request>(&b) else { return E_INVAL };
    if let Request::Dialog(d) = &mut r {
        if *tag_de(d) & TAG_PROGRAMA != 0 {
            return E_INVAL;
        }
    }
    st.out.requests.push_back(r);
    0
}

fn tag_de(d: &mut DialogRequest) -> &mut u32 {
    match d {
        DialogRequest::Message { tag, .. }
        | DialogRequest::Input { tag, .. }
        | DialogRequest::Password { tag, .. }
        | DialogRequest::Open { tag, .. }
        | DialogRequest::Save { tag, .. }
        | DialogRequest::Folder { tag, .. }
        | DialogRequest::Entropy { tag, .. } => tag,
    }
}

fn tag_resultado(r: &mut DialogResult) -> &mut u32 {
    match r {
        DialogResult::Answer { tag, .. } | DialogResult::Text { tag, .. } | DialogResult::Path { tag, .. } => tag,
    }
}

fn service(mem: &mut dyn UserMem, st: &mut SysState, ptr: u64, len: u64) -> Result<(), i64> {
    if st.service.is_some() {
        return Err(E_BUSY);
    }
    if len > SERVICE_MAX {
        return Err(E_INVAL);
    }
    // Validado antes de reservar en el núcleo.
    if !mem.check(ptr, len, false) {
        return Err(E_FAULT);
    }
    let mut req = vec![0u8; len as usize];
    if !mem.read(ptr, &mut req) {
        return Err(E_FAULT);
    }
    st.reply = None;
    st.service = Some(req);
    Ok(())
}

fn take_reply(mem: &mut dyn UserMem, st: &mut SysState, ptr: u64, cap: u64) -> i64 {
    let Some(r) = &st.reply else { return E_INVAL };
    let len = r.len() as u64;
    if len > cap {
        return E_INVAL;
    }
    if !mem.check(ptr, len, true) || !mem.write(ptr, r) {
        return E_FAULT;
    }
    st.reply = None;
    len as i64
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
    /// Lo ha finalizado el usuario (Administrador de tareas o "no responde").
    Ended,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum Status {
    #[default]
    Alive,
    Exited(i32),
    /// El núcleo lo ha terminado; su memoria ya está liberada.
    Killed(Kill),
}

/// Datos de un programa para el Administrador de tareas.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProcInfo {
    pub id: u64,
    pub name: String,
    /// Memoria que ocupa (imagen, pila y montón).
    pub mem: u64,
    /// Tiempo de procesador usado desde que arrancó.
    pub cpu_ms: u64,
    pub responding: bool,
}

/// Un programa en marcha, visto desde el escritorio (v0.9: se ejecuta por su cuenta). Lo
/// implementa el núcleo y las pruebas. El escritorio y el programa comparten `SysState`: el
/// núcleo garantiza que no se usa a la vez (un solo procesador; el escritorio no se interrumpe
/// para dar paso a un programa y las llamadas corren con las interrupciones cerradas).
/// Soltar el `Process` termina el programa si sigue vivo.
pub trait Process {
    fn id(&self) -> u64;
    /// Estado compartido: cola de eventos (`post`), lo pedido (`out`) y `status`.
    fn state(&mut self) -> &mut SysState;
    /// Termina el programa ya (queda `Killed(Kill::Ended)`) y libera su memoria. Sin efecto si ya terminó.
    fn kill(&mut self);
    /// Memoria que ocupa ahora.
    fn mem(&self) -> u64;
    fn cpu_ms(&self) -> u64;
}

/// Tiempo sin recoger eventos a partir del cual un programa "no responde".
pub const HUNG_MS: u64 = 5000;

const T_SAVE: u32 = 1;
const T_CRASH: u32 = 2;
const T_HUNG: u32 = 3;

/// Ventana de un programa dentro del escritorio.
pub struct ProcApp {
    proc: Box<dyn Process>,
    name: String,
    title: String,
    icon: Option<Icon>,
    min: (i32, i32),
    frame: Option<Surface>,
    /// Último tamaño, foco y tema enviados al programa.
    sent_size: Option<(i32, i32)>,
    sent_focus: Option<bool>,
    sent_theme: Option<Theme>,
    /// Ya se ha avisado del fin (fallo) y se espera a que el usuario acepte.
    noticed: bool,
    close_asked: bool,
    hung: bool,
    /// Datos de un `SAVE_AS` mientras el diálogo Guardar está abierto.
    saving: Option<Vec<u8>>,
    /// Peticiones recogidas del programa y aún sin pasar al `Ctx` (el escritorio atiende un diálogo,
    /// un menú, etc. a la vez).
    pend: VecDeque<Request>,
    /// Hay un diálogo del programa abierto, sin respuesta.
    dlg_open: bool,
    /// Últimas respuestas de arrastre (`DragAnswer`, `DragSourceAnswer`) y la última conocida de `drag_over`.
    over_ans: Option<bool>,
    src_ans: Option<Option<Vec<String>>>,
    last_over: bool,
    /// Es uno de los programas que trae el sistema (`Platform::is_system_binary`): solo ellos
    /// pueden usar las llamadas de cuentas, claves, cifrado, reiniciar y apagar de `SERVICE`.
    system: bool,
}

/// Cuánto espera `drag_over`/`drag_source` la respuesta del programa, y cuántas veces como mucho
/// cede el procesador a los programas mientras tanto (el reloj puede no avanzar en las pruebas).
const DRAG_WAIT_MS: u64 = 5;
const DRAG_SPINS: u32 = 100_000;

impl ProcApp {
    pub fn new(proc: Box<dyn Process>, name: &str, icon: Option<Icon>, system: bool) -> Self {
        ProcApp {
            proc,
            name: String::from(name),
            title: String::from(name),
            icon,
            min: (120, 60),
            frame: None,
            sent_size: None,
            sent_focus: None,
            sent_theme: None,
            noticed: false,
            close_asked: false,
            hung: false,
            saving: None,
            pend: VecDeque::new(),
            dlg_open: false,
            over_ans: None,
            src_ans: None,
            last_over: false,
            system,
        }
    }

    /// Nombre del programa (de su `app.txt`), aunque el título cambie.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn status(&mut self) -> Status {
        self.proc.state().status.clone()
    }

    fn post(&mut self, mut ev: WireEvent, now: u64) {
        ev.now_ms = now;
        self.proc.state().post(ev);
    }

    fn post_data<T: crate::wire::Wire>(&mut self, ev: WireEvent, data: &T, now: u64) {
        self.proc.state().post_data(WireEvent { now_ms: now, ..ev }, encode(data));
    }

    /// Pasa las peticiones `REQUEST` del programa a `pend` y guarda las respuestas de arrastre.
    fn recoger(&mut self) {
        let st = self.proc.state();
        while let Some(r) = st.out.requests.front() {
            let respuesta = matches!(r, Request::DragAnswer(_) | Request::DragSourceAnswer(_));
            if !respuesta && self.pend.len() >= REQUESTS_MAX {
                break;
            }
            match st.out.requests.pop_front() {
                Some(Request::DragAnswer(b)) => self.over_ans = Some(b),
                Some(Request::DragSourceAnswer(v)) => self.src_ans = Some(v),
                Some(r) => self.pend.push_back(r),
                None => {}
            }
        }
    }

    /// Pasa al `Ctx` las peticiones pendientes, en orden. Cada casilla del `Ctx` admite una: si
    /// está ocupada, esa petición y las que siguen esperan al próximo turno.
    fn atender(&mut self, cx: &mut Ctx) {
        while let Some(r) = self.pend.front() {
            let ocupado = match r {
                Request::Open(_) => cx.open.is_some(),
                Request::OpenNew(_) => cx.open_new.is_some(),
                Request::OpenTerminal(_) => cx.open_terminal.is_some(),
                Request::Dialog(_) => cx.dialog.is_some() || self.dlg_open || self.saving.is_some(),
                Request::Menu(_) => cx.menu.is_some(),
                Request::Drag(_) => cx.drag.is_some(),
                _ => false,
            };
            if ocupado {
                break;
            }
            match self.pend.pop_front() {
                Some(Request::Open(p)) => cx.open = Some(p),
                Some(Request::OpenNew(p)) => cx.open_new = Some(p),
                Some(Request::OpenTerminal(p)) => cx.open_terminal = Some(p),
                Some(Request::Dialog(mut d)) => {
                    *tag_de(&mut d) |= TAG_PROGRAMA;
                    self.dlg_open = true;
                    cx.dialog = Some(d);
                }
                Some(Request::Menu(m)) => cx.menu = Some(m),
                Some(Request::Drag(paths)) => cx.drag = Some(DragRequest { paths }),
                Some(Request::SetTheme(t)) => {
                    *cx.theme = t;
                    cx.theme_changed = true;
                }
                _ => {}
            }
        }
    }

    /// Cede el procesador a los programas hasta que `listo` diga que ha llegado la respuesta o pasen
    /// `DRAG_WAIT_MS`. `true` si llegó.
    fn esperar(&mut self, cx: &mut Ctx, listo: impl Fn(&Self) -> bool) -> bool {
        let t0 = cx.sys.uptime_ms();
        for _ in 0..DRAG_SPINS {
            cx.sys.run_programs();
            self.recoger();
            if listo(self) {
                return true;
            }
            if cx.sys.uptime_ms().saturating_sub(t0) >= DRAG_WAIT_MS || self.proc.state().status != Status::Alive {
                break;
            }
        }
        false
    }

    /// Atiende las peticiones `SERVICE` del programa: contesta, deja correr a los programas para que
    /// se despierte y repite si pide otra, hasta `SERVICE_SLICE_MS` (el resto, en el siguiente tick).
    fn serve(&mut self, cx: &mut Ctx) {
        let t0 = cx.sys.uptime_ms();
        while self.proc.state().status == Status::Alive {
            let Some(req) = self.proc.state().service.take() else { break };
            let mut r = serve(cx.sys, &req, self.system);
            if r.len() as u64 > SERVICE_MAX {
                r = vec![1];
            }
            self.proc.state().reply = Some(r);
            cx.sys.run_programs();
            if cx.sys.uptime_ms().saturating_sub(t0) >= SERVICE_SLICE_MS {
                break;
            }
        }
    }

    /// Recoge lo que el programa ha pedido y mira si sigue vivo y respondiendo.
    fn poll(&mut self, cx: &mut Ctx) {
        let now = cx.now_ms;
        self.recoger();
        let st = self.proc.state();
        let out = core::mem::take(&mut st.out);
        let status = st.status.clone();
        let hung = st.status == Status::Alive && st.not_responding(now);
        if let Some(f) = out.frame {
            self.frame = Some(f);
            cx.redraw = true;
        }
        if let Some(t) = out.title {
            self.title = t;
        }
        if let Some(m) = out.min_size {
            self.min = m;
        }
        if hung != self.hung {
            self.hung = hung;
            cx.redraw = true;
        }
        if let Some((name, data)) = out.save {
            if self.saving.is_none() && !self.dlg_open && cx.dialog.is_none() {
                let dir = cx.sys.files().std_dir(if name.to_lowercase().ends_with(".bmp") { Std::Pictures } else { Std::Documents });
                cx.dialog = Some(DialogRequest::Save { tag: T_SAVE, title: String::from("Guardar como"), dir, name });
                self.saving = Some(data);
            } else {
                self.post(WireEvent { kind: EV_SAVED, a: -1, ..Default::default() }, now);
            }
        }
        if !matches!(status, Status::Killed(_)) {
            self.atender(cx);
        }
        match status {
            Status::Alive => {}
            Status::Exited(_) | Status::Killed(Kill::Ended) => cx.close = true,
            Status::Killed(Kill::Fault(why)) => {
                if !self.noticed && cx.dialog.is_none() {
                    self.noticed = true;
                    let text = format!("«{}» dejó de funcionar y se ha cerrado.\nMotivo: {why}", self.name);
                    cx.dialog = Some(DialogRequest::Message { tag: T_CRASH, title: self.name.clone(), text, buttons: Buttons::Ok });
                }
            }
        }
    }
}

impl App for ProcApp {
    fn kind(&self) -> AppKind {
        AppKind::Program
    }
    fn title(&self) -> String {
        if self.hung {
            format!("{} (No responde)", self.title)
        } else {
            self.title.clone()
        }
    }
    fn icon(&self) -> Option<Icon> {
        self.icon
    }
    fn min_size(&self) -> (i32, i32) {
        self.min
    }
    fn draw(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, focused: bool) {
        let now = sys.uptime_ms();
        if self.sent_theme != Some(*theme) {
            self.sent_theme = Some(*theme);
            self.post(WireEvent { kind: EV_THEME, a: theme.wallpaper as i32, d: theme.accent, ..Default::default() }, now);
        }
        if self.sent_size != Some((s.w, s.h)) {
            self.sent_size = Some((s.w, s.h));
            self.post(WireEvent { kind: EV_RESIZE, a: s.w, b: s.h, ..Default::default() }, now);
        }
        if self.sent_focus != Some(focused) {
            self.sent_focus = Some(focused);
            self.post(WireEvent { kind: EV_FOCUS, a: focused as i32, ..Default::default() }, now);
        }
        s.fill_rect(Rect::new(0, 0, s.w, s.h), 0xFFFFFF);
        if let Some(f) = &self.frame {
            s.blit(f, 0, 0);
        }
        // Sin respuesta, el contenido se ve apagado (como en Windows).
        if self.hung || self.noticed {
            for p in s.px.iter_mut() {
                *p = blend(*p, 0xFFFFFF, 140);
            }
        }
    }
    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
        self.post(WireEvent::key(ev, 0), cx.now_ms);
    }
    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
        self.post(WireEvent::mouse(ev, 0), cx.now_ms);
        // Un arrastre empieza con un `Move`: se deja correr al programa en seguida para que su
        // `Request::Drag` llegue mientras el botón sigue pulsado (si esperara al tick, se perdería
        // al soltar rápido).
        if matches!(ev, MouseEv::Move { .. }) {
            cx.sys.run_programs();
            self.recoger();
            self.atender(cx);
        }
    }
    fn tick(&mut self, cx: &mut Ctx) {
        self.serve(cx);
        self.poll(cx);
    }
    fn dialog_result(&mut self, result: DialogResult, cx: &mut Ctx) {
        match result {
            DialogResult::Path { tag: T_SAVE, path } => {
                let data = self.saving.take().unwrap_or_default();
                let a = match path {
                    None => 0,
                    Some(p) => {
                        let now = cx.sys.now();
                        match cx.sys.files().write(&p, &data, now) {
                            Ok(()) => 1,
                            Err(e) => {
                                let text = format!("No se puede guardar «{}».\n{}", files::file_name(&p), e.message());
                                cx.dialog = Some(DialogRequest::Message { tag: 0, title: self.name.clone(), text, buttons: Buttons::Ok });
                                -1
                            }
                        }
                    }
                };
                self.post(WireEvent { kind: EV_SAVED, a, ..Default::default() }, cx.now_ms);
            }
            DialogResult::Answer { tag: T_CRASH, .. } => cx.close = true,
            DialogResult::Answer { tag: T_HUNG, answer: Answer::Yes } => {
                self.proc.kill();
                cx.close = true;
            }
            // Diálogo del programa: vuelve con su etiqueta original.
            mut r => {
                let tag = tag_resultado(&mut r);
                if *tag & TAG_PROGRAMA != 0 {
                    *tag &= !TAG_PROGRAMA;
                    self.dlg_open = false;
                    self.post_data(WireEvent::new(EV_DIALOG, 0), &r, cx.now_ms);
                }
            }
        }
    }
    fn menu_result(&mut self, tag: u32, id: u32, cx: &mut Ctx) {
        self.post(WireEvent { kind: EV_MENU, a: tag as i32, b: id as i32, ..Default::default() }, cx.now_ms);
    }
    fn drop_files(&mut self, x: i32, y: i32, paths: &[String], copy: bool, cx: &mut Ctx) {
        let ev = WireEvent { kind: EV_DROP, a: x, b: y, c: copy as i32, ..Default::default() };
        self.post_data(ev, &paths.to_vec(), cx.now_ms);
    }
    fn open_path(&mut self, path: &str, cx: &mut Ctx) {
        self.post_data(WireEvent::new(EV_OPEN, 0), &String::from(path), cx.now_ms);
    }
    /// Pregunta al programa y espera su respuesta un momento (la llamada del escritorio es síncrona).
    /// Si no llega a tiempo, vale la última respuesta conocida.
    fn drag_over(&mut self, pos: Option<(i32, i32)>, paths: &[String], cx: &mut Ctx) -> bool {
        let (x, y) = pos.unwrap_or((0, 0));
        let ev = WireEvent { kind: EV_DRAG_OVER, a: x, b: y, c: pos.is_some() as i32, ..Default::default() };
        self.post_data(ev, &paths.to_vec(), cx.now_ms);
        if pos.is_none() {
            self.last_over = false;
            return false;
        }
        self.over_ans = None;
        if self.esperar(cx, |s| s.over_ans.is_some()) {
            self.last_over = self.over_ans.take().unwrap_or(false);
        }
        self.atender(cx);
        self.last_over
    }
    fn drag_source(&mut self, x: i32, y: i32, cx: &mut Ctx) -> Option<Vec<String>> {
        self.post(WireEvent { kind: EV_DRAG_SOURCE, a: x, b: y, ..Default::default() }, cx.now_ms);
        self.src_ans = None;
        let ok = self.esperar(cx, |s| s.src_ans.is_some());
        self.atender(cx);
        if ok { self.src_ans.take().flatten() } else { None }
    }
    fn request_close(&mut self, cx: &mut Ctx) -> bool {
        if self.proc.state().status != Status::Alive {
            return true;
        }
        if self.hung {
            let text = format!("«{}» no responde.\n¿Quieres finalizarlo? Se perderá lo que no esté guardado.", self.name);
            self.dlg_open = false;
            cx.dialog = Some(DialogRequest::Message { tag: T_HUNG, title: self.name.clone(), text, buttons: Buttons::YesNo });
            return false;
        }
        // Segunda vez sin que el programa se haya cerrado: se cierra la ventana (y el programa con ella).
        if self.close_asked {
            return true;
        }
        self.close_asked = true;
        self.post(WireEvent::new(EV_CLOSE, 0), cx.now_ms);
        false
    }
}

impl Drop for ProcApp {
    fn drop(&mut self) {
        self.proc.kill();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::menu::MenuRequest;
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

    /// Programa simulado: el estado compartido sin nada que lo ejecute; la prueba hace de programa.
    struct Fake {
        st: alloc::rc::Rc<core::cell::RefCell<SysState>>,
        killed: alloc::rc::Rc<core::cell::Cell<bool>>,
    }

    impl Process for Fake {
        fn id(&self) -> u64 {
            1
        }
        fn state(&mut self) -> &mut SysState {
            // Solo en pruebas: la prueba y la ventana se turnan, nunca a la vez.
            unsafe { &mut *self.st.as_ptr() }
        }
        fn kill(&mut self) {
            self.killed.set(true);
            let mut st = self.st.borrow_mut();
            if st.status == Status::Alive {
                st.status = Status::Killed(Kill::Ended);
            }
        }
        fn mem(&self) -> u64 {
            0
        }
        fn cpu_ms(&self) -> u64 {
            0
        }
    }

    type Shared = alloc::rc::Rc<core::cell::RefCell<SysState>>;

    fn fake() -> (ProcApp, Shared, alloc::rc::Rc<core::cell::Cell<bool>>) {
        fake_con(false)
    }

    fn fake_con(system: bool) -> (ProcApp, Shared, alloc::rc::Rc<core::cell::Cell<bool>>) {
        let st: Shared = Default::default();
        let killed = alloc::rc::Rc::new(core::cell::Cell::new(false));
        let p = Fake { st: st.clone(), killed: killed.clone() };
        (ProcApp::new(Box::new(p), "Prueba", None, system), st, killed)
    }

    /// `TestPlatform` donde cada vez que se deja correr a los programas pasan 5 ms y el programa
    /// pide otra cosa (mientras le queden `pide`).
    struct Corre {
        inner: TestPlatform,
        sh: Shared,
        pide: u32,
        corridas: u32,
    }

    impl Platform for Corre {
        fn now(&mut self) -> DateTime {
            self.inner.now()
        }
        fn uptime_ms(&self) -> u64 {
            self.inner.ms
        }
        fn mem(&self) -> crate::platform::MemInfo {
            self.inner.mem()
        }
        fn cpu(&self) -> String {
            self.inner.cpu()
        }
        fn screen(&self) -> (u32, u32) {
            self.inner.screen()
        }
        fn reboot(&mut self) {
            self.inner.reboot()
        }
        fn shutdown(&mut self) {
            self.inner.shutdown()
        }
        fn files(&mut self) -> &mut files::Files {
            self.inner.files()
        }
        fn entropy(&mut self) -> u64 {
            self.inner.entropy()
        }
        fn pool(&mut self) -> &mut crate::keys::Pool {
            self.inner.pool()
        }
        fn run_programs(&mut self) {
            self.corridas += 1;
            self.inner.ms += 5;
            if self.pide > 0 {
                self.pide -= 1;
                let mut st = self.sh.borrow_mut();
                st.reply = None;
                st.service = Some(crate::wire::encode(&crate::service::Call::Cpu));
            }
        }
    }

    #[test]
    fn service_valida_y_espera_respuesta() {
        let mut m = mem();
        let mut st = SysState::default();
        let req = crate::wire::encode(&crate::service::Call::Cpu);
        m.write(D, &req);
        let n = req.len() as u64;
        assert_eq!(dispatch(SYS_SERVICE, [D, n, 0, 0, 0], &mut m, &mut st), Sys::WaitReply);
        assert_eq!(st.service.as_deref(), Some(&req[..]));
        assert_eq!(dispatch(SYS_SERVICE, [D, n, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_BUSY), "una a la vez");
        assert_eq!(dispatch(SYS_TAKE_REPLY, [D, 64, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_INVAL), "aún sin respuesta");
        st.service = None;
        assert_eq!(dispatch(SYS_SERVICE, [D, SERVICE_MAX + 1, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_INVAL));
        assert_eq!(dispatch(SYS_SERVICE, [0x1000, 4, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_FAULT));
        assert_eq!(dispatch(SYS_SERVICE, [D + 0x4FF0, 0x100, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_FAULT));
        assert!(st.service.is_none(), "lo rechazado no queda pendiente");
        // TAKE_REPLY: poco sitio y memoria de solo lectura no la gastan; bien, la recoge.
        st.reply = Some(vec![0, 1, 2, 3]);
        assert_eq!(dispatch(SYS_TAKE_REPLY, [D, 3, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_INVAL));
        assert_eq!(dispatch(SYS_TAKE_REPLY, [D + 0x4000, 4, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_FAULT));
        assert_eq!(dispatch(SYS_TAKE_REPLY, [0x1000, 4, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_FAULT));
        assert!(st.reply.is_some());
        assert_eq!(dispatch(SYS_TAKE_REPLY, [D + 8, 4, 0, 0, 0], &mut m, &mut st), Sys::Ret(4));
        let mut got = [0u8; 4];
        m.read(D + 8, &mut got);
        assert_eq!(got, [0, 1, 2, 3]);
        assert!(st.reply.is_none());
        assert_eq!(dispatch(SYS_TAKE_REPLY, [D, 4, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_INVAL), "ya recogida");
    }

    #[test]
    fn la_ventana_atiende_el_servicio() {
        use crate::service::Call;
        let (mut app, sh, _) = fake();
        let mut sys = TestPlatform::default();
        let mut theme = Theme::default();
        sh.borrow_mut().service = Some(crate::wire::encode(&Call::Cpu));
        app.tick(&mut Ctx::new(&mut sys, &mut theme, 5));
        let mut esperada = vec![0];
        esperada.extend(crate::wire::encode(&String::from("CPU de pruebas")));
        assert_eq!(sh.borrow().reply, Some(esperada));
        assert!(sh.borrow().service.is_none());
        // Una petición rota recibe el error de siempre; no entra en pánico.
        sh.borrow_mut().service = Some(vec![250]);
        app.tick(&mut Ctx::new(&mut sys, &mut theme, 6));
        assert_eq!(sh.borrow().reply, Some(vec![1]));
        // Reiniciar solo para los programas del sistema (marcados al abrirlos, no por el programa).
        let (mut ajeno, sh, _) = fake_con(false);
        sh.borrow_mut().service = Some(crate::wire::encode(&Call::Reboot));
        ajeno.tick(&mut Ctx::new(&mut sys, &mut theme, 7));
        assert!(!sys.rebooted);
        let (mut propio, sh, _) = fake_con(true);
        sh.borrow_mut().service = Some(crate::wire::encode(&Call::Reboot));
        propio.tick(&mut Ctx::new(&mut sys, &mut theme, 8));
        assert!(sys.rebooted);
        assert_eq!(sh.borrow().reply, Some(vec![0]));
    }

    #[test]
    fn el_servicio_repite_con_tope_de_tiempo() {
        use crate::service::Call;
        let (mut app, sh, _) = fake();
        let mut theme = Theme::default();
        // El programa pide una vez más: se atienden las dos seguidas, y al final no queda nada.
        let mut sys = Corre { inner: TestPlatform::default(), sh: sh.clone(), pide: 1, corridas: 0 };
        sh.borrow_mut().service = Some(crate::wire::encode(&Call::Cpu));
        app.tick(&mut Ctx::new(&mut sys, &mut theme, 5));
        assert_eq!(sys.corridas, 2);
        assert!(sh.borrow().service.is_none() && sh.borrow().reply.is_some());
        // Si no para de pedir, se corta a los 8 ms (2 vueltas de 5 ms) y lo demás espera al siguiente tick.
        let mut sys = Corre { inner: TestPlatform::default(), sh: sh.clone(), pide: 1000, corridas: 0 };
        sh.borrow_mut().service = Some(crate::wire::encode(&Call::Cpu));
        app.tick(&mut Ctx::new(&mut sys, &mut theme, 6));
        assert_eq!(sys.corridas, 2);
        assert!(sh.borrow().service.is_some(), "pendiente para el siguiente tick");
    }

    fn kinds(st: &Shared) -> Vec<u32> {
        st.borrow().queue.iter().map(|e| e.kind).collect()
    }

    #[test]
    fn cola_de_eventos() {
        let mut st = SysState::default();
        for i in 0..3 {
            st.post(WireEvent { kind: EV_MOUSE_MOVE, a: i, now_ms: 10, ..Default::default() });
        }
        assert_eq!(st.queue.len(), 1, "los movimientos seguidos se juntan");
        assert_eq!(st.queue[0].a, 2);
        st.post(WireEvent { kind: EV_KEY, now_ms: 20, ..Default::default() });
        assert!(!st.not_responding(10 + HUNG_MS - 1));
        assert!(st.not_responding(10 + HUNG_MS), "el evento más viejo lleva 5 s sin recoger");
        assert_eq!(st.next_event(30, None).unwrap().kind, EV_MOUSE_MOVE);
        assert_eq!(st.next_event(30, None).unwrap().kind, EV_KEY);
        assert!(!st.not_responding(1 << 40), "cola vacía: responde");
        // Temporizador y plazo de la espera.
        st.timer_at = Some(100);
        assert_eq!(st.next_event(99, Some(200)), None);
        assert_eq!(st.next_event(100, Some(200)).unwrap().kind, EV_TIMER);
        assert_eq!(st.timer_at, None);
        assert_eq!(st.next_event(150, Some(200)), None);
        assert_eq!(st.next_event(200, Some(200)).unwrap().kind, EV_NONE);
        assert_eq!(st.wake_at(Some(500)), Some(500));
        st.timer_at = Some(300);
        assert_eq!(st.wake_at(Some(500)), Some(300));
        // Cola llena: lo nuevo se descarta, sin crecer.
        for _ in 0..QUEUE_MAX + 10 {
            st.post(WireEvent { kind: EV_KEY, ..Default::default() });
        }
        assert_eq!(st.queue.len(), QUEUE_MAX);
    }

    #[test]
    fn guardar_como_con_dialogo() {
        let mut m = mem();
        let mut st = SysState::default();
        m.write(D, b"hola");
        m.write(D + 100, b"dibujo.bmp");
        assert_eq!(dispatch(SYS_SAVE_AS, [D, 4, D + 100, 10, 0], &mut m, &mut st), Sys::Ret(0));
        assert_eq!(st.out.save, Some((String::from("dibujo.bmp"), b"hola".to_vec())));
        assert_eq!(dispatch(SYS_SAVE_AS, [D, 4, D + 100, 3, 0], &mut m, &mut st), Sys::Ret(E_BUSY), "uno a la vez");
        st.out.save = None;
        assert_eq!(dispatch(SYS_SAVE_AS, [D, 4, D + 100, 0, 0], &mut m, &mut st), Sys::Ret(E_INVAL), "sin nombre");
        assert_eq!(dispatch(SYS_SAVE_AS, [D, 4, D + 100, 4, 0], &mut m, &mut st), Sys::Ret(0));
        st.out.save = None;
        m.write(D + 100, b"a\\b");
        assert_eq!(dispatch(SYS_SAVE_AS, [D, 4, D + 100, 3, 0], &mut m, &mut st), Sys::Ret(E_INVAL), "el nombre no es una ruta");
        assert_eq!(dispatch(SYS_SAVE_AS, [D, SAVE_MAX + 1, D + 100, 4, 0], &mut m, &mut st), Sys::Ret(E_INVAL));
        m.write(D + 100, b"abcd");
        assert_eq!(dispatch(SYS_SAVE_AS, [D + 0x4FF0, 0x100, D + 100, 4, 0], &mut m, &mut st), Sys::Ret(E_FAULT));

        // La ventana enseña el diálogo y escribe con los permisos de la sesión.
        let (mut app, sh, _) = fake();
        let mut sys = TestPlatform::default();
        let mut theme = Theme::default();
        sh.borrow_mut().out.save = Some((String::from("dibujo.bmp"), b"BM..".to_vec()));
        let mut cx = Ctx::new(&mut sys, &mut theme, 5);
        app.tick(&mut cx);
        let Some(DialogRequest::Save { dir, name, .. }) = cx.dialog.take() else { panic!("sin diálogo") };
        assert!(dir.ends_with("Imágenes") && name == "dibujo.bmp");
        let p = format!("{dir}\\dibujo.bmp");
        app.dialog_result(DialogResult::Path { tag: T_SAVE, path: Some(p.clone()) }, &mut cx);
        drop(cx);
        assert_eq!(sys.files.read(&p).unwrap(), b"BM..");
        assert_eq!(sh.borrow().queue.back().map(|e| (e.kind, e.a)), Some((EV_SAVED, 1)));
        // Cancelar: el programa recibe 0. Sin permiso: -1 y un mensaje.
        let mut cx = Ctx::new(&mut sys, &mut theme, 6);
        app.dialog_result(DialogResult::Path { tag: T_SAVE, path: None }, &mut cx);
        assert_eq!(sh.borrow().queue.back().map(|e| e.a), Some(0));
        app.dialog_result(DialogResult::Path { tag: T_SAVE, path: Some(String::from("C:\\Usuarios\\x.bmp")) }, &mut cx);
        assert_eq!(sh.borrow().queue.back().map(|e| e.a), Some(-1));
        assert!(cx.dialog.is_some());
    }

    fn pide(m: &mut FakeMem, st: &mut SysState, r: &Request) -> Sys {
        let b = encode(r);
        m.write(D, &b);
        dispatch(SYS_REQUEST, [D, b.len() as u64, 0, 0, 0], m, st)
    }

    #[test]
    fn request_valida_y_encola() {
        let mut m = mem();
        let mut st = SysState::default();
        assert_eq!(pide(&mut m, &mut st, &Request::Open(String::from("C:\\"))), Sys::Ret(0));
        assert_eq!(st.out.requests.len(), 1);
        let dlg = |tag| Request::Dialog(DialogRequest::Message { tag, title: String::new(), text: String::new(), buttons: Buttons::Ok });
        assert_eq!(pide(&mut m, &mut st, &dlg(5)), Sys::Ret(0));
        assert_eq!(pide(&mut m, &mut st, &dlg(TAG_PROGRAMA)), Sys::Ret(E_INVAL), "ese bit es del escritorio");
        m.write(D, &[99, 1, 2]);
        assert_eq!(dispatch(SYS_REQUEST, [D, 3, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_INVAL), "roto");
        assert_eq!(dispatch(SYS_REQUEST, [D, REQUEST_MAX + 1, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_INVAL));
        assert_eq!(dispatch(SYS_REQUEST, [0x1000, 4, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_FAULT));
        assert_eq!(dispatch(SYS_REQUEST, [D + 0x4FF0, 0x100, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_FAULT));
        assert_eq!(st.out.requests.len(), 2, "lo rechazado no se encola");
        while st.out.requests.len() < REQUESTS_MAX {
            assert_eq!(pide(&mut m, &mut st, &Request::DragAnswer(true)), Sys::Ret(0));
        }
        assert_eq!(pide(&mut m, &mut st, &Request::DragAnswer(true)), Sys::Ret(E_BUSY));
    }

    #[test]
    fn event_data_acompana_al_evento() {
        let mut m = mem();
        let mut st = SysState::default();
        st.post_data(WireEvent::new(EV_OPEN, 1), encode(&String::from("C:\\a")));
        st.post(WireEvent::new(EV_KEY, 1));
        let get = |m: &mut FakeMem, st: &mut SysState, cap| dispatch(SYS_EVENT_DATA, [D, cap, 0, 0, 0], m, st);
        assert_eq!(get(&mut m, &mut st, 100), Sys::Ret(0), "antes de recibir nada");
        let e = st.next_event(2, None).unwrap();
        assert_eq!((e.kind, e.d), (EV_OPEN, 8));
        assert_eq!(get(&mut m, &mut st, 7), Sys::Ret(E_INVAL), "no cabe");
        assert_eq!(get(&mut m, &mut st, 100), Sys::Ret(8));
        let mut b = [0u8; 8];
        m.read(D, &mut b);
        assert_eq!(decode::<String>(&b).as_deref(), Ok("C:\\a"));
        assert_eq!(dispatch(SYS_EVENT_DATA, [D + 0x4000, 100, 0, 0, 0], &mut m, &mut st), Sys::Ret(E_FAULT));
        st.next_event(3, None);
        assert_eq!(get(&mut m, &mut st, 100), Sys::Ret(0), "valen hasta el siguiente WAIT_EVENT");
        // Cola llena: el evento con datos se descarta con los suyos.
        for _ in 0..QUEUE_MAX {
            st.post(WireEvent::new(EV_KEY, 1));
        }
        st.post_data(WireEvent::new(EV_OPEN, 1), vec![1]);
        assert!(st.payload.is_empty());
    }

    fn dialogo(tag: u32) -> Request {
        Request::Dialog(DialogRequest::Input { tag, title: String::from("t"), label: String::new(), value: String::new() })
    }

    #[test]
    fn peticiones_pasan_al_ctx_de_una_en_una() {
        let (mut app, sh, _) = fake();
        let mut sys = TestPlatform::default();
        let mut theme = Theme::default();
        {
            let mut s = sh.borrow_mut();
            let q = &mut s.out.requests;
            q.push_back(Request::Open(String::from("C:\\a")));
            q.push_back(Request::OpenNew(String::from("C:\\b")));
            q.push_back(Request::OpenTerminal(String::from("C:\\c")));
            q.push_back(Request::Drag(vec![String::from("C:\\d")]));
            q.push_back(dialogo(7));
            q.push_back(dialogo(8));
            q.push_back(Request::Menu(MenuRequest { tag: 4, x: 1, y: 2, items: vec![] }));
            q.push_back(Request::SetTheme(Theme { accent: 0x123456, wallpaper: 1 }));
            q.push_back(Request::Open(String::from("C:\\e")));
        }
        let mut cx = Ctx::new(&mut sys, &mut theme, 1);
        app.tick(&mut cx);
        assert_eq!(cx.open.take().as_deref(), Some("C:\\a"));
        assert_eq!(cx.open_new.as_deref(), Some("C:\\b"));
        assert_eq!(cx.open_terminal.as_deref(), Some("C:\\c"));
        assert_eq!(cx.drag.as_ref().map(|d| d.paths.len()), Some(1));
        assert!(matches!(cx.dialog.take(), Some(DialogRequest::Input { tag, .. }) if tag == 7 | TAG_PROGRAMA));
        assert!(cx.menu.is_none() && !cx.theme_changed, "el segundo diálogo y todo lo que sigue esperan");
        // Mientras el primer diálogo siga abierto, el segundo espera.
        app.tick(&mut cx);
        assert!(cx.dialog.is_none() && cx.open.is_none());
        // El programa recibe su respuesta con su etiqueta; entonces sale el segundo.
        app.dialog_result(DialogResult::Text { tag: 7 | TAG_PROGRAMA, value: Some(String::from("hola")) }, &mut cx);
        let d = sh.borrow().payload.back().cloned().unwrap();
        assert_eq!(sh.borrow().queue.back().map(|e| (e.kind, e.d as usize)), Some((EV_DIALOG, d.len())));
        assert_eq!(decode::<DialogResult>(&d), Ok(DialogResult::Text { tag: 7, value: Some(String::from("hola")) }));
        app.tick(&mut cx);
        assert!(matches!(cx.dialog.take(), Some(DialogRequest::Input { tag, .. }) if tag == 8 | TAG_PROGRAMA));
        assert_eq!(cx.menu.take().map(|m| m.tag), Some(4), "el menú no espera al diálogo");
        app.dialog_result(DialogResult::Text { tag: 8 | TAG_PROGRAMA, value: None }, &mut cx);
        app.tick(&mut cx);
        assert!(cx.theme_changed && cx.theme.accent == 0x123456);
        assert_eq!(cx.open.as_deref(), Some("C:\\e"));
        // Sus diálogos internos no se confunden con los del programa (etiquetas 1 a 3).
        let n = sh.borrow().queue.len();
        app.dialog_result(DialogResult::Answer { tag: T_HUNG, answer: Answer::No }, &mut cx);
        assert_eq!(sh.borrow().queue.len(), n);
    }

    #[test]
    fn eventos_con_datos_hacia_el_programa() {
        let (mut app, sh, _) = fake();
        let mut sys = TestPlatform::default();
        let mut theme = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut theme, 9);
        app.menu_result(3, 11, &mut cx);
        let paths = vec![String::from("C:\\x"), String::from("C:\\y")];
        app.drop_files(5, 6, &paths, true, &mut cx);
        app.open_path("C:\\z", &mut cx);
        let s = sh.borrow();
        let ev: Vec<_> = s.queue.iter().map(|e| (e.kind, e.a, e.b, e.c, e.now_ms)).collect();
        assert_eq!(ev, [(EV_MENU, 3, 11, 0, 9), (EV_DROP, 5, 6, 1, 9), (EV_OPEN, 0, 0, 0, 9)]);
        assert_eq!(decode::<Vec<String>>(&s.payload[0]), Ok(paths));
        assert_eq!(decode::<String>(&s.payload[1]).as_deref(), Ok("C:\\z"));
        assert_eq!(s.queue[1].d as usize, s.payload[0].len());
    }

    /// Plataforma que, al ceder el turno a los programas, hace de programa: contesta con `respuesta`.
    struct Contesta {
        p: TestPlatform,
        st: Shared,
        respuesta: Option<Request>,
        cedidas: u32,
    }

    impl Platform for Contesta {
        fn now(&mut self) -> DateTime {
            self.p.now()
        }
        fn uptime_ms(&self) -> u64 {
            self.p.uptime_ms()
        }
        fn mem(&self) -> crate::platform::MemInfo {
            self.p.mem()
        }
        fn cpu(&self) -> String {
            self.p.cpu()
        }
        fn screen(&self) -> (u32, u32) {
            self.p.screen()
        }
        fn reboot(&mut self) {}
        fn shutdown(&mut self) {}
        fn files(&mut self) -> &mut crate::files::Files {
            &mut self.p.files
        }
        fn entropy(&mut self) -> u64 {
            0
        }
        fn pool(&mut self) -> &mut crate::keys::Pool {
            &mut self.p.pool
        }
        fn run_programs(&mut self) {
            self.cedidas += 1;
            if let Some(r) = self.respuesta.take() {
                self.st.borrow_mut().out.requests.push_back(r);
            }
        }
    }

    #[test]
    fn arrastre_espera_la_respuesta_del_programa() {
        let (mut app, sh, _) = fake();
        let mut sys = Contesta { p: TestPlatform::default(), st: sh.clone(), respuesta: Some(Request::DragAnswer(true)), cedidas: 0 };
        let mut theme = Theme::default();
        let paths = vec![String::from("C:\\x")];
        let mut cx = Ctx::new(&mut sys, &mut theme, 1);
        assert!(app.drag_over(Some((3, 4)), &paths, &mut cx));
        let e = *sh.borrow().queue.back().unwrap();
        assert_eq!((e.kind, e.a, e.b, e.c), (EV_DRAG_OVER, 3, 4, 1));
        assert_eq!(decode::<Vec<String>>(sh.borrow().payload.back().unwrap()), Ok(paths.clone()));
        // Sin respuesta a tiempo (el reloj de prueba no avanza: salen los giros): vale la última conocida.
        drop(cx);
        sys.respuesta = None;
        let mut cx = Ctx::new(&mut sys, &mut theme, 2);
        assert!(app.drag_over(Some((5, 4)), &paths, &mut cx), "última respuesta conocida");
        // Al salir se avisa sin esperar y se olvida.
        drop(cx);
        let antes = sys.cedidas;
        let mut cx = Ctx::new(&mut sys, &mut theme, 3);
        assert!(!app.drag_over(None, &paths, &mut cx));
        drop(cx);
        assert_eq!(sys.cedidas, antes);
        assert_eq!(sh.borrow().queue.back().map(|e| (e.kind, e.c)), Some((EV_DRAG_OVER, 0)));
        // Origen del arrastre con el botón derecho; las peticiones que llegan a la vez no se pierden.
        sys.respuesta = Some(Request::DragSourceAnswer(Some(paths.clone())));
        sh.borrow_mut().out.requests.push_back(Request::OpenNew(String::from("C:\\n")));
        let mut cx = Ctx::new(&mut sys, &mut theme, 4);
        assert_eq!(app.drag_source(7, 8, &mut cx), Some(paths));
        assert_eq!(cx.open_new.as_deref(), Some("C:\\n"));
        assert_eq!(sh.borrow().queue.back().map(|e| (e.kind, e.a, e.b)), Some((EV_DRAG_SOURCE, 7, 8)));
        assert_eq!(app.drag_source(1, 1, &mut cx), None, "sin respuesta a tiempo");
    }

    #[test]
    fn ventana_de_programa() {
        let (mut app, sh, killed) = fake();
        let mut sys = TestPlatform::default();
        let mut theme = Theme::default();
        let mut s = Surface::new(10, 5, 0);
        app.draw(&mut s, &theme, &mut sys, true);
        assert_eq!(kinds(&sh), [EV_THEME, EV_RESIZE, EV_FOCUS]);
        app.draw(&mut s, &theme, &mut sys, true);
        assert_eq!(sh.borrow().queue.len(), 3, "sin cambios no se reenvía nada");
        // El "programa" recoge los eventos y presenta una imagen y un título.
        sh.borrow_mut().queue.clear();
        sh.borrow_mut().out.frame = Some(Surface::new(4, 1, 0x111111));
        sh.borrow_mut().out.title = Some(String::from("Nuevo"));
        let mut cx = Ctx::new(&mut sys, &mut theme, 10);
        app.tick(&mut cx);
        assert!(cx.redraw && !cx.close);
        assert_eq!(app.title(), "Nuevo");
        drop(cx);
        app.draw(&mut s, &theme, &mut sys, true);
        assert_eq!(&s.px[..5], &[0x111111, 0x111111, 0x111111, 0x111111, 0xFFFFFF]);
        // Teclas y ratón van a la cola.
        let k = KeyEvent { key: crate::input::Key::Char('a'), ch: Some('a'), down: true, shift: false, ctrl: false, alt: false };
        let mut cx = Ctx::new(&mut sys, &mut theme, 20);
        app.key(&k, &mut cx);
        app.mouse(MouseEv::Down { x: 1, y: 2 }, &mut cx);
        assert_eq!(kinds(&sh), [EV_KEY, EV_MOUSE_DOWN]);
        assert_eq!(sh.borrow().queue[0].now_ms, 20);
        // Cerrar: se le pide al programa; cuando sale, se cierra la ventana.
        assert!(!app.request_close(&mut cx));
        assert_eq!(sh.borrow().queue.back().unwrap().kind, EV_CLOSE);
        sh.borrow_mut().status = Status::Exited(0);
        app.tick(&mut cx);
        assert!(cx.close);
        drop(cx);
        drop(app);
        assert!(killed.get(), "al cerrar la ventana se suelta el programa");
    }

    #[test]
    fn no_responde_y_finalizar() {
        let (mut app, _, killed) = fake();
        let mut sys = TestPlatform::default();
        let mut theme = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut theme, 0);
        app.key(&KeyEvent { key: crate::input::Key::Enter, ch: None, down: true, shift: false, ctrl: false, alt: false }, &mut cx);
        drop(cx);
        let mut cx = Ctx::new(&mut sys, &mut theme, HUNG_MS + 1);
        app.tick(&mut cx);
        assert_eq!(app.title(), "Prueba (No responde)");
        assert!(!app.request_close(&mut cx));
        assert!(matches!(cx.dialog.take(), Some(DialogRequest::Message { tag: T_HUNG, .. })));
        app.dialog_result(DialogResult::Answer { tag: T_HUNG, answer: Answer::Yes }, &mut cx);
        assert!(cx.close && killed.get());
        // Vuelve a recoger eventos: deja de estar marcado.
        let (mut app, sh, _) = fake();
        let mut cx = Ctx::new(&mut sys, &mut theme, 0);
        app.mouse(MouseEv::Down { x: 0, y: 0 }, &mut cx);
        drop(cx);
        let mut cx = Ctx::new(&mut sys, &mut theme, HUNG_MS + 1);
        app.tick(&mut cx);
        assert!(app.title().ends_with("(No responde)"));
        sh.borrow_mut().next_event(HUNG_MS + 2, None);
        app.tick(&mut cx);
        assert_eq!(app.title(), "Prueba");
    }

    #[test]
    fn programa_que_falla_avisa() {
        let (mut app, sh, _) = fake();
        let mut sys = TestPlatform::default();
        let mut theme = Theme::default();
        sh.borrow_mut().status = Status::Killed(Kill::Fault(String::from("fallo de página en 0x0")));
        let mut cx = Ctx::new(&mut sys, &mut theme, 0);
        app.tick(&mut cx);
        assert!(!cx.close, "la ventana espera a que se lea el aviso");
        let Some(DialogRequest::Message { tag, text, .. }) = cx.dialog.take() else { panic!() };
        assert!(tag == T_CRASH && text.contains("dejó de funcionar") && text.contains("fallo de página"));
        app.tick(&mut cx);
        assert!(cx.dialog.is_none(), "un solo aviso");
        app.dialog_result(DialogResult::Answer { tag: T_CRASH, answer: Answer::Ok }, &mut cx);
        assert!(cx.close);
        // Finalizado desde el Administrador de tareas: se cierra sin aviso.
        let (mut app, sh, _) = fake();
        sh.borrow_mut().status = Status::Killed(Kill::Ended);
        let mut cx = Ctx::new(&mut sys, &mut theme, 0);
        app.tick(&mut cx);
        assert!(cx.close && cx.dialog.is_none());
    }
}
