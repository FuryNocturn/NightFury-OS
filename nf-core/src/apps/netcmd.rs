// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Órdenes de red de la Terminal (`ping`, `ipconfig`, `nslookup`, `curl`). `parse` las analiza (sin
//! tocar la red); las que tardan quedan como un `Job` que `Terminal::tick` avanza con la pila.
use super::terminal::tokens;
use crate::files::resolve;
use crate::fs::FsError;
use crate::net::http::{parse_url, HttpEvent, HttpGet};
use crate::net::{ip_text, mac_text, parse_ip, Ipv4, NetApi, NetError, NetStatus, PingId, QueryId, State, DHCP_TIMEOUT_MS};
use crate::platform::Platform;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

const NO_CARD: &str = "No hay ninguna tarjeta de red.";
const NO_CABLE: &str = "El cable de red está desconectado.";
const NO_IP: &str = "Todavía no hay dirección IP.";
const PING_USO: &str = "Uso: ping [-n N] [-t] destino";
const CURL_USO: &str = "Uso: curl [-o nombre] [-I] [-L] [-k] [-v] URL";
const PING_BYTES: u16 = 32;
const PING_TTL: u8 = 128;
const PING_EVERY_MS: u64 = 1000;
/// `ipconfig /renew`: lo que tarda DHCP en rendirse, más un margen.
const RENEW_MS: u64 = DHCP_TIMEOUT_MS + 2_000;
/// Lo que `curl -o` acumula en memoria como mucho.
const MAX_SAVE: usize = crate::abi::SAVE_MAX as usize;
/// Una línea del cuerpo que no acaba nunca se parte a estos bytes.
const MAX_LINE: usize = 4096;

#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum NetCmd {
    /// `count` = `None` con `-t`.
    Ping { host: String, count: Option<u32> },
    Ipconfig { all: bool },
    Renew,
    Nslookup(String),
    /// `out` = (ruta completa, nombre tal como lo escribió el usuario).
    /// `insecure` = `-k` (seguir con un certificado malo, avisando); `verbose` = `-v`.
    Curl { url: String, head: bool, follow: bool, insecure: bool, verbose: bool, out: Option<(String, String)> },
}

fn uso(s: &str) -> Result<NetCmd, Vec<String>> {
    Err(vec![String::from(s)])
}

/// Analiza la orden de red `cmd` (en minúsculas). `Err` = líneas a escribir sin hacer nada más.
pub(super) fn parse(cmd: &str, args: &str, cwd: &str, sys: &mut dyn Platform) -> Result<NetCmd, Vec<String>> {
    let a = tokens(args);
    match cmd {
        "ping" => {
            let (mut host, mut count, mut forever, mut i) = (None, 4u32, false, 0);
            while i < a.len() {
                match a[i].to_lowercase().as_str() {
                    "-t" => forever = true,
                    "-n" => {
                        i += 1;
                        match a.get(i).and_then(|n| n.parse::<u32>().ok()).filter(|&n| n > 0) {
                            Some(n) => count = n,
                            None => return uso(PING_USO),
                        }
                    }
                    t if t.starts_with('-') || host.is_some() => return uso(PING_USO),
                    _ => host = Some(a[i].clone()),
                }
                i += 1;
            }
            match host {
                Some(host) => Ok(NetCmd::Ping { host, count: (!forever).then_some(count) }),
                None => uso(PING_USO),
            }
        }
        "ipconfig" => match a.iter().map(|t| t.to_lowercase()).collect::<Vec<_>>().as_slice() {
            [] => Ok(NetCmd::Ipconfig { all: false }),
            [t] if t == "/all" => Ok(NetCmd::Ipconfig { all: true }),
            [t] if t == "/renew" => Ok(NetCmd::Renew),
            _ => uso("Uso: ipconfig [/all | /renew]"),
        },
        "nslookup" => match a.as_slice() {
            [h] => Ok(NetCmd::Nslookup(h.clone())),
            _ => uso("Uso: nslookup host"),
        },
        _ => {
            let (mut url, mut head, mut follow, mut insecure, mut verbose, mut out, mut i) = (None, false, false, false, false, None, 0);
            while i < a.len() {
                match a[i].as_str() {
                    "-I" => head = true,
                    "-L" => follow = true,
                    "-k" => insecure = true,
                    "-v" => verbose = true,
                    "-o" => {
                        i += 1;
                        match a.get(i) {
                            Some(n) if out.is_none() => out = Some(n.clone()),
                            _ => return uso(CURL_USO),
                        }
                    }
                    t if t.starts_with('-') || url.is_some() => return uso(CURL_USO),
                    t => url = Some(String::from(t)),
                }
                i += 1;
            }
            let Some(url) = url else { return uso(CURL_USO) };
            if head && out.is_some() {
                return uso("curl: -I y -o no se pueden usar juntos.");
            }
            if parse_url(&url).is_err() {
                return uso("curl: (3) URL no válida");
            }
            let out = match out {
                None => None,
                Some(name) => {
                    let p = resolve(cwd, &name).map_err(|e| vec![String::from(e.message())])?;
                    if sys.fs().stat(&p).is_ok_and(|e| e.dir) {
                        return uso(FsError::IsDir.message());
                    }
                    Some((p, name))
                }
            };
            Ok(NetCmd::Curl { url, head, follow, insecure, verbose, out })
        }
    }
}

/// Texto de un error de red cualquiera.
fn net_err(e: &NetError) -> String {
    String::from(match e {
        NetError::NoAdapter => NO_CARD,
        NetError::NoCable => NO_CABLE,
        NetError::NoAddress => NO_IP,
        NetError::Busy => "Hay demasiadas operaciones de red a la vez.",
        NetError::NotFound => "No se encontró el equipo.",
        NetError::Timeout => "Tiempo de espera agotado.",
        _ => "Error de red.",
    })
}

/// «   Etiqueta. . . . . : valor», con los dos puntos siempre en la misma columna.
fn kv(label: &str, val: &str) -> String {
    let mut s = format!("   {label}");
    for i in s.chars().count()..44 {
        s.push(if i % 2 == 1 { '.' } else { ' ' });
    }
    format!("{s} : {val}")
}

/// «1 Gb/s, dúplex completo» (también lo usa Configuración).
pub(super) fn speed_text(mbps: u32, full_duplex: bool) -> String {
    let s = if mbps >= 1000 {
        let (g, d) = (mbps / 1000, mbps % 1000 / 100);
        if d == 0 { format!("{g} Gb/s") } else { format!("{g},{d} Gb/s") }
    } else {
        format!("{mbps} Mb/s")
    };
    format!("{s}, {}", if full_duplex { "dúplex completo" } else { "semidúplex" })
}

/// Salida de `ipconfig` (y de `/renew` al acabar). `now_ms` para lo que queda de la concesión.
pub(super) fn ipconfig_lines(st: &NetStatus, all: bool, now_ms: u64) -> Vec<String> {
    let mut v = vec![String::from("Configuración IP de NightFury"), String::new(), String::from("Adaptador de Ethernet Ethernet:"), String::new()];
    if st.state == State::NoCable {
        v.push(kv("Estado de los medios", "medios desconectados"));
    }
    if all {
        v.push(kv("Descripción", &st.adapter));
        v.push(kv("Dirección física", &mac_text(st.mac)));
        v.push(kv("DHCP habilitado", "sí"));
        if st.speed_mbps > 0 {
            v.push(kv("Velocidad", &speed_text(st.speed_mbps, st.full_duplex)));
        }
    }
    match st.ip {
        Some(ip) => {
            v.push(kv("Dirección IPv4", &ip_text(ip)));
            if let Some(m) = st.mask {
                v.push(kv("Máscara de subred", &ip_text(m)));
            }
            if let Some(g) = st.gateway {
                v.push(kv("Puerta de enlace predeterminada", &ip_text(g)));
            }
        }
        None if st.state == State::NoCable => {}
        None if st.state == State::Configuring => v.push(String::from("   Pidiendo dirección…")),
        None => v.push(format!("   {NO_IP}")),
    }
    if all {
        if let Some(s) = st.dhcp_server {
            v.push(kv("Servidor DHCP", &ip_text(s)));
        }
        if st.lease_s > 0 {
            let left = (st.lease_s as u64).saturating_sub(now_ms.saturating_sub(st.lease_since_ms) / 1000);
            v.push(kv("Concesión", &format!("quedan {} h {} min", left / 3600, left / 60 % 60)));
        }
        for (i, d) in st.dns.iter().enumerate() {
            v.push(if i == 0 { kv("Servidores DNS", &ip_text(*d)) } else { format!("{:47}{}", "", ip_text(*d)) });
        }
    }
    v
}

pub(super) struct Ping {
    host: String,
    ip: Option<Ipv4>,
    /// Resolviendo el nombre.
    q: Option<QueryId>,
    count: Option<u32>,
    sent: u32,
    recv: u32,
    min: u64,
    max: u64,
    sum: u64,
    /// Eco en vuelo y cuándo salió.
    pending: Option<(PingId, u64)>,
    next_at: u64,
}

pub(super) struct Curl {
    get: HttpGet,
    head: bool,
    verbose: bool,
    /// (ruta, nombre escrito, cuerpo acumulado).
    save: Option<(String, String, Vec<u8>)>,
    host: String,
    port: u16,
    /// Resto de línea del cuerpo todavía sin terminar.
    line: Vec<u8>,
}

/// Una orden de red en marcha.
pub(super) enum Job {
    Ping(Ping),
    Renew { since: u64 },
    Nslookup { q: QueryId, host: String, server: String },
    Curl(Curl),
}

impl Job {
    /// Empieza `cmd`. `None` si ya está hecha (sale por `out`) o no se pudo empezar.
    pub(super) fn start(cmd: NetCmd, sys: &mut dyn Platform, now: u64, out: &mut Vec<String>) -> Option<Job> {
        // Raíces y hora para comprobar certificados (una redirección puede llevar a https aunque la URL no lo sea).
        let tls = matches!(cmd, NetCmd::Curl { .. }).then(|| (sys.files().root_store(), crate::service::hora_x509(sys.now())));
        let Some(net) = sys.net() else {
            out.push(String::from(NO_CARD));
            return None;
        };
        if let Some((roots, t)) = tls {
            net.set_tls_env(roots, t);
        }
        let st = net.status();
        let plain = matches!(cmd, NetCmd::Ipconfig { .. });
        if st.state == State::NoAdapter || (st.state == State::NoCable && !plain) {
            out.push(String::from(if st.state == State::NoAdapter { NO_CARD } else { NO_CABLE }));
            return None;
        }
        if !plain && cmd != NetCmd::Renew && st.ip.is_none() {
            out.push(String::from(NO_IP));
            return None;
        }
        match cmd {
            NetCmd::Ipconfig { all } => {
                out.extend(ipconfig_lines(&st, all, now));
                None
            }
            NetCmd::Renew => {
                net.renew();
                Some(Job::Renew { since: now })
            }
            NetCmd::Nslookup(host) => match net.resolve(&host) {
                Ok(q) => Some(Job::Nslookup { q, host, server: st.dns.first().map_or(String::from("desconocido"), |d| ip_text(*d)) }),
                Err(e) => {
                    out.push(net_err(&e));
                    None
                }
            },
            NetCmd::Ping { host, count } => {
                let mut p = Ping { host, ip: None, q: None, count, sent: 0, recv: 0, min: u64::MAX, max: 0, sum: 0, pending: None, next_at: now };
                match parse_ip(&p.host) {
                    Some(ip) => {
                        p.ip = Some(ip);
                        out.push(format!("Haciendo ping a {} con {PING_BYTES} bytes de datos:", p.host));
                    }
                    None => match net.resolve(&p.host) {
                        Ok(q) => p.q = Some(q),
                        Err(e) => {
                            out.push(ping_resolve_err(&p.host, &e));
                            return None;
                        }
                    },
                }
                Some(Job::Ping(p))
            }
            NetCmd::Curl { url, head, follow, insecure, verbose, out: save } => {
                // parse ya validó la URL; el host y el puerto son los de la URL original (sirven para los errores).
                let u = parse_url(&url).ok()?;
                match HttpGet::start_tls(net, &url, head, follow, insecure) {
                    Ok(get) => Some(Job::Curl(Curl { get, head, verbose, save: save.map(|(p, n)| (p, n, Vec::new())), host: u.host, port: u.port, line: Vec::new() })),
                    Err(e) => {
                        out.push(curl_err(&e, &u.host, u.port));
                        None
                    }
                }
            }
        }
    }

    /// Un paso; `true` cuando ha terminado.
    pub(super) fn step(&mut self, sys: &mut dyn Platform, now: u64, out: &mut Vec<String>) -> bool {
        if let Job::Curl(c) = self {
            return step_curl(c, sys, now, out);
        }
        let Some(net) = sys.net() else {
            out.push(String::from(NO_CARD));
            return true;
        };
        match self {
            Job::Ping(p) => step_ping(p, net, now, out),
            Job::Renew { since } => {
                let st = net.status();
                if st.state == State::Configuring && now.saturating_sub(*since) < RENEW_MS {
                    return false;
                }
                if st.ip.is_some() {
                    out.extend(ipconfig_lines(&st, false, now));
                } else {
                    out.push(String::from("No se pudo renovar la dirección."));
                }
                true
            }
            Job::Nslookup { q, host, server } => match net.resolve_poll(*q) {
                None => false,
                Some(Ok(ips)) if !ips.is_empty() => {
                    out.push(format!("Servidor:  {server}"));
                    out.push(String::new());
                    out.push(format!("Nombre:    {host}"));
                    for (i, ip) in ips.iter().enumerate() {
                        let lead = match (i, ips.len()) {
                            (0, 1) => "Dirección: ",
                            (0, _) => "Direcciones: ",
                            _ => "             ",
                        };
                        out.push(format!("{lead}{}", ip_text(*ip)));
                    }
                    true
                }
                Some(Ok(_)) | Some(Err(NetError::NotFound)) => {
                    out.push(format!("*** {server} no encuentra {host}: el dominio no existe"));
                    true
                }
                Some(Err(NetError::Timeout)) => {
                    out.push(format!("*** Se agotó el tiempo de espera de la solicitud a {server}"));
                    true
                }
                Some(Err(e)) => {
                    out.push(net_err(&e));
                    true
                }
            },
            Job::Curl(_) => true,
        }
    }

    /// Ctrl+C: corta lo que haya en marcha y escribe lo que corresponde.
    pub(super) fn cancel(&mut self, sys: &mut dyn Platform, out: &mut Vec<String>) {
        match self {
            Job::Ping(p) if p.ip.is_some() => {
                // el eco en vuelo no se cuenta como perdido
                if p.pending.take().is_some() {
                    p.sent -= 1;
                }
                stats(p, out);
                out.push(String::from("Control-C"));
            }
            Job::Curl(c) => {
                if let Some(net) = sys.net() {
                    c.get.cancel(net);
                }
            }
            _ => {}
        }
        out.push(String::from("^C"));
    }
}

fn ping_resolve_err(host: &str, e: &NetError) -> String {
    match e {
        NetError::NotFound | NetError::Timeout | NetError::Protocol(_) => format!("La solicitud de ping no pudo encontrar el host {host}. Compruebe el nombre y vuelva a intentarlo."),
        e => net_err(e),
    }
}

fn stats(p: &Ping, out: &mut Vec<String>) {
    let lost = p.sent - p.recv;
    out.push(String::new());
    out.push(format!("Estadísticas de ping para {}:", p.ip.map_or(String::new(), ip_text)));
    out.push(format!("    Paquetes: enviados = {}, recibidos = {}, perdidos = {lost}", p.sent, p.recv));
    out.push(format!("    ({}% perdidos),", if p.sent == 0 { 0 } else { lost * 100 / p.sent }));
    if p.recv > 0 {
        out.push(String::from("Tiempos aproximados de ida y vuelta en milisegundos:"));
        out.push(format!("    Mínimo = {}ms, Máximo = {}ms, Media = {}ms", p.min, p.max, p.sum / p.recv as u64));
    }
}

fn step_ping(p: &mut Ping, net: &mut dyn NetApi, now: u64, out: &mut Vec<String>) -> bool {
    if let Some(q) = p.q {
        match net.resolve_poll(q) {
            None => return false,
            Some(Ok(ips)) if !ips.is_empty() => {
                p.q = None;
                p.ip = Some(ips[0]);
                p.next_at = now;
                out.push(format!("Haciendo ping a {} [{}] con {PING_BYTES} bytes de datos:", p.host, ip_text(ips[0])));
            }
            Some(Err(e)) => {
                out.push(ping_resolve_err(&p.host, &e));
                return true;
            }
            Some(Ok(_)) => {
                out.push(ping_resolve_err(&p.host, &NetError::NotFound));
                return true;
            }
        }
    }
    let Some(ip) = p.ip else { return true };
    if let Some((id, t0)) = p.pending {
        let Some(r) = net.ping_poll(id) else { return false };
        p.pending = None;
        p.next_at = t0 + PING_EVERY_MS;
        out.push(match r {
            Ok(rep) => {
                p.recv += 1;
                (p.min, p.max, p.sum) = (p.min.min(rep.rtt_ms), p.max.max(rep.rtt_ms), p.sum + rep.rtt_ms);
                let t = if rep.rtt_ms == 0 { String::from("<1ms") } else { format!("={}ms", rep.rtt_ms) };
                format!("Respuesta desde {}: bytes={} tiempo{t} TTL={}", ip_text(rep.from), rep.bytes, rep.ttl)
            }
            Err(NetError::Timeout) => String::from("Tiempo de espera agotado para esta solicitud."),
            Err(NetError::Unreachable(f)) => format!("Respuesta desde {}: Host de destino inaccesible.", ip_text(f)),
            Err(NetError::TtlExpired(f)) => format!("Respuesta desde {}: TTL caducado en tránsito.", ip_text(f)),
            Err(e) => net_err(&e),
        });
    }
    if p.pending.is_none() && p.count.is_some_and(|c| p.sent >= c) {
        stats(p, out);
        return true;
    }
    if p.pending.is_none() && now >= p.next_at {
        match net.ping(ip, PING_BYTES, PING_TTL) {
            Ok(id) => {
                p.sent += 1;
                p.pending = Some((id, now));
            }
            Err(e) => {
                out.push(net_err(&e));
                stats(p, out);
                return true;
            }
        }
    }
    false
}

fn curl_err(e: &NetError, host: &str, port: u16) -> String {
    match e {
        NetError::NotFound => format!("curl: (6) No se pudo resolver el host: {host}"),
        NetError::Refused | NetError::Unreachable(_) | NetError::TtlExpired(_) => format!("curl: (7) No se pudo conectar con {host} puerto {port}"),
        NetError::Timeout => String::from("curl: (28) Tiempo de espera agotado"),
        NetError::Reset => String::from("curl: (56) Conexión cortada"),
        NetError::Protocol(m) => format!("curl: (8) Respuesta no válida: {m}"),
        NetError::BadUrl => String::from("curl: (3) URL no válida"),
        NetError::Tls(m) => format!("curl: (35) {m}"),
        e => format!("curl: {}", net_err(e)),
    }
}

/// Una línea del cuerpo: UTF-8 con pérdidas, tabuladores a 4 espacios y sin caracteres de control.
fn clean(b: &[u8]) -> String {
    let s = String::from_utf8_lossy(b);
    s.trim_end_matches(['\r', '\n']).replace('\t', "    ").chars().map(|c| if c.is_control() { '.' } else { c }).collect()
}

fn feed(line: &mut Vec<u8>, data: &[u8], out: &mut Vec<String>) {
    line.extend_from_slice(data);
    // un cursor y un solo `drain` al final: con muchas líneas cortas no se mueve el búfer en cada una
    let mut start = 0;
    loop {
        let rest = &line[start..];
        let end = match rest.iter().take(MAX_LINE).position(|&b| b == b'\n') {
            Some(i) => i + 1,
            None if rest.len() >= MAX_LINE => MAX_LINE,
            None => break,
        };
        out.push(clean(&rest[..end]));
        start += end;
    }
    line.drain(..start);
}

fn step_curl(c: &mut Curl, sys: &mut dyn Platform, now: u64, out: &mut Vec<String>) -> bool {
    let Some(net) = sys.net() else {
        out.push(String::from(NO_CARD));
        return true;
    };
    for ev in c.get.poll(net, now) {
        match ev {
            HttpEvent::Head { status, reason, headers } if c.head || c.verbose => {
                let p = if c.verbose { "< " } else { "" };
                out.push(format!("{p}HTTP/1.1 {status} {reason}").trim_end().into());
                out.extend(headers.iter().map(|(k, v)| format!("{p}{k}: {v}")));
            }
            HttpEvent::Head { .. } => {}
            HttpEvent::Secure(i) => {
                // `-k` con un certificado que no vale: sigue, pero se dice claro
                if let Some(e) = &i.cert_error {
                    out.push(format!("Aviso: la conexión no es segura (-k): {e}"));
                }
                if c.verbose {
                    out.push(format!("* Conexión segura: {}, {}, grupo {}", i.version, i.suite, i.group));
                    if i.subject.is_empty() {
                        out.push(String::from("* Certificado: sin comprobar"));
                    } else {
                        let (y, m, d) = i.expires;
                        out.push(format!("* Certificado de: {}", i.subject));
                        out.push(format!("* Emitido por: {}", i.issuer));
                        out.push(format!("* Caduca el: {d:02}/{m:02}/{y:04}"));
                    }
                }
            }
            HttpEvent::Body(b) => match &mut c.save {
                Some((_, _, data)) if data.len() + b.len() > MAX_SAVE => {
                    c.get.cancel(net);
                    out.push(String::from("curl: el archivo es demasiado grande (más de 32 MB)"));
                    return true;
                }
                Some((_, _, data)) => data.extend_from_slice(&b),
                None if !c.head => feed(&mut c.line, &b, out),
                None => {}
            },
            HttpEvent::Done => {
                if !c.line.is_empty() {
                    out.push(clean(&c.line));
                }
                if let Some((path, shown, data)) = c.save.take() {
                    let d = sys.now();
                    out.push(match sys.fs().write(&path, &data, d) {
                        Ok(()) => format!("Guardado {shown} ({} bytes)", data.len()),
                        Err(e) => String::from(e.message()),
                    });
                }
                return true;
            }
            HttpEvent::Error(e) => {
                if !c.line.is_empty() {
                    out.push(clean(&c.line));
                }
                out.push(curl_err(&e, &c.host, c.port));
                return true;
            }
        }
    }
    false
}
