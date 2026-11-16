//! Bloc de notas: editor de texto plano de varias líneas, sin ajuste de línea ni selección.
//! CONTRATO: `pub fn new() -> Box<dyn App>`.
use super::{App, AppKind, Ctx, MouseEv};
use crate::font::{self, Align};
use crate::geom::Rect;
use crate::input::{Key, KeyEvent};
use crate::platform::Platform;
use crate::surface::Surface;
use crate::theme::Theme;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

const SIZE: u32 = 10;
const MARGIN: i32 = 6;
const STATUS_H: i32 = 22;
const BLINK_MS: u64 = 530;

/// Texto y cursor, sin nada de dibujo.
struct Editor {
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
    /// Columna deseada al subir y bajar por líneas más cortas.
    want: usize,
}

impl Editor {
    fn new() -> Self {
        Editor { lines: vec![Vec::new()], row: 0, col: 0, want: 0 }
    }

    fn insert(&mut self, c: char) {
        self.lines[self.row].insert(self.col, c);
        self.col += 1;
        self.want = self.col;
    }

    fn newline(&mut self) {
        let rest = self.lines[self.row].split_off(self.col);
        self.row += 1;
        self.lines.insert(self.row, rest);
        self.col = 0;
        self.want = 0;
    }

    fn backspace(&mut self) {
        if self.col > 0 {
            self.col -= 1;
            self.lines[self.row].remove(self.col);
        } else if self.row > 0 {
            let cur = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].len();
            self.lines[self.row].extend(cur);
        }
        self.want = self.col;
    }

    fn delete(&mut self) {
        if self.col < self.lines[self.row].len() {
            self.lines[self.row].remove(self.col);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].extend(next);
        }
    }

    fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].len();
        }
        self.want = self.col;
    }

    fn right(&mut self) {
        if self.col < self.lines[self.row].len() {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
        self.want = self.col;
    }

    /// Mueve el cursor `n` líneas (negativo = arriba) conservando la columna deseada.
    fn vertical(&mut self, n: i32) {
        let r = (self.row as i32 + n).clamp(0, self.lines.len() as i32 - 1) as usize;
        self.row = r;
        self.col = self.want.min(self.lines[r].len());
    }

    fn home(&mut self) {
        self.col = 0;
        self.want = 0;
    }

    fn end(&mut self) {
        self.col = self.lines[self.row].len();
        self.want = self.col;
    }

    fn set_pos(&mut self, row: usize, col: usize) {
        self.row = row.min(self.lines.len() - 1);
        self.col = col.min(self.lines[self.row].len());
        self.want = self.col;
    }

    /// Caracteres del texto, contando un salto entre líneas.
    fn char_count(&self) -> usize {
        self.lines.iter().map(|l| l.len()).sum::<usize>() + self.lines.len() - 1
    }
}

pub struct Notepad {
    ed: Editor,
    top: usize,
    scroll_x: i32,
    /// Líneas que caben a la vista (última vez dibujado).
    vis: usize,
    w: i32,
    h: i32,
    focused: bool,
    /// Instante desde el que cuenta el parpadeo.
    t0: u64,
    shown: bool,
}

pub fn new() -> Box<dyn App> {
    Box::new(Notepad { ed: Editor::new(), top: 0, scroll_x: 0, vis: 10, w: 0, h: 0, focused: false, t0: 0, shown: true })
}

fn line_text(l: &[char]) -> String {
    l.iter().collect()
}

impl Notepad {
    fn lh() -> i32 {
        font::line_h(SIZE)
    }

    fn touch(&mut self, cx: &mut Ctx) {
        self.t0 = cx.now_ms;
        self.shown = true;
        cx.redraw = true;
    }

    /// Ancho en píxeles de los primeros `n` caracteres de la línea `row`.
    fn x_of(&self, row: usize, n: usize) -> i32 {
        self.ed.lines[row].iter().take(n).map(|&c| font::adv(c, SIZE, false)).sum()
    }

    fn follow_cursor(&mut self, view_w: i32, view_h: i32) {
        self.vis = ((view_h / Self::lh()).max(1)) as usize;
        if self.ed.row < self.top {
            self.top = self.ed.row;
        } else if self.ed.row >= self.top + self.vis {
            self.top = self.ed.row + 1 - self.vis;
        }
        let cx = self.x_of(self.ed.row, self.ed.col);
        let vw = (view_w - MARGIN - 2).max(1);
        if cx < self.scroll_x {
            self.scroll_x = cx;
        } else if cx > self.scroll_x + vw {
            self.scroll_x = cx - vw;
        }
        // Si todo el texto cabe, no queda desplazado.
        if self.scroll_x < 0 {
            self.scroll_x = 0;
        }
    }
}

impl App for Notepad {
    fn kind(&self) -> AppKind {
        AppKind::Notepad
    }
    fn title(&self) -> String {
        String::from("Sin título: Bloc de notas")
    }
    fn min_size(&self) -> (i32, i32) {
        (240, 140)
    }

    fn draw(&mut self, s: &mut Surface, _theme: &Theme, _sys: &mut dyn Platform, focused: bool) {
        self.w = s.w;
        self.h = s.h;
        self.focused = focused;
        let text_h = (s.h - STATUS_H).max(0);
        self.follow_cursor(s.w, text_h);
        let lh = Self::lh();
        s.fill_rect(s.bounds(), 0xFFFFFF);

        s.set_clip(Rect::new(0, 0, s.w, text_h));
        let last = (self.top + self.vis + 1).min(self.ed.lines.len());
        for row in self.top..last {
            let y = (row - self.top) as i32 * lh;
            let t = line_text(&self.ed.lines[row]);
            font::draw(s, MARGIN - self.scroll_x, y, &t, SIZE, false, 0x000000);
        }
        if focused && self.shown {
            let x = MARGIN - self.scroll_x + self.x_of(self.ed.row, self.ed.col);
            let y = (self.ed.row - self.top) as i32 * lh;
            s.fill_rect(Rect::new(x, y + 1, 1, lh - 2), 0x000000);
        }
        s.reset_clip();

        // Barra de estado.
        let bar = Rect::new(0, text_h, s.w, s.h - text_h);
        s.fill_rect(bar, 0xF0F0F0);
        s.fill_rect(Rect::new(0, text_h, s.w, 1), 0xD7D7D7);
        let left = format!("{} caracteres", self.ed.char_count());
        let right = format!("Ln {}, Col {}", self.ed.row + 1, self.ed.col + 1);
        font::draw_in(s, Rect::new(MARGIN, text_h, s.w - 2 * MARGIN, STATUS_H), &left, 9, false, 0x404040, Align::Left);
        font::draw_in(s, Rect::new(MARGIN, text_h, s.w - 2 * MARGIN, STATUS_H), &right, 9, false, 0x404040, Align::Right);
    }

    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
        let page = self.vis.max(1) as i32;
        match ev.key {
            Key::Enter => self.ed.newline(),
            Key::Backspace => self.ed.backspace(),
            Key::Delete => self.ed.delete(),
            Key::Tab => (0..4).for_each(|_| self.ed.insert(' ')),
            Key::Left => self.ed.left(),
            Key::Right => self.ed.right(),
            Key::Up => self.ed.vertical(-1),
            Key::Down => self.ed.vertical(1),
            Key::Home => self.ed.home(),
            Key::End => self.ed.end(),
            Key::PageUp => self.ed.vertical(-page),
            Key::PageDown => self.ed.vertical(page),
            _ => match ev.ch {
                Some(c) if !ev.ctrl && !c.is_control() => self.ed.insert(c),
                _ => return,
            },
        }
        self.touch(cx);
    }

    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
        let MouseEv::Down { x, y } = ev else { return };
        if y >= self.h - STATUS_H {
            return;
        }
        let row = (self.top as i32 + y.max(0) / Self::lh()) as usize;
        let row = row.min(self.ed.lines.len() - 1);
        // Carácter más cercano: el borde se parte por la mitad de cada glifo.
        let px = x - MARGIN + self.scroll_x;
        let (mut acc, mut col) = (0, 0);
        for &c in &self.ed.lines[row] {
            let a = font::adv(c, SIZE, false);
            if px < acc + a / 2 {
                break;
            }
            acc += a;
            col += 1;
        }
        self.ed.set_pos(row, col);
        self.touch(cx);
    }

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

    fn ev(key: Key, ch: Option<char>) -> KeyEvent {
        KeyEvent { key, ch, down: true, shift: false, ctrl: false, alt: false }
    }

    fn escribe(ed: &mut Editor, t: &str) {
        for c in t.chars() {
            if c == '\n' {
                ed.newline()
            } else {
                ed.insert(c)
            }
        }
    }

    fn texto(ed: &Editor) -> String {
        let l: Vec<String> = ed.lines.iter().map(|l| line_text(l)).collect();
        l.join("\n")
    }

    #[test]
    fn inserta_parte_y_une() {
        let mut ed = Editor::new();
        escribe(&mut ed, "hola\nmundo");
        assert_eq!(texto(&ed), "hola\nmundo");
        ed.home();
        ed.backspace(); // une con la línea anterior
        assert_eq!(texto(&ed), "holamundo");
        assert_eq!((ed.row, ed.col), (0, 4));
        ed.newline();
        assert_eq!(texto(&ed), "hola\nmundo");
        ed.vertical(-1);
        ed.end();
        ed.delete(); // une con la siguiente
        assert_eq!(texto(&ed), "holamundo");
    }

    #[test]
    fn extremos() {
        let mut ed = Editor::new();
        ed.backspace();
        ed.delete();
        ed.left();
        ed.right();
        ed.vertical(-3);
        ed.vertical(3);
        assert_eq!((ed.row, ed.col), (0, 0));
        assert_eq!(ed.char_count(), 0);
        escribe(&mut ed, "ab\ncd");
        ed.right(); // final del texto: no se mueve
        assert_eq!((ed.row, ed.col), (1, 2));
        ed.home();
        ed.left(); // salta al final de la línea anterior
        assert_eq!((ed.row, ed.col), (0, 2));
        ed.right(); // salta al principio de la siguiente
        assert_eq!((ed.row, ed.col), (1, 0));
        assert_eq!(ed.char_count(), 5);
    }

    #[test]
    fn columna_se_conserva() {
        let mut ed = Editor::new();
        escribe(&mut ed, "abcdef\nab\nabcdef");
        ed.set_pos(0, 5);
        ed.vertical(1);
        assert_eq!(ed.col, 2);
        ed.vertical(1);
        assert_eq!(ed.col, 5);
    }

    #[test]
    fn no_ascii() {
        let mut ed = Editor::new();
        escribe(&mut ed, "año ñandú\ncañón ¿qué?");
        assert_eq!(ed.char_count(), 9 + 1 + 11);
        ed.backspace();
        ed.backspace();
        assert_eq!(texto(&ed), "año ñandú\ncañón ¿qu");
    }

    fn con_cx<R>(f: impl FnOnce(&mut Ctx) -> R) -> R {
        let mut sys = TestPlatform::default();
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        f(&mut cx)
    }

    #[test]
    fn teclas_y_tab() {
        con_cx(|cx| {
            let mut np = Notepad { ed: Editor::new(), top: 0, scroll_x: 0, vis: 10, w: 100, h: 100, focused: true, t0: 0, shown: true };
            np.key(&ev(Key::Tab, None), cx);
            np.key(&ev(Key::Char('a'), Some('a')), cx);
            np.key(&ev(Key::Enter, Some('\n')), cx);
            np.key(&ev(Key::Char('ñ'), Some('ñ')), cx);
            assert_eq!(texto(&np.ed), "    a\nñ");
            assert!(cx.redraw);
        });
    }

    #[test]
    fn clic_coloca_el_cursor() {
        con_cx(|cx| {
            let mut np = Notepad { ed: Editor::new(), top: 0, scroll_x: 0, vis: 10, w: 300, h: 200, focused: true, t0: 0, shown: true };
            escribe(&mut np.ed, "hola\nmundo");
            let lh = Notepad::lh();
            let x3 = MARGIN + font::width("mun", SIZE, false);
            np.mouse(MouseEv::Down { x: x3 + 1, y: lh + 2 }, cx);
            assert_eq!((np.ed.row, np.ed.col), (1, 3));
            np.mouse(MouseEv::Down { x: 0, y: 2 }, cx);
            assert_eq!((np.ed.row, np.ed.col), (0, 0));
            np.mouse(MouseEv::Down { x: 290, y: 190 }, cx); // barra de estado: se ignora
            assert_eq!((np.ed.row, np.ed.col), (0, 0));
            np.mouse(MouseEv::Down { x: 290, y: 150 }, cx); // más allá del texto
            assert_eq!((np.ed.row, np.ed.col), (1, 5));
        });
    }

    #[test]
    fn dibuja_en_cualquier_tamano() {
        let mut sys = TestPlatform::default();
        let mut np = new();
        for (w, h) in [(240, 140), (1, 1), (0, 0), (10, 30), (640, 420)] {
            let mut s = Surface::new(w, h, 0);
            np.draw(&mut s, &Theme::default(), &mut sys, true);
            np.draw(&mut s, &Theme::default(), &mut sys, false);
        }
    }

    #[test]
    fn el_cursor_siempre_visible() {
        let mut sys = TestPlatform::default();
        let mut np = Notepad { ed: Editor::new(), top: 0, scroll_x: 0, vis: 10, w: 0, h: 0, focused: true, t0: 0, shown: true };
        for i in 0..40 {
            escribe(&mut np.ed, &format!("línea {} {}\n", i, "x".repeat(80)));
        }
        let mut s = Surface::new(240, 140, 0);
        np.draw(&mut s, &Theme::default(), &mut sys, true);
        assert!(np.top > 0 && np.ed.row >= np.top && np.ed.row < np.top + np.vis);
        np.ed.set_pos(5, 80);
        np.draw(&mut s, &Theme::default(), &mut sys, true);
        assert!(np.scroll_x > 0 && np.ed.row == 5 && np.top <= 5);
        np.ed.home();
        np.draw(&mut s, &Theme::default(), &mut sys, true);
        assert_eq!(np.scroll_x, 0);
    }

    #[test]
    fn parpadeo_solo_pide_redibujar_si_cambia() {
        con_cx(|cx| {
            let mut np = Notepad { ed: Editor::new(), top: 0, scroll_x: 0, vis: 10, w: 100, h: 100, focused: true, t0: 0, shown: true };
            cx.now_ms = 100;
            np.tick(cx);
            assert!(!cx.redraw);
            cx.now_ms = 600;
            np.tick(cx);
            assert!(cx.redraw && !np.shown);
            cx.redraw = false;
            cx.now_ms = 700;
            np.tick(cx);
            assert!(!cx.redraw);
            np.key(&ev(Key::Char('a'), Some('a')), cx);
            assert!(np.shown && cx.redraw);
        });
    }
}
