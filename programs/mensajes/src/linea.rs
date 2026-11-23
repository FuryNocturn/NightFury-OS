// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Línea de entrada de una sola línea (no hay widget de caja de texto): cursor, Retroceso, Supr,
//! Inicio/Fin, flechas y Enter. Tope de 2000 caracteres y 4000 bytes, como el protocolo.
use crate::proto::{MAX_TEXTO_BYTES, MAX_TEXTO_CHARS};
use alloc::string::String;
use alloc::vec::Vec;
use nf_core::font;
use nf_core::input::{Key, KeyEvent};

#[derive(Default)]
pub struct Linea {
    t: Vec<char>,
    cur: usize,
}

#[derive(PartialEq, Eq, Debug)]
pub enum Accion {
    Nada,
    Cambio,
    Enviar,
}

impl Linea {
    pub fn texto(&self) -> String {
        self.t.iter().collect()
    }

    pub fn vacia(&self) -> bool {
        self.t.iter().all(|c| c.is_whitespace())
    }

    pub fn vaciar(&mut self) {
        self.t.clear();
        self.cur = 0;
    }

    pub fn tecla(&mut self, ev: &KeyEvent) -> Accion {
        match ev.key {
            Key::Enter => return Accion::Enviar,
            Key::Backspace if self.cur > 0 => {
                self.cur -= 1;
                self.t.remove(self.cur);
            }
            Key::Delete if self.cur < self.t.len() => {
                self.t.remove(self.cur);
            }
            Key::Left if self.cur > 0 => self.cur -= 1,
            Key::Right if self.cur < self.t.len() => self.cur += 1,
            Key::Home if self.cur > 0 => self.cur = 0,
            Key::End if self.cur < self.t.len() => self.cur = self.t.len(),
            Key::Backspace | Key::Delete | Key::Left | Key::Right | Key::Home | Key::End => return Accion::Nada,
            _ => match ev.ch {
                Some(c) if !c.is_control() && self.t.len() < MAX_TEXTO_CHARS && self.t.iter().map(|c| c.len_utf8()).sum::<usize>() + c.len_utf8() <= MAX_TEXTO_BYTES => {
                    self.t.insert(self.cur, c);
                    self.cur += 1;
                }
                _ => return Accion::Nada,
            },
        }
        Accion::Cambio
    }

    /// Lo que cabe en `ancho` píxeles manteniendo visible el cursor, y la x del cursor dentro de ello.
    pub fn visible(&self, ancho: i32, size: u32) -> (String, i32) {
        let mut ini = self.cur;
        let mut w = 0;
        while ini > 0 {
            let a = font::adv(self.t[ini - 1], size, false);
            if w + a > ancho {
                break;
            }
            w += a;
            ini -= 1;
        }
        let (mut s, mut x) = (String::new(), 0);
        for &c in &self.t[ini..] {
            x += font::adv(c, size, false);
            if x > ancho {
                break;
            }
            s.push(c);
        }
        (s, w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(key: Key, ch: Option<char>) -> KeyEvent {
        KeyEvent { key, ch, down: true, shift: false, ctrl: false, alt: false }
    }
    fn escribe(l: &mut Linea, t: &str) {
        for c in t.chars() {
            l.tecla(&k(Key::Char(c), Some(c)));
        }
    }

    #[test]
    fn edicion() {
        let mut l = Linea::default();
        escribe(&mut l, "hola");
        assert_eq!(l.tecla(&k(Key::Left, None)), Accion::Cambio);
        l.tecla(&k(Key::Backspace, None));
        escribe(&mut l, "ñ");
        l.tecla(&k(Key::Home, None));
        l.tecla(&k(Key::Delete, None));
        escribe(&mut l, "H");
        l.tecla(&k(Key::End, None));
        escribe(&mut l, "!");
        assert_eq!(l.texto(), "Hoña!");
        assert_eq!(l.tecla(&k(Key::Right, None)), Accion::Nada, "al final no hay más");
        assert_eq!(l.tecla(&k(Key::Enter, None)), Accion::Enviar);
        l.tecla(&k(Key::Tab, Some('\t')));
        assert_eq!(l.texto(), "Hoña!", "los controles no se insertan");
        l.vaciar();
        assert!(l.vacia());
        assert_eq!(l.tecla(&k(Key::Backspace, None)), Accion::Nada);
    }

    #[test]
    fn topes() {
        let mut l = Linea::default();
        escribe(&mut l, &"a".repeat(2100));
        assert_eq!(l.texto().chars().count(), MAX_TEXTO_CHARS);
        l.vaciar();
        escribe(&mut l, &"é".repeat(2100));
        assert_eq!(l.texto().len(), MAX_TEXTO_BYTES, "4000 bytes = 2000 caracteres de 2 bytes");
    }

    #[test]
    fn el_cursor_siempre_se_ve() {
        let mut l = Linea::default();
        escribe(&mut l, &"abcdefghij".repeat(30));
        let (s, x) = l.visible(100, 10);
        assert!(font::width(&s, 10, false) <= 100 && x <= 100 && x > 0);
        assert!(s.ends_with('j'));
        l.tecla(&k(Key::Home, None));
        let (s, x) = l.visible(100, 10);
        assert!(s.starts_with('a') && x == 0);
    }
}
