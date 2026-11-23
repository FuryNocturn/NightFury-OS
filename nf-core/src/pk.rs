// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE

//! Clave pública: verificación de firmas (RSA, ECDSA) y ECDH en P-256, para TLS y X.509.
//! Contrato fijo de la v0.12.2: `x509` y `tls` solo usan lo que hay aquí.

use alloc::vec::Vec;

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
pub fn verify(_key: &PubKey, _alg: SigAlg, _msg: &[u8], _sig: &[u8]) -> bool {
    false // lo rellena el paso 1
}
