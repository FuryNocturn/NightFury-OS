// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Lo que Mensajes guarda en `<carpeta del usuario>\Mensajes\`: contactos y conversaciones, como
//! texto con una línea por entrada (ver docs/Mensajes-protocolo.md). Los archivos son del propio
//! usuario, pero se leen sin fiarse: una línea rota se salta, nunca da pánico.
use crate::proto::{nombre_valido, Clave, MAX_TEXTO_BYTES, MAX_TEXTO_CHARS};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use nf_core::crypto::{from_hex, to_hex};
use nf_core::platform::DateTime;

/// Mensajes que se guardan por conversación.
pub const MAX_HISTORIAL: usize = 1000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Estado {
    Entregado,
    Pendiente,
    Recibido,
}

impl Estado {
    fn letra(self) -> char {
        match self {
            Estado::Entregado => 'E',
            Estado::Pendiente => 'P',
            Estado::Recibido => 'R',
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Mensaje {
    pub saliente: bool,
    pub fecha: DateTime,
    pub id: u64,
    pub estado: Estado,
    pub texto: String,
}

/// Lo que se guarda de un contacto.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ficha {
    pub pk: Clave,
    pub nombre: String,
    pub verificado: bool,
}

fn escapar(t: &str) -> String {
    let mut o = String::with_capacity(t.len());
    for c in t.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            // Un retorno de carro rompería la línea al leerla con `lines()`.
            '\r' => o.push_str("\\r"),
            c => o.push(c),
        }
    }
    o
}

fn desescapar(t: &str) -> String {
    let mut o = String::with_capacity(t.len());
    let mut it = t.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            o.push(c);
            continue;
        }
        match it.next() {
            Some('n') => o.push('\n'),
            Some('t') => o.push('\t'),
            Some('r') => o.push('\r'),
            Some('\\') => o.push('\\'),
            Some(x) => {
                o.push('\\');
                o.push(x);
            }
            None => o.push('\\'),
        }
    }
    o
}

pub fn contactos_a_texto(v: &[Ficha]) -> String {
    let mut s = String::new();
    for f in v {
        s.push_str(&format!("{}\t{}\t{}\n", to_hex(&f.pk), f.nombre, if f.verificado { 1 } else { 0 }));
    }
    s
}

pub fn contactos_de_texto(t: &str) -> Vec<Ficha> {
    let mut v: Vec<Ficha> = Vec::new();
    for l in t.lines() {
        let mut p = l.split('\t');
        let (Some(k), Some(n), Some(ver), None) = (p.next(), p.next(), p.next(), p.next()) else { continue };
        let Some(pk) = from_hex(k).and_then(|b| Clave::try_from(b).ok()) else { continue };
        if !nombre_valido(n) || !matches!(ver, "0" | "1") || v.iter().any(|f| f.pk == pk) {
            continue;
        }
        v.push(Ficha { pk, nombre: String::from(n), verificado: ver == "1" });
    }
    v
}

fn fecha_a_texto(f: &DateTime) -> String {
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", f.year, f.month, f.day, f.hour, f.minute, f.second)
}

fn fecha_de_texto(t: &str) -> Option<DateTime> {
    let (d, h) = t.split_once(' ')?;
    let mut d = d.split('-');
    let mut h = h.split(':');
    let f = DateTime { year: d.next()?.parse().ok()?, month: d.next()?.parse().ok()?, day: d.next()?.parse().ok()?, hour: h.next()?.parse().ok()?, minute: h.next()?.parse().ok()?, second: h.next()?.parse().ok()? };
    (d.next().is_none() && h.next().is_none() && (1..=12).contains(&f.month) && (1..=31).contains(&f.day) && f.hour < 24 && f.minute < 60 && f.second < 60).then_some(f)
}

pub fn historial_a_texto(v: &[Mensaje]) -> String {
    let mut s = String::new();
    for m in v.iter().skip(v.len().saturating_sub(MAX_HISTORIAL)) {
        s.push_str(&format!("{}\t{}\t{:016x}\t{}\t{}\n", if m.saliente { '>' } else { '<' }, fecha_a_texto(&m.fecha), m.id, m.estado.letra(), escapar(&m.texto)));
    }
    s
}

/// Recorta un texto a los topes del protocolo (por si el archivo se editó a mano).
fn acotar(t: String) -> String {
    let mut t: String = t.chars().take(MAX_TEXTO_CHARS).collect();
    while t.len() > MAX_TEXTO_BYTES {
        t.pop();
    }
    t
}

pub fn historial_de_texto(t: &str) -> Vec<Mensaje> {
    let mut v = Vec::new();
    for l in t.lines() {
        let mut p = l.splitn(5, '\t');
        let (Some(dir), Some(f), Some(id), Some(e), Some(texto)) = (p.next(), p.next(), p.next(), p.next(), p.next()) else { continue };
        let saliente = match dir {
            ">" => true,
            "<" => false,
            _ => continue,
        };
        let estado = match (e, saliente) {
            ("E", true) => Estado::Entregado,
            ("P", true) => Estado::Pendiente,
            ("R", false) => Estado::Recibido,
            _ => continue,
        };
        let (Some(fecha), Ok(id)) = (fecha_de_texto(f), u64::from_str_radix(id, 16)) else { continue };
        let texto = acotar(desescapar(texto));
        if texto.is_empty() {
            continue;
        }
        v.push(Mensaje { saliente, fecha, id, estado, texto });
    }
    let sobran = v.len().saturating_sub(MAX_HISTORIAL);
    v.drain(..sobran);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_de_ida_y_vuelta() {
        for t in ["", "hola", "a\\b", "uno\ndos\ttres", "\\n no es un salto", "fin\\", "\\\\n\n\\t\t", "a\rb", "ñandú → ✓"] {
            assert_eq!(desescapar(&escapar(t)), t, "{t:?}");
            assert!(!escapar(t).contains(['\n', '\t', '\r']));
        }
        assert_eq!(desescapar("\\x"), "\\x");
    }

    #[test]
    fn contactos_de_ida_y_vuelta() {
        let v = alloc::vec![Ficha { pk: [1; 32], nombre: String::from("Ana"), verificado: true }, Ficha { pk: [2; 32], nombre: String::from("Equipo de Luis"), verificado: false }];
        assert_eq!(contactos_de_texto(&contactos_a_texto(&v)), v);
        // Líneas rotas, nombres con control, claves repetidas o cortas: se saltan.
        let k = to_hex(&[3; 32]);
        let sucio = format!("basura\n{k}\tBen\t2\n{k}\tBen\n{k}\tBe\u{7}n\t1\n{}\tcorta\t1\n{k}\tBen\t1\n{k}\tOtro\t0\n{k}\tBen\t1\textra\n", to_hex(&[3; 31]));
        assert_eq!(contactos_de_texto(&sucio), alloc::vec![Ficha { pk: [3; 32], nombre: String::from("Ben"), verificado: true }]);
    }

    fn msg(i: u64, saliente: bool, texto: &str) -> Mensaje {
        let estado = if saliente { Estado::Pendiente } else { Estado::Recibido };
        Mensaje { saliente, fecha: DateTime { year: 2026, month: 11, day: 15, hour: 21, minute: 37, second: 5 }, id: i, estado, texto: String::from(texto) }
    }

    #[test]
    fn historial_de_ida_y_vuelta() {
        let mut v = alloc::vec![msg(1, true, "uno\ndos"), msg(u64::MAX, false, "t\tab \\ fin"), msg(3, true, "x")];
        v[2].estado = Estado::Entregado;
        let t = historial_a_texto(&v);
        assert_eq!(t.lines().count(), 3);
        assert!(t.starts_with(">\t2026-11-15 21:37:05\t0000000000000001\tP\tuno\\ndos\n"));
        assert_eq!(historial_de_texto(&t), v);
    }

    #[test]
    fn historial_se_recorta_y_salta_lo_roto() {
        let v: Vec<Mensaje> = (0..1005).map(|i| msg(i, i % 2 == 0, "m")).collect();
        let t = historial_a_texto(&v);
        assert_eq!(t.lines().count(), MAX_HISTORIAL);
        let l = historial_de_texto(&t);
        assert_eq!((l.len(), l[0].id, l[MAX_HISTORIAL - 1].id), (MAX_HISTORIAL, 5, 1004));
        let roto = "x\ty\n>\t2026-13-01 00:00:00\t1\tP\ta\n>\t2026-11-15 21:37:05\tzz\tP\ta\n<\t2026-11-15 21:37:05\t1\tP\ta\n>\t2026-11-15 21:37:05\t1\tP\t\n>\t2026-11-15 21:37:05\t1\tP\tbien\n";
        let l = historial_de_texto(roto);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].texto, "bien");
        let enorme = format!(">\t2026-11-15 21:37:05\t1\tP\t{}\n", "é".repeat(5000));
        assert_eq!(historial_de_texto(&enorme)[0].texto.len(), MAX_TEXTO_BYTES);
    }
}
