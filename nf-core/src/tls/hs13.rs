// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Mensajes de saludo de TLS 1.3 del cliente (RFC 8446 §4 y §7): ClientHello, HelloRetryRequest,
//! ServerHello, EncryptedExtensions, Certificate, CertificateVerify, Finished y KeyUpdate.

use super::hs12::SUITES12;
use super::keys::{secretos_aplicacion, secretos_saludo, Hash};
use super::record::{Aead, Keys, CT_CCS, CT_SALUDO};
use super::{ilegal, Client, Fallo, Rd, SessionInfo, St, TlsError, DEC, UNEXP};
use crate::crypto::{ct_eq, sha256, wipe, x25519, X25519_BASE};
use crate::pk::{self, PubKey, SigAlg};
use crate::x509::{comprobar_ocsp, verify_server_emisor, Cert, CertError, Revocacion};
use alloc::vec;
use alloc::vec::Vec;

/// Versiones que ofrece el ClientHello (supported_versions).
const OFRECIDAS: [u16; 2] = [0x0304, 0x0303];
/// Mayor mensaje de saludo, certificados incluidos, y transcript total que se aceptan.
const MAX_SALUDO: usize = 1 << 16;
const MAX_TRANSCRIPT: usize = 1 << 18;
pub(super) const MAX_CERTS: usize = 20;
pub(super) const CCS: [u8; 6] = [CT_CCS, 3, 3, 0, 1, 1];

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

/// signature_algorithms ofrecidos: ECDSA, RSA-PSS y RSA PKCS#1 (este último solo vale en 1.2 y en certificados).
const SIGALGS: [u16; 8] = [0x0403, 0x0503, 0x0804, 0x0805, 0x0806, 0x0401, 0x0501, 0x0601];

pub(super) fn mensaje(t: u8, body: &[u8]) -> Vec<u8> {
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

/// OCSPResponse de un CertificateStatus (`status_type` ocsp = 1 y respuesta de hasta 2^24-1 bytes); vacía
/// si no tiene esa forma (se tratará como respuesta no válida, sin tumbar la conexión).
pub(super) fn respuesta_ocsp(b: &[u8]) -> Vec<u8> {
    let mut r = Rd(b);
    match (r.u8(), r.v24()) {
        (Some(1), Some(o)) if r.0.is_empty() => o.to_vec(),
        _ => Vec::new(),
    }
}

/// ALPN del servidor (RFC 7301): un solo protocolo y tiene que ser el que ofrecimos.
pub(super) fn alpn(b: &[u8]) -> Result<(), Fallo> {
    let mut l = Rd(Rd(b).v16().ok_or(DEC)?);
    let n = l.v8().ok_or(DEC)?;
    if !l.0.is_empty() {
        return Err(DEC);
    }
    if n != b"http/1.1" {
        return Err(ilegal("el servidor eligió un protocolo que no se ofreció"));
    }
    Ok(())
}

/// Nombre del grupo para mostrar.
pub(super) fn nombre_grupo(g: u16) -> &'static str {
    match g {
        0x0017 => "secp256r1",
        0x0018 => "secp384r1",
        _ => "x25519",
    }
}

fn alerta_cert(e: CertError) -> u8 {
    match e {
        CertError::Revocado => 44,
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
        b.extend_from_slice(&(((SUITES.len() + SUITES12.len()) * 2) as u16).to_be_bytes());
        for s in SUITES.iter().chain(&SUITES12) {
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
        ext(&mut e, 0x000a, &[0, 6, 0x00, 0x1d, 0x00, 0x17, 0x00, 0x18]);
        let share: Vec<u8> = match (self.grupo, &self.p_priv) {
            (0x0017, Some(p)) => pk::p256_public(p).map(|k| k.to_vec()).unwrap_or_default(),
            (0x0018, _) => self.q_priv.as_ref().and_then(pk::p384_public).map(|k| k.to_vec()).unwrap_or_default(),
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
        // status_request (RFC 6066 §8): OCSP, sin responder_id ni extensiones.
        ext(&mut e, 0x0005, &[1, 0, 0, 0, 0]);
        // TLS 1.2: formatos de punto (solo sin comprimir), extended_master_secret y renegotiation_info vacía.
        ext(&mut e, 0x000b, &[1, 0]);
        ext(&mut e, 0x0017, &[]);
        ext(&mut e, 0xff01, &[0]);
        if !self.cookie.is_empty() {
            let mut c = (self.cookie.len() as u16).to_be_bytes().to_vec();
            c.extend_from_slice(&self.cookie);
            ext(&mut e, 0x002c, &c);
        }
        b.extend_from_slice(&(e.len() as u16).to_be_bytes());
        b.extend_from_slice(&e);
        mensaje(1, &b)
    }

    pub(super) fn agregar(&mut self, m: &[u8]) -> Result<(), Fallo> {
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

    pub(super) fn suite(&self) -> Result<Suite, Fallo> {
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
                (St::Conectado, 4) if !self.s12.activo => {} // NewSessionTicket: sin reanudación, se ignora
                (St::Conectado, 24) if !self.s12.activo => self.en_key_update(&msg)?,
                (St::Esp12Cert, 11) => {
                    self.agregar(&msg)?;
                    self.en_cert12(&msg)?
                }
                (St::Esp12Ske, 12) => {
                    self.agregar(&msg)?;
                    self.en_ske(&msg)?
                }
                // CertificateStatus (RFC 6066 §8): solo si el servidor anunció status_request, y una vez.
                (St::Esp12Ske, 22) if self.s12.status => {
                    self.agregar(&msg)?;
                    self.s12.status = false;
                    self.stapled = Some(respuesta_ocsp(&msg[4..]));
                    self.revisar_ocsp()?
                }
                (St::Esp12Hd, 13) if self.cert_ctx.is_none() => {
                    self.agregar(&msg)?;
                    self.en_cert_req12(&msg)?
                }
                (St::Esp12Hd, 14) => {
                    self.agregar(&msg)?;
                    self.en_shd(&msg)?
                }
                (St::Esp12Fin, 20) => self.en_fin12(&msg)?,
                // HelloRequest (§7.4.1.1): no renegociamos, se ignora.
                (St::Conectado, 0) if self.s12.activo && msg.len() == 4 => {}
                _ => return Err(UNEXP),
            }
        }
    }

    fn en_server_hello(&mut self, msg: &[u8]) -> Result<(), Fallo> {
        let mut r = Rd(&msg[4..]);
        match r.u16().ok_or(DEC)? {
            0x0303 => {}
            0x0300..=0x0302 => return Err(Fallo(70, TlsError::NoSoportado("el servidor solo admite TLS 1.1 o anterior"))),
            _ => return Err(ilegal("versión heredada incorrecta")),
        }
        let random = r.take(32).ok_or(DEC)?;
        let sid = r.v8().ok_or(DEC)?;
        let suite_id = r.u16().ok_or(DEC)?;
        let comp = r.u8().ok_or(DEC)?;
        let exts = r.v16().ok_or(DEC)?;
        if !r.0.is_empty() {
            return Err(DEC);
        }
        if comp != 0 {
            return Err(ilegal("ServerHello no cuadra con el ClientHello"));
        }
        // §4.1.3: centinelas de degradación (ofrecimos 1.3, así que tampoco vale que se negocie 1.2).
        if &random[24..31] == b"DOWNGRD" && random[31] <= 1 {
            return Err(Fallo(47, TlsError::Protocolo("el servidor indica degradación de versión")));
        }
        let hrr = ct_eq(random, &sha256(b"HelloRetryRequest"));
        let exts = extensiones(exts)?;
        let busca = |t: u16| exts.iter().find(|e| e.0 == t).map(|e| e.1);
        match busca(0x002b) {
            Some(&[3, 4]) => {}
            Some(&[_, _]) => return Err(ilegal("versión elegida no ofrecida")),
            Some(_) => return Err(DEC),
            // Sin supported_versions: el servidor eligió TLS 1.2 (un HelloRetryRequest no puede ser 1.2).
            None if hrr || self.hrr_visto || self.s12.activo => return Err(ilegal("ServerHello sin supported_versions")),
            None => return self.en_sh12(msg, random, sid, suite_id, &exts),
        }
        if sid != &self.sid[..] {
            return Err(ilegal("ServerHello no cuadra con el ClientHello"));
        }
        if exts.iter().any(|e| !matches!(e.0, 0x002b | 0x0033) && !(hrr && e.0 == 0x002c)) {
            return Err(Fallo(110, TlsError::Protocolo("el servidor usa una extensión que no se pidió")));
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
        } else if grupo == 0x0017 {
            let p = self.p_priv.as_ref().ok_or(UNEXP)?;
            pk::p256_ecdh(p, share).ok_or(ilegal("key_share P-256 no válido"))?.to_vec()
        } else {
            let q = self.q_priv.as_ref().ok_or(UNEXP)?;
            pk::p384_ecdh(q, share).ok_or(ilegal("key_share P-384 no válido"))?.to_vec()
        };
        self.suite = Some(suite);
        self.agregar(msg)?;
        let h = suite.hash;
        let (hs, c, s) = secretos_saludo(h, &shared, &h.hash(&self.transcript));
        wipe(&mut shared);
        wipe(&mut self.x_priv);
        self.p_priv = None;
        if let Some(q) = self.q_priv.as_mut() {
            wipe(q);
        }
        self.q_priv = None;
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
        let mut nuevo = 0;
        if let Some(k) = ks {
            // Solo puede pedir un grupo que ofrecimos y que aún no mandamos: P-256 o P-384.
            let [a, b] = *k else { return Err(DEC) };
            nuevo = u16::from_be_bytes([a, b]);
            if !matches!(nuevo, 0x0017 | 0x0018) || nuevo == self.grupo {
                return Err(ilegal("grupo no válido en HelloRetryRequest"));
            }
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
        match nuevo {
            0x0017 => self.nueva_p256()?,
            0x0018 => self.nueva_p384()?,
            _ => {}
        }
        if !self.ccs_enviado {
            self.out.extend_from_slice(&CCS);
            self.ccs_enviado = true;
        }
        let ch = self.armar_hello();
        self.enviar_saludo(&ch)?;
        if self.hsbuf.is_empty() { Ok(()) } else { Err(UNEXP) }
    }

    /// Genera la privada P-256 (y pasa el grupo a secp256r1).
    pub(super) fn nueva_p256(&mut self) -> Result<(), Fallo> {
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
        Ok(())
    }

    /// Genera la privada P-384 (y pasa el grupo a secp384r1).
    pub(super) fn nueva_p384(&mut self) -> Result<(), Fallo> {
        let mut k = [0u8; 48];
        for _ in 0..8 {
            self.rng.fill(&mut k);
            if pk::p384_public(&k).is_some() {
                self.q_priv = Some(k);
                self.grupo = 0x0018;
                break;
            }
        }
        wipe(&mut k);
        if self.q_priv.is_none() {
            return Err(Fallo(80, TlsError::Protocolo("no se pudo generar la clave P-384")));
        }
        Ok(())
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
                0x0010 => alpn(b)?,
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
            let ex = l.v16().ok_or(DEC)?;
            // Solo la primera entrada (la hoja) puede traer status_request (§4.4.2.1); las demás extensiones se ignoran.
            if ders.is_empty() {
                if let Some((_, b)) = extensiones(ex)?.into_iter().find(|e| e.0 == 0x0005) {
                    self.stapled = Some(respuesta_ocsp(b));
                }
            }
            if der.is_empty() {
                return Err(DEC);
            }
            if ders.len() == MAX_CERTS {
                return Err(Fallo(42, TlsError::Cert(CertError::CadenaDemasiadoLarga)));
            }
            ders.push(der);
        }
        if ders.is_empty() {
            return Err(DEC);
        }
        self.cadena(&ders)?;
        self.revisar_ocsp()?;
        self.st = St::EsperaCv;
        Ok(())
    }

    /// Valida la cadena (hoja primero) y guarda la clave de la hoja; con `insecure` solo anota el fallo.
    pub(super) fn cadena(&mut self, ders: &[&[u8]]) -> Result<(), Fallo> {
        let hoja = ders[0];
        match verify_server_emisor(ders, &self.host, self.now, &self.roots, &pk::verify) {
            Ok((ci, emisor)) => {
                self.chain = Some(ci);
                self.leaf_der = hoja.to_vec();
                self.issuer_der = emisor;
            }
            Err(e) if self.insecure => self.cert_error = Some(e),
            Err(e) => return Err(Fallo(alerta_cert(e), TlsError::Cert(e))),
        }
        // Con -k también se comprueba el CertificateVerify, así que hace falta la clave de la hoja.
        let key = Cert::parse(hoja).ok().and_then(|c| c.public_key().cloned());
        self.leaf = Some(key.ok_or(Fallo(43, TlsError::Cert(CertError::NoSoportado)))?);
        Ok(())
    }

    /// Comprueba la respuesta OCSP adjunta (si la hay). Revocado: error, o solo aviso con `-k`; cualquier
    /// otro resultado deja seguir (anotado en `revoc`).
    pub(super) fn revisar_ocsp(&mut self) -> Result<(), Fallo> {
        let Some(r) = self.stapled.take() else { return Ok(()) };
        self.revoc = if self.issuer_der.is_empty() {
            Revocacion::Invalida // sin cadena validada (-k) no hay emisor contra el que comprobarla
        } else {
            comprobar_ocsp(&r, &self.leaf_der, &self.issuer_der, self.now, &pk::verify)
        };
        if self.revoc == Revocacion::Revocada {
            if !self.insecure {
                return Err(Fallo(44, TlsError::Cert(CertError::Revocado)));
            }
            self.cert_error.get_or_insert(CertError::Revocado);
        }
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
            group: nombre_grupo(self.grupo),
            revocation: self.revoc,
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
