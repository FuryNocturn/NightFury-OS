// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Maqueta un HTML con las fuentes Liberation reales y guarda la captura como BMP.
//! Uso: `cargo run -p nf-web --example captura -- salida.bmp [pagina.html] [ancho]`
use nf_core::surface::Surface;
use nf_core::ttf::{self, Font, Raster, TextCache};
use nf_web::{Documento, Familia, Fuentes, Letra};

const PAGINA: &str = "<html><head><title>Prueba</title><style>
body { font-family: sans-serif; background: #fafafa }
h1 { color: #1a3c6e; border-bottom: 2px solid #1a3c6e }
.caja { background: #fff3cd; border: 1px solid #c9a227; padding: 8px; margin: 12px 0 }
.der { float: right; width: 120px; background: #ddeeff; padding: 6px; margin-left: 10px }
table { border-collapse: collapse } td, th { border: 1px solid #888; padding: 4px 8px }
a { color: #0645ad } code { font-family: monospace; background: #eee }
</style></head><body>
<h1>NightFury: prueba del motor web</h1>
<div class=der>Un flotante a la derecha.</div>
<p>Texto con <b>negrita</b>, <i>cursiva</i>, <code>monoespaciado</code> y un <a href=\"/otra\">enlace</a>.
El párrafo es lo bastante largo como para dar varias líneas y rodear al flotante. Acentos: áéíóú ñ €.</p>
<div class=caja>Una caja con relleno y borde.</div>
<ul><li>Uno<li>Dos<ol><li>Anidado</ol></ul>
<table><tr><th>Columna A<th>Columna B<tr><td>1<td>texto más largo en la celda</table>
</body></html>";

struct Letras {
    fuentes: Vec<Font>,
    cache: TextCache,
}

impl Letras {
    fn cargar() -> Letras {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../nf-core/data/fuentes/");
        let mut fuentes = Vec::new();
        for fam in ["Sans", "Serif", "Mono"] {
            for est in ["Regular", "Bold", "Italic", "BoldItalic"] {
                let datos = std::fs::read(format!("{dir}Liberation{fam}-{est}.ttf")).expect("fuente");
                fuentes.push(Font::parse(datos).expect("ttf"));
            }
        }
        Letras { fuentes, cache: TextCache::new() }
    }

    fn id(l: Letra) -> usize {
        let f = match l.familia {
            Familia::Sans => 0,
            Familia::Serif => 1,
            Familia::Mono => 2,
        };
        f * 4 + l.negrita as usize + 2 * l.cursiva as usize
    }
}

impl Fuentes for Letras {
    fn medir(&mut self, l: Letra, px: f32, t: &str) -> f32 {
        ttf::measure(&self.fuentes[Self::id(l)], px, t)
    }
    fn metricas(&mut self, l: Letra, px: f32) -> (f32, f32) {
        let f = &self.fuentes[Self::id(l)];
        let e = px / f.units_per_em() as f32;
        (f.ascender() as f32 * e, -(f.descender() as f32) * e)
    }
    fn dibujar(&mut self, s: &mut Surface, l: Letra, px: f32, x: f32, base: f32, color: u32, t: &str) {
        let i = Self::id(l);
        ttf::draw_text(s, &mut self.cache, i as u32, &self.fuentes[i], x, base, px, color, Raster::default(), t);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let salida = args.get(1).map(String::as_str).unwrap_or("captura.bmp");
    let html = match args.get(2) {
        Some(p) => std::fs::read(p).expect("leer html"),
        None => PAGINA.as_bytes().to_vec(),
    };
    let ancho: i32 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(640);
    let mut letras = Letras::cargar();
    let mut d = Documento::cargar(&html, Some("text/html; charset=utf-8"), "http://ejemplo.test/");
    let alto = d.maquetar(ancho, &mut letras);
    let alto = alto.clamp(1, 4000);
    let mut s = Surface::new(ancho, alto, d.fondo());
    d.pintar(&mut s, 0, &mut letras);
    std::fs::write(salida, nf_core::bmp::encode(&s)).expect("escribir bmp");
    println!("{ancho}x{alto} -> {salida} (título: {:?})", d.titulo());
}
