//! Configuración: personalización (fondo y color de énfasis), cuentas y datos del sistema.
//! CONTRATO: `pub fn new() -> Box<dyn App>`.
//!
//! Cuentas: las operaciones con varios pasos (poner o cambiar la clave, añadir, renombrar y
//! eliminar una cuenta) encadenan diálogos; lo que se va recogiendo vive en `Job`. Las claves
//! solo pasan por `Secret`, que las borra de la memoria al soltarlas, y nunca van a un título,
//! al registro ni a un archivo.
//!
//! Cifrado (v0.6.0): "Mi cuenta" cifra la carpeta del usuario y la página Disco el disco entero.
//! Ambos flujos son largos (aviso, clave, repetir, pantalla del ratón, código de recuperación);
//! las claves viven en `Job` solo entre un diálogo y el siguiente y el código de recuperación
//! solo existe mientras se construye el diálogo que lo muestra.
use super::{App, AppKind, Ctx, MouseEv};
use crate::accounts::{salt_from, AccountError, MIN_PASSWORD};
use crate::crypto::{ct_eq, wipe};
use crate::dialogs::{Answer, Buttons, DialogRequest, DialogResult};
use crate::font::{self, Align};
use crate::files::Files;
use crate::fs::same;
use crate::geom::Rect;
use crate::icons::{self, Icon};
use crate::input::KeyEvent;
use crate::keys::{self, Pool};
use crate::platform::{DateTime, Platform};
use crate::scrollbar::{Metrics, ScrollBar, SB_W};
use crate::surface::Surface;
use crate::theme::{render_wallpaper, Theme, ACCENTS, WALLPAPERS};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const PANEL_W: i32 = 220;
const THUMB: (i32, i32) = (128, 72);
const TEXT: u32 = 0x1A1A1A;
const GRAY: u32 = 0x6A6A6A;
const SECTIONS: [(&str, Icon); 4] = [("Personalización", Icon::Palette), ("Cuentas", Icon::Cuentas), ("Disco", Icon::DiscoSeguro), ("Acerca de", Icon::Info)];
const PAGE_ACCOUNTS: usize = 1;
const PAGE_DISK: usize = 2;
const PAGE_ABOUT: usize = 3;
const BTN_H: i32 = 26;
/// Paso de la rueda y de las flechas de la barra, en píxeles.
const STEP: i32 = 24;

const TITLE: &str = "Cuentas";
const T_MSG: u32 = 1;
const T_PW_OLD: u32 = 10;
const T_PW_NEW: u32 = 11;
const T_PW_REP: u32 = 12;
const T_ADD_NAME: u32 = 20;
const T_ADD_PW: u32 = 21;
const T_ADD_REP: u32 = 22;
const T_ADD_ADMIN: u32 = 23;
const T_REN: u32 = 30;
const T_DEL_FILES: u32 = 40;
const T_DEL_NAME: u32 = 41;
const T_CONF: u32 = 50;
const T_SEC: u32 = 51;
const T_NEWK: u32 = 52;
const T_REPK: u32 = 53;
const T_WEAK: u32 = 54;
const T_ENT: u32 = 55;
const T_CODE: u32 = 56;
const T_ASK_HOME: u32 = 57;
const NO_MATCH: &str = "Las claves no coinciden.";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Act {
    SetPwd,
    ChangePwd,
    RemovePwd,
    Add,
    /// Índice en `Settings::others`.
    Rename(usize),
    Admin(usize),
    Delete(usize),
    HomeEnc,
    HomeDec,
    HomeCode,
    DiskEnc,
    DiskDec,
    DiskChange,
    DiskCode,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hit {
    Nav(usize),
    Wall(u8),
    Accent(usize),
    Act(Act),
}

/// Texto secreto (claves) que se borra de la memoria al soltarlo.
#[derive(Default)]
struct Secret(String);

impl Drop for Secret {
    fn drop(&mut self) {
        // SAFETY: se escriben ceros, que son UTF-8 válido.
        wipe(unsafe { self.0.as_bytes_mut() });
    }
}

impl Secret {
    fn same(&self, other: &Secret) -> bool {
        ct_eq(self.0.as_bytes(), other.0.as_bytes())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    SetPwd,
    ChangePwd,
    RemovePwd,
    Add,
    Rename,
    Delete,
    HomeEnc,
    HomeDec,
    HomeCode,
    DiskEnc,
    DiskDec,
    DiskChange,
    DiskCode,
}

/// Operación de cuentas en marcha entre un diálogo y el siguiente.
struct Job {
    op: Op,
    /// Cuenta sobre la que se actúa (o la que se crea).
    name: String,
    old: Secret,
    new: Secret,
    /// Mensaje que se muestra al cerrar el diálogo del código de recuperación.
    after: &'static str,
}

struct Settings {
    page: usize,
    thumbs: Vec<Surface>,
    /// Color de énfasis con el que se generó la miniatura de "Color sólido".
    thumb_accent: u32,
    hover: Option<usize>,
    /// Zonas pulsables de lo dibujado por última vez.
    hits: Vec<(Rect, Hit)>,
    /// Segundo (de `now_ms`) de la última vez que se redibujó "Acerca de".
    last_sec: u64,
    /// Página de cuentas: desplazamiento, alto del contenido y tamaño dibujado, barra, y las
    /// demás cuentas (nombre, administrador) tal como se vieron al dibujar.
    scroll: i32,
    content_h: i32,
    size: (i32, i32),
    sb: ScrollBar,
    others: Vec<(String, bool)>,
    job: Option<Job>,
    /// Botón de la página Cuentas bajo el puntero.
    hover_hit: Option<Act>,
}

pub fn new() -> Box<dyn App> {
    Box::new(init())
}

fn init() -> Settings {
    Settings {
        page: 0,
        thumbs: Vec::new(),
        thumb_accent: 0,
        hover: None,
        hits: Vec::new(),
        last_sec: 0,
        scroll: 0,
        content_h: 0,
        size: (0, 0),
        sb: ScrollBar::new(false),
        others: Vec::new(),
        job: None,
        hover_hit: None,
    }
}

fn nav_rect(i: usize) -> Rect {
    Rect::new(0, 56 + 40 * i as i32, PANEL_W, 40)
}

impl Settings {
    fn panel(&mut self, s: &mut Surface, theme: &Theme) {
        s.fill_rect(Rect::new(0, 0, PANEL_W, s.h), 0xF2F2F2);
        font::draw(s, 16, 18, "Configuración", 12, true, TEXT);
        for (i, (name, icon)) in SECTIONS.iter().enumerate() {
            let r = nav_rect(i);
            self.hits.push((r, Hit::Nav(i)));
            if self.page == i {
                s.fill_rect(r, 0xE5E5E5);
                s.fill_rect(Rect::new(0, r.y + 8, 3, r.h - 16), theme.accent);
            } else if self.hover == Some(i) {
                s.fill_rect(r, 0xEAEAEA);
            }
            icons::draw(s, *icon, 16, r.y + 10, 20, 0x444444);
            font::draw_in(s, Rect::new(48, r.y, PANEL_W - 48, r.h), name, 9, false, TEXT, Align::Left);
        }
    }

    fn personalization(&mut self, s: &mut Surface, theme: &Theme) {
        let compact = s.h < 440;
        let x0 = PANEL_W + 24;
        let avail = (s.w - x0 - if compact { 12 } else { 24 }).max(1);
        let mut y = if compact { 14 } else { 24 };
        font::draw(s, x0, y, "Personalización", 18, false, TEXT);
        y += font::line_h(18) + if compact { 4 } else { 10 };
        font::draw(s, x0, y, "Fondo", 11, true, TEXT);
        y += font::line_h(11) + 4;
        // Miniaturas de los fondos.
        let (tw, th) = THUMB;
        let per = ((avail + 8) / (tw + 8)).max(1);
        let row_h = th + 2 + font::line_h(9) + if compact { 4 } else { 8 };
        for (i, name) in WALLPAPERS.iter().enumerate() {
            let (col, row) = (i as i32 % per, i as i32 / per);
            let r = Rect::new(x0 + col * (tw + 8), y + row * row_h, tw, th);
            s.blit(&self.thumbs[i], r.x, r.y);
            let sel = theme.wallpaper as usize == i;
            if sel {
                s.frame(r.inflate(2), theme.accent);
                s.frame(r.inflate(1), theme.accent);
            }
            font::draw(s, r.x, r.bottom() + 2, name, 9, sel, TEXT);
            self.hits.push((r, Hit::Wall(i as u8)));
        }
        y += ((WALLPAPERS.len() as i32 + per - 1) / per) * row_h + if compact { 0 } else { 4 };
        font::draw(s, x0, y, "Color de énfasis", 11, true, TEXT);
        y += font::line_h(11) + 6;
        // Muestras de color.
        let (sz, gap) = if compact { (30, 6) } else { (40, 6) };
        let per = ((avail + gap) / (sz + gap)).max(1);
        for (i, &c) in ACCENTS.iter().enumerate() {
            let (col, row) = (i as i32 % per, i as i32 / per);
            let r = Rect::new(x0 + col * (sz + gap), y + row * (sz + gap), sz, sz);
            s.fill_rect(r, c);
            if c == theme.accent {
                s.frame(r.inflate(2), 0x333333);
                s.frame(r.inflate(1), 0x333333);
                font::draw_in(s, r, "✓", 14, true, 0xFFFFFF, Align::Center);
            }
            self.hits.push((r, Hit::Accent(i)));
        }
    }

    fn about(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform) {
        let compact = s.h < 440;
        let x0 = PANEL_W + 24;
        let mut y = if compact { 14 } else { 24 };
        font::draw(s, x0, y, "Acerca de", 18, false, TEXT);
        y += font::line_h(18) + if compact { 8 } else { 16 };
        icons::draw(s, Icon::Logo, x0, y, 48, theme.accent);
        let name = format!("{} {}", crate::NAME, crate::VERSION);
        font::draw_in(s, Rect::new(x0 + 60, y, (s.w - x0 - 60).max(0), 48), &name, 20, true, TEXT, Align::Left);
        y += 48 + if compact { 12 } else { 20 };
        let mem = sys.mem();
        let mb = |b: u64| b / (1 << 20);
        let up = sys.uptime_ms() / 60_000;
        let (sw, sh) = sys.screen();
        let (total, free) = sys.files().space();
        let disk = format!(
            "{} libres de {}{}",
            super::terminal::units(free),
            super::terminal::units(total),
            if sys.files().persistent() { "" } else { " (en memoria)" }
        );
        let rows = [
            ("Procesador", sys.cpu()),
            ("Memoria instalada", format!("{} MB", mb(mem.total))),
            ("Memoria del sistema", format!("{} / {} MB", mb(mem.heap_used), mb(mem.heap_total))),
            ("Pantalla", format!("{} x {}", sw, sh)),
            ("Disco C:", disk),
            ("Tipo de sistema", String::from("64 bits, arranque UEFI")),
            ("Tiempo encendido", format!("{} h {} min", up / 60, up % 60)),
        ];
        let row_h = if compact { 22 } else { 28 };
        for (label, value) in rows.iter() {
            font::draw_in(s, Rect::new(x0, y, 170, row_h), label, 9, false, GRAY, Align::Left);
            font::draw_in(s, Rect::new(x0 + 170, y, (s.w - x0 - 170).max(0), row_h), value, 9, false, TEXT, Align::Left);
            y += row_h;
        }
        y += if compact { 6 } else { 14 };
        font::draw(s, x0, y, "Sistema operativo escrito desde cero en Rust.", 9, false, GRAY);
    }
}

fn message(cx: &mut Ctx, text: &str) {
    cx.dialog = Some(DialogRequest::Message { tag: T_MSG, title: String::from(TITLE), text: String::from(text), buttons: Buttons::Ok });
}

fn password(cx: &mut Ctx, tag: u32, label: &str) {
    cx.dialog = Some(DialogRequest::Password { tag, title: String::from(TITLE), label: String::from(label) });
}

fn input(cx: &mut Ctx, tag: u32, label: &str, value: &str) {
    cx.dialog = Some(DialogRequest::Input { tag, title: String::from(TITLE), label: String::from(label), value: String::from(value) });
}

/// Texto gris que salta de línea para caber en `width` píxeles. Devuelve la `y` siguiente.
fn note(s: &mut Surface, x: i32, mut y: i32, width: i32, text: &str) -> i32 {
    let mut line = String::new();
    for word in text.split(' ') {
        let try_line = if line.is_empty() { String::from(word) } else { format!("{line} {word}") };
        if !line.is_empty() && font::width(&try_line, 9, false) > width {
            font::draw(s, x, y, &line, 9, false, GRAY);
            y += font::line_h(9);
            line = String::from(word);
        } else {
            line = try_line;
        }
    }
    font::draw(s, x, y, &line, 9, false, GRAY);
    y + font::line_h(9)
}

fn confirm(cx: &mut Ctx, text: &str) {
    cx.dialog = Some(DialogRequest::Message { tag: T_CONF, title: String::from(TITLE), text: String::from(text), buttons: Buttons::YesNo });
}

/// Llama a `f` con el sistema de archivos y el acumulador de aleatoriedad del sistema. El
/// acumulador se saca de `sys` (son dos préstamos del mismo objeto) y SIEMPRE se devuelve.
fn with_pool<R>(cx: &mut Ctx, f: impl FnOnce(&mut Files, &mut Pool) -> R) -> R {
    let mut pool = core::mem::take(cx.sys.pool());
    let r = f(cx.sys.files(), &mut pool);
    *cx.sys.pool() = pool;
    r
}

/// Muestra el código de recuperación (una sola vez) y borra la copia que se tenía aquí.
fn show_code(cx: &mut Ctx, mut code: String) {
    let text = format!("{code}\n\nApúntalo en papel y guárdalo en un lugar seguro. Es la única forma de recuperar tus datos si olvidas la clave. No se volverá a mostrar.");
    // SAFETY: se escriben ceros, que son UTF-8 válido.
    wipe(unsafe { code.as_bytes_mut() });
    cx.dialog = Some(DialogRequest::Message { tag: T_CODE, title: String::from("Código de recuperación"), text, buttons: Buttons::Ok });
}

fn salt(cx: &mut Ctx) -> [u8; 16] {
    let (a, b) = (cx.sys.entropy(), cx.sys.entropy());
    salt_from(a, b)
}

// ---- Página Cuentas: dibujo ----
impl Settings {
    /// Botón en la posición del cursor `at`; salta de línea si no cabe. Devuelve su ancho.
    fn button(&mut self, s: &mut Surface, at: &mut (i32, i32), x0: i32, right: i32, label: &str, act: Act, theme: &Theme) {
        self.button_if(s, at, x0, right, label, act, true, theme);
    }

    /// Como `button`; con `on == false` se dibuja apagado y no se puede pulsar.
    #[allow(clippy::too_many_arguments)]
    fn button_if(&mut self, s: &mut Surface, at: &mut (i32, i32), x0: i32, right: i32, label: &str, act: Act, on: bool, theme: &Theme) {
        let w = font::width(label, 9, false) + 24;
        if at.0 > x0 && at.0 + w > right {
            *at = (x0, at.1 + BTN_H + 6);
        }
        let r = Rect::new(at.0, at.1, w, BTN_H);
        if !on {
            s.fill_rect(r, 0xF0F0F0);
            s.frame(r, 0xCFCFCF);
            font::draw_in(s, r, label, 9, false, 0xA8A8A8, Align::Center);
            at.0 += w + 8;
            return;
        }
        let hot = self.hover_hit == Some(act);
        s.fill_rect(r, if hot { 0xD8D8D8 } else { 0xE6E6E6 });
        s.frame(r, if hot { theme.accent } else { 0xA0A0A0 });
        font::draw_in(s, r, label, 9, false, TEXT, Align::Center);
        self.hits.push((r, Hit::Act(act)));
        at.0 += w + 8;
    }

    fn accounts_page(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform) {
        let compact = s.h < 440;
        let x0 = PANEL_W + 24;
        let right = (s.w - SB_W - 8).max(x0 + 1);
        let mut y = if compact { 14 } else { 24 } - self.scroll;
        font::draw(s, x0, y, TITLE, 18, false, TEXT);
        y += font::line_h(18) + if compact { 6 } else { 12 };
        self.others.clear();
        if !sys.files().persistent() {
            font::draw(s, x0, y, "Sin disco no hay cuentas que guardar.", 9, false, GRAY);
            self.content_h = y + self.scroll + font::line_h(9) + 16;
            return;
        }
        let me = String::from(sys.files().user().unwrap_or(""));
        let admin = sys.files().is_admin();
        let has_pwd = sys.files().has_password();
        let list = sys.files().accounts();
        font::draw(s, x0, y, "Mi cuenta", 11, true, TEXT);
        y += font::line_h(11) + 6;
        let busy = sys.files().home_conversion();
        let crypt = match busy {
            Some((_, _, true)) => "Cifrando…",
            Some((_, _, false)) => "Descifrando…",
            None if sys.files().home_encrypted() => "Sí",
            None => "No",
        };
        for (label, value) in [
            ("Nombre", me.clone()),
            ("Tipo", String::from(if admin { "Administrador" } else { "Usuario" })),
            ("Clave", String::from(if has_pwd { "Puesta" } else { "Sin clave" })),
            ("Carpeta cifrada", String::from(crypt)),
        ] {
            font::draw_in(s, Rect::new(x0, y, 120, 22), label, 9, false, GRAY, Align::Left);
            font::draw_in(s, Rect::new(x0 + 120, y, (right - x0 - 120).max(0), 22), &value, 9, false, TEXT, Align::Left);
            y += 22;
        }
        y += 6;
        let mut at = (x0, y);
        if has_pwd {
            self.button(s, &mut at, x0, right, "Cambiar clave", Act::ChangePwd, theme);
            self.button(s, &mut at, x0, right, "Quitar clave", Act::RemovePwd, theme);
        } else {
            self.button(s, &mut at, x0, right, "Poner clave", Act::SetPwd, theme);
        }
        if has_pwd {
            let (enc, on) = (sys.files().home_encrypted(), busy.is_none());
            if !enc {
                self.button_if(s, &mut at, x0, right, "Cifrar mi carpeta", Act::HomeEnc, on, theme);
            } else {
                self.button_if(s, &mut at, x0, right, "Quitar el cifrado", Act::HomeDec, on, theme);
                self.button_if(s, &mut at, x0, right, "Nuevo código de recuperación", Act::HomeCode, on, theme);
            }
        } else {
            at = (x0, note(s, x0, at.1 + BTN_H + 6, right - x0, "Para cifrar tu carpeta, primero pon una clave.") - BTN_H);
        }
        y = at.1 + BTN_H + if compact { 12 } else { 20 };
        if admin {
            font::draw(s, x0, y, "Otras cuentas", 11, true, TEXT);
            y += font::line_h(11) + 8;
            self.others = list.iter().filter(|a| !same(&a.name, &me)).map(|a| (a.name.clone(), a.admin)).collect();
            if self.others.is_empty() {
                font::draw(s, x0, y, "No hay más cuentas.", 9, false, GRAY);
                y += 24;
            }
            for i in 0..self.others.len() {
                let (name, is_admin) = self.others[i].clone();
                let end = font::draw(s, x0, y, &name, 9, true, TEXT);
                font::draw(s, end + 10, y, if is_admin { "Administrador" } else { "Usuario" }, 9, false, GRAY);
                y += font::line_h(9) + 4;
                let mut at = (x0, y);
                self.button(s, &mut at, x0, right, "Cambiar nombre", Act::Rename(i), theme);
                self.button(s, &mut at, x0, right, if is_admin { "Hacer usuario normal" } else { "Hacer administrador" }, Act::Admin(i), theme);
                self.button(s, &mut at, x0, right, "Eliminar", Act::Delete(i), theme);
                y = at.1 + BTN_H + 12;
            }
            let mut at = (x0, y + 4);
            self.button(s, &mut at, x0, right, "Añadir cuenta", Act::Add, theme);
            y = at.1 + BTN_H;
        }
        self.content_h = y + self.scroll + 16;
    }

    fn disk_page(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform) {
        let compact = s.h < 440;
        let x0 = PANEL_W + 24;
        let right = (s.w - SB_W - 8).max(x0 + 1);
        let mut y = if compact { 14 } else { 24 } - self.scroll;
        font::draw(s, x0, y, "Disco", 18, false, TEXT);
        y += font::line_h(18) + if compact { 6 } else { 12 };
        if !sys.files().persistent() {
            font::draw(s, x0, y, "Sin disco no hay nada que cifrar.", 9, false, GRAY);
            self.content_h = y + self.scroll + font::line_h(9) + 16;
            return;
        }
        let units = super::terminal::units;
        let (total, free) = sys.files().space();
        let admin = sys.files().is_admin();
        let enc = sys.files().disk_encrypted();
        let busy = sys.files().disk_conversion();
        let state = match busy {
            Some((done, all, encrypting)) => format!("{}… {} %", if encrypting { "Cifrando" } else { "Descifrando" }, if all == 0 { 0 } else { (done.min(all) * 100 / all) as u32 }),
            None => String::from(if enc { "Sí" } else { "No" }),
        };
        for (label, value) in [
            ("Unidad", String::from("C:")),
            ("Tamaño", units(total)),
            ("Espacio libre", units(free)),
            ("Persistente", String::from("Sí")),
            ("Cifrado total", state),
        ] {
            font::draw_in(s, Rect::new(x0, y, 120, 22), label, 9, false, GRAY, Align::Left);
            font::draw_in(s, Rect::new(x0 + 120, y, (right - x0 - 120).max(0), 22), &value, 9, false, TEXT, Align::Left);
            y += 22;
        }
        y += 6;
        if admin {
            let mut at = (x0, y);
            let on = busy.is_none();
            if enc {
                self.button_if(s, &mut at, x0, right, "Quitar cifrado total", Act::DiskDec, on, theme);
                self.button_if(s, &mut at, x0, right, "Cambiar la clave del disco", Act::DiskChange, on, theme);
                self.button_if(s, &mut at, x0, right, "Nuevo código de recuperación", Act::DiskCode, on, theme);
            } else {
                self.button_if(s, &mut at, x0, right, "Activar cifrado total", Act::DiskEnc, on, theme);
            }
            y = at.1 + BTN_H;
        } else {
            y = note(s, x0, y, right - x0, "Solo un administrador puede cambiar el cifrado del disco.");
        }
        self.content_h = y + self.scroll + 16;
    }

    fn scrolls(&self) -> bool {
        self.page == PAGE_ACCOUNTS || self.page == PAGE_DISK
    }

    fn metrics(&self) -> Metrics {
        Metrics { rect: Rect::new(self.size.0 - SB_W, 0, SB_W, self.size.1), total: self.content_h, page: self.size.1, pos: self.scroll, line: STEP }
    }

    fn max_scroll(&self) -> i32 {
        (self.content_h - self.size.1).max(0)
    }
}

// ---- Página Cuentas: operaciones ----
impl Settings {
    fn start(&mut self, act: Act, cx: &mut Ctx) {
        let crypto = match act {
            Act::HomeEnc => Some(Op::HomeEnc),
            Act::HomeDec => Some(Op::HomeDec),
            Act::HomeCode => Some(Op::HomeCode),
            Act::DiskEnc => Some(Op::DiskEnc),
            Act::DiskDec => Some(Op::DiskDec),
            Act::DiskChange => Some(Op::DiskChange),
            Act::DiskCode => Some(Op::DiskCode),
            _ => None,
        };
        if let Some(op) = crypto {
            return self.start_crypto(op, cx);
        }
        if act == Act::RemovePwd && cx.sys.files().home_encrypted() {
            return message(cx, AccountError::Encrypted.text());
        }
        let other = |i: usize| self.others.get(i).cloned();
        let (op, name) = match act {
            Act::SetPwd => (Op::SetPwd, String::new()),
            Act::ChangePwd => (Op::ChangePwd, String::new()),
            Act::RemovePwd => (Op::RemovePwd, String::new()),
            Act::Add => (Op::Add, String::new()),
            Act::Rename(i) => match other(i) {
                Some((n, _)) => (Op::Rename, n),
                None => return,
            },
            Act::Delete(i) => match other(i) {
                Some((n, _)) => (Op::Delete, n),
                None => return,
            },
            Act::HomeEnc | Act::HomeDec | Act::HomeCode | Act::DiskEnc | Act::DiskDec | Act::DiskChange | Act::DiskCode => return,
            Act::Admin(i) => {
                let Some((n, is_admin)) = other(i) else { return };
                let now = cx.sys.now();
                if let Err(e) = cx.sys.files().set_admin(&n, !is_admin, now) {
                    message(cx, e.text());
                }
                cx.redraw = true;
                return;
            }
        };
        match act {
            Act::SetPwd => password(cx, T_PW_NEW, "Clave nueva (mínimo 6 caracteres):"),
            Act::ChangePwd | Act::RemovePwd => password(cx, T_PW_OLD, "Clave actual:"),
            Act::Add => input(cx, T_ADD_NAME, "Nombre de la cuenta nueva:", ""),
            Act::Rename(_) => input(cx, T_REN, "Nombre nuevo:", &name),
            _ => {
                cx.dialog = Some(DialogRequest::Message {
                    tag: T_DEL_FILES,
                    title: String::from(TITLE),
                    text: format!("¿Eliminar también sus archivos?\nSí: se borran para siempre.\nNo: se conserva la carpeta de «{name}»."),
                    buttons: Buttons::YesNoCancel,
                })
            }
        }
        self.job = Some(Job { op, name, old: Secret::default(), new: Secret::default(), after: "" });
    }

    /// Primer paso de las operaciones de cifrado: el aviso o la primera clave.
    fn start_crypto(&mut self, op: Op, cx: &mut Ctx) {
        match op {
            Op::HomeEnc => confirm(cx, "Tus archivos quedarán cifrados con una llave que solo abre tu clave. Si olvidas la clave y pierdes el código de recuperación, no se podrán recuperar. ¿Continuar?"),
            Op::HomeDec => confirm(cx, "Tu carpeta se descifrará y sus archivos quedarán sin protección. ¿Continuar?"),
            Op::HomeCode => password(cx, T_SEC, "Tu clave actual:"),
            Op::DiskEnc => confirm(cx, "Se cifrará todo el disco con una clave del disco, distinta de las claves de usuario. Se pedirá cada vez que arranques. Haz antes una copia de seguridad: si olvidas la clave y pierdes el código de recuperación, se pierde todo. ¿Continuar?"),
            Op::DiskDec => confirm(cx, "Se descifrará todo el disco y quedará sin protección. ¿Continuar?"),
            Op::DiskChange => password(cx, T_SEC, "Clave actual del disco:"),
            _ => password(cx, T_SEC, "Clave del disco:"),
        }
        self.job = Some(Job { op, name: String::new(), old: Secret::default(), new: Secret::default(), after: "" });
    }

    /// Pantalla del ratón; al terminar se ejecuta la operación.
    fn entropy(&mut self, job: Job, cx: &mut Ctx) {
        cx.dialog = Some(DialogRequest::Entropy { tag: T_ENT, title: String::from(TITLE) });
        self.job = Some(job);
    }

    /// Ejecuta la operación de cifrado con las claves recogidas y las borra al terminar.
    fn run_crypto(&mut self, mut job: Job, cx: &mut Ctx) {
        let now: DateTime = cx.sys.now();
        let (old, new) = (&job.old.0, &job.new.0);
        let r = with_pool(cx, |f, pool| match job.op {
            Op::HomeEnc => f.encrypt_home(old, pool, now).map(Some),
            Op::HomeDec => f.decrypt_home(old, now).map(|()| None),
            Op::HomeCode => f.new_home_recovery(old, pool, now).map(Some),
            Op::DiskEnc => f.encrypt_disk(new, pool, now).map(Some),
            Op::DiskDec => f.decrypt_disk(old, now).map(|()| None),
            Op::DiskChange => f.change_disk_password(old, new, pool, now).map(|()| None),
            Op::DiskCode => f.new_disk_recovery(old, pool, now).map(Some),
            _ => Ok(None),
        });
        job.old = Secret::default();
        job.new = Secret::default();
        match r {
            Err(e) => message(cx, e.text()),
            Ok(None) => message(
                cx,
                match job.op {
                    Op::HomeDec => "Se está descifrando tu carpeta.",
                    Op::DiskDec => "Se está descifrando el disco. Puedes seguir usando el equipo.",
                    _ => "Se ha cambiado la clave del disco.",
                },
            ),
            Ok(Some(code)) => {
                job.after = match job.op {
                    Op::HomeEnc => "Se está cifrando tu carpeta. Puedes seguir usando el equipo.",
                    Op::DiskEnc => "El disco se está cifrando. Puedes seguir usando el equipo.",
                    _ => "",
                };
                show_code(cx, code);
                self.job = Some(job);
            }
        }
    }

    /// Termina la operación de clave con la clave nueva ya repetida (`None` = quitarla).
    fn finish_password(&mut self, mut job: Job, new: Option<&str>, cx: &mut Ctx) {
        let (now, salt) = (cx.sys.now(), salt(cx));
        let r = cx.sys.files().set_password(&job.old.0, new, salt, now);
        if r.is_ok() && job.op == Op::SetPwd {
            // Recién puesta la clave: se ofrece cifrar la carpeta con ella, sin volver a pedirla.
            job.new = Secret(String::from(new.unwrap_or("")));
            cx.dialog = Some(DialogRequest::Message {
                tag: T_ASK_HOME,
                title: String::from(TITLE),
                text: String::from("Se ha puesto la clave.\n¿Cifrar también tu carpeta? (Recomendado)"),
                buttons: Buttons::YesNo,
            });
            self.job = Some(job);
            return;
        }
        match r {
            Ok(()) => message(
                cx,
                match job.op {
                    Op::SetPwd => "Se ha puesto la clave.",
                    Op::ChangePwd => "Se ha cambiado la clave.",
                    _ => "Se ha quitado la clave.",
                },
            ),
            Err(e) => message(cx, e.text()),
        }
    }

    fn step_text(&mut self, tag: u32, value: Option<Secret>, cx: &mut Ctx) {
        let Some(mut job) = self.job.take() else { return };
        let Some(v) = value else { return };
        match tag {
            T_PW_OLD => {
                if !cx.sys.files().verify_password(&v.0) {
                    return message(cx, AccountError::WrongPassword.text());
                }
                if job.op == Op::RemovePwd {
                    job.old = v;
                    return self.finish_password(job, None, cx);
                }
                job.old = v;
                password(cx, T_PW_NEW, "Clave nueva (mínimo 6 caracteres):");
            }
            T_PW_NEW | T_ADD_PW => {
                // Al crear una cuenta, la clave vacía significa "sin clave".
                if tag == T_ADD_PW && v.0.is_empty() {
                    job.new = v;
                    return self.ask_admin(job, cx);
                }
                if v.0.chars().count() < MIN_PASSWORD {
                    return message(cx, AccountError::WeakPassword.text());
                }
                job.new = v;
                password(cx, if tag == T_PW_NEW { T_PW_REP } else { T_ADD_REP }, "Repite la clave:");
            }
            T_PW_REP | T_ADD_REP => {
                if !job.new.same(&v) {
                    return message(cx, NO_MATCH);
                }
                if tag == T_ADD_REP {
                    return self.ask_admin(job, cx);
                }
                let new = Secret(core::mem::take(&mut job.new.0));
                return self.finish_password(job, Some(&new.0), cx);
            }
            T_SEC => {
                job.old = v;
                match job.op {
                    Op::HomeEnc | Op::HomeDec | Op::HomeCode => {
                        if !cx.sys.files().verify_password(&job.old.0) {
                            return message(cx, AccountError::WrongPassword.text());
                        }
                        return if job.op == Op::HomeDec { self.run_crypto(job, cx) } else { self.entropy(job, cx) };
                    }
                    Op::DiskDec => return self.run_crypto(job, cx),
                    Op::DiskCode => return self.entropy(job, cx),
                    Op::DiskChange => password(cx, T_NEWK, "Clave nueva del disco (mínimo 6 caracteres):"),
                    _ => return,
                }
            }
            T_NEWK => {
                if v.0.chars().count() < MIN_PASSWORD {
                    return message(cx, keys::KeyError::Weak.text());
                }
                job.new = v;
                password(cx, T_REPK, "Repite la clave del disco:");
            }
            T_REPK => {
                if !job.new.same(&v) {
                    return message(cx, NO_MATCH);
                }
                if job.op == Op::DiskChange {
                    return self.run_crypto(job, cx);
                }
                if keys::strength(&job.new.0) < 2 {
                    cx.dialog = Some(DialogRequest::Message {
                        tag: T_WEAK,
                        title: String::from(TITLE),
                        text: String::from("La clave es débil. ¿Usarla de todos modos?"),
                        buttons: Buttons::YesNo,
                    });
                    self.job = Some(job);
                    return;
                }
                return self.entropy(job, cx);
            }
            T_ADD_NAME => {
                if !crate::accounts::valid_user_name(&v.0) {
                    return message(cx, AccountError::BadName.text());
                }
                if cx.sys.files().accounts().iter().any(|a| same(&a.name, &v.0)) {
                    return message(cx, AccountError::Exists.text());
                }
                job.name = v.0.clone();
                password(cx, T_ADD_PW, "Clave (vacía = sin clave):");
            }
            T_REN => {
                let now = cx.sys.now();
                if let Err(e) = cx.sys.files().rename_account(&job.name, &v.0, now) {
                    message(cx, e.text());
                }
                return;
            }
            T_DEL_NAME => {
                if v.0 != job.name {
                    return message(cx, "El nombre no coincide. No se ha eliminado nada.");
                }
                return self.delete(&job.name, false, cx);
            }
            _ => return,
        }
        self.job = Some(job);
    }

    fn ask_admin(&mut self, job: Job, cx: &mut Ctx) {
        cx.dialog = Some(DialogRequest::Message {
            tag: T_ADD_ADMIN,
            title: String::from(TITLE),
            text: format!("¿Será «{}» administrador?\nSí: administrador. No: usuario normal.", job.name),
            buttons: Buttons::YesNoCancel,
        });
        self.job = Some(job);
    }

    fn delete(&mut self, name: &str, keep: bool, cx: &mut Ctx) {
        let now = cx.sys.now();
        match cx.sys.files().delete_account(name, keep, now) {
            Ok(()) => message(cx, if keep { "Cuenta eliminada. Su carpeta se ha conservado." } else { "Cuenta eliminada junto con sus archivos." }),
            Err(e) => message(cx, e.text()),
        }
    }

    fn step_answer(&mut self, tag: u32, answer: Answer, cx: &mut Ctx) {
        let Some(mut job) = self.job.take() else { return };
        if tag == T_CODE {
            if !job.after.is_empty() {
                message(cx, job.after);
            }
            return;
        }
        if answer == Answer::Cancel || (answer == Answer::No && matches!(tag, T_CONF | T_WEAK | T_ASK_HOME)) {
            return;
        }
        match tag {
            T_CONF => {
                match job.op {
                    Op::HomeEnc | Op::HomeDec => password(cx, T_SEC, "Tu clave actual:"),
                    Op::DiskEnc => password(cx, T_NEWK, "Clave nueva del disco (mínimo 6 caracteres):"),
                    Op::DiskDec => password(cx, T_SEC, "Clave del disco:"),
                    _ => return,
                }
                self.job = Some(job);
            }
            T_WEAK => self.entropy(job, cx),
            T_ASK_HOME => {
                job.old = Secret(core::mem::take(&mut job.new.0));
                job.op = Op::HomeEnc;
                self.entropy(job, cx);
            }
            T_ENT => self.run_crypto(job, cx),
            T_ADD_ADMIN => {
                let (now, salt) = (cx.sys.now(), salt(cx));
                let pw = Some(job.new.0.as_str()).filter(|p| !p.is_empty());
                if let Err(e) = cx.sys.files().create_account(&job.name, pw, answer == Answer::Yes, salt, now) {
                    message(cx, e.text());
                }
            }
            T_DEL_FILES if answer == Answer::No => self.delete(&job.name, true, cx),
            T_DEL_FILES => {
                input(cx, T_DEL_NAME, &format!("Para confirmar, escribe el nombre ({}):", job.name), "");
                self.job = Some(job);
            }
            _ => {}
        }
    }
}

impl App for Settings {
    fn kind(&self) -> AppKind {
        AppKind::Settings
    }
    fn title(&self) -> String {
        String::from("Configuración")
    }
    fn min_size(&self) -> (i32, i32) {
        (520, 360)
    }

    fn draw(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, _focused: bool) {
        // Las miniaturas se generan una vez; la de "Color sólido" se rehace si cambia el énfasis.
        if self.thumbs.is_empty() {
            self.thumbs = (0..WALLPAPERS.len() as u8).map(|i| render_wallpaper(i, THUMB.0, THUMB.1, theme.accent)).collect();
            self.thumb_accent = theme.accent;
        } else if self.thumb_accent != theme.accent {
            self.thumbs[3] = render_wallpaper(3, THUMB.0, THUMB.1, theme.accent);
            self.thumb_accent = theme.accent;
        }
        let b = s.bounds();
        self.size = (s.w, s.h);
        // Si el contenido de Cuentas ha encogido, se redibuja ya con el desplazamiento corregido.
        for _ in 0..2 {
            s.fill_rect(b, 0xFFFFFF);
            self.hits.clear();
            self.panel(s, theme);
            // Lo de la derecha no se sale de su zona.
            s.set_clip(Rect::new(PANEL_W, 0, (s.w - PANEL_W).max(0), s.h));
            match self.page {
                0 => self.personalization(s, theme),
                PAGE_ACCOUNTS => self.accounts_page(s, theme, sys),
                PAGE_DISK => self.disk_page(s, theme, sys),
                _ => self.about(s, theme, sys),
            }
            s.reset_clip();
            if !self.scrolls() || self.scroll <= self.max_scroll() {
                break;
            }
            self.scroll = self.max_scroll();
        }
        if self.scrolls() && self.metrics().needed() {
            s.set_clip(Rect::new(PANEL_W, 0, (s.w - PANEL_W).max(0), s.h));
            self.sb.draw(s, &self.metrics(), theme);
            s.reset_clip();
        }
    }

    fn key(&mut self, _ev: &KeyEvent, _cx: &mut Ctx) {}

    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
        let at = |s: &Self, x: i32, y: i32| s.hits.iter().find(|(r, _)| r.contains(x, y)).map(|h| h.1);
        if self.scrolls() {
            if let MouseEv::Wheel { dz, .. } = ev {
                let to = (self.scroll + dz * STEP * 2).clamp(0, self.max_scroll());
                if to != self.scroll {
                    self.scroll = to;
                    cx.redraw = true;
                }
                return;
            }
            let m = self.metrics();
            if m.needed() {
                let e = self.sb.mouse(ev, &m, cx.now_ms);
                if let Some(p) = e.pos {
                    self.scroll = p;
                }
                cx.redraw |= e.redraw;
                if e.captured {
                    return;
                }
            }
        }
        match ev {
            MouseEv::RightDown { .. } | MouseEv::Wheel { .. } => {}
            MouseEv::Move { x, y } => {
                let (h, b) = match at(self, x, y) {
                    Some(Hit::Nav(i)) => (Some(i), None),
                    Some(Hit::Act(a)) => (None, Some(a)),
                    _ => (None, None),
                };
                if h != self.hover || b != self.hover_hit {
                    self.hover = h;
                    self.hover_hit = b;
                    cx.redraw = true;
                }
            }
            MouseEv::Leave => {
                if self.hover.take().is_some() | self.hover_hit.take().is_some() {
                    cx.redraw = true;
                }
            }
            MouseEv::Up { .. } => {}
            MouseEv::Down { x, y } => match at(self, x, y) {
                Some(Hit::Nav(i)) if i != self.page => {
                    self.page = i;
                    self.hits.clear();
                    self.scroll = 0;
                    cx.redraw = true;
                }
                Some(Hit::Act(a)) => self.start(a, cx),
                Some(Hit::Wall(i)) if cx.theme.wallpaper != i => {
                    cx.theme.wallpaper = i;
                    cx.theme_changed = true;
                    cx.redraw = true;
                }
                Some(Hit::Accent(i)) if cx.theme.accent != ACCENTS[i] => {
                    cx.theme.accent = ACCENTS[i];
                    cx.theme_changed = true;
                    cx.redraw = true;
                }
                _ => {}
            },
        }
    }

    fn dialog_result(&mut self, result: DialogResult, cx: &mut Ctx) {
        cx.redraw = true;
        match result {
            DialogResult::Text { tag, value } => self.step_text(tag, value.map(Secret), cx),
            DialogResult::Answer { tag, answer } => self.step_answer(tag, answer, cx),
            DialogResult::Path { .. } => {}
        }
    }

    fn tick(&mut self, cx: &mut Ctx) {
        if self.scrolls() {
            let m = self.metrics();
            if let Some(p) = self.sb.tick(&m, cx.now_ms) {
                self.scroll = p;
                cx.redraw = true;
            }
        }
        // "Tiempo encendido" y memoria cambian solos: redibuja una vez por segundo.
        let sec = cx.now_ms / 1000;
        // El avance de las conversiones de cifrado también cambia solo.
        let moving = match self.page {
            PAGE_ABOUT | PAGE_DISK => true,
            PAGE_ACCOUNTS => cx.sys.files().home_conversion().is_some(),
            _ => false,
        };
        if moving && sec != self.last_sec {
            self.last_sec = sec;
            cx.redraw = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::TestPlatform;

    struct Banco {
        app: Settings,
        sys: TestPlatform,
        theme: Theme,
        redraw: bool,
        changed: bool,
        /// Último diálogo pedido por la app (se queda hasta que se coge con `take`).
        dialog: Option<DialogRequest>,
    }

    impl Banco {
        fn new() -> Self {
            Banco { app: init(), sys: TestPlatform { size: (1280, 800), ..Default::default() }, theme: Theme::default(), redraw: false, changed: false, dialog: None }
        }
        fn dibuja(&mut self, w: i32, h: i32) -> Surface {
            let mut s = Surface::new(w, h, 0);
            self.app.draw(&mut s, &self.theme, &mut self.sys, true);
            s
        }
        fn ev(&mut self, ev: MouseEv) {
            let mut cx = Ctx::new(&mut self.sys, &mut self.theme, 0);
            self.app.mouse(ev, &mut cx);
            self.redraw |= cx.redraw;
            self.changed |= cx.theme_changed;
            if cx.dialog.is_some() {
                self.dialog = cx.dialog.take();
            }
        }
        /// Contesta al diálogo pendiente.
        fn responde(&mut self, r: DialogResult) {
            self.dialog = None;
            let mut cx = Ctx::new(&mut self.sys, &mut self.theme, 0);
            self.app.dialog_result(r, &mut cx);
            self.dialog = cx.dialog.take();
        }
        fn texto(&mut self, tag: u32, v: &str) {
            self.responde(DialogResult::Text { tag, value: Some(String::from(v)) });
        }
        fn respuesta(&mut self, tag: u32, answer: Answer) {
            self.responde(DialogResult::Answer { tag, answer });
        }
        /// Pulsa el botón `act` de la página de cuentas (tras dibujarla).
        fn pulsa(&mut self, act: Act) {
            self.dibuja(780, 520);
            let r = self.app.hits.iter().find(|h| h.1 == Hit::Act(act)).unwrap_or_else(|| panic!("no hay botón {act:?}")).0;
            self.clic(r.x + r.w / 2, r.y + r.h / 2);
        }
        /// Como `pulsa` pero sin volver a dibujar: la pantalla se ha quedado con lo que se vio antes.
        fn pulsa_viejo(&mut self, act: Act) {
            let r = self.app.hits.iter().find(|h| h.1 == Hit::Act(act)).unwrap_or_else(|| panic!("no hay botón {act:?}")).0;
            self.clic(r.x + r.w / 2, r.y + r.h / 2);
        }
        fn hay(&mut self, act: Act) -> bool {
            self.dibuja(780, 520);
            self.app.hits.iter().any(|h| h.1 == Hit::Act(act))
        }
        fn pagina(&mut self, i: usize) {
            self.dibuja(780, 520);
            self.clic(50, 56 + 40 * i as i32 + 5);
        }
        fn mensaje(&mut self) -> String {
            match self.dialog.take() {
                Some(DialogRequest::Message { text, tag: T_MSG, .. }) => text,
                otro => panic!("se esperaba un mensaje y hay {otro:?}"),
            }
        }
        /// Posición de la cuenta `name` en la lista de "Otras cuentas".
        fn fila(&mut self, name: &str) -> usize {
            self.dibuja(780, 520);
            self.app.others.iter().position(|o| o.0 == name).unwrap_or_else(|| panic!("no está {name}"))
        }
        fn files(&mut self) -> &mut crate::files::Files {
            self.sys.files()
        }
        /// Disco persistente con la administradora Ana y el usuario normal Luis; la sesión es de `quien`,
        /// con la página de cuentas abierta.
        fn con_disco(quien: &str) -> Self {
            let n = crate::platform::DateTime::default();
            let mut f = crate::files::Files::probe(Some(Box::new(crate::block::RamDisk::new(2048))), n);
            f.format_pending(n).unwrap();
            f.create_account("Ana", None, true, [1; 16], n).unwrap();
            f.login("Ana", "").unwrap();
            f.create_account("Luis", None, false, [2; 16], n).unwrap();
            f.login(quien, "").unwrap();
            let mut b = Banco::new();
            b.sys.files = f;
            b.pagina(PAGE_ACCOUNTS);
            b.dibuja(780, 520);
            b
        }
        fn clic(&mut self, x: i32, y: i32) {
            self.redraw = false;
            self.changed = false;
            self.ev(MouseEv::Down { x, y });
            self.ev(MouseEv::Up { x, y });
        }
    }

    #[test]
    fn muestra_cambia_acento() {
        let mut b = Banco::new();
        b.dibuja(780, 520);
        // Posición de la muestra 5: buscada por el dibujo (color único en la zona).
        let s = b.dibuja(780, 520);
        let (x, y) = (0..s.h).flat_map(|y| (PANEL_W..s.w).map(move |x| (x, y))).find(|&(x, y)| s.px[(y * s.w + x) as usize] == ACCENTS[5]).unwrap();
        b.clic(x + 2, y + 2);
        assert_eq!(b.theme.accent, ACCENTS[5]);
        assert!(b.changed && b.redraw);
    }

    #[test]
    fn miniatura_cambia_fondo() {
        let mut b = Banco::new();
        b.dibuja(780, 520);
        // Miniatura del fondo 1: en la primera fila, segunda columna.
        b.clic(PANEL_W + 24 + 136 + 10, 24 + font::line_h(18) + 10 + font::line_h(11) + 4 + 10);
        assert_eq!(b.theme.wallpaper, 1);
        assert!(b.changed);
    }

    #[test]
    fn cambio_de_seccion() {
        let mut b = Banco::new();
        b.dibuja(780, 520);
        b.ev(MouseEv::Move { x: 50, y: 56 + 120 + 5 });
        assert!(b.redraw);
        b.clic(50, 56 + 120 + 5);
        assert!(b.redraw && !b.changed);
        let s = b.dibuja(780, 520);
        // En "Acerca de" ya no hay muestras de color en la zona de contenido.
        assert!(!s.px.iter().any(|&p| p == ACCENTS[6]));
        b.clic(50, 56 + 5);
        let s = b.dibuja(780, 520);
        assert!(s.px.iter().any(|&p| p == ACCENTS[6]));
    }

    #[test]
    fn tick_solo_en_acerca_de() {
        let mut b = Banco::new();
        b.dibuja(780, 520);
        let mut cx = Ctx::new(&mut b.sys, &mut b.theme, 5000);
        b.app.tick(&mut cx);
        assert!(!cx.redraw);
        drop(cx);
        b.clic(50, 56 + 120 + 5);
        b.dibuja(780, 520);
        let mut cx = Ctx::new(&mut b.sys, &mut b.theme, 6000);
        b.app.tick(&mut cx);
        assert!(cx.redraw);
        cx.redraw = false;
        b.app.tick(&mut cx);
        assert!(!cx.redraw);
    }

    #[test]
    fn tamanos_extremos() {
        let mut b = Banco::new();
        for pagina in 0..4 {
            b.clic(50, 56 + 40 * pagina + 5);
            for (w, h) in [(520, 360), (780, 520), (1, 1), (0, 0), (221, 400), (600, 1)] {
                b.dibuja(w, h);
            }
            b.ev(MouseEv::Leave);
        }
        // Color sólido se regenera con el nuevo énfasis sin pánico.
        b.theme.accent = ACCENTS[3];
        b.dibuja(520, 360);
    }

    fn pw_dialog(b: &Banco, tag: u32) {
        assert!(matches!(&b.dialog, Some(DialogRequest::Password { tag: t, .. }) if *t == tag), "{:?}", b.dialog);
    }

    #[test]
    fn poner_cambiar_y_quitar_la_clave() {
        let mut b = Banco::con_disco("Ana");
        assert!(b.hay(Act::SetPwd) && !b.hay(Act::ChangePwd) && !b.hay(Act::RemovePwd));
        // Poner.
        b.pulsa(Act::SetPwd);
        pw_dialog(&b, T_PW_NEW);
        b.texto(T_PW_NEW, "secreta1");
        pw_dialog(&b, T_PW_REP);
        b.texto(T_PW_REP, "secreta1");
        assert!(b.files().has_password() && b.files().verify_password("secreta1"));
        assert!(matches!(&b.dialog, Some(DialogRequest::Message { tag: T_ASK_HOME, buttons: Buttons::YesNo, text, .. }) if text.contains("Se ha puesto la clave")));
        b.respuesta(T_ASK_HOME, Answer::No);
        assert!(b.dialog.is_none() && b.app.job.is_none(), "las claves se sueltan al terminar");
        // Cambiar: la clave actual mala corta el proceso.
        assert!(b.hay(Act::ChangePwd) && b.hay(Act::RemovePwd) && !b.hay(Act::SetPwd));
        b.pulsa(Act::ChangePwd);
        pw_dialog(&b, T_PW_OLD);
        b.texto(T_PW_OLD, "mala-clave");
        assert_eq!(b.mensaje(), AccountError::WrongPassword.text());
        assert!(b.app.job.is_none() && b.files().verify_password("secreta1"));
        b.pulsa(Act::ChangePwd);
        b.texto(T_PW_OLD, "secreta1");
        pw_dialog(&b, T_PW_NEW);
        b.texto(T_PW_NEW, "otra-clave");
        pw_dialog(&b, T_PW_REP);
        b.texto(T_PW_REP, "otra-clave");
        assert_eq!(b.mensaje(), "Se ha cambiado la clave.");
        assert!(b.files().verify_password("otra-clave") && !b.files().verify_password("secreta1"));
        // Quitar.
        b.pulsa(Act::RemovePwd);
        b.texto(T_PW_OLD, "otra-clave");
        assert_eq!(b.mensaje(), "Se ha quitado la clave.");
        assert!(!b.files().has_password() && b.files().verify_password(""));
        assert!(b.hay(Act::SetPwd));
    }

    #[test]
    fn claves_que_no_coinciden_cortas_o_canceladas_no_cambian_nada() {
        let mut b = Banco::con_disco("Ana");
        b.pulsa(Act::SetPwd);
        b.texto(T_PW_NEW, "secreta1");
        b.texto(T_PW_REP, "secreta2");
        assert_eq!(b.mensaje(), "Las claves no coinciden.");
        assert!(!b.files().has_password() && b.app.job.is_none());
        b.pulsa(Act::SetPwd);
        b.texto(T_PW_NEW, "corta");
        assert_eq!(b.mensaje(), AccountError::WeakPassword.text());
        assert!(!b.files().has_password() && b.app.job.is_none());
        // Cancelar en cualquier paso.
        b.pulsa(Act::SetPwd);
        b.responde(DialogResult::Text { tag: T_PW_NEW, value: None });
        assert!(b.dialog.is_none() && b.app.job.is_none());
        b.pulsa(Act::SetPwd);
        b.texto(T_PW_NEW, "secreta1");
        b.responde(DialogResult::Text { tag: T_PW_REP, value: None });
        assert!(b.dialog.is_none() && b.app.job.is_none() && !b.files().has_password());
        // Con clave puesta, cancelar la actual tampoco cambia nada.
        let n = crate::platform::DateTime::default();
        b.files().set_password("", Some("secreta1"), [3; 16], n).unwrap();
        b.pulsa(Act::RemovePwd);
        b.responde(DialogResult::Text { tag: T_PW_OLD, value: None });
        assert!(b.dialog.is_none() && b.files().has_password());
    }

    #[test]
    fn anadir_cuenta_con_y_sin_clave_y_con_errores() {
        let mut b = Banco::con_disco("Ana");
        // Sin clave y administradora.
        b.pulsa(Act::Add);
        assert!(matches!(&b.dialog, Some(DialogRequest::Input { tag: T_ADD_NAME, .. })));
        b.texto(T_ADD_NAME, "Marta");
        pw_dialog(&b, T_ADD_PW);
        b.texto(T_ADD_PW, "");
        assert!(matches!(&b.dialog, Some(DialogRequest::Message { tag: T_ADD_ADMIN, buttons: Buttons::YesNoCancel, .. })));
        b.respuesta(T_ADD_ADMIN, Answer::Yes);
        assert!(b.dialog.is_none());
        let marta = b.files().accounts().into_iter().find(|a| a.name == "Marta").unwrap();
        assert!(marta.admin && !marta.has_password());
        assert!(b.files().exists("C:\\Usuarios\\Marta\\Documentos"));
        // Con clave y usuaria normal.
        b.pulsa(Act::Add);
        b.texto(T_ADD_NAME, "Pepe");
        b.texto(T_ADD_PW, "clavepepe");
        pw_dialog(&b, T_ADD_REP);
        b.texto(T_ADD_REP, "clavepepe");
        b.respuesta(T_ADD_ADMIN, Answer::No);
        let pepe = b.files().accounts().into_iter().find(|a| a.name == "Pepe").unwrap();
        assert!(!pepe.admin && pepe.has_password() && pepe.verify("clavepepe") && !pepe.verify(""));
        // Errores: nombre mal, repetido (sin distinguir mayúsculas), claves distintas, corta y cancelar al final.
        b.pulsa(Act::Add);
        b.texto(T_ADD_NAME, "a|b");
        assert_eq!(b.mensaje(), AccountError::BadName.text());
        b.pulsa(Act::Add);
        b.texto(T_ADD_NAME, "luis");
        assert_eq!(b.mensaje(), AccountError::Exists.text());
        b.pulsa(Act::Add);
        b.texto(T_ADD_NAME, "Nora");
        b.texto(T_ADD_PW, "clavenora");
        b.texto(T_ADD_REP, "otraclave");
        assert_eq!(b.mensaje(), "Las claves no coinciden.");
        b.pulsa(Act::Add);
        b.texto(T_ADD_NAME, "Nora");
        b.texto(T_ADD_PW, "corta");
        assert_eq!(b.mensaje(), AccountError::WeakPassword.text());
        b.pulsa(Act::Add);
        b.texto(T_ADD_NAME, "Nora");
        b.texto(T_ADD_PW, "");
        b.respuesta(T_ADD_ADMIN, Answer::Cancel);
        assert!(b.dialog.is_none() && !b.files().accounts().iter().any(|a| a.name == "Nora"));
        assert_eq!(b.files().accounts().len(), 4);
        // La lista dibujada incluye las nuevas.
        b.dibuja(780, 520);
        assert_eq!(b.app.others.iter().map(|o| o.0.as_str()).collect::<Vec<_>>(), ["Luis", "Marta", "Pepe"]);
    }

    #[test]
    fn renombrar_y_cambiar_el_tipo_de_otra_cuenta() {
        let mut b = Banco::con_disco("Ana");
        let luis = b.fila("Luis");
        b.files().write("C:\\Usuarios\\Luis\\Documentos\\a.txt", b"x", crate::platform::DateTime::default()).unwrap();
        b.pulsa(Act::Rename(luis));
        assert!(matches!(&b.dialog, Some(DialogRequest::Input { tag: T_REN, value, .. }) if value == "Luis"));
        b.texto(T_REN, "Luisa");
        assert!(b.dialog.is_none());
        assert!(b.files().accounts().iter().any(|a| a.name == "Luisa") && b.files().exists("C:\\Usuarios\\Luisa\\Documentos\\a.txt"));
        assert!(!b.files().exists("C:\\Usuarios\\Luis"));
        // Errores: nombre repetido y nombre malo.
        let luisa = b.fila("Luisa");
        b.pulsa(Act::Rename(luisa));
        b.texto(T_REN, "ana");
        assert_eq!(b.mensaje(), AccountError::Exists.text());
        b.pulsa(Act::Rename(luisa));
        b.texto(T_REN, "x|y");
        assert_eq!(b.mensaje(), AccountError::BadName.text());
        // Hacer administradora y volver a usuaria normal.
        b.pulsa(Act::Admin(luisa));
        assert!(b.dialog.is_none() && b.files().accounts().iter().find(|a| a.name == "Luisa").unwrap().admin);
        b.dibuja(780, 520);
        assert!(b.app.others[luisa].1);
        b.pulsa(Act::Admin(luisa));
        assert!(!b.files().accounts().iter().find(|a| a.name == "Luisa").unwrap().admin);
    }

    #[test]
    fn eliminar_conservando_o_con_archivos_y_confirmando_el_nombre() {
        let mut b = Banco::con_disco("Ana");
        let n = crate::platform::DateTime::default();
        for c in ["Marta", "Pepe"] {
            b.files().create_account(c, None, false, [5; 16], n).unwrap();
            b.files().write(&format!("C:\\Usuarios\\{c}\\Documentos\\a.txt"), b"x", n).unwrap();
        }
        // Conservar la carpeta: No.
        let luis = b.fila("Luis");
        b.files().write("C:\\Usuarios\\Luis\\Documentos\\a.txt", b"x", n).unwrap();
        b.pulsa(Act::Delete(luis));
        assert!(matches!(&b.dialog, Some(DialogRequest::Message { tag: T_DEL_FILES, buttons: Buttons::YesNoCancel, .. })));
        b.respuesta(T_DEL_FILES, Answer::No);
        assert!(b.mensaje().contains("conservado"));
        assert!(!b.files().accounts().iter().any(|a| a.name == "Luis") && b.files().exists("C:\\Usuarios\\Luis\\Documentos\\a.txt"));
        // Cancelar: no se toca nada.
        let marta = b.fila("Marta");
        b.pulsa(Act::Delete(marta));
        b.respuesta(T_DEL_FILES, Answer::Cancel);
        assert!(b.dialog.is_none() && b.app.job.is_none() && b.files().accounts().iter().any(|a| a.name == "Marta"));
        // Con archivos: pide el nombre; mal escrito (otra capitalización o vacío) no elimina.
        b.pulsa(Act::Delete(marta));
        b.respuesta(T_DEL_FILES, Answer::Yes);
        assert!(matches!(&b.dialog, Some(DialogRequest::Input { tag: T_DEL_NAME, .. })));
        b.texto(T_DEL_NAME, "marta");
        assert!(b.mensaje().contains("No se ha eliminado nada"));
        assert!(b.files().accounts().iter().any(|a| a.name == "Marta") && b.files().exists("C:\\Usuarios\\Marta\\Documentos\\a.txt"));
        b.pulsa(Act::Delete(marta));
        b.respuesta(T_DEL_FILES, Answer::Yes);
        b.texto(T_DEL_NAME, "");
        assert!(b.mensaje().contains("No se ha eliminado nada") && b.files().accounts().iter().any(|a| a.name == "Marta"));
        // Cancelar el nombre tampoco.
        b.pulsa(Act::Delete(marta));
        b.respuesta(T_DEL_FILES, Answer::Yes);
        b.responde(DialogResult::Text { tag: T_DEL_NAME, value: None });
        assert!(b.dialog.is_none() && b.files().accounts().iter().any(|a| a.name == "Marta"));
        // Bien escrito: se elimina con sus archivos.
        b.pulsa(Act::Delete(marta));
        b.respuesta(T_DEL_FILES, Answer::Yes);
        b.texto(T_DEL_NAME, "Marta");
        assert!(b.mensaje().contains("sus archivos"));
        assert!(!b.files().accounts().iter().any(|a| a.name == "Marta") && !b.files().exists("C:\\Usuarios\\Marta"));
        assert!(b.files().exists("C:\\Usuarios\\Pepe\\Documentos\\a.txt") && b.app.job.is_none());
    }

    #[test]
    fn errores_de_cuentas_se_muestran() {
        // Tres administradoras y una usuaria; la lista se dibuja con una sesión y luego cambia
        // la situación (la pantalla puede ir por detrás del disco).
        let mut b = Banco::con_disco("Ana");
        let n = crate::platform::DateTime::default();
        b.files().set_admin("Luis", true, n).unwrap();
        let luis = b.fila("Luis");
        // Sesión de Luis, único administrador: no puede quitarse el tipo ni eliminarse a sí mismo.
        b.files().login("Luis", "").unwrap();
        b.files().set_admin("Ana", false, n).unwrap();
        b.pulsa_viejo(Act::Admin(luis));
        assert_eq!(b.mensaje(), AccountError::LastAdmin.text());
        b.pulsa_viejo(Act::Delete(luis));
        b.respuesta(T_DEL_FILES, Answer::No);
        assert_eq!(b.mensaje(), AccountError::InUse.text());
        // Un usuario normal que aún tiene la pantalla de administrador.
        let mut b = Banco::con_disco("Ana");
        let luis = b.fila("Luis");
        b.files().login("Luis", "").unwrap();
        b.pulsa_viejo(Act::Admin(luis));
        assert_eq!(b.mensaje(), AccountError::NotAdmin.text());
        b.pulsa_viejo(Act::Rename(luis));
        b.texto(T_REN, "Otro");
        assert_eq!(b.mensaje(), AccountError::NotAdmin.text());
        b.pulsa_viejo(Act::Delete(luis));
        b.respuesta(T_DEL_FILES, Answer::No);
        assert_eq!(b.mensaje(), AccountError::NotAdmin.text());
    }

    #[test]
    fn un_usuario_normal_solo_ve_su_cuenta() {
        let mut b = Banco::con_disco("Luis");
        assert!(b.hay(Act::SetPwd) && !b.hay(Act::Add));
        assert!(b.app.others.is_empty() && !b.app.hits.iter().any(|h| matches!(h.1, Hit::Act(Act::Rename(_) | Act::Admin(_) | Act::Delete(_)))));
        // Su clave sí la puede cambiar.
        b.pulsa(Act::SetPwd);
        b.texto(T_PW_NEW, "clave-luis");
        b.texto(T_PW_REP, "clave-luis");
        b.respuesta(T_ASK_HOME, Answer::No);
        assert!(b.files().verify_password("clave-luis"));
        // Sin sesión de administrador, las cuentas de los demás siguen intactas.
        assert_eq!(b.files().accounts().len(), 2);
    }

    #[test]
    fn sin_disco_se_avisa_y_no_hay_acciones() {
        let mut b = Banco::new();
        b.pagina(PAGE_ACCOUNTS);
        let s = b.dibuja(780, 520);
        assert!(!b.app.hits.iter().any(|h| matches!(h.1, Hit::Act(_))));
        assert!(s.px.iter().any(|&p| p != 0xFFFFFF));
        assert!(!b.sys.files().persistent());
        // Un clic donde estaría un botón no hace nada.
        b.clic(PANEL_W + 40, 200);
        assert!(b.dialog.is_none());
    }

    #[test]
    fn la_lista_larga_se_desplaza_y_se_dibuja_en_cualquier_tamano() {
        let mut b = Banco::con_disco("Ana");
        let n = crate::platform::DateTime::default();
        for i in 0..14 {
            b.files().create_account(&format!("Cuenta{i:02}"), None, false, [6; 16], n).unwrap();
        }
        b.dibuja(780, 520);
        assert!(b.app.metrics().needed());
        let max = b.app.max_scroll();
        b.ev(MouseEv::Wheel { x: 400, y: 300, dz: 1 });
        assert!(b.app.scroll > 0 && b.redraw);
        b.ev(MouseEv::Wheel { x: 400, y: 300, dz: 1000 });
        assert_eq!(b.app.scroll, max);
        b.dibuja(780, 520);
        // El último botón (Añadir cuenta) está a la vista y se puede pulsar.
        b.pulsa(Act::Add);
        assert!(matches!(&b.dialog, Some(DialogRequest::Input { tag: T_ADD_NAME, .. })));
        // Si la ventana crece, el desplazamiento se corrige solo.
        b.dialog = None;
        b.dibuja(780, 1400);
        assert_eq!(b.app.scroll, 0);
        for (w, h) in [(520, 360), (1, 1), (0, 0), (221, 400), (600, 1), (780, 520)] {
            b.dibuja(w, h);
        }
        // La barra arrastra el contenido.
        b.dibuja(780, 520);
        let m = b.app.metrics();
        let (ts, _) = b.app.sb.thumb(&m);
        b.ev(MouseEv::Down { x: m.rect.x + 5, y: m.rect.y + ts + 2 });
        b.ev(MouseEv::Move { x: m.rect.x + 5, y: 9000 });
        b.ev(MouseEv::Up { x: m.rect.x + 5, y: 9000 });
        assert_eq!(b.app.scroll, b.app.max_scroll());
    }

    // ---- Cifrado (v0.6.0) ----

    /// Disco en memoria que se puede reabrir tras "reiniciar".
    #[derive(Clone)]
    struct Compartido(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);
    impl crate::block::BlockDev for Compartido {
        fn blocks(&self) -> u64 {
            (self.0.borrow().len() / crate::block::BLOCK) as u64
        }
        fn read(&mut self, n: u64, buf: &mut [u8]) -> Result<(), crate::block::DiskError> {
            let o = n as usize * crate::block::BLOCK;
            buf.copy_from_slice(self.0.borrow().get(o..o + crate::block::BLOCK).ok_or(crate::block::DiskError::Range)?);
            Ok(())
        }
        fn write(&mut self, n: u64, buf: &[u8]) -> Result<(), crate::block::DiskError> {
            let o = n as usize * crate::block::BLOCK;
            self.0.borrow_mut().get_mut(o..o + crate::block::BLOCK).ok_or(crate::block::DiskError::Range)?.copy_from_slice(buf);
            Ok(())
        }
    }

    const CLAVE_DISCO: &str = "Clave-del-disco-9!";
    const NUEVA: &str = "Otra-clave-nueva-7?";
    const AHORA: crate::platform::DateTime = crate::platform::DateTime { year: 2026, month: 11, day: 15, hour: 21, minute: 37, second: 5 };

    impl Banco {
        /// Como `con_disco` pero sobre un disco compartido de 300 bloques, con la página `pagina` abierta.
        fn con_disco_compartido(quien: &str, pagina: usize) -> (Banco, Compartido) {
            let disco = Compartido(std::rc::Rc::new(std::cell::RefCell::new(vec![0; 300 * crate::block::BLOCK])));
            let mut f = crate::files::Files::probe(Some(Box::new(disco.clone())), AHORA);
            f.format_pending(AHORA).unwrap();
            f.create_account("Ana", None, true, [1; 16], AHORA).unwrap();
            f.login("Ana", "").unwrap();
            f.create_account("Luis", None, false, [2; 16], AHORA).unwrap();
            f.login(quien, "").unwrap();
            let mut b = Banco::new();
            b.sys.files = f;
            b.pagina(pagina);
            b.dibuja(780, 520);
            (b, disco)
        }
        fn pool_listo(&mut self) {
            for i in 0..300 {
                self.sys.pool.add_event(i, i * 5, i as u64, i as u64 * 11);
            }
        }
        /// Texto del diálogo `Message` pendiente con esa etiqueta (sin quitarlo).
        fn texto_de(&self, tag: u32) -> String {
            match &self.dialog {
                Some(DialogRequest::Message { tag: t, text, .. }) if *t == tag => text.clone(),
                otro => panic!("se esperaba el mensaje {tag} y hay {otro:?}"),
            }
        }
        fn entropia(&self) {
            assert!(matches!(&self.dialog, Some(DialogRequest::Entropy { tag: T_ENT, .. })), "{:?}", self.dialog);
        }
        /// Recoge el diálogo del código de recuperación y devuelve el código.
        fn codigo(&mut self) -> String {
            match self.dialog.take() {
                Some(DialogRequest::Message { tag: T_CODE, title, text, buttons: Buttons::Ok }) => {
                    assert_eq!(title, "Código de recuperación");
                    assert!(text.ends_with("No se volverá a mostrar."));
                    let (code, resto) = text.split_once("\n\n").unwrap();
                    assert!(!code.contains(' ') && code.len() >= 40, "el código va solo en su línea");
                    assert!(resto.contains("Apúntalo en papel"));
                    String::from(code)
                }
                otro => panic!("se esperaba el código y hay {otro:?}"),
            }
        }
        /// Activa el cifrado total de principio a fin; devuelve el código de recuperación.
        fn activa_disco(&mut self, clave: &str) -> String {
            self.pool_listo();
            self.pulsa(Act::DiskEnc);
            assert!(self.texto_de(T_CONF).contains("copia de seguridad"));
            self.respuesta(T_CONF, Answer::Yes);
            pw_dialog(self, T_NEWK);
            self.texto(T_NEWK, clave);
            pw_dialog(self, T_REPK);
            self.texto(T_REPK, clave);
            self.entropia();
            self.respuesta(T_ENT, Answer::Ok);
            let code = self.codigo();
            assert!(self.app.job.as_ref().is_some_and(|j| j.old.0.is_empty() && j.new.0.is_empty()), "las claves ya se han soltado");
            self.respuesta(T_CODE, Answer::Ok);
            assert_eq!(self.mensaje(), "El disco se está cifrando. Puedes seguir usando el equipo.");
            assert!(self.app.job.is_none());
            code
        }
        fn termina_conversion(&mut self) {
            let mut n = 0;
            while !self.files().convert_step(40, AHORA).unwrap() {
                n += 1;
                assert!(n < 1000);
            }
        }
        /// Disco cifrado y con la conversión terminada.
        fn disco_cifrado(quien: &str) -> (Banco, Compartido, String) {
            let (mut b, d) = Banco::con_disco_compartido(quien, PAGE_DISK);
            let code = b.activa_disco(CLAVE_DISCO);
            b.termina_conversion();
            (b, d, code)
        }
        fn eventos(&self) -> u32 {
            self.sys.pool.events()
        }
        /// Lanza una acción sin pasar por el dibujo (botones que la pantalla aún no ofrece).
        fn lanza(&mut self, act: Act) {
            self.dialog = None;
            let mut cx = Ctx::new(&mut self.sys, &mut self.theme, 0);
            self.app.start(act, &mut cx);
            self.dialog = cx.dialog.take();
        }
    }

    /// Reinicio: vuelve a abrir el disco compartido.
    fn reabre(d: &Compartido) -> crate::files::Files {
        crate::files::Files::probe(Some(Box::new(d.clone())), AHORA)
    }

    #[test]
    fn la_pagina_disco_segun_quien_mira() {
        // Administradora: ve el botón de activar y nada más.
        let (mut b, _) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        assert!(b.hay(Act::DiskEnc) && !b.hay(Act::DiskDec) && !b.hay(Act::DiskChange) && !b.hay(Act::DiskCode));
        // Usuario normal: ve el estado pero ningún botón.
        let (mut b, _) = Banco::con_disco_compartido("Luis", PAGE_DISK);
        b.dibuja(780, 520);
        assert!(!b.app.hits.iter().any(|h| matches!(h.1, Hit::Act(_))));
        assert!(b.app.content_h > 0);
        // Sin disco: solo el aviso.
        let mut b = Banco::new();
        b.pagina(PAGE_DISK);
        b.dibuja(780, 520);
        assert!(!b.app.hits.iter().any(|h| matches!(h.1, Hit::Act(_))));
        b.clic(PANEL_W + 40, 100);
        assert!(b.dialog.is_none());
        // Tamaños extremos con el disco cifrándose.
        let (mut b, _) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        b.activa_disco(CLAVE_DISCO);
        for (w, h) in [(520, 360), (1, 1), (0, 0), (221, 400), (780, 520)] {
            b.dibuja(w, h);
        }
    }

    #[test]
    fn el_estado_del_disco_muestra_el_porcentaje_y_apaga_los_botones() {
        let (mut b, _) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        b.activa_disco(CLAVE_DISCO);
        for a in [Act::DiskEnc, Act::DiskDec, Act::DiskChange, Act::DiskCode] {
            assert!(!b.hay(a), "{a:?} debe estar desactivado mientras convierte");
        }
        let antes = b.dibuja(780, 520);
        b.files().convert_step(100, AHORA).unwrap();
        let (hechos, total, cifrando) = b.files().disk_conversion().unwrap();
        assert!(cifrando && hechos > 0 && hechos < total);
        // La pantalla cambia con el avance y el tick pide redibujar.
        assert!(b.dibuja(780, 520).px != antes.px);
        let mut cx = Ctx::new(&mut b.sys, &mut b.theme, 5000);
        b.app.tick(&mut cx);
        assert!(cx.redraw);
        drop(cx);
        b.termina_conversion();
        assert!(b.files().disk_conversion().is_none() && b.files().disk_encrypted());
        assert!(b.hay(Act::DiskDec) && b.hay(Act::DiskChange) && b.hay(Act::DiskCode) && !b.hay(Act::DiskEnc));
        // Descifrando también apaga los botones.
        b.pulsa(Act::DiskDec);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_SEC, CLAVE_DISCO);
        assert_eq!(b.mensaje(), "Se está descifrando el disco. Puedes seguir usando el equipo.");
        assert!(matches!(b.files().disk_conversion(), Some((_, _, false))));
        assert!(!b.hay(Act::DiskDec) && !b.hay(Act::DiskChange));
    }

    #[test]
    fn activar_el_cifrado_total_y_abrir_con_la_clave_o_con_el_codigo() {
        let (mut b, disco) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        b.files().write("C:\\Usuarios\\Ana\\Documentos\\nota.txt", b"hola", AHORA).unwrap();
        let code = b.activa_disco(CLAVE_DISCO);
        assert!(b.files().disk_encrypted());
        assert_eq!(b.eventos(), 300, "el acumulador se devuelve al sistema");
        b.termina_conversion();
        // Con el código de recuperación.
        let mut g = reabre(&disco);
        assert!(g.disk_locked());
        assert_eq!(g.unlock_disk("no-es-la-clave", false, AHORA), Err(keys::KeyError::WrongSecret));
        g.unlock_disk(&code, true, AHORA).unwrap();
        g.login("Ana", "").unwrap();
        assert_eq!(g.read("C:\\Usuarios\\Ana\\Documentos\\nota.txt").unwrap(), b"hola");
        // Y con la clave del disco.
        reabre(&disco).unlock_disk(CLAVE_DISCO, false, AHORA).unwrap();
        // Ni el código ni la clave están en claro en el disco.
        let raw = disco.0.borrow().clone();
        assert!(!raw.windows(code.len()).any(|w| w == code.as_bytes()));
        assert!(!raw.windows(CLAVE_DISCO.len()).any(|w| w == CLAVE_DISCO.as_bytes()));
        assert!(b.hay(Act::DiskDec) && !b.hay(Act::DiskEnc));
    }

    #[test]
    fn claves_del_disco_distintas_cortas_y_debiles() {
        let (mut b, _) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        b.pool_listo();
        // No coinciden.
        b.pulsa(Act::DiskEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_NEWK, CLAVE_DISCO);
        b.texto(T_REPK, NUEVA);
        assert_eq!(b.mensaje(), NO_MATCH);
        assert!(b.app.job.is_none() && !b.files().disk_encrypted());
        // Corta.
        b.pulsa(Act::DiskEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_NEWK, "abc");
        assert_eq!(b.mensaje(), keys::KeyError::Weak.text());
        assert!(b.app.job.is_none() && !b.files().disk_encrypted());
        // Débil y se dice que no: no cambia nada.
        b.pulsa(Act::DiskEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_NEWK, "abcdef");
        b.texto(T_REPK, "abcdef");
        assert_eq!(b.texto_de(T_WEAK), "La clave es débil. ¿Usarla de todos modos?");
        b.respuesta(T_WEAK, Answer::No);
        assert!(b.dialog.is_none() && b.app.job.is_none() && !b.files().disk_encrypted());
        // Débil y se dice que sí: sigue con la pantalla del ratón y la acepta.
        b.pulsa(Act::DiskEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_NEWK, "abcdef");
        b.texto(T_REPK, "abcdef");
        b.respuesta(T_WEAK, Answer::Yes);
        b.entropia();
        b.respuesta(T_ENT, Answer::Ok);
        b.codigo();
        assert!(b.files().disk_encrypted());
    }

    #[test]
    fn cancelar_en_cualquier_paso_no_cambia_nada_ni_deja_claves() {
        let (mut b, _) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        b.pool_listo();
        // Se cancela en el paso `n` de la activación (0 aviso, 1 clave, 2 repetir, 3 ratón).
        for n in 0..4 {
            b.pulsa(Act::DiskEnc);
            for paso in 0..=n {
                let cancela = paso == n;
                match paso {
                    0 if cancela => b.respuesta(T_CONF, Answer::Cancel),
                    0 => b.respuesta(T_CONF, Answer::Yes),
                    1 if cancela => b.responde(DialogResult::Text { tag: T_NEWK, value: None }),
                    1 => b.texto(T_NEWK, CLAVE_DISCO),
                    2 if cancela => b.responde(DialogResult::Text { tag: T_REPK, value: None }),
                    2 => b.texto(T_REPK, CLAVE_DISCO),
                    _ => b.respuesta(T_ENT, Answer::Cancel),
                }
            }
            assert!(b.dialog.is_none() && b.app.job.is_none(), "paso {n}");
            assert!(!b.files().disk_encrypted() && b.eventos() == 300);
        }
        // "No" en el aviso también lo deja todo igual.
        b.pulsa(Act::DiskEnc);
        b.respuesta(T_CONF, Answer::No);
        assert!(b.dialog.is_none() && b.app.job.is_none());
        // Cerrar la ventana a mitad: el trabajo (con su clave) se suelta con la app.
        b.pulsa(Act::DiskEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_NEWK, CLAVE_DISCO);
        assert!(b.app.job.as_ref().unwrap().new.same(&Secret(String::from(CLAVE_DISCO))));
        b.app.job = None;
        assert!(!b.files().disk_encrypted());
        // Con el disco ya cifrado: cancelar al quitar, cambiar y pedir código.
        let (mut b, _, _) = Banco::disco_cifrado("Ana");
        b.pulsa(Act::DiskDec);
        b.respuesta(T_CONF, Answer::Cancel);
        b.pulsa(Act::DiskDec);
        b.respuesta(T_CONF, Answer::Yes);
        b.responde(DialogResult::Text { tag: T_SEC, value: None });
        b.pulsa(Act::DiskChange);
        b.texto(T_SEC, CLAVE_DISCO);
        b.responde(DialogResult::Text { tag: T_NEWK, value: None });
        b.pulsa(Act::DiskCode);
        b.texto(T_SEC, CLAVE_DISCO);
        b.respuesta(T_ENT, Answer::Cancel);
        assert!(b.dialog.is_none() && b.app.job.is_none() && b.files().disk_encrypted() && b.files().disk_conversion().is_none());
    }

    #[test]
    fn la_app_no_guarda_la_clave_ni_el_codigo() {
        let (mut b, _) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        b.activa_disco(CLAVE_DISCO);
        assert!(b.app.job.is_none());
        // Un error tampoco deja claves: clave del disco mala al quitar el cifrado.
        b.termina_conversion();
        b.pulsa(Act::DiskDec);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_SEC, "clave-equivocada");
        assert_eq!(b.mensaje(), keys::KeyError::WrongSecret.text());
        assert!(b.app.job.is_none() && b.files().disk_conversion().is_none());
    }

    #[test]
    fn quitar_cambiar_clave_y_nuevo_codigo_del_disco() {
        let (mut b, disco, viejo) = Banco::disco_cifrado("Ana");
        // Cambiar la clave: la actual mala falla, la corta y la que no coincide también.
        b.pulsa(Act::DiskChange);
        b.texto(T_SEC, "clave-equivocada");
        b.texto(T_NEWK, NUEVA);
        b.texto(T_REPK, NUEVA);
        assert_eq!(b.mensaje(), keys::KeyError::WrongSecret.text());
        b.pulsa(Act::DiskChange);
        b.texto(T_SEC, CLAVE_DISCO);
        b.texto(T_NEWK, "corta");
        assert_eq!(b.mensaje(), keys::KeyError::Weak.text());
        b.pulsa(Act::DiskChange);
        b.texto(T_SEC, CLAVE_DISCO);
        b.texto(T_NEWK, NUEVA);
        b.texto(T_REPK, "otra-clave-distinta");
        assert_eq!(b.mensaje(), NO_MATCH);
        // Buena: la vieja deja de abrir.
        b.pulsa(Act::DiskChange);
        b.texto(T_SEC, CLAVE_DISCO);
        b.texto(T_NEWK, NUEVA);
        b.texto(T_REPK, NUEVA);
        assert_eq!(b.mensaje(), "Se ha cambiado la clave del disco.");
        let mut g = reabre(&disco);
        assert_eq!(g.unlock_disk(CLAVE_DISCO, false, AHORA), Err(keys::KeyError::WrongSecret));
        g.unlock_disk(NUEVA, false, AHORA).unwrap();
        // Nuevo código: el anterior deja de valer y el nuevo abre.
        b.pulsa(Act::DiskCode);
        b.texto(T_SEC, NUEVA);
        b.entropia();
        b.respuesta(T_ENT, Answer::Ok);
        let nuevo = b.codigo();
        b.respuesta(T_CODE, Answer::Ok);
        assert!(b.dialog.is_none(), "un código de reemplazo no lleva mensaje de cierre");
        assert_ne!(nuevo, viejo);
        let mut h = reabre(&disco);
        assert_eq!(h.unlock_disk(&viejo, true, AHORA), Err(keys::KeyError::WrongSecret));
        h.unlock_disk(&nuevo, true, AHORA).unwrap();
        // Código con la clave mala.
        b.pulsa(Act::DiskCode);
        b.texto(T_SEC, "clave-equivocada");
        b.respuesta(T_ENT, Answer::Ok);
        assert_eq!(b.mensaje(), keys::KeyError::WrongSecret.text());
        // Quitar el cifrado.
        b.pulsa(Act::DiskDec);
        assert!(b.texto_de(T_CONF).contains("descifrará todo el disco"));
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_SEC, NUEVA);
        assert_eq!(b.mensaje(), "Se está descifrando el disco. Puedes seguir usando el equipo.");
        b.termina_conversion();
        assert!(!b.files().disk_encrypted() && !reabre(&disco).disk_locked());
        assert!(b.hay(Act::DiskEnc));
    }

    #[test]
    fn errores_del_disco_no_admin_y_acumulador_sin_llenar() {
        // Un usuario normal con la pantalla de administrador todavía abierta.
        let (mut b, _) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        b.dibuja(780, 520);
        b.files().login("Luis", "").unwrap();
        b.pool_listo();
        b.pulsa_viejo(Act::DiskEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_NEWK, CLAVE_DISCO);
        b.texto(T_REPK, CLAVE_DISCO);
        b.respuesta(T_ENT, Answer::Ok);
        assert_eq!(b.mensaje(), keys::KeyError::NotAdmin.text());
        assert!(b.app.job.is_none() && b.eventos() == 300);
        // Aleatoriedad insuficiente: se avisa y no se cifra; el acumulador vuelve.
        let (mut b, _) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        b.pulsa(Act::DiskEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_NEWK, CLAVE_DISCO);
        b.texto(T_REPK, CLAVE_DISCO);
        b.respuesta(T_ENT, Answer::Ok);
        assert_eq!(b.mensaje(), keys::KeyError::NotReady.text());
        assert!(!b.files().disk_encrypted() && b.app.job.is_none());
        // Cambiar la clave con la pantalla vieja de un usuario normal.
        let (mut b, _, _) = Banco::disco_cifrado("Ana");
        b.dibuja(780, 520);
        b.files().login("Luis", "").unwrap();
        b.pulsa_viejo(Act::DiskChange);
        b.texto(T_SEC, CLAVE_DISCO);
        b.texto(T_NEWK, NUEVA);
        b.texto(T_REPK, NUEVA);
        assert_eq!(b.mensaje(), keys::KeyError::NotAdmin.text());
    }

    #[test]
    fn mi_cuenta_botones_de_cifrado_segun_la_clave() {
        let mut b = Banco::con_disco("Luis");
        // Sin clave no se ofrece cifrar.
        assert!(!b.hay(Act::HomeEnc) && !b.hay(Act::HomeDec) && !b.hay(Act::HomeCode));
        b.files().set_password("", Some("clave-luis"), [3; 16], AHORA).unwrap();
        assert!(b.hay(Act::HomeEnc) && !b.hay(Act::HomeDec) && !b.hay(Act::HomeCode));
        for (w, h) in [(520, 360), (1, 1), (221, 400)] {
            b.dibuja(w, h);
        }
    }

    /// Resultado de pedir cifrar/renovar con la clave buena: el código o el error de la llamada.
    fn resultado_de_la_llamada(b: &mut Banco) {
        match b.dialog.take() {
            Some(DialogRequest::Message { tag: T_CODE, .. }) => {}
            Some(DialogRequest::Message { tag: T_MSG, text, .. }) => assert!(
                [keys::KeyError::Unavailable, keys::KeyError::NotEncrypted].iter().any(|e| e.text() == text),
                "{text}"
            ),
            otro => panic!("{otro:?}"),
        }
    }

    #[test]
    fn mi_cuenta_cifrar_pide_aviso_clave_y_raton_y_muestra_el_resultado() {
        let mut b = Banco::con_disco("Luis");
        b.files().set_password("", Some("clave-luis"), [3; 16], AHORA).unwrap();
        b.pool_listo();
        b.pulsa(Act::HomeEnc);
        assert!(b.texto_de(T_CONF).contains("Si olvidas la clave y pierdes el código de recuperación, no se podrán recuperar. ¿Continuar?"));
        b.respuesta(T_CONF, Answer::Yes);
        pw_dialog(&b, T_SEC);
        // Clave mala: se corta antes de la pantalla del ratón.
        b.texto(T_SEC, "mala-clave");
        assert_eq!(b.mensaje(), AccountError::WrongPassword.text());
        assert!(b.app.job.is_none());
        // Clave buena: pantalla del ratón y la llamada; la pantalla muestra lo que ella devuelva.
        b.pulsa(Act::HomeEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_SEC, "clave-luis");
        b.entropia();
        b.respuesta(T_ENT, Answer::Ok);
        resultado_de_la_llamada(&mut b);
        assert!(b.app.job.as_ref().map_or(true, |j| j.old.0.is_empty() && j.new.0.is_empty()));
        assert_eq!(b.eventos(), 300, "el acumulador vuelve al sistema también si la llamada falla");
        // Cancelar en cada paso no cambia nada.
        b.app.job = None;
        b.pulsa(Act::HomeEnc);
        b.respuesta(T_CONF, Answer::Cancel);
        b.pulsa(Act::HomeEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.responde(DialogResult::Text { tag: T_SEC, value: None });
        b.pulsa(Act::HomeEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_SEC, "clave-luis");
        b.respuesta(T_ENT, Answer::Cancel);
        assert!(b.dialog.is_none() && b.app.job.is_none());
    }

    #[test]
    fn mi_cuenta_quitar_cifrado_y_nuevo_codigo_muestran_el_resultado_de_la_llamada() {
        let mut b = Banco::con_disco("Luis");
        b.files().set_password("", Some("clave-luis"), [3; 16], AHORA).unwrap();
        b.pool_listo();
        // Quitar el cifrado: con clave mala se corta; con la buena sale lo que devuelva la llamada.
        b.lanza(Act::HomeDec);
        assert!(b.texto_de(T_CONF).contains("se descifrará"));
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_SEC, "mala-clave");
        assert_eq!(b.mensaje(), AccountError::WrongPassword.text());
        b.lanza(Act::HomeDec);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_SEC, "clave-luis");
        if !b.files().home_encrypted() {
            assert_eq!(b.mensaje(), keys::KeyError::NotEncrypted.text());
        }
        // Nuevo código.
        b.app.job = None;
        b.lanza(Act::HomeCode);
        b.texto(T_SEC, "clave-luis");
        b.entropia();
        b.respuesta(T_ENT, Answer::Ok);
        resultado_de_la_llamada(&mut b);
        assert_eq!(b.eventos(), 300);
    }

    #[test]
    fn mi_cuenta_tras_poner_la_clave_ofrece_cifrar_la_carpeta() {
        let mut b = Banco::con_disco("Ana");
        b.pool_listo();
        // Sí: sigue con la pantalla del ratón sin volver a pedir la clave.
        b.pulsa(Act::SetPwd);
        b.texto(T_PW_NEW, "secreta1");
        b.texto(T_PW_REP, "secreta1");
        assert_eq!(b.texto_de(T_ASK_HOME), "Se ha puesto la clave.\n¿Cifrar también tu carpeta? (Recomendado)");
        assert!(matches!(&b.dialog, Some(DialogRequest::Message { buttons: Buttons::YesNo, .. })));
        b.respuesta(T_ASK_HOME, Answer::Yes);
        b.entropia();
        b.respuesta(T_ENT, Answer::Ok);
        assert!(b.files().verify_password("secreta1"));
        resultado_de_la_llamada(&mut b);
        assert!(b.app.job.as_ref().map_or(true, |j| j.old.0.is_empty() && j.new.0.is_empty()));
        if b.files().home_encrypted() {
            return; // con el cifrado real la clave ya no se puede quitar: lo cubre la prueba de integración
        }
        // Quitar la clave y volver a ponerla; esta vez se cancela la pantalla del ratón: la clave queda puesta.
        b.app.job = None;
        b.pulsa(Act::RemovePwd);
        b.texto(T_PW_OLD, "secreta1");
        assert_eq!(b.mensaje(), "Se ha quitado la clave.");
        b.pulsa(Act::SetPwd);
        b.texto(T_PW_NEW, "secreta2");
        b.texto(T_PW_REP, "secreta2");
        b.respuesta(T_ASK_HOME, Answer::Yes);
        b.respuesta(T_ENT, Answer::Cancel);
        assert!(b.dialog.is_none() && b.app.job.is_none() && b.files().verify_password("secreta2"));
    }

    #[test]
    #[ignore = "necesita el cifrado de carpeta de files.rs"]
    fn integracion_real_cifrar_la_carpeta_desde_mi_cuenta() {
        let mut b = Banco::con_disco("Ana");
        b.pool_listo();
        b.files().write("C:\\Usuarios\\Ana\\Documentos\\nota.txt", b"hola", AHORA).unwrap();
        // Poner la clave y aceptar cifrar la carpeta.
        b.pulsa(Act::SetPwd);
        b.texto(T_PW_NEW, "secreta1");
        b.texto(T_PW_REP, "secreta1");
        b.respuesta(T_ASK_HOME, Answer::Yes);
        b.respuesta(T_ENT, Answer::Ok);
        let code = b.codigo();
        b.respuesta(T_CODE, Answer::Ok);
        assert_eq!(b.mensaje(), "Se está cifrando tu carpeta. Puedes seguir usando el equipo.");
        assert!(b.files().home_encrypted());
        // Mientras convierte, los botones están desactivados.
        if b.files().home_conversion().is_some() {
            assert!(!b.hay(Act::HomeDec) && !b.hay(Act::HomeCode));
        }
        while !b.files().home_convert_step(10, AHORA).unwrap() {}
        assert_eq!(b.files().read("C:\\Usuarios\\Ana\\Documentos\\nota.txt").unwrap(), b"hola");
        assert!(b.hay(Act::HomeDec) && b.hay(Act::HomeCode) && !b.hay(Act::HomeEnc));
        // Con la carpeta cifrada no se puede quitar la clave.
        b.pulsa(Act::RemovePwd);
        assert_eq!(b.mensaje(), AccountError::Encrypted.text());
        // Nuevo código de recuperación: cambia.
        b.pulsa(Act::HomeCode);
        b.texto(T_SEC, "secreta1");
        b.respuesta(T_ENT, Answer::Ok);
        let nuevo = b.codigo();
        assert_ne!(code, nuevo);
        b.respuesta(T_CODE, Answer::Ok);
        // Quitar el cifrado.
        b.pulsa(Act::HomeDec);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_SEC, "secreta1");
        assert_eq!(b.mensaje(), "Se está descifrando tu carpeta.");
        while !b.files().home_convert_step(10, AHORA).unwrap() {}
        assert!(!b.files().home_encrypted() && b.hay(Act::HomeEnc));
    }
}
