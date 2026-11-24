// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Contenido en línea: átomos (palabras, espacios, imágenes, cajas en línea), partido en líneas,
//! alineación y emisión de los trozos de texto.
use super::*;
use crate::estilo::{Alin, Blanco};
use crate::forms::{alto_linea, Tipo};

/// Espacios fijos seguidos que se mantienen como mucho (los `pre` hostiles).
const MAX_ESP: u32 = 1024;

enum Atomo<'a> {
    Texto { s: &'a str, w: i32, nodo: usize, nobr: bool },
    Espacio { w: i32, n: u32, nodo: usize, fijo: bool, nobr: bool },
    Img { w: i32, h: i32, img: u32, nodo: usize, ctl: u32 },
    /// Control de formulario; `baja` px quedan bajo la línea base.
    Ctl { w: i32, h: i32, baja: i32, ctl: u32, nodo: usize },
    Bloque { w: i32, h: i32, items: Vec<Item> },
    Hueco(i32),
    Ancla(usize),
}

struct Linea<'a> {
    atomos: Vec<Atomo<'a>>,
    ancho: i32,
    contenido: bool,
    /// Borde izquierdo libre y ancho disponible de la línea que se está llenando.
    lx: i32,
    disp: i32,
    cx: i32,
    cw: i32,
    padre: usize,
}

fn blanco(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0C)
}

impl<'a> Linea<'a> {
    fn recontar(&mut self) {
        self.ancho = self
            .atomos
            .iter()
            .map(|a| match a {
                Atomo::Texto { w, .. } | Atomo::Espacio { w, .. } | Atomo::Img { w, .. } | Atomo::Bloque { w, .. } | Atomo::Ctl { w, .. } => *w,
                Atomo::Hueco(w) => *w,
                Atomo::Ancla(_) => 0,
            })
            .fold(0i32, |a, b| a.saturating_add(b));
        self.contenido = self.atomos.iter().any(|a| matches!(a, Atomo::Texto { .. } | Atomo::Img { .. } | Atomo::Bloque { .. } | Atomo::Ctl { .. } | Atomo::Espacio { fijo: true, .. }));
    }
}

/// Texto acumulado de un mismo estilo en una línea.
struct Trozo {
    idx: usize,
    nodo: usize,
    x: i32,
    w: i32,
    s: String,
}

impl<'a> Maq<'a> {
    /// Recalcula el hueco libre de la línea siguiente (depende de los flotantes).
    fn reiniciar(&mut self, l: &mut Linea<'a>) {
        let y = self.y + self.pend;
        let h = px_i(self.est.de(l.padre).alto_linea()).max(1);
        let (lx, rx) = self.rango(y, h, l.cx, l.cx + l.cw);
        l.lx = lx;
        l.disp = rx - lx;
    }

    /// Maqueta los hijos de `padre` dentro de [cx, cx+cw] desde la posición actual.
    pub(super) fn fluir(&mut self, padre: usize, cx: i32, cw: i32, prof: u32) {
        let (dom, est) = (self.dom, self.est);
        let mut l = Linea { atomos: Vec::new(), ancho: 0, contenido: false, lx: cx, disp: cw, cx, cw, padre };
        self.reiniciar(&mut l);
        // pila de (nodo, siguiente hijo, hueco de cierre)
        let mut pila: Vec<(usize, usize, i32)> = alloc::vec![(padre, 0, 0)];
        while let Some(&(nodo, i, fin)) = pila.last() {
            if self.lleno {
                break;
            }
            if i >= dom.nodos[nodo].hijos.len() {
                pila.pop();
                if fin != 0 {
                    self.poner(&mut l, Atomo::Hueco(fin));
                }
                continue;
            }
            if let Some(t) = pila.last_mut() {
                t.1 += 1;
            }
            let h = dom.nodos[nodo].hijos[i] as usize;
            let el = match &dom.nodos[h].dato {
                Dato::Texto(t) => {
                    self.texto(&mut l, t, h);
                    continue;
                }
                Dato::Elem(el) => el,
                Dato::Raiz => continue,
            };
            let e = est.de(h);
            if e.display == Display::None {
                continue;
            }
            if el.etq == "br" {
                self.emitir(&mut l, true);
                continue;
            }
            let k = self.forms.ctl_de[h];
            if k != SIN_CTL {
                self.control(&mut l, h, k);
                continue;
            }
            if e.flot != Flot::No && prof < MAX_PROF {
                self.emitir(&mut l, false);
                self.colocar_flotante(h, cx, cw, prof);
                self.reiniciar(&mut l);
                continue;
            }
            if el.etq == "img" {
                self.imagen(&mut l, h);
                continue;
            }
            let en_linea = e.display == Display::Inline || prof >= MAX_PROF;
            if en_linea {
                self.poner(&mut l, Atomo::Ancla(h));
                let p = |x: Len| largo(x, cw).unwrap_or(0).max(0);
                let b = |i: usize| px_i(e.borde[i]).max(0);
                let (ini, fin) = (
                    largo(e.margen[IZQ], cw).unwrap_or(0) + p(e.relleno[IZQ]) + b(IZQ),
                    largo(e.margen[DER], cw).unwrap_or(0) + p(e.relleno[DER]) + b(DER),
                );
                if ini != 0 {
                    self.poner(&mut l, Atomo::Hueco(ini));
                }
                pila.push((h, 0, fin));
            } else if e.display == Display::InlineBlock {
                self.atomo_bloque(&mut l, h, cw, prof);
            } else {
                self.emitir(&mut l, false);
                self.bloque(h, cx, cw, prof + 1);
                self.reiniciar(&mut l);
            }
        }
        self.emitir(&mut l, false);
    }

    /// Maqueta una caja `inline-block` aparte y la mete como un átomo.
    fn atomo_bloque(&mut self, l: &mut Linea<'a>, n: usize, cw: i32, prof: u32) {
        let (items, y, pend, flot, marc, na) =
            (core::mem::take(&mut self.items), self.y, self.pend, core::mem::take(&mut self.flot), self.marcador.take(), self.anclas.len());
        self.y = 0;
        self.pend = 0;
        let (_, ow) = self.bloque(n, 0, cw, prof + 1);
        let h = self.y + self.pend;
        let local = core::mem::replace(&mut self.items, items);
        self.y = y;
        self.pend = pend;
        self.flot = flot;
        self.marcador = marc;
        self.anclas.truncate(na);
        self.poner(l, Atomo::Bloque { w: ow, h, items: local });
    }

    /// Un control de formulario como átomo de la línea (sus hijos, las opciones o el rótulo, no se maquetan).
    fn control(&mut self, l: &mut Linea<'a>, n: usize, k: u32) {
        let (est, forms) = (self.est, self.forms);
        let c = &forms.ctl[k as usize];
        if c.tipo == Tipo::Oculto {
            return;
        }
        // `input type=image` con su imagen: se pone como imagen que, además, envía el formulario.
        let im = self.img_de[n];
        if c.tipo == Tipo::Imagen && im != SIN_IMG && !matches!(self.imgs.get(im as usize).map(|i| &i.estado), Some(EstadoImg::Fallo) | None) {
            return self.imagen(l, n);
        }
        let e = est.de(n);
        let (w, h) = self.tam_control(n, k, l.cw);
        let (ml, mr) = (largo(e.margen[IZQ], l.cw).unwrap_or(0).max(0), largo(e.margen[DER], l.cw).unwrap_or(0).max(0));
        if ml != 0 {
            self.poner(l, Atomo::Hueco(ml));
        }
        self.poner(l, Atomo::Ancla(n));
        // el texto de un campo se centra en su caja; casillas y radios apoyan en la línea base
        let baja = match c.tipo {
            Tipo::Casilla | Tipo::Radio => 0,
            _ => ((h - alto_linea(e.px)) / 2).max(0) + px_i(self.metricas(est.idx[n] as usize).desc),
        };
        self.poner(l, Atomo::Ctl { w, h, baja: baja.min(h), ctl: k, nodo: n });
        if mr != 0 {
            self.poner(l, Atomo::Hueco(mr));
        }
    }

    /// Tamaño con el que se reserva una imagen (0×0 si aún no se sabe).
    pub(super) fn tam_img(&self, n: usize, avail: i32) -> (i32, i32) {
        let e = self.est.de(n);
        let k = self.img_de[n];
        let nat = self.imgs.get(k as usize).and_then(|im| match &im.estado {
            EstadoImg::Lista(p) if p.w > 0 && p.h > 0 => Some((p.w as i64, p.h as i64)),
            _ => im.dim.filter(|&(w, h)| w > 0 && h > 0).map(|(w, h)| (w as i64, h as i64)),
        });
        let (wd, hd) = (largo(e.ancho, avail), largo(e.alto, avail));
        let (mut w, mut h) = match (wd, hd, nat) {
            (Some(w), Some(h), _) => (w as i64, h as i64),
            (Some(w), None, Some((nw, nh))) => (w as i64, w as i64 * nh / nw),
            (None, Some(h), Some((nw, nh))) => (h as i64 * nw / nh, h as i64),
            (None, None, Some(n)) => n,
            (Some(w), None, None) => (w as i64, 0),
            _ => (0, 0),
        };
        if let Some(m) = largo(e.max_ancho, avail) {
            if w > m as i64 && w > 0 {
                h = h * m as i64 / w;
                w = m as i64;
            }
        }
        let a = avail.max(1) as i64;
        if w > a {
            h = h * a / w;
            w = a;
        }
        (w.clamp(0, MAX_ANCHO as i64) as i32, h.clamp(0, MAX_ALTO as i64) as i32)
    }

    fn imagen(&mut self, l: &mut Linea<'a>, n: usize) {
        let (dom, est) = (self.dom, self.est);
        let k = self.img_de[n];
        let fallo = k == SIN_IMG || matches!(self.imgs.get(k as usize).map(|i| &i.estado), Some(EstadoImg::Fallo) | None);
        if fallo {
            if let Some(alt) = dom.attr(n, "alt").filter(|a| !a.trim().is_empty()) {
                self.texto(l, alt, n);
            }
            return;
        }
        let (w, h) = self.tam_img(n, l.cw);
        if w == 0 || h == 0 {
            return;
        }
        let e = est.de(n);
        let (ml, mr) = (largo(e.margen[IZQ], l.cw).unwrap_or(0).max(0), largo(e.margen[DER], l.cw).unwrap_or(0).max(0));
        if ml != 0 {
            self.poner(l, Atomo::Hueco(ml));
        }
        self.poner(l, Atomo::Ancla(n));
        self.poner(l, Atomo::Img { w, h, img: k, nodo: n, ctl: self.forms.ctl_de[n] });
        if mr != 0 {
            self.poner(l, Atomo::Hueco(mr));
        }
    }

    /// Parte un texto en palabras y espacios según `white-space`.
    fn texto(&mut self, l: &mut Linea<'a>, t: &'a str, nodo: usize) {
        let est = self.est;
        let e = est.de(nodo);
        let met = self.metricas(est.idx[nodo] as usize);
        let esp = nf_core::math::iround(met.esp.clamp(0.0, 1000.0));
        let b = t.as_bytes();
        match e.blanco {
            Blanco::Normal | Blanco::NoWrap => {
                let nobr = e.blanco == Blanco::NoWrap;
                let mut i = 0;
                while i < b.len() && !self.lleno {
                    let bl = blanco(b[i]);
                    let mut j = i;
                    while j < b.len() && blanco(b[j]) == bl {
                        j += 1;
                    }
                    if bl {
                        self.poner(l, Atomo::Espacio { w: esp, n: 1, nodo, fijo: false, nobr });
                    } else {
                        let s = &t[i..j];
                        let w = self.medir(e, s);
                        self.poner(l, Atomo::Texto { s, w, nodo, nobr });
                    }
                    i = j;
                }
            }
            Blanco::Pre | Blanco::PreWrap => {
                let nobr = e.blanco == Blanco::Pre;
                for (k, linea) in t.split('\n').enumerate() {
                    if k > 0 {
                        self.emitir(l, true);
                    }
                    let linea = linea.trim_end_matches('\r');
                    let b = linea.as_bytes();
                    let mut i = 0;
                    while i < b.len() && !self.lleno {
                        let esp_run = b[i] == b' ' || b[i] == b'\t';
                        let mut j = i;
                        let mut cols = 0u32;
                        while j < b.len() && (b[j] == b' ' || b[j] == b'\t') == esp_run {
                            cols += if b[j] == b'\t' { 8 } else { 1 };
                            j += 1;
                        }
                        if esp_run {
                            let n = cols.min(MAX_ESP);
                            self.poner(l, Atomo::Espacio { w: (n as i64 * esp as i64).min(MAX_ANCHO as i64) as i32, n, nodo, fijo: true, nobr });
                        } else {
                            let s = &linea[i..j];
                            let w = self.medir(e, s);
                            self.poner(l, Atomo::Texto { s, w, nodo, nobr });
                        }
                        i = j;
                    }
                }
            }
        }
    }

    /// Índice (exclusivo) hasta donde se puede cortar la línea: tras el último espacio o caja.
    fn corte(l: &Linea<'a>) -> Option<usize> {
        l.atomos.iter().rposition(|a| matches!(a, Atomo::Espacio { nobr: false, .. } | Atomo::Img { .. } | Atomo::Bloque { .. } | Atomo::Ctl { .. })).map(|i| i + 1)
    }

    /// Sitúa un átomo en la línea, partiéndola si no cabe.
    fn poner(&mut self, l: &mut Linea<'a>, a: Atomo<'a>) {
        let (w, rompible) = match &a {
            Atomo::Espacio { fijo, w, .. } => {
                if !*fijo && (!l.contenido || matches!(l.atomos.last(), Some(Atomo::Espacio { fijo: false, .. }))) {
                    return;
                }
                l.ancho = l.ancho.saturating_add(*w);
                l.contenido |= *fijo;
                l.atomos.push(a);
                return;
            }
            Atomo::Hueco(w) => {
                l.ancho = l.ancho.saturating_add(*w);
                l.atomos.push(a);
                return;
            }
            Atomo::Ancla(_) => {
                l.atomos.push(a);
                return;
            }
            Atomo::Texto { w, nobr, .. } => (*w, !*nobr),
            Atomo::Img { w, .. } | Atomo::Bloque { w, .. } | Atomo::Ctl { w, .. } => (*w, true),
        };
        for _ in 0..10_000 {
            if l.ancho.saturating_add(w) <= l.disp {
                break;
            }
            if l.contenido {
                if !rompible {
                    break;
                }
                let Some(k) = Self::corte(l) else { break };
                let cola = l.atomos.split_off(k);
                self.emitir(l, false);
                l.atomos = cola;
                l.recontar();
                continue;
            }
            // línea sin contenido: si un flotante la estrecha, se baja hasta que acabe
            let y = self.y + self.pend;
            match self.siguiente_flot(y, px_i(self.est.de(l.padre).alto_linea()).max(1)) {
                Some(ny) if ny > y => {
                    self.y = ny;
                    self.pend = 0;
                    self.reiniciar(l);
                }
                _ => break,
            }
        }
        if let Atomo::Texto { s, nodo, .. } = &a {
            if rompible && !l.contenido && l.ancho.saturating_add(w) > l.disp && l.disp > 0 {
                self.dividir_palabra(l, s, *nodo);
                return;
            }
        }
        l.ancho = l.ancho.saturating_add(w);
        l.contenido = true;
        l.atomos.push(a);
    }

    /// Una palabra más ancha que la línea se corta por letras.
    fn dividir_palabra(&mut self, l: &mut Linea<'a>, s: &'a str, nodo: usize) {
        let e = self.est.de(nodo);
        let (mut ini, mut acc) = (0, 0i32);
        let mut buf = [0u8; 4];
        for (i, ch) in s.char_indices() {
            let cw = self.medir(e, ch.encode_utf8(&mut buf));
            if acc + cw > l.disp.max(1) && i > ini {
                l.atomos.push(Atomo::Texto { s: &s[ini..i], w: acc, nodo, nobr: true });
                l.ancho = acc;
                l.contenido = true;
                self.emitir(l, false);
                ini = i;
                acc = 0;
            }
            acc = acc.saturating_add(cw);
        }
        l.atomos.push(Atomo::Texto { s: &s[ini..], w: acc, nodo, nobr: false });
        l.ancho = l.ancho.saturating_add(acc);
        l.contenido = true;
    }

    /// (alto sobre la línea base, alto bajo ella) de una línea de texto con el estilo `idx`.
    fn alturas(&mut self, idx: usize) -> (i32, i32) {
        let m = self.metricas(idx);
        let lh = self.est.lista[idx].alto_linea();
        let lead = lh - (m.asc + m.desc);
        let top = px_i(m.asc + lead / 2.0);
        (top, (px_i(lh) - top).max(0))
    }

    fn soltar_trozo(&mut self, t: Option<Trozo>, base: i32) {
        let Some(t) = t else { return };
        let est = self.est;
        let e = &est.lista[t.idx];
        if !e.visible || t.s.is_empty() {
            return;
        }
        let m = self.metricas(t.idx);
        let (asc, desc) = (px_i(m.asc), px_i(m.desc));
        if e.display == Display::Inline {
            if let Some(f) = e.fondo {
                self.push(Item::Caja { r: Rect::new(t.x, base - asc, t.w, asc + desc), fondo: Some(f), borde: [0; 4], color: [0; 4] });
            }
        }
        self.push(Item::Texto(Texto {
            x: t.x,
            y: base,
            w: t.w,
            asc,
            desc,
            letra: e.letra(),
            px: e.px,
            color: e.color,
            deco: e.deco,
            enlace: self.enlace_de[t.nodo],
            etiq: self.forms.etiqueta_de[t.nodo],
            s: t.s,
        }));
    }

    /// Cierra la línea actual: la coloca (alineación, flotantes), emite sus elementos y avanza `y`.
    /// Con `forzada`, una línea vacía también ocupa su altura (es el caso de `<br>`).
    fn emitir(&mut self, l: &mut Linea<'a>, forzada: bool) {
        while let Some(Atomo::Espacio { fijo: false, w, .. }) = l.atomos.last() {
            l.ancho -= *w;
            l.atomos.pop();
        }
        if !l.contenido && !forzada {
            // solo anclas o huecos: se anotan las anclas y nada más
            let y = self.y + self.pend;
            for a in l.atomos.drain(..) {
                if let Atomo::Ancla(n) = a {
                    if self.anclas.len() < MAX_ANCLAS {
                        let dom = self.dom;
                        if dom.attr(n, "id").is_some() || dom.attr(n, "name").is_some() {
                            self.anclar(n, y);
                        }
                    }
                }
            }
            l.ancho = 0;
            return;
        }
        let est = self.est;
        let (mut top, mut bot) = self.alturas(est.idx[l.padre] as usize);
        for a in &l.atomos {
            let (t, b) = match a {
                Atomo::Texto { nodo, .. } | Atomo::Espacio { nodo, .. } => self.alturas(est.idx[*nodo] as usize),
                Atomo::Img { h, .. } | Atomo::Bloque { h, .. } => (*h, 0),
                Atomo::Ctl { h, baja, .. } => (*h - *baja, *baja),
                _ => continue,
            };
            top = top.max(t);
            bot = bot.max(b);
        }
        self.y += self.pend;
        self.pend = 0;
        let y_l = self.y;
        let (lx, rx) = self.rango(y_l, top + bot, l.cx, l.cx + l.cw);
        let libre = (rx - lx - l.ancho).max(0);
        let x0 = lx
            + match est.de(l.padre).alinear {
                Alin::Izq => 0,
                Alin::Centro => libre / 2,
                Alin::Der => libre,
            };
        let base = y_l + top;
        let mut x = x0;
        let mut trozo: Option<Trozo> = None;
        for a in core::mem::take(&mut l.atomos) {
            match a {
                Atomo::Texto { s, w, nodo, .. } => {
                    let idx = est.idx[nodo] as usize;
                    match &mut trozo {
                        Some(t) if t.idx == idx => {
                            t.s.push_str(s);
                            t.w += w;
                        }
                        _ => {
                            let ant = trozo.take();
                            self.soltar_trozo(ant, base);
                            trozo = Some(Trozo { idx, nodo, x, w, s: s.to_string() });
                        }
                    }
                    x += w;
                }
                Atomo::Espacio { w, n, nodo, .. } => {
                    let idx = est.idx[nodo] as usize;
                    match &mut trozo {
                        Some(t) if t.idx == idx => {
                            t.s.extend(core::iter::repeat(' ').take(n as usize));
                            t.w += w;
                        }
                        _ => {
                            let ant = trozo.take();
                            self.soltar_trozo(ant, base);
                            trozo = Some(Trozo { idx, nodo, x, w, s: " ".repeat(n as usize) });
                        }
                    }
                    x += w;
                }
                Atomo::Img { w, h, img, nodo, ctl } => {
                    let ant = trozo.take();
                    self.soltar_trozo(ant, base);
                    if est.de(nodo).visible {
                        self.push(Item::Imagen { r: Rect::new(x, base - h, w, h), img, enlace: self.enlace_de[nodo], ctl });
                    }
                    x += w;
                }
                Atomo::Ctl { w, h, baja, ctl, nodo } => {
                    let ant = trozo.take();
                    self.soltar_trozo(ant, base);
                    let e = est.de(nodo);
                    if e.visible {
                        self.push(Item::Control { r: Rect::new(x, base + baja - h, w, h), ctl, px: e.px, letra: e.letra(), color: e.color, fondo: e.fondo });
                    }
                    x += w;
                }
                Atomo::Bloque { w, h, mut items } => {
                    let ant = trozo.take();
                    self.soltar_trozo(ant, base);
                    for it in &mut items {
                        it.mover(x, base - h);
                    }
                    for it in items {
                        self.push(it);
                    }
                    x += w;
                }
                Atomo::Hueco(w) => {
                    let ant = trozo.take();
                    self.soltar_trozo(ant, base);
                    x += w;
                }
                Atomo::Ancla(n) => {
                    let dom = self.dom;
                    if dom.attr(n, "id").is_some() || dom.attr(n, "name").is_some() {
                        self.anclar(n, y_l);
                    }
                }
            }
        }
        self.soltar_trozo(trozo, base);
        if let Some(m) = self.marcador.take() {
            self.poner_marcador(&m, base);
        }
        self.y = y_l + top + bot;
        l.ancho = 0;
        l.contenido = false;
        self.reiniciar(l);
    }
}
