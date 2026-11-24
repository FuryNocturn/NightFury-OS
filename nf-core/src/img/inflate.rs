// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Descompresión DEFLATE (RFC 1951) con envoltorio zlib (RFC 1950) o gzip (RFC 1952), a trozos: el
//! cuerpo HTTP llega por partes. Toda la entrada es hostil: nada entra en pánico y la salida tiene tope.
//!
//! Método: cada «unidad» (cabecera de bloque, un símbolo con sus bits extra…) se decodifica de forma
//! atómica; si la entrada se acaba a mitad, se deshace y se espera al siguiente `push`.
use super::ImgError;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wrapper {
    Raw,
    Zlib,
    Gzip,
}

const VENTANA: usize = 32768;
/// Cabecera gzip/zlib más larga que se espera (nombre, comentario, extra).
const MAX_CABECERA: usize = 70_000;

// ---- sumas de comprobación ----

const fn tabla_crc() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}
static TABLA_CRC: [u32; 256] = tabla_crc();

/// CRC-32 IEEE incremental: empieza con `!0` y se termina con `!`.
// shortcut: duplica el algoritmo de `vol::part::crc32` (que es bit a bit y de un solo golpe); unificar si se toca `vol`.
pub fn crc32_update(mut c: u32, data: &[u8]) -> u32 {
    for &b in data {
        c = TABLA_CRC[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c
}

pub fn crc32(data: &[u8]) -> u32 {
    !crc32_update(!0, data)
}

fn adler_update(mut a: u32, mut b: u32, data: &[u8]) -> (u32, u32) {
    for trozo in data.chunks(5000) {
        for &x in trozo {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (a, b)
}

// ---- tablas de Huffman ----

const RAPIDA: u32 = 9;

/// Código canónico: `rapida` resuelve los códigos de hasta 9 bits de un vistazo (índice = próximos 9
/// bits del flujo; entrada = símbolo << 4 | longitud, 0 = no resuelto); el resto, por longitudes.
struct Huff {
    cuenta: [u16; 16],
    primero: [u16; 16],
    desde: [u16; 16],
    simbolos: [u16; 288],
    rapida: [u16; 1 << RAPIDA],
}

impl Huff {
    fn nueva(largos: &[u8]) -> Result<Huff, ImgError> {
        let mut h = Huff { cuenta: [0; 16], primero: [0; 16], desde: [0; 16], simbolos: [0; 288], rapida: [0; 1 << RAPIDA] };
        for &l in largos {
            h.cuenta[l as usize & 15] += 1;
        }
        h.cuenta[0] = 0;
        // sobresuscrito = más códigos de los que caben
        let mut libres: i32 = 1;
        for l in 1..16 {
            libres = libres * 2 - h.cuenta[l] as i32;
            if libres < 0 {
                return Err(ImgError::Formato);
            }
        }
        let (mut codigo, mut indice) = (0u32, 0u16);
        for l in 1..16 {
            codigo = (codigo + h.cuenta[l - 1] as u32) << 1;
            h.primero[l] = codigo as u16;
            h.desde[l] = indice;
            indice += h.cuenta[l];
        }
        let mut sig = h.desde;
        let mut cod = h.primero;
        for (s, &l) in largos.iter().enumerate() {
            let l = l as usize;
            if l == 0 {
                continue;
            }
            h.simbolos[sig[l] as usize] = s as u16;
            sig[l] += 1;
            if l as u32 <= RAPIDA {
                // el código se lee bit a bit desde el más significativo: en el índice va invertido
                let inv = (cod[l] as u32).reverse_bits() >> (32 - l as u32);
                let mut i = inv as usize;
                while i < 1 << RAPIDA {
                    h.rapida[i] = (s as u16) << 4 | l as u16;
                    i += 1 << l;
                }
            }
            cod[l] += 1;
        }
        Ok(h)
    }
}

pub(super) const LARGO_BASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
pub(super) const LARGO_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
pub(super) const DIST_BASE: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
pub(super) const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
const ORDEN_CL: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

// ---- estado ----

enum Fase {
    Cabecera,
    Bloque,
    Almacenado(u32),
    Codigos,
    Cola,
    Fin,
}

enum Unidad {
    Literal(u8),
    Copia(u32, u32),
    FinBloque,
}

pub struct Inflate {
    w: Wrapper,
    max_out: usize,
    fase: Fase,
    ultimo: bool,
    // entrada pendiente y lector de bits (LSB primero)
    datos: Vec<u8>,
    pos: usize,
    bits: u64,
    nbits: u32,
    // tablas del bloque actual
    lit: Option<Box<Huff>>,
    dist: Option<Box<Huff>>,
    // ventana circular y totales
    ventana: Vec<u8>,
    total: u64,
    crc: u32,
    adler: (u32, u32),
}

impl Inflate {
    /// `max_out`: bytes de salida como mucho (bomba de descompresión → `Limite`).
    pub fn new(w: Wrapper, max_out: usize) -> Self {
        Inflate {
            w,
            max_out,
            fase: if w == Wrapper::Raw { Fase::Bloque } else { Fase::Cabecera },
            ultimo: false,
            datos: Vec::new(),
            pos: 0,
            bits: 0,
            nbits: 0,
            lit: None,
            dist: None,
            ventana: vec![0; VENTANA],
            total: 0,
            crc: !0,
            adler: (1, 0),
        }
    }

    /// Añade a `out` lo que se pueda descomprimir con `input`; `Ok(true)` cuando el flujo termina
    /// (tras comprobar Adler-32, o CRC-32 e ISIZE). Lo que sobre después del final se ignora.
    pub fn push(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<bool, ImgError> {
        if matches!(self.fase, Fase::Fin) {
            return Ok(true);
        }
        self.datos.extend_from_slice(input);
        let mut marca = out.len();
        let r = self.correr(out, &mut marca);
        self.sumar(out, &mut marca);
        // los bytes que aún estén en el búfer de bits se conservan (`alinear` puede devolverlos al flujo)
        let guardar = self.nbits.div_ceil(8) as usize;
        self.datos.drain(..self.pos - guardar);
        self.pos = guardar;
        r
    }

    /// Suma de comprobación de la salida producida desde `marca`.
    fn sumar(&mut self, out: &[u8], marca: &mut usize) {
        let nuevo = &out[(*marca).min(out.len())..];
        match self.w {
            Wrapper::Gzip => self.crc = crc32_update(self.crc, nuevo),
            Wrapper::Zlib => self.adler = adler_update(self.adler.0, self.adler.1, nuevo),
            Wrapper::Raw => {}
        }
        *marca = out.len();
    }

    /// Ejecuta `f` y, si la entrada se acaba a mitad, deja todo como estaba (`None`).
    fn atomica<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, ImgError>) -> Result<Option<T>, ImgError> {
        let (pos, bits, nbits) = (self.pos, self.bits, self.nbits);
        match f(self) {
            Ok(v) => Ok(Some(v)),
            Err(ImgError::Truncado) => {
                (self.pos, self.bits, self.nbits) = (pos, bits, nbits);
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    fn correr(&mut self, out: &mut Vec<u8>, marca: &mut usize) -> Result<bool, ImgError> {
        loop {
            match self.fase {
                Fase::Fin => return Ok(true),
                Fase::Cabecera => {
                    if self.atomica(|s| s.cabecera())?.is_none() {
                        if self.datos.len() > MAX_CABECERA {
                            return Err(ImgError::Formato);
                        }
                        return Ok(false);
                    }
                    self.fase = Fase::Bloque;
                }
                Fase::Bloque => {
                    let Some(f) = self.atomica(|s| s.cabecera_bloque())? else { return Ok(false) };
                    self.fase = f;
                }
                Fase::Almacenado(n) => {
                    let n = n as usize;
                    let tomar = n.min(self.datos.len() - self.pos);
                    if self.total + tomar as u64 > self.max_out as u64 {
                        return Err(ImgError::Limite);
                    }
                    for i in 0..tomar {
                        let b = self.datos[self.pos + i];
                        self.emitir(b, out);
                    }
                    self.pos += tomar;
                    if tomar < n {
                        self.fase = Fase::Almacenado((n - tomar) as u32);
                        return Ok(false);
                    }
                    self.fase = self.tras_bloque();
                }
                Fase::Codigos => {
                    let Some(u) = self.atomica(|s| s.unidad())? else { return Ok(false) };
                    match u {
                        Unidad::Literal(b) => {
                            if self.total >= self.max_out as u64 {
                                return Err(ImgError::Limite);
                            }
                            self.emitir(b, out);
                        }
                        Unidad::Copia(largo, dist) => {
                            if dist as u64 > self.total.min(VENTANA as u64) {
                                return Err(ImgError::Formato);
                            }
                            if self.total + largo as u64 > self.max_out as u64 {
                                return Err(ImgError::Limite);
                            }
                            for _ in 0..largo {
                                let b = self.ventana[(self.total as usize).wrapping_sub(dist as usize) & (VENTANA - 1)];
                                self.emitir(b, out);
                            }
                        }
                        Unidad::FinBloque => self.fase = self.tras_bloque(),
                    }
                }
                Fase::Cola => {
                    self.sumar(out, marca);
                    let w = self.w;
                    let Some(()) = self.atomica(|s| s.cola(w))? else { return Ok(false) };
                    self.fase = Fase::Fin;
                }
            }
        }
    }

    fn tras_bloque(&self) -> Fase {
        match (self.ultimo, self.w) {
            (false, _) => Fase::Bloque,
            (true, Wrapper::Raw) => Fase::Fin,
            (true, _) => Fase::Cola,
        }
    }

    fn emitir(&mut self, b: u8, out: &mut Vec<u8>) {
        self.ventana[self.total as usize & (VENTANA - 1)] = b;
        self.total += 1;
        out.push(b);
    }

    // ---- lector de bits ----

    fn llenar(&mut self) {
        while self.nbits <= 56 && self.pos < self.datos.len() {
            self.bits |= (self.datos[self.pos] as u64) << self.nbits;
            self.pos += 1;
            self.nbits += 8;
        }
    }

    fn leer(&mut self, n: u32) -> Result<u32, ImgError> {
        if self.nbits < n {
            self.llenar();
            if self.nbits < n {
                return Err(ImgError::Truncado);
            }
        }
        let v = (self.bits & ((1u64 << n) - 1)) as u32;
        self.bits >>= n;
        self.nbits -= n;
        Ok(v)
    }

    /// Tira los bits que faltan para el límite de byte y devuelve al flujo los bytes enteros ya cargados.
    fn alinear(&mut self) {
        self.pos -= (self.nbits / 8) as usize;
        self.bits = 0;
        self.nbits = 0;
    }

    fn bytes(&mut self, n: usize) -> Result<&[u8], ImgError> {
        let s = self.pos;
        if self.datos.len() - s < n {
            return Err(ImgError::Truncado);
        }
        self.pos += n;
        Ok(&self.datos[s..s + n])
    }

    fn simbolo(&mut self, h: &Huff) -> Result<u16, ImgError> {
        if self.nbits < 15 {
            self.llenar();
        }
        let e = h.rapida[(self.bits & ((1 << RAPIDA) - 1)) as usize];
        if e != 0 {
            let l = (e & 15) as u32;
            if l > self.nbits {
                return Err(ImgError::Truncado);
            }
            self.bits >>= l;
            self.nbits -= l;
            return Ok(e >> 4);
        }
        let mut cod = 0u32;
        for l in 1..16usize {
            if l as u32 > self.nbits {
                return Err(ImgError::Truncado);
            }
            cod = cod << 1 | ((self.bits >> (l - 1)) & 1) as u32;
            let pri = h.primero[l] as u32;
            if cod >= pri && cod - pri < h.cuenta[l] as u32 {
                self.bits >>= l;
                self.nbits -= l as u32;
                return Ok(h.simbolos[h.desde[l] as usize + (cod - pri) as usize]);
            }
        }
        Err(ImgError::Formato)
    }

    // ---- unidades ----

    fn cabecera(&mut self) -> Result<(), ImgError> {
        match self.w {
            Wrapper::Raw => Ok(()),
            Wrapper::Zlib => {
                let h = self.bytes(2)?;
                let (cmf, flg) = (h[0] as u32, h[1] as u32);
                if cmf & 15 != 8 || cmf >> 4 > 7 || (cmf << 8 | flg) % 31 != 0 {
                    return Err(ImgError::Formato);
                }
                if flg & 0x20 != 0 {
                    return Err(ImgError::NoSoportado); // diccionario predefinido
                }
                Ok(())
            }
            Wrapper::Gzip => {
                let h = self.bytes(10)?;
                if h[0] != 0x1F || h[1] != 0x8B || h[2] != 8 || h[3] & 0xE0 != 0 {
                    return Err(ImgError::Formato);
                }
                let flg = h[3];
                if flg & 4 != 0 {
                    let n = self.bytes(2).map(|x| u16::from_le_bytes([x[0], x[1]]))?;
                    self.bytes(n as usize)?;
                }
                for bit in [8, 16] {
                    if flg & bit != 0 {
                        while self.bytes(1)?[0] != 0 {}
                    }
                }
                if flg & 2 != 0 {
                    self.bytes(2)?; // shortcut: no se comprueba el CRC-16 de la cabecera; hacerlo si importa
                }
                Ok(())
            }
        }
    }

    fn cola(&mut self, w: Wrapper) -> Result<(), ImgError> {
        self.alinear();
        match w {
            Wrapper::Zlib => {
                let b = self.bytes(4)?;
                let esperado = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
                if esperado != (self.adler.1 << 16 | self.adler.0) {
                    return Err(ImgError::Formato);
                }
            }
            Wrapper::Gzip => {
                let b = self.bytes(8)?;
                let crc = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                let tam = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                // shortcut: un solo miembro gzip; los siguientes se ignoran, soportarlos si algún servidor los manda
                if crc != !self.crc || tam != self.total as u32 {
                    return Err(ImgError::Formato);
                }
            }
            Wrapper::Raw => {}
        }
        Ok(())
    }

    fn cabecera_bloque(&mut self) -> Result<Fase, ImgError> {
        self.ultimo = self.leer(1)? == 1;
        match self.leer(2)? {
            0 => {
                self.alinear();
                let b = self.bytes(4)?;
                let (n, c) = (u16::from_le_bytes([b[0], b[1]]), u16::from_le_bytes([b[2], b[3]]));
                if n != !c {
                    return Err(ImgError::Formato);
                }
                Ok(Fase::Almacenado(n as u32))
            }
            1 => {
                let mut l = [8u8; 288];
                l[144..256].fill(9);
                l[256..280].fill(7);
                self.lit = Some(Box::new(Huff::nueva(&l)?));
                self.dist = Some(Box::new(Huff::nueva(&[5u8; 30])?));
                Ok(Fase::Codigos)
            }
            2 => {
                self.dinamico()?;
                Ok(Fase::Codigos)
            }
            _ => Err(ImgError::Formato),
        }
    }

    fn dinamico(&mut self) -> Result<(), ImgError> {
        let nlit = self.leer(5)? as usize + 257;
        let ndist = self.leer(5)? as usize + 1;
        let ncl = self.leer(4)? as usize + 4;
        if nlit > 286 || ndist > 30 {
            return Err(ImgError::Formato);
        }
        let mut cl = [0u8; 19];
        for &i in &ORDEN_CL[..ncl] {
            cl[i] = self.leer(3)? as u8;
        }
        let tcl = Huff::nueva(&cl)?;
        let mut largos = [0u8; 286 + 30];
        let n = nlit + ndist;
        let mut i = 0;
        while i < n {
            let s = self.simbolo(&tcl)?;
            let (valor, rep) = match s {
                0..=15 => (s as u8, 1),
                16 => {
                    if i == 0 {
                        return Err(ImgError::Formato);
                    }
                    (largos[i - 1], 3 + self.leer(2)? as usize)
                }
                17 => (0, 3 + self.leer(3)? as usize),
                _ => (0, 11 + self.leer(7)? as usize),
            };
            if i + rep > n {
                return Err(ImgError::Formato);
            }
            largos[i..i + rep].fill(valor);
            i += rep;
        }
        if largos[256] == 0 {
            return Err(ImgError::Formato); // sin fin de bloque
        }
        self.lit = Some(Box::new(Huff::nueva(&largos[..nlit])?));
        self.dist = Some(Box::new(Huff::nueva(&largos[nlit..n])?));
        Ok(())
    }

    /// Un símbolo completo (con sus bits extra y la distancia, si es una copia).
    fn unidad(&mut self) -> Result<Unidad, ImgError> {
        let (Some(lit), Some(dist)) = (self.lit.take(), self.dist.take()) else { return Err(ImgError::Formato) };
        let r = self.unidad_con(&lit, &dist);
        self.lit = Some(lit);
        self.dist = Some(dist);
        r
    }

    fn unidad_con(&mut self, lit: &Huff, dist: &Huff) -> Result<Unidad, ImgError> {
        let s = self.simbolo(lit)? as usize;
        match s {
            0..=255 => Ok(Unidad::Literal(s as u8)),
            256 => Ok(Unidad::FinBloque),
            257..=285 => {
                let k = s - 257;
                let largo = LARGO_BASE[k] as u32 + self.leer(LARGO_EXTRA[k] as u32)?;
                let d = self.simbolo(dist)? as usize;
                if d >= 30 {
                    return Err(ImgError::Formato);
                }
                Ok(Unidad::Copia(largo, DIST_BASE[d] as u32 + self.leer(DIST_EXTRA[d] as u32)?))
            }
            _ => Err(ImgError::Formato),
        }
    }
}

/// Todo de una vez; el flujo debe estar completo.
pub fn inflate_all(data: &[u8], w: Wrapper, max_out: usize) -> Result<Vec<u8>, ImgError> {
    let mut inf = Inflate::new(w, max_out);
    let mut out = Vec::new();
    if inf.push(data, &mut out)? {
        Ok(out)
    } else {
        Err(ImgError::Truncado)
    }
}
