//! Explorador de archivos: navegación, lista (detalles, lista o iconos), Papelera, menú contextual
//! y operaciones sobre `Files`.
//! CONTRATO: `pub fn new() -> Box<dyn App>`.
use super::{App, AppKind, Ctx, DragRequest, MouseEv};
use crate::dialogs::{self, Answer, Buttons, DialogRequest, DialogResult};
use crate::files::{self, file_name, join, parent, valid_name, Files, Std};
use crate::font::{self, Align};
use crate::fs::FsError;
use crate::geom::Rect;
use crate::icons::{self, Icon};
use crate::input::{Key, KeyEvent};
use crate::menu::{MenuItem, MenuRequest};
use crate::platform::{DateTime, Platform};
use crate::ribbon::{Group, Ribbon, RibbonAct, RibbonItem, Tab};
use crate::scrollbar::{Metrics, ScrollBar, SB_W};
use crate::surface::{blend, Surface};
use crate::theme::Theme;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Ordering;

/// Alto de la cinta (`Ribbon::height`, lo comprueba una prueba).
const RIB_H: i32 = 116;
const NAV_H: i32 = 34;
const HEAD_H: i32 = 22;
const ROW_H: i32 = 22;
/// Vista Detalles: franja libre a la izquierda de las filas, para empezar el recuadro de selección
/// aunque la lista llene la ventana.
const GUTTER: i32 = 16;
const STATUS_H: i32 = 22;
const PANEL_W: i32 = 140;
const COL_W: [i32; 3] = [152, 152, 76];
/// Lista: ancho de cada columna.
const LCOL_W: i32 = 190;
/// Filas que mueve la rueda por muesca en Detalles (en Iconos, una fila de la rejilla; en Lista, una columna).
const WHEEL_ROWS: i32 = 3;
const DOUBLE_MS: u64 = 500;
/// Píxeles que hay que mover con el botón pulsado para que sea un arrastre.
const DRAG_PX: i32 = 5;
/// Milisegundos entre línea y línea del desplazamiento automático del recuadro.
const AUTOSCROLL_MS: u64 = 60;
const TEXT: u32 = 0x1A1A1A;
const GRAY: u32 = 0x6A6A6A;
const DISABLED: u32 = 0xA6A6A6;
const LINE: u32 = 0xDADADA;
/// Alto de cada entrada del panel izquierdo.
const PLACE_H: i32 = 26;
/// Entradas del panel izquierdo, en orden.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Place {
    Std(Std),
    Disk,
    Trash,
}
const PLACES: [Place; 8] = [
    Place::Std(Std::Desktop),
    Place::Std(Std::Documents),
    Place::Std(Std::Downloads),
    Place::Std(Std::Pictures),
    Place::Std(Std::Music),
    Place::Std(Std::Videos),
    Place::Disk,
    Place::Trash,
];

impl Place {
    fn name(self) -> &'static str {
        match self {
            Place::Std(d) => d.name(),
            Place::Disk => "Disco local (C:)",
            Place::Trash => "Papelera",
        }
    }
    fn icon(self) -> Icon {
        match self {
            Place::Std(d) => std_icon(d),
            Place::Disk => Icon::Drive,
            Place::Trash => Icon::Trash,
        }
    }
    /// Carpeta de la sesión actual (o `TRASH` / `C:\`).
    fn path(self, f: &Files) -> String {
        match self {
            Place::Std(d) => f.std_dir(d),
            Place::Disk => String::from("C:\\"),
            Place::Trash => String::from(files::TRASH),
        }
    }
}

fn std_icon(d: Std) -> Icon {
    match d {
        Std::Desktop => Icon::Escritorio,
        Std::Documents => Icon::Documentos,
        Std::Downloads => Icon::Descargas,
        Std::Pictures => Icon::Imagenes,
        Std::Music => Icon::Musica,
        Std::Videos => Icon::Videos,
    }
}
const TAG_NEW: u32 = 1;
const TAG_RENAME: u32 = 2;
const TAG_EMPTY: u32 = 3;
const TAG_ERROR: u32 = 4;
const TAG_PROPS: u32 = 5;
const TAG_NEWDOC: u32 = 6;
const TAG_MOVETO: u32 = 7;
const TAG_COPYTO: u32 = 8;
const TAG_SYS: u32 = 9;
/// Confirmación de enviar a la Papelera.
const TAG_DEL: u32 = 10;
/// Menús: sobre un elemento / sobre la zona vacía / de la cinta (Archivo y Ordenar por).
const TAG_MENU_ITEM: u32 = 1;
const TAG_MENU_BG: u32 = 2;
const TAG_MENU_FILE: u32 = 3;
const TAG_MENU_SORT: u32 = 4;
const M_OPEN: u32 = 1;
const M_OPEN_NEW: u32 = 2;
const M_NOTEPAD: u32 = 3;
const M_CUT: u32 = 4;
const M_COPY: u32 = 5;
const M_PASTE: u32 = 6;
const M_DELETE: u32 = 7;
const M_RENAME: u32 = 8;
const M_PROPS: u32 = 9;
const M_RESTORE: u32 = 10;
const M_EMPTY: u32 = 11;
const M_REFRESH: u32 = 12;
const M_UNDO: u32 = 13;
const M_NEW_FOLDER: u32 = 14;
const M_NEW_DOC: u32 = 15;
const M_VIEW_DETAILS: u32 = 20;
const M_VIEW_LIST: u32 = 21;
const M_VIEW_ICONS: u32 = 22;
const M_VIEW_MEDIUM: u32 = 23;
const M_VIEW_SMALL: u32 = 24;
const M_SORT_NAME: u32 = 30;
const M_SORT_DATE: u32 = 31;
const M_SORT_TYPE: u32 = 32;
const M_SORT_SIZE: u32 = 33;
const M_ASC: u32 = 34;
const M_DESC: u32 = 35;
const M_FILE_NEWWIN: u32 = 40;
const M_FILE_TERM: u32 = 41;
const M_FILE_CLOSE: u32 = 42;
/// Botones de la cinta.
const R_COPY: u32 = 1;
const R_PASTE: u32 = 2;
const R_CUT: u32 = 3;
const R_MOVE: u32 = 4;
const R_COPYTO: u32 = 5;
const R_DEL: u32 = 6;
const R_REN: u32 = 7;
const R_NEWF: u32 = 8;
const R_NEWD: u32 = 9;
const R_PROPS: u32 = 10;
const R_OPEN: u32 = 11;
const R_SELALL: u32 = 12;
const R_SELNONE: u32 = 13;
const R_INVERT: u32 = 14;
const R_RESTORE: u32 = 15;
const R_EMPTY: u32 = 16;
const R_V_LARGE: u32 = 20;
const R_V_MEDIUM: u32 = 21;
const R_V_SMALL: u32 = 22;
const R_V_LIST: u32 = 23;
const R_V_DETAILS: u32 = 24;
const R_SORT: u32 = 25;
const R_EXT: u32 = 26;
/// Botón de cinta -> orden, para los que se activan o desactivan según el contexto.
const RIB_CMDS: [(u32, Cmd); 16] = [
    (R_COPY, Cmd::Copy),
    (R_PASTE, Cmd::Paste),
    (R_CUT, Cmd::Cut),
    (R_MOVE, Cmd::MoveTo),
    (R_COPYTO, Cmd::CopyTo),
    (R_DEL, Cmd::Delete),
    (R_REN, Cmd::Rename),
    (R_NEWF, Cmd::NewFolder),
    (R_NEWD, Cmd::NewDoc),
    (R_PROPS, Cmd::Props),
    (R_OPEN, Cmd::Open),
    (R_SELALL, Cmd::SelAll),
    (R_SELNONE, Cmd::SelNone),
    (R_INVERT, Cmd::Invert),
    (R_RESTORE, Cmd::Restore),
    (R_EMPTY, Cmd::EmptyTrash),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cmd {
    NewFolder,
    NewDoc,
    Rename,
    Copy,
    Cut,
    Paste,
    Delete,
    MoveTo,
    CopyTo,
    Open,
    Props,
    SelAll,
    SelNone,
    Invert,
    Restore,
    EmptyTrash,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum View {
    Details,
    List,
    Icons,
    Medium,
    Small,
}

impl View {
    /// Rejillas con desplazamiento vertical: (ancho de celda, alto de celda, icono, nombre a la derecha).
    fn grid(self) -> Option<(i32, i32, i32, bool)> {
        match self {
            View::Icons => Some((84, 76, 48, false)),
            View::Medium => Some((80, 68, 40, false)),
            View::Small => Some((176, 22, 16, true)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sort {
    Name,
    Date,
    Type,
    Size,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Hit {
    Back,
    Up,
    Crumb(String),
    Place(usize),
    Head(Sort),
}

/// Operación que modifica archivos; si toca `C:\Sistema` espera a que el usuario confirme.
#[derive(Clone, Debug)]
enum Act {
    Trash(Vec<String>),
    Rename { from: String, to: String },
    Create { path: String, dir: bool },
    Paste(String),
    Transfer { paths: Vec<String>, dir: String, mv: bool },
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

/// Pulsación sobre un elemento: puede acabar en arrastre de archivos.
struct Press {
    x: i32,
    y: i32,
    i: usize,
    /// Estaba dentro de una selección múltiple: si no hay arrastre, al soltar queda solo él.
    defer: bool,
    /// Ya se movió y no se pudo arrastrar (Papelera, protegidas): al soltar no cambia la selección.
    dead: bool,
}

/// Recuadro de selección. El ancla está en coordenadas del contenido (se desplaza con él); el
/// puntero en coordenadas de la ventana.
struct Mq {
    ax: i32,
    ay: i32,
    px: i32,
    py: i32,
    /// Selección de partida (la de antes si se empezó con Ctrl, vacía si no).
    base: Vec<usize>,
    add: bool,
    /// Ya se ha movido lo bastante para considerarlo un recuadro y no un clic.
    on: bool,
    last: u64,
}

/// Destino resaltado de un arrastre (`Area` no se resalta).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tgt {
    Row(usize),
    Place(usize),
    Area,
}

struct Explorer {
    /// Carpeta actual o `files::TRASH`.
    cwd: String,
    back: Vec<String>,
    rows: Vec<Row>,
    /// Elemento con el foco (el último tocado); si hay selección, está en `marked`.
    sel: Option<usize>,
    /// Extremo fijo de la selección por rango (Mayús).
    anchor: usize,
    /// Todos los seleccionados, ordenados.
    marked: Vec<usize>,
    /// Primera línea visible: fila (Detalles), fila de la rejilla (Iconos) o columna (Lista).
    top: usize,
    gen: u64,
    w: i32,
    h: i32,
    hits: Vec<(Rect, Hit)>,
    hover: Option<Hit>,
    /// Carpeta de cada entrada del panel (según la sesión; se pone al día al dibujar).
    places: Vec<String>,
    last_click: Option<(usize, u64)>,
    sb_v: ScrollBar,
    sb_h: ScrollBar,
    rib: Ribbon,
    /// La cinta está montada para la Papelera.
    rib_trash: bool,
    /// Nombres a seleccionar en la próxima recarga.
    want: Vec<String>,
    rename_from: String,
    /// Extensión que se conserva al cambiar nombre con las extensiones ocultas (con el punto).
    rename_ext: String,
    /// Rutas de Mover a / Copiar a mientras está abierto el selector de carpeta.
    pending: Vec<String>,
    view: View,
    sort: Sort,
    desc: bool,
    show_ext: bool,
    /// Archivo de preferencias ya leído (cambia con la cuenta de la sesión).
    prefs_for: String,
    /// Se puede crear y pegar en la carpeta actual (permisos de la sesión).
    can_create: bool,
    /// Operación en espera del Sí del aviso de `C:\Sistema`.
    pending_sys: Option<Act>,
    /// Hay algo en el portapapeles (se actualiza al dibujar y al ejecutar órdenes).
    has_clip: bool,
    /// Estado de Ctrl y Mayús para el ratón (`MouseEv` no lleva modificadores): el escritorio lo
    /// avisa con un `KeyEvent` de `Key::Other` cada vez que cambia.
    ctrl: bool,
    shift: bool,
    press: Option<Press>,
    mq: Option<Mq>,
    drop_tgt: Option<Tgt>,
}

pub fn new() -> Box<dyn App> {
    Box::new(make())
}

fn item(id: u32, t: &'static str, i: Icon) -> RibbonItem {
    RibbonItem::new(id, t, i)
}

/// La cinta del Explorador. En la Papelera, Inicio lleva además el grupo "Papelera".
fn build_ribbon(trash: bool) -> Ribbon {
    let mut inicio = vec![
        Group { title: "Portapapeles", items: vec![item(R_COPY, "Copiar", Icon::Copiar).large(), item(R_PASTE, "Pegar", Icon::Pegar).large(), item(R_CUT, "Cortar", Icon::Cortar)] },
        Group {
            title: "Organizar",
            items: vec![item(R_MOVE, "Mover a", Icon::MoverA), item(R_COPYTO, "Copiar a", Icon::CopiarA), item(R_DEL, "Eliminar", Icon::Eliminar), item(R_REN, "Cambiar nombre", Icon::CambiarNombre)],
        },
        Group { title: "Nuevo", items: vec![item(R_NEWF, "Nueva carpeta", Icon::CarpetaNueva).large(), item(R_NEWD, "Documento de texto", Icon::DocumentoNuevo)] },
        Group { title: "Abrir", items: vec![item(R_PROPS, "Propiedades", Icon::Propiedades).large(), item(R_OPEN, "Abrir", Icon::Abrir)] },
        Group {
            title: "Seleccionar",
            items: vec![
                item(R_SELALL, "Seleccionar todo", Icon::SeleccionarTodo),
                item(R_SELNONE, "No seleccionar nada", Icon::SeleccionarNada),
                item(R_INVERT, "Invertir selección", Icon::InvertirSeleccion),
            ],
        },
    ];
    if trash {
        // "Nuevo" no sirve en la Papelera: su sitio lo ocupa el grupo de la Papelera.
        inicio.remove(2);
        inicio.insert(
            0,
            Group { title: "Papelera", items: vec![item(R_RESTORE, "Restaurar", Icon::Abrir).large(), item(R_EMPTY, "Vaciar Papelera", Icon::Trash).large()] },
        );
    }
    let vista = vec![
        Group {
            title: "Diseño",
            items: vec![
                item(R_V_LARGE, "Iconos grandes", Icon::VistaIconosGrandes),
                item(R_V_MEDIUM, "Iconos medianos", Icon::VistaIconosMedianos),
                item(R_V_SMALL, "Iconos pequeños", Icon::VistaIconosPequenos),
                item(R_V_LIST, "Lista", Icon::VistaLista),
                item(R_V_DETAILS, "Detalles", Icon::VistaDetalles),
            ],
        },
        Group { title: "Vista actual", items: vec![item(R_SORT, "Ordenar por", Icon::Ordenar).large().dropdown()] },
        Group { title: "Mostrar u ocultar", items: vec![item(R_EXT, "Extensiones de nombre de archivo", Icon::FileBlank).checkbox()] },
    ];
    Ribbon::new(Some("Archivo"), vec![Tab { title: "Inicio", groups: inicio }, Tab { title: "Vista", groups: vista }])
}

fn make() -> Explorer {
    Explorer {
        // Vacío hasta el primer `reload`: Documentos depende de la sesión.
        cwd: String::new(),
        back: Vec::new(),
        rows: Vec::new(),
        sel: None,
        anchor: 0,
        marked: Vec::new(),
        top: 0,
        gen: u64::MAX,
        w: 1000,
        h: 520,
        hits: Vec::new(),
        hover: None,
        places: Vec::new(),
        last_click: None,
        sb_v: ScrollBar::new(false),
        sb_h: ScrollBar::new(true),
        rib: build_ribbon(false),
        rib_trash: false,
        want: Vec::new(),
        rename_from: String::new(),
        rename_ext: String::new(),
        pending: Vec::new(),
        view: View::Details,
        sort: Sort::Name,
        desc: false,
        show_ext: true,
        prefs_for: String::new(),
        can_create: true,
        pending_sys: None,
        has_clip: false,
        ctrl: false,
        shift: false,
        press: None,
        mq: None,
        drop_tgt: None,
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

/// "2 KB (1500 bytes)"; por debajo de 1 KB solo los bytes.
fn size_text(b: u64) -> String {
    if b < 1024 {
        fmt_size(b)
    } else {
        format!("{} ({b} bytes)", fmt_size(b))
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

fn kind_of(name: &str, dir: bool) -> &'static str {
    if dir {
        "Carpeta de archivos"
    } else if is_txt(name) {
        "Documento de texto"
    } else {
        "Archivo"
    }
}

fn count_text(n: usize) -> String {
    if n == 1 {
        String::from("1 elemento")
    } else {
        format!("{n} elementos")
    }
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn cmp_name(a: &str, b: &str) -> Ordering {
    a.chars().flat_map(char::to_lowercase).cmp(b.chars().flat_map(char::to_lowercase))
}

fn date_key(d: DateTime) -> (u16, u8, u8, u8, u8, u8) {
    (d.year, d.month, d.day, d.hour, d.minute, d.second)
}

fn clipped(s: &mut Surface, r: Rect, text: &str, color: u32, align: Align) {
    let old = s.clip();
    s.set_clip(old.intersect(r));
    font::draw_in(s, r, text, 9, false, color, align);
    s.set_clip(old);
}

/// ¿Deja la sesión crear o pegar algo directamente en `dir`? (No en `C:\Usuarios` ni en la
/// carpeta de una cuenta, ni en `C:\Sistema` sin ser administrador.)
fn can_create_in(f: &Files, dir: &str) -> bool {
    let c = dir.get(3..).unwrap_or("");
    let mut parts = c.split('\\').filter(|p| !p.is_empty());
    let users = parts.next().is_some_and(|p| same(p, "Usuarios"));
    if users && parts.count() <= 1 {
        return false;
    }
    !files::in_system(dir) || f.is_admin()
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

    /// El único seleccionado (con 0 o varios, `None`).
    fn single(&self) -> Option<&Row> {
        match self.marked.as_slice() {
            [i] => self.rows.get(*i),
            _ => None,
        }
    }

    /// Rutas de todos los seleccionados.
    fn paths(&self) -> Vec<String> {
        self.marked.iter().filter_map(|&i| self.rows.get(i)).map(|r| join(&self.cwd, &r.name)).collect()
    }

    // ---- selección ----

    fn is_sel(&self, i: usize) -> bool {
        self.marked.binary_search(&i).is_ok()
    }

    fn set_one(&mut self, i: usize) {
        self.sel = Some(i);
        self.anchor = i;
        self.marked = vec![i];
    }

    fn clear_sel(&mut self) {
        self.sel = None;
        self.marked.clear();
    }

    /// Ctrl+clic: añade o quita `i`.
    fn toggle(&mut self, i: usize) {
        match self.marked.binary_search(&i) {
            Ok(p) => {
                self.marked.remove(p);
                self.sel = self.marked.last().copied();
            }
            Err(p) => {
                self.marked.insert(p, i);
                self.sel = Some(i);
            }
        }
        self.anchor = i;
    }

    /// Mayús+clic: del ancla a `i`.
    fn range_to(&mut self, i: usize) {
        let a = self.anchor.min(self.rows.len().saturating_sub(1));
        self.marked = (a.min(i)..=a.max(i)).collect();
        self.sel = Some(i);
    }

    fn select_all(&mut self) {
        let n = self.rows.len();
        self.marked = (0..n).collect();
        self.sel = n.checked_sub(1);
        self.anchor = 0;
    }

    fn invert(&mut self) {
        let old = core::mem::take(&mut self.marked);
        self.marked = (0..self.rows.len()).filter(|i| old.binary_search(i).is_err()).collect();
        self.sel = self.marked.last().copied();
        self.anchor = self.sel.unwrap_or(0);
    }

    // ---- recordar vista, orden y extensiones ----

    fn load_prefs(&mut self, f: &mut Files) {
        let path = f.explorer_prefs();
        self.prefs_for = path.clone();
        // Cada cuenta tiene las suyas: se parte de los valores por defecto.
        (self.view, self.sort, self.desc) = (View::Details, Sort::Name, false);
        let Ok(bytes) = f.read(&path) else { return };
        let text = String::from_utf8_lossy(&bytes);
        for l in text.lines() {
            match l.split_once('=') {
                Some(("vista", "detalles")) => self.view = View::Details,
                Some(("vista", "lista")) => self.view = View::List,
                Some(("vista", "iconos")) => self.view = View::Icons,
                Some(("vista", "iconos-medianos")) => self.view = View::Medium,
                Some(("vista", "iconos-pequenos")) => self.view = View::Small,
                Some(("orden", "nombre")) => self.sort = Sort::Name,
                Some(("orden", "fecha")) => self.sort = Sort::Date,
                Some(("orden", "tipo")) => self.sort = Sort::Type,
                Some(("orden", "tamano")) => self.sort = Sort::Size,
                Some(("desc", "0")) => self.desc = false,
                Some(("desc", "1")) => self.desc = true,
                _ => {}
            }
        }
    }

    /// Escribe esas claves y conserva las demás líneas del archivo.
    fn write_prefs(sys: &mut dyn Platform, new: &[(&str, String)]) {
        let path = sys.files().explorer_prefs();
        let old = sys.files().read(&path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let mut out = String::new();
        for (k, v) in new {
            out += &format!("{k}={v}\n");
        }
        for l in old.lines().filter(|l| !l.trim().is_empty()) {
            let key = l.split('=').next().unwrap_or("").trim();
            if !new.iter().any(|(k, _)| *k == key) {
                out += l;
                out.push('\n');
            }
        }
        let now = sys.now();
        let _ = sys.files().write(&path, out.as_bytes(), now);
    }

    fn save_prefs(&self, sys: &mut dyn Platform) {
        let v = match self.view {
            View::Details => "detalles",
            View::List => "lista",
            View::Icons => "iconos",
            View::Medium => "iconos-medianos",
            View::Small => "iconos-pequenos",
        };
        let o = match self.sort {
            Sort::Name => "nombre",
            Sort::Date => "fecha",
            Sort::Type => "tipo",
            Sort::Size => "tamano",
        };
        Self::write_prefs(sys, &[("vista", v.into()), ("orden", o.into()), ("desc", format!("{}", self.desc as u8))]);
    }

    /// Tras cambiar vista u orden: se guarda, se reordena y se redibuja.
    fn prefs_changed(&mut self, cx: &mut Ctx) {
        self.top = 0;
        self.save_prefs(cx.sys);
        self.reload(cx.sys);
        if let Some(i) = self.sel {
            self.reveal(i);
        }
        cx.redraw = true;
    }

    fn sort_rows(&mut self) {
        let (key, desc) = (self.sort, self.desc);
        self.rows.sort_by(|a, b| {
            b.dir.cmp(&a.dir).then_with(|| {
                let o = match key {
                    Sort::Name => cmp_name(&a.name, &b.name),
                    Sort::Date => date_key(a.date).cmp(&date_key(b.date)),
                    Sort::Type => kind_of(&a.name, a.dir).cmp(kind_of(&b.name, b.dir)),
                    Sort::Size => a.size.cmp(&b.size),
                };
                (if desc { o.reverse() } else { o }).then_with(|| cmp_name(&a.name, &b.name))
            })
        });
    }

    // ---- geometría (depende de la vista) ----

    fn body_y(&self) -> i32 {
        RIB_H + NAV_H
    }

    fn top_y(&self) -> i32 {
        self.body_y() + if self.view == View::Details { HEAD_H } else { 0 }
    }

    fn horiz(&self) -> bool {
        self.view == View::List
    }

    /// (elementos por línea, líneas que caben, líneas totales) con o sin barra de desplazamiento.
    fn compute(&self, sb: bool) -> (usize, usize, usize) {
        let n = self.rows.len();
        let free_h = self.h - STATUS_H - self.top_y();
        let free_w = self.w - PANEL_W;
        let (pl, page) = match (self.view, self.view.grid()) {
            (View::Details, _) => (1, (free_h / ROW_H).max(1) as usize),
            (View::List, _) => (((free_h - if sb { SB_W } else { 0 }) / ROW_H).max(1) as usize, (free_w / LCOL_W).max(1) as usize),
            (_, Some((cw, ch, _, _))) => (((free_w - if sb { SB_W } else { 0 }) / cw).max(1) as usize, (free_h / ch).max(1) as usize),
            _ => (1, 1),
        };
        (pl, page, n.div_ceil(pl))
    }

    fn need_sb(&self) -> bool {
        let (_, page, lines) = self.compute(false);
        lines > page
    }

    fn dims(&self) -> (usize, usize, usize) {
        self.compute(self.need_sb())
    }

    /// Elementos por línea: 1 (Detalles), columnas de la rejilla o filas por columna (Lista).
    fn per_line(&self) -> usize {
        self.dims().0
    }

    /// Líneas que caben a la vez.
    fn page(&self) -> usize {
        self.dims().1
    }

    #[cfg(test)]
    fn lines(&self) -> usize {
        self.dims().2
    }

    fn max_top(&self) -> usize {
        let (_, page, lines) = self.dims();
        lines.saturating_sub(page)
    }

    /// Zona donde se dibujan los elementos (sin barra de desplazamiento).
    fn area(&self) -> Rect {
        let sb = if self.need_sb() { SB_W } else { 0 };
        let (vs, hs) = if self.horiz() { (0, sb) } else { (sb, 0) };
        Rect::new(PANEL_W, self.top_y(), self.w - PANEL_W - vs, (self.h - STATUS_H - self.top_y() - hs).max(1))
    }

    fn track(&self) -> Rect {
        if self.horiz() {
            Rect::new(PANEL_W, self.h - STATUS_H - SB_W, self.w - PANEL_W, SB_W)
        } else {
            Rect::new(self.w - SB_W, self.top_y(), SB_W, (self.h - STATUS_H - self.top_y()).max(1))
        }
    }

    fn metrics(&self) -> Metrics {
        let (_, page, lines) = self.dims();
        Metrics { rect: self.track(), total: lines as i32, page: page as i32, pos: self.top as i32, line: 1 }
    }

    fn col_x(&self) -> [i32; 4] {
        let lw = self.w - PANEL_W - if self.need_sb() { SB_W } else { 0 };
        let size = PANEL_W + lw - COL_W[2];
        let b = size - COL_W[1];
        let a = b - COL_W[0];
        [PANEL_W + GUTTER, a, b, size]
    }

    /// Rectángulo del elemento `i` (puede quedar fuera de la zona si no está visible).
    fn item_rect(&self, i: usize) -> Rect {
        let a = self.area();
        let pl = self.per_line();
        let (line, slot) = ((i / pl) as i32 - self.top as i32, (i % pl) as i32);
        match (self.view, self.view.grid()) {
            (_, Some((cw, ch, _, _))) => Rect::new(a.x + slot * cw, a.y + line * ch, cw, ch),
            (View::List, _) => Rect::new(a.x + line * LCOL_W, a.y + slot * ROW_H, LCOL_W, ROW_H),
            _ => Rect::new(a.x + GUTTER, a.y + line * ROW_H, a.w - GUTTER, ROW_H),
        }
    }

    fn item_at(&self, x: i32, y: i32) -> Option<usize> {
        let a = self.area();
        if !a.contains(x, y) {
            return None;
        }
        let (dx, dy) = (x - a.x, y - a.y);
        let (line, slot) = match (self.view, self.view.grid()) {
            (_, Some((cw, ch, _, _))) => (dy / ch, dx / cw),
            (View::List, _) => (dx / LCOL_W, dy / ROW_H),
            _ if dx < GUTTER => return None,
            _ => (dy / ROW_H, 0),
        };
        let (pl, page, _) = self.dims();
        if line as usize >= page || slot as usize >= pl {
            return None;
        }
        let i = (self.top + line as usize) * pl + slot as usize;
        (i < self.rows.len()).then_some(i)
    }

    fn hit_at(&self, x: i32, y: i32) -> Option<Hit> {
        self.hits.iter().find(|(r, _)| r.contains(x, y)).map(|(_, h)| h.clone())
    }

    // ---- recuadro de selección y arrastre ----

    /// Desplazamiento del contenido: coordenada de contenido = de ventana + esto.
    fn off(&self) -> (i32, i32) {
        let t = self.top as i32;
        match (self.view, self.view.grid()) {
            (_, Some((_, ch, _, _))) => (0, t * ch),
            (View::List, _) => (t * LCOL_W, 0),
            _ => (0, t * ROW_H),
        }
    }

    /// Selección = lo que toca el recuadro (respecto a la selección de partida).
    fn mq_apply(&mut self) {
        let Some(m) = &self.mq else { return };
        let (ox, oy) = self.off();
        let (px, py) = (m.px + ox, m.py + oy);
        let r = Rect::new(m.ax.min(px), m.ay.min(py), (m.ax - px).abs() + 1, (m.ay - py).abs() + 1);
        let mut v = Vec::new();
        for i in 0..self.rows.len() {
            let ir = self.item_rect(i);
            let hit = r.intersects(Rect::new(ir.x + ox, ir.y + oy, ir.w, ir.h));
            let was = m.base.binary_search(&i).is_ok();
            if if m.add { hit != was } else { hit || was } {
                v.push(i);
            }
        }
        self.sel = v.last().copied();
        self.anchor = self.sel.unwrap_or(0);
        self.marked = v;
    }

    /// Rectángulo del recuadro en la ventana, si hay uno dibujándose.
    fn mq_rect(&self) -> Option<Rect> {
        let m = self.mq.as_ref().filter(|m| m.on)?;
        let (ox, oy) = self.off();
        let (x0, y0) = (m.ax - ox, m.ay - oy);
        Some(Rect::new(x0.min(m.px), y0.min(m.py), (x0 - m.px).abs() + 1, (y0 - m.py).abs() + 1))
    }

    /// Destino de soltar `paths` en (x, y): carpeta y qué resaltar. `None` si ahí no se puede soltar.
    fn target(&self, x: i32, y: i32, paths: &[String], f: &Files) -> Option<(String, Tgt)> {
        let (dest, tgt) = if x < PANEL_W {
            let y0 = self.body_y() + 8;
            if y < y0 || y >= self.h - STATUS_H {
                return None;
            }
            let i = ((y - y0) / PLACE_H) as usize;
            (PLACES.get(i)?.path(f), Tgt::Place(i))
        } else if let Some(i) = self.item_at(x, y).filter(|&i| self.rows[i].dir && !self.trash()) {
            (join(&self.cwd, &self.rows[i].name), Tgt::Row(i))
        } else if self.area().contains(x, y) && !self.trash() {
            (self.cwd.clone(), Tgt::Area)
        } else {
            return None;
        };
        let inside = |p: &String| same(&dest, p) || (dest.len() > p.len() && same(&dest[..p.len()], p) && dest.as_bytes()[p.len()] == b'\\');
        let there = |p: &String| parent(p).is_some_and(|q| same(&q, &dest));
        let ok = !paths.is_empty()
            && !paths.iter().any(inside)
            && !paths.iter().all(there)
            && (dest == files::TRASH || can_create_in(f, &dest));
        ok.then_some((dest, tgt))
    }

    // ---- carga y operaciones ----

    fn reload(&mut self, sys: &mut dyn Platform) {
        if self.cwd.is_empty() {
            self.cwd = sys.files().std_dir(Std::Documents);
        }
        if self.prefs_for != sys.files().explorer_prefs() {
            self.load_prefs(sys.files());
        }
        let (keep, cur) = if self.want.is_empty() {
            (self.marked.iter().filter_map(|&i| self.rows.get(i)).map(|r| r.name.clone()).collect::<Vec<_>>(), self.sel_row().map(|r| r.name.clone()))
        } else {
            let w = core::mem::take(&mut self.want);
            let c = w.last().cloned();
            (w, c)
        };
        let f = sys.files();
        self.gen = f.generation();
        (self.press, self.mq) = (None, None);
        let prefs = f.explorer_prefs();
        self.show_ext = f.read(&prefs).map_or(true, |d| !String::from_utf8_lossy(&d).lines().any(|l| l.trim() == "extensiones=no"));
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
        self.can_create = !self.trash() && can_create_in(f, &self.cwd);
        self.sort_rows();
        self.marked = (0..self.rows.len()).filter(|&i| keep.contains(&self.rows[i].name)).collect();
        self.sel = cur.and_then(|c| self.marked.iter().copied().find(|&i| self.rows[i].name == c)).or(self.marked.last().copied());
        self.anchor = self.sel.unwrap_or(0);
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
        self.clear_sel();
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
        if let Some(r) = self.single() {
            let path = join(&self.cwd, &r.name);
            if r.dir {
                self.go(&path, true, cx);
            } else {
                cx.open = Some(path);
            }
        }
    }

    /// ¿La selección incluye una carpeta protegida? Solo puede haberla si la carpeta actual lo es.
    fn sel_protected(&self) -> bool {
        !self.trash() && files::is_protected(&self.cwd) && self.paths().iter().any(|p| files::is_protected(p))
    }

    fn enabled(&self, c: Cmd) -> bool {
        let n = self.marked.len();
        let trash = self.trash();
        match c {
            Cmd::NewFolder | Cmd::NewDoc => self.can_create,
            Cmd::Open => n == 1 && !trash,
            Cmd::Rename => n == 1 && !trash && !self.sel_protected(),
            Cmd::Copy | Cmd::CopyTo => n >= 1 && !trash,
            Cmd::Cut | Cmd::Delete | Cmd::MoveTo => n >= 1 && !trash && !self.sel_protected(),
            Cmd::Restore => n >= 1 && trash,
            Cmd::Paste => self.has_clip && self.can_create,
            Cmd::EmptyTrash => trash && !self.rows.is_empty(),
            Cmd::Props => true,
            Cmd::SelAll | Cmd::Invert => !self.rows.is_empty(),
            Cmd::SelNone => n > 0,
        }
    }

    fn run(&mut self, c: Cmd, cx: &mut Ctx) {
        self.has_clip = cx.sys.files().clip().is_some();
        if !self.enabled(c) {
            return;
        }
        let paths = self.paths();
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
            Cmd::NewDoc => self.new_doc(cx),
            Cmd::Rename => {
                let Some(r) = self.single() else { return };
                let shown = files::display_name(&r.name, r.dir, self.show_ext);
                let (value, ext) = (String::from(shown), String::from(&r.name[shown.len()..]));
                self.rename_from = paths[0].clone();
                self.rename_ext = ext;
                cx.dialog = Some(DialogRequest::Input {
                    tag: TAG_RENAME,
                    title: String::from("Cambiar nombre"),
                    label: String::from("Nuevo nombre:"),
                    value,
                });
            }
            Cmd::Copy | Cmd::Cut => {
                cx.sys.files().set_clip_many(&paths, c == Cmd::Cut);
                cx.redraw = true; // Pegar pasa a estar disponible en la cinta
            }
            Cmd::Paste => {
                let dir = self.cwd.clone();
                self.paste_into(&dir, cx);
            }
            Cmd::Delete => {
                let text = match paths.as_slice() {
                    [one] => format!("¿Quieres enviar «{}» a la Papelera?", files::file_name(one)),
                    many => format!("¿Quieres enviar estos {} elementos a la Papelera?", many.len()),
                };
                self.pending_sys = Some(Act::Trash(paths));
                cx.dialog = Some(DialogRequest::Message { tag: TAG_DEL, title: String::from("Eliminar"), text, buttons: Buttons::YesNo });
            }
            Cmd::MoveTo | Cmd::CopyTo => {
                let mv = c == Cmd::MoveTo;
                self.pending = paths;
                cx.dialog = Some(DialogRequest::Folder {
                    tag: if mv { TAG_MOVETO } else { TAG_COPYTO },
                    title: String::from(if mv { "Mover a" } else { "Copiar a" }),
                    dir: self.cwd.clone(),
                });
            }
            Cmd::Open => self.open_sel(cx),
            Cmd::Props => self.props(!self.marked.is_empty(), cx),
            Cmd::SelAll => self.select_all(),
            Cmd::SelNone => self.clear_sel(),
            Cmd::Invert => self.invert(),
            Cmd::Restore => {
                let ids: Vec<String> = self.marked.iter().filter_map(|&i| self.rows.get(i)).map(|r| r.id.clone()).collect();
                let mut r = Ok(());
                for id in ids {
                    if let Err(e) = cx.sys.files().restore(&id) {
                        r = Err(e);
                        break;
                    }
                }
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

    fn paste_into(&mut self, dir: &str, cx: &mut Ctx) {
        self.act(Act::Paste(String::from(dir)), cx);
    }

    /// Hace la operación; si toca `C:\Sistema`, antes pregunta (Sí/No) y la deja en espera.
    fn act(&mut self, a: Act, cx: &mut Ctx) {
        let touches = |p: &String| files::in_system(p);
        let sys = match &a {
            Act::Trash(v) => v.iter().any(touches),
            Act::Rename { from, to } => touches(from) || touches(to),
            Act::Create { path, .. } => touches(path),
            Act::Paste(dir) => {
                let (src, cut) = cx.sys.files().clip_paths();
                touches(dir) || (cut && src.iter().any(touches))
            }
            Act::Transfer { paths, dir, mv } => touches(dir) || (*mv && paths.iter().any(touches)),
        };
        if sys {
            self.pending_sys = Some(a);
            cx.dialog = Some(DialogRequest::Message {
                tag: TAG_SYS,
                title: String::from("Carpeta del sistema"),
                text: String::from(
                    "Vas a modificar una carpeta del sistema. Si cambias o borras algo aquí, NightFury puede dejar de funcionar. ¿Continuar?",
                ),
                buttons: Buttons::YesNo,
            });
            cx.redraw = true;
        } else {
            self.apply(a, cx);
        }
    }

    fn apply(&mut self, a: Act, cx: &mut Ctx) {
        let now = cx.sys.now();
        let r = match a {
            Act::Trash(paths) => cx.sys.files().user_trash_many(&paths, now),
            Act::Rename { from, to } => {
                self.want = vec![String::from(file_name(&to))];
                cx.sys.files().user_rename(&from, &to)
            }
            Act::Create { path, dir } => {
                self.want = vec![String::from(file_name(&path))];
                cx.sys.files().user_create(&path, dir, now)
            }
            Act::Paste(dir) => {
                let r = cx.sys.files().paste_all(&dir, now);
                if let (Ok(v), true) = (&r, same(&dir, &self.cwd)) {
                    self.want = v.iter().map(|p| String::from(file_name(p))).collect();
                }
                r.map(|_| ())
            }
            Act::Transfer { paths, dir, mv } => cx.sys.files().transfer(&paths, &dir, mv, now).map(|v| {
                if same(&dir, &self.cwd) {
                    self.want = v.iter().map(|p| String::from(file_name(p))).collect();
                }
            }),
        };
        self.done(r, cx);
    }

    fn undo(&mut self, cx: &mut Ctx) {
        if cx.sys.files().undo_label().is_none() {
            return;
        }
        let now = cx.sys.now();
        let r = cx.sys.files().undo(now);
        self.done(r, cx);
    }

    fn new_doc(&mut self, cx: &mut Ctx) {
        if !self.can_create {
            return;
        }
        let name = cx.sys.files().free_name(&self.cwd, "Nuevo documento de texto.txt");
        cx.dialog = Some(DialogRequest::Input {
            tag: TAG_NEWDOC,
            title: String::from("Nuevo documento de texto"),
            label: String::from("Nombre del documento:"),
            value: name,
        });
    }

    /// Desplaza lo justo para que se vea el elemento `i`.
    fn reveal(&mut self, i: usize) {
        let (pl, page, _) = self.dims();
        let line = i / pl;
        if line < self.top {
            self.top = line;
        } else if line >= self.top + page {
            self.top = line + 1 - page;
        }
    }

    /// Lleva el foco a `i`: selección simple o, con `extend`, rango desde el ancla.
    fn go_to(&mut self, i: usize, extend: bool) {
        if self.rows.is_empty() {
            return;
        }
        let i = i.min(self.rows.len() - 1);
        self.reveal(i);
        if extend && self.sel.is_some() {
            self.range_to(i);
        } else {
            self.set_one(i);
        }
    }

    /// Mueve la selección `d` elementos (sin selección: primero o último).
    fn step(&mut self, d: i32, extend: bool) {
        let n = self.rows.len() as i32;
        if n == 0 {
            return;
        }
        let i = match self.sel {
            None if d > 0 => 0,
            None => n - 1,
            Some(c) => (c as i32 + d).clamp(0, n - 1),
        };
        self.go_to(i as usize, extend);
    }

    // ---- cinta ----

    /// Pone al día los botones desactivados y marcados según el contexto.
    fn sync_ribbon(&mut self) {
        if self.trash() != self.rib_trash {
            self.rib_trash = self.trash();
            let a = self.rib.active();
            self.rib = build_ribbon(self.rib_trash);
            self.rib.set_active(a);
        }
        for (id, c) in RIB_CMDS {
            let off = !self.enabled(c);
            self.rib.set_disabled(id, off);
        }
        for (id, v) in [(R_V_LARGE, View::Icons), (R_V_MEDIUM, View::Medium), (R_V_SMALL, View::Small), (R_V_LIST, View::List), (R_V_DETAILS, View::Details)] {
            self.rib.set_checked(id, self.view == v);
        }
        self.rib.set_checked(R_EXT, self.show_ext);
    }

    fn rib_pick(&mut self, id: u32, cx: &mut Ctx) {
        if let Some(&(_, c)) = RIB_CMDS.iter().find(|(i, _)| *i == id) {
            self.run(c, cx);
            return;
        }
        match id {
            R_V_LARGE | R_V_MEDIUM | R_V_SMALL | R_V_LIST | R_V_DETAILS => {
                self.view = match id {
                    R_V_LARGE => View::Icons,
                    R_V_MEDIUM => View::Medium,
                    R_V_SMALL => View::Small,
                    R_V_LIST => View::List,
                    _ => View::Details,
                };
                self.prefs_changed(cx);
            }
            R_SORT => {
                if let Some(r) = self.rib.item_rect(R_SORT, self.w) {
                    cx.menu = Some(MenuRequest { tag: TAG_MENU_SORT, x: r.x, y: r.bottom(), items: self.sort_items() });
                }
            }
            R_EXT => {
                let v = if self.show_ext { "no" } else { "si" };
                Self::write_prefs(cx.sys, &[("extensiones", v.into())]);
                self.reload(cx.sys);
            }
            _ => {}
        }
        cx.redraw = true;
    }

    fn file_menu(&self, cx: &mut Ctx) {
        let r = self.rib.file_rect();
        let n = MenuItem::new;
        cx.menu = Some(MenuRequest {
            tag: TAG_MENU_FILE,
            x: r.x,
            y: r.bottom(),
            items: vec![
                n(M_FILE_NEWWIN, "Abrir ventana nueva"),
                n(M_FILE_TERM, "Abrir Terminal aquí").disabled(self.trash()),
                MenuItem::sep(),
                n(M_FILE_CLOSE, "Cerrar"),
            ],
        });
    }

    // ---- menús ----

    fn sort_items(&self) -> Vec<MenuItem> {
        let k = |s: Sort, id: u32, t: &str| MenuItem::new(id, t).checked(self.sort == s);
        vec![
            k(Sort::Name, M_SORT_NAME, "Nombre"),
            k(Sort::Date, M_SORT_DATE, "Fecha de modificación"),
            k(Sort::Type, M_SORT_TYPE, "Tipo"),
            k(Sort::Size, M_SORT_SIZE, "Tamaño"),
            MenuItem::sep(),
            MenuItem::new(M_ASC, "Ascendente").checked(!self.desc),
            MenuItem::new(M_DESC, "Descendente").checked(self.desc),
        ]
    }

    fn sort_menu(&self) -> MenuItem {
        MenuItem::submenu("Ordenar por", self.sort_items())
    }

    /// `paste_ok`: se puede pegar dentro de la carpeta clicada.
    fn item_menu(&self, dir: bool, has_clip: bool, multi: bool, paste_ok: bool) -> Vec<MenuItem> {
        let n = MenuItem::new;
        if self.trash() {
            return vec![n(M_RESTORE, "Restaurar"), MenuItem::sep(), n(M_PROPS, "Propiedades")];
        }
        let mut v = vec![n(M_OPEN, "Abrir").bold().disabled(multi)];
        if dir {
            v.push(n(M_OPEN_NEW, "Abrir en ventana nueva"));
        } else {
            v.push(MenuItem::submenu("Abrir con", vec![n(M_NOTEPAD, "Bloc de notas")]).disabled(multi));
        }
        v.push(MenuItem::sep());
        let prot = self.sel_protected();
        v.push(n(M_CUT, "Cortar").disabled(prot));
        v.push(n(M_COPY, "Copiar"));
        if dir {
            v.push(n(M_PASTE, "Pegar").disabled(!has_clip || !paste_ok));
        }
        v.extend([
            MenuItem::sep(),
            n(M_DELETE, "Eliminar").disabled(prot),
            n(M_RENAME, "Cambiar nombre").disabled(multi || prot),
            MenuItem::sep(),
            n(M_PROPS, "Propiedades"),
        ]);
        v
    }

    fn bg_menu(&self, has_clip: bool, undo: Option<&'static str>) -> Vec<MenuItem> {
        let n = MenuItem::new;
        if self.trash() {
            return vec![
                self.sort_menu(),
                n(M_REFRESH, "Actualizar"),
                MenuItem::sep(),
                n(M_EMPTY, "Vaciar Papelera").disabled(self.rows.is_empty()),
            ];
        }
        let undo = match undo {
            Some(l) => n(M_UNDO, &format!("Deshacer {l}")),
            None => n(M_UNDO, "Deshacer").disabled(true),
        };
        vec![
            MenuItem::submenu(
                "Ver",
                vec![
                    n(M_VIEW_ICONS, "Iconos grandes").checked(self.view == View::Icons),
                    n(M_VIEW_MEDIUM, "Iconos medianos").checked(self.view == View::Medium),
                    n(M_VIEW_SMALL, "Iconos pequeños").checked(self.view == View::Small),
                    n(M_VIEW_LIST, "Lista").checked(self.view == View::List),
                    n(M_VIEW_DETAILS, "Detalles").checked(self.view == View::Details),
                ],
            ),
            self.sort_menu(),
            n(M_REFRESH, "Actualizar"),
            MenuItem::sep(),
            n(M_PASTE, "Pegar").disabled(!has_clip || !self.can_create),
            undo.shortcut("Ctrl+Z"),
            MenuItem::sep(),
            MenuItem::submenu("Nuevo", vec![n(M_NEW_FOLDER, "Carpeta"), n(M_NEW_DOC, "Documento de texto")]).disabled(!self.can_create),
            MenuItem::sep(),
            n(M_PROPS, "Propiedades"),
        ]
    }

    /// Cuadro "Propiedades" de la selección (`item`) o de la carpeta actual.
    fn props(&self, item: bool, cx: &mut Ctx) {
        let (title, text) = if item && self.marked.len() > 1 {
            let rows: Vec<&Row> = self.marked.iter().filter_map(|&i| self.rows.get(i)).collect();
            let dirs = rows.iter().filter(|r| r.dir).count();
            let total: u64 = rows.iter().filter(|r| !r.dir).map(|r| r.size).sum();
            let mut t = format!("Elementos: {}\nArchivos: {}\nCarpetas: {dirs}\n", rows.len(), rows.len() - dirs);
            t += &format!("Tamaño total de los archivos: {}", size_text(total));
            (format!("{} elementos", rows.len()), t)
        } else if let (true, Some(r)) = (item, self.sel_row()) {
            let ty = kind_of(&r.name, r.dir);
            let mut t = format!("Nombre: {}\nTipo: {ty}\n", r.name);
            if self.trash() {
                t += &format!("Ubicación original: {}\n", r.loc);
                if !r.dir {
                    t += &format!("Tamaño: {}\n", size_text(r.size));
                }
                t += &format!("Eliminado: {}", fmt_date(r.date));
            } else {
                let path = join(&self.cwd, &r.name);
                t += &format!("Ubicación: {}\n", self.cwd);
                if r.dir {
                    let n = cx.sys.files().list(&path).map_or(0, |v| v.len());
                    t += &format!("Contiene: {}\n", count_text(n));
                } else {
                    t += &format!("Tamaño: {}\n", size_text(r.size));
                }
                t += &format!("Modificado: {}", fmt_date(r.date));
            }
            (r.name.clone(), t)
        } else {
            let name = match file_name(&self.cwd) {
                "" => "Disco local (C:)",
                n => n,
            };
            let mut t = format!("Nombre: {name}\nTipo: Carpeta de archivos\n");
            if let Some(p) = parent(&self.cwd) {
                t += &format!("Ubicación: {p}\n");
            }
            t += &format!("Contiene: {}", count_text(self.rows.len()));
            if let Ok(e) = cx.sys.files().stat(&self.cwd) {
                t += &format!("\nModificado: {}", fmt_date(e.modified));
            }
            (String::from(name), t)
        };
        cx.dialog = Some(DialogRequest::Message {
            tag: TAG_PROPS,
            title: format!("Propiedades de {title}"),
            text,
            buttons: Buttons::Ok,
        });
    }

    fn head_sort(&self, k: usize) -> Option<Sort> {
        match (self.trash(), k) {
            (_, 0) => Some(Sort::Name),
            (true, 2) | (false, 1) => Some(Sort::Date),
            (false, 2) => Some(Sort::Type),
            (_, 3) => Some(Sort::Size),
            _ => None,
        }
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
        let y0 = RIB_H;
        s.fill_rect(Rect::new(0, y0, self.w, NAV_H), 0xFFFFFF);
        s.fill_rect(Rect::new(0, y0 + NAV_H - 1, self.w, 1), LINE);
        let can_back = !self.back.is_empty();
        let can_up = !self.trash() && parent(&self.cwd).is_some();
        for (i, (hit, ok)) in [(Hit::Back, can_back), (Hit::Up, can_up)].into_iter().enumerate() {
            let r = Rect::new(6 + i as i32 * 30, y0 + 5, 26, 24);
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
        let bar = Rect::new(70, y0 + 5, self.w - 76, 24);
        s.fill_rect(bar, 0xFFFFFF);
        s.frame(bar, 0xC8C8C8);
        let old = s.clip();
        s.set_clip(old.intersect(bar));
        self.crumbs(s, bar);
        s.set_clip(old);
    }

    fn panel(&mut self, s: &mut Surface, theme: &Theme) {
        let y0 = self.body_y();
        s.fill_rect(Rect::new(0, y0, PANEL_W, self.h - y0 - STATUS_H), 0xF7F7F7);
        s.fill_rect(Rect::new(PANEL_W - 1, y0, 1, self.h - y0 - STATUS_H), LINE);
        for (i, place) in PLACES.iter().enumerate() {
            let r = Rect::new(0, y0 + 8 + i as i32 * PLACE_H, PANEL_W - 1, PLACE_H);
            if same(&self.cwd, &self.places[i]) {
                s.fill_rect(r, blend(0xF7F7F7, theme.accent, 55));
                s.fill_rect(Rect::new(0, r.y + 5, 3, r.h - 10), theme.accent);
            } else if self.hover == Some(Hit::Place(i)) {
                s.fill_rect(r, 0xE9E9E9);
            }
            if self.drop_tgt == Some(Tgt::Place(i)) {
                s.blend_rect(r, theme.accent, 70);
                s.frame(r, theme.accent);
            }
            icons::draw(s, place.icon(), 14, r.y + (PLACE_H - 16) / 2, 16, 0x444444);
            clipped(s, Rect::new(38, r.y, PANEL_W - 42, r.h), place.name(), TEXT, Align::Left);
            self.hits.push((r, Hit::Place(i)));
        }
    }

    /// Cabecera de columnas de Detalles; cada título es un botón de orden.
    fn heads(&mut self, s: &mut Surface, end: i32) {
        let y0 = self.body_y();
        let cx = self.col_x();
        let heads: [&str; 4] = if self.trash() {
            ["Nombre", "Ubicación original", "Fecha de eliminación", "Tamaño"]
        } else {
            ["Nombre", "Fecha de modificación", "Tipo", "Tamaño"]
        };
        for (k, h) in heads.iter().enumerate() {
            let x1 = if k == 3 { end } else { cx[k + 1] };
            let cell = Rect::new(cx[k], y0, x1 - cx[k], HEAD_H);
            let sort = self.head_sort(k);
            if let Some(so) = sort {
                if self.hover == Some(Hit::Head(so)) {
                    s.fill_rect(cell, 0xE5F1FB);
                }
                self.hits.push((cell, Hit::Head(so)));
            }
            let mut r = Rect::new(cx[k] + 6, y0, x1 - cx[k] - 10, HEAD_H);
            if sort == Some(self.sort) {
                // Flecha ↑ (ascendente) o ↓ (descendente); a la izquierda en la columna alineada a la derecha.
                let ax = if k == 3 { r.x + 5 } else { r.right() - 5 } as f32;
                let (ay, d) = ((y0 + HEAD_H / 2) as f32, if self.desc { 1.0 } else { -1.0 });
                s.stroke(&[(ax - 3.0, ay - 1.5 * d), (ax, ay + 1.5 * d), (ax + 3.0, ay - 1.5 * d)], 1.0, TEXT);
                if k == 3 {
                    r.x += 12;
                }
                r.w -= 12;
            }
            clipped(s, r, h, GRAY, if k == 3 { Align::Right } else { Align::Left });
            if k > 0 {
                s.fill_rect(Rect::new(cx[k], y0 + 3, 1, HEAD_H - 6), LINE);
            }
        }
    }

    fn list(&mut self, s: &mut Surface, theme: &Theme) {
        let trash = self.trash();
        let y0 = self.body_y();
        let a = self.area();
        s.fill_rect(Rect::new(PANEL_W, y0, self.w - PANEL_W, self.h - y0 - STATUS_H), 0xFFFFFF);
        if self.view == View::Details {
            self.heads(s, a.right());
        }
        let cx = self.col_x();
        let in_home = !trash && parent(&self.cwd).is_some_and(|p| same(&p, files::USERS_DIR));
        let (pl, page, _) = self.dims();
        let old = s.clip();
        s.set_clip(old.intersect(a));
        for (n, row) in self.rows.iter().enumerate().skip(self.top * pl).take(page * pl) {
            let r = self.item_rect(n);
            let icon = if row.dir {
                // Dentro de la carpeta de una cuenta, las seis estándar llevan su icono.
                match Std::ALL.iter().find(|d| in_home && same(d.name(), &row.name)) {
                    Some(&d) => std_icon(d),
                    None => Icon::Folder,
                }
            } else if is_txt(&row.name) {
                Icon::FileText
            } else {
                Icon::FileBlank
            };
            let hl = blend(0xFFFFFF, theme.accent, 55);
            let on = self.is_sel(n);
            let name = files::display_name(&row.name, row.dir, self.show_ext);
            match (self.view, self.view.grid()) {
                (_, Some((_, _, ic, right))) => {
                    if on {
                        s.fill_rect(Rect::new(r.x + 2, r.y + 1, r.w - 4, r.h - 2), hl);
                    }
                    if right {
                        icons::draw(s, icon, r.x + 4, r.y + (r.h - ic) / 2, ic, 0x444444);
                        clipped(s, Rect::new(r.x + ic + 10, r.y, r.w - ic - 14, r.h), name, TEXT, Align::Left);
                    } else {
                        icons::draw(s, icon, r.x + (r.w - ic) / 2, r.y + 5, ic, 0x444444);
                        let name = dialogs::fit(name, 9, r.w - 10);
                        clipped(s, Rect::new(r.x + 3, r.y + ic + 8, r.w - 6, 18), &name, TEXT, Align::Center);
                    }
                }
                (View::List, _) => {
                    if on {
                        s.fill_rect(Rect::new(r.x, r.y, r.w - 4, r.h), hl);
                    }
                    icons::draw(s, icon, r.x + 4, r.y + 3, 16, 0x444444);
                    clipped(s, Rect::new(r.x + 26, r.y, r.w - 34, r.h), name, TEXT, Align::Left);
                }
                _ => {
                    if on {
                        s.fill_rect(r, hl);
                    }
                    let y = r.y;
                    icons::draw(s, icon, cx[0] + 6, y + 3, 16, 0x444444);
                    clipped(s, Rect::new(cx[0] + 28, y, cx[1] - cx[0] - 32, ROW_H), name, TEXT, Align::Left);
                    let (c1, c2) = if trash {
                        (row.loc.clone(), fmt_date(row.date))
                    } else {
                        (fmt_date(row.date), String::from(kind_of(&row.name, row.dir)))
                    };
                    clipped(s, Rect::new(cx[1] + 6, y, cx[2] - cx[1] - 10, ROW_H), &c1, GRAY, Align::Left);
                    clipped(s, Rect::new(cx[2] + 6, y, cx[3] - cx[2] - 10, ROW_H), &c2, GRAY, Align::Left);
                    if !row.dir {
                        let sz = Rect::new(cx[3] + 6, y, a.right() - cx[3] - 10, ROW_H);
                        clipped(s, sz, &fmt_size(row.size), GRAY, Align::Right);
                    }
                }
            }
        }
        if let Some(Tgt::Row(i)) = self.drop_tgt.filter(|_| !trash) {
            let r = self.item_rect(i);
            s.blend_rect(r, theme.accent, 60);
            s.frame(r, theme.accent);
        }
        if let Some(r) = self.mq_rect() {
            s.blend_rect(r, theme.accent, 50);
            s.frame(r, theme.accent);
        }
        s.set_clip(old);
        let m = self.metrics();
        if m.needed() {
            if self.horiz() {
                self.sb_h.draw(s, &m, theme);
            } else {
                self.sb_v.draw(s, &m, theme);
            }
        }
        if self.rows.is_empty() {
            let t = if trash { "La Papelera está vacía." } else { "Esta carpeta está vacía." };
            font::draw_in(s, Rect::new(PANEL_W, a.y + 10, a.w, 20), t, 9, false, GRAY, Align::Center);
        }
    }

    fn status(&mut self, s: &mut Surface, sys: &mut dyn Platform) {
        let y = self.h - STATUS_H;
        s.fill_rect(Rect::new(0, y, self.w, STATUS_H), 0xF7F7F7);
        s.fill_rect(Rect::new(0, y, self.w, 1), LINE);
        let mut t = count_text(self.rows.len());
        if self.marked.len() > 1 {
            t += &format!("     {} elementos seleccionados", self.marked.len());
        }
        font::draw_in(s, Rect::new(10, y, 320, STATUS_H), &t, 9, false, GRAY, Align::Left);
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
                "" if self.cwd.is_empty() => String::from(Std::Documents.name()),
                "" => String::from("Disco local (C:)"),
                n => String::from(n),
            }
        }
    }
    fn min_size(&self) -> (i32, i32) {
        (760, 400)
    }

    fn draw(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, _focused: bool) {
        self.w = s.w;
        self.h = s.h;
        if sys.files().generation() != self.gen || self.prefs_for != sys.files().explorer_prefs() {
            self.reload(sys);
        }
        self.has_clip = sys.files().clip().is_some();
        self.places = PLACES.iter().map(|p| p.path(sys.files())).collect();
        self.top = self.top.min(self.max_top());
        self.hits.clear();
        self.sync_ribbon();
        let b = s.bounds();
        s.fill_rect(b, 0xFFFFFF);
        self.list(s, theme);
        self.panel(s, theme);
        self.nav_bar(s);
        self.rib.draw(s, self.w, theme);
        self.status(s, sys);
    }

    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
        // El escritorio avisa con `Key::Other` de cada cambio de Ctrl y Mayús (también al soltar).
        if ev.key == Key::Other {
            (self.ctrl, self.shift) = (ev.ctrl, ev.shift);
            return;
        }
        if ev.ctrl {
            match ev.key {
                Key::Char('c') => self.run(Cmd::Copy, cx),
                Key::Char('x') => self.run(Cmd::Cut, cx),
                Key::Char('v') => self.run(Cmd::Paste, cx),
                Key::Char('z') => self.undo(cx),
                Key::Char('a') => self.run(Cmd::SelAll, cx),
                _ => {}
            }
            return;
        }
        let trash = self.trash();
        let (pl, page) = (self.per_line() as i32, self.page() as i32);
        // Desplazamientos de la selección según la vista: (arriba, abajo, izquierda, derecha, página).
        let (up, down, left, right, pg) = match self.view {
            View::Details => (-1, 1, 0, 0, page),
            View::List => (-1, 1, -pl, pl, pl * page),
            _ => (-pl, pl, -1, 1, pl * page),
        };
        let x = ev.shift;
        match ev.key {
            Key::Up => self.step(up, x),
            Key::Down => self.step(down, x),
            Key::Left if left != 0 => self.step(left, x),
            Key::Right if right != 0 => self.step(right, x),
            Key::PageUp => self.step(-pg, x),
            Key::PageDown => self.step(pg, x),
            Key::Home => self.go_to(0, x),
            Key::End => self.go_to(usize::MAX, x),
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
        // La cinta recibe los movimientos y las soltadas siempre, y las pulsaciones sobre ella.
        let on_rib = matches!(ev, MouseEv::Down { y, .. } if y < RIB_H);
        if on_rib || matches!(ev, MouseEv::Up { .. } | MouseEv::Move { .. } | MouseEv::Leave) {
            match self.rib.mouse(ev, self.w) {
                RibbonAct::None => {}
                RibbonAct::Redraw => cx.redraw = true,
                RibbonAct::Pick(id) => {
                    cx.redraw = true; // el botón deja de estar pulsado
                    self.rib_pick(id, cx);
                }
                RibbonAct::File => {
                    self.file_menu(cx);
                    cx.redraw = true;
                }
            }
            if on_rib {
                return;
            }
        }
        // La barra de desplazamiento, antes que los elementos.
        if !matches!(ev, MouseEv::Wheel { .. } | MouseEv::RightDown { .. }) {
            let m = self.metrics();
            let e = if self.horiz() { self.sb_h.mouse(ev, &m, cx.now_ms) } else { self.sb_v.mouse(ev, &m, cx.now_ms) };
            if let Some(p) = e.pos {
                self.top = (p.max(0) as usize).min(self.max_top());
            }
            if e.redraw {
                cx.redraw = true;
            }
            if e.captured {
                return;
            }
        }
        match ev {
            MouseEv::RightDown { x, y } => {
                let has_clip = cx.sys.files().clip().is_some();
                let (tag, items) = if let Some(i) = self.item_at(x, y) {
                    if self.is_sel(i) {
                        self.sel = Some(i);
                    } else {
                        self.set_one(i);
                    }
                    self.last_click = None;
                    let multi = self.marked.len() > 1;
                    let dir = self.rows[i].dir && !multi;
                    let paste_ok = !dir || can_create_in(cx.sys.files(), &join(&self.cwd, &self.rows[i].name));
                    (TAG_MENU_ITEM, self.item_menu(dir, has_clip, multi, paste_ok))
                } else if self.area().contains(x, y) {
                    self.clear_sel();
                    (TAG_MENU_BG, self.bg_menu(has_clip, cx.sys.files().undo_label()))
                } else {
                    return;
                };
                cx.menu = Some(MenuRequest { tag, x, y, items });
                cx.redraw = true;
            }
            MouseEv::Wheel { x, y, dz } => {
                if x >= PANEL_W && y >= self.body_y() && y < self.h - STATUS_H {
                    let step = if self.view == View::Details { WHEEL_ROWS } else { 1 };
                    let t = (self.top as i32 + dz * step).clamp(0, self.max_top() as i32) as usize;
                    if t != self.top {
                        self.top = t;
                        cx.redraw = true;
                    }
                }
            }
            MouseEv::Up { .. } => {
                if self.mq.take().is_some_and(|m| m.on) {
                    cx.redraw = true;
                }
                // Pulsó un elemento de una selección múltiple y no hubo arrastre: queda solo ese.
                if let Some(p) = self.press.take().filter(|p| p.defer && !p.dead && p.i < self.rows.len()) {
                    self.set_one(p.i);
                    self.last_click = Some((p.i, cx.now_ms));
                    cx.redraw = true;
                }
            }
            MouseEv::Leave => {
                if self.hover.take().is_some() {
                    cx.redraw = true;
                }
            }
            MouseEv::Move { x, y } => {
                let h = self.hit_at(x, y);
                if h != self.hover {
                    self.hover = h;
                    cx.redraw = true;
                }
                if let Some(p) = self.press.as_ref().filter(|p| !p.dead && (x - p.x).abs().max((y - p.y).abs()) > DRAG_PX) {
                    let paths = self.paths();
                    if p.i >= self.rows.len() || self.trash() || paths.iter().any(|q| files::is_protected(q)) {
                        if let Some(p) = &mut self.press {
                            p.dead = true;
                        }
                    } else {
                        self.press = None;
                        self.hover = None;
                        cx.drag = Some(DragRequest { paths });
                        cx.redraw = true;
                    }
                }
                let (ox, oy) = self.off();
                if let Some(m) = &mut self.mq {
                    (m.px, m.py) = (x, y);
                    m.on |= (m.ax - (x + ox)).abs().max((m.ay - (y + oy)).abs()) > 3;
                    if m.on {
                        self.mq_apply();
                        cx.redraw = true;
                    }
                }
            }
            MouseEv::Down { x, y } => {
                cx.redraw = true;
                (self.press, self.mq) = (None, None);
                if let Some(h) = self.hit_at(x, y) {
                    match h {
                        Hit::Back => self.go_back(cx),
                        Hit::Up => self.up(cx),
                        Hit::Crumb(p) => self.go(&p, true, cx),
                        Hit::Place(i) => {
                            let p = PLACES[i].path(cx.sys.files());
                            self.go(&p, true, cx);
                        }
                        Hit::Head(so) => {
                            if self.sort == so {
                                self.desc = !self.desc;
                            } else {
                                (self.sort, self.desc) = (so, false);
                            }
                            self.prefs_changed(cx);
                        }
                    }
                    return;
                }
                match self.item_at(x, y) {
                    Some(i) if self.shift => {
                        self.range_to(i);
                        self.last_click = None;
                    }
                    Some(i) if self.ctrl => {
                        self.toggle(i);
                        self.last_click = None;
                    }
                    Some(i) if self.is_sel(i) && self.marked.len() > 1 => {
                        // No deshace la selección: se decide al soltar (o se arrastra todo).
                        self.press = Some(Press { x, y, i, defer: true, dead: false });
                        self.last_click = None;
                    }
                    Some(i) => {
                        self.set_one(i);
                        if self.last_click.is_some_and(|(j, t)| j == i && cx.now_ms.saturating_sub(t) < DOUBLE_MS) {
                            self.last_click = None;
                            self.open_sel(cx);
                        } else {
                            self.last_click = Some((i, cx.now_ms));
                            self.press = Some(Press { x, y, i, defer: false, dead: false });
                        }
                    }
                    None => {
                        if self.area().contains(x, y) {
                            if !self.ctrl && !self.shift {
                                self.clear_sel();
                            }
                            let (ox, oy) = self.off();
                            self.mq = Some(Mq { ax: x + ox, ay: y + oy, px: x, py: y, base: self.marked.clone(), add: self.ctrl, on: false, last: cx.now_ms });
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
        let m = self.metrics();
        let p = if self.horiz() { self.sb_h.tick(&m, cx.now_ms) } else { self.sb_v.tick(&m, cx.now_ms) };
        if let Some(p) = p {
            self.top = (p.max(0) as usize).min(self.max_top());
            cx.redraw = true;
        }
        // Recuadro de selección contra un borde de la zona: el contenido avanza solo.
        if let Some(m) = self.mq.as_ref().filter(|m| m.on && cx.now_ms.saturating_sub(m.last) >= AUTOSCROLL_MS) {
            let a = self.area();
            let (lo, hi, v) = if self.horiz() { (a.x, a.right(), m.px) } else { (a.y, a.bottom(), m.py) };
            let top = if v < lo { self.top.saturating_sub(1) } else if v >= hi { (self.top + 1).min(self.max_top()) } else { self.top };
            if top != self.top {
                self.top = top;
                if let Some(m) = &mut self.mq {
                    m.last = cx.now_ms;
                }
                self.mq_apply();
                cx.redraw = true;
            }
        }
    }

    /// Arrastre con el botón derecho: selecciona como `RightDown` (conserva la selección múltiple si
    /// el elemento ya estaba en ella) y da las rutas; nada en zona vacía, en la Papelera o con protegidas.
    fn drag_source(&mut self, x: i32, y: i32, cx: &mut Ctx) -> Option<Vec<String>> {
        let i = self.item_at(x, y).filter(|&i| i < self.rows.len())?;
        if self.is_sel(i) {
            self.sel = Some(i);
        } else {
            self.set_one(i);
        }
        self.last_click = None;
        cx.redraw = true;
        let paths = self.paths();
        (!self.trash() && !paths.iter().any(|q| files::is_protected(q))).then_some(paths)
    }

    fn drag_over(&mut self, pos: Option<(i32, i32)>, paths: &[String], cx: &mut Ctx) -> bool {
        let t = pos.and_then(|(x, y)| self.target(x, y, paths, cx.sys.files()));
        let tgt = t.as_ref().map(|(_, g)| *g).filter(|g| *g != Tgt::Area);
        if tgt != self.drop_tgt {
            self.drop_tgt = tgt;
            cx.redraw = true;
        }
        t.is_some()
    }

    fn drop_files(&mut self, x: i32, y: i32, paths: &[String], copy: bool, cx: &mut Ctx) {
        let t = self.target(x, y, paths, cx.sys.files());
        if self.drop_tgt.take().is_some() {
            cx.redraw = true;
        }
        let Some((dest, _)) = t else { return };
        let paths = paths.to_vec();
        if dest == files::TRASH {
            self.act(Act::Trash(paths), cx);
        } else {
            self.act(Act::Transfer { paths, dir: dest, mv: !copy }, cx);
        }
    }

    fn dialog_result(&mut self, result: DialogResult, cx: &mut Ctx) {
        match result {
            DialogResult::Text { tag: TAG_NEW, value: Some(v) } => {
                if valid_name(&v) {
                    self.act(Act::Create { path: join(&self.cwd, &v), dir: true }, cx);
                } else {
                    self.done(Err(FsError::BadName), cx);
                }
            }
            DialogResult::Text { tag: TAG_NEWDOC, value: Some(mut v) } => {
                if !v.rsplit_once('.').is_some_and(|(a, b)| !a.is_empty() && !b.is_empty()) {
                    v.push_str(".txt");
                }
                if valid_name(&v) {
                    self.act(Act::Create { path: join(&self.cwd, &v), dir: false }, cx);
                } else {
                    self.done(Err(FsError::BadName), cx);
                }
            }
            DialogResult::Text { tag: TAG_RENAME, value: Some(v) } => {
                let from = core::mem::take(&mut self.rename_from);
                let ext = core::mem::take(&mut self.rename_ext);
                let empty = v.trim().is_empty();
                // Con las extensiones ocultas se escribe solo el nombre; la extensión se conserva.
                let v = v + &ext;
                if !empty && file_name(&from) == v {
                    self.done(Ok(()), cx);
                } else if empty || !valid_name(&v) {
                    self.done(Err(FsError::BadName), cx);
                } else {
                    let to = join(&parent(&from).unwrap_or_default(), &v);
                    self.act(Act::Rename { from, to }, cx);
                }
            }
            DialogResult::Path { tag, path: Some(dir) } if tag == TAG_MOVETO || tag == TAG_COPYTO => {
                let paths = core::mem::take(&mut self.pending);
                let mv = tag == TAG_MOVETO;
                self.act(Act::Transfer { paths, dir, mv }, cx);
            }
            DialogResult::Path { path: None, .. } => self.pending.clear(),
            DialogResult::Answer { tag: TAG_DEL, answer } => {
                // Sí: se elimina (si toca Sistema, `act` pregunta además por eso).
                if let (Answer::Yes, Some(a)) = (answer, self.pending_sys.take()) {
                    self.act(a, cx);
                }
            }
            DialogResult::Answer { tag: TAG_SYS, answer } => {
                if let (Answer::Yes, Some(a)) = (answer, self.pending_sys.take()) {
                    self.apply(a, cx);
                }
                self.pending_sys = None;
            }
            DialogResult::Answer { tag: TAG_EMPTY, answer: Answer::Yes } => {
                let r = cx.sys.files().empty_trash();
                self.done(r, cx);
            }
            _ => {}
        }
    }

    fn menu_result(&mut self, tag: u32, id: u32, cx: &mut Ctx) {
        let item = tag == TAG_MENU_ITEM;
        let path = self.single().map(|r| join(&self.cwd, &r.name));
        let dir = self.single().is_some_and(|r| r.dir);
        match id {
            M_OPEN => self.open_sel(cx),
            M_OPEN_NEW if dir => cx.open_new = path,
            M_NOTEPAD if !dir => cx.open = path,
            M_CUT => self.run(Cmd::Cut, cx),
            M_COPY => self.run(Cmd::Copy, cx),
            M_PASTE => {
                self.has_clip = cx.sys.files().clip().is_some();
                match path {
                    Some(p) if item && dir && self.has_clip && can_create_in(cx.sys.files(), &p) => self.paste_into(&p, cx),
                    _ if self.enabled(Cmd::Paste) => {
                        let dir = self.cwd.clone();
                        self.paste_into(&dir, cx);
                    }
                    _ => {}
                }
            }
            M_DELETE => self.run(Cmd::Delete, cx),
            M_RENAME => self.run(Cmd::Rename, cx),
            M_PROPS => self.props(item, cx),
            M_RESTORE => self.run(Cmd::Restore, cx),
            M_EMPTY => self.run(Cmd::EmptyTrash, cx),
            M_REFRESH => self.reload(cx.sys),
            M_UNDO => self.undo(cx),
            M_NEW_FOLDER => self.run(Cmd::NewFolder, cx),
            M_NEW_DOC if self.can_create => self.new_doc(cx),
            M_FILE_NEWWIN => cx.open_new = Some(self.cwd.clone()),
            M_FILE_TERM if !self.trash() => cx.open_terminal = Some(self.cwd.clone()),
            M_FILE_CLOSE => cx.close = true,
            M_VIEW_DETAILS | M_VIEW_LIST | M_VIEW_ICONS | M_VIEW_MEDIUM | M_VIEW_SMALL => {
                self.view = match id {
                    M_VIEW_DETAILS => View::Details,
                    M_VIEW_LIST => View::List,
                    M_VIEW_MEDIUM => View::Medium,
                    M_VIEW_SMALL => View::Small,
                    _ => View::Icons,
                };
                self.prefs_changed(cx);
            }
            M_SORT_NAME | M_SORT_DATE | M_SORT_TYPE | M_SORT_SIZE => {
                self.sort = match id {
                    M_SORT_NAME => Sort::Name,
                    M_SORT_DATE => Sort::Date,
                    M_SORT_TYPE => Sort::Type,
                    _ => Sort::Size,
                };
                self.prefs_changed(cx);
            }
            M_ASC | M_DESC => {
                self.desc = id == M_DESC;
                self.prefs_changed(cx);
            }
            _ => {}
        }
        cx.redraw = true;
    }

    fn open_path(&mut self, path: &str, cx: &mut Ctx) {
        self.go(path, true, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::TestPlatform;


    /// Carpeta estándar de la cuenta de las pruebas (la de la unidad en memoria).
    fn dd(d: Std) -> String {
        join(&join(files::USERS_DIR, files::DEFAULT_USER), d.name())
    }
    /// Preferencias de esa cuenta (`explorer_prefs`).
    fn prefs() -> String {
        join(&join(&join(files::USERS_DIR, files::DEFAULT_USER), ".config"), "explorador.txt")
    }
    fn pth(d: Std, name: &str) -> String {
        join(&dd(d), name)
    }

    struct T {
        e: Explorer,
        sys: TestPlatform,
        th: Theme,
        now: u64,
        menu: Option<MenuRequest>,
        open_new: Option<String>,
        term: Option<String>,
        closed: bool,
        drag: Option<DragRequest>,
        redraw: bool,
        /// Contesta Sí solo a "¿enviar a la Papelera?" (las pruebas antiguas eliminan sin preguntar).
        auto_del: bool,
    }

    impl T {
        fn new() -> T {
            let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
            let now = sys.now();
            sys.files.write(pth(Std::Documents, "nota.txt").as_str(), b"hola", now).unwrap();
            sys.files.mkdir(pth(Std::Documents, "Sub").as_str(), now).unwrap();
            T::with(sys)
        }
        fn with(sys: TestPlatform) -> T {
            let mut t = T { e: make(), sys, th: Theme::default(), now: 1000, menu: None, open_new: None, term: None, closed: false, drag: None, redraw: false, auto_del: true };
            t.draw();
            t
        }
        fn draw(&mut self) -> Surface {
            let mut s = Surface::new(1000, 520, 0);
            self.e.draw(&mut s, &self.th, &mut self.sys, true);
            s
        }
        fn cx<R>(&mut self, f: impl FnOnce(&mut Explorer, &mut Ctx) -> R) -> (R, Option<DialogRequest>, Option<String>) {
            let mut cx = Ctx::new(&mut self.sys, &mut self.th, self.now);
            let r = f(&mut self.e, &mut cx);
            self.menu = cx.menu.take();
            self.open_new = cx.open_new.take();
            self.term = cx.open_terminal.take();
            self.closed |= cx.close;
            self.drag = cx.drag.take().or(self.drag.take());
            self.redraw = cx.redraw;
            let (d, o) = (cx.dialog.take(), cx.open.take());
            if self.auto_del && matches!(d, Some(DialogRequest::Message { tag: TAG_DEL, .. })) {
                let mut cx = Ctx::new(&mut self.sys, &mut self.th, self.now);
                self.e.dialog_result(DialogResult::Answer { tag: TAG_DEL, answer: Answer::Yes }, &mut cx);
                return (r, cx.dialog.take(), o);
            }
            (r, d, o)
        }
        fn key(&mut self, key: Key, ctrl: bool) -> (Option<DialogRequest>, Option<String>) {
            let ev = KeyEvent { key, ch: None, down: true, shift: false, ctrl, alt: false };
            let (_, d, o) = self.cx(|e, cx| e.key(&ev, cx));
            self.draw();
            (d, o)
        }
        fn click(&mut self, x: i32, y: i32) -> (Option<DialogRequest>, Option<String>) {
            self.now += 100;
            let (_, d, o) = self.cx(|e, cx| {
                e.mouse(MouseEv::Down { x, y }, cx);
                e.mouse(MouseEv::Up { x, y }, cx);
            });
            self.draw();
            (d, o)
        }
        /// Solo pulsa (sin soltar).
        fn press(&mut self, x: i32, y: i32) {
            self.now += 100;
            self.cx(|e, cx| e.mouse(MouseEv::Down { x, y }, cx));
            self.draw();
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
            self.sys.files.list(dd(Std::Documents).as_str()).unwrap().into_iter().map(|e| e.name).collect()
        }
        /// Pulsa y suelta el botón de la cinta `id` (en la pestaña activa).
        fn rib(&mut self, id: u32) -> (Option<DialogRequest>, Option<String>) {
            let r = self.e.rib.item_rect(id, self.e.w).expect("botón en la pestaña activa");
            let (x, y) = (r.x + r.w / 2, r.y + r.h / 2);
            self.now += 100;
            let (_, d, o) = self.cx(|e, cx| {
                e.mouse(MouseEv::Move { x, y }, cx);
                e.mouse(MouseEv::Down { x, y }, cx);
                e.mouse(MouseEv::Up { x, y }, cx);
            });
            self.draw();
            (d, o)
        }
        fn tab(&mut self, i: usize) {
            self.e.rib.set_active(i);
            self.draw();
        }
        fn row_y(i: i32) -> i32 {
            RIB_H + NAV_H + HEAD_H + i * ROW_H + 5
        }
    }

    fn text(v: &str) -> DialogResult {
        DialogResult::Text { tag: TAG_NEW, value: Some(String::from(v)) }
    }

    #[test]
    fn navega_y_vuelve_atras() {
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
        let (x, y) = (16, RIB_H + 17);
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
        let mut t = T::new();
        // Orden: carpeta Sub, archivo nota.txt.
        t.click(300, T::row_y(1));
        let (_, o) = t.click(300, T::row_y(1));
        assert_eq!(o.as_deref(), Some(pth(Std::Documents, "nota.txt").as_str()));
        t.click(300, T::row_y(0));
        t.now += 5000; // demasiado lento: no es doble clic
        t.click(300, T::row_y(0));
        assert_eq!(t.e.title(), "Documentos");
        t.click(300, T::row_y(0));
        assert_eq!(t.e.title(), "Sub");
        // Enter sobre un archivo.
        t.open(dd(Std::Documents).as_str());
        t.key(Key::Down, false);
        t.key(Key::Down, false);
        let (_, o) = t.key(Key::Enter, false);
        assert_eq!(o.as_deref(), Some(pth(Std::Documents, "nota.txt").as_str()));
    }

    #[test]
    fn nueva_carpeta_y_renombrar() {
        let mut t = T::new();
        let (d, _) = t.rib(R_NEWF);
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
        let mut t = T::new();
        selecciona(&mut t, "nota.txt");
        t.key(Key::Char('c'), true);
        t.key(Key::Char('v'), true);
        t.key(Key::Char('v'), true);
        let n = t.names();
        assert!(n.contains(&String::from("nota (2).txt")) && n.contains(&String::from("nota (3).txt")), "{n:?}");
        // A otra carpeta, con el botón Pegar.
        t.open(dd(Std::Pictures).as_str());
        t.rib(R_PASTE);
        assert!(t.sys.files.exists(pth(Std::Pictures, "nota.txt").as_str()));
        assert!(t.sys.files.exists(pth(Std::Documents, "nota.txt").as_str()));
    }

    #[test]
    fn cortar_y_pegar() {
        let mut t = T::new();
        selecciona(&mut t, "nota.txt");
        t.key(Key::Char('x'), true);
        t.key(Key::Char('v'), true); // misma carpeta: nada
        t.key(Key::Char('x'), true);
        assert!(t.sys.files.exists(pth(Std::Documents, "nota.txt").as_str()));
        t.open(dd(Std::Desktop).as_str());
        t.key(Key::Char('v'), true);
        assert!(t.sys.files.exists(pth(Std::Desktop, "nota.txt").as_str()));
        assert!(!t.sys.files.exists(pth(Std::Documents, "nota.txt").as_str()));
        // El portapapeles quedó vacío: Pegar está desactivado.
        t.key(Key::Char('v'), true);
        assert_eq!(t.sys.files.list(dd(Std::Desktop).as_str()).unwrap().len(), 1);
    }

    #[test]
    fn eliminar_restaurar_y_vaciar() {
        let mut t = T::new();
        selecciona(&mut t, "nota.txt");
        t.key(Key::Delete, false);
        assert!(!t.sys.files.exists(pth(Std::Documents, "nota.txt").as_str()));
        assert_eq!(t.sys.files.trash_list().unwrap().len(), 1);
        t.open(files::TRASH);
        assert_eq!(t.e.title(), "Papelera");
        // En la Papelera, Supr y doble clic no hacen nada.
        t.key(Key::Down, false);
        t.key(Key::Delete, false);
        let (_, o) = t.key(Key::Enter, false);
        assert!(o.is_none());
        assert_eq!(t.sys.files.trash_list().unwrap().len(), 1);
        t.rib(R_RESTORE);
        assert!(t.sys.files.exists(pth(Std::Documents, "nota.txt").as_str()));
        assert!(t.sys.files.trash_list().unwrap().is_empty());
        // Vaciar pide confirmación y solo actúa con Sí.
        t.open(dd(Std::Documents).as_str());
        selecciona(&mut t, "nota.txt");
        t.key(Key::Delete, false);
        t.open(files::TRASH);
        let (d, _) = t.rib(R_EMPTY);
        assert!(matches!(d, Some(DialogRequest::Message { tag: TAG_EMPTY, buttons: Buttons::YesNo, .. })));
        t.answer(DialogResult::Answer { tag: TAG_EMPTY, answer: Answer::No });
        assert_eq!(t.sys.files.trash_list().unwrap().len(), 1);
        t.answer(DialogResult::Answer { tag: TAG_EMPTY, answer: Answer::Yes });
        assert!(t.sys.files.trash_list().unwrap().is_empty());
    }

    #[test]
    fn teclado_y_recarga() {
        let mut t = T::new();
        t.key(Key::End, false);
        t.key(Key::Home, false);
        t.key(Key::PageDown, false);
        t.key(Key::Up, false);
        t.key(Key::Down, false);
        // Un cambio externo se ve en tick sin perder la selección por nombre.
        t.key(Key::End, false);
        let now = t.now_dt();
        t.sys.files.mkdir(pth(Std::Documents, "Aaa").as_str(), now).unwrap();
        t.cx(|e, cx| e.tick(cx));
        let s = t.draw();
        assert!(s.px.iter().any(|&p| p != 0));
        t.key(Key::Enter, false); // la selección sigue en nota.txt (archivo)
        let (_, o) = t.key(Key::Enter, false);
        assert_eq!(o.as_deref(), Some(pth(Std::Documents, "nota.txt").as_str()));
        // Una carpeta borrada desde fuera: se sube a una que exista.
        t.open(pth(Std::Documents, "Sub").as_str());
        t.sys.files.delete(pth(Std::Documents, "Sub").as_str()).unwrap();
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
        let mut t = T::new();
        t.click(30, RIB_H + NAV_H + 8 + 4); // Escritorio
        assert_eq!(t.e.title(), "Escritorio");
        t.click(30, RIB_H + NAV_H + 8 + PLACE_H * 6 + 4); // Disco local
        assert_eq!(t.e.title(), "Disco local (C:)");
        t.click(30, RIB_H + NAV_H + 8 + PLACE_H * 7 + 4); // Papelera
        assert_eq!(t.e.title(), "Papelera");
        // Muchas filas: aparece la barra y se puede desplazar.
        let now = t.now_dt();
        for i in 0..40 {
            t.sys.files.mkdir(&pth(Std::Documents, &format!("d{i:02}")), now).unwrap();
        }
        t.open(dd(Std::Documents).as_str());
        t.key(Key::End, false);
        t.draw();
        let w = 760;
        t.click(w - 5, 400);
        t.cx(|e, cx| e.mouse(MouseEv::Up { x: w - 5, y: 400 }, cx));
        assert_eq!(fmt_size(1536), "2 KB");
        assert_eq!(fmt_size(5 << 20), "5.0 MB");
    }

    // ---- menú contextual, orden, vistas, deshacer, propiedades, rueda, preferencias ----

    fn texts(v: &[MenuItem]) -> Vec<String> {
        v.iter()
            .map(|m| {
                if m.separator {
                    String::from("-")
                } else if !m.sub.is_empty() {
                    format!("{} >", m.text)
                } else {
                    m.text.clone()
                }
            })
            .collect()
    }

    fn dt(day: u8) -> DateTime {
        DateTime { year: 2026, month: 11, day, hour: 9, minute: 0, second: 0 }
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| String::from(*s)).collect()
    }

    impl T {
        fn right(&mut self, x: i32, y: i32) -> Option<MenuRequest> {
            self.cx(|e, cx| e.mouse(MouseEv::RightDown { x, y }, cx));
            self.draw();
            self.menu.take()
        }
        /// Elige `id` del menú de `tag` y devuelve el diálogo que pida.
        fn pick(&mut self, tag: u32, id: u32) -> (Option<DialogRequest>, Option<String>) {
            let (_, d, o) = self.cx(|e, cx| e.menu_result(tag, id, cx));
            self.draw();
            (d, o)
        }
        fn right_row(&mut self, name: &str) -> MenuRequest {
            let i = self.rows().iter().position(|n| n == name).unwrap();
            let r = self.e.item_rect(i);
            self.right(r.x + 10, r.y + 5).expect("menú")
        }
        fn rows(&self) -> Vec<String> {
            self.e.rows.iter().map(|r| r.name.clone()).collect()
        }
        fn sel_name(&self) -> Option<String> {
            self.e.sel_row().map(|r| r.name.clone())
        }
        fn view(&mut self, id: u32) {
            self.pick(TAG_MENU_BG, id);
        }
        fn many(&mut self, n: usize) {
            let now = self.now_dt();
            for i in 0..n {
                self.sys.files.write(&pth(Std::Documents, &format!("f{i:02}")), b"x", now).unwrap();
            }
            self.draw();
        }
        fn wheel(&mut self, dz: i32) {
            self.cx(|e, cx| e.mouse(MouseEv::Wheel { x: 300, y: 200, dz }, cx));
            self.draw();
        }
        fn click_item(&mut self, i: usize) {
            let r = self.e.item_rect(i);
            self.click(r.x + 10, r.y + 5);
        }
    }

    const EMPTY: (i32, i32) = (300, 440);

    #[test]
    fn menu_de_archivo_y_carpeta() {
        let mut t = T::new();
        let m = t.right_row("nota.txt");
        assert_eq!((m.tag, m.x, m.y), (TAG_MENU_ITEM, t.e.item_rect(1).x + 10, t.e.item_rect(1).y + 5));
        assert_eq!(
            texts(&m.items),
            ["Abrir", "Abrir con >", "-", "Cortar", "Copiar", "-", "Eliminar", "Cambiar nombre", "-", "Propiedades"]
        );
        assert!(m.items[0].bold && !m.items[1].bold);
        assert_eq!(texts(&m.items[1].sub), ["Bloc de notas"]);
        assert_eq!(t.sel_name().as_deref(), Some("nota.txt"));
        // Carpeta: Pegar desactivado sin portapapeles y activo con él.
        let m = t.right_row("Sub");
        assert_eq!(
            texts(&m.items),
            ["Abrir", "Abrir en ventana nueva", "-", "Cortar", "Copiar", "Pegar", "-", "Eliminar", "Cambiar nombre", "-", "Propiedades"]
        );
        assert!(m.items.iter().find(|i| i.id == M_PASTE).unwrap().disabled);
        t.sys.files.set_clip(pth(Std::Documents, "nota.txt").as_str(), false);
        let m = t.right_row("Sub");
        assert!(!m.items.iter().find(|i| i.id == M_PASTE).unwrap().disabled);
        // Fuera de la lista (cabecera, panel, barra de órdenes) no hay menú.
        assert!(t.right(300, RIB_H + NAV_H + 5).is_none());
        assert!(t.right(30, 200).is_none());
        assert!(t.right(300, 10).is_none());
    }

    #[test]
    fn menu_de_zona_vacia() {
        let mut t = T::new();
        t.click_item(0);
        let m = t.right(EMPTY.0, EMPTY.1).expect("menú");
        assert_eq!(t.sel_name(), None);
        assert_eq!((m.tag, m.x, m.y), (TAG_MENU_BG, EMPTY.0, EMPTY.1));
        assert_eq!(
            texts(&m.items),
            ["Ver >", "Ordenar por >", "Actualizar", "-", "Pegar", "Deshacer", "-", "Nuevo >", "-", "Propiedades"]
        );
        assert_eq!(texts(&m.items[0].sub), ["Iconos grandes", "Iconos medianos", "Iconos pequeños", "Lista", "Detalles"]);
        assert_eq!(
            texts(&m.items[1].sub),
            ["Nombre", "Fecha de modificación", "Tipo", "Tamaño", "-", "Ascendente", "Descendente"]
        );
        assert_eq!(texts(&m.items[8 - 1].sub), ["Carpeta", "Documento de texto"]);
        let ch = |v: &[MenuItem]| v.iter().filter(|i| i.checked).map(|i| i.text.clone()).collect::<Vec<_>>();
        assert_eq!(ch(&m.items[0].sub), ["Detalles"]);
        assert_eq!(ch(&m.items[1].sub), ["Nombre", "Ascendente"]);
        let (pegar, deshacer) = (&m.items[4], &m.items[5]);
        assert!(pegar.disabled && deshacer.disabled && deshacer.shortcut == "Ctrl+Z");
        // Con algo que deshacer y en el portapapeles.
        t.sys.files.set_clip(pth(Std::Documents, "nota.txt").as_str(), true);
        t.click_item(1);
        t.key(Key::Delete, false);
        let m = t.right(EMPTY.0, EMPTY.1).unwrap();
        assert!(!m.items[4].disabled);
        assert_eq!((m.items[5].text.as_str(), m.items[5].disabled), ("Deshacer Eliminar", false));
    }

    #[test]
    fn menu_en_la_papelera() {
        let mut t = T::new();
        t.open(files::TRASH);
        let m = t.right(EMPTY.0, EMPTY.1).unwrap();
        assert_eq!(texts(&m.items), ["Ordenar por >", "Actualizar", "-", "Vaciar Papelera"]);
        assert!(m.items[3].disabled);
        t.open(dd(Std::Documents).as_str());
        t.click_item(1);
        t.key(Key::Delete, false);
        t.open(files::TRASH);
        let m = t.right_row("nota.txt");
        assert_eq!(texts(&m.items), ["Restaurar", "-", "Propiedades"]);
        let m = t.right(EMPTY.0, EMPTY.1).unwrap();
        assert!(!m.items[3].disabled);
        // Propiedades de un elemento de la Papelera.
        t.right_row("nota.txt");
        let (d, _) = t.pick(TAG_MENU_ITEM, M_PROPS);
        let Some(DialogRequest::Message { title, text, buttons, .. }) = d else { panic!("sin diálogo") };
        assert_eq!((title.as_str(), buttons), ("Propiedades de nota.txt", Buttons::Ok));
        assert!(text.contains(&format!("Ubicación original: {}", dd(Std::Documents))) && text.contains("Eliminado: 15/11/2026"), "{text}");
        // Restaurar desde el menú.
        t.right_row("nota.txt");
        t.pick(TAG_MENU_ITEM, M_RESTORE);
        assert!(t.sys.files.exists(pth(Std::Documents, "nota.txt").as_str()));
        // Vaciar desde el menú pide confirmación.
        t.open(dd(Std::Documents).as_str());
        t.click_item(1);
        t.key(Key::Delete, false);
        t.open(files::TRASH);
        t.right(EMPTY.0, EMPTY.1);
        let (d, _) = t.pick(TAG_MENU_BG, M_EMPTY);
        assert!(matches!(d, Some(DialogRequest::Message { tag: TAG_EMPTY, .. })));
    }

    #[test]
    fn acciones_del_menu_de_elemento() {
        let mut t = T::new();
        t.right_row("nota.txt");
        let (_, o) = t.pick(TAG_MENU_ITEM, M_OPEN);
        assert_eq!(o.as_deref(), Some(pth(Std::Documents, "nota.txt").as_str()));
        let (_, o) = t.pick(TAG_MENU_ITEM, M_NOTEPAD);
        assert_eq!(o.as_deref(), Some(pth(Std::Documents, "nota.txt").as_str()));
        // Copiar y pegar dentro de la carpeta del menú; cortar y pegar con el menú de zona vacía.
        t.right_row("nota.txt");
        t.pick(TAG_MENU_ITEM, M_COPY);
        assert_eq!(t.sys.files.clip(), Some((pth(Std::Documents, "nota.txt").as_str(), false)));
        t.right_row("Sub");
        t.pick(TAG_MENU_ITEM, M_PASTE);
        assert!(t.sys.files.exists(pth(Std::Documents, "Sub\\nota.txt").as_str()));
        assert_eq!(t.sel_name().as_deref(), Some("Sub"));
        t.right_row("nota.txt");
        t.pick(TAG_MENU_ITEM, M_CUT);
        assert_eq!(t.sys.files.clip(), Some((pth(Std::Documents, "nota.txt").as_str(), true)));
        t.open(dd(Std::Desktop).as_str());
        t.right(EMPTY.0, EMPTY.1);
        t.pick(TAG_MENU_BG, M_PASTE);
        assert!(t.sys.files.exists(pth(Std::Desktop, "nota.txt").as_str()) && !t.sys.files.exists(pth(Std::Documents, "nota.txt").as_str()));
        assert_eq!(t.sel_name().as_deref(), Some("nota.txt"));
        // Ventana nueva, cambiar nombre y eliminar.
        t.open(dd(Std::Documents).as_str());
        t.right_row("Sub");
        t.pick(TAG_MENU_ITEM, M_OPEN_NEW);
        assert_eq!(t.open_new.as_deref(), Some(pth(Std::Documents, "Sub").as_str()));
        let (d, _) = t.pick(TAG_MENU_ITEM, M_RENAME);
        assert!(matches!(d, Some(DialogRequest::Input { tag: TAG_RENAME, .. })));
        t.answer(DialogResult::Text { tag: TAG_RENAME, value: Some(String::from("Otra")) });
        assert!(t.sys.files.exists(pth(Std::Documents, "Otra").as_str()));
        assert_eq!(t.sys.files.undo_label(), Some("Cambiar nombre"));
        t.right_row("Otra");
        t.pick(TAG_MENU_ITEM, M_DELETE);
        assert!(!t.sys.files.exists(pth(Std::Documents, "Otra").as_str()));
        // Actualizar recoge cambios externos sin esperar al tick.
        let now = t.now_dt();
        t.sys.files.mkdir(pth(Std::Documents, "Externa").as_str(), now).unwrap();
        t.right(EMPTY.0, EMPTY.1);
        t.pick(TAG_MENU_BG, M_REFRESH);
        assert!(t.rows().contains(&String::from("Externa")));
    }

    fn setup_orden(t: &mut T) {
        let f = &mut t.sys.files;
        f.write(pth(Std::Documents, "b.bin").as_str(), &[0; 100], dt(2)).unwrap();
        f.write(pth(Std::Documents, "A.txt").as_str(), &[0; 50], dt(3)).unwrap();
        f.write(pth(Std::Documents, "c").as_str(), &[0; 10], dt(1)).unwrap();
        // nota.txt (4 bytes) y Sub ya existen, creados el 15/11.
        f.mkdir(pth(Std::Documents, "Zeta").as_str(), dt(9)).unwrap();
        t.draw();
    }

    #[test]
    fn ordenar_por_cada_criterio() {
        let mut t = T::new();
        setup_orden(&mut t);
        // Nombre (inicial): carpetas primero.
        assert_eq!(t.rows(), strs(&["Sub", "Zeta", "A.txt", "b.bin", "c", "nota.txt"]));
        t.pick(TAG_MENU_BG, M_DESC);
        assert_eq!(t.rows(), strs(&["Zeta", "Sub", "nota.txt", "c", "b.bin", "A.txt"]));
        t.pick(TAG_MENU_BG, M_SORT_DATE);
        assert_eq!(t.rows(), strs(&["Sub", "Zeta", "nota.txt", "A.txt", "b.bin", "c"]));
        t.pick(TAG_MENU_BG, M_ASC);
        assert_eq!(t.rows(), strs(&["Zeta", "Sub", "c", "b.bin", "A.txt", "nota.txt"]));
        t.pick(TAG_MENU_BG, M_SORT_SIZE);
        assert_eq!(t.rows(), strs(&["Sub", "Zeta", "nota.txt", "c", "A.txt", "b.bin"]));
        t.pick(TAG_MENU_BG, M_DESC);
        assert_eq!(t.rows(), strs(&["Sub", "Zeta", "b.bin", "A.txt", "c", "nota.txt"]));
        // Tipo: "Archivo" antes que "Documento de texto"; dentro, por nombre.
        t.pick(TAG_MENU_BG, M_SORT_TYPE);
        t.pick(TAG_MENU_BG, M_ASC);
        assert_eq!(t.rows(), strs(&["Sub", "Zeta", "b.bin", "c", "A.txt", "nota.txt"]));
        t.pick(TAG_MENU_BG, M_DESC);
        assert_eq!(t.rows(), strs(&["Sub", "Zeta", "A.txt", "nota.txt", "b.bin", "c"]));
        // Las marcas del menú siguen al estado.
        let m = t.right(EMPTY.0, EMPTY.1).unwrap();
        let sub = &m.items[1].sub;
        assert!(sub[2].checked && !sub[0].checked && sub[6].checked && !sub[5].checked);
    }

    #[test]
    fn ordenar_con_la_cabecera() {
        let mut t = T::new();
        setup_orden(&mut t);
        let y = RIB_H + NAV_H + 5;
        // Tamaño (columna 3): ascendente; segundo clic, descendente.
        let x = t.e.col_x()[3] + 20;
        t.click(x, y);
        assert_eq!((t.e.sort, t.e.desc), (Sort::Size, false));
        assert_eq!(t.rows(), strs(&["Sub", "Zeta", "nota.txt", "c", "A.txt", "b.bin"]));
        t.click(x, y);
        assert_eq!((t.e.sort, t.e.desc), (Sort::Size, true));
        assert_eq!(t.rows()[2], "b.bin");
        // Cambiar de columna vuelve a ascendente.
        let x = t.e.col_x()[1] + 20;
        t.click(x, y);
        assert_eq!((t.e.sort, t.e.desc), (Sort::Date, false));
        let s = t.draw();
        assert!(s.px.iter().any(|&p| p != 0));
        // En la Papelera la columna de ubicación no ordena.
        t.open(files::TRASH);
        let x = t.e.col_x()[1] + 20;
        t.click(x, y);
        assert_eq!(t.e.sort, Sort::Date);
    }

    #[test]
    fn vista_iconos_seleccion_y_teclado() {
        let mut t = T::new();
        t.many(60);
        t.view(M_VIEW_ICONS);
        assert_eq!(t.e.view, View::Icons);
        assert_eq!((t.e.per_line(), t.e.page(), t.e.lines()), (10, 4, 7));
        // Ratón: la celda 8 está en la fila 1, columna 1.
        t.click_item(8);
        assert_eq!(t.e.sel, Some(8));
        let r8 = t.e.item_rect(8);
        assert_eq!(t.e.item_at(r8.x + 40, r8.y + 70), Some(8));
        // Zona vacía a la derecha de la última celda de la rejilla.
        t.click(PANEL_W + 10 * 84 + 2, 220);
        assert_eq!(t.e.sel, None);
        t.click_item(8);
        t.key(Key::Right, false);
        assert_eq!(t.e.sel, Some(9));
        t.key(Key::Down, false);
        assert_eq!(t.e.sel, Some(19));
        t.key(Key::Left, false);
        t.key(Key::Up, false);
        assert_eq!(t.e.sel, Some(8));
        // Seguir bajando desplaza la rejilla para mantener visible la selección.
        for _ in 0..5 {
            t.key(Key::Down, false);
        }
        assert_eq!(t.e.sel, Some(58));
        assert_eq!(t.e.top, 2);
        let r = t.e.item_rect(58);
        assert_eq!(t.e.area().intersect(r), r);
        t.key(Key::End, false);
        assert_eq!((t.e.sel, t.e.top), (Some(61), 3));
        t.key(Key::Home, false);
        assert_eq!((t.e.sel, t.e.top), (Some(0), 0));
        t.key(Key::PageDown, false);
        assert_eq!(t.e.sel, Some(40));
        // Doble clic: abre la carpeta.
        t.key(Key::Home, false);
        t.click_item(0);
        t.click_item(0);
        assert_eq!(t.e.title(), "Sub");
        t.open(dd(Std::Documents).as_str());
        // Clic derecho: selecciona y pide el menú de elemento.
        let r = t.e.item_rect(3);
        let m = t.right(r.x + 5, r.y + 5).unwrap();
        assert_eq!((t.e.sel, m.tag), (Some(3), TAG_MENU_ITEM));
        // Con la rueda se desplaza una fila de la rejilla por muesca.
        t.wheel(1);
        assert_eq!(t.e.top, 1);
        t.wheel(-5);
        assert_eq!(t.e.top, 0);
        t.wheel(50);
        assert_eq!(t.e.top, 3);
        let s = t.draw();
        assert!(s.px.iter().any(|&p| p != 0));
    }

    #[test]
    fn vista_lista_seleccion_y_teclado() {
        let mut t = T::new();
        t.many(60);
        t.view(M_VIEW_LIST);
        assert_eq!(t.e.view, View::List);
        assert_eq!((t.e.per_line(), t.e.page(), t.e.lines()), (15, 4, 5));
        t.click_item(20);
        assert_eq!(t.e.sel, Some(20));
        t.key(Key::Down, false);
        assert_eq!(t.e.sel, Some(21));
        t.key(Key::Right, false);
        assert_eq!(t.e.sel, Some(36));
        t.key(Key::Left, false);
        t.key(Key::Up, false);
        assert_eq!(t.e.sel, Some(20));
        // Llegar a la última columna desplaza horizontalmente.
        t.key(Key::End, false);
        assert_eq!((t.e.sel, t.e.top), (Some(61), 1));
        let r = t.e.item_rect(61);
        assert_eq!(t.e.item_at(r.x + 5, r.y + 5), Some(61));
        t.key(Key::Home, false);
        assert_eq!((t.e.sel, t.e.top), (Some(0), 0));
        // Rueda: una columna por muesca.
        t.wheel(1);
        assert_eq!(t.e.top, 1);
        t.wheel(1);
        assert_eq!(t.e.top, 1);
        t.wheel(-1);
        assert_eq!(t.e.top, 0);
        // Barra horizontal: clic en la pista a la derecha del pulgar avanza una página.
        let tr = t.e.track();
        t.click(tr.right() - 3, tr.y + 4);
        assert_eq!(t.e.top, 1);
        // Zona vacía bajo la última columna (que tiene 10 elementos): menú de carpeta.
        let r = t.e.item_rect(61);
        let m = t.right(r.x + 5, r.y + 2 * ROW_H).unwrap();
        assert_eq!((m.tag, t.sel_name()), (TAG_MENU_BG, None));
    }

    #[test]
    fn vista_detalles_con_rueda() {
        let mut t = T::new();
        t.many(60);
        assert_eq!(t.e.view, View::Details);
        t.wheel(1);
        assert_eq!(t.e.top, 3);
        t.wheel(-1);
        t.wheel(-1);
        assert_eq!(t.e.top, 0);
        t.wheel(100);
        let top = t.e.max_top();
        assert_eq!(t.e.top, top);
        // Rueda fuera de la lista: nada.
        t.cx(|e, cx| e.mouse(MouseEv::Wheel { x: 30, y: 200, dz: -1 }, cx));
        assert_eq!(t.e.top, top);
        // Clic derecho sobre una fila desplazada selecciona la correcta.
        let r = t.e.item_rect(top + 10);
        t.right(r.x + 5, r.y + 5);
        assert_eq!(t.e.sel, Some(top + 10));
    }

    #[test]
    fn deshacer_con_ctrl_z_y_menu() {
        let mut t = T::new();
        // Sin nada que deshacer: no pasa nada.
        let (d, _) = t.key(Key::Char('z'), true);
        assert!(d.is_none());
        t.click_item(1);
        t.key(Key::Delete, false);
        assert!(!t.sys.files.exists(pth(Std::Documents, "nota.txt").as_str()));
        t.key(Key::Char('z'), true);
        assert!(t.sys.files.exists(pth(Std::Documents, "nota.txt").as_str()));
        assert_eq!(t.rows(), strs(&["Sub", "nota.txt"]));
        // Desde el menú.
        t.right_row("nota.txt");
        t.pick(TAG_MENU_ITEM, M_DELETE);
        t.right(EMPTY.0, EMPTY.1);
        t.pick(TAG_MENU_BG, M_UNDO);
        assert!(t.sys.files.exists(pth(Std::Documents, "nota.txt").as_str()));
        // Un error al deshacer se muestra: el nombre original ya lo ocupa otro archivo.
        t.right_row("nota.txt");
        t.pick(TAG_MENU_ITEM, M_RENAME);
        t.answer(DialogResult::Text { tag: TAG_RENAME, value: Some(String::from("nuevo.txt")) });
        let now = t.now_dt();
        t.sys.files.write(pth(Std::Documents, "nota.txt").as_str(), b"otro", now).unwrap();
        let (_, d, _) = t.cx(|e, cx| e.undo(cx));
        assert!(matches!(d, Some(DialogRequest::Message { tag: TAG_ERROR, .. })));
    }

    #[test]
    fn documento_de_texto_nuevo() {
        let mut t = T::new();
        t.right(EMPTY.0, EMPTY.1);
        let (d, _) = t.pick(TAG_MENU_BG, M_NEW_DOC);
        let Some(DialogRequest::Input { tag, value, .. }) = d else { panic!("sin diálogo") };
        assert_eq!((tag, value.as_str()), (TAG_NEWDOC, "Nuevo documento de texto.txt"));
        t.answer(DialogResult::Text { tag: TAG_NEWDOC, value: Some(value) });
        assert!(t.sys.files.exists(pth(Std::Documents, "Nuevo documento de texto.txt").as_str()));
        assert_eq!(t.sel_name().as_deref(), Some("Nuevo documento de texto.txt"));
        // Se propone otro nombre libre la segunda vez; sin extensión se añade .txt.
        t.right(EMPTY.0, EMPTY.1);
        let (d, _) = t.pick(TAG_MENU_BG, M_NEW_DOC);
        let Some(DialogRequest::Input { value, .. }) = d else { panic!() };
        assert_eq!(value, "Nuevo documento de texto (2).txt");
        t.answer(DialogResult::Text { tag: TAG_NEWDOC, value: Some(String::from("apuntes")) });
        assert!(t.sys.files.exists(pth(Std::Documents, "apuntes.txt").as_str()));
        t.answer(DialogResult::Text { tag: TAG_NEWDOC, value: Some(String::from("datos.csv")) });
        assert!(t.sys.files.exists(pth(Std::Documents, "datos.csv").as_str()));
        // Repetido o inválido: error.
        let d = t.answer(DialogResult::Text { tag: TAG_NEWDOC, value: Some(String::from("apuntes.txt")) });
        assert!(matches!(d, Some(DialogRequest::Message { tag: TAG_ERROR, .. })));
        let d = t.answer(DialogResult::Text { tag: TAG_NEWDOC, value: Some(String::from("a/b")) });
        assert!(matches!(d, Some(DialogRequest::Message { tag: TAG_ERROR, .. })));
        // Deshacer manda a la Papelera el último documento creado.
        t.answer(DialogResult::Text { tag: TAG_NEWDOC, value: Some(String::from("ultimo")) });
        t.key(Key::Char('z'), true);
        assert!(!t.sys.files.exists(pth(Std::Documents, "ultimo.txt").as_str()));
    }

    #[test]
    fn propiedades_de_archivo_carpeta_y_zona() {
        let mut t = T::new();
        let now = t.now_dt();
        t.sys.files.write(pth(Std::Documents, "grande.bin").as_str(), &vec![0; 1500], now).unwrap();
        t.sys.files.write(pth(Std::Documents, "Sub\\x").as_str(), b"1", now).unwrap();
        t.draw();
        let msg = |d: Option<DialogRequest>| match d {
            Some(DialogRequest::Message { title, text, buttons: Buttons::Ok, .. }) => (title, text),
            o => panic!("{o:?}"),
        };
        t.right_row("grande.bin");
        let (title, text) = msg(t.pick(TAG_MENU_ITEM, M_PROPS).0);
        assert_eq!(title, "Propiedades de grande.bin");
        assert_eq!(
            text,
            format!("Nombre: grande.bin\nTipo: Archivo\nUbicación: {}\nTamaño: 2 KB (1500 bytes)\nModificado: 15/11/2026 21:37", dd(Std::Documents))
        );
        t.right_row("nota.txt");
        let (_, text) = msg(t.pick(TAG_MENU_ITEM, M_PROPS).0);
        assert!(text.contains("Tipo: Documento de texto") && text.contains("Tamaño: 4 bytes"), "{text}");
        t.right_row("Sub");
        let (_, text) = msg(t.pick(TAG_MENU_ITEM, M_PROPS).0);
        assert!(text.contains("Contiene: 1 elemento\n") && !text.contains("Tamaño"), "{text}");
        t.right(EMPTY.0, EMPTY.1);
        let (title, text) = msg(t.pick(TAG_MENU_BG, M_PROPS).0);
        assert_eq!(title, "Propiedades de Documentos");
        assert!(text.contains("Contiene: 3 elementos") && text.contains(&format!("Ubicación: {}\n", parent(&dd(Std::Documents)).unwrap())), "{text}");
    }

    #[test]
    fn recordar_vista_y_orden() {
        let mut t = T::new();
        t.view(M_VIEW_ICONS);
        t.pick(TAG_MENU_BG, M_SORT_SIZE);
        t.pick(TAG_MENU_BG, M_DESC);
        assert_eq!(t.sys.files.read(&prefs()).unwrap(), b"vista=iconos\norden=tamano\ndesc=1\n");
        // Otra ventana lee lo guardado al abrirse.
        let mut e2 = make();
        let mut s = Surface::new(1000, 520, 0);
        e2.draw(&mut s, &t.th, &mut t.sys, true);
        assert_eq!((e2.view, e2.sort, e2.desc), (View::Icons, Sort::Size, true));
        // Los cambios por la cabecera también se guardan.
        t.view(M_VIEW_DETAILS);
        let x = t.e.col_x()[1] + 20;
        t.click(x, RIB_H + NAV_H + 5);
        assert_eq!(t.sys.files.read(&prefs()).unwrap(), b"vista=detalles\norden=fecha\ndesc=0\n");
        // Valores que no se entienden se ignoran.
        let now = t.now_dt();
        t.sys.files.write(&prefs(), b"vista=raro\nbasura\norden=tipo\ndesc=1\n", now).unwrap();
        let mut e3 = make();
        e3.draw(&mut s, &t.th, &mut t.sys, true);
        assert_eq!((e3.view, e3.sort, e3.desc), (View::Details, Sort::Type, true));
    }

    #[test]
    fn dibuja_las_tres_vistas_y_la_barra_con_nombre_nuevo() {
        let mut t = T::new();
        t.many(40);
        for id in [M_VIEW_ICONS, M_VIEW_LIST, M_VIEW_DETAILS] {
            t.view(id);
            let s = t.draw();
            assert!(s.px.iter().any(|&p| p != 0));
        }
        // El botón se llama "Cambiar nombre" y se puede pulsar.
        t.click_item(1);
        let (d, _) = t.rib(R_REN);
        assert!(matches!(d, Some(DialogRequest::Input { tag: TAG_RENAME, .. })));
        let r = t.e.rib.item_rect(R_DEL, 760).unwrap();
        assert!(r.right() < 760);
    }
    // ---- cinta, selección múltiple, lotes, Mover a / Copiar a, vistas, extensiones y barra ----

    impl T {
        fn ks(&mut self, key: Key, shift: bool) {
            let ev = KeyEvent { key, ch: None, down: true, shift, ctrl: false, alt: false };
            self.cx(|e, cx| e.key(&ev, cx));
            self.draw();
        }
        /// Como el escritorio: aviso de Ctrl y Mayús con `Key::Other`.
        fn mods(&mut self, ctrl: bool, shift: bool) {
            let ev = KeyEvent { key: Key::Other, ch: None, down: ctrl || shift, shift, ctrl, alt: false };
            self.cx(|e, cx| e.key(&ev, cx));
        }
        fn marked(&self) -> Vec<usize> {
            self.e.marked.clone()
        }
        fn select_range(&mut self, a: usize, b: usize) {
            self.click_item(a);
            self.mods(false, true);
            self.click_item(b);
            self.mods(false, false);
        }
        fn up_(&mut self, x: i32, y: i32) {
            self.cx(|e, cx| e.mouse(MouseEv::Up { x, y }, cx));
            self.draw();
        }
    }

    #[test]
    fn la_cinta_mide_lo_que_se_reserva() {
        assert_eq!(build_ribbon(false).height(), RIB_H);
        assert_eq!(build_ribbon(true).height(), RIB_H);
        let t = T::new();
        // Inicio pide 1010 px: a 1000 (tamaño por defecto) el final del último grupo se recorta.
        assert!(t.e.rib.content_width() <= 1010, "{}", t.e.rib.content_width());
        assert_eq!(t.e.rib.active(), 0);
    }

    #[test]
    fn cinta_inicio_un_boton_de_cada_grupo() {
        let mut t = T::new();
        t.many(1); // Sub, f00, nota.txt
        t.click_item(2);
        // Portapapeles: Copiar, Cortar y Pegar.
        t.rib(R_COPY);
        assert_eq!(t.sys.files.clip(), Some((pth(Std::Documents, "nota.txt").as_str(), false)));
        t.rib(R_CUT);
        assert_eq!(t.sys.files.clip(), Some((pth(Std::Documents, "nota.txt").as_str(), true)));
        t.rib(R_COPY);
        t.rib(R_PASTE);
        assert!(t.sys.files.exists(pth(Std::Documents, "nota (2).txt").as_str()));
        // Organizar: Cambiar nombre y Eliminar.
        t.click_item(2);
        let (d, _) = t.rib(R_REN);
        assert!(matches!(d, Some(DialogRequest::Input { tag: TAG_RENAME, .. })));
        t.rib(R_DEL);
        assert!(!t.sys.files.exists(pth(Std::Documents, "nota (2).txt").as_str()) || !t.sys.files.exists(pth(Std::Documents, "nota.txt").as_str()));
        // Nuevo: carpeta y documento.
        let (d, _) = t.rib(R_NEWD);
        assert!(matches!(d, Some(DialogRequest::Input { tag: TAG_NEWDOC, .. })));
        let (d, _) = t.rib(R_NEWF);
        assert!(matches!(d, Some(DialogRequest::Input { tag: TAG_NEW, .. })));
        // Abrir: Propiedades y Abrir.
        t.click_item(0);
        let (d, _) = t.rib(R_PROPS);
        assert!(matches!(d, Some(DialogRequest::Message { tag: TAG_PROPS, .. })));
        t.rib(R_OPEN);
        assert_eq!(t.e.title(), "Sub");
        t.open(dd(Std::Documents).as_str());
        // Seleccionar.
        t.rib(R_SELALL);
        assert_eq!(t.marked().len(), t.e.rows.len());
        t.rib(R_SELNONE);
        assert!(t.marked().is_empty());
    }

    #[test]
    fn cinta_estados_segun_seleccion_portapapeles_y_papelera() {
        let mut t = T::new();
        let uno = [Cmd::Copy, Cmd::Cut, Cmd::Delete, Cmd::MoveTo, Cmd::CopyTo, Cmd::Rename, Cmd::Open];
        for c in uno {
            assert!(!t.e.enabled(c), "{c:?}");
        }
        assert!(!t.e.enabled(Cmd::Paste) && !t.e.enabled(Cmd::SelNone) && t.e.enabled(Cmd::NewFolder));
        // Un botón gris no hace nada.
        t.rib(R_COPY);
        assert!(t.sys.files.clip().is_none());
        t.click_item(1);
        for c in uno {
            assert!(t.e.enabled(c), "{c:?}");
        }
        // Varios: Cambiar nombre y Abrir en gris.
        t.many(2);
        t.select_range(1, 2);
        assert!(t.e.enabled(Cmd::Copy) && t.e.enabled(Cmd::MoveTo) && !t.e.enabled(Cmd::Rename) && !t.e.enabled(Cmd::Open));
        let (d, _) = t.rib(R_REN);
        assert!(d.is_none());
        let (d, o) = t.rib(R_OPEN);
        assert!(d.is_none() && o.is_none());
        // Pegar se activa con algo en el portapapeles.
        assert!(!t.e.enabled(Cmd::Paste));
        t.rib(R_COPY);
        assert!(t.e.enabled(Cmd::Paste));
        // Papelera: lo de siempre en gris, Restaurar y Vaciar según el caso.
        t.rib(R_DEL);
        t.open(files::TRASH);
        t.rib(R_SELALL);
        for c in [Cmd::Copy, Cmd::Cut, Cmd::Paste, Cmd::Rename, Cmd::NewFolder, Cmd::NewDoc, Cmd::MoveTo, Cmd::CopyTo, Cmd::Delete, Cmd::Open] {
            assert!(!t.e.enabled(c), "{c:?}");
        }
        assert!(t.e.enabled(Cmd::Restore) && t.e.enabled(Cmd::EmptyTrash) && t.e.enabled(Cmd::Props));
        let (d, _) = t.rib(R_EMPTY);
        assert!(matches!(d, Some(DialogRequest::Message { tag: TAG_EMPTY, .. })));
        // Restaurar actúa sobre todos los seleccionados.
        let n = t.sys.files.trash_list().unwrap().len();
        assert_eq!(n, 2);
        t.rib(R_RESTORE);
        assert!(t.sys.files.trash_list().unwrap().is_empty());
        assert!(t.sys.files.exists(pth(Std::Documents, "f00").as_str()) && t.sys.files.exists(pth(Std::Documents, "f01").as_str()));
    }

    #[test]
    fn menu_archivo_de_la_cinta() {
        let mut t = T::new();
        let r = t.e.rib.file_rect();
        let (x, y) = (r.x + r.w / 2, r.y + r.h / 2);
        t.cx(|e, cx| {
            e.mouse(MouseEv::Move { x, y }, cx);
            e.mouse(MouseEv::Down { x, y }, cx);
            e.mouse(MouseEv::Up { x, y }, cx);
        });
        let m = t.menu.take().expect("menú Archivo");
        assert_eq!((m.tag, m.x, m.y), (TAG_MENU_FILE, r.x, r.bottom()));
        assert_eq!(texts(&m.items), ["Abrir ventana nueva", "Abrir Terminal aquí", "-", "Cerrar"]);
        assert!(!m.items[1].disabled);
        t.pick(TAG_MENU_FILE, M_FILE_NEWWIN);
        assert_eq!(t.open_new.as_deref(), Some(dd(Std::Documents).as_str()));
        t.pick(TAG_MENU_FILE, M_FILE_TERM);
        assert_eq!(t.term.as_deref(), Some(dd(Std::Documents).as_str()));
        assert!(!t.closed);
        t.pick(TAG_MENU_FILE, M_FILE_CLOSE);
        assert!(t.closed);
        // En la Papelera no hay terminal.
        t.open(files::TRASH);
        t.term = None;
        t.pick(TAG_MENU_FILE, M_FILE_TERM);
        assert!(t.term.is_none());
    }

    #[test]
    fn seleccion_multiple_con_teclado_y_botones() {
        let mut t = T::new();
        t.many(5); // Sub, f00..f04, nota.txt
        t.key(Key::Char('a'), true);
        assert_eq!(t.marked(), (0..7).collect::<Vec<_>>());
        t.rib(R_SELNONE);
        assert!(t.marked().is_empty() && t.e.sel.is_none());
        t.click_item(1);
        t.rib(R_INVERT);
        assert_eq!(t.marked(), [0, 2, 3, 4, 5, 6]);
        t.rib(R_SELALL);
        assert_eq!(t.marked().len(), 7);
        // Mayús + flechas amplía y reduce desde el ancla.
        t.click_item(2);
        t.ks(Key::Down, true);
        t.ks(Key::Down, true);
        assert_eq!(t.marked(), [2, 3, 4]);
        t.ks(Key::Up, true);
        assert_eq!(t.marked(), [2, 3]);
        t.ks(Key::End, true);
        assert_eq!(t.marked(), [2, 3, 4, 5, 6]);
        // Una flecha sola vuelve a la selección simple.
        t.ks(Key::Up, false);
        assert_eq!(t.marked(), [5]);
    }

    #[test]
    fn seleccion_multiple_con_ctrl_y_mayus_clic() {
        let mut t = T::new();
        t.many(5);
        t.select_range(2, 5);
        assert_eq!(t.marked(), [2, 3, 4, 5]);
        t.mods(true, false);
        t.click_item(0);
        t.click_item(3);
        t.mods(false, false);
        assert_eq!(t.marked(), [0, 2, 4, 5]);
        // Un clic normal deja solo ese.
        t.click_item(4);
        assert_eq!(t.marked(), [4]);
        // Ctrl+clic sobre el único seleccionado lo quita.
        t.mods(true, false);
        t.click_item(4);
        t.mods(false, false);
        assert!(t.marked().is_empty() && t.e.sel.is_none());
        // El estado de teclas se vacía al soltarlas.
        assert!(!t.e.ctrl && !t.e.shift);
    }

    #[test]
    fn estado_con_varios_seleccionados() {
        let mut t = T::new();
        t.many(3);
        t.select_range(1, 3);
        let s = t.draw();
        assert!(s.px.iter().any(|&p| p != 0));
        assert_eq!(t.e.marked.len(), 3);
        assert_eq!(count_text(t.e.rows.len()), "5 elementos");
    }

    #[test]
    fn lotes_copiar_cortar_pegar_eliminar_y_deshacer() {
        let mut t = T::new();
        t.many(3); // Sub, f00, f01, f02, nota.txt
        t.select_range(1, 3);
        t.key(Key::Char('c'), true);
        assert_eq!(t.sys.files.clip_paths().0.len(), 3);
        t.open(dd(Std::Pictures).as_str());
        t.key(Key::Char('v'), true);
        for n in ["f00", "f01", "f02"] {
            assert!(t.sys.files.exists(&pth(Std::Pictures, &format!("{n}"))));
        }
        assert_eq!(t.marked().len(), 3); // lo pegado queda seleccionado
        t.key(Key::Char('z'), true);
        assert!(!t.sys.files.exists(pth(Std::Pictures, "f01").as_str()), "Deshacer quita todo el lote");
        // Pegar en la misma carpeta deja los nuevos seleccionados.
        t.open(dd(Std::Documents).as_str());
        t.select_range(1, 2);
        t.key(Key::Char('c'), true);
        t.key(Key::Char('v'), true);
        assert_eq!(t.marked().len(), 2, "{:?}", t.rows());
        // Cortar y pegar varios en otra carpeta.
        t.key(Key::Char('a'), true);
        t.key(Key::Char('x'), true);
        t.open(dd(Std::Desktop).as_str());
        t.key(Key::Char('v'), true);
        assert!(t.sys.files.list(dd(Std::Documents).as_str()).unwrap().is_empty());
        t.key(Key::Char('z'), true);
        assert!(t.sys.files.list(dd(Std::Desktop).as_str()).unwrap().is_empty());
        assert!(!t.sys.files.list(dd(Std::Documents).as_str()).unwrap().is_empty());
        // Eliminar varios de una vez y deshacer el lote.
        t.open(dd(Std::Documents).as_str());
        let antes = t.names();
        let en_papelera = t.sys.files.trash_list().unwrap().len();
        t.key(Key::Char('a'), true);
        let (d, _) = t.key(Key::Delete, false);
        assert!(d.is_none(), "sin confirmación, como con uno");
        assert!(t.names().is_empty());
        assert_eq!(t.sys.files.trash_list().unwrap().len(), en_papelera + antes.len());
        t.key(Key::Char('z'), true);
        assert_eq!(t.names(), antes);
    }

    #[test]
    fn menu_contextual_con_varios() {
        let mut t = T::new();
        t.many(2); // Sub, f00, f01, nota.txt
        t.select_range(1, 2);
        let m = t.right_row("f01");
        assert_eq!(t.marked(), [1, 2], "conserva la selección");
        assert_eq!(
            texts(&m.items),
            ["Abrir", "Abrir con >", "-", "Cortar", "Copiar", "-", "Eliminar", "Cambiar nombre", "-", "Propiedades"]
        );
        let off: Vec<&str> = m.items.iter().filter(|i| i.disabled).map(|i| i.text.as_str()).collect();
        assert_eq!(off, ["Abrir", "Abrir con", "Cambiar nombre"]);
        t.pick(TAG_MENU_ITEM, M_COPY);
        assert_eq!(t.sys.files.clip_paths().0.len(), 2);
        // Sobre uno no seleccionado: solo ese.
        t.right_row("nota.txt");
        assert_eq!(t.marked(), [3]);
        // Zona vacía: sin selección.
        t.right(EMPTY.0, EMPTY.1);
        assert!(t.marked().is_empty());
    }

    #[test]
    fn mover_a_y_copiar_a() {
        let mut t = T::new();
        t.many(2); // Sub, f00, f01, nota.txt
        t.key(Key::Char('a'), true);
        let (d, _) = t.rib(R_MOVE);
        let Some(DialogRequest::Folder { tag, title, dir }) = d else { panic!("sin selector") };
        assert_eq!((tag, title.as_str(), dir.as_str()), (TAG_MOVETO, "Mover a", dd(Std::Documents).as_str()));
        t.answer(DialogResult::Path { tag: TAG_MOVETO, path: Some(String::from(dd(Std::Pictures).as_str())) });
        assert!(t.sys.files.list(dd(Std::Documents).as_str()).unwrap().is_empty());
        assert_eq!(t.sys.files.list(dd(Std::Pictures).as_str()).unwrap().len(), 4);
        assert_eq!(t.sys.files.undo_label(), Some("Mover"));
        t.key(Key::Char('z'), true);
        assert_eq!(t.sys.files.list(dd(Std::Documents).as_str()).unwrap().len(), 4);
        // Copiar a.
        t.key(Key::Char('a'), true);
        let (d, _) = t.rib(R_COPYTO);
        let Some(DialogRequest::Folder { tag, title, .. }) = d else { panic!("sin selector") };
        assert_eq!((tag, title.as_str()), (TAG_COPYTO, "Copiar a"));
        t.answer(DialogResult::Path { tag: TAG_COPYTO, path: Some(String::from(dd(Std::Desktop).as_str())) });
        assert_eq!(t.sys.files.list(dd(Std::Desktop).as_str()).unwrap().len(), 4);
        assert_eq!(t.sys.files.list(dd(Std::Documents).as_str()).unwrap().len(), 4);
        assert_eq!(t.sys.files.undo_label(), Some("Copiar"));
        // Cancelar no hace nada; un destino que falla muestra el error.
        let antes = t.sys.files.list(dd(Std::Pictures).as_str()).unwrap().len();
        t.rib(R_COPYTO);
        t.answer(DialogResult::Path { tag: TAG_COPYTO, path: None });
        assert!(t.e.pending.is_empty());
        t.rib(R_COPYTO);
        let d = t.answer(DialogResult::Path { tag: TAG_COPYTO, path: Some(String::from("C:\\NoExiste")) });
        assert!(matches!(d, Some(DialogRequest::Message { tag: TAG_ERROR, .. })), "{d:?}");
        assert_eq!(t.sys.files.list(dd(Std::Pictures).as_str()).unwrap().len(), antes);
    }

    #[test]
    fn propiedades_de_varios() {
        let mut t = T::new();
        t.many(2); // f00 y f01 de 1 byte, nota.txt de 4, y la carpeta Sub
        t.key(Key::Char('a'), true);
        let (d, _) = t.rib(R_PROPS);
        let Some(DialogRequest::Message { title, text, buttons: Buttons::Ok, .. }) = d else { panic!("sin diálogo") };
        assert_eq!(title, "Propiedades de 4 elementos");
        assert!(text.contains("Elementos: 4") && text.contains("Archivos: 3") && text.contains("Carpetas: 1"), "{text}");
        assert!(text.contains("Tamaño total de los archivos: 6 bytes"), "{text}");
        // Con uno sigue como antes; sin selección, la carpeta actual.
        t.click_item(3);
        let (d, _) = t.rib(R_PROPS);
        assert!(matches!(d, Some(DialogRequest::Message { title, .. }) if title == "Propiedades de nota.txt"));
        t.rib(R_SELNONE);
        let (d, _) = t.rib(R_PROPS);
        assert!(matches!(d, Some(DialogRequest::Message { title, .. }) if title == "Propiedades de Documentos"));
    }

    #[test]
    fn ordenar_por_de_la_cinta() {
        let mut t = T::new();
        t.tab(1);
        t.rib(R_SORT);
        let m = t.menu.take().expect("menú de orden");
        let r = t.e.rib.item_rect(R_SORT, t.e.w).unwrap();
        assert_eq!((m.tag, m.x, m.y), (TAG_MENU_SORT, r.x, r.bottom()));
        assert_eq!(texts(&m.items), ["Nombre", "Fecha de modificación", "Tipo", "Tamaño", "-", "Ascendente", "Descendente"]);
        let on: Vec<&str> = m.items.iter().filter(|i| i.checked).map(|i| i.text.as_str()).collect();
        assert_eq!(on, ["Nombre", "Ascendente"]);
        t.pick(TAG_MENU_SORT, M_SORT_SIZE);
        t.pick(TAG_MENU_SORT, M_DESC);
        assert_eq!((t.e.sort, t.e.desc), (Sort::Size, true));
        assert_eq!(t.sys.files.read(&prefs()).unwrap(), b"vista=detalles\norden=tamano\ndesc=1\n");
    }

    #[test]
    fn casilla_de_extensiones_y_cambiar_nombre() {
        let mut t = T::new();
        t.view(M_VIEW_ICONS);
        t.tab(1);
        t.rib(R_EXT);
        assert!(!t.e.show_ext);
        let p = String::from_utf8(t.sys.files.read(&prefs()).unwrap()).unwrap();
        assert!(p.contains("extensiones=no") && p.contains("vista=iconos"), "{p}");
        // Los cambios de vista conservan la línea de extensiones.
        t.view(M_VIEW_LIST);
        let p = String::from_utf8(t.sys.files.read(&prefs()).unwrap()).unwrap();
        assert!(p.contains("extensiones=no") && p.contains("vista=lista"), "{p}");
        // El diálogo propone el nombre sin extensión y la extensión se conserva.
        t.tab(0);
        t.click_item(1);
        let (d, _) = t.key(Key::F(2), false);
        let Some(DialogRequest::Input { value, .. }) = d else { panic!("sin diálogo") };
        assert_eq!(value, "nota");
        t.answer(DialogResult::Text { tag: TAG_RENAME, value: Some(String::from("apuntes")) });
        assert!(t.sys.files.exists(pth(Std::Documents, "apuntes.txt").as_str()) && !t.sys.files.exists(pth(Std::Documents, "nota.txt").as_str()));
        assert_eq!(t.sel_name().as_deref(), Some("apuntes.txt"));
        // Una carpeta no tiene extensión que ocultar.
        t.click_item(0);
        let (d, _) = t.key(Key::F(2), false);
        assert!(matches!(d, Some(DialogRequest::Input { value, .. }) if value == "Sub"));
        // Sin nombre no se renombra.
        t.click_item(1);
        t.key(Key::F(2), false);
        let d = t.answer(DialogResult::Text { tag: TAG_RENAME, value: Some(String::new()) });
        assert!(matches!(d, Some(DialogRequest::Message { tag: TAG_ERROR, .. })));
        // Vuelve a mostrarlas.
        t.tab(1);
        t.rib(R_EXT);
        assert!(t.e.show_ext && t.sys.files.show_ext());
        let p = String::from_utf8(t.sys.files.read(&prefs()).unwrap()).unwrap();
        assert!(p.contains("extensiones=si") && !p.contains("extensiones=no"), "{p}");
        t.tab(0);
        t.click_item(1);
        let (d, _) = t.key(Key::F(2), false);
        assert!(matches!(d, Some(DialogRequest::Input { value, .. }) if value == "apuntes.txt"));
        let s = t.draw();
        assert!(s.px.iter().any(|&p| p != 0));
    }

    #[test]
    fn vistas_medianos_y_pequenos_con_su_guardado() {
        let mut t = T::new();
        t.many(60);
        t.tab(1);
        t.rib(R_V_MEDIUM);
        assert_eq!(t.e.view, View::Medium);
        assert_eq!(t.sys.files.read(&prefs()).unwrap(), b"vista=iconos-medianos\norden=nombre\ndesc=0\n");
        assert_eq!((t.e.item_rect(0).w, t.e.item_rect(0).h), (80, 68));
        assert_eq!(t.e.per_line(), (1000 - PANEL_W - SB_W) as usize / 80);
        let s = t.draw();
        assert!(s.px.iter().any(|&p| p != 0));
        t.rib(R_V_SMALL);
        assert_eq!(t.e.view, View::Small);
        assert_eq!(t.sys.files.read(&prefs()).unwrap(), b"vista=iconos-pequenos\norden=nombre\ndesc=0\n");
        assert_eq!((t.e.item_rect(0).w, t.e.item_rect(0).h), (176, 22));
        // Selección y teclado en la rejilla pequeña.
        t.click_item(5);
        assert_eq!(t.marked(), [5]);
        t.ks(Key::Down, false);
        assert_eq!(t.e.sel, Some(5 + t.e.per_line()));
        t.ks(Key::Right, true);
        assert_eq!(t.marked().len(), 2);
        // Otra ventana lee los nombres nuevos.
        let mut e2 = make();
        let mut s = Surface::new(1000, 520, 0);
        e2.draw(&mut s, &t.th, &mut t.sys, true);
        assert_eq!(e2.view, View::Small);
        // La vista activa queda marcada en la cinta y la de la rueda funciona.
        t.wheel(1);
        // El menú "Ver" ofrece las cinco, con la actual marcada.
        let m = MenuRequest { tag: TAG_MENU_BG, x: 0, y: 0, items: t.e.bg_menu(false, None) };
        let ch: Vec<&str> = m.items[0].sub.iter().filter(|i| i.checked).map(|i| i.text.as_str()).collect();
        assert_eq!(ch, ["Iconos pequeños"]);
        t.pick(TAG_MENU_BG, M_VIEW_MEDIUM);
        assert_eq!(t.e.view, View::Medium);
    }

    #[test]
    fn barra_solo_si_hace_falta_y_se_maneja() {
        let mut t = T::new();
        assert!(!t.e.need_sb() && !t.e.metrics().needed());
        assert_eq!(t.e.area().w, 1000 - PANEL_W);
        t.many(60);
        assert!(t.e.need_sb());
        let m = t.e.metrics();
        assert_eq!((m.rect.w, m.total, m.pos), (SB_W, 62, 0));
        assert_eq!(t.e.area().w, 1000 - PANEL_W - SB_W);
        // Flecha de abajo: una línea; mantenida, se repite con tick.
        let (x, y) = (m.rect.x + 4, m.rect.bottom() - 4);
        t.press(x, y);
        assert_eq!(t.e.top, 1);
        t.now += 450;
        t.cx(|e, cx| e.tick(cx));
        assert!(t.e.top >= 2);
        t.up_(x, y);
        let top = t.e.top;
        t.now += 1000;
        t.cx(|e, cx| e.tick(cx));
        assert_eq!(t.e.top, top, "soltado: no repite");
        // Pista: una página.
        let page = t.e.page();
        let (_, len) = t.e.sb_v.thumb(&t.e.metrics());
        t.press(x, m.rect.y + SB_W + 8 + len + 20);
        t.up_(x, y);
        assert_eq!(t.e.top, top + page);
        // Flecha de arriba.
        t.press(x, m.rect.y + 4);
        t.up_(x, y);
        assert_eq!(t.e.top, top + page - 1);
        // La rueda y el teclado siguen igual.
        t.wheel(-100);
        assert_eq!(t.e.top, 0);
        t.key(Key::End, false);
        assert_eq!(t.e.top, t.e.max_top());
        // Los clics en la barra no tocan la selección.
        let sel = t.marked();
        t.press(x, y);
        t.up_(x, y);
        assert_eq!(t.marked(), sel);
        // Al quedar pocos elementos, desaparece.
        for i in 0..60 {
            let _ = t.sys.files.delete(&pth(Std::Documents, &format!("f{i:02}")));
        }
        t.draw();
        assert!(!t.e.need_sb() && t.e.top == 0);
    }

    #[test]
    fn barra_horizontal_en_lista() {
        let mut t = T::new();
        t.many(100);
        t.view(M_VIEW_LIST);
        assert!(t.e.horiz() && t.e.need_sb());
        let m = t.e.metrics();
        assert_eq!((m.rect.h, m.rect.w), (SB_W, 1000 - PANEL_W));
        let (x, y) = (m.rect.right() - 4, m.rect.y + 4);
        t.click(x, y);
        t.up_(x, y);
        assert_eq!(t.e.top, 1);
        t.click(m.rect.x + 4, y);
        t.up_(x, y);
        assert_eq!(t.e.top, 0);
        t.wheel(1);
        assert_eq!(t.e.top, 1);
    }
    // ---- estructura nueva: panel, cuentas, permisos y aviso de Sistema ----

    /// Unidad con disco, dos cuentas (Ana administradora, Luis normal) y la sesión de `user`.
    fn con_cuentas(user: &str) -> TestPlatform {
        let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
        let now = sys.now();
        sys.files = Files::probe(Some(Box::new(crate::block::RamDisk::new(2048))), now);
        sys.files.format_pending(now).unwrap();
        sys.files.create_account("Ana", None, true, [1; 16], now).unwrap();
        sys.files.login("Ana", "").unwrap();
        sys.files.create_account("Luis", None, false, [2; 16], now).unwrap();
        sys.files.login(user, "").unwrap();
        sys
    }

    fn mensaje(d: Option<DialogRequest>) -> (u32, String, Buttons) {
        match d {
            Some(DialogRequest::Message { tag, text, buttons, .. }) => (tag, text, buttons),
            other => panic!("se esperaba un mensaje, hay {other:?}"),
        }
    }

    impl T {
        fn select(&mut self, name: &str) {
            let i = self.rows().iter().position(|n| n == name).unwrap();
            self.e.set_one(i);
            self.draw();
        }
        fn go_dir(&mut self, p: &str) -> Option<DialogRequest> {
            let (_, d, _) = self.cx(|e, cx| e.open_path(p, cx));
            self.draw();
            d
        }
    }

    #[test]
    fn panel_con_ocho_entradas_y_navegacion() {
        let mut t = T::new();
        let nombres = ["Escritorio", "Documentos", "Descargas", "Imágenes", "Música", "Vídeos", "Disco local (C:)", "Papelera"];
        assert_eq!(PLACES.map(|p| p.name()), nombres);
        let iconos: Vec<Icon> = PLACES.iter().map(|p| p.icon()).collect();
        assert!((0..8).all(|i| (0..i).all(|j| iconos[i] != iconos[j])), "cada entrada lleva su icono: {iconos:?}");
        for (i, n) in nombres.iter().enumerate() {
            t.click(30, RIB_H + NAV_H + 8 + PLACE_H * i as i32 + 4);
            assert_eq!(t.e.title(), *n);
            let esperada = match PLACES[i] {
                Place::Std(d) => dd(d),
                Place::Disk => String::from("C:\\"),
                Place::Trash => String::from(files::TRASH),
            };
            assert_eq!(t.e.cwd, esperada);
        }
        // La entrada activa se resalta: la de la carpeta actual.
        t.open(dd(Std::Music).as_str());
        assert_eq!(t.e.places.iter().position(|p| same(p, &t.e.cwd)), Some(4));
    }

    #[test]
    fn abre_en_documentos_con_la_ruta_real() {
        let mut t = T::new();
        assert_eq!(t.e.cwd, dd(Std::Documents));
        let migas: Vec<String> = t.e.hits.iter().filter_map(|(_, h)| if let Hit::Crumb(p) = h { Some(p.clone()) } else { None }).collect();
        assert_eq!(migas, ["C:\\", files::USERS_DIR, "C:\\Usuarios\\Usuario", dd(Std::Documents).as_str()]);
        // Dentro de la carpeta de la cuenta, las seis estándar se listan con su icono (sin error).
        t.open("C:\\Usuarios\\Usuario");
        assert_eq!(t.rows(), ["Descargas", "Documentos", "Escritorio", "Imágenes", "Música", "Vídeos"]);
        assert!(t.draw().px.iter().any(|&p| p != 0));
    }

    #[test]
    fn preferencias_por_usuario() {
        let mut t = T::with(con_cuentas("Ana"));
        t.view(M_VIEW_ICONS);
        let ana = t.sys.files.explorer_prefs();
        assert!(ana.contains("Ana") && String::from_utf8(t.sys.files.read(&ana).unwrap()).unwrap().contains("vista=iconos"));
        // Otra cuenta en el mismo equipo: ve la vista por defecto y sus cambios no tocan los de Ana.
        t.sys.files.login("Luis", "").unwrap();
        t.go_dir(t.sys.files.std_dir(Std::Documents).as_str());
        assert_eq!(t.e.view, View::Details);
        t.view(M_VIEW_LIST);
        let luis = t.sys.files.explorer_prefs();
        assert_ne!(ana, luis);
        assert!(String::from_utf8(t.sys.files.read(&luis).unwrap()).unwrap().contains("vista=lista"));
        t.sys.files.login("Ana", "").unwrap();
        assert!(String::from_utf8(t.sys.files.read(&ana).unwrap()).unwrap().contains("vista=iconos"));
        t.go_dir(t.sys.files.std_dir(Std::Documents).as_str());
        assert_eq!(t.e.view, View::Icons);
    }

    #[test]
    fn carpeta_de_otro_usuario_da_permiso_denegado_y_no_se_mueve() {
        let mut t = T::with(con_cuentas("Luis"));
        let mia = t.sys.files.std_dir(Std::Documents);
        t.go_dir(&mia);
        assert_eq!(t.e.cwd, mia);
        let (tag, text, _) = mensaje(t.go_dir("C:\\Usuarios\\Ana"));
        assert_eq!((tag, text.as_str()), (TAG_ERROR, "No tienes permiso para hacer eso."));
        assert_eq!(t.e.cwd, mia, "se queda donde estaba");
        assert!(t.rows().is_empty() || t.rows().iter().all(|n| n != "Escritorio"));
        // Igual desde una miga o con el botón Atrás: nunca queda una carpeta vacía de otro.
        assert!(t.go_dir("C:\\Usuarios\\Ana\\Documentos").is_some());
        assert_eq!(t.e.cwd, mia);
    }

    #[test]
    fn botones_desactivados_con_carpeta_protegida() {
        let mut t = T::new();
        t.open("C:\\Usuarios\\Usuario");
        t.select("Documentos");
        for c in [Cmd::Delete, Cmd::Cut, Cmd::Rename, Cmd::MoveTo] {
            assert!(!t.e.enabled(c), "{c:?} con una carpeta protegida");
        }
        assert!(t.e.enabled(Cmd::Copy) && t.e.enabled(Cmd::CopyTo) && t.e.enabled(Cmd::Props));
        // En la cinta también (botones en gris) y en el menú contextual.
        t.tab(0);
        let (d, _) = t.rib(R_DEL);
        assert!(d.is_none() && t.sys.files.exists(dd(Std::Documents).as_str()));
        let m = t.right_row("Documentos");
        let off: Vec<&str> = m.items.iter().filter(|i| i.disabled).map(|i| i.text.as_str()).collect();
        assert_eq!(off, ["Cortar", "Pegar", "Eliminar", "Cambiar nombre"]);
        // Una carpeta normal sí se puede tocar.
        t.open(dd(Std::Documents).as_str());
        t.select("Sub");
        assert!(t.e.enabled(Cmd::Delete) && t.e.enabled(Cmd::Cut) && t.e.enabled(Cmd::Rename) && t.e.enabled(Cmd::MoveTo));
        // No se crea ni se pega directamente en Usuarios ni en la carpeta de la cuenta.
        t.sys.files.set_clip(pth(Std::Documents, "nota.txt").as_str(), false);
        for dir in [files::USERS_DIR, "C:\\Usuarios\\Usuario"] {
            t.open(dir);
            t.e.has_clip = true;
            assert!(!t.e.enabled(Cmd::NewFolder) && !t.e.enabled(Cmd::NewDoc) && !t.e.enabled(Cmd::Paste), "{dir}");
            let m = t.right(EMPTY.0, EMPTY.1).expect("menú de la zona vacía");
            let off: Vec<&str> = m.items.iter().filter(|i| i.disabled).map(|i| i.text.as_str()).collect();
            assert_eq!(off, ["Pegar", "Deshacer", "Nuevo"], "{dir}");
        }
        t.open(dd(Std::Documents).as_str());
        assert!(t.e.enabled(Cmd::NewFolder) && t.e.enabled(Cmd::NewDoc) && t.e.enabled(Cmd::Paste));
        // Sin ser administrador tampoco se crea en C:\Sistema (y sí siéndolo).
        t.open("C:\\Sistema\\temp");
        assert!(t.e.enabled(Cmd::NewFolder));
        let mut l = T::with(con_cuentas("Luis"));
        l.open("C:\\Sistema\\temp");
        assert!(!l.e.enabled(Cmd::NewFolder) && !l.e.enabled(Cmd::NewDoc) && !l.e.enabled(Cmd::Paste));
    }

    #[test]
    fn aviso_de_sistema_con_si_y_con_no() {
        let mut t = T::new();
        let temp = "C:\\Sistema\\temp";
        let now = t.now_dt();
        t.sys.files.write("C:\\Sistema\\temp\\viejo.txt", b"x", now).unwrap();
        t.sys.files.write("C:\\Sistema\\temp\\otro.txt", b"y", now).unwrap();
        t.open(temp);
        let aviso = "Vas a modificar una carpeta del sistema. Si cambias o borras algo aquí, NightFury puede dejar de funcionar. ¿Continuar?";
        // Crear: pregunta; No no crea nada; Sí lo crea.
        let (d, _) = t.rib(R_NEWF);
        assert!(matches!(d, Some(DialogRequest::Input { tag: TAG_NEW, .. })));
        let (tag, txt, b) = mensaje(t.answer(text("Cache")));
        assert_eq!((tag, txt.as_str(), b), (TAG_SYS, aviso, Buttons::YesNo));
        assert!(t.answer(DialogResult::Answer { tag: TAG_SYS, answer: Answer::No }).is_none());
        assert!(!t.sys.files.exists("C:\\Sistema\\temp\\Cache"));
        t.answer(text("Cache"));
        t.answer(DialogResult::Answer { tag: TAG_SYS, answer: Answer::Yes });
        assert!(t.sys.files.exists("C:\\Sistema\\temp\\Cache"));
        // Eliminar.
        t.select("viejo.txt");
        let (d, _) = t.key(Key::Delete, false);
        assert_eq!(mensaje(d).0, TAG_SYS);
        t.answer(DialogResult::Answer { tag: TAG_SYS, answer: Answer::No });
        assert!(t.sys.files.exists("C:\\Sistema\\temp\\viejo.txt"));
        t.key(Key::Delete, false);
        t.answer(DialogResult::Answer { tag: TAG_SYS, answer: Answer::Yes });
        assert!(!t.sys.files.exists("C:\\Sistema\\temp\\viejo.txt"));
        // Cambiar nombre.
        t.select("otro.txt");
        t.key(Key::F(2), false);
        let (tag, ..) = mensaje(t.answer(DialogResult::Text { tag: TAG_RENAME, value: Some(String::from("nuevo.txt")) }));
        assert_eq!(tag, TAG_SYS);
        t.answer(DialogResult::Answer { tag: TAG_SYS, answer: Answer::Yes });
        assert!(t.sys.files.exists("C:\\Sistema\\temp\\nuevo.txt") && !t.sys.files.exists("C:\\Sistema\\temp\\otro.txt"));
        // Pegar y mover desde fuera hacia Sistema también preguntan; fuera de Sistema no.
        t.sys.files.set_clip(pth(Std::Documents, "nota.txt").as_str(), false);
        t.key(Key::Char('v'), true);
        t.answer(DialogResult::Answer { tag: TAG_SYS, answer: Answer::No });
        assert!(!t.sys.files.exists("C:\\Sistema\\temp\\nota.txt"));
        t.key(Key::Char('v'), true);
        t.answer(DialogResult::Answer { tag: TAG_SYS, answer: Answer::Yes });
        assert!(t.sys.files.exists("C:\\Sistema\\temp\\nota.txt"));
        t.open(dd(Std::Documents).as_str());
        let (d, _) = t.rib(R_NEWF);
        assert!(matches!(d, Some(DialogRequest::Input { .. })));
        assert!(t.answer(text("Libre")).is_none() && t.sys.files.exists(pth(Std::Documents, "Libre").as_str()));
    }

    // ---- v0.7.2: arrastre con el botón derecho ----

    impl T {
        fn source(&mut self, p: (i32, i32)) -> Option<Vec<String>> {
            let (r, ..) = self.cx(|e, cx| e.drag_source(p.0, p.1, cx));
            self.draw();
            r
        }
    }

    #[test]
    fn drag_source_selecciona_como_el_menu_y_conserva_la_multiple() {
        let mut t = T::new();
        let (nota, sub) = (pth(Std::Documents, "nota.txt"), pth(Std::Documents, "Sub"));
        let i = t.rows().iter().position(|n| n == "nota.txt").unwrap();
        // Elemento sin seleccionar: se selecciona solo y se devuelve.
        assert_eq!(t.source(t.pt(i)), Some(vec![nota.clone()]));
        assert_eq!(t.sel_name().as_deref(), Some("nota.txt"));
        // Con los dos seleccionados, arrastrar uno conserva la selección múltiple.
        t.mods(false, true);
        t.click_item(0);
        t.mods(false, false);
        assert_eq!(t.e.marked.len(), 2);
        let got = t.source(t.pt(i)).unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.contains(&nota) && got.contains(&sub));
        assert_eq!(t.e.marked.len(), 2);
        // Zona vacía, cabecera y panel: nada.
        assert_eq!(t.source(EMPTY), None);
        assert_eq!(t.source((300, RIB_H + NAV_H + 5)), None);
        assert_eq!(t.source((30, RIB_H + NAV_H + 20)), None);
    }

    #[test]
    fn drag_source_no_da_nada_en_papelera_ni_con_protegidas() {
        let mut t = T::new();
        // Carpeta personal: «Documentos» y compañía están protegidas.
        let home = parent(dd(Std::Documents).as_str()).unwrap();
        t.open(&home);
        let i = t.rows().iter().position(|n| n == "Documentos").unwrap();
        assert_eq!(t.source(t.pt(i)), None);
        // Papelera.
        let now = t.now_dt();
        t.sys.files.user_trash_many(&[pth(Std::Documents, "nota.txt")], now).unwrap();
        t.open(files::TRASH);
        assert!(!t.rows().is_empty());
        assert_eq!(t.source(t.pt(0)), None);
    }

    // ---- v0.7.0: recuadro de selección y arrastre ----

    impl T {
        fn down(&mut self, x: i32, y: i32) {
            self.now += 600;
            self.press(x, y);
        }
        fn mv(&mut self, x: i32, y: i32) {
            self.cx(|e, cx| e.mouse(MouseEv::Move { x, y }, cx));
            self.draw();
        }
        fn pt(&self, i: usize) -> (i32, i32) {
            let r = self.e.item_rect(i);
            (r.x + 10, r.y + 5)
        }
        fn tick(&mut self, ms: u64) {
            self.now += ms;
            self.cx(|e, cx| e.tick(cx));
            self.draw();
        }
        /// Ctrl pulsado (el escritorio lo avisa con `Key::Other`).
        fn ctrl(&mut self, on: bool) {
            self.mods(on, false);
        }
        fn over(&mut self, p: Option<(i32, i32)>, paths: &[String]) -> (bool, bool) {
            let (r, ..) = self.cx(|e, cx| e.drag_over(p, paths, cx));
            (r, self.redraw)
        }
        fn drop_at(&mut self, p: (i32, i32), paths: &[String], copy: bool) -> Option<DialogRequest> {
            let (_, d, _) = self.cx(|e, cx| e.drop_files(p.0, p.1, paths, copy, cx));
            self.draw();
            d
        }
        fn place_pt(i: i32) -> (i32, i32) {
            (30, RIB_H + NAV_H + 8 + PLACE_H * i + 4)
        }
        fn exists_in(&mut self, d: Std, n: &str) -> bool {
            self.sys.files.exists(pth(d, n).as_str())
        }
    }

    #[test]
    fn suprimir_pide_confirmacion() {
        let mut t = T::new();
        t.auto_del = false;
        t.many(2); // Sub, f00, f01, nota.txt
        t.select("nota.txt");
        let (d, _) = t.key(Key::Delete, false);
        let Some(DialogRequest::Message { tag: TAG_DEL, text, buttons: Buttons::YesNo, .. }) = d else { panic!("{d:?}") };
        assert_eq!(text, "¿Quieres enviar «nota.txt» a la Papelera?");
        t.answer(DialogResult::Answer { tag: TAG_DEL, answer: Answer::No });
        assert!(t.exists_in(Std::Documents, "nota.txt"), "con No no se elimina");
        t.answer(DialogResult::Answer { tag: TAG_DEL, answer: Answer::Yes });
        assert!(t.exists_in(Std::Documents, "nota.txt"), "un Sí suelto, sin pregunta pendiente, no hace nada");
        t.key(Key::Char('a'), true);
        let (d, _) = t.key(Key::Delete, false);
        let Some(DialogRequest::Message { text, .. }) = d else { panic!() };
        assert_eq!(text, "¿Quieres enviar estos 4 elementos a la Papelera?");
        t.answer(DialogResult::Answer { tag: TAG_DEL, answer: Answer::Yes });
        assert!(t.names().is_empty());
        t.key(Key::Char('z'), true);
        assert_eq!(t.names().len(), 4);
    }

    #[test]
    fn recuadro_desde_el_margen_izquierdo_con_la_lista_llena() {
        let mut t = T::new();
        t.many(40); // más filas de las que caben: no queda zona vacía debajo
        let (_, y0) = t.pt(0);
        let (_, y3) = t.pt(3);
        let gx = PANEL_W + GUTTER / 2;
        assert_eq!(t.e.item_at(gx, y0), None, "el margen no es una fila");
        t.down(gx, y3);
        t.mv(gx + 300, y0);
        assert_eq!(t.marked(), [0, 1, 2, 3]);
        t.up_(gx + 300, y0);
        assert_eq!(t.marked(), [0, 1, 2, 3]);
    }

    #[test]
    fn recuadro_en_detalles_con_ctrl_y_clic_vacio() {
        let mut t = T::new();
        t.many(5); // Sub, f00..f04, nota.txt
        let (ex, ey) = EMPTY;
        // Sin arrastrar: un clic en zona vacía deselecciona.
        t.click_item(1);
        t.click(ex, ey);
        assert!(t.marked().is_empty());
        // Recuadro de abajo hacia arriba hasta la fila 2.
        let (_, y2) = t.pt(2);
        t.down(ex, ey);
        t.mv(ex, y2);
        assert_eq!(t.marked(), [2, 3, 4, 5, 6]);
        assert!(t.e.mq_rect().is_some());
        t.mv(ex, t.pt(5).1);
        assert_eq!(t.marked(), [5, 6], "al encoger, suelta los que ya no toca");
        t.up_(ex, t.pt(5).1);
        assert_eq!(t.marked(), [5, 6], "al soltar queda la selección");
        assert!(t.e.mq_rect().is_none());
        // Con Ctrl, los que toca cambian de estado respecto a la selección inicial.
        t.click_item(0);
        t.ctrl(true);
        t.down(ex, ey);
        t.mv(ex, t.pt(0).1);
        assert_eq!(t.marked(), [1, 2, 3, 4, 5, 6]);
        t.up_(ex, t.pt(0).1);
        t.ctrl(false);
        // Clic simple con Ctrl en zona vacía conserva la selección.
        t.ctrl(true);
        t.click(ex, ey);
        t.ctrl(false);
        assert_eq!(t.marked(), [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn recuadro_en_lista_e_iconos() {
        let mut t = T::new();
        t.many(5);
        t.e.view = View::List;
        t.draw();
        let (_, y2) = t.pt(2);
        t.down(600, 300);
        t.mv(t.pt(2).0, y2);
        assert_eq!(t.marked(), [2, 3, 4, 5, 6]);
        t.up_(600, 300);
        t.e.view = View::Icons;
        t.e.clear_sel();
        t.draw();
        let (x4, _) = t.pt(4);
        t.down(x4 + 20, 400);
        t.mv(t.pt(1).0, t.pt(1).1);
        assert_eq!(t.marked(), [1, 2, 3, 4]);
        t.up_(0, 0);
    }

    #[test]
    fn recuadro_se_desplaza_solo_en_el_borde() {
        let mut t = T::new();
        t.many(80); // 82 elementos
        t.e.view = View::Icons;
        t.draw();
        t.key(Key::End, false);
        let top = t.e.top;
        assert!(top >= 3);
        // Empieza en un hueco de la última línea (a la derecha del último elemento).
        let last = t.e.rows.len() - 1;
        let r = t.e.item_rect(last);
        t.down(r.right() + 30, r.y + 5);
        assert!(t.e.mq.is_some());
        // Puntero por encima de la zona: sube de línea en línea con `tick`, sin mover el ratón.
        let a = t.e.area();
        t.mv(a.x + 5, a.y - 3);
        let sel0 = t.marked().len();
        t.tick(100);
        assert_eq!(t.e.top, top - 1);
        assert!(t.marked().len() > sel0, "el recuadro sigue anclado al contenido");
        t.tick(10);
        assert_eq!(t.e.top, top - 1, "respeta el ritmo");
        for _ in 0..top {
            t.tick(100);
        }
        assert_eq!(t.e.top, 0);
        assert!(t.marked().contains(&0) && t.marked().contains(&last));
        // Por debajo: baja.
        t.mv(a.x + 5, a.bottom() + 3);
        t.tick(100);
        assert_eq!(t.e.top, 1);
        // Soltado el botón, ya no se mueve.
        t.up_(a.x, a.bottom());
        t.tick(500);
        assert_eq!(t.e.top, 1);
        // Lista: avance lateral.
        t.e.view = View::List;
        t.e.top = 0;
        t.draw();
        t.key(Key::End, false);
        let top = t.e.top;
        assert!(top >= 1);
        let a = t.e.area();
        let r = t.e.item_rect(t.e.rows.len() - 1);
        t.down(r.x + 5, r.bottom() + 5);
        t.mv(a.x - 3, r.y);
        t.tick(100);
        assert_eq!(t.e.top, top - 1);
    }

    #[test]
    fn origen_del_arrastre() {
        let mut t = T::new();
        t.many(5); // Sub, f00..f04, nota.txt
        let p = |n: &str| pth(Std::Documents, n);
        // Menos de 5 px: no arrastra.
        let (x, y) = t.pt(3);
        t.down(x, y);
        t.mv(x + 3, y + 2);
        assert!(t.drag.is_none());
        t.up_(x + 3, y + 2);
        assert_eq!(t.marked(), [3]);
        // Más de 5 px: arrastra la selección (un no seleccionado se selecciona y va solo).
        t.down(x, y);
        t.mv(x + 8, y);
        assert_eq!(t.drag.take().map(|d| d.paths), Some(vec![p("f02")]));
        let (x5, y5) = t.pt(5);
        t.down(x5, y5);
        t.mv(x5, y5 + 12);
        assert_eq!(t.drag.take().unwrap().paths, vec![p("f04")]);
        assert_eq!(t.marked(), [5]);
        t.mv(x5, y5 + 40);
        assert!(t.drag.is_none(), "una vez pedido, no se repite");
        // Selección múltiple: pulsar un seleccionado no la deshace; arrastra todo.
        t.select_range(2, 4);
        let (x3, y3) = t.pt(3);
        t.down(x3, y3);
        assert_eq!(t.marked(), [2, 3, 4]);
        t.mv(x3 + 10, y3);
        assert_eq!(t.drag.take().unwrap().paths, vec![p("f01"), p("f02"), p("f03")]);
        assert_eq!(t.marked(), [2, 3, 4]);
        // Sin arrastre, al soltar queda solo ese.
        t.select_range(2, 4);
        t.down(x3, y3);
        t.mv(x3 + 2, y3);
        t.up_(x3, y3);
        assert_eq!(t.marked(), [3]);
        // Con Ctrl no se arrastra.
        t.ctrl(true);
        t.down(x3, y3 + ROW_H);
        t.mv(x3 + 20, y3 + ROW_H);
        t.ctrl(false);
        assert!(t.drag.is_none());
        // Doble clic sigue abriendo.
        t.up_(0, 0);
        t.click_item(0);
        t.click_item(0);
        assert_eq!(t.e.cwd, p("Sub"));
        // Papelera: no se arrastra.
        let now = t.now_dt();
        t.sys.files.trash(&p("f00"), now).ok();
        t.open(files::TRASH);
        assert!(!t.rows().is_empty());
        let (xt, yt) = t.pt(0);
        t.down(xt, yt);
        t.mv(xt + 20, yt);
        assert!(t.drag.is_none());
        t.up_(xt, yt);
        // Carpetas protegidas: no se arrastran.
        t.open(dd(Std::Documents).rsplit_once('\\').unwrap().0);
        let i = t.rows().iter().position(|n| n == "Documentos").unwrap();
        let (xp, yp) = t.pt(i);
        t.down(xp, yp);
        t.mv(xp + 20, yp);
        assert!(t.drag.is_none());
        t.up_(xp, yp);
        assert_eq!(t.marked(), [i]);
    }

    #[test]
    fn destino_del_arrastre() {
        let mut t = T::new();
        t.many(2); // Sub, f00, f01, nota.txt
        let p = |n: &str| pth(Std::Documents, n);
        let now = t.now_dt();
        let otra = pth(Std::Pictures, "x.txt");
        t.sys.files.write(&otra, b"x", now).unwrap();
        // Sobre una carpeta de la lista: sí, con resaltado; redibuja solo al cambiar.
        let sub = t.pt(0);
        assert_eq!(t.over(Some(sub), &[p("f00")]), (true, true));
        assert_eq!(t.e.drop_tgt, Some(Tgt::Row(0)));
        assert_eq!(t.over(Some((sub.0 + 3, sub.1)), &[p("f00")]), (true, false));
        // Sobre un archivo o zona vacía de la misma carpeta: no, y quita el resaltado.
        assert_eq!(t.over(Some(t.pt(2)), &[p("f00")]), (false, true));
        assert_eq!(t.e.drop_tgt, None);
        // Zona vacía con archivos de otra carpeta: sí, sin resaltar.
        assert_eq!(t.over(Some(EMPTY), &[otra.clone()]), (true, false));
        assert_eq!(t.e.drop_tgt, None);
        // Panel: Imágenes sí; la carpeta donde ya están, no; Papelera sí.
        assert_eq!(t.over(Some(T::place_pt(3)), &[p("f00")]), (true, true));
        assert_eq!(t.e.drop_tgt, Some(Tgt::Place(3)));
        assert_eq!(t.over(Some(T::place_pt(1)), &[p("f00")]), (false, true));
        assert_eq!(t.over(Some(T::place_pt(7)), &[p("f00")]), (true, true));
        assert_eq!(t.e.drop_tgt, Some(Tgt::Place(7)));
        // Sobre sí misma o dentro de sí misma: no.
        assert_eq!(t.over(Some(sub), &[p("Sub")]).0, false);
        let now = t.now_dt();
        t.sys.files.mkdir(&join(&p("Sub"), "Hijo"), now).unwrap();
        t.open(&p("Sub"));
        assert_eq!(t.over(Some(t.pt(0)), &[p("Sub")]).0, false, "dentro de lo arrastrado");
        // Fuera de la ventana: quita el resaltado.
        assert_eq!(t.over(Some(T::place_pt(2)), &[otra.clone()]).0, true);
        assert_eq!(t.over(None, &[otra.clone()]), (false, true));
        assert_eq!(t.e.drop_tgt, None);
        // La Papelera: su lista no admite soltar; la cinta tampoco.
        t.open(files::TRASH);
        assert_eq!(t.over(Some(EMPTY), &[otra.clone()]).0, false);
        assert_eq!(t.over(Some((500, 40)), &[otra.clone()]).0, false);
        // Donde no se puede escribir (directamente en Usuarios).
        t.open(files::USERS_DIR);
        assert_eq!(t.over(Some(EMPTY), &[otra.clone()]).0, false);
        let sub = t.pt(0);
        assert_eq!(t.over(Some(sub), &[otra]).0, false);
    }

    #[test]
    fn soltar_mueve_copia_y_va_a_la_papelera() {
        let mut t = T::new();
        t.many(3);
        let p = |n: &str| pth(Std::Documents, n);
        // Mover a una carpeta de la lista; deshacer como un lote.
        let sub = t.pt(0);
        t.over(Some(sub), &[p("f00"), p("f01")]);
        assert!(t.drop_at(sub, &[p("f00"), p("f01")], false).is_none());
        assert_eq!(t.e.drop_tgt, None);
        assert!(!t.exists_in(Std::Documents, "f00") && !t.exists_in(Std::Documents, "f01"));
        assert!(t.sys.files.exists(&join(&p("Sub"), "f00")) && t.sys.files.exists(&join(&p("Sub"), "f01")));
        t.key(Key::Char('z'), true);
        assert!(t.exists_in(Std::Documents, "f00") && t.exists_in(Std::Documents, "f01"));
        assert!(!t.sys.files.exists(&join(&p("Sub"), "f00")));
        // Copiar (Ctrl).
        t.drop_at(sub, &[p("f02")], true);
        assert!(t.exists_in(Std::Documents, "f02") && t.sys.files.exists(&join(&p("Sub"), "f02")));
        // A una entrada del panel.
        t.drop_at(T::place_pt(3), &[p("f00")], false);
        assert!(!t.exists_in(Std::Documents, "f00") && t.exists_in(Std::Pictures, "f00"));
        // A la Papelera, y se deshace.
        t.drop_at(T::place_pt(7), &[p("f01")], false);
        assert!(!t.exists_in(Std::Documents, "f01"));
        assert_eq!(t.sys.files.trash_list().unwrap().len(), 1);
        t.key(Key::Char('z'), true);
        assert!(t.exists_in(Std::Documents, "f01"));
        // A la misma carpeta: nada.
        let antes = t.names();
        t.drop_at(EMPTY, &[p("f01")], false);
        t.drop_at(EMPTY, &[p("f01")], true);
        assert_eq!(t.names(), antes);
        // Carpeta dentro de sí misma: nada y no se pierde.
        let sub = t.pt(0);
        assert!(t.drop_at(sub, &[p("Sub")], false).is_none());
        assert!(t.sys.files.exists(&p("Sub")) && t.names() == antes);
        // Lo movido a la carpeta visible queda seleccionado.
        t.open(&p("Sub"));
        t.drop_at(EMPTY, &[p("f01")], false);
        assert_eq!(t.rows().iter().filter(|n| *n == "f01").count(), 1);
        assert_eq!(t.sel_name().as_deref(), Some("f01"));
    }

    #[test]
    fn soltar_con_permiso_denegado() {
        let mut t = T::with(con_cuentas("Luis"));
        let now = t.now_dt();
        t.sys.files.login("Ana", "").unwrap();
        let ajeno = "C:\\Usuarios\\Ana\\Documentos\\secreto.txt";
        t.sys.files.write(ajeno, b"x", now).unwrap();
        t.sys.files.login("Luis", "").unwrap();
        let mia = t.sys.files.std_dir(Std::Documents);
        t.go_dir(&mia);
        let d = t.drop_at(EMPTY, &[String::from(ajeno)], false);
        let (tag, text, _) = mensaje(d);
        assert_eq!((tag, text.as_str()), (TAG_ERROR, "No tienes permiso para hacer eso."));
        t.sys.files.login("Ana", "").unwrap();
        assert!(t.sys.files.exists(ajeno), "no se pierde nada");
    }

    #[test]
    fn soltar_en_sistema_pide_confirmacion() {
        let mut t = T::new();
        let now = t.now_dt();
        let temp = "C:\\Sistema\\temp";
        let nota = pth(Std::Documents, "nota.txt");
        t.sys.files.write("C:\\Sistema\\temp\\viejo.txt", b"x", now).unwrap();
        t.open(temp);
        // Destino en Sistema: pregunta; No no hace nada; Sí mueve.
        let (tag, ..) = mensaje(t.drop_at(EMPTY, &[nota.clone()], false));
        assert_eq!(tag, TAG_SYS);
        t.answer(DialogResult::Answer { tag: TAG_SYS, answer: Answer::No });
        assert!(t.sys.files.exists(&nota) && !t.sys.files.exists("C:\\Sistema\\temp\\nota.txt"));
        t.drop_at(EMPTY, &[nota.clone()], false);
        t.answer(DialogResult::Answer { tag: TAG_SYS, answer: Answer::Yes });
        assert!(!t.sys.files.exists(&nota) && t.sys.files.exists("C:\\Sistema\\temp\\nota.txt"));
        // Origen en Sistema: también pregunta.
        let (tag, ..) = mensaje(t.drop_at(T::place_pt(1), &[String::from("C:\\Sistema\\temp\\viejo.txt")], false));
        assert_eq!(tag, TAG_SYS);
        t.answer(DialogResult::Answer { tag: TAG_SYS, answer: Answer::No });
        assert!(t.sys.files.exists("C:\\Sistema\\temp\\viejo.txt"));
        t.drop_at(T::place_pt(1), &[String::from("C:\\Sistema\\temp\\viejo.txt")], false);
        t.answer(DialogResult::Answer { tag: TAG_SYS, answer: Answer::Yes });
        assert!(t.exists_in(Std::Documents, "viejo.txt"));
        // Copiar desde Sistema no modifica Sistema: sin aviso.
        t.sys.files.write("C:\\Sistema\\temp\\otro.txt", b"y", now).unwrap();
        assert!(t.drop_at(T::place_pt(1), &[String::from("C:\\Sistema\\temp\\otro.txt")], true).is_none());
        assert!(t.exists_in(Std::Documents, "otro.txt") && t.sys.files.exists("C:\\Sistema\\temp\\otro.txt"));
    }

    /// Capturas para revisar a ojo: `NF_CAPTURAS=carpeta cargo test -p nf-core capturas -- --ignored`.
    #[test]
    #[ignore]
    fn capturas_de_la_ventana() {
        let Ok(dir) = std::env::var("NF_CAPTURAS") else { return };
        std::fs::create_dir_all(&dir).unwrap();
        let mut t = T::new();
        t.many(14);
        let now = t.now_dt();
        t.sys.files.write(pth(Std::Documents, "Lista de la compra con un nombre bastante largo.txt").as_str(), b"leche", now).unwrap();
        let guarda = |s: &Surface, n: &str| std::fs::write(format!("{dir}/{n}.bmp"), crate::bmp::encode(s)).unwrap();
        t.select_range(2, 5);
        guarda(&t.draw(), "inicio_varios");
        t.tab(1);
        t.rib(R_V_MEDIUM);
        guarda(&t.draw(), "vista_medianos");
        t.rib(R_V_SMALL);
        guarda(&t.draw(), "vista_pequenos");
        t.rib(R_V_LIST);
        guarda(&t.draw(), "vista_lista");
        t.rib(R_EXT);
        t.rib(R_V_DETAILS);
        guarda(&t.draw(), "vista_sin_extensiones");
        t.tab(0);
        t.open(files::TRASH);
        guarda(&t.draw(), "papelera");
        // v0.7.0: recuadro a medio dibujar y carpeta resaltada como destino.
        t.open(dd(Std::Documents).as_str());
        t.e.view = View::Icons;
        t.e.clear_sel();
        t.draw();
        let (x3, y3) = t.pt(3);
        t.down(x3 + 20, 440);
        t.mv(x3 - 150, y3);
        guarda(&t.draw(), "recuadro");
        t.up_(0, 0);
        t.e.view = View::Details;
        t.e.clear_sel();
        t.draw();
        let p = pth(Std::Documents, "f01");
        t.over(Some(t.pt(0)), &[p.clone()]);
        guarda(&t.draw(), "destino_carpeta");
        t.over(Some(T::place_pt(3)), &[p]);
        guarda(&t.draw(), "destino_panel");
    }
}
