// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Capa de registros de TLS (RFC 8446 §5): tipos, límites y cifrado AEAD con número de secuencia.
//! La reutilizará TLS 1.2 (cambian el nonce y los datos adicionales, no los tipos ni los límites).

use super::keys::Secret;
use crate::cipher::{gcm_open, gcm_seal};
use crate::crypto::{aead_open, aead_seal, wipe};
use alloc::vec::Vec;

pub const CT_CCS: u8 = 20;
pub const CT_ALERTA: u8 = 21;
pub const CT_SALUDO: u8 = 22;
pub const CT_DATOS: u8 = 23;
/// Texto claro máximo de un registro (2^14) y cifrado máximo (2^14 + 256).
pub const MAX_CLARO: usize = 1 << 14;
pub const MAX_CIFRADO: usize = MAX_CLARO + 256;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Aead {
    Chacha,
    Aes128,
    Aes256,
}

impl Aead {
    pub fn key_len(self) -> usize {
        if self == Aead::Aes128 { 16 } else { 32 }
    }

    fn seal(self, key: &[u8], nonce: &[u8; 12], aad: &[u8], pt: &[u8]) -> Vec<u8> {
        match <&[u8; 32]>::try_from(key) {
            Ok(k) if self == Aead::Chacha => aead_seal(k, nonce, aad, pt),
            _ => gcm_seal(key, nonce, aad, pt),
        }
    }

    fn open(self, key: &[u8], nonce: &[u8; 12], aad: &[u8], ct: &[u8]) -> Option<Vec<u8>> {
        match <&[u8; 32]>::try_from(key) {
            Ok(k) if self == Aead::Chacha => aead_open(k, nonce, aad, ct),
            _ => gcm_open(key, nonce, aad, ct),
        }
    }
}

/// Claves de un sentido: clave, IV y contador de registros.
pub struct Keys {
    aead: Aead,
    pub(super) key: Secret,
    pub(super) iv: [u8; 12],
    pub(super) seq: u64,
}

impl Keys {
    pub fn new(aead: Aead, key: Secret, iv: &[u8]) -> Keys {
        let mut b = [0u8; 12];
        b.copy_from_slice(iv);
        Keys { aead, key, iv: b, seq: 0 }
    }

    /// Nonce del registro actual (IV ⊕ secuencia) y avance del contador; `None` si se agotó.
    fn nonce(&mut self) -> Option<[u8; 12]> {
        if self.seq == u64::MAX {
            return None;
        }
        let mut n = self.iv;
        for (d, s) in n[4..].iter_mut().zip(self.seq.to_be_bytes()) {
            *d ^= s;
        }
        self.seq += 1;
        Some(n)
    }

    /// Cifra `data` como registro TLS 1.3 de tipo `ct` (con `ct` dentro, sin relleno).
    pub fn seal(&mut self, ct: u8, data: &[u8]) -> Option<Vec<u8>> {
        let nonce = self.nonce()?;
        let mut inner = Vec::with_capacity(data.len() + 1);
        inner.extend_from_slice(data);
        inner.push(ct);
        let len = (inner.len() + 16) as u16;
        let aad = [CT_DATOS, 3, 3, (len >> 8) as u8, len as u8];
        let body = self.aead.seal(&self.key, &nonce, &aad, &inner);
        wipe(&mut inner);
        let mut rec = aad.to_vec();
        rec.extend_from_slice(&body);
        Some(rec)
    }

    /// Descifra el cuerpo de un registro (la cabecera es el AAD). Devuelve (tipo interno, contenido);
    /// `None` si la etiqueta no cuadra, se agotó la secuencia o solo había relleno.
    pub fn open(&mut self, header: &[u8], body: &[u8]) -> Option<(u8, Vec<u8>)> {
        let nonce = self.nonce()?;
        let mut pt = self.aead.open(&self.key, &nonce, header, body)?;
        let n = pt.iter().rposition(|&b| b != 0)?;
        let ct = pt[n];
        pt.truncate(n);
        Some((ct, pt))
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        wipe(&mut self.iv);
    }
}
