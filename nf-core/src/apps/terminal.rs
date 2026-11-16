//! Terminal: consola de órdenes del sistema. CONTRATO: `pub fn new() -> Box<dyn App>`.
use super::{App, AppKind, Ctx, MouseEv};
use crate::font;
use crate::geom::Rect;
use crate::files::{file_name, join, parent, resolve, valid_name};
use crate::fs::FsError;
use crate::input::{Key, KeyEvent};
use crate::platform::Platform;
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

/// Efecto de una orden sobre la ventana.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Nothing,
    Clear,
    Close,
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

fn tokens(s: &str) -> Vec<String> {
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
    let f = sys.files();
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
            out.push(format!("Archivos: {}, carpetas: {}, {} bytes. Libre: {}", nf, nd, bytes,units(f.space().1)));
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
            } else {
                f.rename(&src, &dst)?;
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
    /// Carpeta actual.
    cwd: String,
    input: String,
    history: Vec<String>,
    /// Posición en `history` al recorrerlo con las flechas; `history.len()` = línea nueva.
    hist_pos: usize,
    /// Filas desplazadas hacia arriba desde el final.
    scroll: usize,
    max_scroll: usize,
    vis: usize,
    focused: bool,
    t0: u64,
    shown: bool,
}

pub fn new() -> Box<dyn App> {
    Box::new(Terminal::new())
}

impl Terminal {
    fn new() -> Self {
        let mut t = Terminal {
            out: Vec::new(),
            cwd: String::from("C:\\"),
            input: String::new(),
            history: Vec::new(),
            hist_pos: 0,
            scroll: 0,
            max_scroll: 0,
            vis: 10,
            focused: false,
            t0: 0,
            shown: true,
        };
        t.push(format!("{} {}", NAME, VERSION));
        t.push(String::from("Escribe 'ayuda' para ver las órdenes."));
        t
    }

    fn push(&mut self, l: String) {
        self.out.push(l);
        if self.out.len() > MAX_LINES {
            self.out.drain(..self.out.len() - MAX_LINES);
        }
    }

    fn submit(&mut self, sys: &mut dyn Platform) -> Action {
        let line = core::mem::take(&mut self.input);
        self.push(format!("{}{}", prompt(&self.cwd), line));
        let cmd = line.trim();
        if !cmd.is_empty() && self.history.last().map(String::as_str) != Some(cmd) {
            self.history.push(cmd.to_string());
        }
        self.hist_pos = self.history.len();
        let (lines, act) = run(&line, sys, &mut self.cwd);
        if act == Action::Clear {
            self.out.clear();
        }
        for l in lines {
            self.push(l);
        }
        act
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

    fn draw(&mut self, s: &mut Surface, _theme: &Theme, _sys: &mut dyn Platform, focused: bool) {
        self.focused = focused;
        s.fill_rect(s.bounds(), BG);
        let lh = font::line_h(SIZE);
        let max_w = (s.w - 2 * MARGIN).max(1);
        // Todas las filas visuales; la última línea lleva un espacio final que hace de cursor.
        let mut rows: Vec<String> = Vec::new();
        for l in &self.out {
            rows.extend(wrap(l, max_w));
        }
        let input_rows = wrap(&format!("{}{} ", prompt(&self.cwd), self.input), max_w);
        let n_input = input_rows.len();
        rows.extend(input_rows);

        self.vis = (((s.h - 2 * MARGIN) / lh).max(1)) as usize;
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
    }

    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
        match ev.key {
            Key::PageUp => self.scroll = (self.scroll + self.vis.saturating_sub(1).max(1)).min(self.max_scroll),
            Key::PageDown => self.scroll = self.scroll.saturating_sub(self.vis.saturating_sub(1).max(1)),
            Key::Enter => {
                self.scroll = 0;
                if self.submit(&mut *cx.sys) == Action::Close {
                    cx.close = true;
                }
            }
            Key::Backspace => {
                self.scroll = 0;
                self.input.pop();
            }
            Key::Escape => {
                self.scroll = 0;
                self.input.clear();
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
        if let MouseEv::Wheel { dz, .. } = ev {
            // dz > 0 = hacia abajo (líneas recientes); `scroll` cuenta filas desde el final.
            self.scroll = (self.scroll as i64 - dz as i64 * 3).clamp(0, self.max_scroll as i64) as usize;
            cx.redraw = true;
        }
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

    fn ejecuta(l: &str) -> (Vec<String>, Action) {
        run(l, &mut TestPlatform { ms: 3_723_000, size: (1024, 768), ..Default::default() }, &mut String::from("C:\\"))
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
        let mut cwd = String::from("C:\\");
        let out = ordenes.iter().map(|o| run(o, &mut p, &mut cwd).0).collect();
        (out, cwd, p)
    }

    #[test]
    fn ir_cambia_carpeta_y_prompt() {
        let (o, cwd, _) = sesion(&["ir", "ir Documentos", "cd", "ir ..", "ir C:\\Escritorio", "ir ../Imágenes", "ir nada"]);
        assert_eq!(o[0], ["C:\\"]);
        assert!(o[1].is_empty());
        assert_eq!(o[2], ["C:\\Documentos"]);
        assert_eq!(o[6], [FsError::NotFound.message()]);
        assert_eq!(cwd, "C:\\Imágenes");
        assert_eq!(prompt(&cwd), "C:\\Imágenes> ");
        let (o, cwd, _) = sesion(&["escribir a.txt hola", "ir a.txt", "ir a b"]);
        assert_eq!(o[1], [FsError::NotDir.message()]);
        assert_eq!(o[2], ["Uso: ir [ruta]"]);
        assert_eq!(cwd, "C:\\");
        // el indicador del Terminal sigue la carpeta
        con_cx(|cx| {
            let mut t = Terminal::new();
            escribe(&mut t, "ir Documentos", cx);
            assert_eq!(t.cwd, "C:\\Documentos");
            escribe(&mut t, "eco x", cx);
            assert_eq!(t.out[t.out.len() - 2], "C:\\Documentos> eco x");
        });
    }

    #[test]
    fn lista_y_disco() {
        let (o, _, _) = sesion(&[
            "carpeta Documentos\\sub",
            "escribir Documentos\\a.txt hola",
            "lista Documentos",
            "dir \"C:\\\"",
            "lista a b",
            "lista nada",
        ]);
        let l = &o[2];
        assert_eq!(l[0], "Carpeta C:\\Documentos");
        assert!(l[1].starts_with("15/11/2026 21:37") && l[1].contains("<CARPETA>") && l[1].ends_with("sub"));
        assert!(l[2].contains(" 6  a.txt"), "{:?}", l[2]);
        assert!(l[3].starts_with("Archivos: 1, carpetas: 1, 6 bytes. Libre: "), "{:?}", l[3]);
        assert_eq!(o[3].len(), 4 + 2);
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
        assert_eq!(p.files().read("C:\\mi nota.txt").unwrap(), "Hola  mundo ñ\r\n".as_bytes());
        assert_eq!(o[1], ["Hola  mundo ñ"]);
        assert_eq!(o[2], ["Uso: ver archivo"]);
        assert_eq!(o[3], [FsError::NotFound.message()]);
        assert_eq!(o[4], [FsError::IsDir.message()]);
        assert_eq!(o[5], ["Uso: escribir archivo texto"]);
        // no UTF-8 y recorte
        let now = p.now();
        p.files().write("C:\\bin", &[0xff, 0xfe, 0], now).unwrap();
        let mut cwd = String::from("C:\\");
        assert_eq!(run("ver bin", &mut p, &mut cwd).0, ["No es un archivo de texto (UTF-8)."]);
        let largo = "linea\n".repeat(500);
        p.files().write("C:\\largo", largo.as_bytes(), now).unwrap();
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
        assert_eq!(p.files().read("C:\\Mi carpeta\\a.txt").unwrap(), b"uno\r\n");
        assert_eq!(o[4], [FsError::Exists.message()]);
        assert!(!p.files().exists("C:\\b.txt"));
        assert_eq!(p.files().read("C:\\Escritorio\\c.txt").unwrap(), b"uno\r\n");
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
            "borrar /TODO C:\\Documentos\\p",
            "carpeta vacia",
            "borrar vacia",
            "borrar",
            "borrar /todo",
            "borrar nada",
            "borrar C:\\Sistema",
        ]);
        assert!(o[3].is_empty());
        assert_eq!(o[4].len(), 2);
        assert_eq!(o[4][0], FsError::NotEmpty.message());
        assert!(o[4][1].contains("borrar /todo"));
        assert!(o[6].is_empty());
        assert!(!p.files().exists("C:\\Documentos\\p"));
        assert_eq!(cwd, "C:\\Documentos");
        assert!(o[8].is_empty() && !p.files().exists("C:\\Documentos\\vacia"));
        assert_eq!(o[9], ["Uso: borrar [/todo] ruta"]);
        assert_eq!(o[10], ["Uso: borrar [/todo] ruta"]);
        assert_eq!(o[11], [FsError::NotFound.message()]);
        assert_eq!(o[12], [FsError::Invalid.message()]);
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
            assert_eq!(t.out[n], prompt("C:\\"));
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
}
