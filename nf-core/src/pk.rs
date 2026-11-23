// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE

//! Clave pública: verificación de firmas (RSA, ECDSA) y ECDH en P-256, para TLS y X.509.
//! Contrato fijo de la v0.12.2: `x509` y `tls` solo usan lo que hay aquí.
//! Normas: RFC 8017 (RSA PKCS#1 v1.5 y PSS), FIPS 186-4 y SEC 1 (ECDSA, curvas), RFC 5903 (ECDH).

use crate::crypto::{ct_eq, sha256, sha384, sha512};
use alloc::vec;
use alloc::vec::Vec;

mod bn;
mod ec;
#[cfg(test)]
mod vectores;

pub use ec::{p256_ecdh, p256_public};

/// Clave pública de un certificado (SubjectPublicKeyInfo ya decodificado).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PubKey {
    /// Módulo y exponente en big endian, sin ceros a la izquierda.
    Rsa { n: Vec<u8>, e: Vec<u8> },
    /// Punto sin comprimir (0x04 ‖ X ‖ Y), 65 bytes.
    EcP256(Vec<u8>),
    /// Punto sin comprimir (0x04 ‖ X ‖ Y), 97 bytes.
    EcP384(Vec<u8>),
}

/// Algoritmo de firma (de un certificado o del CertificateVerify / ServerKeyExchange de TLS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigAlg {
    RsaPkcs1Sha256,
    RsaPkcs1Sha384,
    RsaPkcs1Sha512,
    RsaPssSha256,
    RsaPssSha384,
    RsaPssSha512,
    EcdsaSha256,
    EcdsaSha384,
    EcdsaSha512,
}

/// Comprueba `sig` sobre `msg` (el mensaje entero; la función hace el hash). Las firmas ECDSA vienen en
/// DER (`SEQUENCE { r INTEGER, s INTEGER }`), como en X.509 y TLS. PSS: MGF1 con el mismo hash y sal del
/// tamaño del hash. RSA: módulos de 2048 a 4096 bits; fuera de eso, `false`.
/// Solo usa datos públicos (puede tardar según el contenido). Un algoritmo que no corresponde al tipo de
/// clave (RSA con clave EC y al revés) da `false`; una clave EC admite cualquiera de los tres hashes.
pub fn verify(key: &PubKey, alg: SigAlg, msg: &[u8], sig: &[u8]) -> bool {
    let (kind, hash) = split(alg);
    match (key, kind) {
        (PubKey::Rsa { n, e }, Kind::Pkcs1) => rsa_verify(n, e, sig, |em, _| em_pkcs1(em, hash, &hash.digest(msg))),
        (PubKey::Rsa { n, e }, Kind::Pss) => rsa_verify(n, e, sig, |em, bits| pss_check(em, bits, hash, &hash.digest(msg))),
        (PubKey::EcP256(q), Kind::Ecdsa) => ecdsa(ec::Id::P256, q, hash, msg, sig),
        (PubKey::EcP384(q), Kind::Ecdsa) => ecdsa(ec::Id::P384, q, hash, msg, sig),
        _ => false,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Pkcs1,
    Pss,
    Ecdsa,
}

#[derive(Clone, Copy, PartialEq)]
enum Hash {
    S256,
    S384,
    S512,
}

impl Hash {
    fn len(self) -> usize {
        match self {
            Hash::S256 => 32,
            Hash::S384 => 48,
            Hash::S512 => 64,
        }
    }

    fn digest(self, data: &[u8]) -> Vec<u8> {
        match self {
            Hash::S256 => sha256(data).to_vec(),
            Hash::S384 => sha384(data).to_vec(),
            Hash::S512 => sha512(data).to_vec(),
        }
    }

    /// Prefijo DER del DigestInfo (RFC 8017 §9.2, nota 1); el resumen va detrás.
    fn digest_info(self) -> &'static [u8] {
        match self {
            Hash::S256 => &[0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20],
            Hash::S384 => &[0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02, 0x05, 0x00, 0x04, 0x30],
            Hash::S512 => &[0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05, 0x00, 0x04, 0x40],
        }
    }
}

fn split(alg: SigAlg) -> (Kind, Hash) {
    use SigAlg::*;
    match alg {
        RsaPkcs1Sha256 => (Kind::Pkcs1, Hash::S256),
        RsaPkcs1Sha384 => (Kind::Pkcs1, Hash::S384),
        RsaPkcs1Sha512 => (Kind::Pkcs1, Hash::S512),
        RsaPssSha256 => (Kind::Pss, Hash::S256),
        RsaPssSha384 => (Kind::Pss, Hash::S384),
        RsaPssSha512 => (Kind::Pss, Hash::S512),
        EcdsaSha256 => (Kind::Ecdsa, Hash::S256),
        EcdsaSha384 => (Kind::Ecdsa, Hash::S384),
        EcdsaSha512 => (Kind::Ecdsa, Hash::S512),
    }
}

// ---------------------------------------------------------------------------------------------
// RSA (RFC 8017): sig^e mod n y comprobación del bloque codificado
// ---------------------------------------------------------------------------------------------

/// Calcula `sig^e mod n` y pasa el bloque (k bytes) y el tamaño del módulo en bits a `check`.
/// Rechaza módulos fuera de 2048..=4096 bits o pares, exponentes pares, < 3 o de más de 32 bits
/// (shortcut: e de hasta 32 bits cubre 3, 17 y 65537; ampliar si un certificado real lo pidiera),
/// y firmas de longitud distinta a la del módulo o mayores o iguales que n.
fn rsa_verify(n: &[u8], e: &[u8], sig: &[u8], check: impl Fn(&[u8], usize) -> bool) -> bool {
    let n = &n[n.iter().take_while(|&&x| x == 0).count()..];
    let e = &e[e.iter().take_while(|&&x| x == 0).count()..];
    if n.is_empty() || e.is_empty() || e.len() > 4 || n[n.len() - 1] & 1 == 0 {
        return false;
    }
    let exp = e.iter().fold(0u64, |a, &b| a << 8 | b as u64);
    let mod_bits = n.len() * 8 - n[0].leading_zeros() as usize;
    if !(2048..=4096).contains(&mod_bits) || exp < 3 || exp & 1 == 0 || sig.len() != n.len() {
        return false;
    }
    let l = n.len().div_ceil(8);
    let (Some(nl), Some(s)) = (bn::from_be(n, l), bn::from_be(sig, l)) else {
        return false;
    };
    if bn::lt(&s, &nl) == 0 {
        return false;
    }
    let f = bn::Mont::new(nl);
    let m = f.from(&f.pow(&f.to(&s), &[exp]));
    check(&bn::to_be(&m, n.len()), mod_bits)
}

/// EM = 00 01 FF…FF 00 ‖ DigestInfo ‖ H, con al menos 8 FF; se compara el bloque entero (RFC 8017 §8.2.2).
fn em_pkcs1(em: &[u8], hash: Hash, h: &[u8]) -> bool {
    let t = [hash.digest_info(), h].concat();
    if em.len() < t.len() + 11 {
        return false;
    }
    let mut want = vec![0xffu8; em.len()];
    want[0] = 0;
    want[1] = 1;
    want[em.len() - t.len() - 1] = 0;
    want[em.len() - t.len()..].copy_from_slice(&t);
    ct_eq(em, &want)
}

/// MGF1 (RFC 8017 §B.2.1).
fn mgf1(seed: &[u8], len: usize, hash: Hash) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + hash.len());
    let mut c = 0u32;
    while out.len() < len {
        out.extend(hash.digest(&[seed, &c.to_be_bytes()].concat()));
        c += 1;
    }
    out.truncate(len);
    out
}

/// EMSA-PSS-VERIFY (RFC 8017 §9.1.2) con sal del tamaño del resumen.
fn pss_check(full: &[u8], mod_bits: usize, hash: Hash, m_hash: &[u8]) -> bool {
    let (hl, sl) = (hash.len(), hash.len());
    let em_bits = mod_bits - 1;
    let em_len = em_bits.div_ceil(8);
    if full.len() < em_len || full[..full.len() - em_len].iter().any(|&b| b != 0) || em_len < hl + sl + 2 {
        return false;
    }
    let em = &full[full.len() - em_len..];
    let top = 0xffu8 >> (8 * em_len - em_bits); // bits que pueden estar activos en el primer byte
    if em[em_len - 1] != 0xbc || em[0] & !top != 0 {
        return false;
    }
    let (masked, rest) = em.split_at(em_len - hl - 1);
    let h = &rest[..hl];
    let mut db: Vec<u8> = masked.iter().zip(mgf1(h, masked.len(), hash)).map(|(a, b)| a ^ b).collect();
    db[0] &= top;
    let z = db.len() - sl - 1;
    if db[..z].iter().any(|&b| b != 0) || db[z] != 1 {
        return false;
    }
    let h2 = hash.digest(&[&[0u8; 8], m_hash, &db[z + 1..]].concat());
    ct_eq(h, &h2)
}

// ---------------------------------------------------------------------------------------------
// ECDSA con firma en DER
// ---------------------------------------------------------------------------------------------

/// Longitud DER mínima (forma corta o larga de 1-2 bytes) y el resto de la entrada.
fn der_len(b: &[u8]) -> Option<(usize, &[u8])> {
    match *b.first()? {
        l @ 0..=0x7f => Some((l as usize, &b[1..])),
        0x81 => b.get(1).filter(|&&l| l >= 0x80).map(|&l| (l as usize, &b[2..])),
        0x82 => {
            let l = u16::from_be_bytes([*b.get(1)?, *b.get(2)?]) as usize;
            (l >= 0x100 && b.len() >= 3).then(|| (l, &b[3..]))
        }
        _ => None,
    }
}

/// INTEGER DER positivo y mínimo: devuelve su valor sin el cero de signo y lo que sigue.
fn der_uint(b: &[u8]) -> Option<(&[u8], &[u8])> {
    let (&tag, b) = b.split_first()?;
    let (l, b) = der_len(b)?;
    if tag != 2 || l == 0 || b.len() < l {
        return None;
    }
    let (v, rest) = b.split_at(l);
    if v[0] & 0x80 != 0 || (l > 1 && v[0] == 0 && v[1] & 0x80 == 0) {
        return None;
    }
    Some((if v[0] == 0 { &v[1..] } else { v }, rest))
}

fn ecdsa(id: ec::Id, q: &[u8], hash: Hash, msg: &[u8], sig: &[u8]) -> bool {
    let Some((&0x30, body)) = sig.split_first() else {
        return false;
    };
    let Some((l, body)) = der_len(body) else {
        return false;
    };
    if body.len() != l {
        return false;
    }
    let Some((r, rest)) = der_uint(body) else {
        return false;
    };
    match der_uint(rest) {
        Some((s, [])) => ec::Curve::new(id).ecdsa_verify(q, &hash.digest(msg), r, s),
        _ => false,
    }
}
