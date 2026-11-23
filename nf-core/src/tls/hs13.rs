// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Mensajes de saludo de TLS 1.3 del cliente (RFC 8446 §4 y §7): ClientHello, HelloRetryRequest,
//! ServerHello, EncryptedExtensions, Certificate, CertificateVerify, Finished y KeyUpdate.

use super::keys::{secretos_aplicacion, secretos_saludo, Hash};
use super::record::{Aead, Keys, CT_CCS, CT_SALUDO};
use super::{ilegal, Client, Fallo, Rd, SessionInfo, St, TlsError, DEC, UNEXP};
use crate::crypto::{ct_eq, sha256, wipe, x25519, X25519_BASE};
use crate::pk::{self, PubKey, SigAlg};
use crate::x509::{verify_server, Cert, CertError};
use alloc::vec;
use alloc::vec::Vec;

/// Versiones que ofrece el ClientHello. Único sitio donde añadir 0x0303 cuando llegue TLS 1.2.
const OFRECIDAS: [u16; 1] = [0x0304];
/// Mayor mensaje de saludo, certificados incluidos, y transcript total que se aceptan.
const MAX_SALUDO: usize = 1 << 16;
const MAX_TRANSCRIPT: usize = 1 << 18;
const MAX_CERTS: usize = 20;
const CCS: [u8; 6] = [CT_CCS, 3, 3, 0, 1, 1];

#[derive(Clone, Copy)]
pub struct Suite {
    pub(super) id: u16,
    pub(super) aead: Aead,
    pub(super) hash: Hash,
    pub(super) nombre: &'static str,
}

/// En orden de preferencia: ChaCha20 primero (el AES del proyecto usa tablas).
const SUITES: [Suite; 3] = [
    Suite { id: 0x1303, aead: Aead::Chacha, hash: Hash::S256, nombre: "TLS_CHACHA20_POLY1305_SHA256" },
    Suite { id: 0x1301, aead: Aead::Aes128, hash: Hash::S256, nombre: "TLS_AES_128_GCM_SHA256" },
    Suite { id: 0x1302, aead: Aead::Aes256, hash: Hash::S384, nombre: "TLS_AES_256_GCM_SHA384" },
];

/// signature_algorithms ofrecidos: ECDSA, RSA-PSS y (solo para certificados) RSA PKCS#1.
const SIGALGS: [u16; 8] = [0x0403, 0x0503, 0x0804, 0x0805, 0x0806, 0x0401, 0x0501, 0x0601];

fn mensaje(t: u8, body: &[u8]) -> Vec<u8> {
    let mut m = vec![t, (body.len() >> 16) as u8, (body.len() >> 8) as u8, body.len() as u8];
    m.extend_from_slice(body);
    m
}

fn ext(out: &mut Vec<u8>, ty: u16, body: &[u8]) {
    out.extend_from_slice(&ty.to_be_bytes());
    out.extend_from_slice(&(body.len() as u16).to_be_bytes());
    out.extend_from_slice(body);
}

pub(super) fn claves(h: Hash, a: Aead, secret: &[u8]) -> Keys {
    Keys::new(a, h.expand_label(secret, "key", &[], a.key_len()), &h.expand_label(secret, "iv", &[], 12))
}

/// Extensiones de una lista; una repetida es un error (§4.2).
fn extensiones(data: &[u8]) -> Result<Vec<(u16, &[u8])>, Fallo> {
    let mut r = Rd(data);
    let mut v: Vec<(u16, &[u8])> = Vec::new();
    while !r.0.is_empty() {
        let t = r.u16().ok_or(DEC)?;
        let b = r.v16().ok_or(DEC)?;
        if v.iter().any(|e| e.0 == t) {
            return Err(ilegal("extensión repetida"));
        }
        v.push((t, b));
    }
    Ok(v)
}

fn alerta_cert(e: CertError) -> u8 {
    match e {
        CertError::Caducado | CertError::AunNoValido => 45,
        CertError::RaizDesconocida | CertError::Autofirmado => 48,
        CertError::NoSoportado => 43,
        _ => 42,
    }
}

impl Client {
    pub(super) fn armar_hello(&self) -> Vec<u8> {
        let mut b = vec![3, 3];
        b.extend_from_slice(&self.random);
        b.push(self.sid.len() as u8);
        b.extend_from_slice(&self.sid);
        b.extend_from_slice(&((SUITES.len() * 2) as u16).to_be_bytes());
        for s in SUITES {
            b.extend_from_slice(&s.id.to_be_bytes());
        }
        b.extend_from_slice(&[1, 0]);
        let mut e = Vec::new();
        // SNI (RFC 6066): nunca con una IP literal.
        if !self.ip && !self.host.is_empty() && self.host.len() <= 255 {
            let n = self.host.len() as u16;
            let mut s = ((n + 3) as u16).to_be_bytes().to_vec();
            s.push(0);
            s.extend_from_slice(&n.to_be_bytes());
            s.extend_from_slice(self.host.as_bytes());
            ext(&mut e, 0x0000, &s);
        }
        let mut v = vec![(OFRECIDAS.len() * 2) as u8];
        for x in OFRECIDAS {
            v.extend_from_slice(&x.to_be_bytes());
        }
        ext(&mut e, 0x002b, &v);
        ext(&mut e, 0x000a, &[0, 4, 0x00, 0x1d, 0x00, 0x17]);
        let share: Vec<u8> = match (self.grupo, &self.p_priv) {
            (0x0017, Some(p)) => pk::p256_public(p).map(|k| k.to_vec()).unwrap_or_default(),
            _ => x25519(&self.x_priv, &X25519_BASE).to_vec(),
        };
        let mut k = ((share.len() + 4) as u16).to_be_bytes().to_vec();
        k.extend_from_slice(&self.grupo.to_be_bytes());
        k.extend_from_slice(&(share.len() as u16).to_be_bytes());
        k.extend_from_slice(&share);
        ext(&mut e, 0x0033, &k);
        let mut sa = ((SIGALGS.len() * 2) as u16).to_be_bytes().to_vec();
        for a in SIGALGS {
            sa.extend_from_slice(&a.to_be_bytes());
        }
        ext(&mut e, 0x000d, &sa);
        ext(&mut e, 0x0010, b"\x00\x09\x08http/1.1");
        if !self.cookie.is_empty() {
            let mut c = (self.cookie.len() as u16).to_be_bytes().to_vec();
            c.extend_from_slice(&self.cookie);
            ext(&mut e, 0x002c, &c);
        }
        b.extend_from_slice(&(e.len() as u16).to_be_bytes());
        b.extend_from_slice(&e);
        mensaje(1, &b)
    }

    fn agregar(&mut self, m: &[u8]) -> Result<(), Fallo> {
        if self.transcript.len() + m.len() > MAX_TRANSCRIPT {
            return Err(Fallo(80, TlsError::Protocolo("saludo demasiado largo")));
        }
        self.transcript.extend_from_slice(m);
        Ok(())
    }

    /// Añade al transcript y manda un mensaje de saludo.
    pub(super) fn enviar_saludo(&mut self, m: &[u8]) -> Result<(), Fallo> {
        self.agregar(m)?;
        self.enviar(CT_SALUDO, m)
    }

    fn suite(&self) -> Result<Suite, Fallo> {
        self.suite.ok_or(UNEXP)
    }

    /// Procesa los mensajes completos que haya en `hsbuf`.
    pub(super) fn tratar_saludo(&mut self) -> Result<(), Fallo> {
        loop {
            let Some(cab) = self.hsbuf.get(..4) else { return Ok(()) };
            let n = 4 + Rd(&cab[1..]).u24().ok_or(DEC)?;
            if n - 4 > MAX_SALUDO {
                return Err(DEC);
            }
            if self.hsbuf.len() < n {
                return Ok(());
            }
            let msg: Vec<u8> = self.hsbuf.drain(..n).collect();
            match (self.st, msg[0]) {
                (St::EsperaSh, 2) => self.en_server_hello(&msg)?,
                (St::EsperaEe, 8) => {
                    self.agregar(&msg)?;
                    self.en_ee(&msg)?
                }
                (St::EsperaCert, 13) if self.cert_ctx.is_none() => {
                    self.agregar(&msg)?;
                    self.en_cert_req(&msg)?
                }
                (St::EsperaCert, 11) => {
                    self.agregar(&msg)?;
                    self.en_cert(&msg)?
                }
                (St::EsperaCv, 15) => self.en_cv(&msg)?,
                (St::EsperaFin, 20) => self.en_fin(&msg)?,
                (St::Conectado, 4) => {} // NewSessionTicket: sin reanudación, se ignora
                (St::Conectado, 24) => self.en_key_update(&msg)?,
                _ => return Err(UNEXP),
            }
        }
    }

    fn en_server_hello(&mut self, msg: &[u8]) -> Result<(), Fallo> {
        let mut r = Rd(&msg[4..]);
        if r.u16().ok_or(DEC)? != 0x0303 {
            return Err(ilegal("versión heredada incorrecta"));
        }
        let random = r.take(32).ok_or(DEC)?;
        let sid = r.v8().ok_or(DEC)?;
        let suite_id = r.u16().ok_or(DEC)?;
        let comp = r.u8().ok_or(DEC)?;
        let exts = r.v16().ok_or(DEC)?;
        if !r.0.is_empty() {
            return Err(DEC);
        }
        if comp != 0 || sid != &self.sid[..] {
            return Err(ilegal("ServerHello no cuadra con el ClientHello"));
        }
        // §4.1.3: centinelas de degradación a TLS 1.2 o inferior.
        if &random[24..31] == b"DOWNGRD" && random[31] <= 1 {
            return Err(Fallo(47, TlsError::Protocolo("el servidor indica degradación de versión")));
        }
        let hrr = ct_eq(random, &sha256(b"HelloRetryRequest"));
        let exts = extensiones(exts)?;
        let busca = |t: u16| exts.iter().find(|e| e.0 == t).map(|e| e.1);
        if exts.iter().any(|e| !matches!(e.0, 0x002b | 0x0033) && !(hrr && e.0 == 0x002c)) {
            return Err(Fallo(110, TlsError::Protocolo("el servidor usa una extensión que no se pidió")));
        }
        match busca(0x002b) {
            Some(&[a, b]) if OFRECIDAS.contains(&u16::from_be_bytes([a, b])) && [a, b] == [3, 4] => {}
            Some(&[_, _]) => return Err(ilegal("versión elegida no ofrecida")),
            Some(_) => return Err(DEC),
            None => return Err(Fallo(70, TlsError::NoSoportado("TLS 1.2 llega en el paso 4"))),
        }
        let suite = *SUITES.iter().find(|s| s.id == suite_id).ok_or(ilegal("suite no ofrecida"))?;
        if self.suite.is_some_and(|s| s.id != suite.id) {
            return Err(ilegal("suite distinta a la del HelloRetryRequest"));
        }
        if hrr {
            return self.en_hrr(msg, suite, busca(0x0033), busca(0x002c));
        }
        let ks = busca(0x0033).ok_or(Fallo(109, TlsError::Protocolo("falta key_share")))?;
        let mut kr = Rd(ks);
        let (grupo, share) = (kr.u16().ok_or(DEC)?, kr.v16().ok_or(DEC)?);
        if !kr.0.is_empty() {
            return Err(DEC);
        }
        if grupo != self.grupo {
            return Err(ilegal("grupo distinto del ofrecido en key_share"));
        }
        let mut shared: Vec<u8> = if grupo == 0x001d {
            let peer: [u8; 32] = share.try_into().map_err(|_| ilegal("key_share x25519 de longitud incorrecta"))?;
            let s = x25519(&self.x_priv, &peer);
            if ct_eq(&s, &[0u8; 32]) {
                return Err(ilegal("secreto x25519 nulo"));
            }
            s.to_vec()
        } else {
            let p = self.p_priv.as_ref().ok_or(UNEXP)?;
            pk::p256_ecdh(p, share).ok_or(ilegal("key_share P-256 no válido"))?.to_vec()
        };
        self.suite = Some(suite);
        self.agregar(msg)?;
        let h = suite.hash;
        let (hs, c, s) = secretos_saludo(h, &shared, &h.hash(&self.transcript));
        wipe(&mut shared);
        wipe(&mut self.x_priv);
        self.p_priv = None;
        self.rx = Some(claves(h, suite.aead, &s));
        (self.hs_secret, self.c_hs, self.s_hs) = (hs, c, s);
        self.st = St::EsperaEe;
        // §5.1: un mensaje no puede cruzar un cambio de claves.
        if self.hsbuf.is_empty() { Ok(()) } else { Err(UNEXP) }
    }

    /// HelloRetryRequest (§4.1.4): un solo reintento y tiene que cambiar algo.
    fn en_hrr(&mut self, msg: &[u8], suite: Suite, ks: Option<&[u8]>, cookie: Option<&[u8]>) -> Result<(), Fallo> {
        if self.hrr_visto {
            return Err(UNEXP);
        }
        let mut cambia = false;
        let mut p256 = false;
        if let Some(k) = ks {
            // Solo puede pedir un grupo que ofrecimos y que aún no mandamos: P-256.
            let [a, b] = *k else { return Err(DEC) };
            if u16::from_be_bytes([a, b]) != 0x0017 || self.grupo == 0x0017 {
                return Err(ilegal("grupo no válido en HelloRetryRequest"));
            }
            p256 = true;
            cambia = true;
        }
        if let Some(c) = cookie {
            let mut r = Rd(c);
            let ck = r.v16().ok_or(DEC)?;
            if !r.0.is_empty() || ck.is_empty() {
                return Err(DEC);
            }
            self.cookie = ck.to_vec();
            cambia = true;
        }
        if !cambia {
            return Err(ilegal("HelloRetryRequest sin ningún cambio"));
        }
        // §4.4.1: el ClientHello1 pasa a ser un message_hash.
        let hh = suite.hash.hash(&self.transcript);
        self.transcript = mensaje(254, &hh);
        self.suite = Some(suite);
        self.hrr_visto = true;
        self.agregar(msg)?;
        if p256 {
            let mut k = [0u8; 32];
            // Una privada fuera de [1, n-1] es casi imposible; unos pocos intentos bastan.
            for _ in 0..8 {
                self.rng.fill(&mut k);
                if pk::p256_public(&k).is_some() {
                    self.p_priv = Some(k);
                    self.grupo = 0x0017;
                    break;
                }
            }
            wipe(&mut k);
            if self.p_priv.is_none() {
                return Err(Fallo(80, TlsError::Protocolo("no se pudo generar la clave P-256")));
            }
        }
        if !self.ccs_enviado {
            self.out.extend_from_slice(&CCS);
            self.ccs_enviado = true;
        }
        let ch = self.armar_hello();
        self.enviar_saludo(&ch)?;
        if self.hsbuf.is_empty() { Ok(()) } else { Err(UNEXP) }
    }

    fn en_ee(&mut self, msg: &[u8]) -> Result<(), Fallo> {
        let mut r = Rd(&msg[4..]);
        let e = r.v16().ok_or(DEC)?;
        if !r.0.is_empty() {
            return Err(DEC);
        }
        for (t, b) in extensiones(e)? {
            match t {
                0x0000 | 0x000a => {}
                0x0010 => {
                    // ALPN (RFC 7301): un solo protocolo y tiene que ser el que ofrecimos.
                    let mut l = Rd(Rd(b).v16().ok_or(DEC)?);
                    let n = l.v8().ok_or(DEC)?;
                    if !l.0.is_empty() {
                        return Err(DEC);
                    }
                    if n != b"http/1.1" {
                        return Err(ilegal("el servidor eligió un protocolo que no se ofreció"));
                    }
                }
                _ => return Err(Fallo(110, TlsError::Protocolo("el servidor usa una extensión que no se pidió"))),
            }
        }
        self.st = St::EsperaCert;
        Ok(())
    }

    fn en_cert_req(&mut self, msg: &[u8]) -> Result<(), Fallo> {
        let mut r = Rd(&msg[4..]);
        let ctx = r.v8().ok_or(DEC)?;
        let e = r.v16().ok_or(DEC)?;
        if !r.0.is_empty() {
            return Err(DEC);
        }
        extensiones(e)?;
        self.cert_ctx = Some(ctx.to_vec()); // se contestará con un Certificate vacío
        Ok(())
    }

    fn en_cert(&mut self, msg: &[u8]) -> Result<(), Fallo> {
        let mut r = Rd(&msg[4..]);
        if !r.v8().ok_or(DEC)?.is_empty() {
            return Err(ilegal("contexto de certificado no vacío"));
        }
        let lista = r.v24().ok_or(DEC)?;
        if !r.0.is_empty() {
            return Err(DEC);
        }
        let mut l = Rd(lista);
        let mut ders: Vec<&[u8]> = Vec::new();
        while !l.0.is_empty() {
            let der = l.v24().ok_or(DEC)?;
            l.v16().ok_or(DEC)?; // extensiones de la entrada (OCSP, SCT…): se ignoran
            if der.is_empty() {
                return Err(DEC);
            }
            if ders.len() == MAX_CERTS {
                return Err(Fallo(42, TlsError::Cert(CertError::CadenaDemasiadoLarga)));
            }
            ders.push(der);
        }
        let Some(&hoja) = ders.first() else { return Err(DEC) };
        match verify_server(&ders, &self.host, self.now, &self.roots, &pk::verify) {
            Ok(ci) => self.chain = Some(ci),
            Err(e) if self.insecure => self.cert_error = Some(e),
            Err(e) => return Err(Fallo(alerta_cert(e), TlsError::Cert(e))),
        }
        // Con -k también se comprueba el CertificateVerify, así que hace falta la clave de la hoja.
        let key = Cert::parse(hoja).ok().and_then(|c| c.public_key().cloned());
        self.leaf = Some(key.ok_or(Fallo(43, TlsError::Cert(CertError::NoSoportado)))?);
        self.st = St::EsperaCv;
        Ok(())
    }

    fn en_cv(&mut self, msg: &[u8]) -> Result<(), Fallo> {
        let mut r = Rd(&msg[4..]);
        let alg = r.u16().ok_or(DEC)?;
        let sig = r.v16().ok_or(DEC)?;
        if !r.0.is_empty() {
            return Err(DEC);
        }
        let key = self.leaf.as_ref().ok_or(UNEXP)?;
        // Solo ECDSA con la curva que toca y RSA-PSS: nunca PKCS#1 (§4.2.3).
        let (sa, casa) = match alg {
            0x0403 => (SigAlg::EcdsaSha256, matches!(key, PubKey::EcP256(_))),
            0x0503 => (SigAlg::EcdsaSha384, matches!(key, PubKey::EcP384(_))),
            0x0804 => (SigAlg::RsaPssSha256, matches!(key, PubKey::Rsa { .. })),
            0x0805 => (SigAlg::RsaPssSha384, matches!(key, PubKey::Rsa { .. })),
            0x0806 => (SigAlg::RsaPssSha512, matches!(key, PubKey::Rsa { .. })),
            _ => return Err(ilegal("algoritmo de firma no permitido en CertificateVerify")),
        };
        if !casa {
            return Err(ilegal("el algoritmo de firma no casa con la clave del certificado"));
        }
        let mut c = vec![0x20u8; 64];
        c.extend_from_slice(b"TLS 1.3, server CertificateVerify\0");
        c.extend_from_slice(&self.suite()?.hash.hash(&self.transcript));
        if !pk::verify(key, sa, &c, sig) {
            return Err(Fallo(51, TlsError::Protocolo("la firma del servidor (CertificateVerify) no es válida")));
        }
        self.agregar(msg)?;
        self.st = St::EsperaFin;
        Ok(())
    }

    fn en_fin(&mut self, msg: &[u8]) -> Result<(), Fallo> {
        let s = self.suite()?;
        let h = s.hash;
        if msg.len() - 4 != h.len() {
            return Err(DEC);
        }
        let esperado = h.finished(&self.s_hs, &h.hash(&self.transcript));
        if !ct_eq(&esperado, &msg[4..]) {
            return Err(Fallo(51, TlsError::Protocolo("el Finished del servidor no es válido")));
        }
        self.agregar(msg)?;
        if !self.hsbuf.is_empty() {
            return Err(UNEXP);
        }
        let (c_ap, s_ap) = secretos_aplicacion(h, &self.hs_secret, &h.hash(&self.transcript));
        // Segundo vuelo del cliente, con las claves del saludo.
        if !self.ccs_enviado {
            self.out.extend_from_slice(&CCS);
            self.ccs_enviado = true;
        }
        self.tx = Some(claves(h, s.aead, &self.c_hs));
        if let Some(ctx) = self.cert_ctx.take() {
            // No tenemos certificado de cliente: Certificate vacío (§4.4.2).
            let mut b = vec![ctx.len() as u8];
            b.extend_from_slice(&ctx);
            b.extend_from_slice(&[0, 0, 0]);
            self.enviar_saludo(&mensaje(11, &b))?;
        }
        let fin = h.finished(&self.c_hs, &h.hash(&self.transcript));
        self.enviar_saludo(&mensaje(20, &fin))?;
        self.tx = Some(claves(h, s.aead, &c_ap));
        self.rx = Some(claves(h, s.aead, &s_ap));
        (self.rx_secret, self.tx_secret) = (s_ap, c_ap);
        (self.hs_secret, self.c_hs, self.s_hs) = (super::Secret::vacio(), super::Secret::vacio(), super::Secret::vacio());
        self.transcript = Vec::new();
        self.leaf = None;
        self.info = Some(SessionInfo {
            version: "TLS 1.3",
            suite: s.nombre,
            chain: self.chain.take(),
            cert_error: self.cert_error,
            group: if self.grupo == 0x0017 { "secp256r1" } else { "x25519" },
        });
        self.listo = true;
        self.st = St::Conectado;
        Ok(())
    }

    /// KeyUpdate (§4.6.3): renueva las claves de recepción y, si lo piden, las de envío.
    fn en_key_update(&mut self, msg: &[u8]) -> Result<(), Fallo> {
        let pide = match &msg[4..] {
            [0] => false,
            [1] => true,
            [_] => return Err(ilegal("KeyUpdate con valor no válido")),
            _ => return Err(DEC),
        };
        if !self.hsbuf.is_empty() {
            return Err(UNEXP);
        }
        let s = self.suite()?;
        let h = s.hash;
        let n = h.expand_label(&self.rx_secret, "traffic upd", &[], h.len());
        self.rx = Some(claves(h, s.aead, &n));
        self.rx_secret = n;
        if pide {
            self.enviar(CT_SALUDO, &mensaje(24, &[0]))?;
            let n = h.expand_label(&self.tx_secret, "traffic upd", &[], h.len());
            self.tx = Some(claves(h, s.aead, &n));
            self.tx_secret = n;
        }
        Ok(())
    }
}
