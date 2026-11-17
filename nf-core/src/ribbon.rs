// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Cinta con pestañas del Explorador (v0.4.0). Ver docs/PLAN-cinta.md.
//! Widget reutilizable: fila de pestañas (con botón de archivo opcional) y, debajo, la cinta de la
//! pestaña activa con grupos de botones. No guarda nada del Explorador: solo emite `RibbonAct`.
//! Coordenadas relativas a la esquina (0,0) de la cinta; `w` es el ancho disponible.
//! Pulsar y soltar: `Pick(id)` y `File` salen al SOLTAR sobre el mismo botón; cambiar de pestaña, al pulsar.
//! Las casillas (`checkbox`) no se marcan solas: quien recibe `Pick` llama a `set_checked`.
use crate::apps::MouseEv;
use crate::font::{self, Align};
use crate::geom::Rect;
use crate::icons::{self, Icon};
use crate::surface::{blend, Surface};
use crate::theme::Theme;
use alloc::vec::Vec;

/// Alto de la fila de pestañas.
const TAB_H: i32 = 24;
/// Alto de la cinta propiamente dicha (botones + título de grupo).
const BODY_H: i32 = 92;
const TITLE_H: i32 = 16;
const BTN_TOP: i32 = TAB_H + 3;
/// Alto de un botón pequeño (tres caben en la zona de botones).
const SMALL_H: i32 = 24;
const LARGE_H: i32 = 72;
const FS: u32 = 9;

const BODY_BG: u32 = 0xF5F6F7;
const TAB_BG: u32 = 0xFFFFFF;
const LINE: u32 = 0xC8C8C8;
const SEP: u32 = 0xDADADA;
const TEXT: u32 = 0x1A1A1A;
const GRAY: u32 = 0x6A6A6A;
const DISABLED: u32 = 0xA6A6A6;
const HOVER_BG: u32 = 0xE5F1FB;
const HOVER_BORDER: u32 = 0xA9CFF0;
const PRESS_BG: u32 = 0xCCE4F7;
const PRESS_BORDER: u32 = 0x7DB7E8;

#[derive(Clone, Debug)]
pub struct RibbonItem {
    pub id: u32,
    pub text: &'static str,
    pub icon: Icon,
    /// Botón grande (icono de 32 px arriba, texto debajo).
    pub large: bool,
    pub disabled: bool,
    /// Resaltado como opción activa (p. ej. la vista actual).
    pub checked: bool,
    /// Lleva una flecha ▾ (abre un desplegable que gestiona quien usa la cinta).
    pub dropdown: bool,
    /// Se dibuja una casilla (con ✓ si `checked`) en lugar del icono.
    pub checkbox: bool,
}

impl RibbonItem {
    pub fn new(id: u32, text: &'static str, icon: Icon) -> RibbonItem {
        RibbonItem { id, text, icon, large: false, disabled: false, checked: false, dropdown: false, checkbox: false }
    }
    pub fn large(mut self) -> RibbonItem {
        self.large = true;
        self
    }
    pub fn dropdown(mut self) -> RibbonItem {
        self.dropdown = true;
        self
    }
    pub fn checkbox(mut self) -> RibbonItem {
        self.checkbox = true;
        self
    }
}

#[derive(Clone, Debug)]
pub struct Group {
    pub title: &'static str,
    pub items: Vec<RibbonItem>,
}

#[derive(Clone, Debug)]
pub struct Tab {
    pub title: &'static str,
    pub groups: Vec<Group>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RibbonAct {
    None,
    /// Hay que volver a dibujar la cinta.
    Redraw,
    /// Se ha pulsado el botón con ese `id`.
    Pick(u32),
    /// Se ha pulsado el botón de archivo.
    File,
}

/// Qué hay bajo el puntero. `Item(grupo, elemento)` indexa la pestaña activa.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hit {
    None,
    File,
    Tab(usize),
    Item(usize, usize),
}

/// Posiciones calculadas de la pestaña activa para un ancho dado (sin recortar).
struct Layout {
    file: Rect,
    tabs: Vec<Rect>,
    /// (índice de grupo, rectángulo completo del grupo)
    groups: Vec<(usize, Rect)>,
    /// (grupo, elemento, rectángulo)
    items: Vec<(usize, usize, Rect)>,
}

pub struct Ribbon {
    file_label: Option<&'static str>,
    tabs: Vec<Tab>,
    active: usize,
    hover: Hit,
    down: Hit,
}

/// Texto de un botón grande en una o dos líneas (parte por el espacio más cercano al centro).
fn split_lines(text: &str) -> (&str, Option<&str>) {
    let mid = text.len() / 2;
    let cut = text.char_indices().filter(|(_, c)| *c == ' ').map(|(i, _)| i).min_by_key(|i| i.abs_diff(mid));
    match cut {
        Some(i) => (&text[..i], Some(&text[i + 1..])),
        None => (text, None),
    }
}

const ARROW_W: i32 = 9;

fn large_w(it: &RibbonItem) -> i32 {
    let (a, b) = split_lines(it.text);
    let wa = font::width(a, FS, false);
    let wb = b.map_or(0, |b| font::width(b, FS, false));
    let arrow = if it.dropdown { ARROW_W } else { 0 };
    let texto = if b.is_some() { wa.max(wb + arrow) } else { wa + arrow };
    (texto + 12).max(46)
}

fn small_w(it: &RibbonItem) -> i32 {
    4 + 16 + 4 + font::width(it.text, FS, false) + if it.dropdown { ARROW_W + 2 } else { 0 } + 6
}

impl Ribbon {
    pub fn new(file_label: Option<&'static str>, tabs: Vec<Tab>) -> Ribbon {
        Ribbon { file_label, tabs, active: 0, hover: Hit::None, down: Hit::None }
    }

    /// Alto total (fila de pestañas + cinta). Constante.
    pub fn height(&self) -> i32 {
        TAB_H + BODY_H
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn set_active(&mut self, i: usize) {
        if i < self.tabs.len() && i != self.active {
            self.active = i;
            self.hover = Hit::None;
            self.down = Hit::None;
        }
    }

    fn find_mut(&mut self, id: u32) -> Vec<&mut RibbonItem> {
        let mut v = Vec::new();
        for t in self.tabs.iter_mut() {
            for g in t.groups.iter_mut() {
                for it in g.items.iter_mut() {
                    if it.id == id {
                        v.push(it);
                    }
                }
            }
        }
        v
    }

    /// Activa o desactiva el botón `id` (en cualquier pestaña). Devuelve `true` si cambió algo.
    pub fn set_disabled(&mut self, id: u32, b: bool) -> bool {
        let mut cambio = false;
        for it in self.find_mut(id) {
            cambio |= it.disabled != b;
            it.disabled = b;
        }
        cambio
    }

    /// Marca o desmarca el botón `id`. Devuelve `true` si cambió algo.
    pub fn set_checked(&mut self, id: u32, b: bool) -> bool {
        let mut cambio = false;
        for it in self.find_mut(id) {
            cambio |= it.checked != b;
            it.checked = b;
        }
        cambio
    }

    fn layout(&self) -> Layout {
        let mut x = 0;
        let file = match self.file_label {
            Some(t) => {
                x = font::width(t, FS, true) + 28;
                Rect::new(0, 0, x, TAB_H)
            }
            None => Rect::default(),
        };
        x += 4;
        let mut tabs = Vec::new();
        for t in &self.tabs {
            let w = font::width(t.title, FS, false) + 24;
            tabs.push(Rect::new(x, 0, w, TAB_H));
            x += w;
        }
        let (mut groups, mut items) = (Vec::new(), Vec::new());
        let mut x = 2;
        if let Some(tab) = self.tabs.get(self.active) {
            for (gi, g) in tab.groups.iter().enumerate() {
                let x0 = x;
                x += 4;
                let mut ii = 0;
                while ii < g.items.len() {
                    if g.items[ii].large {
                        let w = large_w(&g.items[ii]);
                        items.push((gi, ii, Rect::new(x, BTN_TOP, w, LARGE_H)));
                        x += w;
                        ii += 1;
                        continue;
                    }
                    // Racha de botones pequeños: columnas de hasta tres, repartidas por igual.
                    let fin = (ii..g.items.len()).find(|&j| g.items[j].large).unwrap_or(g.items.len());
                    let n = fin - ii;
                    let cols = (n + 2) / 3;
                    let por_col = (n + cols - 1) / cols;
                    for c in 0..cols {
                        let a = ii + c * por_col;
                        let b = (a + por_col).min(fin);
                        let w = (a..b).map(|j| small_w(&g.items[j])).max().unwrap_or(0);
                        let y0 = BTN_TOP + (LARGE_H - (b - a) as i32 * SMALL_H) / 2;
                        for j in a..b {
                            items.push((gi, j, Rect::new(x, y0 + (j - a) as i32 * SMALL_H, w, SMALL_H)));
                        }
                        x += w;
                    }
                    ii = fin;
                }
                x += 4;
                let wt = font::width(g.title, FS, false) + 12;
                let ancho = (x - x0).max(wt);
                x = x0 + ancho;
                groups.push((gi, Rect::new(x0, TAB_H, ancho, BODY_H)));
            }
        }
        Layout { file, tabs, groups, items }
    }

    /// Rectángulo del botón `id` en la pestaña activa, para anclar un desplegable. `None` si no está
    /// en la pestaña activa o queda fuera del ancho `w`.
    pub fn item_rect(&self, id: u32, w: i32) -> Option<Rect> {
        let tab = self.tabs.get(self.active)?;
        self.layout().items.iter().find(|(g, i, _)| tab.groups[*g].items[*i].id == id).map(|t| t.2).filter(|r| r.x < w)
    }

    /// Ancho que necesita la pestaña activa para verse entera (con menos, el final se recorta).
    pub fn content_width(&self) -> i32 {
        self.layout().groups.last().map_or(0, |g| g.1.right())
    }

    /// Rectángulo del botón de archivo (vacío si no hay).
    pub fn file_rect(&self) -> Rect {
        self.layout().file
    }

    fn hit(&self, lay: &Layout, x: i32, y: i32, w: i32) -> Hit {
        if x < 0 || x >= w {
            return Hit::None;
        }
        if lay.file.contains(x, y) {
            return Hit::File;
        }
        if let Some(i) = lay.tabs.iter().position(|r| r.contains(x, y)) {
            return Hit::Tab(i);
        }
        if let Some(tab) = self.tabs.get(self.active) {
            for (g, i, r) in &lay.items {
                if r.contains(x, y) && !tab.groups[*g].items[*i].disabled {
                    return Hit::Item(*g, *i);
                }
            }
        }
        Hit::None
    }

    pub fn mouse(&mut self, ev: MouseEv, w: i32) -> RibbonAct {
        match ev {
            MouseEv::Move { x, y } => {
                let h = self.hit(&self.layout(), x, y, w);
                if h != self.hover {
                    self.hover = h;
                    RibbonAct::Redraw
                } else {
                    RibbonAct::None
                }
            }
            MouseEv::Down { x, y } => {
                let h = self.hit(&self.layout(), x, y, w);
                self.hover = h;
                match h {
                    Hit::Tab(i) => {
                        self.down = Hit::None;
                        if i == self.active {
                            RibbonAct::None
                        } else {
                            self.set_active(i);
                            self.hover = Hit::Tab(i);
                            RibbonAct::Redraw
                        }
                    }
                    Hit::None => {
                        self.down = Hit::None;
                        RibbonAct::None
                    }
                    _ => {
                        self.down = h;
                        RibbonAct::Redraw
                    }
                }
            }
            MouseEv::Up { x, y } => {
                let d = core::mem::replace(&mut self.down, Hit::None);
                if d == Hit::None {
                    return RibbonAct::None;
                }
                let h = self.hit(&self.layout(), x, y, w);
                self.hover = h;
                match (d, h) {
                    (Hit::File, Hit::File) => RibbonAct::File,
                    (Hit::Item(g, i), Hit::Item(g2, i2)) if g == g2 && i == i2 => {
                        RibbonAct::Pick(self.tabs[self.active].groups[g].items[i].id)
                    }
                    _ => RibbonAct::Redraw,
                }
            }
            MouseEv::Leave => {
                if self.hover != Hit::None {
                    self.hover = Hit::None;
                    RibbonAct::Redraw
                } else {
                    RibbonAct::None
                }
            }
            MouseEv::RightDown { .. } | MouseEv::Wheel { .. } => RibbonAct::None,
        }
    }

    /// Dibuja la cinta en (0,0)-(w, height). Lo que no cabe en `w` queda recortado.
    pub fn draw(&self, s: &mut Surface, w: i32, theme: &Theme) {
        let old = s.clip();
        let area = Rect::new(0, 0, w, self.height());
        s.set_clip(old.intersect(area));
        let lay = self.layout();
        s.fill_rect(Rect::new(0, 0, w, TAB_H), TAB_BG);
        s.fill_rect(Rect::new(0, TAB_H, w, BODY_H), BODY_BG);
        s.fill_rect(Rect::new(0, self.height() - 1, w, 1), LINE);
        s.fill_rect(Rect::new(0, TAB_H, w, 1), LINE);
        // Pestañas.
        for (i, r) in lay.tabs.iter().enumerate() {
            let t = &self.tabs[i];
            if i == self.active {
                s.fill_rect(Rect::new(r.x, 2, r.w, TAB_H - 1), BODY_BG);
                s.fill_rect(Rect::new(r.x, 2, r.w, 1), LINE);
                s.fill_rect(Rect::new(r.x, 2, 1, TAB_H - 2), LINE);
                s.fill_rect(Rect::new(r.right() - 1, 2, 1, TAB_H - 2), LINE);
                font::draw_in(s, Rect::new(r.x, 2, r.w, TAB_H - 2), t.title, FS, false, TEXT, Align::Center);
            } else {
                if self.hover == Hit::Tab(i) {
                    s.fill_rect(Rect::new(r.x, 4, r.w, TAB_H - 4), HOVER_BG);
                }
                font::draw_in(s, Rect::new(r.x, 2, r.w, TAB_H - 2), t.title, FS, false, GRAY, Align::Center);
            }
        }
        // Botón de archivo.
        if let Some(t) = self.file_label {
            let r = lay.file;
            let base = if self.down == Hit::File && self.hover == Hit::File {
                blend(theme.accent, 0, 70)
            } else if self.hover == Hit::File {
                blend(theme.accent, 0xFFFFFF, 40)
            } else {
                theme.accent
            };
            s.fill_rect(r, base);
            font::draw_in(s, r, t, FS, true, 0xFFFFFF, Align::Center);
        }
        // Grupos.
        if let Some(tab) = self.tabs.get(self.active) {
            for (gi, r) in &lay.groups {
                let g = &tab.groups[*gi];
                s.fill_rect(Rect::new(r.right() - 1, TAB_H + 6, 1, BODY_H - 12), SEP);
                let tr = Rect::new(r.x, TAB_H + BODY_H - TITLE_H - 1, r.w - 1, TITLE_H);
                font::draw_in(s, tr, g.title, FS, false, GRAY, Align::Center);
            }
            for (g, i, r) in &lay.items {
                self.draw_item(s, &tab.groups[*g].items[*i], *r, Hit::Item(*g, *i), theme);
            }
        }
        s.set_clip(old);
    }

    fn draw_item(&self, s: &mut Surface, it: &RibbonItem, r: Rect, me: Hit, theme: &Theme) {
        let pressed = !it.disabled && self.down == me && self.hover == me;
        let hover = !it.disabled && self.hover == me && (self.down == Hit::None || self.down == me);
        let (bg, border) = if pressed {
            (PRESS_BG, Some(PRESS_BORDER))
        } else if hover {
            (HOVER_BG, Some(HOVER_BORDER))
        } else if it.checked && !it.checkbox {
            (blend(BODY_BG, theme.accent, 55), Some(blend(BODY_BG, theme.accent, 140)))
        } else {
            (BODY_BG, None)
        };
        if let Some(b) = border {
            s.fill_rect(r, bg);
            s.frame(r, b);
        }
        let col = if it.disabled { DISABLED } else { TEXT };
        if it.large {
            let ix = r.x + (r.w - 32) / 2;
            self.draw_icon(s, it, ix, r.y + 4, 32, bg);
            let (a, b) = split_lines(it.text);
            let lh = font::line_h(FS) - 1;
            let y0 = r.y + 4 + 32 + 2;
            for (n, linea) in [Some(a), b].into_iter().flatten().enumerate() {
                let lw = font::width(linea, FS, false);
                let arrow = if it.dropdown && (b.is_none() || n == 1) { ARROW_W } else { 0 };
                let x = r.x + (r.w - lw - arrow) / 2;
                font::draw(s, x, y0 + n as i32 * lh, linea, FS, false, col);
                if arrow > 0 {
                    arrow_down(s, x + lw + 4, y0 + n as i32 * lh + lh / 2 - 1, col);
                }
            }
        } else {
            let cy = r.y + (r.h - 16) / 2;
            if it.checkbox {
                let bx = Rect::new(r.x + 5, r.y + (r.h - 13) / 2, 13, 13);
                s.fill_rect(bx, if it.disabled { BODY_BG } else { 0xFFFFFF });
                s.frame(bx, if it.disabled { DISABLED } else { GRAY });
                if it.checked {
                    let c = if it.disabled { DISABLED } else { theme.accent };
                    let p = |u: f32, v: f32| (bx.x as f32 + u, bx.y as f32 + v);
                    s.stroke(&[p(3.2, 6.8), p(5.6, 9.4), p(10.0, 3.8)], 1.6, c);
                }
            } else {
                self.draw_icon(s, it, r.x + 4, cy, 16, bg);
            }
            let tx = r.x + 4 + 16 + 4;
            let end = font::draw(s, tx, r.y + (r.h - font::line_h(FS)) / 2, it.text, FS, false, col);
            if it.dropdown {
                arrow_down(s, end + 5, r.y + r.h / 2 - 1, col);
            }
        }
    }

    fn draw_icon(&self, s: &mut Surface, it: &RibbonItem, x: i32, y: i32, size: i32, bg: u32) {
        icons::draw(s, it.icon, x, y, size, 0x444444);
        if it.disabled {
            // Desvanece el icono hacia el fondo para que se lea como no disponible.
            s.blend_rect(Rect::new(x, y, size, size), bg, 170);
        }
    }
}

/// Flechita ▾ de 7 px de ancho con su centro en x + 3.
fn arrow_down(s: &mut Surface, x: i32, y: i32, c: u32) {
    for i in 0..4 {
        s.fill_rect(Rect::new(x + i, y + i, 7 - 2 * i, 1), c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    pub fn demo() -> Ribbon {
        let inicio = Tab {
            title: "Inicio",
            groups: vec![
                Group {
                    title: "Portapapeles",
                    items: vec![
                        RibbonItem::new(1, "Copiar", Icon::Copiar).large(),
                        RibbonItem::new(2, "Pegar", Icon::Pegar).large(),
                        RibbonItem::new(3, "Cortar", Icon::Cortar),
                    ],
                },
                Group {
                    title: "Organizar",
                    items: vec![
                        RibbonItem::new(4, "Mover a", Icon::MoverA).dropdown(),
                        RibbonItem::new(5, "Copiar a", Icon::CopiarA),
                        RibbonItem::new(6, "Eliminar", Icon::Eliminar),
                        RibbonItem::new(7, "Cambiar nombre", Icon::CambiarNombre),
                    ],
                },
            ],
        };
        let vista = Tab {
            title: "Vista",
            groups: vec![Group {
                title: "Mostrar u ocultar",
                items: vec![RibbonItem::new(20, "Extensiones de nombre de archivo", Icon::FileBlank).checkbox()],
            }],
        };
        Ribbon::new(Some("Archivo"), vec![inicio, vista])
    }

    fn centro(r: Rect) -> (i32, i32) {
        (r.x + r.w / 2, r.y + r.h / 2)
    }
    fn click(rb: &mut Ribbon, x: i32, y: i32, w: i32) -> RibbonAct {
        rb.mouse(MouseEv::Down { x, y }, w);
        rb.mouse(MouseEv::Up { x, y }, w)
    }

    #[test]
    fn tab_switch_on_click() {
        let mut rb = demo();
        assert_eq!(rb.active(), 0);
        let tab2 = rb.layout().tabs[1];
        let (x, y) = centro(tab2);
        assert_eq!(rb.mouse(MouseEv::Down { x, y }, 800), RibbonAct::Redraw);
        assert_eq!(rb.active(), 1);
        // Pulsar la activa no cambia nada.
        assert_eq!(rb.mouse(MouseEv::Down { x, y }, 800), RibbonAct::None);
    }

    #[test]
    fn pick_only_on_release_over_same_button() {
        let mut rb = demo();
        let (x, y) = centro(rb.item_rect(1, 800).unwrap());
        assert_eq!(rb.mouse(MouseEv::Down { x, y }, 800), RibbonAct::Redraw);
        assert_eq!(rb.mouse(MouseEv::Up { x, y }, 800), RibbonAct::Pick(1));
        // Pulsar en uno y soltar en otro: nada.
        let (x2, y2) = centro(rb.item_rect(2, 800).unwrap());
        rb.mouse(MouseEv::Down { x, y }, 800);
        assert_eq!(rb.mouse(MouseEv::Up { x: x2, y: y2 }, 800), RibbonAct::Redraw);
        // Soltar sin haber pulsado: nada.
        assert_eq!(rb.mouse(MouseEv::Up { x, y }, 800), RibbonAct::None);
    }

    #[test]
    fn disabled_does_not_pick() {
        let mut rb = demo();
        assert!(rb.set_disabled(1, true));
        assert!(!rb.set_disabled(1, true));
        let (x, y) = centro(rb.item_rect(1, 800).unwrap());
        assert_eq!(click(&mut rb, x, y, 800), RibbonAct::None);
        assert_eq!(rb.mouse(MouseEv::Move { x, y }, 800), RibbonAct::None);
        // Se desactiva con el botón ya pulsado: tampoco emite.
        rb.set_disabled(1, false);
        rb.mouse(MouseEv::Down { x, y }, 800);
        rb.set_disabled(1, true);
        assert_eq!(rb.mouse(MouseEv::Up { x, y }, 800), RibbonAct::Redraw);
    }

    #[test]
    fn file_button() {
        let mut rb = demo();
        let (x, y) = centro(rb.file_rect());
        assert_eq!(click(&mut rb, x, y, 800), RibbonAct::File);
        // Sin botón de archivo el rectángulo está vacío.
        assert!(Ribbon::new(None, vec![]).file_rect().is_empty());
    }

    #[test]
    fn item_rect_matches_click_and_layout() {
        let mut rb = demo();
        for id in 1..=7 {
            let r = rb.item_rect(id, 800).unwrap();
            assert!(r.right() <= 800 && r.bottom() <= rb.height());
            let (x, y) = centro(r);
            assert_eq!(click(&mut rb, x, y, 800), RibbonAct::Pick(id), "id {id}");
        }
        // Fuera de la pestaña activa o de la cinta.
        assert!(rb.item_rect(20, 800).is_none());
        assert!(rb.item_rect(99, 800).is_none());
        // Los botones no se solapan entre sí.
        let it = rb.layout().items;
        for (a, (_, _, ra)) in it.iter().enumerate() {
            for (_, _, rb2) in it.iter().skip(a + 1) {
                assert!(!ra.intersects(*rb2));
            }
        }
    }

    #[test]
    fn checked_and_hover_redraw_only_on_change() {
        let mut rb = demo();
        assert!(rb.set_checked(3, true));
        assert!(!rb.set_checked(3, true));
        assert!(rb.set_checked(3, false));
        let (x, y) = centro(rb.item_rect(3, 800).unwrap());
        assert_eq!(rb.mouse(MouseEv::Move { x, y }, 800), RibbonAct::Redraw);
        assert_eq!(rb.mouse(MouseEv::Move { x: x + 1, y }, 800), RibbonAct::None);
        assert_eq!(rb.mouse(MouseEv::Leave, 800), RibbonAct::Redraw);
        assert_eq!(rb.mouse(MouseEv::Leave, 800), RibbonAct::None);
        // Zona vacía de la cinta: sin hover.
        assert_eq!(rb.mouse(MouseEv::Move { x: 790, y: 60 }, 800), RibbonAct::None);
    }

    #[test]
    fn content_width_covers_every_button() {
        let rb = demo();
        let w = rb.content_width();
        assert!(w > 0);
        assert!(rb.layout().items.iter().all(|t| t.2.right() <= w));
    }

    #[test]
    fn narrow_width_does_not_panic_or_overflow() {
        let mut rb = demo();
        for w in [0, 1, 20, 60, 200] {
            let mut s = Surface::new(w.max(1) + 40, rb.height() + 4, 0x123456);
            rb.draw(&mut s, w, &Theme::default());
            // No toca nada más allá de w ni por debajo de la cinta.
            for j in 0..s.h {
                for i in 0..s.w {
                    if i >= w || j >= rb.height() {
                        assert_eq!(s.px[(j * s.w + i) as usize], 0x123456, "w={w} ({i},{j})");
                    }
                }
            }
            rb.mouse(MouseEv::Move { x: 100, y: 50 }, w);
            rb.mouse(MouseEv::Down { x: w - 1, y: 10 }, w);
            rb.mouse(MouseEv::Up { x: w - 1, y: 10 }, w);
        }
        rb.set_active(0);
        // Un botón fuera del ancho ya no se puede pulsar.
        assert!(rb.item_rect(7, 60).is_none());
        let (x, y) = centro(rb.item_rect(7, 800).unwrap());
        assert_eq!(click(&mut rb, x, y, 60), RibbonAct::None);
    }
}
