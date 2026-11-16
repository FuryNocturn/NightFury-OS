//! Terminal: consola de órdenes del sistema. CONTRATO: `pub fn new() -> Box<dyn App>`.
use super::{App, AppKind, Ctx, MouseEv};
use crate::font;
use crate::geom::Rect;
use crate::input::{Key, KeyEvent};
use crate::platform::Platform;
use crate::surface::Surface;
use crate::theme::Theme;
use crate::{NAME, VERSION};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

const SIZE: u32 = 9;
const MARGIN: i32 = 6;
const MAX_LINES: usize = 500;
const BLINK_MS: u64 = 530;
const BG: u32 = 0x0C0C0C;
const FG: u32 = 0xCCCCCC;

/// Efecto de una orden sobre la ventana.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Nothing,
    Clear,
    Close,
}

fn prompt() -> String {
    format!("{}> ", NAME)
}

/// Bytes -> "X,Y MB" con un decimal.
fn mb(bytes: u64) -> String {
    let d = bytes * 10 / (1 << 20);
    format!("{},{} MB", d / 10, d % 10)
}

/// Intérprete de órdenes: devuelve las líneas de salida y la acción pedida.
fn run(line: &str, sys: &mut dyn Platform) -> (Vec<String>, Action) {
    let line = line.trim();
    let (cmd, args) = match line.split_once(' ') {
        Some((c, a)) => (c, a.trim()),
        None => (line, ""),
    };
    let one = |s: String| vec![s];
    let mut act = Action::Nothing;
    let out = match cmd.to_lowercase().as_str() {
        "" => Vec::new(),
        "ayuda" => [
            "ayuda: Muestra esta lista de órdenes",
            "info: Datos del sistema, procesador y pantalla",
            "memoria: Uso de la memoria",
            "fecha: Fecha actual",
            "hora: Hora actual",
            "tiempo: Tiempo que lleva encendido el equipo",
            "eco: Escribe el texto que le pases",
            "limpiar: Borra la pantalla",
            "acerca: Qué es este sistema",
            "reiniciar: Reinicia el equipo",
            "apagar: Apaga el equipo",
            "salir: Cierra esta ventana",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
        "info" => {
            let (w, h) = sys.screen();
            vec![format!("Sistema: {} {}", NAME, VERSION), format!("Procesador: {}", sys.cpu()), format!("Pantalla: {}x{}", w, h)]
        }
        "memoria" => {
            let m = sys.mem();
            vec![
                format!("RAM total: {}", mb(m.total)),
                format!("Memoria del sistema, total: {}", mb(m.heap_total)),
                format!("Memoria del sistema, usada: {}", mb(m.heap_used)),
                format!("Memoria del sistema, libre: {}", mb(m.heap_total.saturating_sub(m.heap_used))),
            ]
        }
        "fecha" => {
            let d = sys.now();
            one(format!("{:02}/{:02}/{:04}", d.day, d.month, d.year))
        }
        "hora" => {
            let d = sys.now();
            one(format!("{:02}:{:02}:{:02}", d.hour, d.minute, d.second))
        }
        "tiempo" => {
            let s = sys.uptime_ms() / 1000;
            one(format!("{} h {} min {} s", s / 3600, s / 60 % 60, s % 60))
        }
        "eco" => one(args.to_string()),
        "limpiar" => {
            act = Action::Clear;
            Vec::new()
        }
        "acerca" => vec![format!("{} es un sistema operativo escrito desde cero en Rust.", NAME), format!("Versión {}", VERSION)],
        "reiniciar" => {
            sys.reboot();
            Vec::new()
        }
        "apagar" => {
            sys.shutdown();
            Vec::new()
        }
        "salir" => {
            act = Action::Close;
            Vec::new()
        }
        _ => one(format!("'{}' no es una orden conocida. Escribe 'ayuda'.", cmd)),
    };
    (out, act)
}

/// Parte `text` en filas de como mucho `max_w` píxeles (siempre devuelve al menos una).
fn wrap(text: &str, max_w: i32) -> Vec<String> {
    let mut rows = Vec::new();
    let (mut cur, mut w) = (String::new(), 0);
    for c in text.chars() {
        let a = font::adv(c, SIZE, false);
        if w + a > max_w && !cur.is_empty() {
            rows.push(core::mem::take(&mut cur));
            w = 0;
        }
        cur.push(c);
        w += a;
    }
    rows.push(cur);
    rows
}

pub struct Terminal {
    out: Vec<String>,
    input: String,
    history: Vec<String>,
    /// Posición en `history` al recorrerlo con las flechas; `history.len()` = línea nueva.
    hist_pos: usize,
    /// Filas desplazadas hacia arriba desde el final.
    scroll: usize,
    max_scroll: usize,
    vis: usize,
    focused: bool,
    t0: u64,
    shown: bool,
}

pub fn new() -> Box<dyn App> {
    Box::new(Terminal::new())
}

impl Terminal {
    fn new() -> Self {
        let mut t = Terminal {
            out: Vec::new(),
            input: String::new(),
            history: Vec::new(),
            hist_pos: 0,
            scroll: 0,
            max_scroll: 0,
            vis: 10,
            focused: false,
            t0: 0,
            shown: true,
        };
        t.push(format!("{} {}", NAME, VERSION));
        t.push(String::from("Escribe 'ayuda' para ver las órdenes."));
        t
    }

    fn push(&mut self, l: String) {
        self.out.push(l);
        if self.out.len() > MAX_LINES {
            self.out.drain(..self.out.len() - MAX_LINES);
        }
    }

    fn submit(&mut self, sys: &mut dyn Platform) -> Action {
        let line = core::mem::take(&mut self.input);
        self.push(format!("{}{}", prompt(), line));
        let cmd = line.trim();
        if !cmd.is_empty() && self.history.last().map(String::as_str) != Some(cmd) {
            self.history.push(cmd.to_string());
        }
        self.hist_pos = self.history.len();
        let (lines, act) = run(&line, sys);
        if act == Action::Clear {
            self.out.clear();
        }
        for l in lines {
            self.push(l);
        }
        act
    }
}

impl App for Terminal {
    fn kind(&self) -> AppKind {
        AppKind::Terminal
    }
    fn title(&self) -> String {
        String::from("Terminal")
    }
    fn min_size(&self) -> (i32, i32) {
        (320, 160)
    }

    fn draw(&mut self, s: &mut Surface, _theme: &Theme, _sys: &mut dyn Platform, focused: bool) {
        self.focused = focused;
        s.fill_rect(s.bounds(), BG);
        let lh = font::line_h(SIZE);
        let max_w = (s.w - 2 * MARGIN).max(1);
        // Todas las filas visuales; la última línea lleva un espacio final que hace de cursor.
        let mut rows: Vec<String> = Vec::new();
        for l in &self.out {
            rows.extend(wrap(l, max_w));
        }
        let input_rows = wrap(&format!("{}{} ", prompt(), self.input), max_w);
        let n_input = input_rows.len();
        rows.extend(input_rows);

        self.vis = (((s.h - 2 * MARGIN) / lh).max(1)) as usize;
        self.max_scroll = rows.len().saturating_sub(self.vis);
        self.scroll = self.scroll.min(self.max_scroll);
        let first = rows.len().saturating_sub(self.vis + self.scroll);
        let last = (first + self.vis).min(rows.len());

        for (i, r) in rows[first..last].iter().enumerate() {
            let y = MARGIN + i as i32 * lh;
            let is_cursor_row = first + i == rows.len() - 1 && n_input > 0;
            if is_cursor_row {
                // Sin el espacio final: el bloque se pinta justo detrás.
                let txt = &r[..r.len() - 1];
                let end = font::draw(s, MARGIN, y, txt, SIZE, false, FG);
                if focused && self.shown {
                    s.fill_rect(Rect::new(end, y + 1, font::adv('0', SIZE, false), lh - 2), FG);
                }
            } else {
                font::draw(s, MARGIN, y, r, SIZE, false, FG);
            }
        }
    }

    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
        match ev.key {
            Key::PageUp => self.scroll = (self.scroll + self.vis.saturating_sub(1).max(1)).min(self.max_scroll),
            Key::PageDown => self.scroll = self.scroll.saturating_sub(self.vis.saturating_sub(1).max(1)),
            Key::Enter => {
                self.scroll = 0;
                if self.submit(&mut *cx.sys) == Action::Close {
                    cx.close = true;
                }
            }
            Key::Backspace => {
                self.scroll = 0;
                self.input.pop();
            }
            Key::Escape => {
                self.scroll = 0;
                self.input.clear();
                self.hist_pos = self.history.len();
            }
            Key::Up => {
                self.scroll = 0;
                if self.hist_pos > 0 {
                    self.hist_pos -= 1;
                    self.input = self.history[self.hist_pos].clone();
                }
            }
            Key::Down => {
                self.scroll = 0;
                if self.hist_pos + 1 < self.history.len() {
                    self.hist_pos += 1;
                    self.input = self.history[self.hist_pos].clone();
                } else {
                    self.hist_pos = self.history.len();
                    self.input.clear();
                }
            }
            _ => match ev.ch {
                Some(c) if !ev.ctrl && !c.is_control() => {
                    self.scroll = 0;
                    self.input.push(c);
                }
                _ => return,
            },
        }
        self.t0 = cx.now_ms;
        self.shown = true;
        cx.redraw = true;
    }

    fn mouse(&mut self, _ev: MouseEv, _cx: &mut Ctx) {}

    fn tick(&mut self, cx: &mut Ctx) {
        let on = !self.focused || (cx.now_ms.saturating_sub(self.t0) / BLINK_MS) % 2 == 0;
        if on != self.shown {
            self.shown = on;
            if self.focused {
                cx.redraw = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::TestPlatform;

    fn ejecuta(l: &str) -> (Vec<String>, Action) {
        run(l, &mut TestPlatform { ms: 3_723_000, size: (1024, 768), ..Default::default() })
    }

    fn ev(key: Key, ch: Option<char>) -> KeyEvent {
        KeyEvent { key, ch, down: true, shift: false, ctrl: false, alt: false }
    }

    fn escribe(t: &mut Terminal, txt: &str, cx: &mut Ctx) {
        for c in txt.chars() {
            t.key(&ev(Key::Char(c), Some(c)), cx);
        }
        t.key(&ev(Key::Enter, Some('\n')), cx);
    }

    fn con_cx<R>(f: impl FnOnce(&mut Ctx) -> R) -> R {
        let mut sys = TestPlatform::default();
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        f(&mut cx)
    }

    #[test]
    fn ordenes_basicas() {
        assert_eq!(ejecuta("fecha").0, ["15/11/2026"]);
        assert_eq!(ejecuta("hora").0, ["21:37:05"]);
        assert_eq!(ejecuta("tiempo").0, ["1 h 2 min 3 s"]);
        assert_eq!(ejecuta("eco  Hola  Mundo ").0, ["Hola  Mundo"]);
        assert_eq!(ejecuta("").0.len(), 0);
        assert_eq!(ejecuta("   ").1, Action::Nothing);
        assert_eq!(ejecuta("limpiar").1, Action::Clear);
        assert_eq!(ejecuta("salir").1, Action::Close);
        assert_eq!(ejecuta("xyz").0, ["'xyz' no es una orden conocida. Escribe 'ayuda'."]);
        assert_eq!(ejecuta("acerca").0.len(), 2);
        assert!(ejecuta("ayuda").0.len() >= 12);
    }

    #[test]
    fn info_y_memoria() {
        let i = ejecuta("info").0;
        assert_eq!(i[0], format!("Sistema: {} {}", NAME, VERSION));
        assert_eq!(i[1], "Procesador: CPU de pruebas");
        assert_eq!(i[2], "Pantalla: 1024x768");
        let m = ejecuta("memoria").0;
        assert_eq!(m[0], "RAM total: 1024,0 MB");
        assert_eq!(m[1], "Memoria del sistema, total: 256,0 MB");
        assert_eq!(m[2], "Memoria del sistema, usada: 20,0 MB");
        assert_eq!(m[3], "Memoria del sistema, libre: 236,0 MB");
    }

    #[test]
    fn mayusculas_indiferentes() {
        assert_eq!(ejecuta("FECHA"), ejecuta("fecha"));
        assert_eq!(ejecuta("Salir").1, Action::Close);
        assert_eq!(ejecuta("ECO Ñu").0, ["Ñu"]);
    }

    #[test]
    fn apagar_y_reiniciar_llaman_a_la_plataforma() {
        let mut p = TestPlatform::default();
        run("reiniciar", &mut p);
        assert!(p.rebooted && !p.halted);
        run("APAGAR", &mut p);
        assert!(p.halted);
    }

    #[test]
    fn terminal_salir_cierra_y_limpiar_borra() {
        con_cx(|cx| {
            let mut t = Terminal::new();
            escribe(&mut t, "fecha", cx);
            assert_eq!(t.out.last().unwrap(), "15/11/2026");
            escribe(&mut t, "limpiar", cx);
            assert!(t.out.is_empty());
            assert!(!cx.close);
            escribe(&mut t, "salir", cx);
            assert!(cx.close);
            // Línea vacía: solo repite el indicador.
            let mut t = Terminal::new();
            let n = t.out.len();
            t.key(&ev(Key::Enter, Some('\n')), cx);
            assert_eq!(t.out.len(), n + 1);
            assert_eq!(t.out[n], prompt());
        });
    }

    #[test]
    fn historial_con_flechas() {
        con_cx(|cx| {
            let mut t = Terminal::new();
            escribe(&mut t, "hora", cx);
            escribe(&mut t, "fecha", cx);
            t.key(&ev(Key::Up, None), cx);
            assert_eq!(t.input, "fecha");
            t.key(&ev(Key::Up, None), cx);
            assert_eq!(t.input, "hora");
            t.key(&ev(Key::Up, None), cx);
            assert_eq!(t.input, "hora");
            t.key(&ev(Key::Down, None), cx);
            assert_eq!(t.input, "fecha");
            t.key(&ev(Key::Down, None), cx);
            assert_eq!(t.input, "");
            t.key(&ev(Key::Char('a'), Some('a')), cx);
            t.key(&ev(Key::Escape, None), cx);
            assert_eq!(t.input, "");
        });
    }

    #[test]
    fn limite_de_500_lineas() {
        con_cx(|cx| {
            let mut t = Terminal::new();
            for i in 0..700 {
                escribe(&mut t, &format!("eco {}", i), cx);
            }
            assert_eq!(t.out.len(), MAX_LINES);
            assert_eq!(t.out.last().unwrap(), "699");
        });
    }

    #[test]
    fn desplazamiento_y_dibujo() {
        let mut sys = TestPlatform::default();
        let mut t = Terminal::new();
        con_cx(|cx| {
            for i in 0..60 {
                escribe(&mut t, &format!("eco {} {}", i, "palabra ".repeat(40)), cx);
            }
            let mut s = Surface::new(320, 160, 0);
            t.draw(&mut s, &Theme::default(), &mut sys, true);
            assert!(t.max_scroll > 0);
            t.key(&ev(Key::PageUp, None), cx);
            assert!(t.scroll > 0);
            t.key(&ev(Key::PageDown, None), cx);
            assert_eq!(t.scroll, 0);
            t.key(&ev(Key::PageUp, None), cx);
            t.key(&ev(Key::Char('x'), Some('x')), cx);
            assert_eq!(t.scroll, 0);
        });
        for (w, h) in [(320, 160), (1, 1), (0, 0), (50, 20), (680, 400)] {
            let mut s = Surface::new(w, h, 0);
            t.draw(&mut s, &Theme::default(), &mut sys, true);
            t.draw(&mut s, &Theme::default(), &mut sys, false);
        }
    }

    #[test]
    fn partir_por_ancho() {
        assert_eq!(wrap("", 100), [""]);
        let w = font::width("abc", SIZE, false);
        let r = wrap("abcabcabc", w);
        assert_eq!(r, ["abc", "abc", "abc"]);
        assert_eq!(wrap("x", 0), ["x"]);
    }

    #[test]
    fn parpadeo() {
        con_cx(|cx| {
            let mut t = Terminal::new();
            t.focused = true;
            cx.now_ms = 100;
            t.tick(cx);
            assert!(!cx.redraw);
            cx.now_ms = 600;
            t.tick(cx);
            assert!(cx.redraw && !t.shown);
        });
    }
}
