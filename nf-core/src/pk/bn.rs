// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Aritmética modular con enteros grandes: limbs de 64 bits (el menos significativo primero) y
//! multiplicación de Montgomery (CIOS) con módulo impar. Todas las operaciones de campo (`mul`, `add`,
//! `sub`, `sel`) tienen el mismo recorrido para cualquier valor: sin ramas ni índices que dependan de los
//! datos. Solo `pow` ramifica, y siempre sobre el exponente, que aquí es público (e de RSA, p-2, n-2).
use alloc::vec;
use alloc::vec::Vec;

/// `b` (big endian) como `n` limbs. `None` si no cabe.
pub fn from_be(b: &[u8], n: usize) -> Option<Vec<u64>> {
    let skip = b.iter().take_while(|&&x| x == 0).count();
    let b = &b[skip..];
    if b.len() > n * 8 {
        return None;
    }
    let mut v = vec![0u64; n];
    for (i, &x) in b.iter().rev().enumerate() {
        v[i / 8] |= (x as u64) << (8 * (i % 8));
    }
    Some(v)
}

/// `a` en big endian con exactamente `len` bytes (los altos se pierden si no caben).
pub fn to_be(a: &[u64], len: usize) -> Vec<u8> {
    (0..len).rev().map(|i| (a[i / 8] >> (8 * (i % 8))) as u8).collect()
}

/// Número de bits (sin ceros altos).
pub fn bits(a: &[u64]) -> usize {
    a.iter().rposition(|&x| x != 0).map_or(0, |i| i * 64 + 64 - a[i].leading_zeros() as usize)
}

/// Borra limbs (secretos) de forma que el compilador no pueda omitirlo.
pub fn wipe64(a: &mut [u64]) {
    for x in a.iter_mut() {
        // SAFETY: `x` es una referencia válida y exclusiva.
        unsafe { core::ptr::write_volatile(x, 0) };
    }
}

/// a + b y acarreo.
fn adc(a: &[u64], b: &[u64]) -> (Vec<u64>, u64) {
    let mut c = 0u64;
    let r = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| {
            let s = x as u128 + y as u128 + c as u128;
            c = (s >> 64) as u64;
            s as u64
        })
        .collect();
    (r, c)
}

/// a - b y préstamo (1 si a < b).
fn sbb(a: &[u64], b: &[u64]) -> (Vec<u64>, u64) {
    let mut bw = 0u64;
    let r = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| {
            let d = (x as u128).wrapping_sub(y as u128).wrapping_sub(bw as u128);
            bw = ((d >> 64) as u64) & 1;
            d as u64
        })
        .collect();
    (r, bw)
}

/// `a` si `mask` es todo unos, `b` si es cero.
pub fn sel(mask: u64, a: &[u64], b: &[u64]) -> Vec<u64> {
    a.iter().zip(b).map(|(&x, &y)| (x & mask) | (y & !mask)).collect()
}

/// Máscara todo unos si `a` es cero.
pub fn is_zero(a: &[u64]) -> u64 {
    let x = a.iter().fold(0u64, |acc, &v| acc | v);
    (((x | x.wrapping_neg()) >> 63) ^ 1).wrapping_neg()
}

/// Máscara todo unos si `a < b`.
pub fn lt(a: &[u64], b: &[u64]) -> u64 {
    sbb(a, b).1.wrapping_neg()
}

/// `a` si `a < m`, si no `a - m` (una sola resta: vale cuando `a < 2m`).
pub fn reduce_once(a: &[u64], m: &[u64]) -> Vec<u64> {
    let (d, bw) = sbb(a, m);
    sel(bw.wrapping_neg(), a, &d)
}

/// Contexto de Montgomery para un módulo impar `m` (con `m > 1`).
pub struct Mont {
    pub m: Vec<u64>,
    n0: u64, // -m^-1 mod 2^64
    r2: Vec<u64>, // R^2 mod m, con R = 2^(64·n)
}

impl Mont {
    pub fn new(m: Vec<u64>) -> Mont {
        assert!(m[0] & 1 == 1, "módulo par");
        let n = m.len();
        // Inverso de m[0] mod 2^64 por Newton (cada vuelta duplica los bits correctos).
        let mut inv = 1u64;
        for _ in 0..6 {
            inv = inv.wrapping_mul(2u64.wrapping_sub(m[0].wrapping_mul(inv)));
        }
        let mut r2 = vec![0u64; n];
        r2[0] = 1;
        for _ in 0..128 * n {
            r2 = Self::add_m(&r2, &r2, &m);
        }
        Mont { m, n0: inv.wrapping_neg(), r2 }
    }

    fn add_m(a: &[u64], b: &[u64], m: &[u64]) -> Vec<u64> {
        let (s, c) = adc(a, b);
        let (d, bw) = sbb(&s, m);
        // Se usa la resta si hubo acarreo o si no hubo préstamo.
        sel((c | (bw ^ 1)).wrapping_neg(), &d, &s)
    }

    pub fn add(&self, a: &[u64], b: &[u64]) -> Vec<u64> {
        Self::add_m(a, b, &self.m)
    }

    pub fn sub(&self, a: &[u64], b: &[u64]) -> Vec<u64> {
        let (d, bw) = sbb(a, b);
        let (e, _) = adc(&d, &self.m);
        sel(bw.wrapping_neg(), &e, &d)
    }

    /// a·b·R^-1 mod m (entradas < m).
    pub fn mul(&self, a: &[u64], b: &[u64]) -> Vec<u64> {
        let (n, m) = (self.m.len(), &self.m);
        let mut t = vec![0u64; n + 2];
        for &bi in b {
            let mut c = 0u128;
            for j in 0..n {
                let s = t[j] as u128 + a[j] as u128 * bi as u128 + c;
                t[j] = s as u64;
                c = s >> 64;
            }
            let s = t[n] as u128 + c;
            t[n] = s as u64;
            t[n + 1] = (s >> 64) as u64;
            let q = t[0].wrapping_mul(self.n0) as u128;
            let mut c = (t[0] as u128 + q * m[0] as u128) >> 64;
            for j in 1..n {
                let s = t[j] as u128 + q * m[j] as u128 + c;
                t[j - 1] = s as u64;
                c = s >> 64;
            }
            let s = t[n] as u128 + c;
            t[n - 1] = s as u64;
            t[n] = t[n + 1] + (s >> 64) as u64;
        }
        let (d, bw) = sbb(&t[..n], m);
        // Se resta si el resultado desbordó n limbs o si t >= m.
        sel((t[n] | (bw ^ 1)).wrapping_neg(), &d, &t[..n])
    }

    /// Entra en el dominio de Montgomery (`a` < m).
    pub fn to(&self, a: &[u64]) -> Vec<u64> {
        self.mul(a, &self.r2)
    }

    /// Sale del dominio de Montgomery.
    pub fn from(&self, a: &[u64]) -> Vec<u64> {
        let mut one = vec![0u64; self.m.len()];
        one[0] = 1;
        self.mul(a, &one)
    }

    /// El 1 en el dominio de Montgomery.
    pub fn one(&self) -> Vec<u64> {
        let mut one = vec![0u64; self.m.len()];
        one[0] = 1;
        self.to(&one)
    }

    /// a^e (a en Montgomery; e = limbs, PÚBLICO: ramifica según sus bits).
    pub fn pow(&self, a: &[u64], e: &[u64]) -> Vec<u64> {
        let mut r = self.one();
        for i in (0..bits(e)).rev() {
            r = self.mul(&r, &r);
            if (e[i / 64] >> (i % 64)) & 1 == 1 {
                r = self.mul(&r, a);
            }
        }
        r
    }

    /// a^-1 por Fermat; solo con módulo primo. `a` en Montgomery (si es 0 devuelve 0).
    pub fn inv(&self, a: &[u64]) -> Vec<u64> {
        let mut two = vec![0u64; self.m.len()];
        two[0] = 2;
        let (e, _) = sbb(&self.m, &two);
        self.pow(a, &e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mont_pequeno() {
        // 97 es primo: 5^96 = 1, 5·inv(5) = 1, (90 + 20) mod 97 = 13, (3 - 5) mod 97 = 95.
        let f = Mont::new(vec![97]);
        let a = f.to(&[5]);
        assert_eq!(f.from(&f.pow(&a, &[96])), [1]);
        assert_eq!(f.from(&f.mul(&a, &f.inv(&a))), [1]);
        assert_eq!(f.from(&f.add(&f.to(&[90]), &f.to(&[20]))), [13]);
        assert_eq!(f.from(&f.sub(&f.to(&[3]), &a)), [95]);
        assert_eq!(f.from(&f.mul(&f.to(&[96]), &f.to(&[96]))), [1]);
    }

    #[test]
    fn mont_multilimb_y_utilidades() {
        // 2^127 - 1 (primo de Mersenne) en 2 limbs: 3^(p-1) = 1 y el inverso funciona.
        let f = Mont::new(vec![u64::MAX, (1 << 63) - 1]);
        let a = f.to(&[3, 0]);
        assert_eq!(f.from(&f.mul(&a, &f.inv(&a))), [1, 0]);
        assert_eq!(f.from(&f.pow(&a, &[u64::MAX - 1, (1 << 63) - 1])), [1, 0]);
        assert_eq!(bits(&[0, 1 << 63]), 128);
        assert_eq!(from_be(&[0, 0, 1, 2], 1).unwrap(), [0x102]);
        assert!(from_be(&[1; 9], 1).is_none());
        assert_eq!(to_be(&[0x0102], 3), [0, 1, 2]);
        assert_eq!(is_zero(&[0, 0]), u64::MAX);
        assert_eq!(is_zero(&[0, 1]), 0);
        assert_eq!(lt(&[1, 0], &[0, 1]), u64::MAX);
        assert_eq!(lt(&[0, 1], &[0, 1]), 0);
    }
}
