//! Explorador de archivos: navegación, lista, Papelera y operaciones básicas sobre `Files`.
//! CONTRATO: `pub fn new() -> Box<dyn App>`.
use super::{App, AppKind, Ctx, MouseEv};
use crate::dialogs::{Answer, Buttons, DialogRequest, DialogResult};
use crate::files::{self, file_name, join, parent, valid_name};
use crate::font::{self, Align};
use crate::fs::FsError;
use crate::geom::Rect;
use crate::icons::{self, Icon};
use crate::input::{Key, KeyEvent};
use crate::platform::{DateTime, Platform};
use crate::surface::{blend, Surface};
use crate::sync::SpinLock;
use crate::theme::Theme;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const NAV_H: i32 = 34;
const CMD_H: i32 = 32;
const HEAD_H: i32 = 22;
const ROW_H: i32 = 22;
const STATUS_H: i32 = 22;
const PANEL_W: i32 = 140;
const SB_W: i32 = 10;
const COL_W: [i32; 3] = [152, 152, 76];
const DOUBLE_MS: u64 = 500;
const TEXT: u32 = 0x1A1A1A;
const GRAY: u32 = 0x6A6A6A;
const DISABLED: u32 = 0xA6A6A6;
const LINE: u32 = 0xDADADA;
const PLACES: [(&str, &str, Icon); 5] = [
    ("Escritorio", files::DESKTOP_DIR, Icon::Folder),
    ("Documentos", files::DOCUMENTS_DIR, Icon::Folder),
    ("Imágenes", files::PICTURES_DIR, Icon::Folder),
    ("Disco local (C:)", "C:\\", Icon::Drive),
    ("Papelera", files::TRASH, Icon::Trash),
];
const TAG_NEW: u32 = 1;
const TAG_RENAME: u32 = 2;
const TAG_EMPTY: u32 = 3;
const TAG_ERROR: u32 = 4;

/// Portapapeles compartido entre ventanas: (ruta, es cortar).
static CLIP: SpinLock<Option<(String, bool)>> = SpinLock::new(None);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cmd {
    NewFolder,
    Rename,
    Copy,
    Cut,
    Paste,
    Delete,
    Restore,
    EmptyTrash,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Hit {
    Back,
    Up,
    Crumb(String),
    Cmd(Cmd),
    Place(usize),
}

struct Row {
    name: String,
    dir: bool,
    size: u64,
    date: DateTime,
    /// Papelera: carpeta original.
    loc: String,
    /// Papelera: identificador para restaurar.
    id: String,
}

struct Explorer {
    /// Carpeta actual o `files::TRASH`.
    cwd: String,
    back: Vec<String>,
    rows: Vec<Row>,
    sel: Option<usize>,
    top: usize,
    gen: u64,
    w: i32,
    h: i32,
    hits: Vec<(Rect, Hit)>,
    hover: Option<Hit>,
    last_click: Option<(usize, u64)>,
    /// Desplazamiento del puntero dentro del pulgar de la barra mientras se arrastra.
    drag: Option<i32>,
    /// Nombre a seleccionar en la próxima recarga.
    want: Option<String>,
    rename_from: String,
}

pub fn new() -> Box<dyn App> {
    Box::new(make())
}

fn make() -> Explorer {
    Explorer {
        cwd: String::from(files::DOCUMENTS_DIR),
        back: Vec::new(),
        rows: Vec::new(),
        sel: None,
        top: 0,
        gen: u64::MAX,
        w: 760,
        h: 480,
        hits: Vec::new(),
        hover: None,
        last_click: None,
        drag: None,
        want: None,
        rename_from: String::new(),
    }
}

fn fmt_size(b: u64) -> String {
    if b < 1024 {
        format!("{b} bytes")
    } else if b < 1 << 20 {
        format!("{} KB", b.div_ceil(1024))
    } else if b < 1 << 30 {
        format!("{}.{} MB", b >> 20, (b & 0xF_FFFF) * 10 >> 20)
    } else {
        format!("{}.{} GB", b >> 30, (b & 0x3FFF_FFFF) * 10 >> 30)
    }
}

fn fmt_date(d: DateTime) -> String {
    if d.year == 0 {
        return String::new();
    }
    format!("{:02}/{:02}/{:04} {:02}:{:02}", d.day, d.month, d.year, d.hour, d.minute)
}

fn is_txt(name: &str) -> bool {
    name.len() > 4 && name[name.len() - 4..].eq_ignore_ascii_case(".txt")
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn clipped(s: &mut Surface, r: Rect, text: &str, color: u32, align: Align) {
    let old = s.clip();
    s.set_clip(old.intersect(r));
    font::draw_in(s, r, text, 9, false, color, align);
    s.set_clip(old);
}

fn chevron(s: &mut Surface, x: f32, y: f32, dir: f32, c: u32) {
    s.stroke(&[(x + 2.0 * dir, y - 3.0), (x, y), (x + 2.0 * dir, y + 3.0)], 1.0, c);
}

impl Explorer {
    fn trash(&self) -> bool {
        self.cwd == files::TRASH
    }

    fn sel_row(&self) -> Option<&Row> {
        self.sel.and_then(|i| self.rows.get(i))
    }

    fn vis(&self) -> usize {
        (((self.h - STATUS_H - NAV_H - CMD_H - HEAD_H) / ROW_H).max(1)) as usize
    }

    fn max_top(&self) -> usize {
        self.rows.len().saturating_sub(self.vis())
    }

    fn reload(&mut self, sys: &mut dyn Platform) {
        let keep = self.want.take().or_else(|| self.sel_row().map(|r| r.name.clone()));
        let f = sys.files();
        self.gen = f.generation();
        self.rows.clear();
        if self.trash() {
            for t in f.trash_list().unwrap_or_default() {
                let loc = parent(&t.original).unwrap_or_default();
                self.rows.push(Row { name: t.name, dir: t.dir, size: t.size, date: t.deleted, loc, id: t.id });
            }
        } else {
            loop {
                match f.list(&self.cwd) {
                    Ok(v) => {
                        for e in v {
                            self.rows.push(Row {
                                name: e.name,
                                dir: e.dir,
                                size: e.size,
                                date: e.modified,
                                loc: String::new(),
                                id: String::new(),
                            });
                        }
                        break;
                    }
                    // La carpeta ya no existe: se sube hasta una que sí.
                    Err(_) => match parent(&self.cwd) {
                        Some(p) => self.cwd = p,
                        None => break,
                    },
                }
            }
        }
        self.sel = keep.and_then(|k| self.rows.iter().position(|r| r.name == k));
        self.top = self.top.min(self.max_top());
    }

    fn fail(&self, e: FsError, cx: &mut Ctx) {
        cx.dialog = Some(DialogRequest::Message {
            tag: TAG_ERROR,
            title: String::from("Explorador de archivos"),
            text: String::from(e.message()),
            buttons: Buttons::Ok,
        });
    }

    fn done(&mut self, r: Result<(), FsError>, cx: &mut Ctx) {
        if let Err(e) = r {
            self.fail(e, cx);
        }
        self.reload(cx.sys);
        cx.redraw = true;
    }

    fn go(&mut self, path: &str, push: bool, cx: &mut Ctx) {
        if path != files::TRASH {
            if let Err(e) = cx.sys.files().list(path) {
                self.fail(e, cx);
                return;
            }
        }
        if push && !same(&self.cwd, path) {
            self.back.push(self.cwd.clone());
        }
        self.cwd = String::from(path);
        self.top = 0;
        self.sel = None;
        self.reload(cx.sys);
        cx.redraw = true;
    }

    fn up(&mut self, cx: &mut Ctx) {
        if !self.trash() {
            if let Some(p) = parent(&self.cwd) {
                self.go(&p, true, cx);
            }
        }
    }

    fn go_back(&mut self, cx: &mut Ctx) {
        if let Some(p) = self.back.pop() {
            self.go(&p, false, cx);
        }
    }

    fn open_sel(&mut self, cx: &mut Ctx) {
        if self.trash() {
            return;
        }
        if let Some(r) = self.sel_row() {
            let path = join(&self.cwd, &r.name);
            if r.dir {
                self.go(&path, true, cx);
            } else {
                cx.open = Some(path);
            }
        }
    }

    fn enabled(&self, c: Cmd) -> bool {
        let sel = self.sel.is_some();
        match c {
            Cmd::NewFolder => true,
            Cmd::Rename | Cmd::Copy | Cmd::Cut | Cmd::Delete | Cmd::Restore => sel,
            Cmd::Paste => CLIP.lock().is_some(),
            Cmd::EmptyTrash => !self.rows.is_empty(),
        }
    }

    fn run(&mut self, c: Cmd, cx: &mut Ctx) {
        if !self.enabled(c) {
            return;
        }
        let now = cx.sys.now();
        let path = self.sel_row().map(|r| join(&self.cwd, &r.name)).unwrap_or_default();
        match c {
            Cmd::NewFolder => {
                let name = cx.sys.files().free_name(&self.cwd, "Nueva carpeta");
                cx.dialog = Some(DialogRequest::Input {
                    tag: TAG_NEW,
                    title: String::from("Nueva carpeta"),
                    label: String::from("Nombre de la carpeta:"),
                    value: name,
                });
            }
            Cmd::Rename => {
                let name = String::from(file_name(&path));
                self.rename_from = path;
                cx.dialog = Some(DialogRequest::Input {
                    tag: TAG_RENAME,
                    title: String::from("Cambiar nombre"),
                    label: String::from("Nuevo nombre:"),
                    value: name,
                });
            }
            Cmd::Copy | Cmd::Cut => *CLIP.lock() = Some((path, c == Cmd::Cut)),
            Cmd::Paste => self.paste(now, cx),
            Cmd::Delete => {
                let r = cx.sys.files().trash(&path, now);
                self.done(r, cx);
            }
            Cmd::Restore => {
                let id = self.sel_row().map(|r| r.id.clone()).unwrap_or_default();
                let r = cx.sys.files().restore(&id).map(|_| ());
                self.done(r, cx);
            }
            Cmd::EmptyTrash => {
                cx.dialog = Some(DialogRequest::Message {
                    tag: TAG_EMPTY,
                    title: String::from("Vaciar Papelera"),
                    text: String::from("¿Quieres eliminar para siempre todos los elementos de la Papelera?"),
                    buttons: Buttons::YesNo,
                });
            }
        }
        cx.redraw = true;
    }

    fn paste(&mut self, now: DateTime, cx: &mut Ctx) {
        let Some((src, cut)) = CLIP.lock().clone() else { return };
        let name = String::from(file_name(&src));
        let f = cx.sys.files();
        let r = if cut && parent(&src).is_some_and(|p| same(&p, &self.cwd)) {
            Ok(()) // mover a la misma carpeta: nada que hacer
        } else {
            let name = if f.exists(&join(&self.cwd, &name)) { f.free_name(&self.cwd, &name) } else { name };
            self.want = Some(name.clone());
            let dst = join(&self.cwd, &name);
            if cut {
                f.rename(&src, &dst)
            } else {
                f.copy(&src, &dst, now)
            }
        };
        if cut && r.is_ok() {
            *CLIP.lock() = None;
        }
        self.done(r, cx);
    }

    fn select(&mut self, i: usize) {
        if self.rows.is_empty() {
            return;
        }
        let i = i.min(self.rows.len() - 1);
        self.sel = Some(i);
        let vis = self.vis();
        if i < self.top {
            self.top = i;
        } else if i >= self.top + vis {
            self.top = i + 1 - vis;
        }
    }

    fn step(&mut self, d: i32) {
        let cur = self.sel.map_or(if d > 0 { -1 } else { self.rows.len() as i32 }, |i| i as i32);
        self.select((cur + d).clamp(0, self.rows.len() as i32 - 1).max(0) as usize);
    }

    // ---- geometría ----

    fn need_sb(&self) -> bool {
        self.rows.len() > self.vis()
    }

    fn track(&self) -> Rect {
        let y = NAV_H + CMD_H + HEAD_H;
        Rect::new(self.w - SB_W, y, SB_W, (self.h - STATUS_H - y).max(1))
    }

    /// Pulgar de la barra: (inicio, largo) dentro de la pista.
    fn thumb(&self) -> (i32, i32) {
        let t = self.track();
        let n = self.rows.len() as i32;
        let len = (t.h * self.vis() as i32 / n.max(1)).clamp(20.min(t.h), t.h);
        let max = self.max_top() as i32;
        let pos = if max == 0 { 0 } else { (t.h - len) * self.top as i32 / max };
        (t.y + pos, len)
    }

    fn col_x(&self) -> [i32; 4] {
        let lw = self.w - PANEL_W - if self.need_sb() { SB_W } else { 0 };
        let size = PANEL_W + lw - COL_W[2];
        let b = size - COL_W[1];
        let a = b - COL_W[0];
        [PANEL_W, a, b, size]
    }

    fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        let y0 = NAV_H + CMD_H + HEAD_H;
        if x < PANEL_W || y < y0 || y >= self.h - STATUS_H || (self.need_sb() && x >= self.w - SB_W) {
            return None;
        }
        let i = self.top + ((y - y0) / ROW_H) as usize;
        (i < self.rows.len()).then_some(i)
    }

    fn hit_at(&self, x: i32, y: i32) -> Option<Hit> {
        self.hits.iter().find(|(r, _)| r.contains(x, y)).map(|(_, h)| h.clone())
    }

    // ---- dibujo ----

    fn crumbs(&mut self, s: &mut Surface, r: Rect) {
        let mut parts: Vec<(String, String)> = Vec::new();
        if self.trash() {
            parts.push((String::from("Papelera"), String::from(files::TRASH)));
        } else {
            let mut acc = String::from("C:");
            parts.push((String::from("C:"), String::from("C:\\")));
            for seg in self.cwd.split('\\').skip(1).filter(|p| !p.is_empty()) {
                acc = join(&acc, seg);
                parts.push((String::from(seg), acc.clone()));
            }
        }
        // Si no caben, se quitan las primeras migas.
        let wid = |p: &(String, String)| font::width(&p.0, 9, false) + 16;
        let mut total: i32 = parts.iter().map(wid).sum();
        let mut first = 0;
        while total > r.w - 12 && first + 1 < parts.len() {
            total -= wid(&parts[first]);
            first += 1;
        }
        let mut x = r.x + 6;
        if first > 0 {
            font::draw_in(s, Rect::new(x, r.y, 12, r.h), "…", 9, false, GRAY, Align::Left);
            x += 14;
        }
        let last = parts.len() - 1;
        for (i, (name, path)) in parts.into_iter().enumerate().skip(first) {
            let w = font::width(&name, 9, false) + 8;
            let cr = Rect::new(x, r.y + 3, w, r.h - 6);
            if self.hover == Some(Hit::Crumb(path.clone())) {
                s.fill_rect(cr, 0xE5F1FB);
            }
            font::draw_in(s, cr, &name, 9, false, TEXT, Align::Center);
            self.hits.push((cr, Hit::Crumb(path)));
            x += w;
            if i != last {
                chevron(s, (x + 4) as f32, (r.y + r.h / 2) as f32, -1.0, GRAY);
                x += 8;
            }
        }
    }

    fn nav_bar(&mut self, s: &mut Surface) {
        s.fill_rect(Rect::new(0, 0, self.w, NAV_H), 0xFFFFFF);
        let can_back = !self.back.is_empty();
        let can_up = !self.trash() && parent(&self.cwd).is_some();
        for (i, (hit, ok)) in [(Hit::Back, can_back), (Hit::Up, can_up)].into_iter().enumerate() {
            let r = Rect::new(6 + i as i32 * 30, 5, 26, 24);
            let c = if ok { TEXT } else { DISABLED };
            if ok {
                if self.hover == Some(hit.clone()) {
                    s.fill_rect(r, 0xE5F1FB);
                }
                self.hits.push((r, hit.clone()));
            }
            let (cx, cy) = ((r.x + 13) as f32, (r.y + 12) as f32);
            if hit == Hit::Back {
                s.stroke(&[(cx - 6.0, cy), (cx + 6.0, cy)], 1.5, c);
                chevron(s, cx - 6.0, cy, 1.0, c);
            } else {
                s.stroke(&[(cx, cy + 6.0), (cx, cy - 6.0)], 1.5, c);
                s.stroke(&[(cx - 3.0, cy - 3.0), (cx, cy - 6.0), (cx + 3.0, cy - 3.0)], 1.5, c);
            }
        }
        let bar = Rect::new(70, 5, self.w - 76, 24);
        s.fill_rect(bar, 0xFFFFFF);
        s.frame(bar, 0xC8C8C8);
        let old = s.clip();
        s.set_clip(old.intersect(bar));
        self.crumbs(s, bar);
        s.set_clip(old);
    }

    fn cmd_bar(&mut self, s: &mut Surface) {
        let y = NAV_H;
        s.fill_rect(Rect::new(0, y, self.w, CMD_H), 0xFFFFFF);
        s.fill_rect(Rect::new(0, y + CMD_H - 1, self.w, 1), LINE);
        let cmds: &[(Cmd, &str)] = if self.trash() {
            &[(Cmd::Restore, "Restaurar"), (Cmd::EmptyTrash, "Vaciar Papelera")]
        } else {
            &[
                (Cmd::NewFolder, "Nueva carpeta"),
                (Cmd::Rename, "Renombrar"),
                (Cmd::Copy, "Copiar"),
                (Cmd::Cut, "Cortar"),
                (Cmd::Paste, "Pegar"),
                (Cmd::Delete, "Eliminar"),
            ]
        };
        let mut x = 6;
        for &(c, name) in cmds {
            let w = font::width(name, 9, false) + 20;
            let r = Rect::new(x, y + 4, w, CMD_H - 9);
            let ok = self.enabled(c);
            if ok {
                if self.hover == Some(Hit::Cmd(c)) {
                    s.fill_rect(r, 0xE5F1FB);
                }
                self.hits.push((r, Hit::Cmd(c)));
            }
            font::draw_in(s, r, name, 9, false, if ok { TEXT } else { DISABLED }, Align::Center);
            x += w + 2;
        }
    }

    fn panel(&mut self, s: &mut Surface, theme: &Theme) {
        let y0 = NAV_H + CMD_H;
        s.fill_rect(Rect::new(0, y0, PANEL_W, self.h - y0 - STATUS_H), 0xF7F7F7);
        s.fill_rect(Rect::new(PANEL_W - 1, y0, 1, self.h - y0 - STATUS_H), LINE);
        for (i, (name, path, icon)) in PLACES.iter().enumerate() {
            let r = Rect::new(0, y0 + 8 + i as i32 * 28, PANEL_W - 1, 28);
            if same(&self.cwd, path) {
                s.fill_rect(r, blend(0xF7F7F7, theme.accent, 55));
                s.fill_rect(Rect::new(0, r.y + 5, 3, r.h - 10), theme.accent);
            } else if self.hover == Some(Hit::Place(i)) {
                s.fill_rect(r, 0xE9E9E9);
            }
            icons::draw(s, *icon, 14, r.y + 6, 16, 0x444444);
            clipped(s, Rect::new(38, r.y, PANEL_W - 42, r.h), name, TEXT, Align::Left);
            self.hits.push((r, Hit::Place(i)));
        }
    }

    fn list(&mut self, s: &mut Surface, theme: &Theme) {
        let trash = self.trash();
        let y0 = NAV_H + CMD_H;
        let cx = self.col_x();
        s.fill_rect(Rect::new(PANEL_W, y0, self.w - PANEL_W, self.h - y0 - STATUS_H), 0xFFFFFF);
        let heads: [&str; 4] = if trash {
            ["Nombre", "Ubicación original", "Fecha de eliminación", "Tamaño"]
        } else {
            ["Nombre", "Fecha de modificación", "Tipo", "Tamaño"]
        };
        let end = if self.need_sb() { self.w - SB_W } else { self.w };
        for (k, h) in heads.iter().enumerate() {
            let x1 = if k == 3 { end } else { cx[k + 1] };
            let r = Rect::new(cx[k] + 6, y0, x1 - cx[k] - 10, HEAD_H);
            clipped(s, r, h, GRAY, if k == 3 { Align::Right } else { Align::Left });
            if k > 0 {
                s.fill_rect(Rect::new(cx[k], y0 + 3, 1, HEAD_H - 6), LINE);
            }
        }
        let vis = self.vis();
        for (n, row) in self.rows.iter().enumerate().skip(self.top).take(vis + 1) {
            let y = y0 + HEAD_H + (n - self.top) as i32 * ROW_H;
            if y + ROW_H > self.h - STATUS_H {
                break;
            }
            if self.sel == Some(n) {
                s.fill_rect(Rect::new(PANEL_W, y, end - PANEL_W, ROW_H), blend(0xFFFFFF, theme.accent, 55));
            }
            let icon = if row.dir {
                Icon::Folder
            } else if is_txt(&row.name) {
                Icon::FileText
            } else {
                Icon::FileBlank
            };
            icons::draw(s, icon, PANEL_W + 6, y + 3, 16, 0x444444);
            clipped(s, Rect::new(PANEL_W + 28, y, cx[1] - PANEL_W - 32, ROW_H), &row.name, TEXT, Align::Left);
            let (c1, c2) = if trash {
                (row.loc.clone(), fmt_date(row.date))
            } else {
                let ty = if row.dir {
                    "Carpeta de archivos"
                } else if is_txt(&row.name) {
                    "Documento de texto"
                } else {
                    "Archivo"
                };
                (fmt_date(row.date), String::from(ty))
            };
            clipped(s, Rect::new(cx[1] + 6, y, cx[2] - cx[1] - 10, ROW_H), &c1, GRAY, Align::Left);
            clipped(s, Rect::new(cx[2] + 6, y, cx[3] - cx[2] - 10, ROW_H), &c2, GRAY, Align::Left);
            if !row.dir {
                clipped(s, Rect::new(cx[3] + 6, y, end - cx[3] - 10, ROW_H), &fmt_size(row.size), GRAY, Align::Right);
            }
        }
        if self.need_sb() {
            let t = self.track();
            s.fill_rect(t, 0xF0F0F0);
            let (ty, len) = self.thumb();
            let c = if self.drag.is_some() { 0x7A7A7A } else { 0xB4B4B4 };
            s.fill_rect(Rect::new(t.x + 2, ty, t.w - 4, len), c);
        }
        if self.rows.is_empty() {
            let t = if trash { "La Papelera está vacía." } else { "Esta carpeta está vacía." };
            font::draw_in(s, Rect::new(PANEL_W, y0 + HEAD_H + 10, end - PANEL_W, 20), t, 9, false, GRAY, Align::Center);
        }
    }

    fn status(&mut self, s: &mut Surface, sys: &mut dyn Platform) {
        let y = self.h - STATUS_H;
        s.fill_rect(Rect::new(0, y, self.w, STATUS_H), 0xF7F7F7);
        s.fill_rect(Rect::new(0, y, self.w, 1), LINE);
        let n = self.rows.len();
        let t = if n == 1 { String::from("1 elemento") } else { format!("{n} elementos") };
        font::draw_in(s, Rect::new(10, y, 200, STATUS_H), &t, 9, false, GRAY, Align::Left);
        let (total, free) = sys.files().space();
        let t = format!("{} libres de {}", fmt_size(free), fmt_size(total));
        font::draw_in(s, Rect::new(self.w - 310, y, 300, STATUS_H), &t, 9, false, GRAY, Align::Right);
    }
}

impl App for Explorer {
    fn kind(&self) -> AppKind {
        AppKind::Files
    }
    fn title(&self) -> String {
        if self.trash() {
            String::from("Papelera")
        } else {
            match file_name(&self.cwd) {
                "" => String::from("Disco local (C:)"),
                n => String::from(n),
            }
        }
    }
    fn min_size(&self) -> (i32, i32) {
        (680, 320)
    }

    fn draw(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, _focused: bool) {
        self.w = s.w;
        self.h = s.h;
        if sys.files().generation() != self.gen {
            self.reload(sys);
        }
        self.top = self.top.min(self.max_top());
        self.hits.clear();
        let b = s.bounds();
        s.fill_rect(b, 0xFFFFFF);
        self.list(s, theme);
        self.panel(s, theme);
        self.cmd_bar(s);
        self.nav_bar(s);
        self.status(s, sys);
    }

    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
        let vis = self.vis() as i32;
        if ev.ctrl {
            match ev.key {
                Key::Char('c') => self.run(Cmd::Copy, cx),
                Key::Char('x') => self.run(Cmd::Cut, cx),
                Key::Char('v') => self.run(Cmd::Paste, cx),
                _ => {}
            }
            return;
        }
        let trash = self.trash();
        match ev.key {
            Key::Up => self.step(-1),
            Key::Down => self.step(1),
            Key::PageUp => self.step(-vis),
            Key::PageDown => self.step(vis),
            Key::Home => self.select(0),
            Key::End => self.select(usize::MAX),
            Key::Enter => self.open_sel(cx),
            Key::Backspace => self.up(cx),
            Key::F(2) if !trash => self.run(Cmd::Rename, cx),
            Key::F(5) => self.reload(cx.sys),
            Key::Delete if !trash => self.run(Cmd::Delete, cx),
            _ => return,
        }
        cx.redraw = true;
    }

    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
        match ev {
            MouseEv::Leave => {
                if self.hover.take().is_some() {
                    cx.redraw = true;
                }
            }
            MouseEv::Move { x, y } => {
                if let Some(off) = self.drag {
                    let t = self.track();
                    let (_, len) = self.thumb();
                    let span = (t.h - len).max(1);
                    let p = (y - off - t.y).clamp(0, span);
                    self.top = (p as usize * self.max_top() + span as usize / 2) / span as usize;
                    cx.redraw = true;
                    return;
                }
                let h = self.hit_at(x, y);
                if h != self.hover {
                    self.hover = h;
                    cx.redraw = true;
                }
            }
            MouseEv::Up { .. } => {
                if self.drag.take().is_some() {
                    cx.redraw = true;
                }
            }
            MouseEv::Down { x, y } => {
                cx.redraw = true;
                if let Some(h) = self.hit_at(x, y) {
                    match h {
                        Hit::Back => self.go_back(cx),
                        Hit::Up => self.up(cx),
                        Hit::Crumb(p) => self.go(&p, true, cx),
                        Hit::Cmd(c) => self.run(c, cx),
                        Hit::Place(i) => self.go(PLACES[i].1, true, cx),
                    }
                    return;
                }
                if self.need_sb() && self.track().contains(x, y) {
                    let (ty, len) = self.thumb();
                    if y >= ty && y < ty + len {
                        self.drag = Some(y - ty);
                    } else {
                        let v = self.vis();
                        self.top = if y < ty { self.top.saturating_sub(v) } else { (self.top + v).min(self.max_top()) };
                    }
                    return;
                }
                match self.row_at(x, y) {
                    Some(i) => {
                        self.sel = Some(i);
                        if self.last_click.is_some_and(|(j, t)| j == i && cx.now_ms.saturating_sub(t) < DOUBLE_MS) {
                            self.last_click = None;
                            self.open_sel(cx);
                        } else {
                            self.last_click = Some((i, cx.now_ms));
                        }
                    }
                    None => {
                        if x >= PANEL_W && y >= NAV_H + CMD_H + HEAD_H && y < self.h - STATUS_H {
                            self.sel = None;
                        }
                    }
                }
            }
        }
    }

    fn tick(&mut self, cx: &mut Ctx) {
        if cx.sys.files().generation() != self.gen {
            self.reload(cx.sys);
            cx.redraw = true;
        }
    }

    fn dialog_result(&mut self, result: DialogResult, cx: &mut Ctx) {
        let now = cx.sys.now();
        match result {
            DialogResult::Text { tag: TAG_NEW, value: Some(v) } => {
                let r = if valid_name(&v) {
                    self.want = Some(v.clone());
                    cx.sys.files().mkdir(&join(&self.cwd, &v), now)
                } else {
                    Err(FsError::BadName)
                };
                self.done(r, cx);
            }
            DialogResult::Text { tag: TAG_RENAME, value: Some(v) } => {
                let from = core::mem::take(&mut self.rename_from);
                let r = if file_name(&from) == v {
                    Ok(())
                } else if !valid_name(&v) {
                    Err(FsError::BadName)
                } else {
                    let to = join(&parent(&from).unwrap_or_default(), &v);
                    self.want = Some(v);
                    cx.sys.files().rename(&from, &to)
                };
                self.done(r, cx);
            }
            DialogResult::Answer { tag: TAG_EMPTY, answer: Answer::Yes } => {
                let r = cx.sys.files().empty_trash();
                self.done(r, cx);
            }
            _ => {}
        }
    }

    fn open_path(&mut self, path: &str, cx: &mut Ctx) {
        self.go(path, true, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::TestPlatform;

    /// Los tests comparten el portapapeles: se serializan.
    static SERIAL: SpinLock<()> = SpinLock::new(());

    struct T {
        e: Explorer,
        sys: TestPlatform,
        th: Theme,
        now: u64,
    }

    impl T {
        fn new() -> T {
            let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
            let now = sys.now();
            sys.files.write("C:\\Documentos\\nota.txt", b"hola", now).unwrap();
            sys.files.mkdir("C:\\Documentos\\Sub", now).unwrap();
            let mut t = T { e: make(), sys, th: Theme::default(), now: 1000 };
            t.draw();
            t
        }
        fn draw(&mut self) -> Surface {
            let mut s = Surface::new(760, 480, 0);
            self.e.draw(&mut s, &self.th, &mut self.sys, true);
            s
        }
        fn cx<R>(&mut self, f: impl FnOnce(&mut Explorer, &mut Ctx) -> R) -> (R, Option<DialogRequest>, Option<String>) {
            let mut cx = Ctx::new(&mut self.sys, &mut self.th, self.now);
            let r = f(&mut self.e, &mut cx);
            (r, cx.dialog.take(), cx.open.take())
        }
        fn key(&mut self, key: Key, ctrl: bool) -> (Option<DialogRequest>, Option<String>) {
            let ev = KeyEvent { key, ch: None, down: true, shift: false, ctrl, alt: false };
            let (_, d, o) = self.cx(|e, cx| e.key(&ev, cx));
            self.draw();
            (d, o)
        }
        fn click(&mut self, x: i32, y: i32) -> (Option<DialogRequest>, Option<String>) {
            self.now += 100;
            let (_, d, o) = self.cx(|e, cx| e.mouse(MouseEv::Down { x, y }, cx));
            self.draw();
            (d, o)
        }
        fn answer(&mut self, r: DialogResult) -> Option<DialogRequest> {
            let (_, d, _) = self.cx(|e, cx| e.dialog_result(r, cx));
            self.draw();
            d
        }
        fn open(&mut self, p: &str) {
            self.cx(|e, cx| e.open_path(p, cx));
            self.draw();
        }
        fn names(&mut self) -> Vec<String> {
            self.sys.files.list("C:\\Documentos").unwrap().into_iter().map(|e| e.name).collect()
        }
        /// Centro del botón de órdenes con ese texto (hay que haber dibujado antes).
        fn btn(&self, c: Cmd) -> (i32, i32) {
            let r = self.e.hits.iter().find(|(_, h)| *h == Hit::Cmd(c)).expect("botón activo").0;
            (r.x + r.w / 2, r.y + r.h / 2)
        }
        fn row_y(i: i32) -> i32 {
            NAV_H + CMD_H + HEAD_H + i * ROW_H + 5
        }
    }

    fn text(v: &str) -> DialogResult {
        DialogResult::Text { tag: TAG_NEW, value: Some(String::from(v)) }
    }

    #[test]
    fn navega_y_vuelve_atras() {
        let _g = SERIAL.lock();
        let mut t = T::new();
        assert_eq!(t.e.title(), "Documentos");
        t.open("C:\\");
        assert_eq!(t.e.title(), "Disco local (C:)");
        t.key(Key::Down, false); // carpeta primero
        t.key(Key::Enter, false);
        t.key(Key::Backspace, false);
        t.key(Key::Backspace, false); // ya en la raíz: no hace nada
        assert_eq!(t.e.title(), "Disco local (C:)");
        // Atrás deshace Backspace, luego la entrada, luego el primer open_path.
        let (x, y) = (16, 17);
        t.click(x, y);
        assert_ne!(t.e.title(), "Disco local (C:)");
        t.click(x, y);
        t.click(x, y);
        assert_eq!(t.e.title(), "Documentos");
        // Sin historial el botón no hace nada.
        t.click(x, y);
        assert_eq!(t.e.title(), "Documentos");
    }

    #[test]
    fn doble_clic_carpeta_y_archivo() {
        let _g = SERIAL.lock();
        let mut t = T::new();
        // Orden: carpeta Sub, archivo nota.txt.
        t.click(300, T::row_y(1));
        let (_, o) = t.click(300, T::row_y(1));
        assert_eq!(o.as_deref(), Some("C:\\Documentos\\nota.txt"));
        t.click(300, T::row_y(0));
        t.now += 5000; // demasiado lento: no es doble clic
        t.click(300, T::row_y(0));
        assert_eq!(t.e.title(), "Documentos");
        t.click(300, T::row_y(0));
        assert_eq!(t.e.title(), "Sub");
        // Enter sobre un archivo.
        t.open("C:\\Documentos");
        t.key(Key::Down, false);
        t.key(Key::Down, false);
        let (_, o) = t.key(Key::Enter, false);
        assert_eq!(o.as_deref(), Some("C:\\Documentos\\nota.txt"));
    }

    #[test]
    fn nueva_carpeta_y_renombrar() {
        let _g = SERIAL.lock();
        let mut t = T::new();
        let (x, y) = t.btn(Cmd::NewFolder);
        let (d, _) = t.click(x, y);
        let Some(DialogRequest::Input { tag, value, .. }) = d else { panic!("sin diálogo") };
        assert_eq!((tag, value.as_str()), (TAG_NEW, "Nueva carpeta"));
        t.answer(text("Fotos"));
        assert!(t.names().contains(&String::from("Fotos")));
        // Queda seleccionada; F2 la renombra.
        let (d, _) = t.key(Key::F(2), false);
        let Some(DialogRequest::Input { tag, value, .. }) = d else { panic!("sin diálogo") };
        assert_eq!((tag, value.as_str()), (TAG_RENAME, "Fotos"));
        t.answer(DialogResult::Text { tag: TAG_RENAME, value: Some(String::from("Vacaciones")) });
        let n = t.names();
        assert!(n.contains(&String::from("Vacaciones")) && !n.contains(&String::from("Fotos")));
        // Nombre inválido y repetido: aviso de error.
        let d = t.answer(text("a/b"));
        assert!(matches!(d, Some(DialogRequest::Message { tag: TAG_ERROR, .. })));
        let d = t.answer(text("Sub"));
        assert!(matches!(d, Some(DialogRequest::Message { tag: TAG_ERROR, .. })));
        // Cancelar no hace nada.
        let antes = t.names();
        t.answer(DialogResult::Text { tag: TAG_NEW, value: None });
        assert_eq!(t.names(), antes);
    }

    fn selecciona(t: &mut T, name: &str) {
        let i = t.names().iter().position(|n| n == name).unwrap() as i32;
        t.click(300, T::row_y(i));
    }

    #[test]
    fn copiar_y_pegar() {
        let _g = SERIAL.lock();
        let mut t = T::new();
        selecciona(&mut t, "nota.txt");
        t.key(Key::Char('c'), true);
        t.key(Key::Char('v'), true);
        t.key(Key::Char('v'), true);
        let n = t.names();
        assert!(n.contains(&String::from("nota (2).txt")) && n.contains(&String::from("nota (3).txt")), "{n:?}");
        // A otra carpeta, con el botón Pegar.
        t.open("C:\\Imágenes");
        let (x, y) = t.btn(Cmd::Paste);
        t.click(x, y);
        assert!(t.sys.files.exists("C:\\Imágenes\\nota.txt"));
        assert!(t.sys.files.exists("C:\\Documentos\\nota.txt"));
    }

    #[test]
    fn cortar_y_pegar() {
        let _g = SERIAL.lock();
        let mut t = T::new();
        selecciona(&mut t, "nota.txt");
        t.key(Key::Char('x'), true);
        t.key(Key::Char('v'), true); // misma carpeta: nada
        t.key(Key::Char('x'), true);
        assert!(t.sys.files.exists("C:\\Documentos\\nota.txt"));
        t.open("C:\\Escritorio");
        t.key(Key::Char('v'), true);
        assert!(t.sys.files.exists("C:\\Escritorio\\nota.txt"));
        assert!(!t.sys.files.exists("C:\\Documentos\\nota.txt"));
        // El portapapeles quedó vacío: Pegar está desactivado.
        t.key(Key::Char('v'), true);
        assert_eq!(t.sys.files.list("C:\\Escritorio").unwrap().len(), 1);
    }

    #[test]
    fn eliminar_restaurar_y_vaciar() {
        let _g = SERIAL.lock();
        let mut t = T::new();
        selecciona(&mut t, "nota.txt");
        t.key(Key::Delete, false);
        assert!(!t.sys.files.exists("C:\\Documentos\\nota.txt"));
        assert_eq!(t.sys.files.trash_list().unwrap().len(), 1);
        t.open(files::TRASH);
        assert_eq!(t.e.title(), "Papelera");
        // En la Papelera, Supr y doble clic no hacen nada.
        t.key(Key::Down, false);
        t.key(Key::Delete, false);
        let (_, o) = t.key(Key::Enter, false);
        assert!(o.is_none());
        assert_eq!(t.sys.files.trash_list().unwrap().len(), 1);
        let (x, y) = t.btn(Cmd::Restore);
        t.click(x, y);
        assert!(t.sys.files.exists("C:\\Documentos\\nota.txt"));
        assert!(t.sys.files.trash_list().unwrap().is_empty());
        // Vaciar pide confirmación y solo actúa con Sí.
        t.open("C:\\Documentos");
        selecciona(&mut t, "nota.txt");
        t.key(Key::Delete, false);
        t.open(files::TRASH);
        let (x, y) = t.btn(Cmd::EmptyTrash);
        let (d, _) = t.click(x, y);
        assert!(matches!(d, Some(DialogRequest::Message { tag: TAG_EMPTY, buttons: Buttons::YesNo, .. })));
        t.answer(DialogResult::Answer { tag: TAG_EMPTY, answer: Answer::No });
        assert_eq!(t.sys.files.trash_list().unwrap().len(), 1);
        t.answer(DialogResult::Answer { tag: TAG_EMPTY, answer: Answer::Yes });
        assert!(t.sys.files.trash_list().unwrap().is_empty());
    }

    #[test]
    fn teclado_y_recarga() {
        let _g = SERIAL.lock();
        let mut t = T::new();
        t.key(Key::End, false);
        t.key(Key::Home, false);
        t.key(Key::PageDown, false);
        t.key(Key::Up, false);
        t.key(Key::Down, false);
        // Un cambio externo se ve en tick sin perder la selección por nombre.
        t.key(Key::End, false);
        let now = t.now_dt();
        t.sys.files.mkdir("C:\\Documentos\\Aaa", now).unwrap();
        t.cx(|e, cx| e.tick(cx));
        let s = t.draw();
        assert!(s.px.iter().any(|&p| p != 0));
        t.key(Key::Enter, false); // la selección sigue en nota.txt (archivo)
        let (_, o) = t.key(Key::Enter, false);
        assert_eq!(o.as_deref(), Some("C:\\Documentos\\nota.txt"));
        // Una carpeta borrada desde fuera: se sube a una que exista.
        t.open("C:\\Documentos\\Sub");
        t.sys.files.delete("C:\\Documentos\\Sub").unwrap();
        t.cx(|e, cx| e.tick(cx));
        assert_eq!(t.e.title(), "Documentos");
    }

    impl T {
        fn now_dt(&mut self) -> DateTime {
            self.sys.now()
        }
    }

    #[test]
    fn panel_izquierdo_y_barra() {
        let _g = SERIAL.lock();
        let mut t = T::new();
        t.click(30, NAV_H + CMD_H + 8 + 4); // Escritorio
        assert_eq!(t.e.title(), "Escritorio");
        t.click(30, NAV_H + CMD_H + 8 + 28 * 3 + 4); // Disco local
        assert_eq!(t.e.title(), "Disco local (C:)");
        t.click(30, NAV_H + CMD_H + 8 + 28 * 4 + 4); // Papelera
        assert_eq!(t.e.title(), "Papelera");
        // Muchas filas: aparece la barra y se puede desplazar.
        let now = t.now_dt();
        for i in 0..40 {
            t.sys.files.mkdir(&format!("C:\\Documentos\\d{i:02}"), now).unwrap();
        }
        t.open("C:\\Documentos");
        t.key(Key::End, false);
        t.draw();
        let w = 760;
        t.click(w - 5, 400);
        t.cx(|e, cx| e.mouse(MouseEv::Up { x: w - 5, y: 400 }, cx));
        assert_eq!(fmt_size(1536), "2 KB");
        assert_eq!(fmt_size(5 << 20), "5.0 MB");
    }
}
