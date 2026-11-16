//! Escritorio y gestor de ventanas. CONTRATO: las firmas de `new`, `event`, `render` y `back`
//! no cambian (las usa el núcleo).
//!
//! `back` es la imagen final: todo se compone ahí, incluido el puntero. Cada cambio añade una zona
//! a `damage`; `render` redibuja lo sucio, normaliza las zonas y recompone solo dentro de cada una.
use crate::apps::{self, App, AppKind, Ctx, MouseEv, APPS};
use crate::dialogs::{self, Answer, Buttons, Dialog, DialogRequest, DialogResult};
use crate::files::{self, Std};
use crate::fs::FsError;
use crate::font::{self, Align};
use crate::geom::Rect;
use crate::icons::{self, Icon};
use crate::input::{Event, Key, KeyEvent};
use crate::login::{Login, Mode, Out};
use crate::menu::{Menu, MenuAct, MenuItem};
use crate::platform::{DateTime, Platform};
use crate::surface::{blend, lighten, Surface};
use crate::theme::{self, Theme, WALLPAPERS};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

const BAR_H: i32 = 40;
const TITLE_H: i32 = 30;
const BAR_BG: u32 = 0x151515;
const HOVER_BAR: u32 = 0x2D2D2D;
const ACTIVE_BTN: u32 = 0x3A3A3A;
const MENU_BG: u32 = 0x1F1F1F;
const DOUBLE_MS: u64 = 450;

/// Flecha propia: '#' contorno, 'o' relleno. El punto activo es la esquina superior izquierda.
const CURSOR: [&str; 19] = [
    "#...........",
    "##..........",
    "#o#.........",
    "#oo#........",
    "#ooo#.......",
    "#oooo#......",
    "#ooooo#.....",
    "#oooooo#....",
    "#ooooooo#...",
    "#oooooooo#..",
    "#ooooooooo#.",
    "#oooooo#####",
    "#ooo#oo#....",
    "#oo#.#oo#...",
    "#o#..#oo#...",
    "##....#oo#..",
    "......#oo#..",
    ".......##...",
    "............",
];
/// Zona que puede ocupar el puntero en (x, y), sea la flecha o una flecha doble de redimensionado.
fn cursor_rect(x: i32, y: i32) -> Rect {
    Rect::new(x - 11, y - 11, 24, 30)
}

/// Puntero para los bordes agarrados `edge` (bit 0 izquierdo, 1 derecho, 2 superior, 3 inferior):
/// 1 horizontal, 2 vertical, 3 diagonal de arriba-izquierda a abajo-derecha, 4 la otra diagonal.
fn resize_cursor(edge: u8) -> u8 {
    match (edge & 3 != 0, edge & 12 != 0) {
        (true, true) => {
            if (edge & 1 != 0) == (edge & 4 != 0) {
                3
            } else {
                4
            }
        }
        (true, false) => 1,
        _ => 2,
    }
}

/// Qué hay bajo el puntero.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hit {
    None,
    Icon(usize),
    Start,
    Task(u32),
    Show,
    Bar,
    Row(usize),
    Tile(usize),
    MSettings,
    MPower,
    MenuBg,
    Shut(usize),
    Cap(u32, u8),
    Title(u32),
    /// Bordes agarrables: bit 0 izquierdo, 1 derecho, 2 superior, 3 inferior.
    Edge(u32, u8),
    Content(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Drag {
    None,
    Move { id: u32, ox: i32, oy: i32 },
    Resize { id: u32, edge: u8, start: Rect, px: i32, py: i32 },
    Cap(u32, u8),
    Content(u32),
}

struct Window {
    id: u32,
    kind: AppKind,
    app: Box<dyn App>,
    title: String,
    /// Marco exterior (borde incluido).
    rect: Rect,
    restore: Rect,
    content: Surface,
    dirty: bool,
    min: bool,
    max: bool,
    /// Si es un cuadro de diálogo: su contenido (la app es un relleno sin uso).
    dlg: Option<Dialog>,
    /// Ventana a la que está ligado el diálogo (`None` = diálogo del sistema).
    owner: Option<u32>,
}

/// App de relleno para las ventanas de diálogo.
struct NoApp(AppKind);

impl App for NoApp {
    fn kind(&self) -> AppKind {
        self.0
    }
    fn title(&self) -> String {
        String::new()
    }
    fn min_size(&self) -> (i32, i32) {
        (0, 0)
    }
    fn draw(&mut self, _s: &mut Surface, _theme: &Theme, _sys: &mut dyn Platform, _focused: bool) {}
    fn key(&mut self, _ev: &KeyEvent, _cx: &mut Ctx) {}
    fn mouse(&mut self, _ev: MouseEv, _cx: &mut Ctx) {}
    fn tick(&mut self, _cx: &mut Ctx) {}
}

/// Qué hace un icono del escritorio al abrirse.
enum Act {
    App(AppKind),
    Path(String),
}

struct DIcon {
    name: String,
    /// Lo que se escribe bajo el icono (sin extensión si el usuario la oculta).
    label: String,
    icon: Icon,
    act: Act,
}

/// Etiquetas de los diálogos del sistema (sin dueña).
const TAG_FORMAT: u32 = 0xFFFF_FF01;
const TAG_SYS: u32 = 0xFFFF_FF02;
/// Diálogos de los menús del escritorio; la ruta afectada va en `Desktop::pending`.
const TAG_RENAME: u32 = 0xFFFF_FF10;
const TAG_NEWDIR: u32 = 0xFFFF_FF11;
const TAG_NEWDOC: u32 = 0xFFFF_FF12;
const TAG_EMPTY: u32 = 0xFFFF_FF13;
/// Mensajes de información o error sin respuesta que tratar.
const TAG_INFO: u32 = 0xFFFF_FF14;

/// Etiquetas e identificadores de los menús contextuales del escritorio.
const MENU_ICON: u32 = 1;
const MENU_DESK: u32 = 2;
const M_OPEN: u32 = 1;
const M_NEWWIN: u32 = 2;
const M_NOTEPAD: u32 = 3;
const M_CUT: u32 = 4;
const M_COPY: u32 = 5;
const M_PASTE_IN: u32 = 6;
const M_DELETE: u32 = 7;
const M_RENAME: u32 = 8;
const M_PROPS: u32 = 9;
const M_EMPTY: u32 = 10;
/// Tamaños de icono: M_VIEW + 0..3. Orden: M_SORT + 0..4.
const M_VIEW: u32 = 20;
const M_SORT: u32 = 30;
const M_REFRESH: u32 = 40;
const M_PASTE: u32 = 41;
const M_UNDO: u32 = 42;
const M_NEWDIR: u32 = 43;
const M_NEWDOC: u32 = 44;
const M_CUSTOM: u32 = 45;

/// Tamaño de los iconos del escritorio.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum IconSize {
    Big,
    Mid,
    Small,
}

impl IconSize {
    const ALL: [(IconSize, &'static str, &'static str); 3] =
        [(IconSize::Big, "grande", "Iconos grandes"), (IconSize::Mid, "mediano", "Iconos medianos"), (IconSize::Small, "pequeno", "Iconos pequeños")];

    /// (ancho de celda, alto de celda, lado del icono, y del icono, y del texto).
    fn metrics(self) -> (i32, i32, i32, i32, i32) {
        match self {
            IconSize::Big => (100, 100, 64, 6, 74),
            IconSize::Mid => (84, 84, 40, 8, 52),
            IconSize::Small => (76, 62, 32, 4, 38),
        }
    }
}

/// Criterio de orden de los archivos del escritorio.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Order {
    Name,
    Size,
    Kind,
    Date,
}

impl Order {
    const ALL: [(Order, &'static str, &'static str); 4] =
        [(Order::Name, "nombre", "Nombre"), (Order::Size, "tamano", "Tamaño"), (Order::Kind, "tipo", "Tipo"), (Order::Date, "fecha", "Fecha de modificación")];
}

impl Window {
    fn bw(&self) -> i32 {
        if self.max {
            0
        } else {
            1
        }
    }
    fn title_rect(&self) -> Rect {
        let b = self.bw();
        Rect::new(self.rect.x + b, self.rect.y + b, self.rect.w - 2 * b, TITLE_H)
    }
    fn content_rect(&self) -> Rect {
        let b = self.bw();
        Rect::new(self.rect.x + b, self.rect.y + b + TITLE_H, self.rect.w - 2 * b, self.rect.h - 2 * b - TITLE_H)
    }
    /// Botón de la barra de título: 0 minimizar, 1 maximizar, 2 cerrar.
    fn cap(&self, k: i32) -> Rect {
        let t = self.title_rect();
        Rect::new(t.right() - 138 + 46 * k, t.y, 46, TITLE_H)
    }
    /// Zona que ocupa en pantalla, sombra incluida.
    fn bounds(&self) -> Rect {
        if self.max {
            self.rect
        } else {
            self.rect.inflate(10)
        }
    }
}

/// Menú contextual abierto: lo pide una ventana (`owner = Some(id)`) o el propio escritorio (`None`).
struct OpenMenu {
    menu: Menu,
    owner: Option<u32>,
    tag: u32,
}

pub struct Desktop {
    w: i32,
    h: i32,
    back: Surface,
    wall: Surface,
    theme: Theme,
    mx: i32,
    my: i32,
    btn: bool,
    damage: Vec<Rect>,
    /// De atrás hacia delante.
    wins: Vec<Window>,
    launched: u32,
    start_open: bool,
    shut_open: bool,
    sel: Option<usize>,
    hot: Hit,
    drag: Drag,
    chover: Option<u32>,
    last_click: Option<(u64, i32, i32, Hit)>,
    now_ms: u64,
    next_clock: u64,
    clock: DateTime,
    last_active: Option<u32>,
    /// Forma del puntero: 0 flecha; 1..=4 flechas dobles de redimensionado (ver `resize_cursor`).
    cur: u8,
    /// Iconos del escritorio: apps, Papelera y lo que haya en `C:\Escritorio`.
    icons: Vec<DIcon>,
    files_gen: u64,
    /// ¿Se muestran las extensiones? Se lee al reconstruir los iconos, no al dibujar.
    show_ext: bool,
    /// Última combinación (ctrl, mayús) entregada a una app, y a qué ventana.
    mods_sent: (bool, bool),
    mods_to: Option<u32>,
    /// Estado actual (ctrl, mayús) del teclado.
    mods: (bool, bool),
    /// ¿Ya se hizo el flujo de arranque del disco (primer tick)?
    booted: bool,
    /// Menú contextual abierto: recibe todo el teclado y el ratón.
    menu: Option<OpenMenu>,
    /// Estado del botón derecho.
    rbtn: bool,
    size: IconSize,
    order: Order,
    /// Ruta afectada por el diálogo del sistema que está abierto (cambiar nombre).
    pending: Option<String>,
    /// Pantallas de cuentas (asistente, inicio de sesión, bloqueo) y el fondo oscurecido que llevan.
    login: Login,
    dim: Surface,
    /// Hay disco con cuentas: el menú de apagado ofrece Bloquear y Cerrar sesión.
    can_session: bool,
    /// Cerrando sesión: se pide cerrar cada ventana, una a una; espera la respuesta de esta.
    logging_out: bool,
    logout_wait: Option<u32>,
}

fn shadow(s: &mut Surface, r: Rect) {
    let (x, y) = (r.x, r.y + 2);
    for i in 1..=8i32 {
        let a = (70 * (9 - i) * (9 - i) / 64) as u32;
        let (rx, ry, rw, rh) = (x - i, y - i, r.w + 2 * i, r.h + 2 * i);
        s.blend_rect(Rect::new(rx, ry, rw, 1), 0, a);
        s.blend_rect(Rect::new(rx, ry + rh - 1, rw, 1), 0, a);
        s.blend_rect(Rect::new(rx, ry + 1, 1, rh - 2), 0, a);
        s.blend_rect(Rect::new(rx + rw - 1, ry + 1, 1, rh - 2), 0, a);
    }
}

/// Dibuja con el recorte limitado a `r` (para no desbordar textos).
fn text_in(s: &mut Surface, r: Rect, text: &str, size: u32, color: u32, align: Align) {
    let old = s.clip();
    s.set_clip(old.intersect(r));
    font::draw_in(s, r, text, size, false, color, align);
    s.set_clip(old);
}

/// Recorta a pantalla, fusiona zonas que se tocan y, si quedan más de 6, las une en una.
fn normalize(list: Vec<Rect>, screen: Rect) -> Vec<Rect> {
    let mut v: Vec<Rect> = list.into_iter().map(|r| r.intersect(screen)).filter(|r| !r.is_empty()).collect();
    loop {
        let mut pair = None;
        'f: for a in 0..v.len() {
            for b in a + 1..v.len() {
                if v[a].inflate(1).intersects(v[b]) {
                    pair = Some((a, b));
                    break 'f;
                }
            }
        }
        match pair {
            Some((a, b)) => {
                v[a] = v[a].union(v[b]);
                v.swap_remove(b);
            }
            None => break,
        }
    }
    if v.len() > 6 {
        let u = v.iter().fold(Rect::default(), |u, r| u.union(*r));
        v = vec![u];
    }
    v
}

/// Aplica `fondo=`, `enfasis=`, `iconos=` y `orden=` de `C:\Sistema\config.txt` si son válidos.
fn load_config(sys: &mut dyn Platform, theme: &mut Theme, size: &mut IconSize, order: &mut Order) {
    let cfg = sys.files().config_file();
    let Ok(bytes) = sys.files().read(&cfg) else { return };
    let Ok(text) = core::str::from_utf8(&bytes) else { return };
    for line in text.lines() {
        match line.trim().split_once('=') {
            Some(("fondo", v)) => {
                if let Ok(n) = v.trim().parse::<usize>() {
                    if n < WALLPAPERS.len() {
                        theme.wallpaper = n as u8;
                    }
                }
            }
            Some(("enfasis", v)) => {
                let v = v.trim();
                if v.len() == 6 {
                    if let Ok(c) = u32::from_str_radix(v, 16) {
                        theme.accent = c;
                    }
                }
            }
            Some(("iconos", v)) => {
                if let Some(e) = IconSize::ALL.iter().find(|e| e.1 == v.trim()) {
                    *size = e.0;
                }
            }
            Some(("orden", v)) => {
                if let Some(e) = Order::ALL.iter().find(|e| e.1 == v.trim()) {
                    *order = e.0;
                }
            }
            _ => {}
        }
    }
}

/// Guarda fondo, énfasis, tamaño de icono y orden; respeta las demás líneas (otras apps guardan
/// ahí sus cosas). Los errores se ignoran.
fn save_config(sys: &mut dyn Platform, theme: &Theme, size: IconSize, order: Order) {
    let now = sys.now();
    let f = sys.files();
    let cfg = f.config_file();
    if let Some(dir) = files::parent(&cfg) {
        let _ = f.mkdir(&dir, now);
    }
    let old = f.read(&cfg).ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
    let mut text = format!(
        "fondo={}\nenfasis={:06X}\niconos={}\norden={}\n",
        theme.wallpaper,
        theme.accent & 0xFFFFFF,
        IconSize::ALL.iter().find(|e| e.0 == size).map_or("mediano", |e| e.1),
        Order::ALL.iter().find(|e| e.0 == order).map_or("nombre", |e| e.1)
    );
    for line in old.lines() {
        let ours = matches!(line.split_once('=').map(|p| p.0.trim()), Some("fondo" | "enfasis" | "iconos" | "orden"));
        if !ours && !line.trim().is_empty() {
            text.push_str(line);
            text.push('\n');
        }
    }
    let _ = f.write(&cfg, text.as_bytes(), now);
}

/// (título, texto) del cuadro "Propiedades de <nombre>".
fn props_text(sys: &mut dyn Platform, path: &str) -> Result<(String, String), FsError> {
    let e = sys.files().stat(path)?;
    let kind = if e.dir {
        "Carpeta de archivos"
    } else if e.name.to_lowercase().ends_with(".txt") {
        "Documento de texto"
    } else {
        "Archivo"
    };
    let size = if e.dir {
        let n = sys.files().list(path).map_or(0, |v| v.len());
        format!("Contiene: {n} {}", if n == 1 { "elemento" } else { "elementos" })
    } else {
        format!("Tamaño: {}", size_text(e.size))
    };
    let m = e.modified;
    let text = format!(
        "Nombre: {}\nTipo: {kind}\nUbicación: {}\n{size}\nModificado: {:02}/{:02}/{:04} {:02}:{:02}",
        e.name,
        files::parent(path).unwrap_or_default(),
        m.day,
        m.month,
        m.year,
        m.hour,
        m.minute
    );
    Ok((format!("Propiedades de {}", e.name), text))
}

fn size_text(b: u64) -> String {
    let human = if b < 1024 {
        return format!("{b} bytes");
    } else if b < 1 << 20 {
        format!("{} KB", b.div_ceil(1024))
    } else if b < 1 << 30 {
        format!("{}.{} MB", b >> 20, (b & 0xF_FFFF) * 10 >> 20)
    } else {
        format!("{}.{} GB", b >> 30, (b & 0x3FFF_FFFF) * 10 >> 30)
    };
    format!("{human} ({b} bytes)")
}

fn date_key(d: DateTime) -> (u16, u8, u8, u8, u8, u8) {
    (d.year, d.month, d.day, d.hour, d.minute, d.second)
}

fn ext(name: &str) -> String {
    name.rsplit_once('.').map_or(String::new(), |(_, e)| e.to_lowercase())
}

/// Apps, Papelera y el contenido de `C:\Escritorio` (carpetas antes que archivos, según `order`).
fn build_icons(sys: &mut dyn Platform, order: Order, show_ext: bool) -> Vec<DIcon> {
    let mut v: Vec<DIcon> = APPS.iter().map(|(k, n, i)| DIcon { name: String::from(*n), label: String::from(*n), icon: *i, act: Act::App(*k) }).collect();
    v.push(DIcon { name: String::from("Papelera"), label: String::from("Papelera"), icon: Icon::Trash, act: Act::Path(String::from(files::TRASH)) });
    let dir = sys.files().std_dir(Std::Desktop);
    let mut list = sys.files().list(&dir).unwrap_or_default();
    list.sort_by(|a, b| {
        let by_name = a.name.to_lowercase().cmp(&b.name.to_lowercase());
        b.dir.cmp(&a.dir).then_with(|| match order {
            Order::Name => by_name,
            Order::Size => a.size.cmp(&b.size).then(by_name),
            Order::Kind => ext(&a.name).cmp(&ext(&b.name)).then(by_name),
            // Lo más reciente primero.
            Order::Date => date_key(b.modified).cmp(&date_key(a.modified)).then(by_name),
        })
    });
    for e in list {
        let icon = if e.dir {
            Icon::Folder
        } else if e.name.to_lowercase().ends_with(".txt") {
            Icon::FileText
        } else {
            Icon::FileBlank
        };
        let label = String::from(files::display_name(&e.name, e.dir, show_ext));
        v.push(DIcon { act: Act::Path(files::join(&dir, &e.name)), label, name: e.name, icon });
    }
    v
}

impl Desktop {
    pub fn new(w: i32, h: i32, sys: &mut dyn Platform) -> Self {
        let theme = Theme::default();
        let mut d = Desktop {
            w,
            h,
            back: Surface::new(w, h, 0),
            wall: theme::render_wallpaper(theme.wallpaper, w, h, theme.accent),
            theme,
            mx: w / 2,
            my: h / 2,
            btn: false,
            damage: vec![Rect::new(0, 0, w, h)],
            wins: Vec::new(),
            launched: 0,
            start_open: false,
            shut_open: false,
            sel: None,
            hot: Hit::None,
            drag: Drag::None,
            chover: None,
            last_click: None,
            now_ms: 0,
            next_clock: 0,
            clock: sys.now(),
            last_active: None,
            cur: 0,
            icons: Vec::new(),
            size: IconSize::Mid,
            order: Order::Name,
            pending: None,
            files_gen: sys.files().generation(),
            show_ext: true,
            mods_sent: (false, false),
            mods_to: None,
            mods: (false, false),
            booted: false,
            menu: None,
            rbtn: false,
            login: Login::new(w, h),
            dim: Surface::empty(),
            can_session: sys.files().persistent(),
            logging_out: false,
            logout_wait: None,
        };
        d.login.set_clock(d.clock);
        if sys.files().user().is_some() {
            d.load_session(sys);
        } else {
            // Disco con cuentas: hasta el primer tick (`boot_flow`) solo se ve el fondo oscuro.
            d.login.start_boot();
            d.screen_on();
        }
        d
    }

    /// Lee todo lo de la sesión que acaba de empezar: configuración, extensiones e iconos.
    fn load_session(&mut self, sys: &mut dyn Platform) {
        self.theme = Theme::default();
        self.size = IconSize::Mid;
        self.order = Order::Name;
        load_config(sys, &mut self.theme, &mut self.size, &mut self.order);
        self.wall = theme::render_wallpaper(self.theme.wallpaper, self.w, self.h, self.theme.accent);
        self.show_ext = sys.files().show_ext();
        self.icons = build_icons(sys, self.order, self.show_ext);
        self.files_gen = sys.files().generation();
        self.can_session = sys.files().persistent();
        self.sel = None;
        self.dmg_all();
    }

    /// Prepara una pantalla de cuentas: nada del escritorio queda activo y el fondo se oscurece.
    fn screen_on(&mut self) {
        self.set_start(false);
        self.set_shut(false);
        self.close_menu();
        self.drag = Drag::None;
        self.chover = None;
        self.sel = None;
        let mut dim = Surface::new(self.w, self.h, 0);
        dim.blit(&self.wall, 0, 0);
        dim.blend_rect(Rect::new(0, 0, self.w, self.h), 0x000000, 150);
        self.dim = dim;
        self.dmg_all();
    }

    fn flush_login(&mut self) {
        for r in self.login.take_damage() {
            self.dmg(r);
        }
    }

    /// Pantalla actual de cuentas (para pruebas y capturas).
    pub fn login_mode(&self) -> Mode {
        self.login.mode
    }

    fn open_session(&mut self, sys: &mut dyn Platform) {
        self.login.off();
        self.load_session(sys);
        self.flush_login();
    }

    /// Orden del arranque: aviso/diálogo de disco, asistente si hace falta, inicio de sesión, escritorio.
    fn enter_flow(&mut self, sys: &mut dyn Platform) {
        if sys.files().user().is_some() {
            if self.login.mode != Mode::Off {
                self.open_session(sys);
            }
            return;
        }
        if sys.files().needs_setup() {
            self.login.start_setup();
            self.screen_on();
            return;
        }
        let list = sys.files().accounts();
        if let [a] = list.as_slice() {
            if !a.has_password() && sys.files().login(&a.name, "").is_ok() {
                self.open_session(sys);
                return;
            }
        }
        self.show_pick(sys);
    }

    fn show_pick(&mut self, sys: &mut dyn Platform) {
        let users = sys.files().accounts().iter().map(|a| (a.name.clone(), a.has_password())).collect();
        self.login.start_pick(users);
        self.screen_on();
    }

    fn login_out(&mut self, out: Out, sys: &mut dyn Platform) {
        match out {
            Out::None => {}
            Out::Entered => self.open_session(sys),
            Out::Unlocked => {
                self.login.off();
                for w in self.wins.iter_mut() {
                    w.dirty = true;
                }
                self.files_gen = 0;
                self.dmg_all();
            }
            Out::SetupDone => self.enter_flow(sys),
            Out::Shutdown => sys.shutdown(),
            Out::Reboot => sys.reboot(),
        }
        self.flush_login();
    }

    /// Bloquea: las ventanas siguen abiertas pero no se ven ni reciben nada.
    fn lock(&mut self, sys: &mut dyn Platform) {
        let Some(name) = sys.files().user().map(String::from) else { return };
        let has = sys.files().has_password();
        self.login.start_lock(&name, has);
        self.screen_on();
        self.flush_login();
        self.sync_mods(sys);
    }

    /// Cierra la sesión: pide cerrar cada ventana (una a una; si alguna se niega, se cancela).
    fn begin_logout(&mut self, sys: &mut dyn Platform) {
        self.set_start(false);
        self.set_shut(false);
        self.logging_out = true;
        self.logout_step(sys);
    }

    fn logout_step(&mut self, sys: &mut dyn Platform) {
        loop {
            if !self.logging_out || self.wins.iter().any(|w| w.dlg.is_some()) {
                return;
            }
            let Some(id) = self.wins.iter().rev().find(|w| w.dlg.is_none()).map(|w| w.id) else {
                self.logging_out = false;
                self.logout_wait = None;
                sys.files().logout();
                self.theme = Theme::default();
                self.wall = theme::render_wallpaper(self.theme.wallpaper, self.w, self.h, self.theme.accent);
                self.icons.clear();
                self.show_pick(sys);
                self.flush_login();
                return;
            };
            self.raise(id);
            self.request_close(id, sys);
            if self.pos(id).is_some() {
                if self.modal_of(id).is_some() {
                    self.logout_wait = Some(id);
                } else {
                    self.logging_out = false;
                }
                return;
            }
        }
    }

    // ---------------------------------------------------------------- API pública

    /// Imagen final de la pantalla (0x00RRGGBB, w x h).
    pub fn back(&self) -> &Surface {
        &self.back
    }

    pub fn window_count(&self) -> usize {
        self.wins.len()
    }

    pub fn start_is_open(&self) -> bool {
        self.start_open
    }

    /// Marcos exteriores de las ventanas, de atrás hacia delante (para pruebas y vista previa).
    pub fn window_rects(&self) -> Vec<Rect> {
        self.wins.iter().map(|w| w.rect).collect()
    }

    /// Fuerza el redibujado completo en el próximo `render`.
    pub fn invalidate(&mut self) {
        self.dmg_all();
    }

    /// Atiende un evento de entrada o de tiempo.
    pub fn event(&mut self, ev: Event, sys: &mut dyn Platform) {
        match ev {
            Event::Mouse(p) => {
                self.mouse_to(self.mx + p.dx, self.my + p.dy, sys);
                let down = p.buttons & 1 != 0;
                if down != self.btn {
                    self.left_button(down, sys);
                }
                let right = p.buttons & 2 != 0;
                if right != self.rbtn {
                    self.right_button(right, sys);
                }
                if p.dz != 0 {
                    self.wheel(p.dz, sys);
                }
            }
            Event::Key(k) => self.key(k, sys),
            Event::Tick(ms) => self.tick(ms, sys),
        }
    }

    /// Mueve el puntero a una posición absoluta.
    pub fn mouse_to(&mut self, x: i32, y: i32, sys: &mut dyn Platform) {
        let (x, y) = (x.clamp(0, (self.w - 1).max(0)), y.clamp(0, (self.h - 1).max(0)));
        if (x, y) == (self.mx, self.my) {
            return;
        }
        self.dmg(cursor_rect(self.mx, self.my));
        self.mx = x;
        self.my = y;
        self.dmg(cursor_rect(x, y));
        if self.login.covers() {
            self.login.hover(x, y);
            self.flush_login();
            return;
        }
        if self.menu.is_some() {
            self.menu_edit(|m| m.hover(x, y));
            return;
        }
        match self.drag {
            Drag::Move { id, ox, oy } => {
                if let Some(i) = self.pos(id) {
                    let r = self.wins[i].rect;
                    let nx = (x - ox).max(-(r.w - 60)).min(self.w - 60);
                    let ny = (y - oy).clamp(0, (self.h - BAR_H - TITLE_H - 1).max(0));
                    self.set_rect(i, Rect::new(nx, ny, r.w, r.h));
                }
            }
            Drag::Resize { id, edge, start, px, py } => {
                if let Some(i) = self.pos(id) {
                    let (mw, mh) = self.wins[i].app.min_size();
                    let r = self.resized(start, edge, x - px, y - py, mw + 2, mh + TITLE_H + 2);
                    self.set_rect(i, r);
                }
            }
            _ => {}
        }
        self.after(sys);
        match self.drag {
            Drag::Content(id) => self.send(id, sys, |x, y| MouseEv::Move { x, y }),
            Drag::None => {
                if let Some(id) = self.chover {
                    self.send(id, sys, |x, y| MouseEv::Move { x, y });
                }
            }
            _ => {}
        }
    }

    /// Pulsa o suelta el botón izquierdo en la posición actual.
    pub fn left_button(&mut self, down: bool, sys: &mut dyn Platform) {
        if down == self.btn {
            return;
        }
        self.btn = down;
        if self.login.covers() {
            if down {
                let out = self.login.click(self.mx, self.my, sys);
                self.login_out(out, sys);
            }
            self.flush_login();
            return;
        }
        if self.menu.is_some() {
            if down {
                self.menu_click(sys);
            }
            return;
        }
        if down {
            self.press(sys);
        } else {
            self.release(sys);
        }
        self.after(sys);
    }

    /// Pulsa o suelta el botón derecho en la posición actual. Pulsado sobre el contenido de una
    /// ventana (no bloqueada por un diálogo) la enfoca y avisa a su app con `RightDown`.
    pub fn right_button(&mut self, down: bool, sys: &mut dyn Platform) {
        if down == self.rbtn {
            return;
        }
        self.rbtn = down;
        if !down || self.login.covers() {
            return;
        }
        if self.menu.is_some() {
            self.menu_click(sys);
            return;
        }
        match self.hit(self.mx, self.my) {
            Hit::None | Hit::Icon(_) if self.login.mode == Mode::Boot => {}
            Hit::Content(id) if self.modal_of(id).is_none() => {
                self.set_start(false);
                self.raise(id);
                self.send(id, sys, |x, y| MouseEv::RightDown { x, y });
            }
            Hit::None | Hit::Icon(_) => self.desktop_menu(self.mx, self.my, sys),
            _ => {}
        }
        self.after(sys);
    }

    /// Gira la rueda `dz` muescas (positivo = hacia abajo) sobre la ventana que está bajo el puntero,
    /// tenga o no el foco. Con un menú abierto se ignora.
    pub fn wheel(&mut self, dz: i32, sys: &mut dyn Platform) {
        if self.menu.is_some() || dz == 0 || self.drag != Drag::None || self.login.covers() {
            return;
        }
        if let Hit::Content(id) = self.hit(self.mx, self.my) {
            if self.modal_of(id).is_none() {
                self.send(id, sys, |x, y| MouseEv::Wheel { x, y, dz });
            }
        }
        self.after(sys);
    }

    /// Hay un menú contextual abierto (y el menú mismo, para pruebas y capturas).
    pub fn menu(&self) -> Option<&Menu> {
        self.menu.as_ref().map(|m| &m.menu)
    }

    /// Menú de clic derecho sobre un icono (lo selecciona) o sobre el fondo del escritorio.
    fn desktop_menu(&mut self, x: i32, y: i32, sys: &mut dyn Platform) {
        self.set_start(false);
        self.set_shut(false);
        match self.hit(x, y) {
            Hit::Icon(i) => {
                self.select(Some(i));
                let items = self.icon_items(i, sys);
                self.open_menu(None, MENU_ICON, x, y, items);
            }
            _ => {
                self.select(None);
                let items = self.bg_items(sys);
                self.open_menu(None, MENU_DESK, x, y, items);
            }
        }
    }

    fn select(&mut self, i: Option<usize>) {
        if self.sel != i {
            if let Some(o) = self.sel {
                self.dmg(self.icon_cell(o));
            }
            if let Some(n) = i {
                self.dmg(self.icon_cell(n));
            }
            self.sel = i;
        }
    }

    /// Ruta del icono `i` si es un archivo o carpeta de `C:\Escritorio` (no app ni Papelera).
    fn icon_path(&self, i: usize) -> Option<String> {
        match self.icons.get(i).map(|d| &d.act) {
            Some(Act::Path(p)) if p != files::TRASH => Some(p.clone()),
            _ => None,
        }
    }

    fn icon_items(&self, i: usize, sys: &mut dyn Platform) -> Vec<MenuItem> {
        let open = MenuItem::new(M_OPEN, "Abrir").bold();
        let Some(DIcon { act, icon, .. }) = self.icons.get(i) else { return Vec::new() };
        let Act::Path(p) = act else { return vec![open] };
        if p == files::TRASH {
            let empty = sys.files().trash_list().map_or(true, |v| v.is_empty());
            return vec![open, MenuItem::new(M_EMPTY, "Vaciar Papelera").disabled(empty)];
        }
        let dir = *icon == Icon::Folder;
        let mut v = vec![open];
        if dir {
            v.push(MenuItem::new(M_NEWWIN, "Abrir en ventana nueva"));
        } else {
            v.push(MenuItem::submenu("Abrir con", vec![MenuItem::new(M_NOTEPAD, "Bloc de notas")]));
        }
        v.extend([MenuItem::sep(), MenuItem::new(M_CUT, "Cortar"), MenuItem::new(M_COPY, "Copiar")]);
        if dir {
            v.push(MenuItem::new(M_PASTE_IN, "Pegar").disabled(sys.files().clip().is_none()));
        }
        v.extend([
            MenuItem::sep(),
            MenuItem::new(M_DELETE, "Eliminar"),
            MenuItem::new(M_RENAME, "Cambiar nombre"),
            MenuItem::sep(),
            MenuItem::new(M_PROPS, "Propiedades"),
        ]);
        v
    }

    fn bg_items(&self, sys: &mut dyn Platform) -> Vec<MenuItem> {
        let view = IconSize::ALL.iter().enumerate().map(|(k, e)| MenuItem::new(M_VIEW + k as u32, e.2).checked(self.size == e.0)).collect();
        let sort = Order::ALL.iter().enumerate().map(|(k, e)| MenuItem::new(M_SORT + k as u32, e.2).checked(self.order == e.0)).collect();
        let f = sys.files();
        let undo = match f.undo_label() {
            Some(l) => MenuItem::new(M_UNDO, &format!("Deshacer {l}")),
            None => MenuItem::new(M_UNDO, "Deshacer").disabled(true),
        };
        vec![
            MenuItem::submenu("Ver", view),
            MenuItem::submenu("Ordenar por", sort),
            MenuItem::new(M_REFRESH, "Actualizar"),
            MenuItem::sep(),
            MenuItem::new(M_PASTE, "Pegar").disabled(f.clip().is_none()),
            undo.shortcut("Ctrl+Z"),
            MenuItem::sep(),
            MenuItem::submenu("Nuevo", vec![MenuItem::new(M_NEWDIR, "Carpeta"), MenuItem::new(M_NEWDOC, "Documento de texto")]),
            MenuItem::sep(),
            MenuItem::new(M_CUSTOM, "Personalizar"),
        ]
    }

    /// Elección en un menú del escritorio (`owner == None`).
    fn desktop_menu_result(&mut self, tag: u32, id: u32, sys: &mut dyn Platform) {
        if tag == MENU_ICON {
            if let Some(i) = self.sel {
                self.icon_action(i, id, sys);
            }
        } else {
            self.bg_action(id, sys);
        }
        self.refresh_icons(sys);
    }

    fn icon_action(&mut self, i: usize, id: u32, sys: &mut dyn Platform) {
        let now = sys.now();
        let path = self.icon_path(i);
        match (id, path) {
            (M_OPEN, _) => self.activate_icon(i, sys),
            (M_EMPTY, None) => {
                let text = String::from("¿Quieres eliminar para siempre todos los elementos de la Papelera? No se podrán recuperar.");
                let req = DialogRequest::Message { tag: TAG_EMPTY, title: String::from("Vaciar Papelera"), text, buttons: Buttons::YesNo };
                self.open_dialog(None, req, sys);
            }
            (M_NEWWIN, Some(p)) => self.open_new_explorer(&p, sys),
            (M_NOTEPAD, Some(p)) => {
                let id = self.launch_id(AppKind::Notepad, sys);
                self.with_app(id, sys, |a, cx| a.open_path(&p, cx));
            }
            (M_CUT | M_COPY, Some(p)) => sys.files().set_clip(&p, id == M_CUT),
            (M_PASTE_IN, Some(p)) => {
                if let Err(e) = sys.files().paste(&p, now) {
                    self.info("No se puede pegar", e.message(), sys);
                }
            }
            (M_DELETE, Some(p)) => self.trash_path(&p, sys),
            (M_RENAME, Some(p)) => self.start_rename(&p, sys),
            (M_PROPS, Some(p)) => self.properties(&p, sys),
            _ => {}
        }
    }

    fn bg_action(&mut self, id: u32, sys: &mut dyn Platform) {
        let now = sys.now();
        match id {
            M_VIEW..=29 => {
                if let Some(e) = IconSize::ALL.get((id - M_VIEW) as usize) {
                    self.size = e.0;
                    // Cambia la rejilla: se repinta toda la zona de iconos.
                    self.dmg(Rect::new(0, 0, self.w, self.bar_y()));
                    save_config(sys, &self.theme, self.size, self.order);
                }
            }
            M_SORT..=39 => {
                if let Some(e) = Order::ALL.get((id - M_SORT) as usize) {
                    self.order = e.0;
                    self.reload_icons(sys);
                    save_config(sys, &self.theme, self.size, self.order);
                }
            }
            M_REFRESH => {
                self.reload_icons(sys);
                self.dmg_all();
            }
            M_PASTE => {
                if let Err(e) = {
                let dir = sys.files().std_dir(Std::Desktop);
                sys.files().paste(&dir, now)
            } {
                    self.info("No se puede pegar", e.message(), sys);
                }
            }
            M_UNDO => self.undo(sys),
            M_NEWDIR | M_NEWDOC => {
                let (tag, title, name) =
                    if id == M_NEWDIR { (TAG_NEWDIR, "Nueva carpeta", "Nueva carpeta") } else { (TAG_NEWDOC, "Nuevo documento de texto", "Nuevo documento de texto.txt") };
                let value = {
                let dir = sys.files().std_dir(Std::Desktop);
                sys.files().free_name(&dir, name)
            };
                let req = DialogRequest::Input { tag, title: String::from(title), label: String::from("Nombre:"), value };
                self.open_dialog(None, req, sys);
            }
            M_CUSTOM => self.launch(AppKind::Settings, sys),
            _ => {}
        }
    }

    /// Mensaje informativo o de error del sistema.
    fn info(&mut self, title: &str, text: &str, sys: &mut dyn Platform) {
        let req = DialogRequest::Message { tag: TAG_INFO, title: String::from(title), text: String::from(text), buttons: Buttons::Ok };
        self.open_dialog(None, req, sys);
    }

    fn undo(&mut self, sys: &mut dyn Platform) {
        let now = sys.now();
        if let Err(e) = sys.files().undo(now) {
            self.info("No se puede deshacer", e.message(), sys);
        }
        self.refresh_icons(sys);
    }

    fn trash_path(&mut self, path: &str, sys: &mut dyn Platform) {
        let now = sys.now();
        if let Err(e) = sys.files().user_trash(path, now) {
            self.info("No se puede eliminar", e.message(), sys);
        }
    }

    fn start_rename(&mut self, path: &str, sys: &mut dyn Platform) {
        self.pending = Some(String::from(path));
        let is_dir = sys.files().stat(path).is_ok_and(|e| e.dir);
        let value = String::from(files::display_name(files::file_name(path), is_dir, self.show_ext));
        let req = DialogRequest::Input { tag: TAG_RENAME, title: String::from("Cambiar nombre"), label: String::from("Nuevo nombre:"), value };
        self.open_dialog(None, req, sys);
    }

    /// Cuadro "Propiedades de <nombre>".
    fn properties(&mut self, path: &str, sys: &mut dyn Platform) {
        match props_text(sys, path) {
            Ok((title, text)) => self.info(&title, &text, sys),
            Err(e) => self.info("Propiedades", e.message(), sys),
        }
    }
    /// Teclas con el foco en el escritorio (ninguna ventana abierta a la vista).
    fn desktop_key(&mut self, k: KeyEvent, sys: &mut dyn Platform) {
        let path = self.sel.and_then(|i| self.icon_path(i));
        match (k.key, path) {
            (Key::Char('z'), _) if k.ctrl => self.undo(sys),
            (Key::Delete, Some(p)) => {
                self.trash_path(&p, sys);
                self.refresh_icons(sys);
            }
            (Key::F(2), Some(p)) => self.start_rename(&p, sys),
            _ => {}
        }
    }

    /// Abre un menú en coordenadas de pantalla; sustituye al que hubiera.
    fn open_menu(&mut self, owner: Option<u32>, tag: u32, x: i32, y: i32, items: Vec<MenuItem>) {
        self.close_menu();
        let screen = Rect::new(0, 0, self.w, self.bar_y());
        let menu = Menu::new(items, x, y, screen);
        self.dmg(menu.bounds());
        self.menu = Some(OpenMenu { menu, owner, tag });
    }

    fn close_menu(&mut self) {
        if let Some(m) = self.menu.take() {
            self.dmg(m.menu.bounds());
        }
    }

    /// Cambia el menú abierto y marca como sucio lo que ocupaba y lo que ocupa ahora.
    fn menu_edit<R>(&mut self, f: impl FnOnce(&mut Menu) -> R) -> Option<R> {
        let m = self.menu.as_mut()?;
        let old = m.menu.bounds();
        let r = f(&mut m.menu);
        let new = m.menu.bounds();
        self.dmg(old);
        self.dmg(new);
        Some(r)
    }

    /// Clic (izquierdo o derecho) con un menú abierto: elige, o cierra si es fuera. No llega a nada más.
    fn menu_click(&mut self, sys: &mut dyn Platform) {
        let (x, y) = (self.mx, self.my);
        let act = self.menu_edit(|m| m.click(x, y));
        self.menu_act(act, sys);
    }

    fn menu_act(&mut self, act: Option<MenuAct>, sys: &mut dyn Platform) {
        match act {
            Some(MenuAct::Pick(id)) => {
                let Some(m) = self.menu.take() else { return };
                self.dmg(m.menu.bounds());
                match m.owner {
                    Some(w) => self.with_app(w, sys, |a, cx| a.menu_result(m.tag, id, cx)),
                    None => self.desktop_menu_result(m.tag, id, sys),
                }
                self.after(sys);
            }
            Some(MenuAct::Close) => {
                self.close_menu();
                self.after(sys);
            }
            _ => {}
        }
    }

    /// Abre una app (o trae al frente Configuración si ya está abierta).
    pub fn launch(&mut self, kind: AppKind, sys: &mut dyn Platform) {
        self.launch_id(kind, sys);
    }

    /// Para pruebas: abre una app propia (por ejemplo, una que pide diálogos).
    pub fn launch_app(&mut self, app: Box<dyn App>, _sys: &mut dyn Platform) {
        self.push_window(app.kind(), app, None);
    }

    /// Para pruebas: abre un diálogo del sistema (sin dueña) y su respuesta la trata el escritorio.
    pub fn request_dialog(&mut self, req: DialogRequest, sys: &mut dyn Platform) {
        self.open_dialog(None, req, sys);
    }

    /// Ventanas abiertas que son diálogos.
    pub fn dialog_count(&self) -> usize {
        self.wins.iter().filter(|w| w.dlg.is_some()).count()
    }

    fn launch_id(&mut self, kind: AppKind, _sys: &mut dyn Platform) -> u32 {
        self.set_start(false);
        self.set_shut(false);
        if kind == AppKind::Settings {
            if let Some(w) = self.wins.iter().find(|w| w.kind == kind && w.dlg.is_none()) {
                let id = w.id;
                self.raise(id);
                return id;
            }
        }
        self.push_window(kind, apps::create(kind), None)
    }

    /// Crea la ventana de una app con el tamaño por defecto de su tipo.
    fn push_window(&mut self, kind: AppKind, app: Box<dyn App>, dlg: Option<Dialog>) -> u32 {
        let (dw, dh) = match &dlg {
            Some(d) => d.size(),
            None => apps::default_size(kind),
        };
        let cw = dw.min(self.w - 2).max(1);
        let ch = dh.min(self.h - BAR_H - TITLE_H - 2).max(1);
        let (ow, oh) = (cw + 2, ch + TITLE_H + 2);
        let n = (self.launched % 8) as i32;
        // Empieza a la derecha de la columna de iconos del escritorio.
        let x = (110 + 30 * n).min(self.w - ow).max(0);
        let y = (30 + 30 * n).min(self.h - BAR_H - oh).max(0);
        let rect = Rect::new(x, y, ow, oh);
        self.launched += 1;
        self.wins.push(Window {
            id: self.launched,
            kind,
            title: dlg.as_ref().map_or_else(|| app.title(), |d| String::from(d.title())),
            app,
            rect,
            restore: rect,
            content: Surface::new(cw, ch, 0xFFFFFF),
            dirty: true,
            min: false,
            max: false,
            dlg,
            owner: None,
        });
        self.dmg_all();
        self.launched
    }

    /// Abre un diálogo ligado a la ventana `owner` (o del sistema) centrado sobre ella.
    fn open_dialog(&mut self, owner: Option<u32>, req: DialogRequest, sys: &mut dyn Platform) {
        if let Some(o) = owner {
            self.close_dialogs_of(o);
        }
        let dlg = Dialog::new(req, sys);
        let kind = owner.and_then(|o| self.pos(o)).map_or(AppKind::Files, |i| self.wins[i].kind);
        let area = owner.and_then(|o| self.pos(o)).map_or(Rect::new(0, 0, self.w, self.h - BAR_H), |i| self.wins[i].rect);
        let id = self.push_window(kind, Box::new(NoApp(kind)), Some(dlg));
        let i = self.pos(id).unwrap();
        let r = self.wins[i].rect;
        let x = (area.x + (area.w - r.w) / 2).clamp(0, (self.w - r.w).max(0));
        let y = (area.y + (area.h - r.h) / 2).clamp(0, (self.h - BAR_H - r.h).max(0));
        self.wins[i].rect = Rect::new(x, y, r.w, r.h);
        self.wins[i].owner = owner;
    }

    /// Recompone en la imagen final las zonas que han cambiado y devuelve esas zonas
    /// (ya recortadas a la pantalla) para que el núcleo las copie al monitor.
    pub fn render(&mut self, sys: &mut dyn Platform) -> Vec<Rect> {
        let act = self.active_id();
        let hidden = self.login.covers();
        if act != self.last_active {
            for w in self.wins.iter_mut() {
                if Some(w.id) == act || Some(w.id) == self.last_active {
                    w.dirty = true;
                }
            }
            self.last_active = act;
            self.dmg_all();
        }
        for w in self.wins.iter_mut() {
            if w.dirty && !w.min && !hidden {
                match w.dlg.as_mut() {
                    Some(d) => d.draw(&mut w.content, &self.theme),
                    None => w.app.draw(&mut w.content, &self.theme, sys, Some(w.id) == act),
                }
                w.dirty = false;
                self.damage.push(w.content_rect());
            }
        }
        let zones = normalize(core::mem::take(&mut self.damage), Rect::new(0, 0, self.w, self.h));
        let mut back = core::mem::replace(&mut self.back, Surface::empty());
        for z in &zones {
            back.set_clip(*z);
            self.paint(&mut back, act);
        }
        back.reset_clip();
        self.back = back;
        zones
    }

    // ---------------------------------------------------------------- daño y utilidades

    fn dmg(&mut self, r: Rect) {
        if r.is_empty() {
            return;
        }
        if self.damage.len() >= 32 {
            let u = self.damage.iter().fold(r, |u, d| u.union(*d));
            self.damage = vec![u];
        } else {
            self.damage.push(r);
        }
    }

    fn dmg_all(&mut self) {
        self.dmg(Rect::new(0, 0, self.w, self.h));
    }

    fn pos(&self, id: u32) -> Option<usize> {
        self.wins.iter().position(|w| w.id == id)
    }

    /// La ventana de más arriba que no está minimizada.
    fn active_id(&self) -> Option<u32> {
        self.wins.iter().rev().find(|w| !w.min).map(|w| w.id)
    }

    /// Ventanas por orden de apertura (el de la barra de tareas); los diálogos no cuentan.
    fn task_ids(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.wins.iter().filter(|w| w.dlg.is_none()).map(|w| w.id).collect();
        v.sort_unstable();
        v
    }

    /// Diálogo ligado a la ventana `id`, si lo hay (la deja bloqueada).
    fn modal_of(&self, id: u32) -> Option<u32> {
        self.wins.iter().find(|w| w.owner == Some(id)).map(|w| w.id)
    }

    fn close_dialogs_of(&mut self, id: u32) {
        while let Some(d) = self.modal_of(id) {
            if let Some(i) = self.pos(d) {
                self.wins.remove(i);
            }
            self.forget(d);
            self.dmg_all();
        }
    }

    /// Llama a la app de la ventana `id` y aplica lo que pida.
    fn with_app(&mut self, id: u32, sys: &mut dyn Platform, f: impl FnOnce(&mut dyn App, &mut Ctx)) {
        let Some(i) = self.pos(id) else { return };
        let mut cx = Ctx::new(sys, &mut self.theme, self.now_ms);
        f(&mut *self.wins[i].app, &mut cx);
        let (redraw, close, themed) = (cx.redraw, cx.close, cx.theme_changed);
        let (dialog, open) = (cx.dialog.take(), cx.open.take());
        let (open_new, menu) = (cx.open_new.take(), cx.menu.take());
        let open_term = cx.open_terminal.take();
        if themed {
            self.wall = theme::render_wallpaper(self.theme.wallpaper, self.w, self.h, self.theme.accent);
            for w in self.wins.iter_mut() {
                w.dirty = true;
            }
            self.dmg_all();
            save_config(sys, &self.theme, self.size, self.order);
        }
        if redraw {
            self.wins[i].dirty = true;
        }
        let title = self.wins[i].app.title();
        if title != self.wins[i].title {
            self.wins[i].title = title;
            self.dmg_all();
        }
        if close {
            self.close(id);
        } else if let Some(req) = dialog {
            self.open_dialog(Some(id), req, sys);
        }
        if let Some(p) = open {
            self.open_path(&p, sys);
        }
        if let Some(p) = open_new {
            self.open_new_explorer(&p, sys);
        }
        if let Some(p) = open_term {
            self.open_terminal_at(&p, sys);
        }
        if let Some(req) = menu {
            // De coordenadas del contenido de la ventana a las de pantalla.
            if let Some(c) = self.pos(id).filter(|&i| !self.wins[i].min).map(|i| self.wins[i].content_rect()) {
                self.open_menu(Some(id), req.tag, c.x + req.x, c.y + req.y, req.items);
            }
        }
    }

    /// Abre un Terminal NUEVO situado en la carpeta `path`.
    fn open_terminal_at(&mut self, path: &str, sys: &mut dyn Platform) {
        let id = self.launch_id(AppKind::Terminal, sys);
        self.with_app(id, sys, |a, cx| a.open_path(path, cx));
    }

    /// Abre `path` en una ventana NUEVA del Explorador.
    fn open_new_explorer(&mut self, path: &str, sys: &mut dyn Platform) {
        let id = self.launch_id(AppKind::Files, sys);
        self.with_app(id, sys, |a, cx| a.open_path(path, cx));
    }

    /// Abre una ruta: carpeta (o Papelera) en el Explorador, archivo en un Bloc de notas nuevo.
    fn open_path(&mut self, path: &str, sys: &mut dyn Platform) {
        if path == files::TRASH || sys.files().stat(path).is_ok_and(|e| e.dir) {
            let id = match self.wins.iter().rev().find(|w| w.kind == AppKind::Files && w.dlg.is_none()) {
                Some(w) => w.id,
                None => self.launch_id(AppKind::Files, sys),
            };
            self.raise(id);
            self.with_app(id, sys, |a, cx| a.open_path(path, cx));
        } else {
            let id = self.launch_id(AppKind::Notepad, sys);
            self.with_app(id, sys, |a, cx| a.open_path(path, cx));
        }
    }

    /// Envía un evento de ratón con coordenadas relativas al contenido de la ventana.
    fn send(&mut self, id: u32, sys: &mut dyn Platform, f: impl Fn(i32, i32) -> MouseEv) {
        let Some(i) = self.pos(id) else { return };
        let c = self.wins[i].content_rect();
        let ev = f(self.mx - c.x, self.my - c.y);
        if self.wins[i].dlg.is_some() {
            self.dlg_event(id, sys, |d, s| d.mouse(ev, s));
        } else {
            self.with_app(id, sys, |a, cx| a.mouse(ev, cx));
        }
    }

    /// Pasa un evento al diálogo de la ventana `id`; si termina, entrega la respuesta.
    fn dlg_event(&mut self, id: u32, sys: &mut dyn Platform, f: impl FnOnce(&mut Dialog, &mut dyn Platform) -> Option<DialogResult>) {
        let Some(i) = self.pos(id) else { return };
        let Some(d) = self.wins[i].dlg.as_mut() else { return };
        let res = f(d, sys);
        if d.take_dirty() {
            self.wins[i].dirty = true;
        }
        if let Some(r) = res {
            self.finish_dialog(id, r, sys);
        }
    }

    /// Cierra el diálogo y entrega su respuesta a la ventana dueña o al sistema.
    fn finish_dialog(&mut self, id: u32, res: DialogResult, sys: &mut dyn Platform) {
        let owner = self.pos(id).and_then(|i| self.wins[i].owner);
        self.close(id);
        match owner {
            Some(o) => {
                self.with_app(o, sys, |a, cx| a.dialog_result(res, cx));
                if self.logging_out && self.logout_wait == Some(o) {
                    // La ventana sigue y no hay otro diálogo suyo: el usuario canceló.
                    if self.pos(o).is_none() {
                        self.logout_wait = None;
                        self.logout_step(sys);
                    } else if self.modal_of(o).is_none() {
                        self.logging_out = false;
                        self.logout_wait = None;
                    }
                }
            }
            None => self.system_result(res, sys),
        }
    }

    /// Respuestas a los diálogos del sistema.
    fn system_result(&mut self, res: DialogResult, sys: &mut dyn Platform) {
        match res {
            DialogResult::Answer { tag: TAG_FORMAT, answer } => {
                if answer == Answer::Yes {
                    let now = sys.now();
                    match sys.files().format_pending(now) {
                        Ok(()) => {
                            // El disco nuevo no tiene cuentas: se descarta la sesión de la unidad en
                            // memoria y se pasa al asistente (tras los avisos).
                            let ids: Vec<u32> = self.wins.iter().map(|w| w.id).collect();
                            for id in ids {
                                self.close(id);
                            }
                            sys.files().logout();
                            self.login.start_boot();
                            self.screen_on();
                        }
                        Err(e) => {
                            let text = format!("No se ha podido preparar el disco: {}", e.message());
                            let req = DialogRequest::Message { tag: TAG_SYS, title: String::from("Disco nuevo"), text, buttons: Buttons::Ok };
                            self.open_dialog(None, req, sys);
                            return;
                        }
                    }
                } else {
                    // Solo el Sí formatea: cualquier otra respuesta descarta el disco.
                    sys.files().decline_format();
                }
                self.show_notice(sys);
            }
            DialogResult::Answer { tag: TAG_SYS, .. } => self.show_notice(sys),
            DialogResult::Answer { tag: u32::MAX, .. } => self.enter_flow(sys),
            DialogResult::Answer { tag: TAG_EMPTY, answer: Answer::Yes } => {
                if let Err(e) = sys.files().empty_trash() {
                    self.info("No se puede vaciar la Papelera", e.message(), sys);
                }
                self.refresh_icons(sys);
            }
            DialogResult::Text { tag: TAG_RENAME, value } => {
                let from = self.pending.take();
                if let (Some(from), Some(v)) = (from, value) {
                    // Con las extensiones ocultas el diálogo no la mostró: se conserva la original.
                    let old = files::file_name(&from);
                    let is_dir = sys.files().stat(&from).is_ok_and(|e| e.dir);
                    let tail = &old[files::display_name(old, is_dir, self.show_ext).len()..];
                    let full = if v.trim().is_empty() { String::new() } else { format!("{}{tail}", v.trim()) };
                    let name = full.as_str();
                    if name == old {
                        return;
                    }
                    let res = if files::valid_name(name) {
                        let to = files::join(&files::parent(&from).unwrap_or_default(), name);
                        sys.files().user_rename(&from, &to)
                    } else {
                        Err(FsError::BadName)
                    };
                    if let Err(e) = res {
                        self.info("No se puede cambiar el nombre", e.message(), sys);
                    }
                    self.refresh_icons(sys);
                }
            }
            DialogResult::Text { tag: tag @ (TAG_NEWDIR | TAG_NEWDOC), value: Some(v) } => {
                let mut name = String::from(v.trim());
                if tag == TAG_NEWDOC && !name.contains('.') {
                    name.push_str(".txt");
                }
                let now = sys.now();
                let res = if files::valid_name(&name) {
                    {
                    let p = files::join(&sys.files().std_dir(Std::Desktop), &name);
                    sys.files().user_create(&p, tag == TAG_NEWDIR, now)
                }
                } else {
                    Err(FsError::BadName)
                };
                if let Err(e) = res {
                    self.info("No se puede crear", e.message(), sys);
                }
                self.refresh_icons(sys);
            }
            _ => {}
        }
    }

    fn show_notice(&mut self, sys: &mut dyn Platform) {
        if let Some(text) = sys.files().notice() {
            let req = DialogRequest::Message { tag: u32::MAX, title: String::from("Almacenamiento"), text: String::from(text), buttons: Buttons::Ok };
            self.open_dialog(None, req, sys);
        } else {
            self.enter_flow(sys);
        }
    }

    /// Primer tick: pregunta por el disco nuevo (si lo hay) y, después, avisa de que no se guarda.
    fn boot_flow(&mut self, sys: &mut dyn Platform) {
        if sys.files().pending_format() {
            let text = "Se ha encontrado un disco sin formato. ¿Quieres prepararlo para NightFury? Si dices que no, los archivos se guardarán solo en la memoria y se perderán al apagar.";
            let req = DialogRequest::Message { tag: TAG_FORMAT, title: String::from("Disco nuevo"), text: String::from(text), buttons: Buttons::YesNo };
            self.open_dialog(None, req, sys);
        } else {
            self.show_notice(sys);
        }
    }

    /// El usuario quiere cerrar la ventana `id` (aspa o Alt+F4).
    fn request_close(&mut self, id: u32, sys: &mut dyn Platform) {
        let Some(i) = self.pos(id) else { return };
        if self.wins[i].dlg.is_some() {
            self.dlg_event(id, sys, |d, _| Some(d.cancel()));
            return;
        }
        let mut ok = false;
        self.with_app(id, sys, |a, cx| ok = a.request_close(cx));
        if ok {
            self.close(id);
        }
    }

    /// Trae `id` al frente; si tiene un diálogo, este queda encima.
    fn raise(&mut self, id: u32) {
        if let Some(i) = self.pos(id) {
            let mut w = self.wins.remove(i);
            w.min = false;
            self.wins.push(w);
            if let Some(j) = self.modal_of(id).and_then(|d| self.pos(d)) {
                let mut d = self.wins.remove(j);
                d.min = false;
                self.wins.push(d);
            }
            self.dmg_all();
        }
    }

    fn minimize(&mut self, id: u32) {
        let ids = [self.modal_of(id), Some(id)];
        for id in ids.into_iter().flatten() {
            if let Some(i) = self.pos(id) {
                let mut w = self.wins.remove(i);
                w.min = true;
                self.wins.insert(0, w);
                self.forget(id);
            }
        }
        self.dmg_all();
    }

    fn close(&mut self, id: u32) {
        self.close_dialogs_of(id);
        if let Some(i) = self.pos(id) {
            self.wins.remove(i);
            self.forget(id);
            self.dmg_all();
        }
    }

    /// Olvida referencias a una ventana que se ha ido (cierre o minimizado).
    fn forget(&mut self, id: u32) {
        if self.menu.as_ref().is_some_and(|m| m.owner == Some(id)) {
            self.close_menu();
        }
        if self.chover == Some(id) {
            self.chover = None;
        }
        let d = match self.drag {
            Drag::Move { id: i, .. } | Drag::Resize { id: i, .. } | Drag::Cap(i, _) | Drag::Content(i) => i == id,
            Drag::None => false,
        };
        if d {
            self.drag = Drag::None;
        }
    }

    fn toggle_max(&mut self, id: u32) {
        let Some(i) = self.pos(id) else { return };
        let r = if self.wins[i].max {
            self.wins[i].max = false;
            self.wins[i].restore
        } else {
            self.wins[i].restore = self.wins[i].rect;
            self.wins[i].max = true;
            Rect::new(0, 0, self.w, self.h - BAR_H)
        };
        self.set_rect(i, r);
        self.dmg_all();
    }

    /// Cambia el marco; si cambia el área de contenido, recrea su superficie.
    fn set_rect(&mut self, i: usize, r: Rect) {
        let old = self.wins[i].bounds();
        let w = &mut self.wins[i];
        w.rect = r;
        let c = w.content_rect();
        if (c.w, c.h) != (w.content.w, w.content.h) {
            w.content = Surface::new(c.w, c.h, 0xFFFFFF);
            w.dirty = true;
        }
        let new = w.bounds();
        self.dmg(old);
        self.dmg(new);
    }

    /// Marco resultante de arrastrar el borde `edge` (dx, dy) desde `s`, con tamaño mínimo (mw, mh).
    fn resized(&self, s: Rect, edge: u8, dx: i32, dy: i32, mw: i32, mh: i32) -> Rect {
        let (mut x, mut y, mut r, mut b) = (s.x, s.y, s.right(), s.bottom());
        if edge & 1 != 0 {
            x = (s.x + dx).min(r - mw);
        }
        if edge & 2 != 0 {
            r = (s.right() + dx).max(x + mw);
        }
        if edge & 4 != 0 {
            y = (s.y + dy).max(0).min(b - mh);
        }
        if edge & 8 != 0 {
            b = (s.bottom() + dy).max(y + mh);
        }
        Rect::new(x, y, r - x, b - y)
    }

    fn set_start(&mut self, open: bool) {
        if open != self.start_open {
            self.start_open = open;
            self.dmg(self.menu_rect());
            self.dmg(self.start_rect());
            if !open {
                self.set_shut(false);
            }
        }
    }

    fn set_shut(&mut self, open: bool) {
        if open != self.shut_open {
            self.shut_open = open;
            self.dmg(self.shut_rect());
            self.dmg(self.mpower_rect());
        }
    }

    // ---------------------------------------------------------------- geometría de la interfaz

    fn bar_y(&self) -> i32 {
        self.h - BAR_H
    }
    fn start_rect(&self) -> Rect {
        Rect::new(0, self.bar_y(), 48, BAR_H)
    }
    fn task_rect(&self, i: usize) -> Rect {
        Rect::new(48 + 48 * i as i32, self.bar_y(), 48, BAR_H)
    }
    fn show_rect(&self) -> Rect {
        Rect::new(self.w - 6, self.bar_y(), 6, BAR_H)
    }
    fn clock_rect(&self) -> Rect {
        Rect::new(self.w - 92, self.bar_y(), 86, BAR_H)
    }
    fn menu_rect(&self) -> Rect {
        let (w, h) = (self.w.min(620), (self.h - BAR_H).min(500));
        Rect::new(0, self.bar_y() - h, w, h)
    }
    fn msettings_rect(&self) -> Rect {
        let m = self.menu_rect();
        Rect::new(m.x, m.bottom() - 96, 48, 48)
    }
    fn mpower_rect(&self) -> Rect {
        let m = self.menu_rect();
        Rect::new(m.x, m.bottom() - 48, 48, 48)
    }
    fn row_rect(&self, i: usize) -> Rect {
        let m = self.menu_rect();
        Rect::new(m.x + 48, m.y + 8 + 40 * i as i32, 240, 40)
    }
    fn tile_rect(&self, i: usize) -> Rect {
        let m = self.menu_rect();
        Rect::new(m.x + 300 + (i % 2) as i32 * 104, m.y + 40 + (i / 2) as i32 * 104, 100, 100)
    }
    fn shut_rect(&self) -> Rect {
        let p = self.mpower_rect();
        let n = self.shut_count() as i32;
        Rect::new(p.x, p.y - 40 * n, 180, 40 * n)
    }
    /// Filas del menú de apagado: Apagar, Reiniciar y, con disco con cuentas, Bloquear y Cerrar sesión.
    fn shut_count(&self) -> usize {
        if self.can_session {
            4
        } else {
            2
        }
    }
    fn shut_row(&self, i: usize) -> Rect {
        let s = self.shut_rect();
        Rect::new(s.x, s.y + 40 * i as i32, s.w, 40)
    }
    /// Iconos en columnas, de arriba abajo; pasan a la columna siguiente si no caben.
    fn icon_cell(&self, i: usize) -> Rect {
        let (cw, ch, ..) = self.size.metrics();
        let rows = ((self.h - BAR_H - 6) / ch).max(1) as usize;
        Rect::new(6 + cw * (i / rows) as i32, 6 + ch * (i % rows) as i32, cw, ch)
    }

    /// Refresca los iconos del escritorio si ha cambiado algo en los archivos.
    fn refresh_icons(&mut self, sys: &mut dyn Platform) {
        let g = sys.files().generation();
        if g == self.files_gen || sys.files().user().is_none() {
            return;
        }
        self.files_gen = g;
        self.reload_icons(sys);
    }

    /// Reconstruye los iconos; si cambian, repinta y conserva la selección por nombre.
    fn reload_icons(&mut self, sys: &mut dyn Platform) {
        self.show_ext = sys.files().show_ext();
        let new = build_icons(sys, self.order, self.show_ext);
        let same = new.len() == self.icons.len()
            && new.iter().zip(&self.icons).all(|(a, b)| a.name == b.name && a.label == b.label && a.icon == b.icon);
        if same {
            return;
        }
        for i in 0..self.icons.len().max(new.len()) {
            self.dmg(self.icon_cell(i));
        }
        let name = self.sel.and_then(|s| self.icons.get(s)).map(|d| d.name.clone());
        self.icons = new;
        self.sel = name.and_then(|n| self.icons.iter().position(|d| d.name == n));
    }

    /// Abre el icono `i` del escritorio.
    fn activate_icon(&mut self, i: usize, sys: &mut dyn Platform) {
        match self.icons.get(i).map(|d| &d.act) {
            Some(Act::App(k)) => {
                let k = *k;
                self.launch(k, sys);
            }
            Some(Act::Path(p)) => {
                let p = p.clone();
                self.set_start(false);
                self.open_path(&p, sys);
            }
            None => {}
        }
    }

    // ---------------------------------------------------------------- puntero y entrada

    fn hit(&self, x: i32, y: i32) -> Hit {
        // Con una pantalla de cuentas no hay nada alcanzable; en el arranque solo los diálogos.
        let boot = self.login.mode == Mode::Boot;
        if self.login.covers() {
            return Hit::None;
        }
        if self.shut_open && self.shut_rect().contains(x, y) {
            return Hit::Shut(((y - self.shut_rect().y) / 40).clamp(0, self.shut_count() as i32 - 1) as usize);
        }
        if self.start_open && self.menu_rect().contains(x, y) {
            if self.msettings_rect().contains(x, y) {
                return Hit::MSettings;
            }
            if self.mpower_rect().contains(x, y) {
                return Hit::MPower;
            }
            for i in 0..APPS.len() {
                if self.row_rect(i).contains(x, y) {
                    return Hit::Row(i);
                }
                if self.tile_rect(i).contains(x, y) {
                    return Hit::Tile(i);
                }
            }
            return Hit::MenuBg;
        }
        if y >= self.bar_y() && !boot {
            if self.start_rect().contains(x, y) {
                return Hit::Start;
            }
            if self.show_rect().contains(x, y) {
                return Hit::Show;
            }
            for (i, id) in self.task_ids().into_iter().enumerate() {
                if self.task_rect(i).contains(x, y) {
                    return Hit::Task(id);
                }
            }
            return Hit::Bar;
        }
        for w in self.wins.iter().rev().filter(|w| !w.min) {
            if w.rect.contains(x, y) {
                return Self::hit_win(w, x, y);
            }
        }
        if boot {
            return Hit::None;
        }
        (0..self.icons.len()).find(|&i| self.icon_cell(i).contains(x, y)).map_or(Hit::None, Hit::Icon)
    }

    fn hit_win(w: &Window, x: i32, y: i32) -> Hit {
        let r = w.rect;
        if !w.max && w.dlg.is_none() {
            let (l, rt, b) = (x < r.x + 6, x >= r.right() - 6, y >= r.bottom() - 6);
            let t = y < r.y + 3 || ((l || rt) && y < r.y + 6);
            let e = l as u8 | (rt as u8) << 1 | (t as u8) << 2 | (b as u8) << 3;
            if e != 0 {
                return Hit::Edge(w.id, e);
            }
        }
        if w.title_rect().contains(x, y) {
            // Un diálogo solo tiene el botón de cerrar.
            for k in (if w.dlg.is_some() { 2 } else { 0 })..3 {
                if w.cap(k).contains(x, y) {
                    return Hit::Cap(w.id, k as u8);
                }
            }
            return Hit::Title(w.id);
        }
        Hit::Content(w.id)
    }

    /// Solo los elementos que cambian de aspecto al pasar el ratón.
    fn hot_of(h: Hit) -> Hit {
        match h {
            Hit::Start | Hit::Task(_) | Hit::Show | Hit::Row(_) | Hit::Tile(_) | Hit::MSettings | Hit::MPower | Hit::Shut(_) | Hit::Cap(..) => h,
            _ => Hit::None,
        }
    }

    fn hot_rect(&self, h: Hit) -> Rect {
        match h {
            Hit::Start => self.start_rect(),
            Hit::Task(id) => self.task_ids().iter().position(|&t| t == id).map_or(Rect::default(), |i| self.task_rect(i)),
            Hit::Show => self.show_rect(),
            Hit::Row(i) => self.row_rect(i),
            Hit::Tile(i) => self.tile_rect(i),
            Hit::MSettings => self.msettings_rect(),
            Hit::MPower => self.mpower_rect(),
            Hit::Shut(i) => self.shut_row(i),
            Hit::Cap(id, k) => self.pos(id).map_or(Rect::default(), |i| self.wins[i].cap(k as i32)),
            _ => Rect::default(),
        }
    }

    /// Tras cualquier cambio: actualiza el resaltado y avisa a la app que el puntero abandona.
    fn after(&mut self, sys: &mut dyn Platform) {
        let hit = self.hit(self.mx, self.my);
        let cur = match (self.drag, hit) {
            (Drag::Resize { edge, .. }, _) | (Drag::None, Hit::Edge(_, edge)) => resize_cursor(edge),
            _ => 0,
        };
        if cur != self.cur {
            self.cur = cur;
            self.dmg(cursor_rect(self.mx, self.my));
        }
        if !matches!(self.drag, Drag::Move { .. } | Drag::Resize { .. }) {
            let hot = Self::hot_of(hit);
            if hot != self.hot {
                self.dmg(self.hot_rect(self.hot));
                self.dmg(self.hot_rect(hot));
                self.hot = hot;
            }
        }
        let target = match hit {
            Hit::Content(id) if matches!(self.drag, Drag::None | Drag::Content(_)) && Some(id) == self.active_id() => Some(id),
            _ => None,
        };
        if target != self.chover {
            if let Some(old) = self.chover {
                if self.pos(old).is_some_and(|i| self.wins[i].dlg.is_some()) {
                    self.dlg_event(old, sys, |d, s| d.mouse(MouseEv::Leave, s));
                } else {
                    self.with_app(old, sys, |a, cx| a.mouse(MouseEv::Leave, cx));
                }
            }
            self.chover = target;
        }
    }

    fn press(&mut self, sys: &mut dyn Platform) {
        let hit = self.hit(self.mx, self.my);
        let shut_was = self.shut_open;
        if !matches!(hit, Hit::Shut(_)) {
            self.set_shut(false);
        }
        let in_menu = matches!(hit, Hit::Row(_) | Hit::Tile(_) | Hit::MSettings | Hit::MPower | Hit::MenuBg | Hit::Shut(_));
        if self.start_open && !in_menu && hit != Hit::Start {
            self.set_start(false);
        }
        match hit {
            Hit::Shut(i) => {
                self.set_shut(false);
                match i {
                    0 => sys.shutdown(),
                    1 => sys.reboot(),
                    2 => self.lock(sys),
                    _ => self.begin_logout(sys),
                }
            }
            Hit::MPower => self.set_shut(!shut_was),
            Hit::MSettings => self.launch(AppKind::Settings, sys),
            Hit::Row(i) | Hit::Tile(i) => self.launch(APPS[i].0, sys),
            Hit::Start => self.set_start(!self.start_open),
            Hit::Task(id) => {
                if self.active_id() == Some(id) {
                    self.minimize(id);
                } else {
                    self.raise(id);
                }
            }
            Hit::Show => {
                for id in self.task_ids() {
                    self.minimize(id);
                }
            }
            Hit::Icon(i) => {
                let old = self.sel.replace(i);
                if let Some(o) = old {
                    self.dmg(self.icon_cell(o));
                }
                self.dmg(self.icon_cell(i));
                if self.double(hit) {
                    self.activate_icon(i, sys);
                }
            }
            Hit::None => {
                if let Some(o) = self.sel.take() {
                    self.dmg(self.icon_cell(o));
                }
            }
            Hit::Cap(id, k) => {
                self.raise(id);
                // Con un diálogo abierto la ventana está bloqueada: solo se enfoca el diálogo.
                if self.modal_of(id).is_none() {
                    self.drag = Drag::Cap(id, k);
                }
            }
            Hit::Title(id) => {
                self.raise(id);
                let dialog = self.pos(id).is_some_and(|i| self.wins[i].dlg.is_some());
                if dialog {
                    if let Some(i) = self.pos(id) {
                        let r = self.wins[i].rect;
                        self.drag = Drag::Move { id, ox: self.mx - r.x, oy: self.my - r.y };
                    }
                } else if self.double(hit) {
                    self.toggle_max(id);
                } else if let Some(i) = self.pos(id) {
                    if !self.wins[i].max {
                        let r = self.wins[i].rect;
                        self.drag = Drag::Move { id, ox: self.mx - r.x, oy: self.my - r.y };
                    }
                }
            }
            Hit::Edge(id, edge) => {
                self.raise(id);
                if let Some(i) = self.pos(id).filter(|_| self.modal_of(id).is_none()) {
                    self.drag = Drag::Resize { id, edge, start: self.wins[i].rect, px: self.mx, py: self.my };
                }
            }
            Hit::Content(id) => {
                self.raise(id);
                if self.modal_of(id).is_none() {
                    self.drag = Drag::Content(id);
                    self.send(id, sys, |x, y| MouseEv::Down { x, y });
                }
            }
            Hit::Bar | Hit::MenuBg => {}
        }
    }

    fn release(&mut self, sys: &mut dyn Platform) {
        let drag = core::mem::replace(&mut self.drag, Drag::None);
        match drag {
            Drag::Cap(id, k) => {
                if self.hit(self.mx, self.my) == Hit::Cap(id, k) {
                    match k {
                        0 => self.minimize(id),
                        1 => self.toggle_max(id),
                        _ => self.request_close(id, sys),
                    }
                }
            }
            Drag::Content(id) => self.send(id, sys, |x, y| MouseEv::Up { x, y }),
            _ => {}
        }
    }

    /// ¿Es este clic el segundo de un doble clic sobre el mismo elemento?
    fn double(&mut self, h: Hit) -> bool {
        let (now, x, y) = (self.now_ms, self.mx, self.my);
        let dbl = matches!(self.last_click, Some((t, lx, ly, lh))
            if lh == h && now.saturating_sub(t) < DOUBLE_MS && (x - lx).abs() < 5 && (y - ly).abs() < 5);
        self.last_click = if dbl { None } else { Some((now, x, y, h)) };
        dbl
    }

    /// Entrega a la app con el foco el estado de Ctrl y Mayús cada vez que cambia: un
    /// `KeyEvent` con `key: Key::Other`, `ch: None`, `alt: false` y las banderas `ctrl`/`shift`
    /// al día (`down` = alguna de las dos pulsada). Si el foco pasa a otra ventana, o se abre un
    /// menú, diálogo o el menú Inicio, la app anterior recibe el estado neutro (todo en `false`).
    fn sync_mods(&mut self, sys: &mut dyn Platform) {
        let target = if self.menu.is_some() || self.start_open || self.shut_open || self.login.covers() {
            None
        } else {
            self.active_id().filter(|&id| self.pos(id).is_some_and(|i| self.wins[i].dlg.is_none()))
        };
        let want = if target.is_some() { self.mods } else { (false, false) };
        if self.mods_to != target {
            if let Some(old) = self.mods_to.take() {
                if self.mods_sent != (false, false) {
                    self.send_mods(old, (false, false), sys);
                }
            }
            self.mods_sent = (false, false);
            self.mods_to = target;
        }
        if let Some(id) = target {
            if want != self.mods_sent {
                self.mods_sent = want;
                self.send_mods(id, want, sys);
            }
        }
    }

    fn send_mods(&mut self, id: u32, (ctrl, shift): (bool, bool), sys: &mut dyn Platform) {
        let ev = KeyEvent { key: Key::Other, ch: None, down: ctrl || shift, shift, ctrl, alt: false };
        self.with_app(id, sys, |a, cx| a.key(&ev, cx));
    }

    fn key(&mut self, k: KeyEvent, sys: &mut dyn Platform) {
        // Ctrl y Mayús llegan como `Key::Other`; se reparten aparte (ver `sync_mods`).
        if k.key == Key::Other {
            self.mods = (k.ctrl, k.shift);
            self.sync_mods(sys);
            self.after(sys);
            return;
        }
        if !k.down {
            return;
        }
        if self.login.covers() {
            let out = self.login.key(&k, sys);
            self.login_out(out, sys);
            self.flush_login();
            return;
        }
        if self.menu.is_some() {
            let act = self.menu_edit(|m| m.key(k.key));
            self.menu_act(act, sys);
            return;
        }
        if self.login.mode == Mode::Boot && (k.key == Key::Super || self.active_id().is_none()) {
            return;
        }
        if k.key == Key::Super {
            self.set_start(!self.start_open);
        } else if k.key == Key::Escape && (self.start_open || self.shut_open) {
            if self.shut_open {
                self.set_shut(false);
            } else {
                self.set_start(false);
            }
        } else if k.key == Key::F(4) && k.alt {
            if let Some(id) = self.active_id() {
                self.request_close(id, sys);
            }
        } else if !self.start_open && self.active_id().is_none() {
            self.desktop_key(k, sys);
        } else if !self.start_open {
            if let Some(id) = self.active_id() {
                if self.pos(id).is_some_and(|i| self.wins[i].dlg.is_some()) {
                    self.dlg_event(id, sys, |d, s| d.key(&k, s));
                } else {
                    self.with_app(id, sys, |a, cx| a.key(&k, cx));
                }
            }
        }
        self.after(sys);
    }

    fn tick(&mut self, ms: u64, sys: &mut dyn Platform) {
        self.now_ms = ms;
        self.login.tick(ms);
        self.flush_login();
        if !self.booted {
            self.booted = true;
            self.boot_flow(sys);
        }
        self.refresh_icons(sys);
        self.sync_mods(sys);
        let ids: Vec<u32> = self.wins.iter().filter(|w| !w.min && w.dlg.is_none()).map(|w| w.id).collect();
        for id in ids {
            self.with_app(id, sys, |a, cx| a.tick(cx));
        }
        // Los diálogos repiten la flecha o la pista de su barra mientras se mantiene pulsada.
        let dlgs: Vec<u32> = self.wins.iter().filter(|w| w.dlg.is_some()).map(|w| w.id).collect();
        for id in dlgs {
            self.dlg_event(id, sys, |d, s| {
                d.tick(s);
                None
            });
        }
        if ms >= self.next_clock {
            self.next_clock = ms + 250;
            let t = sys.now();
            let key = |t: &DateTime| (t.year, t.month, t.day, t.hour, t.minute);
            if key(&t) != key(&self.clock) {
                self.clock = t;
                self.dmg(self.clock_rect());
            }
            self.login.set_clock(t);
            self.flush_login();
        }
        self.after(sys);
    }

    // ---------------------------------------------------------------- composición

    fn paint(&self, s: &mut Surface, active: Option<u32>) {
        if self.login.mode == Mode::Off {
            s.blit(&self.wall, 0, 0);
            self.paint_icons(s);
        } else {
            s.blit(&self.dim, 0, 0);
        }
        if self.login.covers() {
            self.login.paint(s, self.theme.accent);
        } else {
            for w in self.wins.iter().filter(|w| !w.min) {
                self.paint_window(s, w, Some(w.id) == active);
            }
            if self.login.mode == Mode::Off {
                self.paint_bar(s, active);
                if self.start_open {
                    self.paint_menu(s);
                }
                if self.shut_open {
                    self.paint_shut(s);
                }
                if let Some(m) = &self.menu {
                    m.menu.draw(s);
                }
            }
        }
        if !cursor_rect(self.mx, self.my).intersects(s.clip()) {
            return;
        }
        if self.cur != 0 {
            // Flecha doble centrada en el puntero: contorno negro y trazo blanco.
            let (dx, dy) = [(1.0, 0.0), (0.0, 1.0), (0.707, 0.707), (0.707, -0.707)][self.cur as usize - 1];
            let (cx, cy) = (self.mx as f32 + 0.5, self.my as f32 + 0.5);
            let p = |a: f32, b: f32| (cx + a * dx - b * dy, cy + a * dy + b * dx);
            for (t, col) in [(3.6, 0x000000), (1.4, 0xFFFFFF)] {
                s.stroke(&[p(-8.0, 0.0), p(8.0, 0.0)], t, col);
                s.stroke(&[p(4.0, -4.0), p(8.0, 0.0), p(4.0, 4.0)], t, col);
                s.stroke(&[p(-4.0, -4.0), p(-8.0, 0.0), p(-4.0, 4.0)], t, col);
            }
            return;
        }
        {
            for (j, row) in CURSOR.iter().enumerate() {
                for (i, ch) in row.bytes().enumerate() {
                    let col = match ch {
                        b'#' => 0x000000,
                        b'o' => 0xFFFFFF,
                        _ => continue,
                    };
                    s.fill_rect(Rect::new(self.mx + i as i32, self.my + j as i32, 1, 1), col);
                }
            }
        }
    }

    fn paint_icons(&self, s: &mut Surface) {
        for (i, DIcon { label: name, icon, .. }) in self.icons.iter().enumerate() {
            let c = self.icon_cell(i);
            if !c.intersects(s.clip()) {
                continue;
            }
            if self.sel == Some(i) {
                s.blend_rect(c, self.theme.accent, 110);
                s.frame(c, lighten(self.theme.accent, 70));
            }
            let (_, _, isz, iy, ty) = self.size.metrics();
            icons::draw(s, *icon, c.x + (c.w - isz) / 2, c.y + iy, isz, 0xFFFFFF);
            let name = dialogs::fit(name, 8, c.w - 6);
            let t = Rect::new(c.x, c.y + ty, c.w, font::line_h(8));
            text_in(s, Rect::new(t.x + 1, t.y + 1, t.w, t.h), &name, 8, 0x000000, Align::Center);
            text_in(s, t, &name, 8, 0xFFFFFF, Align::Center);
        }
    }

    fn paint_window(&self, s: &mut Surface, w: &Window, active: bool) {
        if !w.bounds().intersects(s.clip()) {
            return;
        }
        if !w.max {
            shadow(s, w.rect);
        }
        let tb = w.title_rect();
        s.fill_rect(tb, 0xFFFFFF);
        let ink = if active { 0x000000 } else { 0x8C8C8C };
        let icon = APPS.iter().find(|a| a.0 == w.kind).map(|a| a.2);
        if let Some(icon) = icon {
            icons::draw(s, icon, tb.x + 8, tb.y + 7, 16, 0x444444);
        }
        text_in(s, Rect::new(tb.x + 32, tb.y, tb.w - 32 - 138, TITLE_H), &w.title, 9, ink, Align::Left);
        for k in (if w.dlg.is_some() { 2 } else { 0 })..3 {
            self.paint_cap(s, w, k, active);
        }
        let c = w.content_rect();
        s.blit(&w.content, c.x, c.y);
        if !w.max {
            s.frame(w.rect, if active { self.theme.accent } else { 0x9A9A9A });
        }
    }

    fn paint_cap(&self, s: &mut Surface, w: &Window, k: i32, active: bool) {
        let r = w.cap(k);
        if !r.intersects(s.clip()) {
            return;
        }
        let hot = self.hot == Hit::Cap(w.id, k as u8);
        if hot {
            s.fill_rect(r, if k == 2 { 0xE0202E } else { 0xE5E5E5 });
        }
        let ink = if hot && k == 2 {
            0xFFFFFF
        } else if active {
            0x1A1A1A
        } else {
            0x8C8C8C
        };
        let (cx, cy) = (r.x + 23, r.y + 15);
        match k {
            0 => s.fill_rect(Rect::new(cx - 5, cy, 10, 1), ink),
            1 if !w.max => s.frame(Rect::new(cx - 5, cy - 5, 10, 10), ink),
            1 => {
                s.fill_rect(Rect::new(cx - 3, cy - 5, 8, 1), ink);
                s.fill_rect(Rect::new(cx + 4, cy - 5, 1, 8), ink);
                s.fill_rect(Rect::new(cx - 3, cy - 5, 1, 2), ink);
                s.frame(Rect::new(cx - 5, cy - 3, 8, 8), ink);
            }
            _ => {
                let (x, y) = (cx as f32, cy as f32);
                s.stroke(&[(x - 4.5, y - 4.5), (x + 4.5, y + 4.5)], 1.0, ink);
                s.stroke(&[(x + 4.5, y - 4.5), (x - 4.5, y + 4.5)], 1.0, ink);
            }
        }
    }

    fn paint_bar(&self, s: &mut Surface, active: Option<u32>) {
        let bar = Rect::new(0, self.bar_y(), self.w, BAR_H);
        if !bar.intersects(s.clip()) {
            return;
        }
        s.fill_rect(bar, BAR_BG);
        let st = self.start_rect();
        let lit = self.start_open || self.hot == Hit::Start;
        if lit {
            s.fill_rect(st, HOVER_BAR);
        }
        icons::draw(s, Icon::Logo, st.x + 14, st.y + 10, 20, if lit { lighten(self.theme.accent, 90) } else { 0xFFFFFF });
        for (i, id) in self.task_ids().into_iter().enumerate() {
            let r = self.task_rect(i);
            let Some(wi) = self.pos(id) else { continue };
            let is_active = active == Some(id);
            if is_active {
                s.fill_rect(r, ACTIVE_BTN);
            } else if self.hot == Hit::Task(id) {
                s.fill_rect(r, HOVER_BAR);
            }
            if let Some(icon) = APPS.iter().find(|a| a.0 == self.wins[wi].kind).map(|a| a.2) {
                icons::draw(s, icon, r.x + 12, r.y + 8, 24, 0xFFFFFF);
            }
            let line = if is_active { Rect::new(r.x, r.bottom() - 2, 48, 2) } else { Rect::new(r.x + 14, r.bottom() - 2, 20, 2) };
            s.fill_rect(line, self.theme.accent);
        }
        let c = self.clock_rect();
        let lh = font::line_h(8);
        let y0 = c.y + (c.h - 2 * lh) / 2;
        let t = &self.clock;
        text_in(s, Rect::new(c.x, y0, c.w, lh), &format!("{:02}:{:02}", t.hour, t.minute), 8, 0xFFFFFF, Align::Center);
        text_in(s, Rect::new(c.x, y0 + lh, c.w, lh), &format!("{}/{}/{}", t.day, t.month, t.year), 8, 0xFFFFFF, Align::Center);
        let sh = self.show_rect();
        if self.hot == Hit::Show {
            s.fill_rect(sh, HOVER_BAR);
        }
        s.fill_rect(Rect::new(sh.x, sh.y, 1, sh.h), 0x4A4A4A);
    }

    fn paint_menu(&self, s: &mut Surface) {
        let m = self.menu_rect();
        if !m.intersects(s.clip()) {
            return;
        }
        s.fill_rect(m, MENU_BG);
        s.fill_rect(Rect::new(m.x, m.y, m.w, 1), 0x383838);
        s.fill_rect(Rect::new(m.right() - 1, m.y, 1, m.h), 0x383838);
        for (r, icon, hot) in [
            (self.msettings_rect(), Icon::Settings, self.hot == Hit::MSettings),
            (self.mpower_rect(), Icon::Power, self.hot == Hit::MPower || self.shut_open),
        ] {
            if hot {
                s.fill_rect(r, ACTIVE_BTN);
            }
            icons::draw(s, icon, r.x + 12, r.y + 12, 24, 0xFFFFFF);
        }
        for (i, (_, name, icon)) in APPS.iter().enumerate() {
            let r = self.row_rect(i);
            if self.hot == Hit::Row(i) {
                s.fill_rect(r, ACTIVE_BTN);
            }
            icons::draw(s, *icon, r.x + 12, r.y + 8, 24, 0xFFFFFF);
            text_in(s, Rect::new(r.x + 50, r.y, r.w - 50, r.h), name, 9, 0xFFFFFF, Align::Left);
        }
        text_in(s, Rect::new(m.x + 300, m.y + 10, 200, 24), "Aplicaciones", 8, 0xB5B5B5, Align::Left);
        for (i, (_, name, icon)) in APPS.iter().enumerate() {
            let r = self.tile_rect(i);
            let bg = if self.hot == Hit::Tile(i) { lighten(self.theme.accent, 45) } else { self.theme.accent };
            s.fill_rect(r, bg);
            icons::draw(s, *icon, r.x + 34, r.y + 16, 32, 0xFFFFFF);
            // Los nombres largos pasan a dos líneas ("Explorador de archivos").
            let lh = font::line_h(8);
            let mut lines = dialogs::wrap(name, 8, r.w - 14);
            lines.truncate(2);
            let top = r.bottom() - 6 - lh * lines.len() as i32;
            for (j, l) in lines.iter().enumerate() {
                text_in(s, Rect::new(r.x + 8, top + j as i32 * lh, r.w - 12, lh), l, 8, 0xFFFFFF, Align::Left);
            }
        }
    }

    fn paint_shut(&self, s: &mut Surface) {
        let r = self.shut_rect();
        if !r.intersects(s.clip()) {
            return;
        }
        s.fill_rect(r, 0x2B2B2B);
        for (i, name) in ["Apagar", "Reiniciar", "Bloquear", "Cerrar sesión"].iter().take(self.shut_count()).enumerate() {
            let row = self.shut_row(i);
            if self.hot == Hit::Shut(i) {
                s.fill_rect(row, 0x4A4A4A);
            }
            text_in(s, Rect::new(row.x + 16, row.y, row.w - 16, row.h), name, 9, 0xFFFFFF, Align::Left);
        }
        s.frame(r, blend(0x2B2B2B, 0xFFFFFF, 40));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::MousePacket;
    use crate::platform::TestPlatform;

    fn setup(w: i32, h: i32) -> (Desktop, TestPlatform) {
        let mut sys = TestPlatform { size: (w as u32, h as u32), ..Default::default() };
        let mut d = Desktop::new(w, h, &mut sys);
        // Sin el flujo de arranque del disco: los avisos del sistema se prueban aparte.
        d.booted = true;
        (d, sys)
    }

    macro_rules! click {
        ($d:expr, $s:expr, $p:expr) => {{
            let p = $p;
            click_at(&mut $d, &mut $s, p)
        }};
    }

    fn click_at(d: &mut Desktop, sys: &mut TestPlatform, p: (i32, i32)) {
        d.mouse_to(p.0, p.1, sys);
        d.left_button(true, sys);
        d.left_button(false, sys);
    }

    fn center(r: Rect) -> (i32, i32) {
        (r.x + r.w / 2, r.y + r.h / 2)
    }

    fn open_all(d: &mut Desktop, sys: &mut TestPlatform) {
        for k in [AppKind::Notepad, AppKind::Calc, AppKind::Terminal] {
            d.launch(k, sys);
        }
    }

    fn kinds(d: &Desktop) -> Vec<AppKind> {
        d.wins.iter().map(|w| w.kind).collect()
    }

    fn key(key: Key, alt: bool) -> Event {
        Event::Key(KeyEvent { key, ch: None, down: true, shift: false, ctrl: false, alt })
    }

    #[test]
    fn open_apps_from_start_menu() {
        let (mut d, mut sys) = setup(1280, 800);
        click!(d, sys, center(d.start_rect()));
        assert!(d.start_is_open());
        click!(d, sys, center(d.row_rect(1)));
        assert_eq!(d.window_count(), 1);
        assert!(!d.start_is_open());
        assert_eq!(kinds(&d), [AppKind::Calc]);
        click!(d, sys, center(d.start_rect()));
        click!(d, sys, center(d.tile_rect(4)));
        assert_eq!(kinds(&d), [AppKind::Calc, AppKind::Terminal]);
        assert!(!d.start_is_open());
    }

    #[test]
    fn settings_is_single_instance() {
        let (mut d, mut sys) = setup(1280, 800);
        for _ in 0..2 {
            click!(d, sys, center(d.start_rect()));
            click!(d, sys, center(d.msettings_rect()));
        }
        assert_eq!(d.window_count(), 1);
        d.launch(AppKind::Notepad, &mut sys);
        d.launch(AppKind::Settings, &mut sys);
        assert_eq!(d.window_count(), 2);
        assert_eq!(d.wins.last().unwrap().kind, AppKind::Settings);
    }

    #[test]
    fn click_raises_window() {
        let (mut d, mut sys) = setup(1280, 800);
        open_all(&mut d, &mut sys);
        assert_eq!(d.active_id(), Some(3));
        let t = d.wins[0].title_rect();
        click!(d, sys, (t.x + 100, t.y + 10));
        assert_eq!(kinds(&d), [AppKind::Calc, AppKind::Terminal, AppKind::Notepad]);
        assert_eq!(d.active_id(), Some(1));
    }

    #[test]
    fn taskbar_minimize_restore() {
        let (mut d, mut sys) = setup(1280, 800);
        open_all(&mut d, &mut sys);
        // El botón 2 (orden de apertura) es la Calculadora, que no es la activa: la trae al frente.
        click!(d, sys, center(d.task_rect(1)));
        assert_eq!(d.wins.last().unwrap().kind, AppKind::Calc);
        // Clic en la activa: minimiza y pasa al fondo.
        click!(d, sys, center(d.task_rect(1)));
        assert!(d.wins[0].min && d.wins[0].kind == AppKind::Calc);
        assert_eq!(d.active_id(), Some(3));
        // Restaura.
        click!(d, sys, center(d.task_rect(1)));
        assert!(!d.wins.last().unwrap().min);
        assert_eq!(d.wins.last().unwrap().kind, AppKind::Calc);
        // Mostrar escritorio.
        click!(d, sys, center(d.show_rect()));
        assert!(d.wins.iter().all(|w| w.min));
        assert_eq!(d.active_id(), None);
    }

    #[test]
    fn maximize_by_double_click_and_restore() {
        let (mut d, mut sys) = setup(1280, 800);
        d.launch(AppKind::Notepad, &mut sys);
        let before = d.wins[0].rect;
        let t = d.wins[0].title_rect();
        let p = (t.x + 120, t.y + 10);
        click!(d, sys, p);
        click!(d, sys, p);
        assert!(d.wins[0].max);
        assert_eq!(d.wins[0].rect, Rect::new(0, 0, 1280, 760));
        let c = d.wins[0].content_rect();
        assert_eq!((d.wins[0].content.w, d.wins[0].content.h), (c.w, c.h));
        let t = d.wins[0].title_rect();
        let p = (t.x + 120, t.y + 10);
        click!(d, sys, p);
        click!(d, sys, p);
        assert!(!d.wins[0].max);
        assert_eq!(d.wins[0].rect, before);
    }

    #[test]
    fn close_on_release_over_button() {
        let (mut d, mut sys) = setup(1280, 800);
        open_all(&mut d, &mut sys);
        let p = center(d.wins[2].cap(2));
        d.mouse_to(p.0, p.1, &mut sys);
        d.left_button(true, &mut sys);
        assert_eq!(d.window_count(), 3, "no cierra al pulsar");
        d.mouse_to(p.0 - 200, p.1 + 200, &mut sys);
        d.left_button(false, &mut sys);
        assert_eq!(d.window_count(), 3, "soltar fuera no cierra");
        click!(d, sys, p);
        assert_eq!(d.window_count(), 2);
        sys.ms = 0;
        d.event(key(Key::F(4), true), &mut sys);
        assert_eq!(d.window_count(), 1);
        d.event(key(Key::F(4), false), &mut sys);
        assert_eq!(d.window_count(), 1, "F4 sin Alt no cierra");
    }

    #[test]
    fn drag_moves_and_clamps_top() {
        let (mut d, mut sys) = setup(1280, 800);
        d.launch(AppKind::Notepad, &mut sys);
        let r0 = d.wins[0].rect;
        let t = d.wins[0].title_rect();
        d.mouse_to(t.x + 100, t.y + 10, &mut sys);
        d.left_button(true, &mut sys);
        d.mouse_to(t.x + 160, t.y + 50, &mut sys);
        assert_eq!((d.wins[0].rect.x, d.wins[0].rect.y), (r0.x + 60, r0.y + 40));
        d.mouse_to(t.x + 160, -500i32.max(0), &mut sys);
        d.mouse_to(t.x + 160, 0, &mut sys);
        assert_eq!(d.wins[0].rect.y, 0);
        d.mouse_to(t.x + 160, 799, &mut sys);
        assert!(d.wins[0].rect.y + TITLE_H <= 800 - BAR_H);
        d.left_button(false, &mut sys);
        assert_eq!(d.drag, Drag::None);
    }

    #[test]
    fn resize_respects_min_size() {
        let (mut d, mut sys) = setup(1280, 800);
        d.launch(AppKind::Notepad, &mut sys);
        let r = d.wins[0].rect;
        let (mw, mh) = d.wins[0].app.min_size();
        d.mouse_to(r.right() - 2, r.bottom() - 2, &mut sys);
        d.left_button(true, &mut sys);
        d.mouse_to(r.x + 5, r.y + 5, &mut sys);
        let n = d.wins[0].rect;
        assert_eq!((n.w, n.h), (mw + 2, mh + TITLE_H + 2));
        assert_eq!((d.wins[0].content.w, d.wins[0].content.h), (mw, mh));
        d.mouse_to(r.right() + 40, r.bottom() + 30, &mut sys);
        assert_eq!((d.wins[0].rect.w, d.wins[0].rect.h), (r.w + 42, r.h + 32));
        d.left_button(false, &mut sys);
        // Borde izquierdo y superior.
        let r = d.wins[0].rect;
        d.mouse_to(r.x + 1, r.y + 100, &mut sys);
        d.left_button(true, &mut sys);
        d.mouse_to(r.x + 1 + 5000, r.y + 100, &mut sys);
        assert_eq!(d.wins[0].rect.w, mw + 2);
        assert_eq!(d.wins[0].rect.right(), r.right());
        d.left_button(false, &mut sys);
    }

    #[test]
    fn super_and_escape() {
        let (mut d, mut sys) = setup(1280, 800);
        d.event(key(Key::Super, false), &mut sys);
        assert!(d.start_is_open());
        d.event(key(Key::Super, false), &mut sys);
        assert!(!d.start_is_open());
        d.event(key(Key::Super, false), &mut sys);
        d.event(key(Key::Escape, false), &mut sys);
        assert!(!d.start_is_open());
        // Un clic fuera cierra el menú.
        d.event(key(Key::Super, false), &mut sys);
        click!(d, sys, (900, 300));
        assert!(!d.start_is_open());
    }

    #[test]
    fn shutdown_and_reboot() {
        let (mut d, mut sys) = setup(1280, 800);
        d.event(key(Key::Super, false), &mut sys);
        click!(d, sys, center(d.mpower_rect()));
        assert!(d.shut_open);
        click!(d, sys, center(d.shut_row(1)));
        assert!(sys.rebooted && !sys.halted && !d.shut_open);
        click!(d, sys, center(d.mpower_rect()));
        click!(d, sys, (900, 300));
        assert!(!d.shut_open && !sys.halted);
        d.event(key(Key::Super, false), &mut sys);
        click!(d, sys, center(d.mpower_rect()));
        click!(d, sys, center(d.shut_row(0)));
        assert!(sys.halted);
    }

    #[test]
    fn desktop_icon_double_click() {
        let (mut d, mut sys) = setup(1280, 800);
        let p = center(d.icon_cell(0));
        click!(d, sys, p);
        assert_eq!((d.window_count(), d.sel), (0, Some(0)));
        click!(d, sys, p);
        assert_eq!(kinds(&d), [AppKind::Notepad]);
        // Dos clics demasiado separados en el tiempo no abren.
        let p = center(d.icon_cell(1));
        d.event(Event::Tick(1000), &mut sys);
        click!(d, sys, p);
        d.event(Event::Tick(2000), &mut sys);
        click!(d, sys, p);
        assert_eq!(d.window_count(), 1);
    }

    #[test]
    fn render_zones_and_idle() {
        let (mut d, mut sys) = setup(1280, 800);
        let z = d.render(&mut sys);
        assert_eq!(z, [Rect::new(0, 0, 1280, 800)]);
        assert!(d.render(&mut sys).is_empty());
        d.event(Event::Mouse(MousePacket { dx: 5000, dy: 5000, buttons: 0, dz: 0 }), &mut sys);
        let z = d.render(&mut sys);
        assert!(!z.is_empty() && z.len() <= 6);
        assert!(z.iter().all(|r| !r.is_empty() && Rect::new(0, 0, 1280, 800).intersect(*r) == *r));
        d.event(Event::Tick(10), &mut sys);
        assert!(d.render(&mut sys).is_empty());
        // El reloj solo daña su zona.
        d.launch(AppKind::Calc, &mut sys);
        d.render(&mut sys);
        d.clock.minute = 0;
        d.event(Event::Tick(500), &mut sys);
        let z = d.render(&mut sys);
        assert_eq!(z, [d.clock_rect()]);
    }

    /// Imagen tras un redibujado completo, sin perder el estado.
    fn full_image(d: &mut Desktop, sys: &mut TestPlatform) -> Vec<u32> {
        d.invalidate();
        d.render(sys);
        d.back().px.clone()
    }

    #[test]
    fn incremental_equals_full_redraw() {
        let (mut d, mut sys) = setup(1280, 800);
        d.render(&mut sys);
        let check = |d: &mut Desktop, sys: &mut TestPlatform, what: &str| {
            d.render(sys);
            let inc = d.back().px.clone();
            assert!(inc == full_image(d, sys), "incremental distinto del completo: {what}");
        };
        // Barrido del ratón sobre iconos, barra y escritorio.
        for i in 0..60 {
            d.mouse_to(20 + i * 20, 10 + i * 12, &mut sys);
            d.render(&mut sys);
        }
        check(&mut d, &mut sys, "ratón");
        click!(d, sys, center(d.icon_cell(2)));
        check(&mut d, &mut sys, "icono");
        d.event(key(Key::Super, false), &mut sys);
        d.mouse_to(330, 500, &mut sys);
        check(&mut d, &mut sys, "menú");
        click!(d, sys, center(d.mpower_rect()));
        d.mouse_to(40, 640, &mut sys);
        check(&mut d, &mut sys, "apagado");
        d.event(key(Key::Escape, false), &mut sys);
        open_all(&mut d, &mut sys);
        check(&mut d, &mut sys, "ventanas");
        // Arrastre de la ventana activa con render en cada paso.
        let t = d.wins[2].title_rect();
        d.mouse_to(t.x + 200, t.y + 10, &mut sys);
        d.left_button(true, &mut sys);
        for i in 1..=25 {
            d.mouse_to(t.x + 200 + i * 17, t.y + 10 + i * 9 - i * i / 3, &mut sys);
            d.render(&mut sys);
        }
        d.left_button(false, &mut sys);
        check(&mut d, &mut sys, "arrastre");
        // Redimensionado por la esquina inferior derecha y por la superior izquierda.
        let r = d.wins[2].rect;
        d.mouse_to(r.right() - 2, r.bottom() - 2, &mut sys);
        d.left_button(true, &mut sys);
        for i in 1..=20 {
            d.mouse_to(r.right() - 2 + i * 9, r.bottom() - 2 - i * 5, &mut sys);
            d.render(&mut sys);
        }
        d.left_button(false, &mut sys);
        let r = d.wins[2].rect;
        d.mouse_to(r.x + 1, r.y + 1, &mut sys);
        d.left_button(true, &mut sys);
        for i in 1..=15 {
            d.mouse_to(r.x + 1 + i * 6, r.y + 1 + i * 4, &mut sys);
            d.render(&mut sys);
        }
        d.left_button(false, &mut sys);
        check(&mut d, &mut sys, "redimensionado");
        // Botones de la barra de título, minimizar/restaurar, maximizar y cerrar.
        let p = center(d.wins[2].cap(1));
        d.mouse_to(p.0, p.1, &mut sys);
        check(&mut d, &mut sys, "hover maximizar");
        click!(d, sys, p);
        check(&mut d, &mut sys, "maximizada");
        let p = center(d.wins[2].cap(1));
        click!(d, sys, p);
        click!(d, sys, center(d.task_rect(0)));
        check(&mut d, &mut sys, "foco por barra");
        click!(d, sys, center(d.show_rect()));
        check(&mut d, &mut sys, "mostrar escritorio");
        click!(d, sys, center(d.task_rect(1)));
        let p = center(d.wins.last().unwrap().cap(2));
        d.mouse_to(p.0, p.1, &mut sys);
        check(&mut d, &mut sys, "hover cerrar");
        click!(d, sys, p);
        check(&mut d, &mut sys, "cerrar");
    }

    #[test]
    fn other_screen_sizes_do_not_panic() {
        for (w, h) in [(640, 480), (1920, 1080), (320, 200)] {
            let (mut d, mut sys) = setup(w, h);
            d.render(&mut sys);
            for k in [AppKind::Notepad, AppKind::Calc, AppKind::Settings, AppKind::Terminal] {
                d.launch(k, &mut sys);
                d.render(&mut sys);
            }
            d.event(key(Key::Super, false), &mut sys);
            d.mouse_to(5, h - 10, &mut sys);
            d.render(&mut sys);
            d.event(key(Key::Super, false), &mut sys);
            let t = d.wins[3].title_rect();
            d.mouse_to(t.x + 40, t.y + 5, &mut sys);
            d.left_button(true, &mut sys);
            d.mouse_to(w - 1, h - 1, &mut sys);
            d.render(&mut sys);
            d.mouse_to(0, 0, &mut sys);
            d.left_button(false, &mut sys);
            let r = d.wins[3].rect;
            d.mouse_to(r.right() - 1, r.bottom() - 1, &mut sys);
            d.left_button(true, &mut sys);
            d.mouse_to(0, 0, &mut sys);
            d.mouse_to(w - 1, h - 1, &mut sys);
            d.left_button(false, &mut sys);
            d.render(&mut sys);
            let id = d.wins[3].id;
            d.toggle_max(id);
            d.render(&mut sys);
            for z in d.render(&mut sys) {
                assert!(Rect::new(0, 0, w, h).intersect(z) == z);
            }
            assert_eq!((d.back().w, d.back().h), (w, h));
        }
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::*;
    use crate::platform::TestPlatform;

    #[test]
    fn resize_cursor_follows_the_edge() {
        assert_eq!(resize_cursor(1), 1);
        assert_eq!(resize_cursor(2), 1);
        assert_eq!(resize_cursor(4), 2);
        assert_eq!(resize_cursor(8), 2);
        assert_eq!(resize_cursor(1 | 4), 3);
        assert_eq!(resize_cursor(2 | 8), 3);
        assert_eq!(resize_cursor(2 | 4), 4);
        assert_eq!(resize_cursor(1 | 8), 4);
    }

    #[test]
    fn pointer_changes_on_edges_and_while_resizing() {
        let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
        let mut d = Desktop::new(1280, 800, &mut sys);
        d.launch(AppKind::Notepad, &mut sys);
        let r = d.window_rects()[0];
        d.mouse_to(r.x + r.w / 2, r.y + r.h / 2, &mut sys);
        assert_eq!(d.cur, 0);
        d.mouse_to(r.right() - 2, r.y + r.h / 2, &mut sys);
        assert_eq!(d.cur, 1);
        d.mouse_to(r.right() - 2, r.bottom() - 2, &mut sys);
        assert_eq!(d.cur, 3);
        // Durante el arrastre se mantiene aunque el puntero se aleje del borde.
        d.left_button(true, &mut sys);
        d.mouse_to(r.right() + 80, r.bottom() + 60, &mut sys);
        assert_eq!(d.cur, 3);
        assert!(d.window_rects()[0].w > r.w);
        d.left_button(false, &mut sys);
        d.mouse_to(600, 700, &mut sys);
        assert_eq!(d.cur, 0);
    }
}

#[cfg(test)]
mod modal_tests {
    use super::*;
    use crate::block::RamDisk;
    use crate::dialogs::Buttons;
    use crate::files::Files;
    use crate::platform::TestPlatform;
    use crate::theme::ACCENTS;
    use alloc::rc::Rc;
    use core::cell::{Cell, RefCell};

    /// App de pruebas: Enter pide `req`, 'o' pide abrir `open`, y si `dirty` no se deja cerrar.
    struct Probe {
        req: Option<DialogRequest>,
        open: Option<String>,
        results: Rc<RefCell<Vec<DialogResult>>>,
        downs: Rc<Cell<u32>>,
        dirty: bool,
    }

    type Log = (Rc<RefCell<Vec<DialogResult>>>, Rc<Cell<u32>>);

    fn probe(req: Option<DialogRequest>, dirty: bool) -> (Box<Probe>, Log) {
        let log: Log = (Rc::new(RefCell::new(Vec::new())), Rc::new(Cell::new(0)));
        let p = Probe { req, open: None, results: log.0.clone(), downs: log.1.clone(), dirty };
        (Box::new(p), log)
    }

    impl App for Probe {
        fn kind(&self) -> AppKind {
            AppKind::Notepad
        }
        fn title(&self) -> String {
            String::from("Probe")
        }
        fn min_size(&self) -> (i32, i32) {
            (100, 100)
        }
        fn draw(&mut self, s: &mut Surface, _theme: &Theme, _sys: &mut dyn Platform, _focused: bool) {
            let b = s.bounds();
            s.fill_rect(b, 0xEEEEEE);
        }
        fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
            match ev.key {
                Key::Enter => cx.dialog = self.req.clone(),
                Key::Char('o') => cx.open = self.open.clone(),
                _ => {}
            }
        }
        fn mouse(&mut self, ev: MouseEv, _cx: &mut Ctx) {
            if matches!(ev, MouseEv::Down { .. }) {
                self.downs.set(self.downs.get() + 1);
            }
        }
        fn tick(&mut self, _cx: &mut Ctx) {}
        fn dialog_result(&mut self, r: DialogResult, cx: &mut Ctx) {
            if let DialogResult::Answer { tag: 9, answer } = r {
                if answer == Answer::Yes {
                    cx.close = true;
                }
            }
            self.results.borrow_mut().push(r);
        }
        fn request_close(&mut self, cx: &mut Ctx) -> bool {
            if self.dirty {
                cx.dialog = Some(DialogRequest::Message {
                    tag: 9,
                    title: String::from("Cerrar"),
                    text: String::from("¿Cerrar sin guardar?"),
                    buttons: Buttons::YesNo,
                });
                return false;
            }
            true
        }
    }

    fn setup() -> (Desktop, TestPlatform) {
        let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
        let mut d = Desktop::new(1280, 800, &mut sys);
        d.booted = true;
        (d, sys)
    }

    fn msg(tag: u32, buttons: Buttons) -> DialogRequest {
        DialogRequest::Message { tag, title: String::from("T"), text: String::from("Hola"), buttons }
    }

    fn key(d: &mut Desktop, sys: &mut TestPlatform, key: Key, ch: Option<char>, alt: bool) {
        d.event(Event::Key(KeyEvent { key, ch, down: true, shift: false, ctrl: false, alt }), sys);
    }

    fn click_at(d: &mut Desktop, sys: &mut TestPlatform, p: (i32, i32)) {
        d.mouse_to(p.0, p.1, sys);
        d.left_button(true, sys);
        d.left_button(false, sys);
    }

    fn kinds(d: &Desktop) -> Vec<AppKind> {
        d.wins.iter().filter(|w| w.dlg.is_none()).map(|w| w.kind).collect()
    }

    fn center(r: Rect) -> (i32, i32) {
        (r.x + r.w / 2, r.y + r.h / 2)
    }

    #[test]
    fn modal_blocks_owner_and_returns_result() {
        let (mut d, mut sys) = setup();
        let (p, (results, downs)) = probe(Some(msg(4, Buttons::YesNo)), false);
        d.launch_app(p, &mut sys);
        key(&mut d, &mut sys, Key::Enter, None, false);
        assert_eq!((d.dialog_count(), d.window_count()), (1, 2));
        // El diálogo queda encima de su dueña y es la ventana activa.
        assert!(d.wins[1].dlg.is_some() && d.wins[1].owner == Some(d.wins[0].id));
        // Un clic sobre la dueña no le llega: enfoca el diálogo.
        let c = d.wins[0].content_rect();
        click_at(&mut d, &mut sys, (c.x + 30, c.y + 30));
        assert_eq!(downs.get(), 0);
        assert!(d.wins.last().unwrap().dlg.is_some());
        // Tampoco las teclas: van al diálogo (Enter = Sí).
        key(&mut d, &mut sys, Key::Enter, None, false);
        assert_eq!(d.dialog_count(), 0);
        assert_eq!(*results.borrow(), [DialogResult::Answer { tag: 4, answer: Answer::Yes }]);
        // Sin diálogo, la dueña vuelve a recibir clics.
        click_at(&mut d, &mut sys, (c.x + 30, c.y + 30));
        assert_eq!(downs.get(), 1);
    }

    #[test]
    fn closing_owner_closes_its_dialog_and_taskbar_ignores_dialogs() {
        let (mut d, mut sys) = setup();
        let (p, _) = probe(Some(msg(1, Buttons::Ok)), false);
        d.launch_app(p, &mut sys);
        key(&mut d, &mut sys, Key::Enter, None, false);
        assert_eq!(d.task_ids().len(), 1);
        let id = d.wins[0].id;
        d.close(id);
        assert_eq!((d.window_count(), d.dialog_count()), (0, 0));
        // Minimizar la dueña minimiza también el diálogo; restaurar los trae a los dos.
        let (p, _) = probe(Some(msg(1, Buttons::Ok)), false);
        d.launch_app(p, &mut sys);
        key(&mut d, &mut sys, Key::Enter, None, false);
        let id = d.wins.iter().find(|w| w.dlg.is_none()).unwrap().id;
        d.minimize(id);
        assert!(d.wins.iter().all(|w| w.min));
        d.raise(id);
        assert!(d.wins.iter().all(|w| !w.min) && d.wins.last().unwrap().dlg.is_some());
    }

    #[test]
    fn request_close_false_keeps_window() {
        let (mut d, mut sys) = setup();
        let (p, (results, _)) = probe(None, true);
        d.launch_app(p, &mut sys);
        // Alt+F4: la app pide confirmación y no se cierra.
        key(&mut d, &mut sys, Key::F(4), None, true);
        assert_eq!((kinds(&d).len(), d.dialog_count()), (1, 1));
        // No (Escape): sigue abierta y sin diálogo.
        key(&mut d, &mut sys, Key::Escape, None, false);
        assert_eq!((kinds(&d).len(), d.dialog_count()), (1, 0));
        assert_eq!(*results.borrow(), [DialogResult::Answer { tag: 9, answer: Answer::No }]);
        // El aspa hace lo mismo; con Sí la app pone `cx.close`.
        let p = center(d.wins[0].cap(2));
        click_at(&mut d, &mut sys, p);
        assert_eq!(d.dialog_count(), 1);
        key(&mut d, &mut sys, Key::Enter, None, false);
        assert_eq!(d.window_count(), 0);
        // Sin cambios pendientes se cierra directamente.
        let (p, _) = probe(None, false);
        d.launch_app(p, &mut sys);
        key(&mut d, &mut sys, Key::F(4), None, true);
        assert_eq!(d.window_count(), 0);
    }

    #[test]
    fn system_dialog_without_owner() {
        let (mut d, mut sys) = setup();
        d.request_dialog(msg(77, Buttons::OkCancel), &mut sys);
        assert_eq!((d.dialog_count(), d.window_count()), (1, 1));
        assert!(d.wins[0].owner.is_none() && d.task_ids().is_empty());
        let r = d.wins[0].rect;
        assert!(r.x > 0 && r.y > 0 && r.right() < 1280);
        // El aspa del diálogo lo cierra; los botones de minimizar/maximizar no existen.
        assert_eq!(d.hit(r.right() - 20, r.y + 10), Hit::Cap(d.wins[0].id, 2));
        assert!(matches!(d.hit(r.right() - 100, r.y + 10), Hit::Title(_)));
        let p = center(d.wins[0].cap(2));
        click_at(&mut d, &mut sys, p);
        assert_eq!(d.window_count(), 0);
    }

    fn disk_platform() -> TestPlatform {
        let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
        let now = sys.now();
        sys.files = Files::probe(Some(Box::new(RamDisk::new(256))), now);
        sys
    }

    #[test]
    fn new_disk_yes_formats_and_saves_config() {
        let mut sys = disk_platform();
        assert!(sys.files.pending_format());
        let mut d = Desktop::new(1280, 800, &mut sys);
        d.event(Event::Tick(10), &mut sys);
        assert_eq!(d.dialog_count(), 1);
        assert!(sys.files.pending_format(), "no se formatea sin respuesta");
        d.event(Event::Tick(20), &mut sys);
        assert_eq!(d.dialog_count(), 1, "el flujo solo ocurre una vez");
        key(&mut d, &mut sys, Key::Enter, None, false); // Sí
        assert!(!sys.files.pending_format() && sys.files.persistent());
        assert_eq!(d.dialog_count(), 0, "formateado: no hay aviso de memoria");
        assert!(sys.files.read(files::CONFIG_FILE).is_ok());
    }

    #[test]
    fn new_disk_no_never_formats_and_then_notifies() {
        let mut sys = disk_platform();
        let mut d = Desktop::new(1280, 800, &mut sys);
        d.event(Event::Tick(10), &mut sys);
        key(&mut d, &mut sys, Key::Escape, None, false); // No
        assert!(!sys.files.pending_format() && !sys.files.persistent());
        // Aviso de almacenamiento, una sola vez.
        assert_eq!(d.dialog_count(), 1);
        assert_eq!(d.wins[0].dlg.as_ref().unwrap().title(), "Almacenamiento");
        key(&mut d, &mut sys, Key::Enter, None, false);
        assert_eq!(d.dialog_count(), 0);
        d.event(Event::Tick(30), &mut sys);
        assert_eq!(d.dialog_count(), 0);
        // Con el aspa tampoco formatea.
        let mut sys = disk_platform();
        let mut d = Desktop::new(1280, 800, &mut sys);
        d.event(Event::Tick(10), &mut sys);
        let p = center(d.wins[0].cap(2));
        click_at(&mut d, &mut sys, p);
        assert!(!sys.files.persistent() && !sys.files.pending_format());
    }

    #[test]
    fn notice_without_disk_shown_once() {
        let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
        let mut d = Desktop::new(1280, 800, &mut sys);
        d.event(Event::Tick(10), &mut sys);
        assert_eq!(d.dialog_count(), 1);
        key(&mut d, &mut sys, Key::Enter, None, false);
        d.event(Event::Tick(20), &mut sys);
        assert_eq!(d.dialog_count(), 0);
    }

    #[test]
    fn config_is_loaded_validated_and_saved() {
        let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
        let now = sys.now();
        sys.files.write(files::CONFIG_FILE, b"fondo=2\nenfasis=D13B3B\n", now).unwrap();
        let d = Desktop::new(1280, 800, &mut sys);
        assert_eq!((d.theme.wallpaper, d.theme.accent), (2, 0xD13B3B));
        // Valores no válidos se ignoran.
        sys.files.write(files::CONFIG_FILE, b"fondo=9\nenfasis=zz\nbasura\n", now).unwrap();
        let d = Desktop::new(1280, 800, &mut sys);
        assert_eq!(d.theme, Theme::default());
        // Guardado al cambiar el tema desde una app.
        let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
        let mut d = Desktop::new(1280, 800, &mut sys);
        d.launch(AppKind::Settings, &mut sys);
        let id = d.wins[0].id;
        d.with_app(id, &mut sys, |_, cx| {
            cx.theme.accent = ACCENTS[3];
            cx.theme.wallpaper = 1;
            cx.theme_changed = true;
        });
        assert_eq!(sys.files.read(files::CONFIG_FILE).unwrap(), b"fondo=1\nenfasis=6B8E23\niconos=mediano\norden=nombre\n");
        let d2 = Desktop::new(1280, 800, &mut sys);
        assert_eq!((d2.theme.wallpaper, d2.theme.accent), (1, ACCENTS[3]));
    }

    #[test]
    fn desktop_folder_items_become_icons() {
        let mut sys = TestPlatform { size: (1280, 400), ..Default::default() };
        let now = sys.now();
        sys.files.mkdir("C:\\Escritorio\\Carpeta", now).unwrap();
        sys.files.write("C:\\Escritorio\\nota.txt", b"hola", now).unwrap();
        sys.files.write("C:\\Escritorio\\datos.bin", b"1", now).unwrap();
        let mut d = Desktop::new(1280, 400, &mut sys);
        d.booted = true;
        let names: Vec<&str> = d.icons.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names[..6], ["Bloc de notas", "Calculadora", "Configuración", "Explorador de archivos", "Terminal", "Papelera"]);
        assert_eq!(names[6..], ["Carpeta", "datos.bin", "nota.txt"]);
        assert_eq!([d.icons[5].icon, d.icons[6].icon, d.icons[7].icon, d.icons[8].icon], [Icon::Trash, Icon::Folder, Icon::FileBlank, Icon::FileText]);
        // Columnas: con 400 px de alto caben 4 por columna.
        assert_eq!((d.icon_cell(3).x, d.icon_cell(4).x, d.icon_cell(4).y), (6, 90, 6));
        // Se refrescan al cambiar los archivos.
        sys.files.write("C:\\Escritorio\\z.txt", b"z", now).unwrap();
        d.event(Event::Tick(10), &mut sys);
        assert_eq!(d.icons.len(), 10);
        sys.files.delete("C:\\Escritorio\\z.txt").unwrap();
        d.event(Event::Tick(20), &mut sys);
        assert_eq!(d.icons.len(), 9);
        // Doble clic: carpeta y Papelera abren el Explorador; el archivo, el Bloc de notas.
        let dbl = |d: &mut Desktop, sys: &mut TestPlatform, i: usize, t: u64| {
            d.event(Event::Tick(t), sys);
            let p = center(d.icon_cell(i));
            click_at(d, sys, p);
            click_at(d, sys, p);
        };
        dbl(&mut d, &mut sys, 8, 1000);
        assert_eq!(kinds(&d), [AppKind::Notepad]);
        // Las ventanas ya tapan los iconos: se activan directamente.
        d.activate_icon(6, &mut sys);
        assert_eq!(kinds(&d), [AppKind::Notepad, AppKind::Files]);
        d.activate_icon(5, &mut sys);
        assert_eq!(kinds(&d), [AppKind::Notepad, AppKind::Files], "se reutiliza el Explorador");
    }

    #[test]
    fn cx_open_routes_folders_and_files() {
        let (mut d, mut sys) = setup();
        let (mut p, _) = probe(None, false);
        p.open = Some(String::from("C:\\Documentos"));
        d.launch_app(p, &mut sys);
        key(&mut d, &mut sys, Key::Char('o'), Some('o'), false);
        assert_eq!(kinds(&d), [AppKind::Notepad, AppKind::Files]);
        let now = sys.now();
        sys.files.write("C:\\Documentos\\a.txt", b"a", now).unwrap();
        let (mut p, _) = probe(None, false);
        p.open = Some(String::from("C:\\Documentos\\a.txt"));
        d.launch_app(p, &mut sys);
        key(&mut d, &mut sys, Key::Char('o'), Some('o'), false);
        assert_eq!(kinds(&d), [AppKind::Notepad, AppKind::Files, AppKind::Notepad, AppKind::Notepad]);
        let (mut p, _) = probe(None, false);
        p.open = Some(String::from(files::TRASH));
        d.launch_app(p, &mut sys);
        key(&mut d, &mut sys, Key::Char('o'), Some('o'), false);
        assert_eq!(kinds(&d).iter().filter(|k| **k == AppKind::Files).count(), 1);
    }

    #[test]
    fn start_menu_tiles_fit_their_names() {
        let (d, _) = setup();
        for (i, (_, name, _)) in APPS.iter().enumerate() {
            let r = d.tile_rect(i);
            let lines = dialogs::wrap(name, 8, r.w - 14);
            assert!(lines.len() <= 2, "{name}");
            assert!(lines.iter().all(|l| font::width(l, 8, false) <= r.w - 12), "{name}");
            let top = r.bottom() - 6 - font::line_h(8) * lines.len() as i32;
            assert!(top >= r.y + 16 + 32, "{name} pisa el icono");
        }
    }

    #[test]
    fn incremental_equals_full_with_dialogs() {
        let (mut d, mut sys) = setup();
        let now = sys.now();
        sys.files.write("C:\\Documentos\\a.txt", b"a", now).unwrap();
        d.render(&mut sys);
        let check = |d: &mut Desktop, sys: &mut TestPlatform, what: &str| {
            d.render(sys);
            let inc = d.back().px.clone();
            d.invalidate();
            d.render(sys);
            assert!(inc == d.back().px, "incremental distinto del completo: {what}");
        };
        let req = DialogRequest::Save { tag: 2, title: String::from("Guardar como"), dir: String::from("C:\\Documentos"), name: String::from("x") };
        let (p, _) = probe(Some(req), false);
        d.launch_app(p, &mut sys);
        check(&mut d, &mut sys, "dueña");
        key(&mut d, &mut sys, Key::Enter, None, false);
        check(&mut d, &mut sys, "diálogo");
        key(&mut d, &mut sys, Key::Char('q'), Some('q'), false);
        check(&mut d, &mut sys, "texto");
        key(&mut d, &mut sys, Key::Tab, None, false);
        key(&mut d, &mut sys, Key::Down, None, false);
        check(&mut d, &mut sys, "lista");
        // Arrastre del diálogo por su barra de título.
        let t = d.wins[1].title_rect();
        d.mouse_to(t.x + 100, t.y + 10, &mut sys);
        d.left_button(true, &mut sys);
        for i in 1..=10 {
            d.mouse_to(t.x + 100 + i * 12, t.y + 10 + i * 7, &mut sys);
            d.render(&mut sys);
        }
        d.left_button(false, &mut sys);
        check(&mut d, &mut sys, "arrastre");
        // Reemplazo, y cierre.
        key(&mut d, &mut sys, Key::Enter, None, false);
        check(&mut d, &mut sys, "reemplazar");
        key(&mut d, &mut sys, Key::Escape, None, false);
        key(&mut d, &mut sys, Key::Escape, None, false);
        check(&mut d, &mut sys, "cerrado");
        assert_eq!(d.dialog_count(), 0);
    }

    /// App de pruebas del menú: clic derecho pide un menú, 'n' pide abrir una carpeta en ventana nueva.
    struct MenuProbe {
        log: Rc<RefCell<Vec<String>>>,
    }

    impl App for MenuProbe {
        fn kind(&self) -> AppKind {
            AppKind::Notepad
        }
        fn title(&self) -> String {
            String::from("MenuProbe")
        }
        fn min_size(&self) -> (i32, i32) {
            (100, 100)
        }
        fn draw(&mut self, s: &mut Surface, _theme: &Theme, _sys: &mut dyn Platform, _focused: bool) {
            let b = s.bounds();
            s.fill_rect(b, 0xEEEEEE);
        }
        fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
            if ev.key == Key::Char('n') {
                cx.open_new = Some(String::from("C:\\Documentos"));
            }
        }
        fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
            match ev {
                MouseEv::Down { .. } => self.log.borrow_mut().push(String::from("down")),
                MouseEv::Wheel { dz, .. } => self.log.borrow_mut().push(format!("rueda {dz}")),
                MouseEv::RightDown { x, y } => {
                    self.log.borrow_mut().push(format!("derecho {x},{y}"));
                    let sub = vec![MenuItem::new(20, "Uno").checked(true), MenuItem::new(21, "Dos")];
                    cx.menu = Some(crate::menu::MenuRequest {
                        tag: 7,
                        x,
                        y,
                        items: vec![MenuItem::submenu("Ver", sub), MenuItem::new(1, "Actualizar"), MenuItem::sep(), MenuItem::new(2, "Pegar").disabled(true)],
                    });
                }
                _ => {}
            }
        }
        fn tick(&mut self, _cx: &mut Ctx) {}
        fn menu_result(&mut self, tag: u32, id: u32, _cx: &mut Ctx) {
            self.log.borrow_mut().push(format!("menu {tag} {id}"));
        }
    }

    fn menu_app(d: &mut Desktop, sys: &mut TestPlatform) -> Rc<RefCell<Vec<String>>> {
        let log = Rc::new(RefCell::new(Vec::new()));
        d.launch_app(Box::new(MenuProbe { log: log.clone() }), sys);
        log
    }

    fn right_click(d: &mut Desktop, sys: &mut TestPlatform, p: (i32, i32)) {
        d.mouse_to(p.0, p.1, sys);
        d.right_button(true, sys);
        d.right_button(false, sys);
    }

    #[test]
    fn context_menu_opens_picks_and_closes() {
        let (mut d, mut sys) = setup();
        let log = menu_app(&mut d, &mut sys);
        let c = d.wins[0].content_rect();
        right_click(&mut d, &mut sys, (c.x + 40, c.y + 50));
        assert_eq!(log.borrow()[0], "derecho 40,50");
        let m = d.menu().expect("menú abierto");
        assert_eq!((m.rect().x, m.rect().y), (c.x + 40, c.y + 50));
        // Elegir: la app recibe tag e id y el menú se cierra.
        let r = m.item_rect(1);
        click_at(&mut d, &mut sys, center(r));
        assert!(d.menu().is_none());
        assert_eq!(log.borrow()[1], "menu 7 1");
        // Esc cierra sin elegir; con flechas y Enter se elige en el submenú.
        right_click(&mut d, &mut sys, (c.x + 40, c.y + 50));
        key(&mut d, &mut sys, Key::Escape, None, false);
        assert!(d.menu().is_none());
        right_click(&mut d, &mut sys, (c.x + 40, c.y + 50));
        key(&mut d, &mut sys, Key::Down, None, false);
        key(&mut d, &mut sys, Key::Right, None, false);
        key(&mut d, &mut sys, Key::Down, None, false);
        key(&mut d, &mut sys, Key::Enter, None, false);
        assert!(d.menu().is_none());
        assert_eq!(log.borrow().last().unwrap(), "menu 7 21");
        // Con el menú abierto el teclado no llega a la app.
        right_click(&mut d, &mut sys, (c.x + 40, c.y + 50));
        key(&mut d, &mut sys, Key::Char('n'), Some('n'), false);
        assert_eq!(d.window_count(), 1);
    }

    #[test]
    fn context_menu_outside_click_only_closes() {
        let (mut d, mut sys) = setup();
        let log = menu_app(&mut d, &mut sys);
        let c = d.wins[0].content_rect();
        right_click(&mut d, &mut sys, (c.x + 40, c.y + 50));
        let n = log.borrow().len();
        // Clic fuera, aunque sea sobre la propia ventana: no llega a la app.
        click_at(&mut d, &mut sys, (c.x + 300, c.y + 200));
        assert!(d.menu().is_none());
        assert_eq!(log.borrow().len(), n);
        // Clic sobre un desactivado no lo cierra ni elige.
        right_click(&mut d, &mut sys, (c.x + 40, c.y + 50));
        let r = d.menu().unwrap().item_rect(3);
        click_at(&mut d, &mut sys, center(r));
        assert!(d.menu().is_some());
        assert_eq!(log.borrow().len(), n + 1);
        // Botón derecho fuera: cierra y no abre otro.
        right_click(&mut d, &mut sys, (c.x + 300, c.y + 200));
        assert!(d.menu().is_none());
        assert_eq!(log.borrow().len(), n + 1);
    }

    #[test]
    fn context_menu_fits_screen_and_follows_owner() {
        let (mut d, mut sys) = setup();
        menu_app(&mut d, &mut sys);
        d.set_rect(0, Rect::new(1000, 560, 270, 190));
        let c = d.wins[0].content_rect();
        right_click(&mut d, &mut sys, (c.x + 200, c.bottom() - 20));
        let m = d.menu().unwrap();
        assert!(m.rect().right() <= 1280 && m.rect().bottom() <= 800 - BAR_H);
        // Si la ventana dueña se cierra, el menú también.
        let id = d.wins[0].id;
        d.close(id);
        assert!(d.menu().is_none());
        // Y si se minimiza.
        menu_app(&mut d, &mut sys);
        let c = d.wins[0].content_rect();
        right_click(&mut d, &mut sys, (c.x + 20, c.y + 20));
        assert!(d.menu().is_some());
        let id = d.wins[0].id;
        d.minimize(id);
        assert!(d.menu().is_none());
    }

    #[test]
    fn right_click_only_on_content_and_wheel_goes_under_pointer() {
        let (mut d, mut sys) = setup();
        let log1 = menu_app(&mut d, &mut sys);
        let log2 = menu_app(&mut d, &mut sys);
        d.set_rect(0, Rect::new(20, 20, 400, 300));
        d.set_rect(1, Rect::new(600, 100, 400, 300));
        let (c0, c1) = (d.wins[0].content_rect(), d.wins[1].content_rect());
        // Título, barra de tareas y fondo: nada.
        let t = d.wins[1].title_rect();
        right_click(&mut d, &mut sys, (t.x + 50, t.y + 5));
        right_click(&mut d, &mut sys, (600, 790));
        assert!(d.menu().is_none() && log2.borrow().is_empty());
        // El fondo sí abre el menú del escritorio (no llega a las apps); se cierra.
        right_click(&mut d, &mut sys, (500, 500));
        assert!(d.menu().is_some() && log2.borrow().is_empty());
        d.close_menu();
        // Rueda sobre la ventana de atrás (sin foco) y sobre la activa.
        d.mouse_to(c0.x + 10, c0.y + 10, &mut sys);
        d.event(Event::Mouse(crate::input::MousePacket { dx: 0, dy: 0, buttons: 0, dz: -2 }), &mut sys);
        assert_eq!(log1.borrow().as_slice(), ["rueda -2"]);
        d.mouse_to(c1.x + 10, c1.y + 10, &mut sys);
        d.wheel(3, &mut sys);
        assert_eq!(log2.borrow().as_slice(), ["rueda 3"]);
        // Botón derecho por evento de ratón: enfoca la de atrás y le llega.
        d.mouse_to(c0.x + 10, c0.y + 10, &mut sys);
        d.event(Event::Mouse(crate::input::MousePacket { dx: 0, dy: 0, buttons: 2, dz: 0 }), &mut sys);
        assert_eq!(log1.borrow()[1], "derecho 10,10");
        assert_eq!(d.active_id(), Some(d.wins[1].id));
        assert!(d.menu().is_some());
        // La rueda no llega a nadie con el menú abierto.
        d.wheel(1, &mut sys);
        assert_eq!(log1.borrow().len(), 2);
    }

    #[test]
    fn right_click_ignored_on_blocked_window() {
        let (mut d, mut sys) = setup();
        let (p, _) = probe(Some(msg(4, Buttons::Ok)), false);
        d.launch_app(p, &mut sys);
        key(&mut d, &mut sys, Key::Enter, None, false);
        let c = d.wins[0].content_rect();
        right_click(&mut d, &mut sys, (c.x + 5, c.y + 5));
        assert!(d.menu().is_none());
    }

    #[test]
    fn open_new_always_opens_a_new_explorer() {
        let (mut d, mut sys) = setup();
        menu_app(&mut d, &mut sys);
        d.launch(AppKind::Files, &mut sys);
        let explorers = |d: &Desktop| d.wins.iter().filter(|w| w.kind == AppKind::Files).count();
        assert_eq!(explorers(&d), 1);
        let id = d.wins[0].id;
        d.with_app(id, &mut sys, |_, cx| cx.open_new = Some(String::from("C:\\Documentos")));
        assert_eq!(explorers(&d), 2);
        d.with_app(id, &mut sys, |_, cx| cx.open_new = Some(String::from("C:\\Documentos")));
        assert_eq!(explorers(&d), 3);
    }

    #[test]
    fn incremental_equals_full_with_context_menu() {
        let (mut d, mut sys) = setup();
        menu_app(&mut d, &mut sys);
        d.render(&mut sys);
        let check = |d: &mut Desktop, sys: &mut TestPlatform, what: &str| {
            d.render(sys);
            let inc = d.back().px.clone();
            d.invalidate();
            d.render(sys);
            assert!(inc == d.back().px, "incremental distinto del completo: {what}");
        };
        let c = d.wins[0].content_rect();
        right_click(&mut d, &mut sys, (c.x + 60, c.y + 60));
        check(&mut d, &mut sys, "abrir");
        let r = d.menu().unwrap().item_rect(1);
        d.mouse_to(r.x + 10, r.y + 5, &mut sys);
        check(&mut d, &mut sys, "resaltar");
        let r = d.menu().unwrap().item_rect(0);
        d.mouse_to(r.x + 10, r.y + 5, &mut sys);
        assert!(d.menu().unwrap().sub_rect().is_some());
        check(&mut d, &mut sys, "abrir submenú");
        let s = d.menu().unwrap().sub_item_rect(1).unwrap();
        d.mouse_to(s.x + 10, s.y + 5, &mut sys);
        check(&mut d, &mut sys, "resaltar en submenú");
        key(&mut d, &mut sys, Key::Left, None, false);
        key(&mut d, &mut sys, Key::Down, None, false);
        check(&mut d, &mut sys, "teclado");
        key(&mut d, &mut sys, Key::Right, None, false);
        check(&mut d, &mut sys, "submenú con teclado");
        key(&mut d, &mut sys, Key::Left, None, false);
        check(&mut d, &mut sys, "cerrar submenú");
        key(&mut d, &mut sys, Key::Escape, None, false);
        check(&mut d, &mut sys, "cerrar");
        // Cerca del borde: recolocado y con submenú a la izquierda.
        right_click(&mut d, &mut sys, (1270, 700));
        key(&mut d, &mut sys, Key::Down, None, false);
        key(&mut d, &mut sys, Key::Right, None, false);
        check(&mut d, &mut sys, "borde");
    }
}

#[cfg(test)]
mod menu_tests {
    use super::*;
    use crate::platform::TestPlatform;

    /// Escritorio con una carpeta y un archivo de texto: iconos 0..5 apps, 5 Papelera, 6 Carpeta, 7 nota.txt.
    fn setup() -> (Desktop, TestPlatform) {
        let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
        let now = sys.now();
        sys.files.mkdir("C:\\Escritorio\\Carpeta", now).unwrap();
        sys.files.write("C:\\Escritorio\\nota.txt", b"hola", now).unwrap();
        let mut d = Desktop::new(1280, 800, &mut sys);
        d.booted = true;
        (d, sys)
    }

    fn texts(items: &[MenuItem]) -> Vec<String> {
        items
            .iter()
            .map(|i| if i.separator { String::from("-") } else if i.sub.is_empty() { i.text.clone() } else { format!("{} >", i.text) })
            .collect()
    }

    fn key(d: &mut Desktop, sys: &mut TestPlatform, key: Key, ch: Option<char>, ctrl: bool) {
        d.event(Event::Key(KeyEvent { key, ch, down: true, shift: false, ctrl, alt: false }), sys);
    }

    fn right_click(d: &mut Desktop, sys: &mut TestPlatform, p: (i32, i32)) {
        d.mouse_to(p.0, p.1, sys);
        d.right_button(true, sys);
        d.right_button(false, sys);
    }

    fn center(r: Rect) -> (i32, i32) {
        (r.x + r.w / 2, r.y + r.h / 2)
    }

    fn pick(d: &mut Desktop, sys: &mut TestPlatform, icon: Option<usize>, id: u32) {
        match icon {
            Some(i) => {
                d.sel = Some(i);
                d.desktop_menu_result(MENU_ICON, id, sys);
            }
            None => d.desktop_menu_result(MENU_DESK, id, sys),
        }
    }

    /// Un clic sobre el icono `i`; avanza el reloj para que no cuente como doble clic.
    fn click_icon(d: &mut Desktop, sys: &mut TestPlatform, i: usize) {
        d.event(Event::Tick(d.now_ms + 1000), sys);
        click_at(d, sys, center(d.icon_cell(i)));
    }

    fn answer_text(d: &mut Desktop, sys: &mut TestPlatform, tag: u32, v: &str) {
        d.system_result(DialogResult::Text { tag, value: Some(String::from(v)) }, sys);
    }

    #[test]
    fn icon_menus_have_the_right_items() {
        let (d, mut sys) = setup();
        assert_eq!(d.icons[6].name, "Carpeta");
        assert_eq!(d.icons[7].name, "nota.txt");
        // App: solo Abrir, en negrita.
        let it = d.icon_items(0, &mut sys);
        assert_eq!(texts(&it), ["Abrir"]);
        assert!(it[0].bold);
        // Papelera vacía: Vaciar desactivado; con algo dentro, activo.
        let it = d.icon_items(5, &mut sys);
        assert_eq!(texts(&it), ["Abrir", "Vaciar Papelera"]);
        assert!(it[1].disabled);
        let now = sys.now();
        sys.files.write("C:\\Escritorio\\viejo.txt", b"x", now).unwrap();
        sys.files.trash("C:\\Escritorio\\viejo.txt", now).unwrap();
        assert!(!d.icon_items(5, &mut sys)[1].disabled);
        // Archivo.
        let it = d.icon_items(7, &mut sys);
        let want = ["Abrir", "Abrir con >", "-", "Cortar", "Copiar", "-", "Eliminar", "Cambiar nombre", "-", "Propiedades"];
        assert_eq!(texts(&it), want);
        assert!(it[0].bold && it.iter().all(|i| !i.disabled));
        assert_eq!(it[1].sub[0].text, "Bloc de notas");
        // Carpeta: Pegar desactivado hasta que haya portapapeles.
        let it = d.icon_items(6, &mut sys);
        let want = ["Abrir", "Abrir en ventana nueva", "-", "Cortar", "Copiar", "Pegar", "-", "Eliminar", "Cambiar nombre", "-", "Propiedades"];
        assert_eq!(texts(&it), want);
        assert!(it[5].disabled);
        sys.files.set_clip("C:\\Escritorio\\nota.txt", false);
        assert!(!d.icon_items(6, &mut sys)[5].disabled);
    }

    #[test]
    fn background_menu_items_marks_and_undo() {
        let (mut d, mut sys) = setup();
        let it = d.bg_items(&mut sys);
        assert_eq!(texts(&it), ["Ver >", "Ordenar por >", "Actualizar", "-", "Pegar", "Deshacer", "-", "Nuevo >", "-", "Personalizar"]);
        assert_eq!(texts(&it[0].sub), ["Iconos grandes", "Iconos medianos", "Iconos pequeños"]);
        assert_eq!(texts(&it[1].sub), ["Nombre", "Tamaño", "Tipo", "Fecha de modificación"]);
        assert_eq!(texts(&it[7].sub), ["Carpeta", "Documento de texto"]);
        let checked = |l: &[MenuItem]| l.iter().map(|i| i.checked).collect::<Vec<_>>();
        assert_eq!(checked(&it[0].sub), [false, true, false]);
        assert_eq!(checked(&it[1].sub), [true, false, false, false]);
        assert!(it[4].disabled && it[5].disabled);
        assert_eq!(it[5].shortcut, "Ctrl+Z");
        // Con portapapeles y algo que deshacer.
        sys.files.set_clip("C:\\Escritorio\\nota.txt", false);
        let now = sys.now();
        sys.files.user_trash("C:\\Escritorio\\nota.txt", now).unwrap();
        let it = d.bg_items(&mut sys);
        assert!(!it[4].disabled);
        assert_eq!((it[5].text.as_str(), it[5].disabled), ("Deshacer Eliminar", false));
        // Las marcas siguen a las opciones.
        pick(&mut d, &mut sys, None, M_VIEW + 2);
        pick(&mut d, &mut sys, None, M_SORT + 3);
        let it = d.bg_items(&mut sys);
        assert_eq!(checked(&it[0].sub), [false, false, true]);
        assert_eq!(checked(&it[1].sub), [false, false, false, true]);
    }

    #[test]
    fn right_click_selects_icon_opens_menu_and_open_works() {
        let (mut d, mut sys) = setup();
        { let p = center(d.icon_cell(7)); right_click(&mut d, &mut sys, p); }
        assert_eq!(d.sel, Some(7));
        let m = d.menu().expect("menú del icono");
        let r = m.item_rect(0);
        click_at(&mut d, &mut sys, center(r));
        assert!(d.menu().is_none());
        assert_eq!(d.wins.last().unwrap().kind, AppKind::Notepad);
        // Sobre el fondo: menú del fondo y sin selección.
        d.close(d.wins[0].id);
        right_click(&mut d, &mut sys, (900, 500));
        assert!(d.menu().is_some() && d.sel.is_none());
        d.close_menu();
        // Sobre una ventana no se abre el menú del escritorio.
        d.launch(AppKind::Calc, &mut sys);
        let c = d.wins[0].content_rect();
        right_click(&mut d, &mut sys, (c.x + 20, c.y + 20));
        assert!(d.menu().is_none());
    }

    fn click_at(d: &mut Desktop, sys: &mut TestPlatform, p: (i32, i32)) {
        d.mouse_to(p.0, p.1, sys);
        d.left_button(true, sys);
        d.left_button(false, sys);
    }

    #[test]
    fn copy_cut_paste_delete_and_undo() {
        let (mut d, mut sys) = setup();
        // Copiar y pegar dentro de la carpeta.
        pick(&mut d, &mut sys, Some(7), M_COPY);
        pick(&mut d, &mut sys, Some(6), M_PASTE_IN);
        assert!(sys.files.exists("C:\\Escritorio\\Carpeta\\nota.txt"));
        // Pegar en el fondo: nombre libre.
        pick(&mut d, &mut sys, None, M_PASTE);
        assert!(sys.files.exists("C:\\Escritorio\\nota (2).txt"));
        assert_eq!(sys.files.undo_label(), Some("Copiar"));
        pick(&mut d, &mut sys, None, M_UNDO);
        assert!(!sys.files.exists("C:\\Escritorio\\nota (2).txt"));
        // Cortar y pegar en la carpeta mueve.
        pick(&mut d, &mut sys, Some(7), M_CUT);
        pick(&mut d, &mut sys, Some(6), M_PASTE_IN);
        assert!(!sys.files.exists("C:\\Escritorio\\nota.txt"));
        assert!(sys.files.exists("C:\\Escritorio\\Carpeta\\nota (2).txt"));
        pick(&mut d, &mut sys, None, M_UNDO);
        assert!(sys.files.exists("C:\\Escritorio\\nota.txt"));
        // Eliminar manda a la Papelera, los iconos se actualizan y se puede deshacer.
        let n = d.icons.len();
        let i = d.icons.iter().position(|x| x.name == "nota.txt").unwrap();
        pick(&mut d, &mut sys, Some(i), M_DELETE);
        assert!(!sys.files.exists("C:\\Escritorio\\nota.txt"));
        assert_eq!(d.icons.len(), n - 1);
        assert_eq!(sys.files.trash_list().unwrap().len(), 2);
        pick(&mut d, &mut sys, None, M_UNDO);
        assert!(sys.files.exists("C:\\Escritorio\\nota.txt"));
        assert_eq!(d.icons.len(), n);
        // Sin nada que deshacer: mensaje de error, sin colgarse.
        assert_eq!(d.dialog_count(), 0);
        pick(&mut d, &mut sys, None, M_UNDO);
        assert_eq!(d.dialog_count(), 1);
    }

    #[test]
    fn rename_and_create_with_dialogs() {
        let (mut d, mut sys) = setup();
        // Cambiar nombre: el diálogo guarda la ruta.
        pick(&mut d, &mut sys, Some(7), M_RENAME);
        assert_eq!((d.dialog_count(), d.pending.as_deref()), (1, Some("C:\\Escritorio\\nota.txt")));
        answer_text(&mut d, &mut sys, TAG_RENAME, "lista.txt");
        assert!(sys.files.exists("C:\\Escritorio\\lista.txt") && d.pending.is_none());
        assert_eq!(sys.files.undo_label(), Some("Cambiar nombre"));
        // Nombre inválido: error y no cambia nada.
        let before = d.dialog_count();
        pick(&mut d, &mut sys, Some(7), M_RENAME);
        answer_text(&mut d, &mut sys, TAG_RENAME, "a/b");
        assert_eq!(d.dialog_count(), before + 2);
        assert!(sys.files.exists("C:\\Escritorio\\lista.txt"));
        // Cancelar no hace nada y limpia lo pendiente.
        pick(&mut d, &mut sys, Some(7), M_RENAME);
        d.system_result(DialogResult::Text { tag: TAG_RENAME, value: None }, &mut sys);
        assert!(d.pending.is_none());
        // Documento sin extensión: se añade .txt. Con extensión se respeta.
        answer_text(&mut d, &mut sys, TAG_NEWDOC, "apuntes");
        assert!(sys.files.exists("C:\\Escritorio\\apuntes.txt"));
        answer_text(&mut d, &mut sys, TAG_NEWDOC, "datos.csv");
        assert!(sys.files.exists("C:\\Escritorio\\datos.csv"));
        // Ya existe: error.
        let n = d.dialog_count();
        answer_text(&mut d, &mut sys, TAG_NEWDOC, "apuntes");
        assert_eq!(d.dialog_count(), n + 1);
        answer_text(&mut d, &mut sys, TAG_NEWDIR, "Cosas");
        assert!(sys.files.stat("C:\\Escritorio\\Cosas").unwrap().dir);
        assert!(d.icons.iter().any(|i| i.name == "Cosas"));
        pick(&mut d, &mut sys, None, M_UNDO);
        assert!(!sys.files.exists("C:\\Escritorio\\Cosas"));
    }

    #[test]
    fn new_folder_dialog_proposes_free_name_and_accepts_typing() {
        let (mut d, mut sys) = setup();
        let now = sys.now();
        sys.files.mkdir("C:\\Escritorio\\Nueva carpeta", now).unwrap();
        pick(&mut d, &mut sys, None, M_NEWDIR);
        assert_eq!(d.dialog_count(), 1);
        key(&mut d, &mut sys, Key::Char('z'), Some('z'), false);
        key(&mut d, &mut sys, Key::Enter, None, false);
        assert_eq!(d.dialog_count(), 0);
        // Proponía "Nueva carpeta (2)" y se le ha añadido la z.
        assert!(sys.files.exists("C:\\Escritorio\\Nueva carpeta (2)z"));
    }

    #[test]
    fn empty_trash_asks_first() {
        let (mut d, mut sys) = setup();
        let now = sys.now();
        sys.files.user_trash("C:\\Escritorio\\nota.txt", now).unwrap();
        pick(&mut d, &mut sys, Some(5), M_EMPTY);
        assert_eq!(d.dialog_count(), 1);
        // No (o cerrar) no vacía.
        d.system_result(DialogResult::Answer { tag: TAG_EMPTY, answer: Answer::No }, &mut sys);
        assert_eq!(sys.files.trash_list().unwrap().len(), 1);
        d.system_result(DialogResult::Answer { tag: TAG_EMPTY, answer: Answer::Yes }, &mut sys);
        assert!(sys.files.trash_list().unwrap().is_empty());
        // Abrir la Papelera desde su menú.
        pick(&mut d, &mut sys, Some(5), M_OPEN);
        assert_eq!(d.wins.iter().filter(|w| w.kind == AppKind::Files && w.dlg.is_none()).count(), 1);
    }

    #[test]
    fn open_actions_and_customize() {
        let (mut d, mut sys) = setup();
        pick(&mut d, &mut sys, Some(6), M_NEWWIN);
        pick(&mut d, &mut sys, Some(6), M_NEWWIN);
        assert_eq!(d.wins.iter().filter(|w| w.kind == AppKind::Files).count(), 2);
        pick(&mut d, &mut sys, Some(7), M_NOTEPAD);
        assert_eq!(d.wins.last().unwrap().kind, AppKind::Notepad);
        pick(&mut d, &mut sys, Some(0), M_OPEN);
        pick(&mut d, &mut sys, None, M_CUSTOM);
        pick(&mut d, &mut sys, None, M_CUSTOM);
        assert_eq!(d.wins.iter().filter(|w| w.kind == AppKind::Settings).count(), 1);
    }

    #[test]
    fn properties_text() {
        let (mut d, mut sys) = setup();
        let (title, text) = props_text(&mut sys, "C:\\Escritorio\\nota.txt").unwrap();
        assert_eq!(title, "Propiedades de nota.txt");
        let want = "Nombre: nota.txt\nTipo: Documento de texto\nUbicación: C:\\Escritorio\nTamaño: 4 bytes\nModificado: 15/11/2026 21:37";
        assert_eq!(text, want);
        let now = sys.now();
        sys.files.write("C:\\Escritorio\\Carpeta\\a.bin", &[0u8; 2000], now).unwrap();
        let (_, text) = props_text(&mut sys, "C:\\Escritorio\\Carpeta").unwrap();
        assert!(text.contains("Tipo: Carpeta de archivos") && text.contains("Contiene: 1 elemento\n"), "{text}");
        let (_, text) = props_text(&mut sys, "C:\\Escritorio\\Carpeta\\a.bin").unwrap();
        assert!(text.contains("Tipo: Archivo") && text.contains("Tamaño: 2 KB (2000 bytes)"), "{text}");
        pick(&mut d, &mut sys, Some(7), M_PROPS);
        assert_eq!(d.dialog_count(), 1);
        assert_eq!(size_text(5 << 20), "5.0 MB (5242880 bytes)");
    }

    #[test]
    fn icon_sizes_change_the_grid_and_hit_areas() {
        let (mut d, mut sys) = setup();
        assert_eq!((d.icon_cell(1).h, d.icon_cell(1).y), (84, 90));
        pick(&mut d, &mut sys, None, M_VIEW);
        let big = d.icon_cell(1);
        assert_eq!((big.w, big.h, big.y), (100, 100, 106));
        assert_eq!(d.hit(center(big).0, center(big).1), Hit::Icon(1));
        assert_eq!(d.hit(6 + 90, 6 + 10), Hit::Icon(0), "la celda grande llega a 100 px");
        pick(&mut d, &mut sys, None, M_VIEW + 2);
        let small = d.icon_cell(1);
        assert_eq!((small.w, small.h), (76, 62));
        assert_eq!(d.hit(center(small).0, center(small).1), Hit::Icon(1));
        // Cada tamaño cabe en su celda: icono y texto dentro.
        for e in IconSize::ALL {
            let (cw, ch, isz, iy, ty) = e.0.metrics();
            assert!(iy + isz <= ty && ty + font::line_h(8) <= ch && isz <= cw, "{:?}", e.0);
        }
        // Columnas nuevas cuando no caben en pantalla.
        let mut d2 = Desktop::new(1280, 300, &mut sys);
        d2.size = IconSize::Big;
        assert_eq!((d2.icon_cell(1).x, d2.icon_cell(2).x, d2.icon_cell(4).x), (6, 106, 206));
    }

    #[test]
    fn order_sorts_desktop_files_after_apps_and_trash() {
        let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
        let mut t = sys.now();
        sys.files.mkdir("C:\\Escritorio\\Zeta", t).unwrap();
        t.month = 1;
        sys.files.write("C:\\Escritorio\\a.txt", b"12345", t).unwrap();
        t.month = 3;
        sys.files.write("C:\\Escritorio\\b.bin", b"1", t).unwrap();
        t.month = 2;
        sys.files.write("C:\\Escritorio\\c.abc", b"123", t).unwrap();
        let mut d = Desktop::new(1280, 800, &mut sys);
        d.booted = true;
        let names = |d: &Desktop| d.icons.iter().skip(6).map(|i| i.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&d), ["Zeta", "a.txt", "b.bin", "c.abc"]);
        pick(&mut d, &mut sys, None, M_SORT + 1);
        assert_eq!(names(&d), ["Zeta", "b.bin", "c.abc", "a.txt"]);
        pick(&mut d, &mut sys, None, M_SORT + 2);
        assert_eq!(names(&d), ["Zeta", "c.abc", "b.bin", "a.txt"]);
        pick(&mut d, &mut sys, None, M_SORT + 3);
        assert_eq!(names(&d), ["Zeta", "b.bin", "c.abc", "a.txt"]);
        pick(&mut d, &mut sys, None, M_SORT);
        assert_eq!(names(&d), ["Zeta", "a.txt", "b.bin", "c.abc"]);
        // Las apps y la Papelera siguen primero y en su orden.
        assert_eq!(d.icons[5].name, "Papelera");
        assert_eq!(d.icons[0].name, "Bloc de notas");
        // La selección sigue al icono al reordenar.
        d.sel = Some(7);
        pick(&mut d, &mut sys, None, M_SORT + 1);
        assert_eq!(d.icons[d.sel.unwrap()].name, "a.txt");
    }

    #[test]
    fn size_and_order_are_saved_loaded_and_foreign_lines_kept() {
        let (mut d, mut sys) = setup();
        let now = sys.now();
        sys.files.write(files::CONFIG_FILE, b"explorador=lista\nfondo=1\n", now).unwrap();
        pick(&mut d, &mut sys, None, M_VIEW);
        pick(&mut d, &mut sys, None, M_SORT + 2);
        let text = String::from_utf8(sys.files.read(files::CONFIG_FILE).unwrap()).unwrap();
        assert_eq!(text, "fondo=0\nenfasis=0078D4\niconos=grande\norden=tipo\nexplorador=lista\n".replace("0078D4", &format!("{:06X}", d.theme.accent & 0xFFFFFF)));
        let d2 = Desktop::new(1280, 800, &mut sys);
        assert_eq!((d2.size, d2.order), (IconSize::Big, Order::Kind));
        // Valores no válidos se ignoran.
        sys.files.write(files::CONFIG_FILE, b"iconos=enorme\norden=azar\n", now).unwrap();
        let d3 = Desktop::new(1280, 800, &mut sys);
        assert_eq!((d3.size, d3.order), (IconSize::Mid, Order::Name));
        sys.files.write(files::CONFIG_FILE, b"iconos=pequeno\norden=fecha\n", now).unwrap();
        let d4 = Desktop::new(1280, 800, &mut sys);
        assert_eq!((d4.size, d4.order), (IconSize::Small, Order::Date));
    }

    #[test]
    fn desktop_keys_undo_delete_rename_only_without_windows() {
        let (mut d, mut sys) = setup();
        sys.files.user_rename("C:\\Escritorio\\nota.txt", "C:\\Escritorio\\otra.txt").unwrap();
        // Con una ventana enfocada, Ctrl+Z no es del escritorio.
        d.launch(AppKind::Calc, &mut sys);
        key(&mut d, &mut sys, Key::Char('z'), None, true);
        assert!(sys.files.exists("C:\\Escritorio\\otra.txt"));
        d.close(d.wins[0].id);
        key(&mut d, &mut sys, Key::Char('z'), None, true);
        assert!(sys.files.exists("C:\\Escritorio\\nota.txt"));
        // Supr sobre el icono seleccionado: a la Papelera.
        click_icon(&mut d, &mut sys, 7);
        key(&mut d, &mut sys, Key::Delete, None, false);
        assert!(!sys.files.exists("C:\\Escritorio\\nota.txt"));
        assert_eq!(sys.files.trash_list().unwrap().len(), 1);
        key(&mut d, &mut sys, Key::Char('z'), None, true);
        assert!(sys.files.exists("C:\\Escritorio\\nota.txt"));
        // F2 pide el nombre; sobre una app o la Papelera no hace nada.
        click_icon(&mut d, &mut sys, 7);
        key(&mut d, &mut sys, Key::F(2), None, false);
        assert_eq!((d.dialog_count(), d.pending.is_some()), (1, true));
        d.system_result(DialogResult::Text { tag: TAG_RENAME, value: None }, &mut sys);
        click_icon(&mut d, &mut sys, 5);
        let n = d.dialog_count();
        key(&mut d, &mut sys, Key::Delete, None, false);
        key(&mut d, &mut sys, Key::F(2), None, false);
        assert_eq!(d.dialog_count(), n);
    }

    #[test]
    fn incremental_equals_full_with_desktop_menus() {
        let (mut d, mut sys) = setup();
        d.render(&mut sys);
        let check = |d: &mut Desktop, sys: &mut TestPlatform, what: &str| {
            d.render(sys);
            let inc = d.back().px.clone();
            d.invalidate();
            d.render(sys);
            assert!(inc == d.back().px, "incremental distinto del completo: {what}");
        };
        right_click(&mut d, &mut sys, (900, 400));
        check(&mut d, &mut sys, "menú del fondo");
        let r = d.menu().unwrap().item_rect(0);
        d.mouse_to(r.x + 10, r.y + 5, &mut sys);
        let s = d.menu().unwrap().sub_item_rect(0).unwrap();
        d.mouse_to(s.x + 10, s.y + 5, &mut sys);
        check(&mut d, &mut sys, "submenú Ver");
        click_at(&mut d, &mut sys, (s.x + 10, s.y + 5));
        check(&mut d, &mut sys, "iconos grandes");
        { let p = center(d.icon_cell(7)); right_click(&mut d, &mut sys, p); }
        check(&mut d, &mut sys, "menú de icono");
        key(&mut d, &mut sys, Key::Escape, None, false);
        pick(&mut d, &mut sys, None, M_VIEW + 2);
        check(&mut d, &mut sys, "iconos pequeños");
        pick(&mut d, &mut sys, None, M_VIEW + 1);
        check(&mut d, &mut sys, "iconos medianos");
        let now = sys.now();
        sys.files.write("C:\\Escritorio\\nuevo.txt", b"n", now).unwrap();
        d.event(Event::Tick(10), &mut sys);
        check(&mut d, &mut sys, "archivo creado");
        sys.files.delete("C:\\Escritorio\\nuevo.txt").unwrap();
        d.event(Event::Tick(20), &mut sys);
        check(&mut d, &mut sys, "archivo eliminado");
        pick(&mut d, &mut sys, None, M_SORT + 1);
        check(&mut d, &mut sys, "orden");
        { let p = center(d.icon_cell(7)); click_at(&mut d, &mut sys, p); }
        key(&mut d, &mut sys, Key::Delete, None, false);
        check(&mut d, &mut sys, "supr");
        key(&mut d, &mut sys, Key::Char('z'), None, true);
        check(&mut d, &mut sys, "deshacer");
    }
}

#[cfg(test)]
mod cinta_tests {
    use super::*;
    use crate::platform::TestPlatform;
    use alloc::rc::Rc;
    use core::cell::RefCell;

    fn setup() -> (Desktop, TestPlatform) {
        let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
        let now = sys.now();
        sys.files.write("C:\\Escritorio\\nota.txt", b"hola", now).unwrap();
        let mut d = Desktop::new(1280, 800, &mut sys);
        d.booted = true;
        (d, sys)
    }

    fn labels(d: &Desktop) -> Vec<String> {
        d.icons.iter().skip(6).map(|i| i.label.clone()).collect()
    }

    #[test]
    fn extensions_hidden_on_icons_and_kept_on_rename() {
        let (mut d, mut sys) = setup();
        assert_eq!(labels(&d), ["nota.txt"]);
        // Ocultar extensiones: el archivo de preferencias cambia y "Actualizar" lo recoge.
        let now = sys.now();
        sys.files.write(files::EXPLORER_PREFS, b"extensiones=no\n", now).unwrap();
        d.bg_action(M_REFRESH, &mut sys);
        assert_eq!(labels(&d), ["nota"]);
        assert_eq!(d.icons[6].name, "nota.txt");
        // El diálogo propone el nombre sin extensión y la extensión original se conserva.
        d.start_rename("C:\\Escritorio\\nota.txt", &mut sys);
        d.system_result(DialogResult::Text { tag: TAG_RENAME, value: Some(String::from("otra")) }, &mut sys);
        assert!(sys.files.exists("C:\\Escritorio\\otra.txt") && !sys.files.exists("C:\\Escritorio\\nota.txt"));
        // Un nombre vacío sigue siendo inválido (no queda ".txt").
        let n = d.dialog_count();
        d.start_rename("C:\\Escritorio\\otra.txt", &mut sys);
        d.system_result(DialogResult::Text { tag: TAG_RENAME, value: Some(String::from("  ")) }, &mut sys);
        assert!(sys.files.exists("C:\\Escritorio\\otra.txt"));
        assert!(d.dialog_count() > n);
    }

    /// App de pruebas que anota los `KeyEvent` que recibe.
    struct Rec(Rc<RefCell<Vec<(Key, bool, bool, bool)>>>);
    impl App for Rec {
        fn kind(&self) -> AppKind {
            AppKind::Calc
        }
        fn title(&self) -> String {
            String::from("Rec")
        }
        fn min_size(&self) -> (i32, i32) {
            (100, 100)
        }
        fn draw(&mut self, _s: &mut Surface, _t: &Theme, _p: &mut dyn Platform, _f: bool) {}
        fn key(&mut self, ev: &KeyEvent, _cx: &mut Ctx) {
            self.0.borrow_mut().push((ev.key, ev.down, ev.ctrl, ev.shift));
        }
        fn mouse(&mut self, _ev: MouseEv, _cx: &mut Ctx) {}
        fn tick(&mut self, _cx: &mut Ctx) {}
    }

    fn other(down: bool, ctrl: bool, shift: bool) -> Event {
        Event::Key(KeyEvent { key: Key::Other, ch: None, down, shift, ctrl, alt: false })
    }

    #[test]
    fn app_receives_ctrl_and_shift_changes_and_neutral_on_focus_loss() {
        let (mut d, mut sys) = setup();
        let log = Rc::new(RefCell::new(Vec::new()));
        d.launch_app(Box::new(Rec(log.clone())), &mut sys);
        d.event(other(true, true, false), &mut sys);
        d.event(other(true, true, true), &mut sys);
        d.event(other(false, false, true), &mut sys);
        d.event(other(false, false, false), &mut sys);
        assert_eq!(
            *log.borrow(),
            [(Key::Other, true, true, false), (Key::Other, true, true, true), (Key::Other, true, false, true), (Key::Other, false, false, false)]
        );
        // Una repetición sin cambios no se reenvía.
        log.borrow_mut().clear();
        d.event(other(true, true, false), &mut sys);
        d.event(other(true, true, false), &mut sys);
        assert_eq!(log.borrow().len(), 1);
        // Otra ventana toma el foco con Ctrl pulsado: la anterior recibe el estado neutro.
        d.launch(AppKind::Notepad, &mut sys);
        d.event(Event::Tick(10), &mut sys);
        assert_eq!(log.borrow().last(), Some(&(Key::Other, false, false, false)));
        // Soltar mientras el foco está en otra ventana no llega a la primera.
        let n = log.borrow().len();
        d.event(other(false, false, false), &mut sys);
        assert_eq!(log.borrow().len(), n);
    }

    #[test]
    fn modifier_events_do_not_disturb_other_apps() {
        let (mut d, mut sys) = setup();
        d.launch(AppKind::Notepad, &mut sys);
        d.launch(AppKind::Terminal, &mut sys);
        d.launch(AppKind::Calc, &mut sys);
        let n = d.window_count();
        for ev in [other(true, true, false), other(true, true, true), other(false, false, true), other(false, false, false)] {
            d.event(ev, &mut sys);
        }
        assert_eq!(d.window_count(), n);
    }

    #[test]
    fn open_terminal_opens_a_new_terminal_in_that_folder() {
        let (mut d, mut sys) = setup();
        let now = sys.now();
        sys.files.mkdir("C:\\Escritorio\\Sub", now).unwrap();
        d.launch(AppKind::Calc, &mut sys);
        let id = d.active_id().unwrap();
        d.with_app(id, &mut sys, |_, cx| cx.open_terminal = Some(String::from("C:\\Escritorio\\Sub")));
        d.with_app(id, &mut sys, |_, cx| cx.open_terminal = Some(String::from("C:\\Escritorio\\Sub")));
        let terms = d.wins.iter().filter(|w| w.kind == AppKind::Terminal).count();
        assert_eq!(terms, 2);
    }

    #[test]
    fn cinta_copiar_tras_seleccionar_todo() {
        let (mut d, mut sys) = setup();
        let now = sys.now();
        sys.files.write("C:\\Documentos\\a.txt", b"hola", now).unwrap();
        sys.files.write("C:\\Documentos\\b.txt", b"hola", now).unwrap();
        d.launch(AppKind::Files, &mut sys);
        d.render(&mut sys);
        let c = d.wins[0].content_rect();
        for (x, y) in [(922, 38), (32, 54)] {
            d.mouse_to(c.x + x, c.y + y, &mut sys);
            sys.ms += 700; d.tick(sys.ms, &mut sys);
            d.render(&mut sys);
            d.left_button(true, &mut sys);
            sys.ms += 700; d.tick(sys.ms, &mut sys);
            d.render(&mut sys);
            d.left_button(false, &mut sys);
            sys.ms += 700; d.tick(sys.ms, &mut sys);
            d.render(&mut sys);
        }
        assert_eq!(sys.files.clip_paths().0.len(), 2);
    }
}
