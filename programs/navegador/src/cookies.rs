// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Tarro de cookies según RFC 6265 (y los prefijos `__Secure-`/`__Host-` de rfc6265bis), con SameSite,
//! bloqueo de cookies de terceros, límites y persistencia en texto. Sin JavaScript no hay `document.cookie`,
//! así que `HttpOnly` solo se guarda. El Navegador decide qué cookies persistentes se escriben (filtro).
// Se integra en el paso 3b.
#![allow(dead_code)]

use alloc::string::String;
use alloc::vec::Vec;

/// Bytes máximos de nombre + valor de una cookie.
pub const MAX_COOKIE: usize = 4096;
pub const MAX_DOMINIO: usize = 50;
pub const MAX_TOTAL: usize = 3000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

/// Desde dónde se hace la petición respecto a la página que se ve.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Contexto {
    /// Mismo sitio que la página (o petición de la propia barra de direcciones).
    PrimeraParte,
    /// Navegación de la pestaña a otro sitio (clic en un enlace): se envían Lax y None, no Strict.
    Navegacion,
    /// Imagen, hoja de estilo, etc. de un sitio distinto al de la página: terceros bloqueados.
    Subrecurso,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Cookie {
    pub nombre: String,
    pub valor: String,
    pub dominio: String,
    /// Sin atributo Domain: solo vale para ese host exacto.
    pub solo_host: bool,
    pub ruta: String,
    /// Segundo Unix de caducidad; `None` = de sesión.
    pub expira: Option<i64>,
    pub seguro: bool,
    pub solo_http: bool,
    pub same_site: SameSite,
    /// Orden de creación y de último uso (contadores del tarro, no relojes).
    pub creada: u64,
    pub usada: u64,
}

impl Cookie {
    fn caducada(&self, ahora: i64) -> bool {
        self.expira.is_some_and(|e| e <= ahora)
    }
}

struct Url {
    https: bool,
    host: String,
    ruta: String,
}

/// Esquema, host (minúsculas, sin puerto ni usuario) y ruta de una URL http(s).
fn partes(url: &str) -> Option<Url> {
    let (esq, resto) = url.split_once("://")?;
    let https = if esq.eq_ignore_ascii_case("https") {
        true
    } else if esq.eq_ignore_ascii_case("http") {
        false
    } else {
        return None;
    };
    let fin = resto.find(['/', '?', '#']).unwrap_or(resto.len());
    let (auth, tras) = resto.split_at(fin);
    let auth = auth.rsplit('@').next()?;
    let host = if auth.starts_with('[') {
        &auth[..=auth.find(']')?]
    } else {
        auth.split(':').next()?
    };
    if host.is_empty() {
        return None;
    }
    let ruta = tras.split(['?', '#']).next().unwrap_or("");
    let ruta = if ruta.starts_with('/') { ruta } else { "/" };
    Some(Url { https, host: host.to_ascii_lowercase(), ruta: String::from(ruta) })
}

fn es_ip(host: &str) -> bool {
    host.starts_with('[') || {
        let mut n = 0;
        host.split('.').all(|p| {
            n += 1;
            p.parse::<u8>().is_ok()
        }) && n == 4
    }
}

// shortcut: lista corta de sufijos públicos, completar si falla en sitios reales
const SUFIJOS: &[&str] = &[
    "com", "net", "org", "es", "eu", "uk", "co.uk", "org.uk", "ac.uk", "gov.uk", "de", "fr", "it", "io", "info",
    "gov", "edu", "mil", "int", "biz", "dev", "app", "me", "tv", "co", "us", "ca", "nl", "be", "ch", "at", "pt",
    "se", "no", "dk", "fi", "pl", "ru", "jp", "cn", "br", "com.br", "au", "com.au", "mx", "com.mx", "ar",
    "com.ar", "in", "co.in", "nz", "co.nz", "za", "co.za", "xyz", "online", "site", "club", "github.io",
    "gitlab.io", "herokuapp.com", "netlify.app", "vercel.app", "pages.dev", "blogspot.com", "wordpress.com",
    "gob.es", "com.es", "org.es", "nom.es", "edu.es",
];

fn es_sufijo(d: &str) -> bool {
    SUFIJOS.contains(&d)
}

/// "Sitio" de un host: su dominio registrable (sufijo público + una etiqueta). Una IP o un host que ya es
/// sufijo se devuelve tal cual.
pub fn sitio(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if es_ip(&host) {
        return host;
    }
    // Inicios de cada sufijo posible, del más largo al más corto.
    let inicios: Vec<usize> =
        core::iter::once(0).chain(host.match_indices('.').map(|(i, _)| i + 1)).collect();
    let n = inicios.len();
    // Sufijo más largo conocido; si ninguno, la última etiqueta (regla por defecto).
    let k = (0..n).find(|&i| es_sufijo(&host[inicios[i]..])).unwrap_or(n - 1);
    String::from(&host[inicios[k.saturating_sub(1)]..])
}

/// ¿El recurso es de otro sitio que la página? Si algo no se entiende, se trata como de tercero.
pub fn es_tercero(url_pagina: &str, url_recurso: &str) -> bool {
    match (partes(url_pagina), partes(url_recurso)) {
        (Some(a), Some(b)) => sitio(&a.host) != sitio(&b.host),
        _ => true,
    }
}

fn domain_match(host: &str, dom: &str) -> bool {
    host == dom
        || (!es_ip(host)
            && host.len() > dom.len()
            && host.ends_with(dom)
            && host[..host.len() - dom.len()].ends_with('.'))
}

fn path_match(ruta_req: &str, ruta_ck: &str) -> bool {
    ruta_req == ruta_ck
        || (ruta_req.starts_with(ruta_ck) && (ruta_ck.ends_with('/') || ruta_req[ruta_ck.len()..].starts_with('/')))
}

/// §5.1.4: la ruta de la petición hasta la última barra (sin ella), o "/".
fn ruta_defecto(ruta: &str) -> String {
    match ruta.rfind('/') {
        Some(i) if i > 0 => String::from(&ruta[..i]),
        _ => String::from("/"),
    }
}

// ---- Fechas (§5.1.1) ----

fn delim(b: u8) -> bool {
    b == 9
        || (0x20..=0x2f).contains(&b)
        || (0x3b..=0x40).contains(&b)
        || (0x5b..=0x60).contains(&b)
        || (0x7b..=0x7e).contains(&b)
}

/// Dígitos iniciales (hasta `max`): valor y cuántos son. El siguiente byte no puede ser otro dígito.
fn num(t: &[u8], max: usize) -> Option<(i64, usize)> {
    let n = t.iter().take_while(|b| b.is_ascii_digit()).count();
    if n == 0 || n > max {
        return None;
    }
    Some((t[..n].iter().fold(0i64, |a, &b| a * 10 + (b - b'0') as i64), n))
}

fn hms(t: &[u8]) -> Option<(i64, i64, i64)> {
    let (h, n) = num(t, 2)?;
    let t = t[n..].strip_prefix(b":")?;
    let (m, n) = num(t, 2)?;
    let t = t[n..].strip_prefix(b":")?;
    Some((h, m, num(t, 2)?.0))
}

/// Días desde 1970-01-01 de una fecha civil (algoritmo de Howard Hinnant, dominio público).
fn dias_civil(a: i64, m: i64, d: i64) -> i64 {
    let a = if m <= 2 { a - 1 } else { a };
    let era = a.div_euclid(400);
    let yoe = a - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
}

/// Fecha de `Expires` a segundo Unix, tolerante con el formato (RFC 1123, RFC 850, asctime…).
pub fn parsear_fecha(s: &str) -> Option<i64> {
    const MESES: [&[u8]; 12] =
        [b"jan", b"feb", b"mar", b"apr", b"may", b"jun", b"jul", b"aug", b"sep", b"oct", b"nov", b"dec"];
    let (mut hora, mut dia, mut mes, mut anio) = (None, None, None, None);
    for t in s.as_bytes().split(|&b| delim(b)).filter(|t| !t.is_empty()) {
        if hora.is_none() && hms(t).is_some() {
            hora = hms(t);
        } else if dia.is_none() && num(t, 2).is_some() {
            dia = num(t, 2).map(|x| x.0);
        } else if mes.is_none() && t.len() >= 3 && MESES.iter().any(|m| t[..3].eq_ignore_ascii_case(m)) {
            mes = MESES.iter().position(|m| t[..3].eq_ignore_ascii_case(m)).map(|p| p as i64 + 1);
        } else if anio.is_none() && matches!(num(t, 4), Some((_, n)) if n >= 2) {
            anio = num(t, 4).map(|x| x.0);
        }
    }
    let (h, mi, se) = hora?;
    let (dia, mes) = (dia?, mes?);
    let anio = match anio? {
        a @ 70..=99 => a + 1900,
        a @ 0..=69 => a + 2000,
        a => a,
    };
    if !(1..=31).contains(&dia) || anio < 1601 || h > 23 || mi > 59 || se > 59 {
        return None;
    }
    Some(dias_civil(anio, mes, dia) * 86400 + h * 3600 + mi * 60 + se)
}

// ---- Set-Cookie ----

/// Interpreta una cabecera `Set-Cookie` recibida de `url_peticion`. `None` si hay que ignorarla.
pub fn parsear_set_cookie(url_peticion: &str, valor_cabecera: &str, ahora_unix: i64) -> Option<Cookie> {
    let u = partes(url_peticion)?;
    if valor_cabecera.chars().any(|c| c.is_control()) {
        return None;
    }
    let mut it = valor_cabecera.split(';');
    let (n, v) = it.next()?.split_once('=')?;
    let (n, v) = (n.trim(), v.trim());
    if n.is_empty() || n.len() + v.len() > MAX_COOKIE {
        return None;
    }
    let mut c = Cookie {
        nombre: String::from(n),
        valor: String::from(v),
        dominio: u.host.clone(),
        solo_host: true,
        ruta: ruta_defecto(&u.ruta),
        expira: None,
        seguro: false,
        solo_http: false,
        same_site: SameSite::Lax,
        creada: 0,
        usada: 0,
    };
    let (mut max_age, mut expires, mut dom, mut ruta_explicita) = (None, None, None, false);
    for a in it {
        let (k, val) = a.split_once('=').map_or((a.trim(), ""), |(k, v)| (k.trim(), v.trim()));
        match k.to_ascii_lowercase().as_str() {
            "expires" => expires = parsear_fecha(val).or(expires),
            "max-age" => {
                let (neg, d) = val.strip_prefix('-').map_or((false, val), |r| (true, r));
                if !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()) {
                    let x = d.parse::<i64>().unwrap_or(i64::MAX);
                    max_age = Some(if neg { -x } else { x });
                }
            }
            "domain" => dom = Some(val.trim_start_matches('.').to_ascii_lowercase()).filter(|d| !d.is_empty()),
            "path" if val.starts_with('/') => {
                c.ruta = String::from(val);
                ruta_explicita = true;
            }
            "secure" => c.seguro = true,
            "httponly" => c.solo_http = true,
            "samesite" => {
                c.same_site = match val.to_ascii_lowercase().as_str() {
                    "strict" => SameSite::Strict,
                    "none" => SameSite::None,
                    _ => SameSite::Lax,
                }
            }
            _ => {}
        }
    }
    if let Some(d) = dom {
        if es_ip(&u.host) || es_sufijo(&d) {
            // Una IP o un sufijo público solo valen si coinciden exactamente con el host: queda host-only.
            if d != u.host {
                return None;
            }
        } else if domain_match(&u.host, &d) {
            c.dominio = d;
            c.solo_host = false;
        } else {
            return None;
        }
    }
    c.expira = match max_age {
        Some(m) if m <= 0 => Some(i64::MIN),
        Some(m) => Some(ahora_unix.saturating_add(m)),
        None => expires,
    };
    if c.seguro && !u.https {
        return None;
    }
    // rfc6265bis: `SameSite=None` sin `Secure` se rechaza (como hacen los navegadores actuales).
    if c.same_site == SameSite::None && !c.seguro {
        return None;
    }
    let nl = c.nombre.to_ascii_lowercase();
    if nl.starts_with("__secure-") && !c.seguro {
        return None;
    }
    if nl.starts_with("__host-") && !(c.seguro && c.solo_host && ruta_explicita && c.ruta == "/") {
        return None;
    }
    Some(c)
}

// ---- Tarro ----

#[derive(Default)]
pub struct Tarro {
    cookies: Vec<Cookie>,
    sec: u64,
}

fn escapar(s: &str, fuera: &mut String) {
    for c in s.chars() {
        match c {
            '\\' => fuera.push_str("\\\\"),
            '\t' => fuera.push_str("\\t"),
            '\n' => fuera.push_str("\\n"),
            '\r' => fuera.push_str("\\r"),
            c => fuera.push(c),
        }
    }
}

fn desescapar(s: &str) -> String {
    let mut o = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            o.push(c);
            continue;
        }
        match it.next() {
            Some('t') => o.push('\t'),
            Some('n') => o.push('\n'),
            Some('r') => o.push('\r'),
            Some(x) => o.push(x),
            None => {}
        }
    }
    o
}

impl Tarro {
    pub fn new() -> Tarro {
        Tarro::default()
    }

    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    pub fn cookies(&self) -> &[Cookie] {
        &self.cookies
    }

    /// Guarda o reemplaza (mismo nombre, dominio y ruta, conservando la creación). Una caducada borra la
    /// anterior. Aplica los límites por dominio y totales.
    pub fn guardar(&mut self, mut c: Cookie, ahora: i64) {
        if c.nombre.len() + c.valor.len() > MAX_COOKIE || c.nombre.is_empty() {
            return;
        }
        let previa = self
            .cookies
            .iter()
            .position(|o| o.nombre == c.nombre && o.dominio == c.dominio && o.ruta == c.ruta);
        if c.caducada(ahora) {
            if let Some(i) = previa {
                self.cookies.remove(i);
            }
            return;
        }
        self.sec += 1;
        c.usada = self.sec;
        c.creada = previa.map_or(self.sec, |i| self.cookies[i].creada);
        match previa {
            Some(i) => self.cookies[i] = c,
            None => {
                let dom = c.dominio.clone();
                self.cookies.push(c);
                while self.cookies.iter().filter(|o| o.dominio == dom).count() > MAX_DOMINIO {
                    self.expulsar(|o| o.dominio == dom);
                }
                while self.cookies.len() > MAX_TOTAL {
                    self.expulsar(|_| true);
                }
            }
        }
    }

    /// Quita la de último uso más antiguo entre las que cumplen `filtro`.
    fn expulsar(&mut self, filtro: impl Fn(&Cookie) -> bool) {
        let i = (0..self.cookies.len())
            .filter(|&i| filtro(&self.cookies[i]))
            .min_by_key(|&i| self.cookies[i].usada);
        if let Some(i) = i {
            self.cookies.remove(i);
        }
    }

    /// Procesa un `Set-Cookie` de una respuesta. Los subrecursos de terceros no pueden poner cookies.
    pub fn guardar_desde_respuesta(&mut self, url: &str, set_cookie: &str, ahora: i64, ctx: Contexto) -> bool {
        if ctx == Contexto::Subrecurso {
            return false;
        }
        match parsear_set_cookie(url, set_cookie, ahora) {
            Some(c) => {
                self.guardar(c, ahora);
                true
            }
            None => false,
        }
    }

    /// Valor de la cabecera `Cookie:` para `url`, o `None` si no hay ninguna que enviar.
    pub fn cabecera(&mut self, url: &str, ahora: i64, ctx: Contexto) -> Option<String> {
        if ctx == Contexto::Subrecurso {
            return None;
        }
        let u = partes(url)?;
        let mut idx: Vec<usize> = (0..self.cookies.len())
            .filter(|&i| {
                let c = &self.cookies[i];
                !c.caducada(ahora)
                    && if c.solo_host { u.host == c.dominio } else { domain_match(&u.host, &c.dominio) }
                    && path_match(&u.ruta, &c.ruta)
                    && (!c.seguro || u.https)
                    && !(ctx == Contexto::Navegacion && c.same_site == SameSite::Strict)
            })
            .collect();
        if idx.is_empty() {
            return None;
        }
        idx.sort_by(|&a, &b| {
            let (a, b) = (&self.cookies[a], &self.cookies[b]);
            b.ruta.len().cmp(&a.ruta.len()).then(a.creada.cmp(&b.creada))
        });
        let mut s = String::new();
        for i in idx {
            self.sec += 1;
            self.cookies[i].usada = self.sec;
            if !s.is_empty() {
                s.push_str("; ");
            }
            s.push_str(&self.cookies[i].nombre);
            s.push('=');
            s.push_str(&self.cookies[i].valor);
        }
        Some(s)
    }

    /// Borra las cookies de un sitio (cualquier host de él; se compara por dominio registrable).
    pub fn borrar_sitio(&mut self, host: &str) {
        let s = sitio(host);
        self.cookies.retain(|c| sitio(&c.dominio) != s);
    }

    pub fn borrar_todo(&mut self) {
        self.cookies.clear();
    }

    pub fn purgar(&mut self, ahora: i64) {
        self.cookies.retain(|c| !c.caducada(ahora));
    }

    /// Texto de las cookies persistentes cuyo dominio pasa el filtro: una por línea, campos separados por
    /// tabulador (nombre, valor, dominio, solo_host, ruta, expira, seguro, solo_http, same_site), con `\\`,
    /// `\t`, `\n` y `\r` escapados. Van por orden de creación.
    pub fn serializar(&self, filtro: impl Fn(&str) -> bool) -> String {
        let mut v: Vec<&Cookie> =
            self.cookies.iter().filter(|c| c.expira.is_some() && filtro(&c.dominio)).collect();
        v.sort_by_key(|c| c.creada);
        let mut s = String::new();
        for c in v {
            for campo in [&c.nombre, &c.valor, &c.dominio] {
                escapar(campo, &mut s);
                s.push('\t');
            }
            s.push(if c.solo_host { '1' } else { '0' });
            s.push('\t');
            escapar(&c.ruta, &mut s);
            s.push('\t');
            s.push_str(&alloc::format!("{}", c.expira.unwrap_or(0)));
            s.push('\t');
            s.push(if c.seguro { '1' } else { '0' });
            s.push('\t');
            s.push(if c.solo_http { '1' } else { '0' });
            s.push('\t');
            s.push(match c.same_site {
                SameSite::Strict => '2',
                SameSite::Lax => '1',
                SameSite::None => '0',
            });
            s.push('\n');
        }
        s
    }

    /// Carga lo escrito por [`Tarro::serializar`]: salta las líneas rotas y las caducadas, y respeta los límites.
    pub fn cargar(&mut self, texto: &str, ahora: i64) {
        for l in texto.lines() {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() != 9 {
                continue;
            }
            let Ok(expira) = f[5].parse::<i64>() else { continue };
            let (nombre, dominio, ruta) = (desescapar(f[0]), desescapar(f[2]), desescapar(f[4]));
            if nombre.is_empty() || dominio.is_empty() || !ruta.starts_with('/') || expira <= ahora {
                continue;
            }
            self.guardar(
                Cookie {
                    nombre,
                    valor: desescapar(f[1]),
                    dominio,
                    solo_host: f[3] == "1",
                    ruta,
                    expira: Some(expira),
                    seguro: f[6] == "1",
                    solo_http: f[7] == "1",
                    same_site: match f[8] {
                        "2" => SameSite::Strict,
                        "0" => SameSite::None,
                        _ => SameSite::Lax,
                    },
                    creada: 0,
                    usada: 0,
                },
                ahora,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_700_000_000;

    fn poner(t: &mut Tarro, url: &str, sc: &str) -> bool {
        t.guardar_desde_respuesta(url, sc, T, Contexto::PrimeraParte)
    }
    fn env(t: &mut Tarro, url: &str) -> Option<String> {
        t.cabecera(url, T, Contexto::PrimeraParte)
    }

    #[test]
    fn fechas() {
        // 2023-11-14 22:13:20 UTC
        assert_eq!(parsear_fecha("Tue, 14 Nov 2023 22:13:20 GMT"), Some(T));
        assert_eq!(parsear_fecha("Tuesday, 14-Nov-23 22:13:20 GMT"), Some(T));
        assert_eq!(parsear_fecha("Tue Nov 14 22:13:20 2023"), Some(T));
        assert_eq!(parsear_fecha("14 nov 2023 22:13:20"), Some(T)); // "nov" también existe en inglés
        assert_eq!(parsear_fecha("14 ene 2023 22:13:20"), None); // mes en español no vale
        assert_eq!(parsear_fecha("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(parsear_fecha("Thu, 01 Jan 70 00:00:00 GMT"), Some(0));
        assert_eq!(parsear_fecha("1 Jan 1600 00:00:00"), None);
        assert_eq!(parsear_fecha("1 Jan 2000 24:00:00"), None);
        assert_eq!(parsear_fecha(""), None);
        assert_eq!(parsear_fecha("ñ ñ 1é:é é\u{1F600}"), None);
    }

    #[test]
    fn host_only_y_domain() {
        let mut t = Tarro::new();
        assert!(poner(&mut t, "https://www.ejemplo.com/a", "a=1"));
        assert!(poner(&mut t, "https://www.ejemplo.com/a", "b=2; Domain=.Ejemplo.com"));
        assert_eq!(env(&mut t, "https://www.ejemplo.com/a").as_deref(), Some("a=1; b=2"));
        // Subdominio: solo la de Domain.
        assert_eq!(env(&mut t, "https://api.ejemplo.com/a").as_deref(), Some("b=2"));
        assert_eq!(env(&mut t, "https://otro.com/a"), None);
        // Domain que no cubre el host.
        assert!(!poner(&mut t, "https://www.ejemplo.com/", "c=3; Domain=otro.com"));
        assert!(!poner(&mut t, "https://ejemplo.com/", "c=3; Domain=www.ejemplo.com"));
        // Sufijo público.
        assert!(!poner(&mut t, "https://www.ejemplo.com/", "c=3; Domain=com"));
        assert!(!poner(&mut t, "https://a.github.io/", "c=3; Domain=github.io"));
        assert!(!poner(&mut t, "https://a.co.uk/", "c=3; Domain=co.uk"));
        // IP literal: solo exacto.
        assert!(!poner(&mut t, "http://10.0.0.5/", "c=3; Domain=0.0.5"));
        assert!(poner(&mut t, "http://10.0.0.5/", "c=3; Domain=10.0.0.5"));
        assert_eq!(env(&mut t, "http://10.0.0.5/").as_deref(), Some("c=3"));
    }

    #[test]
    fn rutas() {
        assert_eq!(parsear_set_cookie("https://x.com/a/b/c", "k=v", T).unwrap().ruta, "/a/b");
        assert_eq!(parsear_set_cookie("https://x.com/a", "k=v", T).unwrap().ruta, "/");
        assert_eq!(parsear_set_cookie("https://x.com", "k=v", T).unwrap().ruta, "/");
        assert_eq!(parsear_set_cookie("https://x.com/a/b?q=/z", "k=v", T).unwrap().ruta, "/a");
        assert_eq!(parsear_set_cookie("https://x.com/a/b", "k=v; Path=relativa", T).unwrap().ruta, "/a");
        let mut t = Tarro::new();
        poner(&mut t, "https://x.com/", "a=1; Path=/docs");
        poner(&mut t, "https://x.com/", "b=2; Path=/docs/api");
        poner(&mut t, "https://x.com/", "c=3; Path=/");
        assert_eq!(env(&mut t, "https://x.com/docs/api/x").as_deref(), Some("b=2; a=1; c=3"));
        assert_eq!(env(&mut t, "https://x.com/docs").as_deref(), Some("a=1; c=3"));
        assert_eq!(env(&mut t, "https://x.com/docsx").as_deref(), Some("c=3"));
    }

    #[test]
    fn caducidad() {
        let c = parsear_set_cookie("https://x.com/", "a=1; Expires=Tue, 14 Nov 2023 22:13:30 GMT", T).unwrap();
        assert_eq!(c.expira, Some(T + 10));
        // Max-Age gana a Expires.
        let c = parsear_set_cookie("https://x.com/", "a=1; Expires=Tue, 14 Nov 2023 22:13:30 GMT; Max-Age=100", T)
            .unwrap();
        assert_eq!(c.expira, Some(T + 100));
        let c = parsear_set_cookie("https://x.com/", "a=1; Max-Age=100; Expires=Tue, 14 Nov 2000 00:00:00 GMT", T)
            .unwrap();
        assert_eq!(c.expira, Some(T + 100));
        assert_eq!(parsear_set_cookie("https://x.com/", "a=1", T).unwrap().expira, None);
        // Borrado con Max-Age=0.
        let mut t = Tarro::new();
        poner(&mut t, "https://x.com/", "a=1");
        assert!(env(&mut t, "https://x.com/").is_some());
        poner(&mut t, "https://x.com/", "a=; Max-Age=0");
        assert_eq!(t.len(), 0);
        poner(&mut t, "https://x.com/", "a=1; Max-Age=10");
        assert_eq!(t.cabecera("https://x.com/", T + 11, Contexto::PrimeraParte), None);
        t.purgar(T + 11);
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn reemplazo_conserva_creacion() {
        let mut t = Tarro::new();
        poner(&mut t, "https://x.com/", "a=1");
        poner(&mut t, "https://x.com/", "b=1");
        poner(&mut t, "https://x.com/", "a=2");
        assert_eq!(t.len(), 2);
        assert_eq!(env(&mut t, "https://x.com/").as_deref(), Some("a=2; b=1"));
    }

    #[test]
    fn secure_y_prefijos() {
        let mut t = Tarro::new();
        assert!(!poner(&mut t, "http://x.com/", "a=1; Secure"));
        assert!(poner(&mut t, "https://x.com/", "s=1; Secure"));
        assert_eq!(env(&mut t, "http://x.com/"), None);
        assert_eq!(env(&mut t, "https://x.com/").as_deref(), Some("s=1"));
        assert!(!poner(&mut t, "https://x.com/", "__Secure-a=1"));
        assert!(poner(&mut t, "https://x.com/", "__Secure-a=1; Secure"));
        assert!(!poner(&mut t, "https://x.com/", "__Host-a=1; Secure")); // sin Path=/
        assert!(!poner(&mut t, "https://x.com/", "__Host-a=1; Secure; Path=/x"));
        assert!(!poner(&mut t, "https://x.com/", "__Host-a=1; Secure; Path=/; Domain=x.com"));
        assert!(!poner(&mut t, "https://x.com/", "__Host-a=1; Path=/"));
        assert!(poner(&mut t, "https://x.com/", "__Host-a=1; Secure; Path=/"));
        assert!(!poner(&mut t, "https://x.com/", "n=1; SameSite=None"));
        assert!(poner(&mut t, "https://x.com/", "n=1; SameSite=None; Secure"));
    }

    #[test]
    fn same_site_y_terceros() {
        let mut t = Tarro::new();
        poner(&mut t, "https://x.com/", "s=1; SameSite=Strict");
        poner(&mut t, "https://x.com/", "l=1");
        poner(&mut t, "https://x.com/", "n=1; SameSite=None; Secure");
        assert_eq!(t.cookies().iter().find(|c| c.nombre == "l").unwrap().same_site, SameSite::Lax);
        assert_eq!(t.cabecera("https://x.com/", T, Contexto::Navegacion).as_deref(), Some("l=1; n=1"));
        assert_eq!(t.cabecera("https://x.com/", T, Contexto::PrimeraParte).as_deref(), Some("s=1; l=1; n=1"));
        assert_eq!(t.cabecera("https://x.com/", T, Contexto::Subrecurso), None);
        // Set-Cookie de un subrecurso de tercero: ignorado.
        assert!(!t.guardar_desde_respuesta("https://track.com/p.png", "t=1", T, Contexto::Subrecurso));
        assert_eq!(t.len(), 3);
        assert!(t.guardar_desde_respuesta("https://y.com/", "t=1", T, Contexto::Navegacion));
    }

    #[test]
    fn sitios() {
        assert_eq!(sitio("www.ejemplo.com"), "ejemplo.com");
        assert_eq!(sitio("a.b.ejemplo.co.uk"), "ejemplo.co.uk");
        assert_eq!(sitio("ejemplo.co.uk"), "ejemplo.co.uk");
        assert_eq!(sitio("co.uk"), "co.uk");
        assert_eq!(sitio("usuario.github.io"), "usuario.github.io");
        assert_eq!(sitio("x.usuario.github.io"), "usuario.github.io");
        assert_eq!(sitio("localhost"), "localhost");
        assert_eq!(sitio("sub.raro.zzz"), "raro.zzz");
        assert_eq!(sitio("10.0.0.5"), "10.0.0.5");
        assert!(!es_tercero("https://www.ejemplo.com/", "http://img.ejemplo.com:8080/a.png"));
        assert!(es_tercero("https://www.ejemplo.com/", "https://cdn.otro.com/a.png"));
        assert!(es_tercero("https://a.github.io/", "https://b.github.io/"));
        assert!(es_tercero("https://a.com/", "basura"));
    }

    #[test]
    fn limites() {
        let mut t = Tarro::new();
        let grande = alloc::format!("a={}", "x".repeat(MAX_COOKIE));
        assert!(!poner(&mut t, "https://x.com/", &grande));
        assert!(poner(&mut t, "https://x.com/", &alloc::format!("a={}", "x".repeat(MAX_COOKIE - 1))));
        // 50 por dominio: se va la de uso más antiguo.
        let mut t = Tarro::new();
        for i in 0..MAX_DOMINIO {
            poner(&mut t, "https://x.com/", &alloc::format!("c{i}=1"));
        }
        env(&mut t, "https://x.com/"); // las usa todas por orden: c0 la más antigua
        poner(&mut t, "https://x.com/", "c0=2"); // reemplazo: c0 pasa a ser la más reciente
        poner(&mut t, "https://x.com/", "nueva=1");
        assert_eq!(t.len(), MAX_DOMINIO);
        assert!(t.cookies().iter().any(|c| c.nombre == "c0" && c.valor == "2"));
        assert!(t.cookies().iter().any(|c| c.nombre == "nueva"));
        assert!(!t.cookies().iter().any(|c| c.nombre == "c1"));
        // 3000 en total.
        let mut t = Tarro::new();
        for i in 0..MAX_TOTAL + 20 {
            poner(&mut t, &alloc::format!("https://h{i}.com/"), "a=1");
        }
        assert_eq!(t.len(), MAX_TOTAL);
        assert!(!t.cookies().iter().any(|c| c.dominio == "h0.com"));
    }

    #[test]
    fn borrar() {
        let mut t = Tarro::new();
        poner(&mut t, "https://www.x.com/", "a=1; Domain=x.com");
        poner(&mut t, "https://api.x.com/", "b=1");
        poner(&mut t, "https://y.com/", "c=1");
        t.borrar_sitio("www.x.com");
        assert_eq!(t.len(), 1);
        t.borrar_todo();
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn persistencia() {
        let mut t = Tarro::new();
        poner(&mut t, "https://x.com/", "sesion=1");
        poner(&mut t, "https://x.com/", "p=a\\b; Max-Age=1000; Path=/; Secure; HttpOnly; SameSite=Strict");
        poner(&mut t, "https://www.y.com/", "q=2; Domain=y.com; Max-Age=500");
        poner(&mut t, "https://z.com/", "z=3; Max-Age=500");
        let s = t.serializar(|d| d != "z.com");
        assert_eq!(s.lines().count(), 2);
        assert!(!s.contains("sesion"));
        let mut u = Tarro::new();
        u.cargar(&s, T);
        assert_eq!(u.len(), 2);
        let p = u.cookies().iter().find(|c| c.nombre == "p").unwrap();
        assert_eq!(
            (p.valor.as_str(), p.seguro, p.solo_http, p.same_site, p.solo_host),
            ("a\\b", true, true, SameSite::Strict, true)
        );
        let q = u.cookies().iter().find(|c| c.nombre == "q").unwrap();
        assert!(!q.solo_host && q.dominio == "y.com");
        // Caducadas al cargar: se descartan.
        let mut v = Tarro::new();
        v.cargar(&s, T + 2000);
        assert_eq!(v.len(), 0);
    }

    #[test]
    fn basura_sin_panico() {
        let mut t = Tarro::new();
        t.cargar(
            "\n\t\t\t\t\t\t\t\t\nsolo\tdos\na\tb\tc\t1\t/\tnan\t0\t0\t1\na\tb\tc\t1\tsin-barra\t99999999999\t0\t0\t1\n\\\\\\\n\u{1F600}\t\u{1F600}",
            T,
        );
        assert_eq!(t.len(), 0);
        for sc in [
            "",
            "=",
            ";;;",
            "a",
            "=v",
            "ñ=é; Domain=é; Path=/é; Max-Age=é; Expires=é",
            "a=1; Max-Age=-",
            "a=1; Max-Age=99999999999999999999999",
            "a=\u{1F600}; Expires=\u{1F600}\u{1F600}",
            "a=1\tb",
        ] {
            let _ = parsear_set_cookie("https://x.com/", sc, T);
        }
        for url in ["", "http://", "https://@", "https://[::1", "https://[::1]:80/x", "http://u:p@h:80", "ñ://é", "https://é.com/ñ"] {
            let _ = parsear_set_cookie(url, "a=1", T);
            let _ = t.cabecera(url, T, Contexto::Navegacion);
            let _ = sitio(url);
        }
        assert_eq!(sitio(""), "");
        assert_eq!(
            partes("http://u:p@Host.COM:80/x?y#z").map(|u| (u.host, u.ruta)),
            Some((String::from("host.com"), String::from("/x")))
        );
    }
}
