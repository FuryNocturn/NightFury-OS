// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Administrador de tareas: los programas en marcha con su estado, CPU, memoria y red (pestaña
//! «Procesos»), las conexiones de red con su programa («Conexiones») y "Finalizar tarea". CONTRATO: `pub fn new() -> Box<dyn App>`.
use super::{App, AppKind, Ctx, MouseEv};
use crate::dialogs;
use crate::font::{self, Align};
use crate::geom::Rect;
use crate::input::{Key, KeyEvent};
use crate::net::{ip_text, ConnInfo, Proto};
use crate::platform::Platform;
use crate::proc::ProcInfo;
use crate::scrollbar::{Metrics, ScrollBar, SB_W};
use crate::surface::{blend, darken, Surface};
use crate::theme::Theme;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

const TAB_H: i32 = 30;
const TAB_W: i32 = 110;
const HEAD_H: i32 = 28;
const ROW_H: i32 = 26;
const FOOT_H: i32 = 52;
const BTN_W: i32 = 140;
const BTN_H: i32 = 30;
/// Cada cuánto se vuelve a mirar la lista de programas.
const REFRESH_MS: u64 = 1000;
const MB: u64 = 1 << 20;
const INK: u32 = 0x1A1A1A;
const MUTED: u32 = 0x6B6B6B;
const RED: u32 = 0xC62828;
/// Columnas de cada pestaña: título, ancho y alineación; la primera (ancho 0) ocupa el resto.
type Cols = [(&'static str, i32, Align)];
const PROC_COLS: &Cols = &[("Nombre", 0, Align::Left), ("Estado", 130, Align::Left), ("CPU", 70, Align::Right), ("Memoria", 100, Align::Right), ("Red", 120, Align::Right)];
const CONN_COLS: &Cols = &[
    ("Programa", 0, Align::Left),
    ("Protocolo", 70, Align::Left),
    ("Dirección local", 100, Align::Left),
    ("Dirección remota", 150, Align::Left),
    ("Estado", 90, Align::Left),
    ("Recibido", 80, Align::Right),
    ("Enviado", 80, Align::Right),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Procs,
    Conns,
}

struct Row {
    id: u64,
    name: String,
    responding: bool,
    /// % de procesador usado en el último intervalo.
    cpu: u32,
    mem: u64,
    /// Bytes por segundo (rx + tx) en el último intervalo y conexiones abiertas.
    net: u64,
    conns: usize,
}

struct ConnRow {
    proto: Proto,
    id: u32,
    /// Programa dueño (0 = el sistema).
    owner: u64,
    name: String,
    local: u16,
    remote: String,
    state: &'static str,
    rx: u64,
    tx: u64,
}

struct TaskMgr {
    tab: Tab,
    rows: Vec<Row>,
    crows: Vec<ConnRow>,
    /// (id, cpu_ms, bytes de red) de la lectura anterior y cuándo se hizo, para sacar el % de CPU y los KB/s.
    prev: Vec<(u64, u64, u64)>,
    last_ms: Option<u64>,
    next_ms: u64,
    sel: Option<u64>,
    /// Conexión seleccionada en la otra pestaña: (protocolo, id).
    csel: Option<(Proto, u32)>,
    /// Desplazamiento de la lista en píxeles.
    top: i32,
    sb: ScrollBar,
    size: (i32, i32),
    hover_btn: bool,
    down_btn: bool,
}

pub fn new() -> Box<dyn App> {
    Box::new(mgr())
}

fn mgr() -> TaskMgr {
    TaskMgr { tab: Tab::Procs, rows: Vec::new(), crows: Vec::new(), prev: Vec::new(), last_ms: None, next_ms: 0, sel: None, csel: None, top: 0, sb: ScrollBar::new(false), size: (0, 0), hover_btn: false, down_btn: false }
}

/// "12,3 MB" (un decimal, coma decimal).
fn mem_text(bytes: u64) -> String {
    let t = bytes * 10 / MB;
    format!("{},{} MB", t / 10, t % 10)
}

/// "12,3 KB/s" (un decimal, coma decimal).
fn rate_text(bps: u64) -> String {
    let t = bps * 10 / 1024;
    format!("{},{} KB/s", t / 10, t % 10)
}

/// "0 B", "1,2 KB", "3,4 MB".
fn size_text(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{} B", bytes);
    }
    if bytes < MB {
        let t = bytes * 10 / 1024;
        return format!("{},{} KB", t / 10, t % 10);
    }
    mem_text(bytes)
}

fn proto_text(p: Proto) -> &'static str {
    match p {
        Proto::Udp => "UDP",
        Proto::Tls => "TLS",
        _ => "TCP",
    }
}

/// Lo que se lee de la plataforma en cada refresco; sin red, sin conexiones.
fn read(sys: &mut dyn Platform) -> (Vec<ProcInfo>, Vec<ConnInfo>) {
    (sys.processes(), sys.net().map(|n| n.connections()).unwrap_or_default())
}

impl TaskMgr {
    fn cols(&self) -> &'static Cols {
        if self.tab == Tab::Procs {
            PROC_COLS
        } else {
            CONN_COLS
        }
    }

    fn count(&self) -> usize {
        if self.tab == Tab::Procs {
            self.rows.len()
        } else {
            self.crows.len()
        }
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Ctx) {
        if tab != self.tab {
            self.tab = tab;
            self.top = 0;
            cx.redraw = true;
        }
    }

    fn tab_rect(i: i32) -> Rect {
        Rect::new(i * TAB_W, 0, TAB_W, TAB_H)
    }

    /// Lista de la derecha del encabezado hasta el pie.
    fn list_rect(&self) -> Rect {
        let top = TAB_H + HEAD_H;
        Rect::new(0, top, self.size.0, (self.size.1 - top - FOOT_H).max(0))
    }

    fn btn_rect(&self) -> Rect {
        Rect::new(self.size.0 - BTN_W - 14, self.size.1 - FOOT_H + (FOOT_H - BTN_H) / 2, BTN_W, BTN_H)
    }

    fn metrics(&self) -> Metrics {
        let l = self.list_rect();
        Metrics { rect: Rect::new(l.right() - SB_W, l.y, SB_W, l.h), total: self.count() as i32 * ROW_H, page: l.h, pos: self.top, line: ROW_H }
    }

    fn clamp_top(&mut self) {
        self.top = self.top.clamp(0, self.metrics().max());
    }

    /// Ancho de la columna Nombre.
    fn name_w(&self) -> i32 {
        let sb = if self.metrics().needed() { SB_W } else { 0 };
        (self.size.0 - sb - self.cols().iter().map(|c| c.1).sum::<i32>()).max(60)
    }

    /// Lee programas y conexiones: CPU y KB/s a partir de la diferencia desde la lectura anterior.
    fn refresh(&mut self, procs: Vec<ProcInfo>, conns: Vec<ConnInfo>, now: u64) {
        let dt = self.last_ms.map_or(0, |t| now.saturating_sub(t));
        let bytes = |id: u64| conns.iter().filter(|c| c.owner == id).map(|c| c.rx + c.tx).sum::<u64>();
        self.rows = procs
            .iter()
            .map(|p| {
                let (cpu, net) = match (dt, self.prev.iter().find(|q| q.0 == p.id)) {
                    (1.., Some(q)) => ((p.cpu_ms.saturating_sub(q.1) * 100 / dt).min(100) as u32, bytes(p.id).saturating_sub(q.2) * 1000 / dt),
                    _ => (0, 0),
                };
                Row { id: p.id, name: p.name.clone(), responding: p.responding, cpu, mem: p.mem, net, conns: conns.iter().filter(|c| c.owner == p.id).count() }
            })
            .collect();
        self.prev = procs.iter().map(|p| (p.id, p.cpu_ms, bytes(p.id))).collect();
        self.crows = conns
            .iter()
            .map(|c| {
                let name = match (c.owner, procs.iter().find(|p| p.id == c.owner)) {
                    (0, _) => "Sistema",
                    (_, Some(p)) => p.name.as_str(),
                    _ => "—",
                };
                let remote = c.remote.map_or(String::from("—"), |(ip, port)| format!("{}:{}", ip_text(ip), port));
                ConnRow { proto: c.proto, id: c.id, owner: c.owner, name: String::from(name), local: c.local_port, remote, state: c.state, rx: c.rx, tx: c.tx }
            })
            .collect();
        self.crows.sort_by(|a, b| (&a.name, proto_text(a.proto), a.local).cmp(&(&b.name, proto_text(b.proto), b.local)));
        self.last_ms = Some(now);
        self.next_ms = now + REFRESH_MS;
        if self.sel.is_some_and(|id| !self.rows.iter().any(|r| r.id == id)) {
            self.sel = None;
        }
        if self.csel.is_some_and(|k| !self.crows.iter().any(|r| (r.proto, r.id) == k)) {
            self.csel = None;
        }
        self.clamp_top();
    }

    fn selected(&self) -> Option<usize> {
        if self.tab == Tab::Procs {
            self.sel.and_then(|id| self.rows.iter().position(|r| r.id == id))
        } else {
            self.csel.and_then(|k| self.crows.iter().position(|r| (r.proto, r.id) == k))
        }
    }

    /// Programa que finaliza el botón: el seleccionado, o el dueño de la conexión (ni el sistema ni uno desconocido).
    fn target(&self) -> Option<u64> {
        if self.tab == Tab::Procs {
            return self.selected().map(|i| self.rows[i].id);
        }
        let c = &self.crows[self.selected()?];
        (c.owner != 0 && self.rows.iter().any(|r| r.id == c.owner)).then_some(c.owner)
    }

    /// Selecciona la fila `i` y la deja a la vista.
    fn select(&mut self, i: usize) {
        if self.tab == Tab::Procs {
            self.sel = self.rows.get(i).map(|r| r.id);
        } else {
            self.csel = self.crows.get(i).map(|r| (r.proto, r.id));
        }
        let l = self.list_rect();
        let (y0, y1) = (i as i32 * ROW_H, (i as i32 + 1) * ROW_H);
        if y0 < self.top {
            self.top = y0;
        } else if y1 > self.top + l.h {
            self.top = y1 - l.h;
        }
        self.clamp_top();
    }

    fn end_task(&mut self, cx: &mut Ctx) {
        if let Some(id) = self.target() {
            cx.sys.kill_process(id);
            // Se vuelve a leer ya: la fila desaparece sin esperar al segundo siguiente.
            self.next_ms = 0;
            cx.redraw = true;
        }
    }

    /// Textos y colores de las celdas de la fila `i` de la pestaña actual.
    fn cells(&self, i: usize) -> Vec<(String, u32)> {
        if self.tab == Tab::Conns {
            let c = &self.crows[i];
            return [c.name.clone(), String::from(proto_text(c.proto)), format!(":{}", c.local), c.remote.clone(), String::from(c.state), size_text(c.rx), size_text(c.tx)].into_iter().map(|t| (t, INK)).collect();
        }
        let r = &self.rows[i];
        let (estado, ink) = if r.responding { ("En ejecución", INK) } else { ("No responde", RED) };
        let red = if r.conns > 0 { format!("{} ({})", rate_text(r.net), r.conns) } else { rate_text(r.net) };
        vec![(r.name.clone(), INK), (String::from(estado), ink), (format!("{} %", r.cpu), INK), (mem_text(r.mem), INK), (red, INK)]
    }

    fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        let l = self.list_rect();
        if !l.contains(x, y) || (self.metrics().needed() && x >= l.right() - SB_W) {
            return None;
        }
        let i = (y - l.y + self.top) / ROW_H;
        (i >= 0 && (i as usize) < self.count()).then_some(i as usize)
    }
}

impl App for TaskMgr {
    fn kind(&self) -> AppKind {
        AppKind::TaskManager
    }
    fn title(&self) -> String {
        String::from("Administrador de tareas")
    }
    fn min_size(&self) -> (i32, i32) {
        (640, 260)
    }

    fn draw(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, _focused: bool) {
        self.size = (s.w, s.h);
        if self.last_ms.is_none() {
            let now = sys.uptime_ms();
            let (procs, conns) = read(sys);
            self.refresh(procs, conns, now);
        }
        self.clamp_top();
        let b = s.bounds();
        s.fill_rect(b, 0xFFFFFF);
        // Pestañas.
        for (i, (t, tab)) in [("Procesos", Tab::Procs), ("Conexiones", Tab::Conns)].into_iter().enumerate() {
            let r = Self::tab_rect(i as i32);
            let on = self.tab == tab;
            s.fill_rect(r, if on { 0xFFFFFF } else { 0xF0F0F0 });
            if on {
                s.fill_rect(Rect::new(r.x, r.y, r.w, 2), theme.accent);
            }
            text(s, r, t, if on { INK } else { MUTED }, Align::Center);
        }
        s.fill_rect(Rect::new(0, TAB_H - 1, s.w, 1), 0xE0E0E0);
        // Encabezado de columnas.
        let nw = self.name_w();
        let cols = self.cols();
        let width = |i: usize| if i == 0 { nw } else { cols[i].1 };
        let mut x = 0;
        for (i, c) in cols.iter().enumerate() {
            let w = width(i);
            text(s, Rect::new(x + 12, TAB_H, w - 24, HEAD_H), c.0, MUTED, c.2);
            s.fill_rect(Rect::new(x + w - 1, TAB_H + 6, 1, HEAD_H - 12), 0xE0E0E0);
            x += w;
        }
        s.fill_rect(Rect::new(0, TAB_H + HEAD_H - 1, s.w, 1), 0xE0E0E0);
        // Filas.
        let l = self.list_rect();
        let old = s.clip();
        s.set_clip(old.intersect(l));
        if self.count() == 0 {
            text(s, l, if self.tab == Tab::Procs { "No hay programas en ejecución" } else { "No hay conexiones de red" }, MUTED, Align::Center);
        }
        let sel = self.selected();
        for i in 0..self.count() {
            let y = l.y + i as i32 * ROW_H - self.top;
            if y + ROW_H <= l.y || y >= l.bottom() {
                continue;
            }
            if sel == Some(i) {
                s.fill_rect(Rect::new(0, y, l.w, ROW_H), blend(0xFFFFFF, theme.accent, 70));
            }
            let mut x = 0;
            for (k, (t, c)) in self.cells(i).into_iter().enumerate() {
                let w = width(k);
                text(s, Rect::new(x + 12, y, w - 24, ROW_H), &dialogs::fit(&t, 9, w - 24), c, cols[k].2);
                x += w;
            }
        }
        s.set_clip(old);
        if self.metrics().needed() {
            self.sb.draw(s, &self.metrics(), theme);
        }
        // Pie con el botón.
        s.fill_rect(Rect::new(0, s.h - FOOT_H, s.w, FOOT_H), 0xF0F0F0);
        s.fill_rect(Rect::new(0, s.h - FOOT_H, s.w, 1), 0xDADADA);
        let br = self.btn_rect();
        let on = self.target().is_some();
        let (fill, ink, edge) = match (on, self.hover_btn) {
            (false, _) => (0xE6E6E6, 0xA0A0A0, 0xD0D0D0),
            (true, true) if self.down_btn => (darken(0xE1E1E1, 30), INK, theme.accent),
            (true, true) => (0xE1E1E1, INK, theme.accent),
            (true, false) => (0xE6E6E6, INK, 0xADADAD),
        };
        s.fill_rect(br, fill);
        s.frame(br, edge);
        text(s, br, "Finalizar tarea", ink, Align::Center);
    }

    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
        let n = self.count();
        let cur = self.selected();
        if ev.key == Key::Tab && ev.ctrl {
            let t = if self.tab == Tab::Procs { Tab::Conns } else { Tab::Procs };
            return self.set_tab(t, cx);
        }
        match ev.key {
            Key::Down if n > 0 => self.select(cur.map_or(0, |i| (i + 1).min(n - 1))),
            Key::Up if n > 0 => self.select(cur.map_or(n - 1, |i| i.saturating_sub(1))),
            Key::Home if n > 0 => self.select(0),
            Key::End if n > 0 => self.select(n - 1),
            Key::Delete => self.end_task(cx),
            _ => return,
        }
        cx.redraw = true;
    }

    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
        let m = self.metrics();
        let e = self.sb.mouse(ev, &m, cx.now_ms);
        if let Some(p) = e.pos {
            self.top = p;
            self.clamp_top();
        }
        cx.redraw |= e.redraw;
        if e.captured {
            return;
        }
        let (hover, down) = match ev {
            MouseEv::Wheel { dz, .. } => {
                self.top += dz * ROW_H * 3;
                self.clamp_top();
                cx.redraw = true;
                return;
            }
            MouseEv::Down { x, y } => {
                if Self::tab_rect(0).contains(x, y) {
                    self.set_tab(Tab::Procs, cx);
                } else if Self::tab_rect(1).contains(x, y) {
                    self.set_tab(Tab::Conns, cx);
                }
                if let Some(i) = self.row_at(x, y) {
                    self.select(i);
                    cx.redraw = true;
                }
                let on = self.btn_rect().contains(x, y);
                (on, on)
            }
            MouseEv::Up { x, y } => {
                if self.down_btn && self.btn_rect().contains(x, y) {
                    self.end_task(cx);
                }
                (self.btn_rect().contains(x, y), false)
            }
            MouseEv::Move { x, y } => (self.btn_rect().contains(x, y), self.down_btn),
            MouseEv::Leave => (false, false),
            MouseEv::RightDown { .. } => return,
        };
        if (hover, down) != (self.hover_btn, self.down_btn) {
            cx.redraw = true;
        }
        self.hover_btn = hover;
        self.down_btn = down;
    }

    fn tick(&mut self, cx: &mut Ctx) {
        if self.last_ms.is_some() && cx.now_ms >= self.next_ms {
            let (procs, conns) = read(cx.sys);
            self.refresh(procs, conns, cx.now_ms);
            cx.redraw = true;
        }
        if let Some(p) = self.sb.tick(&self.metrics(), cx.now_ms) {
            self.top = p;
            self.clamp_top();
            cx.redraw = true;
        }
    }
}

/// Texto con el recorte limitado a `r`.
fn text(s: &mut Surface, r: Rect, t: &str, color: u32, align: Align) {
    let old = s.clip();
    s.set_clip(old.intersect(r));
    font::draw_in(s, r, t, 9, false, color, align);
    s.set_clip(old);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::Files;
    use crate::keys::Pool;
    use crate::net::{Datagram, Ipv4, ListenId, NetApi, NetError, NetStatus, PingId, PingReply, QueryId, SockId, TcpState, UdpId};
    use crate::platform::{DateTime, MemInfo, TestPlatform};

    /// Red falsa: solo devuelve las conexiones fijas que se le den; lo demás no se usa aquí.
    #[derive(Default)]
    struct RedFalsa {
        conns: Vec<ConnInfo>,
    }

    impl NetApi for RedFalsa {
        fn status(&mut self) -> NetStatus {
            unreachable!()
        }
        fn renew(&mut self) {}
        fn ping(&mut self, _: Ipv4, _: u16, _: u8) -> Result<PingId, NetError> {
            unreachable!()
        }
        fn ping_poll(&mut self, _: PingId) -> Option<Result<PingReply, NetError>> {
            unreachable!()
        }
        fn resolve(&mut self, _: &str) -> Result<QueryId, NetError> {
            unreachable!()
        }
        fn resolve_poll(&mut self, _: QueryId) -> Option<Result<Vec<Ipv4>, NetError>> {
            unreachable!()
        }
        fn tcp_connect(&mut self, _: Ipv4, _: u16) -> Result<SockId, NetError> {
            unreachable!()
        }
        fn tcp_state(&mut self, _: SockId) -> TcpState {
            unreachable!()
        }
        fn tcp_send(&mut self, _: SockId, _: &[u8]) -> Result<usize, NetError> {
            unreachable!()
        }
        fn tcp_recv(&mut self, _: SockId, _: &mut [u8]) -> Result<usize, NetError> {
            unreachable!()
        }
        fn tcp_close(&mut self, _: SockId) {}
        fn tcp_abort(&mut self, _: SockId) {}
        fn udp_bind(&mut self, _: u16) -> Result<UdpId, NetError> {
            unreachable!()
        }
        fn udp_send(&mut self, _: UdpId, _: Ipv4, _: u16, _: &[u8]) -> Result<(), NetError> {
            unreachable!()
        }
        fn udp_recv(&mut self, _: UdpId) -> Result<Option<Datagram>, NetError> {
            unreachable!()
        }
        fn udp_close(&mut self, _: UdpId) {}
        fn tcp_listen(&mut self, _: u16) -> Result<ListenId, NetError> {
            unreachable!()
        }
        fn tcp_accept(&mut self, _: ListenId) -> Result<Option<(SockId, Ipv4, u16)>, NetError> {
            unreachable!()
        }
        fn tcp_unlisten(&mut self, _: ListenId) {}
        fn connections(&mut self) -> Vec<ConnInfo> {
            self.conns.clone()
        }
        fn set_owner(&mut self, _: Proto, _: u32, _: u64) {}
    }

    fn conn(proto: Proto, id: u32, local_port: u16, remote: Option<(Ipv4, u16)>, state: &'static str, rx: u64, tx: u64, owner: u64) -> ConnInfo {
        ConnInfo { proto, id, local_port, remote, state, rx, tx, owner }
    }

    /// Plataforma de pruebas con una lista de programas falsa; apunta a quién se finaliza.
    #[derive(Default)]
    struct ConProcesos {
        base: TestPlatform,
        procs: Vec<ProcInfo>,
        killed: Vec<u64>,
        net: Option<RedFalsa>,
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
        fn kill_process(&mut self, id: u64) {
            self.killed.push(id);
            self.procs.retain(|p| p.id != id);
        }
        fn net(&mut self) -> Option<&mut dyn NetApi> {
            self.net.as_mut().map(|n| n as &mut dyn NetApi)
        }
    }

    fn proc(id: u64, name: &str, mem: u64, cpu_ms: u64, responding: bool) -> ProcInfo {
        ProcInfo { id, name: String::from(name), mem, cpu_ms, responding }
    }

    fn tecla(key: Key) -> KeyEvent {
        KeyEvent { key, ch: None, down: true, shift: false, ctrl: false, alt: false }
    }

    #[test]
    fn memoria_con_un_decimal() {
        assert_eq!(mem_text(0), "0,0 MB");
        assert_eq!(mem_text(MB * 12 + MB / 3), "12,3 MB");
        assert_eq!(mem_text(MB * 300), "300,0 MB");
    }

    #[test]
    fn lista_cpu_seleccion_y_finalizar() {
        let mut sys = ConProcesos { procs: vec![proc(1, "Reloj", 5 * MB, 100, true), proc(2, "Paint", 20 * MB, 0, false)], ..Default::default() };
        let mut th = Theme::default();
        let mut app = mgr();
        let mut s = Surface::new(560, 400, 0);
        app.draw(&mut s, &th, &mut sys, true);
        assert_eq!(app.rows.len(), 2);
        assert_eq!(app.rows[0].cpu, 0, "la primera lectura no tiene con qué comparar");
        // Un segundo después: Reloj ha usado 250 ms de procesador (25 %).
        sys.procs[0].cpu_ms = 350;
        sys.procs[1].cpu_ms = 5000;
        sys.base.ms = 1000;
        let mut cx = Ctx::new(&mut sys, &mut th, 999);
        app.tick(&mut cx);
        assert!(!cx.redraw, "aún no toca");
        drop(cx);
        let mut cx = Ctx::new(&mut sys, &mut th, 1000);
        app.tick(&mut cx);
        assert!(cx.redraw);
        assert_eq!((app.rows[0].cpu, app.rows[1].cpu), (25, 100), "el tope es 100");
        // Flechas y Supr.
        app.key(&tecla(Key::Down), &mut cx);
        assert_eq!(app.sel, Some(1));
        app.key(&tecla(Key::Down), &mut cx);
        assert_eq!(app.sel, Some(2));
        app.key(&tecla(Key::Up), &mut cx);
        app.key(&tecla(Key::Up), &mut cx);
        assert_eq!(app.sel, Some(1));
        // Clic en la segunda fila y en el botón.
        app.mouse(MouseEv::Down { x: 50, y: TAB_H + HEAD_H + ROW_H + 5 }, &mut cx);
        assert_eq!(app.sel, Some(2));
        let b = app.btn_rect();
        let (bx, by) = (b.x + 5, b.y + 5);
        app.mouse(MouseEv::Down { x: bx, y: by }, &mut cx);
        app.mouse(MouseEv::Up { x: bx, y: by }, &mut cx);
        drop(cx);
        assert_eq!(sys.killed, vec![2]);
        // La fila desaparece en cuanto se vuelve a leer.
        let mut cx = Ctx::new(&mut sys, &mut th, 1001);
        app.tick(&mut cx);
        assert_eq!((app.rows.len(), app.sel), (1, None));
        // Supr sin selección no hace nada; con selección finaliza.
        app.key(&tecla(Key::Delete), &mut cx);
        app.key(&tecla(Key::Down), &mut cx);
        app.key(&tecla(Key::Delete), &mut cx);
        drop(cx);
        assert_eq!(sys.killed, vec![2, 1]);
        // Sin programas: el aviso se ve (hay píxeles que no son blancos en el centro).
        app.rows.clear();
        app.draw(&mut s, &th, &mut sys, true);
        let l = app.list_rect();
        let zona = (l.y * s.w) as usize..(l.bottom() * s.w) as usize;
        assert!(s.px[zona].iter().any(|&p| p != 0xFFFFFF));
    }

    #[test]
    fn muchas_filas_se_desplazan_y_tamanos_raros_no_fallan() {
        let procs = (0..40).map(|i| proc(i + 1, "Programa", MB, 0, true)).collect();
        let mut sys = ConProcesos { procs, ..Default::default() };
        let mut th = Theme::default();
        let mut app = mgr();
        for (w, h) in [(440, 240), (1, 1), (0, 0), (900, 300)] {
            let mut s = Surface::new(w, h, 0);
            app.draw(&mut s, &th, &mut sys, true);
        }
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        for _ in 0..39 {
            app.key(&tecla(Key::Down), &mut cx);
        }
        assert_eq!(app.sel, Some(39));
        assert!(app.top > 0 && app.top <= app.metrics().max(), "la selección queda a la vista");
        app.mouse(MouseEv::Wheel { x: 5, y: 50, dz: -100 }, &mut cx);
        assert_eq!(app.top, 0);
    }

    fn ctrl_tab() -> KeyEvent {
        KeyEvent { ctrl: true, ..tecla(Key::Tab) }
    }

    #[test]
    fn red_por_proceso_entre_dos_lecturas() {
        let mut sys = ConProcesos { procs: vec![proc(1, "Navegador", MB, 0, true), proc(2, "Reloj", MB, 0, true)], ..Default::default() };
        sys.net = Some(RedFalsa { conns: vec![conn(Proto::Tcp, 1, 5000, Some(([1, 2, 3, 4], 80)), "Abierta", 1000, 24, 1), conn(Proto::Udp, 2, 53, None, "—", 0, 0, 1), conn(Proto::Tcp, 3, 6000, None, "Abierta", 99999, 0, 0)] });
        let mut th = Theme::default();
        let mut app = mgr();
        let mut s = Surface::new(700, 400, 0);
        app.draw(&mut s, &th, &mut sys, true);
        assert_eq!((app.rows[0].net, app.rows[0].conns, app.rows[1].conns), (0, 2, 0));
        // Un segundo después el navegador ha movido 12595 bytes más (12,29 KB: un decimal sin redondear).
        sys.net.as_mut().unwrap().conns[0].rx += 12_595;
        sys.base.ms = 1000;
        let mut cx = Ctx::new(&mut sys, &mut th, 1000);
        app.tick(&mut cx);
        assert_eq!(app.cells(0)[4].0, "12,2 KB/s (2)");
        assert_eq!(app.cells(1)[4].0, "0,0 KB/s");
        assert_eq!(size_text(0), "0 B");
        assert_eq!(size_text(1229), "1,2 KB");
        assert_eq!(size_text(MB * 7 / 2), "3,5 MB");
    }

    #[test]
    fn protocolo_tls_se_ve_como_tls() {
        assert_eq!([proto_text(Proto::Tls), proto_text(Proto::Tcp), proto_text(Proto::Udp)], ["TLS", "TCP", "UDP"]);
    }

    #[test]
    fn pestana_conexiones_textos_orden_y_finalizar() {
        let mut sys = ConProcesos { procs: vec![proc(1, "Navegador", MB, 0, true), proc(2, "Reloj", MB, 0, true)], ..Default::default() };
        sys.net = Some(RedFalsa {
            conns: vec![
                conn(Proto::Udp, 1, 53, None, "—", 5, 6, 2),
                conn(Proto::Tcp, 2, 5000, Some(([10, 0, 2, 15], 443)), "Abierta", 2048, 0, 1),
                conn(Proto::TcpListen, 3, 22, None, "Escuchando", 0, 0, 0),
                conn(Proto::Tcp, 4, 4000, None, "Abierta", 0, 0, 77),
            ],
        });
        let mut th = Theme::default();
        let mut app = mgr();
        let mut s = Surface::new(700, 400, 0);
        app.draw(&mut s, &th, &mut sys, true);
        // Clic en la pestaña Conexiones.
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        app.mouse(MouseEv::Down { x: TAB_W + 10, y: 5 }, &mut cx);
        assert!(app.tab == Tab::Conns && cx.redraw);
        app.mouse(MouseEv::Up { x: TAB_W + 10, y: 5 }, &mut cx);
        drop(cx);
        app.draw(&mut s, &th, &mut sys, true);
        let t = |a: &TaskMgr, i| a.cells(i).into_iter().map(|c| c.0).collect::<Vec<_>>();
        // Orden: programa (Navegador < Reloj < Sistema < «—»), luego protocolo y puerto.
        assert_eq!(t(&app, 3), ["—", "TCP", ":4000", "—", "Abierta", "0 B", "0 B"]);
        assert_eq!(t(&app, 0), ["Navegador", "TCP", ":5000", "10.0.2.15:443", "Abierta", "2,0 KB", "0 B"]);
        assert_eq!(t(&app, 1), ["Reloj", "UDP", ":53", "—", "—", "5 B", "6 B"]);
        assert_eq!(t(&app, 2), ["Sistema", "TCP", ":22", "—", "Escuchando", "0 B", "0 B"]);
        // Del sistema y desconocida: el botón no hace nada.
        for i in [2, 3] {
            app.select(i);
            assert!(app.target().is_none());
            let mut cx = Ctx::new(&mut sys, &mut th, 0);
            app.key(&tecla(Key::Delete), &mut cx);
        }
        assert!(sys.killed.is_empty());
        // Desde la fila del navegador se finaliza al dueño.
        let b = app.btn_rect();
        app.select(0);
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        app.mouse(MouseEv::Down { x: b.x + 5, y: b.y + 5 }, &mut cx);
        app.mouse(MouseEv::Up { x: b.x + 5, y: b.y + 5 }, &mut cx);
        drop(cx);
        assert_eq!(sys.killed, vec![1]);
        // Sin red: lista vacía con su aviso.
        sys.net = None;
        let mut cx = Ctx::new(&mut sys, &mut th, 1000);
        app.tick(&mut cx);
        drop(cx);
        assert_eq!(app.crows.len(), 0);
        app.draw(&mut s, &th, &mut sys, true);
    }

    #[test]
    fn cambiar_de_pestana_con_ctrl_tab_y_tamanos_raros() {
        let mut sys = ConProcesos { procs: vec![proc(1, "Reloj", MB, 0, true)], net: Some(RedFalsa { conns: (0..40).map(|i| conn(Proto::Udp, i, 1000 + i as u16, None, "—", 0, 0, 1)).collect() }), ..Default::default() };
        let mut th = Theme::default();
        let mut app = mgr();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        app.key(&tecla(Key::Tab), &mut cx);
        assert!(app.tab == Tab::Procs, "Tab solo no cambia");
        app.key(&ctrl_tab(), &mut cx);
        assert!(app.tab == Tab::Conns);
        app.key(&ctrl_tab(), &mut cx);
        assert!(app.tab == Tab::Procs);
        app.key(&ctrl_tab(), &mut cx);
        drop(cx);
        for (w, h) in [(640, 260), (1, 1), (0, 0), (900, 300), (30, 400)] {
            let mut s = Surface::new(w, h, 0);
            app.draw(&mut s, &th, &mut sys, true);
        }
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        for _ in 0..40 {
            app.key(&tecla(Key::Down), &mut cx);
        }
        assert_eq!(app.selected(), Some(39));
        assert!(app.top > 0 && app.top <= app.metrics().max());
    }
}
