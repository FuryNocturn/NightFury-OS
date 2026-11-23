// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Hash de la suite y derivación de claves de TLS 1.3 (RFC 8446 §7.1, HKDF de RFC 5869).

use crate::crypto::{hkdf384_expand, hkdf384_extract, hkdf_expand, hkdf_extract, hmac, hmac384, sha256, sha384, wipe};
use alloc::vec;
use alloc::vec::Vec;
use core::ops::Deref;

/// Bytes secretos que se borran al soltarse.
pub struct Secret(pub Vec<u8>);

impl Secret {
    pub fn vacio() -> Secret {
        Secret(Vec::new())
    }
}

impl Deref for Secret {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hash {
    S256,
    S384,
}

impl Hash {
    pub fn len(self) -> usize {
        if self == Hash::S256 { 32 } else { 48 }
    }

    pub fn hash(self, d: &[u8]) -> Vec<u8> {
        if self == Hash::S256 { sha256(d).to_vec() } else { sha384(d).to_vec() }
    }

    pub fn hmac(self, key: &[u8], d: &[u8]) -> Secret {
        Secret(if self == Hash::S256 { hmac(key, d).to_vec() } else { hmac384(key, d).to_vec() })
    }

    /// HKDF-Extract; una sal vacía equivale a ceros.
    pub fn extract(self, salt: &[u8], ikm: &[u8]) -> Secret {
        Secret(if self == Hash::S256 { hkdf_extract(salt, ikm).to_vec() } else { hkdf384_extract(salt, ikm).to_vec() })
    }

    /// HKDF-Expand; `prk` mide `len()` (siempre sale de `extract`/`expand_label`).
    fn expand(self, prk: &[u8], info: &[u8], n: usize) -> Secret {
        let mut o = vec![0u8; n];
        match self {
            Hash::S256 => hkdf_expand(prk.try_into().expect("prk de 32"), info, &mut o),
            Hash::S384 => hkdf384_expand(prk.try_into().expect("prk de 48"), info, &mut o),
        }
        Secret(o)
    }

    /// HKDF-Expand-Label (§7.1) con la etiqueta sin el prefijo «tls13 ».
    pub fn expand_label(self, secret: &[u8], label: &str, ctx: &[u8], n: usize) -> Secret {
        let mut info = Vec::with_capacity(10 + label.len() + ctx.len());
        info.extend_from_slice(&(n as u16).to_be_bytes());
        info.push((6 + label.len()) as u8);
        info.extend_from_slice(b"tls13 ");
        info.extend_from_slice(label.as_bytes());
        info.push(ctx.len() as u8);
        info.extend_from_slice(ctx);
        self.expand(secret, &info, n)
    }

    /// Derive-Secret: la etiqueta con el hash del transcript como contexto.
    pub fn derive(self, secret: &[u8], label: &str, transcript_hash: &[u8]) -> Secret {
        self.expand_label(secret, label, transcript_hash, self.len())
    }

    /// verify_data del Finished (§4.4.4).
    pub fn finished(self, base: &[u8], transcript_hash: &[u8]) -> Secret {
        let k = self.expand_label(base, "finished", &[], self.len());
        self.hmac(&k, transcript_hash)
    }
}

/// Handshake secret y secretos de tráfico del saludo (cliente, servidor) a partir del ECDHE y del
/// hash del transcript ClientHello..ServerHello.
pub fn secretos_saludo(h: Hash, shared: &[u8], th_sh: &[u8]) -> (Secret, Secret, Secret) {
    let early = h.extract(&[], &vec![0u8; h.len()]);
    let derived = h.derive(&early, "derived", &h.hash(&[]));
    let hs = h.extract(&derived, shared);
    let c = h.derive(&hs, "c hs traffic", th_sh);
    let s = h.derive(&hs, "s hs traffic", th_sh);
    (hs, c, s)
}

/// Secretos de tráfico de la aplicación (cliente, servidor); `th_fin` = hash ClientHello..Finished del servidor.
pub fn secretos_aplicacion(h: Hash, hs: &[u8], th_fin: &[u8]) -> (Secret, Secret) {
    let derived = h.derive(hs, "derived", &h.hash(&[]));
    let master = h.extract(&derived, &vec![0u8; h.len()]);
    (h.derive(&master, "c ap traffic", th_fin), h.derive(&master, "s ap traffic", th_fin))
}
