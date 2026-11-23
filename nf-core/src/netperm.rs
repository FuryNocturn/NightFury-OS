// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! v0.12.1: permisos de red de los programas de terceros.
//!
//! Dos permisos por programa, como el cortafuegos de Windows:
//! - `Out`: usar la red (conectarse a otros equipos, DNS, ping, UDP saliente);
//! - `In`: aceptar conexiones de otros equipos de la red local (escuchar en TCP, UDP con puerto fijo).
//!
//! El programa se identifica por el SHA-256 de su `.nfx`: si cambia un solo byte, es otro programa y
//! vuelve a preguntar. Vale para todas las cuentas del equipo. Se guarda en
//! `C:\Sistema\config\red-permisos`, una línea por programa:
//! `<sha256 en hex>\t<out>\t<in>\t<nombre>` con `1` (permitido), `0` (no) o `-` (sin preguntar).
use alloc::string::String;
use alloc::vec::Vec;

pub const PERMS_FILE: &str = "C:\\Sistema\\config\\red-permisos";
/// Tope de programas recordados (el más viejo sale): el archivo no crece sin límite.
const MAX_PROGRAMS: usize = 256;
const NAME_MAX: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Out,
    In,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Perm {
    pub hash: [u8; 32],
    pub name: String,
    pub out: Option<bool>,
    pub inn: Option<bool>,
}

impl Perm {
    pub fn get(&self, k: Kind) -> Option<bool> {
        match k {
            Kind::Out => self.out,
            Kind::In => self.inn,
        }
    }
    pub fn set(&mut self, k: Kind, v: Option<bool>) {
        match k {
            Kind::Out => self.out = v,
            Kind::In => self.inn = v,
        }
    }
}

fn flag(s: &str) -> Option<Option<bool>> {
    match s {
        "1" => Some(Some(true)),
        "0" => Some(Some(false)),
        "-" => Some(None),
        _ => None,
    }
}

fn flag_text(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "1",
        Some(false) => "0",
        None => "-",
    }
}

/// Nombre limpio para guardar y enseñar: sin controles ni tabuladores, con tope.
pub fn clean_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(NAME_MAX).collect()
}

/// Lee el archivo; las líneas rotas se ignoran.
pub fn parse(data: &[u8]) -> Vec<Perm> {
    let text = core::str::from_utf8(data).unwrap_or("");
    let mut v: Vec<Perm> = Vec::new();
    for line in text.lines() {
        let mut f = line.splitn(4, '\t');
        let (Some(h), Some(o), Some(i), Some(n)) = (f.next(), f.next(), f.next(), f.next()) else { continue };
        let Some(bytes) = crate::crypto::from_hex(h) else { continue };
        let Ok(hash) = <[u8; 32]>::try_from(bytes.as_slice()) else { continue };
        let (Some(out), Some(inn)) = (flag(o), flag(i)) else { continue };
        if v.iter().any(|p| p.hash == hash) || v.len() >= MAX_PROGRAMS {
            continue;
        }
        v.push(Perm { hash, name: clean_name(n), out, inn });
    }
    v
}

pub fn render(perms: &[Perm]) -> Vec<u8> {
    let mut s = String::new();
    for p in perms {
        s.push_str(&crate::crypto::to_hex(&p.hash));
        for part in [flag_text(p.out), flag_text(p.inn)] {
            s.push('\t');
            s.push_str(part);
        }
        s.push('\t');
        s.push_str(&clean_name(&p.name));
        s.push('\n');
    }
    s.into_bytes()
}

/// Apunta una respuesta (y el nombre actual). Un programa nuevo entra al final; con la lista llena
/// sale el primero.
pub fn record(perms: &mut Vec<Perm>, hash: [u8; 32], name: &str, k: Kind, v: bool) {
    let i = match perms.iter().position(|p| p.hash == hash) {
        Some(i) => i,
        None => {
            if perms.len() >= MAX_PROGRAMS {
                perms.remove(0);
            }
            perms.push(Perm { hash, name: String::new(), out: None, inn: None });
            perms.len() - 1
        }
    };
    perms[i].name = clean_name(name);
    perms[i].set(k, Some(v));
}

/// Texto del aviso.
pub fn prompt_text(name: &str, k: Kind) -> String {
    let name = clean_name(name);
    match k {
        Kind::Out => alloc::format!("¿Permitir que «{name}» use la red?\n\nPodrá conectarse a otros equipos y a Internet."),
        Kind::In => alloc::format!(
            "¿Permitir que «{name}» acepte conexiones de otros equipos?\n\nSolo de equipos de tu red local."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ida_y_vuelta_y_lineas_rotas() {
        let mut v = Vec::new();
        record(&mut v, [1; 32], "Mensajes", Kind::Out, true);
        record(&mut v, [2; 32], "Juego\tmalo\n", Kind::In, false);
        record(&mut v, [1; 32], "Mensajes 2", Kind::In, true);
        let data = render(&v);
        let mut text = String::from_utf8(data).unwrap();
        text.push_str("basura\nzz\t1\t1\tx\n");
        text.push_str(&alloc::format!("{}\t7\t1\tmal\n", crate::crypto::to_hex(&[3; 32])));
        let w = parse(text.as_bytes());
        assert_eq!(w.len(), 2);
        assert_eq!(w[0], Perm { hash: [1; 32], name: String::from("Mensajes 2"), out: Some(true), inn: Some(true) });
        assert_eq!(w[1], Perm { hash: [2; 32], name: String::from("Juegomalo"), out: None, inn: Some(false) });
    }

    #[test]
    fn tope_de_programas() {
        let mut v = Vec::new();
        for i in 0..=MAX_PROGRAMS {
            let mut h = [0u8; 32];
            h[..8].copy_from_slice(&(i as u64).to_le_bytes());
            record(&mut v, h, "p", Kind::Out, true);
        }
        assert_eq!(v.len(), MAX_PROGRAMS);
        assert_eq!(v[0].hash[0], 1, "salió el más viejo");
    }
}
