// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Administrador de tareas: los programas en marcha con su estado, CPU y memoria, y "Finalizar
//! tarea". CONTRATO: `pub fn new() -> Box<dyn App>`.
use super::{App, AppKind, Ctx, MouseEv};
use crate::dialogs;
use crate::font::{self, Align};
use crate::geom::Rect;
use crate::input::{Key, KeyEvent};
use crate::platform::Platform;
use crate::proc::ProcInfo;
use crate::scrollbar::{Metrics, ScrollBar, SB_W};
use crate::surface::{blend, darken, Surface};
use crate::theme::Theme;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

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
/// Anchos fijos de las columnas Estado, CPU y Memoria; Nombre ocupa el resto.
const COLS: [i32; 3] = [130, 70, 100];

struct Row {
    id: u64,
    name: String,
    responding: bool,
    /// % de procesador usado en el último intervalo.
    cpu: u32,
    mem: u64,
}

struct TaskMgr {
    rows: Vec<Row>,
    /// (id, cpu_ms) de la lectura anterior y cuándo se hizo, para sacar el % de CPU.
    prev: Vec<(u64, u64)>,
    last_ms: Option<u64>,
    next_ms: u64,
    sel: Option<u64>,
    /// Desplazamiento de la lista en píxeles.
    top: i32,
    sb: ScrollBar,
    size: (i32, i32),
    hover_btn: bool,
    down_btn: bool,
}

pub fn new() -> Box<dyn App> {
    Box::new(TaskMgr { rows: Vec::new(), prev: Vec::new(), last_ms: None, next_ms: 0, sel: None, top: 0, sb: ScrollBar::new(false), size: (0, 0), hover_btn: false, down_btn: false })
}

/// "12,3 MB" (un decimal, coma decimal).
fn mem_text(bytes: u64) -> String {
    let t = bytes * 10 / MB;
    format!("{},{} MB", t / 10, t % 10)
}

impl TaskMgr {
    /// Lista de la derecha del encabezado hasta el pie.
    fn list_rect(&self) -> Rect {
        Rect::new(0, HEAD_H, self.size.0, (self.size.1 - HEAD_H - FOOT_H).max(0))
    }

    fn btn_rect(&self) -> Rect {
        Rect::new(self.size.0 - BTN_W - 14, self.size.1 - FOOT_H + (FOOT_H - BTN_H) / 2, BTN_W, BTN_H)
    }

    fn metrics(&self) -> Metrics {
        let l = self.list_rect();
        Metrics { rect: Rect::new(l.right() - SB_W, l.y, SB_W, l.h), total: self.rows.len() as i32 * ROW_H, page: l.h, pos: self.top, line: ROW_H }
    }

    fn clamp_top(&mut self) {
        self.top = self.top.clamp(0, self.metrics().max());
    }

    /// Ancho de la columna Nombre.
    fn name_w(&self) -> i32 {
        let sb = if self.metrics().needed() { SB_W } else { 0 };
        (self.size.0 - sb - COLS.iter().sum::<i32>()).max(60)
    }

    /// Lee la lista de programas: CPU a partir de la diferencia de `cpu_ms` desde la lectura anterior.
    fn refresh(&mut self, procs: Vec<ProcInfo>, now: u64) {
        let dt = self.last_ms.map_or(0, |t| now.saturating_sub(t));
        self.rows = procs
            .iter()
            .map(|p| {
                let cpu = match (dt, self.prev.iter().find(|q| q.0 == p.id)) {
                    (1.., Some(q)) => (p.cpu_ms.saturating_sub(q.1) * 100 / dt).min(100) as u32,
                    _ => 0,
                };
                Row { id: p.id, name: p.name.clone(), responding: p.responding, cpu, mem: p.mem }
            })
            .collect();
        self.prev = procs.iter().map(|p| (p.id, p.cpu_ms)).collect();
        self.last_ms = Some(now);
        self.next_ms = now + REFRESH_MS;
        if self.sel.is_some_and(|id| !self.rows.iter().any(|r| r.id == id)) {
            self.sel = None;
        }
        self.clamp_top();
    }

    fn selected(&self) -> Option<usize> {
        self.sel.and_then(|id| self.rows.iter().position(|r| r.id == id))
    }

    /// Selecciona la fila `i` y la deja a la vista.
    fn select(&mut self, i: usize) {
        self.sel = self.rows.get(i).map(|r| r.id);
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
        if let Some(id) = self.sel {
            cx.sys.kill_process(id);
            // Se vuelve a leer ya: la fila desaparece sin esperar al segundo siguiente.
            self.next_ms = 0;
            cx.redraw = true;
        }
    }

    fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        let l = self.list_rect();
        if !l.contains(x, y) || (self.metrics().needed() && x >= l.right() - SB_W) {
            return None;
        }
        let i = (y - l.y + self.top) / ROW_H;
        (i >= 0 && (i as usize) < self.rows.len()).then_some(i as usize)
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
        (440, 240)
    }

    fn draw(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, _focused: bool) {
        self.size = (s.w, s.h);
        if self.last_ms.is_none() {
            let now = sys.uptime_ms();
            self.refresh(sys.processes(), now);
        }
        self.clamp_top();
        let b = s.bounds();
        s.fill_rect(b, 0xFFFFFF);
        // Encabezado de columnas.
        let nw = self.name_w();
        let head = [("Nombre", nw, Align::Left), ("Estado", COLS[0], Align::Left), ("CPU", COLS[1], Align::Right), ("Memoria", COLS[2], Align::Right)];
        let mut x = 0;
        for (t, w, al) in head {
            let r = Rect::new(x + 12, 0, w - 24, HEAD_H);
            text(s, r, t, MUTED, al);
            s.fill_rect(Rect::new(x + w - 1, 6, 1, HEAD_H - 12), 0xE0E0E0);
            x += w;
        }
        s.fill_rect(Rect::new(0, HEAD_H - 1, s.w, 1), 0xE0E0E0);
        // Filas.
        let l = self.list_rect();
        let old = s.clip();
        s.set_clip(old.intersect(l));
        if self.rows.is_empty() {
            text(s, l, "No hay programas en ejecución", MUTED, Align::Center);
        }
        for (i, r) in self.rows.iter().enumerate() {
            let y = l.y + i as i32 * ROW_H - self.top;
            if y + ROW_H <= l.y || y >= l.bottom() {
                continue;
            }
            if self.sel == Some(r.id) {
                s.fill_rect(Rect::new(0, y, l.w, ROW_H), blend(0xFFFFFF, theme.accent, 70));
            }
            let (estado, ink) = if r.responding { ("En ejecución", INK) } else { ("No responde", RED) };
            let cells = [(r.name.clone(), nw, Align::Left, INK), (String::from(estado), COLS[0], Align::Left, ink), (format!("{} %", r.cpu), COLS[1], Align::Right, INK), (mem_text(r.mem), COLS[2], Align::Right, INK)];
            let mut x = 0;
            for (t, w, al, c) in cells {
                text(s, Rect::new(x + 12, y, w - 24, ROW_H), &dialogs::fit(&t, 9, w - 24), c, al);
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
        let on = self.selected().is_some();
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
        let n = self.rows.len();
        let cur = self.selected();
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
            self.refresh(cx.sys.processes(), cx.now_ms);
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
    use crate::platform::{DateTime, MemInfo, TestPlatform};

    /// Plataforma de pruebas con una lista de programas falsa; apunta a quién se finaliza.
    #[derive(Default)]
    struct ConProcesos {
        base: TestPlatform,
        procs: Vec<ProcInfo>,
        killed: Vec<u64>,
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
        let mut app = TaskMgr { rows: Vec::new(), prev: Vec::new(), last_ms: None, next_ms: 0, sel: None, top: 0, sb: ScrollBar::new(false), size: (0, 0), hover_btn: false, down_btn: false };
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
        app.mouse(MouseEv::Down { x: 50, y: HEAD_H + ROW_H + 5 }, &mut cx);
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
        let mut app = TaskMgr { rows: Vec::new(), prev: Vec::new(), last_ms: None, next_ms: 0, sel: None, top: 0, sb: ScrollBar::new(false), size: (0, 0), hover_btn: false, down_btn: false };
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
}
