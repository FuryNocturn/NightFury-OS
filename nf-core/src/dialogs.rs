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
use crate::scrollbar::{Metrics, ScrollBar, SB_W};
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
    /// Pide una clave: igual que `Input` pero el campo muestra puntos, no se puede copiar y el
    /// búfer se borra al cerrar. Responde `Text` (`None` si se cancela).
    Password { tag: u32, title: String, label: String },
    /// Elegir un archivo existente. `dir` es la carpeta inicial (normalmente `Documentos` del usuario).
    Open { tag: u32, title: String, dir: String },
    /// Elegir dónde guardar. `name` es el nombre propuesto. Si el archivo ya existe, el propio
    /// diálogo pregunta si se reemplaza antes de responder.
    Save { tag: u32, title: String, dir: String, name: String },
    /// Elegir una carpeta. `dir` es la carpeta inicial. Solo lista carpetas (sin la Papelera ni
    /// `C:\Sistema`, salvo a un administrador); "Nueva carpeta" crea una en la carpeta actual.
    Folder { tag: u32, title: String, dir: String },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DialogResult {
    /// Respuesta a `Message`.
    Answer { tag: u32, answer: Answer },
    /// Respuesta a `Input` y `Password`; `None` si se canceló.
    Text { tag: u32, value: Option<String> },
    /// Respuesta a `Open`, `Save` y `Folder`: ruta absoluta; `None` si se canceló.
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
/// Anchos de los botones "Seleccionar carpeta" y "Nueva carpeta".
const PICK_W: i32 = 150;
const NEW_W: i32 = 120;

const SYSTEM: &str = "Sistema";

/// ¿Es `C:\Sistema` o algo dentro?
fn is_system(path: &str) -> bool {
    path.get(3..).and_then(|r| r.split('\\').next()).is_some_and(|f| f.eq_ignore_ascii_case(SYSTEM))
}

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
    /// Muestra puntos en vez del texto (claves).
    mask: bool,
}

impl Field {
    fn new(t: &str) -> Field {
        Field { text: String::from(t), cur: t.chars().count(), mask: false }
    }
    /// Texto tal como se ve: el real o un punto por carácter.
    fn shown(&self) -> String {
        if self.mask {
            self.text.chars().map(|_| '•').collect()
        } else {
            self.text.clone()
        }
    }
    /// Entrega el contenido y deja el campo vacío.
    fn take(&mut self) -> String {
        self.cur = 0;
        core::mem::take(&mut self.text)
    }
    /// Borra el búfer (claves).
    fn wipe(&mut self) {
        // SAFETY: se escriben ceros, que son UTF-8 válido.
        crate::crypto::wipe(unsafe { self.text.as_bytes_mut() });
        self.text.clear();
        self.cur = 0;
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
        let head: String = self.shown().chars().take(self.cur).collect();
        (font::width(&head, FS, false) - (inner_w - 2)).max(0)
    }
    fn click(&mut self, x: i32, inner_w: i32) {
        let x = x + self.shift(inner_w);
        let (mut acc, mut n) = (0, 0);
        for c in self.shown().chars() {
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
        font::draw(s, inner.x - shift, r.y + (r.h - font::line_h(FS)) / 2, &self.shown(), FS, false, INK);
        if focused {
            let head: String = self.shown().chars().take(self.cur).collect();
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
    Password,
    Open,
    Save,
    Folder,
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
    sb: ScrollBar,
    last_click: (usize, u64),
    dirty: bool,
}

impl Drop for Dialog {
    /// Las claves no se quedan en la memoria al cerrar el diálogo.
    fn drop(&mut self) {
        self.field.wipe();
    }
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
            sb: ScrollBar::new(false),
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
            DialogRequest::Password { tag, title, label } => {
                d.tag = tag;
                d.mode = Mode::Password;
                d.title = title;
                d.lines = vec![fit(&label, FS, d.w - 2 * PAD)];
                d.field.mask = true;
                d.h = PAD + lh + 8 + 28 + 24 + BTN_H + 16;
            }
            DialogRequest::Open { tag, title, dir } => {
                d.file(tag, title, Mode::Open, &dir, "", sys);
            }
            DialogRequest::Save { tag, title, dir, name } => {
                d.file(tag, title, Mode::Save, &dir, &name, sys);
            }
            DialogRequest::Folder { tag, title, dir } => {
                d.file(tag, title, Mode::Folder, &dir, "", sys);
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
        matches!(self.mode, Mode::Open | Mode::Save | Mode::Folder)
    }
    fn labels(&self) -> Vec<&'static str> {
        match self.mode {
            Mode::Message => self.btns.iter().map(|b| b.0).collect(),
            _ if self.confirm.is_some() => vec!["Sí", "No"],
            Mode::Save => vec!["Guardar", "Cancelar"],
            Mode::Open => vec!["Abrir", "Cancelar"],
            Mode::Folder => vec!["Seleccionar carpeta", "Cancelar", "Nueva carpeta"],
            Mode::Input | Mode::Password => vec!["Aceptar", "Cancelar"],
        }
    }
    /// Elementos con foco que no son botones.
    fn nbefore(&self) -> usize {
        match self.mode {
            Mode::Message => 0,
            Mode::Input | Mode::Password | Mode::Folder => 1,
            _ if self.confirm.is_some() => 0,
            _ => 2,
        }
    }
    fn btn_rect(&self, i: usize, n: usize) -> Rect {
        if self.mode == Mode::Folder {
            let y = self.h - 16 - BTN_H;
            return match i {
                0 => Rect::new(self.w - 16 - BTN_W - 8 - PICK_W, y, PICK_W, BTN_H),
                1 => Rect::new(self.w - 16 - BTN_W, y, BTN_W, BTN_H),
                _ => Rect::new(12, y, NEW_W, BTN_H),
            };
        }
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
        if self.mode == Mode::Folder {
            return Rect::new(12, 52, self.w - 24, self.h - 16 - BTN_H - 32 - 52);
        }
        Rect::new(12, 52, self.w - 24, self.name_y() - 28 - 52)
    }
    fn field_rect(&self) -> Rect {
        match self.mode {
            Mode::Input | Mode::Password => Rect::new(PAD, PAD + font::line_h(FS) + 8, self.w - 2 * PAD, 28),
            _ => Rect::new(76, self.name_y(), self.w - 88, 28),
        }
    }
    fn rows(&self) -> usize {
        (self.list_rect().h / ROW_H).max(1) as usize
    }
    /// La lista no cabe y lleva barra de desplazamiento.
    fn need_sb(&self) -> bool {
        self.entries.len() > self.rows()
    }
    fn sb_metrics(&self) -> Metrics {
        let l = self.list_rect();
        Metrics { rect: Rect::new(l.right() - 1 - SB_W, l.y + 1, SB_W, l.h - 2), total: self.entries.len() as i32, page: self.rows() as i32, pos: self.scroll as i32, line: 1 }
    }
    fn has_sb(&self) -> bool {
        self.is_file() && self.confirm.is_none()
    }

    /// Reloj del diálogo (unas 100 veces por segundo): repite la flecha o la pista mantenida.
    pub fn tick(&mut self, sys: &mut dyn Platform) {
        if self.has_sb() {
            if let Some(p) = self.sb.tick(&self.sb_metrics(), sys.uptime_ms()) {
                self.scroll = p as usize;
                self.dirty = true;
            }
        }
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
            Mode::Input | Mode::Password => {
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
            text_in(s, Rect::new(list.x + 8, list.y, list.w - 16, ROW_H), if self.mode == Mode::Folder { "(sin subcarpetas)" } else { "(carpeta vacía)" }, MUTED, Align::Left);
        }
        let sbw = if self.need_sb() { SB_W } else { 0 };
        let old = s.clip();
        s.set_clip(old.intersect(list.inflate(-1)));
        for k in 0..self.rows() {
            let i = self.scroll + k;
            let Some(e) = self.entries.get(i) else { break };
            let r = Rect::new(list.x + 1, list.y + 1 + k as i32 * ROW_H, list.w - 2 - sbw, ROW_H);
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
        if sbw > 0 {
            self.sb.draw(s, &self.sb_metrics(), theme);
        }
        if self.mode == Mode::Folder {
            if let Some(e) = &self.error {
                text_in(s, Rect::new(12, list.bottom() + 6, self.w - 24, 20), e, RED, Align::Left);
            }
            return;
        }
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
            Mode::Input | Mode::Password => DialogResult::Text { tag, value: None },
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
        let target = files::resolve("C:\\", dir).unwrap_or_else(|_| String::from("C:\\"));
        let admin = sys.files().is_admin();
        if self.mode == Mode::Folder && is_system(&target) && !admin {
            self.error = Some(String::from("Esa carpeta es del sistema."));
            return false;
        }
        match sys.files().list(dir) {
            Ok(mut v) => {
                if self.mode == Mode::Folder {
                    v.retain(|e| e.dir && (admin || !(target.len() == 3 && e.name.eq_ignore_ascii_case(SYSTEM))));
                }
                self.dir = target;
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
        if self.mode == Mode::Folder {
            let path = match self.sel.and_then(|i| self.entries.get(i)) {
                Some(e) => files::join(&self.dir, &e.name),
                None => self.dir.clone(),
            };
            return Some(DialogResult::Path { tag, path: Some(path) });
        }
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

    /// Crea "Nueva carpeta" (o con número) en la carpeta actual y la deja seleccionada.
    fn new_folder(&mut self, sys: &mut dyn Platform) {
        let name = sys.files().free_name(&self.dir, "Nueva carpeta");
        let path = files::join(&self.dir, &name);
        let now = sys.now();
        if let Err(e) = sys.files().user_create(&path, true, now) {
            self.error = Some(String::from(e.message()));
            return;
        }
        let dir = self.dir.clone();
        if self.go(&dir, sys) {
            if let Some(i) = self.entries.iter().position(|e| e.name == name) {
                self.select(i);
            }
        }
    }

    /// Pulsa el botón `i` (según `labels`).
    fn press(&mut self, i: usize, sys: &mut dyn Platform) -> Option<DialogResult> {
        let tag = self.tag;
        match self.mode {
            Mode::Message => Some(DialogResult::Answer { tag, answer: self.btns[i].1 }),
            Mode::Input => Some(DialogResult::Text { tag, value: if i == 0 { Some(self.field.text.clone()) } else { None } }),
            Mode::Password => {
                let value = if i == 0 { Some(self.field.take()) } else { None };
                self.field.wipe();
                Some(DialogResult::Text { tag, value })
            }
            _ if self.confirm.is_some() => {
                if i == 0 {
                    return Some(DialogResult::Path { tag, path: self.confirm.take() });
                }
                self.confirm = None;
                self.focus = 1;
                None
            }
            Mode::Folder if i == 2 => {
                self.new_folder(sys);
                self.focus = 0;
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
                    Mode::Input | Mode::Password => self.press(0, sys),
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
        if self.has_sb() {
            let e = self.sb.mouse(ev, &self.sb_metrics(), sys.uptime_ms());
            if let Some(p) = e.pos {
                self.scroll = p as usize;
            }
            self.dirty |= e.redraw;
            if e.captured {
                if matches!(ev, MouseEv::Down { .. }) {
                    self.press = None;
                    self.focus = 0;
                }
                return None;
            }
        }
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
                if self.mode != Mode::Message && self.mode != Mode::Folder {
                    if f.contains(x, y) {
                        self.focus = if matches!(self.mode, Mode::Input | Mode::Password) { 0 } else { 1 };
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
            MouseEv::Wheel { dz, .. } if self.is_file() && self.confirm.is_none() => {
                let max = self.entries.len().saturating_sub(self.rows());
                self.scroll = (self.scroll as i64 + dz as i64 * 3).clamp(0, max as i64) as usize;
                self.dirty = true;
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::RamDisk;
    use crate::files::Files;
    use crate::platform::{DateTime, TestPlatform};

    fn docs() -> String {
        files::join(&files::join(files::USERS_DIR, files::DEFAULT_USER), files::Std::Documents.name())
    }
    fn dj(name: &str) -> String {
        files::join(&docs(), name)
    }
    /// Plataforma con disco: administradora Ana y usuario normal Luis, con la sesión de `quien`.
    fn two_users(quien: &str) -> (TestPlatform, DateTime) {
        let n = DateTime::default();
        let mut f = Files::probe(Some(alloc::boxed::Box::new(RamDisk::new(2048))), n);
        f.format_pending(n).unwrap();
        f.create_account("Ana", None, true, [1; 16], n).unwrap();
        f.login("Ana", "").unwrap();
        f.create_account("Luis", None, false, [2; 16], n).unwrap();
        f.login(quien, "").unwrap();
        let mut sys = TestPlatform::default();
        sys.files = f;
        (sys, n)
    }

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
        Dialog::new(DialogRequest::Save { tag: 5, title: "G".into(), dir: docs(), name: name.into() }, sys)
    }
    fn path(p: Option<&str>) -> Option<DialogResult> {
        Some(DialogResult::Path { tag: 5, path: p.map(String::from) })
    }

    #[test]
    fn save_adds_extension_rejects_bad_names_and_confirms_replace() {
        let mut sys = TestPlatform::default();
        let mut d = save("nota", &mut sys);
        assert_eq!(press(&mut d, &mut sys, Key::Enter), path(Some(&dj("nota.txt"))));
        let mut d = save("datos.csv", &mut sys);
        assert_eq!(press(&mut d, &mut sys, Key::Enter), path(Some(&dj("datos.csv"))));
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
        sys.files.write(&dj("viejo.txt"), b"x", now).unwrap();
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
        assert_eq!(press(&mut d, &mut sys, Key::Enter), path(Some(&dj("viejo.txt"))));
        // Cancelar.
        let mut d = save("x", &mut sys);
        assert_eq!(press(&mut d, &mut sys, Key::Escape), path(None));
    }

    #[test]
    fn wheel_scrolls_file_list_only() {
        let mut sys = TestPlatform::default();
        let now = sys.now();
        for i in 0..30 {
            sys.files.write(&dj(&format!("f{i:02}.txt")), b"x", now).unwrap();
        }
        let mut d = Dialog::new(DialogRequest::Open { tag: 5, title: "A".into(), dir: docs() }, &mut sys);
        let max = d.entries.len() - d.rows();
        assert!(max > 6);
        let w = |dz| MouseEv::Wheel { x: 50, y: 50, dz };
        assert!(d.mouse(w(-1), &mut sys).is_none() && d.scroll == 0);
        assert!(d.mouse(w(1), &mut sys).is_none() && d.scroll == 3);
        d.mouse(w(1), &mut sys);
        assert_eq!(d.scroll, 6);
        d.mouse(w(-1), &mut sys);
        assert_eq!(d.scroll, 3);
        d.mouse(w(1000), &mut sys);
        assert_eq!(d.scroll, max);
        d.dirty = false;
        d.mouse(w(-1000), &mut sys);
        assert!(d.scroll == 0 && d.dirty);
        assert!(d.mouse(MouseEv::RightDown { x: 50, y: 50 }, &mut sys).is_none() && d.scroll == 0);
        let (mut m, mut sys) = msg(Buttons::Ok);
        assert!(m.mouse(w(1), &mut sys).is_none() && m.scroll == 0);
        assert!(m.mouse(MouseEv::RightDown { x: 5, y: 5 }, &mut sys).is_none());
        let mut i = Dialog::new(DialogRequest::Input { tag: 3, title: "N".into(), label: "N".into(), value: "ab".into() }, &mut sys);
        assert!(i.mouse(w(1), &mut sys).is_none() && i.scroll == 0);
    }

    #[test]
    fn open_navigates_and_picks_files() {
        let mut sys = TestPlatform::default();
        let now = sys.now();
        sys.files.mkdir(&dj("sub"), now).unwrap();
        sys.files.write(&dj("sub\\a.txt"), b"a", now).unwrap();
        sys.files.write(&dj("b.txt"), b"b", now).unwrap();
        let open = |sys: &mut TestPlatform| Dialog::new(DialogRequest::Open { tag: 5, title: "A".into(), dir: docs() }, sys);
        let mut d = open(&mut sys);
        assert_eq!(d.entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["sub", "b.txt"]);
        // Enter en la carpeta entra; Retroceso sube.
        press(&mut d, &mut sys, Key::Down);
        assert!(press(&mut d, &mut sys, Key::Enter).is_none());
        assert_eq!(d.dir, dj("sub"));
        press(&mut d, &mut sys, Key::Backspace);
        assert_eq!(d.dir, docs());
        // Enter en un archivo lo elige.
        press(&mut d, &mut sys, Key::Down);
        press(&mut d, &mut sys, Key::Down);
        assert_eq!(press(&mut d, &mut sys, Key::Enter), path(Some(&dj("b.txt"))));
        // Doble clic en carpeta, botón ↑ y Abrir con el nombre escrito.
        let mut d = open(&mut sys);
        let l = d.list_rect();
        let p = (l.x + 40, l.y + 5);
        d.mouse(MouseEv::Down { x: p.0, y: p.1 }, &mut sys);
        d.mouse(MouseEv::Up { x: p.0, y: p.1 }, &mut sys);
        d.mouse(MouseEv::Down { x: p.0, y: p.1 }, &mut sys);
        assert_eq!(d.dir, dj("sub"));
        let u = d.up_rect();
        d.mouse(MouseEv::Down { x: u.x + 3, y: u.y + 3 }, &mut sys);
        d.mouse(MouseEv::Up { x: u.x + 3, y: u.y + 3 }, &mut sys);
        assert_eq!(d.dir, docs());
        // Nombre que no existe: error.
        press(&mut d, &mut sys, Key::Tab);
        type_str(&mut d, &mut sys, "zzz");
        assert!(press(&mut d, &mut sys, Key::Enter).is_none() && d.error.is_some());
        let mut s = Surface::new(d.size().0, d.size().1, 0);
        d.draw(&mut s, &Theme::default());
        assert_eq!(press(&mut d, &mut sys, Key::Escape), path(None));
    }

    #[test]
    fn barra_en_la_lista_solo_si_hace_falta() {
        let mut sys = TestPlatform::default();
        let now = sys.now();
        let open = |sys: &mut TestPlatform| Dialog::new(DialogRequest::Open { tag: 5, title: "A".into(), dir: docs() }, sys);
        let mut d = open(&mut sys);
        assert!(!d.need_sb());
        let (w, h) = d.size();
        let mut s = Surface::new(w, h, 0);
        d.draw(&mut s, &Theme::default());
        let m = d.sb_metrics();
        let probe = (m.rect.y + 40) as usize * w as usize + (m.rect.x + 8) as usize;
        assert_eq!(s.px[probe], 0xFFFFFF);
        for i in 0..30 {
            sys.files.write(&dj(&format!("f{i:02}.txt")), b"x", now).unwrap();
        }
        let mut d = open(&mut sys);
        assert!(d.need_sb());
        d.draw(&mut s, &Theme::default());
        assert_ne!(s.px[probe], 0xFFFFFF);
        let m = d.sb_metrics();
        let bx = m.rect.x + 5;
        let rows = d.rows();
        let max = d.entries.len() - rows;
        // Flecha de abajo, flecha de arriba, pista y arrastre.
        let (by, ey) = (m.rect.bottom() - 3, m.rect.y + 3);
        d.mouse(MouseEv::Down { x: bx, y: by }, &mut sys);
        d.mouse(MouseEv::Up { x: bx, y: by }, &mut sys);
        assert_eq!(d.scroll, 1);
        d.mouse(MouseEv::Down { x: bx, y: ey }, &mut sys);
        d.mouse(MouseEv::Up { x: bx, y: ey }, &mut sys);
        assert_eq!(d.scroll, 0);
        let py = m.rect.bottom() - 30;
        d.mouse(MouseEv::Down { x: bx, y: py }, &mut sys);
        d.mouse(MouseEv::Up { x: bx, y: py }, &mut sys);
        assert_eq!(d.scroll, rows);
        let (ts, _) = d.sb.thumb(&d.sb_metrics());
        d.mouse(MouseEv::Down { x: bx, y: m.rect.y + ts + 2 }, &mut sys);
        d.mouse(MouseEv::Move { x: 1, y: 9000 }, &mut sys);
        assert_eq!(d.scroll, max);
        d.mouse(MouseEv::Move { x: 1, y: -9000 }, &mut sys);
        assert_eq!(d.scroll, 0);
        d.mouse(MouseEv::Up { x: 1, y: 1 }, &mut sys);
        assert!(!d.sb.active());
        // Un clic en la barra no selecciona nada; la rueda sigue funcionando.
        assert!(d.sel.is_none());
        d.mouse(MouseEv::Wheel { x: 50, y: 50, dz: 1 }, &mut sys);
        assert_eq!(d.scroll, 3);
    }

    fn folder(dir: &str, sys: &mut TestPlatform) -> Dialog {
        Dialog::new(DialogRequest::Folder { tag: 5, title: "Mover a".into(), dir: dir.into() }, sys)
    }
    fn names(d: &Dialog) -> Vec<&str> {
        d.entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn carpeta_navega_y_solo_lista_carpetas() {
        let mut sys = TestPlatform::default();
        let now = sys.now();
        sys.files.mkdir(&dj("sub"), now).unwrap();
        sys.files.write(&dj("a.txt"), b"a", now).unwrap();
        let mut d = folder(&docs(), &mut sys);
        assert_eq!(names(&d), ["sub"]);
        // Enter con la carpeta resaltada entra; Retroceso sube.
        press(&mut d, &mut sys, Key::Down);
        assert!(press(&mut d, &mut sys, Key::Enter).is_none());
        assert_eq!(d.dir, dj("sub"));
        press(&mut d, &mut sys, Key::Backspace);
        assert_eq!(d.dir, docs());
        // Doble clic entra y el botón de subir vuelve.
        let l = d.list_rect();
        let p = (l.x + 40, l.y + 5);
        for _ in 0..2 {
            d.mouse(MouseEv::Down { x: p.0, y: p.1 }, &mut sys);
            d.mouse(MouseEv::Up { x: p.0, y: p.1 }, &mut sys);
        }
        assert_eq!(d.dir, dj("sub"));
        let u = d.up_rect();
        d.mouse(MouseEv::Down { x: u.x + 3, y: u.y + 3 }, &mut sys);
        d.mouse(MouseEv::Up { x: u.x + 3, y: u.y + 3 }, &mut sys);
        assert_eq!(d.dir, docs());
        let mut s = Surface::new(d.size().0, d.size().1, 0);
        d.draw(&mut s, &Theme::default());
    }

    #[test]
    fn carpeta_elige_la_actual_o_la_resaltada_y_cancela() {
        let mut sys = TestPlatform::default();
        let now = sys.now();
        sys.files.mkdir(&dj("sub"), now).unwrap();
        // Sin selección: la carpeta actual.
        let mut d = folder(&docs(), &mut sys);
        press(&mut d, &mut sys, Key::Tab);
        assert_eq!(press(&mut d, &mut sys, Key::Enter), path(Some(&docs())));
        let mut d = folder(&docs(), &mut sys);
        assert_eq!(press(&mut d, &mut sys, Key::Enter), path(Some(&docs())));
        // Con una resaltada: esa.
        let mut d = folder(&docs(), &mut sys);
        press(&mut d, &mut sys, Key::Down);
        let r = d.btn_rect(0, 3);
        d.mouse(MouseEv::Down { x: r.x + 5, y: r.y + 5 }, &mut sys);
        assert_eq!(d.mouse(MouseEv::Up { x: r.x + 5, y: r.y + 5 }, &mut sys), path(Some(&dj("sub"))));
        // Cancelar con el botón y con Esc.
        let mut d = folder(&docs(), &mut sys);
        let r = d.btn_rect(1, 3);
        d.mouse(MouseEv::Down { x: r.x + 5, y: r.y + 5 }, &mut sys);
        assert_eq!(d.mouse(MouseEv::Up { x: r.x + 5, y: r.y + 5 }, &mut sys), path(None));
        let mut d = folder(&docs(), &mut sys);
        assert_eq!(press(&mut d, &mut sys, Key::Escape), path(None));
        assert_eq!(Some(d.cancel()), path(None));
    }

    #[test]
    fn carpeta_nueva_con_nombre_libre_y_seleccionada() {
        let mut sys = TestPlatform::default();
        let mut d = folder(&docs(), &mut sys);
        let r = d.btn_rect(2, 3);
        for esperado in ["Nueva carpeta", "Nueva carpeta (2)"] {
            d.mouse(MouseEv::Down { x: r.x + 5, y: r.y + 5 }, &mut sys);
            assert!(d.mouse(MouseEv::Up { x: r.x + 5, y: r.y + 5 }, &mut sys).is_none());
            assert!(sys.files.stat(&dj(esperado)).unwrap().dir);
            assert_eq!(d.sel.map(|i| d.entries[i].name.as_str()), Some(esperado));
        }
        // Elegir ahora devuelve la recién creada.
        d.focus = 1;
        assert_eq!(press(&mut d, &mut sys, Key::Enter), path(Some(&dj("Nueva carpeta (2)"))));
        // Con teclado: Tab hasta "Nueva carpeta".
        let pics = sys.files.std_dir(files::Std::Pictures);
        let mut d = folder(&pics, &mut sys);
        for _ in 0..3 {
            press(&mut d, &mut sys, Key::Tab);
        }
        press(&mut d, &mut sys, Key::Enter);
        assert!(sys.files.stat(&files::join(&pics, "Nueva carpeta")).is_ok());
    }

    #[test]
    fn carpeta_oculta_sistema_y_papelera_a_un_usuario_normal() {
        let (mut sys, _) = two_users("Luis");
        assert!(!sys.files.is_admin());
        let d = folder("C:\\", &mut sys);
        let n = names(&d);
        assert!(n.contains(&"Usuarios") && !n.contains(&"Sistema") && !n.iter().any(|x| x.starts_with('$')));
        // Pedir C:\Sistema o la Papelera cae en la raíz.
        let d = folder(files::SYSTEM_DIR, &mut sys);
        assert_eq!(d.dir, "C:\\");
        let d = folder(crate::files::TRASH, &mut sys);
        assert_eq!(d.dir, "C:\\");
        assert!(d.entries.iter().all(|e| e.dir));
        // Dentro de su carpeta no se ven las ocultas (.config, .papelera).
        let d = folder(&sys.files.home(), &mut sys);
        assert!(names(&d).contains(&"Documentos") && !names(&d).iter().any(|x| x.starts_with('.')));
    }

    #[test]
    fn carpeta_deja_entrar_en_sistema_solo_a_un_administrador() {
        let (mut sys, _) = two_users("Ana");
        assert!(sys.files.is_admin());
        let d = folder("C:\\", &mut sys);
        assert!(names(&d).contains(&"Sistema"));
        let d = folder(files::SYS_CONFIG_DIR, &mut sys);
        assert_eq!(d.dir, files::SYS_CONFIG_DIR);
    }

    #[test]
    fn denegado_muestra_el_mensaje_y_no_cambia_de_carpeta() {
        let (mut sys, _) = two_users("Luis");
        let ana = files::join(files::USERS_DIR, "Ana");
        for mut d in [
            Dialog::new(DialogRequest::Open { tag: 5, title: "A".into(), dir: sys.files.std_dir(files::Std::Documents) }, &mut sys),
            folder(&sys.files.std_dir(files::Std::Documents), &mut sys),
        ] {
            let antes = d.dir.clone();
            assert!(!d.go(&ana, &mut sys));
            assert_eq!(d.dir, antes);
            assert_eq!(d.error.as_deref(), Some(FsError::Denied.message()));
            let mut s = Surface::new(d.size().0, d.size().1, 0);
            d.draw(&mut s, &Theme::default());
        }
        // Pedirla desde el principio cae en la raíz.
        let d = Dialog::new(DialogRequest::Open { tag: 5, title: "A".into(), dir: ana }, &mut sys);
        assert_eq!(d.dir, "C:\\");
    }

    fn clave(sys: &mut TestPlatform) -> Dialog {
        Dialog::new(DialogRequest::Password { tag: 9, title: "Clave".into(), label: "Clave actual:".into() }, sys)
    }
    fn texto(v: &str) -> Option<DialogResult> {
        Some(DialogResult::Text { tag: 9, value: Some(v.into()) })
    }

    #[test]
    fn clave_se_enmascara_y_responde_con_enter() {
        let mut sys = TestPlatform::default();
        let mut d = clave(&mut sys);
        type_str(&mut d, &mut sys, "ñandú1");
        assert_eq!(d.field.text, "ñandú1");
        assert_eq!(d.field.shown(), "••••••");
        // Ctrl+C no hace nada y la tecla no escribe.
        let ctrl_c = KeyEvent { key: Key::Char('c'), ch: Some('c'), down: true, shift: false, ctrl: true, alt: false };
        d.key(&ctrl_c, &mut sys);
        assert_eq!(d.field.text, "ñandú1");
        press(&mut d, &mut sys, Key::Backspace);
        type_str(&mut d, &mut sys, "2");
        // Dibujar no falla y el texto real no se pinta (el dibujo con puntos y con letras difiere).
        let mut a = Surface::new(d.size().0, d.size().1, 0);
        d.draw(&mut a, &Theme::default());
        let mut abierto = clave(&mut sys);
        abierto.field.mask = false;
        type_str(&mut abierto, &mut sys, "ñandú2");
        let mut b = Surface::new(d.size().0, d.size().1, 0);
        abierto.draw(&mut b, &Theme::default());
        assert_ne!(a.px, b.px);
        assert_eq!(press(&mut d, &mut sys, Key::Enter), texto("ñandú2"));
        // El búfer queda vacío tras responder.
        assert!(d.field.text.is_empty());
    }

    #[test]
    fn clave_cancelada_con_esc_o_boton_da_none() {
        let mut sys = TestPlatform::default();
        let mut d = clave(&mut sys);
        type_str(&mut d, &mut sys, "secreta");
        assert_eq!(press(&mut d, &mut sys, Key::Escape), Some(DialogResult::Text { tag: 9, value: None }));
        let mut d = clave(&mut sys);
        type_str(&mut d, &mut sys, "secreta");
        press(&mut d, &mut sys, Key::Tab);
        press(&mut d, &mut sys, Key::Tab);
        assert_eq!(press(&mut d, &mut sys, Key::Enter), Some(DialogResult::Text { tag: 9, value: None }));
        assert!(d.field.text.is_empty());
        assert_eq!(d.cancel(), DialogResult::Text { tag: 9, value: None });
    }

    #[test]
    fn clave_el_borrado_deja_ceros_en_el_buffer() {
        let mut f = Field::new("");
        f.mask = true;
        f.text.push_str("abc123");
        let p = f.text.as_ptr();
        f.wipe();
        assert!(f.text.is_empty());
        // SAFETY: el búfer sigue reservado (clear no libera) y se lee dentro de su capacidad.
        let raw = unsafe { core::slice::from_raw_parts(p, 6) };
        assert_eq!(raw, [0u8; 6]);
    }
}
