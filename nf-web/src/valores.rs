// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Valores CSS: números, longitudes, colores, familias de letra y palabras separadas por espacios.
use crate::Familia;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Len {
    Auto,
    Px(f32),
    Pct(f32),
}

/// Contexto para resolver unidades relativas.
#[derive(Clone, Copy)]
pub struct Ctx {
    pub em: f32,
    pub rem: f32,
    pub vw: f32,
    pub vh: f32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Col {
    Transp,
    Rgb(u32),
    Actual,
}

/// Número decimal (`12`, `-1.5`, `.5`). No admite exponente, `inf` ni `nan`.
pub fn numero(s: &str) -> Option<f32> {
    let t = s.trim();
    if t.is_empty() || !t.bytes().all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'-' | b'+')) {
        return None;
    }
    let r: f32 = t.parse().ok()?;
    if r.is_finite() {
        Some(r)
    } else {
        None
    }
}

/// Palabras separadas por espacios, sin partir lo que va entre paréntesis o comillas.
pub fn palabras(v: &str) -> Vec<&str> {
    let b = v.as_bytes();
    let mut r = Vec::new();
    let (mut ini, mut n) = (None, 0i32);
    let mut cad = 0u8;
    for (i, &c) in b.iter().enumerate() {
        if cad != 0 {
            if c == cad {
                cad = 0;
            }
            continue;
        }
        match c {
            b'"' | b'\'' => {
                cad = c;
                ini.get_or_insert(i);
            }
            b'(' => {
                n += 1;
                ini.get_or_insert(i);
            }
            b')' => n = (n - 1).max(0),
            c if c.is_ascii_whitespace() && n == 0 => {
                if let Some(s) = ini.take() {
                    r.push(&v[s..i]);
                }
            }
            _ => {
                ini.get_or_insert(i);
            }
        }
    }
    if let Some(s) = ini {
        r.push(&v[s..]);
    }
    r
}

/// Longitud, porcentaje o `auto`. `em` y compañía se resuelven con `c`. Sin unidad solo vale el 0.
pub fn largo(t: &str, c: &Ctx) -> Option<Len> {
    let t = t.trim().to_ascii_lowercase();
    if t == "auto" {
        return Some(Len::Auto);
    }
    if let Some(n) = t.strip_suffix('%') {
        return Some(Len::Pct(numero(n)?));
    }
    const U: [(&str, f32); 14] = [
        ("px", 1.0),
        ("vmin", -1.0),
        ("vmax", -2.0),
        ("rem", -3.0),
        ("em", -4.0),
        ("vw", -5.0),
        ("vh", -6.0),
        ("pt", 4.0 / 3.0),
        ("pc", 16.0),
        ("in", 96.0),
        ("cm", 37.795),
        ("mm", 3.7795),
        ("ex", -7.0),
        ("ch", -7.0),
    ];
    for (u, k) in U {
        if let Some(n) = t.strip_suffix(u) {
            let x = numero(n)?;
            let px = if k > 0.0 {
                x * k
            } else {
                match k as i32 {
                    -1 => x * c.vw.min(c.vh) / 100.0,
                    -2 => x * c.vw.max(c.vh) / 100.0,
                    -3 => x * c.rem,
                    -4 => x * c.em,
                    -5 => x * c.vw / 100.0,
                    -6 => x * c.vh / 100.0,
                    _ => x * c.em * 0.5,
                }
            };
            return Some(Len::Px(px));
        }
    }
    if numero(&t)? == 0.0 {
        return Some(Len::Px(0.0));
    }
    None
}

/// Longitud en px (sin `auto` ni porcentajes).
pub fn px(t: &str, c: &Ctx) -> Option<f32> {
    match largo(t, c)? {
        Len::Px(x) => Some(x),
        _ => None,
    }
}

const NOMBRES: &str = "aliceblue f0f8ff antiquewhite faebd7 aqua 00ffff aquamarine 7fffd4 azure f0ffff beige f5f5dc \
bisque ffe4c4 black 000000 blanchedalmond ffebcd blue 0000ff blueviolet 8a2be2 brown a52a2a burlywood deb887 \
cadetblue 5f9ea0 chartreuse 7fff00 chocolate d2691e coral ff7f50 cornflowerblue 6495ed cornsilk fff8dc crimson dc143c \
cyan 00ffff darkblue 00008b darkcyan 008b8b darkgoldenrod b8860b darkgray a9a9a9 darkgreen 006400 darkgrey a9a9a9 \
darkkhaki bdb76b darkmagenta 8b008b darkolivegreen 556b2f darkorange ff8c00 darkorchid 9932cc darkred 8b0000 \
darksalmon e9967a darkseagreen 8fbc8f darkslateblue 483d8b darkslategray 2f4f4f darkslategrey 2f4f4f \
darkturquoise 00ced1 darkviolet 9400d3 deeppink ff1493 deepskyblue 00bfff dimgray 696969 dimgrey 696969 \
dodgerblue 1e90ff firebrick b22222 floralwhite fffaf0 forestgreen 228b22 fuchsia ff00ff gainsboro dcdcdc \
ghostwhite f8f8ff gold ffd700 goldenrod daa520 gray 808080 grey 808080 green 008000 greenyellow adff2f \
honeydew f0fff0 hotpink ff69b4 indianred cd5c5c indigo 4b0082 ivory fffff0 khaki f0e68c lavender e6e6fa \
lavenderblush fff0f5 lawngreen 7cfc00 lemonchiffon fffacd lightblue add8e6 lightcoral f08080 lightcyan e0ffff \
lightgoldenrodyellow fafad2 lightgray d3d3d3 lightgreen 90ee90 lightgrey d3d3d3 lightpink ffb6c1 lightsalmon ffa07a \
lightseagreen 20b2aa lightskyblue 87cefa lightslategray 778899 lightslategrey 778899 lightsteelblue b0c4de \
lightyellow ffffe0 lime 00ff00 limegreen 32cd32 linen faf0e6 magenta ff00ff maroon 800000 mediumaquamarine 66cdaa \
mediumblue 0000cd mediumorchid ba55d3 mediumpurple 9370db mediumseagreen 3cb371 mediumslateblue 7b68ee \
mediumspringgreen 00fa9a mediumturquoise 48d1cc mediumvioletred c71585 midnightblue 191970 mintcream f5fffa \
mistyrose ffe4e1 moccasin ffe4b5 navajowhite ffdead navy 000080 oldlace fdf5e6 olive 808000 olivedrab 6b8e23 \
orange ffa500 orangered ff4500 orchid da70d6 palegoldenrod eee8aa palegreen 98fb98 paleturquoise afeeee \
palevioletred db7093 papayawhip ffefd5 peachpuff ffdab9 peru cd853f pink ffc0cb plum dda0dd powderblue b0e0e6 \
purple 800080 rebeccapurple 663399 red ff0000 rosybrown bc8f8f royalblue 4169e1 saddlebrown 8b4513 salmon fa8072 \
sandybrown f4a460 seagreen 2e8b57 seashell fff5ee sienna a0522d silver c0c0c0 skyblue 87ceeb slateblue 6a5acd \
slategray 708090 slategrey 708090 snow fffafa springgreen 00ff7f steelblue 4682b4 tan d2b48c teal 008080 \
thistle d8bfd8 tomato ff6347 turquoise 40e0d0 violet ee82ee wheat f5deb3 white ffffff whitesmoke f5f5f5 \
yellow ffff00 yellowgreen 9acd32";

/// Mezcla `c` con blanco según el alfa (0..255): sin saber el fondo real, se supone claro.
fn con_alfa(c: u32, a: u32) -> Col {
    if a == 0 {
        return Col::Transp;
    }
    if a >= 255 {
        return Col::Rgb(c);
    }
    let m = |sh: u32| {
        let x = c >> sh & 255;
        (x * a + 255 * (255 - a)) / 255
    };
    Col::Rgb(m(16) << 16 | m(8) << 8 | m(0))
}

fn canal(s: &str, max: f32) -> Option<f32> {
    let s = s.trim();
    let v = match s.strip_suffix('%') {
        Some(n) => numero(n)? * max / 100.0,
        None => numero(s)?,
    };
    Some(v.clamp(0.0, max))
}

fn hsl(h: f32, s: f32, l: f32) -> u32 {
    let a = s * l.min(1.0 - l);
    let f = |n: f32| {
        let k = (n + h / 30.0) % 12.0;
        let m = (k - 3.0).min(9.0 - k).min(1.0).max(-1.0);
        ((l - a * m) * 255.0 + 0.5).clamp(0.0, 255.0) as u32
    };
    f(0.0) << 16 | f(8.0) << 8 | f(4.0)
}

pub fn color(v: &str) -> Option<Col> {
    let v = v.trim().to_ascii_lowercase();
    if let Some(h) = v.strip_prefix('#') {
        if !h.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let d = |i: usize| h.as_bytes().get(i).and_then(|&c| (c as char).to_digit(16));
        return match h.len() {
            3 | 4 => {
                let (r, g, b) = (d(0)? * 17, d(1)? * 17, d(2)? * 17);
                Some(con_alfa(r << 16 | g << 8 | b, if h.len() == 4 { d(3)? * 17 } else { 255 }))
            }
            6 | 8 => {
                let n = u32::from_str_radix(&h[..6], 16).ok()?;
                let a = if h.len() == 8 { u32::from_str_radix(&h[6..], 16).ok()? } else { 255 };
                Some(con_alfa(n, a))
            }
            _ => None,
        };
    }
    if let Some(i) = v.find('(') {
        let f = &v[..i];
        let args = v[i + 1..].strip_suffix(')')?;
        let a: Vec<&str> = args.split([',', ' ', '/']).filter(|s| !s.is_empty()).collect();
        if a.len() < 3 {
            return None;
        }
        let alfa = match a.get(3) {
            Some(x) => (canal(x, 1.0).or_else(|| canal(x, 100.0).map(|p| p / 100.0))? * 255.0 + 0.5) as u32,
            None => 255,
        };
        return match f {
            "rgb" | "rgba" => {
                let (r, g, b) = (canal(a[0], 255.0)?, canal(a[1], 255.0)?, canal(a[2], 255.0)?);
                Some(con_alfa(((r + 0.5) as u32) << 16 | ((g + 0.5) as u32) << 8 | (b + 0.5) as u32, alfa))
            }
            "hsl" | "hsla" => {
                let h = (numero(a[0].trim_end_matches("deg"))? % 360.0 + 360.0) % 360.0;
                let (s, l) = (canal(a[1], 100.0)? / 100.0, canal(a[2], 100.0)? / 100.0);
                Some(con_alfa(hsl(h, s, l), alfa))
            }
            _ => None,
        };
    }
    match v.as_str() {
        "transparent" => return Some(Col::Transp),
        "currentcolor" => return Some(Col::Actual),
        _ => {}
    }
    let mut it = NOMBRES.split(' ');
    while let (Some(n), Some(h)) = (it.next(), it.next()) {
        if n == v {
            return u32::from_str_radix(h, 16).ok().map(Col::Rgb);
        }
    }
    None
}

/// Familia de letra de una lista `font-family`: la primera que se reconoce.
pub fn familia(v: &str) -> Option<Familia> {
    for n in v.split(',') {
        let n = n.trim().trim_matches(['"', '\'']).to_ascii_lowercase();
        let t = |l: &[&str]| l.iter().any(|x| n.contains(x));
        if t(&["mono", "courier", "consolas", "menlo", "monaco", "lucida console", "source code"]) {
            return Some(Familia::Mono);
        }
        if n.contains("serif") && !n.contains("sans") {
            return Some(Familia::Serif);
        }
        if t(&["sans", "arial", "helvetica", "verdana", "tahoma", "system-ui", "segoe", "roboto", "calibri", "trebuchet", "apple", "open sans", "lato", "inter", "ubuntu", "noto", "dejavu", "liberation sans"]) {
            return Some(Familia::Sans);
        }
        if t(&["serif", "times", "georgia", "garamond", "palatino", "cambria", "book antiqua", "liberation serif"]) {
            return Some(Familia::Serif);
        }
    }
    None
}

/// Cadena sin tipos de más: copia de `s` en minúsculas.
pub fn minus(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}
