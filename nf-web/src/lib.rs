// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! NightFury: motor de páginas web (fase 1, sin JavaScript). HTML tolerante, CSS, maquetado en bloques y
//! líneas, tablas, listas, imágenes y pintado sobre una `Surface`. Hecho desde las normas públicas (HTML
//! Living Standard, CSS 2.1 y CSS Syntax 3, RFC 3986). Toda la entrada es hostil: sin pánicos, con límites.
#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod css;
pub mod estilo;
pub mod html;
mod maqueta;
mod pinta;
pub mod url;
pub mod valores;

#[cfg(test)]
mod pruebas;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use css::{Hoja, Medias};
use estilo::{Estilos, FuenteHoja};
use maqueta::{EstadoImg, Img, Maq, Resultado, SIN_IMG};
use nf_core::bmp::Picture;
use nf_core::surface::Surface;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Familia {
    Sans,
    Serif,
    Mono,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Letra {
    pub familia: Familia,
    pub negrita: bool,
    pub cursiva: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TipoRecurso {
    Css,
    Imagen,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Recurso {
    pub url: String,
    pub tipo: TipoRecurso,
}

/// Lo que el motor necesita de las fuentes de letra (lo implementa el programa con `nf_core::ttf`).
pub trait Fuentes {
    fn medir(&mut self, l: Letra, px: f32, texto: &str) -> f32;
    /// (ascendente, descendente positivo)
    fn metricas(&mut self, l: Letra, px: f32) -> (f32, f32);
    fn dibujar(&mut self, s: &mut Surface, l: Letra, px: f32, x: f32, linea_base: f32, color: u32, texto: &str);
}

/// Recursos externos que un documento puede pedir como mucho (hojas e imágenes por separado).
const MAX_RECURSOS: usize = 2000;

struct FuenteCss {
    url: Option<String>,
    hoja: Option<Hoja>,
    medias: Medias,
}

pub struct Documento {
    dom: html::Dom,
    base: String,
    titulo: Option<String>,
    css: Vec<FuenteCss>,
    imgs: Vec<Img>,
    /// Imagen de cada nodo (`SIN_IMG` si no es una imagen con URL válida).
    img_de: Vec<u32>,
    recursos: Vec<Recurso>,
    ua: Hoja,
    estilos: Option<(Estilos, i32)>,
    res: Option<Resultado>,
    /// `<meta http-equiv="refresh" content="N; url=…">`: segundos y adónde (ya resuelta).
    refresco: Option<(u32, String)>,
}

/// `content` de un `<meta http-equiv="refresh">`: `N` o `N; url=…` (comillas opcionales). Solo con URL.
fn leer_refresco(base: &str, c: &str) -> Option<(u32, String)> {
    let (n, resto) = c.split_once([';', ',']).unwrap_or((c, ""));
    let seg = n.trim().split('.').next()?.parse::<u32>().ok()?;
    let resto = resto.trim_start();
    // `get`: un carácter de varios bytes en las tres primeras posiciones no debe cortar la cadena por la mitad.
    let u = if resto.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("url")) {
        resto[3..].trim_start().strip_prefix('=')?.trim()
    } else {
        resto.trim()
    };
    let u = u.trim_matches(|c| c == '\'' || c == '"').trim();
    if u.is_empty() {
        return None;
    }
    Some((seg, url::resolver(base, u)?))
}

fn media_vale(m: Option<&str>) -> Medias {
    match m {
        Some(m) if !m.trim().is_empty() => alloc::vec![css::parsear_media(m)],
        _ => Vec::new(),
    }
}

impl Documento {
    /// `tipo` es el Content-Type (para el charset); `url` la URL final (tras redirecciones).
    pub fn cargar(html: &[u8], tipo: Option<&str>, url: &str) -> Documento {
        let texto = html::decodificar(html, tipo);
        let dom = html::parsear(&texto);
        drop(texto);
        let n = dom.nodos.len();
        let mut base = url.to_string();
        if let Some(b) = (0..n).find(|&i| dom.etq(i) == "base" && dom.attr(i, "href").is_some()) {
            if let Some(u) = dom.attr(b, "href").and_then(|h| url::resolver(url, h)) {
                base = u;
            }
        }
        let mut d = Documento {
            dom,
            base,
            titulo: None,
            css: Vec::new(),
            imgs: Vec::new(),
            img_de: alloc::vec![SIN_IMG; n],
            recursos: Vec::new(),
            ua: css::parsear_hoja(estilo::UA, Vec::new()),
            estilos: None,
            res: None,
            refresco: None,
        };
        let mut n_css = 0;
        for i in 0..n {
            let dom = &d.dom;
            match dom.etq(i) {
                "title" if d.titulo.is_none() => d.titulo = Some(html::compactar(&dom.texto(i))),
                "meta" if d.refresco.is_none() && dom.attr(i, "http-equiv").is_some_and(|h| h.trim().eq_ignore_ascii_case("refresh")) => {
                    d.refresco = dom.attr(i, "content").and_then(|c| leer_refresco(&d.base, c));
                }
                "style" => {
                    let tipo_ok = dom.attr(i, "type").map_or(true, |t| t.trim().is_empty() || t.trim().eq_ignore_ascii_case("text/css"));
                    if tipo_ok {
                        let medias = media_vale(dom.attr(i, "media"));
                        let hoja = css::parsear_hoja(&dom.texto(i), medias.clone());
                        d.css.push(FuenteCss { url: None, hoja: Some(hoja), medias });
                    }
                }
                "link" => {
                    let rel = dom.attr(i, "rel").unwrap_or("").to_ascii_lowercase();
                    let mut toks = rel.split_ascii_whitespace();
                    let medias = media_vale(dom.attr(i, "media"));
                    if toks.clone().any(|t| t == "stylesheet")
                        && !toks.any(|t| t == "alternate")
                        && css::media_ok(&medias, 1024.0)
                        && n_css < MAX_RECURSOS
                    {
                        if let Some(u) = dom.attr(i, "href").and_then(|h| url::resolver(&d.base, h)) {
                            n_css += 1;
                            if !d.recursos.iter().any(|r| r.tipo == TipoRecurso::Css && r.url == u) {
                                d.recursos.push(Recurso { url: u.clone(), tipo: TipoRecurso::Css });
                            }
                            d.css.push(FuenteCss { url: Some(u), hoja: None, medias });
                        }
                    }
                }
                "img" => {
                    let src = dom.attr(i, "src").filter(|s| !s.trim().is_empty()).or_else(|| {
                        dom.attr(i, "srcset").and_then(|s| s.split(',').next()).and_then(|c| c.split_ascii_whitespace().next())
                    });
                    if let Some(u) = src.and_then(|s| url::resolver(&d.base, s)) {
                        let k = match d.imgs.iter().position(|im| im.url == u) {
                            Some(k) => k,
                            None if d.imgs.len() < MAX_RECURSOS => {
                                d.recursos.push(Recurso { url: u.clone(), tipo: TipoRecurso::Imagen });
                                d.imgs.push(Img { url: u, estado: EstadoImg::Pendiente, dim: None });
                                d.imgs.len() - 1
                            }
                            None => continue,
                        };
                        d.img_de[i] = k as u32;
                    }
                }
                _ => {}
            }
        }
        d
    }

    pub fn titulo(&self) -> Option<String> {
        self.titulo.clone().filter(|t| !t.is_empty())
    }

    /// Redirección por `<meta http-equiv="refresh">` (segundos, URL absoluta), si la página la pide.
    pub fn refresco(&self) -> Option<(u32, &str)> {
        self.refresco.as_ref().map(|(s, u)| (*s, u.as_str()))
    }

    /// La base efectiva (`<base href>` si lo hay).
    pub fn url(&self) -> &str {
        &self.base
    }

    pub fn recursos(&self) -> Vec<Recurso> {
        self.recursos.clone()
    }

    /// Entrega una hoja pedida; vale en el orden del documento aunque lleguen desordenadas.
    pub fn poner_css(&mut self, url: &str, css: &[u8]) {
        let texto = alloc::string::String::from_utf8_lossy(css);
        for f in self.css.iter_mut().filter(|f| f.url.as_deref() == Some(url)) {
            f.hoja = Some(css::parsear_hoja(&texto, f.medias.clone()));
        }
        self.estilos = None;
    }

    /// `None` = la descarga o la decodificación falló: se dibuja el texto alternativo.
    pub fn poner_imagen(&mut self, url: &str, img: Option<Picture>) {
        for im in self.imgs.iter_mut().filter(|i| i.url == url) {
            im.estado = match &img {
                Some(p) => EstadoImg::Lista(p.clone()),
                None => EstadoImg::Fallo,
            };
        }
    }

    /// Dimensiones conocidas antes de decodificar (reservan el sitio).
    pub fn tamano_imagen(&mut self, url: &str, w: u32, h: u32) {
        for im in self.imgs.iter_mut().filter(|i| i.url == url) {
            im.dim = Some((w, h));
        }
    }

    /// Maqueta para `ancho` px y devuelve el alto total.
    pub fn maquetar(&mut self, ancho: i32, f: &mut dyn Fuentes) -> i32 {
        let ancho = ancho.clamp(1, maqueta::MAX_ANCHO);
        if !matches!(&self.estilos, Some((_, a)) if *a == ancho) {
            let mut hojas = alloc::vec![FuenteHoja { origen: 0, hoja: &self.ua }];
            hojas.extend(self.css.iter().filter_map(|c| c.hoja.as_ref()).map(|hoja| FuenteHoja { origen: 1, hoja }));
            let e = estilo::calcular(&self.dom, &hojas, ancho as f32);
            self.estilos = Some((e, ancho));
        }
        let Some((est, _)) = &self.estilos else { return 0 };
        let r = Maq::nuevo(&self.dom, est, &self.imgs, &self.img_de, &self.base, f).maquetar(ancho);
        let alto = r.alto;
        self.res = Some(r);
        alto
    }

    /// Color de fondo del lienzo (`body` o `html`), 0xRRGGBB.
    pub fn fondo(&self) -> u32 {
        self.estilos.as_ref().and_then(|(e, _)| e.lienzo).unwrap_or(0xFFFFFF)
    }
}
