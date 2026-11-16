//! Cuadros de diálogo modales. CONTRATO: los tipos `Buttons`, `Answer`, `DialogRequest` y
//! `DialogResult` no cambian. Una app pide un diálogo con `cx.dialog = Some(..)` y recibe la
//! respuesta en `App::dialog_result`. `tag` lo elige la app para saber a qué petición corresponde
//! la respuesta. `Dialog` es el widget que dibuja y atiende el diálogo (lo usa el escritorio).
use crate::apps::MouseEv;
use crate::files;
use crate::font::{self, Align};
use crate::fs::{Entry, FsError};
use crate::geom::Rect;
use crate::icons::{self, Icon};
use crate::input::{Key, KeyEvent};
use crate::platform::Platform;
use crate::surface::Surface;
use crate::theme::Theme;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Buttons {
    /// "Aceptar".
    Ok,
    /// "Aceptar" y "Cancelar".
    OkCancel,
    /// "Sí" y "No".
    YesNo,
    /// "Sí", "No" y "Cancelar".
    YesNoCancel,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    Ok,
    Yes,
    No,
    /// También al cerrar el diálogo con su aspa o con Escape.
    Cancel,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DialogRequest {
    /// Mensaje o pregunta.
    Message { tag: u32, title: String, text: String, buttons: Buttons },
    /// Pide una línea de texto (p. ej. un nombre). `value` es el texto inicial.
    Input { tag: u32, title: String, label: String, value: String },
    /// Elegir un archivo existente. `dir` es la carpeta inicial ("C:\\Documentos").
    Open { tag: u32, title: String, dir: String },
    /// Elegir dónde guardar. `name` es el nombre propuesto. Si el archivo ya existe, el propio
    /// diálogo pregunta si se reemplaza antes de responder.
    Save { tag: u32, title: String, dir: String, name: String },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DialogResult {
    /// Respuesta a `Message`.
    Answer { tag: u32, answer: Answer },
    /// Respuesta a `Input`; `None` si se canceló.
    Text { tag: u32, value: Option<String> },
    /// Respuesta a `Open` y `Save`: ruta absoluta; `None` si se canceló.
    Path { tag: u32, path: Option<String> },
}

// ------------------------------------------------------------------ widget Dialog

const FS: u32 = 9;
const PAD: i32 = 20;
const BTN_W: i32 = 88;
const BTN_H: i32 = 28;
const ROW_H: i32 = 24;
const INK: u32 = 0x1A1A1A;
const MUTED: u32 = 0x6B6B6B;
const RED: u32 = 0xC62828;
const DOUBLE_MS: u64 = 450;

/// Dibuja con el recorte limitado a `r`.
fn text_in(s: &mut Surface, r: Rect, text: &str, color: u32, align: Align) {
    let old = s.clip();
    s.set_clip(old.intersect(r));
    font::draw_in(s, r, text, FS, false, color, align);
    s.set_clip(old);
}

/// Parte `text` en líneas de ancho máximo `maxw` (por palabras; respeta '\n').
pub fn wrap(text: &str, size: u32, maxw: i32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ').filter(|w| !w.is_empty()) {
            let cand = if line.is_empty() { String::from(word) } else { format!("{line} {word}") };
            if font::width(&cand, size, false) <= maxw {
                line = cand;
                continue;
            }
            if !line.is_empty() {
                out.push(core::mem::take(&mut line));
            }
            let mut w = String::new();
            for c in word.chars() {
                if !w.is_empty() && font::width(&format!("{w}{c}"), size, false) > maxw {
                    out.push(core::mem::take(&mut w));
                }
                w.push(c);
            }
            line = w;
        }
        out.push(line);
    }
    out
}

/// Recorta `text` con "…" para que quepa en `maxw`.
pub fn fit(text: &str, size: u32, maxw: i32) -> String {
    if font::width(text, size, false) <= maxw {
        return String::from(text);
    }
    let mut t = String::from(text);
    while !t.is_empty() && font::width(&format!("{t}…"), size, false) > maxw {
        t.pop();
    }
    t.push('…');
    t
}

/// Campo de texto de una línea; `cur` cuenta caracteres.
struct Field {
    text: String,
    cur: usize,
}

impl Field {
    fn new(t: &str) -> Field {
        Field { text: String::from(t), cur: t.chars().count() }
    }
    fn len(&self) -> usize {
        self.text.chars().count()
    }
    fn byte(&self, n: usize) -> usize {
        self.text.char_indices().nth(n).map_or(self.text.len(), |(i, _)| i)
    }
    /// ¿La tecla ha sido para el campo?
    fn key(&mut self, ev: &KeyEvent) -> bool {
        match ev.key {
            Key::Left if self.cur > 0 => self.cur -= 1,
            Key::Right if self.cur < self.len() => self.cur += 1,
            Key::Home => self.cur = 0,
            Key::End => self.cur = self.len(),
            Key::Backspace if self.cur > 0 => {
                let b = self.byte(self.cur - 1);
                self.text.remove(b);
                self.cur -= 1;
            }
            Key::Delete if self.cur < self.len() => {
                let b = self.byte(self.cur);
                self.text.remove(b);
            }
            _ => match ev.ch {
                Some(c) if !ev.ctrl && !ev.alt && !c.is_control() && self.len() < 255 => {
                    let b = self.byte(self.cur);
                    self.text.insert(b, c);
                    self.cur += 1;
                }
                _ => return false,
            },
        }
        true
    }
    /// Desplazamiento horizontal para que el cursor quede a la vista en `inner_w` píxeles.
    fn shift(&self, inner_w: i32) -> i32 {
        let head: String = self.text.chars().take(self.cur).collect();
        (font::width(&head, FS, false) - (inner_w - 2)).max(0)
    }
    fn click(&mut self, x: i32, inner_w: i32) {
        let x = x + self.shift(inner_w);
        let (mut acc, mut n) = (0, 0);
        for c in self.text.chars() {
            let a = font::adv(c, FS, false);
            if acc + a / 2 > x {
                break;
            }
            acc += a;
            n += 1;
        }
        self.cur = n;
    }
    fn draw(&self, s: &mut Surface, r: Rect, focused: bool, theme: &Theme) {
        s.fill_rect(r, 0xFFFFFF);
        s.frame(r, if focused { theme.accent } else { 0x8A8A8A });
        let inner = Rect::new(r.x + 6, r.y, r.w - 12, r.h);
        let shift = self.shift(inner.w);
        let old = s.clip();
        s.set_clip(old.intersect(inner));
        font::draw(s, inner.x - shift, r.y + (r.h - font::line_h(FS)) / 2, &self.text, FS, false, INK);
        if focused {
            let head: String = self.text.chars().take(self.cur).collect();
            let x = inner.x + font::width(&head, FS, false) - shift;
            s.fill_rect(Rect::new(x, r.y + 5, 1, r.h - 10), INK);
        }
        s.set_clip(old);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Message,
    Input,
    Open,
    Save,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tgt {
    Btn(usize),
    Up,
}

/// Cuadro de diálogo ya construido: se dibuja en una superficie de `size()`, atiende teclado y
/// ratón (coordenadas del contenido) y devuelve el resultado al terminar.
pub struct Dialog {
    tag: u32,
    mode: Mode,
    title: String,
    w: i32,
    h: i32,
    /// Texto del mensaje (o etiqueta del campo).
    lines: Vec<String>,
    btns: Vec<(&'static str, Answer)>,
    /// Elemento con el foco: primero los que no son botones (Input: campo; Abrir/Guardar:
    /// lista y nombre), luego los botones.
    focus: usize,
    field: Field,
    dir: String,
    entries: Vec<Entry>,
    sel: Option<usize>,
    scroll: usize,
    error: Option<String>,
    /// Ruta existente pendiente de "¿Reemplazar?" (solo Guardar).
    confirm: Option<String>,
    press: Option<Tgt>,
    last_click: (usize, u64),
    dirty: bool,
}

impl Dialog {
    pub fn new(req: DialogRequest, sys: &mut dyn Platform) -> Dialog {
        let mut d = Dialog {
            tag: 0,
            mode: Mode::Message,
            title: String::new(),
            w: 400,
            h: 0,
            lines: Vec::new(),
            btns: Vec::new(),
            focus: 0,
            field: Field::new(""),
            dir: String::new(),
            entries: Vec::new(),
            sel: None,
            scroll: 0,
            error: None,
            confirm: None,
            press: None,
            last_click: (usize::MAX, 0),
            dirty: true,
        };
        let lh = font::line_h(FS);
        match req {
            DialogRequest::Message { tag, title, text, buttons } => {
                d.tag = tag;
                d.title = title;
                d.btns = match buttons {
                    Buttons::Ok => vec![("Aceptar", Answer::Ok)],
                    Buttons::OkCancel => vec![("Aceptar", Answer::Ok), ("Cancelar", Answer::Cancel)],
                    Buttons::YesNo => vec![("Sí", Answer::Yes), ("No", Answer::No)],
                    Buttons::YesNoCancel => vec![("Sí", Answer::Yes), ("No", Answer::No), ("Cancelar", Answer::Cancel)],
                };
                d.lines = wrap(&text, FS, d.w - 2 * PAD);
                d.lines.truncate(14);
                d.h = PAD + d.lines.len() as i32 * lh + 24 + BTN_H + 16;
            }
            DialogRequest::Input { tag, title, label, value } => {
                d.tag = tag;
                d.mode = Mode::Input;
                d.title = title;
                d.lines = vec![fit(&label, FS, d.w - 2 * PAD)];
                d.field = Field::new(&value);
                d.h = PAD + lh + 8 + 28 + 24 + BTN_H + 16;
            }
            DialogRequest::Open { tag, title, dir } => {
                d.file(tag, title, Mode::Open, &dir, "", sys);
            }
            DialogRequest::Save { tag, title, dir, name } => {
                d.file(tag, title, Mode::Save, &dir, &name, sys);
            }
        }
        d
    }

    fn file(&mut self, tag: u32, title: String, mode: Mode, dir: &str, name: &str, sys: &mut dyn Platform) {
        self.tag = tag;
        self.title = title;
        self.mode = mode;
        self.w = 520;
        self.h = 380;
        self.field = Field::new(name);
        self.focus = if mode == Mode::Save { 1 } else { 0 };
        self.dir = String::from("C:\\");
        if !self.go(dir, sys) {
            self.go("C:\\", sys);
        }
    }

    /// Tamaño del contenido.
    pub fn size(&self) -> (i32, i32) {
        (self.w, self.h)
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// ¿Hay algo que redibujar desde la última vez? Se borra al preguntar.
    pub fn take_dirty(&mut self) -> bool {
        core::mem::take(&mut self.dirty)
    }

    // ---------------------------------------------------------------- geometría

    fn is_file(&self) -> bool {
        matches!(self.mode, Mode::Open | Mode::Save)
    }
    fn labels(&self) -> Vec<&'static str> {
        match self.mode {
            Mode::Message => self.btns.iter().map(|b| b.0).collect(),
            _ if self.confirm.is_some() => vec!["Sí", "No"],
            Mode::Save => vec!["Guardar", "Cancelar"],
            Mode::Open => vec!["Abrir", "Cancelar"],
            Mode::Input => vec!["Aceptar", "Cancelar"],
        }
    }
    /// Elementos con foco que no son botones.
    fn nbefore(&self) -> usize {
        match self.mode {
            Mode::Message => 0,
            Mode::Input => 1,
            _ if self.confirm.is_some() => 0,
            _ => 2,
        }
    }
    fn btn_rect(&self, i: usize, n: usize) -> Rect {
        let k = (n - i) as i32;
        Rect::new(self.w - 16 - k * BTN_W - (k - 1) * 8, self.h - 16 - BTN_H, BTN_W, BTN_H)
    }
    fn up_rect(&self) -> Rect {
        Rect::new(12, 12, 32, 28)
    }
    fn name_y(&self) -> i32 {
        self.h - 16 - BTN_H - 36
    }
    fn list_rect(&self) -> Rect {
        Rect::new(12, 52, self.w - 24, self.name_y() - 28 - 52)
    }
    fn field_rect(&self) -> Rect {
        match self.mode {
            Mode::Input => Rect::new(PAD, PAD + font::line_h(FS) + 8, self.w - 2 * PAD, 28),
            _ => Rect::new(76, self.name_y(), self.w - 88, 28),
        }
    }
    fn rows(&self) -> usize {
        (self.list_rect().h / ROW_H).max(1) as usize
    }

    // ---------------------------------------------------------------- dibujo

    pub fn draw(&mut self, s: &mut Surface, theme: &Theme) {
        let b = s.bounds();
        s.fill_rect(b, 0xFFFFFF);
        let lh = font::line_h(FS);
        match self.mode {
            Mode::Message => {
                for (i, l) in self.lines.iter().enumerate() {
                    text_in(s, Rect::new(PAD, PAD + i as i32 * lh, self.w - 2 * PAD, lh), l, INK, Align::Left);
                }
            }
            Mode::Input => {
                text_in(s, Rect::new(PAD, PAD, self.w - 2 * PAD, lh), &self.lines[0], INK, Align::Left);
                self.field.draw(s, self.field_rect(), self.focus == 0, theme);
            }
            _ => self.draw_files(s, theme),
        }
        let labels = self.labels();
        let n = labels.len();
        for (i, label) in labels.iter().enumerate() {
            let r = self.btn_rect(i, n);
            let focused = self.focus == self.nbefore() + i;
            if i == 0 {
                s.fill_rect(r, theme.accent);
            } else {
                s.fill_rect(r, 0xE6E6E6);
                s.frame(r, 0xA0A0A0);
            }
            if focused {
                s.frame(r.inflate(2), theme.accent);
                s.frame(r.inflate(1), 0xFFFFFF);
            }
            text_in(s, r, label, if i == 0 { 0xFFFFFF } else { INK }, Align::Center);
        }
    }

    fn draw_files(&mut self, s: &mut Surface, theme: &Theme) {
        let lh = font::line_h(FS);
        if let Some(p) = &self.confirm {
            let text = format!("«{}» ya existe. ¿Quieres reemplazarlo?", files::file_name(p));
            for (i, l) in wrap(&text, FS, self.w - 2 * PAD).iter().enumerate() {
                text_in(s, Rect::new(PAD, PAD + 40 + i as i32 * lh, self.w - 2 * PAD, lh), l, INK, Align::Left);
            }
            return;
        }
        let up = self.up_rect();
        s.fill_rect(up, 0xE6E6E6);
        s.frame(up, 0xA0A0A0);
        let (cx, cy) = ((up.x + up.w / 2) as f32, (up.y + up.h / 2) as f32);
        s.stroke(&[(cx, cy + 6.0), (cx, cy - 6.0)], 1.5, INK);
        s.stroke(&[(cx - 5.0, cy - 1.0), (cx, cy - 6.0), (cx + 5.0, cy - 1.0)], 1.5, INK);
        let path = Rect::new(52, 12, self.w - 64, 28);
        s.frame(path, 0xC8C8C8);
        text_in(s, Rect::new(path.x + 8, path.y, path.w - 16, path.h), &self.dir, INK, Align::Left);
        let list = self.list_rect();
        s.frame(list, if self.focus == 0 { theme.accent } else { 0x8A8A8A });
        if self.entries.is_empty() {
            text_in(s, Rect::new(list.x + 8, list.y, list.w - 16, ROW_H), "(carpeta vacía)", MUTED, Align::Left);
        }
        let old = s.clip();
        s.set_clip(old.intersect(list.inflate(-1)));
        for k in 0..self.rows() {
            let i = self.scroll + k;
            let Some(e) = self.entries.get(i) else { break };
            let r = Rect::new(list.x + 1, list.y + 1 + k as i32 * ROW_H, list.w - 2, ROW_H);
            let on = self.sel == Some(i);
            if on {
                s.fill_rect(r, if self.focus == 0 { theme.accent } else { 0xD9D9D9 });
            }
            let icon = if e.dir {
                Icon::Folder
            } else if e.name.to_lowercase().ends_with(".txt") {
                Icon::FileText
            } else {
                Icon::FileBlank
            };
            icons::draw(s, icon, r.x + 6, r.y + 4, 16, if on && self.focus == 0 { 0xFFFFFF } else { 0x444444 });
            let ink = if on && self.focus == 0 { 0xFFFFFF } else { INK };
            text_in(s, Rect::new(r.x + 30, r.y, r.w - 36, r.h), &e.name, ink, Align::Left);
        }
        s.set_clip(old);
        if let Some(e) = &self.error {
            text_in(s, Rect::new(12, self.name_y() - 24, self.w - 24, 20), e, RED, Align::Left);
        }
        text_in(s, Rect::new(12, self.name_y(), 60, 28), "Nombre:", INK, Align::Left);
        self.field.draw(s, self.field_rect(), self.focus == 1, theme);
    }

    // ---------------------------------------------------------------- lógica

    /// Resultado de cerrar el diálogo con su aspa.
    pub fn cancel(&self) -> DialogResult {
        let tag = self.tag;
        match self.mode {
            Mode::Message => {
                let has = |a| self.btns.iter().any(|b| b.1 == a);
                let answer = if has(Answer::Cancel) {
                    Answer::Cancel
                } else if has(Answer::No) {
                    Answer::No
                } else {
                    Answer::Ok
                };
                DialogResult::Answer { tag, answer }
            }
            Mode::Input => DialogResult::Text { tag, value: None },
            _ => DialogResult::Path { tag, path: None },
        }
    }

    fn esc(&mut self) -> Option<DialogResult> {
        if self.confirm.take().is_some() {
            self.focus = 1;
            return None;
        }
        Some(self.cancel())
    }

    /// Lista `dir` y pasa a ella; `false` (con el error anotado) si no se puede.
    fn go(&mut self, dir: &str, sys: &mut dyn Platform) -> bool {
        match sys.files().list(dir) {
            Ok(v) => {
                self.dir = files::resolve("C:\\", dir).unwrap_or_else(|_| String::from("C:\\"));
                self.entries = v;
                self.sel = None;
                self.scroll = 0;
                self.error = None;
                true
            }
            Err(e) => {
                self.error = Some(String::from(e.message()));
                false
            }
        }
    }

    fn select(&mut self, i: usize) {
        self.sel = Some(i);
        let rows = self.rows();
        if i < self.scroll {
            self.scroll = i;
        } else if i >= self.scroll + rows {
            self.scroll = i + 1 - rows;
        }
        if !self.entries[i].dir {
            self.field = Field::new(&self.entries[i].name);
        }
    }

    fn activate(&mut self, i: usize, sys: &mut dyn Platform) -> Option<DialogResult> {
        let e = self.entries.get(i)?.clone();
        if e.dir {
            let p = files::join(&self.dir, &e.name);
            self.go(&p, sys);
            return None;
        }
        self.field = Field::new(&e.name);
        self.accept(sys)
    }

    /// Acepta el nombre del campo en Abrir/Guardar.
    fn accept(&mut self, sys: &mut dyn Platform) -> Option<DialogResult> {
        let tag = self.tag;
        let name = String::from(self.field.text.trim());
        if self.mode == Mode::Open {
            if name.is_empty() {
                self.error = Some(String::from("Elige un archivo o escribe su nombre."));
                return None;
            }
            let path = match files::resolve(&self.dir, &name) {
                Ok(p) => p,
                Err(e) => {
                    self.error = Some(String::from(e.message()));
                    return None;
                }
            };
            return match sys.files().stat(&path) {
                Ok(e) if e.dir => {
                    if self.go(&path, sys) {
                        self.field = Field::new("");
                    }
                    None
                }
                Ok(_) => Some(DialogResult::Path { tag, path: Some(path) }),
                Err(e) => {
                    self.error = Some(String::from(e.message()));
                    None
                }
            };
        }
        let base = name.trim_end_matches('.');
        let full = if base.rfind('.').is_some_and(|i| i > 0) { String::from(base) } else { format!("{base}.txt") };
        if base.is_empty() || !files::valid_name(&full) {
            self.error = Some(String::from("El nombre no es válido: no puede estar vacío ni llevar \\ / : * ? \" < > |."));
            return None;
        }
        let path = files::join(&self.dir, &full);
        match sys.files().stat(&path) {
            Ok(e) if e.dir => self.error = Some(String::from("Ya hay una carpeta con ese nombre.")),
            Ok(_) => {
                self.confirm = Some(path);
                self.focus = 0;
            }
            Err(FsError::NotFound) => return Some(DialogResult::Path { tag, path: Some(path) }),
            Err(e) => self.error = Some(String::from(e.message())),
        }
        None
    }

    /// Pulsa el botón `i` (según `labels`).
    fn press(&mut self, i: usize, sys: &mut dyn Platform) -> Option<DialogResult> {
        let tag = self.tag;
        match self.mode {
            Mode::Message => Some(DialogResult::Answer { tag, answer: self.btns[i].1 }),
            Mode::Input => Some(DialogResult::Text { tag, value: if i == 0 { Some(self.field.text.clone()) } else { None } }),
            _ if self.confirm.is_some() => {
                if i == 0 {
                    return Some(DialogResult::Path { tag, path: self.confirm.take() });
                }
                self.confirm = None;
                self.focus = 1;
                None
            }
            _ if i == 0 => self.accept(sys),
            _ => Some(DialogResult::Path { tag, path: None }),
        }
    }

    pub fn key(&mut self, ev: &KeyEvent, sys: &mut dyn Platform) -> Option<DialogResult> {
        if !ev.down {
            return None;
        }
        self.dirty = true;
        let (nb, n) = (self.nbefore(), self.labels().len());
        match ev.key {
            Key::Escape => return self.esc(),
            Key::Tab => {
                let t = nb + n;
                self.focus = if ev.shift { (self.focus + t - 1) % t } else { (self.focus + 1) % t };
                return None;
            }
            Key::Enter => {
                if self.focus >= nb {
                    return self.press(self.focus - nb, sys);
                }
                return match self.mode {
                    Mode::Input => self.press(0, sys),
                    _ if self.focus == 0 => match self.sel {
                        Some(i) => self.activate(i, sys),
                        None => self.accept(sys),
                    },
                    _ => self.accept(sys),
                };
            }
            _ => {}
        }
        if self.focus >= nb {
            match ev.key {
                Key::Left | Key::Up if self.focus > nb => self.focus -= 1,
                Key::Right | Key::Down if self.focus + 1 < nb + n => self.focus += 1,
                _ => {}
            }
        } else if self.is_file() && self.focus == 0 {
            let last = self.entries.len().saturating_sub(1);
            let cur = self.sel.unwrap_or(0);
            let rows = self.rows();
            let to = match ev.key {
                Key::Up => Some(if self.sel.is_some() { cur.saturating_sub(1) } else { 0 }),
                Key::Down => Some(if self.sel.is_some() { (cur + 1).min(last) } else { 0 }),
                Key::Home => Some(0),
                Key::End => Some(last),
                Key::PageUp => Some(cur.saturating_sub(rows)),
                Key::PageDown => Some((cur + rows).min(last)),
                _ => None,
            };
            match (to, ev.key) {
                (Some(i), _) if !self.entries.is_empty() => self.select(i),
                (_, Key::Backspace) => {
                    if let Some(p) = files::parent(&self.dir) {
                        self.go(&p, sys);
                    }
                }
                _ => {}
            }
        } else if self.field.key(ev) {
            self.error = None;
        }
        None
    }

    /// `ev` en coordenadas del contenido del diálogo.
    pub fn mouse(&mut self, ev: MouseEv, sys: &mut dyn Platform) -> Option<DialogResult> {
        match ev {
            MouseEv::Down { x, y } => {
                self.press = None;
                self.dirty = true;
                let n = self.labels().len();
                for i in 0..n {
                    if self.btn_rect(i, n).contains(x, y) {
                        self.press = Some(Tgt::Btn(i));
                        self.focus = self.nbefore() + i;
                        return None;
                    }
                }
                if self.confirm.is_some() {
                    return None;
                }
                let f = self.field_rect();
                if self.mode == Mode::Input || self.is_file() {
                    if f.contains(x, y) {
                        self.focus = if self.mode == Mode::Input { 0 } else { 1 };
                        self.field.click(x - f.x - 6, f.w - 12);
                        return None;
                    }
                }
                if !self.is_file() {
                    return None;
                }
                let list = self.list_rect();
                if self.up_rect().contains(x, y) {
                    self.press = Some(Tgt::Up);
                } else if list.contains(x, y) {
                    self.focus = 0;
                    let i = self.scroll + ((y - list.y - 1).max(0) / ROW_H) as usize;
                    if i < self.entries.len() {
                        let now = sys.uptime_ms();
                        let dbl = self.last_click.0 == i && now.saturating_sub(self.last_click.1) < DOUBLE_MS;
                        self.select(i);
                        self.last_click = if dbl { (usize::MAX, 0) } else { (i, now) };
                        if dbl {
                            return self.activate(i, sys);
                        }
                    }
                }
            }
            MouseEv::Up { x, y } => {
                self.dirty = true;
                match self.press.take() {
                    Some(Tgt::Btn(i)) if self.btn_rect(i, self.labels().len()).contains(x, y) => return self.press(i, sys),
                    Some(Tgt::Up) if self.up_rect().contains(x, y) => {
                        if let Some(p) = files::parent(&self.dir) {
                            self.go(&p, sys);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::TestPlatform;

    fn kev(key: Key, ch: Option<char>) -> KeyEvent {
        KeyEvent { key, ch, down: true, shift: false, ctrl: false, alt: false }
    }
    fn press(d: &mut Dialog, sys: &mut TestPlatform, key: Key) -> Option<DialogResult> {
        d.key(&kev(key, None), sys)
    }
    fn type_str(d: &mut Dialog, sys: &mut TestPlatform, t: &str) {
        for c in t.chars() {
            assert!(d.key(&kev(Key::Char(c), Some(c)), sys).is_none());
        }
    }
    fn msg(buttons: Buttons) -> (Dialog, TestPlatform) {
        let mut sys = TestPlatform::default();
        let d = Dialog::new(
            DialogRequest::Message { tag: 7, title: "T".into(), text: "Hola mundo, esto es un mensaje largo con varias palabras que ocupan más de una línea del cuadro.".into(), buttons },
            &mut sys,
        );
        (d, sys)
    }
    fn ans(a: Answer) -> Option<DialogResult> {
        Some(DialogResult::Answer { tag: 7, answer: a })
    }

    #[test]
    fn wrap_breaks_by_words_and_long_words() {
        let l = wrap("uno dos tres cuatro", 9, font::width("uno dos", 9, false));
        assert_eq!(l, ["uno dos", "tres", "cuatro"]);
        let l = wrap("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 9, 60);
        assert!(l.len() > 1 && l.iter().all(|x| font::width(x, 9, false) <= 60));
        assert_eq!(wrap("a\nb", 9, 100), ["a", "b"]);
        assert_eq!(fit("corto", 9, 500), "corto");
        let f = fit("un nombre larguísimo de archivo", 9, 80);
        assert!(f.ends_with('…') && font::width(&f, 9, false) <= 80);
    }

    #[test]
    fn message_buttons_enter_esc_and_tab() {
        for (b, enter, esc) in [
            (Buttons::Ok, Answer::Ok, Answer::Ok),
            (Buttons::OkCancel, Answer::Ok, Answer::Cancel),
            (Buttons::YesNo, Answer::Yes, Answer::No),
            (Buttons::YesNoCancel, Answer::Yes, Answer::Cancel),
        ] {
            let (mut d, mut sys) = msg(b);
            assert_eq!(press(&mut d, &mut sys, Key::Enter), ans(enter));
            let (mut d, mut sys) = msg(b);
            assert_eq!(press(&mut d, &mut sys, Key::Escape), ans(esc));
        }
        let (mut d, mut sys) = msg(Buttons::YesNoCancel);
        assert!(press(&mut d, &mut sys, Key::Tab).is_none());
        assert_eq!(press(&mut d, &mut sys, Key::Enter), ans(Answer::No));
        let (mut d, mut sys) = msg(Buttons::YesNoCancel);
        press(&mut d, &mut sys, Key::Right);
        press(&mut d, &mut sys, Key::Right);
        press(&mut d, &mut sys, Key::Right);
        assert_eq!(press(&mut d, &mut sys, Key::Enter), ans(Answer::Cancel));
        press(&mut d, &mut sys, Key::Left);
    }

    #[test]
    fn message_mouse_click_on_buttons() {
        let (mut d, mut sys) = msg(Buttons::YesNo);
        let r = d.btn_rect(1, 2);
        let (x, y) = (r.x + 5, r.y + 5);
        assert!(d.mouse(MouseEv::Down { x, y }, &mut sys).is_none());
        assert_eq!(d.mouse(MouseEv::Up { x, y }, &mut sys), ans(Answer::No));
        // Soltar fuera no pulsa.
        let (mut d, mut sys) = msg(Buttons::YesNo);
        d.mouse(MouseEv::Down { x, y }, &mut sys);
        assert!(d.mouse(MouseEv::Up { x: 2, y: 2 }, &mut sys).is_none());
        // Dibujar no falla y rellena.
        let (w, h) = d.size();
        let mut s = Surface::new(w, h, 0);
        d.draw(&mut s, &Theme::default());
        assert_eq!(s.px[0], 0xFFFFFF);
    }

    #[test]
    fn input_edits_and_returns_text() {
        let mut sys = TestPlatform::default();
        let req = DialogRequest::Input { tag: 3, title: "N".into(), label: "Nombre:".into(), value: "ab".into() };
        let mut d = Dialog::new(req.clone(), &mut sys);
        type_str(&mut d, &mut sys, "cñ");
        press(&mut d, &mut sys, Key::Left);
        press(&mut d, &mut sys, Key::Backspace); // borra la c
        assert_eq!(d.field.text, "abñ");
        press(&mut d, &mut sys, Key::Home);
        type_str(&mut d, &mut sys, "X");
        press(&mut d, &mut sys, Key::Delete); // borra la a
        assert_eq!(d.field.text, "Xbñ");
        press(&mut d, &mut sys, Key::End);
        type_str(&mut d, &mut sys, "!");
        assert_eq!(press(&mut d, &mut sys, Key::Enter), Some(DialogResult::Text { tag: 3, value: Some("Xbñ!".into()) }));
        let mut d = Dialog::new(req.clone(), &mut sys);
        assert_eq!(press(&mut d, &mut sys, Key::Escape), Some(DialogResult::Text { tag: 3, value: None }));
        // Tab hasta Cancelar y Enter.
        let mut d = Dialog::new(req, &mut sys);
        press(&mut d, &mut sys, Key::Tab);
        press(&mut d, &mut sys, Key::Tab);
        assert_eq!(press(&mut d, &mut sys, Key::Enter), Some(DialogResult::Text { tag: 3, value: None }));
        let mut s = Surface::new(d.size().0, d.size().1, 0);
        d.draw(&mut s, &Theme::default());
    }

    fn save(name: &str, sys: &mut TestPlatform) -> Dialog {
        Dialog::new(DialogRequest::Save { tag: 5, title: "G".into(), dir: "C:\\Documentos".into(), name: name.into() }, sys)
    }
    fn path(p: Option<&str>) -> Option<DialogResult> {
        Some(DialogResult::Path { tag: 5, path: p.map(String::from) })
    }

    #[test]
    fn save_adds_extension_rejects_bad_names_and_confirms_replace() {
        let mut sys = TestPlatform::default();
        let mut d = save("nota", &mut sys);
        assert_eq!(press(&mut d, &mut sys, Key::Enter), path(Some("C:\\Documentos\\nota.txt")));
        let mut d = save("datos.csv", &mut sys);
        assert_eq!(press(&mut d, &mut sys, Key::Enter), path(Some("C:\\Documentos\\datos.csv")));
        // Nombre inválido: error y sigue abierto.
        let mut d = save("a*b", &mut sys);
        assert!(press(&mut d, &mut sys, Key::Enter).is_none());
        assert!(d.error.is_some());
        let mut d = save("", &mut sys);
        assert!(press(&mut d, &mut sys, Key::Enter).is_none() && d.error.is_some());
        // Dibujo con error.
        let mut s = Surface::new(d.size().0, d.size().1, 0);
        d.draw(&mut s, &Theme::default());
        // Editar quita el error.
        type_str(&mut d, &mut sys, "ok");
        assert!(d.error.is_none());
        // Reemplazo.
        let now = sys.now();
        sys.files.write("C:\\Documentos\\viejo.txt", b"x", now).unwrap();
        let mut d = save("viejo", &mut sys);
        assert!(press(&mut d, &mut sys, Key::Enter).is_none());
        assert!(d.confirm.is_some());
        let mut s = Surface::new(d.size().0, d.size().1, 0);
        d.draw(&mut s, &Theme::default());
        // Esc vuelve al diálogo sin cerrar.
        assert!(press(&mut d, &mut sys, Key::Escape).is_none() && d.confirm.is_none());
        press(&mut d, &mut sys, Key::Enter);
        assert!(d.confirm.is_some());
        press(&mut d, &mut sys, Key::Right); // No
        assert!(press(&mut d, &mut sys, Key::Enter).is_none() && d.confirm.is_none());
        press(&mut d, &mut sys, Key::Enter);
        assert_eq!(press(&mut d, &mut sys, Key::Enter), path(Some("C:\\Documentos\\viejo.txt")));
        // Cancelar.
        let mut d = save("x", &mut sys);
        assert_eq!(press(&mut d, &mut sys, Key::Escape), path(None));
    }

    #[test]
    fn open_navigates_and_picks_files() {
        let mut sys = TestPlatform::default();
        let now = sys.now();
        sys.files.mkdir("C:\\Documentos\\sub", now).unwrap();
        sys.files.write("C:\\Documentos\\sub\\a.txt", b"a", now).unwrap();
        sys.files.write("C:\\Documentos\\b.txt", b"b", now).unwrap();
        let open = |sys: &mut TestPlatform| Dialog::new(DialogRequest::Open { tag: 5, title: "A".into(), dir: "C:\\Documentos".into() }, sys);
        let mut d = open(&mut sys);
        assert_eq!(d.entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["sub", "b.txt"]);
        // Enter en la carpeta entra; Retroceso sube.
        press(&mut d, &mut sys, Key::Down);
        assert!(press(&mut d, &mut sys, Key::Enter).is_none());
        assert_eq!(d.dir, "C:\\Documentos\\sub");
        press(&mut d, &mut sys, Key::Backspace);
        assert_eq!(d.dir, "C:\\Documentos");
        // Enter en un archivo lo elige.
        press(&mut d, &mut sys, Key::Down);
        press(&mut d, &mut sys, Key::Down);
        assert_eq!(press(&mut d, &mut sys, Key::Enter), path(Some("C:\\Documentos\\b.txt")));
        // Doble clic en carpeta, botón ↑ y Abrir con el nombre escrito.
        let mut d = open(&mut sys);
        let l = d.list_rect();
        let p = (l.x + 40, l.y + 5);
        d.mouse(MouseEv::Down { x: p.0, y: p.1 }, &mut sys);
        d.mouse(MouseEv::Up { x: p.0, y: p.1 }, &mut sys);
        d.mouse(MouseEv::Down { x: p.0, y: p.1 }, &mut sys);
        assert_eq!(d.dir, "C:\\Documentos\\sub");
        let u = d.up_rect();
        d.mouse(MouseEv::Down { x: u.x + 3, y: u.y + 3 }, &mut sys);
        d.mouse(MouseEv::Up { x: u.x + 3, y: u.y + 3 }, &mut sys);
        assert_eq!(d.dir, "C:\\Documentos");
        // Nombre que no existe: error.
        press(&mut d, &mut sys, Key::Tab);
        type_str(&mut d, &mut sys, "zzz");
        assert!(press(&mut d, &mut sys, Key::Enter).is_none() && d.error.is_some());
        let mut s = Surface::new(d.size().0, d.size().1, 0);
        d.draw(&mut s, &Theme::default());
        assert_eq!(press(&mut d, &mut sys, Key::Escape), path(None));
    }
}
