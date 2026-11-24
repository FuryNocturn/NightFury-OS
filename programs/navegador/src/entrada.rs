// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Lo que se escribe en la barra de direcciones (URL, ruta local o búsqueda), rutas locales como URL,
//! tipo de contenido y la regla que impide a una página pública tocar la red local.
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use nf_core::img;
use nf_core::net::http::parse_url;

/// Buscador de las búsquedas escritas en la barra.
const BUSQUEDA: &str = "https://html.duckduckgo.com/html/?q=";

/// Servidor ficticio con el que se maqueta una página del disco: sus recursos relativos salen como
/// `http://archivo.invalid/C:/carpeta/x.png` y se leen de disco (nunca de la red).
pub const HOST_LOCAL: &str = "archivo.invalid";

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Entrada {
    /// URL http o https completa.
    Web(String),
    /// Ruta absoluta de disco, con `\`.
    Local(String),
}

/// `X:\…` o `X:/…`.
pub fn es_ruta_local(t: &str) -> bool {
    let b = t.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// Codifica una consulta para `?q=`: letras, cifras y `-_.~` tal cual, el espacio como `+` y el resto en %XX.
pub fn codificar_consulta(q: &str) -> String {
    let mut r = String::new();
    for b in q.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => r.push(b as char),
            b' ' => r.push('+'),
            _ => r.push_str(&format!("%{b:02X}")),
        }
    }
    r
}

/// Host (sin usuario ni puerto) de lo escrito, en minúsculas.
fn host_escrito(t: &str) -> &str {
    let auth = t.split(['/', '?', '#']).next().unwrap_or("");
    let auth = auth.rsplit('@').next().unwrap_or("");
    match auth.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()) => h,
        _ => auth,
    }
}

/// `a.b.c.d` con cuatro números decimales.
fn ipv4(h: &str) -> Option<[u8; 4]> {
    let mut it = h.split('.');
    let mut ip = [0u8; 4];
    for o in ip.iter_mut() {
        let p = it.next()?;
        if p.is_empty() || p.len() > 3 || !p.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        *o = p.parse().ok()?;
    }
    it.next().is_none().then_some(ip)
}

/// Qué hacer con lo escrito: `None` si está vacío.
pub fn clasificar(texto: &str) -> Option<Entrada> {
    let t = texto.trim();
    if t.is_empty() {
        return None;
    }
    if es_ruta_local(t) {
        let mut r = t.replace('/', "\\");
        r[..1].make_ascii_uppercase();
        return Some(Entrada::Local(r));
    }
    let minus = t.to_ascii_lowercase();
    // Con esquema (también uno que no se admite: la página de error lo explicará).
    if minus.split_once("://").is_some_and(|(e, _)| !e.is_empty() && e.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.')) {
        return Some(Entrada::Web(t.replace(' ', "%20")));
    }
    if !t.contains(char::is_whitespace) {
        let host = host_escrito(&minus);
        let es_localhost = host == "localhost";
        let es_ip = ipv4(host).is_some();
        let parece_dominio = host.contains('.') && host.split('.').all(|l| !l.is_empty() && l.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
        if es_localhost || es_ip || parece_dominio {
            // Un servidor de la red local rara vez tiene certificado: ahí se prueba con http.
            return Some(Entrada::Web(format!("{}://{t}", if es_localhost || es_ip { "http" } else { "https" })));
        }
    }
    Some(Entrada::Web(format!("{BUSQUEDA}{}", codificar_consulta(t))))
}

/// Separa el `#fragmento` de una URL.
pub fn sin_fragmento(url: &str) -> (&str, Option<&str>) {
    match url.split_once('#') {
        Some((u, f)) => (u, Some(f)),
        None => (url, None),
    }
}

/// Decodifica %XX (UTF-8); `None` si no es UTF-8 válido.
fn decodificar(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut r: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            let v = u8::from_str_radix(s.get(i + 1..i + 3)?, 16).ok()?;
            r.push(v);
            i += 3;
        } else {
            r.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(r).ok()
}

/// La ruta de disco `C:\a b\x.html` como URL base para maquetar (ver `HOST_LOCAL`).
pub fn url_de_local(ruta: &str) -> String {
    let mut r = format!("http://{HOST_LOCAL}/");
    for b in ruta.bytes() {
        match b {
            b'\\' | b'/' => r.push('/'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' => r.push(b as char),
            _ => r.push_str(&format!("%{b:02X}")),
        }
    }
    r
}

/// Lo contrario de `url_de_local`: `None` si la URL no es de `HOST_LOCAL` o no empieza por una unidad.
pub fn local_de_url(url: &str) -> Option<String> {
    let resto = url.strip_prefix("http://")?.strip_prefix(HOST_LOCAL)?.strip_prefix('/')?;
    let resto = resto.split(['?', '#']).next().unwrap_or("");
    let ruta = decodificar(resto)?.replace('/', "\\");
    if !es_ruta_local(&format!("{ruta}\\")) {
        return None;
    }
    let mut ruta = ruta;
    ruta[..1].make_ascii_uppercase();
    Some(ruta)
}

/// Host de una URL ya normalizada (en minúsculas), si es válida.
fn host_de(url: &str) -> Option<String> {
    parse_url(url).ok().map(|u| u.host.to_ascii_lowercase())
}

/// Dirección de uso interno: localhost, nombres sin punto (intranet), sufijos locales y IP privadas,
/// de bucle, de enlace local o reservadas. Lo dudoso cuenta como privado.
pub fn es_privado(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    if !h.contains('.') || h.starts_with('[') {
        return true;
    }
    const SUFIJOS: [&str; 7] = [".localhost", ".local", ".localdomain", ".internal", ".lan", ".home.arpa", ".invalid"];
    if SUFIJOS.iter().any(|s| h.ends_with(s)) {
        return true;
    }
    // Ningún dominio acaba en un número: así que parece una IP, y si no es decimal normal, es sospechosa.
    let ultimo = h.rsplit('.').next().unwrap_or("");
    if ultimo.bytes().next().is_some_and(|c| c.is_ascii_digit()) {
        return match ipv4(&h) {
            Some([a, b, ..]) => a == 0 || a == 10 || a == 127 || a >= 224 || (a == 100 && (64..128).contains(&b)) || (a == 169 && b == 254) || (a == 172 && (16..32).contains(&b)) || (a == 192 && b == 168) || (a == 198 && (18..20).contains(&b)),
            None => true,
        };
    }
    false
}

/// Una página pública no puede cargar recursos de la red local o de la propia máquina (como el «acceso a
/// redes privadas» de los navegadores). Una página local o de la red local puede cargar lo que quiera.
// shortcut: solo se mira el nombre escrito; un nombre público que resuelva a una IP privada (o un cambio de
// DNS) no se detecta porque HttpGet resuelve por dentro. Hacer la consulta aquí y pasarle la IP cuando se pueda.
pub fn recurso_permitido(pagina: &str, recurso: &str) -> bool {
    let Some(r) = host_de(recurso) else { return false };
    !es_privado(&r) || host_de(pagina).is_none_or(|p| es_privado(&p))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TipoVista {
    Html,
    Texto,
    Imagen,
    Otro,
}

/// Qué se hace con una respuesta según su `Content-Type`; sin él se mira el contenido.
pub fn tipo_vista(ct: Option<&str>, cuerpo: &[u8]) -> TipoVista {
    let mime = ct.map(|c| c.split(';').next().unwrap_or("").trim().to_ascii_lowercase()).unwrap_or_default();
    match mime.as_str() {
        "text/html" | "application/xhtml+xml" => TipoVista::Html,
        "text/plain" => TipoVista::Texto,
        "image/png" | "image/jpeg" | "image/jpg" | "image/gif" => TipoVista::Imagen,
        "" if img::sniff(cuerpo).is_some() => TipoVista::Imagen,
        "" => TipoVista::Html,
        _ => TipoVista::Otro,
    }
}

/// `Content-Type` de un archivo de disco según su extensión.
pub fn tipo_de_extension(ruta: &str) -> Option<&'static str> {
    let ext = ruta.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "html" | "htm" | "xhtml" => "text/html",
        "txt" | "log" | "md" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        _ => return None,
    })
}
