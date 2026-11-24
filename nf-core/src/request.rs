// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Peticiones de un programa al escritorio (v0.10, `SYS_REQUEST`) y los tipos que viajan con ellas
//! y con los eventos con datos (`DialogResult`). Codificación con `wire`; ver `abi` para las
//! llamadas y eventos. Lo que llega de un programa no se fía: un mensaje roto es `Bad`.
use crate::dialogs::{Answer, Buttons, DialogRequest, DialogResult};
use crate::menu::{MenuItem, MenuRequest};
use crate::theme::Theme;
use crate::wire::{Bad, Reader, Wire, WireResult, Writer};
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Request {
    /// `Ctx::open`.
    Open(String),
    /// `Ctx::open_new`.
    OpenNew(String),
    /// `Ctx::open_terminal`.
    OpenTerminal(String),
    Dialog(DialogRequest),
    Menu(MenuRequest),
    /// `Ctx::drag`: rutas que empieza a arrastrar.
    Drag(Vec<String>),
    SetTheme(Theme),
    /// Respuesta a `EV_DRAG_OVER`: ¿soltar ahí haría algo?
    DragAnswer(bool),
    /// Respuesta a `EV_DRAG_SOURCE`.
    DragSourceAnswer(Option<Vec<String>>),
}

/// Atajos de teclado que enseñan los menús de las apps. `MenuItem::shortcut` es `&'static str`:
/// viaja como texto y al leerlo se busca aquí; uno desconocido queda vacío.
const ATAJOS: [&str; 2] = ["Ctrl+Z", "Ctrl+Mayús+Z"];

impl Wire for Request {
    fn put(&self, w: &mut Writer) {
        match self {
            Request::Open(p) => w.put(&0u8).put(p),
            Request::OpenNew(p) => w.put(&1u8).put(p),
            Request::OpenTerminal(p) => w.put(&2u8).put(p),
            Request::Dialog(d) => w.put(&3u8).put(d),
            Request::Menu(m) => w.put(&4u8).put(m),
            Request::Drag(v) => w.put(&5u8).put(v),
            Request::SetTheme(t) => w.put(&6u8).put(t),
            Request::DragAnswer(b) => w.put(&7u8).put(b),
            Request::DragSourceAnswer(v) => w.put(&8u8).put(v),
        };
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(match r.get::<u8>()? {
            0 => Request::Open(r.get()?),
            1 => Request::OpenNew(r.get()?),
            2 => Request::OpenTerminal(r.get()?),
            3 => Request::Dialog(r.get()?),
            4 => Request::Menu(r.get()?),
            5 => Request::Drag(r.get()?),
            6 => Request::SetTheme(r.get()?),
            7 => Request::DragAnswer(r.get()?),
            8 => Request::DragSourceAnswer(r.get()?),
            _ => return Err(Bad),
        })
    }
}

impl Wire for Buttons {
    fn put(&self, w: &mut Writer) {
        w.put(&(*self as u8));
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(match r.get::<u8>()? {
            0 => Buttons::Ok,
            1 => Buttons::OkCancel,
            2 => Buttons::YesNo,
            3 => Buttons::YesNoCancel,
            4 => Buttons::FormatCancel,
            5 => Buttons::EncryptCancel,
            _ => return Err(Bad),
        })
    }
}

impl Wire for Answer {
    fn put(&self, w: &mut Writer) {
        w.put(&(*self as u8));
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(match r.get::<u8>()? {
            0 => Answer::Ok,
            1 => Answer::Yes,
            2 => Answer::No,
            3 => Answer::Cancel,
            _ => return Err(Bad),
        })
    }
}

impl Wire for DialogRequest {
    fn put(&self, w: &mut Writer) {
        match self {
            DialogRequest::Message { tag, title, text, buttons } => w.put(&0u8).put(tag).put(title).put(text).put(buttons),
            DialogRequest::Input { tag, title, label, value } => w.put(&1u8).put(tag).put(title).put(label).put(value),
            DialogRequest::Password { tag, title, label } => w.put(&2u8).put(tag).put(title).put(label),
            DialogRequest::Open { tag, title, dir } => w.put(&3u8).put(tag).put(title).put(dir),
            DialogRequest::Save { tag, title, dir, name } => w.put(&4u8).put(tag).put(title).put(dir).put(name),
            DialogRequest::Folder { tag, title, dir } => w.put(&5u8).put(tag).put(title).put(dir),
            DialogRequest::Entropy { tag, title } => w.put(&6u8).put(tag).put(title),
            // Solo del sistema: nunca viaja por el protocolo y `get` lo rechaza.
            DialogRequest::Unlock { tag, .. } => w.put(&7u8).put(tag),
        };
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(match r.get::<u8>()? {
            0 => DialogRequest::Message { tag: r.get()?, title: r.get()?, text: r.get()?, buttons: r.get()? },
            1 => DialogRequest::Input { tag: r.get()?, title: r.get()?, label: r.get()?, value: r.get()? },
            2 => DialogRequest::Password { tag: r.get()?, title: r.get()?, label: r.get()? },
            3 => DialogRequest::Open { tag: r.get()?, title: r.get()?, dir: r.get()? },
            4 => DialogRequest::Save { tag: r.get()?, title: r.get()?, dir: r.get()?, name: r.get()? },
            5 => DialogRequest::Folder { tag: r.get()?, title: r.get()?, dir: r.get()? },
            6 => DialogRequest::Entropy { tag: r.get()?, title: r.get()? },
            _ => return Err(Bad),
        })
    }
}

impl Wire for DialogResult {
    fn put(&self, w: &mut Writer) {
        match self {
            DialogResult::Answer { tag, answer } => w.put(&0u8).put(tag).put(answer),
            DialogResult::Text { tag, value } => w.put(&1u8).put(tag).put(value),
            DialogResult::Path { tag, path } => w.put(&2u8).put(tag).put(path),
        };
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(match r.get::<u8>()? {
            0 => DialogResult::Answer { tag: r.get()?, answer: r.get()? },
            1 => DialogResult::Text { tag: r.get()?, value: r.get()? },
            2 => DialogResult::Path { tag: r.get()?, path: r.get()? },
            _ => return Err(Bad),
        })
    }
}

impl Wire for MenuRequest {
    fn put(&self, w: &mut Writer) {
        w.put(&self.tag).put(&self.x).put(&self.y).put(&self.items);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(MenuRequest { tag: r.get()?, x: r.get()?, y: r.get()?, items: r.get()? })
    }
}

impl Wire for MenuItem {
    fn put(&self, w: &mut Writer) {
        let flags = self.bold as u8 | (self.disabled as u8) << 1 | (self.checked as u8) << 2 | (self.separator as u8) << 3;
        w.put(&self.text).put(&self.id).put(&flags).put(&String::from(self.shortcut)).put(&self.sub);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        item(r, false)
    }
}

/// Lee un elemento. Los menús tienen un solo nivel de submenú: así un mensaje no puede
/// anidar elementos hasta agotar la pila.
fn item(r: &mut Reader, in_sub: bool) -> WireResult<MenuItem> {
    let text = r.get()?;
    let id = r.get()?;
    let flags: u8 = r.get()?;
    if flags > 15 {
        return Err(Bad);
    }
    let shortcut = r.get::<String>()?;
    let n = r.get::<u32>()? as usize;
    if n > r.remaining() || (in_sub && n > 0) {
        return Err(Bad);
    }
    let mut sub = Vec::with_capacity(n);
    for _ in 0..n {
        sub.push(item(r, true)?);
    }
    Ok(MenuItem {
        text,
        id,
        bold: flags & 1 != 0,
        disabled: flags & 2 != 0,
        checked: flags & 4 != 0,
        shortcut: ATAJOS.iter().find(|&&a| a == shortcut).copied().unwrap_or(""),
        separator: flags & 8 != 0,
        sub,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{decode, encode};
    use alloc::vec;

    fn s(t: &str) -> String {
        String::from(t)
    }

    fn ida_vuelta(r: Request) {
        assert_eq!(decode::<Request>(&encode(&r)), Ok(r));
    }

    #[test]
    fn peticiones_ida_y_vuelta() {
        let menu = MenuRequest {
            tag: 7,
            x: -3,
            y: 40,
            items: vec![
                MenuItem::new(1, "Deshacer").shortcut("Ctrl+Z").bold(),
                MenuItem::sep(),
                MenuItem::submenu("Ver", vec![MenuItem::new(2, "Lista").checked(true), MenuItem::new(3, "Iconos").disabled(true)]),
            ],
        };
        for r in [
            Request::Open(s("C:\\Usuarios\\ñu")),
            Request::OpenNew(s("C:\\")),
            Request::OpenTerminal(s("C:\\Sistema")),
            Request::Dialog(DialogRequest::Message { tag: 1, title: s("t"), text: s("x\ny"), buttons: Buttons::YesNoCancel }),
            Request::Dialog(DialogRequest::Input { tag: 2, title: s("t"), label: s("l"), value: s("v") }),
            Request::Dialog(DialogRequest::Password { tag: 3, title: s("t"), label: s("l") }),
            Request::Dialog(DialogRequest::Open { tag: 4, title: s("t"), dir: s("C:\\") }),
            Request::Dialog(DialogRequest::Save { tag: 5, title: s("t"), dir: s("C:\\"), name: s("a.txt") }),
            Request::Dialog(DialogRequest::Folder { tag: 6, title: s("t"), dir: s("C:\\") }),
            Request::Dialog(DialogRequest::Entropy { tag: 7, title: s("t") }),
            Request::Menu(menu),
            Request::Drag(vec![s("C:\\a"), s("C:\\b")]),
            Request::SetTheme(Theme { accent: 0x112233, wallpaper: 2, letra: 1 }),
            Request::DragAnswer(true),
            Request::DragSourceAnswer(None),
            Request::DragSourceAnswer(Some(vec![s("C:\\a")])),
        ] {
            ida_vuelta(r);
        }
        for r in [
            DialogResult::Answer { tag: 9, answer: Answer::Cancel },
            DialogResult::Text { tag: 1, value: Some(s("hola")) },
            DialogResult::Text { tag: 1, value: None },
            DialogResult::Path { tag: u32::MAX, path: Some(s("C:\\x")) },
        ] {
            assert_eq!(decode::<DialogResult>(&encode(&r)), Ok(r));
        }
    }

    #[test]
    fn atajo_desconocido_queda_vacio() {
        let ok = MenuItem::new(1, "a").shortcut("Ctrl+Mayús+Z");
        assert_eq!(decode::<MenuItem>(&encode(&ok)).unwrap().shortcut, "Ctrl+Mayús+Z");
        let mut w = Writer::new();
        w.put(&s("a")).put(&1u32).put(&0u8).put(&s("Ctrl+Q")).put(&0u32);
        assert_eq!(decode::<MenuItem>(&w.finish()).unwrap().shortcut, "");
    }

    #[test]
    fn mensajes_rotos() {
        assert_eq!(decode::<Request>(&[]), Err(Bad));
        assert_eq!(decode::<Request>(&[99]), Err(Bad), "tipo desconocido");
        assert_eq!(decode::<Request>(&[7, 2]), Err(Bad), "bool inválido");
        assert_eq!(decode::<Request>(&[7, 1, 0]), Err(Bad), "sobra");
        assert_eq!(decode::<Request>(&[3, 0, 1, 0, 0, 0]), Err(Bad), "corto");
        let mut m = encode(&Request::Dialog(DialogRequest::Message { tag: 1, title: s(""), text: s(""), buttons: Buttons::Ok }));
        *m.last_mut().unwrap() = 9;
        assert_eq!(decode::<Request>(&m), Err(Bad), "botones inexistentes");
        // Longitudes que mienten: sin reservar de más.
        let mut big = vec![5u8];
        big.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode::<Request>(&big), Err(Bad));
        // Un submenú dentro de un submenú no se admite.
        let hondo = MenuItem::submenu("a", vec![MenuItem::submenu("b", vec![MenuItem::new(1, "c")])]);
        assert_eq!(decode::<MenuItem>(&encode(&hondo)), Err(Bad));
        let mut w = Writer::new();
        w.put(&s("a")).put(&1u32).put(&16u8).put(&s("")).put(&0u32);
        assert_eq!(decode::<MenuItem>(&w.finish()), Err(Bad), "banderas inexistentes");
    }
}
