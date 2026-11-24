// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Maquetado: flujo de bloques y líneas, flotantes, listas e imágenes. Produce una lista de dibujo
//! (`Item`) en coordenadas del documento. Las líneas están en `linea` y las tablas en `tabla`.
use crate::estilo::{Display, Estilo, Estilos, Flot, Lista, ABA, ARR, DER, IZQ};
use crate::forms::{Forms, SIN_CTL};
use crate::html::{Dato, Dom};
use crate::valores::Len;
use crate::{Fuentes, Letra};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use nf_core::bmp::Picture;
use nf_core::geom::Rect;
use nf_core::math::iround;

mod ctl;
mod linea;
mod tabla;

/// Elementos de dibujo como mucho.
pub const MAX_CAJAS: usize = 500_000;
/// Alto máximo del documento en px.
pub const MAX_ALTO: i32 = 1_000_000;
/// Ancho máximo de cualquier caja en px.
pub const MAX_ANCHO: i32 = 100_000;
/// Profundidad máxima de cajas anidadas; más hondo se aplana (se trata como línea).
const MAX_PROF: u32 = 48;
const MAX_ANCLAS: usize = 20_000;
pub const SIN_IMG: u32 = u32::MAX;

pub enum EstadoImg {
    Pendiente,
    Fallo,
    Lista(Picture),
}

pub struct Img {
    pub url: String,
    pub estado: EstadoImg,
    pub dim: Option<(u32, u32)>,
}

pub struct Texto {
    pub x: i32,
    /// Línea base.
    pub y: i32,
    pub w: i32,
    pub asc: i32,
    pub desc: i32,
    pub letra: Letra,
    pub px: f32,
    pub color: u32,
    pub deco: u8,
    pub enlace: u32,
    /// Control al que apunta su `<label>` (índice + 1; 0 = ninguno).
    pub etiq: u32,
    pub s: String,
}

pub enum Item {
    Caja { r: Rect, fondo: Option<u32>, borde: [i32; 4], color: [u32; 4] },
    Texto(Texto),
    Imagen { r: Rect, img: u32, enlace: u32, ctl: u32 },
    /// Control de formulario: se pinta con su estado (`Documento::forms`), con la letra del elemento.
    Control { r: Rect, ctl: u32, px: f32, letra: Letra, color: u32, fondo: Option<u32> },
    Vineta { cx: i32, cy: i32, r: i32, tipo: Lista, color: u32 },
}

impl Item {
    /// Desplaza el elemento (para colocar lo maquetado en origen local).
    fn mover(&mut self, dx: i32, dy: i32) {
        match self {
            Item::Caja { r, .. } | Item::Imagen { r, .. } | Item::Control { r, .. } => {
                r.x += dx;
                r.y += dy;
            }
            Item::Texto(t) => {
                t.x += dx;
                t.y += dy;
            }
            Item::Vineta { cx, cy, .. } => {
                *cx += dx;
                *cy += dy;
            }
        }
    }
}

struct Flotante {
    izq: bool,
    x0: i32,
    x1: i32,
    y0: i32,
    y1: i32,
}

struct Marcador {
    tipo: Lista,
    num: i32,
    /// x del borde izquierdo del contenido del elemento de lista.
    x: i32,
    nodo: usize,
}

#[derive(Clone, Copy)]
struct Met {
    asc: f32,
    desc: f32,
    esp: f32,
}

/// Medidas de una caja: márgenes (ya resueltos), relleno, borde y ancho de contenido.
struct Geom {
    ml: i32,
    mr: i32,
    mt: i32,
    mb: i32,
    pl: i32,
    pr: i32,
    pt: i32,
    pb: i32,
    bl: i32,
    br: i32,
    bt: i32,
    bb: i32,
    w: i32,
}

impl Geom {
    fn caja_w(&self) -> i32 {
        self.w + self.pl + self.pr + self.bl + self.br
    }
}

pub struct Resultado {
    pub items: Vec<Item>,
    pub enlaces: Vec<String>,
    /// Nodo de cada enlace (el 0 no tiene).
    pub enl_nodo: Vec<u32>,
    pub anclas: Vec<(String, i32)>,
    pub alto: i32,
}

pub struct Maq<'a> {
    dom: &'a Dom,
    est: &'a Estilos,
    imgs: &'a [Img],
    img_de: &'a [u32],
    forms: &'a Forms,
    f: &'a mut dyn Fuentes,
    enlace_de: Vec<u32>,
    enlaces: Vec<String>,
    enl_nodo: Vec<u32>,
    num_item: Vec<i32>,
    items: Vec<Item>,
    anclas: Vec<(String, i32)>,
    y: i32,
    pend: i32,
    flot: Vec<Flotante>,
    marcador: Option<Marcador>,
    memo: Vec<Option<(i32, i32)>>,
    met: Vec<Option<Met>>,
    lleno: bool,
}

pub(crate) fn px_i(x: f32) -> i32 {
    if x.is_nan() {
        0
    } else {
        iround(x.clamp(-(MAX_ANCHO as f32), MAX_ANCHO as f32))
    }
}

/// Longitud resuelta contra `base`; `None` si es `auto`.
fn largo(l: Len, base: i32) -> Option<i32> {
    match l {
        Len::Auto => None,
        Len::Px(x) => Some(px_i(x)),
        Len::Pct(p) => Some(px_i(base as f32 * p / 100.0)),
    }
}

/// Resuelve el enlace de un `href` contra la base; conserva el `#fragmento`.
fn resolver_enlace(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    let (sin, frag) = match href.split_once('#') {
        Some((a, b)) => (a, Some(b)),
        None => (href, None),
    };
    let mut r = crate::url::resolver(base, sin)?;
    if let Some(f) = frag {
        r.push('#');
        r.push_str(f);
    }
    Some(r)
}

impl<'a> Maq<'a> {
    pub fn nuevo(dom: &'a Dom, est: &'a Estilos, imgs: &'a [Img], img_de: &'a [u32], forms: &'a Forms, base: &str, f: &'a mut dyn Fuentes) -> Self {
        let n = dom.nodos.len();
        let mut enlaces = alloc::vec![String::new()]; // el 0 es «sin enlace»
        let mut enl_nodo = alloc::vec![0u32];
        let mut enlace_de = alloc::vec![0u32; n];
        let mut num_item = alloc::vec![0i32; n];
        let mut contador = alloc::vec![0i32; n];
        for i in 1..n {
            let p = dom.nodos[i].padre as usize;
            enlace_de[i] = enlace_de[p];
            if dom.etq(i) == "a" {
                if let Some(u) = dom.attr(i, "href").and_then(|h| resolver_enlace(base, h)) {
                    enlace_de[i] = enlaces.len() as u32;
                    enlaces.push(u);
                    enl_nodo.push(i as u32);
                }
            }
            if dom.elem(i).is_some() && est.de(i).display == Display::ListItem {
                if contador[p] == 0 {
                    contador[p] = dom.attr(p, "start").and_then(|s| s.trim().parse::<i32>().ok()).unwrap_or(1).clamp(-1_000_000, 1_000_000) - 1;
                }
                contador[p] += 1;
                num_item[i] = dom.attr(i, "value").and_then(|s| s.trim().parse::<i32>().ok()).map_or(contador[p], |v| {
                    contador[p] = v;
                    v
                });
            }
        }
        Maq {
            dom,
            est,
            imgs,
            img_de,
            forms,
            f,
            enlace_de,
            enlaces,
            enl_nodo,
            num_item,
            items: Vec::new(),
            anclas: Vec::new(),
            y: 0,
            pend: 0,
            flot: Vec::new(),
            marcador: None,
            memo: alloc::vec![None; n],
            met: alloc::vec![None; est.lista.len()],
            lleno: false,
        }
    }

    /// Maqueta el documento entero con `ancho` px de ancho.
    pub fn maquetar(mut self, ancho: i32) -> Resultado {
        let ancho = ancho.clamp(1, MAX_ANCHO);
        self.fluir(0, 0, ancho, 0);
        self.y += self.pend;
        let alto = self.y.max(self.fondo_flot(3)).clamp(0, MAX_ALTO);
        Resultado { items: self.items, enlaces: self.enlaces, enl_nodo: self.enl_nodo, anclas: self.anclas, alto }
    }

    fn push(&mut self, it: Item) -> usize {
        if self.items.len() >= MAX_CAJAS {
            self.lleno = true;
            return usize::MAX;
        }
        self.items.push(it);
        self.items.len() - 1
    }

    fn anclar(&mut self, n: usize, y: i32) {
        if self.anclas.len() >= MAX_ANCLAS {
            return;
        }
        let dom = self.dom;
        for a in ["id", "name"] {
            if let Some(v) = dom.attr(n, a).filter(|v| !v.is_empty()) {
                if a == "id" || dom.etq(n) == "a" {
                    self.anclas.push((v.to_string(), y));
                }
            }
        }
    }

    /// Métricas de la letra del estilo `i` (cacheadas).
    fn metricas(&mut self, i: usize) -> Met {
        if let Some(m) = self.met[i] {
            return m;
        }
        let e = &self.est.lista[i];
        let (l, px) = (e.letra(), e.px);
        let (asc, desc) = self.f.metricas(l, px);
        let esp = self.f.medir(l, px, " ");
        let m = Met { asc, desc, esp };
        self.met[i] = Some(m);
        m
    }

    /// Ancho en px (redondeado hacia arriba) de un texto con la letra del estilo `e`.
    fn medir(&mut self, e: &Estilo, t: &str) -> i32 {
        let w = self.f.medir(e.letra(), e.px, t);
        if w.is_nan() {
            0
        } else {
            nf_core::math::iround(w.clamp(0.0, MAX_ANCHO as f32))
        }
    }

    // -----------------------------------------------------------------------------------------------------
    // Flotantes

    /// Bajo del lado indicado (bit 0 izquierda, bit 1 derecha) de los flotantes activos.
    fn fondo_flot(&self, lados: u8) -> i32 {
        self.flot.iter().filter(|f| (f.izq && lados & 1 != 0) || (!f.izq && lados & 2 != 0)).map(|f| f.y1).max().unwrap_or(0)
    }

    /// Extremos libres de una línea que ocupa [y, y+h) dentro de [x0, x1].
    fn rango(&self, y: i32, h: i32, x0: i32, x1: i32) -> (i32, i32) {
        let (mut l, mut r) = (x0, x1);
        for f in &self.flot {
            if f.y1 > y && f.y0 < y + h.max(1) {
                if f.izq {
                    l = l.max(f.x1);
                } else {
                    r = r.min(f.x0);
                }
            }
        }
        (l, r.max(l))
    }

    /// Siguiente y mayor que `y` donde acaba un flotante que lo toca.
    fn siguiente_flot(&self, y: i32, h: i32) -> Option<i32> {
        self.flot.iter().filter(|f| f.y1 > y && f.y0 < y + h.max(1)).map(|f| f.y1).min()
    }

    /// Maqueta `n` (flotante) aparte y lo coloca junto al borde de [cx, cx+cw] a partir de `y0`.
    fn colocar_flotante(&mut self, n: usize, cx: i32, cw: i32, prof: u32) {
        let izq = self.est.de(n).flot == Flot::Izq;
        let y0 = self.y + self.pend;
        let (items, y, pend, flot, marc) = (
            core::mem::take(&mut self.items),
            self.y,
            self.pend,
            core::mem::take(&mut self.flot),
            self.marcador.take(),
        );
        self.y = 0;
        self.pend = 0;
        let (ox, ow) = self.bloque(n, 0, cw, prof + 1);
        let alto = self.y + self.pend;
        let mut local = core::mem::replace(&mut self.items, items);
        self.y = y;
        self.pend = pend;
        self.flot = flot;
        self.marcador = marc;
        let _ = ox;
        let mut fy = y0;
        let mut fx;
        for _ in 0..=self.flot.len() {
            let (l, r) = self.rango(fy, alto, cx, cx + cw);
            fx = if izq { l } else { (r - ow).max(l) };
            if r - l >= ow || self.siguiente_flot(fy, alto).is_none() {
                for it in &mut local {
                    it.mover(fx, fy);
                }
                for it in local {
                    self.push(it);
                }
                self.flot.retain(|f| f.y1 > fy);
                self.flot.push(Flotante { izq, x0: fx, x1: fx + ow, y0: fy, y1: fy + alto });
                return;
            }
            fy = self.siguiente_flot(fy, alto).unwrap_or(fy + 1);
        }
    }

    // -----------------------------------------------------------------------------------------------------
    // Cajas de bloque

    /// Márgenes horizontales: `auto` reparte lo que sobra. Devuelve (izquierdo, derecho).
    fn margenes_h(e: &Estilo, cw: i32, caja_w: i32) -> (i32, i32) {
        let (a, b) = (largo(e.margen[IZQ], cw), largo(e.margen[DER], cw));
        let resto = (cw - caja_w).max(0);
        match (a, b) {
            (None, None) => (resto / 2, resto - resto / 2),
            (None, Some(r)) => ((cw - caja_w - r).max(0), r),
            (Some(l), None) => (l, 0),
            (Some(l), Some(r)) => (l, r),
        }
    }

    fn geom(&mut self, n: usize, e: &Estilo, cw: i32, encoger: bool) -> Geom {
        let p = |l: Len| largo(l, cw).unwrap_or(0).max(0);
        let (pl, pr, pt, pb) = (p(e.relleno[IZQ]), p(e.relleno[DER]), p(e.relleno[ARR]), p(e.relleno[ABA]));
        let b = |i: usize| px_i(e.borde[i]).max(0);
        let (bl, br, bt, bb) = (b(IZQ), b(DER), b(ARR), b(ABA));
        let extras = pl + pr + bl + br;
        let (ml0, mr0) = (largo(e.margen[IZQ], cw).unwrap_or(0), largo(e.margen[DER], cw).unwrap_or(0));
        let restar = |x: i32| if e.caja_borde { (x - extras).max(0) } else { x.max(0) };
        let mut w = match largo(e.ancho, cw) {
            Some(x) => restar(x),
            None if encoger => {
                let (mn, mx) = self.mm_interior(n, 0);
                let disp = (cw - ml0 - mr0 - extras).max(0);
                mx.min(disp.max(mn))
            }
            None => (cw - ml0 - mr0 - extras).max(0),
        };
        if let Some(m) = largo(e.max_ancho, cw) {
            w = w.min(restar(m));
        }
        if let Some(m) = largo(e.min_ancho, cw) {
            w = w.max(restar(m));
        }
        w = w.clamp(0, MAX_ANCHO);
        let (ml, mr) = Self::margenes_h(e, cw, w + extras);
        Geom { ml, mr, mt: largo(e.margen[ARR], cw).unwrap_or(0), mb: largo(e.margen[ABA], cw).unwrap_or(0), pl, pr, pt, pb, bl, br, bt, bb, w }
    }

    /// Margen superior que sube hasta fuera de `n` (el suyo y los de sus primeros hijos en cadena).
    fn primer_margen(&self, n: usize, cw: i32) -> i32 {
        let (dom, est) = (self.dom, self.est);
        let mt = |e: &Estilo| largo(e.margen[ARR], cw).unwrap_or(0);
        let mut m = mt(est.de(n));
        let mut cur = n;
        'sube: for _ in 0..MAX_PROF {
            let e = est.de(cur);
            if cur != n && (largo(e.relleno[ARR], cw).unwrap_or(0) > 0 || e.borde[ARR] > 0.0 || e.recorta) {
                break;
            }
            for &h in &dom.nodos[cur].hijos {
                let h = h as usize;
                match &dom.nodos[h].dato {
                    Dato::Texto(t) if t.trim().is_empty() => continue,
                    Dato::Elem(_) => {
                        let eh = est.de(h);
                        if eh.display == Display::None || eh.flot != Flot::No {
                            continue;
                        }
                        if matches!(eh.display, Display::Block | Display::ListItem | Display::Table) {
                            m = m.max(mt(eh));
                            if eh.display == Display::Table {
                                break 'sube;
                            }
                            cur = h;
                            continue 'sube;
                        }
                        break 'sube;
                    }
                    _ => break 'sube,
                }
            }
            break;
        }
        m
    }

    /// Maqueta un elemento de bloque en el flujo, dentro de [cx, cx+cw]. Devuelve (margen izquierdo,
    /// ancho exterior), que usan los flotantes y las cajas en línea.
    fn bloque(&mut self, n: usize, cx: i32, cw: i32, prof: u32) -> (i32, i32) {
        let (dom, est) = (self.dom, self.est);
        let e = est.de(n);
        if self.lleno || self.y > MAX_ALTO {
            self.lleno = true;
            return (0, 0);
        }
        if e.display == Display::Table && prof < MAX_PROF {
            return self.tabla(n, cx, cw, prof);
        }
        let aislado = e.flot != Flot::No || e.display == Display::InlineBlock;
        let bfc = aislado || e.recorta;
        let g = self.geom(n, e, cw, aislado);
        if e.despeja != 0 {
            let b = self.fondo_flot(e.despeja);
            if b > self.y + self.pend {
                self.y = b;
                self.pend = 0;
            }
        }
        let x = cx + g.ml;
        let colapsa_arr = !bfc && g.bt == 0 && g.pt == 0;
        if colapsa_arr {
            let m = self.primer_margen(n, cw);
            self.pend = self.pend.max(m);
        } else {
            self.pend = self.pend.max(g.mt);
            self.y += self.pend;
            self.pend = 0;
        }
        let top = self.y + self.pend;
        let bw = g.caja_w();
        self.anclar(n, top);
        let borde = [g.bt, g.br, g.bb, g.bl];
        let ic = if e.fondo.is_some() || borde.iter().any(|&b| b > 0) {
            Some(self.push(Item::Caja { r: Rect::new(x, top, bw, 0), fondo: e.fondo, borde, color: e.borde_color }))
        } else {
            None
        };
        self.y += g.bt + g.pt;
        let y_cont0 = top + g.bt + g.pt;
        let guardados = if bfc { Some(core::mem::take(&mut self.flot)) } else { None };
        if e.display == Display::ListItem && e.lista != Lista::Ninguno {
            if let Some(ant) = self.marcador.take() {
                let y = self.y + self.pend;
                self.poner_marcador(&ant, y + (est.de(ant.nodo).px * 0.9) as i32);
            }
            self.marcador = Some(Marcador { tipo: e.lista, num: self.num_item[n], x: x + g.bl + g.pl, nodo: n });
        }
        self.fluir(n, x + g.bl + g.pl, g.w, prof + 1);
        if let Some(m) = self.marcador.take_if(|m| m.nodo == n) {
            let y = self.y + self.pend;
            self.poner_marcador(&m, y + (e.px * 0.9) as i32);
        }
        let h_def = largo(e.alto, cw).map(|h| if e.caja_borde { (h - g.pt - g.pb - g.bt - g.bb).max(0) } else { h });
        let h_min = largo(e.min_alto, cw);
        if bfc || g.pb + g.bb > 0 || h_def.is_some() || h_min.is_some() {
            self.y += self.pend;
            self.pend = 0;
            if bfc {
                self.y = self.y.max(self.fondo_flot(3));
            }
            for h in [h_def, h_min].into_iter().flatten() {
                self.y = self.y.max(y_cont0 + h.clamp(0, MAX_ALTO));
            }
            self.y += g.pb + g.bb;
            self.pend = g.mb;
        } else {
            self.pend = self.pend.max(g.mb);
        }
        if let Some(f) = guardados {
            self.flot = f;
        }
        if let Some(i) = ic.filter(|&i| i != usize::MAX) {
            if let Item::Caja { r, .. } = &mut self.items[i] {
                r.h = (self.y - top).max(0);
            }
        }
        let _ = dom;
        (g.ml, g.ml + bw + g.mr)
    }

    /// Viñeta o número de un elemento de lista, con la línea base en `base`.
    fn poner_marcador(&mut self, m: &Marcador, base: i32) {
        let e = self.est.de(m.nodo);
        let px = e.px;
        let color = e.color;
        match m.tipo {
            Lista::Decimal => {
                let mut s = alloc::format!("{}.", m.num);
                s.push(' ');
                let w = self.medir(e, &s);
                let (a, d) = (self.metricas(self.est.idx[m.nodo] as usize).asc, self.metricas(self.est.idx[m.nodo] as usize).desc);
                self.push(Item::Texto(Texto {
                    x: m.x - w,
                    y: base,
                    w,
                    asc: px_i(a),
                    desc: px_i(d),
                    letra: e.letra(),
                    px,
                    color,
                    deco: 0,
                    enlace: 0,
                    etiq: 0,
                    s,
                }));
            }
            tipo => {
                let r = px_i((px * 0.17).max(2.0));
                self.push(Item::Vineta { cx: m.x - px_i(px * 0.6) - r, cy: base - px_i(px * 0.3), r, tipo, color });
            }
        }
    }
}
