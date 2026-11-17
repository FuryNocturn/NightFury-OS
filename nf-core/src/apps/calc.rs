// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Calculadora estándar encadenada. CONTRATO: `pub fn new() -> Box<dyn App>`.
use super::{App, AppKind, Ctx, MouseEv};
use crate::font::{self, Align};
use crate::geom::Rect;
use crate::input::{Key, KeyEvent};
use crate::platform::Platform;
use crate::surface::{darken, Surface};
use crate::theme::Theme;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const DIV0: &str = "No se puede dividir entre cero";
const INVALID: &str = "Entrada no válida";
const MAX_DIGITS: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Btn {
    Digit(u8),
    Comma,
    Op(char),
    Eq,
    C,
    CE,
    Back,
    Neg,
    Pct,
    Inv,
    Sq,
    Sqrt,
}

/// Rejilla de botones: (texto, acción), 6 filas x 4 columnas.
const GRID: [(&str, Btn); 24] = [
    ("%", Btn::Pct),
    ("CE", Btn::CE),
    ("C", Btn::C),
    ("←", Btn::Back),
    ("1/x", Btn::Inv),
    ("x²", Btn::Sq),
    ("√x", Btn::Sqrt),
    ("÷", Btn::Op('÷')),
    ("7", Btn::Digit(7)),
    ("8", Btn::Digit(8)),
    ("9", Btn::Digit(9)),
    ("×", Btn::Op('×')),
    ("4", Btn::Digit(4)),
    ("5", Btn::Digit(5)),
    ("6", Btn::Digit(6)),
    ("−", Btn::Op('−')),
    ("1", Btn::Digit(1)),
    ("2", Btn::Digit(2)),
    ("3", Btn::Digit(3)),
    ("+", Btn::Op('+')),
    ("±", Btn::Neg),
    ("0", Btn::Digit(0)),
    (",", Btn::Comma),
    ("=", Btn::Eq),
];

fn abs(v: f64) -> f64 {
    if v < 0.0 {
        -v
    } else {
        v
    }
}

/// Quita ceros finales (y el punto si queda solo) de un número con punto decimal.
fn trim_zeros(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

/// Formato de pantalla: hasta 10 decimales, coma decimal, científica si es enorme o minúsculo.
fn fmt(v: f64) -> String {
    if v == 0.0 {
        return String::from("0");
    }
    let a = abs(v);
    let s = if !(1e-9..1e16).contains(&a) {
        let t = format!("{:.10e}", v);
        let (m, e) = t.split_once('e').unwrap_or((&t, "0"));
        format!("{}e{}", trim_zeros(m), e)
    } else {
        String::from(trim_zeros(&format!("{:.10}", v)))
    };
    let s = s.replace('.', ",");
    if s == "-0" {
        String::from("0")
    } else {
        s
    }
}

fn apply(a: f64, op: char, b: f64) -> Result<f64, &'static str> {
    let r = match op {
        '+' => a + b,
        '−' => a - b,
        '×' => a * b,
        _ => {
            if b == 0.0 {
                return Err(DIV0);
            }
            a / b
        }
    };
    if r.is_finite() {
        Ok(r)
    } else {
        Err(INVALID)
    }
}

/// Lógica pura de la calculadora (sin dibujo).
struct Calc {
    /// Número en pantalla (lo tecleado o el último resultado).
    cur: String,
    /// Acumulado de la operación pendiente.
    acc: f64,
    op: Option<char>,
    /// La siguiente cifra empieza un número nuevo.
    fresh: bool,
    /// Hay operando introducido para la operación pendiente.
    entered: bool,
    after_eq: bool,
    expr: String,
    err: Option<&'static str>,
}

impl Calc {
    fn new() -> Self {
        Calc { cur: String::from("0"), acc: 0.0, op: None, fresh: false, entered: false, after_eq: false, expr: String::new(), err: None }
    }

    fn text(&self) -> &str {
        self.err.unwrap_or(&self.cur)
    }

    fn val(&self) -> f64 {
        self.cur.replace(',', ".").parse().unwrap_or(0.0)
    }

    fn fail(&mut self, m: &'static str) {
        *self = Calc::new();
        self.err = Some(m);
    }

    fn digits(&self) -> usize {
        self.cur.chars().filter(|c| c.is_ascii_digit()).count()
    }

    /// Prepara la entrada de un número: si venía un resultado u operador, empieza de cero.
    fn start_entry(&mut self) {
        if self.fresh {
            self.cur = String::from("0");
            self.fresh = false;
            if self.after_eq {
                self.expr.clear();
                self.after_eq = false;
            }
        }
        self.entered = true;
    }

    /// Deja un resultado en pantalla; sin operación pendiente borra la expresión.
    fn show(&mut self, v: f64) {
        if !v.is_finite() {
            return self.fail(INVALID);
        }
        self.cur = fmt(v);
        self.fresh = true;
        self.entered = true;
        if self.op.is_none() {
            self.expr.clear();
            self.after_eq = false;
        }
    }

    fn press(&mut self, b: Btn) {
        if self.err.is_some() {
            *self = Calc::new();
            if b == Btn::C {
                return;
            }
        }
        match b {
            Btn::Digit(d) => {
                self.start_entry();
                if self.cur == "0" {
                    self.cur.clear();
                }
                if self.digits() < MAX_DIGITS {
                    self.cur.push((b'0' + d) as char);
                } else if self.cur.is_empty() {
                    self.cur.push('0');
                }
            }
            Btn::Comma => {
                self.start_entry();
                if !self.cur.contains(',') && self.digits() < MAX_DIGITS {
                    self.cur.push(',');
                }
            }
            Btn::Back => {
                if !self.fresh {
                    self.cur.pop();
                    if self.cur.is_empty() || self.cur == "-" {
                        self.cur = String::from("0");
                    }
                }
            }
            Btn::CE => {
                self.cur = String::from("0");
                self.fresh = true;
                self.entered = false;
                if self.after_eq {
                    self.expr.clear();
                    self.after_eq = false;
                }
            }
            Btn::C => *self = Calc::new(),
            Btn::Neg => {
                if self.val() != 0.0 {
                    if self.cur.starts_with('-') {
                        self.cur.remove(0);
                    } else {
                        self.cur.insert(0, '-');
                    }
                }
                self.entered = true;
            }
            Btn::Pct => {
                let v = if self.op.is_some() { self.acc * self.val() / 100.0 } else { 0.0 };
                self.show(v);
            }
            Btn::Inv => {
                let v = self.val();
                if v == 0.0 {
                    self.fail(DIV0);
                } else {
                    self.show(1.0 / v);
                }
            }
            Btn::Sq => {
                let v = self.val();
                self.show(v * v);
            }
            Btn::Sqrt => {
                let v = self.val();
                if v < 0.0 {
                    self.fail(INVALID);
                } else {
                    self.show(crate::math::sqrt64(v));
                }
            }
            Btn::Op(o) => {
                let v = self.val();
                match self.op {
                    Some(p) if self.entered => match apply(self.acc, p, v) {
                        Ok(r) => {
                            self.acc = r;
                            self.cur = fmt(r);
                        }
                        Err(m) => return self.fail(m),
                    },
                    Some(_) => {}
                    None => self.acc = v,
                }
                self.op = Some(o);
                self.expr = format!("{} {}", fmt(self.acc), o);
                self.fresh = true;
                self.entered = false;
                self.after_eq = false;
            }
            Btn::Eq => {
                let b = self.val();
                if let Some(p) = self.op {
                    match apply(self.acc, p, b) {
                        Ok(r) => {
                            self.expr = format!("{} {} {} =", fmt(self.acc), p, fmt(b));
                            self.acc = r;
                            self.cur = fmt(r);
                            self.op = None;
                        }
                        Err(m) => return self.fail(m),
                    }
                } else {
                    self.expr = format!("{} =", fmt(b));
                    self.cur = fmt(b);
                }
                self.fresh = true;
                self.entered = false;
                self.after_eq = true;
            }
        }
    }
}

fn btn_of_char(c: char) -> Option<Btn> {
    match c {
        '0'..='9' => Some(Btn::Digit(c as u8 - b'0')),
        ',' | '.' => Some(Btn::Comma),
        '+' => Some(Btn::Op('+')),
        '-' => Some(Btn::Op('−')),
        '*' => Some(Btn::Op('×')),
        '/' => Some(Btn::Op('÷')),
        '=' => Some(Btn::Eq),
        '%' => Some(Btn::Pct),
        _ => None,
    }
}

struct CalcApp {
    c: Calc,
    /// Rectángulos de los botones (mismo orden que `GRID`), calculados al dibujar.
    rects: Vec<Rect>,
    hover: Option<usize>,
    down: Option<usize>,
}

pub fn new() -> Box<dyn App> {
    Box::new(CalcApp { c: Calc::new(), rects: Vec::new(), hover: None, down: None })
}

impl CalcApp {
    fn hit(&self, x: i32, y: i32) -> Option<usize> {
        self.rects.iter().position(|r| r.contains(x, y))
    }

    /// Calcula la rejilla para una superficie de w x h; devuelve el alto de la zona de pantalla.
    fn layout(&mut self, w: i32, h: i32) -> i32 {
        let disp = (h * 17 / 100).clamp(72, 96).min(h.max(0));
        let (gx, gy) = (4, disp);
        let (gw, gh) = ((w - 8).max(0), (h - disp - 4).max(0));
        self.rects.clear();
        for i in 0..24i32 {
            let (r, c) = (i / 4, i % 4);
            let x0 = gx + c * (gw + 2) / 4;
            let x1 = gx + (c + 1) * (gw + 2) / 4 - 2;
            let y0 = gy + r * (gh + 2) / 6;
            let y1 = gy + (r + 1) * (gh + 2) / 6 - 2;
            self.rects.push(Rect::new(x0, y0, (x1 - x0).max(0), (y1 - y0).max(0)));
        }
        disp
    }
}

impl App for CalcApp {
    fn kind(&self) -> AppKind {
        AppKind::Calc
    }
    fn title(&self) -> String {
        String::from("Calculadora")
    }
    fn min_size(&self) -> (i32, i32) {
        (240, 340)
    }

    fn draw(&mut self, s: &mut Surface, theme: &Theme, _sys: &mut dyn Platform, _focused: bool) {
        let b = s.bounds();
        s.fill_rect(b, 0xF2F2F2);
        let disp = self.layout(s.w, s.h);
        // Expresión y número en pantalla.
        font::draw_in(s, Rect::new(8, 6, s.w - 16, font::line_h(9)), &self.c.expr, 9, false, 0x6A6A6A, Align::Right);
        let text = self.c.text();
        let sizes: &[u32] = if self.c.err.is_some() { &[16, 12, 9] } else { &[26, 20, 16, 12] };
        let size = sizes.iter().copied().find(|&z| font::width(text, z, true) <= s.w - 16).unwrap_or(sizes[sizes.len() - 1]);
        let nh = font::line_h(size);
        font::draw_in(s, Rect::new(8, disp - nh - 6, s.w - 16, nh), text, size, true, 0x1A1A1A, Align::Right);
        // Botones.
        for (i, ((label, btn), r)) in GRID.iter().zip(&self.rects).enumerate() {
            let digit = matches!(btn, Btn::Digit(_));
            let eq = *btn == Btn::Eq;
            let base = if eq {
                theme.accent
            } else if digit {
                0xFFFFFF
            } else {
                0xE6E6E6
            };
            let fill = if self.down == Some(i) && self.hover == Some(i) {
                darken(base, 40)
            } else if self.hover == Some(i) {
                darken(base, 16)
            } else {
                base
            };
            s.fill_rect(*r, fill);
            let (size, bold) = if digit { (13, true) } else { (12, false) };
            font::draw_in(s, *r, label, size, bold, if eq { 0xFFFFFF } else { 0x1A1A1A }, Align::Center);
        }
    }

    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
        let b = match ev.key {
            Key::Enter => Some(Btn::Eq),
            Key::Backspace => Some(Btn::Back),
            Key::Escape => Some(Btn::C),
            Key::Delete => Some(Btn::CE),
            _ => ev.ch.and_then(btn_of_char),
        };
        if let Some(b) = b {
            self.c.press(b);
            cx.redraw = true;
        }
    }

    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
        let (hover, down) = match ev {
            MouseEv::RightDown { .. } | MouseEv::Wheel { .. } => return,
            MouseEv::Move { x, y } => (self.hit(x, y), self.down),
            MouseEv::Down { x, y } => {
                let h = self.hit(x, y);
                (h, h)
            }
            MouseEv::Up { x, y } => {
                let h = self.hit(x, y);
                if let (Some(i), true) = (h, h == self.down) {
                    self.c.press(GRID[i].1);
                }
                (h, None)
            }
            MouseEv::Leave => (None, None),
        };
        if hover != self.hover || down != self.down || matches!(ev, MouseEv::Up { .. }) {
            cx.redraw = true;
        }
        self.hover = hover;
        self.down = down;
    }

    fn tick(&mut self, _cx: &mut Ctx) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::TestPlatform;

    fn run(c: &mut Calc, s: &str) {
        for ch in s.chars() {
            c.press(btn_of_char(ch).unwrap());
        }
    }
    fn calc(s: &str) -> Calc {
        let mut c = Calc::new();
        run(&mut c, s);
        c
    }

    #[test]
    fn basico() {
        assert_eq!(calc("2+3=").text(), "5");
        assert_eq!(calc("2+3*4=").text(), "20");
        assert_eq!(calc("0,1+0,2=").text(), "0,3");
        assert_eq!(calc("7/2=").text(), "3,5");
        assert_eq!(calc("12+").expr, "12 +");
        assert_eq!(calc("12+3=").expr, "12 + 3 =");
        // Tras = la siguiente cifra empieza número nuevo.
        assert_eq!(calc("2+3=7").text(), "7");
        // Operador repetido solo cambia la operación.
        assert_eq!(calc("5+*3=").text(), "15");
    }

    #[test]
    fn errores() {
        let mut c = calc("5/0=");
        assert_eq!(c.text(), DIV0);
        c.press(Btn::Digit(4));
        assert_eq!(c.text(), "4");
        let mut c = calc("9");
        c.press(Btn::Sqrt);
        assert_eq!(c.text(), "3");
        let mut c = calc("1");
        c.press(Btn::Neg);
        c.press(Btn::Sqrt);
        assert_eq!(c.text(), INVALID);
        c.press(Btn::C);
        assert_eq!(c.text(), "0");
        let mut c = calc("0");
        c.press(Btn::Inv);
        assert_eq!(c.text(), DIV0);
    }

    #[test]
    fn funciones() {
        let mut c = calc("5");
        c.press(Btn::Neg);
        assert_eq!(c.text(), "-5");
        assert_eq!(calc("200+10%=").text(), "220");
        let mut c = calc("4");
        c.press(Btn::Inv);
        assert_eq!(c.text(), "0,25");
        let mut c = calc("12");
        c.press(Btn::Sq);
        assert_eq!(c.text(), "144");
        let mut c = calc("5%");
        assert_eq!(c.text(), "0");
        c.press(Btn::Back);
        assert_eq!(c.text(), "0");
    }

    #[test]
    fn edicion() {
        let mut c = calc("123");
        c.press(Btn::Back);
        assert_eq!(c.text(), "12");
        let mut c = calc("5+3");
        c.press(Btn::CE);
        assert_eq!(c.text(), "0");
        assert_eq!(c.expr, "5 +");
        run(&mut c, "2=");
        assert_eq!(c.text(), "7");
        let mut c = calc("5+3");
        c.press(Btn::C);
        assert_eq!((c.text(), c.expr.as_str()), ("0", ""));
        // Límite de 16 cifras.
        assert_eq!(calc("12345678901234567890").text(), "1234567890123456");
        assert_eq!(calc("0,12345678901234567").digits(), 16);
    }

    #[test]
    fn formato() {
        assert_eq!(calc("9999999999999999*10=").text(), "1e17");
        assert_eq!(calc("1/1000000000000=").text(), "1e-12");
        assert_eq!(fmt(-0.0), "0");
        assert_eq!(fmt(1.0 / 3.0), "0,3333333333");
        assert_eq!(fmt(-2.5), "-2,5");
        assert_eq!(calc("0,1-0,1=").text(), "0");
        assert_eq!(calc("0*5-=").text(), "0");
    }

    fn tecla(key: Key, ch: Option<char>) -> KeyEvent {
        KeyEvent { key, ch, down: true, shift: false, ctrl: false, alt: false }
    }

    fn con_ctx(f: impl FnOnce(&mut Ctx)) {
        let mut sys = TestPlatform::default();
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        f(&mut cx);
    }

    #[test]
    fn teclado() {
        let mut app = CalcApp { c: Calc::new(), rects: Vec::new(), hover: None, down: None };
        con_ctx(|cx| {
            for c in "2+3".chars() {
                app.key(&tecla(Key::Char(c), Some(c)), cx);
            }
            app.key(&tecla(Key::Enter, None), cx);
            assert!(cx.redraw);
        });
        assert_eq!(app.c.text(), "5");
        con_ctx(|cx| {
            app.key(&tecla(Key::Char('7'), Some('7')), cx);
            app.key(&tecla(Key::Backspace, None), cx);
            assert_eq!(app.c.text(), "0");
            app.key(&tecla(Key::Char('9'), Some('9')), cx);
            app.key(&tecla(Key::Delete, None), cx);
            assert_eq!(app.c.text(), "0");
            app.key(&tecla(Key::Char('9'), Some('9')), cx);
            app.key(&tecla(Key::Escape, None), cx);
            assert_eq!(app.c.text(), "0");
            app.key(&tecla(Key::Char('a'), Some('a')), cx);
        });
    }

    #[test]
    fn raton_y_dibujo() {
        let mut sys = TestPlatform::default();
        let th = Theme::default();
        let mut s = Surface::new(320, 470, 0);
        let mut app = CalcApp { c: Calc::new(), rects: Vec::new(), hover: None, down: None };
        app.draw(&mut s, &th, &mut sys, true);
        let centro = |a: &CalcApp, label: &str| {
            let r = a.rects[GRID.iter().position(|g| g.0 == label).unwrap()];
            (r.x + r.w / 2, r.y + r.h / 2)
        };
        for l in ["7", "+", "5", "="] {
            let (x, y) = centro(&app, l);
            con_ctx(|cx| {
                app.mouse(MouseEv::Down { x, y }, cx);
                app.mouse(MouseEv::Up { x, y }, cx);
            });
        }
        assert_eq!(app.c.text(), "12");
        // Bajar en un botón y soltar en otro no pulsa.
        let (x1, y1) = centro(&app, "1");
        let (x2, y2) = centro(&app, "2");
        con_ctx(|cx| {
            app.mouse(MouseEv::Down { x: x1, y: y1 }, cx);
            app.mouse(MouseEv::Up { x: x2, y: y2 }, cx);
        });
        assert_eq!(app.c.text(), "12");
        // Tamaños mínimo y diminuto.
        for (w, h) in [(240, 340), (1, 1), (0, 0), (30, 500)] {
            let mut s = Surface::new(w, h, 0);
            app.draw(&mut s, &th, &mut sys, false);
            con_ctx(|cx| {
                app.mouse(MouseEv::Move { x: 5, y: 5 }, cx);
                app.mouse(MouseEv::Leave, cx);
            });
        }
    }
}
