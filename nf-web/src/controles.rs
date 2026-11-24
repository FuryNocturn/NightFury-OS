// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Los controles de formulario en la página: clics, teclado, foco y pintado (estilo Windows 10).
use crate::forms::{alto_linea, Control, Envio, Tipo, SIN_CTL};
use crate::maqueta::Item;
use crate::{Documento, Familia, Fuentes, Letra};
use alloc::string::String;
use alloc::vec::Vec;
use nf_core::geom::Rect;
use nf_core::surface::Surface;

const BORDE: u32 = 0x7A7A7A;
const AZUL: u32 = 0x0078D7;
const TINTA_OFF: u32 = 0x6D6D6D;
/// Alto de cada opción de la lista desplegable y opciones que se ven a la vez.
const ALTO_OPCION: i32 = 20;
const OPCIONES_VISIBLES: usize = 12;

/// Lo que tiene el foco de teclado en la página.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Foco {
    Ctl(usize),
    /// Índice en la lista de enlaces del maquetado.
    Enlace(u32),
}

/// Qué ha pasado tras un clic o una tecla.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Efecto {
    /// No era para los controles: el programa hace lo suyo (seguir un enlace, desplazar, seleccionar).
    Ignorado,
    /// Algo ha cambiado: hay que dibujar.
    Cambio,
    Enviar(Envio),
    /// Enter sobre un enlace con el foco: URL absoluta.
    Enlace(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tecla {
    Car(char),
    Borrar,
    Suprimir,
    Izq,
    Der,
    Arriba,
    Abajo,
    Inicio,
    Fin,
    Enter,
    /// Con `mayus`, hacia atrás.
    Tab,
    Escape,
    /// Ctrl+A.
    Todo,
}

/// Un control hallado bajo el puntero: índice, caja (coordenadas del documento) y su letra.
struct Pieza {
    k: usize,
    r: Rect,
    px: f32,
    letra: Letra,
}

fn recortado(s: &mut Surface, r: Rect, g: impl FnOnce(&mut Surface)) {
    let old = s.clip();
    s.set_clip(old.intersect(r));
    g(s);
    s.set_clip(old);
}

/// Lo que se ve en un campo (la contraseña, como puntos).
fn mostrado(c: &Control) -> String {
    if c.tipo == Tipo::Password {
        c.valor.chars().map(|_| '•').collect()
    } else {
        c.valor.clone()
    }
}

/// Primeras `n` letras.
fn prefijo(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or(s, |(b, _)| &s[..b])
}

/// Dónde se pinta el texto de un campo o un área: caja interior, alto de línea, primera línea visible y
/// cuánto se corre el texto a la izquierda para que se vea el cursor.
struct GeoTexto {
    dentro: Rect,
    lh: i32,
    linea0: usize,
    desp: i32,
}

fn geo_texto(c: &Control, r: Rect, px: f32, letra: Letra, f: &mut dyn Fuentes) -> GeoTexto {
    let dentro = Rect::new(r.x + 4, r.y + 3, (r.w - 8).max(0), (r.h - 6).max(0));
    let lh = alto_linea(px);
    let filas = (dentro.h / lh).max(1) as usize;
    let (l, col) = c.linea_cursor();
    let linea0 = if c.tipo == Tipo::Area { (l + 1).saturating_sub(filas) } else { 0 };
    let m = mostrado(c);
    let linea = m.split('\n').nth(l).unwrap_or("");
    let w = f.medir(letra, px, prefijo(linea, col));
    let desp = if w.is_finite() { (w as i32 - dentro.w + 2).max(0) } else { 0 };
    GeoTexto { dentro, lh, linea0, desp }
}

impl Documento {
    // ---------------------------------------------------------------------------------------------
    // Consultas

    pub(crate) fn caja_de(&self, k: usize) -> Option<Rect> {
        self.res.as_ref()?.items.iter().find_map(|it| match it {
            Item::Control { r, ctl, .. } | Item::Imagen { r, ctl, .. } if *ctl as usize == k => Some(*r),
            _ => None,
        })
    }

    fn pieza_en(&self, x: i32, y: i32) -> Option<Pieza> {
        self.res.as_ref()?.items.iter().rev().find_map(|it| match it {
            Item::Control { r, ctl, px, letra, .. } if r.contains(x, y) => Some(Pieza { k: *ctl as usize, r: *r, px: *px, letra: *letra }),
            Item::Imagen { r, ctl, .. } if *ctl != SIN_CTL && r.contains(x, y) => Some(Pieza { k: *ctl as usize, r: *r, px: 16.0, letra: Letra { familia: Familia::Sans, negrita: false, cursiva: false } }),
            _ => None,
        })
    }

    /// Control al que lleva el `<label>` bajo el punto.
    fn etiqueta_en(&self, x: i32, y: i32) -> Option<usize> {
        self.res.as_ref()?.items.iter().rev().find_map(|it| match it {
            Item::Texto(t) if t.etiq != 0 && t.x <= x && x < t.x + t.w && t.y - t.asc <= y && y < t.y + t.desc => Some(t.etiq as usize - 1),
            _ => None,
        })
    }

    /// Hay algo con el foco de teclado (control o enlace).
    pub fn hay_foco(&self) -> bool {
        self.foco.is_some()
    }

    /// El foco está en un campo de texto, un área o una lista: las flechas y las letras son suyas.
    pub fn foco_texto(&self) -> bool {
        matches!(self.foco, Some(Foco::Ctl(k)) if self.forms.ctl.get(k).is_some_and(|c| c.es_texto() || c.tipo == Tipo::Select))
    }

    pub fn quitar_foco(&mut self) {
        self.foco = None;
        self.abierto = None;
    }

    /// Caja (coordenadas del documento) de lo que tiene el foco, para llevarlo a la vista.
    pub fn rect_foco(&self) -> Option<Rect> {
        match self.foco? {
            Foco::Ctl(k) => self.caja_de(k),
            Foco::Enlace(e) => self.caja_enlace(e),
        }
    }

    pub(crate) fn caja_enlace(&self, e: u32) -> Option<Rect> {
        let mut u: Option<Rect> = None;
        for it in &self.res.as_ref()?.items {
            let r = match it {
                Item::Texto(t) if t.enlace == e => Rect::new(t.x, t.y - t.asc, t.w, t.asc + t.desc),
                Item::Imagen { r, enlace, .. } if *enlace == e => *r,
                _ => continue,
            };
            u = Some(match u {
                None => r,
                Some(a) => {
                    let (x, y) = (a.x.min(r.x), a.y.min(r.y));
                    Rect::new(x, y, a.right().max(r.right()) - x, a.bottom().max(r.bottom()) - y)
                }
            });
        }
        u
    }

    /// Lo que se puede enfocar con Tab, en el orden del documento: controles activos y enlaces visibles.
    fn orden_foco(&self) -> Vec<Foco> {
        let Some(res) = &self.res else { return Vec::new() };
        let (mut ctl, mut enl) = (alloc::vec![false; self.forms.ctl.len()], alloc::vec![false; res.enlaces.len()]);
        for it in &res.items {
            let (c, e) = match it {
                Item::Control { ctl, .. } => (*ctl, 0),
                Item::Imagen { ctl, enlace, .. } => (*ctl, *enlace),
                Item::Texto(t) => (SIN_CTL, t.enlace),
                _ => continue,
            };
            if let Some(b) = ctl.get_mut(c as usize) {
                *b = true;
            }
            if let Some(b) = enl.get_mut(e as usize) {
                *b = e != 0;
            }
        }
        let mut v: Vec<(u32, Foco)> = Vec::new();
        for (k, c) in self.forms.ctl.iter().enumerate() {
            if ctl[k] && !c.deshab {
                v.push((c.nodo as u32, Foco::Ctl(k)));
            }
        }
        for (e, _) in enl.iter().enumerate().filter(|(_, &b)| b) {
            v.push((res.enl_nodo.get(e).copied().unwrap_or(0), Foco::Enlace(e as u32)));
        }
        v.sort_by_key(|&(n, _)| n);
        v.into_iter().map(|(_, f)| f).collect()
    }

    fn tab(&mut self, atras: bool) -> Efecto {
        let o = self.orden_foco();
        self.abierto = None;
        let pos = self.foco.and_then(|f| o.iter().position(|&x| x == f));
        let sig = match (pos, atras) {
            (None, false) => Some(0),
            (None, true) => o.len().checked_sub(1),
            (Some(p), false) => Some(p + 1),
            (Some(p), true) => p.checked_sub(1),
        };
        self.foco = sig.and_then(|i| o.get(i).copied());
        if let Some(Foco::Ctl(k)) = self.foco {
            let c = &mut self.forms.ctl[k];
            c.ancla = None;
        }
        // Sin más a donde ir, el foco sale de la página (lo recoge el programa).
        if self.foco.is_some() { Efecto::Cambio } else { Efecto::Ignorado }
    }

    // ---------------------------------------------------------------------------------------------
    // Ratón

    /// Opciones visibles de la lista de `k` y su caja (bajo `r`).
    fn geo_lista(&self, k: usize, r: Rect) -> (Rect, usize, usize) {
        let c = &self.forms.ctl[k];
        let n = c.opciones.len().min(OPCIONES_VISIBLES);
        let ini = if c.sel >= OPCIONES_VISIBLES { c.sel + 1 - OPCIONES_VISIBLES } else { 0 };
        (Rect::new(r.x, r.bottom(), r.w.max(40), n as i32 * ALTO_OPCION + 2), ini, n)
    }

    /// Clic en (`x`, `y`) del documento. Los controles lo toman; si no, `Ignorado` y se pierde el foco.
    pub fn pulsar(&mut self, x: i32, y: i32, f: &mut dyn Fuentes) -> Efecto {
        if let Some(k) = self.abierto.take() {
            if let Some(r) = self.caja_de(k) {
                let (pr, ini, n) = self.geo_lista(k, r);
                if pr.contains(x, y) {
                    let i = ini + ((y - pr.y - 1).max(0) / ALTO_OPCION) as usize;
                    let c = &mut self.forms.ctl[k];
                    if i < ini + n && c.opciones.get(i).is_some_and(|o| !o.deshab) {
                        c.sel = i;
                    }
                    return Efecto::Cambio;
                }
            }
            if self.pieza_en(x, y).is_some_and(|p| p.k == k) {
                return Efecto::Cambio;
            }
        }
        if let Some(p) = self.pieza_en(x, y) {
            let pos = self.forms.ctl.get(p.k).is_some_and(|c| c.es_texto()).then(|| self.pos_en_texto(p.k, &p, x, y, f));
            return self.activar(p.k, pos, ((x - p.r.x).max(0), (y - p.r.y).max(0)));
        }
        if let Some(k) = self.etiqueta_en(x, y) {
            return self.activar(k, None, (0, 0));
        }
        self.foco = None;
        Efecto::Ignorado
    }

    /// Pulsa el control `k`: `pos` es la letra donde cae el clic en un campo; `xy`, el punto del clic dentro de
    /// una imagen que envía.
    fn activar(&mut self, k: usize, pos: Option<usize>, xy: (i32, i32)) -> Efecto {
        let Some(c) = self.forms.ctl.get(k) else { return Efecto::Cambio };
        let (tipo, form, vacia) = (c.tipo, c.form, c.opciones.is_empty());
        if c.deshab || tipo == Tipo::Oculto {
            return Efecto::Cambio;
        }
        self.foco = Some(Foco::Ctl(k));
        match tipo {
            Tipo::Texto | Tipo::Password | Tipo::Area => {
                if let Some(p) = pos {
                    self.forms.ctl[k].mover(p, false);
                }
            }
            Tipo::Casilla => self.forms.ctl[k].marcado ^= true,
            Tipo::Radio => self.forms.marcar_radio(k),
            Tipo::Select if !vacia => self.abierto = Some(k),
            Tipo::Restablecer => {
                if let Some(fm) = form {
                    self.forms.restablecer(fm);
                }
            }
            Tipo::Enviar | Tipo::Imagen => {
                if let Some(fm) = form {
                    if let Some(e) = self.forms.envio(&self.dom, &self.base, &self.doc_url, fm, Some((k, xy.0, xy.1))) {
                        return Efecto::Enviar(e);
                    }
                }
            }
            _ => {}
        }
        Efecto::Cambio
    }

    /// Letra del campo `k` más cercana al punto (como se pinta: con el desplazamiento del texto).
    fn pos_en_texto(&self, k: usize, p: &Pieza, x: i32, y: i32, f: &mut dyn Fuentes) -> usize {
        let c = &self.forms.ctl[k];
        let g = geo_texto(c, p.r, p.px, p.letra, f);
        let st = c.inicios();
        let fila = ((y - g.dentro.y) / g.lh).max(0) as usize;
        let l = (g.linea0 + fila).min(st.len() - 1);
        let m = mostrado(c);
        let linea = m.split('\n').nth(l).unwrap_or("");
        let meta = (x - g.dentro.x + g.desp) as f32;
        let (mut acc, mut col) = (0.0f32, 0);
        let mut b = [0u8; 4];
        for ch in linea.chars() {
            let w = f.medir(p.letra, p.px, ch.encode_utf8(&mut b));
            let w = if w.is_finite() { w } else { 0.0 };
            if acc + w / 2.0 >= meta {
                break;
            }
            acc += w;
            col += 1;
        }
        st[l] + col
    }

    // ---------------------------------------------------------------------------------------------
    // Teclado

    fn envio_de(&self, k: usize) -> Efecto {
        let Some(fm) = self.forms.ctl[k].form else { return Efecto::Cambio };
        match self.forms.envio_implicito(&self.dom, &self.base, &self.doc_url, fm) {
            Some(e) => Efecto::Enviar(e),
            None => Efecto::Cambio,
        }
    }

    /// Una tecla para lo que tiene el foco. `mayus` extiende la selección (o va hacia atrás con Tab).
    pub fn tecla(&mut self, t: Tecla, mayus: bool) -> Efecto {
        match t {
            Tecla::Tab => return self.tab(mayus),
            Tecla::Escape => {
                let hubo = self.abierto.take().is_some() || self.foco.take().is_some();
                return if hubo { Efecto::Cambio } else { Efecto::Ignorado };
            }
            _ => {}
        }
        let k = match self.foco {
            Some(Foco::Ctl(k)) if k < self.forms.ctl.len() => k,
            Some(Foco::Enlace(e)) if t == Tecla::Enter => {
                return self.res.as_ref().and_then(|r| r.enlaces.get(e as usize)).cloned().map_or(Efecto::Ignorado, Efecto::Enlace);
            }
            _ => return Efecto::Ignorado,
        };
        let tipo = self.forms.ctl[k].tipo;
        if self.abierto == Some(k) {
            match t {
                Tecla::Arriba | Tecla::Abajo => self.mover_opcion(k, t == Tecla::Abajo),
                Tecla::Enter | Tecla::Car(' ') => self.abierto = None,
                _ => {}
            }
            return Efecto::Cambio;
        }
        let pulsa = matches!(t, Tecla::Car(' '));
        match (tipo, t) {
            (Tipo::Texto | Tipo::Password | Tipo::Area, _) => {
                let c = &mut self.forms.ctl[k];
                match t {
                    Tecla::Car(ch) => c.insertar(ch),
                    Tecla::Borrar => c.borrar(true),
                    Tecla::Suprimir => c.borrar(false),
                    Tecla::Izq => {
                        let d = match c.seleccion() {
                            Some((a, _)) if !mayus => a,
                            _ => c.cur.saturating_sub(1),
                        };
                        c.mover(d, mayus);
                    }
                    Tecla::Der => {
                        let d = match c.seleccion() {
                            Some((_, b)) if !mayus => b,
                            _ => c.cur + 1,
                        };
                        c.mover(d, mayus);
                    }
                    Tecla::Inicio => c.a_extremo(false, mayus),
                    Tecla::Fin => c.a_extremo(true, mayus),
                    Tecla::Arriba | Tecla::Abajo if tipo == Tipo::Area => c.mover_linea(if t == Tecla::Abajo { 1 } else { -1 }, mayus),
                    Tecla::Todo => {
                        c.ancla = Some(0);
                        c.cur = c.n();
                    }
                    Tecla::Enter if tipo == Tipo::Area => c.insertar('\n'),
                    Tecla::Enter => return self.envio_de(k),
                    _ => return Efecto::Ignorado,
                }
            }
            (Tipo::Casilla, _) if pulsa => self.forms.ctl[k].marcado ^= true,
            (Tipo::Radio, _) if pulsa => self.forms.marcar_radio(k),
            (Tipo::Casilla | Tipo::Radio, Tecla::Enter) => return self.envio_de(k),
            (Tipo::Enviar | Tipo::Imagen | Tipo::Restablecer | Tipo::Boton, Tecla::Enter | Tecla::Car(' ')) => return self.activar(k, None, (0, 0)),
            (Tipo::Select, Tecla::Arriba | Tecla::Abajo) => self.mover_opcion(k, t == Tecla::Abajo),
            (Tipo::Select, Tecla::Enter | Tecla::Car(' ')) if !self.forms.ctl[k].opciones.is_empty() => self.abierto = Some(k),
            _ => return Efecto::Ignorado,
        }
        Efecto::Cambio
    }

    /// Pasa a la opción anterior o siguiente que no esté desactivada.
    fn mover_opcion(&mut self, k: usize, abajo: bool) {
        let c = &mut self.forms.ctl[k];
        let ops = &c.opciones;
        let cand: Vec<usize> = if abajo { (c.sel + 1..ops.len()).collect() } else { (0..c.sel).rev().collect() };
        if let Some(i) = cand.into_iter().find(|&i| !ops[i].deshab) {
            c.sel = i;
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Pintado

    /// Pinta el control `k` en `r` (coordenadas de la superficie).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn pintar_control(&self, s: &mut Surface, f: &mut dyn Fuentes, r: Rect, k: usize, px: f32, letra: Letra, color: u32, fondo: Option<u32>) {
        let Some(c) = self.forms.ctl.get(k) else { return };
        let foco = self.foco == Some(Foco::Ctl(k));
        let tinta = if c.deshab { TINTA_OFF } else { color };
        let borde = if foco { AZUL } else { BORDE };
        let blanco = fondo.unwrap_or(if c.deshab || c.lectura { 0xF0F0F0 } else { 0xFFFFFF });
        let (asc, desc) = f.metricas(letra, px);
        let base_centrada = |r: Rect| r.y + (r.h - (asc + desc) as i32) / 2 + asc as i32;
        match c.tipo {
            Tipo::Oculto => {}
            Tipo::Casilla => {
                s.fill_rect(r, blanco);
                s.frame(r, if foco { AZUL } else { 0x333333 });
                if c.marcado {
                    let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
                    s.stroke(&[(x + w * 0.22, y + h * 0.52), (x + w * 0.42, y + h * 0.74), (x + w * 0.8, y + h * 0.25)], 1.8, tinta);
                }
            }
            Tipo::Radio => {
                let (cx, cy, rad) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0, r.w.min(r.h) as f32 / 2.0);
                s.fill_circle(cx, cy, rad, if foco { AZUL } else { 0x333333 }, 255);
                s.fill_circle(cx, cy, rad - 1.0, blanco, 255);
                if c.marcado {
                    s.fill_circle(cx, cy, rad * 0.45, tinta, 255);
                }
            }
            Tipo::Enviar | Tipo::Restablecer | Tipo::Boton | Tipo::Imagen => {
                s.fill_rect(r, fondo.unwrap_or(if c.deshab { 0xCCCCCC } else { 0xE1E1E1 }));
                s.frame(r, if foco { AZUL } else if c.deshab { 0xBFBFBF } else { 0xADADAD });
                let w = f.medir(letra, px, &c.etiqueta);
                let x = r.x + ((r.w - if w.is_finite() { w as i32 } else { 0 }) / 2).max(3);
                recortado(s, r.inflate(-2), |s| f.dibujar(s, letra, px, x as f32, base_centrada(r) as f32, if c.deshab { 0x838383 } else { color }, &c.etiqueta));
            }
            Tipo::Select => {
                s.fill_rect(r, blanco);
                s.frame(r, borde);
                let dentro = Rect::new(r.x + 4, r.y + 2, (r.w - 24).max(0), (r.h - 4).max(0));
                if let Some(o) = c.opciones.get(c.sel) {
                    recortado(s, dentro, |s| f.dibujar(s, letra, px, dentro.x as f32, base_centrada(r) as f32, tinta, &o.etiqueta));
                }
                let (cx, cy) = (r.right() as f32 - 11.0, r.y as f32 + r.h as f32 / 2.0);
                s.stroke(&[(cx - 4.0, cy - 2.0), (cx, cy + 2.0), (cx + 4.0, cy - 2.0)], 1.5, 0x333333);
            }
            Tipo::Texto | Tipo::Password | Tipo::Area => {
                s.fill_rect(r, blanco);
                s.frame(r, borde);
                let g = geo_texto(c, r, px, letra, f);
                let m = mostrado(c);
                let (inicios, sel) = (c.inicios(), c.seleccion());
                let (cl, _) = c.linea_cursor();
                recortado(s, g.dentro, |s| {
                    if m.is_empty() && !foco && !c.marcador.is_empty() {
                        f.dibujar(s, letra, px, g.dentro.x as f32, (g.dentro.y + (g.dentro.h - g.lh) / 2 + asc as i32) as f32, 0x757575, &c.marcador);
                    }
                    let filas = (g.dentro.h / g.lh).max(1) as usize;
                    let un_renglon = c.tipo != Tipo::Area;
                    for (i, linea) in m.split('\n').enumerate().skip(g.linea0).take(filas) {
                        let top = if un_renglon { g.dentro.y + (g.dentro.h - g.lh) / 2 } else { g.dentro.y + (i - g.linea0) as i32 * g.lh };
                        let x0 = g.dentro.x - g.desp;
                        if let Some((a, b)) = sel {
                            let ini = inicios[i];
                            let (a, b) = (a.max(ini), b.min(ini + linea.chars().count()));
                            if a < b {
                                let (a, b) = (a - ini, b - ini);
                                let (xa, xb) = (f.medir(letra, px, prefijo(linea, a)), f.medir(letra, px, prefijo(linea, b)));
                                if xa.is_finite() && xb.is_finite() {
                                    s.blend_rect(Rect::new(x0 + xa as i32, top, (xb - xa) as i32, g.lh), AZUL, 90);
                                }
                            }
                        }
                        f.dibujar(s, letra, px, x0 as f32, (top + (g.lh - (asc + desc) as i32) / 2 + asc as i32) as f32, tinta, linea);
                        if foco && i == cl {
                            let (_, col) = c.linea_cursor();
                            let xc = f.medir(letra, px, prefijo(linea, col));
                            let xc = if xc.is_finite() { xc as i32 } else { 0 };
                            s.fill_rect(Rect::new(x0 + xc, top, 1, g.lh), tinta);
                        }
                    }
                });
            }
        }
    }

    /// La lista desplegable abierta, encima de todo (`r` = caja del select en la superficie).
    pub(crate) fn pintar_lista(&self, s: &mut Surface, f: &mut dyn Fuentes, r: Rect, k: usize, px: f32, letra: Letra) {
        let Some(c) = self.forms.ctl.get(k) else { return };
        let (pr, ini, n) = self.geo_lista(k, r);
        s.fill_rect(pr, 0xFFFFFF);
        s.frame(pr, BORDE);
        let (asc, desc) = f.metricas(letra, px);
        for (j, o) in c.opciones.iter().enumerate().skip(ini).take(n) {
            let fila = Rect::new(pr.x + 1, pr.y + 1 + (j - ini) as i32 * ALTO_OPCION, pr.w - 2, ALTO_OPCION);
            let marcada = j == c.sel;
            if marcada {
                s.fill_rect(fila, AZUL);
            }
            let tinta = if marcada { 0xFFFFFF } else if o.deshab { 0x838383 } else { 0x000000 };
            recortado(s, fila, |s| f.dibujar(s, letra, px, (fila.x + 3) as f32, (fila.y + (fila.h - (asc + desc) as i32) / 2 + asc as i32) as f32, tinta, &o.etiqueta));
        }
    }
}
