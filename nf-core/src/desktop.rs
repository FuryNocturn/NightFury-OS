// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
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
use crate::bmp::Picture;
use crate::icons::{self, Icon};
use crate::input::{Event, Key, KeyEvent};
use crate::keys::KeyError;
use crate::login::{Login, Mode, Out};
use crate::menu::{Menu, MenuAct, MenuItem};
use crate::platform::{DateTime, Platform};
use crate::programs::{self, Program};
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
/// Número que identifica la tecla para el acumulador de aleatoriedad.
fn key_code(k: Key) -> i32 {
    match k {
        Key::Char(c) => c as i32,
        Key::F(n) => 0x1000 + n as i32,
        Key::Escape => 0x2001,
        Key::Backspace => 0x2002,
        Key::Tab => 0x2003,
        Key::Enter => 0x2004,
        Key::Left => 0x2005,
        Key::Right => 0x2006,
        Key::Up => 0x2007,
        Key::Down => 0x2008,
        _ => 0x2000,
    }
}

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
    /// Pulsado un icono de archivo o carpeta del escritorio: si se mueve más de 5 px empieza el arrastre.
    /// `collapse`: era uno de varios seleccionados; si se suelta sin arrastrar, la selección se queda en él.
    Icon { i: usize, x: i32, y: i32, collapse: bool },
    /// Recuadro de selección desde (x, y) del fondo del escritorio.
    Band(i32, i32),
    /// Arrastre de archivos llevado por el escritorio (ver `Dnd`).
    Files,
    /// Arrastre cancelado con el botón aún pulsado: se ignora hasta soltar.
    Dead,
}

/// Arrastre de archivos en curso.
struct Dnd {
    paths: Vec<String>,
    /// Ventana de origen (`None` = el escritorio).
    src: Option<u32>,
    /// Ventana que tiene ahora el `drag_over` y lo que contestó la última vez.
    over: Option<u32>,
    ok: bool,
    /// Se ha cerrado la ventana de origen: se cancela en cuanto se pueda.
    dead: bool,
    /// Arrastre con el botón derecho: al soltar sale un menú (Mover / Copiar / Cancelar).
    right: bool,
}

/// Botón derecho pulsado: punto de la pulsación, si ya se probó a arrastrar y si hay arrastre.
#[derive(Clone, Copy)]
struct RPress {
    x: i32,
    y: i32,
    tried: bool,
    dragging: bool,
}

/// Qué hace un soltar sobre un destino, calculado al soltar.
#[derive(Clone, PartialEq, Eq, Debug)]
enum DropAct {
    /// Ventana, con el punto relativo a su contenido.
    Win(u32, i32, i32),
    To(Dest),
}

/// Qué hay bajo el puntero durante un arrastre de archivos.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tgt {
    Win(u32),
    /// Fondo del escritorio.
    Desk,
    /// Icono del escritorio que admite soltar: una carpeta o la Papelera.
    Icon(usize),
    Nothing,
}

/// Destino de un mover, copiar o eliminar hecho por el escritorio.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Dest {
    Dir(String),
    Trash,
}

/// Píxeles que hay que mover un icono pulsado para que empiece el arrastre.
const DRAG_PX: i32 = 5;
/// Píxeles que hay que mover desde el fondo para que aparezca el recuadro de selección.
const BAND_PX: i32 = 4;

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
    /// Icono propio del programa (`icono.bmp`); si está, sustituye a `App::icon`.
    // shortcut: se guarda aquí y no en `App::picture()` para no tocar el contrato; mover si hay más apps con imagen.
    pic: Option<Picture>,
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
    /// Programa de `C:\Programas` (v0.8).
    Program(Program),
}

/// Dibuja el icono propio de un programa (`icono.bmp`) si lo tiene; si no, el del sistema.
fn paint_icon(s: &mut Surface, icon: Icon, pic: Option<&Picture>, x: i32, y: i32, size: i32, fg: u32) {
    match pic {
        Some(p) => p.draw(s, x, y, size),
        None => icons::draw(s, icon, x, y, size, fg),
    }
}

/// Lo que se puede abrir desde el menú Inicio y el escritorio: apps del sistema y programas.
struct Launcher {
    name: String,
    icon: Icon,
    pic: Option<Picture>,
    act: Act,
}

/// Apps del sistema y programas instalados, por nombre. Una app del sistema que ya existe como
/// programa con el mismo nombre (la Calculadora en la v0.8) no se repite.
fn launchers(sys: &mut dyn Platform) -> Vec<Launcher> {
    let progs = programs::installed(sys);
    let mut v: Vec<Launcher> = APPS
        .iter()
        .filter(|(_, n, _)| !progs.iter().any(|p| p.name.eq_ignore_ascii_case(n)))
        .map(|(k, n, i)| Launcher { name: String::from(*n), icon: *i, pic: None, act: Act::App(*k) })
        .collect();
    v.extend(progs.into_iter().map(|p| Launcher { name: p.name.clone(), icon: p.icon, pic: p.picture.clone(), act: Act::Program(p) }));
    v.sort_by_key(|l| l.name.to_lowercase());
    v
}

struct DIcon {
    name: String,
    /// Lo que se escribe bajo el icono (sin extensión si el usuario la oculta).
    label: String,
    icon: Icon,
    pic: Option<Picture>,
    act: Act,
}

/// Etiquetas de los diálogos del sistema (sin dueña).
const TAG_FORMAT: u32 = 0xFFFF_FF01;
const TAG_SYS: u32 = 0xFFFF_FF02;
/// Ventana del ratón del asistente de disco nuevo (llaves y códigos de recuperación).
const TAG_SETUP: u32 = 0xFFFF_FF03;
/// Diálogos de los menús del escritorio; la ruta afectada va en `Desktop::pending`.
const TAG_RENAME: u32 = 0xFFFF_FF10;
const TAG_NEWDIR: u32 = 0xFFFF_FF11;
const TAG_NEWDOC: u32 = 0xFFFF_FF12;
const TAG_EMPTY: u32 = 0xFFFF_FF13;
/// Mensajes de información o error sin respuesta que tratar.
const TAG_INFO: u32 = 0xFFFF_FF14;
/// Confirmación de soltar archivos sobre o desde `C:\Sistema`.
const TAG_DROP: u32 = 0xFFFF_FF15;
/// "¿Enviar a la Papelera?" (Supr y Eliminar sobre iconos).
const TAG_DEL: u32 = 0xFFFF_FF16;

/// Etiquetas e identificadores de los menús contextuales del escritorio.
const MENU_ICON: u32 = 1;
const MENU_DESK: u32 = 2;
/// Menú que sale al soltar un arrastre con el botón derecho.
const MENU_DROP: u32 = 3;
const M_MOVE_HERE: u32 = 1;
const M_COPY_HERE: u32 = 2;
const M_CANCEL_DROP: u32 = 3;
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
    /// Lo que enseña el menú Inicio (se rehace al abrirlo).
    start_items: Vec<Launcher>,
    /// Tamaño del contenido de la próxima ventana que se abra (programas, según su `app.txt`).
    next_size: Option<(i32, i32)>,
    shut_open: bool,
    /// Iconos seleccionados (índices, ordenados) y el ancla del Mayús+clic.
    sel: Vec<usize>,
    anchor: usize,
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
    /// Iconos del escritorio: apps, Papelera y lo que haya en la carpeta Escritorio del usuario.
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
    /// Botón derecho pulsado sin menú abierto: dónde y si ya ha empezado un arrastre.
    rpress: Option<RPress>,
    /// Soltar con el botón derecho pendiente de elegir en el menú: qué hacer y las rutas.
    pending_rdrop: Option<(DropAct, Vec<String>)>,
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
    /// Conversión de cifrado en curso (la que muestra el aviso), aviso final y si falló.
    conv: Option<Conv>,
    conv_note: Option<(&'static str, u64)>,
    conv_failed: bool,
    /// Arrastre de archivos en curso, su etiqueta (último rectángulo pintado) y el icono resaltado como destino.
    dnd: Option<Dnd>,
    dnd_lbl: Rect,
    dnd_hl: Option<usize>,
    /// Una app ha pedido empezar a arrastrar (se atiende en el movimiento que la provocó).
    drag_req: Option<(u32, Vec<String>)>,
    /// Recuadro de selección visible y la selección que había antes de empezarlo (con Ctrl).
    band: Option<Rect>,
    band_base: Vec<usize>,
    /// Soltar pendiente de confirmar (toca `C:\Sistema`): destino, rutas y si es copiar.
    pending_drop: Option<(Dest, Vec<String>, bool)>,
    /// Lo que se enviará a la Papelera si se responde Sí a `TAG_DEL`.
    pending_del: Option<Vec<String>>,
}

/// Bloques del disco que se convierten en cada tick (el reloj da unas 100 vueltas por segundo).
const CONV_DISK_BLOCKS: u32 = 16;
/// Archivos de la carpeta que se convierten en cada tick.
const CONV_HOME_FILES: u32 = 1;
/// Cuánto se ve "Cifrado completado".
const CONV_NOTE_MS: u64 = 4000;
/// Confirmaciones de apagar (0), reiniciar (1) y cerrar sesión (3) con un cifrado en curso.
const TAG_POWER: u32 = 0xFFFF_FF20;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ConvWhat {
    Disk,
    Home,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Conv {
    what: ConvWhat,
    /// `true` = cifrando, `false` = descifrando.
    enc: bool,
    pct: u32,
}

impl Conv {
    fn text(&self) -> String {
        let (verb, what) = match (self.enc, self.what) {
            (true, ConvWhat::Disk) => ("Cifrando", "el disco"),
            (false, ConvWhat::Disk) => ("Descifrando", "el disco"),
            (true, ConvWhat::Home) => ("Cifrando", "tu carpeta"),
            (false, ConvWhat::Home) => ("Descifrando", "tu carpeta"),
        };
        format!("{verb} {what}… {} %", self.pct)
    }
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

/// Aplica `fondo=`, `enfasis=`, `iconos=` y `orden=` de la configuración del usuario si son válidos.
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

/// Apps, Papelera y el contenido de la carpeta Escritorio del usuario (carpetas antes que archivos, según `order`).
fn build_icons(sys: &mut dyn Platform, order: Order, show_ext: bool) -> Vec<DIcon> {
    // El Administrador de tareas no tiene icono en el escritorio (se abre con Ctrl+Mayús+Esc o desde Inicio).
    let mut v: Vec<DIcon> = launchers(sys)
        .into_iter()
        .filter(|l| !matches!(l.act, Act::App(AppKind::TaskManager)))
        .map(|l| DIcon { label: l.name.clone(), name: l.name, icon: l.icon, pic: l.pic, act: l.act })
        .collect();
    v.push(DIcon { name: String::from("Papelera"), label: String::from("Papelera"), icon: Icon::Trash, pic: None, act: Act::Path(String::from(files::TRASH)) });
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
        v.push(DIcon { act: Act::Path(files::join(&dir, &e.name)), label, name: e.name, icon, pic: None });
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
            start_items: Vec::new(),
            next_size: None,
            shut_open: false,
            sel: Vec::new(),
            anchor: 0,
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
            rpress: None,
            pending_rdrop: None,
            login: Login::new(w, h),
            dim: Surface::empty(),
            can_session: sys.files().persistent(),
            logging_out: false,
            logout_wait: None,
            conv: None,
            conv_note: None,
            conv_failed: false,
            dnd: None,
            dnd_lbl: Rect::default(),
            dnd_hl: None,
            drag_req: None,
            band: None,
            band_base: Vec::new(),
            pending_drop: None,
            pending_del: None,
        };
        d.login.set_clock(d.clock);
        if sys.files().user().is_some() && !sys.files().disk_locked() {
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
        self.sel.clear();
        self.dmg_all();
    }

    /// Prepara una pantalla de cuentas: nada del escritorio queda activo y el fondo se oscurece.
    fn screen_on(&mut self) {
        self.set_start(false);
        self.set_shut(false);
        self.close_menu();
        self.drag = Drag::None;
        self.chover = None;
        self.sel.clear();
        self.dnd = None;
        self.band = None;
        self.dnd_hl = None;
        self.dnd_lbl = Rect::default();
        self.pending_drop = None;
        self.pending_rdrop = None;
        self.rpress = None;
        self.pending_del = None;
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
        // Disco cifrado: primero su clave, antes del asistente y del inicio de sesión.
        if sys.files().disk_locked() {
            self.login.start_disk();
            self.screen_on();
            return;
        }
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
        let enc = self.login.user_names().into_iter().map(|n| sys.files().account_encrypted(&n)).collect();
        self.login.set_recoverable(enc);
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
            Out::SetupDone | Out::DiskDone => self.enter_flow(sys),
            Out::Shutdown => sys.shutdown(),
            Out::Reboot => sys.reboot(),
            Out::NeedEntropy => {
                let req = DialogRequest::Entropy { tag: TAG_SETUP, title: String::from("Generar las llaves") };
                self.open_dialog(None, req, sys);
            }
        }
        self.flush_login();
    }

    /// Apagar (0), reiniciar (1) o cerrar sesión (3). Con un cifrado en curso, antes pide confirmación.
    fn power(&mut self, act: u32, sys: &mut dyn Platform) {
        if self.converting(sys) {
            let (title, what) = match act {
                0 => ("Apagar", "apagar: continuará al volver a arrancar"),
                1 => ("Reiniciar", "reiniciar: continuará al volver a arrancar"),
                _ => ("Cerrar sesión", "cerrar la sesión: continuará al volver a entrar"),
            };
            let text = format!("Hay un cifrado en curso. Puedes {what}.");
            let req = DialogRequest::Message { tag: TAG_POWER + act, title: String::from(title), text, buttons: Buttons::YesNo };
            self.open_dialog(None, req, sys);
            return;
        }
        self.do_power(act, sys);
    }

    fn do_power(&mut self, act: u32, sys: &mut dyn Platform) {
        match act {
            0 => sys.shutdown(),
            1 => sys.reboot(),
            _ => self.begin_logout(sys),
        }
    }

    /// ¿Hay una conversión de cifrado en marcha?
    fn converting(&self, sys: &mut dyn Platform) -> bool {
        self.conv.is_some() || sys.files().disk_conversion().is_some() || sys.files().home_conversion().is_some()
    }

    fn conv_rect(&self) -> Rect {
        Rect::new(self.w - 292, self.bar_y() - 60, 280, 52)
    }

    /// Un paso de la conversión en curso (si la hay) y actualización del aviso. Si falla, no se
    /// reintenta: un diálogo lo cuenta una sola vez.
    fn convert_tick(&mut self, sys: &mut dyn Platform) {
        if let Some((_, until)) = self.conv_note {
            if self.now_ms >= until {
                self.conv_note = None;
                self.dmg(self.conv_rect());
            }
        }
        if self.conv_failed || !self.booted || sys.files().disk_locked() {
            return;
        }
        let now = sys.now();
        let (what, res) = if sys.files().disk_conversion().is_some() {
            (ConvWhat::Disk, sys.files().convert_step(CONV_DISK_BLOCKS, now))
        } else if sys.files().home_conversion().is_some() {
            (ConvWhat::Home, sys.files().home_convert_step(CONV_HOME_FILES, now))
        } else {
            self.set_conv(ConvWhat::Disk, None);
            return;
        };
        match res {
            Ok(_) => {
                let state = if what == ConvWhat::Disk { sys.files().disk_conversion() } else { sys.files().home_conversion() };
                self.set_conv(what, state);
            }
            Err(e) => self.conv_error(e, sys),
        }
    }

    /// Anota el estado de la conversión: (hechos, total, cifrando) o `None` si ya terminó.
    fn set_conv(&mut self, what: ConvWhat, state: Option<(u64, u64, bool)>) {
        let old = self.conv;
        match state {
            Some((done, total, enc)) => {
                let pct = if total == 0 { 100 } else { (done.min(total) * 100 / total) as u32 };
                self.conv = Some(Conv { what, enc, pct });
                self.conv_note = None;
            }
            None => {
                if let Some(c) = self.conv.take() {
                    self.conv_note = Some((if c.enc { "Cifrado completado" } else { "Descifrado completado" }, self.now_ms + CONV_NOTE_MS));
                }
            }
        }
        if self.conv != old || (old.is_some() && self.conv.is_none()) {
            self.dmg(self.conv_rect());
        }
    }

    fn conv_error(&mut self, e: KeyError, sys: &mut dyn Platform) {
        self.conv_failed = true;
        self.conv = None;
        self.dmg(self.conv_rect());
        let now = sys.now();
        sys.files().log(&format!("Conversión de cifrado detenida: {e:?}"), now);
        let text = format!("El cifrado se ha detenido. {}", e.text());
        let req = DialogRequest::Message { tag: TAG_SYS, title: String::from("Cifrado"), text, buttons: Buttons::Ok };
        self.open_dialog(None, req, sys);
    }

    fn paint_conv(&self, s: &mut Surface) {
        let r = self.conv_rect();
        if !r.intersects(s.clip()) {
            return;
        }
        let (text, pct) = match (&self.conv, self.conv_note) {
            (Some(c), _) => (c.text(), Some(c.pct)),
            (None, Some((t, _))) => (String::from(t), None),
            _ => return,
        };
        s.fill_rect(r, 0x2B2B2B);
        s.frame(r, blend(0x2B2B2B, 0xFFFFFF, 40));
        text_in(s, Rect::new(r.x + 14, r.y + 6, r.w - 28, 26), &text, 9, 0xFFFFFF, Align::Left);
        if let Some(p) = pct {
            let track = Rect::new(r.x + 14, r.bottom() - 14, r.w - 28, 4);
            s.fill_rect(track, 0x555555);
            s.fill_rect(Rect::new(track.x, track.y, track.w * p as i32 / 100, track.h), self.theme.accent);
        }
    }

    /// Solo para vistas previas: pinta el aviso de conversión sin tocar el disco.
    #[doc(hidden)]
    pub fn preview_conversion(&mut self, disk: bool, enc: bool, pct: u32) {
        self.set_conv(if disk { ConvWhat::Disk } else { ConvWhat::Home }, Some((pct as u64, 100, enc)));
    }

    /// Solo para vistas previas y pruebas: muestra la pantalla "Disco cifrado".
    #[doc(hidden)]
    pub fn preview_disk_screen(&mut self) {
        self.login.start_disk();
        self.screen_on();
    }

    /// Solo para vistas previas: abre el diálogo de aleatoriedad como diálogo del sistema.
    #[doc(hidden)]
    pub fn preview_entropy(&mut self, sys: &mut dyn Platform) {
        let req = DialogRequest::Entropy { tag: TAG_SYS, title: String::from("Generar la llave") };
        self.open_dialog(None, req, sys);
    }

    /// Bloquea: las ventanas siguen abiertas pero no se ven ni reciben nada.
    pub fn lock(&mut self, sys: &mut dyn Platform) {
        let Some(name) = sys.files().user().map(String::from) else { return };
        let has = sys.files().has_password();
        // La llave de la carpeta se borra de la memoria hasta que se desbloquee con la clave.
        sys.files().lock_keys();
        self.cancel_dnd(sys);
        self.login.start_lock(&name, has);
        self.screen_on();
        self.flush_login();
        self.sync_mods(sys);
    }

    /// Cierra la sesión: pide cerrar cada ventana (una a una; si alguna se niega, se cancela).
    fn begin_logout(&mut self, sys: &mut dyn Platform) {
        self.set_start(false);
        self.set_shut(false);
        self.cancel_dnd(sys);
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
        // Cada movimiento alimenta el acumulador de aleatoriedad (también en las pantallas de inicio).
        let fine = sys.entropy();
        sys.pool().add_event(x, y, self.now_ms, fine);
        self.dmg(cursor_rect(self.mx, self.my));
        self.mx = x;
        self.my = y;
        self.dmg(cursor_rect(x, y));
        if self.login.covers() {
            self.cancel_dnd(sys);
            self.login.hover(x, y);
            self.flush_login();
            return;
        }
        if self.menu.is_some() {
            self.menu_edit(|m| m.hover(x, y));
            return;
        }
        if let Some(p) = self.rpress.filter(|p| !p.tried && self.drag == Drag::None) {
            if (x - p.x).abs() > DRAG_PX || (y - p.y).abs() > DRAG_PX {
                self.begin_right_drag(p.x, p.y, sys);
            }
        }
        match self.drag {
            Drag::Icon { i, x: px, y: py, .. } => {
                if (x - px).abs() > DRAG_PX || (y - py).abs() > DRAG_PX {
                    let paths = self.sel_paths();
                    if paths.is_empty() || !self.sel.contains(&i) {
                        self.drag = Drag::Dead;
                    } else {
                        self.begin_dnd(None, paths, false, sys);
                    }
                }
            }
            Drag::Band(x0, y0) => self.band_move(x0, y0),
            Drag::Files => self.update_dnd(sys),
            _ => {}
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
            Drag::Content(id) => {
                self.send(id, sys, |x, y| MouseEv::Move { x, y });
                if let Some((wid, paths)) = self.drag_req.take() {
                    if wid == id && self.drag == Drag::Content(id) {
                        self.begin_dnd(Some(id), paths, false, sys);
                        self.after(sys);
                    }
                }
            }
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
        if self.rpress.is_some_and(|p| p.dragging) {
            return; // arrastrando con el derecho: el izquierdo no hace nada
        }
        if self.login.covers() {
            self.cancel_dnd(sys);
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
        if self.login.covers() {
            self.rpress = None;
            return;
        }
        if down {
            if matches!(self.drag, Drag::Files | Drag::Band(..) | Drag::Icon { .. }) {
                return;
            }
            if self.menu.is_some() {
                self.menu_click(sys);
                return;
            }
            // El menú sale al soltar; ahora solo se recuerda el punto (y la ventana pasa al frente).
            let (x, y) = (self.mx, self.my);
            let ok = match self.hit(x, y) {
                Hit::None | Hit::Icon(_) => self.login.mode != Mode::Boot,
                Hit::Content(id) if self.modal_of(id).is_none() => {
                    self.set_start(false);
                    self.raise(id);
                    true
                }
                _ => false,
            };
            self.rpress = ok.then_some(RPress { x, y, tried: false, dragging: false });
            self.after(sys);
            return;
        }
        let Some(p) = self.rpress.take() else { return };
        if p.dragging {
            if self.dnd.as_ref().is_some_and(|d| d.right) {
                self.end_dnd(true, sys);
            } else if self.drag == Drag::Dead {
                self.drag = Drag::None; // se canceló con Esc: ya se puede soltar
            }
        } else if self.menu.is_none() {
            match self.hit(p.x, p.y) {
                Hit::Content(id) if self.modal_of(id).is_none() => self.send_at(id, p.x, p.y, sys, |x, y| MouseEv::RightDown { x, y }),
                Hit::None | Hit::Icon(_) => self.desktop_menu(p.x, p.y, sys),
                _ => {}
            }
        }
        self.after(sys);
    }

    /// Pasado el umbral con el derecho pulsado en (x, y): empieza un arrastre de archivos si ahí hay algo.
    fn begin_right_drag(&mut self, x: i32, y: i32, sys: &mut dyn Platform) {
        if let Some(p) = self.rpress.as_mut() {
            p.tried = true;
        }
        let mut paths = Vec::new();
        let mut src = None;
        match self.hit(x, y) {
            Hit::Content(id) if self.modal_of(id).is_none() => {
                let Some(i) = self.pos(id).filter(|&i| self.wins[i].dlg.is_none()) else { return };
                let c = self.wins[i].content_rect();
                self.with_app(id, sys, |a, cx| paths = a.drag_source(x - c.x, y - c.y, cx).unwrap_or_default());
                src = Some(id);
            }
            Hit::Icon(i) if self.icon_path(i).is_some() => {
                if !self.sel.contains(&i) {
                    self.set_sel(vec![i]);
                }
                self.anchor = i;
                paths = self.sel_paths();
            }
            _ => {}
        }
        if !paths.is_empty() {
            self.begin_dnd(src, paths, true, sys);
            if let Some(p) = self.rpress.as_mut() {
                p.dragging = true;
            }
        }
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
                // Sobre un icono ya seleccionado se conserva la selección; si no, se selecciona solo.
                if !self.sel.contains(&i) {
                    self.set_sel(vec![i]);
                }
                self.anchor = i;
                let items = self.icon_items(sys);
                self.open_menu(None, MENU_ICON, x, y, items);
            }
            _ => {
                self.set_sel(Vec::new());
                let items = self.bg_items(sys);
                self.open_menu(None, MENU_DESK, x, y, items);
            }
        }
    }

    /// Cambia la selección (índices ordenados y sin repetir) ensuciando solo los iconos que cambian.
    fn set_sel(&mut self, v: Vec<usize>) {
        if self.sel == v {
            return;
        }
        let old = core::mem::replace(&mut self.sel, v);
        let changed: Vec<usize> = old.iter().filter(|i| !self.sel.contains(i)).chain(self.sel.iter().filter(|i| !old.contains(i))).copied().collect();
        for i in changed {
            self.dmg(self.icon_cell(i));
        }
    }

    /// Rutas de los iconos seleccionados que son archivos o carpetas (no apps ni Papelera).
    fn sel_paths(&self) -> Vec<String> {
        self.sel.iter().filter_map(|&i| self.icon_path(i)).collect()
    }

    /// Ruta del icono `i` si es un archivo o carpeta de la carpeta Escritorio del usuario (no app ni Papelera).
    fn icon_path(&self, i: usize) -> Option<String> {
        match self.icons.get(i).map(|d| &d.act) {
            Some(Act::Path(p)) if p != files::TRASH => Some(p.clone()),
            _ => None,
        }
    }

    fn icon_items(&self, sys: &mut dyn Platform) -> Vec<MenuItem> {
        if self.sel.len() > 1 {
            let none = self.sel_paths().is_empty();
            return vec![
                MenuItem::new(M_OPEN, "Abrir").bold().disabled(true),
                MenuItem::new(M_NOTEPAD, "Abrir con").disabled(true),
                MenuItem::sep(),
                MenuItem::new(M_CUT, "Cortar").disabled(none),
                MenuItem::new(M_COPY, "Copiar").disabled(none),
                MenuItem::sep(),
                MenuItem::new(M_DELETE, "Eliminar").disabled(none),
                MenuItem::new(M_RENAME, "Cambiar nombre").disabled(true),
                MenuItem::sep(),
                MenuItem::new(M_PROPS, "Propiedades"),
            ];
        }
        let Some(&i) = self.sel.first() else { return Vec::new() };
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
        if tag == MENU_DROP {
            if let Some((act, paths)) = self.pending_rdrop.take() {
                match id {
                    M_MOVE_HERE => self.run_drop_act(act, paths, false, sys),
                    M_COPY_HERE => self.run_drop_act(act, paths, true, sys),
                    _ => {}
                }
            }
        } else if tag == MENU_ICON {
            if self.sel.len() > 1 {
                self.multi_action(id, sys);
            } else if let Some(&i) = self.sel.first() {
                self.icon_action(i, id, sys);
            }
        } else {
            self.bg_action(id, sys);
        }
        self.refresh_icons(sys);
    }

    /// Órdenes del menú con varios iconos seleccionados (los de apps y la Papelera se ignoran).
    fn multi_action(&mut self, id: u32, sys: &mut dyn Platform) {
        let paths = self.sel_paths();
        match id {
            M_CUT | M_COPY if !paths.is_empty() => sys.files().set_clip_many(&paths, id == M_CUT),
            M_DELETE if !paths.is_empty() => self.ask_trash(paths, sys),
            M_PROPS => {
                let text = format!("{} elementos seleccionados.", self.sel.len());
                self.info("Propiedades", &text, sys);
            }
            _ => {}
        }
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
        self.ask_trash(vec![String::from(path)], sys);
    }

    /// Pregunta antes de enviar a la Papelera; con Sí sigue como soltar en la Papelera (que avisa
    /// además si toca `C:\Sistema`).
    fn ask_trash(&mut self, paths: Vec<String>, sys: &mut dyn Platform) {
        let text = match paths.as_slice() {
            [one] => format!("¿Quieres enviar «{}» a la Papelera?", files::file_name(one)),
            many => format!("¿Quieres enviar estos {} elementos a la Papelera?", many.len()),
        };
        self.pending_del = Some(paths);
        let req = DialogRequest::Message { tag: TAG_DEL, title: String::from("Eliminar"), text, buttons: Buttons::YesNo };
        self.open_dialog(None, req, sys);
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
        let paths = self.sel_paths();
        match k.key {
            Key::Char('z') if k.ctrl => self.undo(sys),
            Key::Char('a') if k.ctrl => {
                self.set_sel((0..self.icons.len()).collect());
                self.anchor = 0;
            }
            Key::Delete if !paths.is_empty() => self.ask_trash(paths, sys),
            Key::F(2) if self.sel.len() == 1 && paths.len() == 1 => self.start_rename(&paths[0], sys),
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

    fn toggle_start(&mut self, sys: &mut dyn Platform) {
        if !self.start_open {
            self.start_items = launchers(sys);
            // shortcut: el menú no se desplaza; caben 8 mosaicos. Con más programas, lista con barra.
            self.start_items.truncate(8);
        }
        self.set_start(!self.start_open);
    }

    /// Abre el elemento `i` del menú Inicio.
    fn start_item(&mut self, i: usize, sys: &mut dyn Platform) {
        match self.start_items.get(i).map(|l| &l.act) {
            Some(Act::App(k)) => {
                let k = *k;
                self.launch(k, sys);
            }
            Some(Act::Program(p)) => {
                let p = p.clone();
                self.launch_program(&p, &p.binary(), sys);
            }
            Some(Act::Path(p)) => {
                let p = p.clone();
                self.set_start(false);
                self.open_path(&p, sys);
            }
            None => {}
        }
    }

    /// Arranca el programa `p` desde su `.nfx` (`path`). Si no se puede, lo dice en un mensaje.
    pub fn launch_program(&mut self, p: &Program, path: &str, sys: &mut dyn Platform) {
        self.set_start(false);
        self.set_shut(false);
        match programs::open(sys, p, path) {
            Ok(app) => {
                self.next_size = Some(p.size);
                let id = self.push_window(AppKind::Program, Box::new(app), None);
                if let Some(i) = self.pos(id) {
                    self.wins[i].pic = p.picture.clone();
                }
            }
            Err(e) => {
                let text = format!("No se puede abrir «{}».\n{e}", p.name);
                self.open_dialog(None, DialogRequest::Message { tag: TAG_INFO, title: p.name.clone(), text, buttons: Buttons::Ok }, sys);
            }
        }
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
        if matches!(kind, AppKind::Settings | AppKind::TaskManager) {
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
            None => self.next_size.take().unwrap_or(apps::default_size(kind)),
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
            pic: None,
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
        let drag = cx.drag.take();
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
        if let Some(req) = drag.filter(|r| !r.paths.is_empty() && self.drag == Drag::Content(id)) {
            self.drag_req = Some((id, req.paths));
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
        if let Some(p) = programs::for_binary(sys, path) {
            self.launch_program(&p, path, sys);
        } else if path == files::TRASH || sys.files().stat(path).is_ok_and(|e| e.dir) {
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
        self.send_at(id, self.mx, self.my, sys, f);
    }

    /// Igual que `send` pero con el punto de pantalla dado (no el del puntero).
    fn send_at(&mut self, id: u32, px: i32, py: i32, sys: &mut dyn Platform, f: impl Fn(i32, i32) -> MouseEv) {
        let Some(i) = self.pos(id) else { return };
        let c = self.wins[i].content_rect();
        let ev = f(px - c.x, py - c.y);
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
                    // El disco todavía no se toca: el asistente pregunta el cifrado y las cuentas y lo
                    // formatea al final. Se descarta ya la sesión de la unidad en memoria.
                    let ids: Vec<u32> = self.wins.iter().map(|w| w.id).collect();
                    for id in ids {
                        self.close(id);
                    }
                    self.login.start_new_disk();
                    self.screen_on();
                    self.flush_login();
                    return;
                } else {
                    // Solo el Sí formatea: cualquier otra respuesta descarta el disco.
                    sys.files().decline_format();
                }
                self.show_notice(sys);
            }
            DialogResult::Answer { tag: TAG_SYS, .. } => self.show_notice(sys),
            DialogResult::Answer { tag: TAG_SETUP, answer } => {
                if answer == Answer::Ok {
                    self.login.begin_provision();
                } else {
                    self.login.cancel_entropy();
                }
                self.screen_on();
                self.flush_login();
            }
            DialogResult::Answer { tag: t @ TAG_POWER..=0xFFFF_FF23, answer: Answer::Yes } => self.do_power(t - TAG_POWER, sys),
            DialogResult::Answer { tag: u32::MAX, .. } => self.enter_flow(sys),
            DialogResult::Answer { tag: TAG_EMPTY, answer: Answer::Yes } => {
                if let Err(e) = sys.files().empty_trash() {
                    self.info("No se puede vaciar la Papelera", e.message(), sys);
                }
                self.refresh_icons(sys);
            }
            DialogResult::Answer { tag: TAG_DEL, answer } => {
                if let (Answer::Yes, Some(paths)) = (answer, self.pending_del.take()) {
                    self.desk_drop(Dest::Trash, paths, false, sys);
                }
                self.pending_del = None;
            }
            DialogResult::Answer { tag: TAG_DROP, answer } => {
                if let Some((dest, paths, copy)) = self.pending_drop.take() {
                    if answer == Answer::Yes {
                        self.run_drop(dest, paths, copy, sys);
                    }
                }
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
        // Con un disco cifrado esperando su clave, el aviso de "solo en memoria" sobra: primero
        // va la pantalla del disco (si elige seguir sin él, ya se le advierte allí).
        if let Some(text) = sys.files().notice().filter(|_| !sys.files().disk_locked()) {
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
        if let Some(d) = self.dnd.as_mut() {
            d.dead |= d.src == Some(id);
            if d.over == Some(id) {
                d.over = None;
            }
        }
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
            Drag::None | Drag::Icon { .. } | Drag::Band(..) | Drag::Files | Drag::Dead => false,
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
            && new.iter().zip(&self.icons).all(|(a, b)| a.name == b.name && a.label == b.label && a.icon == b.icon && a.pic == b.pic);
        // Mismo aspecto pero otro destino (la app interna pasa a ser un programa): se cambia sin repintar.
        if same {
            self.icons = new;
            return;
        }
        for i in 0..self.icons.len().max(new.len()) {
            self.dmg(self.icon_cell(i));
        }
        let names: Vec<String> = self.sel.iter().filter_map(|&s| self.icons.get(s)).map(|d| d.name.clone()).collect();
        let anchor = self.icons.get(self.anchor).map(|d| d.name.clone());
        self.icons = new;
        self.sel = self.icons.iter().enumerate().filter(|(_, d)| names.contains(&d.name)).map(|(i, _)| i).collect();
        self.anchor = anchor.and_then(|n| self.icons.iter().position(|d| d.name == n)).unwrap_or(0);
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
            Some(Act::Program(p)) => {
                let p = p.clone();
                self.launch_program(&p, &p.binary(), sys);
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
            for i in 0..self.start_items.len() {
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
        if self.dnd.as_ref().is_some_and(|d| d.dead) {
            self.cancel_dnd(sys);
        }
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
                    2 => self.lock(sys),
                    _ => self.power(i as u32, sys),
                }
            }
            Hit::MPower => self.set_shut(!shut_was),
            Hit::MSettings => self.launch(AppKind::Settings, sys),
            Hit::Row(i) | Hit::Tile(i) => self.start_item(i, sys),
            Hit::Start => self.toggle_start(sys),
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
            Hit::Icon(i) => self.press_icon(i, hit, sys),
            Hit::None => {
                // Fondo: empieza el recuadro (con Ctrl se suma a lo seleccionado).
                if self.login.mode == Mode::Off {
                    self.band_base = if self.mods.0 { self.sel.clone() } else { Vec::new() };
                    self.set_sel(self.band_base.clone());
                    self.drag = Drag::Band(self.mx, self.my);
                } else {
                    self.set_sel(Vec::new());
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
                    self.drag_req = None;
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
            Drag::Icon { i, collapse: true, .. } => self.set_sel(vec![i]),
            Drag::Band(..) => self.end_band(),
            Drag::Files => self.end_dnd(true, sys),
            _ => {}
        }
    }

    /// Pulsación sobre el icono `i`: selección (Ctrl suma o quita, Mayús rango) y posible inicio de arrastre.
    fn press_icon(&mut self, i: usize, hit: Hit, sys: &mut dyn Platform) {
        let (ctrl, shift) = self.mods;
        let mut collapse = false;
        if shift {
            let (a, b) = (self.anchor.min(i), self.anchor.max(i));
            let mut v: Vec<usize> = (a..=b).collect();
            if ctrl {
                v.extend(self.sel.iter().copied());
                v.sort_unstable();
                v.dedup();
            }
            self.set_sel(v);
        } else if ctrl {
            let mut v = self.sel.clone();
            match v.iter().position(|&x| x == i) {
                Some(p) => {
                    v.remove(p);
                }
                None => {
                    v.push(i);
                    v.sort_unstable();
                }
            }
            self.set_sel(v);
            self.anchor = i;
        } else {
            self.anchor = i;
            if self.sel.contains(&i) {
                collapse = self.sel.len() > 1;
            } else {
                self.set_sel(vec![i]);
            }
            if self.double(hit) {
                self.set_sel(vec![i]);
                self.activate_icon(i, sys);
                return;
            }
        }
        if self.sel.contains(&i) && self.icon_path(i).is_some() {
            self.drag = Drag::Icon { i, x: self.mx, y: self.my, collapse };
        }
    }

    // ---------------------------------------------------------------- arrastre de archivos

    /// Qué hay bajo el puntero como destino de `paths`.
    fn dnd_target(&self, paths: &[String]) -> Tgt {
        match self.hit(self.mx, self.my) {
            Hit::Content(id) if self.modal_of(id).is_none() && self.pos(id).is_some_and(|i| self.wins[i].dlg.is_none()) => Tgt::Win(id),
            Hit::None if self.login.mode == Mode::Off => Tgt::Desk,
            Hit::Icon(i) => match self.icons.get(i) {
                Some(DIcon { act: Act::Path(p), icon, .. }) if (p == files::TRASH || *icon == Icon::Folder) && !paths.contains(p) => Tgt::Icon(i),
                _ => Tgt::Nothing,
            },
            _ => Tgt::Nothing,
        }
    }

    /// El escritorio toma el arrastre (de la ventana `src`, o de sus propios iconos si es `None`).
    fn begin_dnd(&mut self, src: Option<u32>, paths: Vec<String>, right: bool, sys: &mut dyn Platform) {
        self.drag = Drag::Files;
        self.dnd = Some(Dnd { paths, src, over: None, ok: false, dead: false, right });
        self.update_dnd(sys);
    }

    /// Con el puntero en una posición nueva: avisa a la ventana que deja y a la que entra, y
    /// resalta el icono destino.
    fn update_dnd(&mut self, sys: &mut dyn Platform) {
        let Some(d) = self.dnd.as_ref() else { return };
        let paths = d.paths.clone();
        let tgt = self.dnd_target(&paths);
        let win = if let Tgt::Win(id) = tgt { Some(id) } else { None };
        if d.over != win {
            if let Some(o) = d.over {
                self.with_app(o, sys, |a, cx| {
                    a.drag_over(None, &paths, cx);
                });
            }
            if let Some(d) = self.dnd.as_mut() {
                d.over = win;
            }
        }
        let mut ok = matches!(tgt, Tgt::Desk | Tgt::Icon(_));
        if let Some(id) = win {
            if let Some(i) = self.pos(id) {
                let c = self.wins[i].content_rect();
                let pos = (self.mx - c.x, self.my - c.y);
                self.with_app(id, sys, |a, cx| ok = a.drag_over(Some(pos), &paths, cx));
            }
        }
        if let Some(d) = self.dnd.as_mut() {
            d.ok = ok;
        }
        let hl = if let Tgt::Icon(i) = tgt { Some(i) } else { None };
        if hl != self.dnd_hl {
            for i in self.dnd_hl.into_iter().chain(hl) {
                self.dmg(self.icon_cell(i));
            }
            self.dnd_hl = hl;
        }
        self.dnd_paint();
    }

    /// Texto de la etiqueta que acompaña al puntero.
    fn dnd_text(&self) -> String {
        let Some(d) = &self.dnd else { return String::new() };
        let mut t = match d.paths.as_slice() {
            [one] => String::from(dialogs::fit(files::file_name(one), 8, 220)),
            v => format!("{} elementos", v.len()),
        };
        if self.mods.0 && !d.right {
            t.push_str("  + Copiar");
        }
        t
    }

    fn dnd_rect(&self) -> Rect {
        if self.dnd.is_none() {
            return Rect::default();
        }
        let (w, h) = (font::width(&self.dnd_text(), 8, false) + 16, font::line_h(8) + 8);
        Rect::new((self.mx + 14).min(self.w - w).max(0), (self.my + 18).min(self.h - h).max(0), w, h)
    }

    /// Ensucia la etiqueta donde estaba y donde está ahora.
    fn dnd_paint(&mut self) {
        let n = self.dnd_rect();
        self.dmg(self.dnd_lbl);
        self.dmg(n);
        self.dnd_lbl = n;
    }

    fn paint_dnd(&self, s: &mut Surface) {
        let Some(d) = &self.dnd else { return };
        let r = self.dnd_rect();
        if !r.intersects(s.clip()) {
            return;
        }
        if d.ok {
            s.fill_rect(r, 0xF2F2F2);
            s.frame(r, self.theme.accent);
            text_in(s, r, &self.dnd_text(), 8, 0x000000, Align::Center);
        } else {
            s.blend_rect(r, 0xF2F2F2, 130);
            s.frame(r, 0x9A9A9A);
            text_in(s, r, &self.dnd_text(), 8, 0x6E6E6E, Align::Center);
        }
    }

    /// Termina el arrastre; con `drop` y un destino que lo admite, hace lo que toque.
    fn end_dnd(&mut self, drop: bool, sys: &mut dyn Platform) {
        let Some(d) = self.dnd.take() else { return };
        if self.drag == Drag::Files {
            self.drag = Drag::None;
        }
        let copy = self.mods.0;
        let tgt = if drop && !d.dead && !self.login.covers() { self.dnd_target(&d.paths) } else { Tgt::Nothing };
        let act = match tgt {
            Tgt::Win(id) if d.over == Some(id) && d.ok => self.pos(id).map(|i| {
                let c = self.wins[i].content_rect();
                DropAct::Win(id, self.mx - c.x, self.my - c.y)
            }),
            Tgt::Desk => Some(DropAct::To(Dest::Dir(sys.files().std_dir(Std::Desktop)))),
            Tgt::Icon(i) => match self.icons.get(i).map(|x| &x.act) {
                Some(Act::Path(p)) if p == files::TRASH => Some(DropAct::To(Dest::Trash)),
                Some(Act::Path(p)) => Some(DropAct::To(Dest::Dir(p.clone()))),
                _ => None,
            },
            _ => None,
        };
        match act {
            Some(act) if d.right => self.open_drop_menu(act, d.paths.clone()),
            Some(act) => self.run_drop_act(act, d.paths.clone(), copy, sys),
            None => {}
        }
        if let Some(o) = d.over {
            self.with_app(o, sys, |a, cx| {
                a.drag_over(None, &d.paths, cx);
            });
        }
        self.dmg(self.dnd_lbl);
        self.dnd_lbl = Rect::default();
        if let Some(i) = self.dnd_hl.take() {
            self.dmg(self.icon_cell(i));
        }
    }

    /// Hace lo que toque al soltar `paths` en `act` (mover, o copiar si `copy`).
    fn run_drop_act(&mut self, act: DropAct, paths: Vec<String>, copy: bool, sys: &mut dyn Platform) {
        match act {
            DropAct::Win(id, x, y) => {
                self.raise(id);
                self.with_app(id, sys, |a, cx| a.drop_files(x, y, &paths, copy, cx));
            }
            DropAct::To(dest) => self.desk_drop(dest, paths, copy, sys),
        }
    }

    /// Soltar con el botón derecho: el menú en el puntero decide si se mueve o se copia.
    fn open_drop_menu(&mut self, act: DropAct, paths: Vec<String>) {
        let items = if act == DropAct::To(Dest::Trash) {
            vec![MenuItem::new(M_MOVE_HERE, "Mover a la Papelera").bold(), MenuItem::sep(), MenuItem::new(M_CANCEL_DROP, "Cancelar")]
        } else {
            vec![
                MenuItem::new(M_MOVE_HERE, "Mover aquí").bold(),
                MenuItem::new(M_COPY_HERE, "Copiar aquí"),
                MenuItem::sep(),
                MenuItem::new(M_CANCEL_DROP, "Cancelar"),
            ]
        };
        self.pending_rdrop = Some((act, paths));
        let (x, y) = (self.mx, self.my);
        self.open_menu(None, MENU_DROP, x, y, items);
    }

    /// Cancela el arrastre en curso (Esc, ventana de origen cerrada, bloqueo...) sin hacer nada.
    fn cancel_dnd(&mut self, sys: &mut dyn Platform) {
        if self.dnd.is_none() {
            return;
        }
        let files = self.drag == Drag::Files;
        self.end_dnd(false, sys);
        if files {
            self.drag = Drag::Dead;
        }
    }

    /// Mover, copiar o eliminar lo arrastrado; si toca `C:\Sistema` pregunta antes.
    fn desk_drop(&mut self, dest: Dest, paths: Vec<String>, copy: bool, sys: &mut dyn Platform) {
        let touches = match &dest {
            Dest::Dir(d) => files::in_system(d) || (!copy && paths.iter().any(|p| files::in_system(p))),
            Dest::Trash => paths.iter().any(|p| files::in_system(p)),
        };
        if touches {
            self.pending_drop = Some((dest, paths, copy));
            let text = "Vas a modificar una carpeta del sistema. Si cambias o borras algo aquí, NightFury puede dejar de funcionar. ¿Continuar?";
            let req = DialogRequest::Message { tag: TAG_DROP, title: String::from("Carpeta del sistema"), text: String::from(text), buttons: Buttons::YesNo };
            self.open_dialog(None, req, sys);
        } else {
            self.run_drop(dest, paths, copy, sys);
        }
    }

    fn run_drop(&mut self, dest: Dest, paths: Vec<String>, copy: bool, sys: &mut dyn Platform) {
        let now = sys.now();
        let (res, what) = match &dest {
            Dest::Dir(d) => (sys.files().transfer(&paths, d, !copy, now).map(|_| ()), if copy { "No se puede copiar" } else { "No se puede mover" }),
            Dest::Trash => (sys.files().user_trash_many(&paths, now), "No se puede eliminar"),
        };
        if let Err(e) = res {
            self.info(what, e.message(), sys);
        }
        self.refresh_icons(sys);
    }

    /// Recuadro de selección: sigue al puntero y selecciona los iconos que toca.
    fn band_move(&mut self, x0: i32, y0: i32) {
        let (x, y) = (self.mx, self.my);
        let shown = (x - x0).abs() >= BAND_PX || (y - y0).abs() >= BAND_PX || self.band.is_some();
        let new = shown.then(|| Rect::new(x0.min(x), y0.min(y), (x0 - x).abs() + 1, (y0 - y).abs() + 1));
        if new == self.band {
            return;
        }
        if let Some(o) = self.band {
            self.dmg(o);
        }
        if let Some(n) = new {
            self.dmg(n);
            let mut v = self.band_base.clone();
            v.extend((0..self.icons.len()).filter(|&i| self.icon_cell(i).intersects(n)));
            v.sort_unstable();
            v.dedup();
            self.set_sel(v);
        }
        self.band = new;
    }

    fn end_band(&mut self) {
        if let Some(b) = self.band.take() {
            self.dmg(b);
        }
        self.band_base.clear();
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
            if self.dnd.is_some() {
                self.dnd_paint();
            }
            self.sync_mods(sys);
            self.after(sys);
            return;
        }
        if !k.down {
            return;
        }
        let fine = sys.entropy();
        sys.pool().add_event(key_code(k.key), 0, self.now_ms, fine);
        if self.dnd.is_some() && !self.login.covers() {
            // Durante un arrastre solo vale Esc (cancelar); el resto del teclado no se usa.
            if k.key == Key::Escape {
                self.cancel_dnd(sys);
            }
            self.after(sys);
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
            self.toggle_start(sys);
        } else if k.key == Key::Escape && k.ctrl && k.shift {
            // Ctrl+Mayús+Esc: Administrador de tareas desde cualquier sitio (trae al frente el abierto).
            self.launch(AppKind::TaskManager, sys);
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
        let out = self.login.tick(ms, sys);
        self.login_out(out, sys);
        if !self.booted {
            self.booted = true;
            self.boot_flow(sys);
        }
        self.convert_tick(sys);
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
            if let Some(b) = self.band {
                s.blend_rect(b, self.theme.accent, 70);
                s.frame(b, lighten(self.theme.accent, 70));
            }
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
                self.paint_conv(s);
                if self.start_open {
                    self.paint_menu(s);
                }
                if self.shut_open {
                    self.paint_shut(s);
                }
                if let Some(m) = &self.menu {
                    m.menu.draw(s);
                }
                self.paint_dnd(s);
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
        for (i, DIcon { label: name, icon, pic, .. }) in self.icons.iter().enumerate() {
            let c = self.icon_cell(i);
            if !c.intersects(s.clip()) {
                continue;
            }
            if self.sel.contains(&i) {
                s.blend_rect(c, self.theme.accent, 110);
                s.frame(c, lighten(self.theme.accent, 70));
            }
            if self.dnd_hl == Some(i) {
                // Destino del arrastre: más fuerte que la selección.
                s.blend_rect(c, self.theme.accent, 190);
                s.frame(c, 0xFFFFFF);
            }
            let (_, _, isz, iy, ty) = self.size.metrics();
            paint_icon(s, *icon, pic.as_ref(), c.x + (c.w - isz) / 2, c.y + iy, isz, 0xFFFFFF);
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
        let icon = w.app.icon().or_else(|| APPS.iter().find(|a| a.0 == w.kind).map(|a| a.2));
        if let Some(icon) = icon {
            paint_icon(s, icon, w.pic.as_ref(), tb.x + 8, tb.y + 7, 16, 0x444444);
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
            if let Some(icon) = self.wins[wi].app.icon().or_else(|| APPS.iter().find(|a| a.0 == self.wins[wi].kind).map(|a| a.2)) {
                paint_icon(s, icon, self.wins[wi].pic.as_ref(), r.x + 12, r.y + 8, 24, 0xFFFFFF);
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
        for (i, Launcher { name, icon, pic, .. }) in self.start_items.iter().enumerate() {
            let r = self.row_rect(i);
            if self.hot == Hit::Row(i) {
                s.fill_rect(r, ACTIVE_BTN);
            }
            paint_icon(s, *icon, pic.as_ref(), r.x + 12, r.y + 8, 24, 0xFFFFFF);
            text_in(s, Rect::new(r.x + 50, r.y, r.w - 50, r.h), name, 9, 0xFFFFFF, Align::Left);
        }
        text_in(s, Rect::new(m.x + 300, m.y + 10, 200, 24), "Aplicaciones", 8, 0xB5B5B5, Align::Left);
        for (i, Launcher { name, icon, pic, .. }) in self.start_items.iter().enumerate() {
            let r = self.tile_rect(i);
            let bg = if self.hot == Hit::Tile(i) { lighten(self.theme.accent, 45) } else { self.theme.accent };
            s.fill_rect(r, bg);
            paint_icon(s, *icon, pic.as_ref(), r.x + 34, r.y + 16, 32, 0xFFFFFF);
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

/// Ruta de `name` dentro de la carpeta estándar `d` del usuario actual (para pruebas).
#[cfg(test)]
fn sp(f: &files::Files, d: Std, name: &str) -> String {
    files::join(&f.std_dir(d), name)
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
        click!(d, sys, center(d.row_rect(2)));
        assert_eq!(d.window_count(), 1);
        assert!(!d.start_is_open());
        assert_eq!(kinds(&d), [AppKind::Calc]);
        click!(d, sys, center(d.start_rect()));
        click!(d, sys, center(d.tile_rect(5)));
        assert_eq!(kinds(&d), [AppKind::Calc, AppKind::Terminal]);
        assert!(!d.start_is_open());
    }

    #[test]
    fn task_manager_ctrl_shift_esc_single_window_no_desktop_icon() {
        let (mut d, mut sys) = setup(1280, 800);
        assert!(d.icons.iter().all(|i| i.name != "Administrador de tareas"), "sin icono en el escritorio");
        let atajo = Event::Key(KeyEvent { key: Key::Escape, ch: None, down: true, shift: true, ctrl: true, alt: false });
        d.event(atajo, &mut sys);
        assert_eq!(kinds(&d), [AppKind::TaskManager]);
        // Con otra ventana delante, el atajo trae la misma al frente en vez de abrir otra.
        d.launch(AppKind::Notepad, &mut sys);
        d.event(atajo, &mut sys);
        assert_eq!(kinds(&d), [AppKind::Notepad, AppKind::TaskManager], "no se duplica");
        assert_eq!(d.wins.last().unwrap().kind, AppKind::TaskManager);
        // Esc solo no lo abre, y desde Inicio sí está.
        d.event(key(Key::Escape, false), &mut sys);
        assert_eq!(d.window_count(), 2);
        assert!(launchers(&mut sys).iter().any(|l| l.name == "Administrador de tareas"));
    }

    #[test]
    fn icono_propio_sustituye_al_del_sistema() {
        let mut s = Surface::new(8, 8, 0x000000);
        let pic = Picture { w: 1, h: 1, argb: vec![0xFF12_3456] };
        paint_icon(&mut s, Icon::Logo, Some(&pic), 0, 0, 4, 0xFFFFFF);
        assert!(s.px[..4].iter().all(|&p| p == 0x123456), "el BMP llena el cuadrado");
        assert_eq!(s.px[4], 0, "y no se sale");
        paint_icon(&mut s, Icon::Logo, None, 4, 4, 4, 0xFFFFFF);
        assert!(s.px.iter().any(|&p| p != 0 && p != 0x123456), "sin BMP, el del sistema");
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
        assert_eq!((d.window_count(), d.sel.clone()), (0, vec![0]));
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

    #[test]
    fn programas_en_inicio_y_al_abrir() {
        let (mut d, mut sys) = setup();
        let n = sys.now();
        sys.files.install_system_program("Calculadora", &[("app.txt", b"nombre=Calculadora\nicono=calc\n"), ("programa.nfx", b"NFX1")], n).unwrap();
        sys.files.install_system_program("Plantilla", &[("app.txt", b"nombre=Plantilla\n"), ("programa.nfx", b"NFX1")], n).unwrap();
        let names: Vec<String> = launchers(&mut sys).into_iter().map(|l| l.name).collect();
        assert_eq!(names, ["Administrador de tareas", "Bloc de notas", "Calculadora", "Configuración", "Explorador de archivos", "Plantilla", "Terminal"]);
        assert!(matches!(launchers(&mut sys)[2].act, Act::Program(_)), "la Calculadora es ya el programa");
        // Abrirlo: la plataforma de pruebas no ejecuta programas, así que sale un mensaje.
        d.toggle_start(&mut sys);
        assert_eq!(d.start_items.len(), 7);
        d.start_item(2, &mut sys);
        assert!(!d.start_open);
        assert_eq!(d.dialog_count(), 1);
        // Un .nfx abierto desde el Explorador o la Terminal pasa por el mismo camino.
        d.open_path("C:\\Programas\\Plantilla\\programa.nfx", &mut sys);
        assert_eq!(d.dialog_count(), 2);
        assert!(d.wins.iter().all(|w| w.kind != AppKind::Notepad), "no se abre como texto");
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
        // Con el asistente nuevo el disco se formatea al final: antes se pregunta el cifrado.
        assert!(sys.files.pending_format() && !sys.files.persistent());
        assert_eq!(d.dialog_count(), 0, "sin aviso de memoria");
        assert_eq!(d.login_mode(), Mode::Setup);
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
        sys.files.write(&sys.files.config_file(), b"fondo=2\nenfasis=D13B3B\n", now).unwrap();
        let d = Desktop::new(1280, 800, &mut sys);
        assert_eq!((d.theme.wallpaper, d.theme.accent), (2, 0xD13B3B));
        // Valores no válidos se ignoran.
        sys.files.write(&sys.files.config_file(), b"fondo=9\nenfasis=zz\nbasura\n", now).unwrap();
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
        assert_eq!(sys.files.read(&sys.files.config_file()).unwrap(), b"fondo=1\nenfasis=6B8E23\niconos=mediano\norden=nombre\n");
        let d2 = Desktop::new(1280, 800, &mut sys);
        assert_eq!((d2.theme.wallpaper, d2.theme.accent), (1, ACCENTS[3]));
    }

    #[test]
    fn desktop_folder_items_become_icons() {
        let mut sys = TestPlatform { size: (1280, 400), ..Default::default() };
        let now = sys.now();
        sys.files.mkdir(&sp(&sys.files, Std::Desktop, "Carpeta"), now).unwrap();
        sys.files.write(&sp(&sys.files, Std::Desktop, "nota.txt"), b"hola", now).unwrap();
        sys.files.write(&sp(&sys.files, Std::Desktop, "datos.bin"), b"1", now).unwrap();
        let mut d = Desktop::new(1280, 400, &mut sys);
        d.booted = true;
        let names: Vec<&str> = d.icons.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names[..6], ["Bloc de notas", "Calculadora", "Configuración", "Explorador de archivos", "Terminal", "Papelera"]);
        assert_eq!(names[6..], ["Carpeta", "datos.bin", "nota.txt"]);
        assert_eq!([d.icons[5].icon, d.icons[6].icon, d.icons[7].icon, d.icons[8].icon], [Icon::Trash, Icon::Folder, Icon::FileBlank, Icon::FileText]);
        // Columnas: con 400 px de alto caben 4 por columna.
        assert_eq!((d.icon_cell(3).x, d.icon_cell(4).x, d.icon_cell(4).y), (6, 90, 6));
        // Se refrescan al cambiar los archivos.
        sys.files.write(&sp(&sys.files, Std::Desktop, "z.txt"), b"z", now).unwrap();
        d.event(Event::Tick(10), &mut sys);
        assert_eq!(d.icons.len(), 10);
        sys.files.delete(&sp(&sys.files, Std::Desktop, "z.txt")).unwrap();
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
        p.open = Some(String::from(sys.files.std_dir(Std::Documents)));
        d.launch_app(p, &mut sys);
        key(&mut d, &mut sys, Key::Char('o'), Some('o'), false);
        assert_eq!(kinds(&d), [AppKind::Notepad, AppKind::Files]);
        let now = sys.now();
        sys.files.write(&sp(&sys.files, Std::Documents, "a.txt"), b"a", now).unwrap();
        let (mut p, _) = probe(None, false);
        p.open = Some(String::from(&sp(&sys.files, Std::Documents, "a.txt")));
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
        sys.files.write(&sp(&sys.files, Std::Documents, "a.txt"), b"a", now).unwrap();
        d.render(&mut sys);
        let check = |d: &mut Desktop, sys: &mut TestPlatform, what: &str| {
            d.render(sys);
            let inc = d.back().px.clone();
            d.invalidate();
            d.render(sys);
            assert!(inc == d.back().px, "incremental distinto del completo: {what}");
        };
        let req = DialogRequest::Save { tag: 2, title: String::from("Guardar como"), dir: String::from(sys.files.std_dir(Std::Documents)), name: String::from("x") };
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
                cx.open_new = Some(cx.sys.files().std_dir(Std::Documents));
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
        assert_eq!(log1.borrow().len(), 1); // el menú sale al soltar, no al pulsar
        assert_eq!(d.active_id(), Some(d.wins[1].id));
        d.event(Event::Mouse(crate::input::MousePacket { dx: 0, dy: 0, buttons: 0, dz: 0 }), &mut sys);
        assert_eq!(log1.borrow()[1], "derecho 10,10");
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
        d.with_app(id, &mut sys, |_, cx| cx.open_new = Some(cx.sys.files().std_dir(Std::Documents)));
        assert_eq!(explorers(&d), 2);
        d.with_app(id, &mut sys, |_, cx| cx.open_new = Some(cx.sys.files().std_dir(Std::Documents)));
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
        sys.files.mkdir(&sp(&sys.files, Std::Desktop, "Carpeta"), now).unwrap();
        sys.files.write(&sp(&sys.files, Std::Desktop, "nota.txt"), b"hola", now).unwrap();
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
                d.sel = vec![i];
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

    fn items_of(d: &mut Desktop, i: usize, sys: &mut TestPlatform) -> Vec<MenuItem> {
        d.sel = vec![i];
        d.icon_items(sys)
    }

    fn answer_text(d: &mut Desktop, sys: &mut TestPlatform, tag: u32, v: &str) {
        d.system_result(DialogResult::Text { tag, value: Some(String::from(v)) }, sys);
    }

    #[test]
    fn icon_menus_have_the_right_items() {
        let (mut d, mut sys) = setup();
        assert_eq!(d.icons[6].name, "Carpeta");
        assert_eq!(d.icons[7].name, "nota.txt");
        // App: solo Abrir, en negrita.
        let it = items_of(&mut d, 0, &mut sys);
        assert_eq!(texts(&it), ["Abrir"]);
        assert!(it[0].bold);
        // Papelera vacía: Vaciar desactivado; con algo dentro, activo.
        let it = items_of(&mut d, 5, &mut sys);
        assert_eq!(texts(&it), ["Abrir", "Vaciar Papelera"]);
        assert!(it[1].disabled);
        let now = sys.now();
        sys.files.write(&sp(&sys.files, Std::Desktop, "viejo.txt"), b"x", now).unwrap();
        sys.files.trash(&sp(&sys.files, Std::Desktop, "viejo.txt"), now).unwrap();
        assert!(!items_of(&mut d, 5, &mut sys)[1].disabled);
        // Archivo.
        let it = items_of(&mut d, 7, &mut sys);
        let want = ["Abrir", "Abrir con >", "-", "Cortar", "Copiar", "-", "Eliminar", "Cambiar nombre", "-", "Propiedades"];
        assert_eq!(texts(&it), want);
        assert!(it[0].bold && it.iter().all(|i| !i.disabled));
        assert_eq!(it[1].sub[0].text, "Bloc de notas");
        // Carpeta: Pegar desactivado hasta que haya portapapeles.
        let it = items_of(&mut d, 6, &mut sys);
        let want = ["Abrir", "Abrir en ventana nueva", "-", "Cortar", "Copiar", "Pegar", "-", "Eliminar", "Cambiar nombre", "-", "Propiedades"];
        assert_eq!(texts(&it), want);
        assert!(it[5].disabled);
        sys.files.set_clip(&sp(&sys.files, Std::Desktop, "nota.txt"), false);
        assert!(!items_of(&mut d, 6, &mut sys)[5].disabled);
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
        sys.files.set_clip(&sp(&sys.files, Std::Desktop, "nota.txt"), false);
        let now = sys.now();
        sys.files.user_trash(&sp(&sys.files, Std::Desktop, "nota.txt"), now).unwrap();
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
        assert_eq!(d.sel, vec![7]);
        let m = d.menu().expect("menú del icono");
        let r = m.item_rect(0);
        click_at(&mut d, &mut sys, center(r));
        assert!(d.menu().is_none());
        assert_eq!(d.wins.last().unwrap().kind, AppKind::Notepad);
        // Sobre el fondo: menú del fondo y sin selección.
        d.close(d.wins[0].id);
        right_click(&mut d, &mut sys, (900, 500));
        assert!(d.menu().is_some() && d.sel.is_empty());
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
        assert!(sys.files.exists(&sp(&sys.files, Std::Desktop, "Carpeta\\nota.txt")));
        // Pegar en el fondo: nombre libre.
        pick(&mut d, &mut sys, None, M_PASTE);
        assert!(sys.files.exists(&sp(&sys.files, Std::Desktop, "nota (2).txt")));
        assert_eq!(sys.files.undo_label(), Some("Copiar"));
        pick(&mut d, &mut sys, None, M_UNDO);
        assert!(!sys.files.exists(&sp(&sys.files, Std::Desktop, "nota (2).txt")));
        // Cortar y pegar en la carpeta mueve.
        pick(&mut d, &mut sys, Some(7), M_CUT);
        pick(&mut d, &mut sys, Some(6), M_PASTE_IN);
        assert!(!sys.files.exists(&sp(&sys.files, Std::Desktop, "nota.txt")));
        assert!(sys.files.exists(&sp(&sys.files, Std::Desktop, "Carpeta\\nota (2).txt")));
        pick(&mut d, &mut sys, None, M_UNDO);
        assert!(sys.files.exists(&sp(&sys.files, Std::Desktop, "nota.txt")));
        // Eliminar manda a la Papelera, los iconos se actualizan y se puede deshacer.
        let n = d.icons.len();
        let i = d.icons.iter().position(|x| x.name == "nota.txt").unwrap();
        pick(&mut d, &mut sys, Some(i), M_DELETE);
        assert_eq!(d.dialog_count(), 1, "pregunta antes de eliminar");
        key(&mut d, &mut sys, Key::Enter, None, false); // Sí
        assert!(!sys.files.exists(&sp(&sys.files, Std::Desktop, "nota.txt")));
        assert_eq!(d.icons.len(), n - 1);
        assert_eq!(sys.files.trash_list().unwrap().len(), 2);
        pick(&mut d, &mut sys, None, M_UNDO);
        assert!(sys.files.exists(&sp(&sys.files, Std::Desktop, "nota.txt")));
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
        assert_eq!((d.dialog_count(), d.pending.as_deref()), (1, Some(sp(&sys.files, Std::Desktop, "nota.txt").as_str())));
        answer_text(&mut d, &mut sys, TAG_RENAME, "lista.txt");
        assert!(sys.files.exists(&sp(&sys.files, Std::Desktop, "lista.txt")) && d.pending.is_none());
        assert_eq!(sys.files.undo_label(), Some("Cambiar nombre"));
        // Nombre inválido: error y no cambia nada.
        let before = d.dialog_count();
        pick(&mut d, &mut sys, Some(7), M_RENAME);
        answer_text(&mut d, &mut sys, TAG_RENAME, "a/b");
        assert_eq!(d.dialog_count(), before + 2);
        assert!(sys.files.exists(&sp(&sys.files, Std::Desktop, "lista.txt")));
        // Cancelar no hace nada y limpia lo pendiente.
        pick(&mut d, &mut sys, Some(7), M_RENAME);
        d.system_result(DialogResult::Text { tag: TAG_RENAME, value: None }, &mut sys);
        assert!(d.pending.is_none());
        // Documento sin extensión: se añade .txt. Con extensión se respeta.
        answer_text(&mut d, &mut sys, TAG_NEWDOC, "apuntes");
        assert!(sys.files.exists(&sp(&sys.files, Std::Desktop, "apuntes.txt")));
        answer_text(&mut d, &mut sys, TAG_NEWDOC, "datos.csv");
        assert!(sys.files.exists(&sp(&sys.files, Std::Desktop, "datos.csv")));
        // Ya existe: error.
        let n = d.dialog_count();
        answer_text(&mut d, &mut sys, TAG_NEWDOC, "apuntes");
        assert_eq!(d.dialog_count(), n + 1);
        answer_text(&mut d, &mut sys, TAG_NEWDIR, "Cosas");
        assert!(sys.files.stat(&sp(&sys.files, Std::Desktop, "Cosas")).unwrap().dir);
        assert!(d.icons.iter().any(|i| i.name == "Cosas"));
        pick(&mut d, &mut sys, None, M_UNDO);
        assert!(!sys.files.exists(&sp(&sys.files, Std::Desktop, "Cosas")));
    }

    #[test]
    fn new_folder_dialog_proposes_free_name_and_accepts_typing() {
        let (mut d, mut sys) = setup();
        let now = sys.now();
        sys.files.mkdir(&sp(&sys.files, Std::Desktop, "Nueva carpeta"), now).unwrap();
        pick(&mut d, &mut sys, None, M_NEWDIR);
        assert_eq!(d.dialog_count(), 1);
        key(&mut d, &mut sys, Key::Char('z'), Some('z'), false);
        key(&mut d, &mut sys, Key::Enter, None, false);
        assert_eq!(d.dialog_count(), 0);
        // Proponía "Nueva carpeta (2)" y se le ha añadido la z.
        assert!(sys.files.exists(&sp(&sys.files, Std::Desktop, "Nueva carpeta (2)z")));
    }

    #[test]
    fn empty_trash_asks_first() {
        let (mut d, mut sys) = setup();
        let now = sys.now();
        sys.files.user_trash(&sp(&sys.files, Std::Desktop, "nota.txt"), now).unwrap();
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
        let path = sp(&sys.files, Std::Desktop, "nota.txt");
        let (title, text) = props_text(&mut sys, &path).unwrap();
        assert_eq!(title, "Propiedades de nota.txt");
        let want = format!(
            "Nombre: nota.txt\nTipo: Documento de texto\nUbicación: {}\nTamaño: 4 bytes\nModificado: 15/11/2026 21:37",
            sys.files.std_dir(Std::Desktop)
        );
        assert_eq!(text, want);
        let now = sys.now();
        sys.files.write(&sp(&sys.files, Std::Desktop, "Carpeta\\a.bin"), &[0u8; 2000], now).unwrap();
        let path = sp(&sys.files, Std::Desktop, "Carpeta");
        let (_, text) = props_text(&mut sys, &path).unwrap();
        assert!(text.contains("Tipo: Carpeta de archivos") && text.contains("Contiene: 1 elemento\n"), "{text}");
        let path = sp(&sys.files, Std::Desktop, "Carpeta\\a.bin");
        let (_, text) = props_text(&mut sys, &path).unwrap();
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
        sys.files.mkdir(&sp(&sys.files, Std::Desktop, "Zeta"), t).unwrap();
        t.month = 1;
        sys.files.write(&sp(&sys.files, Std::Desktop, "a.txt"), b"12345", t).unwrap();
        t.month = 3;
        sys.files.write(&sp(&sys.files, Std::Desktop, "b.bin"), b"1", t).unwrap();
        t.month = 2;
        sys.files.write(&sp(&sys.files, Std::Desktop, "c.abc"), b"123", t).unwrap();
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
        d.sel = vec![7];
        pick(&mut d, &mut sys, None, M_SORT + 1);
        assert_eq!(d.icons[d.sel[0]].name, "a.txt");
    }

    #[test]
    fn size_and_order_are_saved_loaded_and_foreign_lines_kept() {
        let (mut d, mut sys) = setup();
        let now = sys.now();
        sys.files.write(&sys.files.config_file(), b"explorador=lista\nfondo=1\n", now).unwrap();
        pick(&mut d, &mut sys, None, M_VIEW);
        pick(&mut d, &mut sys, None, M_SORT + 2);
        let text = String::from_utf8(sys.files.read(&sys.files.config_file()).unwrap()).unwrap();
        assert_eq!(text, "fondo=0\nenfasis=0078D4\niconos=grande\norden=tipo\nexplorador=lista\n".replace("0078D4", &format!("{:06X}", d.theme.accent & 0xFFFFFF)));
        let d2 = Desktop::new(1280, 800, &mut sys);
        assert_eq!((d2.size, d2.order), (IconSize::Big, Order::Kind));
        // Valores no válidos se ignoran.
        sys.files.write(&sys.files.config_file(), b"iconos=enorme\norden=azar\n", now).unwrap();
        let d3 = Desktop::new(1280, 800, &mut sys);
        assert_eq!((d3.size, d3.order), (IconSize::Mid, Order::Name));
        sys.files.write(&sys.files.config_file(), b"iconos=pequeno\norden=fecha\n", now).unwrap();
        let d4 = Desktop::new(1280, 800, &mut sys);
        assert_eq!((d4.size, d4.order), (IconSize::Small, Order::Date));
    }

    #[test]
    fn desktop_keys_undo_delete_rename_only_without_windows() {
        let (mut d, mut sys) = setup();
        sys.files.user_rename(&sp(&sys.files, Std::Desktop, "nota.txt"), &sp(&sys.files, Std::Desktop, "otra.txt")).unwrap();
        // Con una ventana enfocada, Ctrl+Z no es del escritorio.
        d.launch(AppKind::Calc, &mut sys);
        key(&mut d, &mut sys, Key::Char('z'), None, true);
        assert!(sys.files.exists(&sp(&sys.files, Std::Desktop, "otra.txt")));
        d.close(d.wins[0].id);
        key(&mut d, &mut sys, Key::Char('z'), None, true);
        assert!(sys.files.exists(&sp(&sys.files, Std::Desktop, "nota.txt")));
        // Supr sobre el icono seleccionado: a la Papelera.
        click_icon(&mut d, &mut sys, 7);
        key(&mut d, &mut sys, Key::Delete, None, false);
        assert!(sys.files.exists(&sp(&sys.files, Std::Desktop, "nota.txt")), "aún no: pregunta");
        key(&mut d, &mut sys, Key::Escape, None, false); // No
        assert!(sys.files.exists(&sp(&sys.files, Std::Desktop, "nota.txt")), "con No se queda");
        key(&mut d, &mut sys, Key::Delete, None, false);
        key(&mut d, &mut sys, Key::Enter, None, false); // Sí
        assert!(!sys.files.exists(&sp(&sys.files, Std::Desktop, "nota.txt")));
        assert_eq!(sys.files.trash_list().unwrap().len(), 1);
        key(&mut d, &mut sys, Key::Char('z'), None, true);
        assert!(sys.files.exists(&sp(&sys.files, Std::Desktop, "nota.txt")));
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
        sys.files.write(&sp(&sys.files, Std::Desktop, "nuevo.txt"), b"n", now).unwrap();
        d.event(Event::Tick(10), &mut sys);
        check(&mut d, &mut sys, "archivo creado");
        sys.files.delete(&sp(&sys.files, Std::Desktop, "nuevo.txt")).unwrap();
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
        sys.files.write(&sp(&sys.files, Std::Desktop, "nota.txt"), b"hola", now).unwrap();
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
        sys.files.write(&sys.files.explorer_prefs(), b"extensiones=no\n", now).unwrap();
        d.bg_action(M_REFRESH, &mut sys);
        assert_eq!(labels(&d), ["nota"]);
        assert_eq!(d.icons[6].name, "nota.txt");
        // El diálogo propone el nombre sin extensión y la extensión original se conserva.
        d.start_rename(&sp(&sys.files, Std::Desktop, "nota.txt"), &mut sys);
        d.system_result(DialogResult::Text { tag: TAG_RENAME, value: Some(String::from("otra")) }, &mut sys);
        assert!(sys.files.exists(&sp(&sys.files, Std::Desktop, "otra.txt")) && !sys.files.exists(&sp(&sys.files, Std::Desktop, "nota.txt")));
        // Un nombre vacío sigue siendo inválido (no queda ".txt").
        let n = d.dialog_count();
        d.start_rename(&sp(&sys.files, Std::Desktop, "otra.txt"), &mut sys);
        d.system_result(DialogResult::Text { tag: TAG_RENAME, value: Some(String::from("  ")) }, &mut sys);
        assert!(sys.files.exists(&sp(&sys.files, Std::Desktop, "otra.txt")));
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
        sys.files.mkdir(&sp(&sys.files, Std::Desktop, "Sub"), now).unwrap();
        d.launch(AppKind::Calc, &mut sys);
        let id = d.active_id().unwrap();
        d.with_app(id, &mut sys, |_, cx| cx.open_terminal = Some(files::join(&cx.sys.files().std_dir(Std::Desktop), "Sub")));
        d.with_app(id, &mut sys, |_, cx| cx.open_terminal = Some(files::join(&cx.sys.files().std_dir(Std::Desktop), "Sub")));
        let terms = d.wins.iter().filter(|w| w.kind == AppKind::Terminal).count();
        assert_eq!(terms, 2);
    }

    #[test]
    fn cinta_copiar_tras_seleccionar_todo() {
        let (mut d, mut sys) = setup();
        let now = sys.now();
        sys.files.write(&sp(&sys.files, Std::Documents, "a.txt"), b"hola", now).unwrap();
        sys.files.write(&sp(&sys.files, Std::Documents, "b.txt"), b"hola", now).unwrap();
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

#[cfg(test)]
mod login_tests {
    use super::*;
    use crate::block::RamDisk;
    use crate::dialogs::Buttons;
    use crate::files::Files;
    use crate::platform::TestPlatform;
    use alloc::rc::Rc;
    use core::cell::Cell;

    /// App de pruebas: cuenta las teclas y los clics; con `dirty` pregunta antes de cerrar.
    struct Stub {
        dirty: bool,
        keys: Rc<Cell<u32>>,
        downs: Rc<Cell<u32>>,
    }

    impl App for Stub {
        fn kind(&self) -> AppKind {
            AppKind::Notepad
        }
        fn title(&self) -> String {
            String::from("Stub")
        }
        fn min_size(&self) -> (i32, i32) {
            (100, 100)
        }
        fn draw(&mut self, s: &mut Surface, _t: &Theme, _p: &mut dyn Platform, _f: bool) {
            let b = s.bounds();
            s.fill_rect(b, 0xEEEEEE);
        }
        fn key(&mut self, ev: &KeyEvent, _cx: &mut Ctx) {
            if matches!(ev.key, Key::Char(_)) {
                self.keys.set(self.keys.get() + 1);
            }
        }
        fn mouse(&mut self, ev: MouseEv, _cx: &mut Ctx) {
            if matches!(ev, MouseEv::Down { .. }) {
                self.downs.set(self.downs.get() + 1);
            }
        }
        fn tick(&mut self, _cx: &mut Ctx) {}
        fn dialog_result(&mut self, r: DialogResult, cx: &mut Ctx) {
            if let DialogResult::Answer { tag: 9, answer: Answer::Yes } = r {
                cx.close = true;
            }
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

    fn stub(d: &mut Desktop, sys: &mut TestPlatform, dirty: bool) -> (Rc<Cell<u32>>, Rc<Cell<u32>>) {
        let (k, c) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        d.launch_app(Box::new(Stub { dirty, keys: k.clone(), downs: c.clone() }), sys);
        (k, c)
    }

    /// Disco formateado y sin cuentas.
    fn disk() -> TestPlatform {
        let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
        let now = sys.now();
        sys.files = Files::probe(Some(Box::new(RamDisk::new(2048))), now);
        sys.files.format_pending(now).unwrap();
        sys.files.logout();
        sys
    }

    /// Disco con estas cuentas (la primera es la administradora).
    fn with_accounts(list: &[(&str, Option<&str>)]) -> TestPlatform {
        let mut sys = disk();
        let now = sys.now();
        for (i, (n, p)) in list.iter().enumerate() {
            sys.files.create_account(n, *p, false, [i as u8 + 1; 16], now).unwrap();
            if i == 0 {
                sys.files.login(n, p.unwrap_or("")).unwrap();
            }
        }
        sys.files.logout();
        sys
    }

    fn boot(sys: &mut TestPlatform) -> Desktop {
        let mut d = Desktop::new(1280, 800, sys);
        d.event(Event::Tick(10), sys);
        d
    }

    fn press(d: &mut Desktop, sys: &mut TestPlatform, key: Key, ch: Option<char>) {
        d.event(Event::Key(KeyEvent { key, ch, down: true, shift: false, ctrl: false, alt: false }), sys);
    }

    fn typ(d: &mut Desktop, sys: &mut TestPlatform, text: &str) {
        for c in text.chars() {
            press(d, sys, Key::Char(c.to_ascii_lowercase()), Some(c));
        }
    }

    fn back(d: &mut Desktop, sys: &mut TestPlatform, n: usize) {
        for _ in 0..n {
            press(d, sys, Key::Backspace, None);
        }
    }

    fn click(d: &mut Desktop, sys: &mut TestPlatform, p: (i32, i32)) {
        d.mouse_to(p.0, p.1, sys);
        d.left_button(true, sys);
        d.left_button(false, sys);
    }

    /// Lo dibujado por zonas es igual que un redibujado completo.
    fn same_as_full(d: &mut Desktop, sys: &mut TestPlatform) {
        d.render(sys);
        let a = d.back().px.clone();
        d.invalidate();
        d.render(sys);
        assert!(a == d.back().px, "el redibujado incremental difiere del completo");
    }

    #[test]
    fn la_unidad_en_memoria_va_directa_al_escritorio() {
        let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
        let mut d = Desktop::new(1280, 800, &mut sys);
        d.event(Event::Tick(10), &mut sys);
        assert_eq!(d.login_mode(), Mode::Off);
        assert!(sys.files.user().is_some());
    }

    #[test]
    fn asistente_crea_cuentas_y_pasa_al_inicio_de_sesion() {
        let mut sys = disk();
        let mut d = boot(&mut sys);
        assert_eq!(d.login_mode(), Mode::Setup);
        same_as_full(&mut d, &mut sys);
        // Nombre vacío o no válido: error y no se crea nada.
        press(&mut d, &mut sys, Key::Enter, None);
        typ(&mut d, &mut sys, "a|b");
        press(&mut d, &mut sys, Key::Enter, None);
        assert!(sys.files.accounts().is_empty());
        same_as_full(&mut d, &mut sys);
        back(&mut d, &mut sys, 3);
        typ(&mut d, &mut sys, "Ana");
        press(&mut d, &mut sys, Key::Enter, None);
        let list = sys.files.accounts();
        assert!(list.len() == 1 && list[0].admin && !list[0].has_password());
        // Segunda tarjeta: nombre repetido, clave corta, claves distintas y por fin una buena.
        typ(&mut d, &mut sys, "Ana");
        press(&mut d, &mut sys, Key::Enter, None);
        assert_eq!(sys.files.accounts().len(), 1);
        back(&mut d, &mut sys, 3);
        typ(&mut d, &mut sys, "Luis");
        press(&mut d, &mut sys, Key::Tab, None);
        typ(&mut d, &mut sys, "12345");
        press(&mut d, &mut sys, Key::Tab, None);
        typ(&mut d, &mut sys, "12345");
        press(&mut d, &mut sys, Key::Enter, None);
        assert_eq!(sys.files.accounts().len(), 1, "clave de menos de 6 caracteres");
        back(&mut d, &mut sys, 5);
        typ(&mut d, &mut sys, "secreto1");
        press(&mut d, &mut sys, Key::Tab, None);
        press(&mut d, &mut sys, Key::Enter, None);
        assert_eq!(sys.files.accounts().len(), 1, "las claves no coinciden");
        back(&mut d, &mut sys, 5);
        typ(&mut d, &mut sys, "secreto1");
        press(&mut d, &mut sys, Key::Enter, None);
        let list = sys.files.accounts();
        assert!(list.len() == 2 && list[1].has_password() && !list[1].admin);
        same_as_full(&mut d, &mut sys);
        // Terminar (5.º elemento tras Nombre).
        for _ in 0..5 {
            press(&mut d, &mut sys, Key::Tab, None);
        }
        press(&mut d, &mut sys, Key::Enter, None);
        assert_eq!(d.login_mode(), Mode::Pick);
        assert!(sys.files.user().is_none());
    }

    #[test]
    fn una_cuenta_sin_clave_entra_directa_y_con_clave_pide_la_pantalla() {
        let mut sys = with_accounts(&[("Ana", None)]);
        let d = boot(&mut sys);
        assert_eq!((d.login_mode(), sys.files.user()), (Mode::Off, Some("Ana")));
        assert!(d.window_count() == 0 && !d.icons.is_empty());
        let mut sys = with_accounts(&[("Ana", Some("secreto1"))]);
        let d = boot(&mut sys);
        assert_eq!((d.login_mode(), sys.files.user()), (Mode::Pick, None));
    }

    #[test]
    fn lista_de_cuentas_flechas_y_entrada_sin_clave() {
        let mut sys = with_accounts(&[("Ana", Some("secreto1")), ("Luis", None)]);
        let mut d = boot(&mut sys);
        assert_eq!(d.login_mode(), Mode::Pick);
        same_as_full(&mut d, &mut sys);
        press(&mut d, &mut sys, Key::Right, None);
        same_as_full(&mut d, &mut sys);
        press(&mut d, &mut sys, Key::Enter, None);
        assert_eq!((d.login_mode(), sys.files.user()), (Mode::Off, Some("Luis")));
        // Nada de la pantalla queda activo: el escritorio responde como siempre.
        press(&mut d, &mut sys, Key::Super, None);
        assert!(d.start_is_open());
    }

    #[test]
    fn clave_incorrecta_espera_tras_tres_fallos_y_acierto() {
        let mut sys = with_accounts(&[("Ana", Some("secreto1"))]);
        let mut d = boot(&mut sys);
        for _ in 0..2 {
            typ(&mut d, &mut sys, "mala12");
            press(&mut d, &mut sys, Key::Enter, None);
            assert_eq!(sys.files.user(), None);
        }
        same_as_full(&mut d, &mut sys);
        typ(&mut d, &mut sys, "mala12");
        press(&mut d, &mut sys, Key::Enter, None);
        // Espera de 5 s desde el último tick (10 ms): la clave buena no se acepta todavía.
        typ(&mut d, &mut sys, "secreto1");
        press(&mut d, &mut sys, Key::Enter, None);
        assert_eq!(sys.files.user(), None);
        d.event(Event::Tick(2000), &mut sys);
        d.render(&mut sys);
        same_as_full(&mut d, &mut sys);
        typ(&mut d, &mut sys, "secreto1");
        press(&mut d, &mut sys, Key::Enter, None);
        assert_eq!(sys.files.user(), None, "sigue la espera");
        d.event(Event::Tick(5100), &mut sys);
        typ(&mut d, &mut sys, "secreto1");
        press(&mut d, &mut sys, Key::Enter, None);
        assert_eq!((d.login_mode(), sys.files.user()), (Mode::Off, Some("Ana")));
    }

    #[test]
    fn cada_usuario_tiene_su_configuracion_y_sus_iconos() {
        let mut sys = with_accounts(&[("Ana", None), ("Luis", None)]);
        let mut d = boot(&mut sys);
        let now = sys.now();
        press(&mut d, &mut sys, Key::Enter, None);
        let home_a = sys.files.home();
        sys.files.write(&sys.files.config_file(), b"fondo=2\nenfasis=D13B3B\n", now).unwrap();
        sys.files.write(&sp(&sys.files, Std::Desktop, "de_ana.txt"), b"a", now).unwrap();
        d.begin_logout(&mut sys);
        assert_eq!(d.login_mode(), Mode::Pick);
        press(&mut d, &mut sys, Key::Right, None);
        press(&mut d, &mut sys, Key::Enter, None);
        assert_eq!(sys.files.user(), Some("Luis"));
        if sys.files.home() == home_a {
            return; // estructura antigua: todos comparten carpeta (la prueba vale con la nueva)
        }
        assert_eq!(d.theme.wallpaper, 0);
        assert!(!d.icons.iter().any(|i| i.name == "de_ana.txt"));
        d.begin_logout(&mut sys);
        press(&mut d, &mut sys, Key::Enter, None);
        assert_eq!(sys.files.user(), Some("Ana"));
        assert_eq!((d.theme.wallpaper, d.theme.accent), (2, 0xD13B3B));
        assert!(d.icons.iter().any(|i| i.name == "de_ana.txt"));
    }

    #[test]
    fn cerrar_sesion_respeta_la_cancelacion_de_una_ventana() {
        let mut sys = with_accounts(&[("Ana", None)]);
        let mut d = boot(&mut sys);
        stub(&mut d, &mut sys, false);
        stub(&mut d, &mut sys, true);
        d.begin_logout(&mut sys);
        assert_eq!(d.dialog_count(), 1, "pregunta por cambios sin guardar");
        press(&mut d, &mut sys, Key::Escape, None); // cancela
        assert_eq!((d.login_mode(), sys.files.user(), d.window_count()), (Mode::Off, Some("Ana"), 2));
        d.begin_logout(&mut sys);
        press(&mut d, &mut sys, Key::Enter, None); // Sí
        assert_eq!((d.login_mode(), sys.files.user(), d.window_count()), (Mode::Pick, None, 0));
        // Una cuenta sin clave tampoco vuelve a entrar sola tras cerrar sesión.
        press(&mut d, &mut sys, Key::Enter, None);
        assert_eq!(sys.files.user(), Some("Ana"));
    }

    #[test]
    fn el_menu_de_apagado_ofrece_bloquear_y_cerrar_sesion_solo_con_disco() {
        let mut sys = with_accounts(&[("Ana", None)]);
        let mut d = boot(&mut sys);
        assert_eq!(d.shut_count(), 4);
        click(&mut d, &mut sys, (24, 780)); // Inicio
        let p = d.mpower_rect();
        click(&mut d, &mut sys, (p.x + 24, p.y + 24));
        let r = d.shut_row(2);
        click(&mut d, &mut sys, (r.x + 20, r.y + 20));
        assert_eq!(d.login_mode(), Mode::Lock);
        let mut mem = TestPlatform { size: (1280, 800), ..Default::default() };
        assert_eq!(Desktop::new(1280, 800, &mut mem).shut_count(), 2);
    }

    #[test]
    fn bloquear_tapa_las_ventanas_y_desbloquea_con_la_clave() {
        let mut sys = with_accounts(&[("Ana", Some("secreto1"))]);
        let mut d = boot(&mut sys);
        typ(&mut d, &mut sys, "secreto1");
        press(&mut d, &mut sys, Key::Enter, None);
        let (keys, downs) = stub(&mut d, &mut sys, false);
        let c = d.window_rects()[0];
        d.lock(&mut sys);
        assert_eq!((d.login_mode(), d.window_count()), (Mode::Lock, 1));
        same_as_full(&mut d, &mut sys);
        typ(&mut d, &mut sys, "x");
        click(&mut d, &mut sys, (c.x + 40, c.y + 60));
        assert_eq!((keys.get(), downs.get()), (0, 0), "la app no recibe nada bloqueada");
        press(&mut d, &mut sys, Key::Enter, None); // clave vacía
        assert_eq!(d.login_mode(), Mode::Lock);
        typ(&mut d, &mut sys, "mala12");
        press(&mut d, &mut sys, Key::Enter, None);
        assert_eq!(d.login_mode(), Mode::Lock);
        typ(&mut d, &mut sys, "secreto1");
        press(&mut d, &mut sys, Key::Enter, None);
        assert_eq!((d.login_mode(), d.window_count()), (Mode::Off, 1));
        same_as_full(&mut d, &mut sys);
        typ(&mut d, &mut sys, "x");
        assert_eq!(keys.get(), 1);
    }

    #[test]
    fn bloqueo_sin_clave_basta_un_clic() {
        let mut sys = with_accounts(&[("Ana", None)]);
        let mut d = boot(&mut sys);
        d.lock(&mut sys);
        assert_eq!(d.login_mode(), Mode::Lock);
        click(&mut d, &mut sys, (100, 100));
        assert_eq!(d.login_mode(), Mode::Off);
    }

    #[test]
    fn apagar_desde_la_pantalla_de_inicio() {
        let mut sys = with_accounts(&[("Ana", Some("secreto1"))]);
        let mut d = boot(&mut sys);
        click(&mut d, &mut sys, (1240, 760));
        same_as_full(&mut d, &mut sys);
        click(&mut d, &mut sys, (1180, 672));
        assert!(sys.halted);
    }

    #[test]
    fn el_escritorio_alimenta_el_acumulador_con_raton_y_teclado() {
        let mut sys = with_accounts(&[("Ana", Some("secreto1"))]);
        let mut d = boot(&mut sys);
        let base = sys.pool.events();
        d.mouse_to(100, 100, &mut sys);
        d.mouse_to(100, 100, &mut sys); // misma posición: no cuenta
        d.mouse_to(120, 90, &mut sys);
        assert_eq!(sys.pool.events(), base + 2);
        let before = sys.pool.preview();
        typ(&mut d, &mut sys, "a");
        assert_eq!(sys.pool.events(), base + 3);
        assert_ne!(sys.pool.preview(), before);
    }

    // ---- Cifrado (v0.6.0): pantallas

    fn sesion() -> (Desktop, TestPlatform) {
        let mut sys = with_accounts(&[("Ana", None)]);
        let mut d = boot(&mut sys);
        d.render(&mut sys);
        assert_eq!(d.login_mode(), Mode::Off);
        (d, sys)
    }

    fn texto(d: &Desktop) -> String {
        d.conv.map(|c| c.text()).unwrap_or_default()
    }

    #[test]
    fn el_aviso_de_conversion_aparece_avanza_y_se_va() {
        let (mut d, mut sys) = sesion();
        let r = d.conv_rect();
        d.set_conv(ConvWhat::Disk, Some((37, 100, true)));
        assert_eq!(texto(&d), "Cifrando el disco… 37 %");
        // Solo se repinta su rectángulo.
        let zonas = d.render(&mut sys);
        assert!(!zonas.is_empty() && zonas.iter().all(|z| z.intersect(r) == *z), "{zonas:?}");
        same_as_full(&mut d, &mut sys);
        let (track_x, y) = (r.x + 14, r.bottom() - 12);
        assert_eq!(d.back().px[(y * 1280 + track_x + 10) as usize], d.theme.accent);
        assert_eq!(d.back().px[(y * 1280 + track_x + 200) as usize], 0x555555);
        // El mismo porcentaje no ensucia nada; uno nuevo, sí.
        d.set_conv(ConvWhat::Disk, Some((37, 100, true)));
        assert!(d.render(&mut sys).is_empty());
        d.set_conv(ConvWhat::Disk, Some((38, 100, true)));
        assert!(!d.render(&mut sys).is_empty());
        // Cada tipo de conversión con su texto.
        for (disk, enc, t) in [(true, false, "Descifrando el disco… 5 %"), (false, true, "Cifrando tu carpeta… 5 %"), (false, false, "Descifrando tu carpeta… 5 %")] {
            d.preview_conversion(disk, enc, 5);
            assert_eq!(texto(&d), t);
        }
        // Al terminar: aviso de 4 s y luego nada.
        d.set_conv(ConvWhat::Home, Some((100, 100, true)));
        d.set_conv(ConvWhat::Home, None);
        assert_eq!((d.conv, d.conv_note.map(|n| n.0)), (None, Some("Cifrado completado")));
        same_as_full(&mut d, &mut sys);
        d.event(Event::Tick(d.now_ms + 1000), &mut sys);
        assert!(d.conv_note.is_some());
        d.event(Event::Tick(d.now_ms + CONV_NOTE_MS), &mut sys);
        assert!(d.conv_note.is_none());
        same_as_full(&mut d, &mut sys);
        d.set_conv(ConvWhat::Disk, Some((9, 10, false)));
        d.set_conv(ConvWhat::Disk, None);
        assert_eq!(d.conv_note.map(|n| n.0), Some("Descifrado completado"));
    }

    #[test]
    fn sin_conversion_el_tick_no_pinta_nada() {
        let (mut d, mut sys) = sesion();
        d.event(Event::Tick(d.now_ms + 10), &mut sys);
        assert!(d.conv.is_none() && d.conv_note.is_none());
    }

    #[test]
    fn apagar_con_un_cifrado_en_curso_pide_confirmacion() {
        let (mut d, mut sys) = sesion();
        d.power(0, &mut sys);
        assert!(sys.halted);
        sys.halted = false;
        d.set_conv(ConvWhat::Disk, Some((1, 100, true)));
        d.power(0, &mut sys);
        assert!(!sys.halted);
        assert_eq!(d.window_count(), 1);
        // Escape = No.
        press(&mut d, &mut sys, Key::Escape, None);
        assert!(!sys.halted && d.window_count() == 0);
        d.power(0, &mut sys);
        press(&mut d, &mut sys, Key::Enter, None);
        assert!(sys.halted);
        // Reiniciar y cerrar sesión, igual.
        d.power(1, &mut sys);
        assert!(!sys.rebooted);
        press(&mut d, &mut sys, Key::Enter, None);
        assert!(sys.rebooted);
        d.power(3, &mut sys);
        assert_eq!(d.login_mode(), Mode::Off);
        press(&mut d, &mut sys, Key::Enter, None);
        assert_eq!(d.login_mode(), Mode::Pick);
    }

    #[test]
    fn un_error_de_conversion_se_cuenta_una_sola_vez() {
        let (mut d, mut sys) = sesion();
        d.set_conv(ConvWhat::Disk, Some((1, 100, true)));
        d.conv_error(KeyError::Fs(FsError::Invalid), &mut sys);
        assert!(d.conv.is_none() && d.conv_failed);
        assert_eq!(d.window_count(), 1);
        for i in 1..5 {
            d.event(Event::Tick(d.now_ms + 10 * i), &mut sys);
        }
        assert_eq!(d.window_count(), 1);
    }

    #[test]
    fn la_pantalla_de_disco_cifrado_sigue_el_arranque_normal() {
        // Con cuentas: tras abrir (o seguir sin disco) viene el inicio de sesión.
        let mut sys = with_accounts(&[("Ana", Some("secreto1"))]);
        let mut d = boot(&mut sys);
        d.preview_disk_screen();
        assert_eq!(d.login_mode(), Mode::Disk);
        same_as_full(&mut d, &mut sys);
        // Escribir una clave, enviarla y esperar el tick: aquí el disco no está cifrado y lo dice.
        typ(&mut d, &mut sys, "clavedisco");
        press(&mut d, &mut sys, Key::Enter, None);
        assert_eq!(d.login_mode(), Mode::Disk);
        d.event(Event::Tick(d.now_ms + 200), &mut sys);
        same_as_full(&mut d, &mut sys);
        // Seguir sin disco: Tab hasta el enlace, Enter, Mayús+Tab a "Seguir sin disco" y Enter.
        for _ in 0..3 {
            press(&mut d, &mut sys, Key::Tab, None);
        }
        press(&mut d, &mut sys, Key::Enter, None);
        same_as_full(&mut d, &mut sys);
        assert_eq!(d.login_mode(), Mode::Disk);
        d.event(Event::Key(KeyEvent { key: Key::Tab, ch: None, down: true, shift: true, ctrl: false, alt: false }), &mut sys);
        press(&mut d, &mut sys, Key::Enter, None);
        assert_eq!(d.login_mode(), Mode::Pick);
        // Sin cuentas: el asistente.
        let mut sys = disk();
        let mut d = boot(&mut sys);
        assert_eq!(d.login_mode(), Mode::Setup);
        d.preview_disk_screen();
        d.login_out(Out::DiskDone, &mut sys);
        assert_eq!(d.login_mode(), Mode::Setup);
    }

    #[test]
    fn apagar_desde_la_pantalla_de_disco_cifrado() {
        let mut sys = with_accounts(&[("Ana", Some("secreto1"))]);
        let mut d = boot(&mut sys);
        d.preview_disk_screen();
        click(&mut d, &mut sys, (1240, 760));
        same_as_full(&mut d, &mut sys);
        click(&mut d, &mut sys, (1180, 672));
        assert!(sys.halted);
    }

    #[test]
    fn el_dialogo_de_aleatoriedad_cuenta_cada_movimiento_una_sola_vez() {
        let (mut d, mut sys) = sesion();
        d.preview_entropy(&mut sys);
        d.render(&mut sys);
        let (c, (w, h)) = {
            let win = d.wins.last().unwrap();
            (win.content_rect(), win.dlg.as_ref().unwrap().size())
        };
        let base = sys.pool.events();
        for i in 0..20 {
            d.mouse_to(c.x + 10 + i * 3, c.y + 60, &mut sys);
        }
        // El escritorio alimenta el acumulador y el diálogo solo cuenta: ni dobles ni pérdidas.
        assert_eq!(sys.pool.events(), base + 20);
        // "Continuar" no responde hasta completar los movimientos.
        let boton = |d: &mut Desktop, sys: &mut TestPlatform| {
            d.mouse_to(c.x + w - 150, c.y + h - 30, sys);
            d.left_button(true, sys);
            d.left_button(false, sys);
        };
        boton(&mut d, &mut sys);
        assert_eq!(d.window_count(), 1);
        for i in 0..crate::keys::POOL_EVENTS_NEEDED as i32 {
            d.mouse_to(c.x + 5 + i % 450, c.y + 60 + i / 450, &mut sys);
        }
        d.render(&mut sys);
        boton(&mut d, &mut sys);
        assert_eq!(d.window_count(), 0);
    }

    /// Disco en memoria que sigue vivo tras entregárselo a `Files` (para "reiniciar" y montarlo otra vez).
    #[derive(Clone)]
    struct Shared(Rc<core::cell::RefCell<Vec<u8>>>);
    impl crate::block::BlockDev for Shared {
        fn blocks(&self) -> u64 {
            (self.0.borrow().len() / crate::block::BLOCK) as u64
        }
        fn read(&mut self, n: u64, buf: &mut [u8]) -> Result<(), crate::block::DiskError> {
            if n >= self.blocks() || buf.len() != crate::block::BLOCK {
                return Err(crate::block::DiskError::Range);
            }
            buf.copy_from_slice(&self.0.borrow()[n as usize * crate::block::BLOCK..][..crate::block::BLOCK]);
            Ok(())
        }
        fn write(&mut self, n: u64, buf: &[u8]) -> Result<(), crate::block::DiskError> {
            if n >= self.blocks() || buf.len() != crate::block::BLOCK {
                return Err(crate::block::DiskError::Range);
            }
            self.0.borrow_mut()[n as usize * crate::block::BLOCK..][..crate::block::BLOCK].copy_from_slice(buf);
            Ok(())
        }
    }

    /// Integración con el cifrado de disco real de `files.rs` (el integrador la activa cuando exista):
    /// un disco cifrado arranca en la pantalla "Disco cifrado", rechaza una clave mala, se abre con la
    /// buena o con el código de recuperación, y sigue con el inicio de sesión.
    #[test]
    fn disco_cifrado_real_pide_su_clave_antes_del_inicio_de_sesion() {
        let ram = Shared(Rc::new(core::cell::RefCell::new(vec![0; 4096 * crate::block::BLOCK])));
        let now = DateTime { year: 2026, month: 11, day: 15, hour: 21, minute: 37, second: 5 };
        let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
        sys.files = Files::probe(Some(Box::new(ram.clone())), now);
        sys.files.format_pending(now).unwrap();
        sys.files.create_account("Ana", Some("secreto1"), true, [1; 16], now).unwrap();
        sys.files.login("Ana", "secreto1").unwrap();
        for i in 0..crate::keys::POOL_EVENTS_NEEDED {
            sys.pool.add_event(i as i32, 3, 1, i as u64 * 7919);
        }
        let mut pool = core::mem::take(&mut sys.pool);
        let code = sys.files.encrypt_disk("clavedisco", &mut pool, now).unwrap();
        while !sys.files.convert_step(64, now).unwrap() {}
        sys.files.logout();
        drop(sys);

        let remount = |ram: &Shared| {
            let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
            sys.files = Files::probe(Some(Box::new(ram.clone())), now);
            assert!(sys.files.disk_locked());
            sys
        };
        // Clave mala, y luego la buena.
        let mut sys = remount(&ram);
        let mut d = boot(&mut sys);
        assert_eq!(d.login_mode(), Mode::Disk);
        typ(&mut d, &mut sys, "mala");
        press(&mut d, &mut sys, Key::Enter, None);
        d.event(Event::Tick(d.now_ms + 200), &mut sys);
        assert_eq!(d.login_mode(), Mode::Disk);
        typ(&mut d, &mut sys, "clavedisco");
        press(&mut d, &mut sys, Key::Enter, None);
        d.event(Event::Tick(d.now_ms + 200), &mut sys);
        assert_eq!(d.login_mode(), Mode::Pick);
        // Con el código de recuperación (tecleado en minúsculas y con guiones).
        let mut sys = remount(&ram);
        let mut d = boot(&mut sys);
        for _ in 0..2 {
            press(&mut d, &mut sys, Key::Tab, None);
        }
        press(&mut d, &mut sys, Key::Enter, None);
        typ(&mut d, &mut sys, &code.to_lowercase());
        press(&mut d, &mut sys, Key::Enter, None);
        d.event(Event::Tick(d.now_ms + 200), &mut sys);
        assert_eq!(d.login_mode(), Mode::Pick);
    }

    // ---------------------------------------------------------------- disco nuevo: asistente de cifrado (v0.7.1)

    fn ram_nueva() -> Shared {
        Shared(Rc::new(core::cell::RefCell::new(vec![0; 4096 * crate::block::BLOCK])))
    }

    fn disco_nuevo(ram: &Shared) -> TestPlatform {
        let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
        let now = sys.now();
        sys.files = Files::probe(Some(Box::new(ram.clone())), now);
        assert!(sys.files.pending_format());
        sys
    }

    /// Arranca con un disco en blanco, contesta Sí a "Disco nuevo" y deja el asistente en la elección.
    fn arranca_nuevo(sys: &mut TestPlatform) -> Desktop {
        let mut d = Desktop::new(1280, 800, sys);
        d.event(Event::Tick(10), sys);
        assert_eq!(d.dialog_count(), 1);
        press(&mut d, sys, Key::Enter, None);
        assert_eq!((d.login_mode(), d.login.step_name()), (Mode::Setup, "cifrado"));
        assert!(sys.files.pending_format(), "hasta Terminar no se toca el disco");
        d
    }

    /// Elige la opción `i` (0 Ninguno, 1 Mi carpeta, 2 Disco entero, 3 Ambos) con las flechas y sigue.
    fn elige_y_sigue(d: &mut Desktop, sys: &mut TestPlatform, i: usize) {
        for _ in 0..(i + 3) % 4 {
            press(d, sys, Key::Down, None);
        }
        press(d, sys, Key::Enter, None);
    }

    fn clave_disco(d: &mut Desktop, sys: &mut TestPlatform, clave: &str) {
        assert_eq!(d.login.step_name(), "clave del disco");
        typ(d, sys, clave);
        press(d, sys, Key::Tab, None);
        typ(d, sys, clave);
        press(d, sys, Key::Enter, None);
    }

    fn cuenta(d: &mut Desktop, sys: &mut TestPlatform, nombre: &str, clave: &str) {
        typ(d, sys, nombre);
        if !clave.is_empty() {
            press(d, sys, Key::Tab, None);
            typ(d, sys, clave);
            press(d, sys, Key::Tab, None);
            typ(d, sys, clave);
        }
        press(d, sys, Key::Enter, None);
    }

    /// Terminar (el sexto elemento de la tarjeta "¿Añadir otra cuenta?").
    fn terminar(d: &mut Desktop, sys: &mut TestPlatform) {
        for _ in 0..5 {
            press(d, sys, Key::Tab, None);
        }
        press(d, sys, Key::Enter, None);
    }

    /// Mueve el ratón sobre la ventana de aleatoriedad hasta llenarla y pulsa "Continuar".
    fn mueve_el_raton(d: &mut Desktop, sys: &mut TestPlatform) {
        d.render(sys);
        let (c, (w, h)) = {
            let win = d.wins.last().unwrap();
            (win.content_rect(), win.dlg.as_ref().unwrap().size())
        };
        for i in 0..crate::keys::POOL_EVENTS_NEEDED as i32 {
            d.mouse_to(c.x + 5 + i % 450, c.y + 60 + i / 450, sys);
        }
        d.render(sys);
        d.mouse_to(c.x + w - 150, c.y + h - 30, sys);
        d.left_button(true, sys);
        d.left_button(false, sys);
    }

    fn tick_mas(d: &mut Desktop, sys: &mut TestPlatform) {
        d.event(Event::Tick(d.now_ms + 200), sys);
    }

    /// Recorre una opción completa (Ana con clave y Luis sin ella cuando la carpeta se cifra) y devuelve los
    /// códigos mostrados, en orden. Comprueba el redibujado incremental en cada pantalla.
    fn recorrido(opcion: usize, ram: &Shared) -> (TestPlatform, Vec<(String, String)>) {
        let mut sys = disco_nuevo(ram);
        let mut d = arranca_nuevo(&mut sys);
        same_as_full(&mut d, &mut sys);
        elige_y_sigue(&mut d, &mut sys, opcion);
        if opcion >= 2 {
            same_as_full(&mut d, &mut sys);
            clave_disco(&mut d, &mut sys, "claveDisco1");
        }
        assert_eq!(d.login.step_name(), "cuentas");
        same_as_full(&mut d, &mut sys);
        cuenta(&mut d, &mut sys, "Ana", "secreto1");
        // Luis sin clave: con la carpeta cifrada avisa primero.
        typ(&mut d, &mut sys, "Luis");
        press(&mut d, &mut sys, Key::Enter, None);
        if opcion == 1 || opcion == 3 {
            assert!(d.login.pend_len() == 1, "avisa de la carpeta sin cifrar");
            press(&mut d, &mut sys, Key::Enter, None);
        }
        assert_eq!(d.login.pend_len(), 2);
        same_as_full(&mut d, &mut sys);
        terminar(&mut d, &mut sys);
        if opcion == 0 {
            assert_eq!(d.dialog_count(), 0, "sin cifrado no hay ventana del ratón");
        } else {
            assert_eq!((d.dialog_count(), d.login_mode()), (1, Mode::Boot));
            assert!(sys.files.pending_format());
            same_as_full(&mut d, &mut sys);
            mueve_el_raton(&mut d, &mut sys);
            assert_eq!(d.dialog_count(), 0);
        }
        // "Preparando…" se pinta antes del trabajo: el disco aún no está formateado.
        assert_eq!((d.login_mode(), d.login.step_name()), (Mode::Setup, "preparando"));
        assert!(sys.files.pending_format());
        same_as_full(&mut d, &mut sys);
        tick_mas(&mut d, &mut sys);
        let mut codes = Vec::new();
        while d.login.step_name() == "codigos" {
            same_as_full(&mut d, &mut sys);
            codes.push(d.login.shown_code().unwrap());
            press(&mut d, &mut sys, Key::Enter, None);
        }
        // Tras el último código sigue el inicio de sesión (hay dos cuentas).
        assert_eq!(d.login_mode(), Mode::Pick);
        same_as_full(&mut d, &mut sys);
        (sys, codes)
    }

    #[test]
    fn disco_nuevo_ninguno_no_cifra_nada() {
        let ram = ram_nueva();
        let (mut sys, codes) = recorrido(0, &ram);
        assert!(codes.is_empty());
        assert!(!sys.files.disk_encrypted() && !sys.files.account_encrypted("Ana") && !sys.files.account_encrypted("Luis"));
        assert_eq!(sys.files.accounts().len(), 2);
        sys.files.login("Ana", "secreto1").unwrap();
    }

    #[test]
    fn disco_nuevo_mi_carpeta_cifra_la_carpeta_de_cada_cuenta_con_clave() {
        let ram = ram_nueva();
        let (mut sys, codes) = recorrido(1, &ram);
        assert_eq!(codes.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["de la cuenta Ana"]);
        assert!(!sys.files.disk_encrypted() && sys.files.account_encrypted("Ana") && !sys.files.account_encrypted("Luis"));
        // El código abre la cuenta: recuperarla con una clave nueva.
        let now = sys.now();
        let code = crate::keys::normalize_recovery(&codes[0].1).unwrap();
        let mut pool = core::mem::take(&mut sys.pool);
        for i in 0..crate::keys::POOL_EVENTS_NEEDED {
            pool.add_event(i as i32, 5, 1, i as u64 * 31);
        }
        sys.files.recover_account("Ana", &code, "nuevaClave1", &mut pool, now).unwrap();
        sys.files.login("Ana", "nuevaClave1").unwrap();
    }

    #[test]
    fn disco_nuevo_disco_entero_cifra_el_disco_y_su_codigo_lo_abre() {
        let ram = ram_nueva();
        let (mut sys, codes) = recorrido(2, &ram);
        assert_eq!(codes.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["del disco"]);
        assert!(sys.files.disk_encrypted() && !sys.files.account_encrypted("Ana"));
        drop(sys);
        // Reiniciar: el disco pide su clave y se abre con el código (y con la clave).
        let now = TestPlatform::default().now();
        let mut otra = TestPlatform { size: (1280, 800), ..Default::default() };
        otra.files = Files::probe(Some(Box::new(ram.clone())), now);
        assert!(otra.files.disk_locked());
        otra.files.unlock_disk(&codes[0].1, true, now).unwrap();
        assert_eq!(otra.files.accounts().len(), 2);
        let mut otra = TestPlatform { size: (1280, 800), ..Default::default() };
        otra.files = Files::probe(Some(Box::new(ram.clone())), now);
        otra.files.unlock_disk("claveDisco1", false, now).unwrap();
    }

    #[test]
    fn disco_nuevo_ambos_cifra_disco_y_carpetas_y_muestra_los_codigos_en_orden() {
        let ram = ram_nueva();
        let (mut sys, codes) = recorrido(3, &ram);
        assert_eq!(codes.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["del disco", "de la cuenta Ana"]);
        assert!(sys.files.disk_encrypted() && sys.files.account_encrypted("Ana") && !sys.files.account_encrypted("Luis"));
        drop(sys);
        let now = TestPlatform::default().now();
        let mut otra = TestPlatform { size: (1280, 800), ..Default::default() };
        otra.files = Files::probe(Some(Box::new(ram.clone())), now);
        otra.files.unlock_disk(&codes[0].1, true, now).unwrap();
        assert!(otra.files.account_encrypted("Ana"));
        otra.files.login("Ana", "secreto1").unwrap();
    }

    #[test]
    fn disco_nuevo_cancelar_la_ventana_del_raton_no_toca_el_disco() {
        let ram = ram_nueva();
        let mut sys = disco_nuevo(&ram);
        let mut d = arranca_nuevo(&mut sys);
        press(&mut d, &mut sys, Key::Enter, None); // Mi carpeta
        cuenta(&mut d, &mut sys, "Ana", "secreto1");
        terminar(&mut d, &mut sys);
        assert_eq!((d.dialog_count(), d.login_mode()), (1, Mode::Boot));
        press(&mut d, &mut sys, Key::Escape, None);
        assert_eq!((d.dialog_count(), d.login_mode(), d.login.step_name()), (0, Mode::Setup, "cuentas"));
        assert!(sys.files.pending_format() && !sys.files.persistent());
        same_as_full(&mut d, &mut sys);
        // Y se puede volver a intentar: ahora sí.
        terminar(&mut d, &mut sys);
        mueve_el_raton(&mut d, &mut sys);
        tick_mas(&mut d, &mut sys);
        assert_eq!(d.login.step_name(), "codigos");
        assert!(sys.files.persistent() && sys.files.account_encrypted("Ana"));
    }

    #[test]
    fn disco_nuevo_un_error_al_preparar_vuelve_a_la_eleccion() {
        // Un disco de 64 bloques sirve para el modo "Ninguno" pero no para el cifrado total (NoRoom).
        let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
        let now = sys.now();
        sys.files = Files::probe(Some(Box::new(RamDisk::new(64))), now);
        let mut d = arranca_nuevo(&mut sys);
        elige_y_sigue(&mut d, &mut sys, 2);
        clave_disco(&mut d, &mut sys, "claveDisco1");
        cuenta(&mut d, &mut sys, "Ana", "");
        terminar(&mut d, &mut sys);
        mueve_el_raton(&mut d, &mut sys);
        tick_mas(&mut d, &mut sys);
        assert_eq!((d.login_mode(), d.login.step_name()), (Mode::Setup, "cifrado"));
        assert_eq!(d.login.msg_text().as_deref(), Some(crate::keys::KeyError::NoRoom.text()));
        assert!(sys.files.pending_format() && sys.files.accounts().is_empty());
        same_as_full(&mut d, &mut sys);
        // Sin cifrado de disco, este disco diminuto tampoco cabe: otro error, y otra vez a la elección,
        // con el disco aún sin formatear y sin cuentas a medias.
        press(&mut d, &mut sys, Key::Up, None);
        press(&mut d, &mut sys, Key::Up, None);
        press(&mut d, &mut sys, Key::Enter, None);
        assert_eq!(d.login.step_name(), "cuentas");
        cuenta(&mut d, &mut sys, "Ana", "");
        terminar(&mut d, &mut sys);
        tick_mas(&mut d, &mut sys);
        assert_eq!((d.login_mode(), d.login.step_name()), (Mode::Setup, "cifrado"));
        assert_eq!(d.login.msg_text().as_deref(), Some("No queda espacio en el disco."));
        // (`format_pending` consume el disco pendiente aunque falle: un reinicio lo vuelve a ofrecer.)
        assert!(sys.files.accounts().is_empty() && d.login.pend_len() == 0);
        same_as_full(&mut d, &mut sys);
    }
}

#[cfg(test)]
mod dnd_tests {
    use super::*;
    use crate::apps::DragRequest;
    use crate::platform::TestPlatform;
    use alloc::rc::Rc;
    use core::cell::RefCell;

    type Log = Rc<RefCell<Vec<String>>>;

    /// App de pruebas que registra todo lo que recibe. Con `drag`, el primer `Move` tras un `Down` pide arrastrar.
    struct DProbe {
        log: Log,
        drag: Option<Vec<String>>,
        accept: bool,
        down: bool,
        /// Lo que devuelve `drag_source` (arrastre con el botón derecho).
        rsrc: Option<Vec<String>>,
    }

    impl App for DProbe {
        fn kind(&self) -> AppKind {
            AppKind::Notepad
        }
        fn title(&self) -> String {
            String::from("DProbe")
        }
        fn min_size(&self) -> (i32, i32) {
            (100, 100)
        }
        fn draw(&mut self, s: &mut Surface, _theme: &Theme, _sys: &mut dyn Platform, _focused: bool) {
            let b = s.bounds();
            s.fill_rect(b, 0xEEEEEE);
        }
        fn key(&mut self, _ev: &KeyEvent, _cx: &mut Ctx) {}
        fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
            let mut l = self.log.borrow_mut();
            match ev {
                MouseEv::Down { x, y } => {
                    self.down = true;
                    l.push(format!("down {x},{y}"));
                }
                MouseEv::Move { x, y } => {
                    l.push(format!("move {x},{y}"));
                    if self.down {
                        if let Some(paths) = self.drag.take() {
                            cx.drag = Some(DragRequest { paths });
                        }
                    }
                }
                MouseEv::Up { .. } => {
                    self.down = false;
                    l.push(String::from("up"));
                }
                MouseEv::Leave => l.push(String::from("leave")),
                MouseEv::RightDown { x, y } => l.push(format!("right {x},{y}")),
                _ => {}
            }
        }
        fn tick(&mut self, _cx: &mut Ctx) {}
        fn drag_source(&mut self, x: i32, y: i32, _cx: &mut Ctx) -> Option<Vec<String>> {
            self.log.borrow_mut().push(format!("src {x},{y}"));
            self.rsrc.clone()
        }
        fn drag_over(&mut self, pos: Option<(i32, i32)>, _paths: &[String], _cx: &mut Ctx) -> bool {
            self.log.borrow_mut().push(match pos {
                Some((x, y)) => format!("over {x},{y}"),
                None => String::from("over none"),
            });
            self.accept && pos.is_some()
        }
        fn drop_files(&mut self, x: i32, y: i32, paths: &[String], copy: bool, _cx: &mut Ctx) {
            self.log.borrow_mut().push(format!("drop {x},{y} {copy} {}", paths.join("|")));
        }
    }

    /// Escritorio: iconos 0..5 apps, 5 Papelera, 6 Carpeta, 7 a.txt, 8 b.txt; Documentos con f.txt y g.txt.
    fn setup() -> (Desktop, TestPlatform) {
        let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
        let now = sys.now();
        sys.files.mkdir(&sp(&sys.files, Std::Desktop, "Carpeta"), now).unwrap();
        sys.files.write(&sp(&sys.files, Std::Desktop, "a.txt"), b"a", now).unwrap();
        sys.files.write(&sp(&sys.files, Std::Desktop, "b.txt"), b"b", now).unwrap();
        sys.files.write(&sp(&sys.files, Std::Documents, "f.txt"), b"f", now).unwrap();
        sys.files.write(&sp(&sys.files, Std::Documents, "g.txt"), b"g", now).unwrap();
        let mut d = Desktop::new(1280, 800, &mut sys);
        d.booted = true;
        (d, sys)
    }

    fn docs(sys: &TestPlatform, name: &str) -> String {
        sp(&sys.files, Std::Documents, name)
    }

    fn desk(sys: &TestPlatform, name: &str) -> String {
        sp(&sys.files, Std::Desktop, name)
    }

    /// Abre una app de pruebas en `rect` (marco exterior).
    fn add(d: &mut Desktop, sys: &mut TestPlatform, rect: Rect, drag: Option<Vec<String>>, accept: bool) -> (u32, Log) {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        d.launch_app(Box::new(DProbe { log: log.clone(), rsrc: drag.clone(), drag, accept, down: false }), sys);
        let id = d.wins.last().unwrap().id;
        let i = d.pos(id).unwrap();
        d.set_rect(i, rect);
        (id, log)
    }

    /// Cambia la app de la ventana `id` por una nueva que arrastrará `paths`.
    fn rearm(d: &mut Desktop, id: u32, paths: Vec<String>) {
        let i = d.pos(id).unwrap();
        d.wins[i].app = Box::new(DProbe { log: Rc::new(RefCell::new(Vec::new())), rsrc: Some(paths.clone()), drag: Some(paths), accept: true, down: false });
    }

    /// Punto (dx, dy) del contenido de la ventana, en pantalla.
    fn at(d: &Desktop, id: u32, dx: i32, dy: i32) -> (i32, i32) {
        let c = d.wins[d.pos(id).unwrap()].content_rect();
        (c.x + dx, c.y + dy)
    }

    fn mv(d: &mut Desktop, sys: &mut TestPlatform, p: (i32, i32)) {
        d.mouse_to(p.0, p.1, sys);
    }

    /// Mueve el puntero al punto (dx, dy) del contenido de la ventana `id`.
    fn go(d: &mut Desktop, sys: &mut TestPlatform, id: u32, dx: i32, dy: i32) {
        let p = at(d, id, dx, dy);
        d.mouse_to(p.0, p.1, sys);
    }

    /// Mueve el puntero al centro del icono `i`.
    fn goi(d: &mut Desktop, sys: &mut TestPlatform, i: usize) {
        let p = center(d.icon_cell(i));
        d.mouse_to(p.0, p.1, sys);
    }

    fn center(r: Rect) -> (i32, i32) {
        (r.x + r.w / 2, r.y + r.h / 2)
    }

    fn mods(d: &mut Desktop, sys: &mut TestPlatform, ctrl: bool, shift: bool) {
        d.event(Event::Key(KeyEvent { key: Key::Other, ch: None, down: ctrl || shift, shift, ctrl, alt: false }), sys);
    }

    fn key(d: &mut Desktop, sys: &mut TestPlatform, key: Key, ch: Option<char>, ctrl: bool) {
        d.event(Event::Key(KeyEvent { key, ch, down: true, shift: false, ctrl, alt: false }), sys);
    }

    fn snapshot(l: &Log) -> Vec<String> {
        l.borrow().clone()
    }

    fn count(l: &Log, what: &str) -> usize {
        l.borrow().iter().filter(|s| s.as_str() == what).count()
    }

    fn has(l: &Log, prefix: &str) -> bool {
        l.borrow().iter().any(|s| s.starts_with(prefix))
    }

    /// Clic sobre el icono `i` (el reloj avanza para que no cuente como doble clic).
    fn click_icon(d: &mut Desktop, sys: &mut TestPlatform, i: usize) {
        d.event(Event::Tick(d.now_ms + 1000), sys);
        goi(d, sys, i);
        d.left_button(true, sys);
        d.left_button(false, sys);
    }

    fn render_equal(d: &mut Desktop, sys: &mut TestPlatform, what: &str) {
        d.render(sys);
        let inc = d.back().px.clone();
        d.invalidate();
        d.render(sys);
        assert!(inc == d.back().px, "incremental distinto del completo: {what}");
    }

    /// Dos ventanas: la 1 arrastra `paths`; la 2 está a la derecha. Devuelve ids y registros.
    fn two(d: &mut Desktop, sys: &mut TestPlatform, paths: Vec<String>, accept: bool) -> ((u32, Log), (u32, Log)) {
        let a = add(d, sys, Rect::new(300, 100, 300, 300), Some(paths), true);
        let b = add(d, sys, Rect::new(700, 100, 300, 300), None, accept);
        (a, b)
    }

    /// Pulsa en la ventana 1 y se mueve lo justo para que empiece el arrastre.
    fn start_from(d: &mut Desktop, sys: &mut TestPlatform, id: u32) {
        go(d, sys, id, 10, 10);
        d.left_button(true, sys);
        go(d, sys, id, 20, 20);
    }

    #[test]
    fn la_app_deja_de_recibir_eventos_al_pedir_arrastrar() {
        let (mut d, mut sys) = setup();
        let f = docs(&sys, "f.txt");
        let ((a, la), (b, lb)) = two(&mut d, &mut sys, vec![f.clone()], true);
        start_from(&mut d, &mut sys, a);
        let n = snapshot(&la).len();
        assert!(has(&la, "down 10,10") && has(&la, "move 20,20"));
        assert!(d.dnd.is_some() && d.drag == Drag::Files);
        // La propia ventana de origen también recibe drag_over con coordenadas de contenido.
        assert!(has(&la, "over 20,20"));
        // Más movimientos y el Up ya no llegan a la app de origen (solo drag_over).
        go(&mut d, &mut sys, a, 40, 40);
        go(&mut d, &mut sys, b, 15, 25);
        d.left_button(false, &mut sys);
        let l = snapshot(&la);
        assert!(l[n..].iter().all(|s| s.starts_with("over") || s == "leave"), "{l:?}");
        assert!(!l.iter().any(|s| s == "up"));
        assert_eq!(count(&la, "move 40,40"), 0);
        // Salir de la ventana de origen: drag_over(None) una vez; en la destino, coordenadas de contenido.
        assert_eq!(count(&la, "over none"), 1);
        assert!(has(&lb, "over 15,25"));
        assert!(has(&lb, "drop 15,25 false"), "{:?}", snapshot(&lb));
        assert_eq!(count(&lb, "over none"), 1);
        assert!(d.dnd.is_none() && d.drag == Drag::None);
    }

    #[test]
    fn drag_over_sale_una_vez_y_drop_solo_si_la_ultima_respuesta_fue_si() {
        let (mut d, mut sys) = setup();
        let f = docs(&sys, "f.txt");
        let ((a, _la), (b, lb)) = two(&mut d, &mut sys, vec![f], false);
        start_from(&mut d, &mut sys, a);
        go(&mut d, &mut sys, b, 10, 10);
        go(&mut d, &mut sys, b, 12, 10);
        assert!(!d.dnd.as_ref().unwrap().ok);
        d.left_button(false, &mut sys);
        assert!(!has(&lb, "drop"), "no admite: no hay drop");
        assert_eq!(count(&lb, "over none"), 1);
        // Salir de la ventana a otra zona del escritorio avisa una vez.
        let (mut d, mut sys) = setup();
        let f = docs(&sys, "f.txt");
        let ((a, _), (b, lb)) = two(&mut d, &mut sys, vec![f], true);
        start_from(&mut d, &mut sys, a);
        go(&mut d, &mut sys, b, 10, 10);
        mv(&mut d, &mut sys, (1100, 600));
        mv(&mut d, &mut sys, (1110, 610));
        assert_eq!(count(&lb, "over none"), 1);
        // Al soltar en el fondo no hay más avisos a la ventana.
        d.left_button(false, &mut sys);
        assert_eq!(count(&lb, "over none"), 1);
    }

    #[test]
    fn con_ctrl_el_drop_llega_como_copiar() {
        let (mut d, mut sys) = setup();
        let f = docs(&sys, "f.txt");
        let ((a, _), (b, lb)) = two(&mut d, &mut sys, vec![f.clone()], true);
        start_from(&mut d, &mut sys, a);
        go(&mut d, &mut sys, b, 10, 10);
        mods(&mut d, &mut sys, true, false);
        d.left_button(false, &mut sys);
        assert!(has(&lb, &format!("drop 10,10 true {f}")), "{:?}", snapshot(&lb));
    }

    #[test]
    fn esc_cancela_sin_mover_nada_y_la_app_no_recibe_mas() {
        let (mut d, mut sys) = setup();
        let f = docs(&sys, "f.txt");
        let ((a, la), (b, lb)) = two(&mut d, &mut sys, vec![f.clone()], true);
        start_from(&mut d, &mut sys, a);
        go(&mut d, &mut sys, b, 10, 10);
        key(&mut d, &mut sys, Key::Escape, None, false);
        assert!(d.dnd.is_none());
        assert_eq!(count(&lb, "over none"), 1);
        let n = snapshot(&la).len();
        go(&mut d, &mut sys, a, 50, 50);
        go(&mut d, &mut sys, b, 50, 50);
        d.left_button(false, &mut sys);
        assert!(!has(&lb, "drop") && snapshot(&la).len() == n);
        assert!(sys.files.exists(&f));
        assert_eq!(d.drag, Drag::None);
        // Una pulsación nueva funciona con normalidad.
        go(&mut d, &mut sys, b, 5, 5);
        d.left_button(true, &mut sys);
        d.left_button(false, &mut sys);
        assert!(has(&lb, "down 5,5") && count(&lb, "up") == 1);
    }

    #[test]
    fn cerrar_la_ventana_de_origen_o_bloquear_cancela() {
        let (mut d, mut sys) = setup();
        let f = docs(&sys, "f.txt");
        let ((a, _), (b, lb)) = two(&mut d, &mut sys, vec![f.clone()], true);
        start_from(&mut d, &mut sys, a);
        go(&mut d, &mut sys, b, 10, 10);
        d.close(a);
        go(&mut d, &mut sys, b, 11, 10);
        assert!(d.dnd.is_none());
        d.left_button(false, &mut sys);
        assert!(!has(&lb, "drop") && sys.files.exists(&f));
        // Bloqueo.
        let (mut d, mut sys) = setup();
        let ((a, _), (b, lb)) = two(&mut d, &mut sys, vec![f.clone()], true);
        start_from(&mut d, &mut sys, a);
        go(&mut d, &mut sys, b, 10, 10);
        d.lock(&mut sys);
        assert!(d.dnd.is_none());
        d.left_button(false, &mut sys);
        assert!(!has(&lb, "drop"));
        assert_eq!(count(&lb, "over none"), 1);
    }

    #[test]
    fn una_ventana_con_dialogo_modal_no_recibe_arrastres() {
        let (mut d, mut sys) = setup();
        let f = docs(&sys, "f.txt");
        let ((a, _), (b, lb)) = two(&mut d, &mut sys, vec![f], true);
        d.open_dialog(
            Some(b),
            DialogRequest::Message { tag: 1, title: String::from("T"), text: String::from("x"), buttons: crate::dialogs::Buttons::Ok },
            &mut sys,
        );
        start_from(&mut d, &mut sys, a);
        // Sobre la ventana bloqueada (fuera del diálogo) y sobre el propio diálogo.
        for p in [at(&d, b, 2, 2), at(&d, b, 150, 150), (710, 400)] {
            mv(&mut d, &mut sys, p);
            assert!(!has(&lb, "over"), "{:?}", snapshot(&lb));
        }
        d.left_button(false, &mut sys);
        assert!(!has(&lb, "drop"));
    }

    #[test]
    fn soltar_en_barra_inicio_y_titulos_no_hace_nada() {
        let (mut d, mut sys) = setup();
        let f = docs(&sys, "f.txt");
        let ((a, _), (b, lb)) = two(&mut d, &mut sys, vec![f.clone()], true);
        let title = d.wins[d.pos(b).unwrap()].title_rect();
        // Barra de tareas, borde de ventana y título de ventana.
        for p in [(400, 790), (title.x + 40, title.y + 5), (d.wins[d.pos(b).unwrap()].rect.x, 200)] {
            rearm(&mut d, a, vec![f.clone()]);
            start_from(&mut d, &mut sys, a);
            mv(&mut d, &mut sys, p);
            assert!(!d.dnd.as_ref().unwrap().ok, "{p:?}");
            d.left_button(false, &mut sys);
            assert!(sys.files.exists(&f) && !has(&lb, "drop") && d.dialog_count() == 0, "{p:?}");
        }
        // Menú Inicio abierto: soltar sobre él tampoco hace nada.
        rearm(&mut d, a, vec![f.clone()]);
        start_from(&mut d, &mut sys, a);
        d.set_start(true);
        let m = center(d.menu_rect());
        mv(&mut d, &mut sys, m);
        assert!(!d.dnd.as_ref().unwrap().ok);
        d.left_button(false, &mut sys);
        assert!(sys.files.exists(&f) && d.window_count() == 2);
    }

    #[test]
    fn el_escritorio_como_destino_fondo_carpeta_papelera_app_y_misma_carpeta() {
        let (mut d, mut sys) = setup();
        let (f, g) = (docs(&sys, "f.txt"), docs(&sys, "g.txt"));
        let a = add(&mut d, &mut sys, Rect::new(400, 100, 300, 300), Some(vec![f.clone()]), true).0;
        assert_eq!(d.hit(1000, 500), Hit::None);
        // Fondo: mueve a Escritorio.
        start_from(&mut d, &mut sys, a);
        mv(&mut d, &mut sys, (1000, 500));
        assert!(d.dnd.as_ref().unwrap().ok);
        d.left_button(false, &mut sys);
        assert!(!sys.files.exists(&f) && sys.files.exists(&desk(&sys, "f.txt")));
        // Deshacer con Ctrl+Z (sin ventanas a la vista el atajo es del escritorio).
        d.minimize(a);
        key(&mut d, &mut sys, Key::Char('z'), Some('z'), true);
        assert!(sys.files.exists(&f) && !sys.files.exists(&desk(&sys, "f.txt")));
        d.raise(a);
        // Con Ctrl, copia.
        rearm(&mut d, a, vec![f.clone()]);
        start_from(&mut d, &mut sys, a);
        mv(&mut d, &mut sys, (1000, 500));
        mods(&mut d, &mut sys, true, false);
        d.left_button(false, &mut sys);
        mods(&mut d, &mut sys, false, false);
        assert!(sys.files.exists(&f) && sys.files.exists(&desk(&sys, "f.txt")));
        // Carpeta del escritorio: resalta y mueve dentro.
        d.event(Event::Tick(50), &mut sys);
        let carpeta = d.icons.iter().position(|i| i.name == "Carpeta").unwrap();
        rearm(&mut d, a, vec![g.clone()]);
        start_from(&mut d, &mut sys, a);
        goi(&mut d, &mut sys, carpeta);
        assert_eq!(d.dnd_hl, Some(carpeta));
        render_equal(&mut d, &mut sys, "icono destino resaltado");
        d.left_button(false, &mut sys);
        assert_eq!(d.dnd_hl, None);
        assert!(!sys.files.exists(&g) && sys.files.exists(&format!("{}\\g.txt", desk(&sys, "Carpeta"))));
        // Papelera.
        let h = docs(&sys, "h.txt");
        let now = sys.now();
        sys.files.write(&h, b"h", now).unwrap();
        rearm(&mut d, a, vec![h.clone()]);
        start_from(&mut d, &mut sys, a);
        goi(&mut d, &mut sys, 5);
        assert_eq!(d.dnd_hl, Some(5));
        d.left_button(false, &mut sys);
        assert!(!sys.files.exists(&h) && sys.files.trash_list().unwrap().len() == 1);
        // Icono de app: nada, y la etiqueta se ve atenuada.
        let k = docs(&sys, "k.txt");
        sys.files.write(&k, b"k", now).unwrap();
        rearm(&mut d, a, vec![k.clone()]);
        start_from(&mut d, &mut sys, a);
        goi(&mut d, &mut sys, 1);
        assert!(!d.dnd.as_ref().unwrap().ok && d.dnd_hl.is_none());
        d.left_button(false, &mut sys);
        assert!(sys.files.exists(&k));
        // Misma carpeta: soltar un archivo del escritorio en el fondo no cambia nada ni da error.
        let ad = desk(&sys, "a.txt");
        rearm(&mut d, a, vec![ad.clone()]);
        start_from(&mut d, &mut sys, a);
        mv(&mut d, &mut sys, (1000, 500));
        d.left_button(false, &mut sys);
        assert!(sys.files.exists(&ad) && d.dialog_count() == 0);
        // Una carpeta no se puede soltar sobre sí misma.
        let car = desk(&sys, "Carpeta");
        rearm(&mut d, a, vec![car]);
        start_from(&mut d, &mut sys, a);
        goi(&mut d, &mut sys, carpeta);
        assert_eq!(d.dnd_hl, None);
        d.left_button(false, &mut sys);
    }

    #[test]
    fn soltar_desde_o_hacia_sistema_pide_confirmacion() {
        let (mut d, mut sys) = setup();
        let now = sys.now();
        let s = String::from("C:\\Sistema\\temp\\x.txt");
        sys.files.write(&s, b"x", now).unwrap();
        let a = add(&mut d, &mut sys, Rect::new(400, 100, 300, 300), Some(vec![s.clone()]), true).0;
        start_from(&mut d, &mut sys, a);
        mv(&mut d, &mut sys, (1000, 500));
        d.left_button(false, &mut sys);
        assert_eq!(d.dialog_count(), 1);
        assert!(sys.files.exists(&s), "no se mueve hasta confirmar");
        // No: nada.
        d.system_result(DialogResult::Answer { tag: TAG_DROP, answer: Answer::No }, &mut sys);
        assert!(sys.files.exists(&s) && d.pending_drop.is_none());
        // Sí: se mueve.
        rearm(&mut d, a, vec![s.clone()]);
        d.wins.retain(|w| w.dlg.is_none());
        start_from(&mut d, &mut sys, a);
        mv(&mut d, &mut sys, (1000, 500));
        d.left_button(false, &mut sys);
        d.system_result(DialogResult::Answer { tag: TAG_DROP, answer: Answer::Yes }, &mut sys);
        assert!(!sys.files.exists(&s) && sys.files.exists(&desk(&sys, "x.txt")));
    }

    #[test]
    fn el_escritorio_como_origen_umbral_seleccion_y_exclusiones() {
        let (mut d, mut sys) = setup();
        let (_, lb) = add(&mut d, &mut sys, Rect::new(500, 100, 300, 300), None, true);
        let b = d.wins.last().unwrap().id;
        let p = center(d.icon_cell(7));
        mv(&mut d, &mut sys, p);
        d.left_button(true, &mut sys);
        mv(&mut d, &mut sys, (p.0 + 5, p.1 + 5));
        assert!(d.dnd.is_none(), "5 px aún no es arrastre");
        mv(&mut d, &mut sys, (p.0 + 6, p.1));
        assert_eq!(d.dnd.as_ref().map(|x| x.paths.clone()), Some(vec![desk(&sys, "a.txt")]));
        go(&mut d, &mut sys, b, 20, 30);
        assert!(has(&lb, "over 20,30"));
        d.left_button(false, &mut sys);
        assert!(has(&lb, &format!("drop 20,30 false {}", desk(&sys, "a.txt"))));
        // Las apps y la Papelera no se arrastran.
        for i in [1usize, 5] {
            let p = center(d.icon_cell(i));
            d.event(Event::Tick(d.now_ms + 1000), &mut sys);
            mv(&mut d, &mut sys, p);
            d.left_button(true, &mut sys);
            mv(&mut d, &mut sys, (p.0 + 20, p.1 + 20));
            assert!(d.dnd.is_none(), "icono {i}");
            d.left_button(false, &mut sys);
        }
        // Una selección de varios se arrastra entera; el arrastre desde uno no seleccionado lleva solo ese.
        click_icon(&mut d, &mut sys, 7);
        mods(&mut d, &mut sys, true, false);
        click_icon(&mut d, &mut sys, 8);
        mods(&mut d, &mut sys, false, false);
        assert_eq!(d.sel, vec![7, 8]);
        let p = center(d.icon_cell(8));
        d.event(Event::Tick(d.now_ms + 1000), &mut sys);
        mv(&mut d, &mut sys, p);
        d.left_button(true, &mut sys);
        mv(&mut d, &mut sys, (p.0 + 20, p.1));
        assert_eq!(d.dnd.as_ref().unwrap().paths.len(), 2);
        // Soltar sobre la carpeta del propio escritorio.
        goi(&mut d, &mut sys, 6);
        d.left_button(false, &mut sys);
        assert!(sys.files.exists(&format!("{}\\a.txt", desk(&sys, "Carpeta"))));
        assert!(sys.files.exists(&format!("{}\\b.txt", desk(&sys, "Carpeta"))));
        d.event(Event::Tick(d.now_ms + 1000), &mut sys);
        assert_eq!(d.icons.len(), 7, "los iconos se recargan");
        click_icon(&mut d, &mut sys, 8);
        click_icon(&mut d, &mut sys, 6);
        assert_eq!(d.sel, vec![6]);
    }

    #[test]
    fn seleccion_multiple_ctrl_mayus_ctrl_a_y_recuadro() {
        let (mut d, mut sys) = setup();
        click_icon(&mut d, &mut sys, 6);
        assert_eq!(d.sel, vec![6]);
        mods(&mut d, &mut sys, true, false);
        click_icon(&mut d, &mut sys, 8);
        assert_eq!(d.sel, vec![6, 8]);
        click_icon(&mut d, &mut sys, 6);
        assert_eq!(d.sel, vec![8]);
        mods(&mut d, &mut sys, false, false);
        click_icon(&mut d, &mut sys, 6);
        mods(&mut d, &mut sys, false, true);
        click_icon(&mut d, &mut sys, 8);
        mods(&mut d, &mut sys, false, false);
        assert_eq!(d.sel, vec![6, 7, 8]);
        // Clic sobre un icono ya seleccionado de varios: se conserva hasta soltar; al soltar se queda solo él.
        d.event(Event::Tick(d.now_ms + 1000), &mut sys);
        goi(&mut d, &mut sys, 7);
        d.left_button(true, &mut sys);
        assert_eq!(d.sel, vec![6, 7, 8]);
        d.left_button(false, &mut sys);
        assert_eq!(d.sel, vec![7]);
        // Ctrl+A.
        key(&mut d, &mut sys, Key::Char('a'), Some('a'), true);
        assert_eq!(d.sel, (0..d.icons.len()).collect::<Vec<_>>());
        // Recuadro: sin Ctrl, reemplaza.
        // 8 iconos por columna: 0..=7 en la primera y 8 arriba en la segunda.
        let c6 = d.icon_cell(6);
        let start = (110, d.icon_cell(7).bottom() + 22);
        assert_eq!(d.hit(start.0, start.1), Hit::None);
        mv(&mut d, &mut sys, start);
        d.left_button(true, &mut sys);
        assert!(d.sel.is_empty());
        mv(&mut d, &mut sys, (start.0 - 2, start.1 - 2));
        assert!(d.band.is_none(), "movimiento mínimo: aún sin recuadro");
        mv(&mut d, &mut sys, (c6.x + 5, c6.y + 5));
        assert!(d.band.is_some());
        assert_eq!(d.sel, vec![6, 7]);
        render_equal(&mut d, &mut sys, "recuadro");
        d.left_button(false, &mut sys);
        assert!(d.band.is_none() && d.sel == vec![6, 7]);
        render_equal(&mut d, &mut sys, "recuadro soltado");
        // Con Ctrl suma a lo que había.
        click_icon(&mut d, &mut sys, 8);
        mods(&mut d, &mut sys, true, false);
        mv(&mut d, &mut sys, start);
        d.left_button(true, &mut sys);
        let y7 = d.icon_cell(7).y + 10;
        mv(&mut d, &mut sys, (c6.x + 5, y7));
        d.left_button(false, &mut sys);
        mods(&mut d, &mut sys, false, false);
        assert_eq!(d.sel, vec![7, 8]);
        // Pulsar fuera del escritorio (ventana o barra) no inicia recuadro.
        let w = add(&mut d, &mut sys, Rect::new(500, 100, 300, 300), None, true).0;
        go(&mut d, &mut sys, w, 20, 20);
        d.left_button(true, &mut sys);
        assert!(!matches!(d.drag, Drag::Band(..)));
        d.left_button(false, &mut sys);
        mv(&mut d, &mut sys, (400, 790));
        d.left_button(true, &mut sys);
        assert!(!matches!(d.drag, Drag::Band(..)));
        d.left_button(false, &mut sys);
    }

    #[test]
    fn supr_cortar_copiar_y_menu_actuan_sobre_todos_los_seleccionados() {
        let (mut d, mut sys) = setup();
        let right_click = |d: &mut Desktop, sys: &mut TestPlatform, p: (i32, i32)| {
            d.mouse_to(p.0, p.1, sys);
            d.right_button(true, sys);
            d.right_button(false, sys);
        };
        click_icon(&mut d, &mut sys, 7);
        mods(&mut d, &mut sys, true, false);
        click_icon(&mut d, &mut sys, 8);
        mods(&mut d, &mut sys, false, false);
        // Clic derecho sobre uno ya seleccionado conserva la selección y sale el menú de varios.
        let p = center(d.icon_cell(7));
        right_click(&mut d, &mut sys, p);
        assert_eq!(d.sel, vec![7, 8]);
        assert!(d.menu().is_some());
        let it = d.icon_items(&mut sys);
        let off: Vec<&str> = it.iter().filter(|i| i.disabled).map(|i| i.text.as_str()).collect();
        assert_eq!(off, ["Abrir", "Abrir con", "Cambiar nombre"]);
        d.desktop_menu_result(MENU_ICON, M_PROPS, &mut sys);
        assert_eq!(d.dialog_count(), 1);
        d.wins.retain(|w| w.dlg.is_none());
        d.desktop_menu_result(MENU_ICON, M_COPY, &mut sys);
        assert_eq!(sys.files.clip_paths().0.len(), 2);
        d.close_menu();
        // Sobre uno no seleccionado: lo selecciona solo.
        let p = center(d.icon_cell(6));
        right_click(&mut d, &mut sys, p);
        assert_eq!(d.sel, vec![6]);
        d.close_menu();
        // F2 exige uno solo.
        d.set_sel(vec![7, 8]);
        key(&mut d, &mut sys, Key::F(2), None, false);
        assert_eq!(d.dialog_count(), 0);
        // Ctrl+A y Supr: solo se eliminan archivos y carpetas; las apps y la Papelera se quedan.
        key(&mut d, &mut sys, Key::Char('a'), Some('a'), true);
        key(&mut d, &mut sys, Key::Delete, None, false);
        key(&mut d, &mut sys, Key::Enter, None, false); // Sí
        assert_eq!(d.icons.len(), 6);
        assert_eq!(d.dialog_count(), 0);
        // Deshacer lo devuelve como un solo lote.
        key(&mut d, &mut sys, Key::Char('z'), Some('z'), true);
        d.event(Event::Tick(d.now_ms + 10), &mut sys);
        assert_eq!(d.icons.len(), 9);
        // Todo apps: Cortar y Eliminar en gris.
        d.set_sel(vec![0, 1]);
        let it = d.icon_items(&mut sys);
        assert!(it.iter().filter(|i| ["Cortar", "Copiar", "Eliminar"].contains(&i.text.as_str())).all(|i| i.disabled));
    }

    #[test]
    fn zonas_danadas_de_etiqueta_recuadro_y_resaltado_son_exactas() {
        let (mut d, mut sys) = setup();
        let f = docs(&sys, "f.txt");
        let g = docs(&sys, "g.txt");
        let a = add(&mut d, &mut sys, Rect::new(400, 100, 300, 300), Some(vec![f.clone(), g]), true).0;
        d.render(&mut sys);
        start_from(&mut d, &mut sys, a);
        render_equal(&mut d, &mut sys, "etiqueta al empezar");
        for p in [(450, 300), (800, 500), (1270, 780), (5, 5), (center(d.icon_cell(6)).0, center(d.icon_cell(6)).1), (600, 200)] {
            mv(&mut d, &mut sys, p);
            render_equal(&mut d, &mut sys, "etiqueta en movimiento");
        }
        mods(&mut d, &mut sys, true, false);
        render_equal(&mut d, &mut sys, "etiqueta con + Copiar");
        mods(&mut d, &mut sys, false, false);
        key(&mut d, &mut sys, Key::Escape, None, false);
        render_equal(&mut d, &mut sys, "arrastre cancelado");
        d.left_button(false, &mut sys);
        render_equal(&mut d, &mut sys, "tras soltar");
        // Recuadro creciendo, encogiendo y soltado.
        d.minimize(a);
        mv(&mut d, &mut sys, (600, 700));
        d.left_button(true, &mut sys);
        for p in [(400, 500), (100, 100), (300, 300), (1000, 750)] {
            mv(&mut d, &mut sys, p);
            render_equal(&mut d, &mut sys, "recuadro");
        }
        d.left_button(false, &mut sys);
        render_equal(&mut d, &mut sys, "recuadro soltado");
    }

    /// Capturas para revisar a ojo: `NF_CAPTURAS=carpeta cargo test -p nf-core capturas_del_arrastre`.
    // ---- v0.7.2: arrastre con el botón derecho ----

    /// Pulsa el derecho en `p`.
    fn rdown(d: &mut Desktop, sys: &mut TestPlatform, p: (i32, i32)) {
        mv(d, sys, p);
        d.right_button(true, sys);
    }

    /// Pulsa el derecho en (10, 10) del contenido de `id` y mueve a (20, 20): empieza el arrastre.
    fn rstart(d: &mut Desktop, sys: &mut TestPlatform, id: u32) {
        let p = at(d, id, 10, 10);
        rdown(d, sys, p);
        go(d, sys, id, 20, 20);
    }

    fn clk(d: &mut Desktop, sys: &mut TestPlatform, p: (i32, i32)) {
        mv(d, sys, p);
        d.left_button(true, sys);
        d.left_button(false, sys);
    }

    /// Elige el elemento `k` del menú abierto con un clic.
    fn pick(d: &mut Desktop, sys: &mut TestPlatform, k: usize) {
        let r = d.menu().expect("menú").item_rect(k);
        clk(d, sys, center(r));
    }

    #[test]
    fn el_derecho_sin_arrastrar_abre_el_menu_al_soltar_en_el_punto_de_la_pulsacion() {
        let (mut d, mut sys) = setup();
        let (a, la) = add(&mut d, &mut sys, Rect::new(300, 100, 300, 300), None, true);
        let p = at(&d, a, 10, 10);
        rdown(&mut d, &mut sys, p);
        assert!(!has(&la, "right"), "al pulsar no llega nada");
        // Se mueve menos que el umbral y se suelta: RightDown en el punto de la pulsación.
        mv(&mut d, &mut sys, (p.0 + 3, p.1 + 2));
        assert!(d.dnd.is_none());
        d.right_button(false, &mut sys);
        assert!(has(&la, "right 10,10") && !has(&la, "src"), "{:?}", snapshot(&la));
        // Más de 5 px pero la app no ofrece nada: tampoco hay arrastre y al soltar sale lo de siempre.
        let (b, lb) = add(&mut d, &mut sys, Rect::new(700, 100, 300, 300), None, true);
        let q = at(&d, b, 30, 30);
        rdown(&mut d, &mut sys, q);
        go(&mut d, &mut sys, b, 50, 50);
        assert!(d.dnd.is_none() && has(&lb, "src 30,30"));
        d.right_button(false, &mut sys);
        assert!(has(&lb, "right 30,30"));
        // Fondo e icono: el menú del escritorio sale al soltar, no al pulsar.
        d.close_menu();
        rdown(&mut d, &mut sys, (1000, 600));
        assert!(d.menu().is_none());
        d.right_button(false, &mut sys);
        assert!(d.menu.as_ref().unwrap().tag == MENU_DESK);
        d.close_menu();
        let c = center(d.icon_cell(6));
        rdown(&mut d, &mut sys, c);
        assert!(d.menu().is_none());
        d.right_button(false, &mut sys);
        assert!(d.menu.as_ref().unwrap().tag == MENU_ICON && d.sel == vec![6]);
    }

    #[test]
    fn arrastre_derecho_a_otra_ventana_pide_mover_o_copiar_al_soltar() {
        let (mut d, mut sys) = setup();
        let f = docs(&sys, "f.txt");
        let ((a, la), (b, lb)) = two(&mut d, &mut sys, vec![f.clone()], true);
        rstart(&mut d, &mut sys, a);
        assert!(has(&la, "src 10,10") && d.dnd.as_ref().is_some_and(|x| x.right) && d.drag == Drag::Files);
        assert!(has(&la, "over 20,20"));
        // Con Ctrl la etiqueta no dice "Copiar": se decide en el menú.
        mods(&mut d, &mut sys, true, false);
        assert!(!d.dnd_text().contains("Copiar"));
        mods(&mut d, &mut sys, false, false);
        go(&mut d, &mut sys, b, 15, 25);
        d.right_button(false, &mut sys);
        assert!(d.dnd.is_none() && d.drag == Drag::None);
        assert!(d.menu.as_ref().unwrap().tag == MENU_DROP);
        assert!(!has(&lb, "drop"), "nada hasta elegir");
        assert_eq!(count(&lb, "over none"), 1);
        // Mover aquí (elemento 0).
        pick(&mut d, &mut sys, 0);
        assert!(has(&lb, &format!("drop 15,25 false {f}")), "{:?}", snapshot(&lb));
        assert!(d.menu().is_none());
        // Copiar aquí (elemento 1), con las coordenadas de AL SOLTAR.
        rearm(&mut d, a, vec![f.clone()]);
        lb.borrow_mut().clear();
        rstart(&mut d, &mut sys, a);
        go(&mut d, &mut sys, b, 40, 50);
        d.right_button(false, &mut sys);
        pick(&mut d, &mut sys, 1);
        assert!(has(&lb, &format!("drop 40,50 true {f}")), "{:?}", snapshot(&lb));
        // Cancelar (elemento 3, tras el separador) y clic fuera: nada.
        for fuera in [false, true] {
            lb.borrow_mut().clear();
            rearm(&mut d, a, vec![f.clone()]);
            rstart(&mut d, &mut sys, a);
            go(&mut d, &mut sys, b, 40, 50);
            d.right_button(false, &mut sys);
            if fuera {
                clk(&mut d, &mut sys, (5, 5));
            } else {
                pick(&mut d, &mut sys, 3);
            }
            assert!(d.menu().is_none() && !has(&lb, "drop"), "{:?}", snapshot(&lb));
        }
        // Esc con el menú abierto: tampoco.
        rearm(&mut d, a, vec![f.clone()]);
        rstart(&mut d, &mut sys, a);
        go(&mut d, &mut sys, b, 40, 50);
        d.right_button(false, &mut sys);
        key(&mut d, &mut sys, Key::Escape, None, false);
        assert!(d.menu().is_none() && !has(&lb, "drop"));
    }

    #[test]
    fn arrastre_derecho_al_escritorio_carpeta_y_papelera_con_deshacer() {
        let (mut d, mut sys) = setup();
        let (f, g) = (docs(&sys, "f.txt"), docs(&sys, "g.txt"));
        let a = add(&mut d, &mut sys, Rect::new(400, 100, 300, 300), Some(vec![f.clone()]), true).0;
        // Al fondo: Mover aquí.
        rstart(&mut d, &mut sys, a);
        mv(&mut d, &mut sys, (1000, 500));
        assert!(d.dnd.as_ref().unwrap().ok);
        d.right_button(false, &mut sys);
        assert!(sys.files.exists(&f), "no se mueve hasta elegir");
        assert!(d.dnd_hl.is_none());
        pick(&mut d, &mut sys, 0);
        assert!(!sys.files.exists(&f) && sys.files.exists(&desk(&sys, "f.txt")));
        // Deshacer con Ctrl+Z.
        d.minimize(a);
        key(&mut d, &mut sys, Key::Char('z'), Some('z'), true);
        assert!(sys.files.exists(&f) && !sys.files.exists(&desk(&sys, "f.txt")));
        d.raise(a);
        // A la carpeta: Copiar aquí.
        rearm(&mut d, a, vec![f.clone()]);
        rstart(&mut d, &mut sys, a);
        goi(&mut d, &mut sys, 6);
        assert_eq!(d.dnd_hl, Some(6));
        d.right_button(false, &mut sys);
        pick(&mut d, &mut sys, 1);
        assert!(sys.files.exists(&f) && sys.files.exists(&format!("{}\\f.txt", desk(&sys, "Carpeta"))));
        // A la Papelera: "Mover a la Papelera" (0) y Cancelar (2, tras el separador).
        rearm(&mut d, a, vec![g.clone()]);
        rstart(&mut d, &mut sys, a);
        goi(&mut d, &mut sys, 5);
        d.right_button(false, &mut sys);
        pick(&mut d, &mut sys, 2);
        assert!(sys.files.exists(&g));
        rearm(&mut d, a, vec![g.clone()]);
        rstart(&mut d, &mut sys, a);
        goi(&mut d, &mut sys, 5);
        d.right_button(false, &mut sys);
        pick(&mut d, &mut sys, 0);
        assert!(!sys.files.exists(&g));
        // Fuera de un destino válido (barra de tareas): sin menú.
        rearm(&mut d, a, vec![g.clone()]);
        rstart(&mut d, &mut sys, a);
        mv(&mut d, &mut sys, (400, 790));
        d.right_button(false, &mut sys);
        assert!(d.menu().is_none() && d.drag == Drag::None);
    }

    #[test]
    fn arrastre_derecho_desde_un_icono_y_sus_exclusiones() {
        let (mut d, mut sys) = setup();
        let (_, lb) = add(&mut d, &mut sys, Rect::new(500, 100, 300, 300), None, true);
        let b = d.wins.last().unwrap().id;
        let a_txt = desk(&sys, "a.txt");
        goi(&mut d, &mut sys, 7);
        d.right_button(true, &mut sys);
        assert!(d.sel.is_empty() && d.menu().is_none(), "al pulsar no se selecciona ni sale menú");
        let p = center(d.icon_cell(7));
        mv(&mut d, &mut sys, (p.0 + 3, p.1));
        assert!(d.dnd.is_none());
        go(&mut d, &mut sys, b, 20, 20);
        assert!(d.dnd.as_ref().unwrap().right && d.sel == vec![7]);
        d.right_button(false, &mut sys);
        pick(&mut d, &mut sys, 0);
        assert!(has(&lb, &format!("drop 20,20 false {a_txt}")), "{:?}", snapshot(&lb));
        // Un icono de app (0) no arrastra: al soltar sale su menú.
        goi(&mut d, &mut sys, 0);
        d.right_button(true, &mut sys);
        mv(&mut d, &mut sys, (1000, 500));
        assert!(d.dnd.is_none());
        d.right_button(false, &mut sys);
        assert!(d.menu.as_ref().unwrap().tag == MENU_ICON);
        d.close_menu();
        // Fondo vacío: moverse no hace nada; al soltar, menú del escritorio.
        rdown(&mut d, &mut sys, (1100, 700));
        mv(&mut d, &mut sys, (1150, 740));
        assert!(d.dnd.is_none() && d.band.is_none());
        d.right_button(false, &mut sys);
        assert!(d.menu.as_ref().unwrap().tag == MENU_DESK);
    }

    #[test]
    fn arrastre_derecho_esc_menu_abierto_y_bloqueo() {
        let (mut d, mut sys) = setup();
        let f = docs(&sys, "f.txt");
        let ((a, _), (b, lb)) = two(&mut d, &mut sys, vec![f.clone()], true);
        rstart(&mut d, &mut sys, a);
        go(&mut d, &mut sys, b, 10, 10);
        key(&mut d, &mut sys, Key::Escape, None, false);
        assert!(d.dnd.is_none());
        assert_eq!(count(&lb, "over none"), 1);
        d.right_button(false, &mut sys);
        assert!(d.menu().is_none() && d.drag == Drag::None && !has(&lb, "drop"));
        // Tras cancelar, un clic normal funciona.
        go(&mut d, &mut sys, b, 5, 5);
        d.left_button(true, &mut sys);
        d.left_button(false, &mut sys);
        assert!(has(&lb, "down 5,5"));
        // El izquierdo no interfiere con un arrastre derecho en curso.
        rearm(&mut d, a, vec![f.clone()]);
        rstart(&mut d, &mut sys, a);
        go(&mut d, &mut sys, b, 10, 10);
        d.left_button(true, &mut sys);
        d.left_button(false, &mut sys);
        assert!(d.dnd.is_some());
        d.right_button(false, &mut sys);
        assert!(d.menu.as_ref().unwrap().tag == MENU_DROP);
        // Con un menú abierto, el derecho sigue sirviendo para elegir/cerrar y no arranca nada.
        let r = d.menu().unwrap().item_rect(3);
        mv(&mut d, &mut sys, center(r));
        d.right_button(true, &mut sys);
        d.right_button(false, &mut sys);
        assert!(d.menu().is_none() && !has(&lb, "drop") && d.rpress.is_none());
        // Bloqueo en pleno arrastre derecho.
        rearm(&mut d, a, vec![f.clone()]);
        rstart(&mut d, &mut sys, a);
        d.lock(&mut sys);
        assert!(d.dnd.is_none() && d.rpress.is_none());
        d.right_button(false, &mut sys);
        assert!(d.menu().is_none());
    }

    #[test]
    fn arrastre_derecho_hacia_sistema_pregunta_y_el_redibujado_es_igual() {
        let (mut d, mut sys) = setup();
        let now = sys.now();
        let s = String::from("C:\\Sistema\\temp\\x.txt");
        sys.files.write(&s, b"x", now).unwrap();
        let a = add(&mut d, &mut sys, Rect::new(400, 100, 300, 300), Some(vec![s.clone()]), true).0;
        rstart(&mut d, &mut sys, a);
        render_equal(&mut d, &mut sys, "arrastre derecho");
        mv(&mut d, &mut sys, (1000, 500));
        d.right_button(false, &mut sys);
        assert!(d.menu().is_some());
        render_equal(&mut d, &mut sys, "menú de soltar");
        pick(&mut d, &mut sys, 0);
        assert_eq!(d.dialog_count(), 1);
        assert!(sys.files.exists(&s), "no se mueve hasta confirmar");
        d.system_result(DialogResult::Answer { tag: TAG_DROP, answer: Answer::Yes }, &mut sys);
        assert!(!sys.files.exists(&s) && sys.files.exists(&desk(&sys, "x.txt")));
    }

    #[test]
    fn arrastre_derecho_entre_dos_exploradores_reales() {
        let (mut d, mut sys) = setup();
        let (dd, dk) = (sp(&sys.files, Std::Documents, ""), sp(&sys.files, Std::Desktop, ""));
        d.open_new_explorer(&dd, &mut sys);
        d.open_new_explorer(&dk, &mut sys);
        let (a, b) = (d.wins[0].id, d.wins[1].id);
        d.set_rect(0, Rect::new(10, 10, 600, 500));
        d.set_rect(1, Rect::new(650, 10, 600, 500));
        d.render(&mut sys);
        let docs_n = |sys: &mut TestPlatform| ["f.txt", "g.txt"].iter().filter(|n| sys.files.exists(&docs(sys, n))).count();
        // Busca un archivo de la lista de A: ahí el derecho arrastra.
        let mut found = false;
        for y in (60..260).step_by(6) {
            let p = at(&d, a, 300, y);
            rdown(&mut d, &mut sys, p);
            mv(&mut d, &mut sys, (p.0 + 8, p.1));
            if d.dnd.is_some() {
                found = true;
                break;
            }
            d.right_button(false, &mut sys);
            d.close_menu();
        }
        assert!(found, "el Explorador debe ofrecer un archivo");
        assert!(d.dnd.as_ref().unwrap().right);
        let drop = at(&d, b, 300, 380);
        mv(&mut d, &mut sys, drop);
        d.right_button(false, &mut sys);
        assert!(d.menu.as_ref().unwrap().tag == MENU_DROP);
        assert_eq!(docs_n(&mut sys), 2, "nada se ha movido aún");
        pick(&mut d, &mut sys, 0);
        assert_eq!(docs_n(&mut sys), 1, "mover");
        // Deshacer con Ctrl+Z (la ventana B tiene el foco y el Explorador lo atiende).
        key(&mut d, &mut sys, Key::Char('z'), Some('z'), true);
        assert_eq!(docs_n(&mut sys), 2);
        // Soltar el derecho sin arrastrar sobre zona vacía de A: menú del Explorador (no el de soltar).
        let q = at(&d, a, 300, 420);
        rdown(&mut d, &mut sys, q);
        d.right_button(false, &mut sys);
        assert!(d.menu.as_ref().is_some_and(|m| m.owner == Some(a)));
    }

    #[test]
    fn capturas_del_arrastre() {
        let Ok(dir) = std::env::var("NF_CAPTURAS") else { return };
        let save = |d: &mut Desktop, sys: &mut TestPlatform, n: &str| {
            d.render(sys);
            std::fs::write(format!("{dir}/{n}.bmp"), crate::bmp::encode(d.back())).unwrap();
        };
        let (mut d, mut sys) = setup();
        // Recuadro de selección sobre los iconos del escritorio.
        mv(&mut d, &mut sys, (180, 520));
        d.left_button(true, &mut sys);
        mv(&mut d, &mut sys, (20, 150));
        save(&mut d, &mut sys, "escritorio_recuadro");
        d.left_button(false, &mut sys);
        // Etiqueta de arrastre sobre una carpeta resaltada.
        let lista = vec![docs(&sys, "f.txt"), docs(&sys, "g.txt")];
        let a = add(&mut d, &mut sys, Rect::new(400, 100, 300, 300), Some(lista), true).0;
        start_from(&mut d, &mut sys, a);
        let c = d.icon_cell(6);
        mv(&mut d, &mut sys, (c.x + 30, c.y + 20));
        mods(&mut d, &mut sys, true, false);
        save(&mut d, &mut sys, "escritorio_etiqueta_carpeta");
    }
}
