// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Primitivas criptográficas propias (clean room): escritas a partir de las normas públicas,
//! sin copiar código de nadie. Cada una se comprueba con los vectores oficiales en las pruebas.
//!
//! - SHA-256: FIPS 180-4.
//! - HMAC-SHA-256: RFC 2104 (vectores de RFC 4231).
//! - PBKDF2-HMAC-SHA-256: RFC 8018.
//! - HKDF-SHA-256: RFC 5869.
//! - ChaCha20, Poly1305 y su AEAD: RFC 8439.
//! - X25519: RFC 7748.
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

/// HKDF-Extract (RFC 5869, SHA-256). Una sal vacía equivale a 32 ceros, como pide la norma.
pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
    hmac(salt, ikm)
}

/// HKDF-Expand (RFC 5869, SHA-256): rellena `out`.
/// Panic si `out.len() > 255 * 32` (error de programación, no de datos).
pub fn hkdf_expand(prk: &[u8; 32], info: &[u8], out: &mut [u8]) {
    assert!(out.len() <= 255 * 32, "hkdf_expand: salida demasiado larga");
    let h = Hmac::new(prk);
    let mut t = [0u8; 32];
    for (i, chunk) in out.chunks_mut(32).enumerate() {
        let prev: &[u8] = if i == 0 { &[] } else { &t };
        t = h.mac(&[prev, info, &[i as u8 + 1]]);
        chunk.copy_from_slice(&t[..chunk.len()]);
    }
    wipe(&mut t);
}

// ---------------------------------------------------------------------------------------------
// ChaCha20 (RFC 8439 §2.1-2.4)
// ---------------------------------------------------------------------------------------------

fn quarter(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

/// Un bloque de 64 bytes del flujo de ChaCha20.
pub fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut init = [0u32; 16];
    init[..4].copy_from_slice(&[0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574]);
    for (w, c) in init[4..12].iter_mut().zip(key.chunks_exact(4)) {
        *w = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
    }
    init[12] = counter;
    for (w, c) in init[13..].iter_mut().zip(nonce.chunks_exact(4)) {
        *w = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
    }
    let mut s = init;
    for _ in 0..10 {
        quarter(&mut s, 0, 4, 8, 12);
        quarter(&mut s, 1, 5, 9, 13);
        quarter(&mut s, 2, 6, 10, 14);
        quarter(&mut s, 3, 7, 11, 15);
        quarter(&mut s, 0, 5, 10, 15);
        quarter(&mut s, 1, 6, 11, 12);
        quarter(&mut s, 2, 7, 8, 13);
        quarter(&mut s, 3, 4, 9, 14);
    }
    let mut out = [0u8; 64];
    for i in 0..16 {
        out[i * 4..i * 4 + 4].copy_from_slice(&s[i].wrapping_add(init[i]).to_le_bytes());
    }
    // Borrado simple: el estado contiene la clave.
    for w in s.iter_mut().chain(init.iter_mut()) {
        // SAFETY: `w` es una referencia válida y exclusiva.
        unsafe { core::ptr::write_volatile(w, 0) };
    }
    out
}

/// Cifra o descifra `data` en el sitio (XOR con el flujo), empezando en el contador `counter`.
pub fn chacha20_xor(key: &[u8; 32], counter: u32, nonce: &[u8; 12], data: &mut [u8]) {
    for (n, chunk) in data.chunks_mut(64).enumerate() {
        let mut ks = chacha20_block(key, counter.wrapping_add(n as u32), nonce);
        for (d, k) in chunk.iter_mut().zip(ks) {
            *d ^= k;
        }
        wipe(&mut ks);
    }
}

// ---------------------------------------------------------------------------------------------
// Poly1305 (RFC 8439 §2.5), limbs de 26 bits: sin ramas ni índices que dependan de la clave
// ---------------------------------------------------------------------------------------------

fn le32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// Etiqueta Poly1305 de 16 bytes de `msg` con una clave de un solo uso.
pub fn poly1305(key: &[u8; 32], msg: &[u8]) -> [u8; 16] {
    const M: u32 = 0x3ff_ffff;
    let r: [u64; 5] = [
        (le32(key, 0) & 0x3ff_ffff) as u64,
        ((le32(key, 3) >> 2) & 0x3ff_ff03) as u64,
        ((le32(key, 6) >> 4) & 0x3ff_c0ff) as u64,
        ((le32(key, 9) >> 6) & 0x3f0_3fff) as u64,
        ((le32(key, 12) >> 8) & 0x00f_ffff) as u64,
    ];
    let s = [0, r[1] * 5, r[2] * 5, r[3] * 5, r[4] * 5];
    let mut h = [0u32; 5];
    for chunk in msg.chunks(16) {
        let mut b = [0u8; 17];
        b[..chunk.len()].copy_from_slice(chunk);
        b[chunk.len()] = 1; // el bit 1 va tras el último byte (en un bloque completo, es el bit 128)
        h[0] += le32(&b, 0) & M;
        h[1] += (le32(&b, 3) >> 2) & M;
        h[2] += (le32(&b, 6) >> 4) & M;
        h[3] += (le32(&b, 9) >> 6) & M;
        h[4] += (le32(&b, 12) >> 8) | (b[16] as u32) << 24;
        let m = |x: usize, y: u64| h[x] as u64 * y;
        let d = [
            m(0, r[0]) + m(1, s[4]) + m(2, s[3]) + m(3, s[2]) + m(4, s[1]),
            m(0, r[1]) + m(1, r[0]) + m(2, s[4]) + m(3, s[3]) + m(4, s[2]),
            m(0, r[2]) + m(1, r[1]) + m(2, r[0]) + m(3, s[4]) + m(4, s[3]),
            m(0, r[3]) + m(1, r[2]) + m(2, r[1]) + m(3, r[0]) + m(4, s[4]),
            m(0, r[4]) + m(1, r[3]) + m(2, r[2]) + m(3, r[1]) + m(4, r[0]),
        ];
        let mut c = 0u64;
        for i in 0..5 {
            let v = d[i] + c;
            h[i] = (v & M as u64) as u32;
            c = v >> 26;
        }
        h[0] += (c * 5) as u32;
        h[1] += h[0] >> 26;
        h[0] &= M;
    }
    // Reducción final completa módulo 2^130-5.
    for _ in 0..3 {
        for i in 0..4 {
            h[i + 1] += h[i] >> 26;
            h[i] &= M;
        }
        h[0] += 5 * (h[4] >> 26);
        h[4] &= M;
    }
    let mut g = [0u32; 5];
    let mut c = 5u32;
    for i in 0..5 {
        g[i] = h[i].wrapping_add(c);
        c = g[i] >> 26;
        g[i] &= M;
    }
    // Si h + 5 desborda 2^130, h >= p y se usa g; la máscara evita ramas.
    let mask = 0u32.wrapping_sub(c);
    for i in 0..5 {
        h[i] = (h[i] & !mask) | (g[i] & mask);
    }
    let w = [h[0] | h[1] << 26, h[1] >> 6 | h[2] << 20, h[2] >> 12 | h[3] << 14, h[3] >> 18 | h[4] << 8];
    let mut tag = [0u8; 16];
    let mut f = 0u64;
    for i in 0..4 {
        f = w[i] as u64 + le32(key, 16 + 4 * i) as u64 + (f >> 32);
        tag[4 * i..4 * i + 4].copy_from_slice(&(f as u32).to_le_bytes());
    }
    tag
}

// ---------------------------------------------------------------------------------------------
// AEAD ChaCha20-Poly1305 (RFC 8439 §2.8)
// ---------------------------------------------------------------------------------------------

/// Etiqueta sobre aad ‖ relleno ‖ cifrado ‖ relleno ‖ longitudes, con la clave de un solo uso del bloque 0.
fn aead_tag(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], ct: &[u8]) -> [u8; 16] {
    let mut block = chacha20_block(key, 0, nonce);
    let mut otk = [0u8; 32];
    otk.copy_from_slice(&block[..32]);
    let pad = |n: usize| (16 - n % 16) % 16;
    let mut m = Vec::with_capacity(aad.len() + ct.len() + 32);
    m.extend_from_slice(aad);
    m.resize(m.len() + pad(aad.len()), 0);
    m.extend_from_slice(ct);
    m.resize(m.len() + pad(ct.len()), 0);
    m.extend_from_slice(&(aad.len() as u64).to_le_bytes());
    m.extend_from_slice(&(ct.len() as u64).to_le_bytes());
    let tag = poly1305(&otk, &m);
    wipe(&mut block);
    wipe(&mut otk);
    tag
}

/// Cifra y autentica: devuelve cifrado ‖ etiqueta de 16 bytes. Nunca repitas (clave, nonce).
pub fn aead_seal(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let mut out = plaintext.to_vec();
    chacha20_xor(key, 1, nonce, &mut out);
    let tag = aead_tag(key, nonce, aad, &out);
    out.extend_from_slice(&tag);
    out
}

/// Verifica y descifra. `None` si es más corto que la etiqueta o esta no coincide (no se descifra nada).
pub fn aead_open(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    let n = sealed.len().checked_sub(16)?;
    let (ct, tag) = sealed.split_at(n);
    if !ct_eq(&aead_tag(key, nonce, aad, ct), tag) {
        return None;
    }
    let mut out = ct.to_vec();
    chacha20_xor(key, 1, nonce, &mut out);
    Some(out)
}

// ---------------------------------------------------------------------------------------------
// X25519 (RFC 7748): campo GF(2^255-19) con 5 limbs de 51 bits, escalera de Montgomery
// ---------------------------------------------------------------------------------------------

type Fe = [u64; 5];
const M51: u64 = (1 << 51) - 1;

/// Punto base: u = 9.
pub const X25519_BASE: [u8; 32] = {
    let mut b = [0u8; 32];
    b[0] = 9;
    b
};

fn fe_load(b: &[u8; 32]) -> Fe {
    let l = |i: usize| u64::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3], b[i + 4], b[i + 5], b[i + 6], b[i + 7]]);
    // El bit 255 se descarta con la máscara del último limb.
    [l(0) & M51, (l(6) >> 3) & M51, (l(12) >> 6) & M51, (l(19) >> 1) & M51, (l(24) >> 12) & M51]
}

/// Reduce 5 acumuladores de 128 bits a limbs de ~51 bits.
fn fe_carry(mut t: [u128; 5]) -> Fe {
    for _ in 0..2 {
        for i in 0..4 {
            t[i + 1] += t[i] >> 51;
            t[i] &= M51 as u128;
        }
        t[0] += (t[4] >> 51) * 19;
        t[4] &= M51 as u128;
    }
    [t[0] as u64, t[1] as u64, t[2] as u64, t[3] as u64, t[4] as u64]
}

fn fe_add(a: &Fe, b: &Fe) -> Fe {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3], a[4] + b[4]]
}

/// a - b sumando antes 2p para no pasar de cero (b debe venir de fe_mul/fe_carry).
fn fe_sub(a: &Fe, b: &Fe) -> Fe {
    [
        a[0] + 0xf_ffff_ffff_ffda - b[0],
        a[1] + 0xf_ffff_ffff_fffe - b[1],
        a[2] + 0xf_ffff_ffff_fffe - b[2],
        a[3] + 0xf_ffff_ffff_fffe - b[3],
        a[4] + 0xf_ffff_ffff_fffe - b[4],
    ]
}

fn fe_mul(a: &Fe, b: &Fe) -> Fe {
    let (a0, a1, a2, a3, a4) = (a[0] as u128, a[1] as u128, a[2] as u128, a[3] as u128, a[4] as u128);
    let (b0, b1, b2, b3, b4) = (b[0] as u128, b[1] as u128, b[2] as u128, b[3] as u128, b[4] as u128);
    let (c1, c2, c3, c4) = (b1 * 19, b2 * 19, b3 * 19, b4 * 19);
    fe_carry([
        a0 * b0 + a1 * c4 + a2 * c3 + a3 * c2 + a4 * c1,
        a0 * b1 + a1 * b0 + a2 * c4 + a3 * c3 + a4 * c2,
        a0 * b2 + a1 * b1 + a2 * b0 + a3 * c4 + a4 * c3,
        a0 * b3 + a1 * b2 + a2 * b1 + a3 * b0 + a4 * c4,
        a0 * b4 + a1 * b3 + a2 * b2 + a3 * b1 + a4 * b0,
    ])
}

/// Eleva al cuadrado `n` veces (n >= 1).
fn fe_sq_n(a: &Fe, n: u32) -> Fe {
    let mut r = fe_mul(a, a);
    for _ in 1..n {
        r = fe_mul(&r, &r);
    }
    r
}

/// a^(p-2) = a^-1 (cadena fija de cuadrados y productos; 0 da 0).
fn fe_invert(z: &Fe) -> Fe {
    let z2 = fe_sq_n(z, 1);
    let z9 = fe_mul(&fe_sq_n(&z2, 2), z);
    let z11 = fe_mul(&z9, &z2);
    let z5 = fe_mul(&fe_sq_n(&z11, 1), &z9); // 2^5-1
    let z10 = fe_mul(&fe_sq_n(&z5, 5), &z5);
    let z20 = fe_mul(&fe_sq_n(&z10, 10), &z10);
    let z40 = fe_mul(&fe_sq_n(&z20, 20), &z20);
    let z50 = fe_mul(&fe_sq_n(&z40, 10), &z10);
    let z100 = fe_mul(&fe_sq_n(&z50, 50), &z50);
    let z200 = fe_mul(&fe_sq_n(&z100, 100), &z100);
    let z250 = fe_mul(&fe_sq_n(&z200, 50), &z50);
    fe_mul(&fe_sq_n(&z250, 5), &z11)
}

/// Serializa reduciendo del todo módulo p.
fn fe_store(a: &Fe) -> [u8; 32] {
    let mut h = fe_carry([a[0] as u128, a[1] as u128, a[2] as u128, a[3] as u128, a[4] as u128]);
    // q = 1 si h >= p (acarreo de h + 19 más allá del bit 255).
    let mut q = (h[0] + 19) >> 51;
    for i in 1..5 {
        q = (h[i] + q) >> 51;
    }
    h[0] += 19 * q;
    for i in 0..4 {
        h[i + 1] += h[i] >> 51;
        h[i] &= M51;
    }
    h[4] &= M51;
    let mut out = [0u8; 32];
    let (mut acc, mut bits, mut o) = (0u128, 0u32, 0usize);
    for limb in h {
        acc |= (limb as u128) << bits;
        bits += 51;
        while bits >= 8 && o < 32 {
            out[o] = acc as u8;
            acc >>= 8;
            bits -= 8;
            o += 1;
        }
    }
    if o < 32 {
        out[o] = acc as u8;
    }
    out
}

/// Intercambia a y b si `swap` es 1, sin ramas.
fn fe_cswap(swap: u64, a: &mut Fe, b: &mut Fe) {
    let mask = 0u64.wrapping_sub(swap);
    for i in 0..5 {
        let t = mask & (a[i] ^ b[i]);
        a[i] ^= t;
        b[i] ^= t;
    }
}

/// X25519: multiplica el punto `u` por el escalar (con el recorte de la norma). El bit alto de `u` se ignora.
/// Tiempo constante: 255 vueltas fijas, intercambio por máscara y aritmética sin ramas ni índices secretos.
pub fn x25519(scalar: &[u8; 32], u: &[u8; 32]) -> [u8; 32] {
    let mut k = *scalar;
    k[0] &= 248;
    k[31] &= 127;
    k[31] |= 64;
    let x1 = fe_load(u);
    let (mut x2, mut z2, mut x3, mut z3) = ([1, 0, 0, 0, 0], [0; 5], x1, [1, 0, 0, 0, 0]);
    let mut swap = 0u64;
    for t in (0..255).rev() {
        let bit = ((k[t >> 3] >> (t & 7)) & 1) as u64;
        fe_cswap(swap ^ bit, &mut x2, &mut x3);
        fe_cswap(swap ^ bit, &mut z2, &mut z3);
        swap = bit;
        let a = fe_add(&x2, &z2);
        let aa = fe_mul(&a, &a);
        let b = fe_sub(&x2, &z2);
        let bb = fe_mul(&b, &b);
        let e = fe_sub(&aa, &bb);
        let c = fe_add(&x3, &z3);
        let d = fe_sub(&x3, &z3);
        let da = fe_mul(&d, &a);
        let cb = fe_mul(&c, &b);
        let s = fe_add(&da, &cb);
        x3 = fe_mul(&s, &s);
        let df = fe_sub(&da, &cb);
        z3 = fe_mul(&x1, &fe_mul(&df, &df));
        x2 = fe_mul(&aa, &bb);
        let e121665 = fe_carry(e.map(|v| v as u128 * 121665));
        z2 = fe_mul(&e, &fe_add(&aa, &e121665));
    }
    fe_cswap(swap, &mut x2, &mut x3);
    fe_cswap(swap, &mut z2, &mut z3);
    let out = fe_store(&fe_mul(&x2, &fe_invert(&z2)));
    wipe(&mut k);
    out
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

    fn a32(s: &str) -> [u8; 32] {
        from_hex(s).unwrap().try_into().unwrap()
    }

    #[test]
    fn x25519_vectores_rfc_7748_5_2() {
        let o = x25519(
            &a32("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4"),
            &a32("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c"),
        );
        assert_eq!(hx(&o), "c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552");
        // El segundo trae el bit alto de u activado (se ignora).
        let o = x25519(
            &a32("4b66e9d4d1b4673c5ad22691957d6af5c11b6421e0ea01d42ca4169e7918ba0d"),
            &a32("e5210f12786811d3f4b7959d0538ae2c31dbe7106fc03c3efc4cd549c715a493"),
        );
        assert_eq!(hx(&o), "95cbde9476e8907d7aade45cb4b873f88b595a68799fa152e6f8f7647aac7957");
    }

    fn iterar(n: u32) -> [u8; 32] {
        let (mut k, mut u) = (X25519_BASE, X25519_BASE);
        for _ in 0..n {
            let r = x25519(&k, &u);
            u = k;
            k = r;
        }
        k
    }

    #[test]
    fn x25519_iteraciones_rfc_7748() {
        assert_eq!(hx(&iterar(1)), "422c8e7a6227d7bca1350b3e2bb7279f7897b87bb6854b783c60e80311ae3079");
        assert_eq!(hx(&iterar(1000)), "684cf59ba83309552800ef566f2f4d3c1c3887c49360e3875f2eb94d99532c51");
    }

    #[test]
    #[ignore = "un millón de vueltas: lento"]
    fn x25519_un_millon_de_iteraciones() {
        assert_eq!(hx(&iterar(1_000_000)), "7c3911e0ab2586fd864497297e575e6f3bc601c0883c30df5f4dd2d24f665424");
    }

    #[test]
    fn x25519_diffie_hellman_rfc_7748_6_1() {
        let a = a32("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
        let b = a32("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb");
        let (pa, pb) = (x25519(&a, &X25519_BASE), x25519(&b, &X25519_BASE));
        assert_eq!(hx(&pa), "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a");
        assert_eq!(hx(&pb), "de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f");
        let k = "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742";
        assert_eq!(hx(&x25519(&a, &pb)), k);
        assert_eq!(hx(&x25519(&b, &pa)), k);
    }

    fn clave_00_1f() -> [u8; 32] {
        core::array::from_fn(|i| i as u8)
    }

    const SUNSCREEN: &[u8] = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";

    #[test]
    fn chacha20_rfc_8439() {
        // §2.3.2: bloque.
        let n: [u8; 12] = from_hex("000000090000004a00000000").unwrap().try_into().unwrap();
        assert_eq!(
            hx(&chacha20_block(&clave_00_1f(), 1, &n)),
            "10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4ed2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e"
        );
        // §2.4.2: cifrado.
        let n: [u8; 12] = from_hex("000000000000004a00000000").unwrap().try_into().unwrap();
        let mut d = SUNSCREEN.to_vec();
        chacha20_xor(&clave_00_1f(), 1, &n, &mut d);
        assert_eq!(
            hx(&d),
            "6e2e359a2568f98041ba0728dd0d6981e97e7aec1d4360c20a27afccfd9fae0bf91b65c5524733ab8f593dabcd62b3571639d624e65152ab8f530c359f0861d8\
             07ca0dbf500d6a6156a38e088a22b65e52bc514d16ccf806818ce91ab77937365af90bbf74a35be6b40b8eedf2785e42874d"
        );
        chacha20_xor(&clave_00_1f(), 1, &n, &mut d);
        assert_eq!(d, SUNSCREEN);
        // A.1: bloques con clave y nonce a cero.
        let (k, n) = ([0u8; 32], [0u8; 12]);
        assert_eq!(
            hx(&chacha20_block(&k, 0, &n)),
            "76b8e0ada0f13d90405d6ae55386bd28bdd219b8a08ded1aa836efcc8b770dc7da41597c5157488d7724e03fb8d84a376a43b8f41518a11cc387b669b2ee6586"
        );
        assert_eq!(
            hx(&chacha20_block(&k, 1, &n)),
            "9f07e7be5551387a98ba977c732d080dcb0f29a048e3656912c6533e32ee7aed29b721769ce64e43d57133b074d839d531ed1f28510afb45ace10a1f4b794d6f"
        );
    }

    #[test]
    fn poly1305_rfc_8439() {
        // §2.5.2
        let k = a32("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b");
        assert_eq!(hx(&poly1305(&k, b"Cryptographic Forum Research Group")), "a8061dc1305136c6c22b8baf0c0127a9");
        // A.3 nº 1: todo a cero.
        assert_eq!(poly1305(&[0; 32], &[0; 64]), [0; 16]);
        // A.3 nº 2 y 3: r = 0 (la etiqueta es s) y s = 0 sobre el mismo texto.
        let txt = b"Any submission to the IETF intended by the Contributor for publication as all or part of an IETF Internet-Draft or RFC and any statement made within the context of an IETF activity is considered an \"IETF Contribution\". Such statements include oral statements in IETF sessions, as well as written and electronic communications made at any time or place, which are addressed to";
        let mut k = [0u8; 32];
        k[16..].copy_from_slice(&from_hex("36e5f6b5c5e06070f0efca96227a863e").unwrap());
        assert_eq!(hx(&poly1305(&k, txt)), "36e5f6b5c5e06070f0efca96227a863e");
        k.rotate_left(16);
        assert_eq!(hx(&poly1305(&k, txt)), "f3477e7cd95417af89a6b8794c310cf0");
        // A.3 nº 5 y 6: casos de acarreo en la reducción final.
        let mut k = [0u8; 32];
        k[0] = 2;
        assert_eq!(hx(&poly1305(&k, &[0xff; 16])), "03000000000000000000000000000000");
        k[16..].fill(0xff);
        let mut m = [0u8; 16];
        m[0] = 2;
        assert_eq!(hx(&poly1305(&k, &m)), "03000000000000000000000000000000");
    }

    const AAD: &str = "50515253c0c1c2c3c4c5c6c7";
    const NONCE: &str = "070000004041424344454647";

    fn aead_de_prueba() -> ([u8; 32], [u8; 12], Vec<u8>, Vec<u8>) {
        let k: [u8; 32] = core::array::from_fn(|i| 0x80 + i as u8);
        let n: [u8; 12] = from_hex(NONCE).unwrap().try_into().unwrap();
        let aad = from_hex(AAD).unwrap();
        let s = aead_seal(&k, &n, &aad, SUNSCREEN);
        (k, n, aad, s)
    }

    #[test]
    fn aead_rfc_8439_2_8_2() {
        let (k, n, aad, s) = aead_de_prueba();
        assert_eq!(
            hx(&s),
            "d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d63dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b36\
             92ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc3ff4def08e4b7a9de576d26586cec64b6116\
             1ae10b594f09e26a7e902ecbd0600691"
        );
        assert_eq!(aead_open(&k, &n, &aad, &s).unwrap(), SUNSCREEN);
        // Mensaje vacío y de longitud múltiplo de 16.
        for len in [0usize, 16, 64] {
            let p = vec![7u8; len];
            assert_eq!(aead_open(&k, &n, &[], &aead_seal(&k, &n, &[], &p)).unwrap(), p);
        }
    }

    #[test]
    fn aead_rechaza_lo_alterado() {
        let (k, n, aad, s) = aead_de_prueba();
        let mut t = s.clone();
        *t.last_mut().unwrap() ^= 1; // etiqueta
        assert!(aead_open(&k, &n, &aad, &t).is_none());
        let mut t = s.clone();
        t[5] ^= 0x80; // cifrado
        assert!(aead_open(&k, &n, &aad, &t).is_none());
        assert!(aead_open(&k, &n, b"otra", &s).is_none()); // aad distinta
        let mut n2 = n;
        n2[0] ^= 1;
        assert!(aead_open(&k, &n2, &aad, &s).is_none()); // nonce distinto
        assert!(aead_open(&k, &n, &aad, &s[..15]).is_none()); // más corto que la etiqueta
        assert!(aead_open(&k, &n, &aad, &[]).is_none());
    }

    #[test]
    fn hkdf_rfc_5869() {
        let c = |s: &str| from_hex(s).unwrap();
        let rango = |a: u8, b: u8| (a..=b).collect::<Vec<u8>>();
        // Caso 1
        let prk = hkdf_extract(&c("000102030405060708090a0b0c"), &[0x0b; 22]);
        assert_eq!(hx(&prk), "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5");
        let mut o = [0u8; 42];
        hkdf_expand(&prk, &c("f0f1f2f3f4f5f6f7f8f9"), &mut o);
        assert_eq!(hx(&o), "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865");
        // Caso 2: entradas de 80 bytes
        let prk = hkdf_extract(&rango(0x60, 0xaf), &rango(0x00, 0x4f));
        assert_eq!(hx(&prk), "06a6b88c5853361a06104c9ceb35b45cef760014904671014a193f40c15fc244");
        let mut o = [0u8; 82];
        hkdf_expand(&prk, &rango(0xb0, 0xff), &mut o);
        assert_eq!(
            hx(&o),
            "b11e398dc80327a1c8e7f78c596a49344f012eda2d4efad8a050cc4c19afa97c59045a99cac7827271cb41c65e590e09da3275600c2f09b8367793a9aca3db71cc30c58179ec3e87c14c01d5c1f3434f1d87"
        );
        // Caso 3: sin sal ni info
        let prk = hkdf_extract(&[], &[0x0b; 22]);
        assert_eq!(hx(&prk), "19ef24a32c717b167f33a91d6f648bdf96596776afdb6377ac434c1c293ccb04");
        let mut o = [0u8; 42];
        hkdf_expand(&prk, &[], &mut o);
        assert_eq!(hx(&o), "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8");
    }
}
