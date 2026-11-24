// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Resolución de URLs relativas (RFC 3986 §5): solo http y https. Cualquier otro esquema da `None`.
use alloc::string::String;
use alloc::vec::Vec;

/// Largo máximo de una URL que se acepta.
const MAX_URL: usize = 8192;

struct Partes<'a> {
    esquema: String,
    autoridad: &'a str,
    ruta: &'a str,
    consulta: Option<&'a str>,
}

/// Esquema (en minúsculas) y resto, si `s` empieza por `esquema:`.
fn esquema(s: &str) -> Option<(String, &str)> {
    let i = s.find(':')?;
    let e = &s[..i];
    let mut it = e.bytes();
    if !it.next()?.is_ascii_alphabetic() || !it.all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.') {
        return None;
    }
    Some((e.to_ascii_lowercase(), &s[i + 1..]))
}

fn partes(s: &str) -> Option<Partes<'_>> {
    let (e, resto) = esquema(s)?;
    if e != "http" && e != "https" {
        return None;
    }
    let resto = resto.strip_prefix("//")?;
    let resto = resto.split('#').next().unwrap_or("");
    let fin = resto.find(['/', '?']).unwrap_or(resto.len());
    let (autoridad, resto) = resto.split_at(fin);
    if autoridad.is_empty() {
        return None;
    }
    let (ruta, consulta) = match resto.find('?') {
        Some(i) => (&resto[..i], Some(&resto[i + 1..])),
        None => (resto, None),
    };
    Some(Partes { esquema: e, autoridad, ruta, consulta })
}

/// RFC 3986 §5.2.4 sobre una ruta absoluta (o vacía).
fn quitar_puntos(ruta: &str) -> String {
    let mut pila: Vec<&str> = Vec::new();
    let mut barra_final = false;
    let mut it = ruta.split('/');
    it.next(); // lo anterior a la primera «/» (vacío)
    for seg in it {
        barra_final = false;
        match seg {
            "." => barra_final = true,
            ".." => {
                pila.pop();
                barra_final = true;
            }
            _ => pila.push(seg),
        }
    }
    let mut r = String::from("/");
    r.push_str(&pila.join("/"));
    if barra_final && !r.ends_with('/') {
        r.push('/');
    }
    r
}

/// Codifica con %XX lo que no puede ir tal cual en una petición (espacios, controles, no ASCII, comillas).
fn codificar(s: &str) -> String {
    let mut r = String::with_capacity(s.len());
    for b in s.bytes() {
        if b <= b' ' || b >= 0x7F || matches!(b, b'"' | b'<' | b'>' | b'\\' | b'^' | b'`' | b'{' | b'|' | b'}') {
            r.push('%');
            for n in [b >> 4, b & 15] {
                r.push(char::from_digit(n as u32, 16).unwrap_or('0').to_ascii_uppercase());
            }
        } else {
            r.push(b as char);
        }
    }
    r
}

fn montar(esq: &str, aut: &str, ruta: &str, consulta: Option<&str>) -> String {
    let mut r = String::new();
    r.push_str(esq);
    r.push_str("://");
    r.push_str(&aut.to_ascii_lowercase());
    r.push_str(&quitar_puntos(ruta));
    if let Some(q) = consulta {
        r.push('?');
        r.push_str(q);
    }
    codificar(&r)
}

/// Resuelve `rel` contra `base`. Quita el `#fragmento` y normaliza `.` y `..`. `None` si el resultado no es
/// http/https (about:, data:, javascript:, mailto:…) o la URL es absurda.
pub fn resolver(base: &str, rel: &str) -> Option<String> {
    let rel: String = rel.trim().chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    if rel.len() > MAX_URL {
        return None;
    }
    if let Some((e, _)) = esquema(&rel) {
        if e != "http" && e != "https" {
            return None;
        }
        let p = partes(&rel)?;
        return Some(montar(&p.esquema, p.autoridad, p.ruta, p.consulta));
    }
    let b = partes(base.trim())?;
    let rel = rel.split('#').next().unwrap_or("");
    if let Some(r) = rel.strip_prefix("//") {
        let abs = alloc::format!("{}://{}", b.esquema, r);
        let p = partes(&abs)?;
        return Some(montar(&p.esquema, p.autoridad, p.ruta, p.consulta));
    }
    if rel.is_empty() {
        return Some(montar(&b.esquema, b.autoridad, b.ruta, b.consulta));
    }
    if let Some(q) = rel.strip_prefix('?') {
        return Some(montar(&b.esquema, b.autoridad, b.ruta, Some(q)));
    }
    let (ruta_rel, q) = match rel.find('?') {
        Some(i) => (&rel[..i], Some(&rel[i + 1..])),
        None => (rel, None),
    };
    let ruta: String = if ruta_rel.starts_with('/') {
        ruta_rel.into()
    } else {
        let corte = b.ruta.rfind('/').map(|i| i + 1).unwrap_or(0);
        let mut r = String::from("/");
        r.push_str(b.ruta.get(1..corte).unwrap_or(""));
        r.push_str(ruta_rel);
        r
    };
    Some(montar(&b.esquema, b.autoridad, &ruta, q))
}
