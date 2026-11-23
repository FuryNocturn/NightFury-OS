// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Mensajes: chat cifrado de extremo a extremo entre equipos NightFury de la red local, sin servidor.
//! Programa en modo usuario (ver `main.rs`); la lógica vive aquí para probarla y fotografiarla en
//! el PC. Protocolo: docs/Mensajes-protocolo.md. `motor` es la red y los datos; este archivo, la ventana.
#![cfg_attr(not(test), no_std)]
extern crate alloc;

mod almacen;
mod linea;
mod motor;
mod palabras;
mod proto;
#[cfg(test)]
mod pruebas;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use almacen::{Estado, Mensaje};
use linea::{Accion, Linea};
use motor::{Motor, Permiso, Red};
use nf_core::apps::{App, AppKind, Ctx, MouseEv};
use nf_core::dialogs::{self, Answer, Buttons, DialogRequest, DialogResult};
use nf_core::font::{self, Align};
use nf_core::geom::Rect;
use nf_core::input::{Key, KeyEvent};
use nf_core::platform::Platform;
use nf_core::surface::{blend, lighten, Surface};
use nf_core::theme::Theme;
use proto::{huella, palabras, Clave};

const LEFT: i32 = 200;
const HEAD_H: i32 = 48;
const STRIP_H: i32 = 26;
const IN_H: i32 = 44;
const ITEM_H: i32 = 52;
const PAD_H: i32 = 10;
const PAD_V: i32 = 7;
const GAP: i32 = 6;
const TS: u32 = 10;
const TS_S: u32 = 9;
const INK: u32 = 0x1A1A1A;
const MUTED: u32 = 0x6B6B6B;
const LINE: u32 = 0xE0E0E0;
const PANEL: u32 = 0xF3F3F3;
const THEIRS: u32 = 0xE6E6E6;
const RED: u32 = 0xC62828;
const GREEN: u32 = 0x2E7D32;
const DOT_ON: u32 = 0x2E9E4F;
const DOT_OFF: u32 = 0xB0B0B0;
const TAG_VERIF: u32 = 1;

/// Zonas de la ventana según su tamaño (las usan `draw` y el ratón).
struct Zonas {
    lista: Rect,
    cab: Rect,
    palabras: Rect,
    msgs: Rect,
    entrada: Rect,
    campo: Rect,
    enviar: Rect,
    franja: Rect,
    reintentar: Rect,
}

fn zonas(w: i32, h: i32) -> Zonas {
    let x0 = LEFT + 1;
    let rw = (w - x0).max(0);
    let entrada = Rect::new(x0, h - STRIP_H - IN_H, rw, IN_H);
    let franja = Rect::new(0, h - STRIP_H, w, STRIP_H);
    Zonas {
        lista: Rect::new(0, HEAD_H, LEFT, (h - HEAD_H - STRIP_H).max(0)),
        cab: Rect::new(x0, 0, rw, HEAD_H),
        palabras: Rect::new(w - 12 - 176, 9, 176, 30),
        msgs: Rect::new(x0, HEAD_H, rw, (entrada.y - HEAD_H).max(0)),
        entrada,
        campo: Rect::new(x0 + 10, entrada.y + 7, (rw - 20 - 88).max(0), 30),
        enviar: Rect::new(w - 10 - 80, entrada.y + 7, 80, 30),
        franja,
        reintentar: Rect::new(w - 8 - 100, franja.y + 2, 100, STRIP_H - 4),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hit {
    Nada,
    Palabras,
    Enviar,
    Reintentar,
}

/// Líneas ya partidas de cada mensaje de la conversación abierta (partir texto es lo más caro de dibujar).
struct Cache {
    pk: Clave,
    ancho: i32,
    len: usize,
    primero: u64,
    lineas: Vec<Vec<String>>,
}

pub struct Mensajes {
    /// Se crea en el primer evento: necesita el sistema para leer la carpeta del usuario.
    motor: Option<Motor>,
    linea: Linea,
    sel: Option<Clave>,
    /// Píxeles hacia arriba desde el final de la conversación (0 = al final).
    scroll: i32,
    max_scroll: i32,
    lista_scroll: i32,
    size: (i32, i32),
    cache: Option<Cache>,
    verificando: Option<Clave>,
    hover: Hit,
}

/// App lista para `nf_app::programa!`.
pub fn crear() -> Box<dyn App> {
    Box::new(Mensajes::new())
}

/// Para las capturas del PC (`tools/nfprog`): dos contactos y una conversación de ejemplo, sin red.
#[doc(hidden)]
pub fn crear_demo(sys: &mut dyn Platform) -> Box<dyn App> {
    let mut a = Mensajes::new();
    let m = Motor::nuevo(sys, 0).con_ejemplo();
    a.sel = m.contactos.first().map(|c| c.pk);
    a.motor = Some(m);
    Box::new(a)
}

fn hora(f: &nf_core::platform::DateTime) -> String {
    format!("{:02}:{:02}", f.hour, f.minute)
}

fn caja(s: &mut Surface, r: Rect, c: u32) {
    s.fill_rect(Rect::new(r.x + 2, r.y, r.w - 4, r.h), c);
    s.fill_rect(Rect::new(r.x + 1, r.y + 1, r.w - 2, r.h - 2), c);
    s.fill_rect(Rect::new(r.x, r.y + 2, r.w, r.h - 4), c);
}

fn con_clip(s: &mut Surface, r: Rect, f: impl FnOnce(&mut Surface)) {
    let old = s.clip();
    s.set_clip(old.intersect(r));
    f(s);
    s.set_clip(old);
}

fn boton(s: &mut Surface, r: Rect, texto: &str, relleno: Option<u32>, hover: bool) {
    match relleno {
        Some(c) => {
            caja(s, r, if hover { lighten(c, 25) } else { c });
            font::draw_in(s, r, texto, TS_S, true, 0xFFFFFF, Align::Center);
        }
        None => {
            caja(s, r, if hover { 0xE4E4E4 } else { 0xF7F7F7 });
            s.frame(r, 0xBDBDBD);
            font::draw_in(s, r, texto, TS_S, false, INK, Align::Center);
        }
    }
}

impl Mensajes {
    fn new() -> Mensajes {
        Mensajes { motor: None, linea: Linea::default(), sel: None, scroll: 0, max_scroll: 0, lista_scroll: 0, size: (0, 0), cache: None, verificando: None, hover: Hit::Nada }
    }

    fn asegurar(&mut self, sys: &mut dyn Platform, ms: u64) -> &mut Motor {
        self.motor.get_or_insert_with(|| Motor::nuevo(sys, ms))
    }

    /// Índices de los contactos en el orden de la lista: conectados primero, luego por nombre.
    fn orden(&self) -> Vec<usize> {
        let Some(m) = self.motor.as_ref() else { return Vec::new() };
        let mut v: Vec<usize> = (0..m.contactos.len()).collect();
        v.sort_by_cached_key(|&i| (!m.contactos[i].en_linea(m.ahora), m.contactos[i].nombre.to_lowercase()));
        v
    }

    fn hit(&self, x: i32, y: i32) -> Hit {
        let z = zonas(self.size.0, self.size.1);
        let denegado = self.motor.as_ref().is_some_and(|m| matches!(m.red, Red::Denegado(_)));
        if self.sel.is_some() && z.palabras.contains(x, y) {
            Hit::Palabras
        } else if self.sel.is_some() && z.enviar.contains(x, y) {
            Hit::Enviar
        } else if denegado && z.reintentar.contains(x, y) {
            Hit::Reintentar
        } else {
            Hit::Nada
        }
    }

    fn elegir(&mut self, pk: Clave) {
        if self.sel != Some(pk) {
            self.sel = Some(pk);
            self.scroll = 0;
            self.linea.vaciar();
        }
        if let Some(m) = self.motor.as_mut() {
            m.seleccionado = self.sel;
            m.leido(&pk);
        }
    }

    fn enviar(&mut self, cx: &mut Ctx) {
        let (Some(pk), false) = (self.sel, self.linea.vacia()) else { return };
        let texto = self.linea.texto();
        let ms = cx.now_ms;
        if self.asegurar(cx.sys, ms).enviar(cx.sys, &pk, &texto) {
            self.linea.vaciar();
            self.scroll = 0;
            cx.redraw = true;
        }
    }

    fn pedir_verificacion(&mut self, cx: &mut Ctx) {
        let (Some(pk), Some(m)) = (self.sel, self.motor.as_ref()) else { return };
        let Some(c) = m.contacto(&pk) else { return };
        let p = palabras(&m.publica, &pk);
        let text = format!("Conversación con {}:\n\n{}\n\nHuella de su clave: {}\n\n¿Coinciden con las que ve la otra persona?", c.nombre, p.join(" "), huella(&pk));
        self.verificando = Some(pk);
        cx.dialog = Some(DialogRequest::Message { tag: TAG_VERIF, title: String::from("Palabras de verificación"), text, buttons: Buttons::YesNo });
    }

    /// Texto de la franja de estado y si lleva el botón «Reintentar».
    fn franja(&self) -> (String, bool) {
        let Some(m) = self.motor.as_ref() else { return (String::from("Buscando equipos en la red local…"), false) };
        let (texto, reintentar) = match m.red {
            Red::Esperando(Permiso::Usar) => (String::from("Esperando permiso para usar la red…"), false),
            Red::Esperando(Permiso::Aceptar) => (String::from("Esperando permiso para aceptar conexiones…"), false),
            Red::Denegado(Permiso::Usar) => (String::from("Sin permiso para usar la red. Puedes cambiarlo en Configuración → Red."), true),
            Red::Denegado(Permiso::Aceptar) => (String::from("Sin permiso para aceptar conexiones. Puedes cambiarlo en Configuración → Red."), true),
            Red::SinRed => (String::from("Sin conexión de red"), false),
            Red::Error(e) => (String::from(e), false),
            Red::Iniciando => (String::from("Buscando equipos en la red local…"), false),
            Red::Activo => match m.equipos_en_linea() {
                0 => (String::from("Buscando equipos en la red local…"), false),
                1 => (String::from("1 equipo en la red local"), false),
                n => (format!("{n} equipos en la red local"), false),
            },
        };
        (texto, reintentar)
    }

    // ------------------------------------------------------------ dibujo

    fn dibujar_lista(&mut self, s: &mut Surface, theme: &Theme, z: &Zonas) {
        let orden = self.orden();
        let Some(m) = self.motor.as_ref() else { return };
        font::draw_in(s, Rect::new(14, 0, LEFT - 14, HEAD_H), "Contactos", 12, true, INK, Align::Left);
        let total = orden.len() as i32 * ITEM_H;
        self.lista_scroll = self.lista_scroll.clamp(0, (total - z.lista.h).max(0));
        if orden.is_empty() {
            font::draw(s, 14, z.lista.y + 12, "Todavía no hay equipos.", TS_S, false, MUTED);
        }
        con_clip(s, z.lista, |s| {
            for (k, &i) in orden.iter().enumerate() {
                let c = &m.contactos[i];
                let y = z.lista.y + k as i32 * ITEM_H - self.lista_scroll;
                if y + ITEM_H <= z.lista.y || y >= z.lista.bottom() {
                    continue;
                }
                if self.sel == Some(c.pk) {
                    s.fill_rect(Rect::new(0, y, LEFT, ITEM_H), blend(PANEL, theme.accent, 45));
                }
                let en_linea = c.en_linea(m.ahora);
                s.fill_circle(16.0, (y + ITEM_H / 2) as f32, 5.0, if en_linea { DOT_ON } else { DOT_OFF }, 255);
                let nombre = dialogs::fit(&c.nombre, TS, LEFT - 36 - 20);
                font::draw(s, 32, y + 8, &nombre, TS, c.sin_leer, INK);
                let (texto, color) = if c.verificado {
                    ("Verificado", GREEN)
                } else if m.otra_clave(c) {
                    ("Otra clave", RED)
                } else {
                    ("Sin verificar", MUTED)
                };
                font::draw(s, 32, y + 28, texto, TS_S, false, color);
                if c.sin_leer {
                    s.fill_circle((LEFT - 14) as f32, (y + ITEM_H / 2) as f32, 4.0, theme.accent, 255);
                }
            }
        });
    }

    /// Cabecera, burbujas y línea de entrada de la conversación elegida.
    fn dibujar_conversacion(&mut self, s: &mut Surface, theme: &Theme, z: &Zonas, focused: bool, pk: Clave) {
        let Some(m) = self.motor.as_ref() else { return };
        let Some(c) = m.contacto(&pk) else { return };
        // Cabecera.
        let nombre = dialogs::fit(&c.nombre, 12, (z.palabras.x - z.cab.x - 24).max(20));
        font::draw(s, z.cab.x + 14, 7, &nombre, 12, true, INK);
        let en_linea = c.en_linea(m.ahora);
        font::draw(s, z.cab.x + 14, 27, if en_linea { "Conectado" } else { "Desconectado" }, TS_S, false, if en_linea { GREEN } else { MUTED });
        boton(s, z.palabras, "Palabras de verificación", None, self.hover == Hit::Palabras);
        s.fill_rect(Rect::new(z.cab.x, HEAD_H - 1, z.cab.w, 1), LINE);
        // Burbujas.
        let ancho = (z.msgs.w * 7 / 10).max(120) - 2 * PAD_H;
        let vale = matches!(&self.cache, Some(k) if k.pk == pk && k.ancho == ancho && k.len == c.msgs.len() && k.primero == c.msgs.first().map_or(0, |m| m.id));
        if !vale {
            self.cache = Some(Cache { pk, ancho, len: c.msgs.len(), primero: c.msgs.first().map_or(0, |m| m.id), lineas: c.msgs.iter().map(|m| dialogs::wrap(&m.texto, TS, ancho)).collect() });
        }
        let Some(cache) = self.cache.as_ref() else { return };
        let (lh, mh) = (font::line_h(TS), font::line_h(TS_S));
        let alto = |k: usize| cache.lineas[k].len() as i32 * lh + 2 * PAD_V + mh + GAP;
        let total: i32 = (0..c.msgs.len()).map(alto).sum();
        self.max_scroll = (total + 12 - z.msgs.h).max(0);
        self.scroll = self.scroll.clamp(0, self.max_scroll);
        let scroll = self.scroll;
        con_clip(s, z.msgs, |s| {
            let mut fin = z.msgs.bottom() - 6 + scroll;
            for k in (0..c.msgs.len()).rev() {
                let top = fin - alto(k);
                if top >= z.msgs.bottom() {
                    fin = top;
                    continue;
                }
                if fin <= z.msgs.y {
                    break;
                }
                fin = top;
                let (msg, lineas): (&Mensaje, &Vec<String>) = (&c.msgs[k], &cache.lineas[k]);
                let ancho_txt = lineas.iter().map(|l| font::width(l, TS, false)).max().unwrap_or(0);
                let bw = ancho_txt + 2 * PAD_H;
                let bh = lineas.len() as i32 * lh + 2 * PAD_V;
                let bx = if msg.saliente { z.msgs.right() - 14 - bw } else { z.msgs.x + 14 };
                let (fondo, tinta) = if msg.saliente { (theme.accent, 0xFFFFFF) } else { (THEIRS, INK) };
                caja(s, Rect::new(bx, top, bw, bh), fondo);
                for (j, l) in lineas.iter().enumerate() {
                    font::draw(s, bx + PAD_H, top + PAD_V + j as i32 * lh, l, TS, false, tinta);
                }
                let estado = match msg.estado {
                    Estado::Entregado => " · Entregado",
                    Estado::Pendiente if en_linea => " · Enviando…",
                    Estado::Pendiente => " · Pendiente",
                    Estado::Recibido => "",
                };
                let meta = format!("{}{}", hora(&msg.fecha), estado);
                let mx = if msg.saliente { bx + bw - font::width(&meta, TS_S, false) } else { bx };
                font::draw(s, mx, top + bh + 1, &meta, TS_S, false, MUTED);
            }
        });
        // Línea de entrada.
        s.fill_rect(Rect::new(z.entrada.x, z.entrada.y, z.entrada.w, 1), LINE);
        s.fill_rect(z.campo, 0xFFFFFF);
        s.frame(z.campo, if focused { theme.accent } else { 0xBDBDBD });
        let inner = Rect::new(z.campo.x + 8, z.campo.y, z.campo.w - 16, z.campo.h);
        let (vis, cx) = self.linea.visible(inner.w, TS);
        if vis.is_empty() {
            font::draw_in(s, inner, "Escribe un mensaje", TS, false, 0x9A9A9A, Align::Left);
        } else {
            font::draw_in(s, inner, &vis, TS, false, INK, Align::Left);
        }
        if focused {
            s.fill_rect(Rect::new(inner.x + cx, z.campo.y + 7, 1, z.campo.h - 14), INK);
        }
        let activo = !self.linea.vacia();
        if activo {
            boton(s, z.enviar, "Enviar", Some(theme.accent), self.hover == Hit::Enviar);
        } else {
            caja(s, z.enviar, 0xD0D0D0);
            font::draw_in(s, z.enviar, "Enviar", TS_S, true, 0xFFFFFF, Align::Center);
        }
    }

    fn dibujar_vacio(&self, s: &mut Surface, z: &Zonas) {
        let ancho = (z.msgs.w - 80).max(60);
        let r = Rect::new(z.cab.x, HEAD_H, z.cab.w, z.entrada.bottom() - HEAD_H);
        let lineas = dialogs::wrap("Aquí aparecen solos los equipos NightFury de la red local que tengan Mensajes abierto. Elige uno de la lista para escribirle. Los mensajes viajan cifrados de extremo a extremo.", TS, ancho);
        let lh = font::line_h(TS);
        let alto = lh * (lineas.len() as i32 + 2);
        let mut y = r.y + (r.h - alto) / 2;
        font::draw_in(s, Rect::new(r.x, y, r.w, lh), "Elige un contacto", 12, true, INK, Align::Center);
        y += lh * 2;
        for l in &lineas {
            font::draw_in(s, Rect::new(r.x, y, r.w, lh), l, TS, false, MUTED, Align::Center);
            y += lh;
        }
    }
}

impl App for Mensajes {
    fn kind(&self) -> AppKind {
        AppKind::Program
    }

    fn title(&self) -> String {
        String::from("Mensajes")
    }

    fn min_size(&self) -> (i32, i32) {
        (480, 300)
    }

    fn draw(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, focused: bool) {
        self.size = (s.w, s.h);
        let ms = sys.uptime_ms();
        self.asegurar(sys, ms);
        // Si el contacto elegido ya no existe (se olvidó), vuelve la pantalla de bienvenida.
        if let (Some(pk), Some(m)) = (self.sel, self.motor.as_ref()) {
            if m.contacto(&pk).is_none() {
                self.sel = None;
            }
        }
        let z = zonas(s.w, s.h);
        s.fill_rect(Rect::new(0, 0, s.w, s.h), 0xFFFFFF);
        s.fill_rect(Rect::new(0, 0, LEFT, s.h - STRIP_H), PANEL);
        s.fill_rect(Rect::new(LEFT, 0, 1, s.h - STRIP_H), LINE);
        s.fill_rect(Rect::new(0, HEAD_H - 1, LEFT, 1), LINE);
        self.dibujar_lista(s, theme, &z);
        match self.sel {
            Some(pk) => self.dibujar_conversacion(s, theme, &z, focused, pk),
            None => self.dibujar_vacio(s, &z),
        }
        // Franja de estado.
        s.fill_rect(z.franja, 0xF0F0F0);
        s.fill_rect(Rect::new(0, z.franja.y, s.w, 1), LINE);
        let (texto, reintentar) = self.franja();
        let ancho = z.franja.w - 24 - if reintentar { 116 } else { 0 };
        font::draw_in(s, Rect::new(12, z.franja.y, ancho, STRIP_H), &dialogs::fit(&texto, TS_S, ancho), TS_S, false, if reintentar { RED } else { INK }, Align::Left);
        if reintentar {
            boton(s, z.reintentar, "Reintentar", None, self.hover == Hit::Reintentar);
        }
    }

    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
        let Some(_) = self.sel else { return };
        let pagina = (self.size.1 - HEAD_H - STRIP_H - IN_H - 40).max(40);
        match ev.key {
            Key::PageUp => self.scroll = (self.scroll + pagina).min(self.max_scroll),
            Key::PageDown => self.scroll = (self.scroll - pagina).max(0),
            _ => match self.linea.tecla(ev) {
                Accion::Enviar => return self.enviar(cx),
                Accion::Nada => return,
                Accion::Cambio => {}
            },
        }
        cx.redraw = true;
    }

    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
        let ms = cx.now_ms;
        self.asegurar(cx.sys, ms);
        match ev {
            MouseEv::Down { x, y } => {
                let z = zonas(self.size.0, self.size.1);
                match self.hit(x, y) {
                    Hit::Palabras => self.pedir_verificacion(cx),
                    Hit::Enviar => self.enviar(cx),
                    Hit::Reintentar => {
                        if let Some(m) = self.motor.as_mut() {
                            m.reintentar(cx.sys);
                        }
                    }
                    Hit::Nada if z.lista.contains(x, y) => {
                        let k = ((y - z.lista.y + self.lista_scroll) / ITEM_H) as usize;
                        let pk = self.orden().get(k).and_then(|&i| self.motor.as_ref()?.contactos.get(i)).map(|c| c.pk);
                        if let Some(pk) = pk {
                            self.elegir(pk);
                        }
                    }
                    Hit::Nada => {}
                }
                cx.redraw = true;
            }
            MouseEv::Wheel { x, dz, .. } => {
                if x < LEFT {
                    self.lista_scroll = (self.lista_scroll + dz * 30).max(0);
                } else {
                    self.scroll = (self.scroll - dz * 40).clamp(0, self.max_scroll);
                }
                cx.redraw = true;
            }
            MouseEv::Move { x, y } => {
                let h = self.hit(x, y);
                if h != self.hover {
                    self.hover = h;
                    cx.redraw = true;
                }
            }
            MouseEv::Leave => {
                if self.hover != Hit::Nada {
                    self.hover = Hit::Nada;
                    cx.redraw = true;
                }
            }
            _ => {}
        }
    }

    fn tick(&mut self, cx: &mut Ctx) {
        let ms = cx.now_ms;
        let m = self.asegurar(cx.sys, ms);
        m.tick(cx.sys, ms);
        if core::mem::take(&mut m.cambio) {
            cx.redraw = true;
        }
    }

    fn dialog_result(&mut self, r: DialogResult, cx: &mut Ctx) {
        if let DialogResult::Answer { tag: TAG_VERIF, answer } = r {
            if let (Some(pk), Some(m)) = (self.verificando.take(), self.motor.as_mut()) {
                match answer {
                    Answer::Yes => m.verificar(&pk, true),
                    // Si no coinciden, lo seguro es no darlo por verificado.
                    Answer::No => m.verificar(&pk, false),
                    _ => {}
                }
                cx.redraw = true;
            }
        }
    }

    fn release(&mut self, sys: &mut dyn Platform) {
        if let Some(m) = self.motor.as_mut() {
            m.liberar(sys);
        }
    }
}
