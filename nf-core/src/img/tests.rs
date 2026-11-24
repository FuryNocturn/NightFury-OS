// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Pruebas de los decodificadores: vectores de `testdata/` (hechos con Python y System.Drawing) y
//! archivos armados aquí con codificadores mínimos (DEFLATE fijo y almacenado, PNG, GIF y JPEG de solo DC).
use super::inflate::{crc32, inflate_all, Inflate, Wrapper, DIST_BASE, DIST_EXTRA, LARGO_BASE, LARGO_EXTRA};
use super::*;
use alloc::vec;
use alloc::vec::Vec;

/// Los mismos datos que generó `gen.py` para los vectores de `testdata/`.
pub(crate) fn var(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * i / 7 + i / 3) % 61 + 32) as u8).collect()
}

fn per(n: usize) -> Vec<u8> {
    (0..n).map(|i| (((i % 97) * 5 + i % 7) % 61 + 32) as u8).collect()
}

fn px(p: &Picture, x: usize, y: usize) -> u32 {
    p.argb[y * p.w as usize + x]
}

fn canal(c: u32, k: u32) -> i32 {
    (c >> (8 * k) & 255) as i32
}

/// Diferencia máxima por canal entre dos colores.
fn dif(a: u32, b: u32) -> i32 {
    (0..4).map(|k| (canal(a, k) - canal(b, k)).abs()).max().unwrap_or(0)
}

// ---------------------------------------------------------------- DEFLATE

/// Escritor de bits (LSB primero) para armar flujos DEFLATE a mano.
pub(crate) struct Bw {
    v: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bw {
    pub(crate) fn new() -> Bw {
        Bw { v: Vec::new(), acc: 0, n: 0 }
    }
    fn bits(&mut self, val: u32, n: u32) {
        self.acc |= (val as u64) << self.n;
        self.n += n;
        while self.n >= 8 {
            self.v.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }
    fn huff(&mut self, code: u32, n: u32) {
        self.bits(code.reverse_bits() >> (32 - n), n);
    }
    fn alinear(&mut self) {
        if self.n > 0 {
            self.bits(0, 8 - self.n);
        }
    }
    fn cabecera(&mut self, ultimo: bool, tipo: u32) {
        self.bits(ultimo as u32, 1);
        self.bits(tipo, 2);
    }
    fn almacenado(&mut self, ultimo: bool, d: &[u8]) {
        self.cabecera(ultimo, 0);
        self.alinear();
        self.v.extend_from_slice(&(d.len() as u16).to_le_bytes());
        self.v.extend_from_slice(&(!(d.len() as u16)).to_le_bytes());
        self.v.extend_from_slice(d);
    }
    fn simbolo_fijo(&mut self, s: u32) {
        match s {
            0..=143 => self.huff(0x30 + s, 8),
            144..=255 => self.huff(0x190 + s - 144, 9),
            256..=279 => self.huff(s - 256, 7),
            _ => self.huff(0xC0 + s - 280, 8),
        }
    }
    fn lit(&mut self, b: u8) {
        self.simbolo_fijo(b as u32);
    }
    fn copia(&mut self, largo: u32, dist: u32) {
        let k = LARGO_BASE.iter().rposition(|&b| b as u32 <= largo).unwrap();
        self.simbolo_fijo(257 + k as u32);
        self.bits(largo - LARGO_BASE[k] as u32, LARGO_EXTRA[k] as u32);
        let d = DIST_BASE.iter().rposition(|&b| b as u32 <= dist).unwrap();
        self.huff(d as u32, 5);
        self.bits(dist - DIST_BASE[d] as u32, DIST_EXTRA[d] as u32);
    }
    pub(crate) fn fin(mut self) -> Vec<u8> {
        self.alinear();
        self.v
    }
}

/// DEFLATE crudo que expande a 1 + 258·k bytes (ceros): una «bomba» minúscula.
pub(crate) fn bomba(k: usize) -> Vec<u8> {
    let mut w = Bw::new();
    w.cabecera(true, 1);
    w.lit(0);
    for _ in 0..k {
        w.copia(258, 1);
    }
    w.simbolo_fijo(256);
    w.fin()
}

fn adler(d: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in d {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    b << 16 | a
}

/// zlib con bloques almacenados (sin comprimir): para armar PNG a mano.
fn zlib_almacenado(d: &[u8]) -> Vec<u8> {
    let mut v = vec![0x78, 0x01];
    let mut trozos: Vec<&[u8]> = d.chunks(65535).collect();
    if trozos.is_empty() {
        trozos.push(&[]);
    }
    for (i, t) in trozos.iter().enumerate() {
        v.push((i + 1 == trozos.len()) as u8);
        v.extend_from_slice(&(t.len() as u16).to_le_bytes());
        v.extend_from_slice(&(!(t.len() as u16)).to_le_bytes());
        v.extend_from_slice(t);
    }
    v.extend_from_slice(&adler(d).to_be_bytes());
    v
}

const LIM: usize = 1 << 20;

#[test]
fn vectores_de_python() {
    let v = var(800);
    let casos: [(&[u8], Wrapper, &[u8]); 7] = [
        (include_bytes!("testdata/var_l0.zlib"), Wrapper::Zlib, &v),
        (include_bytes!("testdata/var_l1.zlib"), Wrapper::Zlib, &v),
        (include_bytes!("testdata/var_l9.zlib"), Wrapper::Zlib, &v),
        (include_bytes!("testdata/var_l6.raw"), Wrapper::Raw, &v),
        (include_bytes!("testdata/var.gz"), Wrapper::Gzip, &v),
        (include_bytes!("testdata/hola.zlib"), Wrapper::Zlib, b"hola"),
        (include_bytes!("testdata/per_l9.zlib"), Wrapper::Zlib, &[]),
    ];
    let p = per(5000);
    for (i, (z, w, esperado)) in casos.iter().enumerate() {
        let esperado = if esperado.is_empty() { &p[..] } else { esperado };
        assert_eq!(inflate_all(z, *w, LIM).unwrap(), esperado, "caso {i}");
        // a trozos de cualquier tamaño, y solo termina con el último byte
        for paso in [1, 2, 3, 7, 64, 1000] {
            let mut inf = Inflate::new(*w, LIM);
            let mut out = Vec::new();
            let trozos: Vec<&[u8]> = z.chunks(paso).collect();
            for (j, t) in trozos.iter().enumerate() {
                let fin = inf.push(t, &mut out).unwrap();
                assert_eq!(fin, j + 1 == trozos.len(), "caso {i} paso {paso} trozo {j}");
            }
            assert_eq!(out, esperado, "caso {i} paso {paso}");
            assert_eq!(inf.push(b"basura", &mut out), Ok(true), "tras el final se ignora todo");
            assert_eq!(out, esperado);
        }
    }
}

#[test]
fn sumas_y_cabeceras_mal() {
    let gz = include_bytes!("testdata/var.gz").to_vec();
    let zl = include_bytes!("testdata/var_l9.zlib").to_vec();
    for k in 1..=8 {
        let mut m = gz.clone();
        let i = m.len() - k;
        m[i] ^= 1;
        assert_eq!(inflate_all(&m, Wrapper::Gzip, LIM), Err(ImgError::Formato), "gzip, byte -{k} de la cola");
    }
    for k in 1..=4 {
        let mut m = zl.clone();
        let i = m.len() - k;
        m[i] ^= 1;
        assert_eq!(inflate_all(&m, Wrapper::Zlib, LIM), Err(ImgError::Formato), "zlib, byte -{k} de la cola");
    }
    let mut m = zl.clone();
    m[1] ^= 1; // la suma de la cabecera zlib deja de cuadrar
    assert_eq!(inflate_all(&m, Wrapper::Zlib, LIM), Err(ImgError::Formato));
    m = zl.clone();
    m[1] |= 0x20; // FDICT
    m[1] = (m[1] & !31) | (31 - ((m[0] as u32 * 256 + m[1] as u32) % 31) as u8 & 31);
    assert!(inflate_all(&m, Wrapper::Zlib, LIM).is_err());
    let mut m = gz.clone();
    m[2] = 7; // método desconocido
    assert_eq!(inflate_all(&m, Wrapper::Gzip, LIM), Err(ImgError::Formato));
    assert_eq!(inflate_all(b"", Wrapper::Raw, LIM), Err(ImgError::Truncado));
}

#[test]
fn recortes_y_bytes_alterados_de_deflate() {
    let todos: [(&[u8], Wrapper); 6] = [
        (include_bytes!("testdata/var_l0.zlib"), Wrapper::Zlib),
        (include_bytes!("testdata/var_l1.zlib"), Wrapper::Zlib),
        (include_bytes!("testdata/var_l9.zlib"), Wrapper::Zlib),
        (include_bytes!("testdata/var_l6.raw"), Wrapper::Raw),
        (include_bytes!("testdata/var.gz"), Wrapper::Gzip),
        (include_bytes!("testdata/per_l9.zlib"), Wrapper::Zlib),
    ];
    for (z, w) in todos {
        for n in 0..z.len() {
            assert!(inflate_all(&z[..n], w, LIM).is_err(), "recorte a {n} de {}", z.len());
        }
        for i in 0..z.len() {
            for x in [0x01u8, 0x80, 0xFF] {
                let mut m = z.to_vec();
                m[i] ^= x;
                let _ = inflate_all(&m, w, LIM); // no debe entrar en pánico
            }
        }
    }
}

#[test]
fn bombas_ventana_y_distancias() {
    // 1 + 258·200 000 ≈ 51 MB desde unos 300 KB
    let b = bomba(200_000);
    assert!(b.len() < 400_000);
    let mut inf = Inflate::new(Wrapper::Raw, 1 << 20);
    let mut out = Vec::new();
    assert_eq!(inf.push(&b, &mut out), Err(ImgError::Limite));
    assert!(out.len() <= 1 << 20, "no se pasa del tope: {}", out.len());
    assert_eq!(inflate_all(&bomba(10), Wrapper::Raw, LIM).unwrap(), vec![0u8; 1 + 2580]);
    // dos bloques de 20 000 y una copia desde exactamente 32 768 atrás
    let datos: Vec<u8> = (0..40_000).map(|i| (i % 251) as u8).collect();
    let armar = |dist: u32| {
        let mut w = Bw::new();
        w.almacenado(false, &datos[..20_000]);
        w.almacenado(false, &datos[20_000..]);
        w.cabecera(true, 1);
        w.copia(10, dist);
        w.simbolo_fijo(256);
        w.fin()
    };
    let r = inflate_all(&armar(32768), Wrapper::Raw, LIM).unwrap();
    assert_eq!(&r[40_000..], &datos[40_000 - 32768..40_000 - 32768 + 10]);
    assert_eq!(inflate_all(&armar(32769 - 1), Wrapper::Raw, LIM).map(|v| v.len()), Ok(40_010));
    // distancias que salen de lo ya escrito
    let mut w = Bw::new();
    w.almacenado(false, b"abc");
    w.cabecera(true, 1);
    w.copia(3, 4);
    assert_eq!(inflate_all(&w.fin(), Wrapper::Raw, LIM), Err(ImgError::Formato));
    let mut w = Bw::new();
    w.cabecera(true, 1);
    w.copia(3, 1);
    assert_eq!(inflate_all(&w.fin(), Wrapper::Raw, LIM), Err(ImgError::Formato), "copia sin nada antes");
    // bloque de tipo 3, almacenado con NLEN mal, y código de distancia 30
    assert_eq!(inflate_all(&[0b111], Wrapper::Raw, LIM), Err(ImgError::Formato));
    assert_eq!(inflate_all(&[1, 4, 0, 0, 0, 1, 2, 3, 4], Wrapper::Raw, LIM), Err(ImgError::Formato));
    let mut w = Bw::new();
    w.cabecera(true, 1);
    w.lit(1);
    w.simbolo_fijo(257);
    w.huff(30, 5);
    w.bits(0, 16);
    assert_eq!(inflate_all(&w.fin(), Wrapper::Raw, LIM), Err(ImgError::Formato));
    // un bloque almacenado que supera el tope
    let mut w = Bw::new();
    w.almacenado(true, &[7u8; 1000]);
    assert_eq!(inflate_all(&w.fin(), Wrapper::Raw, 999), Err(ImgError::Limite));
}

#[test]
fn crc_coincide_con_el_de_vol() {
    for d in [&b""[..], b"123456789", &var(1000)] {
        assert_eq!(crc32(d), crate::vol::part::crc32(d));
    }
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

// ---------------------------------------------------------------- PNG

fn trozo(t: &[u8; 4], d: &[u8]) -> Vec<u8> {
    let mut v = (d.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(t);
    v.extend_from_slice(d);
    let c = crc32(&v[4..]);
    v.extend_from_slice(&c.to_be_bytes());
    v
}

/// PNG con los bloques `antes` entre IHDR e IDAT y `crudo` (filas con su byte de filtro) en IDAT sueltos.
fn png(w: u32, h: u32, prof: u8, color: u8, inter: bool, antes: &[Vec<u8>], crudo: &[u8]) -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10];
    let mut ih = [w.to_be_bytes(), h.to_be_bytes()].concat();
    ih.extend_from_slice(&[prof, color, 0, 0, inter as u8]);
    v.extend(trozo(b"IHDR", &ih));
    for a in antes {
        v.extend_from_slice(a);
    }
    for t in zlib_almacenado(crudo).chunks(37) {
        v.extend(trozo(b"IDAT", t));
    }
    v.extend(trozo(b"IEND", &[]));
    v
}

fn paeth_t(a: u8, b: u8, c: u8) -> u8 {
    let (p, pa, pb, pc) = (a as i32 + b as i32 - c as i32, 0, 0, 0);
    let _ = (pa, pb, pc);
    let d = |x: u8| (p - x as i32).abs();
    if d(a) <= d(b) && d(a) <= d(c) {
        a
    } else if d(b) <= d(c) {
        b
    } else {
        c
    }
}

/// Fila filtrada con el filtro `f` (con el byte de filtro delante).
fn filtrar(f: u8, fila: &[u8], previa: &[u8], bpp: usize) -> Vec<u8> {
    let mut v = vec![f];
    for i in 0..fila.len() {
        let a = if i >= bpp { fila[i - bpp] } else { 0 };
        let (b, c) = (previa[i], if i >= bpp { previa[i - bpp] } else { 0 });
        let p = match f {
            0 => 0,
            1 => a,
            2 => b,
            3 => ((a as u16 + b as u16) / 2) as u8,
            _ => paeth_t(a, b, c),
        };
        v.push(fila[i].wrapping_sub(p));
    }
    v
}

const PASADAS: [(usize, usize, usize, usize); 7] = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];

/// Datos crudos de una imagen de ≥ 8 bits; cada fila con un filtro distinto (0..4, rotando).
fn filas(w: usize, h: usize, bpp: usize, pixel: &dyn Fn(usize, usize) -> Vec<u8>, inter: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let pasadas: Vec<_> = if inter { PASADAS.to_vec() } else { vec![(0, 0, 1, 1)] };
    for (n, (x0, y0, dx, dy)) in pasadas.into_iter().enumerate() {
        let xs: Vec<usize> = (x0..w).step_by(dx).collect();
        if xs.is_empty() || y0 >= h {
            continue; // una pasada sin píxeles no lleva ni bytes de filtro
        }
        let mut previa = vec![0u8; xs.len() * bpp];
        for (r, y) in (y0..h).step_by(dy).enumerate() {
            let fila: Vec<u8> = xs.iter().flat_map(|&x| pixel(x, y)).collect();
            out.extend(filtrar(((r + n) % 5) as u8, &fila, &previa, bpp));
            previa = fila;
        }
    }
    out
}

/// Empaqueta muestras de `prof` bits (1, 2 o 4), MSB primero, con relleno al final de la fila.
fn empaquetar(v: &[u8], prof: u8) -> Vec<u8> {
    let mut out = Vec::new();
    for g in v.chunks(8 / prof as usize) {
        let mut b = 0u8;
        for (i, &m) in g.iter().enumerate() {
            b |= m << (8 - prof as usize * (i + 1));
        }
        out.push(b);
    }
    out
}

#[test]
fn png_gdi_degradado_alfa_y_paleta() {
    let p = decode(include_bytes!("testdata/grad.png"), Limits::default()).unwrap();
    assert_eq!((p.w, p.h), (16, 8));
    for (x, y) in [(0, 0), (15, 7), (3, 5), (8, 2)] {
        assert_eq!(px(&p, x, y), 0xFF00_0000 | ((x * 16) as u32) << 16 | ((y * 32) as u32) << 8 | 128, "({x},{y})");
    }
    let p = decode(include_bytes!("testdata/alfa.png"), Limits::default()).unwrap();
    assert_eq!((p.w, p.h), (8, 4));
    for (x, y) in [(0, 0), (7, 3), (2, 1), (5, 2)] {
        let (a, r, b) = ((255 - x * 32) as u32, (x * 32) as u32, (255 - y * 64) as u32);
        assert_eq!(px(&p, x, y), a << 24 | r << 16 | 64 << 8 | b, "({x},{y})");
    }
    // paleta + tRNS: índice 0 transparente, 1 con alfa 128
    let p = decode(include_bytes!("testdata/pal.png"), Limits::default()).unwrap();
    assert_eq!((p.w, p.h), (4, 2));
    assert_eq!(px(&p, 0, 0) >> 24, 0, "índice 0 transparente");
    assert_eq!(px(&p, 1, 0), 0x80_00FF00);
    assert_eq!(px(&p, 2, 0), 0xFF_0000FF);
    assert_eq!(px(&p, 3, 1), 0x00FF_0000, "índice 0 al final de la fila 2: rojo transparente");
    assert_eq!(px(&p, 0, 1), 0xFF_0A141E);
    assert_eq!(dimensions(include_bytes!("testdata/grad.png")), Some((16, 8)));
}

#[test]
fn png_gris_de_1_2_y_4_bits_y_paleta_de_2_bits() {
    // gris 1 bit, 10×2: (x+y) par = negro, impar = blanco
    let mut crudo = Vec::new();
    for y in 0..2u8 {
        crudo.push(0);
        crudo.extend(empaquetar(&(0..10u8).map(|x| (x + y) & 1).collect::<Vec<_>>(), 1));
    }
    let p = decode(&png(10, 2, 1, 0, false, &[], &crudo), Limits::default()).unwrap();
    for y in 0..2 {
        for x in 0..10 {
            assert_eq!(px(&p, x, y), if (x + y) & 1 == 1 { 0xFFFF_FFFF } else { 0xFF00_0000 });
        }
    }
    // gris 2 y 4 bits
    let crudo = [vec![0u8], empaquetar(&[0, 1, 2, 3, 3], 2)].concat();
    let p = decode(&png(5, 1, 2, 0, false, &[], &crudo), Limits::default()).unwrap();
    assert_eq!([px(&p, 0, 0), px(&p, 1, 0), px(&p, 2, 0), px(&p, 4, 0)], [0xFF00_0000, 0xFF55_5555, 0xFFAA_AAAA, 0xFFFF_FFFF]);
    let crudo = [vec![0u8], empaquetar(&[0, 5, 15], 4)].concat();
    let p = decode(&png(3, 1, 4, 0, false, &[], &crudo), Limits::default()).unwrap();
    assert_eq!([px(&p, 0, 0), px(&p, 1, 0), px(&p, 2, 0)], [0xFF00_0000, 0xFF55_5555, 0xFFFF_FFFF]);
    // clave de transparencia de gris (16 bits, valor 5 de 4 bits → muestra cruda 5)
    let t = trozo(b"tRNS", &[0, 5]);
    let p = decode(&png(3, 1, 4, 0, false, &[t], &crudo), Limits::default()).unwrap();
    assert_eq!(px(&p, 1, 0) >> 24, 0);
    assert_eq!(px(&p, 0, 0) >> 24, 255);
    // paleta de 2 bits con tRNS más corta que la paleta
    let plte = trozo(b"PLTE", &[255, 0, 0, 0, 255, 0, 0, 0, 255, 9, 9, 9]);
    let trns = trozo(b"tRNS", &[0, 100]);
    let crudo = [vec![0u8], empaquetar(&[0, 1, 2, 3, 2], 2), vec![0u8], empaquetar(&[3, 2, 1, 0, 0], 2)].concat();
    let p = decode(&png(5, 2, 2, 3, false, &[plte.clone(), trns], &crudo), Limits::default()).unwrap();
    assert_eq!([px(&p, 0, 0), px(&p, 1, 0), px(&p, 2, 0), px(&p, 3, 0)], [0x00FF_0000, 0x6400_FF00, 0xFF00_00FF, 0xFF09_0909]);
    assert_eq!(px(&p, 4, 1), 0x00FF_0000);
    // índice fuera de la paleta de 2 entradas → error
    let plte2 = trozo(b"PLTE", &[1, 2, 3, 4, 5, 6]);
    let crudo = [vec![0u8], empaquetar(&[0, 1, 2], 2)].concat();
    assert_eq!(decode(&png(3, 1, 2, 3, false, &[plte2], &crudo), Limits::default()), Err(ImgError::Formato));
    // paleta que falta
    assert_eq!(decode(&png(3, 1, 2, 3, false, &[], &crudo), Limits::default()), Err(ImgError::Formato));
}

#[test]
fn png_rgba_16_bits_con_todos_los_filtros() {
    let v16 = |x: usize, y: usize, c: usize| (x * 9000 + y * 2500 + c * 333 + 17) as u16;
    let pixel = |x: usize, y: usize| (0..4).flat_map(|c| v16(x, y, c).to_be_bytes()).collect::<Vec<u8>>();
    for inter in [false, true] {
        let crudo = filas(7, 6, 8, &pixel, inter);
        let p = decode(&png(7, 6, 16, 6, inter, &[], &crudo), Limits::default()).unwrap();
        for y in 0..6 {
            for x in 0..7 {
                let m = |c| (v16(x, y, c) >> 8) as u32;
                assert_eq!(px(&p, x, y), m(3) << 24 | m(0) << 16 | m(1) << 8 | m(2), "({x},{y}) inter={inter}");
            }
        }
    }
}

#[test]
fn png_adam7_en_todos_los_tipos_y_tamanos() {
    for (w, h) in [(1usize, 1usize), (2, 1), (1, 3), (5, 3), (8, 8), (9, 9), (16, 5)] {
        // gris 8 bits
        let g = |x: usize, y: usize| ((y * w + x) * 7 + 1) as u8;
        let crudo = filas(w, h, 1, &|x, y| vec![g(x, y)], true);
        let p = decode(&png(w as u32, h as u32, 8, 0, true, &[], &crudo), Limits::default()).unwrap();
        for y in 0..h {
            for x in 0..w {
                assert_eq!(px(&p, x, y), 0xFF00_0000 | g(x, y) as u32 * 0x01_0101, "gris {w}x{h} ({x},{y})");
            }
        }
        // RGB 8 bits y gris+alfa 16 bits
        let c = |x: usize, y: usize| vec![(x * 30) as u8, (y * 40) as u8, (x + y) as u8];
        let crudo = filas(w, h, 3, &c, true);
        let p = decode(&png(w as u32, h as u32, 8, 2, true, &[], &crudo), Limits::default()).unwrap();
        for y in 0..h {
            for x in 0..w {
                let q = c(x, y);
                assert_eq!(px(&p, x, y), 0xFF00_0000 | (q[0] as u32) << 16 | (q[1] as u32) << 8 | q[2] as u32, "rgb {w}x{h}");
            }
        }
        let ga = |x: usize, y: usize| vec![(x * 10) as u8, 3, (y * 20) as u8, 4];
        let crudo = filas(w, h, 4, &ga, true);
        let p = decode(&png(w as u32, h as u32, 16, 4, true, &[], &crudo), Limits::default()).unwrap();
        assert_eq!(px(&p, w - 1, h - 1), (((h - 1) * 20) as u32) << 24 | ((w - 1) * 10) as u32 * 0x01_0101, "gris+alfa {w}x{h}");
    }
    // Adam7 de 1 bit (8×8): las filas de cada pasada van empaquetadas aparte
    let bit = |x: usize, y: usize| ((x * 3 + y) % 2) as u8;
    let mut crudo = Vec::new();
    for (x0, y0, dx, dy) in PASADAS {
        for y in (y0..8).step_by(dy) {
            crudo.push(0);
            crudo.extend(empaquetar(&(x0..8).step_by(dx).map(|x| bit(x, y)).collect::<Vec<_>>(), 1));
        }
    }
    let p = decode(&png(8, 8, 1, 0, true, &[], &crudo), Limits::default()).unwrap();
    for y in 0..8 {
        for x in 0..8 {
            assert_eq!(px(&p, x, y), if bit(x, y) == 1 { 0xFFFF_FFFF } else { 0xFF00_0000 }, "1 bit adam7 ({x},{y})");
        }
    }
}

#[test]
fn png_clave_rgb_y_bloques_auxiliares() {
    let c = |x: usize, _y: usize| vec![x as u8, 2, 3];
    let crudo = filas(3, 1, 3, &c, false);
    let trns = trozo(b"tRNS", &[0, 1, 0, 2, 0, 3]);
    let aux = trozo(b"teSt", b"cualquier cosa");
    let actl = trozo(b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]);
    let p = decode(&png(3, 1, 8, 2, false, &[aux, actl, trns], &crudo), Limits::default()).unwrap();
    assert_eq!([px(&p, 0, 0) >> 24, px(&p, 1, 0) >> 24, px(&p, 2, 0) >> 24], [255, 0, 255], "solo (1,2,3) es transparente");
    // un bloque crítico desconocido no se puede ignorar
    let crit = trozo(b"TeSt", b"x");
    assert_eq!(decode(&png(3, 1, 8, 2, false, &[crit], &crudo), Limits::default()), Err(ImgError::NoSoportado));
}

#[test]
fn png_errores_y_limites() {
    let crudo = filas(4, 4, 3, &|x, y| vec![x as u8, y as u8, 0], false);
    let bueno = png(4, 4, 8, 2, false, &[], &crudo);
    assert!(decode(&bueno, Limits::default()).is_ok());
    // CRC malo en cualquier bloque (se altera un byte de cada uno)
    let mut m = bueno.clone();
    m[20] ^= 1; // dentro de IHDR
    assert_eq!(decode(&m, Limits::default()), Err(ImgError::Formato));
    // todos los recortes y bytes alterados
    for n in 0..bueno.len() {
        assert!(decode(&bueno[..n], Limits::default()).is_err(), "recorte a {n}");
    }
    for i in 0..bueno.len() {
        for x in [1u8, 0x80, 0xFF] {
            let mut m = bueno.clone();
            m[i] ^= x;
            let _ = decode(&m, Limits::default());
        }
    }
    // filtro 9, datos de menos y datos de más
    let mut mal = crudo.clone();
    mal[0] = 9;
    assert_eq!(decode(&png(4, 4, 8, 2, false, &[], &mal), Limits::default()), Err(ImgError::Formato));
    assert!(decode(&png(4, 4, 8, 2, false, &[], &crudo[..crudo.len() - 1]), Limits::default()).is_err());
    let mut de_mas = crudo.clone();
    de_mas.extend_from_slice(&[0; 50]);
    assert!(decode(&png(4, 4, 8, 2, false, &[], &de_mas), Limits::default()).is_err());
    // tamaños: 100 000 × 100 000 en un archivo diminuto, y combinaciones de tipo y profundidad inválidas
    assert_eq!(decode(&png(100_000, 100_000, 8, 2, false, &[], &crudo), Limits::default()), Err(ImgError::Limite));
    assert_eq!(decode(&png(4, 4, 8, 2, false, &[], &crudo), Limits { max_pixels: 15, max_bytes: 1 << 20 }), Err(ImgError::Limite));
    assert_eq!(decode(&png(4, 4, 8, 2, false, &[], &crudo), Limits { max_pixels: 100, max_bytes: 50 }), Err(ImgError::Limite));
    for (prof, color) in [(3u8, 0u8), (16, 3), (4, 2), (8, 5), (1, 6), (0, 0)] {
        assert_eq!(decode(&png(4, 4, prof, color, false, &[], &crudo), Limits::default()), Err(ImgError::Formato), "{prof}/{color}");
    }
    assert_eq!(decode(&png(0, 4, 8, 2, false, &[], &crudo), Limits::default()), Err(ImgError::Formato));
    // crudo aleatorio con los filtros y rellenos que salgan: nunca pánico
    let mut r = 0x1234_5678u32;
    for n in 0..200 {
        let datos: Vec<u8> = (0..1 + n % 90).map(|_| { r = r.wrapping_mul(1664525).wrapping_add(1013904223); (r >> 24) as u8 }).collect();
        for (c, p, i) in [(0, 8, false), (2, 16, true), (3, 4, true), (6, 8, false), (4, 16, true)] {
            let plte = trozo(b"PLTE", &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
            let _ = decode(&png(5 + n as u32 % 9, 4, p, c, i, &[plte], &datos), Limits::default());
        }
    }
}

// ---------------------------------------------------------------- GIF

fn lzw_codificar(min: u8, idx: &[u8]) -> Vec<u8> {
    let (clear, eoi) = (1u16 << min, (1u16 << min) + 1);
    let mut w = Bw::new();
    let (mut sig, mut tam) = (eoi + 1, min as u32 + 1);
    let mut dic: std::collections::HashMap<(u16, u8), u16> = std::collections::HashMap::new();
    w.bits(clear as u32, tam);
    let mut cur: Option<u16> = None;
    for &k in idx {
        let Some(c) = cur else {
            cur = Some(k as u16);
            continue;
        };
        if let Some(&n) = dic.get(&(c, k)) {
            cur = Some(n);
            continue;
        }
        w.bits(c as u32, tam);
        dic.insert((c, k), sig);
        sig += 1;
        if sig - 1 == 1 << tam && tam < 12 {
            tam += 1;
        }
        if sig == 4096 {
            w.bits(clear as u32, tam);
            dic.clear();
            (sig, tam) = (eoi + 1, min as u32 + 1);
        }
        cur = Some(k as u16);
    }
    if let Some(c) = cur {
        w.bits(c as u32, tam);
    }
    w.bits(eoi as u32, tam);
    w.fin()
}

struct G<'a> {
    w: u16,
    h: u16,
    gct: &'a [[u8; 3]],
    /// (izquierda, arriba, ancho, alto, entrelazado, paleta local, transparente, índices)
    izq: u16,
    arr: u16,
    fw: u16,
    fh: u16,
    entrelazado: bool,
    lct: Option<&'a [[u8; 3]]>,
    transp: Option<u8>,
    idx: &'a [u8],
}

fn tam_paleta(n: usize) -> u8 {
    (n.trailing_zeros() - 1) as u8
}

fn gif(g: &G) -> Vec<u8> {
    let mut v = b"GIF89a".to_vec();
    v.extend_from_slice(&g.w.to_le_bytes());
    v.extend_from_slice(&g.h.to_le_bytes());
    v.extend_from_slice(&[0x80 | 0x70 | tam_paleta(g.gct.len()), 0, 0]);
    v.extend(g.gct.iter().flatten());
    if let Some(t) = g.transp {
        v.extend_from_slice(&[0x21, 0xF9, 4, 1, 10, 0, t, 0]);
    }
    v.push(0x2C);
    for n in [g.izq, g.arr, g.fw, g.fh] {
        v.extend_from_slice(&n.to_le_bytes());
    }
    v.push(g.lct.map_or(0, |l| 0x80 | tam_paleta(l.len())) | if g.entrelazado { 0x40 } else { 0 });
    if let Some(l) = g.lct {
        v.extend(l.iter().flatten());
    }
    let bits = g.lct.unwrap_or(g.gct).len().trailing_zeros() as u8;
    let min = bits.max(2);
    v.push(min);
    for t in lzw_codificar(min, g.idx).chunks(255) {
        v.push(t.len() as u8);
        v.extend_from_slice(t);
    }
    v.extend_from_slice(&[0, 0x3B]);
    v
}

const PAL4: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [250, 250, 10]];

fn rgb(c: [u8; 3]) -> u32 {
    0xFF00_0000 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32
}

#[test]
fn gif_basico_entrelazado_y_local() {
    let idx: Vec<u8> = (0..20).map(|i| (i / 5 + i % 3) as u8 % 4).collect();
    let base = G { w: 5, h: 4, gct: &PAL4, izq: 0, arr: 0, fw: 5, fh: 4, entrelazado: false, lct: None, transp: None, idx: &idx };
    let p = decode(&gif(&base), Limits::default()).unwrap();
    assert_eq!((p.w, p.h), (5, 4));
    for i in 0..20 {
        assert_eq!(p.argb[i], rgb(PAL4[idx[i] as usize]), "{i}");
    }
    assert_eq!(dimensions(&gif(&base)), Some((5, 4)));
    // entrelazado: las filas viajan en el orden 0, 8, 4, 2, 6, 1, 3, 5, 7, 9
    let fila_de = |y: usize| (y % 4) as u8;
    let orden = [0usize, 8, 4, 2, 6, 1, 3, 5, 7, 9];
    let tx: Vec<u8> = orden.iter().flat_map(|&y| vec![fila_de(y); 3]).collect();
    let g = G { w: 3, h: 10, fw: 3, fh: 10, entrelazado: true, idx: &tx, ..G { ..base } };
    let p = decode(&gif(&g), Limits::default()).unwrap();
    for y in 0..10 {
        assert_eq!(px(&p, 2, y), rgb(PAL4[fila_de(y) as usize]), "fila {y}");
    }
    // fotograma de 3×2 en (2,1) de un lienzo de 6×5, paleta local y color 0 transparente
    let local = [[1, 2, 3], [100, 110, 120], [200, 210, 220], [9, 9, 9]];
    let idx = [0, 1, 2, 3, 0, 1];
    let g = G { w: 6, h: 5, izq: 2, arr: 1, fw: 3, fh: 2, entrelazado: false, lct: Some(&local), transp: Some(0), idx: &idx, ..G { ..base } };
    let p = decode(&gif(&g), Limits::default()).unwrap();
    assert_eq!((p.w, p.h), (6, 5));
    assert_eq!(px(&p, 0, 0), 0, "fuera del fotograma: transparente");
    assert_eq!(px(&p, 2, 1) >> 24, 0, "índice transparente");
    assert_eq!(px(&p, 3, 1), rgb(local[1]));
    assert_eq!(px(&p, 4, 1), rgb(local[2]));
    assert_eq!(px(&p, 2, 2), rgb(local[3]));
    assert_eq!(px(&p, 5, 4), 0);
    // un fotograma que se sale del lienzo se recorta
    let g = G { w: 2, h: 2, izq: 1, arr: 1, fw: 3, fh: 3, entrelazado: false, lct: None, transp: None, idx: &[1; 9], ..G { ..base } };
    let p = decode(&gif(&g), Limits::default()).unwrap();
    assert_eq!([px(&p, 0, 0), px(&p, 1, 1)], [0, rgb(PAL4[1])]);
}

#[test]
fn gif_lzw_hasta_12_bits_con_tabla_llena() {
    let pal: Vec<[u8; 3]> = (0..=255u8).map(|i| [i, 255 - i, i / 2]).collect();
    let mut r = 99u32;
    let idx: Vec<u8> = (0..64 * 64).map(|_| { r = r.wrapping_mul(1664525).wrapping_add(1013904223); (r >> 24) as u8 }).collect();
    let g = G { w: 64, h: 64, gct: &pal, izq: 0, arr: 0, fw: 64, fh: 64, entrelazado: false, lct: None, transp: None, idx: &idx };
    let p = decode(&gif(&g), Limits::default()).unwrap();
    assert!(p.argb.iter().zip(&idx).all(|(&c, &i)| c == rgb(pal[i as usize])));
    // y con repeticiones largas (caso «KwKwK»)
    let idx: Vec<u8> = (0..400).map(|i| if i < 300 { 1 } else { (i % 4) as u8 }).collect();
    let g = G { w: 20, h: 20, gct: &PAL4, izq: 0, arr: 0, fw: 20, fh: 20, entrelazado: false, lct: None, transp: None, idx: &idx };
    let p = decode(&gif(&g), Limits::default()).unwrap();
    assert!(p.argb.iter().zip(&idx).all(|(&c, &i)| c == rgb(PAL4[i as usize])));
}

#[test]
fn gif_de_gdi() {
    let d = include_bytes!("testdata/grad.gif");
    assert_eq!(dimensions(d), Some((20, 12)));
    let p = decode(d, Limits::default()).unwrap();
    assert_eq!((p.w, p.h), (20, 12));
    // GDI+ cuantiza a una paleta fija de 216 colores y tramea: un píxel suelto puede errar hasta ~50, pero
    // la media de cada canal tiene que acercarse a la del degradado original
    let media = |f: &dyn Fn(usize, usize) -> u32, k: u32| (0..240).map(|i| canal(f(i % 20, i / 20), k)).sum::<i32>() / 240;
    let original = |x: usize, y: usize| 0xFF00_0000 | ((x * 12) as u32) << 16 | ((y * 20) as u32) << 8 | 200;
    for k in 0..3 {
        let (a, b) = (media(&|x, y| px(&p, x, y), k), media(&original, k));
        assert!((a - b).abs() <= 12, "canal {k}: {a} vs {b}");
    }
    assert!((0..240).all(|i| p.argb[i] >> 24 == 255));
}

#[test]
fn gif_recortes_y_alteraciones() {
    let idx: Vec<u8> = (0..300).map(|i| (i * i / 7 % 4) as u8).collect();
    let g = G { w: 20, h: 15, gct: &PAL4, izq: 1, arr: 1, fw: 18, fh: 13, entrelazado: true, lct: None, transp: Some(2), idx: &idx[..18 * 13] };
    let todos = [gif(&g), include_bytes!("testdata/grad.gif").to_vec()];
    for d in &todos {
        assert!(decode(d, Limits::default()).is_ok());
        for n in 0..d.len() {
            assert!(decode(&d[..n], Limits::default()).is_err(), "recorte a {n} de {}", d.len());
        }
        for i in 0..d.len() {
            for x in [1u8, 0x80, 0xFF] {
                let mut m = d.clone();
                m[i] ^= x;
                let _ = decode(&m, Limits::default());
            }
        }
    }
    assert_eq!(decode(&gif(&G { w: 0, ..G { w: 1, h: 1, gct: &PAL4, izq: 0, arr: 0, fw: 1, fh: 1, entrelazado: false, lct: None, transp: None, idx: &[0] } }), Limits::default()), Err(ImgError::Formato));
    assert_eq!(decode(&todos[0], Limits { max_pixels: 10, max_bytes: 1 << 20 }), Err(ImgError::Limite));
}

// ---------------------------------------------------------------- JPEG

struct Jw {
    v: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Jw {
    fn bits(&mut self, val: u32, n: u32) {
        for i in (0..n).rev() {
            self.acc = self.acc << 1 | (val >> i) & 1;
            self.n += 1;
            if self.n == 8 {
                self.v.push(self.acc as u8);
                if self.acc == 0xFF {
                    self.v.push(0);
                }
                (self.acc, self.n) = (0, 0);
            }
        }
    }
    fn alinear(&mut self) {
        while self.n != 0 {
            self.bits(1, 1);
        }
    }
}

fn seg(m: u8, p: &[u8]) -> Vec<u8> {
    let mut v = vec![0xFF, m];
    v.extend_from_slice(&(p.len() as u16 + 2).to_be_bytes());
    v.extend_from_slice(p);
    v
}

fn dqt(q: u8) -> Vec<u8> {
    seg(0xDB, &[&[0][..], &[q; 64]].concat())
}

fn sof(m: u8, w: u16, h: u16, samp: &[(u8, u8)]) -> Vec<u8> {
    let mut p = vec![8];
    p.extend_from_slice(&h.to_be_bytes());
    p.extend_from_slice(&w.to_be_bytes());
    p.push(samp.len() as u8);
    for (k, (hh, vv)) in samp.iter().enumerate() {
        p.extend_from_slice(&[k as u8 + 1, hh << 4 | vv, 0]);
    }
    seg(m, &p)
}

fn dht(clase_id: u8, cuentas: &[(usize, u8)], vals: &[u8]) -> Vec<u8> {
    let mut p = vec![clase_id];
    let mut c = [0u8; 16];
    for &(l, n) in cuentas {
        c[l - 1] = n;
    }
    p.extend_from_slice(&c);
    p.extend_from_slice(vals);
    seg(0xC4, &p)
}

/// JPEG base con solo el coeficiente DC de cada bloque (cuantización 8: muestra = 128 + dc).
/// `dc(componente, bloque x, bloque y)`; `ri` = MCU entre marcadores de reinicio (0 = sin).
fn jpeg_dc(w: u16, h: u16, samp: &[(u8, u8)], ri: usize, dc: &dyn Fn(usize, usize, usize) -> i32) -> Vec<u8> {
    let mut v = vec![0xFF, 0xD8];
    v.extend(dqt(8));
    v.extend(sof(0xC0, w, h, samp));
    v.extend(dht(0x00, &[(4, 12)], &(0..12).collect::<Vec<u8>>()));
    v.extend(dht(0x10, &[(1, 1)], &[0]));
    if ri > 0 {
        v.extend(seg(0xDD, &(ri as u16).to_be_bytes()));
    }
    let mut sos = vec![samp.len() as u8];
    for k in 0..samp.len() {
        sos.extend_from_slice(&[k as u8 + 1, 0]);
    }
    sos.extend_from_slice(&[0, 63, 0]);
    v.extend(seg(0xDA, &sos));
    let (hmax, vmax) = (samp.iter().map(|s| s.0).max().unwrap() as usize, samp.iter().map(|s| s.1).max().unwrap() as usize);
    let (mcux, mcuy) = ((w as usize).div_ceil(8 * hmax), (h as usize).div_ceil(8 * vmax));
    let mut jw = Jw { v: Vec::new(), acc: 0, n: 0 };
    let mut pred = [0i32; 3];
    let mut n = 0;
    for my in 0..mcuy {
        for mx in 0..mcux {
            if ri > 0 && n > 0 && n % ri == 0 {
                jw.alinear();
                jw.v.extend_from_slice(&[0xFF, 0xD0 + ((n / ri - 1) % 8) as u8]);
                pred = [0; 3];
            }
            n += 1;
            for (k, &(hh, vv)) in samp.iter().enumerate() {
                for by in 0..vv as usize {
                    for bx in 0..hh as usize {
                        let d = dc(k, mx * hh as usize + bx, my * vv as usize + by);
                        let diff = d - pred[k];
                        pred[k] = d;
                        let cat = 32 - diff.unsigned_abs().leading_zeros();
                        jw.bits(cat, 4);
                        if cat > 0 {
                            jw.bits(if diff > 0 { diff as u32 } else { (diff + (1 << cat) - 1) as u32 }, cat);
                        }
                        jw.bits(0, 1); // fin de bloque
                    }
                }
            }
        }
    }
    jw.alinear();
    v.extend(jw.v);
    v.extend_from_slice(&[0xFF, 0xD9]);
    v
}

fn ycc(y: i32, cb: i32, cr: i32) -> u32 {
    let (y, cb, cr) = (y as f32, cb as f32 - 128.0, cr as f32 - 128.0);
    let c = |v: f32| v.round().clamp(0.0, 255.0) as u32;
    0xFF00_0000 | c(y + 1.402 * cr) << 16 | c(y - 0.344136 * cb - 0.714136 * cr) << 8 | c(y + 1.772 * cb)
}

#[test]
fn jpeg_gris_y_reinicios() {
    // 16×8 con dos bloques: +4 y -4 (tras un reinicio, el predictor vuelve a 0)
    let f = |_k: usize, bx: usize, _by: usize| if bx == 0 { 4 } else { -4 };
    for ri in [0, 1] {
        let d = jpeg_dc(16, 8, &[(1, 1)], ri, &f);
        let p = decode(&d, Limits::default()).unwrap();
        assert_eq!((p.w, p.h), (16, 8));
        assert!(dif(px(&p, 3, 3), 0xFF84_8484) <= 1 && dif(px(&p, 12, 5), 0xFF7C_7C7C) <= 1, "ri={ri}: {:08x} {:08x}", px(&p, 3, 3), px(&p, 12, 5));
    }
    assert_eq!(dimensions(&jpeg_dc(16, 8, &[(1, 1)], 0, &f)), Some((16, 8)));
    // 8 reinicios seguidos (RST0..RST7 y vuelta) en 40×8
    let d = jpeg_dc(40, 8, &[(1, 1)], 1, &|_, bx, _| (bx as i32 - 2) * 3);
    let p = decode(&d, Limits::default()).unwrap();
    for bx in 0..5 {
        let e = 128 + (bx as i32 - 2) * 3;
        assert!(dif(px(&p, bx * 8 + 4, 4), 0xFF00_0000 | (e as u32) * 0x01_0101) <= 1, "bloque {bx}");
    }
}

#[test]
fn jpeg_submuestreos() {
    // (muestreo, ancho, alto): 4:4:4, 4:2:2, 4:2:0, 4:4:0 y uno raro de 4:1:1
    let casos: [(&[(u8, u8)], u16, u16); 5] = [
        (&[(1, 1), (1, 1), (1, 1)], 24, 16),
        (&[(2, 1), (1, 1), (1, 1)], 40, 8),
        (&[(2, 2), (1, 1), (1, 1)], 37, 21),
        (&[(1, 2), (1, 1), (1, 1)], 9, 40),
        (&[(4, 1), (1, 1), (1, 1)], 70, 10),
    ];
    for (samp, w, h) in casos {
        // Y constante; Cb crece de izquierda a derecha, Cr de arriba abajo, por bloque de cada componente
        let (hmax, vmax) = (samp[0].0 as usize, samp[0].1 as usize);
        let nbx = (w as usize).div_ceil(8 * hmax);
        let nby = (h as usize).div_ceil(8 * vmax);
        let f = |k: usize, bx: usize, by: usize| match k {
            0 => 0,
            1 => (bx as i32 - nbx as i32 / 2) * 6,
            _ => (by as i32 - nby as i32 / 2) * 6,
        };
        let p = decode(&jpeg_dc(w, h, samp, 0, &f), Limits::default()).unwrap();
        assert_eq!((p.w, p.h), (w as i32, h as i32));
        // los extremos caen en el centro de su primer y último bloque de crominancia
        let (cb0, cr0) = (128 + f(1, 0, 0), 128 + f(2, 0, 0));
        assert!(dif(px(&p, 0, 0), ycc(128, cb0, cr0)) <= 3, "{samp:?} esquina: {:08x} vs {:08x}", px(&p, 0, 0), ycc(128, cb0, cr0));
        // la crominancia cambia de forma monótona a lo largo de la fila (Cb → canal azul) y de la columna (Cr → rojo)
        let fila: Vec<i32> = (0..w as usize).map(|x| canal(px(&p, x, 0), 0)).collect();
        assert!(fila.windows(2).all(|v| v[1] >= v[0] - 1), "{samp:?} azul no monótono: {fila:?}");
        let col: Vec<i32> = (0..h as usize).map(|y| canal(px(&p, 0, y), 2)).collect();
        assert!(col.windows(2).all(|v| v[1] >= v[0] - 1), "{samp:?} rojo no monótono: {col:?}");
        if nbx > 1 {
            assert!(fila[w as usize - 1] > fila[0] + 5, "{samp:?}");
        }
        if nby > 1 {
            assert!(col[h as usize - 1] > col[0] + 5, "{samp:?}");
        }
    }
    // en 4:2:0 el valor de un píxel intermedio es la mezcla de los dos bloques vecinos de crominancia
    let d = jpeg_dc(32, 16, &[(2, 2), (1, 1), (1, 1)], 0, &|k, bx, _| if k == 2 { if bx == 0 { -40 } else { 40 } } else { 0 });
    let p = decode(&d, Limits::default()).unwrap();
    let r: Vec<i32> = (0..32).map(|x| canal(px(&p, x, 4), 2)).collect();
    assert!(r[0] < r[15] && r[15] < r[16] && r[16] < r[31], "{r:?}");
    assert!(r[0] <= r[1] && r[30] <= r[31]);
}

#[test]
fn jpeg_de_gdi() {
    for (d, tol) in [(&include_bytes!("testdata/grad90.jpg")[..], 14), (&include_bytes!("testdata/grad50.jpg")[..], 40)] {
        assert_eq!(dimensions(d), Some((37, 21)));
        let p = decode(d, Limits::default()).unwrap();
        assert_eq!((p.w, p.h), (37, 21));
        let mut peor = 0;
        for y in 0..21usize {
            for x in 0..37usize {
                let e = 0xFF00_0000 | ((40 + x * 5) as u32) << 16 | ((200 - y * 6) as u32) << 8 | (90 + x * 2 + y * 3) as u32;
                peor = peor.max(dif(px(&p, x, y), e));
            }
        }
        assert!(peor <= tol, "error máximo {peor} (tolerancia {tol})");
    }
}

/// Un bloque con DC = 12 y el coeficiente 1 (primer AC en zigzag) = 3, de las dos maneras: base y progresivo.
fn jpeg_un_bloque(progresivo: bool) -> Vec<u8> {
    let mut v = vec![0xFF, 0xD8];
    v.extend(dqt(1));
    v.extend(sof(if progresivo { 0xC2 } else { 0xC0 }, 8, 8, &[(1, 1)]));
    v.extend(dht(0x00, &[(2, 2)], &[3, 4])); // categorías 3 = «00», 4 = «01»
    v.extend(dht(0x10, &[(2, 3)], &[0x00, 0x01, 0x02])); // FDB = «00», (0,1) = «01», (0,2) = «10»
    let escaneo = |ss: u8, se: u8, ahal: u8, f: &dyn Fn(&mut Jw)| {
        let mut jw = Jw { v: Vec::new(), acc: 0, n: 0 };
        f(&mut jw);
        jw.alinear();
        let mut s = seg(0xDA, &[1, 1, 0x00, ss, se, ahal]);
        s.extend(jw.v);
        s
    };
    if !progresivo {
        v.extend(escaneo(0, 63, 0, &|j| {
            j.bits(0b01, 2); // DC: categoría 4
            j.bits(0b1100, 4); // 12
            j.bits(0b10, 2); // AC: (0,2)
            j.bits(0b11, 2); // 3
            j.bits(0b00, 2); // fin de bloque
        }));
    } else {
        v.extend(escaneo(0, 0, 0x01, &|j| {
            j.bits(0b00, 2); // categoría 3
            j.bits(0b110, 3); // 6 = 12 >> 1
        }));
        v.extend(escaneo(1, 63, 0x01, &|j| {
            j.bits(0b01, 2); // (0,1)
            j.bits(1, 1); // 3 >> 1 = 1
            j.bits(0b00, 2); // fin de bloque
        }));
        v.extend(escaneo(0, 0, 0x10, &|j| j.bits(0, 1))); // bit 0 de 12
        v.extend(escaneo(1, 63, 0x10, &|j| {
            j.bits(0b00, 2); // fin de bloque (de una racha de 1)…
            j.bits(1, 1); // …y el bit de corrección del coeficiente ya conocido: 2 + 1 = 3
        }));
    }
    v.extend_from_slice(&[0xFF, 0xD9]);
    v
}

#[test]
fn jpeg_progresivo_igual_que_base() {
    let base = decode(&jpeg_un_bloque(false), Limits::default()).unwrap();
    let prog = decode(&jpeg_un_bloque(true), Limits::default()).unwrap();
    assert_eq!(base, prog);
    assert!(base.argb.iter().any(|&c| c != base.argb[0]), "el bloque no es plano (lleva un AC)");
    // fuera de la ruta feliz: cada recorte da error, cada alteración, sin pánico
    for d in [jpeg_un_bloque(true), jpeg_un_bloque(false)] {
        for n in 0..d.len() {
            assert!(decode(&d[..n], Limits::default()).is_err(), "recorte a {n}");
        }
        for i in 0..d.len() {
            for x in [1u8, 0x80, 0xFF] {
                let mut m = d.clone();
                m[i] ^= x;
                let _ = decode(&m, Limits::default());
            }
        }
    }
}

#[test]
#[ignore = "necesita un JPEG progresivo real: NF_JPEG_PROG=ruta cargo test -p nf-core jpeg_progresivo_real -- --ignored"]
fn jpeg_progresivo_real() {
    let Ok(ruta) = std::env::var("NF_JPEG_PROG") else { return };
    let d = std::fs::read(ruta).unwrap();
    let p = decode(&d, Limits::default()).unwrap();
    assert_eq!(dimensions(&d), Some((p.w as u32, p.h as u32)));
}

#[test]
fn jpeg_no_soportado_y_robustez() {
    let f = |_: usize, _: usize, _: usize| 3;
    let ok = jpeg_dc(16, 16, &[(1, 1)], 0, &f);
    assert!(decode(&ok, Limits::default()).is_ok());
    // aritmético (SOF9), sin pérdida (SOF3), 12 bits, CMYK (4 componentes) y altura 0 (DNL)
    for (m, esperado) in [(0xC9u8, ImgError::NoSoportado), (0xC3, ImgError::NoSoportado), (0xCA, ImgError::NoSoportado)] {
        let mut d = ok.clone();
        let i = d.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        d[i + 1] = m;
        assert_eq!(decode(&d, Limits::default()), Err(esperado), "SOF {m:02x}");
    }
    let mut d = ok.clone();
    let i = d.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    d[i + 4] = 12;
    assert_eq!(decode(&d, Limits::default()), Err(ImgError::NoSoportado));
    d = ok.clone();
    d[i + 5] = 0;
    d[i + 6] = 0;
    assert_eq!(decode(&d, Limits::default()), Err(ImgError::NoSoportado));
    d = ok.clone();
    d[i + 9] = 4;
    assert_eq!(decode(&d, Limits::default()), Err(ImgError::NoSoportado));
    // tamaños
    assert_eq!(decode(&jpeg_dc(60000, 60000, &[(1, 1)], 0, &f), Limits::default()), Err(ImgError::Limite));
    assert_eq!(decode(&ok, Limits { max_pixels: 100, max_bytes: 1 << 20 }), Err(ImgError::Limite));
    assert!(decode(&ok, Limits { max_pixels: 1000, max_bytes: 100 }).is_err());
    // recortes y alteraciones de las pruebas con submuestreo y reinicios
    for d in [jpeg_dc(20, 20, &[(2, 2), (1, 1), (1, 1)], 2, &f), include_bytes!("testdata/grad90.jpg").to_vec()] {
        for n in 0..d.len() {
            assert!(decode(&d[..n], Limits::default()).is_err(), "recorte a {n} de {}", d.len());
        }
        for i in 0..d.len() {
            for x in [1u8, 0x80, 0xFF] {
                let mut m = d.clone();
                m[i] ^= x;
                let _ = decode(&m, Limits::default());
            }
        }
    }
    // muchísimas pasadas: tope
    let mut bomba = ok[..ok.len() - 2].to_vec();
    let sos = seg(0xDA, &[1, 1, 0, 0, 63, 0]);
    for _ in 0..200 {
        bomba.extend_from_slice(&sos);
    }
    bomba.extend_from_slice(&[0xFF, 0xD9]);
    assert!(decode(&bomba, Limits::default()).is_err());
}

// ---------------------------------------------------------------- general y dibujo

#[test]
fn sniff_y_decode_general() {
    assert_eq!(sniff(include_bytes!("testdata/grad.png")), Some(Kind::Png));
    assert_eq!(sniff(include_bytes!("testdata/grad90.jpg")), Some(Kind::Jpeg));
    assert_eq!(sniff(include_bytes!("testdata/grad.gif")), Some(Kind::Gif));
    assert_eq!(sniff(b"BM....."), None);
    assert_eq!(sniff(b""), None);
    assert_eq!(decode(b"nada", Limits::default()), Err(ImgError::Formato));
    assert_eq!(dimensions(b"nada"), None);
    assert_eq!(dimensions(&include_bytes!("testdata/grad.png")[..10]), None);
    let l = Limits::default();
    assert_eq!((l.max_pixels, l.max_bytes), (16_000_000, 64 << 20));
}

fn cuadro() -> Picture {
    Picture { w: 2, h: 2, argb: vec![0xFFFF_0000, 0xFF00_FF00, 0xFF00_00FF, 0xFFFF_FFFF] }
}

#[test]
fn dibujo_escalado() {
    let p = cuadro();
    // 1:1 y ampliación 4×4: las esquinas conservan su color; el centro es una mezcla
    let mut s = Surface::new(8, 8, 0x000000);
    draw_scaled(&p, &mut s, 0, 0, 2, 2);
    assert_eq!([s.px[0], s.px[1], s.px[8], s.px[9]], [0xFF0000, 0x00FF00, 0x0000FF, 0xFFFFFF]);
    let mut s = Surface::new(8, 8, 0x000000);
    draw_scaled(&p, &mut s, 2, 2, 4, 4);
    assert_eq!(s.px[2 * 8 + 2], 0xFF0000);
    assert_eq!(s.px[5 * 8 + 5], 0xFFFFFF);
    assert_eq!(s.px[0], 0, "fuera del rectángulo no se toca");
    let medio = s.px[3 * 8 + 3];
    assert!(medio != 0xFF0000 && medio != 0xFFFFFF && medio != 0, "{medio:06x}");
    // reducción 4×4 → 2×2: promedio de área
    let gris = Picture { w: 4, h: 4, argb: (0..16).map(|i| if i % 4 < 2 { 0xFF000000 } else { 0xFFFFFFFF }).collect() };
    let mut s = Surface::new(4, 4, 0x808080);
    draw_scaled(&gris, &mut s, 0, 0, 2, 2);
    assert_eq!([s.px[0], s.px[1]], [0x000000, 0xFFFFFF]);
    // transparencia: el píxel transparente deja el fondo; el medio mezcla
    let t = Picture { w: 2, h: 1, argb: vec![0x00FF_0000, 0x80FF_FFFF] };
    let mut s = Surface::new(2, 1, 0x000000);
    draw_scaled(&t, &mut s, 0, 0, 2, 1);
    assert_eq!(s.px[0], 0);
    assert!((s.px[1] & 255) > 0x70 && (s.px[1] & 255) < 0x90, "{:06x}", s.px[1]);
    // recorte y rectángulos fuera, enormes o vacíos: sin pánico
    let mut s = Surface::new(8, 8, 0x000000);
    s.set_clip(crate::geom::Rect::new(2, 2, 2, 2));
    draw_scaled(&p, &mut s, -100, -100, 1000, 1000);
    assert_eq!(s.px[0], 0);
    assert!(s.px[2 * 8 + 2] != 0);
    for (x, y, w, h) in [(0, 0, 0, 5), (0, 0, -3, 4), (i32::MAX, 0, 5, 5), (i32::MIN, i32::MIN, i32::MAX, i32::MAX), (-5, -5, 3, 3)] {
        draw_scaled(&p, &mut s, x, y, w, h);
    }
    draw_scaled(&Picture { w: 0, h: 0, argb: vec![] }, &mut s, 0, 0, 4, 4);
    draw_scaled(&Picture { w: 5, h: 5, argb: vec![0; 3] }, &mut s, 0, 0, 4, 4);
}
