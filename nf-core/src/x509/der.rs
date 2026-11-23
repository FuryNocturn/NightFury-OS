// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Lector DER estricto (X.690): etiquetas de un byte, longitudes definidas y mínimas, sin bytes
//! sobrantes. Toda entrada es hostil: nunca entra en pánico, los errores son `Err`.
//!
//! No hay recursión genérica: quien lo usa recorre estructuras de forma fija, así que la
//! profundidad queda acotada por el código y el tamaño total por `MAX_DER`.

/// Tamaño máximo de un certificado (cabe en el `u16` del paquete de raíces).
pub const MAX_DER: usize = 65535;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DerError;

pub type R<T> = Result<T, DerError>;

#[derive(Clone, Copy)]
pub struct Tlv<'a> {
    pub tag: u8,
    pub content: &'a [u8],
    /// Cabecera + contenido tal cual venían (lo que se firma o se compara).
    pub raw: &'a [u8],
}

pub struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf }
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn peek(&self) -> Option<u8> {
        self.buf.first().copied()
    }

    /// Lee el siguiente elemento.
    pub fn read(&mut self) -> R<Tlv<'a>> {
        let b = self.buf;
        if b.len() < 2 || b[0] & 0x1f == 0x1f {
            return Err(DerError);
        }
        let l0 = b[1] as usize;
        let (len, hdr) = if l0 < 0x80 {
            (l0, 2)
        } else {
            let n = l0 & 0x7f;
            // Con MAX_DER bastan dos bytes de longitud; 0x80 (indefinida) y 0xff no valen.
            if n == 0 || n > 2 || b.len() < 2 + n || b[2] == 0 {
                return Err(DerError);
            }
            let v = b[2..2 + n].iter().fold(0usize, |a, &x| a << 8 | x as usize);
            if v < 0x80 {
                return Err(DerError); // forma larga innecesaria
            }
            (v, 2 + n)
        };
        let end = hdr + len;
        if end > b.len() {
            return Err(DerError);
        }
        self.buf = &b[end..];
        Ok(Tlv { tag: b[0], content: &b[hdr..end], raw: &b[..end] })
    }

    /// Lee un elemento con esa etiqueta o falla.
    pub fn expect(&mut self, tag: u8) -> R<Tlv<'a>> {
        let t = self.read()?;
        if t.tag == tag { Ok(t) } else { Err(DerError) }
    }

    /// Si el siguiente elemento tiene esa etiqueta, lo lee.
    pub fn optional(&mut self, tag: u8) -> R<Option<Tlv<'a>>> {
        if self.peek() == Some(tag) { self.read().map(Some) } else { Ok(None) }
    }

    /// Falla si quedan bytes sin leer.
    pub fn finish(&self) -> R<()> {
        if self.buf.is_empty() { Ok(()) } else { Err(DerError) }
    }
}

/// Contenido de un INTEGER con codificación mínima (sin ceros ni 0xff de más).
pub fn int_bytes<'a>(t: Tlv<'a>) -> R<&'a [u8]> {
    let c = t.content;
    if t.tag != 0x02 || c.is_empty() {
        return Err(DerError);
    }
    if c.len() > 1 && ((c[0] == 0 && c[1] < 0x80) || (c[0] == 0xff && c[1] >= 0x80)) {
        return Err(DerError);
    }
    Ok(c)
}

/// INTEGER no negativo, sin el cero inicial de signo.
pub fn uint_bytes<'a>(t: Tlv<'a>) -> R<&'a [u8]> {
    let c = int_bytes(t)?;
    if c[0] >= 0x80 {
        return Err(DerError);
    }
    Ok(if c.len() > 1 && c[0] == 0 { &c[1..] } else { c })
}

/// BOOLEAN DER: 0x00 o 0xFF.
pub fn bool_val(t: Tlv) -> R<bool> {
    match (t.tag, t.content) {
        (0x01, [0x00]) => Ok(false),
        (0x01, [0xff]) => Ok(true),
        _ => Err(DerError),
    }
}
