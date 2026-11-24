// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Navegador de NightFury: páginas web (HTTP y HTTPS) y archivos del disco con pestañas, sin JavaScript.
//! Programa en modo usuario (ver `main.rs`); la lógica vive aquí para probarla y fotografiarla en el PC.
//! El motor de páginas es `nf-web`; `pestana` lleva el historial y las descargas, `entrada` lo escrito en la
//! barra y las reglas de seguridad, y este archivo la ventana.
#![cfg_attr(not(test), no_std)]
extern crate alloc;

mod campo;
mod cookies;
mod dibujo;
mod entrada;
mod fuentes;
mod paginas;
mod pestana;
#[cfg(test)]
mod pruebas;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use campo::{Accion, Campo};
use dibujo::{caja, capsula, icono, Ico};
use entrada::{clasificar, es_ruta_local, local_de_url, sin_fragmento, Entrada};
use fuentes::Letras;
use nf_core::apps::{App, AppKind, Ctx, MouseEv};
use nf_core::dialogs;
use nf_core::font;
use nf_core::geom::Rect;
use nf_core::input::{Key, KeyEvent};
use nf_core::platform::Platform;
use nf_core::scrollbar::{Metrics, ScrollBar, SB_W};
use nf_core::surface::Surface;
use nf_core::theme::Theme;
use nf_web::{Efecto, Envio, Familia, Fuentes, Letra, Metodo, Tecla};
use pestana::{ajustar_imagen, Pestana, Seguridad, Vista, MARGEN_IMG};

const TAB_H: i32 = 36;
const TOOL_H: i32 = 42;
const TS: u32 = 10;
const TS_S: u32 = 9;
const MAX_PESTANAS: usize = 24;
const TIRA: u32 = 0xDEE1E6;
const LINEA: u32 = 0xDADCE0;
const TINTA: u32 = 0x202124;
const APAGADO: u32 = 0x5F6368;
const GRIS: u32 = 0x80868B;
const ROJO: u32 = 0xC5221F;
const VERDE: u32 = 0x188038;
const FONDO_IMG: u32 = 0x202124;
/// Lo que se desplaza la página con cada muesca de la rueda y con las flechas.
const PASO_RUEDA: i32 = 60;
const PASO_FLECHA: i32 = 40;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Foco {
    Pagina,
    Barra,
}

/// Pregunta que espera respuesta bajo la barra (en rojo), antes de enviar un formulario.
enum Aviso {
    /// Contraseña que viajaría sin cifrar.
    Password(Envio),
    /// Atrás, adelante o recargar volverían a enviar un formulario: el paso (-1, 1 o 0).
    Reenviar(i32),
}

const AVISO_H: i32 = 52;

fn rect_aviso(p: Rect) -> Rect {
    Rect::new(p.x, p.y, p.w - SB_W, AVISO_H)
}
fn rect_aviso_si(r: Rect) -> Rect {
    Rect::new(r.right() - 196, r.y + 12, 90, 28)
}
fn rect_aviso_no(r: Rect) -> Rect {
    Rect::new(r.right() - 100, r.y + 12, 90, 28)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hit {
    Nada,
    Pestana(usize),
    Cerrar(usize),
    Mas,
    Atras,
    Adelante,
    Recargar,
    Barra,
    Pagina,
}

fn ancho_pestana(n: usize, w: i32) -> i32 {
    ((w - 8 - 44) / n.max(1) as i32).clamp(60, 220)
}
fn rect_pestana(i: usize, n: usize, w: i32) -> Rect {
    Rect::new(8 + i as i32 * ancho_pestana(n, w), 6, ancho_pestana(n, w), TAB_H - 6)
}
fn rect_mas(n: usize, w: i32) -> Rect {
    Rect::new(8 + n as i32 * ancho_pestana(n, w) + 4, 6, 28, 28)
}
fn rect_cerrar(r: Rect) -> Rect {
    Rect::new(r.right() - 26, r.y + (r.h - 18) / 2, 18, 18)
}
fn rect_boton(k: i32) -> Rect {
    Rect::new(8 + k * 36, TAB_H + 5, 32, 32)
}
fn rect_omnibox(w: i32) -> Rect {
    Rect::new(120, TAB_H + 6, (w - 120 - 12).max(40), 30)
}
/// Caja de búsqueda de la página de inicio.
fn rect_caja(p: Rect) -> Rect {
    let w = (p.w - 80).clamp(120, 584);
    Rect::new(p.x + (p.w - w) / 2, p.y + p.h * 45 / 100, w, 46)
}

pub struct Navegador {
    pest: Vec<Pestana>,
    act: usize,
    sig_id: u32,
    barra: Campo,
    foco: Foco,
    letras: Letras,
    size: (i32, i32),
    ctrl: bool,
    hover: Hit,
    sb: ScrollBar,
    /// URL del enlace bajo el ratón.
    enlace: Option<String>,
    /// Pulsación en la página (en coordenadas de la página) y si ya se ha arrastrado.
    pulsado: Option<(i32, i32)>,
    arrastro: bool,
    /// Último texto copiado con Ctrl+C (el sistema aún no tiene portapapeles de texto).
    copiado: String,
    lienzo: Surface,
    /// Último reloj visto (ms), para animar el indicador de carga.
    reloj: u64,
    aviso: Option<Aviso>,
}

/// App lista para `nf_app::programa!`.
pub fn crear() -> Box<dyn App> {
    Box::new(Navegador::nuevo())
}

fn con_clip(s: &mut Surface, r: Rect, f: impl FnOnce(&mut Surface)) {
    let old = s.clip();
    s.set_clip(old.intersect(r));
    f(s);
    s.set_clip(old);
}

impl Navegador {
    fn nuevo() -> Navegador {
        Navegador {
            pest: alloc::vec![Pestana::nueva(0)],
            act: 0,
            sig_id: 1,
            barra: Campo::default(),
            foco: Foco::Barra,
            letras: Letras::nuevas(),
            size: (1024, 700),
            ctrl: false,
            hover: Hit::Nada,
            sb: ScrollBar::new(false),
            enlace: None,
            pulsado: None,
            arrastro: false,
            copiado: String::new(),
            lienzo: Surface::empty(),
            reloj: 0,
            aviso: None,
        }
    }

    fn pagina(&self) -> Rect {
        Rect::new(0, TAB_H + TOOL_H, self.size.0, (self.size.1 - TAB_H - TOOL_H).max(1))
    }

    fn metricas(&self) -> Metrics {
        let p = self.pagina();
        Metrics { rect: Rect::new(p.right() - SB_W, p.y, SB_W, p.h), total: self.pest[self.act].alto, page: p.h, pos: self.pest[self.act].scroll, line: PASO_FLECHA }
    }

    fn hit(&self, x: i32, y: i32) -> Hit {
        let (w, n) = (self.size.0, self.pest.len());
        if y < TAB_H {
            if rect_mas(n, w).contains(x, y) {
                return Hit::Mas;
            }
            return (0..n).find_map(|i| {
                let r = rect_pestana(i, n, w);
                r.contains(x, y).then(|| if rect_cerrar(r).contains(x, y) { Hit::Cerrar(i) } else { Hit::Pestana(i) })
            }).unwrap_or(Hit::Nada);
        }
        if y < TAB_H + TOOL_H {
            return if rect_boton(0).contains(x, y) {
                Hit::Atras
            } else if rect_boton(1).contains(x, y) {
                Hit::Adelante
            } else if rect_boton(2).contains(x, y) {
                Hit::Recargar
            } else if rect_omnibox(w).contains(x, y) {
                Hit::Barra
            } else {
                Hit::Nada
            };
        }
        Hit::Pagina
    }

    // ------------------------------------------------------------ pestañas

    fn nueva(&mut self, sys: &mut dyn Platform, e: Option<Entrada>, activar: bool, junto: bool, ahora: u64) {
        if self.pest.len() >= MAX_PESTANAS {
            return;
        }
        let pos = if junto { self.act + 1 } else { self.pest.len() };
        self.pest.insert(pos, Pestana::nueva(self.sig_id));
        self.sig_id += 1;
        if let Some(e) = e {
            self.pest[pos].ir(sys, e, ahora);
        }
        if activar {
            self.activar(pos);
        }
    }

    fn activar(&mut self, i: usize) {
        self.act = i;
        self.aviso = None;
        self.sb = ScrollBar::new(false);
        self.enlace = None;
        self.pulsado = None;
        if self.pest[i].url_visible().is_empty() {
            self.barra.poner("");
            self.foco = Foco::Barra;
        } else {
            self.foco = Foco::Pagina;
        }
    }

    fn cerrar(&mut self, i: usize, cx: &mut Ctx) {
        self.pest[i].cancelar(cx.sys);
        self.pest.remove(i);
        if self.pest.is_empty() {
            // Cerrar la última pestaña cierra la ventana.
            cx.close = true;
            self.pest.push(Pestana::nueva(self.sig_id));
            self.sig_id += 1;
            self.act = 0;
        } else if i < self.act {
            self.act -= 1;
        } else if i == self.act {
            self.activar(i.min(self.pest.len() - 1));
        }
    }

    fn enfocar_barra(&mut self) {
        let u = String::from(self.pest[self.act].url_visible());
        self.barra.poner(&u);
        self.barra.seleccionar_todo();
        self.foco = Foco::Barra;
    }

    fn ir_a(&mut self, texto: &str, cx: &mut Ctx) {
        if let Some(e) = clasificar(texto) {
            self.aviso = None;
            self.pest[self.act].ir(cx.sys, e, cx.now_ms);
            self.foco = Foco::Pagina;
            self.enlace = None;
        }
    }

    fn abrir_enlace(&mut self, url: &str, nueva: bool, cx: &mut Ctx) {
        let t = &self.pest[self.act];
        let (sin, frag) = sin_fragmento(url);
        if let (false, Some(f), Vista::Documento(d)) = (nueva, frag, &t.vista) {
            // Misma página con `#fragmento`: solo se desplaza.
            if nf_web::url::resolver(&t.base, sin) == nf_web::url::resolver(&t.base, &t.base) {
                if let Some(y) = if f.is_empty() { Some(0) } else { d.ancla(f) } {
                    self.desplazar_a(y);
                }
                return;
            }
        }
        let e = if t.local { local_de_url(sin).map(Entrada::Local).unwrap_or_else(|| Entrada::Web(String::from(url))) } else { Entrada::Web(String::from(url)) };
        let local = matches!(e, Entrada::Local(_));
        if nueva {
            self.nueva(cx.sys, Some(e), false, true, cx.now_ms);
            return;
        }
        self.pest[self.act].ir(cx.sys, e, cx.now_ms);
        if local {
            self.pest[self.act].ancla = frag.filter(|f| !f.is_empty()).map(String::from);
        }
        self.enlace = None;
    }

    // ------------------------------------------------------------ formularios

    /// Atrás (-1), adelante (1) o recargar (0); si volvería a enviar un formulario, antes se pregunta.
    fn mover(&mut self, d: i32, cx: &mut Ctx) {
        let a = self.act;
        self.aviso = None;
        if self.pest[a].pide_post(d) {
            self.aviso = Some(Aviso::Reenviar(d));
            return;
        }
        self.hacer_mover(d, cx);
    }

    fn hacer_mover(&mut self, d: i32, cx: &mut Ctx) {
        let (a, ahora) = (self.act, cx.now_ms);
        match d {
            0 => self.pest[a].recargar(cx.sys, ahora),
            d if d < 0 => self.pest[a].atras(cx.sys, ahora),
            _ => self.pest[a].adelante(cx.sys, ahora),
        }
    }

    /// Lo que han hecho los controles de la página: enviar, seguir un enlace con Enter.
    fn aplicar(&mut self, ef: Efecto, cx: &mut Ctx) {
        match ef {
            Efecto::Enviar(e) => self.enviar(e, cx),
            Efecto::Enlace(u) => self.abrir_enlace(&u, false, cx),
            Efecto::Cambio | Efecto::Ignorado => {}
        }
    }

    fn enviar(&mut self, e: Envio, cx: &mut Ctx) {
        let (a, ahora) = (self.act, cx.now_ms);
        self.enlace = None;
        // Una página del disco que envía a otra del disco: se abre el archivo (sin consulta).
        if let (true, Metodo::Get, Some(ruta)) = (self.pest[a].local, e.metodo, local_de_url(&e.url)) {
            self.aviso = None;
            self.pest[a].ir(cx.sys, Entrada::Local(ruta), ahora);
            return;
        }
        if e.tiene_password && e.url.starts_with("http://") {
            self.aviso = Some(Aviso::Password(e));
            return;
        }
        self.aviso = None;
        self.pest[a].enviar(cx.sys, e, ahora);
        self.foco = Foco::Pagina;
    }

    /// Respuesta a la pregunta de la barra roja.
    fn responder(&mut self, si: bool, cx: &mut Ctx) {
        match self.aviso.take() {
            Some(Aviso::Password(e)) if si => {
                let a = self.act;
                self.pest[a].enviar(cx.sys, e, cx.now_ms);
                self.foco = Foco::Pagina;
            }
            Some(Aviso::Reenviar(d)) if si => self.hacer_mover(d, cx),
            _ => {}
        }
    }

    /// Teclas para el foco de la página (campos, botones, enlaces). `true` si las ha tomado.
    fn tecla_doc(&mut self, ev: &KeyEvent, cx: &mut Ctx) -> bool {
        let a = self.act;
        let Vista::Documento(d) = &mut self.pest[a].vista else { return false };
        let t = match ev.key {
            Key::Tab => Tecla::Tab,
            Key::Enter => Tecla::Enter,
            Key::Backspace => Tecla::Borrar,
            Key::Delete => Tecla::Suprimir,
            Key::Left => Tecla::Izq,
            Key::Right => Tecla::Der,
            Key::Up => Tecla::Arriba,
            Key::Down => Tecla::Abajo,
            Key::Home => Tecla::Inicio,
            Key::End => Tecla::Fin,
            Key::Char('a') if ev.ctrl => Tecla::Todo,
            _ => match ev.ch {
                Some(c) if !ev.ctrl && !ev.alt && !c.is_control() => Tecla::Car(c),
                _ => return false,
            },
        };
        // Con un botón o un enlace enfocado, solo Tab, Enter y espacio son suyos; el resto desplaza la página.
        if t != Tecla::Tab && (!d.hay_foco() || !(d.foco_texto() || matches!(t, Tecla::Enter | Tecla::Car(' ')))) {
            return false;
        }
        let ef = d.tecla(t, ev.shift);
        let r = d.rect_foco();
        if ef == Efecto::Ignorado {
            // Tab sin más controles: el foco pasa a la barra de direcciones
            if t == Tecla::Tab && !ev.shift {
                self.enfocar_barra();
                return true;
            }
            return false;
        }
        if let Some(r) = r {
            let (sc, h) = (self.pest[a].scroll, self.pagina().h);
            if r.y < sc {
                self.desplazar_a(r.y - 20);
            } else if r.bottom() > sc + h {
                self.desplazar_a(r.bottom() - h + 20);
            }
        }
        self.aplicar(ef, cx);
        true
    }

    fn dibujar_aviso(&self, s: &mut Surface, p: Rect) {
        let Some(av) = &self.aviso else { return };
        let r = rect_aviso(p);
        s.fill_rect(r, 0xFCE8E6);
        s.fill_rect(Rect::new(r.x, r.bottom() - 1, r.w, 1), ROJO);
        let (l1, l2, si) = match av {
            Aviso::Password(_) => ("Este formulario envía una contraseña sin cifrar.", "¿Enviar igualmente?", "Enviar"),
            Aviso::Reenviar(_) => ("¿Volver a enviar el formulario?", "Se repetirán los datos enviados antes a esta página.", "Reenviar"),
        };
        let ancho = (rect_aviso_si(r).x - r.x - 24).max(0);
        font::draw(s, r.x + 14, r.y + 8, &dialogs::fit(l1, TS, ancho), TS, true, ROJO);
        font::draw(s, r.x + 14, r.y + 8 + font::line_h(TS) + 2, &dialogs::fit(l2, TS_S, ancho), TS_S, false, TINTA);
        for (b, t, tinta, fondo) in [(rect_aviso_si(r), si, 0xFFFFFF, ROJO), (rect_aviso_no(r), "Cancelar", TINTA, 0xFFFFFF)] {
            capsula(s, b, if fondo == ROJO { ROJO } else { LINEA });
            capsula(s, b.inflate(-1), fondo);
            font::draw_in(s, b, t, TS, false, tinta, font::Align::Center);
        }
    }

    // ------------------------------------------------------------ desplazamiento y selección

    fn max_scroll(&self) -> i32 {
        (self.pest[self.act].alto - self.pagina().h).max(0)
    }

    fn desplazar_a(&mut self, y: i32) {
        let m = self.max_scroll();
        let a = self.act;
        self.pest[a].desplazar_a(y.clamp(0, m));
    }

    fn desplazar(&mut self, d: i32) {
        let y = self.pest[self.act].scroll + d;
        self.desplazar_a(y);
    }

    fn copiar(&mut self) {
        if let (Some((a, b)), Vista::Documento(d)) = (self.pest[self.act].sel, &self.pest[self.act].vista) {
            self.copiado = d.texto_en(a.0, a.1, b.0, b.1);
        }
    }

    /// Pasa un evento a la barra de desplazamiento; `true` si lo ha tomado ella.
    fn evento_barra(&mut self, ev: MouseEv, ahora: u64) -> bool {
        let m = self.metricas();
        let e = self.sb.mouse(ev, &m, ahora);
        if let Some(p) = e.pos {
            self.desplazar_a(p);
        }
        e.captured
    }

    fn enlace_en(&self, x: i32, y: i32) -> Option<String> {
        let (p, t) = (self.pagina(), &self.pest[self.act]);
        match &t.vista {
            Vista::Documento(d) if x < p.w - SB_W && y >= p.y => d.enlace_en(x, y - p.y + t.scroll),
            _ => None,
        }
    }

    // ------------------------------------------------------------ dibujo

    fn dibujar_pestanas(&self, s: &mut Surface, theme: &Theme) {
        let (w, n) = (self.size.0, self.pest.len());
        s.fill_rect(Rect::new(0, 0, w, TAB_H), TIRA);
        let ang = self.giro() as f32;
        for (i, t) in self.pest.iter().enumerate() {
            let r = rect_pestana(i, n, w);
            let activa = i == self.act;
            let sobre = matches!(self.hover, Hit::Pestana(k) | Hit::Cerrar(k) if k == i);
            if activa {
                caja(s, Rect::new(r.x, r.y, r.w, r.h + 1), 0xFFFFFF);
            } else if sobre {
                caja(s, Rect::new(r.x + 2, r.y + 2, r.w - 4, r.h - 4), 0xE8EAED);
            } else if i + 1 < n && i + 1 != self.act {
                s.fill_rect(Rect::new(r.right() - 1, r.y + 8, 1, r.h - 16), 0xA0A4AA);
            }
            let (cx, cy) = ((r.x + 16) as f32, (r.y + r.h / 2) as f32);
            if t.cargando() || t.pendiente.is_some() {
                s.arc(cx, cy, 6.0, ang, ang + 270.0, 2.0, theme.accent);
            } else {
                icono(s, Ico::Globo, cx, cy, GRIS);
            }
            let ancho = (r.w - 30 - 28).max(0);
            let txt = dialogs::fit(&t.titulo_visible(), TS, ancho);
            con_clip(s, Rect::new(r.x, r.y, r.w - 26, r.h), |s| {
                font::draw(s, r.x + 30, r.y + (r.h - font::line_h(TS)) / 2, &txt, TS, false, if activa { TINTA } else { APAGADO });
            });
            let c = rect_cerrar(r);
            if self.hover == Hit::Cerrar(i) {
                s.fill_circle((c.x + 9) as f32, (c.y + 9) as f32, 9.0, LINEA, 255);
            }
            icono(s, Ico::Cerrar, (c.x + 9) as f32, (c.y + 9) as f32, APAGADO);
        }
        let m = rect_mas(n, w);
        if self.hover == Hit::Mas {
            s.fill_circle((m.x + 14) as f32, (m.y + 14) as f32, 13.0, 0xCDD0D5, 255);
        }
        icono(s, Ico::Mas, (m.x + 14) as f32, (m.y + 14) as f32, APAGADO);
    }

    /// Ángulo del indicador de carga, según el reloj.
    fn giro(&self) -> i32 {
        (self.reloj / 3 % 360) as i32
    }

    fn dibujar_barra(&self, s: &mut Surface, theme: &Theme) {
        let w = self.size.0;
        let t = &self.pest[self.act];
        s.fill_rect(Rect::new(0, TAB_H, w, TOOL_H), 0xFFFFFF);
        s.fill_rect(Rect::new(0, TAB_H + TOOL_H - 1, w, 1), LINEA);
        let botones = [(0, Ico::Atras, t.puede_atras(), Hit::Atras), (1, Ico::Adelante, t.puede_adelante(), Hit::Adelante), (2, if t.cargando() || t.pendiente.is_some() { Ico::Detener } else { Ico::Recargar }, true, Hit::Recargar)];
        for (k, ico, activo, hit) in botones {
            let r = rect_boton(k);
            let (cx, cy) = ((r.x + 16) as f32, (r.y + 16) as f32);
            if self.hover == hit && activo {
                s.fill_circle(cx, cy, 16.0, 0xE8EAED, 255);
            }
            icono(s, ico, cx, cy, if activo { APAGADO } else { 0xBDC1C6 });
        }
        let r = rect_omnibox(w);
        let editando = self.foco == Foco::Barra;
        capsula(s, r, if editando { theme.accent } else { 0xF1F3F4 });
        if editando {
            capsula(s, r.inflate(-2), 0xFFFFFF);
        }
        let lh = font::line_h(TS);
        let ty = r.y + (r.h - lh) / 2;
        con_clip(s, r.inflate(-2), |s| {
            if editando {
                let (x0, ancho) = (r.x + 16, r.w - 32);
                let (txt, cur, wv) = self.barra.visible(ancho, TS);
                if self.barra.todo {
                    s.blend_rect(Rect::new(x0, ty, wv.max(1), lh), theme.accent, 70);
                }
                font::draw(s, x0, ty, &txt, TS, false, TINTA);
                if !self.barra.todo {
                    s.fill_rect(Rect::new(x0 + cur, ty + 2, 1, lh - 4), TINTA);
                }
                return;
            }
            let u = t.url_visible();
            let mut x0 = r.x + 16;
            match t.seguridad() {
                Seguridad::Segura => {
                    icono(s, Ico::Candado, (r.x + 20) as f32, (r.y + 15) as f32, VERDE);
                    x0 = r.x + 38;
                }
                Seguridad::NoSegura => {
                    icono(s, Ico::Aviso, (r.x + 20) as f32, (r.y + 15) as f32, ROJO);
                    x0 = font::draw(s, r.x + 34, ty, "No seguro", TS, false, ROJO) + 8;
                    s.fill_rect(Rect::new(x0, r.y + 8, 1, r.h - 16), LINEA);
                    x0 += 8;
                }
                Seguridad::Archivo => {
                    x0 = font::draw(s, x0, ty, "Archivo", TS, false, APAGADO) + 8;
                    s.fill_rect(Rect::new(x0, r.y + 8, 1, r.h - 16), LINEA);
                    x0 += 8;
                }
                Seguridad::Ninguna => {}
            }
            if u.is_empty() {
                font::draw(s, x0, ty, "Busca o escribe una dirección", TS, false, GRIS);
            } else {
                font::draw(s, x0, ty, &dialogs::fit(u, TS, (r.right() - 16 - x0).max(0)), TS, false, TINTA);
            }
        });
    }

    fn dibujar_inicio(&mut self, s: &mut Surface, theme: &Theme, p: Rect, focused: bool) {
        s.fill_rect(p, 0xFFFFFF);
        let px = 56.0;
        let l = Letra { familia: Familia::Sans, negrita: true, cursiva: false };
        let (wn, wf) = (self.letras.medir(l, px, "Night"), self.letras.medir(l, px, "Fury"));
        let x0 = p.x as f32 + (p.w as f32 - wn - wf) / 2.0;
        let base = (p.y + p.h * 45 / 100 - 56) as f32;
        self.letras.dibujar(s, l, px, x0, base, TINTA, "Night");
        self.letras.dibujar(s, l, px, x0 + wn, base, theme.accent, "Fury");
        let r = rect_caja(p);
        let caja_foco = self.foco == Foco::Pagina && focused;
        capsula(s, r, if caja_foco { theme.accent } else { 0xDFE1E5 });
        capsula(s, r.inflate(-if caja_foco { 2 } else { 1 }), 0xFFFFFF);
        icono(s, Ico::Lupa, (r.x + 26) as f32, (r.y + 22) as f32, GRIS);
        let c = &self.pest[self.act].caja;
        let (x0, ancho, lh) = (r.x + 48, r.w - 72, font::line_h(TS + 2));
        let ty = r.y + (r.h - lh) / 2;
        if c.vacio() {
            font::draw(s, x0, ty, "Buscar en la web", TS + 2, false, GRIS);
        }
        let (txt, cur, _) = c.visible(ancho, TS + 2);
        font::draw(s, x0, ty, &txt, TS + 2, false, TINTA);
        if caja_foco {
            s.fill_rect(Rect::new(x0 + cur, ty + 2, 1, lh - 4), TINTA);
        }
        font::draw_in(s, Rect::new(p.x, r.bottom() + 14, p.w, 16), "Las búsquedas se hacen en DuckDuckGo", TS_S, false, GRIS, font::Align::Center);
        // Favoritos (0.12.5): el hueco queda debajo de la caja.
    }

    fn dibujar_pagina(&mut self, s: &mut Surface, theme: &Theme, focused: bool) {
        let p = self.pagina();
        let w = (p.w - SB_W).max(1);
        let a = self.act;
        match &self.pest[a].vista {
            Vista::Inicio => return self.dibujar_inicio(s, theme, p, focused),
            Vista::Documento(d) => {
                let t = &self.pest[a];
                let fondo = d.fondo();
                if self.lienzo.w != w || self.lienzo.h != p.h {
                    self.lienzo = Surface::new(w, p.h, fondo);
                } else {
                    self.lienzo.fill_rect(Rect::new(0, 0, w, p.h), fondo);
                }
                d.pintar(&mut self.lienzo, t.scroll, &mut self.letras);
                if let Some((i, f)) = t.sel {
                    for r in d.rects_seleccion(i.0, i.1, f.0, f.1) {
                        self.lienzo.blend_rect(Rect::new(r.x, r.y - t.scroll, r.w, r.h), theme.accent, 90);
                    }
                }
                s.blit(&self.lienzo, p.x, p.y);
                s.fill_rect(Rect::new(w, p.y, SB_W, p.h), fondo);
            }
            Vista::Imagen(img) => {
                s.fill_rect(p, FONDO_IMG);
                let (iw, ih) = ajustar_imagen(img, w);
                let (x, y) = ((w - iw) / 2, p.y + MARGEN_IMG - self.pest[a].scroll);
                con_clip(s, Rect::new(0, p.y, w, p.h), |s| nf_core::img::draw_scaled(img, s, x, y, iw, ih));
            }
        }
        let m = self.metricas();
        if m.needed() {
            self.sb.draw(s, &m, theme);
        }
        self.dibujar_aviso(s, p);
        if let Some(u) = &self.enlace {
            let txt = dialogs::fit(u, TS_S, (p.w * 6 / 10).max(40));
            let r = Rect::new(p.x, p.bottom() - 22, font::width(&txt, TS_S, false) + 20, 22);
            s.fill_rect(r, 0xF1F3F4);
            s.frame(r, LINEA);
            font::draw_in(s, Rect::new(r.x + 10, r.y, r.w - 10, r.h), &txt, TS_S, false, TINTA, font::Align::Left);
        }
    }
}

impl App for Navegador {
    fn kind(&self) -> AppKind {
        AppKind::Program
    }

    fn title(&self) -> String {
        let t = self.pest[self.act].titulo_visible();
        alloc::format!("{t} - Navegador")
    }

    fn min_size(&self) -> (i32, i32) {
        (480, 300)
    }

    fn draw(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, focused: bool) {
        self.size = (s.w, s.h);
        self.reloj = sys.uptime_ms();
        let (p, ahora, a) = (self.pagina(), self.reloj, self.act);
        let ancho = (p.w - SB_W).max(1);
        self.pest[a].preparar(ancho, &mut self.letras, sys.fs(), ahora);
        let max = self.max_scroll();
        self.pest[a].scroll = self.pest[a].scroll.clamp(0, max);
        s.fill_rect(Rect::new(0, 0, s.w, s.h), 0xFFFFFF);
        self.dibujar_pagina(s, theme, focused);
        self.dibujar_pestanas(s, theme);
        self.dibujar_barra(s, theme);
    }

    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
        // El escritorio avisa con `Key::Other` de cada cambio de Ctrl y Mayús.
        self.ctrl = ev.ctrl;
        if ev.key == Key::Other {
            return;
        }
        let (a, ahora) = (self.act, cx.now_ms);
        cx.redraw = true;
        if ev.ctrl {
            match ev.key {
                Key::Char('t') => return self.nueva(cx.sys, None, true, false, ahora),
                Key::Char('w') => return self.cerrar(a, cx),
                Key::Tab => {
                    let n = self.pest.len();
                    return self.activar(if ev.shift { (a + n - 1) % n } else { (a + 1) % n });
                }
                Key::Char('l') => return self.enfocar_barra(),
                Key::Char('r') => return self.mover(0, cx),
                Key::Char('c') => return self.copiar(),
                _ => {}
            }
        }
        match ev.key {
            Key::F(6) => return self.enfocar_barra(),
            Key::F(5) => return self.mover(0, cx),
            Key::Left if ev.alt => return self.mover(-1, cx),
            Key::Right if ev.alt => return self.mover(1, cx),
            Key::Escape => {
                if self.aviso.take().is_some() {
                    return;
                }
                if let (Foco::Pagina, Vista::Documento(d)) = (self.foco, &mut self.pest[a].vista) {
                    if d.tecla(Tecla::Escape, false) != Efecto::Ignorado {
                        return;
                    }
                }
                if self.foco == Foco::Barra && !self.pest[a].url_visible().is_empty() {
                    self.foco = Foco::Pagina;
                } else if self.pest[a].cargando() || self.pest[a].pendiente.is_some() {
                    self.pest[a].detener(cx.sys);
                }
                return;
            }
            _ => {}
        }
        if self.aviso.is_some() {
            return;
        }
        if self.foco == Foco::Barra && ev.key == Key::Tab && !ev.shift && matches!(self.pest[a].vista, Vista::Documento(_)) {
            self.foco = Foco::Pagina;
            if let Vista::Documento(d) = &mut self.pest[a].vista {
                d.quitar_foco();
            }
            if !self.tecla_doc(ev, cx) {
                self.foco = Foco::Barra;
            }
            return;
        }
        if self.foco == Foco::Barra {
            if self.barra.tecla(ev) == Accion::Enter {
                let t = self.barra.texto();
                self.ir_a(&t, cx);
            }
            return;
        }
        if matches!(self.pest[a].vista, Vista::Inicio) {
            if self.pest[a].caja.tecla(ev) == Accion::Enter {
                let t = self.pest[a].caja.texto();
                self.ir_a(&t, cx);
            }
            return;
        }
        if self.tecla_doc(ev, cx) {
            return;
        }
        let pagina = (self.pagina().h - PASO_FLECHA).max(PASO_FLECHA);
        match ev.key {
            Key::Down => self.desplazar(PASO_FLECHA),
            Key::Up => self.desplazar(-PASO_FLECHA),
            Key::PageDown => self.desplazar(pagina),
            Key::PageUp => self.desplazar(-pagina),
            Key::Char(' ') => self.desplazar(if ev.shift { -pagina } else { pagina }),
            Key::Home => self.desplazar_a(0),
            Key::End => self.desplazar_a(self.max_scroll()),
            _ => {}
        }
    }

    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
        let (a, ahora) = (self.act, cx.now_ms);
        cx.redraw = true;
        match ev {
            MouseEv::Down { x, y } => match self.hit(x, y) {
                Hit::Pestana(i) => self.activar(i),
                Hit::Cerrar(i) => self.cerrar(i, cx),
                Hit::Mas => self.nueva(cx.sys, None, true, false, ahora),
                Hit::Atras => self.mover(-1, cx),
                Hit::Adelante => self.mover(1, cx),
                Hit::Recargar if self.pest[a].cargando() || self.pest[a].pendiente.is_some() => self.pest[a].detener(cx.sys),
                Hit::Recargar => self.mover(0, cx),
                Hit::Barra if self.foco != Foco::Barra => self.enfocar_barra(),
                Hit::Barra => {
                    let r = rect_omnibox(self.size.0);
                    self.barra.clic(x - (r.x + 16), r.w - 32, TS);
                }
                Hit::Pagina => {
                    let m = self.metricas();
                    if m.needed() && !matches!(self.pest[a].vista, Vista::Inicio) && m.rect.contains(x, y) {
                        self.evento_barra(ev, ahora);
                        return;
                    }
                    self.foco = Foco::Pagina;
                    let p = self.pagina();
                    if self.aviso.is_some() {
                        let r = rect_aviso(p);
                        if rect_aviso_si(r).contains(x, y) || rect_aviso_no(r).contains(x, y) {
                            self.responder(rect_aviso_si(r).contains(x, y), cx);
                        }
                        return;
                    }
                    let t = &mut self.pest[a];
                    match &mut t.vista {
                        Vista::Inicio => {
                            let r = rect_caja(p);
                            if r.contains(x, y) {
                                t.caja.clic(x - (r.x + 48), r.w - 72, TS + 2);
                            }
                        }
                        Vista::Documento(d) => {
                            let ef = if x < p.w - SB_W { d.pulsar(x, y - p.y + t.scroll, &mut self.letras) } else { Efecto::Ignorado };
                            if ef != Efecto::Ignorado {
                                self.pulsado = None;
                                t.sel = None;
                                return self.aplicar(ef, cx);
                            }
                            self.pulsado = Some((x, y - p.y + t.scroll));
                            self.arrastro = false;
                            t.sel = None;
                        }
                        Vista::Imagen(_) => {}
                    }
                }
                Hit::Nada => {}
            },
            MouseEv::Move { x, y } => {
                if self.evento_barra(ev, ahora) {
                    return;
                }
                self.hover = self.hit(x, y);
                let p = self.pagina();
                if let Some(ini) = self.pulsado {
                    let fin = (x.clamp(0, p.w), y - p.y + self.pest[a].scroll);
                    if self.arrastro || (fin.0 - ini.0).abs() + (fin.1 - ini.1).abs() > 4 {
                        self.arrastro = true;
                        self.pest[a].sel = Some((ini, fin));
                    }
                    if y < p.y {
                        self.desplazar(-PASO_FLECHA / 2);
                    } else if y > p.bottom() {
                        self.desplazar(PASO_FLECHA / 2);
                    }
                    self.enlace = None;
                } else {
                    self.enlace = self.enlace_en(x, y);
                }
            }
            MouseEv::Up { .. } => {
                self.evento_barra(ev, ahora);
                if let Some(ini) = self.pulsado.take() {
                    let url = match (&self.pest[a].vista, self.arrastro) {
                        (Vista::Documento(d), false) => d.enlace_en(ini.0, ini.1),
                        _ => None,
                    };
                    if let Some(u) = url {
                        // Clic central no llega a los programas: Ctrl+clic abre la pestaña nueva.
                        let nueva = self.ctrl;
                        self.abrir_enlace(&u, nueva, cx);
                    }
                }
            }
            MouseEv::Wheel { dz, .. } => {
                if !matches!(self.pest[a].vista, Vista::Inicio) {
                    self.desplazar(dz * PASO_RUEDA);
                }
            }
            MouseEv::Leave => {
                self.evento_barra(ev, ahora);
                self.hover = Hit::Nada;
                self.enlace = None;
            }
            MouseEv::RightDown { .. } => {}
        }
    }

    fn tick(&mut self, cx: &mut Ctx) {
        let ahora = cx.now_ms;
        self.reloj = ahora;
        let mut anima = false;
        for t in self.pest.iter_mut() {
            anima |= t.tick(cx.sys, ahora);
            anima |= t.cargando() || t.pendiente.is_some();
        }
        let m = self.metricas();
        if let Some(p) = self.sb.tick(&m, ahora) {
            self.desplazar_a(p);
            anima = true;
        }
        if anima {
            cx.redraw = true;
        }
    }

    fn open_path(&mut self, path: &str, cx: &mut Ctx) {
        if es_ruta_local(path) {
            if let Some(e) = clasificar(path) {
                self.pest[self.act].ir(cx.sys, e, cx.now_ms);
                self.foco = Foco::Pagina;
                cx.redraw = true;
            }
        }
    }

    fn release(&mut self, sys: &mut dyn Platform) {
        for t in self.pest.iter_mut() {
            t.cancelar(sys);
        }
    }
}
