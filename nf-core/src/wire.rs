// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Codificación de los mensajes entre programas y sistema (v0.10): `SERVICE`, `REQUEST` y los datos
//! de los eventos. Binario, little endian, sin versiones: los dos lados usan este mismo código.
//!
//! - Enteros: tal cual (`u8`…`u64`, `i32`, `i64`); `bool` = un byte 0/1 (otro valor es error).
//! - `String` = longitud `u32` + UTF-8 válido. `Vec<T>` = número `u32` + cada elemento.
//! - `Option<T>` = byte 0 (nada) o 1 + el valor.
//!
//! CONTRATO: `Wire`, `Writer`, `Reader` y el formato de arriba no cambian. Cada tipo que viaja
//! implementa `Wire` junto a su definición o en el módulo que lo usa.
//! Al leer datos de un programa nada se fía: cada longitud se compara con lo que queda antes de
//! reservar memoria, así que un mensaje de N bytes nunca hace reservar mucho más de N.
use crate::platform::DateTime;
use crate::theme::Theme;
use alloc::string::String;
use alloc::vec::Vec;

/// Mensaje roto o que no se entiende.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bad;

pub type WireResult<T> = Result<T, Bad>;

#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Writer { buf: Vec::new() }
    }
    pub fn put<T: Wire + ?Sized>(&mut self, v: &T) -> &mut Self {
        v.put(self);
        self
    }
    pub fn bytes(&mut self, b: &[u8]) {
        (b.len() as u32).put(self);
        self.buf.extend_from_slice(b);
    }
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

pub struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Reader { b, pos: 0 }
    }
    pub fn get<T: Wire>(&mut self) -> WireResult<T> {
        T::get(self)
    }
    pub fn take(&mut self, n: usize) -> WireResult<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(Bad)?;
        let s = self.b.get(self.pos..end).ok_or(Bad)?;
        self.pos = end;
        Ok(s)
    }
    pub fn bytes(&mut self) -> WireResult<&'a [u8]> {
        let n = u32::get(self)? as usize;
        self.take(n)
    }
    pub fn remaining(&self) -> usize {
        self.b.len() - self.pos
    }
    /// Error si sobra algo: un mensaje es exactamente un valor.
    pub fn end(&self) -> WireResult<()> {
        if self.remaining() == 0 { Ok(()) } else { Err(Bad) }
    }
}

/// Lee un mensaje entero de tipo `T`.
pub fn decode<T: Wire>(b: &[u8]) -> WireResult<T> {
    let mut r = Reader::new(b);
    let v = T::get(&mut r)?;
    r.end()?;
    Ok(v)
}

pub fn encode<T: Wire + ?Sized>(v: &T) -> Vec<u8> {
    let mut w = Writer::new();
    v.put(&mut w);
    w.finish()
}

pub trait Wire: Sized {
    fn put(&self, w: &mut Writer);
    fn get(r: &mut Reader) -> WireResult<Self>;
}

macro_rules! ints {
    ($($t:ty),*) => {$(
        impl Wire for $t {
            fn put(&self, w: &mut Writer) {
                w.buf.extend_from_slice(&self.to_le_bytes());
            }
            fn get(r: &mut Reader) -> WireResult<Self> {
                let s = r.take(core::mem::size_of::<$t>())?;
                let mut a = [0u8; core::mem::size_of::<$t>()];
                a.copy_from_slice(s);
                Ok(<$t>::from_le_bytes(a))
            }
        }
    )*};
}
ints!(u8, u16, u32, u64, i32, i64);

impl Wire for bool {
    fn put(&self, w: &mut Writer) {
        w.buf.push(*self as u8);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        match u8::get(r)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Bad),
        }
    }
}

impl Wire for String {
    fn put(&self, w: &mut Writer) {
        w.bytes(self.as_bytes());
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        let b = r.bytes()?;
        core::str::from_utf8(b).map(String::from).map_err(|_| Bad)
    }
}

impl<T: Wire> Wire for Vec<T> {
    fn put(&self, w: &mut Writer) {
        (self.len() as u32).put(w);
        for v in self {
            v.put(w);
        }
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        let n = u32::get(r)? as usize;
        // Cada elemento ocupa al menos un byte: un número mayor que lo que queda es mentira.
        if n > r.remaining() {
            return Err(Bad);
        }
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(T::get(r)?);
        }
        Ok(v)
    }
}

impl<T: Wire> Wire for Option<T> {
    fn put(&self, w: &mut Writer) {
        match self {
            None => w.buf.push(0),
            Some(v) => {
                w.buf.push(1);
                v.put(w);
            }
        }
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        match u8::get(r)? {
            0 => Ok(None),
            1 => Ok(Some(T::get(r)?)),
            _ => Err(Bad),
        }
    }
}

impl<A: Wire, B: Wire> Wire for (A, B) {
    fn put(&self, w: &mut Writer) {
        self.0.put(w);
        self.1.put(w);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok((A::get(r)?, B::get(r)?))
    }
}

impl<A: Wire, B: Wire, C: Wire> Wire for (A, B, C) {
    fn put(&self, w: &mut Writer) {
        self.0.put(w);
        self.1.put(w);
        self.2.put(w);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok((A::get(r)?, B::get(r)?, C::get(r)?))
    }
}

impl Wire for [u8; 16] {
    fn put(&self, w: &mut Writer) {
        w.buf.extend_from_slice(self);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        let mut a = [0u8; 16];
        a.copy_from_slice(r.take(16)?);
        Ok(a)
    }
}

impl Wire for () {
    fn put(&self, _w: &mut Writer) {}
    fn get(_r: &mut Reader) -> WireResult<Self> {
        Ok(())
    }
}

impl Wire for DateTime {
    fn put(&self, w: &mut Writer) {
        w.put(&self.year).put(&self.month).put(&self.day).put(&self.hour).put(&self.minute).put(&self.second);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(DateTime { year: r.get()?, month: r.get()?, day: r.get()?, hour: r.get()?, minute: r.get()?, second: r.get()? })
    }
}

impl Wire for Theme {
    fn put(&self, w: &mut Writer) {
        w.put(&self.accent).put(&self.wallpaper);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(Theme { accent: r.get()?, wallpaper: r.get()? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn ida_y_vuelta() {
        let v: (Vec<String>, Option<u64>, bool) = (vec![String::from("C:\\Usuarios\\ñu"), String::new()], Some(u64::MAX), true);
        assert_eq!(decode::<(Vec<String>, Option<u64>, bool)>(&encode(&v)), Ok(v));
        let t = Theme { accent: 0x123456, wallpaper: 3 };
        assert_eq!(decode::<Theme>(&encode(&t)), Ok(t));
        let d = DateTime { year: 2026, month: 11, day: 18, hour: 1, minute: 2, second: 3 };
        assert_eq!(decode::<DateTime>(&encode(&d)), Ok(d));
        assert_eq!(decode::<i32>(&encode(&-7i32)), Ok(-7));
    }

    #[test]
    fn mensajes_rotos() {
        assert_eq!(decode::<u32>(&[1, 2, 3]), Err(Bad), "corto");
        assert_eq!(decode::<u8>(&[1, 2]), Err(Bad), "sobra");
        assert_eq!(decode::<bool>(&[2]), Err(Bad));
        assert_eq!(decode::<Option<u8>>(&[7, 0]), Err(Bad));
        // Longitudes que mienten: no se reserva nada enorme, solo error.
        assert_eq!(decode::<Vec<u64>>(&u32::MAX.to_le_bytes()), Err(Bad));
        assert_eq!(decode::<String>(&u32::MAX.to_le_bytes()), Err(Bad));
        let mut bad = encode(&String::from("ab"));
        bad[4] = 0xFF;
        assert_eq!(decode::<String>(&bad), Err(Bad), "UTF-8 inválido");
    }
}
