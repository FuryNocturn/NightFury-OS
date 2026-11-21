// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Terminal: consola de órdenes del sistema. CONTRATO: `pub fn new() -> Box<dyn App>`.
use super::netcmd::{self, Job, NetCmd};
use super::{App, AppKind, Ctx, MouseEv};
use crate::font;
use crate::geom::Rect;
use crate::files::{file_name, in_system, join, parent, resolve, valid_name};
use crate::fs::FsError;
use crate::input::{Key, KeyEvent};
use crate::platform::Platform;
use crate::programs;
use crate::scrollbar::{Metrics, ScrollBar, SB_W};
use crate::surface::Surface;
use crate::theme::Theme;
use crate::{NAME, VERSION};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

const SIZE: u32 = 9;
const MARGIN: i32 = 6;
const MAX_LINES: usize = 500;
const BLINK_MS: u64 = 530;
const BG: u32 = 0x0C0C0C;
const FG: u32 = 0xCCCCCC;
const CONFIRM: &str = "Vas a modificar una carpeta del sistema. Escribe si para continuar:";

/// Efecto de una orden sobre la ventana.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Action {
    Nothing,
    Clear,
    Close,
    /// Abrir esta ruta (un programa `.nfx`) con el escritorio.
    Open(String),
    /// Orden de red: el Terminal la empieza y `tick` la avanza.
    Net(NetCmd),
}

fn prompt(cwd: &str) -> String {
    format!("{}> ", cwd)
}

/// Líneas máximas y bytes máximos que muestra `ver`.
const VER_LINES: usize = 200;
const VER_BYTES: usize = 16 * 1024;

/// Bytes -> "X,Y KB/MB/GB" con un decimal (también lo usa Configuración).
pub(super) fn units(b: u64) -> String {
    let (div, u) = if b >= 1 << 30 {
        (1 << 30, "GB")
    } else if b >= 1 << 20 {
        (1 << 20, "MB")
    } else {
        (1 << 10, "KB")
    };
    let d = b * 10 / div;
    format!("{},{} {}", d / 10, d % 10, u)
}

/// Primer argumento (admite comillas dobles) y el resto sin tocar.
fn first_token(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    let (mut tok, mut q) = (String::new(), false);
    for (i, c) in s.char_indices() {
        if c == '"' {
            q = !q;
        } else if c.is_whitespace() && !q {
            return Some((tok, s[i..].trim_start()));
        } else {
            tok.push(c);
        }
    }
    Some((tok, ""))
}

pub(super) fn tokens(s: &str) -> Vec<String> {
    let (mut v, mut r) = (Vec::new(), s);
    while let Some((t, rest)) = first_token(r) {
        v.push(t);
        r = rest;
    }
    v
}

fn lower(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}

/// Órdenes de archivos; `None` si `cmd` no es una de ellas.
fn archivos(cmd: &str, args: &str, sys: &mut dyn Platform, cwd: &mut String) -> Option<Vec<String>> {
    let now = sys.now();
    let f = sys.fs();
    let a = tokens(args);
    let uso = |s: &str| Ok(vec![format!("Uso: {}", s)]);
    let r: Result<Vec<String>, FsError> = match cmd {
        "lista" | "dir" if a.len() <= 1 => (|| {
            let p = resolve(cwd, a.first().map_or("", |s| s))?;
            let entries = f.list(&p)?;
            let mut out = vec![format!("Carpeta {}", p)];
            let (mut nf, mut nd, mut bytes) = (0, 0, 0u64);
            for e in &entries {
                let m = e.modified;
                let date = format!("{:02}/{:02}/{:04} {:02}:{:02}", m.day, m.month, m.year, m.hour, m.minute);
                if e.dir {
                    nd += 1;
                    out.push(format!("{}  {:>10}  {}", date, "<CARPETA>", e.name));
                } else {
                    nf += 1;
                    bytes += e.size;
                    out.push(format!("{}  {:>10}  {}", date, e.size, e.name));
                }
            }
            let free = f.drives().iter().find(|d| p.starts_with(d.letter)).map_or(0, |d| d.free);
            out.push(format!("Archivos: {}, carpetas: {}, {} bytes. Libre: {}", nf, nd, bytes, units(free)));
            Ok(out)
        })(),
        "lista" | "dir" => uso("lista [ruta]"),
        "ir" | "cd" if a.is_empty() => Ok(vec![cwd.clone()]),
        "ir" | "cd" if a.len() == 1 => (|| {
            let p = resolve(cwd, &a[0])?;
            if !f.stat(&p)?.dir {
                return Err(FsError::NotDir);
            }
            *cwd = p;
            Ok(Vec::new())
        })(),
        "ir" | "cd" => uso("ir [ruta]"),
        "ver" if a.len() == 1 => (|| {
            let data = f.read(&resolve(cwd, &a[0])?)?;
            let Ok(text) = String::from_utf8(data) else {
                return Ok(vec![String::from("No es un archivo de texto (UTF-8).")]);
            };
            let mut end = text.len().min(VER_BYTES);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let mut out: Vec<String> = text[..end]
                .lines()
                .take(VER_LINES)
                .map(|l| l.replace('\t', "    ").chars().filter(|c| !c.is_control()).collect())
                .collect();
            if end < text.len() || text[..end].lines().count() > VER_LINES {
                out.push(String::from("... (archivo recortado: solo se muestra el principio)"));
            }
            Ok(out)
        })(),
        "ver" => uso("ver archivo"),
        "carpeta" if a.len() == 1 => (|| {
            f.mkdir(&resolve(cwd, &a[0])?, now)?;
            Ok(Vec::new())
        })(),
        "carpeta" => uso("carpeta nombre"),
        "copiar" | "mover" if a.len() == 2 => (|| {
            let src = resolve(cwd, &a[0])?;
            let mut dst = resolve(cwd, &a[1])?;
            if f.stat(&dst).is_ok_and(|e| e.dir) {
                dst = join(&dst, file_name(&src));
            }
            if cmd == "copiar" {
                f.copy(&src, &dst, now)?;
            } else if src.as_bytes()[0] == dst.as_bytes()[0] {
                f.rename(&src, &dst)?;
            } else {
                // Entre unidades no hay renombrado: se copia y se borra el origen.
                f.copy(&src, &dst, now)?;
                f.delete(&src)?;
            }
            Ok(Vec::new())
        })(),
        "copiar" => uso("copiar origen destino"),
        "mover" => uso("mover origen destino"),
        "renombrar" if a.len() == 2 => (|| {
            let src = resolve(cwd, &a[0])?;
            if !valid_name(&a[1]) {
                return Err(FsError::BadName);
            }
            let dst = join(&parent(&src).ok_or(FsError::Invalid)?, &a[1]);
            f.rename(&src, &dst)?;
            Ok(Vec::new())
        })(),
        "renombrar" => uso("renombrar ruta nuevo_nombre"),
        "borrar" => {
            let todo = a.first().is_some_and(|t| t.eq_ignore_ascii_case("/todo"));
            let rest = if todo { &a[1..] } else { &a[..] };
            if rest.len() != 1 {
                uso("borrar [/todo] ruta")
            } else {
                (|| {
                    let p = resolve(cwd, &rest[0])?;
                    if f.stat(&p)?.dir && !todo && !f.list(&p)?.is_empty() {
                        return Ok(vec![
                            String::from(FsError::NotEmpty.message()),
                            String::from("Usa 'borrar /todo ruta' para borrarla con todo lo que contiene."),
                        ]);
                    }
                    f.delete(&p)?;
                    // Si la carpeta actual estaba dentro de lo borrado, sube a la que lo contenía.
                    let (c, q) = (lower(cwd), lower(&p));
                    if c == q || c.starts_with(&format!("{}\\", q)) {
                        *cwd = parent(&p).unwrap_or_else(|| String::from("C:\\"));
                    }
                    Ok(Vec::new())
                })()
            }
        }
        "escribir" => match first_token(args) {
            Some((name, text)) if !text.is_empty() => (|| {
                let data = format!("{}\r\n", text);
                f.write(&resolve(cwd, &name)?, data.as_bytes(), now)?;
                Ok(Vec::new())
            })(),
            _ => uso("escribir archivo texto"),
        },
        "quien" => Ok(match f.user() {
            Some(u) => vec![format!("Usuario: {}", u), format!("Tipo: {}", if f.is_admin() { "administrador" } else { "usuario normal" })],
            None => vec![String::from("No hay ninguna sesión abierta.")],
        }),
        "disco" => {
            let (total, free) = f.space();
            Ok(vec![
                String::from("Disco C:"),
                format!("Tamaño: {}", units(total)),
                format!("Libre: {}", units(free)),
                String::from(if f.persistent() { "Guardado en disco" } else { "En memoria (se pierde al apagar)" }),
            ])
        }
        _ => return None,
    };
    Some(r.unwrap_or_else(|e| vec![String::from(e.message())]))
}

/// Bytes -> "X,Y MB" con un decimal.
fn mb(bytes: u64) -> String {
    let d = bytes * 10 / (1 << 20);
    format!("{},{} MB", d / 10, d % 10)
}

/// ¿Modifica la orden algo dentro de `C:\Sistema`? Solo se pregunta a un administrador (a los
/// demás `Files` les deniega el acceso sin más). Mira el origen y el destino.
fn modifies_system(line: &str, sys: &mut dyn Platform, cwd: &str) -> bool {
    if !sys.fs().is_admin() {
        return false;
    }
    let line = line.trim();
    let (cmd, args) = line.split_once(' ').map_or((line, ""), |(c, a)| (c, a.trim()));
    let a = tokens(args);
    let paths: Vec<String> = match lower(cmd).as_str() {
        "carpeta" | "renombrar" => a.into_iter().take(1).collect(),
        "copiar" | "mover" => a.into_iter().take(2).collect(),
        "borrar" => a.into_iter().filter(|t| !t.eq_ignore_ascii_case("/todo")).take(1).collect(),
        "escribir" => first_token(args).map(|t| t.0).into_iter().collect(),
        "curl" => a.iter().position(|t| t == "-o").and_then(|i| a.get(i + 1)).cloned().into_iter().collect(),
        _ => Vec::new(),
    };
    paths.iter().any(|p| resolve(cwd, p).is_ok_and(|p| in_system(&p)))
}

/// Intérprete de órdenes: devuelve las líneas de salida y la acción pedida.
fn run(line: &str, sys: &mut dyn Platform, cwd: &mut String) -> (Vec<String>, Action) {
    let line = line.trim();
    let (cmd, args) = match line.split_once(' ') {
        Some((c, a)) => (c, a.trim()),
        None => (line, ""),
    };
    let one = |s: String| vec![s];
    let mut act = Action::Nothing;
    let lc = cmd.to_lowercase();
    let out = match lc.as_str() {
        "" => Vec::new(),
        "ayuda" => [
            "Sistema:",
            "  ayuda: Muestra esta lista de órdenes",
            "  info: Datos del sistema, procesador y pantalla",
            "  memoria: Uso de la memoria",
            "  fecha: Fecha actual",
            "  hora: Hora actual",
            "  tiempo: Tiempo que lleva encendido el equipo",
            "  eco: Escribe el texto que le pases",
            "  limpiar: Borra la pantalla",
            "  acerca: Qué es este sistema",
            "  reiniciar: Reinicia el equipo",
            "  apagar: Apaga el equipo",
            "  salir: Cierra esta ventana",
            "Archivos (rutas con espacios, entre comillas):",
            "  lista [ruta] (dir): Muestra el contenido de una carpeta",
            "  ir [ruta] (cd): Cambia de carpeta; sin ruta, muestra la actual",
            "  ver archivo: Muestra un archivo de texto",
            "  carpeta nombre: Crea una carpeta",
            "  copiar origen destino: Copia un archivo o carpeta",
            "  mover origen destino: Mueve un archivo o carpeta",
            "  renombrar ruta nuevo_nombre: Cambia el nombre",
            "  borrar [/todo] ruta: Borra para siempre (/todo: carpeta con contenido)",
            "  escribir archivo texto: Crea o reemplaza un archivo con ese texto",
            "  disco: Tamaño, espacio libre y tipo del disco C:",
            "  quien: Muestra el usuario de la sesión y si es administrador",
            "Programas:",
            "  programas: Lista los programas instalados",
            "  ejecutar nombre|ruta.nfx: Abre un programa",
            "Red:",
            "  ping [-n N] [-t] destino: Comprueba si un equipo responde (-t sin parar; Ctrl+C corta)",
            "  ipconfig [/all|/renew]: Muestra o renueva la dirección IP",
            "  nslookup host: Busca la dirección de un nombre",
            "  curl [-o nombre] [-I] [-L] URL: Descarga una página (-I cabeceras, -L redirecciones)",
            "Modificar algo dentro de C:\\Sistema pide confirmación: escribe si para continuar.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
        "info" => {
            let (w, h) = sys.screen();
            vec![format!("Sistema: {} {}", NAME, VERSION), format!("Procesador: {}", sys.cpu()), format!("Pantalla: {}x{}", w, h)]
        }
        "memoria" => {
            let m = sys.mem();
            vec![
                format!("RAM total: {}", mb(m.total)),
                format!("Memoria del sistema, total: {}", mb(m.heap_total)),
                format!("Memoria del sistema, usada: {}", mb(m.heap_used)),
                format!("Memoria del sistema, libre: {}", mb(m.heap_total.saturating_sub(m.heap_used))),
            ]
        }
        "fecha" => {
            let d = sys.now();
            one(format!("{:02}/{:02}/{:04}", d.day, d.month, d.year))
        }
        "hora" => {
            let d = sys.now();
            one(format!("{:02}:{:02}:{:02}", d.hour, d.minute, d.second))
        }
        "tiempo" => {
            let s = sys.uptime_ms() / 1000;
            one(format!("{} h {} min {} s", s / 3600, s / 60 % 60, s % 60))
        }
        "eco" => one(args.to_string()),
        "limpiar" => {
            act = Action::Clear;
            Vec::new()
        }
        "acerca" => vec![format!("{} es un sistema operativo escrito desde cero en Rust.", NAME), format!("Versión {}", VERSION)],
        "reiniciar" => {
            sys.reboot();
            Vec::new()
        }
        "apagar" => {
            sys.shutdown();
            Vec::new()
        }
        "salir" => {
            act = Action::Close;
            Vec::new()
        }
        "programas" => {
            let v: Vec<String> = programs::installed(sys).iter().map(|p| format!("  {}  {}", p.name, p.version)).collect();
            if v.is_empty() {
                one(String::from("No hay programas instalados"))
            } else {
                v
            }
        }
        "ejecutar" if args.is_empty() => one(String::from("Uso: ejecutar nombre|ruta.nfx")),
        "ejecutar" => {
            let name = tokens(args).into_iter().next().unwrap_or_default();
            let by_name = programs::installed(sys).into_iter().find(|p| p.name.eq_ignore_ascii_case(&name)).map(|p| p.binary());
            match by_name.or_else(|| resolve(cwd, &name).ok().filter(|p| p.to_lowercase().ends_with(".nfx"))) {
                Some(p) if sys.fs().stat(&p).is_ok_and(|e| !e.dir) => {
                    act = Action::Open(p);
                    Vec::new()
                }
                _ => one(format!("No hay ningún programa llamado '{name}'")),
            }
        }
        "ping" | "ipconfig" | "nslookup" | "curl" => match netcmd::parse(&lc, args, cwd, sys) {
            Ok(c) => {
                act = Action::Net(c);
                Vec::new()
            }
            Err(v) => v,
        },
        _ => match archivos(&lc, args, sys, cwd) {
            Some(v) => v,
            None => one(format!("'{}' no es una orden conocida. Escribe 'ayuda'.", cmd)),
        },
    };
    (out, act)
}

/// Parte `text` en filas de como mucho `max_w` píxeles (siempre devuelve al menos una).
fn wrap(text: &str, max_w: i32) -> Vec<String> {
    let mut rows = Vec::new();
    let (mut cur, mut w) = (String::new(), 0);
    for c in text.chars() {
        let a = font::adv(c, SIZE, false);
        if w + a > max_w && !cur.is_empty() {
            rows.push(core::mem::take(&mut cur));
            w = 0;
        }
        cur.push(c);
        w += a;
    }
    rows.push(cur);
    rows
}

pub struct Terminal {
    out: Vec<String>,
    /// Carpeta actual (vacía hasta el primer uso: entonces pasa a ser la del usuario).
    cwd: String,
    /// Orden de `C:Sistema` esperando que se escriba `si`.
    pending: Option<String>,
    input: String,
    history: Vec<String>,
    /// Posición en `history` al recorrerlo con las flechas; `history.len()` = línea nueva.
    hist_pos: usize,
    /// Filas desplazadas hacia arriba desde el final.
    scroll: usize,
    max_scroll: usize,
    vis: usize,
    /// Filas visuales totales y tamaño del contenido (última vez dibujado), para la barra.
    total: usize,
    size: (i32, i32),
    sb: ScrollBar,
    focused: bool,
    t0: u64,
    shown: bool,
    /// Orden de red en marcha: mientras dura, el teclado solo atiende Ctrl+C.
    job: Option<Job>,
}

pub fn new() -> Box<dyn App> {
    Box::new(Terminal::new())
}

impl Terminal {
    fn new() -> Self {
        let mut t = Terminal {
            out: Vec::new(),
            cwd: String::new(),
            pending: None,
            input: String::new(),
            history: Vec::new(),
            hist_pos: 0,
            scroll: 0,
            max_scroll: 0,
            vis: 10,
            total: 0,
            size: (0, 0),
            sb: ScrollBar::new(false),
            focused: false,
            t0: 0,
            shown: true,
            job: None,
        };
        t.push(format!("{} {}", NAME, VERSION));
        t.push(String::from("Escribe 'ayuda' para ver las órdenes."));
        t
    }

    /// La barra cuenta desde arriba; `scroll` desde el final.
    fn metrics(&self) -> Metrics {
        let (w, h) = self.size;
        Metrics {
            rect: Rect::new(w - SB_W, 0, SB_W, h),
            total: self.total as i32,
            page: self.vis as i32,
            pos: self.max_scroll.saturating_sub(self.scroll) as i32,
            line: 1,
        }
    }

    fn push(&mut self, l: String) {
        self.out.push(l);
        if self.out.len() > MAX_LINES {
            self.out.drain(..self.out.len() - MAX_LINES);
        }
    }

    /// La carpeta de partida es la del usuario de la sesión.
    fn ready(&mut self, sys: &mut dyn Platform) {
        if self.cwd.is_empty() {
            self.cwd = sys.fs().home();
        }
    }

    /// Si la orden era de red, la empieza (lo que escriba de inmediato va a la pantalla).
    fn net_job(&mut self, act: Action, sys: &mut dyn Platform, now: u64) -> Action {
        let Action::Net(cmd) = act else { return act };
        let mut out = Vec::new();
        self.job = Job::start(cmd, sys, now, &mut out);
        for l in out {
            self.push(l);
        }
        Action::Nothing
    }

    fn submit(&mut self, sys: &mut dyn Platform, now: u64) -> Action {
        self.ready(sys);
        let line = core::mem::take(&mut self.input);
        self.push(format!("{}{}", prompt(&self.cwd), line));
        let cmd = line.trim();
        // Respuesta a "¿Seguro?": solo `si` sigue adelante; cualquier otra cosa cancela.
        if let Some(orden) = self.pending.take() {
            self.hist_pos = self.history.len();
            if !(cmd.eq_ignore_ascii_case("si") || cmd.eq_ignore_ascii_case("sí")) {
                self.push(String::from("Cancelado."));
                return Action::Nothing;
            }
            let (lines, act) = run(&orden, sys, &mut self.cwd);
            for l in lines {
                self.push(l);
            }
            return self.net_job(act, sys, now);
        }
        if !cmd.is_empty() && self.history.last().map(String::as_str) != Some(cmd) {
            self.history.push(cmd.to_string());
        }
        self.hist_pos = self.history.len();
        if modifies_system(&line, sys, &self.cwd) {
            self.pending = Some(line);
            self.push(String::from(CONFIRM));
            return Action::Nothing;
        }
        let (lines, act) = run(&line, sys, &mut self.cwd);
        if act == Action::Clear {
            self.out.clear();
        }
        for l in lines {
            self.push(l);
        }
        self.net_job(act, sys, now)
    }
}

impl App for Terminal {
    fn kind(&self) -> AppKind {
        AppKind::Terminal
    }
    fn title(&self) -> String {
        String::from("Terminal")
    }
    fn min_size(&self) -> (i32, i32) {
        (320, 160)
    }

    /// "Abrir Terminal aquí": si la ruta es una carpeta existente pasa a ser la actual.
    fn open_path(&mut self, path: &str, cx: &mut Ctx) {
        if cx.sys.fs().stat(path).is_ok_and(|e| e.dir) {
            self.cwd = String::from(path);
            cx.redraw = true;
        }
    }

    fn draw(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, focused: bool) {
        self.ready(sys);
        self.focused = focused;
        s.fill_rect(s.bounds(), BG);
        let lh = font::line_h(SIZE);
        self.vis = (((s.h - 2 * MARGIN) / lh).max(1)) as usize;
        // Todas las filas visuales; la última línea lleva un espacio final que hace de cursor.
        // Si no caben, se vuelven a partir con el ancho que deja la barra.
        let mut sbw = 0;
        let (rows, n_input) = loop {
            let max_w = (s.w - 2 * MARGIN - sbw).max(1);
            let mut rows: Vec<String> = Vec::new();
            for l in &self.out {
                rows.extend(wrap(l, max_w));
            }
            // Con una orden de red en marcha no hay indicador ni cursor.
            let input_rows = if self.job.is_some() { Vec::new() } else { wrap(&format!("{}{} ", prompt(&self.cwd), self.input), max_w) };
            let n_input = input_rows.len();
            rows.extend(input_rows);
            if sbw > 0 || rows.len() <= self.vis {
                break (rows, n_input);
            }
            sbw = SB_W;
        };
        self.total = rows.len();
        self.size = (s.w, s.h);
        self.max_scroll = rows.len().saturating_sub(self.vis);
        self.scroll = self.scroll.min(self.max_scroll);
        let first = rows.len().saturating_sub(self.vis + self.scroll);
        let last = (first + self.vis).min(rows.len());

        for (i, r) in rows[first..last].iter().enumerate() {
            let y = MARGIN + i as i32 * lh;
            let is_cursor_row = first + i == rows.len() - 1 && n_input > 0;
            if is_cursor_row {
                // Sin el espacio final: el bloque se pinta justo detrás.
                let txt = &r[..r.len() - 1];
                let end = font::draw(s, MARGIN, y, txt, SIZE, false, FG);
                if focused && self.shown {
                    s.fill_rect(Rect::new(end, y + 1, font::adv('0', SIZE, false), lh - 2), FG);
                }
            } else {
                font::draw(s, MARGIN, y, r, SIZE, false, FG);
            }
        }
        if sbw > 0 {
            self.sb.draw(s, &self.metrics(), theme);
        }
    }

    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
        if self.job.is_some() && !matches!(ev.key, Key::PageUp | Key::PageDown) {
            if ev.ctrl && ev.key == Key::Char('c') {
                let mut out = Vec::new();
                if let Some(mut j) = self.job.take() {
                    j.cancel(&mut *cx.sys, &mut out);
                }
                for l in out {
                    self.push(l);
                }
                self.scroll = 0;
                cx.redraw = true;
            }
            return;
        }
        match ev.key {
            Key::PageUp => self.scroll = (self.scroll + self.vis.saturating_sub(1).max(1)).min(self.max_scroll),
            Key::PageDown => self.scroll = self.scroll.saturating_sub(self.vis.saturating_sub(1).max(1)),
            Key::Enter => {
                self.scroll = 0;
                match self.submit(&mut *cx.sys, cx.now_ms) {
                    Action::Close => cx.close = true,
                    Action::Open(p) => cx.open = Some(p),
                    _ => {}
                }
            }
            Key::Backspace => {
                self.scroll = 0;
                self.input.pop();
            }
            Key::Escape => {
                self.scroll = 0;
                self.input.clear();
                self.pending = None;
                self.hist_pos = self.history.len();
            }
            Key::Up => {
                self.scroll = 0;
                if self.hist_pos > 0 {
                    self.hist_pos -= 1;
                    self.input = self.history[self.hist_pos].clone();
                }
            }
            Key::Down => {
                self.scroll = 0;
                if self.hist_pos + 1 < self.history.len() {
                    self.hist_pos += 1;
                    self.input = self.history[self.hist_pos].clone();
                } else {
                    self.hist_pos = self.history.len();
                    self.input.clear();
                }
            }
            _ => match ev.ch {
                Some(c) if !ev.ctrl && !c.is_control() => {
                    self.scroll = 0;
                    self.input.push(c);
                }
                _ => return,
            },
        }
        self.t0 = cx.now_ms;
        self.shown = true;
        cx.redraw = true;
    }

    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
        if !matches!(ev, MouseEv::Wheel { .. }) {
            let e = self.sb.mouse(ev, &self.metrics(), cx.now_ms);
            if let Some(p) = e.pos {
                self.scroll = self.max_scroll.saturating_sub(p as usize);
            }
            cx.redraw |= e.redraw;
            return;
        }
        if let MouseEv::Wheel { dz, .. } = ev {
            // dz > 0 = hacia abajo (líneas recientes); `scroll` cuenta filas desde el final.
            self.scroll = (self.scroll as i64 - dz as i64 * 3).clamp(0, self.max_scroll as i64) as usize;
            cx.redraw = true;
        }
    }

    fn tick(&mut self, cx: &mut Ctx) {
        if let Some(mut job) = self.job.take() {
            let mut out = Vec::new();
            let done = job.step(&mut *cx.sys, cx.now_ms, &mut out);
            if !done {
                self.job = Some(job);
            }
            if done || !out.is_empty() {
                for l in out {
                    self.push(l);
                }
                self.scroll = 0;
                cx.redraw = true;
            }
        }
        if let Some(p) = self.sb.tick(&self.metrics(), cx.now_ms) {
            self.scroll = self.max_scroll.saturating_sub(p as usize);
            cx.redraw = true;
        }
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
    use crate::block::RamDisk;
    use crate::files::{Files, DEFAULT_USER, USERS_DIR};
    use crate::platform::{DateTime, TestPlatform};

    /// Carpeta del usuario de la sesión por defecto de las pruebas.
    fn home() -> String {
        join(USERS_DIR, DEFAULT_USER)
    }
    fn h(rel: &str) -> String {
        join(&home(), rel)
    }

    fn ejecuta(l: &str) -> (Vec<String>, Action) {
        run(l, &mut TestPlatform { ms: 3_723_000, size: (1024, 768), ..Default::default() }, &mut home())
    }

    /// Plataforma con disco: administradora Ana y usuario normal Luis, con la sesión de `quien`.
    fn con_disco(quien: &str) -> TestPlatform {
        let n = DateTime::default();
        let mut f = Files::probe(Some(Box::new(RamDisk::new(2048))), n);
        f.format_pending(n).unwrap();
        f.create_account("Ana", None, true, [1; 16], n).unwrap();
        f.login("Ana", "").unwrap();
        f.create_account("Luis", None, false, [2; 16], n).unwrap();
        f.write(&join(&join(&join(USERS_DIR, "Ana"), "Documentos"), "secreto.txt"), b"x", n).unwrap();
        f.login(quien, "").unwrap();
        let mut p = TestPlatform::default();
        p.files = f;
        p
    }

    fn ev(key: Key, ch: Option<char>) -> KeyEvent {
        KeyEvent { key, ch, down: true, shift: false, ctrl: false, alt: false }
    }

    fn escribe(t: &mut Terminal, txt: &str, cx: &mut Ctx) {
        for c in txt.chars() {
            t.key(&ev(Key::Char(c), Some(c)), cx);
        }
        t.key(&ev(Key::Enter, Some('\n')), cx);
    }

    fn con_cx<R>(f: impl FnOnce(&mut Ctx) -> R) -> R {
        let mut sys = TestPlatform::default();
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        f(&mut cx)
    }

    #[test]
    fn ordenes_basicas() {
        assert_eq!(ejecuta("fecha").0, ["15/11/2026"]);
        assert_eq!(ejecuta("hora").0, ["21:37:05"]);
        assert_eq!(ejecuta("tiempo").0, ["1 h 2 min 3 s"]);
        assert_eq!(ejecuta("eco  Hola  Mundo ").0, ["Hola  Mundo"]);
        assert_eq!(ejecuta("").0.len(), 0);
        assert_eq!(ejecuta("   ").1, Action::Nothing);
        assert_eq!(ejecuta("limpiar").1, Action::Clear);
        assert_eq!(ejecuta("salir").1, Action::Close);
        assert_eq!(ejecuta("xyz").0, ["'xyz' no es una orden conocida. Escribe 'ayuda'."]);
        assert_eq!(ejecuta("acerca").0.len(), 2);
        assert!(ejecuta("ayuda").0.len() >= 20);
    }

    /// Ejecuta varias órdenes seguidas con la misma plataforma y carpeta actual.
    fn sesion(ordenes: &[&str]) -> (Vec<Vec<String>>, String, TestPlatform) {
        let mut p = TestPlatform::default();
        let mut cwd = p.files().home();
        let out = ordenes.iter().map(|o| run(o, &mut p, &mut cwd).0).collect();
        (out, cwd, p)
    }

    #[test]
    fn ir_cambia_carpeta_y_prompt() {
        let (o, cwd, _) = sesion(&["ir", "ir Documentos", "cd", "ir ..", "ir Escritorio", "ir ../Imágenes", "ir nada"]);
        assert_eq!(o[0], [home()]);
        assert!(o[1].is_empty());
        assert_eq!(o[2], [h("Documentos")]);
        assert_eq!(o[6], [FsError::NotFound.message()]);
        assert_eq!(cwd, h("Imágenes"));
        assert_eq!(prompt(&cwd), format!("{}> ", h("Imágenes")));
        let (o, cwd, _) = sesion(&["escribir a.txt hola", "ir a.txt", "ir a b"]);
        assert_eq!(o[1], [FsError::NotDir.message()]);
        assert_eq!(o[2], ["Uso: ir [ruta]"]);
        assert_eq!(cwd, home());
        // el indicador del Terminal sigue la carpeta
        con_cx(|cx| {
            let mut t = Terminal::new();
            escribe(&mut t, "ir Documentos", cx);
            assert_eq!(t.cwd, h("Documentos"));
            escribe(&mut t, "eco x", cx);
            assert_eq!(t.out[t.out.len() - 2], format!("{}> eco x", h("Documentos")));
        });
    }

    #[test]
    fn open_path_cambia_la_carpeta_solo_si_existe() {
        let mut sys = TestPlatform::default();
        let now = sys.now();
        sys.fs().write(&h("a.txt"), b"x", now).unwrap();
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        let mut t = Terminal::new();
        t.open_path(&h("Documentos"), &mut cx);
        assert_eq!(t.cwd, h("Documentos"));
        assert_eq!(prompt(&t.cwd), format!("{}> ", h("Documentos")));
        t.open_path(&h("nada"), &mut cx);
        t.open_path(&h("a.txt"), &mut cx);
        assert_eq!(t.cwd, h("Documentos"));
    }

    #[test]
    fn ctrl_y_mayus_sueltos_no_escriben() {
        con_cx(|cx| {
            let mut t = Terminal::new();
            let n = t.out.len();
            let m = KeyEvent { key: Key::Other, ch: None, down: true, shift: false, ctrl: true, alt: false };
            t.key(&m, cx);
            t.key(&KeyEvent { down: false, ctrl: false, ..m }, cx);
            assert!(t.input.is_empty() && t.out.len() == n);
        });
    }

    #[test]
    fn lista_y_disco() {
        let (o, _, _) = sesion(&[
            "carpeta Documentos\\sub",
            "escribir Documentos\\a.txt hola",
            "lista Documentos",
            "dir \"C:\\Usuarios\"",
            "lista a b",
            "lista nada",
        ]);
        let l = &o[2];
        assert_eq!(l[0], format!("Carpeta {}", h("Documentos")));
        assert!(l[1].starts_with("15/11/2026 21:37") && l[1].contains("<CARPETA>") && l[1].ends_with("sub"));
        assert!(l[2].contains(" 6  a.txt"), "{:?}", l[2]);
        assert!(l[3].starts_with("Archivos: 1, carpetas: 1, 6 bytes. Libre: "), "{:?}", l[3]);
        assert_eq!(o[3].len(), 1 + 1 + 1); // cabecera, la carpeta de la cuenta y el resumen
        assert_eq!(o[4], ["Uso: lista [ruta]"]);
        assert_eq!(o[5], [FsError::NotFound.message()]);
        let d = ejecuta("disco").0;
        assert_eq!(d[0], "Disco C:");
        assert_eq!(d[1], "Tamaño: 4,0 MB");
        assert!(d[2].starts_with("Libre: ") && d[3].contains("En memoria"));
    }

    #[test]
    fn ver_y_escribir() {
        let (o, _, mut p) = sesion(&[
            "escribir \"mi nota.txt\" Hola  mundo ñ",
            "ver \"mi nota.txt\"",
            "ver",
            "ver nada",
            "ver Documentos",
            "escribir x",
        ]);
        assert!(o[0].is_empty());
        assert_eq!(p.files().read(&h("mi nota.txt")).unwrap(), "Hola  mundo ñ\r\n".as_bytes());
        assert_eq!(o[1], ["Hola  mundo ñ"]);
        assert_eq!(o[2], ["Uso: ver archivo"]);
        assert_eq!(o[3], [FsError::NotFound.message()]);
        assert_eq!(o[4], [FsError::IsDir.message()]);
        assert_eq!(o[5], ["Uso: escribir archivo texto"]);
        // no UTF-8 y recorte
        let now = p.now();
        p.files().write(&h("bin"), &[0xff, 0xfe, 0], now).unwrap();
        let mut cwd = home();
        assert_eq!(run("ver bin", &mut p, &mut cwd).0, ["No es un archivo de texto (UTF-8)."]);
        let largo = "linea\n".repeat(500);
        p.files().write(&h("largo"), largo.as_bytes(), now).unwrap();
        let v = run("ver largo", &mut p, &mut cwd).0;
        assert_eq!(v.len(), VER_LINES + 1);
        assert!(v[VER_LINES].starts_with("..."));
    }

    #[test]
    fn carpeta_copiar_mover_renombrar() {
        let (o, _, mut p) = sesion(&[
            "carpeta \"Mi carpeta\"",
            "escribir Documentos\\a.txt uno",
            "copiar Documentos\\a.txt \"Mi carpeta\"",
            "copiar Documentos\\a.txt b.txt",
            "copiar Documentos\\a.txt b.txt",
            "mover b.txt Escritorio",
            "renombrar Escritorio\\b.txt c.txt",
            "renombrar Escritorio\\c.txt a\\b",
            "mover \"Mi carpeta\" \"Mi carpeta\\dentro\"",
            "copiar a",
            "mover a",
            "renombrar a",
            "carpeta",
            "carpeta Documentos",
            "mover nada x",
        ]);
        assert!(o[..4].iter().all(Vec::is_empty));
        assert_eq!(p.files().read(&join(&h("Mi carpeta"), "a.txt")).unwrap(), b"uno\r\n");
        assert_eq!(o[4], [FsError::Exists.message()]);
        assert!(!p.files().exists(&h("b.txt")));
        assert_eq!(p.files().read(&join(&h("Escritorio"), "c.txt")).unwrap(), b"uno\r\n");
        assert_eq!(o[7], [FsError::BadName.message()]);
        assert_eq!(o[8].len(), 1);
        assert_eq!(o[9], ["Uso: copiar origen destino"]);
        assert_eq!(o[10], ["Uso: mover origen destino"]);
        assert_eq!(o[11], ["Uso: renombrar ruta nuevo_nombre"]);
        assert_eq!(o[12], ["Uso: carpeta nombre"]);
        assert_eq!(o[13], [FsError::Exists.message()]);
        assert_eq!(o[14], [FsError::NotFound.message()]);
    }

    #[test]
    fn borrar_con_y_sin_todo() {
        let (o, cwd, mut p) = sesion(&[
            "carpeta Documentos\\p",
            "escribir Documentos\\p\\a.txt x",
            "escribir Documentos\\suelto.txt x",
            "borrar Documentos\\suelto.txt",
            "borrar Documentos\\p",
            "ir Documentos\\p",
            "borrar /TODO .",
            "carpeta vacia",
            "borrar vacia",
            "borrar",
            "borrar /todo",
            "borrar nada",
            "borrar /todo C:\\Sistema",
        ]);
        assert!(o[3].is_empty());
        assert_eq!(o[4].len(), 2);
        assert_eq!(o[4][0], FsError::NotEmpty.message());
        assert!(o[4][1].contains("borrar /todo"));
        assert!(o[6].is_empty());
        assert!(!p.files().exists(&join(&h("Documentos"), "p")));
        assert_eq!(cwd, h("Documentos"));
        assert!(o[8].is_empty() && !p.files().exists(&join(&h("Documentos"), "vacia")));
        assert_eq!(o[9], ["Uso: borrar [/todo] ruta"]);
        assert_eq!(o[10], ["Uso: borrar [/todo] ruta"]);
        assert_eq!(o[11], [FsError::NotFound.message()]);
        assert_eq!(o[12], [FsError::Invalid.message()]);
    }

    #[test]
    fn arranca_en_la_carpeta_del_usuario_y_el_indicador_la_muestra() {
        let mut sys = con_disco("Luis");
        let mut t = Terminal::new();
        let mut s = Surface::new(320, 160, 0);
        t.draw(&mut s, &Theme::default(), &mut sys, true);
        assert_eq!(t.cwd, join(USERS_DIR, "Luis"));
        assert_eq!(prompt(&t.cwd), "C:\\Usuarios\\Luis> ");
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        escribe(&mut t, "ir Documentos", &mut cx);
        assert_eq!(t.out[t.out.len() - 1], "C:\\Usuarios\\Luis> ir Documentos");
        assert_eq!(prompt(&t.cwd), "C:\\Usuarios\\Luis\\Documentos> ");
        // Aunque la primera cosa sea una tecla: la carpeta se fija al ejecutar.
        let mut t = Terminal::new();
        escribe(&mut t, "ir", &mut cx);
        assert_eq!(t.out.last().unwrap(), "C:\\Usuarios\\Luis");
    }

    #[test]
    fn quien_muestra_usuario_y_tipo() {
        let mut p = con_disco("Ana");
        assert_eq!(run("quien", &mut p, &mut home()).0, ["Usuario: Ana", "Tipo: administrador"]);
        let mut p = con_disco("Luis");
        assert_eq!(run("QUIEN", &mut p, &mut home()).0, ["Usuario: Luis", "Tipo: usuario normal"]);
        p.files.logout();
        assert_eq!(run("quien", &mut p, &mut home()).0, ["No hay ninguna sesión abierta."]);
        assert!(ejecuta("ayuda").0.iter().any(|l| l.trim_start().starts_with("quien")));
    }

    /// Escribe órdenes en un terminal ya abierto y devuelve lo que salió tras la última.
    fn teclea(t: &mut Terminal, cx: &mut Ctx, orden: &str) -> Vec<String> {
        let n = t.out.len();
        escribe(t, orden, cx);
        t.out[n + 1..].to_vec()
    }

    #[test]
    fn en_sistema_pide_si_y_solo_si_continua() {
        let mut sys = con_disco("Ana");
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        let mut t = Terminal::new();
        let conf = String::from(CONFIRM);
        // Con "si" se ejecuta.
        assert_eq!(teclea(&mut t, &mut cx, "carpeta C:\\Sistema\\temp\\prueba"), [conf.clone()]);
        assert!(!cx.sys.fs().exists("C:\\Sistema\\temp\\prueba"));
        assert!(teclea(&mut t, &mut cx, "si").is_empty());
        assert!(cx.sys.fs().exists("C:\\Sistema\\temp\\prueba"));
        // Con otra cosa se cancela y esa otra cosa no se ejecuta como orden.
        assert_eq!(teclea(&mut t, &mut cx, "escribir C:\\Sistema\\temp\\n.txt hola"), [conf.clone()]);
        assert_eq!(teclea(&mut t, &mut cx, "eco hola"), ["Cancelado."]);
        assert!(!cx.sys.fs().exists("C:\\Sistema\\temp\\n.txt"));
        // Tras cancelar, el siguiente comando va normal.
        assert_eq!(teclea(&mut t, &mut cx, "eco x"), ["x"]);
        // Origen o destino dentro de Sistema, y renombrar y borrar.
        let docs = join(&t.cwd, "Documentos");
        cx.sys.fs().write(&join(&docs, "a.txt"), b"x", DateTime::default()).unwrap();
        for orden in ["copiar Documentos\\a.txt C:\\Sistema\\temp", "copiar C:\\Sistema\\config\\cuentas Documentos\\c.txt", "mover Documentos\\a.txt C:\\Sistema\\temp", "borrar /todo C:\\Sistema\\temp\\prueba", "renombrar C:\\Sistema\\temp\\prueba otra"] {
            assert_eq!(teclea(&mut t, &mut cx, orden), [conf.clone()], "{orden}");
            assert_eq!(teclea(&mut t, &mut cx, "no"), ["Cancelado."]);
        }
        assert!(cx.sys.fs().exists("C:\\Sistema\\temp\\prueba") && cx.sys.fs().exists(&join(&docs, "a.txt")));
        // Escape también cancela.
        teclea(&mut t, &mut cx, "borrar C:\\Sistema\\temp\\prueba");
        t.key(&ev(Key::Escape, None), &mut cx);
        assert_eq!(teclea(&mut t, &mut cx, "si"), ["'si' no es una orden conocida. Escribe 'ayuda'."]);
        // Fuera de Sistema no pregunta.
        assert!(teclea(&mut t, &mut cx, "borrar Documentos\\a.txt").is_empty());
        // Preguntar ni ejecuta ni registra la orden dos veces en el historial.
        assert_eq!(t.history.iter().filter(|h| h.starts_with("borrar C:\\Sistema")).count(), 1);
    }

    #[test]
    fn un_usuario_normal_recibe_el_mensaje_sin_confirmacion() {
        let mut sys = con_disco("Luis");
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, 0);
        let mut t = Terminal::new();
        let denied = [FsError::Denied.message()];
        // Escribir en Sistema y entrar o escribir en la carpeta de otro usuario.
        assert_eq!(teclea(&mut t, &mut cx, "carpeta C:\\Sistema\\temp\\x"), denied);
        assert_eq!(teclea(&mut t, &mut cx, "escribir C:\\Sistema\\temp\\x.txt hola"), denied);
        assert_eq!(teclea(&mut t, &mut cx, "ir C:\\Usuarios\\Ana"), denied);
        assert_eq!(teclea(&mut t, &mut cx, "ver C:\\Usuarios\\Ana\\Documentos\\secreto.txt"), denied);
        assert_eq!(teclea(&mut t, &mut cx, "lista C:\\Usuarios\\Ana\\Documentos"), denied);
        assert_eq!(teclea(&mut t, &mut cx, "copiar Documentos C:\\Usuarios\\Ana\\Documentos"), denied);
        assert_eq!(teclea(&mut t, &mut cx, "borrar C:\\Usuarios\\Ana\\Documentos\\secreto.txt"), denied);
        assert!(teclea(&mut t, &mut cx, "escribir Documentos\\mio.txt hola").is_empty());
        assert_eq!(teclea(&mut t, &mut cx, "mover Documentos\\mio.txt C:\\Usuarios\\Ana\\Documentos"), denied);
        assert_eq!(teclea(&mut t, &mut cx, "renombrar C:\\Usuarios\\Ana\\Documentos\\secreto.txt otro.txt"), denied);
        assert_eq!(t.cwd, "C:\\Usuarios\\Luis");
        // Sus propias cosas, con normalidad.
        assert!(teclea(&mut t, &mut cx, "mover Documentos\\mio.txt Escritorio").is_empty());
    }

    #[test]
    fn ayuda_agrupada_y_unidades() {
        let a = ejecuta("ayuda").0;
        assert!(a[0] == "Sistema:" && a.iter().any(|l| l.starts_with("Archivos")));
        for o in ["lista", "ir", "ver", "carpeta", "copiar", "mover", "renombrar", "borrar", "escribir", "disco"] {
            assert!(a.iter().any(|l| l.trim_start().starts_with(o)), "{o}");
        }
        assert_eq!(units(0), "0,0 KB");
        assert_eq!(units(1536), "1,5 KB");
        assert_eq!(units(4 << 20), "4,0 MB");
        assert_eq!(units(3 << 30), "3,0 GB");
        assert_eq!(tokens("a \"b c\"  d"), ["a", "b c", "d"]);
    }

    #[test]
    fn info_y_memoria() {
        let i = ejecuta("info").0;
        assert_eq!(i[0], format!("Sistema: {} {}", NAME, VERSION));
        assert_eq!(i[1], "Procesador: CPU de pruebas");
        assert_eq!(i[2], "Pantalla: 1024x768");
        let m = ejecuta("memoria").0;
        assert_eq!(m[0], "RAM total: 1024,0 MB");
        assert_eq!(m[1], "Memoria del sistema, total: 256,0 MB");
        assert_eq!(m[2], "Memoria del sistema, usada: 20,0 MB");
        assert_eq!(m[3], "Memoria del sistema, libre: 236,0 MB");
    }

    #[test]
    fn programas_y_ejecutar() {
        let mut sys = TestPlatform::default();
        let n = crate::platform::DateTime::default();
        sys.files.install_system_program("Calculadora", &[("app.txt", b"nombre=Calculadora\nversion=1.0\n"), ("programa.nfx", b"NFX1")], n).unwrap();
        let mut cwd = String::from("C:\\");
        assert_eq!(run("programas", &mut sys, &mut cwd).0, ["  Calculadora  1.0"]);
        assert_eq!(run("ejecutar calculadora", &mut sys, &mut cwd).1, Action::Open(String::from("C:\\Programas\\Calculadora\\programa.nfx")));
        assert_eq!(run("ejecutar Programas\\Calculadora\\programa.nfx", &mut sys, &mut cwd).1, Action::Open(String::from("C:\\Programas\\Calculadora\\programa.nfx")));
        assert_eq!(run("ejecutar nada", &mut sys, &mut cwd).1, Action::Nothing);
        assert_eq!(run("ejecutar Programas\\Calculadora\\app.txt", &mut sys, &mut cwd).1, Action::Nothing);
    }

    #[test]
    fn mayusculas_indiferentes() {
        assert_eq!(ejecuta("FECHA"), ejecuta("fecha"));
        assert_eq!(ejecuta("Salir").1, Action::Close);
        assert_eq!(ejecuta("ECO Ñu").0, ["Ñu"]);
    }

    #[test]
    fn apagar_y_reiniciar_llaman_a_la_plataforma() {
        let mut p = TestPlatform::default();
        run("reiniciar", &mut p, &mut String::new());
        assert!(p.rebooted && !p.halted);
        run("APAGAR", &mut p, &mut String::new());
        assert!(p.halted);
    }

    #[test]
    fn terminal_salir_cierra_y_limpiar_borra() {
        con_cx(|cx| {
            let mut t = Terminal::new();
            escribe(&mut t, "fecha", cx);
            assert_eq!(t.out.last().unwrap(), "15/11/2026");
            escribe(&mut t, "limpiar", cx);
            assert!(t.out.is_empty());
            assert!(!cx.close);
            escribe(&mut t, "salir", cx);
            assert!(cx.close);
            // Línea vacía: solo repite el indicador.
            let mut t = Terminal::new();
            let n = t.out.len();
            t.key(&ev(Key::Enter, Some('\n')), cx);
            assert_eq!(t.out.len(), n + 1);
            assert_eq!(t.out[n], prompt(&home()));
        });
    }

    #[test]
    fn historial_con_flechas() {
        con_cx(|cx| {
            let mut t = Terminal::new();
            escribe(&mut t, "hora", cx);
            escribe(&mut t, "fecha", cx);
            t.key(&ev(Key::Up, None), cx);
            assert_eq!(t.input, "fecha");
            t.key(&ev(Key::Up, None), cx);
            assert_eq!(t.input, "hora");
            t.key(&ev(Key::Up, None), cx);
            assert_eq!(t.input, "hora");
            t.key(&ev(Key::Down, None), cx);
            assert_eq!(t.input, "fecha");
            t.key(&ev(Key::Down, None), cx);
            assert_eq!(t.input, "");
            t.key(&ev(Key::Char('a'), Some('a')), cx);
            t.key(&ev(Key::Escape, None), cx);
            assert_eq!(t.input, "");
        });
    }

    #[test]
    fn limite_de_500_lineas() {
        con_cx(|cx| {
            let mut t = Terminal::new();
            for i in 0..700 {
                escribe(&mut t, &format!("eco {}", i), cx);
            }
            assert_eq!(t.out.len(), MAX_LINES);
            assert_eq!(t.out.last().unwrap(), "699");
        });
    }

    #[test]
    fn rueda_desplaza_el_historial() {
        let mut sys = TestPlatform::default();
        let mut t = Terminal::new();
        con_cx(|cx| {
            for i in 0..60 {
                escribe(&mut t, &format!("eco {}", i), cx);
            }
            let mut s = Surface::new(320, 160, 0);
            t.draw(&mut s, &Theme::default(), &mut sys, true);
            assert!(t.max_scroll > 6);
            t.mouse(MouseEv::Wheel { x: 5, y: 5, dz: 1 }, cx); // abajo en el final: tope
            assert_eq!(t.scroll, 0);
            t.mouse(MouseEv::Wheel { x: 5, y: 5, dz: -1 }, cx);
            assert_eq!(t.scroll, 3);
            t.mouse(MouseEv::Wheel { x: 5, y: 5, dz: -1 }, cx);
            assert_eq!(t.scroll, 6);
            t.mouse(MouseEv::Wheel { x: 5, y: 5, dz: 1 }, cx);
            assert_eq!(t.scroll, 3);
            t.mouse(MouseEv::Wheel { x: 5, y: 5, dz: -1000 }, cx); // arriba: tope
            assert_eq!(t.scroll, t.max_scroll);
            t.key(&ev(Key::Char('x'), Some('x')), cx);
            assert_eq!(t.scroll, 0);
        });
    }

    #[test]
    fn barra_solo_si_hace_falta_y_desplaza() {
        let mut sys = TestPlatform::default();
        let mut t = Terminal::new();
        con_cx(|cx| {
            let mut s = Surface::new(320, 160, 0);
            t.draw(&mut s, &Theme::default(), &mut sys, true);
            assert!(!t.metrics().needed());
            assert_eq!(s.px[(80 * 320 + 310) as usize], BG);
            for i in 0..60 {
                escribe(&mut t, &format!("eco {}", i), cx);
            }
            t.draw(&mut s, &Theme::default(), &mut sys, true);
            let m = t.metrics();
            assert!(m.needed() && t.scroll == 0 && m.pos == m.max());
            assert_ne!(s.px[(80 * 320 + 310) as usize], BG);
            let bx = m.rect.x + 5;
            // Flecha de arriba: una fila hacia atrás.
            t.mouse(MouseEv::Down { x: bx, y: 3 }, cx);
            t.mouse(MouseEv::Up { x: bx, y: 3 }, cx);
            assert_eq!(t.scroll, 1);
            // Flecha de abajo: vuelve.
            t.mouse(MouseEv::Down { x: bx, y: 157 }, cx);
            t.mouse(MouseEv::Up { x: bx, y: 157 }, cx);
            assert_eq!(t.scroll, 0);
            // Pista sobre el tirador (abajo): una página hacia arriba.
            t.mouse(MouseEv::Down { x: bx, y: 40 }, cx);
            t.mouse(MouseEv::Up { x: bx, y: 40 }, cx);
            assert_eq!(t.scroll, t.vis);
            // Arrastre hasta arriba del todo y vuelta abajo.
            let (ts, _) = t.sb.thumb(&t.metrics());
            t.mouse(MouseEv::Down { x: bx, y: ts + 2 }, cx);
            t.mouse(MouseEv::Move { x: 900, y: -900 }, cx);
            assert_eq!(t.scroll, t.max_scroll);
            t.mouse(MouseEv::Move { x: 900, y: 9000 }, cx);
            assert_eq!(t.scroll, 0);
            t.mouse(MouseEv::Up { x: 0, y: 0 }, cx);
            assert!(!t.sb.active());
        });
    }

    #[test]
    fn desplazamiento_y_dibujo() {
        let mut sys = TestPlatform::default();
        let mut t = Terminal::new();
        con_cx(|cx| {
            for i in 0..60 {
                escribe(&mut t, &format!("eco {} {}", i, "palabra ".repeat(40)), cx);
            }
            let mut s = Surface::new(320, 160, 0);
            t.draw(&mut s, &Theme::default(), &mut sys, true);
            assert!(t.max_scroll > 0);
            t.key(&ev(Key::PageUp, None), cx);
            assert!(t.scroll > 0);
            t.key(&ev(Key::PageDown, None), cx);
            assert_eq!(t.scroll, 0);
            t.key(&ev(Key::PageUp, None), cx);
            t.key(&ev(Key::Char('x'), Some('x')), cx);
            assert_eq!(t.scroll, 0);
        });
        for (w, h) in [(320, 160), (1, 1), (0, 0), (50, 20), (680, 400)] {
            let mut s = Surface::new(w, h, 0);
            t.draw(&mut s, &Theme::default(), &mut sys, true);
            t.draw(&mut s, &Theme::default(), &mut sys, false);
        }
    }

    #[test]
    fn partir_por_ancho() {
        assert_eq!(wrap("", 100), [""]);
        let w = font::width("abc", SIZE, false);
        let r = wrap("abcabcabc", w);
        assert_eq!(r, ["abc", "abc", "abc"]);
        assert_eq!(wrap("x", 0), ["x"]);
    }

    #[test]
    fn parpadeo() {
        con_cx(|cx| {
            let mut t = Terminal::new();
            t.focused = true;
            cx.now_ms = 100;
            t.tick(cx);
            assert!(!cx.redraw);
            cx.now_ms = 600;
            t.tick(cx);
            assert!(cx.redraw && !t.shown);
        });
    }

    // ---- órdenes de red (Lan: el router simulado de la pila) ----

    use crate::net::tests::{Lan, SRV};
    use crate::net::Net;

    struct Red {
        t: Terminal,
        sys: TestPlatform,
        lan: Lan,
        th: Theme,
    }

    impl Red {
        fn new(lan: Lan) -> Red {
            Red::con(TestPlatform::default(), lan)
        }
        /// La pila vive en la plataforma; `lan` guarda una vacía y la toma prestada en cada paso.
        fn con(mut sys: TestPlatform, mut lan: Lan) -> Red {
            let mut net = Net::new([0; 32]);
            core::mem::swap(&mut lan.net, &mut net);
            sys.net = Some(net);
            Red { t: Terminal::new(), sys, lan, th: Theme::default() }
        }
        fn paso(&mut self) {
            core::mem::swap(&mut self.lan.net, self.sys.net.as_mut().unwrap());
            self.lan.pump();
            core::mem::swap(&mut self.lan.net, self.sys.net.as_mut().unwrap());
            let mut cx = Ctx::new(&mut self.sys, &mut self.th, self.lan.now);
            self.t.tick(&mut cx);
        }
        fn corre(&mut self, ms: u64) {
            for _ in 0..ms / 20 {
                self.lan.now += 20;
                self.paso();
            }
        }
        fn tecla(&mut self, e: KeyEvent) {
            let mut cx = Ctx::new(&mut self.sys, &mut self.th, self.lan.now);
            self.t.key(&e, &mut cx);
        }
        fn ctrl_c(&mut self) {
            self.tecla(KeyEvent { key: Key::Char('c'), ch: None, down: true, shift: false, ctrl: true, alt: false });
        }
        /// Escribe la orden y pulsa Intro.
        fn lanza(&mut self, l: &str) {
            for c in l.chars() {
                self.tecla(ev(Key::Char(c), Some(c)));
            }
            self.tecla(ev(Key::Enter, Some('\n')));
        }
        /// Escribe la orden, espera a que acabe y devuelve lo que salió (sin la línea de la orden).
        fn orden(&mut self, l: &str) -> Vec<String> {
            let n = self.t.out.len();
            self.lanza(l);
            for _ in 0..3000 {
                if self.t.job.is_none() {
                    return self.t.out[n + 1..].to_vec();
                }
                self.corre(20);
            }
            panic!("la orden no termina: {:?}", &self.t.out[n..]);
        }
    }

    fn con_red() -> Red {
        let mut lan = Lan::up();
        lan.router.names.push((String::from("ejemplo.com"), vec![SRV], 300));
        Red::new(lan)
    }

    #[test]
    fn ipconfig_con_y_sin_cable() {
        let mut r = con_red();
        assert_eq!(
            r.orden("ipconfig"),
            [
                "Configuración IP de NightFury",
                "",
                "Adaptador de Ethernet Ethernet:",
                "",
                "   Dirección IPv4. . . . . . . . . . . . . . : 192.168.1.50",
                "   Máscara de subred . . . . . . . . . . . . : 255.255.255.0",
                "   Puerta de enlace predeterminada . . . . . : 192.168.1.1",
            ]
        );
        let all = r.orden("ipconfig /all");
        assert_eq!(all[4], "   Descripción . . . . . . . . . . . . . . . : Prueba");
        assert_eq!(all[5], "   Dirección física. . . . . . . . . . . . . : 02-00-00-00-00-01");
        assert_eq!(all[6], "   DHCP habilitado . . . . . . . . . . . . . : sí");
        assert_eq!(all[7], "   Velocidad . . . . . . . . . . . . . . . . : 1 Gb/s, dúplex completo");
        assert_eq!(all[8], "   Dirección IPv4. . . . . . . . . . . . . . : 192.168.1.50");
        assert_eq!(all[11], "   Servidor DHCP . . . . . . . . . . . . . . : 192.168.1.1");
        assert!(all[12].starts_with("   Concesión") && (all[12].ends_with(" h 59 min") || all[12].ends_with(" h 0 min")), "{:?}", all[12]);
        assert_eq!(all[13], "   Servidores DNS. . . . . . . . . . . . . . : 192.168.1.1");
        assert_eq!(all[14], format!("{:47}192.168.1.2", ""));
        assert_eq!(r.orden("ipconfig /x"), ["Uso: ipconfig [/all | /renew]"]);
        // Sin cable.
        r.lan.wire.borrow_mut().link = None;
        r.corre(100);
        let s = r.orden("ipconfig");
        assert_eq!(s[4], "   Estado de los medios. . . . . . . . . . . : medios desconectados");
        assert_eq!(s.len(), 5);
        assert!(r.orden("ipconfig /all").iter().all(|l| !l.contains("IPv4") && !l.contains("Velocidad")));
        assert_eq!(r.orden("ping 93.184.216.34"), ["El cable de red está desconectado."]);
        assert_eq!(r.orden("ipconfig /renew"), ["El cable de red está desconectado."]);
    }

    #[test]
    fn sin_tarjeta_y_sin_direccion() {
        con_cx(|cx| {
            let mut t = Terminal::new();
            for o in ["ping 1.1.1.1", "ipconfig", "nslookup a.org", "curl http://a.org/"] {
                assert_eq!(teclea(&mut t, cx, o), ["No hay ninguna tarjeta de red."], "{o}");
            }
        });
        let mut lan = Lan::new();
        lan.router.dhcp_on = false;
        let mut r = Red::new(lan);
        r.corre(100);
        assert_eq!(r.orden("ping 1.1.1.1"), ["Todavía no hay dirección IP."]);
        assert_eq!(r.orden("nslookup a.org"), ["Todavía no hay dirección IP."]);
        assert_eq!(r.orden("ipconfig")[4], "   Pidiendo dirección…");
    }

    #[test]
    fn renovar_la_direccion() {
        let mut r = con_red();
        let lineas = r.orden("ipconfig");
        r.lanza("ipconfig /renew");
        assert!(r.t.job.is_some(), "espera mientras se configura");
        let n = r.t.out.len();
        for _ in 0..3000 {
            if r.t.job.is_none() {
                break;
            }
            r.corre(20);
        }
        assert_eq!(r.t.out[n..], lineas);
        // Sin servidor DHCP: se rinde.
        r.lan.router.dhcp_on = false;
        assert_eq!(r.orden("ipconfig /renew"), ["No se pudo renovar la dirección."]);
    }

    #[test]
    fn ping_con_respuestas_y_con_tiempo_agotado() {
        let mut r = con_red();
        let t0 = r.lan.now;
        let o = r.orden("ping 93.184.216.34");
        assert!(r.lan.now - t0 >= 3000, "uno por segundo");
        assert_eq!(o[0], "Haciendo ping a 93.184.216.34 con 32 bytes de datos:");
        for l in &o[1..5] {
            assert!(l.starts_with("Respuesta desde 93.184.216.34: bytes=32 tiempo") && l.ends_with("TTL=64"), "{l}");
        }
        assert_eq!(o[5], "");
        assert_eq!(o[6], "Estadísticas de ping para 93.184.216.34:");
        assert_eq!(o[7], "    Paquetes: enviados = 4, recibidos = 4, perdidos = 0");
        assert_eq!(o[8], "    (0% perdidos),");
        assert_eq!(o[9], "Tiempos aproximados de ida y vuelta en milisegundos:");
        assert!(o[10].starts_with("    Mínimo = ") && o[10].contains("Máximo = ") && o[10].contains("Media = "));
        // Por nombre, con -n.
        let o = r.orden("ping -n 1 ejemplo.com");
        assert_eq!(o[0], "Haciendo ping a ejemplo.com [93.184.216.34] con 32 bytes de datos:");
        assert!(o[1].starts_with("Respuesta desde 93.184.216.34") && o[4].contains("enviados = 1, recibidos = 1"));
        // Nadie contesta.
        let o = r.orden("ping -n 1 8.8.8.8");
        assert_eq!(o[1], "Tiempo de espera agotado para esta solicitud.");
        assert_eq!(o[4], "    Paquetes: enviados = 1, recibidos = 0, perdidos = 1");
        assert_eq!(o[5], "    (100% perdidos),");
        assert_eq!(o.len(), 6, "sin recibidos no hay bloque de tiempos");
        // Destino inaccesible.
        r.lan.router.unreach.push([10, 9, 9, 9]);
        let o = r.orden("ping -n 1 10.9.9.9");
        assert_eq!(o[1], "Respuesta desde 192.168.1.1: Host de destino inaccesible.");
        // Nombre que no existe y opciones mal puestas.
        assert_eq!(r.orden("ping noexiste.invalid"), ["La solicitud de ping no pudo encontrar el host noexiste.invalid. Compruebe el nombre y vuelva a intentarlo."]);
        for o in ["ping", "ping -n", "ping -n 0 a", "ping -n x a", "ping -z a", "ping a b"] {
            assert_eq!(r.orden(o), ["Uso: ping [-n N] [-t] destino"], "{o}");
        }
    }

    #[test]
    fn ctrl_c_corta_un_ping_sin_fin() {
        let mut r = con_red();
        let n = r.t.out.len();
        r.lanza("ping -t 93.184.216.34");
        r.corre(2500);
        assert!(r.t.job.is_some());
        // Mientras dura, el teclado no escribe.
        r.tecla(ev(Key::Char('x'), Some('x')));
        assert!(r.t.input.is_empty());
        r.ctrl_c();
        assert!(r.t.job.is_none());
        let o = &r.t.out[n + 1..];
        let k = o.len();
        assert_eq!((o[k - 2].as_str(), o[k - 1].as_str()), ("Control-C", "^C"));
        assert!(o.iter().any(|l| l == "Estadísticas de ping para 93.184.216.34:"));
        assert!(o.iter().filter(|l| l.starts_with("Respuesta desde")).count() >= 3);
        // Vuelve el indicador y se puede escribir.
        r.tecla(ev(Key::Char('y'), Some('y')));
        assert_eq!(r.t.input, "y");
        // La orden no se queda corriendo.
        let n = r.t.out.len();
        r.corre(3000);
        assert_eq!(r.t.out.len(), n);
    }

    #[test]
    fn nslookup_resuelve_y_cuenta_los_fallos() {
        let mut r = con_red();
        r.lan.router.names.push((String::from("multi.org"), vec![SRV, [93, 184, 216, 35]], 300));
        assert_eq!(r.orden("nslookup ejemplo.com"), ["Servidor:  192.168.1.1", "", "Nombre:    ejemplo.com", "Dirección: 93.184.216.34"]);
        assert_eq!(r.orden("nslookup multi.org")[3..], ["Direcciones: 93.184.216.34", "             93.184.216.35"]);
        assert_eq!(r.orden("nslookup 1.2.3.4")[2..], ["Nombre:    1.2.3.4", "Dirección: 1.2.3.4"]);
        assert_eq!(r.orden("nslookup ejemplo.invalid"), ["*** 192.168.1.1 no encuentra ejemplo.invalid: el dominio no existe"]);
        assert_eq!(r.orden("nslookup"), ["Uso: nslookup host"]);
        r.lan.router.dns_hook = Some(Box::new(|_, _, _| Vec::new()));
        assert_eq!(r.orden("nslookup otro.org"), ["*** Se agotó el tiempo de espera de la solicitud a 192.168.1.1"]);
    }

    #[test]
    fn curl_cuerpo_cabeceras_y_guardado() {
        let mut r = con_red();
        let cuerpo = "hola\nmundo\u{1}\tx\n";
        let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{cuerpo}", cuerpo.len());
        r.lan.router.srv.listen(80, resp.as_bytes(), true, false);
        assert_eq!(r.orden("curl http://ejemplo.com/"), ["hola", "mundo.    x"]);
        assert_eq!(r.orden("curl -L ejemplo.com"), ["hola", "mundo.    x"]);
        assert_eq!(r.orden("curl -I http://ejemplo.com/"), ["HTTP/1.1 200 OK", "Content-Type: text/plain", &format!("Content-Length: {}", cuerpo.len())]);
        assert_eq!(r.orden("curl -o pagina.txt http://ejemplo.com/"), [format!("Guardado pagina.txt ({} bytes)", cuerpo.len())]);
        assert_eq!(r.sys.fs().read(&h("pagina.txt")).unwrap(), cuerpo.as_bytes());
        // HTTPS, URL mala, errores de conexión y de uso.
        assert_eq!(r.orden("curl https://ejemplo.com/"), ["HTTPS llega en una versión próxima"]);
        assert_eq!(r.orden("curl http://"), ["curl: (3) URL no válida"]);
        assert_eq!(r.orden("curl http://noexiste.invalid/"), ["curl: (6) No se pudo resolver el host: noexiste.invalid"]);
        assert_eq!(r.orden("curl http://ejemplo.com:81/"), ["curl: (7) No se pudo conectar con ejemplo.com puerto 81"]);
        for o in ["curl", "curl -o", "curl -x a", "curl a b", "curl -I -o f a"] {
            let l = r.orden(o);
            assert!(l[0].starts_with("Uso: curl") || l[0].starts_with("curl: -I"), "{o}");
        }
        // Ctrl+C mientras conecta.
        r.lan.router.srv.ignore_syn = u32::MAX;
        let n = r.t.out.len();
        r.lanza("curl http://ejemplo.com/");
        r.corre(500);
        assert!(r.t.job.is_some());
        r.ctrl_c();
        assert!(r.t.job.is_none() && r.t.out[n + 1..] == ["^C"]);
    }

    #[test]
    fn curl_cuerpo_con_fallo_a_medias() {
        let mut r = con_red();
        // Promete más de lo que envía y corta: error de respuesta incompleta, con lo recibido ya escrito.
        r.lan.router.srv.listen(80, b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\nuna linea\nsin fin", true, false);
        let o = r.orden("curl ejemplo.com");
        assert_eq!(o[0], "una linea");
        assert!(o[1] == "sin fin" && o[2].starts_with("curl: (8) Respuesta no válida"), "{o:?}");
    }

    #[test]
    fn curl_a_sistema_pide_confirmacion() {
        let mut lan = Lan::up();
        lan.router.names.push((String::from("ejemplo.com"), vec![SRV], 300));
        lan.router.srv.listen(80, b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc", true, false);
        let mut r = Red::con(con_disco("Ana"), lan);
        assert_eq!(r.orden("curl -o C:\\Sistema\\temp\\x.bin ejemplo.com"), [CONFIRM]);
        assert!(!r.sys.fs().exists("C:\\Sistema\\temp\\x.bin"));
        assert_eq!(r.orden("si"), ["Guardado C:\\Sistema\\temp\\x.bin (3 bytes)"]);
        assert_eq!(r.sys.fs().read("C:\\Sistema\\temp\\x.bin").unwrap(), b"abc");
        // Una carpeta como destino se rechaza antes de descargar.
        assert_eq!(r.orden("curl -o Documentos ejemplo.com"), [FsError::IsDir.message()]);
    }

    /// La Terminal como programa en modo usuario: la red le llega por `RemoteNet` + `serve`.
    #[test]
    fn ping_y_curl_por_el_servicio() {
        use crate::service::tests::{prog, Prog};
        let (real, mut p) = prog(true);
        let mut lan = Lan::up();
        lan.router.names.push((String::from("ejemplo.com"), vec![SRV], 300));
        let cuerpo = "hola\nmundo\n";
        let resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{cuerpo}", cuerpo.len());
        lan.router.srv.listen(80, resp.as_bytes(), true, false);
        real.borrow_mut().net = Some(core::mem::replace(&mut lan.net, Net::new([0; 32])));
        let (mut t, mut th) = (Terminal::new(), Theme::default());
        let mut orden = |l: &str, t: &mut Terminal, p: &mut Prog| -> Vec<String> {
            let n = t.out.len();
            for c in l.chars().map(|c| ev(Key::Char(c), Some(c))).chain([ev(Key::Enter, Some('\n'))]) {
                t.key(&c, &mut Ctx::new(p, &mut th, lan.now));
            }
            for _ in 0..3000 {
                if t.job.is_none() {
                    return t.out[n + 1..].to_vec();
                }
                lan.now += 20;
                {
                    let mut r = real.borrow_mut();
                    let net = r.net.as_mut().unwrap();
                    core::mem::swap(&mut lan.net, net);
                    lan.pump();
                    core::mem::swap(&mut lan.net, net);
                }
                t.tick(&mut Ctx::new(p, &mut th, lan.now));
            }
            panic!("la orden no termina: {:?}", &t.out[n..]);
        };
        let o = orden("ping -n 1 ejemplo.com", &mut t, &mut p);
        assert!(o.iter().any(|l| l.starts_with("Respuesta desde 93.184.216.34")), "{o:?}");
        assert_eq!(orden("curl http://ejemplo.com/", &mut t, &mut p), ["hola", "mundo"]);
        assert_eq!(orden("nslookup ejemplo.com", &mut t, &mut p)[3], "Dirección: 93.184.216.34");
        assert_eq!(orden("ipconfig", &mut t, &mut p)[4], "   Dirección IPv4. . . . . . . . . . . . . . : 192.168.1.50");
    }
}
