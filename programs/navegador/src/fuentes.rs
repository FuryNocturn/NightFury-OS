// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Las letras de las páginas: `nf_web::Fuentes` con TrueType. Se prefieren Arial, Times y Courier de una
//! carpeta `\Windows\Fonts\` de cualquier unidad (solo se leen); si no, las Liberation del sistema. Cada
//! variante se carga la primera vez que una página la pide (`cargar`); mientras tanto vale otra, y si no hay
//! ninguna, la letra de trazos del sistema.
use alloc::format;
use alloc::vec::Vec;
use nf_core::font as trazos;
use nf_core::fsapi::FilesApi;
use nf_core::surface::Surface;
use nf_core::ttf::{self, Font, Raster, TextCache};
use nf_web::{Familia, Fuentes, Letra};

const LIBERATION: [&str; 3] = ["Sans", "Serif", "Mono"];
const ESTILOS: [&str; 4] = ["Regular", "Bold", "Italic", "BoldItalic"];
const WINDOWS: [[&str; 4]; 3] = [
    ["arial.ttf", "arialbd.ttf", "ariali.ttf", "arialbi.ttf"],
    ["times.ttf", "timesbd.ttf", "timesi.ttf", "timesbi.ttf"],
    ["cour.ttf", "courbd.ttf", "couri.ttf", "courbi.ttf"],
];

pub struct Letras {
    fuentes: [Option<Font>; 12],
    intentada: [bool; 12],
    /// Variantes que se han pedido sin tenerlas cargadas (un bit cada una).
    pedidas: u16,
    cache: TextCache,
    unidades: Option<Vec<char>>,
}

fn id(l: Letra) -> usize {
    let f = match l.familia {
        Familia::Sans => 0,
        Familia::Serif => 1,
        Familia::Mono => 2,
    };
    f * 4 + l.negrita as usize + 2 * l.cursiva as usize
}

impl Letras {
    pub fn nuevas() -> Letras {
        Letras { fuentes: core::array::from_fn(|_| None), intentada: [false; 12], pedidas: 0, cache: TextCache::new(), unidades: None }
    }

    /// ¿Se ha pedido alguna variante que aún no se ha intentado cargar? Si es así, `cargar` y volver a maquetar.
    pub fn faltan(&self) -> bool {
        self.pedidas != 0
    }

    /// Carga las variantes pedidas. `true` si ha entrado alguna nueva.
    pub fn cargar(&mut self, fs: &mut dyn FilesApi) -> bool {
        let mut nueva = false;
        for i in 0..12 {
            if self.pedidas & 1 << i == 0 {
                continue;
            }
            self.pedidas &= !(1 << i);
            self.intentada[i] = true;
            if let Some(f) = self.leer(fs, i) {
                self.fuentes[i] = Some(f);
                nueva = true;
            }
        }
        nueva
    }

    fn leer(&mut self, fs: &mut dyn FilesApi, i: usize) -> Option<Font> {
        let unidades = self.unidades.get_or_insert_with(|| fs.drives().iter().filter(|d| !d.locked).map(|d| d.letter).collect());
        for u in unidades.iter() {
            if let Ok(f) = fs.read(&format!("{u}:\\Windows\\Fonts\\{}", WINDOWS[i / 4][i % 4])).map_err(|_| ()).and_then(|d| Font::parse(d).map_err(|_| ())) {
                return Some(f);
            }
        }
        let ruta = format!("C:\\Sistema\\fuentes\\Liberation{}-{}.ttf", LIBERATION[i / 4], ESTILOS[i % 4]);
        fs.read(&ruta).ok().and_then(|d| Font::parse(d).ok())
    }

    /// La fuente cargada para `l` o, si no está, la más parecida que haya (empezando por su familia).
    fn usable(&mut self, l: Letra) -> Option<usize> {
        let i = id(l);
        if self.fuentes[i].is_some() {
            return Some(i);
        }
        if !self.intentada[i] {
            self.pedidas |= 1 << i;
        }
        (0..12).map(|k| (i / 4 * 4 + k) % 12).find(|&j| self.fuentes[j].is_some())
    }

    #[cfg(test)]
    pub fn poner(&mut self, i: usize, f: Font) {
        self.fuentes[i] = Some(f);
        self.intentada[i] = true;
    }
}

/// Tamaño de la letra de trazos para `px`.
fn tam(px: f32) -> u32 {
    (px as u32).clamp(6, 96)
}

impl Fuentes for Letras {
    fn medir(&mut self, l: Letra, px: f32, t: &str) -> f32 {
        match self.usable(l) {
            Some(i) => self.fuentes[i].as_ref().map_or(0.0, |f| ttf::measure(f, px, t)),
            None => trazos::width(t, tam(px), l.negrita) as f32,
        }
    }

    fn metricas(&mut self, l: Letra, px: f32) -> (f32, f32) {
        match self.usable(l).and_then(|i| self.fuentes[i].as_ref()) {
            Some(f) => {
                let e = px / f.units_per_em().max(1) as f32;
                (f.ascender() as f32 * e, -(f.descender() as f32) * e)
            }
            None => (px * 0.9, px * 0.25),
        }
    }

    fn dibujar(&mut self, s: &mut Surface, l: Letra, px: f32, x: f32, base: f32, color: u32, t: &str) {
        match self.usable(l) {
            Some(i) => {
                if let Some(f) = self.fuentes[i].as_ref() {
                    ttf::draw_text(s, &mut self.cache, i as u32, f, x, base, px, color, Raster::default(), t);
                }
            }
            // shortcut: sin ninguna TrueType se pinta con la letra de trazos, sin cursiva; pulir si llega a verse.
            None => {
                let size = tam(px);
                trazos::draw(s, x as i32, base as i32 - size as i32 - size as i32 / 8, t, size, l.negrita, color);
            }
        }
    }
}
