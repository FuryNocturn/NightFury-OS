//! Bloc de notas: editor de texto plano de varias líneas, sin ajuste de línea ni selección.
//! CONTRATO: `pub fn new() -> Box<dyn App>`.
use super::{App, AppKind, Ctx, MouseEv};
use crate::dialogs::{Answer, Buttons, DialogRequest, DialogResult};
use crate::files::{self, DOCUMENTS_DIR};
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
const MENU_H: i32 = 24;
const ITEM_H: i32 = 26;
const DROP_W: i32 = 270;
const MAX_FILE: usize = 1 << 20;
const TAG_OPEN: u32 = 1;
const TAG_SAVE: u32 = 2;
const TAG_ASK: u32 = 3;
const TAG_MSG: u32 = 4;
const TITLE: &str = "Bloc de notas";

/// Entradas del menú Archivo: (texto, atajo).
const ITEMS: [(&str, &str); 5] =
    [("Nuevo", "Ctrl+N"), ("Abrir…", "Ctrl+O"), ("Guardar", "Ctrl+S"), ("Guardar como…", "Ctrl+Mayús+S"), ("Salir", "")];

/// Acción que espera a que se resuelva "¿guardar los cambios?".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pending {
    Ninguna,
    Nuevo,
    Abrir,
    Cerrar,
}

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
    path: Option<String>,
    modified: bool,
    pending: Pending,
    menu_open: bool,
    hover: Option<usize>,
}

fn blank() -> Notepad {
    Notepad {
        ed: Editor::new(),
        top: 0,
        scroll_x: 0,
        vis: 10,
        w: 0,
        h: 0,
        focused: false,
        t0: 0,
        shown: true,
        path: None,
        modified: false,
        pending: Pending::Ninguna,
        menu_open: false,
        hover: None,
    }
}

pub fn new() -> Box<dyn App> {
    Box::new(blank())
}

/// Texto del archivo -> líneas (acepta `\r\n` y `\n`; tabuladores a 4 espacios).
fn parse(text: &str) -> Vec<Vec<char>> {
    text.split('\n')
        .map(|l| {
            let l = l.strip_suffix('\r').unwrap_or(l);
            l.chars().flat_map(|c| if c == '\t' { vec![' '; 4] } else { vec![c] }).collect()
        })
        .collect()
}

fn message(cx: &mut Ctx, text: &str) {
    cx.dialog = Some(DialogRequest::Message {
        tag: TAG_MSG,
        title: String::from(TITLE),
        text: String::from(text),
        buttons: Buttons::Ok,
    });
}

fn line_text(l: &[char]) -> String {
    l.iter().collect()
}

impl Notepad {
    fn lh() -> i32 {
        font::line_h(SIZE)
    }

    fn doc_name(&self) -> &str {
        self.path.as_deref().map_or("Sin título", files::file_name)
    }

    fn doc_dir(&self) -> String {
        self.path.as_deref().and_then(files::parent).unwrap_or_else(|| String::from(DOCUMENTS_DIR))
    }

    /// Ejecuta una acción ya autorizada.
    fn run(&mut self, a: Pending, cx: &mut Ctx) {
        match a {
            Pending::Ninguna => {}
            Pending::Nuevo => {
                self.ed = Editor::new();
                self.top = 0;
                self.scroll_x = 0;
                self.path = None;
                self.modified = false;
            }
            Pending::Abrir => {
                cx.dialog = Some(DialogRequest::Open { tag: TAG_OPEN, title: String::from("Abrir"), dir: self.doc_dir() });
            }
            Pending::Cerrar => cx.close = true,
        }
        cx.redraw = true;
    }

    /// Pide confirmación si hay cambios sin guardar; si no, ejecuta la acción.
    fn guard(&mut self, a: Pending, cx: &mut Ctx) {
        if !self.modified {
            return self.run(a, cx);
        }
        self.pending = a;
        cx.dialog = Some(DialogRequest::Message {
            tag: TAG_ASK,
            title: String::from(TITLE),
            text: format!("¿Quieres guardar los cambios de {}?", self.doc_name()),
            buttons: Buttons::YesNoCancel,
        });
    }

    fn save_as(&mut self, cx: &mut Ctx) {
        let name = self.path.as_deref().map_or("Sin título.txt", files::file_name);
        cx.dialog = Some(DialogRequest::Save {
            tag: TAG_SAVE,
            title: String::from("Guardar como"),
            dir: self.doc_dir(),
            name: String::from(name),
        });
    }

    fn save(&mut self, cx: &mut Ctx) {
        match self.path.clone() {
            Some(p) => self.write_to(&p, cx),
            None => self.save_as(cx),
        }
    }

    /// Escribe el documento en `path`; si sale bien sigue con la acción pendiente.
    fn write_to(&mut self, path: &str, cx: &mut Ctx) {
        let mut text = String::new();
        for (i, l) in self.ed.lines.iter().enumerate() {
            if i > 0 {
                text.push_str("\r\n");
            }
            text.extend(l.iter());
        }
        let now = cx.sys.now();
        match cx.sys.files().write(path, text.as_bytes(), now) {
            Ok(()) => {
                self.path = Some(String::from(path));
                self.modified = false;
                let a = core::mem::replace(&mut self.pending, Pending::Ninguna);
                self.run(a, cx);
            }
            Err(e) => {
                self.pending = Pending::Ninguna;
                message(cx, e.message());
            }
        }
        cx.redraw = true;
    }

    fn load(&mut self, path: &str, cx: &mut Ctx) {
        let data = match cx.sys.files().read(path) {
            Ok(d) => d,
            Err(e) => return message(cx, e.message()),
        };
        if data.len() > MAX_FILE {
            return message(cx, "El archivo es demasiado grande para el Bloc de notas (máximo 1 MiB).");
        }
        let Ok(text) = core::str::from_utf8(&data) else {
            return message(cx, "Este archivo no es un archivo de texto.");
        };
        self.ed = Editor::new();
        self.ed.lines = parse(text);
        self.top = 0;
        self.scroll_x = 0;
        self.path = Some(String::from(path));
        self.modified = false;
        cx.redraw = true;
    }

    fn menu_action(&mut self, i: usize, cx: &mut Ctx) {
        self.menu_open = false;
        match i {
            0 => self.guard(Pending::Nuevo, cx),
            1 => self.guard(Pending::Abrir, cx),
            2 => self.save(cx),
            3 => self.save_as(cx),
            _ => self.guard(Pending::Cerrar, cx),
        }
        cx.redraw = true;
    }

    fn button_w() -> i32 {
        font::width("Archivo", SIZE, false) + 20
    }

    /// Entrada del desplegable bajo el punto (x, y), si hay.
    fn item_at(x: i32, y: i32) -> Option<usize> {
        let r = Rect::new(4, MENU_H, DROP_W, ITEMS.len() as i32 * ITEM_H + 4);
        if x < r.x || x >= r.x + r.w || y < r.y + 2 || y >= r.y + r.h - 2 {
            return None;
        }
        Some(((y - r.y - 2) / ITEM_H) as usize)
    }

    fn draw_menu(&self, s: &mut Surface) {
        s.fill_rect(Rect::new(0, 0, s.w, MENU_H), 0xF0F0F0);
        s.fill_rect(Rect::new(0, MENU_H - 1, s.w, 1), 0xD7D7D7);
        let b = Rect::new(4, 2, Self::button_w(), MENU_H - 4);
        if self.menu_open {
            s.fill_rect(b, 0xCCE4F7);
        }
        font::draw_in(s, b, "Archivo", SIZE, false, 0x000000, Align::Center);
        if !self.menu_open {
            return;
        }
        let r = Rect::new(4, MENU_H, DROP_W, ITEMS.len() as i32 * ITEM_H + 4);
        s.fill_rect(r, 0x808080);
        s.fill_rect(Rect::new(r.x + 1, r.y + 1, r.w - 2, r.h - 2), 0xFAFAFA);
        for (i, (name, key)) in ITEMS.iter().enumerate() {
            let row = Rect::new(r.x + 2, r.y + 2 + i as i32 * ITEM_H, r.w - 4, ITEM_H);
            if self.hover == Some(i) {
                s.fill_rect(row, 0xCCE4F7);
            }
            font::draw_in(s, Rect::new(row.x + 10, row.y, row.w - 20, row.h), name, SIZE, false, 0x000000, Align::Left);
            font::draw_in(s, Rect::new(row.x + 10, row.y, row.w - 20, row.h), key, 9, false, 0x606060, Align::Right);
        }
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
        format!("{}{}: {}", if self.modified { "*" } else { "" }, self.doc_name(), TITLE)
    }
    fn min_size(&self) -> (i32, i32) {
        (240, 140)
    }

    fn draw(&mut self, s: &mut Surface, _theme: &Theme, _sys: &mut dyn Platform, focused: bool) {
        self.w = s.w;
        self.h = s.h;
        self.focused = focused;
        let text_h = (s.h - STATUS_H - MENU_H).max(0);
        self.follow_cursor(s.w, text_h);
        let lh = Self::lh();
        s.fill_rect(s.bounds(), 0xFFFFFF);

        s.set_clip(Rect::new(0, MENU_H, s.w, text_h));
        let last = (self.top + self.vis + 1).min(self.ed.lines.len());
        for row in self.top..last {
            let y = MENU_H + (row - self.top) as i32 * lh;
            let t = line_text(&self.ed.lines[row]);
            font::draw(s, MARGIN - self.scroll_x, y, &t, SIZE, false, 0x000000);
        }
        if focused && self.shown {
            let x = MARGIN - self.scroll_x + self.x_of(self.ed.row, self.ed.col);
            let y = MENU_H + (self.ed.row - self.top) as i32 * lh;
            s.fill_rect(Rect::new(x, y + 1, 1, lh - 2), 0x000000);
        }
        s.reset_clip();

        // Barra de estado.
        let by = text_h + MENU_H;
        let bar = Rect::new(0, by, s.w, s.h - by);
        s.fill_rect(bar, 0xF0F0F0);
        s.fill_rect(Rect::new(0, by, s.w, 1), 0xD7D7D7);
        let left = format!("{} caracteres", self.ed.char_count());
        let right = format!("Ln {}, Col {}", self.ed.row + 1, self.ed.col + 1);
        font::draw_in(s, Rect::new(MARGIN, by, s.w - 2 * MARGIN, STATUS_H), &left, 9, false, 0x404040, Align::Left);
        font::draw_in(s, Rect::new(MARGIN, by, s.w - 2 * MARGIN, STATUS_H), &right, 9, false, 0x404040, Align::Right);
        self.draw_menu(s);
    }

    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
        let page = self.vis.max(1) as i32;
        if ev.key == Key::Escape && self.menu_open {
            self.menu_open = false;
            cx.redraw = true;
            return;
        }
        if ev.ctrl {
            if let Key::Char(c) = ev.key {
                match c.to_ascii_lowercase() {
                    'n' => self.guard(Pending::Nuevo, cx),
                    'o' => self.guard(Pending::Abrir, cx),
                    's' if ev.shift => self.save_as(cx),
                    's' => self.save(cx),
                    _ => return,
                }
                self.menu_open = false;
                cx.redraw = true;
                return;
            }
        }
        let editing = matches!(ev.key, Key::Enter | Key::Backspace | Key::Delete | Key::Tab)
            || matches!(ev.ch, Some(c) if !ev.ctrl && !c.is_control());
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
        self.modified |= editing;
        self.menu_open = false;
        self.touch(cx);
    }

    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
        match ev {
            MouseEv::Move { x, y } if self.menu_open => {
                let h = Self::item_at(x, y);
                if h != self.hover {
                    self.hover = h;
                    cx.redraw = true;
                }
                return;
            }
            MouseEv::Leave if self.menu_open => {
                self.hover = None;
                cx.redraw = true;
                return;
            }
            MouseEv::Down { x, y } => {
                if y < MENU_H && x >= 4 && x < 4 + Self::button_w() {
                    self.menu_open = !self.menu_open;
                    self.hover = None;
                    cx.redraw = true;
                    return;
                }
                if self.menu_open {
                    match Self::item_at(x, y) {
                        Some(i) => self.menu_action(i, cx),
                        None => {
                            self.menu_open = false;
                            cx.redraw = true;
                        }
                    }
                    return;
                }
            }
            _ => {}
        }
        let MouseEv::Down { x, y } = ev else { return };
        if y < MENU_H || y >= self.h - STATUS_H {
            return;
        }
        let y = y - MENU_H;
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

    fn dialog_result(&mut self, result: DialogResult, cx: &mut Ctx) {
        match result {
            DialogResult::Answer { tag: TAG_ASK, answer } => match answer {
                Answer::Yes => self.save(cx),
                Answer::No => {
                    let a = core::mem::replace(&mut self.pending, Pending::Ninguna);
                    self.run(a, cx);
                }
                _ => self.pending = Pending::Ninguna,
            },
            DialogResult::Path { tag: TAG_OPEN, path: Some(p) } => self.load(&p, cx),
            DialogResult::Path { tag: TAG_SAVE, path: Some(p) } => self.write_to(&p, cx),
            DialogResult::Path { tag: TAG_SAVE, path: None } => self.pending = Pending::Ninguna,
            _ => {}
        }
    }

    fn open_path(&mut self, path: &str, cx: &mut Ctx) {
        self.load(path, cx);
    }

    fn request_close(&mut self, cx: &mut Ctx) -> bool {
        if !self.modified {
            return true;
        }
        self.guard(Pending::Cerrar, cx);
        false
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
            let mut np = Notepad { w: 100, h: 100, focused: true, ..blank() };
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
            let mut np = Notepad { w: 300, h: 200, focused: true, ..blank() };
            escribe(&mut np.ed, "hola\nmundo");
            let lh = Notepad::lh();
            let x3 = MARGIN + font::width("mun", SIZE, false);
            np.mouse(MouseEv::Down { x: x3 + 1, y: MENU_H + lh + 2 }, cx);
            assert_eq!((np.ed.row, np.ed.col), (1, 3));
            np.mouse(MouseEv::Down { x: 0, y: MENU_H + 2 }, cx);
            assert_eq!((np.ed.row, np.ed.col), (0, 0));
            np.mouse(MouseEv::Down { x: 290, y: 190 }, cx); // barra de estado: se ignora
            assert_eq!((np.ed.row, np.ed.col), (0, 0));
            np.mouse(MouseEv::Down { x: 290, y: MENU_H + 150 }, cx); // más allá del texto
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
        let mut np = Notepad { focused: true, ..blank() };
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
            let mut np = Notepad { w: 100, h: 100, focused: true, ..blank() };
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

    fn ctrl(c: char, shift: bool) -> KeyEvent {
        KeyEvent { key: Key::Char(c), ch: None, down: true, shift, ctrl: true, alt: false }
    }

    fn resp(tag: u32, a: Answer) -> DialogResult {
        DialogResult::Answer { tag, answer: a }
    }

    fn ruta(p: &str) -> DialogResult {
        DialogResult::Path { tag: TAG_SAVE, path: Some(String::from(p)) }
    }

    const DOC: &str = "C:\\Documentos\\a.txt";

    #[test]
    fn guardar_nuevo_escribe_crlf_y_actualiza_titulo() {
        let mut sys = TestPlatform::default();
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        let mut np = blank();
        assert_eq!(np.title(), "Sin título: Bloc de notas");
        escribe(&mut np.ed, "uno\ndos ñ");
        np.key(&ev(Key::Char('x'), Some('x')), &mut cx);
        assert_eq!(np.title(), "*Sin título: Bloc de notas");
        np.key(&ctrl('s', false), &mut cx);
        let Some(DialogRequest::Save { dir, name, .. }) = cx.dialog.take() else { panic!("sin Guardar como") };
        assert_eq!((dir.as_str(), name.as_str()), (DOCUMENTS_DIR, "Sin título.txt"));
        np.dialog_result(ruta(DOC), &mut cx);
        assert_eq!(cx.sys.files().read(DOC).unwrap(), "uno\r\ndos ñx".as_bytes());
        assert!(!np.modified);
        assert_eq!(np.title(), "a.txt: Bloc de notas");
        // Con ruta, Ctrl+S escribe sin diálogo.
        np.key(&ev(Key::Char('y'), Some('y')), &mut cx);
        np.key(&ctrl('s', false), &mut cx);
        assert!(cx.dialog.is_none() && !np.modified);
        assert_eq!(cx.sys.files().read(DOC).unwrap(), "uno\r\ndos ñxy".as_bytes());
        // Guardar como propone el nombre actual en su carpeta.
        np.key(&ctrl('s', true), &mut cx);
        let Some(DialogRequest::Save { dir, name, .. }) = cx.dialog.take() else { panic!() };
        assert_eq!((dir.as_str(), name.as_str()), (DOCUMENTS_DIR, "a.txt"));
    }

    #[test]
    fn abrir_acepta_lf_y_tabuladores() {
        let mut sys = TestPlatform::default();
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        let now = cx.sys.now();
        cx.sys.files().write(DOC, "a\tb\nc\r\nd".as_bytes(), now).unwrap();
        let mut np = blank();
        np.key(&ctrl('o', false), &mut cx);
        let Some(DialogRequest::Open { dir, .. }) = cx.dialog.take() else { panic!() };
        assert_eq!(dir, DOCUMENTS_DIR);
        np.dialog_result(DialogResult::Path { tag: TAG_OPEN, path: Some(String::from(DOC)) }, &mut cx);
        assert_eq!(texto(&np.ed), "a    b\nc\nd");
        assert!(!np.modified && np.title() == "a.txt: Bloc de notas");
    }

    #[test]
    fn rechaza_no_utf8_y_demasiado_grande() {
        let mut sys = TestPlatform::default();
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        let now = cx.sys.now();
        cx.sys.files().write(DOC, &[0xFF, 0xFE, 0x41], now).unwrap();
        cx.sys.files().write("C:\\Documentos\\g.txt", &vec![b'a'; MAX_FILE + 1], now).unwrap();
        let mut np = blank();
        escribe(&mut np.ed, "previo");
        np.open_path(DOC, &mut cx);
        assert!(matches!(cx.dialog.take(), Some(DialogRequest::Message { text, .. }) if text.contains("no es un archivo de texto")));
        np.open_path("C:\\Documentos\\g.txt", &mut cx);
        assert!(matches!(cx.dialog.take(), Some(DialogRequest::Message { text, .. }) if text.contains("1 MiB")));
        assert_eq!(texto(&np.ed), "previo");
        assert!(np.path.is_none());
    }

    #[test]
    fn open_path_carga_el_archivo() {
        let mut sys = TestPlatform::default();
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        let now = cx.sys.now();
        cx.sys.files().write(DOC, b"hola", now).unwrap();
        let mut np = new();
        np.open_path(DOC, &mut cx);
        assert_eq!(np.title(), "a.txt: Bloc de notas");
        assert!(cx.dialog.is_none());
    }

    #[test]
    fn cerrar_con_cambios_pregunta() {
        let mut sys = TestPlatform::default();
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        let mut np = blank();
        assert!(np.request_close(&mut cx) && cx.dialog.is_none());
        np.key(&ev(Key::Char('a'), Some('a')), &mut cx);
        assert!(!np.request_close(&mut cx));
        let Some(DialogRequest::Message { tag, buttons, text, .. }) = cx.dialog.take() else { panic!() };
        assert_eq!((tag, buttons), (TAG_ASK, Buttons::YesNoCancel));
        assert_eq!(text, "¿Quieres guardar los cambios de Sin título?");
        // Cancelar: nada.
        np.dialog_result(resp(TAG_ASK, Answer::Cancel), &mut cx);
        assert!(!cx.close && np.modified && np.pending == Pending::Ninguna);
        // No: cierra sin guardar.
        np.request_close(&mut cx);
        cx.dialog = None;
        np.dialog_result(resp(TAG_ASK, Answer::No), &mut cx);
        assert!(cx.close);
        // Sí: Guardar como, y al guardar cierra.
        cx.close = false;
        np.request_close(&mut cx);
        cx.dialog = None;
        np.dialog_result(resp(TAG_ASK, Answer::Yes), &mut cx);
        assert!(matches!(cx.dialog.take(), Some(DialogRequest::Save { .. })) && !cx.close);
        np.dialog_result(ruta(DOC), &mut cx);
        assert!(cx.close && !np.modified);
    }

    #[test]
    fn guardar_como_cancelado_descarta_la_accion() {
        let mut sys = TestPlatform::default();
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        let mut np = blank();
        np.key(&ev(Key::Char('a'), Some('a')), &mut cx);
        np.key(&ctrl('n', false), &mut cx);
        cx.dialog = None;
        np.dialog_result(resp(TAG_ASK, Answer::Yes), &mut cx);
        cx.dialog = None;
        np.dialog_result(DialogResult::Path { tag: TAG_SAVE, path: None }, &mut cx);
        assert_eq!(np.pending, Pending::Ninguna);
        assert!(np.modified && texto(&np.ed) == "a");
    }

    #[test]
    fn nuevo_y_error_de_escritura() {
        let mut sys = TestPlatform::default();
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        let mut np = blank();
        np.key(&ev(Key::Char('a'), Some('a')), &mut cx);
        // Error de escritura (carpeta inexistente): mensaje y sigue modificado.
        np.dialog_result(ruta("C:\\NoExiste\\x.txt"), &mut cx);
        assert!(matches!(cx.dialog.take(), Some(DialogRequest::Message { tag: TAG_MSG, .. })));
        assert!(np.modified && np.path.is_none());
        // Nuevo + No: documento vacío.
        np.key(&ctrl('n', false), &mut cx);
        np.dialog_result(resp(TAG_ASK, Answer::No), &mut cx);
        assert_eq!(texto(&np.ed), "");
        assert!(!np.modified);
    }

    #[test]
    fn menu_con_raton() {
        let mut sys = TestPlatform::default();
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        let mut np = Notepad { w: 400, h: 300, focused: true, ..blank() };
        np.mouse(MouseEv::Down { x: 10, y: 5 }, &mut cx);
        assert!(np.menu_open);
        np.mouse(MouseEv::Move { x: 20, y: MENU_H + 2 + ITEM_H + 3 }, &mut cx);
        assert_eq!(np.hover, Some(1));
        let mut s = Surface::new(400, 300, 0);
        np.draw(&mut s, &Theme::default(), &mut sys_dummy(), true);
        np.mouse(MouseEv::Down { x: 350, y: 200 }, &mut cx); // fuera: cierra
        assert!(!np.menu_open && cx.dialog.is_none());
        np.mouse(MouseEv::Down { x: 10, y: 5 }, &mut cx);
        np.key(&ev(Key::Escape, None), &mut cx);
        assert!(!np.menu_open);
        np.mouse(MouseEv::Down { x: 10, y: 5 }, &mut cx);
        np.mouse(MouseEv::Down { x: 20, y: MENU_H + 2 + ITEM_H + 3 }, &mut cx); // Abrir…
        assert!(matches!(cx.dialog.take(), Some(DialogRequest::Open { .. })) && !np.menu_open);
        np.mouse(MouseEv::Down { x: 10, y: 5 }, &mut cx);
        np.mouse(MouseEv::Down { x: 20, y: MENU_H + 2 + 4 * ITEM_H + 3 }, &mut cx); // Salir
        assert!(cx.close);
    }

    fn sys_dummy() -> TestPlatform {
        TestPlatform::default()
    }
}