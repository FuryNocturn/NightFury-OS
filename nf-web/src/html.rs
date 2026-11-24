// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! HTML: codificación de caracteres, entidades, tokenizador y construcción del árbol (WHATWG, simplificados
//! pero tolerantes). La entrada es hostil: nada entra en pánico y el tamaño, la profundidad y el número de
//! nodos están limitados.
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Bytes de documento que se leen como mucho.
pub const MAX_DOC: usize = 8 << 20;
/// Profundidad máxima de elementos abiertos; más hondo se aplana (se cuelga del último sin abrirlo).
pub const MAX_PROF: usize = 256;
/// Nodos máximos del árbol.
pub const MAX_NODOS: usize = 200_000;
const MAX_ATTRS: usize = 64;

pub const SIN_PADRE: u32 = u32::MAX;

pub struct Elem {
    pub etq: String,
    pub attrs: Vec<(String, String)>,
}

pub enum Dato {
    Raiz,
    Elem(Elem),
    Texto(String),
}

pub struct Nodo {
    pub padre: u32,
    /// Posición dentro de `hijos` del padre.
    pub pos: u32,
    pub hijos: Vec<u32>,
    pub dato: Dato,
}

/// Árbol en arena. Los padres siempre van antes que sus hijos (orden del documento).
pub struct Dom {
    pub nodos: Vec<Nodo>,
}

impl Dom {
    pub fn elem(&self, n: usize) -> Option<&Elem> {
        match &self.nodos.get(n)?.dato {
            Dato::Elem(e) => Some(e),
            _ => None,
        }
    }
    pub fn etq(&self, n: usize) -> &str {
        self.elem(n).map(|e| e.etq.as_str()).unwrap_or("")
    }
    pub fn attr(&self, n: usize, nombre: &str) -> Option<&str> {
        self.elem(n)?.attrs.iter().find(|(k, _)| k == nombre).map(|(_, v)| v.as_str())
    }
    pub fn padre(&self, n: usize) -> Option<usize> {
        match self.nodos.get(n)?.padre {
            SIN_PADRE => None,
            p => Some(p as usize),
        }
    }
    /// Texto de todos los descendientes de `n` (para `<title>`, `<style>`…).
    pub fn texto(&self, n: usize) -> String {
        let mut r = String::new();
        let mut pila = alloc::vec![n];
        while let Some(x) = pila.pop() {
            let Some(nodo) = self.nodos.get(x) else { continue };
            if let Dato::Texto(t) = &nodo.dato {
                r.push_str(t);
            }
            pila.extend(nodo.hijos.iter().rev().map(|&h| h as usize));
        }
        r
    }
}

// ---------------------------------------------------------------------------------------------------------
// Codificación

const W1252: [u16; 32] = [
    0x20AC, 0x81, 0x201A, 0x192, 0x201E, 0x2026, 0x2020, 0x2021, 0x2C6, 0x2030, 0x160, 0x2039, 0x152, 0x8D, 0x17D,
    0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x2DC, 0x2122, 0x161, 0x203A, 0x153, 0x9D,
    0x17E, 0x178,
];

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Codif {
    Utf8,
    Latin,
}

fn etiqueta(s: &str) -> Option<Codif> {
    match s.trim().trim_matches(['"', '\'']).to_ascii_lowercase().as_str() {
        "utf-8" | "utf8" | "unicode-1-1-utf-8" => Some(Codif::Utf8),
        "iso-8859-1" | "iso8859-1" | "latin1" | "latin-1" | "l1" | "windows-1252" | "cp1252" | "x-cp1252" | "us-ascii"
        | "ascii" | "iso-8859-15" | "iso_8859-1" => Some(Codif::Latin),
        _ => None,
    }
}

/// Valor de `charset=` dentro de `s` (ya en minúsculas ASCII).
fn buscar_charset(s: &str) -> Option<Codif> {
    let i = s.find("charset")? + 7;
    let r = s[i..].trim_start().strip_prefix('=')?.trim_start().trim_start_matches(['"', '\'']);
    let fin = r.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).unwrap_or(r.len());
    etiqueta(&r[..fin])
}

/// Decodifica los bytes: BOM, `charset` del Content-Type, `<meta>` en el primer KiB; por defecto UTF-8 con
/// sustitución. shortcut: sin UTF-16 ni juegos asiáticos (se leen como UTF-8); añadir si hace falta.
pub fn decodificar(b: &[u8], tipo: Option<&str>) -> String {
    let b = &b[..b.len().min(MAX_DOC)];
    let (b, bom) = match b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(r) => (r, true),
        None => (b, false),
    };
    let mut cod = if bom { Some(Codif::Utf8) } else { None };
    if cod.is_none() {
        cod = tipo.and_then(|t| buscar_charset(&t.to_ascii_lowercase()));
    }
    if cod.is_none() {
        let cab: String = b[..b.len().min(1024)].iter().map(|&c| (c as char).to_ascii_lowercase()).collect();
        cod = cab.find("<meta").and_then(|_| buscar_charset(&cab));
    }
    match cod.unwrap_or(Codif::Utf8) {
        Codif::Utf8 => String::from_utf8_lossy(b).into_owned(),
        Codif::Latin => b
            .iter()
            .map(|&c| if (0x80..0xA0).contains(&c) { char::from_u32(W1252[(c - 0x80) as usize] as u32).unwrap_or('\u{FFFD}') } else { c as char })
            .collect(),
    }
}

// ---------------------------------------------------------------------------------------------------------
// Entidades

const LATIN1: [&str; 96] = [
    "nbsp", "iexcl", "cent", "pound", "curren", "yen", "brvbar", "sect", "uml", "copy", "ordf", "laquo", "not", "shy",
    "reg", "macr", "deg", "plusmn", "sup2", "sup3", "acute", "micro", "para", "middot", "cedil", "sup1", "ordm",
    "raquo", "frac14", "frac12", "frac34", "iquest", "Agrave", "Aacute", "Acirc", "Atilde", "Auml", "Aring", "AElig",
    "Ccedil", "Egrave", "Eacute", "Ecirc", "Euml", "Igrave", "Iacute", "Icirc", "Iuml", "ETH", "Ntilde", "Ograve",
    "Oacute", "Ocirc", "Otilde", "Ouml", "times", "Oslash", "Ugrave", "Uacute", "Ucirc", "Uuml", "Yacute", "THORN",
    "szlig", "agrave", "aacute", "acirc", "atilde", "auml", "aring", "aelig", "ccedil", "egrave", "eacute", "ecirc",
    "euml", "igrave", "iacute", "icirc", "iuml", "eth", "ntilde", "ograve", "oacute", "ocirc", "otilde", "ouml",
    "divide", "oslash", "ugrave", "uacute", "ucirc", "uuml", "yacute", "thorn", "yuml",
];

const GRIEGAS: [&str; 25] = [
    "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa", "lambda", "mu", "nu", "xi",
    "omicron", "pi", "rho", "sigmaf", "sigma", "tau", "upsilon", "phi", "chi", "psi", "omega",
];

/// Entidad con nombre (sin `&` ni `;`).
fn entidad(n: &str) -> Option<char> {
    if let Some(i) = LATIN1.iter().position(|&x| x == n) {
        return char::from_u32(160 + i as u32);
    }
    let c = match n {
        "quot" => 0x22,
        "amp" => 0x26,
        "apos" => 0x27,
        "lt" => 0x3C,
        "gt" => 0x3E,
        "OElig" => 0x152,
        "oelig" => 0x153,
        "Scaron" => 0x160,
        "scaron" => 0x161,
        "Yuml" => 0x178,
        "fnof" => 0x192,
        "circ" => 0x2C6,
        "tilde" => 0x2DC,
        "ensp" => 0x2002,
        "emsp" => 0x2003,
        "thinsp" => 0x2009,
        "zwnj" => 0x200C,
        "zwj" => 0x200D,
        "lrm" => 0x200E,
        "rlm" => 0x200F,
        "ndash" => 0x2013,
        "mdash" => 0x2014,
        "lsquo" => 0x2018,
        "rsquo" => 0x2019,
        "sbquo" => 0x201A,
        "ldquo" => 0x201C,
        "rdquo" => 0x201D,
        "bdquo" => 0x201E,
        "dagger" => 0x2020,
        "Dagger" => 0x2021,
        "bull" => 0x2022,
        "hellip" => 0x2026,
        "permil" => 0x2030,
        "prime" => 0x2032,
        "Prime" => 0x2033,
        "lsaquo" => 0x2039,
        "rsaquo" => 0x203A,
        "euro" => 0x20AC,
        "trade" => 0x2122,
        "larr" => 0x2190,
        "uarr" => 0x2191,
        "rarr" => 0x2192,
        "darr" => 0x2193,
        "harr" => 0x2194,
        "lArr" => 0x21D0,
        "rArr" => 0x21D2,
        "hArr" => 0x21D4,
        "forall" => 0x2200,
        "part" => 0x2202,
        "exist" => 0x2203,
        "empty" => 0x2205,
        "nabla" => 0x2207,
        "isin" => 0x2208,
        "notin" => 0x2209,
        "ni" => 0x220B,
        "prod" => 0x220F,
        "sum" => 0x2211,
        "minus" => 0x2212,
        "lowast" => 0x2217,
        "radic" => 0x221A,
        "prop" => 0x221D,
        "infin" => 0x221E,
        "ang" => 0x2220,
        "and" => 0x2227,
        "or" => 0x2228,
        "cap" => 0x2229,
        "cup" => 0x222A,
        "int" => 0x222B,
        "there4" => 0x2234,
        "sim" => 0x223C,
        "cong" => 0x2245,
        "asymp" => 0x2248,
        "ne" => 0x2260,
        "equiv" => 0x2261,
        "le" => 0x2264,
        "ge" => 0x2265,
        "sub" => 0x2282,
        "sup" => 0x2283,
        "loz" => 0x25CA,
        "spades" => 0x2660,
        "clubs" => 0x2663,
        "hearts" => 0x2665,
        "diams" => 0x2666,
        _ => {
            // letras griegas: minúsculas 3B1.., mayúsculas 391.. (sin «sigmaf» mayúscula)
            if let Some(i) = GRIEGAS.iter().position(|&x| x == n) {
                return char::from_u32(0x3B1 + i as u32);
            }
            let mut it = n.chars();
            let p = it.next()?;
            let mut min = String::new();
            min.push(p.to_ascii_lowercase());
            min.push_str(it.as_str());
            return match GRIEGAS.iter().position(|&x| x == min) {
                Some(i) if p.is_ascii_uppercase() && min != "sigmaf" => char::from_u32(0x391 + i as u32),
                _ => None,
            };
        }
    };
    char::from_u32(c)
}

/// Entidades que valen sin `;` final (herencia de HTML antiguo).
fn sin_punto_y_coma(n: &str) -> bool {
    matches!(n, "amp" | "lt" | "gt" | "quot") || LATIN1.contains(&n)
}

/// Escribe en `out` el texto de `s` con las referencias de caracteres resueltas. En atributos, una entidad
/// sin `;` seguida de `=` o letra se deja tal cual (norma).
pub fn decodificar_entidades(s: &str, atributo: bool, out: &mut String) {
    let mut resto = s;
    while let Some(i) = resto.find('&') {
        out.push_str(&resto[..i]);
        resto = &resto[i + 1..];
        let b = resto.as_bytes();
        if b.first() == Some(&b'#') {
            let hex = matches!(b.get(1), Some(b'x' | b'X'));
            let ini = if hex { 2 } else { 1 };
            let radix = if hex { 16 } else { 10 };
            let mut n: u32 = 0;
            let mut k = ini;
            while let Some(d) = b.get(k).and_then(|&c| (c as char).to_digit(radix)) {
                n = n.saturating_mul(radix).saturating_add(d);
                k += 1;
            }
            if k == ini {
                out.push('&');
                continue;
            }
            if b.get(k) == Some(&b';') {
                k += 1;
            }
            let c = match n {
                0x80..=0x9F => char::from_u32(W1252[(n - 0x80) as usize] as u32),
                0 | 0xD800..=0xDFFF => None,
                _ => char::from_u32(n),
            };
            out.push(c.unwrap_or('\u{FFFD}'));
            resto = &resto[k..];
            continue;
        }
        let n = b.iter().take(32).take_while(|c| c.is_ascii_alphanumeric()).count();
        let nombre = &resto[..n];
        let con_pc = b.get(n) == Some(&b';');
        let valida = if con_pc {
            entidad(nombre)
        } else if sin_punto_y_coma(nombre) && !(atributo && matches!(b.get(n), Some(b'='))) {
            entidad(nombre)
        } else {
            None
        };
        match valida {
            Some(c) => {
                out.push(c);
                resto = &resto[n + con_pc as usize..];
            }
            None => out.push('&'),
        }
    }
    out.push_str(resto);
}

// ---------------------------------------------------------------------------------------------------------
// Tokenizador y árbol

fn es_blanco(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0C)
}

fn vacio(e: &str) -> bool {
    matches!(
        e,
        "area" | "base" | "basefont" | "bgsound" | "br" | "col" | "embed" | "frame" | "hr" | "img" | "input" | "keygen"
            | "link" | "meta" | "param" | "source" | "track" | "wbr"
    )
}

/// Etiquetas cuyo contenido es texto sin marcas: (¿decodifica entidades?).
fn crudo(e: &str) -> Option<bool> {
    match e {
        "script" | "style" | "xmp" | "iframe" | "noembed" | "noframes" => Some(false),
        "textarea" | "title" => Some(true),
        _ => None,
    }
}

/// Fronteras que no se cruzan al buscar un elemento que cerrar implícitamente.
const ALCANCE: &[&str] = &["html", "table", "td", "th", "caption", "button", "template", "object", "marquee"];
const LISTAS: &[&str] = &["html", "body", "ul", "ol", "menu", "dir", "table", "td", "th", "template"];

fn cierra_p(e: &str) -> bool {
    matches!(
        e,
        "address" | "article" | "aside" | "blockquote" | "center" | "details" | "dialog" | "dir" | "div" | "dl"
            | "fieldset" | "figcaption" | "figure" | "footer" | "form" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            | "header" | "hgroup" | "hr" | "main" | "menu" | "nav" | "ol" | "p" | "pre" | "section" | "summary"
            | "table" | "ul" | "li" | "dd" | "dt" | "listing" | "xmp"
    )
}

struct Constructor {
    dom: Dom,
    pila: Vec<u32>,
    vistos: [bool; 3], // html, head, body
    en_head: bool,
}

impl Constructor {
    fn actual(&self) -> usize {
        self.pila.last().map(|&n| n as usize).unwrap_or(0)
    }

    fn nuevo(&mut self, dato: Dato) -> Option<usize> {
        if self.dom.nodos.len() >= MAX_NODOS {
            return None;
        }
        let p = self.actual();
        let id = self.dom.nodos.len();
        let pos = self.dom.nodos[p].hijos.len() as u32;
        self.dom.nodos[p].hijos.push(id as u32);
        self.dom.nodos.push(Nodo { padre: p as u32, pos, hijos: Vec::new(), dato });
        Some(id)
    }

    fn texto(&mut self, t: &mut String) {
        if t.is_empty() {
            return;
        }
        let ws = t.bytes().all(es_blanco);
        let cur = self.actual();
        let etq = self.dom.etq(cur);
        if ws && (cur == 0 || matches!(etq, "html" | "head" | "table" | "tbody" | "thead" | "tfoot" | "tr" | "colgroup" | "select")) {
            t.clear();
            return;
        }
        if !ws && self.en_head && matches!(etq, "html" | "head") {
            self.cerrar_hasta(&["head"], &[]);
            self.en_head = false;
        }
        if !ws && (cur == 0 || matches!(self.dom.etq(self.actual()), "html" | "head")) {
            self.asegurar_cuerpo();
        }
        let cur = self.actual();
        if let Some(u) = self.dom.nodos[cur].hijos.last().copied() {
            if let Dato::Texto(prev) = &mut self.dom.nodos[u as usize].dato {
                prev.push_str(t);
                t.clear();
                return;
            }
        }
        let s = core::mem::take(t);
        self.nuevo(Dato::Texto(s));
    }

    /// Contenido de cuerpo sin `<body>` explícito: se crea el `html` y el `body` que la norma sobreentiende.
    fn asegurar_cuerpo(&mut self) {
        if self.vistos[2] || !matches!(self.dom.etq(self.actual()), "" | "html" | "head") {
            return;
        }
        self.cerrar_hasta(&["head"], &[]);
        self.en_head = false;
        for (i, e) in [(0, "html"), (2, "body")] {
            if self.vistos[i] {
                continue;
            }
            self.vistos[i] = true;
            let Some(id) = self.nuevo(Dato::Elem(Elem { etq: e.to_string(), attrs: Vec::new() })) else { return };
            self.pila.push(id as u32);
        }
    }

    /// Cierra hasta el primer elemento de `nombres` (inclusive) si aparece antes que una frontera.
    fn cerrar_hasta(&mut self, nombres: &[&str], fronteras: &[&str]) {
        for i in (0..self.pila.len()).rev() {
            let e = self.dom.etq(self.pila[i] as usize);
            if nombres.contains(&e) {
                self.pila.truncate(i);
                return;
            }
            if fronteras.contains(&e) {
                return;
            }
        }
    }

    fn abrir(&mut self, etq: String, attrs: Vec<(String, String)>) {
        let idx = match etq.as_str() {
            "html" => Some(0),
            "head" => Some(1),
            "body" => Some(2),
            _ => None,
        };
        if let Some(i) = idx {
            if self.vistos[i] {
                return;
            }
            self.vistos[i] = true;
        }
        if self.en_head && !matches!(etq.as_str(), "title" | "meta" | "link" | "style" | "script" | "base" | "noscript" | "template" | "head") {
            self.cerrar_hasta(&["head"], &[]);
            self.en_head = false;
        }
        if !matches!(etq.as_str(), "html" | "head" | "body" | "title" | "meta" | "link" | "style" | "script" | "base" | "noscript" | "template" | "frameset") {
            self.asegurar_cuerpo();
        }
        let e = etq.as_str();
        if cierra_p(e) {
            self.cerrar_hasta(&["p"], ALCANCE);
        }
        match e {
            "li" => self.cerrar_hasta(&["li"], LISTAS),
            "dd" | "dt" => self.cerrar_hasta(&["dd", "dt"], LISTAS),
            "tr" => self.cerrar_hasta(&["tr"], &["table"]),
            "td" | "th" => self.cerrar_hasta(&["td", "th"], &["tr", "table"]),
            "tbody" | "thead" | "tfoot" => self.cerrar_hasta(&["tbody", "thead", "tfoot"], &["table"]),
            "option" => self.cerrar_hasta(&["option"], &["select", "datalist", "optgroup"]),
            "optgroup" => self.cerrar_hasta(&["optgroup"], &["select"]),
            "a" => self.cerrar_hasta(&["a"], ALCANCE),
            "button" => self.cerrar_hasta(&["button"], ALCANCE),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                if matches!(self.dom.etq(self.actual()), "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
                    self.pila.pop();
                }
            }
            _ => {}
        }
        let es_vacio = vacio(e);
        let es_head = e == "head";
        let Some(id) = self.nuevo(Dato::Elem(Elem { etq, attrs })) else { return };
        if !es_vacio && self.pila.len() < MAX_PROF {
            self.pila.push(id as u32);
            if es_head {
                self.en_head = true;
            }
        }
    }

    fn cerrar(&mut self, etq: &str) {
        match etq {
            "html" | "body" => return,
            "head" => {
                self.en_head = false;
            }
            _ => {}
        }
        let fr: &[&str] = match etq {
            "table" => &["html"],
            "td" | "th" | "tr" | "tbody" | "thead" | "tfoot" | "caption" => &["table", "html"],
            _ => ALCANCE,
        };
        self.cerrar_hasta(&[etq], fr);
    }
}

fn buscar_cierre(b: &[u8], desde: usize, nombre: &str) -> usize {
    let n = nombre.as_bytes();
    let mut i = desde;
    while i + 2 + n.len() <= b.len() {
        if b[i] == b'<'
            && b[i + 1] == b'/'
            && b[i + 2..i + 2 + n.len()].eq_ignore_ascii_case(n)
            && b.get(i + 2 + n.len()).map_or(true, |&c| es_blanco(c) || c == b'/' || c == b'>')
        {
            return i;
        }
        i += 1;
    }
    b.len()
}

fn hasta(b: &[u8], mut i: usize, f: impl Fn(u8) -> bool) -> usize {
    while i < b.len() && !f(b[i]) {
        i += 1;
    }
    i
}

fn saltar_blancos(b: &[u8], i: usize) -> usize {
    hasta(b, i, |c| !es_blanco(c))
}

/// Construye el árbol del documento.
pub fn parsear(src: &str) -> Dom {
    let b = src.as_bytes();
    let mut c = Constructor {
        dom: Dom { nodos: alloc::vec![Nodo { padre: SIN_PADRE, pos: 0, hijos: Vec::new(), dato: Dato::Raiz }] },
        pila: Vec::new(),
        vistos: [false; 3],
        en_head: false,
    };
    let mut texto = String::new();
    let mut i = 0;
    while i < b.len() && c.dom.nodos.len() < MAX_NODOS {
        if b[i] != b'<' {
            let j = hasta(b, i, |x| x == b'<');
            decodificar_entidades(&src[i..j], false, &mut texto);
            i = j;
            continue;
        }
        match b.get(i + 1).copied() {
            Some(b'!') => {
                c.texto(&mut texto);
                if b[i..].starts_with(b"<!--") {
                    i = src[i + 4..].find("-->").map_or(b.len(), |k| i + 4 + k + 3);
                } else if b[i..].starts_with(b"<![CDATA[") {
                    i = src[i + 9..].find("]]>").map_or(b.len(), |k| i + 9 + k + 3);
                } else {
                    i = hasta(b, i, |x| x == b'>') + 1; // DOCTYPE y comentarios mal formados
                }
            }
            Some(b'?') => {
                c.texto(&mut texto);
                i = hasta(b, i, |x| x == b'>') + 1;
            }
            Some(b'/') => {
                if b.get(i + 2).is_some_and(|x| x.is_ascii_alphabetic()) {
                    c.texto(&mut texto);
                    let f = hasta(b, i + 2, |x| es_blanco(x) || x == b'/' || x == b'>');
                    let nombre = src[i + 2..f].to_ascii_lowercase();
                    if nombre == "br" {
                        c.abrir(nombre, Vec::new());
                    } else {
                        c.cerrar(&nombre);
                    }
                    i = hasta(b, f, |x| x == b'>') + 1;
                } else {
                    c.texto(&mut texto);
                    i = hasta(b, i, |x| x == b'>') + 1;
                }
            }
            Some(x) if x.is_ascii_alphabetic() => {
                c.texto(&mut texto);
                let f = hasta(b, i + 1, |x| es_blanco(x) || x == b'/' || x == b'>');
                let nombre = src[i + 1..f].to_ascii_lowercase();
                let mut attrs: Vec<(String, String)> = Vec::new();
                let mut j = f;
                let mut cerrada = false;
                loop {
                    j = hasta(b, j, |x| !(es_blanco(x) || x == b'/'));
                    match b.get(j) {
                        None => break,
                        Some(b'>') => {
                            j += 1;
                            cerrada = true;
                            break;
                        }
                        _ => {}
                    }
                    let ini = j;
                    j = hasta(b, j + 1, |x| es_blanco(x) || x == b'/' || x == b'>' || x == b'=');
                    let an = src[ini..j].to_ascii_lowercase();
                    j = saltar_blancos(b, j);
                    let mut valor = String::new();
                    if b.get(j) == Some(&b'=') {
                        j = saltar_blancos(b, j + 1);
                        match b.get(j) {
                            Some(&q) if q == b'"' || q == b'\'' => {
                                let f = hasta(b, j + 1, |x| x == q);
                                decodificar_entidades(&src[j + 1..f], true, &mut valor);
                                j = f + 1;
                            }
                            Some(_) => {
                                let f = hasta(b, j, |x| es_blanco(x) || x == b'>');
                                decodificar_entidades(&src[j..f], true, &mut valor);
                                j = f;
                            }
                            None => {}
                        }
                    }
                    if attrs.len() < MAX_ATTRS && !attrs.iter().any(|(k, _)| *k == an) {
                        attrs.push((an, valor));
                    }
                }
                i = j.min(b.len());
                if !cerrada {
                    break; // etiqueta sin cerrar al final del documento: se descarta
                }
                let raw = crudo(&nombre);
                c.abrir(nombre.clone(), attrs);
                if matches!(nombre.as_str(), "pre" | "listing" | "textarea") {
                    // el salto de línea justo tras la etiqueta no cuenta (norma)
                    if b.get(i) == Some(&b'\r') {
                        i += 1;
                    }
                    if b.get(i) == Some(&b'\n') {
                        i += 1;
                    }
                }
                if let Some(ent) = raw {
                    let fin = buscar_cierre(b, i, &nombre);
                    let mut t = String::new();
                    if ent {
                        decodificar_entidades(&src[i..fin], false, &mut t);
                    } else {
                        t.push_str(&src[i..fin]);
                    }
                    if !t.is_empty() {
                        c.texto(&mut t);
                    }
                    i = fin;
                }
            }
            _ => {
                texto.push('<');
                i += 1;
            }
        }
    }
    c.texto(&mut texto);
    c.dom
}

/// Descarta espacios repetidos y cambia saltos por espacios (para el título).
pub fn compactar(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_string()
}
