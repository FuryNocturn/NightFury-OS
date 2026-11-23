// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Capa de registros de TLS (RFC 8446 §5): tipos, límites y cifrado AEAD con número de secuencia.
//! TLS 1.2 (RFC 5246/5288/7905) usa los mismos tipos y límites; cambian el nonce, los datos adicionales y
//! que el tipo va en la cabecera (modo `v12`).

use super::keys::Secret;
use crate::cipher::{gcm_open, gcm_seal};
use crate::crypto::{aead_open, aead_seal, wipe};
use alloc::vec;
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
    /// Registros de TLS 1.2: tipo en claro, AAD = secuencia ‖ cabecera; en GCM `iv[..4]` es el nonce implícito.
    v12: bool,
}

impl Keys {
    pub fn new(aead: Aead, key: Secret, iv: &[u8]) -> Keys {
        let mut b = [0u8; 12];
        b.copy_from_slice(iv);
        Keys { aead, key, iv: b, seq: 0, v12: false }
    }

    /// Claves de un sentido de TLS 1.2: `iv` mide 4 (GCM) o 12 bytes (ChaCha20).
    pub fn new12(aead: Aead, key: Secret, iv: &[u8]) -> Keys {
        let mut k = Keys::new(aead, key, &[0; 12]);
        let n = iv.len().min(12);
        k.iv[..n].copy_from_slice(&iv[..n]);
        k.v12 = true;
        k
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
        if self.v12 {
            return self.seal12(ct, data);
        }
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
        if self.v12 {
            return self.open12(header, body);
        }
        let nonce = self.nonce()?;
        let mut pt = self.aead.open(&self.key, &nonce, header, body)?;
        let n = pt.iter().rposition(|&b| b != 0)?;
        let ct = pt[n];
        pt.truncate(n);
        Some((ct, pt))
    }

    /// Nonce y número de secuencia de un registro 1.2 (y avance); `None` si se agotó.
    fn nonce12(&mut self, explicito: &[u8]) -> Option<([u8; 12], u64)> {
        let seq = self.seq;
        let n = if self.aead == Aead::Chacha {
            self.nonce()?
        } else {
            if seq == u64::MAX {
                return None;
            }
            self.seq += 1;
            let mut n = [0u8; 12];
            n[..4].copy_from_slice(&self.iv[..4]);
            n[4..].copy_from_slice(explicito);
            n
        };
        Some((n, seq))
    }

    fn aad12(seq: u64, ct: u8, n: usize) -> Vec<u8> {
        let mut a = seq.to_be_bytes().to_vec();
        a.extend_from_slice(&[ct, 3, 3, (n >> 8) as u8, n as u8]);
        a
    }

    fn seal12(&mut self, ct: u8, data: &[u8]) -> Option<Vec<u8>> {
        let gcm = self.aead != Aead::Chacha;
        let (nonce, seq) = self.nonce12(&self.seq.to_be_bytes())?;
        let body = self.aead.seal(&self.key, &nonce, &Keys::aad12(seq, ct, data.len()), data);
        let n = body.len() + if gcm { 8 } else { 0 };
        let mut rec = vec![ct, 3, 3, (n >> 8) as u8, n as u8];
        if gcm {
            rec.extend_from_slice(&seq.to_be_bytes()); // nonce explícito = secuencia
        }
        rec.extend_from_slice(&body);
        Some(rec)
    }

    /// Registro 1.2: devuelve (tipo de la cabecera, contenido).
    fn open12(&mut self, header: &[u8], body: &[u8]) -> Option<(u8, Vec<u8>)> {
        let (expl, ct) = if self.aead == Aead::Chacha { (&[][..], body) } else { body.split_at_checked(8)? };
        let claro = ct.len().checked_sub(16)?;
        let (nonce, seq) = self.nonce12(expl)?;
        let pt = self.aead.open(&self.key, &nonce, &Keys::aad12(seq, *header.first()?, claro), ct)?;
        Some((header[0], pt))
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        wipe(&mut self.iv);
    }
}
