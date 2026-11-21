// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Cliente HTTP/1.1 (RFC 9110 y 9112) para `curl`, sobre `Net`. Sin bloquear: se crea con `start` y
//! se llama a `poll` en cada tick hasta que devuelve `Done` o `Error`.
//!
//! CONTRATO: las firmas públicas no cambian.
use super::{NetApi, NetError, QueryId, SockId, TcpState};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// URL `http://host[:puerto][/ruta][?consulta]`. El fragmento (`#…`) se descarta.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Url {
    pub host: String,
    pub port: u16,
    /// Empieza por `/` (vale `/` si la URL no trae ruta); incluye la consulta.
    pub path: String,
}

/// Sin esquema se supone `http://`. `https://` da `HttpsNotYet`; otro esquema, `BadUrl`.
// shortcut: sin usuario@, sin IPv6 entre corchetes ni IDN (el nombre va en ASCII); se añaden si hace falta.
pub fn parse_url(s: &str) -> Result<Url, NetError> {
    if s.bytes().any(|c| c <= b' ' || c == 0x7F) {
        return Err(NetError::BadUrl);
    }
    let mut rest = s;
    if let Some(i) = s.find("://") {
        let sch = &s[..i];
        if !sch.is_empty() && sch.bytes().all(|c| c.is_ascii_alphanumeric() || b"+-.".contains(&c)) {
            if sch.eq_ignore_ascii_case("https") {
                return Err(NetError::HttpsNotYet);
            }
            if !sch.eq_ignore_ascii_case("http") {
                return Err(NetError::BadUrl);
            }
            rest = &s[i + 3..];
        }
    }
    let rest = rest.split('#').next().unwrap_or("");
    let (auth, tail) = match rest.find(['/', '?']) {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let (host, port) = match auth.rfind(':') {
        Some(i) => {
            let p = &auth[i + 1..];
            if p.is_empty() || p.len() > 5 || !p.bytes().all(|c| c.is_ascii_digit()) {
                return Err(NetError::BadUrl);
            }
            (&auth[..i], p.parse::<u16>().ok().filter(|p| *p != 0).ok_or(NetError::BadUrl)?)
        }
        None => (auth, 80),
    };
    if host.is_empty() || host.len() > 253 || host.starts_with('.') || !host.bytes().all(|c| c.is_ascii_alphanumeric() || b"-._".contains(&c)) {
        return Err(NetError::BadUrl);
    }
    let mut path = String::new();
    if !tail.starts_with('/') {
        path.push('/');
    }
    for b in tail.bytes() {
        if b >= 0x80 {
            path.push_str(&format!("%{:02X}", b)); // UTF-8 de la ruta en porcentaje
        } else {
            path.push(b as char);
        }
    }
    Ok(Url { host: String::from(host), port, path })
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HttpEvent {
    /// Cabeceras de la respuesta final (tras seguir las redirecciones, si se pidió).
    Head { status: u16, reason: String, headers: Vec<(String, String)> },
    /// Un trozo del cuerpo, ya sin la codificación «chunked».
    Body(Vec<u8>),
    Done,
    Error(NetError),
}

/// Topes: cabeceras de 32 KiB como mucho y 10 redirecciones.
pub const MAX_HEAD: usize = 32 * 1024;
pub const MAX_REDIRECTS: u8 = 10;
/// Una línea de tamaño de bloque o de «trailer», y todos los «trailers» juntos.
const MAX_LINE: usize = 1024;
const MAX_TRAILERS: usize = 8 * 1024;
/// Sin recibir ni enviar nada durante este tiempo, se da por perdida.
const IDLE_MS: u64 = 30_000;

enum Stage {
    Resolving(QueryId),
    Connecting(SockId),
    Sending(SockId),
    Head(SockId),
    Body(SockId),
    Over,
}

#[derive(PartialEq, Eq)]
pub(super) enum ChunkState {
    Size,
    Data,
    DataEnd,
    Trailer,
    End,
}

/// Decodificador de `Transfer-Encoding: chunked` (RFC 9112, 7.1), alimentado a trozos.
pub(super) struct Chunked {
    st: ChunkState,
    line: Vec<u8>,
    left: u64,
    trailers: usize,
}

impl Chunked {
    pub(super) fn new() -> Chunked {
        Chunked { st: ChunkState::Size, line: Vec::new(), left: 0, trailers: 0 }
    }

    /// Añade a `out` el contenido útil; `Ok(true)` cuando termina (tras el último bloque y los trailers).
    pub(super) fn feed(&mut self, data: &[u8], out: &mut Vec<u8>) -> Result<bool, &'static str> {
        let mut i = 0;
        while i < data.len() && self.st != ChunkState::End {
            if self.st == ChunkState::Data {
                let n = (data.len() - i).min(self.left.min(usize::MAX as u64) as usize);
                out.extend_from_slice(&data[i..i + n]);
                self.left -= n as u64;
                i += n;
                if self.left == 0 {
                    self.st = ChunkState::DataEnd;
                }
                continue;
            }
            let b = data[i];
            i += 1;
            if self.st == ChunkState::Trailer {
                self.trailers += 1;
                if self.trailers > MAX_TRAILERS {
                    return Err("trailers demasiado grandes");
                }
            }
            if b != b'\n' {
                if self.st == ChunkState::DataEnd && b != b'\r' {
                    return Err("falta el fin de línea tras un bloque");
                }
                if self.line.len() >= MAX_LINE {
                    return Err("línea de bloque demasiado larga");
                }
                self.line.push(b);
                continue;
            }
            let line = core::mem::take(&mut self.line);
            let line = line.strip_suffix(b"\r").unwrap_or(&line);
            match self.st {
                ChunkState::DataEnd => {
                    if !line.is_empty() {
                        return Err("falta el fin de línea tras un bloque");
                    }
                    self.st = ChunkState::Size;
                }
                ChunkState::Size => {
                    // «tamaño[;extensiones]» en hexadecimal
                    let hex = line.split(|&c| c == b';').next().unwrap_or(&[]).trim_ascii();
                    if hex.is_empty() || hex.len() > 15 || !hex.iter().all(|c| c.is_ascii_hexdigit()) {
                        return Err("tamaño de bloque no válido");
                    }
                    self.left = hex.iter().fold(0u64, |a, &c| a * 16 + (c as char).to_digit(16).unwrap_or(0) as u64);
                    self.st = if self.left == 0 { ChunkState::Trailer } else { ChunkState::Data };
                }
                _ => {
                    if line.is_empty() {
                        self.st = ChunkState::End;
                    }
                }
            }
        }
        Ok(self.st == ChunkState::End)
    }
}

enum Mode {
    Length(u64),
    Chunked(Chunked),
    /// Hasta que el servidor cierre.
    Close,
}

/// Una petición `GET` (o `HEAD` con `head_only`). Con `follow`, sigue las redirecciones 301, 302,
/// 303, 307 y 308 (una a `https://` termina en `HttpsNotYet`). Envía `Host`, `User-Agent:
/// NightFury/<versión>`, `Accept: */*` y `Connection: close`.
pub struct HttpGet {
    head_only: bool,
    follow: bool,
    redirects: u8,
    url: Url,
    stage: Stage,
    req: Vec<u8>,
    sent: usize,
    buf: Vec<u8>,
    mode: Mode,
    last: Option<u64>,
}

fn find(h: &[u8], pat: &[u8]) -> Option<usize> {
    h.windows(pat.len()).position(|w| w == pat)
}

fn header<'a>(h: &'a [(String, String)], name: &str) -> impl Iterator<Item = &'a str> {
    let name = String::from(name);
    h.iter().filter(move |(k, _)| k.eq_ignore_ascii_case(&name)).map(|(_, v)| v.as_str())
}

/// «HTTP/1.x código motivo» y las cabeceras (sin la línea en blanco final).
pub(super) fn parse_head(h: &[u8]) -> Result<(u16, String, Vec<(String, String)>), NetError> {
    let bad = |m| NetError::Protocol(m);
    let mut lines = h.split(|&c| c == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l));
    let first = lines.next().unwrap_or(&[]);
    if !first.starts_with(b"HTTP/1.") || first.len() < 12 || first[8] != b' ' || !first[9..12].iter().all(|c| c.is_ascii_digit()) {
        return Err(bad("línea de estado no válida"));
    }
    let status = first[9..12].iter().fold(0u16, |a, &c| a * 10 + (c - b'0') as u16);
    if !(100..=599).contains(&status) {
        return Err(bad("código de estado no válido"));
    }
    let reason = String::from_utf8_lossy(first.get(13..).unwrap_or(&[])).into_owned();
    let mut headers = Vec::new();
    for l in lines {
        let i = l.iter().position(|&c| c == b':').ok_or(bad("cabecera sin «:»"))?;
        let name = &l[..i];
        if name.is_empty() || name.iter().any(|&c| c <= b' ') {
            return Err(bad("nombre de cabecera no válido"));
        }
        headers.push((String::from_utf8_lossy(name).into_owned(), String::from_utf8_lossy(l[i + 1..].trim_ascii()).into_owned()));
    }
    Ok((status, reason, headers))
}

impl HttpGet {
    pub fn start(net: &mut dyn NetApi, url: &str, head_only: bool, follow: bool) -> Result<HttpGet, NetError> {
        let url = parse_url(url)?;
        let q = net.resolve(&url.host)?;
        let mut g = HttpGet { head_only, follow, redirects: 0, url, stage: Stage::Resolving(q), req: Vec::new(), sent: 0, buf: Vec::new(), mode: Mode::Close, last: None };
        g.build_request();
        Ok(g)
    }

    fn build_request(&mut self) {
        let host = if self.url.port == 80 { self.url.host.clone() } else { format!("{}:{}", self.url.host, self.url.port) };
        let m = if self.head_only { "HEAD" } else { "GET" };
        self.req = format!("{m} {} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: NightFury/{}\r\nAccept: */*\r\nConnection: close\r\n\r\n", self.url.path, crate::VERSION).into_bytes();
        self.sent = 0;
    }

    fn sock(&self) -> Option<SockId> {
        match self.stage {
            Stage::Connecting(s) | Stage::Sending(s) | Stage::Head(s) | Stage::Body(s) => Some(s),
            _ => None,
        }
    }

    /// Cancela (Ctrl+C en la Terminal) y libera la conexión.
    pub fn cancel(&mut self, net: &mut dyn NetApi) {
        if let Some(s) = self.sock() {
            net.tcp_abort(s);
        }
        self.stage = Stage::Over;
    }

    fn fail(&mut self, net: &mut dyn NetApi, e: NetError, ev: &mut Vec<HttpEvent>) -> bool {
        self.cancel(net);
        ev.push(HttpEvent::Error(e));
        false
    }

    fn finish(&mut self, net: &mut dyn NetApi, ev: &mut Vec<HttpEvent>) -> bool {
        if let Some(s) = self.sock() {
            net.tcp_close(s);
        }
        self.stage = Stage::Over;
        ev.push(HttpEvent::Done);
        false
    }

    /// Avanza y devuelve lo que haya pasado desde la última llamada (vacío si nada). Tras `Done` o
    /// `Error`, la conexión ya está cerrada y no devuelve nada más.
    pub fn poll(&mut self, net: &mut dyn NetApi, now_ms: u64) -> Vec<HttpEvent> {
        let mut ev = Vec::new();
        let last = *self.last.get_or_insert(now_ms);
        // tope de pasos por llamada: nunca se queda dando vueltas
        for _ in 0..64 {
            if !self.step(net, &mut ev) {
                break;
            }
            self.last = Some(now_ms);
        }
        if !matches!(self.stage, Stage::Over) && self.last == Some(last) && now_ms.saturating_sub(last) >= IDLE_MS {
            self.fail(net, NetError::Timeout, &mut ev);
        }
        ev
    }

    /// Un paso; `true` si hubo avance y conviene otro.
    fn step(&mut self, net: &mut dyn NetApi, ev: &mut Vec<HttpEvent>) -> bool {
        match self.stage {
            Stage::Over => false,
            Stage::Resolving(q) => match net.resolve_poll(q) {
                None => false,
                Some(Err(e)) => self.fail(net, e, ev),
                // shortcut: solo se prueba la primera dirección A; si no responde, falla; probar las demás si hace falta.
                Some(Ok(ips)) => match ips.first().map(|ip| net.tcp_connect(*ip, self.url.port)) {
                    Some(Ok(s)) => {
                        self.stage = Stage::Connecting(s);
                        true
                    }
                    Some(Err(e)) => self.fail(net, e, ev),
                    None => self.fail(net, NetError::NotFound, ev),
                },
            },
            Stage::Connecting(s) => match net.tcp_state(s) {
                TcpState::Connecting => false,
                TcpState::Open | TcpState::PeerClosed => {
                    self.stage = Stage::Sending(s);
                    true
                }
                TcpState::Failed(e) => self.fail(net, e, ev),
                TcpState::Closed => self.fail(net, NetError::Protocol("conexión cerrada"), ev),
            },
            Stage::Sending(s) => match net.tcp_send(s, &self.req[self.sent..]) {
                Err(e) => self.fail(net, e, ev),
                Ok(n) => {
                    self.sent += n;
                    if self.sent >= self.req.len() {
                        self.stage = Stage::Head(s);
                        return true;
                    }
                    n > 0
                }
            },
            Stage::Head(s) | Stage::Body(s) => {
                let mut tmp = [0u8; 2048];
                match net.tcp_recv(s, &mut tmp) {
                    Err(e) => self.fail(net, e, ev),
                    Ok(0) => match net.tcp_state(s) {
                        TcpState::PeerClosed | TcpState::Closed => self.eof(net, ev),
                        _ => false,
                    },
                    Ok(n) => {
                        if matches!(self.stage, Stage::Head(_)) {
                            self.buf.extend_from_slice(&tmp[..n]);
                            self.on_head_bytes(net, s, ev)
                        } else {
                            self.body(net, &tmp[..n], ev)
                        }
                    }
                }
            }
        }
    }

    /// El servidor cerró y ya no queda nada por leer.
    fn eof(&mut self, net: &mut dyn NetApi, ev: &mut Vec<HttpEvent>) -> bool {
        match (&self.stage, &self.mode) {
            (Stage::Body(_), Mode::Close) => self.finish(net, ev),
            (Stage::Body(_), _) => self.fail(net, NetError::Protocol("respuesta incompleta"), ev),
            _ => self.fail(net, NetError::Protocol("conexión cerrada antes de las cabeceras"), ev),
        }
    }

    /// Mira si el búfer ya trae las cabeceras completas y, si es así, las procesa.
    fn on_head_bytes(&mut self, net: &mut dyn NetApi, s: SockId, ev: &mut Vec<HttpEvent>) -> bool {
        loop {
            let Some(end) = find(&self.buf, b"\r\n\r\n") else {
                if self.buf.len() > MAX_HEAD {
                    return self.fail(net, NetError::Protocol("cabeceras demasiado grandes"), ev);
                }
                return true;
            };
            if end > MAX_HEAD {
                return self.fail(net, NetError::Protocol("cabeceras demasiado grandes"), ev);
            }
            let (status, reason, headers) = match parse_head(&self.buf[..end]) {
                Ok(h) => h,
                Err(e) => return self.fail(net, e, ev),
            };
            let rest = self.buf.split_off(end + 4);
            self.buf.clear();
            if (100..200).contains(&status) && status != 101 {
                self.buf = rest; // 100 Continue y similares: la respuesta real sigue
                continue;
            }
            if self.follow && matches!(status, 301 | 302 | 303 | 307 | 308) {
                if let Some(loc) = header(&headers, "location").next() {
                    return self.redirect(net, s, loc, ev);
                }
            }
            return self.start_body(net, s, status, reason, headers, rest, ev);
        }
    }

    fn redirect(&mut self, net: &mut dyn NetApi, s: SockId, loc: &str, ev: &mut Vec<HttpEvent>) -> bool {
        self.redirects += 1;
        if self.redirects > MAX_REDIRECTS {
            return self.fail(net, NetError::Protocol("demasiadas redirecciones"), ev);
        }
        let auth = if self.url.port == 80 { self.url.host.clone() } else { format!("{}:{}", self.url.host, self.url.port) };
        let full = if loc.contains("://") {
            String::from(loc)
        } else if let Some(r) = loc.strip_prefix("//") {
            format!("http://{r}")
        } else if loc.starts_with('/') {
            format!("http://{auth}{loc}")
        } else {
            // relativa: junto a la ruta actual (sin consulta)
            let p = self.url.path.split('?').next().unwrap_or("/");
            let dir = &p[..p.rfind('/').map_or(0, |i| i + 1)];
            // shortcut: no se normalizan «./» ni «../» de la ruta; el servidor suele hacerlo.
            format!("http://{auth}{dir}{loc}")
        };
        let url = match parse_url(&full) {
            Ok(u) => u,
            Err(e) => return self.fail(net, e, ev),
        };
        let q = match net.resolve(&url.host) {
            Ok(q) => q,
            Err(e) => return self.fail(net, e, ev),
        };
        net.tcp_abort(s); // se descarta el cuerpo de la redirección
        self.url = url;
        self.stage = Stage::Resolving(q);
        self.buf.clear();
        self.build_request();
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn start_body(&mut self, net: &mut dyn NetApi, s: SockId, status: u16, reason: String, headers: Vec<(String, String)>, rest: Vec<u8>, ev: &mut Vec<HttpEvent>) -> bool {
        let chunked = header(&headers, "transfer-encoding").last().is_some_and(|v| v.rsplit(',').next().is_some_and(|t| t.trim().eq_ignore_ascii_case("chunked")));
        let mut len = None;
        if !chunked {
            for v in header(&headers, "content-length").flat_map(|v| v.split(',')) {
                match (v.trim().parse::<u64>(), len) {
                    (Ok(n), None) => len = Some(n),
                    (Ok(n), Some(m)) if n == m => {}
                    _ => return self.fail(net, NetError::Protocol("Content-Length no válido"), ev),
                }
            }
        }
        let no_body = self.head_only || status == 204 || status == 304;
        self.mode = match (chunked, len) {
            (true, _) => Mode::Chunked(Chunked::new()),
            (false, Some(n)) => Mode::Length(n),
            _ => Mode::Close,
        };
        ev.push(HttpEvent::Head { status, reason, headers });
        self.stage = Stage::Body(s);
        if no_body || matches!(self.mode, Mode::Length(0)) {
            return self.finish(net, ev);
        }
        self.body(net, &rest, ev)
    }

    /// Procesa bytes del cuerpo según la codificación.
    fn body(&mut self, net: &mut dyn NetApi, data: &[u8], ev: &mut Vec<HttpEvent>) -> bool {
        let mut out = Vec::new();
        let done = match &mut self.mode {
            Mode::Close => {
                out.extend_from_slice(data);
                false
            }
            Mode::Length(left) => {
                let n = data.len().min((*left).min(usize::MAX as u64) as usize);
                out.extend_from_slice(&data[..n]);
                *left -= n as u64;
                *left == 0
            }
            Mode::Chunked(c) => match c.feed(data, &mut out) {
                Ok(d) => d,
                Err(m) => return self.fail(net, NetError::Protocol(m), ev),
            },
        };
        if !out.is_empty() {
            ev.push(HttpEvent::Body(out));
        }
        if done {
            return self.finish(net, ev);
        }
        true
    }
}
