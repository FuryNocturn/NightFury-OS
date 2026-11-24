// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! JPEG (ITU-T T.81 + JFIF): base (SOF0/SOF1) y progresivo (SOF2) con Huffman, escala de grises e
//! YCbCr (o RGB con Adobe APP14), submuestreos cualesquiera (se sobremuestrea con interpolación
//! bilineal), marcadores de reinicio e IDCT en f32. Aritmético, jerárquico, sin pérdida, de 12 bits y
//! CMYK devuelven `NoSoportado`. La orientación EXIF se ignora.
use super::{comprobar, ImgError, Limits};
use crate::bmp::Picture;
use crate::math::{cos, PI};
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39,
    46, 53, 60, 61, 54, 47, 55, 62, 63,
];
/// Tope de pasadas (SOS): un progresivo normal trae unas diez; más es un ataque de coste.
const MAX_ESCANEOS: u32 = 100;
const RAPIDA: u32 = 9;

fn be16(d: &[u8], i: usize) -> Result<usize, ImgError> {
    match d.get(i..i + 2) {
        Some(b) => Ok((b[0] as usize) << 8 | b[1] as usize),
        None => Err(ImgError::Truncado),
    }
}

/// Segmento que sigue a un marcador: (contenido sin la longitud, posición tras él).
fn segmento(d: &[u8], pos: usize) -> Result<(&[u8], usize), ImgError> {
    let n = be16(d, pos)?;
    if n < 2 {
        return Err(ImgError::Formato);
    }
    Ok((d.get(pos + 2..pos + n).ok_or(ImgError::Truncado)?, pos + n))
}

/// Siguiente marcador desde `pos` (salta basura y bytes de relleno 0xFF): (código, posición tras él).
fn marcador(d: &[u8], mut pos: usize) -> Result<(u8, usize), ImgError> {
    loop {
        while *d.get(pos).ok_or(ImgError::Truncado)? != 0xFF {
            pos += 1;
        }
        while *d.get(pos).ok_or(ImgError::Truncado)? == 0xFF {
            pos += 1;
        }
        let m = *d.get(pos).ok_or(ImgError::Truncado)?;
        pos += 1;
        if m != 0 {
            return Ok((m, pos));
        }
    }
}

fn es_sof(m: u8) -> bool {
    matches!(m, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF)
}

pub fn dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let mut pos = 2;
    loop {
        let (m, p) = marcador(data, pos).ok()?;
        if matches!(m, 0xD0..=0xD9 | 0x01) {
            if m == 0xD9 {
                return None;
            }
            pos = p;
            continue;
        }
        let (seg, sig) = segmento(data, p).ok()?;
        if es_sof(m) {
            let (h, w) = (be16(seg, 1).ok()?, be16(seg, 3).ok()?);
            return (w != 0 && h != 0).then_some((w as u32, h as u32));
        }
        if m == 0xDA {
            return None;
        }
        pos = sig;
    }
}

// ---- Huffman ----

struct Huff {
    /// índice = próximos 9 bits; entrada = símbolo << 4 | longitud (0 = más largo que 9).
    rapida: [u16; 1 << RAPIDA],
    maxcod: [i32; 18],
    mincod: [i32; 17],
    valptr: [i32; 17],
    vals: [u8; 256],
}

impl Huff {
    fn nueva(cuentas: &[u8], vals: &[u8]) -> Result<Huff, ImgError> {
        let mut h = Huff { rapida: [0; 1 << RAPIDA], maxcod: [-1; 18], mincod: [0; 17], valptr: [0; 17], vals: [0; 256] };
        h.vals[..vals.len()].copy_from_slice(vals);
        let (mut cod, mut k) = (0i32, 0i32);
        for l in 1..=16usize {
            let n = cuentas[l - 1] as i32;
            h.valptr[l] = k;
            h.mincod[l] = cod;
            cod += n;
            k += n;
            if cod > 1 << l {
                return Err(ImgError::Formato); // más códigos de los que caben
            }
            h.maxcod[l] = if n > 0 { cod - 1 } else { -1 };
            if l as u32 <= RAPIDA {
                for c in h.mincod[l]..cod {
                    let e = (h.vals[(h.valptr[l] + c - h.mincod[l]) as usize] as u16) << 4 | l as u16;
                    let base = (c as usize) << (RAPIDA as usize - l);
                    h.rapida[base..base + (1 << (RAPIDA as usize - l))].fill(e);
                }
            }
            cod <<= 1;
        }
        Ok(h)
    }
}

// ---- lector de bits de los datos entrópicos (MSB primero, con relleno FF00) ----

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    acc: u64,
    n: u32,
    /// Bytes de relleno (ceros) cargados porque se acabaron los datos o apareció un marcador.
    rellenos: u32,
}

impl<'a> Bits<'a> {
    fn cargar(&mut self) {
        while self.n <= 56 {
            let b = match self.d.get(self.pos) {
                Some(0xFF) if self.d.get(self.pos + 1) == Some(&0) => {
                    self.pos += 2;
                    Some(0xFF)
                }
                Some(0xFF) | None => None,
                Some(&x) => {
                    self.pos += 1;
                    Some(x)
                }
            };
            if b.is_none() {
                self.rellenos += 1;
            }
            self.acc = self.acc << 8 | b.unwrap_or(0) as u64;
            self.n += 8;
        }
    }

    /// ¿Se han consumido bits de relleno? Entonces los datos eran más cortos de lo que dicen las cabeceras.
    fn agotado(&self) -> bool {
        self.rellenos * 8 > self.n
    }

    fn bits(&mut self, k: u32) -> u32 {
        if k == 0 {
            return 0;
        }
        if self.n < k {
            self.cargar();
        }
        self.n -= k;
        ((self.acc >> self.n) & ((1 << k) - 1)) as u32
    }

    /// `s` bits como número con signo (T.81, F.2.2.1).
    fn recibir(&mut self, s: u32) -> i32 {
        if s == 0 {
            return 0;
        }
        let v = self.bits(s) as i32;
        if v < 1 << (s - 1) {
            v - (1 << s) + 1
        } else {
            v
        }
    }

    fn sym(&mut self, h: &Huff) -> Result<u8, ImgError> {
        if self.n < 16 {
            self.cargar();
        }
        let v = ((self.acc >> (self.n - 16)) & 0xFFFF) as u32;
        let e = h.rapida[(v >> (16 - RAPIDA)) as usize];
        if e != 0 {
            self.n -= (e & 15) as u32;
            return Ok((e >> 4) as u8);
        }
        for l in (RAPIDA as usize + 1)..=16 {
            let c = (v >> (16 - l)) as i32;
            if h.maxcod[l] >= 0 && c <= h.maxcod[l] && c >= h.mincod[l] {
                self.n -= l as u32;
                return Ok(h.vals[(h.valptr[l] + c - h.mincod[l]) as usize]);
            }
        }
        Err(ImgError::Formato)
    }

    /// Fin de un intervalo de reinicio: descarta los bits sueltos y salta el marcador RSTn.
    fn reiniciar(&mut self) -> Result<(), ImgError> {
        (self.acc, self.n, self.rellenos) = (0, 0, 0);
        loop {
            match (self.d.get(self.pos), self.d.get(self.pos + 1)) {
                (Some(0xFF), Some(&m)) if (0xD0..=0xD7).contains(&m) => {
                    self.pos += 2;
                    return Ok(());
                }
                (Some(0xFF), Some(&m)) if m != 0 && m != 0xFF => return Err(ImgError::Formato),
                (Some(_), Some(_)) => self.pos += 1,
                _ => return Err(ImgError::Truncado),
            }
        }
    }
}

// ---- estructura de la imagen ----

struct Comp {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    /// Bloques por fila y por columna (con el relleno hasta completar MCU).
    bw: usize,
    bh: usize,
    /// Muestras reales.
    cw: usize,
    ch: usize,
    /// Tabla de cuantización (orden natural), fijada al empezar su primera pasada.
    q: Option<[u16; 64]>,
    /// Muestras 0..=255 (bw*8 × bh*8). En progresivo, solo se rellenan al final.
    plano: Vec<u8>,
    /// Solo progresivo: coeficientes sin cuantizar, 64 por bloque, en orden natural.
    coefs: Vec<i16>,
}

struct Marco {
    w: usize,
    h: usize,
    prog: bool,
    hmax: usize,
    vmax: usize,
    mcux: usize,
    mcuy: usize,
    comps: Vec<Comp>,
}

struct Pasada {
    idx: [usize; 3],
    n: usize,
    td: [usize; 3],
    ta: [usize; 3],
    ss: usize,
    se: usize,
    ah: u32,
    al: u32,
}

#[derive(Default)]
struct Tablas {
    qt: [Option<[u16; 64]>; 4],
    dc: [Option<Box<Huff>>; 4],
    ac: [Option<Box<Huff>>; 4],
    ri: usize,
}

fn leer_sof(seg: &[u8], prog: bool, lim: &Limits) -> Result<Marco, ImgError> {
    if seg.len() < 6 {
        return Err(ImgError::Formato);
    }
    if seg[0] != 8 {
        return Err(ImgError::NoSoportado); // 12 bits
    }
    let (h, w, nc) = (be16(seg, 1)?, be16(seg, 3)?, seg[5] as usize);
    if h == 0 {
        return Err(ImgError::NoSoportado); // altura por DNL
    }
    if w == 0 {
        return Err(ImgError::Formato);
    }
    if nc == 2 || nc == 4 {
        return Err(ImgError::NoSoportado); // CMYK / YCCK
    }
    if (nc != 1 && nc != 3) || seg.len() < 6 + 3 * nc {
        return Err(ImgError::Formato);
    }
    comprobar(w as u32, h as u32, lim)?;
    let mut comps = Vec::new();
    for c in seg[6..6 + 3 * nc].chunks(3) {
        let (hh, vv, tq) = ((c[1] >> 4) as usize, (c[1] & 15) as usize, c[2] as usize);
        if !(1..=4).contains(&hh) || !(1..=4).contains(&vv) || tq > 3 || comps.iter().any(|x: &Comp| x.id == c[0]) {
            return Err(ImgError::Formato);
        }
        comps.push(Comp { id: c[0], h: hh, v: vv, tq, bw: 0, bh: 0, cw: 0, ch: 0, q: None, plano: Vec::new(), coefs: Vec::new() });
    }
    let hmax = comps.iter().map(|c| c.h).max().unwrap_or(1);
    let vmax = comps.iter().map(|c| c.v).max().unwrap_or(1);
    let (mcux, mcuy) = (w.div_ceil(8 * hmax), h.div_ceil(8 * vmax));
    // memoria: planos (1 byte por muestra) y, en progresivo, coeficientes (2 más)
    let mut total = 0u64;
    for c in &mut comps {
        c.bw = mcux * c.h;
        c.bh = mcuy * c.v;
        c.cw = (w * c.h).div_ceil(hmax);
        c.ch = (h * c.v).div_ceil(vmax);
        total += (c.bw * c.bh * 64) as u64 * if prog { 3 } else { 1 };
    }
    if total > lim.max_bytes as u64 {
        return Err(ImgError::Limite);
    }
    for c in &mut comps {
        c.plano = vec![128; c.bw * c.bh * 64];
        if prog {
            c.coefs = vec![0; c.bw * c.bh * 64];
        }
    }
    Ok(Marco { w, h, prog, hmax, vmax, mcux, mcuy, comps })
}

fn leer_sos(seg: &[u8], m: &mut Marco, t: &Tablas) -> Result<Pasada, ImgError> {
    let ns = *seg.first().ok_or(ImgError::Formato)? as usize;
    if ns == 0 || ns > m.comps.len() || seg.len() < 1 + 2 * ns + 3 {
        return Err(ImgError::Formato);
    }
    let mut p = Pasada { idx: [0; 3], n: ns, td: [0; 3], ta: [0; 3], ss: 0, se: 63, ah: 0, al: 0 };
    for i in 0..ns {
        let k = m.comps.iter().position(|c| c.id == seg[1 + 2 * i]).ok_or(ImgError::Formato)?;
        if p.idx[..i].contains(&k) {
            return Err(ImgError::Formato);
        }
        let (td, ta) = ((seg[2 + 2 * i] >> 4) as usize, (seg[2 + 2 * i] & 15) as usize);
        if td > 3 || ta > 3 {
            return Err(ImgError::Formato);
        }
        (p.idx[i], p.td[i], p.ta[i]) = (k, td, ta);
        let c = &mut m.comps[k];
        if c.q.is_none() {
            c.q = Some(t.qt[c.tq].ok_or(ImgError::Formato)?);
        }
    }
    if m.prog {
        let r = &seg[1 + 2 * ns..];
        (p.ss, p.se, p.ah, p.al) = (r[0] as usize, r[1] as usize, (r[2] >> 4) as u32, (r[2] & 15) as u32);
        let mal_ac = p.ss > 0 && (ns != 1 || p.ss > p.se);
        if p.se > 63 || p.ah > 13 || p.al > 13 || (p.ss == 0 && p.se != 0) || mal_ac {
            return Err(ImgError::Formato);
        }
    }
    Ok(p)
}

// ---- decodificación de bloques ----

struct Estado {
    pred: [i32; 3],
    eobrun: u32,
}

fn tabla<'a>(t: &'a [Option<Box<Huff>>; 4], i: usize) -> Result<&'a Huff, ImgError> {
    t[i].as_deref().ok_or(ImgError::Formato)
}

#[allow(clippy::too_many_arguments)]
/// Bloque de un JPEG base: se descomprime, se cuantiza al revés y se pasa por la IDCT al plano.
fn bloque_base(c: &mut Comp, bx: usize, by: usize, k: usize, p: &Pasada, t: &Tablas, bits: &mut Bits, st: &mut Estado, ct: &[[f32; 8]; 8]) -> Result<(), ImgError> {
    let q = c.q.ok_or(ImgError::Formato)?;
    let mut blk = [0i32; 64];
    let tdc = bits.sym(tabla(&t.dc, p.td[k])?)? as u32;
    if tdc > 15 {
        return Err(ImgError::Formato);
    }
    st.pred[k] = st.pred[k].wrapping_add(bits.recibir(tdc));
    blk[0] = st.pred[k].wrapping_mul(q[0] as i32);
    let ac = tabla(&t.ac, p.ta[k])?;
    let mut i = 1;
    while i < 64 {
        let rs = bits.sym(ac)?;
        let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
        if s == 0 {
            if r == 15 {
                i += 16;
                continue;
            }
            break;
        }
        i += r;
        if i > 63 {
            return Err(ImgError::Formato);
        }
        let z = ZIGZAG[i] as usize;
        blk[z] = bits.recibir(s).wrapping_mul(q[z] as i32);
        i += 1;
    }
    let ancho = c.bw * 8;
    let destino = c.plano.get_mut(by * 8 * ancho + bx * 8..).ok_or(ImgError::Formato)?;
    idct(&blk, ct, destino, ancho);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
/// Bloque de un JPEG progresivo: solo acumula coeficientes (T.81, G.1.2).
fn bloque_prog(c: &mut Comp, bx: usize, by: usize, k: usize, p: &Pasada, t: &Tablas, bits: &mut Bits, st: &mut Estado) -> Result<(), ImgError> {
    let o = (by * c.bw + bx) * 64;
    let co = c.coefs.get_mut(o..o + 64).ok_or(ImgError::Formato)?;
    if p.ss == 0 {
        if p.ah == 0 {
            let tdc = bits.sym(tabla(&t.dc, p.td[k])?)? as u32;
            if tdc > 15 {
                return Err(ImgError::Formato);
            }
            st.pred[k] = st.pred[k].wrapping_add(bits.recibir(tdc));
            co[0] = (st.pred[k] << p.al) as i16;
        } else if bits.bits(1) == 1 {
            co[0] |= (1i32 << p.al) as i16;
        }
        return Ok(());
    }
    let ac = tabla(&t.ac, p.ta[k])?;
    let mut i = p.ss;
    if p.ah == 0 {
        if st.eobrun > 0 {
            st.eobrun -= 1;
            return Ok(());
        }
        while i <= p.se {
            let rs = bits.sym(ac)?;
            let (r, s) = ((rs >> 4) as u32, (rs & 15) as u32);
            if s == 0 {
                if r < 15 {
                    st.eobrun = (1 << r) - 1 + bits.bits(r);
                    break;
                }
                i += 16;
                continue;
            }
            i += r as usize;
            if i > p.se {
                return Err(ImgError::Formato);
            }
            co[ZIGZAG[i] as usize] = (bits.recibir(s) << p.al) as i16;
            i += 1;
        }
        return Ok(());
    }
    // refinamiento: un bit más para los coeficientes ya distintos de cero, y nuevos de ±1
    let (p1, m1) = (1i32 << p.al, -(1i32 << p.al));
    let refinar = |co: &mut [i16], z: usize, bits: &mut Bits| {
        if bits.bits(1) == 1 && co[z] as i32 & p1 == 0 {
            co[z] = (co[z] as i32 + if co[z] >= 0 { p1 } else { m1 }) as i16;
        }
    };
    if st.eobrun == 0 {
        while i <= p.se {
            let rs = bits.sym(ac)?;
            let (mut r, s) = ((rs >> 4) as u32, (rs & 15) as u32);
            let mut val = 0i32;
            if s == 0 {
                if r < 15 {
                    st.eobrun = (1 << r) + bits.bits(r);
                    break;
                }
            } else if s == 1 {
                val = if bits.bits(1) == 1 { p1 } else { m1 };
            } else {
                return Err(ImgError::Formato);
            }
            while i <= p.se {
                let z = ZIGZAG[i] as usize;
                i += 1;
                if co[z] != 0 {
                    refinar(co, z, bits);
                } else if r == 0 {
                    if val != 0 {
                        co[z] = val as i16;
                    }
                    break;
                } else {
                    r -= 1;
                }
            }
        }
    }
    if st.eobrun > 0 {
        while i <= p.se {
            let z = ZIGZAG[i] as usize;
            i += 1;
            if co[z] != 0 {
                refinar(co, z, bits);
            }
        }
        st.eobrun -= 1;
    }
    Ok(())
}

/// Una pasada entera; devuelve la posición donde acaban sus datos.
fn escanear(m: &mut Marco, p: &Pasada, t: &Tablas, d: &[u8], pos: usize, ct: &[[f32; 8]; 8]) -> Result<usize, ImgError> {
    let mut bits = Bits { d, pos, acc: 0, n: 0, rellenos: 0 };
    let mut st = Estado { pred: [0; 3], eobrun: 0 };
    let (bxs, unidades) = if p.n == 1 {
        let c = &m.comps[p.idx[0]];
        let bxs = c.cw.div_ceil(8);
        (bxs, bxs * c.ch.div_ceil(8))
    } else {
        (m.mcux, m.mcux * m.mcuy)
    };
    for u in 0..unidades {
        if t.ri > 0 && u > 0 && u % t.ri == 0 {
            bits.reiniciar()?;
            st = Estado { pred: [0; 3], eobrun: 0 };
        }
        for k in 0..p.n {
            let c = &mut m.comps[p.idx[k]];
            // sin entrelazar, la «MCU» es un bloque; entrelazada, h×v bloques de cada componente
            let (nh, nv, x0, y0) = if p.n == 1 { (1, 1, u % bxs, u / bxs) } else { (c.h, c.v, (u % bxs) * c.h, (u / bxs) * c.v) };
            for v in 0..nv {
                for h in 0..nh {
                    let (bx, by) = (x0 + h, y0 + v);
                    if bx >= c.bw || by >= c.bh {
                        return Err(ImgError::Formato);
                    }
                    if m.prog {
                        bloque_prog(c, bx, by, k, p, t, &mut bits, &mut st)?;
                    } else {
                        bloque_base(c, bx, by, k, p, t, &mut bits, &mut st, ct)?;
                    }
                }
            }
        }
        if bits.agotado() {
            return Err(ImgError::Truncado);
        }
    }
    Ok(bits.pos)
}

// ---- IDCT ----

/// `ct[x][u]` = c(u)/2 · cos((2x+1)uπ/16), con c(0) = 1/√2.
fn tabla_idct() -> [[f32; 8]; 8] {
    let mut t = [[0f32; 8]; 8];
    for (x, fila) in t.iter_mut().enumerate() {
        for (u, v) in fila.iter_mut().enumerate() {
            let c = if u == 0 { 0.353_553_4 } else { 0.5 };
            *v = c * cos((2 * x + 1) as f32 * u as f32 * PI / 16.0);
        }
    }
    t
}

// shortcut: IDCT separable de 2×512 productos, saltando filas vacías; una AAN/entera sería ~3 veces más rápida si hiciera falta
fn idct(blk: &[i32; 64], ct: &[[f32; 8]; 8], dst: &mut [u8], ancho: usize) {
    let mut tmp = [[0f32; 8]; 8];
    for v in 0..8 {
        let r = &blk[v * 8..v * 8 + 8];
        if r.iter().all(|&x| x == 0) {
            continue;
        }
        for x in 0..8 {
            tmp[v][x] = (0..8).map(|u| ct[x][u] * r[u] as f32).sum();
        }
    }
    for y in 0..8 {
        for x in 0..8 {
            let s: f32 = (0..8).map(|v| ct[y][v] * tmp[v][x]).sum::<f32>() + 128.5;
            let o = y * ancho + x;
            if o < dst.len() {
                dst[o] = if s <= 0.0 { 0 } else if s >= 255.0 { 255 } else { s as u8 };
            }
        }
    }
}

// ---- de coeficientes a píxeles ----

/// Puntos de interpolación de un eje: (a, b, peso de b en 1/256) por cada píxel de salida.
fn tomas(n_out: usize, n_comp: usize, f_comp: usize, f_max: usize) -> Vec<(u32, u32, u32)> {
    (0..n_out)
        .map(|x| {
            // centro del píxel en coordenadas de la componente, en 1/256
            let s = ((2 * x + 1) * f_comp * 128 / f_max) as i64 - 128;
            let ult = n_comp as i64 - 1;
            let a = (s >> 8).clamp(0, ult);
            let b = ((s >> 8) + 1).clamp(0, ult);
            (a as u32, b as u32, if s < 0 || s >> 8 >= ult { 0 } else { (s & 255) as u32 })
        })
        .collect()
}

fn terminar(mut m: Marco, rgb_directo: bool, ct: &[[f32; 8]; 8]) -> Picture {
    if m.prog {
        for c in &mut m.comps {
            let Some(q) = c.q else { continue };
            let ancho = c.bw * 8;
            for by in 0..c.bh {
                for bx in 0..c.bw {
                    let o = (by * c.bw + bx) * 64;
                    let mut blk = [0i32; 64];
                    for (i, b) in blk.iter_mut().enumerate() {
                        *b = (c.coefs[o + i] as i32).wrapping_mul(q[i] as i32);
                    }
                    idct(&blk, ct, &mut c.plano[by * 8 * ancho + bx * 8..], ancho);
                }
            }
        }
    }
    let (w, h) = (m.w, m.h);
    let tx: Vec<_> = m.comps.iter().map(|c| tomas(w, c.cw, c.h, m.hmax)).collect();
    let ty: Vec<_> = m.comps.iter().map(|c| tomas(h, c.ch, c.v, m.vmax)).collect();
    let mut argb = vec![0u32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = [0i32; 3];
            for (k, c) in m.comps.iter().enumerate() {
                let (xa, xb, fx) = tx[k][x];
                let (ya, yb, fy) = ty[k][y];
                let ancho = c.bw * 8;
                let g = |yy: u32, xx: u32| c.plano[yy as usize * ancho + xx as usize] as u32;
                s[k] = (if fx == 0 && fy == 0 {
                    g(ya, xa)
                } else {
                    let arriba = g(ya, xa) * (256 - fx) + g(ya, xb) * fx;
                    let abajo = g(yb, xa) * (256 - fx) + g(yb, xb) * fx;
                    (arriba * (256 - fy) + abajo * fy + 32768) >> 16
                }) as i32;
            }
            let (r, g, b) = if m.comps.len() == 1 {
                (s[0], s[0], s[0])
            } else if rgb_directo {
                (s[0], s[1], s[2])
            } else {
                let (yy, cb, cr) = (s[0], s[1] - 128, s[2] - 128);
                (yy + ((91881 * cr + 32768) >> 16), yy - ((22554 * cb + 46802 * cr + 32768) >> 16), yy + ((116130 * cb + 32768) >> 16))
            };
            argb[y * w + x] = 0xFF00_0000 | (r.clamp(0, 255) as u32) << 16 | (g.clamp(0, 255) as u32) << 8 | b.clamp(0, 255) as u32;
        }
    }
    Picture { w: w as i32, h: h as i32, argb }
}

pub fn decode(d: &[u8], lim: Limits) -> Result<Picture, ImgError> {
    if !d.starts_with(&[0xFF, 0xD8]) {
        return Err(ImgError::Formato);
    }
    let mut t = Tablas::default();
    let mut marco: Option<Marco> = None;
    let (mut adobe_transform, mut escaneos) = (None, 0u32);
    let ct = tabla_idct();
    let mut pos = 2;
    loop {
        let (mk, p) = marcador(d, pos)?;
        pos = p;
        match mk {
            0xD9 => break,
            0xD0..=0xD8 | 0x01 => continue,
            _ => {}
        }
        let (seg, sig) = segmento(d, pos)?;
        pos = sig;
        match mk {
            0xDB => {
                let mut s = seg;
                while !s.is_empty() {
                    let (pq, tq) = (s[0] >> 4, (s[0] & 15) as usize);
                    let n = if pq == 0 { 64 } else { 128 };
                    if pq > 1 || tq > 3 || s.len() < 1 + n {
                        return Err(ImgError::Formato);
                    }
                    let mut q = [0u16; 64];
                    for i in 0..64 {
                        q[ZIGZAG[i] as usize] = if pq == 0 { s[1 + i] as u16 } else { (s[1 + 2 * i] as u16) << 8 | s[2 + 2 * i] as u16 };
                    }
                    t.qt[tq] = Some(q);
                    s = &s[1 + n..];
                }
            }
            0xC4 => {
                let mut s = seg;
                while !s.is_empty() {
                    if s.len() < 17 {
                        return Err(ImgError::Formato);
                    }
                    let (clase, id) = (s[0] >> 4, (s[0] & 15) as usize);
                    let n: usize = s[1..17].iter().map(|&x| x as usize).sum();
                    if clase > 1 || id > 3 || n > 256 || s.len() < 17 + n {
                        return Err(ImgError::Formato);
                    }
                    let h = Box::new(Huff::nueva(&s[1..17], &s[17..17 + n])?);
                    if clase == 0 { t.dc[id] = Some(h) } else { t.ac[id] = Some(h) }
                    s = &s[17 + n..];
                }
            }
            0xDD => t.ri = be16(seg, 0)?,
            0xC0..=0xC2 => {
                if marco.is_some() {
                    return Err(ImgError::Formato);
                }
                marco = Some(leer_sof(seg, mk == 0xC2, &lim)?);
            }
            _ if es_sof(mk) => return Err(ImgError::NoSoportado), // sin pérdida, jerárquico, aritmético
            0xEE if seg.len() >= 12 && seg.starts_with(b"Adobe") => adobe_transform = Some(seg[11]),
            0xDA => {
                escaneos += 1;
                if escaneos > MAX_ESCANEOS {
                    return Err(ImgError::Limite);
                }
                let m = marco.as_mut().ok_or(ImgError::Formato)?;
                let p = leer_sos(seg, m, &t)?;
                pos = escanear(m, &p, &t, d, pos, &ct)?;
            }
            _ => {}
        }
    }
    let m = marco.ok_or(ImgError::Formato)?;
    if escaneos == 0 {
        return Err(ImgError::Formato);
    }
    // Adobe con transformación 0 y tres componentes: ya es RGB
    let rgb = m.comps.len() == 3 && adobe_transform == Some(0);
    Ok(terminar(m, rgb, &ct))
}
