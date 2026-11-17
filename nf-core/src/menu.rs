// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Menú contextual (clic derecho). CONTRATO: estos tipos no cambian.
//! Lo pide una app con `cx.menu = Some(MenuRequest { .. })` y la elección llega por
//! `App::menu_result(tag, id, cx)`. Lo dibuja el escritorio, que también lo usa para sus menús.
use alloc::string::String;
use crate::font;
use crate::geom::Rect;
use crate::input::Key;
use crate::surface::Surface;
use alloc::vec::Vec;

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct MenuItem {
    /// Texto visible. Se ignora si `separator`.
    pub text: String,
    /// Identificador que se devuelve al elegirlo (lo decide quien pide el menú).
    pub id: u32,
    /// Acción por defecto: se dibuja en negrita.
    pub bold: bool,
    /// En gris y no se puede elegir.
    pub disabled: bool,
    /// Marca a la izquierda (opción activa de "Ver" u "Ordenar por").
    pub checked: bool,
    /// Atajo que se muestra a la derecha, p. ej. "Ctrl+Z". Vacío = ninguno.
    pub shortcut: &'static str,
    /// Línea separadora.
    pub separator: bool,
    /// Si no está vacío, el elemento abre este submenú (un solo nivel) y no se elige él mismo.
    pub sub: Vec<MenuItem>,
}

impl MenuItem {
    pub fn new(id: u32, text: &str) -> MenuItem {
        MenuItem { id, text: String::from(text), ..Default::default() }
    }
    pub fn sep() -> MenuItem {
        MenuItem { separator: true, ..Default::default() }
    }
    pub fn submenu(text: &str, sub: Vec<MenuItem>) -> MenuItem {
        MenuItem { text: String::from(text), sub, ..Default::default() }
    }
    pub fn bold(mut self) -> MenuItem {
        self.bold = true;
        self
    }
    pub fn disabled(mut self, d: bool) -> MenuItem {
        self.disabled = d;
        self
    }
    pub fn checked(mut self, c: bool) -> MenuItem {
        self.checked = c;
        self
    }
    pub fn shortcut(mut self, s: &'static str) -> MenuItem {
        self.shortcut = s;
        self
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MenuRequest {
    /// Para que la app sepa a qué menú responde.
    pub tag: u32,
    /// Esquina superior izquierda deseada, en coordenadas del contenido de la ventana que lo pide
    /// (el escritorio lo recoloca para que no se salga de la pantalla).
    pub x: i32,
    pub y: i32,
    pub items: Vec<MenuItem>,
}

// ------------------------------------------------------------------ widget

const FONT: u32 = 9;
const ROW_H: i32 = 26;
const SEP_H: i32 = 9;
const PAD: i32 = 3;
const MARK_W: i32 = 28;
const ARROW_W: i32 = 24;
const GAP: i32 = 28;
const MIN_W: i32 = 150;
const SHADOW: i32 = 6;
const BG: u32 = 0xFAFAFA;
const BORDER: u32 = 0xA9A9A9;
const HOT: u32 = 0xD5E6F7;
const INK: u32 = 0x1A1A1A;
const GREY: u32 = 0x9C9C9C;
const SEP: u32 = 0xD7D7D7;

/// Resultado de atender una entrada del menú.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuAct {
    /// Sigue abierto.
    None,
    /// Se ha elegido el elemento con este `id`.
    Pick(u32),
    /// Se ha cerrado sin elegir (Esc o clic fuera).
    Close,
}

fn selectable(it: &MenuItem) -> bool {
    !it.separator && !it.disabled
}

fn row_h(it: &MenuItem) -> i32 {
    if it.separator {
        SEP_H
    } else {
        ROW_H
    }
}

/// Posición vertical de cada fila y tamaño total de una lista de elementos.
fn measure(items: &[MenuItem]) -> (Vec<i32>, i32, i32) {
    let mut ys = Vec::with_capacity(items.len());
    let mut y = PAD;
    let (mut tw, mut sw, mut arrow) = (0, 0, false);
    for it in items {
        ys.push(y);
        y += row_h(it);
        if !it.separator {
            tw = tw.max(font::width(&it.text, FONT, it.bold));
            sw = sw.max(font::width(it.shortcut, FONT, false));
            arrow |= !it.sub.is_empty();
        }
    }
    let w = MARK_W + tw + if sw > 0 { GAP + sw } else { 0 } + if arrow { GAP / 2 } else { 0 } + ARROW_W;
    (ys, w.max(MIN_W), y + PAD)
}

/// Menú contextual abierto en pantalla, con un submenú como máximo.
pub struct Menu {
    items: Vec<MenuItem>,
    ys: Vec<i32>,
    rect: Rect,
    screen: Rect,
    /// Elemento resaltado del menú principal.
    sel: Option<usize>,
    sub: Option<Sub>,
}

/// Submenú abierto; `focus` = las flechas del teclado actúan sobre él.
struct Sub {
    parent: usize,
    rect: Rect,
    ys: Vec<i32>,
    sel: Option<usize>,
    focus: bool,
}

impl Menu {
    /// `x`, `y`: esquina deseada, en pantalla; se recoloca para caber dentro de `screen`.
    pub fn new(items: Vec<MenuItem>, x: i32, y: i32, screen: Rect) -> Menu {
        let (ys, w, h) = measure(&items);
        let rx = x.min(screen.right() - w).max(screen.x);
        let ry = y.min(screen.bottom() - h).max(screen.y);
        Menu { items, ys, rect: Rect::new(rx, ry, w, h), screen, sel: None, sub: None }
    }

    /// Zona que ocupa en pantalla (sombra y submenú incluidos).
    pub fn bounds(&self) -> Rect {
        self.sub.as_ref().map_or(self.rect, |s| self.rect.union(s.rect)).inflate(SHADOW + 1)
    }

    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// Rectángulo del submenú, si está abierto.
    pub fn sub_rect(&self) -> Option<Rect> {
        self.sub.as_ref().map(|s| s.rect)
    }

    /// Elemento resaltado del menú principal.
    pub fn selected(&self) -> Option<usize> {
        self.sel
    }

    /// Elemento resaltado del submenú abierto.
    pub fn sub_selected(&self) -> Option<usize> {
        self.sub.as_ref().and_then(|s| s.sel)
    }

    /// Rectángulo del elemento `i` del menú principal.
    pub fn item_rect(&self, i: usize) -> Rect {
        Rect::new(self.rect.x + 1, self.rect.y + self.ys[i], self.rect.w - 2, row_h(&self.items[i]))
    }

    /// Rectángulo del elemento `j` del submenú abierto.
    pub fn sub_item_rect(&self, j: usize) -> Option<Rect> {
        let s = self.sub.as_ref()?;
        Some(Rect::new(s.rect.x + 1, s.rect.y + s.ys[j], s.rect.w - 2, row_h(&self.items[s.parent].sub[j])))
    }

    fn open_sub(&mut self, parent: usize, focus: bool) {
        let list = &self.items[parent].sub;
        let (ys, w, h) = measure(list);
        let mut x = self.rect.right() - 2;
        if x + w > self.screen.right() {
            x = self.rect.x - w + 2;
        }
        let y = (self.item_rect(parent).y - PAD).min(self.screen.bottom() - h).max(self.screen.y);
        let sel = if focus { list.iter().position(selectable) } else { None };
        self.sub = Some(Sub { parent, rect: Rect::new(x.max(self.screen.x), y, w, h), ys, sel, focus });
    }

    fn row_at(ys: &[i32], items: &[MenuItem], r: Rect, y: i32) -> Option<usize> {
        (0..items.len()).find(|&i| y >= r.y + ys[i] && y < r.y + ys[i] + row_h(&items[i]))
    }

    fn state(&self) -> (Option<usize>, Option<(usize, Option<usize>, bool)>) {
        (self.sel, self.sub.as_ref().map(|s| (s.parent, s.sel, s.focus)))
    }

    /// El puntero se mueve: resalta y abre o cierra el submenú. Devuelve `true` si algo cambió.
    pub fn hover(&mut self, x: i32, y: i32) -> bool {
        let before = self.state();
        if let Some(s) = self.sub.as_mut().filter(|s| s.rect.contains(x, y)) {
            let list = &self.items[s.parent].sub;
            s.sel = Self::row_at(&s.ys, list, s.rect, y).filter(|&j| selectable(&list[j]));
            s.focus = true;
        } else if self.rect.contains(x, y) {
            let i = Self::row_at(&self.ys, &self.items, self.rect, y).filter(|&i| selectable(&self.items[i]));
            self.sel = i;
            match i {
                Some(i) if !self.items[i].sub.is_empty() => {
                    if self.sub.as_ref().map(|s| s.parent) != Some(i) {
                        self.open_sub(i, false);
                    } else if let Some(s) = self.sub.as_mut() {
                        s.focus = false;
                    }
                }
                _ => self.sub = None,
            }
        }
        before != self.state()
    }

    /// Clic izquierdo en (x, y).
    pub fn click(&mut self, x: i32, y: i32) -> MenuAct {
        if let Some(s) = self.sub.as_ref().filter(|s| s.rect.contains(x, y)) {
            let list = &self.items[s.parent].sub;
            return match Self::row_at(&s.ys, list, s.rect, y).map(|j| &list[j]) {
                Some(it) if selectable(it) => MenuAct::Pick(it.id),
                _ => MenuAct::None,
            };
        }
        if !self.rect.contains(x, y) {
            return MenuAct::Close;
        }
        match Self::row_at(&self.ys, &self.items, self.rect, y) {
            Some(i) if selectable(&self.items[i]) => {
                if self.items[i].sub.is_empty() {
                    return MenuAct::Pick(self.items[i].id);
                }
                self.sel = Some(i);
                self.open_sub(i, false);
                MenuAct::None
            }
            _ => MenuAct::None,
        }
    }

    /// Siguiente elemento elegible desde `from` en el sentido `dir` (da la vuelta al llegar al extremo).
    fn step(items: &[MenuItem], from: Option<usize>, dir: i32) -> Option<usize> {
        let n = items.len() as i32;
        let mut i = from.map_or(if dir > 0 { -1 } else { n }, |i| i as i32);
        for _ in 0..n {
            i = (i + dir).rem_euclid(n);
            if selectable(&items[i as usize]) {
                return Some(i as usize);
            }
        }
        None
    }

    /// Tecla pulsada: flechas, Enter y Esc; el resto se ignora.
    pub fn key(&mut self, k: Key) -> MenuAct {
        let in_sub = self.sub.as_ref().is_some_and(|s| s.focus);
        match k {
            Key::Escape => return MenuAct::Close,
            Key::Up | Key::Down => {
                let dir = if k == Key::Down { 1 } else { -1 };
                if let Some(s) = self.sub.as_mut().filter(|s| s.focus) {
                    s.sel = Self::step(&self.items[s.parent].sub, s.sel, dir);
                } else {
                    self.sel = Self::step(&self.items, self.sel, dir);
                    self.sub = None;
                }
            }
            Key::Left if in_sub => self.sub = None,
            Key::Enter if in_sub => {
                if let Some(s) = &self.sub {
                    if let Some(it) = s.sel.map(|j| &self.items[s.parent].sub[j]) {
                        return MenuAct::Pick(it.id);
                    }
                }
            }
            Key::Right | Key::Enter if !in_sub => {
                if let Some(i) = self.sel {
                    if !self.items[i].sub.is_empty() {
                        self.open_sub(i, true);
                    } else if k == Key::Enter {
                        return MenuAct::Pick(self.items[i].id);
                    }
                }
            }
            _ => {}
        }
        MenuAct::None
    }

    pub fn draw(&self, s: &mut Surface) {
        self.draw_list(s, &self.items, &self.ys, self.rect, self.sel, self.sub.as_ref().map(|u| u.parent));
        if let Some(u) = &self.sub {
            self.draw_list(s, &self.items[u.parent].sub, &u.ys, u.rect, u.sel, None);
        }
    }

    fn draw_list(&self, s: &mut Surface, items: &[MenuItem], ys: &[i32], r: Rect, sel: Option<usize>, open: Option<usize>) {
        if !r.inflate(SHADOW + 1).intersects(s.clip()) {
            return;
        }
        // Sombra suave: anillos cada vez más tenues, algo desplazados hacia abajo.
        for i in 1..=SHADOW {
            let a = (40 * (SHADOW + 1 - i) * (SHADOW + 1 - i) / (SHADOW * SHADOW)) as u32;
            let q = Rect::new(r.x - i + 1, r.y - i + 2, r.w + 2 * i - 2, r.h + 2 * i - 2);
            s.blend_rect(Rect::new(q.x, q.y, q.w, 1), 0, a);
            s.blend_rect(Rect::new(q.x, q.bottom() - 1, q.w, 1), 0, a);
            s.blend_rect(Rect::new(q.x, q.y + 1, 1, q.h - 2), 0, a);
            s.blend_rect(Rect::new(q.right() - 1, q.y + 1, 1, q.h - 2), 0, a);
        }
        s.fill_rect(r, BG);
        s.frame(r, BORDER);
        for (i, it) in items.iter().enumerate() {
            let row = Rect::new(r.x + 1, r.y + ys[i], r.w - 2, row_h(it));
            if it.separator {
                s.fill_rect(Rect::new(row.x + MARK_W - 4, row.y + SEP_H / 2, row.w - MARK_W, 1), SEP);
                continue;
            }
            if sel == Some(i) || open == Some(i) {
                s.fill_rect(Rect::new(row.x + 2, row.y, row.w - 4, row.h), HOT);
            }
            let ink = if it.disabled { GREY } else { INK };
            if it.checked {
                let (cx, cy) = ((row.x + MARK_W / 2) as f32, (row.y + row.h / 2) as f32);
                s.stroke(&[(cx - 4.0, cy), (cx - 1.0, cy + 3.5), (cx + 4.5, cy - 4.0)], 1.5, ink);
            }
            let tr = Rect::new(row.x + MARK_W, row.y, row.w - MARK_W - ARROW_W, row.h);
            font::draw_in(s, tr, &it.text, FONT, it.bold, ink, font::Align::Left);
            if !it.shortcut.is_empty() {
                let sr = Rect::new(row.x, row.y, row.w - ARROW_W, row.h);
                font::draw_in(s, sr, it.shortcut, FONT, false, if it.disabled { GREY } else { 0x666666 }, font::Align::Right);
            }
            if !it.sub.is_empty() {
                let (ax, ay) = ((row.right() - 14) as f32, (row.y + row.h / 2) as f32);
                s.stroke(&[(ax - 2.0, ay - 4.0), (ax + 2.0, ay), (ax - 2.0, ay + 4.0)], 1.3, ink);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn sample() -> Vec<MenuItem> {
        vec![
            MenuItem::submenu("Ver", vec![MenuItem::new(10, "Grandes").checked(true), MenuItem::new(11, "Pequeños")]),
            MenuItem::new(1, "Actualizar"),
            MenuItem::sep(),
            MenuItem::new(2, "Pegar").disabled(true),
            MenuItem::new(3, "Deshacer").shortcut("Ctrl+Z"),
        ]
    }

    fn screen() -> Rect {
        Rect::new(0, 0, 800, 560)
    }

    #[test]
    fn tamano_y_recolocacion() {
        let m = Menu::new(sample(), 100, 100, screen());
        assert!(m.rect().w >= MIN_W && m.rect().h > 4 * ROW_H);
        assert_eq!((m.rect().x, m.rect().y), (100, 100));
        let m = Menu::new(sample(), 790, 555, screen());
        assert_eq!(m.rect().right(), 800);
        assert_eq!(m.rect().bottom(), 560);
        let m = Menu::new(sample(), -50, -50, screen());
        assert_eq!((m.rect().x, m.rect().y), (0, 0));
        let largo = Menu::new(vec![MenuItem::new(1, "Un texto bastante largo para el menú").shortcut("Ctrl+Mayús+Z")], 0, 0, screen());
        assert!(largo.rect().w > MIN_W);
    }

    #[test]
    fn teclado_salta_separadores_y_desactivados() {
        let mut m = Menu::new(sample(), 10, 10, screen());
        assert_eq!(m.key(Key::Down), MenuAct::None);
        assert_eq!(m.selected(), Some(0));
        m.key(Key::Down);
        assert_eq!(m.selected(), Some(1));
        m.key(Key::Down);
        assert_eq!(m.selected(), Some(4)); // salta separador y "Pegar"
        m.key(Key::Down);
        assert_eq!(m.selected(), Some(0)); // da la vuelta
        m.key(Key::Up);
        assert_eq!(m.selected(), Some(4));
        assert_eq!(m.key(Key::Enter), MenuAct::Pick(3));
        assert_eq!(m.key(Key::Escape), MenuAct::Close);
    }

    #[test]
    fn teclado_submenu() {
        let mut m = Menu::new(sample(), 10, 10, screen());
        m.key(Key::Down);
        assert_eq!(m.key(Key::Right), MenuAct::None);
        assert!(m.sub_rect().is_some());
        assert_eq!(m.sub_selected(), Some(0));
        m.key(Key::Down);
        assert_eq!(m.sub_selected(), Some(1));
        assert_eq!(m.key(Key::Enter), MenuAct::Pick(11));
        assert_eq!(m.key(Key::Left), MenuAct::None);
        assert!(m.sub_rect().is_none());
        assert_eq!(m.selected(), Some(0));
        // Enter sobre un elemento con submenú lo abre, no lo elige.
        assert_eq!(m.key(Key::Enter), MenuAct::None);
        assert!(m.sub_rect().is_some());
        assert_eq!(m.key(Key::Escape), MenuAct::Close);
    }

    #[test]
    fn raton() {
        let mut m = Menu::new(sample(), 10, 10, screen());
        let r1 = m.item_rect(1);
        assert!(m.hover(r1.x + 5, r1.y + 5));
        assert_eq!(m.selected(), Some(1));
        assert!(!m.hover(r1.x + 6, r1.y + 6));
        // El submenú se abre al pasar, a la derecha.
        let r0 = m.item_rect(0);
        m.hover(r0.x + 5, r0.y + 5);
        assert!(m.sub_rect().unwrap().x >= m.rect().right() - 2);
        let s0 = m.sub_item_rect(0).unwrap();
        m.hover(s0.x + 5, s0.y + 5);
        assert_eq!(m.sub_selected(), Some(0));
        assert_eq!(m.click(s0.x + 5, s0.y + 5), MenuAct::Pick(10));
        // Volver a otro elemento lo cierra.
        m.hover(r1.x + 5, r1.y + 5);
        assert!(m.sub_rect().is_none());
        // Separador y desactivado: ni se eligen ni se resaltan.
        let sep = m.item_rect(2);
        assert_eq!(m.click(sep.x + 5, sep.y + 2), MenuAct::None);
        let dis = m.item_rect(3);
        assert_eq!(m.click(dis.x + 5, dis.y + 5), MenuAct::None);
        m.hover(dis.x + 5, dis.y + 5);
        assert_eq!(m.selected(), None);
        assert_eq!(m.click(r1.x + 5, r1.y + 5), MenuAct::Pick(1));
        assert_eq!(m.click(700, 500), MenuAct::Close);
        // Clic en el padre abre el submenú sin elegir.
        assert_eq!(m.click(r0.x + 5, r0.y + 5), MenuAct::None);
        assert!(m.sub_rect().is_some());
    }

    #[test]
    fn submenu_a_la_izquierda_si_no_cabe() {
        let mut m = Menu::new(sample(), 700, 10, screen());
        m.key(Key::Down);
        m.key(Key::Right);
        let sr = m.sub_rect().unwrap();
        assert!(sr.right() <= 800 && sr.x < m.rect().x);
    }

    #[test]
    fn dibuja_dentro_de_sus_limites() {
        let mut m = Menu::new(sample(), 50, 50, screen());
        m.key(Key::Down);
        m.key(Key::Right);
        let mut s = Surface::new(800, 560, 0x123456);
        m.draw(&mut s);
        let b = m.bounds();
        for y in 0..560 {
            for x in 0..800 {
                if !b.contains(x, y) {
                    assert_eq!(s.px[(y * 800 + x) as usize], 0x123456, "fuera en {x},{y}");
                }
            }
        }
        assert_ne!(s.px[(60 * 800 + 60) as usize], 0x123456);
    }
}
