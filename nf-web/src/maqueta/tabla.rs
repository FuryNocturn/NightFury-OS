// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Anchos mínimos y máximos de contenido (para cajas que se encogen) y tablas.
use super::*;
use crate::estilo::Blanco;

/// Columnas máximas de una tabla; las celdas más allá se ignoran.
const MAX_COLS: usize = 1000;

#[derive(Default)]
struct Acum {
    min: i32,
    max: i32,
    linea: i32,
    espacio: bool,
}

impl Acum {
    fn cerrar(&mut self) {
        self.max = self.max.max(self.linea);
        self.linea = 0;
        self.espacio = false;
    }
}

struct Celda {
    nodo: usize,
    col: usize,
    span: usize,
    mn: i32,
    mx: i32,
}

struct Fila {
    nodo: usize,
    celdas: Vec<Celda>,
}

struct Medidas {
    filas: Vec<Fila>,
    captions: Vec<usize>,
    min: Vec<i32>,
    max: Vec<i32>,
}

fn blanco(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0C)
}

/// Reparte `libre` px entre columnas: de sus mínimos a sus máximos, y lo que sobre en proporción al máximo.
fn repartir(min: &[i32], max: &[i32], libre: i32) -> Vec<i32> {
    let (smin, smax): (i64, i64) = (min.iter().map(|&x| x as i64).sum(), max.iter().map(|&x| x as i64).sum());
    let libre = libre.max(0) as i64;
    (0..min.len())
        .map(|i| {
            let (a, b) = (min[i] as i64, max[i].max(min[i]) as i64);
            let w = if libre <= smin {
                a
            } else if libre < smax {
                a + (b - a) * (libre - smin) / (smax - smin).max(1)
            } else if smax > 0 {
                b + (libre - smax) * b / smax
            } else {
                libre / min.len().max(1) as i64
            };
            w.clamp(0, MAX_ANCHO as i64) as i32
        })
        .collect()
}

impl<'a> Maq<'a> {
    /// Anchos mínimo y máximo del contenido de `n` (sin su relleno, borde ni margen).
    pub(super) fn mm_interior(&mut self, n: usize, prof: u32) -> (i32, i32) {
        if prof > 200 {
            return (0, 0);
        }
        if self.est.de(n).display == Display::Table {
            let m = self.medidas_tabla(n, prof);
            let s = px_i(self.est.de(n).espacio);
            let sp = s.saturating_mul(m.min.len() as i32 + 1);
            return (m.min.iter().sum::<i32>().saturating_add(sp), m.max.iter().sum::<i32>().saturating_add(sp));
        }
        let mut a = Acum::default();
        self.mm_flujo(n, &mut a, prof);
        a.cerrar();
        (a.min, a.max)
    }

    /// Lo que suman a lo ancho el relleno, el borde y los márgenes fijos de `e`.
    fn extras_h(e: &Estilo, margenes: bool) -> i32 {
        let f = |l: Len| if let Len::Px(x) = l { px_i(x).max(0) } else { 0 };
        let mut r = f(e.relleno[IZQ]) + f(e.relleno[DER]) + px_i(e.borde[IZQ]).max(0) + px_i(e.borde[DER]).max(0);
        if margenes {
            let m = |l: Len| if let Len::Px(x) = l { px_i(x) } else { 0 };
            r += m(e.margen[IZQ]) + m(e.margen[DER]);
        }
        r
    }

    /// (mínimo, máximo) de la caja de `n` contando relleno, borde y márgenes. Cacheado.
    pub(super) fn mm(&mut self, n: usize, prof: u32) -> (i32, i32) {
        if let Some(r) = self.memo[n] {
            return r;
        }
        let e = self.est.de(n);
        let ext = Self::extras_h(e, false);
        let (mut a, mut b) = if self.dom.etq(n) == "img" {
            let w = self.tam_img(n, MAX_ANCHO).0;
            (w, w)
        } else {
            self.mm_interior(n, prof)
        };
        let resta = |x: i32| if e.caja_borde { (x - ext).max(0) } else { x.max(0) };
        if let Len::Px(x) = e.ancho {
            (a, b) = (resta(px_i(x)), resta(px_i(x)));
        }
        if let Len::Px(m) = e.max_ancho {
            (a, b) = (a.min(resta(px_i(m))), b.min(resta(px_i(m))));
        }
        if let Len::Px(m) = e.min_ancho {
            (a, b) = (a.max(resta(px_i(m))), b.max(resta(px_i(m))));
        }
        let mar = Self::extras_h(e, true) - ext;
        let r = (a.saturating_add(ext + mar).max(0), b.saturating_add(ext + mar).max(0));
        self.memo[n] = Some(r);
        r
    }

    fn mm_texto(&mut self, e: &Estilo, nodo: usize, t: &str, a: &mut Acum) {
        let met = self.metricas(self.est.idx[nodo] as usize);
        let esp = nf_core::math::ceil(met.esp.clamp(0.0, 1000.0)) as i32;
        match e.blanco {
            Blanco::Pre => {
                for linea in t.split('\n') {
                    let w = self.medir(e, linea.trim_end_matches('\r'));
                    a.min = a.min.max(w);
                    a.linea = a.linea.saturating_add(w);
                    a.cerrar();
                }
            }
            b => {
                let bytes = t.as_bytes();
                let mut i = 0;
                let mut acum_nobr = 0i32;
                while i < bytes.len() {
                    let bl = blanco(bytes[i]);
                    let mut j = i;
                    while j < bytes.len() && blanco(bytes[j]) == bl {
                        j += 1;
                    }
                    let w = if bl {
                        if a.espacio {
                            0
                        } else {
                            a.espacio = true;
                            esp
                        }
                    } else {
                        a.espacio = false;
                        self.medir(e, &t[i..j])
                    };
                    a.linea = a.linea.saturating_add(w);
                    acum_nobr = acum_nobr.saturating_add(w);
                    if !bl && b != Blanco::NoWrap {
                        a.min = a.min.max(w);
                    }
                    i = j;
                }
                if b == Blanco::NoWrap {
                    a.min = a.min.max(acum_nobr);
                }
            }
        }
    }

    fn mm_flujo(&mut self, n: usize, a: &mut Acum, prof: u32) {
        let (dom, est) = (self.dom, self.est);
        if prof > 200 {
            return;
        }
        for &h in &dom.nodos[n].hijos {
            let h = h as usize;
            let el = match &dom.nodos[h].dato {
                Dato::Texto(t) => {
                    self.mm_texto(est.de(h), h, t, a);
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
                a.cerrar();
                continue;
            }
            let k = self.forms.ctl_de[h];
            if k != SIN_CTL {
                let w = self.tam_control(h, k, MAX_ANCHO).0.saturating_add(Self::extras_h(e, true) - Self::extras_h(e, false));
                a.min = a.min.max(w);
                a.linea = a.linea.saturating_add(w);
                a.espacio = false;
            } else if el.etq == "img" || (e.display == Display::InlineBlock && e.flot == Flot::No) {
                let (mn, mx) = self.mm(h, prof + 1);
                a.min = a.min.max(mn);
                a.linea = a.linea.saturating_add(mx);
                a.espacio = false;
            } else if e.display == Display::Inline && e.flot == Flot::No {
                let ext = Self::extras_h(e, true);
                a.linea = a.linea.saturating_add(ext);
                self.mm_flujo(h, a, prof + 1);
            } else {
                a.cerrar();
                let (mn, mx) = self.mm(h, prof + 1);
                a.min = a.min.max(mn);
                a.max = a.max.max(mx);
            }
        }
    }

    fn medidas_tabla(&mut self, n: usize, prof: u32) -> Medidas {
        let (dom, est) = (self.dom, self.est);
        let mut m = Medidas { filas: Vec::new(), captions: Vec::new(), min: Vec::new(), max: Vec::new() };
        let mut sueltas: Vec<usize> = Vec::new();
        let mut filas_nodo: Vec<usize> = Vec::new();
        let hijos = |x: usize| dom.nodos[x].hijos.iter().map(|&h| h as usize).filter(|&h| dom.elem(h).is_some());
        for c in hijos(n) {
            match est.de(c).display {
                Display::TableRow => filas_nodo.push(c),
                Display::TableRowGroup => {
                    for r in hijos(c) {
                        match est.de(r).display {
                            Display::TableRow => filas_nodo.push(r),
                            Display::TableCell => sueltas.push(r),
                            _ => {}
                        }
                    }
                }
                Display::TableCaption => m.captions.push(c),
                Display::TableCell => sueltas.push(c),
                _ => {}
            }
        }
        // celdas sueltas (sin <tr>): forman una fila propia
        let mut todas: Vec<(usize, Vec<usize>)> = filas_nodo.iter().map(|&r| (r, hijos(r).filter(|&c| est.de(c).display == Display::TableCell).collect())).collect();
        if !sueltas.is_empty() {
            todas.push((n, sueltas));
        }
        let mut ncols = 0usize;
        for (fnodo, celdas) in todas {
            let mut fila = Fila { nodo: fnodo, celdas: Vec::new() };
            let mut col = 0usize;
            for c in celdas {
                let span = dom.attr(c, "colspan").and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, MAX_COLS);
                if col >= MAX_COLS {
                    break;
                }
                let span = span.min(MAX_COLS - col);
                let (mn, mx) = self.mm_celda(c, prof);
                fila.celdas.push(Celda { nodo: c, col, span, mn, mx });
                col += span;
            }
            ncols = ncols.max(col);
            m.filas.push(fila);
        }
        m.min = alloc::vec![0; ncols];
        m.max = alloc::vec![0; ncols];
        for f in &m.filas {
            for c in f.celdas.iter().filter(|c| c.span == 1) {
                m.min[c.col] = m.min[c.col].max(c.mn);
                m.max[c.col] = m.max[c.col].max(c.mx);
            }
        }
        let s = px_i(est.de(n).espacio);
        for f in &m.filas {
            for c in f.celdas.iter().filter(|c| c.span > 1) {
                let sp = s * (c.span as i32 - 1);
                for (necesito, v) in [(c.mn, &mut m.min), (c.mx, &mut m.max)] {
                    let tiene: i32 = v[c.col..c.col + c.span].iter().sum::<i32>() + sp;
                    if tiene < necesito {
                        let extra = (necesito - tiene) / c.span as i32 + 1;
                        for x in &mut v[c.col..c.col + c.span] {
                            *x += extra;
                        }
                    }
                }
            }
        }
        for i in 0..ncols {
            m.max[i] = m.max[i].max(m.min[i]);
        }
        m
    }

    /// (mínimo, máximo) de una celda con su relleno y borde; el `width` fijo vale de preferido. Cacheado.
    fn mm_celda(&mut self, c: usize, prof: u32) -> (i32, i32) {
        if let Some(r) = self.memo[c] {
            return r;
        }
        let e = self.est.de(c);
        let ext = Self::extras_h(e, false);
        let (imn, imx) = self.mm_interior(c, prof + 1);
        let pref = match e.ancho {
            Len::Px(x) => Some(if e.caja_borde { (px_i(x) - ext).max(0) } else { px_i(x).max(0) }),
            _ => None,
        };
        let mx = pref.map_or(imx, |p| p.max(imn));
        let r = (imn.saturating_add(ext), mx.max(imn).saturating_add(ext));
        self.memo[c] = Some(r);
        r
    }

    /// Maqueta una celda en (x, y) con `w` de ancho de caja. Devuelve su alto y el índice de su fondo.
    fn celda(&mut self, c: usize, x: i32, y: i32, w: i32, prof: u32) -> (i32, Option<usize>) {
        let e = self.est.de(c);
        let p = |l: Len| largo(l, w).unwrap_or(0).max(0);
        let (pl, pr, pt, pb) = (p(e.relleno[IZQ]), p(e.relleno[DER]), p(e.relleno[ARR]), p(e.relleno[ABA]));
        let b = |i: usize| px_i(e.borde[i]).max(0);
        let (bl, br, bt, bb) = (b(IZQ), b(DER), b(ARR), b(ABA));
        let ic = if e.fondo.is_some() || bl + br + bt + bb > 0 {
            Some(self.push(Item::Caja { r: Rect::new(x, y, w, 0), fondo: e.fondo, borde: [bt, br, bb, bl], color: e.borde_color })).filter(|&i| i != usize::MAX)
        } else {
            None
        };
        let (sy, sp, sf, sm) = (self.y, self.pend, core::mem::take(&mut self.flot), self.marcador.take());
        self.anclar(c, y);
        self.y = y + bt + pt;
        self.pend = 0;
        self.fluir(c, x + bl + pl, (w - pl - pr - bl - br).max(0), prof + 1);
        self.y += self.pend;
        self.y = self.y.max(self.fondo_flot(3));
        let mut h = self.y + pb + bb - y;
        if let Some(a) = largo(e.alto, w) {
            h = h.max(a);
        }
        self.y = sy;
        self.pend = sp;
        self.flot = sf;
        self.marcador = sm;
        (h, ic)
    }

    fn poner_alto(&mut self, ic: Option<usize>, h: i32) {
        if let Some(Item::Caja { r, .. }) = ic.and_then(|i| self.items.get_mut(i)) {
            r.h = h;
        }
    }

    /// Maqueta una tabla (`display: table`). Devuelve (margen izquierdo, ancho exterior).
    pub(super) fn tabla(&mut self, n: usize, cx: i32, cw: i32, prof: u32) -> (i32, i32) {
        let e = self.est.de(n);
        let t = self.medidas_tabla(n, prof);
        let nc = t.min.len();
        let s = px_i(e.espacio).max(0);
        let sp_tot = s.saturating_mul(nc as i32 + 1);
        let p = |l: Len| largo(l, cw).unwrap_or(0).max(0);
        let (pl, pr, pt, pb) = (p(e.relleno[IZQ]), p(e.relleno[DER]), p(e.relleno[ARR]), p(e.relleno[ABA]));
        let b = |i: usize| px_i(e.borde[i]).max(0);
        let (bl, br, bt, bb) = (b(IZQ), b(DER), b(ARR), b(ABA));
        let ext = pl + pr + bl + br;
        let (ml0, mr0) = (largo(e.margen[IZQ], cw).unwrap_or(0), largo(e.margen[DER], cw).unwrap_or(0));
        let disp = (cw - ml0 - mr0 - ext).max(0);
        let (smin, smax): (i32, i32) = (t.min.iter().sum(), t.max.iter().sum());
        let minimo = smin.saturating_add(sp_tot);
        let wt = match largo(e.ancho, cw) {
            Some(x) => (if e.caja_borde { x - ext } else { x }).max(minimo),
            None => smax.saturating_add(sp_tot).min(disp).max(minimo),
        }
        .clamp(0, MAX_ANCHO);
        let cols = repartir(&t.min, &t.max, wt - sp_tot);
        let (ml, mr) = Self::margenes_h(e, cw, wt + ext);
        if e.despeja != 0 {
            let b = self.fondo_flot(e.despeja);
            if b > self.y + self.pend {
                self.y = b;
                self.pend = 0;
            }
        }
        let mt = largo(e.margen[ARR], cw).unwrap_or(0);
        self.pend = self.pend.max(mt);
        self.y += self.pend;
        self.pend = 0;
        let (x, top) = (cx + ml, self.y);
        self.anclar(n, top);
        let ic = if e.fondo.is_some() || bl + br + bt + bb > 0 {
            Some(self.push(Item::Caja { r: Rect::new(x, top, wt + ext, 0), fondo: e.fondo, borde: [bt, br, bb, bl], color: e.borde_color })).filter(|&i| i != usize::MAX)
        } else {
            None
        };
        let x0 = x + bl + pl;
        self.y += bt + pt;
        for c in &t.captions {
            self.bloque(*c, x0, wt, prof + 1);
        }
        self.y += self.pend;
        self.pend = 0;
        self.y += s;
        for f in &t.filas {
            if self.lleno {
                break;
            }
            let y_f = self.y;
            let fe = if f.nodo != n { Some(self.est.de(f.nodo)) } else { None };
            let ic_f = fe.filter(|fe| fe.fondo.is_some()).and_then(|fe| {
                let ancho: i32 = cols.iter().sum::<i32>() + s * (nc as i32 - 1).max(0);
                Some(self.push(Item::Caja { r: Rect::new(x0 + s, y_f, ancho, 0), fondo: fe.fondo, borde: [0; 4], color: [0; 4] })).filter(|&i| i != usize::MAX)
            });
            let mut alto = 0;
            let mut cajas: Vec<Option<usize>> = Vec::new();
            for c in &f.celdas {
                let xc = x0 + s + cols[..c.col].iter().sum::<i32>() + s * c.col as i32;
                let wc = cols[c.col..c.col + c.span].iter().sum::<i32>() + s * (c.span as i32 - 1);
                let (h, ic) = self.celda(c.nodo, xc, y_f, wc, prof);
                alto = alto.max(h);
                cajas.push(ic);
            }
            for ic in cajas {
                self.poner_alto(ic, alto);
            }
            self.poner_alto(ic_f, alto);
            self.y = y_f + alto + s;
            if self.y > MAX_ALTO {
                self.lleno = true;
            }
        }
        self.y += pb + bb;
        self.poner_alto(ic, (self.y - top).max(0));
        self.pend = largo(e.margen[ABA], cw).unwrap_or(0);
        (ml, ml + wt + ext + mr)
    }
}
