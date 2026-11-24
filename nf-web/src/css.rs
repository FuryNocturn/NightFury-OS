// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! CSS: análisis de hojas con recuperación de errores (CSS Syntax 3), selectores (CSS 2.1 y un poco de
//! Selectors 3) y condiciones @media simples. Los valores se guardan como texto; `estilo` los interpreta.
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Bytes de hoja que se leen como mucho.
pub const MAX_CSS: usize = 2 << 20;
/// Reglas (una por selector) máximas por hoja.
const MAX_REGLAS: usize = 60_000;
/// Compuestos máximos por selector (más largo se descarta: la coincidencia es recursiva).
const MAX_PARTES: usize = 8;
const MAX_ANIDA: u32 = 4;

#[derive(Clone, Debug)]
pub struct Decl {
    pub prop: String,
    pub valor: String,
    pub importante: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Comb {
    Desc,
    Hijo,
    Sig,
    Hermano,
}

#[derive(Clone, Debug)]
pub enum Pseudo {
    Enlace,
    PrimerHijo,
    UltimoHijo,
    Raiz,
    No(Box<Compuesto>),
    /// :hover, ::before y demás: se aceptan pero nunca coinciden.
    Nunca,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpAttr {
    Existe,
    Igual,
    Palabra,
    Guion,
    Prefijo,
    Sufijo,
    Contiene,
}

#[derive(Clone, Debug)]
pub struct AtrSel {
    pub nombre: String,
    pub op: OpAttr,
    pub valor: String,
}

#[derive(Clone, Debug, Default)]
pub struct Compuesto {
    pub etq: Option<String>,
    pub id: Option<String>,
    pub clases: Vec<String>,
    pub attrs: Vec<AtrSel>,
    pub pseudos: Vec<Pseudo>,
}

#[derive(Clone, Debug)]
pub struct Parte {
    /// Relación con la parte anterior (a su izquierda); en la primera no se usa.
    pub comb: Comb,
    pub c: Compuesto,
}

#[derive(Clone, Debug)]
pub struct Selector {
    pub partes: Vec<Parte>,
    /// Especificidad: ids<<16 | clases<<8 | tipos.
    pub espec: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Consulta {
    pub ok: bool,
    pub min: f32,
    pub max: f32,
    pub neg: bool,
}

pub type Medias = Vec<Vec<Consulta>>;

pub struct Regla {
    pub sel: Selector,
    pub decls: Rc<Vec<Decl>>,
    /// Condiciones @media que la envuelven (todas deben cumplirse; dentro de cada una vale cualquier consulta).
    pub medias: Medias,
}

#[derive(Default)]
pub struct Hoja {
    pub reglas: Vec<Regla>,
}

// ---------------------------------------------------------------------------------------------------------
// Texto: comentarios y búsqueda de límites

fn quitar_comentarios(s: &str) -> String {
    let b = s.as_bytes();
    let mut r = String::with_capacity(s.len());
    let (mut i, mut ini) = (0, 0);
    let mut cadena = 0u8;
    while i < b.len() {
        let c = b[i];
        if cadena != 0 {
            if c == b'\\' {
                i += 1;
            } else if c == cadena || c == b'\n' {
                cadena = 0;
            }
        } else if c == b'"' || c == b'\'' {
            cadena = c;
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            r.push_str(&s[ini..i]);
            r.push(' ');
            i = s[i + 2..].find("*/").map_or(b.len(), |k| i + 2 + k + 2);
            ini = i.min(b.len());
            continue;
        }
        i += 1;
    }
    if ini < s.len() {
        r.push_str(&s[ini..]);
    }
    r
}

/// Índice del primer byte de `fin` (a nivel 0 de paréntesis, corchetes y cadenas) desde `desde`, o `len`.
fn buscar_nivel0(s: &str, desde: usize, fin: &[u8]) -> usize {
    let b = s.as_bytes();
    let (mut i, mut n) = (desde, 0i32);
    let mut cadena = 0u8;
    while i < b.len() {
        let c = b[i];
        if cadena != 0 {
            if c == b'\\' {
                i += 1;
            } else if c == cadena {
                cadena = 0;
            }
        } else if c == b'"' || c == b'\'' {
            cadena = c;
        } else if c == b'\\' {
            i += 1;
        } else if n == 0 && fin.contains(&c) {
            return i;
        } else if c == b'(' || c == b'[' || c == b'{' {
            n += 1;
        } else if (c == b')' || c == b']' || c == b'}') && n > 0 {
            n -= 1;
        }
        i += 1;
    }
    b.len()
}

/// Dado `s[desde]` justo después de un `{`, índice del `}` que lo cierra (o `len` si falta).
fn fin_bloque(s: &str, desde: usize) -> usize {
    let b = s.as_bytes();
    let (mut i, mut n) = (desde, 1u32);
    let mut cadena = 0u8;
    while i < b.len() {
        let c = b[i];
        if cadena != 0 {
            if c == b'\\' {
                i += 1;
            } else if c == cadena {
                cadena = 0;
            }
        } else if c == b'"' || c == b'\'' {
            cadena = c;
        } else if c == b'\\' {
            i += 1;
        } else if c == b'{' {
            n += 1;
        } else if c == b'}' {
            n -= 1;
            if n == 0 {
                return i;
            }
        }
        i += 1;
    }
    b.len()
}

fn partir(s: &str, sep: u8) -> Vec<&str> {
    let mut r = Vec::new();
    let mut i = 0;
    loop {
        let f = buscar_nivel0(s, i, &[sep]);
        r.push(&s[i..f]);
        if f >= s.len() {
            return r;
        }
        i = f + 1;
    }
}

/// Declaraciones de un bloque o de un atributo `style`. Las inválidas se saltan.
pub fn parsear_decls(texto: &str) -> Vec<Decl> {
    let mut r = Vec::new();
    for d in partir(texto, b';') {
        let Some(i) = d.find(':') else { continue };
        let prop = d[..i].trim().to_ascii_lowercase();
        if prop.is_empty() || !prop.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') || prop.starts_with("--") {
            continue;
        }
        let mut valor = d[i + 1..].trim();
        let mut importante = false;
        if let Some(p) = valor.rfind('!') {
            if valor[p + 1..].trim().eq_ignore_ascii_case("important") {
                importante = true;
                valor = valor[..p].trim_end();
            }
        }
        if !valor.is_empty() {
            r.push(Decl { prop, valor: valor.to_string(), importante });
        }
    }
    r
}

// ---------------------------------------------------------------------------------------------------------
// Selectores

fn es_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_' || !c.is_ascii()
}

/// Lee un identificador (con escapes `\`) desde `*i`.
fn ident(cs: &[char], i: &mut usize) -> String {
    let mut r = String::new();
    while let Some(&c) = cs.get(*i) {
        if c == '\\' {
            let Some(&n) = cs.get(*i + 1) else { break };
            *i += 2;
            if n.is_ascii_hexdigit() {
                let mut v = n.to_digit(16).unwrap_or(0);
                let mut k = 1;
                while k < 6 && cs.get(*i).is_some_and(|c| c.is_ascii_hexdigit()) {
                    v = v * 16 + cs[*i].to_digit(16).unwrap_or(0);
                    *i += 1;
                    k += 1;
                }
                if cs.get(*i).is_some_and(|c| c.is_whitespace()) {
                    *i += 1;
                }
                r.push(char::from_u32(v).filter(|&c| c != '\0').unwrap_or('\u{FFFD}'));
            } else {
                r.push(n);
            }
        } else if es_ident(c) {
            r.push(c);
            *i += 1;
        } else {
            break;
        }
    }
    r
}

fn parse_attr(cs: &[char], i: &mut usize) -> Option<AtrSel> {
    // *i apunta tras '['
    let blanco = |cs: &[char], i: &mut usize| {
        while cs.get(*i).is_some_and(|c| c.is_whitespace()) {
            *i += 1;
        }
    };
    blanco(cs, i);
    let nombre = ident(cs, i).to_ascii_lowercase();
    if nombre.is_empty() {
        return None;
    }
    blanco(cs, i);
    let op = match cs.get(*i)? {
        ']' => {
            *i += 1;
            return Some(AtrSel { nombre, op: OpAttr::Existe, valor: String::new() });
        }
        '=' => {
            *i += 1;
            OpAttr::Igual
        }
        c @ ('~' | '|' | '^' | '$' | '*') => {
            if cs.get(*i + 1) != Some(&'=') {
                return None;
            }
            *i += 2;
            match c {
                '~' => OpAttr::Palabra,
                '|' => OpAttr::Guion,
                '^' => OpAttr::Prefijo,
                '$' => OpAttr::Sufijo,
                _ => OpAttr::Contiene,
            }
        }
        _ => return None,
    };
    blanco(cs, i);
    let valor = match cs.get(*i)? {
        &q @ ('"' | '\'') => {
            *i += 1;
            let mut v = String::new();
            while let Some(&c) = cs.get(*i) {
                *i += 1;
                if c == q {
                    break;
                }
                if c == '\\' {
                    if let Some(&n) = cs.get(*i) {
                        v.push(n);
                        *i += 1;
                    }
                } else {
                    v.push(c);
                }
            }
            v
        }
        _ => ident(cs, i),
    };
    blanco(cs, i);
    // bandera `i` (sin distinguir mayúsculas): se acepta pero no se aplica. shortcut: siempre distingue.
    if matches!(cs.get(*i), Some('i' | 'I' | 's' | 'S')) {
        *i += 1;
        blanco(cs, i);
    }
    if cs.get(*i) != Some(&']') {
        return None;
    }
    *i += 1;
    Some(AtrSel { nombre, op, valor })
}

fn compuesto(cs: &[char], i: &mut usize, dentro_not: bool) -> Option<Compuesto> {
    let mut c = Compuesto::default();
    let mut algo = false;
    while let Some(&ch) = cs.get(*i) {
        match ch {
            '*' => {
                *i += 1;
                algo = true;
            }
            '.' => {
                *i += 1;
                let n = ident(cs, i);
                if n.is_empty() {
                    return None;
                }
                c.clases.push(n);
                algo = true;
            }
            '#' => {
                *i += 1;
                let n = ident(cs, i);
                if n.is_empty() {
                    return None;
                }
                c.id = Some(n);
                algo = true;
            }
            '[' => {
                *i += 1;
                c.attrs.push(parse_attr(cs, i)?);
                algo = true;
            }
            ':' => {
                *i += 1;
                let doble = cs.get(*i) == Some(&':');
                if doble {
                    *i += 1;
                }
                let n = ident(cs, i).to_ascii_lowercase();
                if n.is_empty() {
                    return None;
                }
                let mut arg: Option<String> = None;
                if cs.get(*i) == Some(&'(') {
                    let ini = *i + 1;
                    let (mut k, mut prof) = (ini, 1);
                    while let Some(&x) = cs.get(k) {
                        match x {
                            '(' => prof += 1,
                            ')' => {
                                prof -= 1;
                                if prof == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        k += 1;
                    }
                    if prof != 0 {
                        return None;
                    }
                    arg = Some(cs[ini..k].iter().collect());
                    *i = k + 1;
                }
                c.pseudos.push(if doble {
                    Pseudo::Nunca
                } else {
                    match (n.as_str(), arg) {
                        ("link" | "visited" | "any-link", None) => Pseudo::Enlace,
                        ("first-child", None) => Pseudo::PrimerHijo,
                        ("last-child", None) => Pseudo::UltimoHijo,
                        ("root", None) => Pseudo::Raiz,
                        ("not", Some(a)) if !dentro_not => {
                            let cs2: Vec<char> = a.trim().chars().collect();
                            let mut j = 0;
                            match compuesto(&cs2, &mut j, true) {
                                Some(inner) if j == cs2.len() => Pseudo::No(Box::new(inner)),
                                _ => Pseudo::Nunca,
                            }
                        }
                        _ => Pseudo::Nunca,
                    }
                });
                algo = true;
            }
            c2 if es_ident(c2) || c2 == '\\' => {
                if c.etq.is_some() || algo {
                    return None;
                }
                c.etq = Some(ident(cs, i).to_ascii_lowercase());
                algo = true;
            }
            _ => break,
        }
    }
    if algo {
        Some(c)
    } else {
        None
    }
}

fn espec_de(c: &Compuesto) -> u32 {
    let mut s = 0u32;
    if c.id.is_some() {
        s += 1 << 16;
    }
    s += (c.clases.len() + c.attrs.len()) as u32 * (1 << 8);
    for p in &c.pseudos {
        s += match p {
            Pseudo::No(i) => espec_de(i),
            _ => 1 << 8,
        };
    }
    if c.etq.as_deref().is_some_and(|e| !e.is_empty()) {
        s += 1;
    }
    s
}

fn parse_selector(s: &str) -> Option<Selector> {
    let cs: Vec<char> = s.trim().chars().collect();
    let mut i = 0;
    let mut partes: Vec<Parte> = Vec::new();
    let mut comb = Comb::Desc;
    loop {
        let c = compuesto(&cs, &mut i, false)?;
        partes.push(Parte { comb, c });
        if partes.len() > MAX_PARTES {
            return None;
        }
        let ini = i;
        while cs.get(i).is_some_and(|c| c.is_whitespace()) {
            i += 1;
        }
        match cs.get(i) {
            None => break,
            Some(&x @ ('>' | '+' | '~')) => {
                i += 1;
                while cs.get(i).is_some_and(|c| c.is_whitespace()) {
                    i += 1;
                }
                comb = match x {
                    '>' => Comb::Hijo,
                    '+' => Comb::Sig,
                    _ => Comb::Hermano,
                };
            }
            Some(_) if i > ini => comb = Comb::Desc,
            Some(_) => return None,
        }
    }
    let espec = partes.iter().map(|p| espec_de(&p.c)).fold(0u32, |a, b| a.saturating_add(b));
    Some(Selector { partes, espec })
}

// ---------------------------------------------------------------------------------------------------------
// @media

fn largo_media(v: &str) -> Option<f32> {
    let v = v.trim();
    let (n, mult) = if let Some(n) = v.strip_suffix("px") {
        (n, 1.0)
    } else if let Some(n) = v.strip_suffix("em").or_else(|| v.strip_suffix("rem")) {
        (n, 16.0)
    } else {
        (v, 1.0)
    };
    Some(crate::valores::numero(n.trim())? * mult)
}

fn una_consulta(q: &str) -> Consulta {
    let mut q = q.trim().to_ascii_lowercase();
    let mut neg = false;
    if let Some(r) = q.strip_prefix("not ") {
        neg = true;
        q = r.trim().to_string();
    } else if let Some(r) = q.strip_prefix("only ") {
        q = r.trim().to_string();
    }
    let mut c = Consulta { ok: true, min: 0.0, max: f32::INFINITY, neg };
    if q.is_empty() {
        return c;
    }
    for p in q.replace(") and", ") and ").split(" and ") {
        let p = p.trim();
        if let Some(inner) = p.strip_prefix('(') {
            let inner = inner.strip_suffix(')').unwrap_or(inner);
            match inner.split_once(':') {
                Some(("min-width" | "min-device-width", v)) => match largo_media(v) {
                    Some(x) => c.min = c.min.max(x),
                    None => c.ok = false,
                },
                Some(("max-width" | "max-device-width", v)) => match largo_media(v) {
                    Some(x) => c.max = c.max.min(x),
                    None => c.ok = false,
                },
                _ => c.ok = false,
            }
        } else if !matches!(p, "all" | "screen") {
            c.ok = false;
        }
    }
    c
}

pub fn parsear_media(prelude: &str) -> Vec<Consulta> {
    partir(prelude, b',').into_iter().map(una_consulta).collect()
}

/// ¿Se cumplen todas las condiciones con un viewport de `ancho` px?
pub fn media_ok(m: &Medias, ancho: f32) -> bool {
    m.iter().all(|g| g.iter().any(|c| (c.ok && ancho >= c.min && ancho <= c.max) != c.neg))
}

// ---------------------------------------------------------------------------------------------------------
// Hojas

fn analizar(css: &str, medias: &Medias, hoja: &mut Hoja, prof: u32) {
    let b = css.as_bytes();
    let mut i = 0;
    while i < b.len() && hoja.reglas.len() < MAX_REGLAS {
        match b[i] {
            c if c.is_ascii_whitespace() => i += 1,
            b'}' => i += 1,
            b'<' if css[i..].starts_with("<!--") => i += 4,
            b'-' if css[i..].starts_with("-->") => i += 3,
            b'@' => {
                let f = buscar_nivel0(css, i, &[b';', b'{']);
                let cabeza = &css[i + 1..f];
                if f >= b.len() {
                    break;
                }
                if b[f] == b';' {
                    i = f + 1;
                    continue;
                }
                let k = fin_bloque(css, f + 1);
                let nombre_fin = cabeza.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).unwrap_or(cabeza.len());
                if cabeza[..nombre_fin].eq_ignore_ascii_case("media") && prof < MAX_ANIDA {
                    let mut m = medias.clone();
                    m.push(parsear_media(&cabeza[nombre_fin..]));
                    analizar(&css[f + 1..k], &m, hoja, prof + 1);
                }
                i = k + 1;
            }
            _ => {
                let f = buscar_nivel0(css, i, &[b'{']);
                if f >= b.len() {
                    break;
                }
                let k = fin_bloque(css, f + 1);
                let prelude = &css[i..f];
                i = k + 1;
                let sels: Option<Vec<Selector>> = partir(prelude, b',').into_iter().map(parse_selector).collect();
                if let Some(sels) = sels {
                    let decls = Rc::new(parsear_decls(&css[f + 1..k]));
                    if decls.is_empty() {
                        continue;
                    }
                    for sel in sels {
                        hoja.reglas.push(Regla { sel, decls: decls.clone(), medias: medias.clone() });
                    }
                }
            }
        }
    }
}

/// Analiza una hoja; `medias` son las condiciones del `<link media>` o `<style media>` que la trae.
pub fn parsear_hoja(css: &str, medias: Medias) -> Hoja {
    let mut fin = css.len().min(MAX_CSS);
    while !css.is_char_boundary(fin) {
        fin -= 1;
    }
    let limpio = quitar_comentarios(&css[..fin]);
    let mut h = Hoja::default();
    analizar(&limpio, &medias, &mut h, 0);
    h
}
