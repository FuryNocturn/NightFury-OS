// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Estilos calculados: hoja del navegador, cascada (origen, importancia, especificidad, orden), herencia y
//! traducción de las propiedades soportadas.
use crate::css::{self, AtrSel, Compuesto, Decl, Hoja, OpAttr, Pseudo, Regla, Selector};
use crate::html::{Dato, Dom};
use crate::valores::{color, familia, largo, minus, numero, palabras, px, Col, Ctx, Len};
use crate::{Familia, Letra};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Display {
    None,
    Block,
    Inline,
    InlineBlock,
    ListItem,
    Table,
    TableRow,
    TableCell,
    TableRowGroup,
    TableCaption,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Alin {
    Izq,
    Der,
    Centro,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Blanco {
    Normal,
    Pre,
    NoWrap,
    PreWrap,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lista {
    Disco,
    Circulo,
    Cuadrado,
    Decimal,
    Ninguno,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flot {
    No,
    Izq,
    Der,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Altura {
    Normal,
    Mult(f32),
    Px(f32),
}

/// Lados en orden CSS: arriba, derecha, abajo, izquierda.
pub const ARR: usize = 0;
pub const DER: usize = 1;
pub const ABA: usize = 2;
pub const IZQ: usize = 3;

pub const DECO_SUBRAYA: u8 = 1;
pub const DECO_TACHA: u8 = 2;

const SIN_COLOR: u32 = u32::MAX;

#[derive(Clone, Debug)]
pub struct Estilo {
    pub display: Display,
    pub color: u32,
    pub fondo: Option<u32>,
    pub familia: Familia,
    pub px: f32,
    pub negrita: bool,
    pub cursiva: bool,
    pub lh: Altura,
    pub margen: [Len; 4],
    pub relleno: [Len; 4],
    pub borde: [f32; 4],
    pub borde_vis: [bool; 4],
    pub borde_color: [u32; 4],
    pub ancho: Len,
    pub max_ancho: Len,
    pub min_ancho: Len,
    pub alto: Len,
    pub min_alto: Len,
    pub alinear: Alin,
    pub deco: u8,
    pub blanco: Blanco,
    pub lista: Lista,
    pub visible: bool,
    pub flot: Flot,
    /// Bit 0: left, bit 1: right.
    pub despeja: u8,
    pub caja_borde: bool,
    /// overflow distinto de visible: contiene sus flotantes.
    pub recorta: bool,
    pub espacio: f32,
}

impl Estilo {
    pub fn inicial() -> Estilo {
        Estilo {
            display: Display::Inline,
            color: 0x000000,
            fondo: None,
            familia: Familia::Serif,
            px: 16.0,
            negrita: false,
            cursiva: false,
            lh: Altura::Normal,
            margen: [Len::Px(0.0); 4],
            relleno: [Len::Px(0.0); 4],
            borde: [3.0; 4],
            borde_vis: [false; 4],
            borde_color: [SIN_COLOR; 4],
            ancho: Len::Auto,
            max_ancho: Len::Auto,
            min_ancho: Len::Auto,
            alto: Len::Auto,
            min_alto: Len::Auto,
            alinear: Alin::Izq,
            deco: 0,
            blanco: Blanco::Normal,
            lista: Lista::Disco,
            visible: true,
            flot: Flot::No,
            despeja: 0,
            caja_borde: false,
            recorta: false,
            espacio: 0.0,
        }
    }

    /// Estilo de un hijo: hereda lo heredable y el resto vuelve al valor inicial.
    fn hijo_de(p: &Estilo) -> Estilo {
        Estilo {
            color: p.color,
            familia: p.familia,
            px: p.px,
            negrita: p.negrita,
            cursiva: p.cursiva,
            lh: p.lh,
            alinear: p.alinear,
            deco: p.deco,
            blanco: p.blanco,
            lista: p.lista,
            visible: p.visible,
            espacio: p.espacio,
            ..Estilo::inicial()
        }
    }

    pub fn letra(&self) -> Letra {
        Letra { familia: self.familia, negrita: self.negrita, cursiva: self.cursiva }
    }

    /// Altura de línea en px.
    pub fn alto_linea(&self) -> f32 {
        match self.lh {
            Altura::Normal => self.px * 1.15,
            Altura::Mult(m) => self.px * m,
            Altura::Px(x) => x,
        }
    }
}

/// Hoja del navegador.
pub const UA: &str = "
html, address, blockquote, body, center, dd, details, dir, div, dl, dt, fieldset, figcaption, figure, footer, form,
header, hgroup, legend, main, menu, nav, ol, p, section, summary, ul, article, aside, h1, h2, h3, h4, h5, h6, hr, pre,
listing, xmp, noscript, optgroup { display: block }
head, title, meta, link, style, script, base, template, area, param, source, track, datalist, svg, canvas, video,
audio, iframe, object, embed, noembed, colgroup, col, map, frame, frameset, [hidden] { display: none }
li { display: list-item }
table { display: table; border-spacing: 2px }
caption { display: table-caption; text-align: center }
thead, tbody, tfoot { display: table-row-group }
tr { display: table-row }
td, th { display: table-cell; padding: 1px }
th { font-weight: bold; text-align: center }
body { margin: 8px }
p, dl, pre, listing, xmp { margin: 1em 0 }
blockquote, figure { margin: 1em 40px }
ul, ol, menu, dir { margin: 1em 0; padding-left: 40px }
ul ul, ul ol, ol ul, ol ol { margin: 0 }
ul { list-style-type: disc } ul ul { list-style-type: circle } ul ul ul { list-style-type: square }
ol { list-style-type: decimal }
dd { margin-left: 40px }
h1 { font-size: 2em; margin: .67em 0 } h2 { font-size: 1.5em; margin: .83em 0 } h3 { font-size: 1.17em; margin: 1em 0 }
h4 { margin: 1.33em 0 } h5 { font-size: .83em; margin: 1.67em 0 } h6 { font-size: .67em; margin: 2.33em 0 }
h1, h2, h3, h4, h5, h6, b, strong { font-weight: bold }
pre, listing, xmp, code, kbd, samp, tt { font-family: monospace }
pre, listing, xmp { white-space: pre }
i, em, cite, var, dfn, address { font-style: italic }
u, ins { text-decoration: underline }
s, strike, del { text-decoration: line-through }
a:link { color: #0000ee; text-decoration: underline }
small, sub, sup { font-size: .83em } big { font-size: 1.17em }
mark { background-color: #ffff00 }
hr { margin: 8px 0; border-top: 1px solid #9a9a9a; border-bottom: 1px solid #eeeeee }
center { text-align: center }
nobr { white-space: nowrap }
fieldset { margin: 0 2px; padding: .35em .75em .625em; border: 1px solid #c0c0c0 }
input, select, textarea, button { display: inline-block; font-family: sans-serif; font-size: 13.3333px; color: #000000; text-align: left }
textarea { font-family: monospace }
input[type=checkbox], input[type=radio] { margin: 3px 3px 3px 4px }
";

// ---------------------------------------------------------------------------------------------------------
// Propiedades

fn lados(v: &str, c: &Ctx, vacio_ok: bool) -> Option<[Len; 4]> {
    let p = palabras(v);
    let l: Option<Vec<Len>> = p.iter().map(|t| largo(t, c)).collect();
    let l = l?;
    let r = match l.len() {
        1 => [l[0]; 4],
        2 => [l[0], l[1], l[0], l[1]],
        3 => [l[0], l[1], l[2], l[1]],
        4 => [l[0], l[1], l[2], l[3]],
        _ => return None,
    };
    if !vacio_ok && r.iter().any(|x| matches!(x, Len::Auto) || matches!(x, Len::Px(n) | Len::Pct(n) if *n < 0.0)) {
        return None;
    }
    Some(r)
}

/// Reparte 1 a 4 valores en los cuatro lados.
fn por_lados<T: Copy>(v: &[T]) -> Option<[T; 4]> {
    Some(match v.len() {
        1 => [v[0]; 4],
        2 => [v[0], v[1], v[0], v[1]],
        3 => [v[0], v[1], v[2], v[1]],
        4 => [v[0], v[1], v[2], v[3]],
        _ => return None,
    })
}

fn ancho_borde(t: &str, c: &Ctx) -> Option<f32> {
    match t {
        "thin" => Some(1.0),
        "medium" => Some(3.0),
        "thick" => Some(5.0),
        _ => px(t, c).filter(|&x| x >= 0.0),
    }
}

fn es_estilo_borde(t: &str) -> Option<bool> {
    match t {
        "none" | "hidden" => Some(false),
        "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset" => Some(true),
        _ => None,
    }
}

fn col_borde(c: Col) -> u32 {
    match c {
        Col::Rgb(x) => x,
        _ => SIN_COLOR,
    }
}

/// `border`, `border-top`… en forma corta: ancho, estilo y color en cualquier orden.
fn borde_corto(e: &mut Estilo, lados: &[usize], v: &str, c: &Ctx) {
    let (mut w, mut vis, mut col) = (3.0, None, SIN_COLOR);
    for t in palabras(v) {
        let t = minus(t);
        if let Some(x) = ancho_borde(&t, c) {
            w = x;
        } else if let Some(s) = es_estilo_borde(&t) {
            vis = Some(s);
        } else if let Some(x) = color(&t) {
            col = col_borde(x);
        } else {
            return;
        }
    }
    for &l in lados {
        e.borde[l] = w;
        e.borde_vis[l] = vis.unwrap_or(false);
        e.borde_color[l] = col;
    }
}

fn tam_fuente(t: &str, padre: f32, c: &Ctx) -> Option<f32> {
    let t = minus(t);
    let k = match t.as_str() {
        "xx-small" => 9.0,
        "x-small" => 10.0,
        "small" => 13.0,
        "medium" => 16.0,
        "large" => 18.0,
        "x-large" => 24.0,
        "xx-large" => 32.0,
        "larger" => padre * 1.2,
        "smaller" => padre / 1.2,
        _ => {
            return match largo(&t, c)? {
                Len::Px(x) => Some(x),
                Len::Pct(p) => Some(padre * p / 100.0),
                Len::Auto => None,
            }
        }
    };
    Some(k)
}

fn altura(t: &str, px_propio: f32, c: &Ctx) -> Option<Altura> {
    let t = minus(t);
    if t == "normal" {
        return Some(Altura::Normal);
    }
    if let Some(n) = numero(&t) {
        return (n >= 0.0).then_some(Altura::Mult(n));
    }
    match largo(&t, c)? {
        Len::Px(x) => Some(Altura::Px(x)),
        Len::Pct(p) => Some(Altura::Px(px_propio * p / 100.0)),
        Len::Auto => None,
    }
}

fn peso(t: &str) -> Option<bool> {
    match t {
        "bold" | "bolder" => Some(true),
        "normal" | "lighter" => Some(false),
        _ => numero(t).map(|n| n >= 600.0),
    }
}

/// `font`: [estilo] [variante] [peso] tamaño[/altura] familias.
fn fuente_corta(e: &mut Estilo, v: &str, c: &Ctx, padre: f32, pasada: u8) {
    let ws = palabras(v);
    let (mut neg, mut curs) = (false, false);
    for (i, t) in ws.iter().enumerate() {
        let t = minus(t);
        match t.as_str() {
            "normal" | "small-caps" => continue,
            "italic" | "oblique" => {
                curs = true;
                continue;
            }
            _ => {}
        }
        if let Some(p) = peso(&t) {
            neg = p;
            continue;
        }
        let (tam, lh) = match t.split_once('/') {
            Some((a, b)) => (a, if b.is_empty() { ws.get(i + 1).copied() } else { Some(b) }),
            None => (t.as_str(), None),
        };
        let Some(px) = tam_fuente(tam, padre, c) else { return };
        if pasada == 1 {
            e.px = px;
            return;
        }
        e.negrita = neg;
        e.cursiva = curs;
        e.lh = lh.and_then(|l| altura(l, e.px, c)).unwrap_or(Altura::Normal);
        let resto = ws[i + 1..].join(" ");
        if let Some(f) = familia(&resto) {
            e.familia = f;
        }
        return;
    }
}

fn texto_alinea(t: &str) -> Option<Alin> {
    match t {
        "left" | "start" | "justify" => Some(Alin::Izq),
        "right" | "end" => Some(Alin::Der),
        "center" | "-webkit-center" | "-moz-center" => Some(Alin::Centro),
        _ => None,
    }
}

fn dimension(t: &str, c: &Ctx) -> Option<Len> {
    match t.trim().to_ascii_lowercase().as_str() {
        "none" | "fit-content" | "max-content" | "min-content" | "initial" => Some(Len::Auto),
        x => largo(x, c),
    }
}

/// Aplica una declaración. `pasada` 1: solo el tamaño de letra (lo necesitan los `em`); 2: todo lo demás.
fn aplicar(e: &mut Estilo, p: &Estilo, d: &Decl, c: &Ctx, pasada: u8) {
    let prop = d.prop.as_str();
    let v = d.valor.trim();
    if pasada == 1 {
        match prop {
            "font-size" => {
                if minus(v) == "inherit" {
                    e.px = p.px;
                } else if let Some(x) = tam_fuente(v, p.px, c) {
                    e.px = x;
                }
            }
            "font" => fuente_corta(e, v, c, p.px, 1),
            _ => {}
        }
        return;
    }
    let vm = minus(v);
    let heredar = vm == "inherit";
    match prop {
        "display" => {
            e.display = match vm.as_str() {
                "none" => Display::None,
                "block" | "flex" | "grid" | "flow-root" => Display::Block,
                "inline" | "contents" => Display::Inline,
                "inline-block" | "inline-flex" | "inline-grid" | "inline-table" => Display::InlineBlock,
                "list-item" => Display::ListItem,
                "table" => Display::Table,
                "table-row" => Display::TableRow,
                "table-cell" => Display::TableCell,
                "table-row-group" | "table-header-group" | "table-footer-group" => Display::TableRowGroup,
                "table-caption" => Display::TableCaption,
                "table-column" | "table-column-group" => Display::None,
                "inherit" => p.display,
                _ => return,
            }
        }
        "color" => match color(v) {
            _ if heredar => e.color = p.color,
            Some(Col::Rgb(x)) => e.color = x,
            Some(Col::Actual) => e.color = p.color,
            _ => {}
        },
        "background-color" => match color(v) {
            _ if heredar => e.fondo = p.fondo,
            Some(Col::Rgb(x)) => e.fondo = Some(x),
            Some(Col::Transp) => e.fondo = None,
            Some(Col::Actual) => e.fondo = Some(e.color),
            None => {}
        },
        "background" => {
            // solo el color (shortcut: sin imágenes de fondo; añadir con las imágenes CSS)
            e.fondo = None;
            for t in palabras(v) {
                if let Some(c) = color(t) {
                    match c {
                        Col::Rgb(x) => e.fondo = Some(x),
                        Col::Actual => e.fondo = Some(e.color),
                        Col::Transp => {}
                    }
                }
            }
        }
        "font-family" => {
            if heredar {
                e.familia = p.familia;
            } else if let Some(f) = familia(v) {
                e.familia = f;
            }
        }
        "font-weight" => {
            if heredar {
                e.negrita = p.negrita;
            } else if let Some(b) = peso(&vm) {
                e.negrita = b;
            }
        }
        "font-style" => match vm.as_str() {
            "italic" | "oblique" => e.cursiva = true,
            "normal" => e.cursiva = false,
            "inherit" => e.cursiva = p.cursiva,
            _ => {}
        },
        "font" => fuente_corta(e, v, c, p.px, 2),
        "line-height" => {
            if heredar {
                e.lh = p.lh;
            } else if let Some(a) = altura(v, e.px, c) {
                e.lh = a;
            }
        }
        "margin" => {
            if let Some(l) = lados(v, c, true) {
                e.margen = l;
            }
        }
        "padding" => {
            if let Some(l) = lados(v, c, false) {
                e.relleno = l;
            }
        }
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" | "padding-top" | "padding-right"
        | "padding-bottom" | "padding-left" => {
            let Some(l) = largo(v, c) else { return };
            let i = match prop.rsplit('-').next() {
                Some("top") => ARR,
                Some("right") => DER,
                Some("bottom") => ABA,
                _ => IZQ,
            };
            if prop.starts_with("margin") {
                e.margen[i] = l;
            } else if !matches!(l, Len::Auto) {
                e.relleno[i] = l;
            }
        }
        "border" => borde_corto(e, &[ARR, DER, ABA, IZQ], v, c),
        "border-top" => borde_corto(e, &[ARR], v, c),
        "border-right" => borde_corto(e, &[DER], v, c),
        "border-bottom" => borde_corto(e, &[ABA], v, c),
        "border-left" => borde_corto(e, &[IZQ], v, c),
        "border-width" => {
            let w: Option<Vec<f32>> = palabras(v).iter().map(|t| ancho_borde(&minus(t), c)).collect();
            if let Some(l) = w.as_deref().and_then(por_lados) {
                e.borde = l;
            }
        }
        "border-style" => {
            let w: Option<Vec<bool>> = palabras(v).iter().map(|t| es_estilo_borde(&minus(t))).collect();
            if let Some(l) = w.as_deref().and_then(por_lados) {
                e.borde_vis = l;
            }
        }
        "border-color" => {
            let w: Option<Vec<u32>> = palabras(v).iter().map(|t| color(t).map(col_borde)).collect();
            if let Some(l) = w.as_deref().and_then(por_lados) {
                e.borde_color = l;
            }
        }
        "border-top-width" | "border-right-width" | "border-bottom-width" | "border-left-width" | "border-top-style"
        | "border-right-style" | "border-bottom-style" | "border-left-style" | "border-top-color"
        | "border-right-color" | "border-bottom-color" | "border-left-color" => {
            let i = match &prop[7..prop.len() - prop.rsplit('-').next().map_or(0, |s| s.len()) - 1] {
                "top" => ARR,
                "right" => DER,
                "bottom" => ABA,
                _ => IZQ,
            };
            if prop.ends_with("width") {
                if let Some(x) = ancho_borde(&vm, c) {
                    e.borde[i] = x;
                }
            } else if prop.ends_with("style") {
                if let Some(x) = es_estilo_borde(&vm) {
                    e.borde_vis[i] = x;
                }
            } else if let Some(x) = color(v) {
                e.borde_color[i] = col_borde(x);
            }
        }
        "width" | "height" | "max-width" | "min-width" | "min-height" => {
            let Some(l) = (if heredar { None } else { dimension(v, c) }) else { return };
            match prop {
                "width" => e.ancho = l,
                "height" => e.alto = l,
                "max-width" => e.max_ancho = l,
                "min-width" => e.min_ancho = l,
                _ => e.min_alto = l,
            }
        }
        "text-align" => match texto_alinea(&vm) {
            Some(a) => e.alinear = a,
            None if heredar => e.alinear = p.alinear,
            None => {}
        },
        "text-decoration" | "text-decoration-line" => {
            let mut d = 0;
            let mut ninguno = false;
            for t in palabras(&vm) {
                match t {
                    "underline" => d |= DECO_SUBRAYA,
                    "line-through" => d |= DECO_TACHA,
                    "none" => ninguno = true,
                    _ => {}
                }
            }
            if d != 0 || ninguno {
                e.deco = d;
            } else if heredar {
                e.deco = p.deco;
            }
        }
        "white-space" => match vm.as_str() {
            "normal" | "pre-line" => e.blanco = Blanco::Normal,
            "pre" => e.blanco = Blanco::Pre,
            "nowrap" => e.blanco = Blanco::NoWrap,
            "pre-wrap" | "break-spaces" => e.blanco = Blanco::PreWrap,
            "inherit" => e.blanco = p.blanco,
            _ => {}
        },
        "list-style-type" | "list-style" => {
            for t in palabras(&vm) {
                match t {
                    "disc" => e.lista = Lista::Disco,
                    "circle" => e.lista = Lista::Circulo,
                    "square" => e.lista = Lista::Cuadrado,
                    "none" => e.lista = Lista::Ninguno,
                    "decimal" | "decimal-leading-zero" | "lower-alpha" | "upper-alpha" | "lower-roman"
                    | "upper-roman" | "lower-latin" | "upper-latin" | "lower-greek" => e.lista = Lista::Decimal,
                    _ => {}
                }
            }
        }
        "visibility" => match vm.as_str() {
            "hidden" | "collapse" => e.visible = false,
            "visible" => e.visible = true,
            "inherit" => e.visible = p.visible,
            _ => {}
        },
        "float" => match vm.as_str() {
            "left" => e.flot = Flot::Izq,
            "right" => e.flot = Flot::Der,
            "none" => e.flot = Flot::No,
            _ => {}
        },
        "clear" => match vm.as_str() {
            "left" => e.despeja = 1,
            "right" => e.despeja = 2,
            "both" => e.despeja = 3,
            "none" => e.despeja = 0,
            _ => {}
        },
        "overflow" | "overflow-x" | "overflow-y" => e.recorta = matches!(vm.as_str(), "hidden" | "auto" | "scroll"),
        "box-sizing" => e.caja_borde = vm == "border-box",
        "border-spacing" => {
            if let Some(x) = palabras(v).first().and_then(|t| px(t, c)) {
                e.espacio = x.max(0.0);
            }
        }
        "border-collapse" => {
            if vm == "collapse" {
                e.espacio = 0.0;
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------------------
// Coincidencia de selectores

fn tiene_clase(dom: &Dom, n: usize, cl: &str) -> bool {
    dom.attr(n, "class").is_some_and(|v| v.split_ascii_whitespace().any(|x| x == cl))
}

fn attr_coincide(dom: &Dom, n: usize, a: &AtrSel) -> bool {
    let Some(v) = dom.attr(n, &a.nombre) else { return false };
    let w = a.valor.as_str();
    match a.op {
        OpAttr::Existe => true,
        OpAttr::Igual => v == w,
        OpAttr::Palabra => !w.is_empty() && v.split_ascii_whitespace().any(|x| x == w),
        OpAttr::Guion => v == w || (v.starts_with(w) && v.as_bytes().get(w.len()) == Some(&b'-')),
        OpAttr::Prefijo => !w.is_empty() && v.starts_with(w),
        OpAttr::Sufijo => !w.is_empty() && v.ends_with(w),
        OpAttr::Contiene => !w.is_empty() && v.contains(w),
    }
}

fn es_elem(dom: &Dom, n: usize) -> bool {
    matches!(dom.nodos[n].dato, Dato::Elem(_))
}

/// Hermano elemento anterior (el más cercano).
fn hermano_ant(dom: &Dom, n: usize) -> Option<usize> {
    let p = dom.padre(n)?;
    let pos = dom.nodos[n].pos as usize;
    dom.nodos[p].hijos[..pos].iter().rev().map(|&h| h as usize).find(|&h| es_elem(dom, h))
}

fn comp_coincide(dom: &Dom, n: usize, c: &Compuesto) -> bool {
    let Some(e) = dom.elem(n) else { return false };
    if let Some(t) = &c.etq {
        if *t != e.etq {
            return false;
        }
    }
    if let Some(id) = &c.id {
        if dom.attr(n, "id") != Some(id.as_str()) {
            return false;
        }
    }
    if !c.clases.iter().all(|cl| tiene_clase(dom, n, cl)) || !c.attrs.iter().all(|a| attr_coincide(dom, n, a)) {
        return false;
    }
    c.pseudos.iter().all(|p| match p {
        Pseudo::Enlace => matches!(e.etq.as_str(), "a" | "area") && dom.attr(n, "href").is_some(),
        Pseudo::PrimerHijo => hermano_ant(dom, n).is_none(),
        Pseudo::UltimoHijo => match dom.padre(n) {
            Some(p) => !dom.nodos[p].hijos[dom.nodos[n].pos as usize + 1..].iter().any(|&h| es_elem(dom, h as usize)),
            None => false,
        },
        Pseudo::Raiz => dom.padre(n) == Some(0),
        Pseudo::No(i) => !comp_coincide(dom, n, i),
        Pseudo::Nunca => false,
    })
}

fn sel_coincide(dom: &Dom, n: usize, s: &Selector, k: usize, pres: &mut u32) -> bool {
    if *pres == 0 {
        return false;
    }
    *pres -= 1;
    let parte = &s.partes[k];
    if !comp_coincide(dom, n, &parte.c) {
        return false;
    }
    if k == 0 {
        return true;
    }
    use css::Comb::*;
    match parte.comb {
        Desc => {
            let mut a = dom.padre(n);
            while let Some(x) = a {
                if es_elem(dom, x) && sel_coincide(dom, x, s, k - 1, pres) {
                    return true;
                }
                if *pres == 0 {
                    return false;
                }
                a = dom.padre(x);
            }
            false
        }
        Hijo => dom.padre(n).is_some_and(|p| es_elem(dom, p) && sel_coincide(dom, p, s, k - 1, pres)),
        Sig => hermano_ant(dom, n).is_some_and(|h| sel_coincide(dom, h, s, k - 1, pres)),
        Hermano => {
            let (Some(p), pos) = (dom.padre(n), dom.nodos[n].pos as usize) else { return false };
            for i in (0..pos).rev() {
                let h = dom.nodos[p].hijos[i] as usize;
                if es_elem(dom, h) && sel_coincide(dom, h, s, k - 1, pres) {
                    return true;
                }
                if *pres == 0 {
                    return false;
                }
            }
            false
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Cascada

/// Una hoja con su origen (0 navegador, 1 autor).
pub struct FuenteHoja<'a> {
    pub origen: u8,
    pub hoja: &'a Hoja,
}

struct Indice<'a> {
    reglas: Vec<(u8, &'a Regla)>,
    por_id: BTreeMap<&'a str, Vec<u32>>,
    por_clase: BTreeMap<&'a str, Vec<u32>>,
    por_etq: BTreeMap<&'a str, Vec<u32>>,
    univ: Vec<u32>,
}

impl<'a> Indice<'a> {
    fn nuevo(hojas: &[FuenteHoja<'a>], ancho: f32) -> Indice<'a> {
        let mut ix = Indice { reglas: Vec::new(), por_id: BTreeMap::new(), por_clase: BTreeMap::new(), por_etq: BTreeMap::new(), univ: Vec::new() };
        for h in hojas {
            for r in &h.hoja.reglas {
                if !css::media_ok(&r.medias, ancho) {
                    continue;
                }
                let i = ix.reglas.len() as u32;
                ix.reglas.push((h.origen, r));
                let Some(ult) = r.sel.partes.last() else { continue };
                let c = &ult.c;
                if let Some(id) = &c.id {
                    ix.por_id.entry(id.as_str()).or_default().push(i);
                } else if let Some(cl) = c.clases.first() {
                    ix.por_clase.entry(cl.as_str()).or_default().push(i);
                } else if let Some(t) = &c.etq {
                    ix.por_etq.entry(t.as_str()).or_default().push(i);
                } else {
                    ix.univ.push(i);
                }
            }
        }
        ix
    }
}

/// Declaraciones de presentación de los atributos HTML antiguos (bgcolor, width, align…).
fn pistas(dom: &Dom, n: usize, etq: &str) -> Vec<Decl> {
    let mut r = Vec::new();
    let mut ponr = |p: &str, v: String| r.push(Decl { prop: p.to_string(), valor: v, importante: false });
    let dim = |v: &str| -> Option<String> {
        let t = v.trim();
        let fin = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
        if fin == 0 {
            return None;
        }
        Some(if t[fin..].starts_with('%') { format!("{}%", &t[..fin]) } else { format!("{}px", &t[..fin]) })
    };
    if let Some(v) = dom.attr(n, "bgcolor") {
        ponr("background-color", v.to_string());
    }
    if matches!(etq, "table" | "td" | "th" | "img" | "col") {
        if let Some(v) = dom.attr(n, "width").and_then(dim) {
            ponr("width", v);
        }
    }
    if matches!(etq, "img" | "td" | "th") {
        if let Some(v) = dom.attr(n, "height").and_then(dim) {
            ponr("height", v);
        }
    }
    let al = dom.attr(n, "align").map(minus);
    match (etq, al.as_deref()) {
        ("table", Some("center")) => ponr("margin", "0 auto".to_string()),
        ("table", Some("right")) => ponr("float", "right".to_string()),
        ("table", Some("left")) => ponr("float", "left".to_string()),
        ("img", Some("left" | "right")) => ponr("float", al.clone().unwrap_or_default()),
        ("div" | "p" | "td" | "th" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "caption" | "tbody" | "thead", Some(a)) => {
            ponr("text-align", a.to_string())
        }
        _ => {}
    }
    match etq {
        "table" => {
            if let Some(b) = dom.attr(n, "border").and_then(|b| b.trim().parse::<u32>().ok()).filter(|&b| b > 0) {
                ponr("border", format!("{}px solid #808080", b.min(20)));
            }
            if let Some(s) = dom.attr(n, "cellspacing").and_then(dim) {
                ponr("border-spacing", s);
            }
        }
        "td" | "th" => {
            // los atributos de la tabla: se busca hasta 4 niveles hacia arriba
            let mut a = dom.padre(n);
            for _ in 0..4 {
                let Some(x) = a else { break };
                if dom.etq(x) == "table" {
                    if let Some(p) = dom.attr(x, "cellpadding").and_then(dim) {
                        ponr("padding", p);
                    }
                    if dom.attr(x, "border").and_then(|b| b.trim().parse::<u32>().ok()).is_some_and(|b| b > 0) {
                        ponr("border", "1px solid #808080".to_string());
                    }
                    break;
                }
                a = dom.padre(x);
            }
            if dom.attr(n, "nowrap").is_some() {
                ponr("white-space", "nowrap".to_string());
            }
        }
        "body" => {
            if let Some(v) = dom.attr(n, "text") {
                ponr("color", v.to_string());
            }
        }
        "font" => {
            if let Some(v) = dom.attr(n, "color") {
                ponr("color", v.to_string());
            }
            if let Some(v) = dom.attr(n, "face") {
                ponr("font-family", v.to_string());
            }
            if let Some(v) = dom.attr(n, "size") {
                let t = v.trim();
                let base: i32 = 3;
                let k = match t.chars().next() {
                    Some('+') => base + t[1..].parse::<i32>().unwrap_or(0),
                    Some('-') => base - t[1..].parse::<i32>().unwrap_or(0),
                    _ => t.parse::<i32>().unwrap_or(base),
                };
                const T: [u8; 7] = [10, 13, 16, 18, 24, 32, 48];
                ponr("font-size", format!("{}px", T[(k.clamp(1, 7) - 1) as usize]));
            }
        }
        _ => {}
    }
    r
}

/// Resultado de la cascada: un estilo por elemento y, para cada nodo, el índice de su estilo (los textos
/// usan el de su padre).
pub struct Estilos {
    pub lista: Vec<Estilo>,
    pub idx: Vec<u32>,
    /// Fondo del lienzo (html o body).
    pub lienzo: Option<u32>,
}

impl Estilos {
    pub fn de(&self, n: usize) -> &Estilo {
        &self.lista[self.idx[n] as usize]
    }
}

/// Altura de ventana que se supone para `vh` (el motor no recibe la real).
/// shortcut: fija; pasar el alto real si el programa lo da.
const ALTO_VP: f32 = 720.0;

pub fn calcular(dom: &Dom, hojas: &[FuenteHoja<'_>], ancho: f32) -> Estilos {
    let ix = Indice::nuevo(hojas, ancho);
    let n_nodos = dom.nodos.len();
    let mut lista: Vec<Estilo> = Vec::new();
    let mut idx: Vec<u32> = alloc::vec![0; n_nodos];
    lista.push(Estilo::inicial()); // raíz del documento
    let mut rem = 16.0f32;
    let mut cand: Vec<u32> = Vec::new();
    for n in 1..n_nodos {
        let padre = dom.nodos[n].padre as usize;
        let ip = idx[padre] as usize;
        let Some(el) = dom.elem(n) else {
            idx[n] = ip as u32;
            continue;
        };
        cand.clear();
        let mut apl: Vec<(u8, u32, u32, &Decl)> = Vec::new();
        let extra = |k: &BTreeMap<&str, Vec<u32>>, clave: &str, out: &mut Vec<u32>| {
            if let Some(v) = k.get(clave) {
                out.extend_from_slice(v);
            }
        };
        if let Some(id) = dom.attr(n, "id") {
            extra(&ix.por_id, id, &mut cand);
        }
        if let Some(cl) = dom.attr(n, "class") {
            for c in cl.split_ascii_whitespace() {
                extra(&ix.por_clase, c, &mut cand);
            }
        }
        extra(&ix.por_etq, &el.etq, &mut cand);
        cand.extend_from_slice(&ix.univ);
        for &ci in &cand {
            let (origen, r) = ix.reglas[ci as usize];
            let mut pres = 4000;
            if sel_coincide(dom, n, &r.sel, r.sel.partes.len() - 1, &mut pres) {
                for d in r.decls.iter() {
                    let nivel = match (origen, d.importante) {
                        (0, false) => 0,
                        (_, false) => 2,
                        (0, true) => 4,
                        _ => 3,
                    };
                    apl.push((nivel, r.sel.espec, ci, d));
                }
            }
        }
        let hints = pistas(dom, n, &el.etq);
        for d in &hints {
            apl.push((1, 0, 0, d));
        }
        let propias = dom.attr(n, "style").map(css::parsear_decls).unwrap_or_default();
        {
            for d in &propias {
                apl.push((if d.importante { 3 } else { 2 }, u32::MAX, u32::MAX, d));
            }
        }
        apl.sort_by_key(|&(a, b, c, _)| (a, b, c));
        let p = &lista[ip];
        let mut e = Estilo::hijo_de(p);
        let mut ctx = Ctx { em: p.px, rem, vw: ancho, vh: ALTO_VP };
        for &(_, _, _, d) in &apl {
            aplicar(&mut e, p, d, &ctx, 1);
        }
        ctx.em = e.px;
        for &(_, _, _, d) in &apl {
            aplicar(&mut e, p, d, &ctx, 2);
        }
        for i in 0..4 {
            if !e.borde_vis[i] {
                e.borde[i] = 0.0;
            }
            if e.borde_color[i] == SIN_COLOR {
                e.borde_color[i] = e.color;
            }
        }
        if el.etq == "html" {
            rem = e.px;
        }
        // los controles que el motor no pinta (type=hidden se envía pero no se ve; type=file no se admite)
        if el.etq == "input" && el.attrs.iter().any(|(k, v)| k == "type" && matches!(v.trim().to_ascii_lowercase().as_str(), "hidden" | "file")) {
            e.display = Display::None;
        }
        idx[n] = lista.len() as u32;
        lista.push(e);
    }
    // el fondo de html (o, si no tiene, el de body) pasa al lienzo y no se pinta dos veces
    let mut lienzo = None;
    for etq in ["html", "body"] {
        if let Some(n) = (1..n_nodos).take(64).find(|&n| dom.etq(n) == etq) {
            let e = &mut lista[idx[n] as usize];
            if lienzo.is_none() {
                lienzo = e.fondo;
            }
            e.fondo = None;
            if lienzo.is_some() {
                break;
            }
        }
    }
    Estilos { lista, idx, lienzo }
}
