// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Navegador de NightFury: páginas web (HTTP y HTTPS) y archivos del disco con pestañas, sin JavaScript.
//! Programa en modo usuario (ver `main.rs`); la lógica vive aquí para probarla y fotografiarla en el PC.
//! El motor de páginas es `nf-web`; `pestana` lleva el historial y las descargas, `entrada` lo escrito en la
//! barra y las reglas de seguridad, y este archivo la ventana.
#![cfg_attr(not(test), no_std)]
extern crate alloc;

mod campo;
mod datos;
mod dibujo;
mod entrada;
mod fuentes;
mod internas;
mod paginas;
mod pestana;
#[cfg(test)]
mod pruebas;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use campo::{Accion, Campo};
use datos::{host, sugerir, Datos, Sugerencia};
use dibujo::{caja, capsula, disco_inicial, envolver, icono, Ico};
use entrada::{clasificar, es_ruta_local, local_de_url, sin_fragmento, Entrada};
use fuentes::Letras;
use nf_core::apps::{App, AppKind, Ctx, MouseEv};
use nf_core::dialogs;
use nf_core::font;
use nf_core::geom::Rect;
use nf_core::hora;
use nf_core::input::{Key, KeyEvent};
use nf_core::menu::{MenuItem, MenuRequest};
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
/// Alto de la barra de favoritos, de cada sugerencia de la barra de direcciones y espacio que se reserva en
/// la tira de pestañas para el rótulo «Incógnito».
const FAV_H: i32 = 30;
const SUG_H: i32 = 38;
const SUG_MAX: usize = 8;
const ANCHO_INCOG: i32 = 112;
/// Favoritos que enseña la pestaña nueva.
const MAX_ACCESOS: usize = 8;
const TAG_FAV: u32 = 1;
const TAG_MENU: u32 = 2;
const M_NUEVA: u32 = 1;
const M_INCOG: u32 = 2;
const M_HIST: u32 = 3;
const M_FAVS: u32 = 4;
const M_BARRA: u32 = 5;
const M_ESTRELLA: u32 = 6;
const URL_HISTORIAL: &str = "nightfury://historial";
const URL_FAVORITOS: &str = "nightfury://favoritos";
/// Lo que se abre para pedir una ventana de incógnito: el escritorio lo entrega a este programa (`abre=nfinc` en
/// su `app.txt`) y `open_path` lo reconoce. El fichero no existe ni hace falta que exista.
const MARCA_INCOGNITO: &str = "AppData\\Navegador\\incognito.nfinc";
const TEXTO_INCOGNITO: &str = "Estás en modo incógnito. Lo que visites no se guarda en el historial y las cookies se borran al cerrar. Las descargas sí se guardan.";

/// Colores de la ventana: claros, u oscuros en incógnito.
struct Paleta {
    tira: u32,
    barra: u32,
    linea: u32,
    tinta: u32,
    apagado: u32,
    gris: u32,
    caja: u32,
    caja_edit: u32,
    sobre: u32,
    sobre_fuerte: u32,
    inactivo: u32,
    separador: u32,
    inicio: u32,
    borde_inicio: u32,
}

fn paleta(incognito: bool) -> Paleta {
    if incognito {
        Paleta { tira: 0x202124, barra: 0x35363A, linea: 0x4A4C50, tinta: 0xE8EAED, apagado: 0xBDC1C6, gris: 0x9AA0A6, caja: 0x202124, caja_edit: 0x202124, sobre: 0x2D2E31, sobre_fuerte: 0x4A4C50, inactivo: 0x6B6F73, separador: 0x5F6368, inicio: 0x35363A, borde_inicio: 0x5F6368 }
    } else {
        Paleta { tira: TIRA, barra: 0xFFFFFF, linea: LINEA, tinta: TINTA, apagado: APAGADO, gris: GRIS, caja: 0xF1F3F4, caja_edit: 0xFFFFFF, sobre: 0xE8EAED, sobre_fuerte: 0xCDD0D5, inactivo: 0xBDC1C6, separador: 0xA0A4AA, inicio: 0xFFFFFF, borde_inicio: 0xDFE1E5 }
    }
}

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
    Estrella,
    Menu,
    /// Favorito o carpeta de la barra de favoritos (su id).
    Fav(u32),
    /// «»»: lo que no cabe en la barra de favoritos.
    FavMas,
    /// Sugerencia de la barra de direcciones.
    Sug(usize),
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
    Rect::new(120, TAB_H + 6, (w - 120 - 52).max(40), 30)
}
/// Botón de los tres puntos, a la derecha de la barra de direcciones.
fn rect_menu(w: i32) -> Rect {
    Rect::new(w - 40, TAB_H + 5, 32, 32)
}
/// La estrella, dentro del extremo derecho de la barra de direcciones.
fn rect_estrella(o: Rect) -> Rect {
    Rect::new(o.right() - 34, o.y + 1, 30, o.h - 2)
}
/// Sugerencia `i` bajo la barra de direcciones.
fn rect_sug(i: usize, w: i32) -> Rect {
    let o = rect_omnibox(w);
    Rect::new(o.x, o.bottom() + 6 + i as i32 * SUG_H, o.w, SUG_H)
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
    /// Historial y favoritos del usuario (se cargan al primer evento).
    datos: Datos,
    /// Ventana de incógnito: sin historial, aspecto oscuro. Se decide al abrirla y no cambia.
    incognito: bool,
    /// Sugerencias de la barra de direcciones para lo escrito y la elegida con las flechas.
    sug: Vec<Sugerencia>,
    sug_sel: Option<usize>,
    /// Barra de favoritos: `None` = visible si hay favoritos (Ctrl+Mayús+B lo fuerza).
    barra_fav: Option<bool>,
    /// Acceso de la pestaña nueva bajo el ratón (id del favorito).
    hover_acceso: Option<u32>,
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
            datos: Datos::default(),
            incognito: false,
            sug: Vec::new(),
            sug_sel: None,
            barra_fav: None,
            hover_acceso: None,
        }
    }

    // ------------------------------------------------------------ datos del usuario

    /// ¿Esta ventana es de incógnito? (Para el tarro de cookies: en incógnito, todo en memoria.)
    pub fn es_incognito(&self) -> bool {
        self.incognito
    }

    /// ¿Hay un favorito de este sitio? (Para el tarro de cookies: los sitios en favoritos conservan la sesión.)
    pub fn es_favorito_sitio(&self, host: &str) -> bool {
        self.datos.favs.es_sitio(host)
    }

    /// Lee historial y favoritos la primera vez (necesita el disco, que el constructor no tiene).
    fn cargar_datos(&mut self, sys: &mut dyn Platform) {
        if !self.datos.cargado {
            let ahora = hora::segundos(sys.now());
            self.datos.cargar(sys.fs(), ahora, self.reloj);
        }
    }

    /// Escribe lo pendiente (todo, con `forzar`).
    fn guardar_datos(&mut self, sys: &mut dyn Platform, ms: u64, forzar: bool) {
        let now = sys.now();
        self.datos.guardar(sys.fs(), ms, now, forzar);
    }

    /// Esta ventana pasa a ser de incógnito: nada del historial se queda ni se escribe.
    fn poner_incognito(&mut self) {
        self.incognito = true;
        self.datos.incog = true;
        self.datos.hist = datos::Historial::default();
        self.sug.clear();
    }

    /// Entrega las páginas internas pedidas por las pestañas.
    fn servir_internas(&mut self, sys: &mut dyn Platform) {
        if self.pest.iter().all(|t| t.interna.is_none()) {
            return;
        }
        let ahora = hora::segundos(sys.now());
        for t in self.pest.iter_mut() {
            if let Some(u) = t.interna.take() {
                t.servir_interna(&u, &internas::pagina(&self.datos, &u, ahora));
            }
        }
    }

    fn barra_fav_visible(&self) -> bool {
        self.barra_fav.unwrap_or(!self.datos.favs.v.is_empty())
    }

    /// Dónde empieza la página: bajo las pestañas, la barra de direcciones y la de favoritos.
    fn y_pagina(&self) -> i32 {
        TAB_H + TOOL_H + if self.barra_fav_visible() { FAV_H } else { 0 }
    }

    fn pagina(&self) -> Rect {
        let y = self.y_pagina();
        Rect::new(0, y, self.size.0, (self.size.1 - y).max(1))
    }

    /// Ancho que ocupan las pestañas (en incógnito, sin el rótulo de la derecha).
    fn ancho_tira(&self) -> i32 {
        self.size.0 - if self.incognito { ANCHO_INCOG } else { 0 }
    }

    /// La pestaña activa se puede poner en favoritos: tiene una dirección que no es interna.
    fn estrella_visible(&self) -> bool {
        let u = self.pest[self.act].url_visible();
        !u.is_empty() && !internas::es_interna(u)
    }

    fn es_favorita(&self) -> bool {
        self.datos.favs.buscar_url(self.pest[self.act].url_visible()).is_some()
    }

    fn metricas(&self) -> Metrics {
        let p = self.pagina();
        Metrics { rect: Rect::new(p.right() - SB_W, p.y, SB_W, p.h), total: self.pest[self.act].alto, page: p.h, pos: self.pest[self.act].scroll, line: PASO_FLECHA }
    }

    fn hit(&self, x: i32, y: i32) -> Hit {
        let (w, n) = (self.size.0, self.pest.len());
        // Las sugerencias tapan lo que hay debajo de la barra de direcciones.
        if self.foco == Foco::Barra {
            if let Some(i) = (0..self.sug.len()).find(|&i| rect_sug(i, w).contains(x, y)) {
                return Hit::Sug(i);
            }
        }
        if y < TAB_H {
            let wt = self.ancho_tira();
            if rect_mas(n, wt).contains(x, y) {
                return Hit::Mas;
            }
            return (0..n).find_map(|i| {
                let r = rect_pestana(i, n, wt);
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
            } else if rect_menu(w).contains(x, y) {
                Hit::Menu
            } else if self.estrella_visible() && rect_estrella(rect_omnibox(w)).contains(x, y) {
                Hit::Estrella
            } else if rect_omnibox(w).contains(x, y) {
                Hit::Barra
            } else {
                Hit::Nada
            };
        }
        if self.barra_fav_visible() && y < TAB_H + TOOL_H + FAV_H {
            let (items, mas) = self.items_fav();
            return items.iter().find(|(r, _)| r.contains(x, y)).map(|&(_, id)| Hit::Fav(id)).or_else(|| mas.filter(|r| r.contains(x, y)).map(|_| Hit::FavMas)).unwrap_or(Hit::Nada);
        }
        Hit::Pagina
    }

    // ------------------------------------------------------------ favoritos

    /// Lo que cabe de la barra de favoritos (rectángulo e id) y el botón de lo que no cabe.
    fn items_fav(&self) -> (Vec<(Rect, u32)>, Option<Rect>) {
        let raiz = self.datos.favs.hijos(0);
        let anchos: Vec<i32> = raiz.iter().map(|f| (font::width(&f.titulo, TS_S, false) + if f.carpeta { 52 } else { 40 }).min(190)).collect();
        let disp = self.size.0 - 16;
        let todo = anchos.iter().map(|a| a + 4).sum::<i32>() <= disp;
        let limite = 8 + if todo { disp } else { disp - 34 };
        let (mut v, mut x) = (Vec::new(), 8);
        for (f, a) in raiz.iter().zip(&anchos) {
            if x + a > limite {
                break;
            }
            v.push((Rect::new(x, TAB_H + TOOL_H + 3, *a, FAV_H - 6), f.id));
            x += a + 4;
        }
        let mas = (v.len() < raiz.len()).then(|| Rect::new(x, TAB_H + TOOL_H + 3, 30, FAV_H - 6));
        (v, mas)
    }

    /// Elementos de menú de lo que cuelga de `padre` (las carpetas, como submenús).
    fn menu_de(&self, f: &datos::Fav) -> MenuItem {
        let t = dialogs::fit(&f.titulo, 9, 280);
        if !f.carpeta {
            return MenuItem::new(f.id, &t);
        }
        let hijos: Vec<MenuItem> = self.datos.favs.hijos(f.id).into_iter().map(|h| MenuItem::new(h.id, &dialogs::fit(&h.titulo, 9, 280))).collect();
        if hijos.is_empty() {
            MenuItem::new(0, &t).disabled(true)
        } else {
            MenuItem::submenu(&t, hijos)
        }
    }

    /// Abre el menú de la carpeta `id` bajo su botón de la barra.
    fn menu_carpeta(&self, id: u32, r: Rect, cx: &mut Ctx) {
        let mut items: Vec<MenuItem> = self.datos.favs.hijos(id).into_iter().map(|h| MenuItem::new(h.id, &dialogs::fit(&h.titulo, 9, 280))).collect();
        if items.is_empty() {
            items.push(MenuItem::new(0, "(Carpeta vacía)").disabled(true));
        }
        cx.menu = Some(MenuRequest { tag: TAG_FAV, x: r.x, y: r.bottom() + 2, items });
    }

    /// Abre el menú de lo que no cabe en la barra de favoritos.
    fn menu_resto(&self, r: Rect, cx: &mut Ctx) {
        let n = self.items_fav().0.len();
        let items: Vec<MenuItem> = self.datos.favs.hijos(0).into_iter().skip(n).map(|f| self.menu_de(f)).collect();
        if !items.is_empty() {
            cx.menu = Some(MenuRequest { tag: TAG_FAV, x: r.x, y: r.bottom() + 2, items });
        }
    }

    /// Abre un favorito en la pestaña actual (o en una nueva).
    fn abrir_fav(&mut self, id: u32, nueva: bool, cx: &mut Ctx) {
        let Some(url) = self.datos.favs.por_id(id).filter(|f| !f.carpeta).map(|f| f.url.clone()) else { return };
        self.abrir_url(&url, nueva, cx);
    }

    fn abrir_url(&mut self, url: &str, nueva: bool, cx: &mut Ctx) {
        if nueva {
            self.nueva(cx.sys, clasificar(url), true, true, cx.now_ms);
        } else {
            self.ir_a(url, cx);
        }
    }

    /// La estrella: añade la página actual a los favoritos (a la barra) o la quita.
    fn alternar_favorito(&mut self, cx: &mut Ctx) {
        if !self.estrella_visible() {
            return;
        }
        let (url, titulo) = (String::from(self.pest[self.act].url_visible()), self.pest[self.act].titulo_visible());
        match self.datos.favs.buscar_url(&url) {
            Some(id) => {
                self.datos.favs.quitar(id);
            }
            None => {
                self.datos.favs.anadir(&url, &titulo, 0);
            }
        }
        self.datos.cambio_favoritos(cx.now_ms);
    }

    // ------------------------------------------------------------ páginas internas e incógnito

    /// Abre una página interna: en la pestaña si está vacía, y si no, en una nueva.
    fn abrir_interna(&mut self, url: &str, cx: &mut Ctx) {
        if self.pest[self.act].url_visible().is_empty() {
            self.ir_interno(url, cx);
        } else {
            self.nueva(cx.sys, Some(Entrada::Web(String::from(url))), true, true, cx.now_ms);
        }
    }

    /// Navega a una dirección interna que ha pedido el usuario (barra, atajo, menú) o una página interna.
    /// Si trae una acción (borrar, mover…), se hace y la entrada del historial se sustituye por la página sin ella.
    fn ir_interno(&mut self, url: &str, cx: &mut Ctx) {
        let ahora = hora::segundos(cx.sys.now());
        self.aviso = None;
        self.enlace = None;
        self.foco = Foco::Pagina;
        let a = self.act;
        match internas::aplicar(&mut self.datos, url, ahora, cx.now_ms) {
            Some(destino) => self.pest[a].reemplazar(cx.sys, &destino, cx.now_ms),
            None => self.pest[a].ir(cx.sys, Entrada::Web(String::from(url)), cx.now_ms),
        }
    }

    /// Pide al escritorio una ventana nueva de este programa y le dice que es de incógnito.
    fn nueva_incognito(&mut self, cx: &mut Ctx) {
        let ruta = nf_core::files::join(&cx.sys.fs().home(), MARCA_INCOGNITO);
        cx.open = Some(ruta);
    }

    fn menu_principal(&self, cx: &mut Ctx) {
        let barra = self.barra_fav_visible();
        let items = alloc::vec![
            MenuItem::new(M_NUEVA, "Nueva pestaña").shortcut("Ctrl+T"),
            MenuItem::new(M_INCOG, "Nueva ventana de incógnito").shortcut("Ctrl+Mayús+N"),
            MenuItem::sep(),
            MenuItem::new(M_HIST, "Historial").shortcut("Ctrl+H"),
            MenuItem::new(M_FAVS, "Administrar favoritos").shortcut("Ctrl+Mayús+O"),
            MenuItem::new(M_BARRA, "Barra de favoritos").shortcut("Ctrl+Mayús+B").checked(barra),
            MenuItem::new(M_ESTRELLA, if self.es_favorita() { "Quitar de favoritos" } else { "Añadir a favoritos" }).shortcut("Ctrl+D").disabled(!self.estrella_visible()),
        ];
        let r = rect_menu(self.size.0);
        cx.menu = Some(MenuRequest { tag: TAG_MENU, x: r.right() - 250, y: r.bottom() + 2, items });
    }

    // ------------------------------------------------------------ sugerencias

    fn actualizar_sugerencias(&mut self) {
        self.sug = sugerir(&self.datos.hist, &self.datos.favs, &self.barra.texto(), SUG_MAX);
        self.sug_sel = None;
    }

    fn limpiar_sugerencias(&mut self) {
        self.sug.clear();
        self.sug_sel = None;
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
        self.limpiar_sugerencias();
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
            self.guardar_datos(cx.sys, cx.now_ms, true);
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
        self.limpiar_sugerencias();
    }

    fn ir_a(&mut self, texto: &str, cx: &mut Ctx) {
        self.limpiar_sugerencias();
        // `nightfury://` solo se abre desde aquí (lo escrito por el usuario), no desde una página.
        if internas::es_interna(texto.trim()) {
            return self.ir_interno(texto.trim(), cx);
        }
        if let Some(e) = clasificar(texto) {
            self.aviso = None;
            self.pest[self.act].ir(cx.sys, e, cx.now_ms);
            self.foco = Foco::Pagina;
            self.enlace = None;
        }
    }

    fn abrir_enlace(&mut self, url: &str, nueva: bool, cx: &mut Ctx) {
        // Una página web no puede abrir ni enlazar las internas: solo una interna lleva a otra.
        if let Some(i) = internas::de_enlace(url) {
            if self.pest[self.act].es_interna() {
                self.ir_interno(&i, cx);
            }
            return;
        }
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
        if let Some(i) = internas::de_enlace(&e.url) {
            if self.pest[a].es_interna() && e.metodo == Metodo::Get {
                self.ir_interno(&i, cx);
            }
            return;
        }
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
        let (w, n, wt, c) = (self.size.0, self.pest.len(), self.ancho_tira(), paleta(self.incognito));
        s.fill_rect(Rect::new(0, 0, w, TAB_H), c.tira);
        let ang = self.giro() as f32;
        for (i, t) in self.pest.iter().enumerate() {
            let r = rect_pestana(i, n, wt);
            let activa = i == self.act;
            let sobre = matches!(self.hover, Hit::Pestana(k) | Hit::Cerrar(k) if k == i);
            if activa {
                caja(s, Rect::new(r.x, r.y, r.w, r.h + 1), c.barra);
            } else if sobre {
                caja(s, Rect::new(r.x + 2, r.y + 2, r.w - 4, r.h - 4), c.sobre);
            } else if i + 1 < n && i + 1 != self.act {
                s.fill_rect(Rect::new(r.right() - 1, r.y + 8, 1, r.h - 16), c.separador);
            }
            let (cx, cy) = ((r.x + 16) as f32, (r.y + r.h / 2) as f32);
            if t.cargando() || t.pendiente.is_some() {
                s.arc(cx, cy, 6.0, ang, ang + 270.0, 2.0, theme.accent);
            } else {
                icono(s, Ico::Globo, cx, cy, c.gris);
            }
            let ancho = (r.w - 30 - 28).max(0);
            let txt = dialogs::fit(&t.titulo_visible(), TS, ancho);
            con_clip(s, Rect::new(r.x, r.y, r.w - 26, r.h), |s| {
                font::draw(s, r.x + 30, r.y + (r.h - font::line_h(TS)) / 2, &txt, TS, false, if activa { c.tinta } else { c.apagado });
            });
            let x = rect_cerrar(r);
            if self.hover == Hit::Cerrar(i) {
                s.fill_circle((x.x + 9) as f32, (x.y + 9) as f32, 9.0, c.linea, 255);
            }
            icono(s, Ico::Cerrar, (x.x + 9) as f32, (x.y + 9) as f32, c.apagado);
        }
        let m = rect_mas(n, wt);
        if self.hover == Hit::Mas {
            s.fill_circle((m.x + 14) as f32, (m.y + 14) as f32, 13.0, c.sobre_fuerte, 255);
        }
        icono(s, Ico::Mas, (m.x + 14) as f32, (m.y + 14) as f32, c.apagado);
        if self.incognito {
            let x0 = w - ANCHO_INCOG + 8;
            icono(s, Ico::Incognito, (x0 + 10) as f32, (TAB_H / 2) as f32, c.tinta);
            font::draw(s, x0 + 26, (TAB_H - font::line_h(TS_S)) / 2, "Incógnito", TS_S, false, c.tinta);
        }
    }

    /// Ángulo del indicador de carga, según el reloj.
    fn giro(&self) -> i32 {
        (self.reloj / 3 % 360) as i32
    }

    fn dibujar_barra(&self, s: &mut Surface, theme: &Theme) {
        let w = self.size.0;
        let (t, c) = (&self.pest[self.act], paleta(self.incognito));
        s.fill_rect(Rect::new(0, TAB_H, w, TOOL_H), c.barra);
        s.fill_rect(Rect::new(0, TAB_H + TOOL_H - 1, w, 1), c.linea);
        let botones = [(0, Ico::Atras, t.puede_atras(), Hit::Atras), (1, Ico::Adelante, t.puede_adelante(), Hit::Adelante), (2, if t.cargando() || t.pendiente.is_some() { Ico::Detener } else { Ico::Recargar }, true, Hit::Recargar)];
        for (k, ico, activo, hit) in botones {
            let r = rect_boton(k);
            let (cx, cy) = ((r.x + 16) as f32, (r.y + 16) as f32);
            if self.hover == hit && activo {
                s.fill_circle(cx, cy, 16.0, c.sobre, 255);
            }
            icono(s, ico, cx, cy, if activo { c.apagado } else { c.inactivo });
        }
        let m = rect_menu(w);
        if self.hover == Hit::Menu {
            s.fill_circle((m.x + 16) as f32, (m.y + 16) as f32, 16.0, c.sobre, 255);
        }
        icono(s, Ico::Menu, (m.x + 16) as f32, (m.y + 16) as f32, c.apagado);
        let r = rect_omnibox(w);
        let editando = self.foco == Foco::Barra;
        capsula(s, r, if editando { theme.accent } else { c.caja });
        if editando {
            capsula(s, r.inflate(-2), c.caja_edit);
        }
        let lh = font::line_h(TS);
        let ty = r.y + (r.h - lh) / 2;
        // A la derecha queda la estrella (si la hay).
        let reserva = if self.estrella_visible() { 30 } else { 0 };
        con_clip(s, r.inflate(-2), |s| {
            if editando {
                let (x0, ancho) = (r.x + 16, r.w - 32 - reserva);
                let (txt, cur, wv) = self.barra.visible(ancho, TS);
                if self.barra.todo {
                    s.blend_rect(Rect::new(x0, ty, wv.max(1), lh), theme.accent, 70);
                }
                font::draw(s, x0, ty, &txt, TS, false, c.tinta);
                if !self.barra.todo {
                    s.fill_rect(Rect::new(x0 + cur, ty + 2, 1, lh - 4), c.tinta);
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
                    s.fill_rect(Rect::new(x0, r.y + 8, 1, r.h - 16), c.linea);
                    x0 += 8;
                }
                Seguridad::Archivo => {
                    x0 = font::draw(s, x0, ty, "Archivo", TS, false, c.apagado) + 8;
                    s.fill_rect(Rect::new(x0, r.y + 8, 1, r.h - 16), c.linea);
                    x0 += 8;
                }
                Seguridad::Ninguna => {}
            }
            if u.is_empty() {
                font::draw(s, x0, ty, "Busca o escribe una dirección", TS, false, c.gris);
            } else {
                font::draw(s, x0, ty, &dialogs::fit(u, TS, (r.right() - 16 - reserva - x0).max(0)), TS, false, c.tinta);
            }
        });
        if reserva > 0 {
            let e = rect_estrella(r);
            let (ex, ey) = ((e.x + e.w / 2) as f32, (e.y + e.h / 2) as f32);
            if self.hover == Hit::Estrella {
                s.fill_circle(ex, ey, 13.0, c.sobre, 255);
            }
            let fav = self.es_favorita();
            icono(s, if fav { Ico::EstrellaLlena } else { Ico::Estrella }, ex, ey, if fav { theme.accent } else { c.apagado });
        }
    }

    /// Barra de favoritos bajo la de direcciones: círculo con la inicial y nombre, carpetas con flecha.
    fn dibujar_favoritos(&self, s: &mut Surface) {
        if !self.barra_fav_visible() {
            return;
        }
        let (c, y0) = (paleta(self.incognito), TAB_H + TOOL_H);
        s.fill_rect(Rect::new(0, y0, self.size.0, FAV_H), c.barra);
        s.fill_rect(Rect::new(0, y0 + FAV_H - 1, self.size.0, 1), c.linea);
        let (items, mas) = self.items_fav();
        for (r, id) in items {
            let Some(f) = self.datos.favs.por_id(id) else { continue };
            if self.hover == Hit::Fav(id) {
                caja(s, r, c.sobre);
            }
            let cy = (r.y + r.h / 2) as f32;
            let ancho = if f.carpeta { r.w - 44 } else { r.w - 32 };
            if f.carpeta {
                icono(s, Ico::Carpeta, (r.x + 15) as f32, cy, c.apagado);
                icono(s, Ico::Abajo, (r.right() - 12) as f32, cy, c.apagado);
            } else {
                disco_inicial(s, (r.x + 14) as f32, cy, 7.0, &host(&f.url).unwrap_or_default(), &f.titulo);
            }
            font::draw(s, r.x + 28, r.y + (r.h - font::line_h(TS_S)) / 2, &dialogs::fit(&f.titulo, TS_S, ancho.max(0)), TS_S, false, c.tinta);
        }
        if let Some(r) = mas {
            if self.hover == Hit::FavMas {
                caja(s, r, c.sobre);
            }
            font::draw_in(s, r, "»", TS, true, c.apagado, font::Align::Center);
        }
    }

    /// Lista de sugerencias bajo la barra de direcciones (encima de la página).
    fn dibujar_sugerencias(&self, s: &mut Surface, theme: &Theme) {
        if self.foco != Foco::Barra || self.sug.is_empty() {
            return;
        }
        let (c, w) = (paleta(self.incognito), self.size.0);
        let (primera, ultima) = (rect_sug(0, w), rect_sug(self.sug.len() - 1, w));
        let panel = Rect::new(primera.x, primera.y - 4, primera.w, ultima.bottom() - primera.y + 8);
        s.fill_rect(panel, c.barra);
        s.frame(panel, c.linea);
        for (i, g) in self.sug.iter().enumerate() {
            let r = rect_sug(i, w);
            if self.sug_sel == Some(i) {
                s.fill_rect(Rect::new(r.x + 1, r.y, r.w - 2, r.h), c.sobre_fuerte);
            }
            let cy = (r.y + r.h / 2) as f32;
            disco_inicial(s, (r.x + 22) as f32, cy, 10.0, &host(&g.url).unwrap_or_default(), &g.titulo);
            let ancho = (r.w - 48 - 40).max(0);
            let titulo = if g.titulo.is_empty() { &g.url } else { &g.titulo };
            font::draw(s, r.x + 44, r.y + 4, &dialogs::fit(titulo, TS, ancho), TS, false, c.tinta);
            font::draw(s, r.x + 44, r.y + 4 + font::line_h(TS), &dialogs::fit(&g.url, TS_S, ancho), TS_S, false, c.gris);
            if g.favorito {
                icono(s, Ico::EstrellaLlena, (r.right() - 22) as f32, cy, theme.accent);
            }
        }
    }

    fn dibujar_inicio(&mut self, s: &mut Surface, theme: &Theme, p: Rect, focused: bool) {
        let pal = paleta(self.incognito);
        s.fill_rect(p, pal.inicio);
        let px = 56.0;
        let l = Letra { familia: Familia::Sans, negrita: true, cursiva: false };
        let (wn, wf) = (self.letras.medir(l, px, "Night"), self.letras.medir(l, px, "Fury"));
        let x0 = p.x as f32 + (p.w as f32 - wn - wf) / 2.0;
        let base = (p.y + p.h * 45 / 100 - 56) as f32;
        self.letras.dibujar(s, l, px, x0, base, pal.tinta, "Night");
        self.letras.dibujar(s, l, px, x0 + wn, base, theme.accent, "Fury");
        let r = rect_caja(p);
        let caja_foco = self.foco == Foco::Pagina && focused;
        capsula(s, r, if caja_foco { theme.accent } else { pal.borde_inicio });
        capsula(s, r.inflate(-if caja_foco { 2 } else { 1 }), pal.caja_edit);
        icono(s, Ico::Lupa, (r.x + 26) as f32, (r.y + 22) as f32, pal.gris);
        let c = &self.pest[self.act].caja;
        let (x0, ancho, lh) = (r.x + 48, r.w - 72, font::line_h(TS + 2));
        let ty = r.y + (r.h - lh) / 2;
        if c.vacio() {
            font::draw(s, x0, ty, "Buscar en la web", TS + 2, false, pal.gris);
        }
        let (txt, cur, _) = c.visible(ancho, TS + 2);
        font::draw(s, x0, ty, &txt, TS + 2, false, pal.tinta);
        if caja_foco {
            s.fill_rect(Rect::new(x0 + cur, ty + 2, 1, lh - 4), pal.tinta);
        }
        if self.incognito {
            let (lh1, lh2) = (font::line_h(TS + 4), font::line_h(TS_S));
            let y = r.bottom() + 18;
            icono(s, Ico::Incognito, (p.x + p.w / 2 - 44) as f32, (y + lh1 / 2) as f32, pal.tinta);
            font::draw(s, p.x + p.w / 2 - 28, y, "Incógnito", TS + 4, true, pal.tinta);
            for (i, l) in self.lineas_incognito(p).iter().enumerate() {
                font::draw_in(s, Rect::new(p.x, y + lh1 + 8 + i as i32 * (lh2 + 3), p.w, lh2), l, TS_S, false, pal.apagado, font::Align::Center);
            }
        } else {
            font::draw_in(s, Rect::new(p.x, r.bottom() + 14, p.w, 16), "Las búsquedas se hacen en DuckDuckGo", TS_S, false, pal.gris, font::Align::Center);
        }
        // Accesos a los primeros favoritos: círculo con la inicial y el nombre debajo.
        for (t, f) in self.accesos(p) {
            if self.hover_acceso == Some(f.id) {
                caja(s, t, pal.sobre);
            }
            disco_inicial(s, (t.x + t.w / 2) as f32, (t.y + 28) as f32, 20.0, &host(&f.url).unwrap_or_default(), &f.titulo);
            font::draw_in(s, Rect::new(t.x + 4, t.y + 54, t.w - 8, 18), &dialogs::fit(&f.titulo, TS_S, t.w - 8), TS_S, false, pal.tinta, font::Align::Center);
        }
    }

    /// Texto de la pestaña de incógnito, partido en líneas del ancho de la página.
    fn lineas_incognito(&self, p: Rect) -> Vec<String> {
        envolver(TEXTO_INCOGNITO, TS_S, (p.w - 80).clamp(120, 520))
    }

    /// Los accesos a favoritos de la pestaña nueva: su casilla y el favorito.
    fn accesos(&self, p: Rect) -> Vec<(Rect, &datos::Fav)> {
        let favs: Vec<&datos::Fav> = self.datos.favs.planos().into_iter().take(MAX_ACCESOS).collect();
        if favs.is_empty() {
            return Vec::new();
        }
        let r = rect_caja(p);
        let y0 = if self.incognito { r.bottom() + 18 + font::line_h(TS + 4) + 8 + self.lineas_incognito(p).len() as i32 * (font::line_h(TS_S) + 3) + 20 } else { r.bottom() + 48 };
        let cols = (((p.w - 40) / 112).clamp(1, MAX_ACCESOS as i32) as usize).min(favs.len());
        let x0 = p.x + (p.w - cols as i32 * 112) / 2;
        favs.into_iter().enumerate().map(|(i, f)| (Rect::new(x0 + (i % cols) as i32 * 112, y0 + (i / cols) as i32 * 82, 108, 76), f)).collect()
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
        if self.incognito {
            return alloc::format!("{t} - Navegador (incógnito)");
        }
        alloc::format!("{t} - Navegador")
    }

    fn min_size(&self) -> (i32, i32) {
        (480, 300)
    }

    fn draw(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, focused: bool) {
        self.size = (s.w, s.h);
        self.reloj = sys.uptime_ms();
        self.cargar_datos(sys);
        self.servir_internas(sys);
        let (p, ahora, a) = (self.pagina(), self.reloj, self.act);
        let ancho = (p.w - SB_W).max(1);
        self.pest[a].preparar(ancho, &mut self.letras, sys.fs(), ahora);
        let max = self.max_scroll();
        self.pest[a].scroll = self.pest[a].scroll.clamp(0, max);
        s.fill_rect(Rect::new(0, 0, s.w, s.h), 0xFFFFFF);
        self.dibujar_pagina(s, theme, focused);
        self.dibujar_pestanas(s, theme);
        self.dibujar_barra(s, theme);
        self.dibujar_favoritos(s);
        self.dibujar_sugerencias(s, theme);
    }

    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
        // El escritorio avisa con `Key::Other` de cada cambio de Ctrl y Mayús.
        self.ctrl = ev.ctrl;
        if ev.key == Key::Other {
            return;
        }
        let (a, ahora) = (self.act, cx.now_ms);
        cx.redraw = true;
        self.cargar_datos(cx.sys);
        if ev.ctrl {
            match ev.key {
                Key::Char('n') if ev.shift => return self.nueva_incognito(cx),
                Key::Char('b') if ev.shift => {
                    self.barra_fav = Some(!self.barra_fav_visible());
                    return;
                }
                Key::Char('o') if ev.shift => return self.abrir_interna(URL_FAVORITOS, cx),
                Key::Char('h') => return self.abrir_interna(URL_HISTORIAL, cx),
                Key::Char('d') => return self.alternar_favorito(cx),
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
                if !self.sug.is_empty() {
                    return self.limpiar_sugerencias();
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
            let n = self.sug.len();
            match ev.key {
                Key::Down if n > 0 => {
                    self.sug_sel = Some(self.sug_sel.map_or(0, |i| (i + 1) % n));
                    return;
                }
                Key::Up if n > 0 => {
                    self.sug_sel = Some(self.sug_sel.map_or(n - 1, |i| (i + n - 1) % n));
                    return;
                }
                Key::Enter if self.sug_sel.is_some() => {
                    if let Some(g) = self.sug_sel.and_then(|i| self.sug.get(i)) {
                        let u = g.url.clone();
                        self.ir_a(&u, cx);
                    }
                    return;
                }
                _ => {}
            }
            match self.barra.tecla(ev) {
                Accion::Enter => {
                    let t = self.barra.texto();
                    self.ir_a(&t, cx);
                }
                Accion::Cambio => self.actualizar_sugerencias(),
                Accion::Nada => {}
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
        self.cargar_datos(cx.sys);
        if let MouseEv::Down { x, y } = ev {
            if !matches!(self.hit(x, y), Hit::Sug(_) | Hit::Barra) {
                self.limpiar_sugerencias();
            }
        }
        match ev {
            MouseEv::Down { x, y } => match self.hit(x, y) {
                Hit::Sug(i) => {
                    if let Some(g) = self.sug.get(i) {
                        let u = g.url.clone();
                        self.ir_a(&u, cx);
                    }
                }
                Hit::Estrella => self.alternar_favorito(cx),
                Hit::Menu => self.menu_principal(cx),
                Hit::Fav(id) => match self.datos.favs.por_id(id).map(|f| f.carpeta) {
                    Some(true) => {
                        if let Some(r) = self.items_fav().0.iter().find(|(_, i)| *i == id).map(|(r, _)| *r) {
                            self.menu_carpeta(id, r, cx);
                        }
                    }
                    Some(false) => self.abrir_fav(id, self.ctrl, cx),
                    None => {}
                },
                Hit::FavMas => {
                    if let Some(r) = self.items_fav().1 {
                        self.menu_resto(r, cx);
                    }
                }
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
                    let reserva = if self.estrella_visible() { 30 } else { 0 };
                    self.barra.clic(x - (r.x + 16), r.w - 32 - reserva, TS);
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
                    if matches!(self.pest[a].vista, Vista::Inicio) {
                        if let Some(id) = self.accesos(p).iter().find(|(r, _)| r.contains(x, y)).map(|(_, f)| f.id) {
                            return self.abrir_fav(id, self.ctrl, cx);
                        }
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
                if let Hit::Sug(i) = self.hover {
                    self.sug_sel = Some(i);
                }
                let p = self.pagina();
                self.hover_acceso = match self.pest[a].vista {
                    Vista::Inicio if self.hover == Hit::Pagina => self.accesos(p).iter().find(|(r, _)| r.contains(x, y)).map(|(_, f)| f.id),
                    _ => None,
                };
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
        self.cargar_datos(cx.sys);
        let (mut anima, seg) = (false, hora::segundos(cx.sys.now()));
        for t in self.pest.iter_mut() {
            anima |= t.tick(cx.sys, ahora);
            anima |= t.cargando() || t.pendiente.is_some();
            // Página recién mostrada: al historial, salvo en incógnito (ahí no se guarda nada).
            if core::mem::take(&mut t.visita) && !self.incognito && self.datos.hist.apuntar(seg, &t.url, &t.titulo) {
                self.datos.cambio_historial(ahora);
            }
        }
        self.servir_internas(cx.sys);
        self.guardar_datos(cx.sys, ahora, false);
        let m = self.metricas();
        if let Some(p) = self.sb.tick(&m, ahora) {
            self.desplazar_a(p);
            anima = true;
        }
        if anima {
            cx.redraw = true;
        }
    }

    fn menu_result(&mut self, tag: u32, id: u32, cx: &mut Ctx) {
        cx.redraw = true;
        self.cargar_datos(cx.sys);
        match (tag, id) {
            (TAG_FAV, id) if id != 0 => self.abrir_fav(id, false, cx),
            (TAG_MENU, M_NUEVA) => self.nueva(cx.sys, None, true, false, cx.now_ms),
            (TAG_MENU, M_INCOG) => self.nueva_incognito(cx),
            (TAG_MENU, M_HIST) => self.abrir_interna(URL_HISTORIAL, cx),
            (TAG_MENU, M_FAVS) => self.abrir_interna(URL_FAVORITOS, cx),
            (TAG_MENU, M_BARRA) => self.barra_fav = Some(!self.barra_fav_visible()),
            (TAG_MENU, M_ESTRELLA) => self.alternar_favorito(cx),
            _ => {}
        }
    }

    fn open_path(&mut self, path: &str, cx: &mut Ctx) {
        // La marca que pide `nueva_incognito`: esta ventana es la de incógnito.
        if path.to_ascii_lowercase().ends_with(".nfinc") {
            self.poner_incognito();
            cx.redraw = true;
            return;
        }
        if es_ruta_local(path) {
            if let Some(e) = clasificar(path) {
                self.pest[self.act].ir(cx.sys, e, cx.now_ms);
                self.foco = Foco::Pagina;
                cx.redraw = true;
            }
        }
    }

    fn request_close(&mut self, cx: &mut Ctx) -> bool {
        // Al cerrar se escribe lo que estuviera esperando su turno.
        self.guardar_datos(cx.sys, cx.now_ms, true);
        true
    }

    fn release(&mut self, sys: &mut dyn Platform) {
        for t in self.pest.iter_mut() {
            t.cancelar(sys);
        }
        self.guardar_datos(sys, 0, true);
    }
}
