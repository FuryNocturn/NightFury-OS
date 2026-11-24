// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Saludo de TLS 1.2 del cliente (RFC 5246, 5289, 7905, 8422, 7627 y 5746): solo ECDHE (x25519,
//! secp256r1 y secp384r1) con AES-GCM o ChaCha20-Poly1305. Sin CBC, sin RSA como intercambio de claves, sin
//! reanudación ni renegociación (por eso, si el servidor no admite extended_master_secret, se sigue
//! sin él: sus ataques necesitan reanudar o renegociar).

use super::hs13::{alpn, mensaje, nombre_grupo, Suite, CCS};
use super::keys::{Hash, Secret};
use super::record::{Aead, Keys};
use super::{ilegal, Client, Fallo, Rd, SessionInfo, St, TlsError, DEC, UNEXP};
use crate::crypto::{ct_eq, wipe, x25519, X25519_BASE};
use crate::pk::{self, PubKey, SigAlg};
use alloc::vec;
use alloc::vec::Vec;

/// Suites de TLS 1.2 que se ofrecen, tras las de 1.3. El hash es el del PRF y del Finished.
pub(super) const SUITES12: [Suite; 6] = [
    Suite { id: 0xcca9, aead: Aead::Chacha, hash: Hash::S256, nombre: "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256" },
    Suite { id: 0xcca8, aead: Aead::Chacha, hash: Hash::S256, nombre: "TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256" },
    Suite { id: 0xc02b, aead: Aead::Aes128, hash: Hash::S256, nombre: "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256" },
    Suite { id: 0xc02f, aead: Aead::Aes128, hash: Hash::S256, nombre: "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256" },
    Suite { id: 0xc02c, aead: Aead::Aes256, hash: Hash::S384, nombre: "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384" },
    Suite { id: 0xc030, aead: Aead::Aes256, hash: Hash::S384, nombre: "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384" },
];

/// Estado propio de TLS 1.2.
pub(super) struct S12 {
    pub activo: bool,
    /// El servidor aceptó extended_master_secret.
    pub ems: bool,
    /// El servidor anunció status_request: puede llegar un CertificateStatus tras el Certificate.
    pub status: bool,
    pub srv_random: [u8; 32],
    /// Curva y punto del ServerKeyExchange, ya verificado.
    ske: Option<(u16, Vec<u8>)>,
    /// Claves de recepción: se activan con el ChangeCipherSpec del servidor.
    pub rx_pend: Option<Keys>,
    /// verify_data que debe traer el Finished del servidor.
    esperado: Secret,
}

impl S12 {
    pub fn nuevo() -> S12 {
        S12 { activo: false, ems: false, status: false, srv_random: [0; 32], ske: None, rx_pend: None, esperado: Secret::vacio() }
    }
}

/// PRF de TLS 1.2 (§5): P_hash(secreto, etiqueta ‖ semilla), `n` bytes.
pub(super) fn prf(h: Hash, secret: &[u8], label: &str, seed: &[u8], n: usize) -> Secret {
    let mut ls = label.as_bytes().to_vec();
    ls.extend_from_slice(seed);
    let mut a = h.hmac(secret, &ls);
    let mut out = Vec::with_capacity(n + h.len());
    while out.len() < n {
        let mut d = a.0.clone();
        d.extend_from_slice(&ls);
        out.extend_from_slice(&h.hmac(secret, &d));
        wipe(&mut d);
        a = h.hmac(secret, &a);
    }
    if let Some(t) = out.get_mut(n..) {
        wipe(t);
    }
    out.truncate(n);
    Secret(out)
}

fn suite_ecdsa(id: u16) -> bool {
    matches!(id, 0xcca9 | 0xc02b | 0xc02c)
}

/// Algoritmo de firma del ServerKeyExchange: entre los ofrecidos y compatible con la clave (en 1.2 ECDSA
/// vale con cualquier curva y cualquiera de los dos hashes).
fn sigalg(alg: u16, key: &PubKey) -> Option<SigAlg> {
    let ec = matches!(key, PubKey::EcP256(_) | PubKey::EcP384(_));
    let rsa = matches!(key, PubKey::Rsa { .. });
    Some(match alg {
        0x0403 if ec => SigAlg::EcdsaSha256,
        0x0503 if ec => SigAlg::EcdsaSha384,
        0x0401 if rsa => SigAlg::RsaPkcs1Sha256,
        0x0501 if rsa => SigAlg::RsaPkcs1Sha384,
        0x0601 if rsa => SigAlg::RsaPkcs1Sha512,
        0x0804 if rsa => SigAlg::RsaPssSha256,
        0x0805 if rsa => SigAlg::RsaPssSha384,
        0x0806 if rsa => SigAlg::RsaPssSha512,
        _ => return None,
    })
}

impl Client {
    /// ServerHello de TLS 1.2 (llegó sin `supported_versions`).
    pub(super) fn en_sh12(&mut self, msg: &[u8], random: &[u8], sid: &[u8], suite_id: u16, exts: &[(u16, &[u8])]) -> Result<(), Fallo> {
        if sid.len() > 32 {
            return Err(DEC);
        }
        let suite = *SUITES12.iter().find(|s| s.id == suite_id).ok_or(ilegal("suite no ofrecida"))?;
        let (mut ems, mut status) = (false, false);
        for &(t, b) in exts {
            match t {
                0x0000 => {}
                0x0005 if b.is_empty() => status = true,
                0x0005 => return Err(DEC),
                0x000b => {
                    let mut r = Rd(b);
                    let l = r.v8().ok_or(DEC)?;
                    if !r.0.is_empty() || !l.contains(&0) {
                        return Err(ilegal("el servidor no admite el formato de punto sin comprimir"));
                    }
                }
                0x0017 if b.is_empty() => ems = true,
                0x0017 => return Err(DEC),
                // renegotiation_info (RFC 5746): en un saludo inicial tiene que ir vacía. Si falta, se sigue:
                // no renegociamos nunca.
                0xff01 if b == [0] => {}
                0xff01 => return Err(Fallo(40, TlsError::Protocolo("renegociación del servidor no válida"))),
                0x0010 => alpn(b)?,
                _ => return Err(Fallo(110, TlsError::Protocolo("el servidor usa una extensión que no se pidió"))),
            }
        }
        self.agregar(msg)?;
        self.suite = Some(suite);
        self.s12.activo = true;
        self.s12.ems = ems;
        self.s12.status = status;
        self.s12.srv_random.copy_from_slice(random);
        self.st = St::Esp12Cert;
        Ok(())
    }

    pub(super) fn en_cert12(&mut self, msg: &[u8]) -> Result<(), Fallo> {
        let mut r = Rd(&msg[4..]);
        let lista = r.v24().ok_or(DEC)?;
        if !r.0.is_empty() {
            return Err(DEC);
        }
        let mut l = Rd(lista);
        let mut ders: Vec<&[u8]> = Vec::new();
        while !l.0.is_empty() {
            let der = l.v24().ok_or(DEC)?;
            if der.is_empty() {
                return Err(DEC);
            }
            if ders.len() == super::hs13::MAX_CERTS {
                return Err(Fallo(42, TlsError::Cert(crate::x509::CertError::CadenaDemasiadoLarga)));
            }
            ders.push(der);
        }
        if ders.is_empty() {
            return Err(DEC);
        }
        self.cadena(&ders)?;
        // La suite fija el tipo de clave del servidor (ECDSA o RSA).
        let ec = matches!(self.leaf, Some(PubKey::EcP256(_) | PubKey::EcP384(_)));
        if ec != suite_ecdsa(self.suite()?.id) {
            return Err(ilegal("la clave del certificado no corresponde a la suite elegida"));
        }
        self.st = St::Esp12Ske;
        Ok(())
    }

    pub(super) fn en_ske(&mut self, msg: &[u8]) -> Result<(), Fallo> {
        let mut r = Rd(&msg[4..]);
        if r.u8().ok_or(DEC)? != 3 {
            return Err(ilegal("el servidor no usa una curva con nombre"));
        }
        let curva = r.u16().ok_or(DEC)?;
        let punto = r.v8().ok_or(DEC)?;
        let params = &msg[4..4 + 4 + punto.len()];
        let alg = r.u16().ok_or(DEC)?;
        let sig = r.v16().ok_or(DEC)?;
        if !r.0.is_empty() {
            return Err(DEC);
        }
        match (curva, punto.len()) {
            (0x001d, 32) => {}
            (0x0017, 65) | (0x0018, 97) if punto[0] == 4 => {}
            (0x001d | 0x0017 | 0x0018, _) => return Err(ilegal("punto de curva de longitud incorrecta")),
            _ => return Err(ilegal("curva no ofrecida")),
        }
        let key = self.leaf.as_ref().ok_or(UNEXP)?;
        let sa = sigalg(alg, key).ok_or(ilegal("algoritmo de firma no ofrecido o que no casa con el certificado"))?;
        let mut firmado = self.random.to_vec();
        firmado.extend_from_slice(&self.s12.srv_random);
        firmado.extend_from_slice(params);
        if !pk::verify(key, sa, &firmado, sig) {
            return Err(Fallo(51, TlsError::Protocolo("la firma del servidor (ServerKeyExchange) no es válida")));
        }
        self.s12.ske = Some((curva, punto.to_vec()));
        self.st = St::Esp12Hd;
        Ok(())
    }

    /// CertificateRequest (§7.4.4): no tenemos certificado de cliente, se contestará con uno vacío.
    pub(super) fn en_cert_req12(&mut self, msg: &[u8]) -> Result<(), Fallo> {
        let mut r = Rd(&msg[4..]);
        r.v8().ok_or(DEC)?;
        let a = r.v16().ok_or(DEC)?;
        r.v16().ok_or(DEC)?;
        if !r.0.is_empty() || a.len() % 2 != 0 {
            return Err(DEC);
        }
        self.cert_ctx = Some(Vec::new());
        Ok(())
    }

    /// ServerHelloDone: segundo vuelo del cliente (Certificate vacío, ClientKeyExchange, CCS, Finished).
    pub(super) fn en_shd(&mut self, msg: &[u8]) -> Result<(), Fallo> {
        if msg.len() != 4 {
            return Err(DEC);
        }
        if !self.hsbuf.is_empty() {
            return Err(UNEXP);
        }
        let s = self.suite()?;
        let h = s.hash;
        let (curva, punto) = self.s12.ske.take().ok_or(UNEXP)?;
        if self.cert_ctx.take().is_some() {
            self.enviar_saludo(&mensaje(11, &[0, 0, 0]))?;
        }
        // Compartido y nuestra clave pública; en P-256 la privada es nueva (la de 1.3 solo era x25519).
        let (mut pms, publica): (Vec<u8>, Vec<u8>) = if curva == 0x001d {
            let peer: [u8; 32] = punto.as_slice().try_into().map_err(|_| ilegal("punto x25519 de longitud incorrecta"))?;
            (x25519(&self.x_priv, &peer).to_vec(), x25519(&self.x_priv, &X25519_BASE).to_vec())
        } else if curva == 0x0017 {
            self.nueva_p256()?;
            let p = self.p_priv.as_ref().ok_or(UNEXP)?;
            let c = pk::p256_ecdh(p, &punto).ok_or(ilegal("punto P-256 no válido"))?;
            (c.to_vec(), pk::p256_public(p).ok_or(UNEXP)?.to_vec())
        } else {
            self.nueva_p384()?;
            let q = self.q_priv.as_ref().ok_or(UNEXP)?;
            let c = pk::p384_ecdh(q, &punto).ok_or(ilegal("punto P-384 no válido"))?;
            (c.to_vec(), pk::p384_public(q).ok_or(UNEXP)?.to_vec())
        };
        self.grupo = curva;
        if ct_eq(&pms, &vec![0u8; pms.len()]) {
            wipe(&mut pms);
            return Err(ilegal("secreto compartido nulo"));
        }
        let mut cke = vec![publica.len() as u8];
        cke.extend_from_slice(&publica);
        self.enviar_saludo(&mensaje(16, &cke))?;
        let mut seed = self.random.to_vec();
        seed.extend_from_slice(&self.s12.srv_random);
        let ms = if self.s12.ems {
            prf(h, &pms, "extended master secret", &h.hash(&self.transcript), 48)
        } else {
            prf(h, &pms, "master secret", &seed, 48)
        };
        wipe(&mut pms);
        wipe(&mut self.x_priv);
        if let Some(p) = self.p_priv.as_mut() {
            wipe(p);
        }
        self.p_priv = None;
        if let Some(q) = self.q_priv.as_mut() {
            wipe(q);
        }
        self.q_priv = None;
        // key_block (§6.3): claves de cliente y servidor, luego los IV (4 bytes implícitos en GCM, 12 en ChaCha20).
        let mut seed = self.s12.srv_random.to_vec();
        seed.extend_from_slice(&self.random);
        let (kl, il) = (s.aead.key_len(), if s.aead == Aead::Chacha { 12 } else { 4 });
        let kb = prf(h, &ms, "key expansion", &seed, 2 * (kl + il));
        let (ck, sk) = (Secret(kb[..kl].to_vec()), Secret(kb[kl..2 * kl].to_vec()));
        self.s12.rx_pend = Some(Keys::new12(s.aead, sk, &kb[2 * kl + il..]));
        self.out.extend_from_slice(&CCS);
        self.tx = Some(Keys::new12(s.aead, ck, &kb[2 * kl..2 * kl + il]));
        let vd = prf(h, &ms, "client finished", &h.hash(&self.transcript), 12);
        self.enviar_saludo(&mensaje(20, &vd))?;
        self.s12.esperado = prf(h, &ms, "server finished", &h.hash(&self.transcript), 12);
        self.st = St::Esp12Ccs;
        Ok(())
    }

    /// Finished del servidor: cierra el saludo.
    pub(super) fn en_fin12(&mut self, msg: &[u8]) -> Result<(), Fallo> {
        if msg.len() != 16 {
            return Err(DEC);
        }
        if !ct_eq(&self.s12.esperado, &msg[4..]) {
            return Err(Fallo(51, TlsError::Protocolo("el Finished del servidor no es válido")));
        }
        if !self.hsbuf.is_empty() {
            return Err(UNEXP);
        }
        let s = self.suite()?;
        self.s12.esperado = Secret::vacio();
        self.transcript = Vec::new();
        self.leaf = None;
        self.info = Some(SessionInfo {
            version: "TLS 1.2",
            suite: s.nombre,
            chain: self.chain.take(),
            cert_error: self.cert_error,
            group: nombre_grupo(self.grupo),
            revocation: self.revoc,
        });
        self.listo = true;
        self.st = St::Conectado;
        Ok(())
    }

    /// Registro recibido en una conexión TLS 1.2 (el tipo va en la cabecera, también cifrado).
    pub(super) fn en_registro12(&mut self, ct: u8, header: &[u8], body: &[u8]) -> Result<(), Fallo> {
        use super::record::{CT_ALERTA, CT_CCS, CT_DATOS, CT_SALUDO};
        if ct == CT_CCS {
            if body != [1] || self.st != St::Esp12Ccs || !self.hsbuf.is_empty() {
                return Err(UNEXP);
            }
            self.rx = self.s12.rx_pend.take();
            self.st = St::Esp12Fin;
            return Ok(());
        }
        let data = match &mut self.rx {
            None => body.to_vec(),
            Some(rx) => {
                let (_, d) = rx.open(header, body).ok_or(Fallo(20, TlsError::Protocolo("registro corrupto")))?;
                if d.len() > super::record::MAX_CLARO {
                    return Err(Fallo(22, TlsError::Protocolo("registro demasiado grande")));
                }
                d
            }
        };
        match ct {
            CT_SALUDO => {
                if data.is_empty() {
                    return Err(DEC);
                }
                self.hsbuf.extend_from_slice(&data);
                self.tratar_saludo()
            }
            CT_ALERTA => self.en_alerta(&data),
            CT_DATOS if self.listo => {
                self.app_in.extend_from_slice(&data);
                Ok(())
            }
            _ => Err(UNEXP),
        }
    }
}
