//! Pantallas de cuentas a pantalla completa: asistente inicial, inicio de sesión y bloqueo.
//! `Desktop` les pasa el teclado y el ratón mientras `covers()` y las pinta encima de un fondo
//! oscurecido; aquí no hay nada del escritorio (ventanas, barra, menús).
//!
//! Las claves y códigos viven solo en los búferes `pw`, `pw2` y `code` (reservados de una vez, sin
//! crecer, y borrados con `crypto::wipe`); no se copian a ningún registro, título ni archivo.
//!
//! Cifrado (v0.6.0): la pantalla "Disco cifrado" (`Mode::Disk`, antes del asistente y del inicio
//! de sesión) y la recuperación de una cuenta (`Mode::Recover`, desde el inicio de sesión). Derivar
//! una llave tarda (PBKDF2): se avisa con "Comprobando…" y el trabajo se hace en un tick posterior
//! (`job`), para que ese aviso llegue a la pantalla antes.
use crate::accounts::{self, AccountError};
use crate::crypto::wipe;
use crate::dialogs::{fit, wrap};
use crate::font::{self, Align};
use crate::geom::Rect;
use crate::files::Files;
use crate::icons::{self, Icon};
use crate::input::{Key, KeyEvent};
use crate::keys::{self, KeyError, Pool};
use crate::platform::{DateTime, Platform};
use crate::surface::{blend, lighten, Surface};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Qué se muestra: nada (escritorio normal), solo el fondo oscuro mientras salen los avisos del
/// disco, el asistente, la lista de usuarios o el bloqueo.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Off,
    Boot,
    Setup,
    Pick,
    Lock,
    /// Disco cifrado esperando su clave (o su código de recuperación).
    Disk,
    /// Recuperar una cuenta cifrada con su código: código, clave nueva y repetirla.
    Recover,
}

/// Lo que el escritorio debe hacer tras un evento.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Out {
    None,
    /// Sesión abierta con éxito (inicio de sesión).
    Entered,
    /// Clave correcta en el bloqueo.
    Unlocked,
    /// El asistente terminó (la sesión del asistente ya está cerrada).
    SetupDone,
    /// El disco cifrado se abrió, o se eligió seguir sin él: continúa el arranque normal.
    DiskDone,
    /// Asistente de disco nuevo: hace falta la ventana del ratón (el escritorio la abre y
    /// responde con `begin_provision` o `cancel_entropy`).
    NeedEntropy,
    Shutdown,
    Reboot,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Item {
    Name,
    Pw,
    Pw2,
    Admin,
    Ok,
    Done,
    Avatar(usize),
    Msg,
    Power,
    Shut(usize),
    /// Código de recuperación (campo siempre visible).
    Code,
    /// Enlace de texto: cambiar de clave a código, o "He olvidado mi clave".
    Link,
    /// Enlace "Seguir sin disco".
    Skip,
    /// "Volver" de la confirmación de seguir sin disco.
    Back,
    /// Opción de cifrado del asistente (0 Ninguno, 1 Mi carpeta, 2 Disco entero, 3 Ambos).
    Opt(usize),
}

/// Qué se cifra en un disco nuevo (v0.7.1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Enc {
    Nothing,
    Home,
    Disk,
    Both,
}

impl Enc {
    const ALL: [Enc; 4] = [Enc::Nothing, Enc::Home, Enc::Disk, Enc::Both];

    fn disk(self) -> bool {
        matches!(self, Enc::Disk | Enc::Both)
    }

    fn home(self) -> bool {
        matches!(self, Enc::Home | Enc::Both)
    }
}

/// Pantalla del asistente. Con disco ya formateado solo se usa `Account`; con disco nuevo el orden
/// es Choice, DiskKey (si hay cifrado de disco), Account, Preparing y Codes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Account,
    Choice,
    DiskKey,
    Preparing,
    Codes,
}

/// Cuenta tecleada en el asistente de disco nuevo: se crea al final, cuando el disco ya está formateado.
struct PendAcc {
    name: String,
    pw: String,
    admin: bool,
    created: bool,
    coded: bool,
}

/// Trabajo lento aplazado un tick (derivar llaves).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Job {
    Disk,
    Recover,
    Provision,
}

const FIELD_H: i32 = 30;
const BTN_H: i32 = 34;
const AV: i32 = 80;
const SLOT_W: i32 = 130;
const SLOT_H: i32 = 128;
const CARD_W: i32 = 460;
const PAD: i32 = 28;
const MAX_LIST: usize = 4;
const MAX_PW: usize = 64;
const CARD_BG: u32 = 0x202020;
const GREY: u32 = 0xB5B5B5;
const RED: u32 = 0xFF8A80;
const AVATARS: [u32; 6] = [0x3B7DD8, 0x2E8B57, 0xC98A0B, 0xB5446E, 0x7A55C9, 0x0E8A8A];
const SHUT_ROWS: [&str; 2] = ["Apagar", "Reiniciar"];
const STRENGTH_W: i32 = 170;
const LOCK_W: i32 = 64;
const LOCK_H: i32 = 72;
/// Espera mínima entre pintar "Comprobando…" y empezar a derivar la llave.
const JOB_DELAY_MS: u64 = 30;
const GREEN: u32 = 0xA5D6A7;
const DISK_SUB_KEY: &str = "Escribe la clave del disco para abrirlo.";
const DISK_SUB_CODE: &str = "Escribe el código de recuperación del disco (8 grupos de 5 caracteres).";
const SKIP_TEXT: &str = "Los archivos se guardarán solo en la memoria y se perderán al apagar.";
const RECOVER_SUB: &str = "Escribe el código de recuperación de esta cuenta y elige una clave nueva.";
const STRENGTH: [(&str, u32); 5] = [("Muy débil", 0xFF6B5E), ("Débil", 0xFF9A4D), ("Aceptable", 0xF2C94C), ("Buena", 0x9BDB6B), ("Fuerte", 0x4CCB6A)];
const OPT_H: i32 = 46;
const OPTS: [(&str, &str); 4] = [
    ("Ninguno", "Los datos se guardan sin cifrar."),
    ("Mi carpeta", "Tus archivos solo se abren con tu clave."),
    ("Disco entero", "Todo el disco pide una clave en cada arranque."),
    ("Ambos", "Clave del disco al arrancar y tu carpeta con su clave."),
];
const CHOICE_SUB: &str = "Elige qué quieres proteger con una clave.";
const CHOICE_WARN: &str = "Si olvidas la clave y pierdes el código de recuperación, no se podrán recuperar los datos.";
const DISKKEY_SUB: &str = "Se pedirá en cada arranque del equipo. Mínimo 6 caracteres.";
const CODE_WARN: &str = "Apúntalo en papel y guárdalo en un lugar seguro. Es la única forma de recuperar tus datos si olvidas la clave. No se volverá a mostrar.";
const AMBER: u32 = 0xF2C94C;
const SUB: &str = "La primera cuenta es la del administrador: podrá crear y quitar otras cuentas.";

impl Layout {
    /// Rectángulo del código de recuperación en una tarjeta de ancho `w` cuyo cuerpo empieza en `body_y`
    /// (relativo a la tarjeta).
    fn code_rect_in(w: i32, body_y: i32) -> Rect {
        Login::code_box(Rect::new(0, 0, w, 0), body_y)
    }
}

struct Layout {
    items: Vec<(Item, Rect)>,
    /// Todo lo que ocupa el contenido (para repintar).
    bounds: Rect,
    card: Rect,
    title: Rect,
    /// Esquina del cuerpo (texto, lista) en pantalla.
    body_y: i32,
}

pub struct Login {
    pub mode: Mode,
    w: i32,
    h: i32,
    /// Asistente: ya hay una cuenta creada (segunda tarjeta).
    second: bool,
    /// (nombre, tiene clave) de las cuentas que se pueden elegir; en el bloqueo solo la actual.
    users: Vec<(String, bool)>,
    sel: usize,
    created: Vec<(String, bool)>,
    name: String,
    pw: String,
    pw2: String,
    /// Código de recuperación (se ve al teclearlo).
    code: String,
    admin: bool,
    focus: Item,
    err: Option<(Item, String)>,
    hot: Option<Item>,
    shut: bool,
    caret: bool,
    next_blink: u64,
    now: u64,
    /// Fallos seguidos (en memoria; se reinicia al acertar) y fin de la espera actual.
    fails: u32,
    until: Option<u64>,
    last_secs: u64,
    clock: DateTime,
    damage: Vec<Rect>,
    /// Por cada cuenta de `users`: su carpeta está cifrada (ofrece "He olvidado mi clave").
    recoverable: Vec<bool>,
    /// Disco: se escribe el código de recuperación en vez de la clave.
    recovery_mode: bool,
    /// Disco: pregunta "¿seguir sin disco?" abierta.
    confirm_skip: bool,
    /// Trabajo lento pendiente y el instante a partir del cual se hace.
    job: Option<(Job, u64)>,
    /// El mensaje de `err` es una buena noticia (se pinta en verde).
    good: bool,
    /// Asistente de disco nuevo (el disco aún no está formateado): pantalla, cifrado elegido y lo tecleado.
    fresh: bool,
    step: Step,
    enc: Enc,
    /// Clave del disco ya confirmada (solo vive hasta formatear).
    disk_pw: String,
    pend: Vec<PendAcc>,
    /// Aviso ya mostrado: la siguiente pulsación sigue igualmente (clave débil o cuenta sin clave).
    weak_ok: bool,
    warned: bool,
    /// El disco ya se formateó (no se repite si hay que reintentar).
    disk_done: bool,
    /// Códigos de recuperación (a quién pertenece, código) y cuál se muestra.
    codes: Vec<(String, String)>,
    code_i: usize,
}

fn secret() -> String {
    String::with_capacity(MAX_PW * 4)
}

/// Borra los bytes del búfer y deja otro vacío de la misma capacidad (no vuelve a crecer).
fn wipe_str(s: &mut String) {
    let mut b = core::mem::replace(s, secret()).into_bytes();
    wipe(&mut b);
}

/// Comprueba el código de recuperación tecleado sin derivar nada (acepta minúsculas, espacios y guiones).
fn check_code(input: &str) -> Result<(), &'static str> {
    let n = input.chars().filter(|c| !c.is_whitespace() && *c != '-').count();
    if n != 40 {
        return Err("El código debe tener 40 caracteres.");
    }
    keys::normalize_recovery(input).map(|_| ()).ok_or("El código tiene caracteres que no son válidos.")
}

/// Da a una operación de `Files` el acumulador de la plataforma (son dos préstamos de `sys`: el
/// acumulador se saca un momento y se devuelve con sus movimientos).
fn with_pool<R>(sys: &mut dyn Platform, f: impl FnOnce(&mut Files, &mut Pool) -> R) -> R {
    let mut pool = core::mem::take(sys.pool());
    let r = f(sys.files(), &mut pool);
    *sys.pool() = pool;
    r
}

/// Texto pequeño en hasta dos líneas dentro de `r`.
fn text_wrapped(s: &mut Surface, r: Rect, text: &str, color: u32) {
    let lh = font::line_h(8);
    for (i, line) in wrap(text, 8, r.w).iter().take(2).enumerate() {
        text_in(s, Rect::new(r.x, r.y + i as i32 * lh, r.w, lh), line, 8, false, color, Align::Left);
    }
}

fn text_in(s: &mut Surface, r: Rect, text: &str, size: u32, bold: bool, color: u32, align: Align) {
    let old = s.clip();
    s.set_clip(old.intersect(r));
    font::draw_in(s, r, text, size, bold, color, align);
    s.set_clip(old);
}

impl Login {
    pub fn new(w: i32, h: i32) -> Login {
        Login {
            mode: Mode::Off,
            w,
            h,
            second: false,
            users: Vec::new(),
            sel: 0,
            created: Vec::new(),
            name: String::new(),
            pw: secret(),
            pw2: secret(),
            code: secret(),
            admin: false,
            focus: Item::Name,
            err: None,
            hot: None,
            shut: false,
            caret: true,
            next_blink: 0,
            now: 0,
            fails: 0,
            until: None,
            last_secs: 0,
            clock: DateTime::default(),
            damage: Vec::new(),
            recoverable: Vec::new(),
            recovery_mode: false,
            confirm_skip: false,
            job: None,
            good: false,
            fresh: false,
            step: Step::Account,
            enc: Enc::Home,
            disk_pw: secret(),
            pend: Vec::new(),
            weak_ok: false,
            warned: false,
            disk_done: false,
            codes: Vec::new(),
            code_i: 0,
        }
    }

    /// Las pantallas que tapan todo el escritorio.
    pub fn covers(&self) -> bool {
        matches!(self.mode, Mode::Setup | Mode::Pick | Mode::Lock | Mode::Disk | Mode::Recover)
    }

    pub fn take_damage(&mut self) -> Vec<Rect> {
        core::mem::take(&mut self.damage)
    }

    fn dmg(&mut self, r: Rect) {
        if !r.is_empty() {
            self.damage.push(r);
        }
    }

    fn dmg_all(&mut self) {
        self.dmg(Rect::new(0, 0, self.w, self.h));
    }

    /// Repinta el contenido (tarjeta o bloque central).
    fn dmg_panel(&mut self) {
        let b = self.layout().bounds.inflate(16);
        self.dmg(b);
    }

    fn wipe_all(&mut self) {
        wipe_str(&mut self.pw);
        wipe_str(&mut self.pw2);
        wipe_str(&mut self.code);
    }

    /// Borra todo lo secreto del asistente de disco nuevo (claves, cuentas tecleadas y códigos).
    fn wipe_setup(&mut self) {
        wipe_str(&mut self.disk_pw);
        for p in self.pend.iter_mut() {
            wipe_str(&mut p.pw);
        }
        self.pend.clear();
        for c in self.codes.iter_mut() {
            wipe_str(&mut c.1);
        }
        self.codes.clear();
        self.code_i = 0;
        self.disk_done = false;
        self.weak_ok = false;
        self.warned = false;
    }

    fn enter(&mut self, mode: Mode) {
        self.wipe_all();
        self.wipe_setup();
        self.fresh = false;
        self.step = Step::Account;
        self.mode = mode;
        self.job = None;
        self.good = false;
        self.confirm_skip = false;
        self.shut = false;
        self.hot = None;
        self.err = None;
        self.caret = true;
        self.next_blink = self.now + 530;
        self.dmg_all();
    }

    /// Cierra la pantalla: borra las claves y no deja nada pendiente.
    pub fn off(&mut self) {
        self.enter(Mode::Off);
        self.users.clear();
        self.recoverable.clear();
        self.created.clear();
        self.name.clear();
    }

    pub fn start_boot(&mut self) {
        self.off();
        self.mode = Mode::Boot;
    }

    pub fn start_setup(&mut self) {
        self.enter(Mode::Setup);
        self.second = false;
        self.created.clear();
        self.name.clear();
        self.admin = false;
        self.focus = Item::Name;
    }

    /// Asistente de un disco recién preparado: primero el cifrado, y el disco se formatea al final.
    pub fn start_new_disk(&mut self) {
        self.start_setup();
        self.fresh = true;
        self.step = Step::Choice;
        self.enc = Enc::Home;
        self.focus = Item::Ok;
    }

    /// La ventana del ratón respondió Ok: a preparar el disco y las cuentas.
    pub fn begin_provision(&mut self) {
        self.mode = Mode::Setup;
        self.step = Step::Preparing;
        self.err = None;
        self.good = false;
        self.focus = Item::Ok;
        self.job = Some((Job::Provision, self.now + JOB_DELAY_MS));
        self.dmg_all();
    }

    /// La ventana del ratón se canceló: vuelve a "¿Añadir otra cuenta?" sin haber tocado el disco.
    pub fn cancel_entropy(&mut self) {
        self.mode = Mode::Setup;
        self.step = Step::Account;
        self.focus = Item::Name;
        self.dmg_all();
    }

    /// Pantalla "Disco cifrado": clave del disco (o su código de recuperación).
    pub fn start_disk(&mut self) {
        self.enter(Mode::Disk);
        self.recovery_mode = false;
        self.focus = Item::Pw;
    }

    /// Lista de cuentas (nombre, tiene clave) para elegir. Después, `set_recoverable`.
    pub fn start_pick(&mut self, users: Vec<(String, bool)>) {
        self.enter(Mode::Pick);
        self.recoverable = alloc::vec![false; users.len()];
        self.users = users;
        self.sel = 0;
        self.focus = self.order().first().copied().unwrap_or(Item::Ok);
    }

    /// Qué cuentas de la lista tienen la carpeta cifrada (una por cuenta, en el mismo orden).
    pub fn set_recoverable(&mut self, v: Vec<bool>) {
        if v.len() == self.users.len() && v != self.recoverable {
            self.recoverable = v;
            self.dmg_panel();
        }
    }

    pub fn start_lock(&mut self, name: &str, has_password: bool) {
        self.enter(Mode::Lock);
        self.users = alloc::vec![(String::from(name), has_password)];
        self.sel = 0;
        self.focus = self.order().first().copied().unwrap_or(Item::Ok);
    }

    /// Nombres de las cuentas de la lista.
    pub fn user_names(&self) -> Vec<String> {
        self.users.iter().map(|u| u.0.clone()).collect()
    }

    pub fn set_clock(&mut self, t: DateTime) {
        if (t.hour, t.minute, t.day) != (self.clock.hour, self.clock.minute, self.clock.day) {
            self.clock = t;
            if self.covers() {
                self.dmg(self.clock_rect());
            }
        }
    }

    fn clock_rect(&self) -> Rect {
        Rect::new(24, self.h - 96, 240, 80)
    }

    fn power_rect(&self) -> Rect {
        Rect::new(self.w - 64, self.h - 64, 48, 48)
    }

    fn shut_rect(&self) -> Rect {
        let p = self.power_rect();
        Rect::new(p.right() - 170, p.y - 84, 170, 80)
    }

    fn shut_row(&self, i: usize) -> Rect {
        let s = self.shut_rect();
        Rect::new(s.x, s.y + 40 * i as i32, s.w, 40)
    }

    // ---------------------------------------------------------------- espera por fallos

    fn waiting(&self) -> bool {
        self.until.is_some_and(|u| self.now < u)
    }

    fn secs_left(&self) -> u64 {
        self.until.map_or(0, |u| u.saturating_sub(self.now).div_ceil(1000))
    }

    /// Fallo de clave: cada 3 seguidos, espera de 5, 15, 30, 60, 120 y 300 s (luego se queda en 300).
    fn fail(&mut self) {
        self.fails += 1;
        if self.fails % 3 == 0 {
            let n = (self.fails / 3 - 1).min(5) as usize;
            let secs = [5u64, 15, 30, 60, 120, 300][n];
            self.until = Some(self.now + secs * 1000);
            self.last_secs = secs;
        }
    }

    fn msg_text(&self) -> Option<String> {
        if self.job.is_some() {
            return Some(String::from("Comprobando…"));
        }
        if self.waiting() {
            return Some(format!("Demasiados intentos. Espera {} s para volver a probar.", self.secs_left()));
        }
        self.err.as_ref().filter(|e| e.0 == Item::Msg).map(|e| e.1.clone())
    }

    /// Reloj del escritorio (unas 100 veces por segundo): parpadeo del cursor y cuenta atrás.
    pub fn tick(&mut self, ms: u64, sys: &mut dyn Platform) -> Out {
        self.now = ms;
        if !self.covers() {
            return Out::None;
        }
        if self.until.is_some() {
            if self.waiting() {
                if self.secs_left() != self.last_secs {
                    self.last_secs = self.secs_left();
                    self.dmg_item(Item::Msg);
                }
            } else {
                self.until = None;
                self.err = None;
                self.dmg_item(Item::Msg);
            }
        }
        if ms >= self.next_blink {
            self.next_blink = ms + 530;
            self.caret = !self.caret;
            if matches!(self.focus, Item::Name | Item::Pw | Item::Pw2 | Item::Code) {
                self.dmg_item(self.focus);
            }
        }
        if let Some((job, at)) = self.job {
            if ms >= at {
                self.job = None;
                self.dmg_item(Item::Msg);
                return self.run_job(job, sys);
            }
        }
        Out::None
    }

    fn dmg_item(&mut self, it: Item) {
        let r = self.layout().items.iter().find(|i| i.0 == it).map(|i| i.1);
        if let Some(r) = r {
            self.dmg(r.inflate(3));
        }
    }

    // ---------------------------------------------------------------- disposición

    fn can_recover(&self) -> bool {
        self.mode == Mode::Pick && self.recoverable.get(self.sel).copied().unwrap_or(false)
    }

    fn has_pw(&self) -> bool {
        self.users.get(self.sel).is_some_and(|u| u.1)
    }

    fn order(&self) -> Vec<Item> {
        match self.mode {
            Mode::Setup if self.fresh && self.step != Step::Account => match self.step {
                Step::Choice | Step::Codes => alloc::vec![Item::Ok],
                Step::DiskKey => alloc::vec![Item::Pw, Item::Pw2, Item::Ok, Item::Done],
                _ => Vec::new(),
            },
            Mode::Setup if !self.second => alloc::vec![Item::Name, Item::Pw, Item::Pw2, Item::Ok],
            Mode::Setup => alloc::vec![Item::Name, Item::Pw, Item::Pw2, Item::Admin, Item::Ok, Item::Done],
            Mode::Recover => alloc::vec![Item::Code, Item::Pw, Item::Pw2, Item::Ok, Item::Done],
            Mode::Disk if self.confirm_skip => alloc::vec![Item::Done, Item::Back],
            Mode::Disk => alloc::vec![Item::Pw, Item::Ok, Item::Link, Item::Skip],
            Mode::Pick if self.has_pw() && self.can_recover() => alloc::vec![Item::Pw, Item::Link, Item::Ok],
            Mode::Pick | Mode::Lock if self.has_pw() => alloc::vec![Item::Pw, Item::Ok],
            Mode::Pick | Mode::Lock => alloc::vec![Item::Ok],
            _ => Vec::new(),
        }
    }

    fn layout(&self) -> Layout {
        let lh = font::line_h(8);
        let mut items: Vec<(Item, Rect)> = Vec::new();
        if self.mode == Mode::Disk {
            return self.layout_disk();
        }
        if self.mode == Mode::Setup && self.fresh && self.step != Step::Account {
            return self.layout_step();
        }
        if matches!(self.mode, Mode::Setup | Mode::Recover) {
            let rec = self.mode == Mode::Recover;
            let w = CARD_W.min(self.w - 16).max(240);
            let inner = w - 2 * PAD;
            let block = lh + 2 + FIELD_H + 2 + lh + 4;
            let mut y = PAD;
            let title = Rect::new(PAD, y, inner, 24);
            y += 30;
            let body = y;
            if rec {
                y += wrap(RECOVER_SUB, 8, inner).len() as i32 * lh + 14;
            } else if self.second {
                let n = self.created.len().min(MAX_LIST) as i32 + (self.created.len() > MAX_LIST) as i32;
                y += lh + 2 + n * (font::line_h(9) + 2) + 12;
            } else {
                y += wrap(SUB, 8, inner).len() as i32 * lh + 14;
            }
            for it in [if rec { Item::Code } else { Item::Name }, Item::Pw, Item::Pw2] {
                items.push((it, Rect::new(PAD, y + lh + 2, inner, FIELD_H)));
                y += block;
            }
            if self.second && !rec {
                items.push((Item::Admin, Rect::new(PAD, y, inner, 22)));
                y += 30;
            }
            y += 6;
            if self.second || rec {
                let bw = (inner - 12) / 2;
                items.push((Item::Ok, Rect::new(PAD, y, bw, BTN_H)));
                items.push((Item::Done, Rect::new(PAD + bw + 12, y, bw, BTN_H)));
            } else {
                items.push((Item::Ok, Rect::new(PAD, y, inner, BTN_H)));
            }
            y += BTN_H;
            if rec {
                items.push((Item::Msg, Rect::new(PAD, y + 8, inner, lh + 4)));
                y += lh + 12;
            } else if self.fresh {
                items.push((Item::Msg, Rect::new(PAD, y + 8, inner, 2 * lh + 4)));
                y += 2 * lh + 12;
            }
            y += PAD;
            let (ox, oy) = ((self.w - w) / 2, ((self.h - y) / 2).max(8));
            for it in items.iter_mut() {
                it.1.x += ox;
                it.1.y += oy;
            }
            let card = Rect::new(ox, oy, w, y);
            let title = Rect::new(title.x + ox, title.y + oy, title.w, title.h);
            items.push((Item::Power, self.power_rect()));
            return Layout { items, bounds: card, card, title, body_y: body + oy };
        }
        // Lista de cuentas (inicio de sesión y bloqueo).
        let n = self.users.len().max(1);
        let per_row = (((self.w - 80) / SLOT_W).max(1) as usize).min(n);
        let rows = n.div_ceil(per_row) as i32;
        let mut total = 50 + rows * SLOT_H + 12 + BTN_H + 10 + lh + 4;
        if self.has_pw() {
            total += FIELD_H + 10;
            if self.can_recover() {
                total += 22;
            }
        }
        let y0 = ((self.h - total) / 2 - 20).max(16);
        let title = Rect::new(0, y0, self.w, 26);
        let ya = y0 + 50;
        for i in 0..self.users.len() {
            let (row, col) = (i / per_row, i % per_row);
            let cnt = per_row.min(n - row * per_row) as i32;
            let x0 = (self.w - cnt * SLOT_W) / 2;
            items.push((Item::Avatar(i), Rect::new(x0 + col as i32 * SLOT_W, ya + row as i32 * SLOT_H, SLOT_W, SLOT_H)));
        }
        let mut yb = ya + rows * SLOT_H + 12;
        if self.has_pw() {
            items.push((Item::Pw, Rect::new((self.w - 280) / 2, yb, 280, FIELD_H)));
            yb += FIELD_H + 10;
            if self.can_recover() {
                items.push((Item::Link, Rect::new((self.w - 280) / 2, yb - 4, 280, 18)));
                yb += 22;
            }
        }
        items.push((Item::Ok, Rect::new((self.w - 150) / 2, yb, 150, BTN_H)));
        yb += BTN_H + 10;
        items.push((Item::Msg, Rect::new((self.w - 440) / 2, yb, 440, lh + 4)));
        let bounds = items.iter().fold(title, |u, i| u.union(i.1));
        items.push((Item::Power, self.power_rect()));
        Layout { items, bounds, card: Rect::default(), title, body_y: 0 }
    }

    /// A quién pertenece el código que se muestra ("del disco", "de la cuenta Ana").
    fn code_owner(&self) -> String {
        self.codes.get(self.code_i).map(|c| c.0.clone()).unwrap_or_default()
    }

    /// Rectángulo del código de recuperación dentro de su tarjeta.
    fn code_box(card: Rect, body_y: i32) -> Rect {
        Rect::new(card.x + PAD, body_y + font::line_h(9) + 10, card.w - 2 * PAD, 52)
    }

    /// Tarjetas del asistente de disco nuevo que no son la de cuentas: cifrado, clave del disco,
    /// preparando y códigos de recuperación.
    fn layout_step(&self) -> Layout {
        let lh = font::line_h(8);
        let mut items: Vec<(Item, Rect)> = Vec::new();
        let w = CARD_W.min(self.w - 16).max(240);
        let inner = w - 2 * PAD;
        let block = lh + 2 + FIELD_H + 2 + lh + 4;
        let mut y = PAD;
        let title = Rect::new(PAD, y, inner, 24);
        y += 30;
        let body = y;
        match self.step {
            Step::Choice => {
                y += wrap(CHOICE_SUB, 8, inner).len() as i32 * lh + 12;
                for i in 0..OPTS.len() {
                    items.push((Item::Opt(i), Rect::new(PAD, y, inner, OPT_H)));
                    y += OPT_H + 4;
                }
                y += 6 + wrap(CHOICE_WARN, 8, inner).len() as i32 * lh + 12;
                items.push((Item::Ok, Rect::new(PAD, y, inner, BTN_H)));
                y += BTN_H;
                items.push((Item::Msg, Rect::new(PAD, y + 8, inner, 2 * lh + 4)));
                y += 2 * lh + 12;
            }
            Step::DiskKey => {
                y += wrap(DISKKEY_SUB, 8, inner).len() as i32 * lh + 14;
                for it in [Item::Pw, Item::Pw2] {
                    items.push((it, Rect::new(PAD, y + lh + 2, inner, FIELD_H)));
                    y += block;
                }
                y += 6;
                let bw = (inner - 12) / 2;
                items.push((Item::Ok, Rect::new(PAD, y, bw, BTN_H)));
                items.push((Item::Done, Rect::new(PAD + bw + 12, y, bw, BTN_H)));
                y += BTN_H;
                items.push((Item::Msg, Rect::new(PAD, y + 8, inner, 2 * lh + 4)));
                y += 2 * lh + 12;
            }
            Step::Preparing => {
                y += font::line_h(9) + 6 + lh + 6;
            }
            _ => {
                let b = Layout::code_rect_in(w, body);
                y = b.bottom() + 14 + wrap(CODE_WARN, 8, inner).len() as i32 * lh + 14;
                items.push((Item::Ok, Rect::new(PAD, y, inner, BTN_H)));
                y += BTN_H;
            }
        }
        y += PAD;
        let (ox, oy) = ((self.w - w) / 2, ((self.h - y) / 2).max(8));
        for it in items.iter_mut() {
            it.1.x += ox;
            it.1.y += oy;
        }
        let card = Rect::new(ox, oy, w, y);
        let title = Rect::new(title.x + ox, title.y + oy, title.w, title.h);
        items.push((Item::Power, self.power_rect()));
        Layout { items, bounds: card, card, title, body_y: body + oy }
    }

    fn disk_sub(&self) -> &'static str {
        if self.recovery_mode {
            DISK_SUB_CODE
        } else {
            DISK_SUB_KEY
        }
    }

    /// Pantalla "Disco cifrado": candado, título, texto y debajo el campo, los botones y los enlaces.
    /// `card` es aquí el rectángulo del candado y `body_y` donde empieza el texto.
    fn layout_disk(&self) -> Layout {
        let lh = font::line_h(8);
        let mut items: Vec<(Item, Rect)> = Vec::new();
        let text = if self.confirm_skip { SKIP_TEXT } else { self.disk_sub() };
        let body = wrap(text, 9, 440).len() as i32 * font::line_h(9);
        let mut total = LOCK_H + 12 + 26 + 8 + body + 14;
        total += if self.confirm_skip { BTN_H } else { FIELD_H + 10 + BTN_H + 12 + 20 + 4 + 20 + 8 + lh + 4 };
        let y0 = ((self.h - total) / 2 - 10).max(16);
        let lock = Rect::new(self.w / 2 - LOCK_W / 2, y0, LOCK_W, LOCK_H);
        let title = Rect::new(0, y0 + LOCK_H + 12, self.w, 26);
        let body_y = title.y + 34;
        let mut y = body_y + body + 14;
        if self.confirm_skip {
            let x0 = (self.w - 332) / 2;
            items.push((Item::Done, Rect::new(x0, y, 200, BTN_H)));
            items.push((Item::Back, Rect::new(x0 + 212, y, 120, BTN_H)));
        } else {
            items.push((Item::Pw, Rect::new((self.w - 360) / 2, y, 360, FIELD_H)));
            y += FIELD_H + 10;
            items.push((Item::Ok, Rect::new((self.w - 150) / 2, y, 150, BTN_H)));
            y += BTN_H + 12;
            items.push((Item::Link, Rect::new((self.w - 360) / 2, y, 360, 20)));
            y += 24;
            items.push((Item::Skip, Rect::new((self.w - 360) / 2, y, 360, 20)));
            y += 28;
            items.push((Item::Msg, Rect::new((self.w - 440) / 2, y, 440, lh + 4)));
        }
        let bounds = items.iter().fold(title.union(lock), |u, i| u.union(i.1));
        items.push((Item::Power, self.power_rect()));
        Layout { items, bounds, card: lock, title, body_y }
    }

    fn item_at(&self, x: i32, y: i32) -> Option<Item> {
        if self.shut && self.shut_rect().contains(x, y) {
            return Some(Item::Shut(((y - self.shut_rect().y) / 40).clamp(0, 1) as usize));
        }
        self.layout().items.iter().rev().find(|i| i.1.contains(x, y) && i.0 != Item::Msg).map(|i| i.0)
    }

    // ---------------------------------------------------------------- entrada

    pub fn hover(&mut self, x: i32, y: i32) {
        if !self.covers() {
            return;
        }
        let hot = self.item_at(x, y).filter(|i| !matches!(i, Item::Name | Item::Pw | Item::Pw2 | Item::Code));
        if hot != self.hot {
            let l = self.layout();
            for it in [self.hot, hot].into_iter().flatten() {
                let r = match it {
                    Item::Shut(i) => Some(self.shut_row(i)),
                    _ => l.items.iter().find(|e| e.0 == it).map(|e| e.1),
                };
                if let Some(r) = r {
                    self.dmg(r.inflate(8));
                }
            }
            self.hot = hot;
        }
    }

    fn set_shut(&mut self, open: bool) {
        if open != self.shut {
            self.shut = open;
            let (s, p) = (self.shut_rect(), self.power_rect());
            self.dmg(s.union(p).inflate(2));
        }
    }

    pub fn click(&mut self, x: i32, y: i32, sys: &mut dyn Platform) -> Out {
        if !self.covers() {
            return Out::None;
        }
        let item = self.item_at(x, y);
        if self.shut {
            self.set_shut(false);
            return match item {
                Some(Item::Shut(0)) => Out::Shutdown,
                Some(Item::Shut(_)) => Out::Reboot,
                Some(Item::Power) | None => Out::None,
                // Cierra el menú y deja que el clic haga lo suyo.
                _ => self.click_item(item, sys),
            };
        }
        self.click_item(item, sys)
    }

    fn click_item(&mut self, item: Option<Item>, sys: &mut dyn Platform) -> Out {
        if self.job.is_some() && item != Some(Item::Power) {
            return Out::None;
        }
        match item {
            Some(Item::Power) => {
                self.set_shut(true);
                Out::None
            }
            Some(it @ (Item::Name | Item::Pw | Item::Pw2 | Item::Code | Item::Admin)) => {
                if it == Item::Admin {
                    self.admin = !self.admin;
                }
                self.set_focus(it);
                Out::None
            }
            Some(Item::Ok) => self.activate(Item::Ok, sys),
            Some(it @ (Item::Done | Item::Link | Item::Skip | Item::Back | Item::Opt(_))) => self.activate(it, sys),
            Some(Item::Avatar(i)) => {
                self.select(i);
                if self.has_pw() {
                    Out::None
                } else {
                    self.attempt(sys)
                }
            }
            // Bloqueo con cuenta sin clave: basta un clic en cualquier sitio.
            _ if self.mode == Mode::Lock && !self.has_pw() => self.attempt(sys),
            _ => Out::None,
        }
    }

    fn set_focus(&mut self, it: Item) {
        if self.focus != it {
            self.dmg_item(self.focus);
            self.focus = it;
            self.dmg_item(it);
        }
        self.caret = true;
    }

    fn select(&mut self, i: usize) {
        if i < self.users.len() && (i != self.sel || self.err.is_some()) {
            self.dmg_panel(); // lo que ocupaba antes (la clave aparece o no según la cuenta)
            self.sel = i;
            self.wipe_all();
            self.err = None;
            self.good = false;
            self.focus = self.order().first().copied().unwrap_or(Item::Ok);
            self.dmg_panel();
        }
    }

    fn move_focus(&mut self, fwd: bool) {
        let o = self.order();
        if o.is_empty() {
            return;
        }
        let i = o.iter().position(|&x| x == self.focus).unwrap_or(0);
        let n = o.len();
        self.set_focus(o[if fwd { (i + 1) % n } else { (i + n - 1) % n }]);
    }

    fn text_mut(&mut self, it: Item) -> Option<&mut String> {
        match it {
            Item::Name => Some(&mut self.name),
            Item::Pw => Some(&mut self.pw),
            Item::Pw2 => Some(&mut self.pw2),
            Item::Code => Some(&mut self.code),
            _ => None,
        }
    }

    pub fn key(&mut self, k: &KeyEvent, sys: &mut dyn Platform) -> Out {
        if !self.covers() || !k.down {
            return Out::None;
        }
        if self.shut {
            if k.key == Key::Escape {
                self.set_shut(false);
            }
            return Out::None;
        }
        if self.job.is_some() {
            return Out::None;
        }
        let multi = self.mode == Mode::Pick && self.users.len() > 1;
        match k.key {
            Key::Tab => self.move_focus(!k.shift),
            Key::Enter => return self.activate(self.focus, sys),
            Key::Up | Key::Left if self.choosing() => self.move_opt(false),
            Key::Down | Key::Right if self.choosing() => self.move_opt(true),
            Key::Escape => match self.mode {
                Mode::Recover => self.cancel_recover(),
                Mode::Setup if self.fresh && self.step == Step::DiskKey => self.to_choice(),
                Mode::Disk if self.confirm_skip => self.set_confirm(false),
                Mode::Setup => {}
                _ => {
                    if !self.pw.is_empty() {
                        wipe_str(&mut self.pw);
                        self.dmg_item(Item::Pw);
                    }
                }
            },
            Key::Left if multi => self.select((self.sel + self.users.len() - 1) % self.users.len()),
            Key::Right if multi => self.select((self.sel + 1) % self.users.len()),
            Key::Backspace => {
                let f = self.focus;
                if self.waiting() && f == Item::Pw {
                    return Out::None;
                }
                if let Some(t) = self.text_mut(f) {
                    if t.pop().is_some() {
                        self.after_edit(f);
                    }
                }
            }
            _ => {
                let f = self.focus;
                let typed = k.ch.filter(|c| !c.is_control() && !k.ctrl && !k.alt);
                if matches!(f, Item::Name | Item::Pw | Item::Pw2 | Item::Code) {
                    if let Some(c) = typed {
                        if self.waiting() && f == Item::Pw {
                            return Out::None;
                        }
                        let max = if f == Item::Name { accounts::MAX_NAME } else { MAX_PW };
                        if let Some(t) = self.text_mut(f) {
                            if t.chars().count() < max {
                                t.push(c);
                                self.after_edit(f);
                            }
                        }
                    }
                } else if k.key == Key::Char(' ') {
                    return self.activate(f, sys);
                }
            }
        }
        Out::None
    }

    fn after_edit(&mut self, f: Item) {
        self.caret = true;
        self.next_blink = self.now + 530;
        let had = self.err.take();
        match had {
            // El error de campo desaparece; el mensaje de clave incorrecta también.
            Some(_) => self.dmg_panel(),
            None => self.dmg_item(f),
        }
        self.weak_ok = false;
        self.warned = false;
        if f == Item::Pw && matches!(self.mode, Mode::Setup | Mode::Recover) {
            self.dmg_strength();
        }
    }

    /// Repinta el indicador de fortaleza (a la derecha de la etiqueta del campo de clave).
    fn dmg_strength(&mut self) {
        let lh = font::line_h(8);
        let r = self.layout().items.iter().find(|i| i.0 == Item::Pw).map(|i| i.1);
        if let Some(r) = r {
            self.dmg(Rect::new(r.right() - STRENGTH_W, r.y - lh - 2, STRENGTH_W, lh).inflate(1));
        }
    }

    /// Enter, espacio o clic sobre `it`.
    fn activate(&mut self, it: Item, sys: &mut dyn Platform) -> Out {
        match self.mode {
            Mode::Setup if self.fresh && self.step != Step::Account => match (self.step, it) {
                (Step::Choice, Item::Opt(i)) => {
                    self.set_opt(i);
                    Out::None
                }
                (Step::Choice, _) => self.choice_next(),
                (Step::DiskKey, Item::Done) => {
                    self.to_choice();
                    Out::None
                }
                (Step::DiskKey, _) => self.diskkey_next(),
                (Step::Codes, _) => self.code_next(),
                _ => Out::None,
            },
            Mode::Setup => match it {
                Item::Admin => {
                    self.admin = !self.admin;
                    self.dmg_item(Item::Admin);
                    Out::None
                }
                Item::Done => self.finish(sys),
                _ => self.submit(sys),
            },
            Mode::Recover => {
                if it == Item::Done {
                    self.cancel_recover();
                    Out::None
                } else {
                    self.recover_submit()
                }
            }
            Mode::Disk => match it {
                Item::Link => {
                    self.toggle_recovery();
                    Out::None
                }
                Item::Skip => {
                    self.set_confirm(true);
                    Out::None
                }
                Item::Back => {
                    self.set_confirm(false);
                    Out::None
                }
                Item::Done => {
                    sys.files().skip_locked_disk();
                    self.wipe_all();
                    Out::DiskDone
                }
                _ => self.disk_submit(),
            },
            _ if it == Item::Link => {
                self.start_recover();
                Out::None
            }
            _ => self.attempt(sys),
        }
    }

    // ---------------------------------------------------------------- disco cifrado y recuperación

    /// Muestra un mensaje bajo los botones (rojo, o verde si es una buena noticia).
    fn say(&mut self, text: &str, good: bool) {
        self.err = Some((Item::Msg, String::from(text)));
        self.good = good;
        self.dmg_panel();
    }

    fn set_confirm(&mut self, on: bool) {
        self.dmg_panel();
        wipe_str(&mut self.pw);
        self.confirm_skip = on;
        self.err = None;
        self.focus = if on { Item::Back } else { Item::Pw };
        self.dmg_panel();
    }

    fn toggle_recovery(&mut self) {
        self.dmg_panel();
        wipe_str(&mut self.pw);
        self.recovery_mode = !self.recovery_mode;
        self.err = None;
        self.focus = Item::Pw;
        self.dmg_panel();
    }

    /// Lo escrito en el campo del disco, ya comprobado (sin derivar nada): el texto de error si no vale.
    fn check_disk_input(&self) -> Result<(), &'static str> {
        if !self.recovery_mode {
            return if self.pw.is_empty() { Err("Escribe la clave del disco.") } else { Ok(()) };
        }
        check_code(&self.pw)
    }

    fn disk_submit(&mut self) -> Out {
        if self.waiting() || self.job.is_some() {
            return Out::None;
        }
        if let Err(e) = self.check_disk_input() {
            self.say(e, false);
            return Out::None;
        }
        self.err = None;
        self.good = false;
        self.job = Some((Job::Disk, self.now + JOB_DELAY_MS));
        self.dmg_item(Item::Msg);
        Out::None
    }

    /// Resultado de abrir el disco. Solo una clave equivocada cuenta para la espera creciente.
    fn disk_result(&mut self, res: Result<(), KeyError>) -> Out {
        match res {
            Ok(()) => {
                self.fails = 0;
                self.until = None;
                self.err = None;
                self.dmg_all();
                Out::DiskDone
            }
            Err(e) => {
                if e == KeyError::WrongSecret {
                    self.fail();
                }
                self.say(e.text(), false);
                Out::None
            }
        }
    }

    fn run_job(&mut self, job: Job, sys: &mut dyn Platform) -> Out {
        let now = sys.now();
        match job {
            Job::Disk => {
                let recovery = self.recovery_mode;
                let mut typed = core::mem::replace(&mut self.pw, secret());
                let mut norm = if recovery { keys::normalize_recovery(&typed).unwrap_or_default() } else { String::new() };
                let res = sys.files().unlock_disk(if recovery { &norm } else { &typed }, recovery, now);
                wipe_str(&mut typed);
                wipe_str(&mut norm);
                self.disk_result(res)
            }
            Job::Provision => match self.provision(sys) {
                Ok(()) if self.codes.is_empty() => Out::SetupDone,
                Ok(()) => {
                    self.step = Step::Codes;
                    self.code_i = 0;
                    self.focus = Item::Ok;
                    self.dmg_all();
                    Out::None
                }
                Err(e) => {
                    self.fail_provision(e);
                    Out::None
                }
            },
            Job::Recover => {
                let name = self.users.get(self.sel).map(|u| u.0.clone()).unwrap_or_default();
                let mut norm = keys::normalize_recovery(&self.code).unwrap_or_default();
                let mut newpw = core::mem::replace(&mut self.pw, secret());
                let res = with_pool(sys, |f, pool| f.recover_account(&name, &norm, &newpw, pool, now));
                wipe_str(&mut norm);
                wipe_str(&mut newpw);
                self.wipe_all();
                self.recover_result(res);
                Out::None
            }
        }
    }

    /// Resultado de recuperar la cuenta: vuelve al inicio de sesión con un aviso, o muestra el error.
    fn recover_result(&mut self, res: Result<(), KeyError>) {
        match res {
            Ok(()) => {
                self.enter(Mode::Pick);
                self.focus = self.order().first().copied().unwrap_or(Item::Ok);
                self.say("Clave cambiada. Ya puedes iniciar sesión.", true);
            }
            Err(e) => {
                self.focus = if e == KeyError::Weak { Item::Pw } else { Item::Code };
                self.say(e.text(), false);
            }
        }
    }

    fn start_recover(&mut self) {
        if self.can_recover() {
            self.enter(Mode::Recover);
            self.focus = Item::Code;
        }
    }

    fn cancel_recover(&mut self) {
        self.enter(Mode::Pick);
        self.focus = self.order().first().copied().unwrap_or(Item::Ok);
    }

    fn recover_submit(&mut self) -> Out {
        if self.job.is_some() {
            return Out::None;
        }
        if let Err(e) = check_code(&self.code) {
            return self.fail_at(Item::Code, e);
        }
        if self.pw != self.pw2 {
            return self.fail_at(Item::Pw2, "Las claves no coinciden.");
        }
        self.err = None;
        self.good = false;
        self.job = Some((Job::Recover, self.now + JOB_DELAY_MS));
        self.dmg_item(Item::Msg);
        Out::None
    }

    // ---------------------------------------------------------------- asistente

    fn fail_at(&mut self, it: Item, e: &str) -> Out {
        self.err = Some((it, String::from(e)));
        self.set_focus(it);
        self.dmg_panel();
        Out::None
    }

    fn submit(&mut self, sys: &mut dyn Platform) -> Out {
        if self.fresh {
            return self.submit_fresh();
        }
        self.err = None;
        if !accounts::valid_user_name(&self.name) {
            return self.fail_at(Item::Name, AccountError::BadName.text());
        }
        if self.pw != self.pw2 {
            return self.fail_at(Item::Pw2, "Las claves no coinciden.");
        }
        let (name, admin) = (self.name.clone(), self.admin || !self.second);
        let salt = accounts::salt_from(sys.entropy(), sys.entropy());
        let now = sys.now();
        let pw = if self.pw.is_empty() { None } else { Some(self.pw.as_str()) };
        let mut res = sys.files().create_account(&name, pw, admin, salt, now);
        if res.is_ok() && !self.second {
            // La sesión de la primera cuenta (administradora) permite crear las demás.
            res = sys.files().login(&name, &self.pw);
        }
        match res {
            Err(AccountError::WeakPassword) => self.fail_at(Item::Pw, AccountError::WeakPassword.text()),
            Err(e) => self.fail_at(Item::Name, e.text()),
            Ok(()) => {
                self.created.push((name, admin));
                self.second = true;
                self.name.clear();
                self.admin = false;
                self.wipe_all();
                self.focus = Item::Name;
                self.dmg_all();
                Out::None
            }
        }
    }

    fn finish(&mut self, sys: &mut dyn Platform) -> Out {
        if self.fresh {
            return self.finish_fresh();
        }
        sys.files().logout();
        self.wipe_all();
        Out::SetupDone
    }

    // ---------------------------------------------------------------- asistente de disco nuevo

    fn choosing(&self) -> bool {
        self.mode == Mode::Setup && self.fresh && self.step == Step::Choice && self.job.is_none()
    }

    fn opt_index(&self) -> usize {
        Enc::ALL.iter().position(|&e| e == self.enc).unwrap_or(0)
    }

    fn set_opt(&mut self, i: usize) {
        if let Some(&e) = Enc::ALL.get(i) {
            if e != self.enc {
                self.enc = e;
                self.dmg_panel();
            }
        }
    }

    fn move_opt(&mut self, fwd: bool) {
        let n = Enc::ALL.len();
        let i = self.opt_index();
        self.set_opt(if fwd { (i + 1) % n } else { (i + n - 1) % n });
    }

    /// Vuelve a la elección de cifrado (borra la clave del disco).
    fn to_choice(&mut self) {
        self.dmg_panel();
        self.wipe_all();
        wipe_str(&mut self.disk_pw);
        self.step = Step::Choice;
        self.err = None;
        self.weak_ok = false;
        self.focus = Item::Ok;
        self.dmg_all();
    }

    /// "Siguiente" en la elección: clave del disco si hace falta, o directamente las cuentas.
    fn choice_next(&mut self) -> Out {
        self.dmg_panel();
        self.wipe_all();
        wipe_str(&mut self.disk_pw);
        self.err = None;
        self.weak_ok = false;
        if self.enc.disk() {
            self.step = Step::DiskKey;
            self.focus = Item::Pw;
        } else {
            self.step = Step::Account;
            self.focus = Item::Name;
        }
        self.dmg_all();
        Out::None
    }

    /// "Siguiente" en la clave del disco: longitud, coincidencia y, si es débil, un aviso antes de seguir.
    fn diskkey_next(&mut self) -> Out {
        self.err = None;
        if self.pw.chars().count() < accounts::MIN_PASSWORD {
            return self.fail_at(Item::Pw, KeyError::Weak.text());
        }
        if self.pw != self.pw2 {
            return self.fail_at(Item::Pw2, "Las claves no coinciden.");
        }
        if keys::strength(&self.pw) < 2 && !self.weak_ok {
            self.weak_ok = true;
            self.say("La clave es débil. Púlsalo otra vez para usarla igualmente.", false);
            return Out::None;
        }
        self.disk_pw = core::mem::replace(&mut self.pw, secret());
        self.wipe_all();
        self.weak_ok = false;
        self.step = Step::Account;
        self.focus = Item::Name;
        self.dmg_all();
        Out::None
    }

    /// Una cuenta del asistente de disco nuevo: se valida ahora y se guarda en memoria hasta el final.
    fn submit_fresh(&mut self) -> Out {
        self.err = None;
        if !accounts::valid_user_name(&self.name) {
            return self.fail_at(Item::Name, AccountError::BadName.text());
        }
        if self.pend.iter().any(|p| crate::fs::same(&p.name, &self.name)) {
            return self.fail_at(Item::Name, AccountError::Exists.text());
        }
        if self.pw != self.pw2 {
            return self.fail_at(Item::Pw2, "Las claves no coinciden.");
        }
        let n = self.pw.chars().count();
        if n > 0 && n < accounts::MIN_PASSWORD {
            return self.fail_at(Item::Pw, AccountError::WeakPassword.text());
        }
        if n == 0 && self.enc.home() {
            if !self.second {
                return self.fail_at(Item::Pw, "Hace falta una clave para cifrar tu carpeta.");
            }
            if !self.warned {
                self.warned = true;
                return self.fail_at(Item::Pw, "Su carpeta quedará sin cifrar. Pulsa otra vez para añadirla igualmente.");
            }
        }
        let admin = self.admin || !self.second;
        let name = core::mem::take(&mut self.name);
        let pw = core::mem::replace(&mut self.pw, secret());
        self.created.push((name.clone(), admin));
        self.pend.push(PendAcc { name, pw, admin, created: false, coded: false });
        self.second = true;
        self.admin = false;
        self.warned = false;
        self.wipe_all();
        self.focus = Item::Name;
        self.dmg_all();
        Out::None
    }

    /// "Terminar": con cifrado pide la ventana del ratón (una vez); sin él, prepara directamente.
    fn finish_fresh(&mut self) -> Out {
        if self.enc == Enc::Nothing {
            self.begin_provision();
            return Out::None;
        }
        // La ventana del ratón es un diálogo del escritorio: esta pantalla se aparta mientras tanto.
        self.mode = Mode::Boot;
        self.job = None;
        self.dmg_all();
        Out::NeedEntropy
    }

    /// Hace todo el trabajo: formatea (cifrado o no), crea las cuentas y cifra sus carpetas. Se puede
    /// repetir tras un fallo: lo ya hecho (`disk_done`, `created`, `coded`) no se vuelve a hacer.
    fn provision(&mut self, sys: &mut dyn Platform) -> Result<(), String> {
        let now = sys.now();
        if !self.disk_done {
            if self.enc.disk() {
                let pw = &self.disk_pw;
                let code = with_pool(sys, |f, pool| f.format_pending_encrypted(pw, pool, now)).map_err(|e| String::from(e.text()))?;
                self.codes.push((String::from("del disco"), code));
            } else {
                sys.files().format_pending(now).map_err(|e| String::from(e.message()))?;
            }
            self.disk_done = true;
        }
        for i in 0..self.pend.len() {
            if !self.pend[i].created {
                let salt = accounts::salt_from(sys.entropy(), sys.entropy());
                let p = &self.pend[i];
                let pw = if p.pw.is_empty() { None } else { Some(p.pw.as_str()) };
                sys.files().create_account(&p.name, pw, p.admin, salt, now).map_err(|e| String::from(e.text()))?;
                self.pend[i].created = true;
            }
            if self.enc.home() && !self.pend[i].coded && !self.pend[i].pw.is_empty() {
                let p = &self.pend[i];
                let code = with_pool(sys, |f, pool| f.encrypt_new_home(&p.name, &p.pw, pool, now)).map_err(|e| String::from(e.text()))?;
                self.pend[i].coded = true;
                let owner = format!("de la cuenta {}", self.pend[i].name);
                self.codes.push((owner, code));
            }
            // La sesión de la primera cuenta (administradora) permite crear las demás.
            if i == 0 && self.pend.len() > 1 && sys.files().user().is_none() {
                let p = &self.pend[0];
                sys.files().login(&p.name, &p.pw).map_err(|e| String::from(e.text()))?;
            }
        }
        sys.files().logout();
        wipe_str(&mut self.disk_pw);
        for p in self.pend.iter_mut() {
            wipe_str(&mut p.pw);
        }
        Ok(())
    }

    /// Solo para pruebas: pantalla del asistente de disco nuevo.
    #[cfg(test)]
    pub(crate) fn step_name(&self) -> &'static str {
        match self.step {
            Step::Account => "cuentas",
            Step::Choice => "cifrado",
            Step::DiskKey => "clave del disco",
            Step::Preparing => "preparando",
            Step::Codes => "codigos",
        }
    }

    /// Solo para pruebas: (a quién pertenece, código) del código que se muestra.
    #[cfg(test)]
    pub(crate) fn shown_code(&self) -> Option<(String, String)> {
        self.codes.get(self.code_i).cloned()
    }

    /// Un fallo al preparar: sin nada hecho vuelve a la elección de cifrado; si el disco ya se formateó o
    /// hay cuentas creadas, a la tarjeta de cuentas (para reintentar con "Terminar", que retoma lo que falta).
    fn fail_provision(&mut self, e: String) {
        let done = self.disk_done || self.pend.iter().any(|p| p.created);
        self.mode = Mode::Setup;
        if done {
            self.step = Step::Account;
            self.focus = Item::Name;
        } else {
            for p in self.pend.iter_mut() {
                wipe_str(&mut p.pw);
            }
            self.pend.clear();
            self.created.clear();
            self.second = false;
            self.step = Step::Choice;
            self.focus = Item::Ok;
        }
        self.err = Some((Item::Msg, e));
        self.good = false;
        self.dmg_all();
    }

    /// "Lo he apuntado": siguiente código; tras el último sigue el arranque normal.
    fn code_next(&mut self) -> Out {
        if let Some(c) = self.codes.get_mut(self.code_i) {
            wipe_str(&mut c.1);
        }
        self.code_i += 1;
        if self.code_i >= self.codes.len() {
            self.codes.clear();
            self.code_i = 0;
            self.dmg_all();
            return Out::SetupDone;
        }
        self.dmg_all();
        Out::None
    }

    // ---------------------------------------------------------------- inicio de sesión y bloqueo

    fn attempt(&mut self, sys: &mut dyn Platform) -> Out {
        if self.waiting() || self.users.is_empty() {
            return Out::None;
        }
        let name = self.users[self.sel].0.clone();
        let res = if self.mode == Mode::Lock {
            if sys.files().verify_password(&self.pw) {
                Ok(())
            } else {
                Err(AccountError::WrongPassword)
            }
        } else {
            sys.files().login(&name, &self.pw)
        };
        wipe_str(&mut self.pw);
        match res {
            Ok(()) => {
                self.fails = 0;
                self.until = None;
                self.err = None;
                if self.mode == Mode::Lock {
                    Out::Unlocked
                } else {
                    Out::Entered
                }
            }
            Err(e) => {
                if e == AccountError::WrongPassword {
                    self.fail();
                }
                self.say(if e == AccountError::WrongPassword { "La clave no es correcta." } else { e.text() }, false);
                Out::None
            }
        }
    }

    // ---------------------------------------------------------------- dibujo

    pub fn paint(&self, s: &mut Surface, accent: u32) {
        if !self.covers() {
            return;
        }
        let l = self.layout();
        match self.mode {
            Mode::Setup if self.fresh && self.step != Step::Account => self.paint_step(s, &l, accent),
            Mode::Setup | Mode::Recover => self.paint_setup(s, &l, accent),
            Mode::Disk => self.paint_disk(s, &l, accent),
            _ => self.paint_pick(s, &l, accent),
        }
        let c = self.clock_rect();
        if c.intersects(s.clip()) {
            let t = &self.clock;
            text_in(s, Rect::new(c.x, c.y, c.w, 46), &format!("{:02}:{:02}", t.hour, t.minute), 22, true, 0xFFFFFF, Align::Left);
            text_in(s, Rect::new(c.x, c.y + 48, c.w, 24), &format!("{}/{}/{}", t.day, t.month, t.year), 9, false, GREY, Align::Left);
        }
        let p = self.power_rect();
        if p.intersects(s.clip()) {
            if self.hot == Some(Item::Power) || self.shut {
                s.fill_rect(p, 0x3A3A3A);
            }
            icons::draw(s, Icon::Power, p.x + 12, p.y + 12, 24, 0xFFFFFF);
        }
        if self.shut {
            let r = self.shut_rect();
            s.fill_rect(r, 0x2B2B2B);
            for (i, name) in SHUT_ROWS.iter().enumerate() {
                let row = self.shut_row(i);
                if self.hot == Some(Item::Shut(i)) {
                    s.fill_rect(row, 0x4A4A4A);
                }
                text_in(s, Rect::new(row.x + 16, row.y, row.w - 16, row.h), name, 9, false, 0xFFFFFF, Align::Left);
            }
            s.frame(r, blend(0x2B2B2B, 0xFFFFFF, 40));
        }
    }

    /// El campo muestra su texto (nombre, código de recuperación) en vez de puntos.
    fn plain(&self, it: Item) -> bool {
        it == Item::Name || it == Item::Code || (it == Item::Pw && self.mode == Mode::Disk && self.recovery_mode)
    }

    fn buf(&self, it: Item) -> &str {
        match it {
            Item::Name => &self.name,
            Item::Code => &self.code,
            Item::Pw => &self.pw,
            _ => &self.pw2,
        }
    }

    fn placeholder(&self, it: Item) -> Option<&'static str> {
        match (self.mode, it) {
            (Mode::Disk, Item::Pw) => Some(if self.recovery_mode { "Código de recuperación" } else { "Clave del disco" }),
            (Mode::Pick | Mode::Lock, Item::Pw) => Some("Clave"),
            _ => None,
        }
    }

    fn field(&self, s: &mut Surface, r: Rect, it: Item, accent: u32) {
        if !r.intersects(s.clip()) {
            return;
        }
        let focused = self.focus == it;
        s.fill_rect(r, 0xFFFFFF);
        s.frame(r, if focused { accent } else { 0x8A8A8A });
        let inner = Rect::new(r.x + 8, r.y, r.w - 16, r.h);
        let end;
        let text = self.buf(it);
        if self.plain(it) {
            let w = font::width(text, 9, false);
            text_in(s, inner, text, 9, false, 0x000000, if w > inner.w { Align::Right } else { Align::Left });
            end = inner.x + w.min(inner.w);
        } else {
            // Puntos: la clave nunca se muestra.
            let n = text.chars().count();
            let fit_n = ((inner.w - 8) / 11).max(1) as usize;
            let shown = n.min(fit_n);
            for i in 0..shown {
                s.fill_circle((inner.x + 5 + i as i32 * 11) as f32, (r.y + r.h / 2) as f32, 3.2, 0x000000, 255);
            }
            end = inner.x + shown as i32 * 11;
        }
        if text.is_empty() && !focused {
            if let Some(p) = self.placeholder(it) {
                text_in(s, inner, p, 9, false, 0x8A8A8A, Align::Left);
            }
        }
        if focused && self.caret {
            s.fill_rect(Rect::new(end + 1, r.y + 6, 1, r.h - 12), 0x000000);
        }
    }

    /// Enlace de texto centrado en `r`, subrayado al pasar el ratón o con el foco.
    fn link(&self, s: &mut Surface, r: Rect, it: Item, text: &str) {
        if !r.intersects(s.clip()) {
            return;
        }
        let lit = self.hot == Some(it) || self.focus == it;
        let color = if lit { 0xFFFFFF } else { 0xCFCFCF };
        text_in(s, Rect::new(r.x + 1, r.y + 1, r.w, r.h), text, 9, false, 0x000000, Align::Center);
        text_in(s, r, text, 9, false, color, Align::Center);
        if lit {
            let w = font::width(text, 9, false).min(r.w);
            s.fill_rect(Rect::new(r.x + (r.w - w) / 2, r.y + r.h - 3, w, 1), color);
        }
    }

    /// Indicador de fortaleza: cuatro tramos y la palabra, a la derecha de la etiqueta del campo `r`.
    fn paint_strength(&self, s: &mut Surface, r: Rect) {
        if self.pw.is_empty() {
            return;
        }
        let lh = font::line_h(8);
        let area = Rect::new(r.right() - STRENGTH_W, r.y - lh - 2, STRENGTH_W, lh);
        if !area.intersects(s.clip()) {
            return;
        }
        let lv = (keys::strength(&self.pw) as usize).min(4);
        let (name, color) = STRENGTH[lv];
        for i in 0..4 {
            let seg = Rect::new(area.x + i * 22, area.y + lh / 2 - 3, 18, 6);
            s.fill_rect(seg, if (i as usize) < lv.max(1) { color } else { 0x4A4A4A });
        }
        text_in(s, Rect::new(area.x + 96, area.y, STRENGTH_W - 96, lh), name, 8, false, color, Align::Right);
    }

    fn button(&self, s: &mut Surface, r: Rect, it: Item, text: &str, accent: u32, primary: bool) {
        if !r.intersects(s.clip()) {
            return;
        }
        let hot = self.hot == Some(it);
        let bg = match (primary, hot) {
            (true, true) => lighten(accent, 45),
            (true, false) => accent,
            (false, true) => 0x4A4A4A,
            (false, false) => 0x3A3A3A,
        };
        s.fill_rect(r, bg);
        if self.focus == it {
            s.frame(r, 0xFFFFFF);
            s.frame(r.inflate(-1), 0x000000);
        }
        text_in(s, r, text, 9, false, 0xFFFFFF, Align::Center);
    }

    fn paint_setup(&self, s: &mut Surface, l: &Layout, accent: u32) {
        let c = l.card;
        s.fill_rect(c, CARD_BG);
        s.frame(c, 0x383838);
        let lh = font::line_h(8);
        let inner = c.w - 2 * PAD;
        let rec = self.mode == Mode::Recover;
        let title = if rec {
            let who = self.users.get(self.sel).map(|u| u.0.as_str()).unwrap_or("");
            fit(&format!("Recuperar la cuenta de {who}"), 14, inner)
        } else if self.second {
            String::from("¿Añadir otra cuenta?")
        } else {
            String::from("Crea tu cuenta")
        };
        text_in(s, l.title, &title, 14, true, 0xFFFFFF, Align::Left);
        let bx = c.x + PAD;
        if rec {
            for (i, line) in wrap(RECOVER_SUB, 8, inner).iter().enumerate() {
                text_in(s, Rect::new(bx, l.body_y + i as i32 * lh, inner, lh), line, 8, false, GREY, Align::Left);
            }
        } else if self.second {
            text_in(s, Rect::new(bx, l.body_y, inner, lh), "Cuentas creadas", 8, false, GREY, Align::Left);
            let rh = font::line_h(9) + 2;
            for (i, (n, admin)) in self.created.iter().take(MAX_LIST).enumerate() {
                let t = if *admin { format!("{n} (administrador)") } else { n.clone() };
                text_in(s, Rect::new(bx, l.body_y + lh + 2 + i as i32 * rh, inner, rh), &fit(&t, 9, inner), 9, false, 0xFFFFFF, Align::Left);
            }
            if self.created.len() > MAX_LIST {
                let t = format!("y {} más", self.created.len() - MAX_LIST);
                text_in(s, Rect::new(bx, l.body_y + lh + 2 + MAX_LIST as i32 * rh, inner, rh), &t, 9, false, GREY, Align::Left);
            }
        } else {
            for (i, line) in wrap(SUB, 8, inner).iter().enumerate() {
                text_in(s, Rect::new(bx, l.body_y + i as i32 * lh, inner, lh), line, 8, false, GREY, Align::Left);
            }
        }
        for &(it, r) in &l.items {
            match it {
                Item::Name | Item::Code | Item::Pw | Item::Pw2 => {
                    let label = match (it, rec) {
                        (Item::Name, _) => "Nombre de usuario",
                        (Item::Code, _) => "Código de recuperación",
                        (Item::Pw, true) => "Clave nueva (mínimo 6 caracteres)",
                        (Item::Pw, false) if self.fresh && self.enc.home() && !self.second => "Clave (obligatoria, mínimo 6 caracteres)",
                        (Item::Pw, false) if self.fresh && self.enc.home() => "Clave (opcional: sin clave no se cifra su carpeta)",
                        (Item::Pw, false) => "Clave (opcional, mínimo 6 caracteres)",
                        (_, true) => "Repite la clave nueva",
                        _ => "Repite la clave",
                    };
                    text_in(s, Rect::new(r.x, r.y - lh - 2, r.w - if it == Item::Pw { STRENGTH_W } else { 0 }, lh), label, 8, false, GREY, Align::Left);
                    self.field(s, r, it, accent);
                    if it == Item::Pw {
                        self.paint_strength(s, r);
                    }
                    if let Some((_, e)) = self.err.as_ref().filter(|e| e.0 == it) {
                        text_in(s, Rect::new(r.x, r.bottom() + 2, r.w, lh), e, 8, false, RED, Align::Left);
                    }
                }
                Item::Admin => {
                    let b = Rect::new(r.x, r.y + 3, 16, 16);
                    s.fill_rect(b, if self.admin { accent } else { 0xFFFFFF });
                    s.frame(b, if self.focus == Item::Admin { lighten(accent, 90) } else { 0x8A8A8A });
                    if self.admin {
                        s.stroke(&[(b.x as f32 + 3.5, b.y as f32 + 8.5), (b.x as f32 + 6.5, b.y as f32 + 11.5), (b.x as f32 + 12.5, b.y as f32 + 4.5)], 1.6, 0xFFFFFF);
                    }
                    text_in(s, Rect::new(r.x + 26, r.y, r.w - 26, r.h), "Administrador", 9, false, 0xFFFFFF, Align::Left);
                }
                Item::Ok => self.button(s, r, it, if rec { "Cambiar clave" } else if self.second { "Añadir" } else { "Crear cuenta" }, accent, true),
                Item::Done => self.button(s, r, it, if rec { "Cancelar" } else { "Terminar" }, accent, false),
                Item::Msg => {
                    if let Some(t) = self.msg_text() {
                        text_wrapped(s, r, &t, if self.good { GREEN } else { RED });
                    }
                }
                _ => {}
            }
        }
    }

    /// Botón de opción: círculo vacío o con punto.
    fn radio(s: &mut Surface, cx: f32, cy: f32, on: bool, accent: u32) {
        s.fill_circle(cx, cy, 8.0, if on { accent } else { 0x8A8A8A }, 255);
        s.fill_circle(cx, cy, 6.3, CARD_BG, 255);
        if on {
            s.fill_circle(cx, cy, 3.8, accent, 255);
        }
    }

    /// Tarjetas del asistente de disco nuevo: cifrado, clave del disco, preparando y código de recuperación.
    fn paint_step(&self, s: &mut Surface, l: &Layout, accent: u32) {
        let c = l.card;
        s.fill_rect(c, CARD_BG);
        s.frame(c, 0x383838);
        let lh = font::line_h(8);
        let inner = c.w - 2 * PAD;
        let bx = c.x + PAD;
        let title = match self.step {
            Step::Choice => "¿Quieres cifrar tus datos?",
            Step::DiskKey => "Clave del disco",
            Step::Preparing => "Preparando tu equipo",
            _ => "Código de recuperación",
        };
        text_in(s, l.title, title, 14, true, 0xFFFFFF, Align::Left);
        let lines = |s: &mut Surface, text: &str, y: i32, color: u32| {
            for (i, line) in wrap(text, 8, inner).iter().enumerate() {
                text_in(s, Rect::new(bx, y + i as i32 * lh, inner, lh), line, 8, false, color, Align::Left);
            }
        };
        match self.step {
            Step::Choice => {
                lines(s, CHOICE_SUB, l.body_y, GREY);
                let last = l.items.iter().filter(|i| matches!(i.0, Item::Opt(_))).map(|i| i.1.bottom()).max().unwrap_or(l.body_y);
                lines(s, CHOICE_WARN, last + 6, AMBER);
            }
            Step::DiskKey => lines(s, DISKKEY_SUB, l.body_y, GREY),
            Step::Preparing => {
                text_in(s, Rect::new(bx, l.body_y, inner, font::line_h(9)), "Preparando…", 9, true, 0xFFFFFF, Align::Left);
                let t = if self.enc == Enc::Nothing { "Un momento." } else { "Generar las llaves tarda un poco. No apagues el equipo." };
                text_in(s, Rect::new(bx, l.body_y + font::line_h(9) + 6, inner, lh), t, 8, false, GREY, Align::Left);
            }
            _ => {
                let t = format!("Código {} de {} · {}", self.code_i + 1, self.codes.len(), self.code_owner());
                text_in(s, Rect::new(bx, l.body_y, inner, font::line_h(9)), &fit(&t, 9, inner), 9, true, 0xFFFFFF, Align::Left);
                let b = Self::code_box(c, l.body_y);
                s.fill_rect(b, 0x121212);
                s.frame(b, 0x4A4A4A);
                let code = self.codes.get(self.code_i).map(|x| x.1.as_str()).unwrap_or("");
                let size = (8..=14u32).rev().find(|&z| font::width(code, z, true) <= b.w - 16).unwrap_or(8);
                text_in(s, b, code, size, true, 0xFFFFFF, Align::Center);
                lines(s, CODE_WARN, b.bottom() + 14, AMBER);
            }
        }
        for &(it, r) in &l.items {
            match it {
                Item::Opt(i) => {
                    if !r.intersects(s.clip()) {
                        continue;
                    }
                    let on = self.opt_index() == i;
                    if on {
                        s.fill_rect(r, 0x2B2B2B);
                        s.frame(r, accent);
                    } else if self.hot == Some(it) {
                        s.fill_rect(r, 0x262626);
                    }
                    Self::radio(s, (r.x + 20) as f32, (r.y + r.h / 2) as f32, on, accent);
                    let lab = OPTS[i].0;
                    text_in(s, Rect::new(r.x + 40, r.y + 6, r.w - 48, font::line_h(9)), lab, 9, true, 0xFFFFFF, Align::Left);
                    text_in(s, Rect::new(r.x + 40, r.y + 8 + font::line_h(9), r.w - 48, lh), &fit(OPTS[i].1, 8, r.w - 48), 8, false, GREY, Align::Left);
                }
                Item::Pw | Item::Pw2 => {
                    let label = if it == Item::Pw { "Clave del disco (mínimo 6 caracteres)" } else { "Repite la clave del disco" };
                    text_in(s, Rect::new(r.x, r.y - lh - 2, r.w - if it == Item::Pw { STRENGTH_W } else { 0 }, lh), label, 8, false, GREY, Align::Left);
                    self.field(s, r, it, accent);
                    if it == Item::Pw {
                        self.paint_strength(s, r);
                    }
                    if let Some((_, e)) = self.err.as_ref().filter(|e| e.0 == it) {
                        text_in(s, Rect::new(r.x, r.bottom() + 2, r.w, lh), e, 8, false, RED, Align::Left);
                    }
                }
                Item::Ok => {
                    let t = match self.step {
                        Step::Codes => "Lo he apuntado",
                        Step::DiskKey if self.weak_ok => "Usar igualmente",
                        Step::Preparing => "Preparando…",
                        _ => "Siguiente",
                    };
                    self.button(s, r, it, t, accent, true);
                }
                Item::Done => self.button(s, r, it, "Atrás", accent, false),
                Item::Msg => {
                    if let Some(t) = self.msg_text() {
                        text_wrapped(s, r, &t, if self.good { GREEN } else if self.weak_ok { AMBER } else { RED });
                    }
                }
                _ => {}
            }
        }
    }

    /// Candado dibujado por código (arco, cuerpo y ojo de la cerradura).
    fn draw_lock(&self, s: &mut Surface, r: Rect, accent: u32) {
        let (cx, top) = ((r.x + r.w / 2) as f32, r.y as f32);
        let c = top + 26.0;
        let arc = [(-15.0, 0.0), (-13.86, -5.74), (-10.61, -10.61), (-5.74, -13.86), (0.0, -15.0), (5.74, -13.86), (10.61, -10.61), (13.86, -5.74), (15.0, 0.0)];
        let mut pts: Vec<(f32, f32)> = alloc::vec![(cx - 15.0, c + 14.0)];
        pts.extend(arc.iter().map(|&(dx, dy)| (cx + dx, c + dy)));
        pts.push((cx + 15.0, c + 14.0));
        s.stroke(&pts, 6.0, 0xE4E4E4);
        let body = Rect::new(r.x + 6, r.y + 32, r.w - 12, 40);
        s.fill_rect(body, accent);
        s.frame(body, lighten(accent, 60));
        s.fill_circle(cx, top + 48.0, 5.5, 0x1C1C1C, 255);
        s.fill_rect(Rect::new(r.x + r.w / 2 - 2, r.y + 50, 4, 12), 0x1C1C1C);
    }

    fn paint_disk(&self, s: &mut Surface, l: &Layout, accent: u32) {
        if l.card.intersects(s.clip()) {
            self.draw_lock(s, l.card, accent);
        }
        let title = if self.confirm_skip { "¿Seguir sin disco?" } else { "Este disco está cifrado" };
        if l.title.intersects(s.clip()) {
            text_in(s, Rect::new(l.title.x + 1, l.title.y + 1, l.title.w, l.title.h), title, 14, true, 0x000000, Align::Center);
            text_in(s, l.title, title, 14, true, 0xFFFFFF, Align::Center);
        }
        let text = if self.confirm_skip { SKIP_TEXT } else { self.disk_sub() };
        let lh = font::line_h(9);
        for (i, line) in wrap(text, 9, 440).iter().enumerate() {
            let r = Rect::new(0, l.body_y + i as i32 * lh, self.w, lh);
            if r.intersects(s.clip()) {
                text_in(s, Rect::new(r.x + 1, r.y + 1, r.w, r.h), line, 9, false, 0x000000, Align::Center);
                text_in(s, r, line, 9, false, 0xE0E0E0, Align::Center);
            }
        }
        for &(it, r) in &l.items {
            if !r.intersects(s.clip()) {
                continue;
            }
            match it {
                Item::Pw => self.field(s, r, it, accent),
                Item::Ok => self.button(s, r, it, "Abrir", accent, true),
                Item::Link => self.link(s, r, it, if self.recovery_mode { "Usar la clave del disco" } else { "Usar el código de recuperación" }),
                Item::Skip => self.link(s, r, it, "Seguir sin disco"),
                Item::Done => self.button(s, r, it, "Seguir sin disco", accent, false),
                Item::Back => self.button(s, r, it, "Volver", accent, true),
                Item::Msg => {
                    if let Some(t) = self.msg_text() {
                        text_in(s, Rect::new(r.x + 1, r.y + 1, r.w, r.h), &t, 8, false, 0x000000, Align::Center);
                        text_in(s, r, &t, 8, false, if self.good { GREEN } else { RED }, Align::Center);
                    }
                }
                _ => {}
            }
        }
    }

    fn paint_pick(&self, s: &mut Surface, l: &Layout, accent: u32) {
        let title = if self.mode == Mode::Lock { "Pantalla bloqueada" } else { "Inicia sesión" };
        if l.title.intersects(s.clip()) {
            text_in(s, Rect::new(l.title.x + 1, l.title.y + 1, l.title.w, l.title.h), title, 14, true, 0x000000, Align::Center);
            text_in(s, l.title, title, 14, true, 0xFFFFFF, Align::Center);
        }
        for &(it, r) in &l.items {
            if !r.intersects(s.clip()) {
                continue;
            }
            match it {
                Item::Avatar(i) => {
                    let (name, _) = &self.users[i];
                    let (cx, cy) = ((r.x + r.w / 2) as f32, (r.y + AV / 2 + 6) as f32);
                    let on = i == self.sel;
                    if on {
                        s.fill_circle(cx, cy, AV as f32 / 2.0 + 5.0, accent, 255);
                    } else if self.hot == Some(it) {
                        s.fill_circle(cx, cy, AV as f32 / 2.0 + 5.0, 0xFFFFFF, 70);
                    }
                    let h = name.bytes().fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize));
                    s.fill_circle(cx, cy, AV as f32 / 2.0, AVATARS[h % AVATARS.len()], 255);
                    let initial: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                    text_in(s, Rect::new(r.x + r.w / 2 - 30, r.y + 6, 60, AV), &initial, 28, true, 0xFFFFFF, Align::Center);
                    let label = fit(name, 9, r.w - 8);
                    let t = Rect::new(r.x, r.y + AV + 14, r.w, font::line_h(9));
                    text_in(s, Rect::new(t.x + 1, t.y + 1, t.w, t.h), &label, 9, on, 0x000000, Align::Center);
                    text_in(s, t, &label, 9, on, if on { 0xFFFFFF } else { 0xD0D0D0 }, Align::Center);
                }
                Item::Pw => self.field(s, r, it, accent),
                Item::Link => self.link(s, r, it, "He olvidado mi clave"),
                Item::Ok => self.button(s, r, it, if self.mode == Mode::Lock { "Desbloquear" } else { "Entrar" }, accent, true),
                Item::Msg => {
                    if let Some(t) = self.msg_text() {
                        text_in(s, Rect::new(r.x + 1, r.y + 1, r.w, r.h), &t, 8, false, 0x000000, Align::Center);
                        text_in(s, r, &t, 8, false, if self.good && !self.waiting() { GREEN } else { RED }, Align::Center);
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::POOL_EVENTS_NEEDED;
    use crate::platform::TestPlatform;

    const CODE: &str = "ABCDEFGHJKMNPQRSTUVWXYZ023456789ABCDEFGH";

    fn sys() -> TestPlatform {
        TestPlatform { size: (1280, 800), ..Default::default() }
    }

    fn disk() -> (Login, TestPlatform) {
        let mut l = Login::new(1280, 800);
        l.start_disk();
        (l, sys())
    }

    fn press(l: &mut Login, sys: &mut TestPlatform, key: Key, ch: Option<char>) -> Out {
        l.key(&KeyEvent { key, ch, down: true, shift: false, ctrl: false, alt: false }, sys)
    }

    fn typ(l: &mut Login, sys: &mut TestPlatform, text: &str) {
        for c in text.chars() {
            press(l, sys, Key::Char(c.to_ascii_lowercase()), Some(c));
        }
    }

    fn rect(l: &Login, it: Item) -> Rect {
        l.layout().items.iter().find(|i| i.0 == it).unwrap().1
    }

    fn click(l: &mut Login, sys: &mut TestPlatform, it: Item) -> Out {
        let r = rect(l, it);
        l.click(r.x + r.w / 2, r.y + r.h / 2, sys)
    }

    fn msg(l: &Login) -> String {
        l.msg_text().unwrap_or_default()
    }

    #[test]
    fn disco_sin_clave_avisa_y_no_deriva_nada() {
        let (mut l, mut sys) = disk();
        assert!(l.covers());
        assert_eq!(press(&mut l, &mut sys, Key::Enter, None), Out::None);
        assert_eq!(msg(&l), "Escribe la clave del disco.");
        assert!(l.job.is_none());
    }

    #[test]
    fn disco_dice_comprobando_y_deriva_un_tick_despues() {
        let (mut l, mut sys) = disk();
        l.tick(100, &mut sys);
        typ(&mut l, &mut sys, "clavedisco");
        assert_eq!(press(&mut l, &mut sys, Key::Enter, None), Out::None);
        assert_eq!(msg(&l), "Comprobando…");
        // Mientras tanto no se acepta nada más.
        typ(&mut l, &mut sys, "x");
        assert_eq!(l.pw.chars().count(), 10);
        assert_eq!(l.tick(110, &mut sys), Out::None);
        assert!(l.job.is_some());
        // El disco de la plataforma de pruebas no está cifrado: error normal, que no cuenta como fallo.
        assert_eq!(l.tick(100 + JOB_DELAY_MS, &mut sys), Out::None);
        assert_eq!(msg(&l), KeyError::NotEncrypted.text());
        assert_eq!((l.fails, l.pw.len()), (0, 0));
    }

    #[test]
    fn disco_en_modo_codigo_valida_antes_de_derivar() {
        let (mut l, mut sys) = disk();
        assert!(!l.plain(Item::Pw));
        click(&mut l, &mut sys, Item::Link);
        assert!(l.recovery_mode && l.plain(Item::Pw));
        typ(&mut l, &mut sys, "abcd-efgh");
        press(&mut l, &mut sys, Key::Enter, None);
        assert_eq!(msg(&l), "El código debe tener 40 caracteres.");
        assert!(l.job.is_none());
        // Con 40 caracteres válidos (minúsculas, espacios y guiones incluidos) sí se pone en marcha.
        for _ in 0..9 {
            press(&mut l, &mut sys, Key::Backspace, None);
        }
        let spaced: String = CODE.to_lowercase().as_bytes().chunks(5).map(|c| core::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("-");
        typ(&mut l, &mut sys, &spaced);
        press(&mut l, &mut sys, Key::Enter, None);
        assert!(l.job.is_some());
        // Un carácter fuera del alfabeto se avisa aparte.
        assert!(check_code(&CODE.replace('A', "I")).is_err());
        assert_eq!(check_code(&CODE.replace('A', "I")), Err("El código tiene caracteres que no son válidos."));
        // Volver a la clave borra lo escrito.
        l.job = None;
        click(&mut l, &mut sys, Item::Link);
        assert!(!l.recovery_mode && l.pw.is_empty());
    }

    #[test]
    fn disco_espera_creciente_tras_tres_fallos() {
        let (mut l, mut sys) = disk();
        l.tick(1000, &mut sys);
        for _ in 0..2 {
            assert_eq!(l.disk_result(Err(KeyError::WrongSecret)), Out::None);
            assert!(!l.waiting());
        }
        // Otros errores no cuentan.
        l.disk_result(Err(KeyError::NotEncrypted));
        assert!(!l.waiting());
        l.disk_result(Err(KeyError::WrongSecret));
        assert!(l.waiting());
        assert!(msg(&l).starts_with("Demasiados intentos. Espera 5 s"));
        typ(&mut l, &mut sys, "x");
        assert!(l.pw.is_empty());
        assert_eq!(press(&mut l, &mut sys, Key::Enter, None), Out::None);
        assert!(l.job.is_none());
        l.tick(1000 + 5000, &mut sys);
        assert!(!l.waiting());
        typ(&mut l, &mut sys, "x");
        assert_eq!(l.pw.len(), 1);
        // Siguientes esperas: 15 s a los 6 fallos.
        for _ in 0..3 {
            l.disk_result(Err(KeyError::WrongSecret));
        }
        assert_eq!(l.last_secs, 15);
        // Acertar lo reinicia todo.
        l.until = None;
        assert_eq!(l.disk_result(Ok(())), Out::DiskDone);
        assert_eq!((l.fails, l.until), (0, None));
    }

    #[test]
    fn disco_seguir_sin_disco_pide_confirmacion() {
        let (mut l, mut sys) = disk();
        typ(&mut l, &mut sys, "algo");
        assert_eq!(click(&mut l, &mut sys, Item::Skip), Out::None);
        assert!(l.confirm_skip && l.pw.is_empty());
        assert_eq!(l.layout().items.iter().filter(|i| matches!(i.0, Item::Done | Item::Back)).count(), 2);
        // Volver (o Escape) no hace nada más.
        assert_eq!(click(&mut l, &mut sys, Item::Back), Out::None);
        assert!(!l.confirm_skip);
        click(&mut l, &mut sys, Item::Skip);
        press(&mut l, &mut sys, Key::Escape, None);
        assert!(!l.confirm_skip);
        // Confirmar sigue el arranque.
        click(&mut l, &mut sys, Item::Skip);
        assert_eq!(click(&mut l, &mut sys, Item::Done), Out::DiskDone);
    }

    #[test]
    fn disco_se_dibuja_en_los_dos_modos_y_con_confirmacion() {
        let (mut l, mut sys) = disk();
        let mut s = Surface::new(1280, 800, 0x101010);
        l.paint(&mut s, 0x3B7DD8);
        let antes = s.px.clone();
        click(&mut l, &mut sys, Item::Link);
        typ(&mut l, &mut sys, "abcde");
        l.paint(&mut s, 0x3B7DD8);
        assert!(s.px != antes);
        click(&mut l, &mut sys, Item::Skip);
        l.paint(&mut s, 0x3B7DD8);
        // La clave no se ve nunca: en modo clave el campo solo lleva puntos.
        assert!(!l.plain(Item::Pw2) && l.users.is_empty());
    }

    #[test]
    fn fortaleza_se_pinta_en_el_asistente() {
        let mut l = Login::new(1280, 800);
        let mut sys = sys();
        l.start_setup();
        l.focus = Item::Pw;
        let lh = font::line_h(8);
        let probe = |l: &Login| {
            let r = rect(l, Item::Pw);
            let mut s = Surface::new(1280, 800, 0x101010);
            l.paint(&mut s, 0x3B7DD8);
            (s.px[((r.y - lh - 2 + lh / 2) * 1280 + r.right() - STRENGTH_W + 2) as usize], s)
        };
        let vacio = probe(&l).0;
        for (pw, nivel) in [("abc123", 1u8), ("Abc12345", 2), ("abcdefgh1234", 3), ("Abc12345xyz!qwertyz", 4)] {
            wipe_str(&mut l.pw);
            typ(&mut l, &mut sys, pw);
            assert_eq!(keys::strength(pw), nivel, "{pw}");
            let (px, _) = probe(&l);
            assert_eq!(px, STRENGTH[nivel as usize].1);
            assert_ne!(px, vacio);
        }
        // También en "Añadir otra cuenta" (misma tarjeta con la segunda cuenta).
        l.second = true;
        assert_eq!(probe(&l).0, STRENGTH[4].1);
    }

    #[test]
    fn fortaleza_ensucia_su_zona_al_teclear() {
        let mut l = Login::new(1280, 800);
        let mut sys = sys();
        l.start_setup();
        l.focus = Item::Pw;
        l.take_damage();
        typ(&mut l, &mut sys, "a");
        let lh = font::line_h(8);
        let r = rect(&l, Item::Pw);
        let zona = Rect::new(r.right() - STRENGTH_W, r.y - lh - 2, STRENGTH_W, lh);
        assert!(l.take_damage().iter().any(|d| d.x <= zona.x && d.right() >= zona.right() && d.y <= zona.y && d.bottom() >= zona.bottom()));
    }

    fn pick_recuperable() -> (Login, TestPlatform) {
        let mut l = Login::new(1280, 800);
        l.start_pick(alloc::vec![(String::from("Ana"), true), (String::from("Luis"), true)]);
        l.set_recoverable(alloc::vec![true, false]);
        (l, sys())
    }

    #[test]
    fn recuperar_cuenta_solo_se_ofrece_si_esta_cifrada() {
        let (mut l, mut sys) = pick_recuperable();
        assert!(l.layout().items.iter().any(|i| i.0 == Item::Link));
        press(&mut l, &mut sys, Key::Right, None);
        assert!(l.layout().items.iter().all(|i| i.0 != Item::Link));
        press(&mut l, &mut sys, Key::Left, None);
        assert_eq!(l.order(), [Item::Pw, Item::Link, Item::Ok]);
    }

    #[test]
    fn recuperar_cuenta_valida_el_formulario_y_borra_los_buffers() {
        let (mut l, mut sys) = pick_recuperable();
        click(&mut l, &mut sys, Item::Link);
        assert_eq!(l.mode, Mode::Recover);
        // Código corto.
        typ(&mut l, &mut sys, "abc");
        press(&mut l, &mut sys, Key::Enter, None);
        assert_eq!(l.err.as_ref().map(|e| (e.0, e.1.as_str())), Some((Item::Code, "El código debe tener 40 caracteres.")));
        // Claves distintas.
        for _ in 0..3 {
            press(&mut l, &mut sys, Key::Backspace, None);
        }
        typ(&mut l, &mut sys, CODE);
        press(&mut l, &mut sys, Key::Tab, None);
        typ(&mut l, &mut sys, "nueva123");
        press(&mut l, &mut sys, Key::Tab, None);
        typ(&mut l, &mut sys, "nueva124");
        press(&mut l, &mut sys, Key::Enter, None);
        assert_eq!(l.err.as_ref().map(|e| (e.0, e.1.as_str())), Some((Item::Pw2, "Las claves no coinciden.")));
        // Iguales: "Comprobando…" y, un tick después, el resultado (el disco de pruebas no está cifrado).
        press(&mut l, &mut sys, Key::Backspace, None);
        typ(&mut l, &mut sys, "3");
        press(&mut l, &mut sys, Key::Enter, None);
        assert_eq!(msg(&l), "Comprobando…");
        l.tick(500, &mut sys);
        assert_eq!(msg(&l), KeyError::NotEncrypted.text());
        assert_eq!((l.code.len(), l.pw.len(), l.pw2.len()), (0, 0, 0));
        assert_eq!(l.mode, Mode::Recover);
        // Cancelar vuelve al inicio de sesión.
        click(&mut l, &mut sys, Item::Done);
        assert_eq!(l.mode, Mode::Pick);
        assert_eq!(l.users.len(), 2);
    }

    #[test]
    fn recuperar_cuenta_con_exito_vuelve_con_un_aviso_verde() {
        let (mut l, mut sys) = pick_recuperable();
        click(&mut l, &mut sys, Item::Link);
        l.recover_result(Ok(()));
        assert_eq!(l.mode, Mode::Pick);
        assert_eq!(msg(&l), "Clave cambiada. Ya puedes iniciar sesión.");
        assert!(l.good);
        // Un intento posterior con clave mala vuelve a ser rojo.
        typ(&mut l, &mut sys, "mala");
        press(&mut l, &mut sys, Key::Enter, None);
        assert!(!l.good);
        // Y un error de clave floja apunta al campo de la clave nueva.
        click(&mut l, &mut sys, Item::Link);
        l.recover_result(Err(KeyError::Weak));
        assert_eq!((l.focus, msg(&l).as_str()), (Item::Pw, KeyError::Weak.text()));
    }

    #[test]
    fn el_acumulador_vuelve_con_sus_eventos_tras_prestarse() {
        let mut sys = sys();
        for i in 0..POOL_EVENTS_NEEDED {
            sys.pool.add_event(i as i32, 1, 1, i as u64);
        }
        let r = with_pool(&mut sys, |_, pool| {
            assert!(pool.ready());
            7
        });
        assert_eq!(r, 7);
        assert!(sys.pool.ready());
    }

    // ---------------------------------------------------------------- asistente de disco nuevo (v0.7.1)

    fn nuevo() -> (Login, TestPlatform) {
        let mut sys = sys();
        let now = sys.now();
        sys.files = Files::probe(Some(Box::new(crate::block::RamDisk::new(2048))), now);
        let mut l = Login::new(1280, 800);
        l.start_new_disk();
        (l, sys)
    }

    fn llena(sys: &mut TestPlatform) {
        for i in 0..POOL_EVENTS_NEEDED {
            sys.pool.add_event(i as i32, 3, 1, i as u64 * 7919);
        }
    }

    fn tecla(l: &mut Login, sys: &mut TestPlatform, key: Key) -> Out {
        press(l, sys, key, None)
    }

    /// Añade una cuenta en la tarjeta de cuentas (Tab entre campos, Enter para crearla).
    fn cuenta(l: &mut Login, sys: &mut TestPlatform, name: &str, pw: &str) -> Out {
        typ(l, sys, name);
        tecla(l, sys, Key::Tab);
        typ(l, sys, pw);
        tecla(l, sys, Key::Tab);
        typ(l, sys, pw);
        tecla(l, sys, Key::Enter)
    }

    #[test]
    fn asistente_nuevo_ofrece_cuatro_opciones_con_mi_carpeta_marcada() {
        let (mut l, mut sys) = nuevo();
        assert_eq!((l.step_name(), l.enc, l.opt_index()), ("cifrado", Enc::Home, 1));
        assert_eq!(l.layout().items.iter().filter(|i| matches!(i.0, Item::Opt(_))).count(), 4);
        tecla(&mut l, &mut sys, Key::Down);
        assert_eq!(l.enc, Enc::Disk);
        tecla(&mut l, &mut sys, Key::Right);
        assert_eq!(l.enc, Enc::Both);
        tecla(&mut l, &mut sys, Key::Down);
        assert_eq!(l.enc, Enc::Nothing);
        tecla(&mut l, &mut sys, Key::Up);
        assert_eq!(l.enc, Enc::Both);
        click(&mut l, &mut sys, Item::Opt(1));
        assert_eq!(l.enc, Enc::Home);
        // Nada se ha tocado: el disco sigue sin formatear.
        assert!(sys.files.pending_format());
    }

    #[test]
    fn eleccion_sin_cifrado_de_disco_salta_la_clave_del_disco() {
        for (opt, clave) in [(Item::Opt(0), false), (Item::Opt(1), false), (Item::Opt(2), true), (Item::Opt(3), true)] {
            let (mut l, mut sys) = nuevo();
            click(&mut l, &mut sys, opt);
            assert_eq!(tecla(&mut l, &mut sys, Key::Enter), Out::None);
            assert_eq!(l.step_name(), if clave { "clave del disco" } else { "cuentas" });
        }
    }

    #[test]
    fn clave_del_disco_corta_distinta_o_debil() {
        let (mut l, mut sys) = nuevo();
        click(&mut l, &mut sys, Item::Opt(2));
        click(&mut l, &mut sys, Item::Ok);
        assert_eq!(l.step_name(), "clave del disco");
        // Vacía o corta.
        tecla(&mut l, &mut sys, Key::Enter);
        assert_eq!(l.err.as_ref().map(|e| (e.0, e.1.as_str())), Some((Item::Pw, KeyError::Weak.text())));
        typ(&mut l, &mut sys, "abc");
        tecla(&mut l, &mut sys, Key::Enter);
        assert_eq!(l.step_name(), "clave del disco");
        // No coinciden.
        typ(&mut l, &mut sys, "def");
        tecla(&mut l, &mut sys, Key::Tab);
        typ(&mut l, &mut sys, "abcdeg");
        tecla(&mut l, &mut sys, Key::Enter);
        assert_eq!(l.err.as_ref().map(|e| (e.0, e.1.as_str())), Some((Item::Pw2, "Las claves no coinciden.")));
        // Débil ("abcdef" es nivel 0 o 1): avisa y a la segunda sigue.
        back(&mut l, &mut sys, 1);
        typ(&mut l, &mut sys, "f");
        assert!(keys::strength("abcdef") < 2);
        tecla(&mut l, &mut sys, Key::Enter);
        assert_eq!(l.step_name(), "clave del disco");
        assert!(l.weak_ok && msg(&l).starts_with("La clave es débil"));
        let mut s = Surface::new(1280, 800, 0x101010);
        l.paint(&mut s, 0x3B7DD8);
        tecla(&mut l, &mut sys, Key::Enter);
        assert_eq!(l.step_name(), "cuentas");
        assert_eq!(l.disk_pw, "abcdef");
        assert_eq!((l.pw.len(), l.pw2.len()), (0, 0));
    }

    fn back(l: &mut Login, sys: &mut TestPlatform, n: usize) {
        for _ in 0..n {
            press(l, sys, Key::Backspace, None);
        }
    }

    #[test]
    fn clave_del_disco_fuerte_sigue_sin_aviso_y_atras_borra() {
        let (mut l, mut sys) = nuevo();
        click(&mut l, &mut sys, Item::Opt(3));
        click(&mut l, &mut sys, Item::Ok);
        typ(&mut l, &mut sys, "Abc12345");
        tecla(&mut l, &mut sys, Key::Tab);
        typ(&mut l, &mut sys, "Abc12345");
        // Atrás vuelve a la elección y no deja nada escrito.
        click(&mut l, &mut sys, Item::Done);
        assert_eq!((l.step_name(), l.pw.len(), l.pw2.len(), l.disk_pw.len()), ("cifrado", 0, 0, 0));
        assert_eq!(l.enc, Enc::Both);
        click(&mut l, &mut sys, Item::Ok);
        typ(&mut l, &mut sys, "Abc12345");
        tecla(&mut l, &mut sys, Key::Tab);
        typ(&mut l, &mut sys, "Abc12345");
        click(&mut l, &mut sys, Item::Ok);
        assert_eq!(l.step_name(), "cuentas");
        // Escape también vuelve desde la clave del disco.
        let (mut l, mut sys) = nuevo();
        click(&mut l, &mut sys, Item::Opt(2));
        click(&mut l, &mut sys, Item::Ok);
        tecla(&mut l, &mut sys, Key::Escape);
        assert_eq!(l.step_name(), "cifrado");
    }

    #[test]
    fn con_mi_carpeta_la_clave_de_la_cuenta_es_obligatoria() {
        let (mut l, mut sys) = nuevo();
        click(&mut l, &mut sys, Item::Ok); // Mi carpeta
        assert_eq!(l.step_name(), "cuentas");
        typ(&mut l, &mut sys, "Ana");
        tecla(&mut l, &mut sys, Key::Enter);
        assert_eq!(l.err.as_ref().map(|e| e.0), Some(Item::Pw));
        assert!(l.pend.is_empty() && !l.second);
        // Corta tampoco vale.
        typ(&mut l, &mut sys, "abc");
        tecla(&mut l, &mut sys, Key::Tab);
        typ(&mut l, &mut sys, "abc");
        tecla(&mut l, &mut sys, Key::Enter);
        assert_eq!(l.err.as_ref().map(|e| (e.0, e.1.as_str())), Some((Item::Pw, AccountError::WeakPassword.text())));
        assert!(l.pend.is_empty());
        // Con clave sí.
        click(&mut l, &mut sys, Item::Pw);
        back(&mut l, &mut sys, 3);
        tecla(&mut l, &mut sys, Key::Tab);
        back(&mut l, &mut sys, 3);
        click(&mut l, &mut sys, Item::Pw);
        typ(&mut l, &mut sys, "secreto1");
        tecla(&mut l, &mut sys, Key::Tab);
        typ(&mut l, &mut sys, "secreto1");
        tecla(&mut l, &mut sys, Key::Enter);
        assert_eq!((l.pend.len(), l.second), (1, true));
        assert!(sys.files.pending_format(), "el disco no se toca hasta Terminar");
        // Con Ninguno la clave sigue siendo opcional.
        let (mut l, mut sys) = nuevo();
        click(&mut l, &mut sys, Item::Opt(0));
        click(&mut l, &mut sys, Item::Ok);
        typ(&mut l, &mut sys, "Ana");
        tecla(&mut l, &mut sys, Key::Enter);
        assert_eq!(l.pend.len(), 1);
        // Con Disco entero también.
        let (mut l, mut sys) = nuevo();
        click(&mut l, &mut sys, Item::Opt(2));
        click(&mut l, &mut sys, Item::Ok);
        typ(&mut l, &mut sys, "claveDisco1");
        tecla(&mut l, &mut sys, Key::Tab);
        typ(&mut l, &mut sys, "claveDisco1");
        tecla(&mut l, &mut sys, Key::Enter);
        typ(&mut l, &mut sys, "Ana");
        tecla(&mut l, &mut sys, Key::Enter);
        assert_eq!(l.pend.len(), 1);
    }

    #[test]
    fn una_cuenta_sin_clave_avisa_de_que_su_carpeta_no_se_cifra() {
        let (mut l, mut sys) = nuevo();
        click(&mut l, &mut sys, Item::Ok);
        cuenta(&mut l, &mut sys, "Ana", "secreto1");
        typ(&mut l, &mut sys, "Luis");
        tecla(&mut l, &mut sys, Key::Enter);
        assert_eq!(l.pend.len(), 1, "el primer Enter solo avisa");
        assert_eq!(l.err.as_ref().map(|e| (e.0, e.1.as_str())), Some((Item::Pw, "Su carpeta quedará sin cifrar. Pulsa otra vez para añadirla igualmente.")));
        tecla(&mut l, &mut sys, Key::Enter);
        assert_eq!(l.pend.len(), 2);
        assert!(!l.pend[1].admin && l.pend[0].admin);
        // Nombres repetidos o no válidos se rechazan ya, en su tarjeta.
        typ(&mut l, &mut sys, "ana");
        tecla(&mut l, &mut sys, Key::Enter);
        assert_eq!(l.err.as_ref().map(|e| (e.0, e.1.as_str())), Some((Item::Name, AccountError::Exists.text())));
        assert_eq!(l.pend.len(), 2);
    }

    #[test]
    fn cancelar_la_ventana_del_raton_vuelve_a_las_cuentas_sin_tocar_el_disco() {
        let (mut l, mut sys) = nuevo();
        click(&mut l, &mut sys, Item::Ok);
        cuenta(&mut l, &mut sys, "Ana", "secreto1");
        assert_eq!(click(&mut l, &mut sys, Item::Done), Out::NeedEntropy);
        assert!(!l.covers());
        l.cancel_entropy();
        assert_eq!((l.mode, l.step_name(), l.pend.len()), (Mode::Setup, "cuentas", 1));
        assert!(sys.files.pending_format() && !sys.files.persistent());
    }

    /// Recorre una opción entera con el `Login` suelto y devuelve los códigos mostrados.
    fn recorre(opt: usize, sys: &mut TestPlatform, l: &mut Login) -> Vec<(String, String)> {
        click(l, sys, Item::Opt(opt));
        click(l, sys, Item::Ok);
        if l.step_name() == "clave del disco" {
            typ(l, sys, "claveDisco1");
            tecla(l, sys, Key::Tab);
            typ(l, sys, "claveDisco1");
            tecla(l, sys, Key::Enter);
        }
        let pw = if opt == 0 || opt == 2 { "" } else { "secreto1" };
        cuenta(l, sys, "Ana", pw);
        let pw2 = if opt == 0 || opt == 2 { "" } else { "otraclave1" };
        typ(l, sys, "Luis");
        if !pw2.is_empty() {
            tecla(l, sys, Key::Tab);
            typ(l, sys, pw2);
            tecla(l, sys, Key::Tab);
            typ(l, sys, pw2);
        }
        tecla(l, sys, Key::Enter);
        assert_eq!(l.pend.len(), 2);
        let out = click(l, sys, Item::Done);
        if opt == 0 {
            assert_eq!(out, Out::None);
        } else {
            assert_eq!(out, Out::NeedEntropy);
            llena(sys);
            l.begin_provision();
        }
        assert_eq!(l.step_name(), "preparando");
        // Mientras se prepara no se acepta nada.
        assert_eq!(tecla(l, sys, Key::Enter), Out::None);
        assert!(l.job.is_some() && l.covers());
        let mut codes = Vec::new();
        let mut out = l.tick(l.now + 1000, sys);
        while out == Out::None {
            codes.push(l.shown_code().expect("código"));
            out = tecla(l, sys, Key::Enter);
        }
        assert_eq!(out, Out::SetupDone);
        codes
    }

    #[test]
    fn las_cuatro_opciones_dejan_el_disco_y_las_carpetas_como_se_pidio() {
        // (opción, disco cifrado, Ana cifrada, Luis cifrada, códigos)
        let casos = [(0usize, false, false, false, 0usize), (1, false, true, true, 2), (2, true, false, false, 1), (3, true, true, true, 3)];
        for (opt, disco, ana, luis, n) in casos {
            let (mut l, mut sys) = nuevo();
            let codes = recorre(opt, &mut sys, &mut l);
            assert_eq!(codes.len(), n, "opción {opt}");
            assert_eq!((sys.files.disk_encrypted(), sys.files.account_encrypted("Ana"), sys.files.account_encrypted("Luis")), (disco, ana, luis), "opción {opt}");
            assert!(sys.files.persistent() && sys.files.user().is_none());
            assert_eq!(sys.files.accounts().len(), 2);
            // Orden: primero el del disco y luego cada cuenta, con a quién pertenece.
            let dueños: Vec<&str> = codes.iter().map(|c| c.0.as_str()).collect();
            let esperado: Vec<&str> = [(disco, "del disco"), (ana, "de la cuenta Ana"), (luis, "de la cuenta Luis")].iter().filter(|x| x.0).map(|x| x.1).collect();
            assert_eq!(dueños, esperado, "opción {opt}");
            // Todo secreto borrado al terminar.
            assert!(l.codes.iter().all(|c| c.1.bytes().all(|b| b == 0)) && l.pend.iter().all(|p| p.pw.bytes().all(|b| b == 0)));
            assert!(l.disk_pw.bytes().all(|b| b == 0));
        }
    }

    #[test]
    fn sin_aleatoriedad_suficiente_vuelve_a_la_eleccion_sin_formatear() {
        let (mut l, mut sys) = nuevo();
        click(&mut l, &mut sys, Item::Opt(2));
        click(&mut l, &mut sys, Item::Ok);
        typ(&mut l, &mut sys, "claveDisco1");
        tecla(&mut l, &mut sys, Key::Tab);
        typ(&mut l, &mut sys, "claveDisco1");
        tecla(&mut l, &mut sys, Key::Enter);
        cuenta(&mut l, &mut sys, "Ana", "");
        click(&mut l, &mut sys, Item::Done);
        // Sin llenar el acumulador la preparación falla: error y de vuelta a la elección.
        l.begin_provision();
        assert_eq!(l.tick(l.now + 1000, &mut sys), Out::None);
        assert_eq!((l.mode, l.step_name()), (Mode::Setup, "cifrado"));
        assert_eq!(msg(&l), KeyError::NotReady.text());
        assert!(sys.files.pending_format() && sys.files.accounts().is_empty());
        assert!(l.pend.is_empty() && !l.second);
        // Se puede elegir otra opción y seguir.
        let mut s = Surface::new(1280, 800, 0x101010);
        l.paint(&mut s, 0x3B7DD8);
        click(&mut l, &mut sys, Item::Opt(0));
        click(&mut l, &mut sys, Item::Ok);
        cuenta(&mut l, &mut sys, "Ana", "");
        assert_eq!(click(&mut l, &mut sys, Item::Done), Out::None);
        assert_eq!(l.tick(l.now + 1000, &mut sys), Out::SetupDone);
        assert!(sys.files.persistent() && sys.files.accounts().len() == 1);
    }

    #[test]
    fn lo_he_apuntado_avanza_y_el_ultimo_codigo_termina() {
        let (mut l, mut sys) = nuevo();
        click(&mut l, &mut sys, Item::Opt(3));
        click(&mut l, &mut sys, Item::Ok);
        typ(&mut l, &mut sys, "claveDisco1");
        tecla(&mut l, &mut sys, Key::Tab);
        typ(&mut l, &mut sys, "claveDisco1");
        tecla(&mut l, &mut sys, Key::Enter);
        cuenta(&mut l, &mut sys, "Ana", "secreto1");
        assert_eq!(click(&mut l, &mut sys, Item::Done), Out::NeedEntropy);
        llena(&mut sys);
        l.begin_provision();
        assert_eq!(l.tick(2000, &mut sys), Out::None);
        assert_eq!((l.step_name(), l.codes.len()), ("codigos", 2));
        let (dueño, c1) = l.shown_code().unwrap();
        assert_eq!(dueño, "del disco");
        assert_eq!(c1.len(), 47);
        let mut s = Surface::new(1280, 800, 0x101010);
        l.paint(&mut s, 0x3B7DD8);
        assert_eq!(click(&mut l, &mut sys, Item::Ok), Out::None);
        let (dueño, c2) = l.shown_code().unwrap();
        assert_eq!(dueño, "de la cuenta Ana");
        assert_ne!(c1, c2);
        assert_eq!(click(&mut l, &mut sys, Item::Ok), Out::SetupDone);
        assert!(l.codes.is_empty());
    }
}
