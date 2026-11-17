// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Primitivas criptográficas propias (clean room): escritas a partir de las normas públicas,
//! sin copiar código de nadie. Cada una se comprueba con los vectores oficiales en las pruebas.
//!
//! - SHA-256: FIPS 180-4.
//! - HMAC-SHA-256: RFC 2104 (vectores de RFC 4231).
//! - PBKDF2-HMAC-SHA-256: RFC 8018.
//!
//! No están auditadas por terceros (ver docs/CLEANROOM.md).
use alloc::string::String;
use alloc::vec::Vec;

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be,
    0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa,
    0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85,
    0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f,
    0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 incremental.
#[derive(Clone)]
pub struct Sha256 {
    h: [u32; 8],
    buf: [u8; 64],
    n: usize,
    len: u64,
}

impl Default for Sha256 {
    fn default() -> Sha256 {
        Sha256::new()
    }
}

impl Sha256 {
    pub fn new() -> Sha256 {
        Sha256 {
            h: [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19],
            buf: [0; 64],
            n: 0,
            len: 0,
        }
    }

    fn block(h: &mut [u32; 8], b: &[u8]) {
        let mut w = [0u32; 64];
        for (i, c) in b.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b2, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b2) ^ (a & c) ^ (b2 & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b2;
            b2 = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b2, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        if self.n > 0 {
            let k = (64 - self.n).min(data.len());
            self.buf[self.n..self.n + k].copy_from_slice(&data[..k]);
            self.n += k;
            data = &data[k..];
            if self.n < 64 {
                return;
            }
            let b = self.buf;
            Self::block(&mut self.h, &b);
            self.n = 0;
        }
        let mut it = data.chunks_exact(64);
        for b in &mut it {
            Self::block(&mut self.h, b);
        }
        let r = it.remainder();
        self.buf[..r.len()].copy_from_slice(r);
        self.n = r.len();
    }

    pub fn finish(mut self) -> [u8; 32] {
        let bits = self.len.wrapping_mul(8);
        self.update(&[0x80]);
        while self.n != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        let mut out = [0u8; 32];
        for (o, x) in out.chunks_exact_mut(4).zip(self.h) {
            o.copy_from_slice(&x.to_be_bytes());
        }
        out
    }
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut s = Sha256::new();
    s.update(data);
    s.finish()
}

/// HMAC-SHA-256 con la clave ya absorbida: se clona para cada mensaje.
#[derive(Clone)]
pub struct Hmac {
    inner: Sha256,
    outer: Sha256,
}

impl Hmac {
    pub fn new(key: &[u8]) -> Hmac {
        let mut k = [0u8; 64];
        if key.len() > 64 {
            k[..32].copy_from_slice(&sha256(key));
        } else {
            k[..key.len()].copy_from_slice(key);
        }
        let (mut inner, mut outer) = (Sha256::new(), Sha256::new());
        inner.update(&k.map(|b| b ^ 0x36));
        outer.update(&k.map(|b| b ^ 0x5c));
        wipe(&mut k);
        Hmac { inner, outer }
    }

    /// HMAC de la concatenación de `parts`.
    pub fn mac(&self, parts: &[&[u8]]) -> [u8; 32] {
        let mut i = self.inner.clone();
        for p in parts {
            i.update(p);
        }
        let mut o = self.outer.clone();
        o.update(&i.finish());
        o.finish()
    }
}

pub fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    Hmac::new(key).mac(&[data])
}

/// PBKDF2-HMAC-SHA-256: rellena `out` (cualquier longitud).
pub fn pbkdf2(password: &[u8], salt: &[u8], iters: u32, out: &mut [u8]) {
    let h = Hmac::new(password);
    for (n, chunk) in out.chunks_mut(32).enumerate() {
        let mut u = h.mac(&[salt, &(n as u32 + 1).to_be_bytes()]);
        let mut t = u;
        for _ in 1..iters.max(1) {
            u = h.mac(&[&u]);
            for (a, b) in t.iter_mut().zip(u) {
                *a ^= b;
            }
        }
        chunk.copy_from_slice(&t[..chunk.len()]);
    }
}

/// Comparación en tiempo constante (no revela en qué byte difieren).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Borra un secreto de la memoria de forma que el compilador no pueda eliminar el borrado.
pub fn wipe(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: `b` es una referencia válida y exclusiva a un byte.
        unsafe { core::ptr::write_volatile(b, 0) };
    }
}

pub fn to_hex(data: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        s.push(D[(b >> 4) as usize] as char);
        s.push(D[(b & 15) as usize] as char);
    }
    s
}

/// `None` si no es hexadecimal de longitud par.
pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 2 != 0 {
        return None;
    }
    b.chunks_exact(2).map(|p| Some(((p[0] as char).to_digit(16)? * 16 + (p[1] as char).to_digit(16)?) as u8)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hx(d: &[u8]) -> String {
        to_hex(d)
    }

    #[test]
    fn sha256_vectores_fips_180_4() {
        assert_eq!(hx(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hx(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(
            hx(&sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // Un millón de 'a', por trozos de tamaño irregular (prueba el relleno del búfer).
        let mut s = Sha256::new();
        let chunk = [b'a'; 977];
        let mut left = 1_000_000;
        while left > 0 {
            let k = left.min(chunk.len());
            s.update(&chunk[..k]);
            left -= k;
        }
        assert_eq!(hx(&s.finish()), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }

    #[test]
    fn hmac_vectores_rfc_4231() {
        assert_eq!(hx(&hmac(&[0x0b; 20], b"Hi There")), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
        assert_eq!(hx(&hmac(b"Jefe", b"what do ya want for nothing?")), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        // Caso 6: clave más larga que un bloque.
        assert_eq!(
            hx(&hmac(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn pbkdf2_vectores() {
        let mut o = [0u8; 32];
        pbkdf2(b"password", b"salt", 1, &mut o);
        assert_eq!(hx(&o), "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b");
        pbkdf2(b"password", b"salt", 2, &mut o);
        assert_eq!(hx(&o), "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43");
        pbkdf2(b"password", b"salt", 4096, &mut o);
        assert_eq!(hx(&o), "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a");
        // Salida de más de un bloque.
        let mut o = [0u8; 40];
        pbkdf2(b"passwordPASSWORDpassword", b"saltSALTsaltSALTsaltSALTsaltSALTsalt", 4096, &mut o);
        assert_eq!(hx(&o), "348c89dbcbd32b2f32d814b8116e84cf2b17347ebc1800181c4e2a1fb8dd53e1c635518c7dac47e9");
    }

    #[test]
    fn utilidades() {
        assert!(ct_eq(b"abc", b"abc") && !ct_eq(b"abc", b"abd") && !ct_eq(b"abc", b"ab"));
        assert_eq!(from_hex("00ff10").unwrap(), [0, 255, 16]);
        assert!(from_hex("0").is_none() && from_hex("zz").is_none());
        let mut k = [7u8; 8];
        wipe(&mut k);
        assert_eq!(k, [0; 8]);
    }
}
