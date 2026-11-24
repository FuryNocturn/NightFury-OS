// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Campo de texto de una línea (no hay widget): cursor, Retroceso, Supr, Inicio/Fin, flechas y «todo seleccionado».
use alloc::string::String;
use alloc::vec::Vec;
use nf_core::font;
use nf_core::input::{Key, KeyEvent};

/// Tope de caracteres (las URL largas caben de sobra).
const MAX: usize = 2048;

#[derive(Default, Clone)]
pub struct Campo {
    t: Vec<char>,
    cur: usize,
    /// Todo el texto seleccionado: lo siguiente que se escriba lo sustituye.
    pub todo: bool,
}

#[derive(PartialEq, Eq, Debug)]
pub enum Accion {
    Nada,
    Cambio,
    Enter,
}

impl Campo {
    pub fn texto(&self) -> String {
        self.t.iter().collect()
    }

    pub fn vacio(&self) -> bool {
        self.t.is_empty()
    }

    pub fn poner(&mut self, s: &str) {
        self.t = s.chars().take(MAX).collect();
        self.cur = self.t.len();
        self.todo = false;
    }

    pub fn seleccionar_todo(&mut self) {
        self.todo = !self.t.is_empty();
        self.cur = self.t.len();
    }

    pub fn tecla(&mut self, ev: &KeyEvent) -> Accion {
        let todo = core::mem::take(&mut self.todo);
        match ev.key {
            Key::Enter => {
                self.todo = todo;
                return Accion::Enter;
            }
            Key::Backspace | Key::Delete if todo => self.poner(""),
            Key::Backspace if self.cur > 0 => {
                self.cur -= 1;
                self.t.remove(self.cur);
            }
            Key::Delete if self.cur < self.t.len() => {
                self.t.remove(self.cur);
            }
            Key::Left if todo => self.cur = 0,
            Key::Left if self.cur > 0 => self.cur -= 1,
            Key::Right if todo => self.cur = self.t.len(),
            Key::Right if self.cur < self.t.len() => self.cur += 1,
            Key::Home => self.cur = 0,
            Key::End => self.cur = self.t.len(),
            Key::Backspace | Key::Delete | Key::Left | Key::Right => {
                self.todo = todo;
                return Accion::Nada;
            }
            _ => match ev.ch {
                Some(c) if !c.is_control() && !ev.ctrl && !ev.alt => {
                    if todo {
                        self.poner("");
                    }
                    if self.t.len() >= MAX {
                        return Accion::Nada;
                    }
                    self.t.insert(self.cur, c);
                    self.cur += 1;
                }
                _ => {
                    self.todo = todo;
                    return Accion::Nada;
                }
            },
        }
        Accion::Cambio
    }

    /// Primer carácter visible en `ancho` píxeles manteniendo a la vista el cursor.
    fn inicio(&self, ancho: i32, size: u32) -> usize {
        if self.todo {
            return 0;
        }
        let (mut ini, mut w) = (self.cur, 0);
        while ini > 0 {
            let a = font::adv(self.t[ini - 1], size, false);
            if w + a > ancho {
                break;
            }
            w += a;
            ini -= 1;
        }
        ini
    }

    /// Lo que cabe en `ancho` píxeles, la x del cursor dentro de ello y el ancho del texto visible.
    pub fn visible(&self, ancho: i32, size: u32) -> (String, i32, i32) {
        let ini = self.inicio(ancho, size);
        let (mut s, mut x, mut cur_x) = (String::new(), 0, 0);
        for (i, &c) in self.t.iter().enumerate().skip(ini) {
            if i == self.cur {
                cur_x = x;
            }
            x += font::adv(c, size, false);
            if x > ancho {
                x -= font::adv(c, size, false);
                break;
            }
            s.push(c);
        }
        if self.cur >= self.t.len() {
            cur_x = x;
        }
        (s, cur_x, x)
    }

    /// Pone el cursor donde se ha hecho clic (`x` relativo al inicio de lo visible).
    pub fn clic(&mut self, x: i32, ancho: i32, size: u32) {
        let ini = self.inicio(ancho, size);
        let (mut px, mut i) = (0, ini);
        while i < self.t.len() {
            let a = font::adv(self.t[i], size, false);
            if px + a / 2 >= x {
                break;
            }
            px += a;
            i += 1;
        }
        self.cur = i;
        self.todo = false;
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    fn k(key: Key, ch: Option<char>) -> KeyEvent {
        KeyEvent { key, ch, down: true, shift: false, ctrl: false, alt: false }
    }
    fn escribe(c: &mut Campo, t: &str) {
        for ch in t.chars() {
            c.tecla(&k(Key::Char(ch), Some(ch)));
        }
    }

    #[test]
    fn edicion_y_seleccion() {
        let mut c = Campo::default();
        escribe(&mut c, "hola");
        c.tecla(&k(Key::Left, None));
        c.tecla(&k(Key::Backspace, None));
        escribe(&mut c, "ñ");
        assert_eq!(c.texto(), "hoña");
        c.seleccionar_todo();
        escribe(&mut c, "x");
        assert_eq!(c.texto(), "x", "escribir sustituye lo seleccionado");
        c.seleccionar_todo();
        c.tecla(&k(Key::Backspace, None));
        assert!(c.vacio());
        assert_eq!(c.tecla(&k(Key::Enter, None)), Accion::Enter);
        assert_eq!(c.tecla(&k(Key::Backspace, None)), Accion::Nada);
        // Ctrl+letra no se escribe
        let ev = KeyEvent { ctrl: true, ..k(Key::Char('l'), None) };
        assert_eq!(c.tecla(&ev), Accion::Nada);
    }

    #[test]
    fn el_cursor_siempre_se_ve() {
        let mut c = Campo::default();
        escribe(&mut c, &"abcdefghij".repeat(30));
        let (s, x, w) = c.visible(100, 10);
        assert!(w <= 100 && x <= 100 && x > 0 && s.ends_with('j'));
        c.tecla(&k(Key::Home, None));
        let (s, x, _) = c.visible(100, 10);
        assert!(s.starts_with('a') && x == 0);
        let tres = font::width("abc", 10, false);
        c.clic(tres + 1, 100, 10);
        assert_eq!(c.visible(100, 10).1, tres);
    }
}
