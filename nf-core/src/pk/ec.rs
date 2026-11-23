// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Curvas P-256 y P-384 (FIPS 186-4, SEC 1; y² = x³ − 3x + b) en coordenadas jacobianas sobre el
//! dominio de Montgomery de `bn`, ECDSA (verificación) y ECDH P-256 (RFC 5903).
//!
//! Tiempo constante en las operaciones con la privada: el producto escalar recorre siempre todos los
//! bits (doblar, sumar y elegir con máscara) y la suma trata el infinito, P = Q y P = -Q con máscaras,
//! sin ramas ni accesos a memoria que dependan del escalar. La inversión (Fermat) usa exponentes públicos.
//! shortcut: los intermedios (`Vec`) no se borran uno a uno, solo el escalar y el punto resultado; con un
//! único programa en la máquina vale, borrarlos también cuando haya programas separados.
use super::bn::{self, Mont};
use crate::crypto::from_hex;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Id {
    P256,
    P384,
}

// p, n, b, Gx, Gy (big endian, hexadecimal).
const P256: [&str; 5] = [
    "ffffffff00000001000000000000000000000000ffffffffffffffffffffffff",
    "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551",
    "5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604b",
    "6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296",
    "4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5",
];
const P384: [&str; 5] = [
    "fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffeffffffff0000000000000000ffffffff",
    "ffffffffffffffffffffffffffffffffffffffffffffffffc7634d81f4372ddf581a0db248b0a77aecec196accc52973",
    "b3312fa7e23ee7e4988e056be3f82d19181d9c6efe8141120314088f5013875ac656398d8a2ed19d2a85c8edd3ec2aef",
    "aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b9859f741e082542a385502f25dbf55296c3a545e3872760ab7",
    "3617de4a96262c6f5d9e98bf9292dc29f8f41dbd289a147ce9da3113b5f0b8c00a60b1ce1d7e819d7a431d7c90ea0e5f",
];

/// Punto jacobiano (X/Z², Y/Z³) con coordenadas en Montgomery; Z = 0 es el infinito.
#[derive(Clone)]
pub struct Pt {
    x: Vec<u64>,
    y: Vec<u64>,
    z: Vec<u64>,
}

impl Pt {
    fn wipe(&mut self) {
        for v in [&mut self.x, &mut self.y, &mut self.z] {
            bn::wipe64(v);
        }
    }
}

fn sel_pt(mask: u64, a: &Pt, b: &Pt) -> Pt {
    Pt { x: bn::sel(mask, &a.x, &b.x), y: bn::sel(mask, &a.y, &b.y), z: bn::sel(mask, &a.z, &b.z) }
}

pub struct Curve {
    pub p: Mont,
    pub n: Mont,
    b: Vec<u64>,
    g: Pt,
    /// Bytes de un escalar o una coordenada.
    pub bytes: usize,
}

impl Curve {
    pub fn new(id: Id) -> Curve {
        let (c, bytes) = if id == Id::P256 { (&P256, 32) } else { (&P384, 48) };
        let l = bytes / 8;
        let v = |i: usize| bn::from_be(&from_hex(c[i]).unwrap(), l).unwrap();
        let (p, n) = (Mont::new(v(0)), Mont::new(v(1)));
        let g = Pt { x: p.to(&v(3)), y: p.to(&v(4)), z: p.one() };
        Curve { b: p.to(&v(2)), p, n, g, bytes }
    }

    fn inf(&self) -> Pt {
        Pt { x: self.p.one(), y: self.p.one(), z: vec![0; self.p.m.len()] }
    }

    /// Punto sin comprimir (0x04 ‖ X ‖ Y) comprobado: coordenadas < p y sobre la curva.
    pub fn parse(&self, b: &[u8]) -> Option<Pt> {
        let (l, k) = (self.p.m.len(), self.bytes);
        if b.len() != 1 + 2 * k || b[0] != 4 {
            return None;
        }
        let f = &self.p;
        let x = bn::from_be(&b[1..1 + k], l)?;
        let y = bn::from_be(&b[1 + k..], l)?;
        if bn::lt(&x, &f.m) == 0 || bn::lt(&y, &f.m) == 0 {
            return None;
        }
        let (x, y) = (f.to(&x), f.to(&y));
        // y² = x³ - 3x + b
        let x2 = f.mul(&x, &x);
        let three_x = f.add(&f.add(&x, &x), &x);
        let rhs = f.add(&f.sub(&f.mul(&x2, &x), &three_x), &self.b);
        if bn::is_zero(&f.sub(&f.mul(&y, &y), &rhs)) == 0 {
            return None;
        }
        Some(Pt { x, y, z: f.one() })
    }

    fn dbl(&self, q: &Pt) -> Pt {
        let f = &self.p;
        let d = |v: &[u64]| f.add(v, v);
        let delta = f.mul(&q.z, &q.z);
        let gamma = f.mul(&q.y, &q.y);
        let beta = f.mul(&q.x, &gamma);
        let t = f.mul(&f.sub(&q.x, &delta), &f.add(&q.x, &delta));
        let alpha = f.add(&d(&t), &t);
        let b4 = d(&d(&beta));
        let x3 = f.sub(&f.mul(&alpha, &alpha), &d(&b4));
        let yz = f.add(&q.y, &q.z);
        let z3 = f.sub(&f.sub(&f.mul(&yz, &yz), &gamma), &delta);
        let g8 = d(&d(&d(&f.mul(&gamma, &gamma))));
        let y3 = f.sub(&f.mul(&alpha, &f.sub(&b4, &x3)), &g8);
        Pt { x: x3, y: y3, z: z3 }
    }

    /// Suma completa: el resultado correcto para cualquier par (infinito, iguales u opuestos incluidos).
    fn add(&self, p: &Pt, q: &Pt) -> Pt {
        let f = &self.p;
        let d = |v: &[u64]| f.add(v, v);
        let (z1z1, z2z2) = (f.mul(&p.z, &p.z), f.mul(&q.z, &q.z));
        let u1 = f.mul(&p.x, &z2z2);
        let u2 = f.mul(&q.x, &z1z1);
        let s1 = f.mul(&p.y, &f.mul(&q.z, &z2z2));
        let s2 = f.mul(&q.y, &f.mul(&p.z, &z1z1));
        let h = f.sub(&u2, &u1);
        let i = f.mul(&d(&h), &d(&h));
        let j = f.mul(&h, &i);
        let r = d(&f.sub(&s2, &s1));
        let v = f.mul(&u1, &i);
        let x3 = f.sub(&f.sub(&f.mul(&r, &r), &j), &d(&v));
        let y3 = f.sub(&f.mul(&r, &f.sub(&v, &x3)), &d(&f.mul(&s1, &j)));
        let zz = f.add(&p.z, &q.z);
        let z3 = f.mul(&f.sub(&f.sub(&f.mul(&zz, &zz), &z1z1), &z2z2), &h);
        let sum = Pt { x: x3, y: y3, z: z3 };
        // Con H = 0 y r ≠ 0 (opuestos) Z3 ya es 0 = infinito; faltan P = Q (doblar) y los infinitos.
        let (inf1, inf2) = (bn::is_zero(&p.z), bn::is_zero(&q.z));
        let same = bn::is_zero(&h) & bn::is_zero(&r) & !inf1 & !inf2;
        let res = sel_pt(same, &self.dbl(p), &sum);
        let res = sel_pt(inf2, p, &res);
        sel_pt(inf1, q, &res)
    }

    /// k·P con `k` en limbs de la longitud de n; recorre todos los bits de la curva.
    pub fn mul(&self, k: &[u64], p: &Pt) -> Pt {
        let mut r = self.inf();
        for i in (0..self.bytes * 8).rev() {
            let t = self.dbl(&r);
            let s = self.add(&t, p);
            let bit = (k[i / 64] >> (i % 64)) & 1;
            r.wipe();
            r = sel_pt(bit.wrapping_neg(), &s, &t);
        }
        r
    }

    /// Coordenadas afines (x, y) en claro; `None` si es el infinito.
    pub fn affine(&self, q: &Pt) -> Option<(Vec<u64>, Vec<u64>)> {
        if bn::is_zero(&q.z) != 0 {
            return None;
        }
        let f = &self.p;
        let zi = f.inv(&q.z);
        let zi2 = f.mul(&zi, &zi);
        Some((f.from(&f.mul(&q.x, &zi2)), f.from(&f.mul(&q.y, &f.mul(&zi2, &zi)))))
    }

    /// Escalar de `b` big endian en [1, n-1].
    fn scalar(&self, b: &[u8]) -> Option<Vec<u64>> {
        let k = bn::from_be(b, self.n.m.len())?;
        (bn::is_zero(&k) == 0 && bn::lt(&k, &self.n.m) != 0).then_some(k)
    }

    /// ECDSA (FIPS 186-4 §6.4): `hash` es el resumen del mensaje, `r` y `s` enteros big endian.
    pub fn ecdsa_verify(&self, q: &[u8], hash: &[u8], r: &[u8], s: &[u8]) -> bool {
        let (Some(q), Some(r), Some(s)) = (self.parse(q), self.scalar(r), self.scalar(s)) else {
            return false;
        };
        let n = &self.n;
        // e = bits más a la izquierda del resumen, módulo n (p y n miden lo mismo: basta una resta).
        let Some(e) = bn::from_be(&hash[..hash.len().min(self.bytes)], n.m.len()) else {
            return false;
        };
        let e = bn::reduce_once(&e, &n.m);
        let w = n.inv(&n.to(&s));
        let u1 = n.from(&n.mul(&n.to(&e), &w));
        let u2 = n.from(&n.mul(&n.to(&r), &w));
        let sum = self.add(&self.mul(&u1, &self.g), &self.mul(&u2, &q));
        match self.affine(&sum) {
            Some((x, _)) => bn::reduce_once(&x, &n.m) == r,
            None => false,
        }
    }
}

/// Clave pública P-256 (0x04 ‖ X ‖ Y) de una privada de 32 bytes; `None` si no está en [1, n-1].
pub fn p256_public(priv_: &[u8; 32]) -> Option<[u8; 65]> {
    let c = Curve::new(Id::P256);
    let mut k = c.scalar(priv_)?;
    let mut q = c.mul(&k, &c.g);
    bn::wipe64(&mut k);
    let a = c.affine(&q);
    q.wipe();
    let (x, y) = a?;
    let mut out = [0u8; 65];
    out[0] = 4;
    out[1..33].copy_from_slice(&bn::to_be(&x, 32));
    out[33..].copy_from_slice(&bn::to_be(&y, 32));
    Some(out)
}

/// ECDH P-256: coordenada X compartida. `None` si la privada no está en [1, n-1] o el punto del otro
/// lado no es válido (formato, fuera de la curva o infinito).
pub fn p256_ecdh(priv_: &[u8; 32], peer: &[u8]) -> Option<[u8; 32]> {
    let c = Curve::new(Id::P256);
    let mut k = c.scalar(priv_)?;
    let Some(q) = c.parse(peer) else {
        bn::wipe64(&mut k);
        return None;
    };
    let mut s = c.mul(&k, &q);
    bn::wipe64(&mut k);
    let r = c.affine(&s);
    s.wipe();
    let (mut x, mut y) = r?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&bn::to_be(&x, 32));
    bn::wipe64(&mut x);
    bn::wipe64(&mut y);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::to_hex;

    fn hx(s: &str) -> Vec<u8> {
        from_hex(s).unwrap()
    }

    #[test]
    fn generador_en_curva_y_orden() {
        for id in [Id::P256, Id::P384] {
            let c = Curve::new(id);
            let mut g = vec![4u8];
            let (x, y) = c.affine(&c.g).unwrap();
            g.extend(bn::to_be(&x, c.bytes));
            g.extend(bn::to_be(&y, c.bytes));
            assert!(c.parse(&g).is_some());
            // n·G = infinito y (n-1)·G = -G (misma x).
            let l = c.n.m.len();
            assert!(c.affine(&c.mul(&c.n.m, &c.g)).is_none());
            let mut one = vec![0u64; l];
            one[0] = 1;
            let nm1 = c.n.sub(&c.n.m.clone(), &one);
            let (xm, ym) = c.affine(&c.mul(&nm1, &c.g)).unwrap();
            assert_eq!(xm, x);
            assert_eq!(c.p.add(&c.p.to(&ym), &c.p.to(&y)), vec![0; l]);
            // Punto fuera de la curva, mal formato y coordenada >= p.
            let mut bad = g.clone();
            bad[c.bytes * 2] ^= 1;
            assert!(c.parse(&bad).is_none());
            assert!(c.parse(&g[..g.len() - 1]).is_none());
            bad = g.clone();
            bad[0] = 2;
            assert!(c.parse(&bad).is_none());
            let mut big = g.clone();
            big[1..1 + c.bytes].copy_from_slice(&bn::to_be(&c.p.m, c.bytes));
            assert!(c.parse(&big).is_none());
        }
    }

    #[test]
    fn suma_completa_casos_limite() {
        let c = Curve::new(Id::P256);
        let g = &c.g;
        let two = c.dbl(g);
        let aff = |p: &Pt| c.affine(p).unwrap();
        assert_eq!(aff(&c.add(g, g)), aff(&two));
        assert_eq!(aff(&c.add(&c.inf(), g)), aff(g));
        assert_eq!(aff(&c.add(g, &c.inf())), aff(g));
        let neg = Pt { x: g.x.clone(), y: c.p.sub(&vec![0; 4], &g.y), z: g.z.clone() };
        assert!(c.affine(&c.add(g, &neg)).is_none());
        let three = c.add(&two, g);
        assert_eq!(aff(&three), aff(&c.mul(&[3, 0, 0, 0], g)));
    }

    #[test]
    fn ecdh_rfc_5903_8_1() {
        let i: [u8; 32] = hx("c88f01f510d9ac3f70a292daa2316de544e9aab8afe84049c62a9c57862d1433").try_into().unwrap();
        let r: [u8; 32] = hx("c6ef9c5d78ae012a011164acb397ce2088685d8f06bf9be0b283ab46476bee53").try_into().unwrap();
        let gi = p256_public(&i).unwrap();
        let gr = p256_public(&r).unwrap();
        assert_eq!(
            to_hex(&gi[1..]),
            "dad0b65394221cf9b051e1feca5787d098dfe637fc90b9ef945d0c37725811805271a0461cdb8252d61f1c456fa3e59ab1f45b33accf5f58389e0577b8990bb3"
        );
        assert_eq!(
            to_hex(&gr[1..]),
            "d12dfb5289c8d4f81208b70270398c342296970a0bccb74c736fc7554494bf6356fbf3ca366cc23e8157854c13c58d6aac23f046ada30f8353e74f33039872ab"
        );
        let shared = "d6840f6b42f6edafd13116e0e12565202fef8e9ece7dce03812464d04b9442de";
        assert_eq!(to_hex(&p256_ecdh(&i, &gr).unwrap()), shared);
        assert_eq!(to_hex(&p256_ecdh(&r, &gi).unwrap()), shared);
    }

    #[test]
    fn ecdh_rechaza_entradas_invalidas() {
        let k = [7u8; 32];
        let g = p256_public(&[1u8; 32]).unwrap();
        assert!(p256_ecdh(&k, &g).is_some());
        assert!(p256_ecdh(&[0u8; 32], &g).is_none());
        let n = hx(P256[1]);
        assert!(p256_ecdh(&n.clone().try_into().unwrap(), &g).is_none());
        assert!(p256_public(&n.try_into().unwrap()).is_none());
        assert!(p256_public(&[0xff; 32]).is_none());
        let mut bad = g;
        bad[64] ^= 1;
        assert!(p256_ecdh(&k, &bad).is_none());
        assert!(p256_ecdh(&k, &g[..64]).is_none());
        assert!(p256_ecdh(&k, &[0u8; 65]).is_none());
        // 1·G es G; 2·G = (G + G).
        let one = {
            let mut o = [0u8; 32];
            o[31] = 1;
            o
        };
        assert_eq!(&p256_public(&one).unwrap()[1..33], &hx(P256[3])[..]);
    }
}
