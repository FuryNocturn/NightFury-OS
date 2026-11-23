// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! v0.12.2: certificados X.509 (RFC 5280) y validación de la cadena de un servidor TLS.
//!
//! Escrito desde las normas: X.690 (DER), RFC 5280 (perfil), RFC 6125 (nombres), RFC 4055 (PSS),
//! RFC 5480 (EC) y RFC 7468 (PEM). Sin dependencias; la verificación de firmas la hace quien llama
//! (`verify_server` recibe la función, normalmente `pk::verify`).
//!
//! Qué se comprueba: firma de cada eslabón, fechas de todos (raíz incluida), CA con
//! basicConstraints cA y pathLen, keyUsage keyCertSign, EKU serverAuth, nameConstraints (solo
//! dNSName; otro tipo rechaza la cadena) y el nombre contra los SAN (nunca el CN).
//! Qué no: revocación (CRL/OCSP), políticas de certificado, nombres de intermedios.
//!
//! # Paquete de raíces
//! `raices.bin` es una secuencia de `[u16 longitud big endian][certificado DER]`, sin cabecera ni
//! relleno. Lo genera `tools/raices.ps1` desde el `certdata.txt` de Mozilla.

mod der;
mod pem;
#[cfg(test)]
mod tests;

use crate::pk::{PubKey, SigAlg};

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use der::{bool_val, int_bytes, uint_bytes, DerError, Reader, Tlv, MAX_DER};

pub use pem::pem_or_der;

/// Eslabones máximos del camino (hoja + intermedios + raíz).
const MAX_PATH: usize = 6;
/// Certificados que se leen de lo que manda el servidor.
const MAX_SENT: usize = 16;
/// Caminos completos que se comprueban como máximo antes de rendirse.
const MAX_TRIES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertError {
    Caducado,
    AunNoValido,
    NombreNoCoincide,
    Autofirmado,
    RaizDesconocida,
    FirmaNoValida,
    UsoNoPermitido,
    CadenaDemasiadoLarga,
    NoSoportado,
    Formato,
}

impl CertError {
    pub fn mensaje(&self) -> &'static str {
        match self {
            CertError::Caducado => "El certificado ha caducado. Revisa la fecha del equipo.",
            CertError::AunNoValido => "El certificado aún no es válido. Revisa la fecha del equipo.",
            CertError::NombreNoCoincide => "El certificado no corresponde a este nombre de servidor.",
            CertError::Autofirmado => "El certificado está autofirmado y no es de confianza.",
            CertError::RaizDesconocida => "El certificado lo emitió una entidad que no es de confianza.",
            CertError::FirmaNoValida => "La firma de un certificado de la cadena no es válida.",
            CertError::UsoNoPermitido => "Un certificado de la cadena no puede usarse para esto.",
            CertError::CadenaDemasiadoLarga => "La cadena de certificados es demasiado larga.",
            CertError::NoSoportado => "El certificado usa algo que este sistema no admite.",
            CertError::Formato => "El certificado está mal formado.",
        }
    }
}

impl From<DerError> for CertError {
    fn from(_: DerError) -> Self {
        CertError::Formato
    }
}

type R<T> = Result<T, CertError>;

/// Fecha y hora UTC. El orden derivado (campo a campo) es el cronológico.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Time {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub min: u8,
    pub sec: u8,
}

/// CN y O legibles (vacíos si no hay).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Name {
    pub cn: String,
    pub org: String,
}

/// Datos de un certificado para mostrar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertInfo {
    pub cn: String,
    pub org: String,
    pub not_before: Time,
    pub not_after: Time,
    pub sha256: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainInfo {
    pub leaf: CertInfo,
    pub issuer: CertInfo,
}

// ---- OIDs (contenido DER) ----
const OID_RSA_ENC: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
const OID_RSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
const OID_RSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c];
const OID_RSA_SHA512: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d];
const OID_PSS: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0a];
const OID_MGF1: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x08];
const OID_SHA256: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
const OID_SHA384: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02];
const OID_SHA512: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03];
const OID_ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const OID_ECDSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];
const OID_ECDSA_SHA512: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x04];
const OID_EC_PUBKEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const OID_P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const OID_P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];
const OID_CN: &[u8] = &[0x55, 0x04, 0x03];
const OID_ORG: &[u8] = &[0x55, 0x04, 0x0a];
const OID_SKI: &[u8] = &[0x55, 0x1d, 0x0e];
const OID_KU: &[u8] = &[0x55, 0x1d, 0x0f];
const OID_SAN: &[u8] = &[0x55, 0x1d, 0x11];
const OID_BC: &[u8] = &[0x55, 0x1d, 0x13];
const OID_NC: &[u8] = &[0x55, 0x1d, 0x1e];
const OID_POLICIES: &[u8] = &[0x55, 0x1d, 0x20];
const OID_AKI: &[u8] = &[0x55, 0x1d, 0x23];
const OID_POLICY_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x24];
const OID_EKU: &[u8] = &[0x55, 0x1d, 0x25];
const OID_INHIBIT_ANY: &[u8] = &[0x55, 0x1d, 0x36];
const OID_SERVER_AUTH: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01];
const OID_ANY_EKU: &[u8] = &[0x55, 0x1d, 0x25, 0x00];

// ---- tiempo ----

fn days_in_month(y: u16, m: u8) -> u8 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        _ => 28,
    }
}

fn digits(b: &[u8]) -> Option<u32> {
    b.iter().try_fold(0u32, |a, &c| if c.is_ascii_digit() { Some(a * 10 + (c - b'0') as u32) } else { None })
}

/// UTCTime (YYMMDDHHMMSSZ; 50–99 → 19xx, 00–49 → 20xx) o GeneralizedTime (YYYYMMDDHHMMSSZ), en UTC.
fn parse_time(t: Tlv) -> R<Time> {
    let c = t.content;
    let (year, rest) = match (t.tag, c.len()) {
        (0x17, 13) => {
            let yy = digits(&c[..2]).ok_or(CertError::Formato)? as u16;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, &c[2..])
        }
        (0x18, 15) => (digits(&c[..4]).ok_or(CertError::Formato)? as u16, &c[4..]),
        _ => return Err(CertError::Formato),
    };
    if rest[10] != b'Z' {
        return Err(CertError::Formato);
    }
    let f = |i: usize| digits(&rest[i..i + 2]).map(|v| v as u8).ok_or(CertError::Formato);
    let tm = Time { year, month: f(0)?, day: f(2)?, hour: f(4)?, min: f(6)?, sec: f(8)? };
    if tm.month == 0 || tm.month > 12 || tm.day == 0 || tm.day > days_in_month(year, tm.month) || tm.hour > 23 || tm.min > 59 || tm.sec > 59 {
        return Err(CertError::Formato);
    }
    Ok(tm)
}

// ---- nombres ----

/// Texto de un DirectoryString; los caracteres de control se cambian por «?» y se recorta.
fn dir_string(t: Tlv) -> String {
    let mut s = String::new();
    match t.tag {
        0x0c => {
            if let Ok(x) = core::str::from_utf8(t.content) {
                s.push_str(x);
            }
        }
        0x13 | 0x14 | 0x16 => s.extend(t.content.iter().map(|&b| b as char)), // Teletex como Latin-1
        0x1e if t.content.len() % 2 == 0 => {
            let u = t.content.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]]));
            s.extend(char::decode_utf16(u).map(|r| r.unwrap_or('?')));
        }
        _ => {}
    }
    s.chars().map(|c| if c.is_control() { '?' } else { c }).take(96).collect()
}

fn parse_name(content: &[u8]) -> R<Name> {
    let mut n = Name::default();
    let mut r = Reader::new(content);
    while !r.is_empty() {
        let rdn = r.expect(0x31)?;
        let mut s = Reader::new(rdn.content);
        if s.is_empty() {
            return Err(CertError::Formato);
        }
        while !s.is_empty() {
            let atv = s.expect(0x30)?;
            let mut a = Reader::new(atv.content);
            let oid = a.expect(0x06)?.content;
            let v = a.read()?;
            a.finish()?;
            if oid == OID_CN && n.cn.is_empty() {
                n.cn = dir_string(v);
            } else if oid == OID_ORG && n.org.is_empty() {
                n.org = dir_string(v);
            }
        }
    }
    Ok(n)
}

// ---- algoritmos ----

/// AlgorithmIdentifier → (OID, parámetros opcionales).
fn alg_parts(alg: Tlv<'_>) -> R<(&[u8], Option<Tlv<'_>>)> {
    let mut r = Reader::new(alg.content);
    let oid = r.expect(0x06)?.content;
    let p = if r.is_empty() { None } else { Some(r.read()?) };
    r.finish()?;
    Ok((oid, p))
}

fn is_null(p: Option<Tlv>) -> bool {
    p.map_or(true, |p| p.tag == 0x05 && p.content.is_empty())
}

/// Hash de un AlgorithmIdentifier de resumen: 0 = SHA-256, 1 = SHA-384, 2 = SHA-512.
fn hash_idx(alg: Tlv) -> Option<usize> {
    let (oid, p) = alg_parts(alg).ok()?;
    if !is_null(p) {
        return None;
    }
    [OID_SHA256, OID_SHA384, OID_SHA512].iter().position(|&o| o == oid)
}

/// RSASSA-PSS (RFC 4055): hash y MGF1 con el mismo hash, sal del tamaño del hash, trailer 1.
fn pss_alg(p: Option<Tlv>) -> Option<SigAlg> {
    let p = p.filter(|p| p.tag == 0x30)?;
    let mut r = Reader::new(p.content);
    let h = hash_idx(Reader::new(r.expect(0xa0).ok()?.content).expect(0x30).ok()?)?;
    let mgf = Reader::new(r.expect(0xa1).ok()?.content).expect(0x30).ok()?;
    let (oid, mp) = alg_parts(mgf).ok()?;
    if oid != OID_MGF1 || hash_idx(mp.filter(|m| m.tag == 0x30)?)? != h {
        return None;
    }
    let salt = uint_bytes(Reader::new(r.expect(0xa2).ok()?.content).expect(0x02).ok()?).ok()?;
    if salt != [[32u8, 48, 64][h]] {
        return None;
    }
    if let Some(t) = r.optional(0xa3).ok()? {
        if uint_bytes(Reader::new(t.content).expect(0x02).ok()?).ok()? != [1] {
            return None;
        }
    }
    r.finish().ok()?;
    Some([SigAlg::RsaPssSha256, SigAlg::RsaPssSha384, SigAlg::RsaPssSha512][h])
}

/// `Ok(None)` = algoritmo bien formado pero que no soportamos.
fn parse_sig_alg(alg: Tlv) -> R<Option<SigAlg>> {
    let (oid, p) = alg_parts(alg)?;
    Ok(match oid {
        OID_RSA_SHA256 | OID_RSA_SHA384 | OID_RSA_SHA512 if !is_null(p) => None,
        OID_RSA_SHA256 => Some(SigAlg::RsaPkcs1Sha256),
        OID_RSA_SHA384 => Some(SigAlg::RsaPkcs1Sha384),
        OID_RSA_SHA512 => Some(SigAlg::RsaPkcs1Sha512),
        OID_PSS => pss_alg(p),
        _ if p.is_some() => None,
        OID_ECDSA_SHA256 => Some(SigAlg::EcdsaSha256),
        OID_ECDSA_SHA384 => Some(SigAlg::EcdsaSha384),
        OID_ECDSA_SHA512 => Some(SigAlg::EcdsaSha512),
        _ => None,
    })
}

/// SubjectPublicKeyInfo → clave soportada (`None` si es de otro tipo o curva).
fn parse_spki(spki: Tlv) -> R<Option<PubKey>> {
    let mut r = Reader::new(spki.content);
    let (oid, p) = alg_parts(r.expect(0x30)?)?;
    let bits = r.expect(0x03)?.content;
    r.finish()?;
    let [0, key @ ..] = bits else { return Err(CertError::Formato) };
    if oid == OID_RSA_ENC {
        let mut s = Reader::new(Reader::new(key).expect(0x30)?.content);
        let n = uint_bytes(s.expect(0x02)?)?;
        let e = uint_bytes(s.expect(0x02)?)?;
        s.finish()?;
        if n.is_empty() || n.len() > 1024 || e.len() > 8 || (n.len() == 1 && n[0] == 0) {
            return Err(CertError::Formato);
        }
        return Ok(if p.is_some_and(|p| p.tag == 0x05 && p.content.is_empty()) { Some(PubKey::Rsa { n: n.to_vec(), e: e.to_vec() }) } else { None });
    }
    if oid == OID_EC_PUBKEY {
        let curve = p.filter(|p| p.tag == 0x06).map(|p| p.content);
        return Ok(match (curve, key) {
            (Some(OID_P256), [0x04, ..]) if key.len() == 65 => Some(PubKey::EcP256(key.to_vec())),
            (Some(OID_P384), [0x04, ..]) if key.len() == 97 => Some(PubKey::EcP384(key.to_vec())),
            _ => None,
        });
    }
    Ok(None)
}

// ---- certificado ----

#[derive(Default)]
struct NameConstraints {
    permitted: Vec<String>,
    excluded: Vec<String>,
}

pub struct Cert {
    der: Vec<u8>,
    /// Rango del tbsCertificate dentro de `der` (lo firmado).
    tbs: (usize, usize),
    sig_alg: Option<SigAlg>,
    sig: Vec<u8>,
    issuer: Vec<u8>,
    subject: Vec<u8>,
    subject_name: Name,
    not_before: Time,
    not_after: Time,
    key: Option<PubKey>,
    /// basicConstraints: (cA, pathLen).
    bc: Option<(bool, Option<u32>)>,
    /// keyUsage presente: ¿incluye keyCertSign?
    key_cert_sign: Option<bool>,
    /// EKU presente: ¿incluye serverAuth (o anyExtendedKeyUsage)?
    eku_server: Option<bool>,
    dns: Vec<String>,
    ips: Vec<Vec<u8>>,
    nc: Option<NameConstraints>,
    /// Motivo por el que no puede formar parte de ninguna cadena (extensión crítica desconocida,
    /// nameConstraints de un tipo que no manejamos).
    rechazo: Option<CertError>,
    sha256: [u8; 32],
}

impl Cert {
    pub fn parse(der: &[u8]) -> R<Cert> {
        if der.len() > MAX_DER {
            return Err(CertError::Formato);
        }
        let mut top = Reader::new(der);
        let cert = top.expect(0x30)?;
        top.finish()?;
        let mut r = Reader::new(cert.content);
        let tbs = r.expect(0x30)?;
        let alg_out = r.expect(0x30)?;
        let sigbits = r.expect(0x03)?;
        r.finish()?;
        let [0, sig @ ..] = sigbits.content else { return Err(CertError::Formato) };
        if sig.is_empty() {
            return Err(CertError::Formato);
        }

        let mut t = Reader::new(tbs.content);
        let mut v3 = false;
        if let Some(v) = t.optional(0xa0)? {
            let mut vr = Reader::new(v.content);
            if int_bytes(vr.expect(0x02)?)? != [2] {
                return Err(CertError::NoSoportado); // v2 u otra
            }
            vr.finish()?;
            v3 = true;
        }
        int_bytes(t.expect(0x02)?)?; // serie
        let alg_in = t.expect(0x30)?;
        if alg_in.raw != alg_out.raw {
            return Err(CertError::Formato);
        }
        let sig_alg = parse_sig_alg(alg_in)?;
        let issuer = t.expect(0x30)?;
        parse_name(issuer.content)?;
        let mut v = Reader::new(t.expect(0x30)?.content);
        let not_before = parse_time(v.read()?)?;
        let not_after = parse_time(v.read()?)?;
        v.finish()?;
        let subject = t.expect(0x30)?;
        let subject_name = parse_name(subject.content)?;
        let key = parse_spki(t.expect(0x30)?)?;
        if v3 {
            t.optional(0x81)?; // issuerUniqueID
            t.optional(0x82)?; // subjectUniqueID
        }

        let off = tbs.raw.as_ptr() as usize - der.as_ptr() as usize;
        let mut c = Cert {
            der: der.to_vec(),
            tbs: (off, off + tbs.raw.len()),
            sig_alg,
            sig: sig.to_vec(),
            issuer: issuer.raw.to_vec(),
            subject: subject.raw.to_vec(),
            subject_name,
            not_before,
            not_after,
            key,
            bc: None,
            key_cert_sign: None,
            eku_server: None,
            dns: Vec::new(),
            ips: Vec::new(),
            nc: None,
            rechazo: None,
            sha256: crate::crypto::sha256(der),
        };
        if let Some(e) = t.optional(0xa3)? {
            if !v3 {
                return Err(CertError::Formato);
            }
            c.parse_extensions(e)?;
        }
        t.finish()?;
        Ok(c)
    }

    fn parse_extensions(&mut self, e: Tlv) -> R<()> {
        let mut er = Reader::new(e.content);
        let mut list = Reader::new(er.expect(0x30)?.content);
        er.finish()?;
        if list.is_empty() {
            return Err(CertError::Formato);
        }
        let mut seen: Vec<&[u8]> = Vec::new();
        while !list.is_empty() {
            let item = list.expect(0x30)?;
            let mut x = Reader::new(item.content);
            let oid = x.expect(0x06)?.content;
            // Un `critical FALSE` explícito no es DER, pero se acepta: no cambia el significado.
            let critical = match x.optional(0x01)? {
                Some(b) => bool_val(b)?,
                None => false,
            };
            let val = x.expect(0x04)?.content;
            x.finish()?;
            if seen.contains(&oid) {
                return Err(CertError::Formato);
            }
            seen.push(oid);
            match oid {
                OID_BC => self.bc = Some(parse_bc(val)?),
                OID_KU => {
                    let bits = Reader::new(val).expect(0x03)?.content;
                    let [unused, b0, ..] = bits else { return Err(CertError::Formato) };
                    if *unused > 7 {
                        return Err(CertError::Formato);
                    }
                    self.key_cert_sign = Some(b0 & 0x04 != 0);
                }
                OID_EKU => {
                    let mut l = Reader::new(Reader::new(val).expect(0x30)?.content);
                    let mut ok = false;
                    if l.is_empty() {
                        return Err(CertError::Formato);
                    }
                    while !l.is_empty() {
                        let o = l.expect(0x06)?.content;
                        ok |= o == OID_SERVER_AUTH || o == OID_ANY_EKU;
                    }
                    self.eku_server = Some(ok);
                }
                OID_SAN => self.parse_san(val)?,
                OID_NC => self.parse_nc(val)?,
                // Se aceptan aunque sean críticas: no se procesan (sin políticas de certificado).
                OID_SKI | OID_AKI | OID_POLICIES | OID_POLICY_CONSTRAINTS | OID_INHIBIT_ANY => {}
                _ if critical => self.rechazo = Some(CertError::NoSoportado),
                _ => {}
            }
        }
        Ok(())
    }

    fn parse_san(&mut self, val: &[u8]) -> R<()> {
        let mut l = Reader::new(Reader::new(val).expect(0x30)?.content);
        if l.is_empty() {
            return Err(CertError::Formato);
        }
        while !l.is_empty() {
            let g = l.read()?;
            match g.tag {
                0x82 => self.dns.push(ascii_lower(g.content)?),
                0x87 if g.content.len() == 4 || g.content.len() == 16 => self.ips.push(g.content.to_vec()),
                0x87 => return Err(CertError::Formato),
                _ => {}
            }
        }
        Ok(())
    }

    fn parse_nc(&mut self, val: &[u8]) -> R<()> {
        let mut r = Reader::new(Reader::new(val).expect(0x30)?.content);
        let mut nc = NameConstraints::default();
        let mut rechazo = false;
        for (tag, dest) in [(0xa0, 0), (0xa1, 1)] {
            let Some(sub) = r.optional(tag)? else { continue };
            let mut l = Reader::new(sub.content);
            if l.is_empty() {
                return Err(CertError::Formato);
            }
            while !l.is_empty() {
                let mut st = Reader::new(l.expect(0x30)?.content);
                let base = st.read()?;
                // minimum/maximum: solo se admite el valor por defecto
                rechazo |= st.optional(0xa0)?.is_some() | st.optional(0xa1)?.is_some();
                st.finish()?;
                if base.tag == 0x82 {
                    let s = ascii_lower(base.content)?;
                    if dest == 0 { nc.permitted.push(s) } else { nc.excluded.push(s) }
                } else {
                    rechazo = true; // IP, correo, directorio…: se rechaza la cadena (conservador)
                }
            }
        }
        r.finish()?;
        if nc.permitted.is_empty() && nc.excluded.is_empty() && !rechazo {
            return Err(CertError::Formato);
        }
        if rechazo {
            self.rechazo = Some(CertError::NoSoportado);
        }
        self.nc = Some(nc);
        Ok(())
    }

    fn tbs(&self) -> &[u8] {
        &self.der[self.tbs.0..self.tbs.1]
    }

    pub fn info(&self) -> CertInfo {
        CertInfo {
            cn: self.subject_name.cn.clone(),
            org: self.subject_name.org.clone(),
            not_before: self.not_before,
            not_after: self.not_after,
            sha256: self.sha256,
        }
    }
}

fn ascii_lower(b: &[u8]) -> R<String> {
    if b.is_empty() || !b.is_ascii() {
        return Err(CertError::Formato);
    }
    Ok(b.iter().map(|&c| c.to_ascii_lowercase() as char).collect())
}

fn parse_bc(val: &[u8]) -> R<(bool, Option<u32>)> {
    let mut r = Reader::new(Reader::new(val).expect(0x30)?.content);
    let ca = match r.optional(0x01)? {
        Some(b) => bool_val(b)?,
        None => false,
    };
    let path = match r.optional(0x02)? {
        Some(i) => {
            let v = uint_bytes(i)?;
            if v.len() > 2 {
                return Err(CertError::Formato);
            }
            Some(v.iter().fold(0u32, |a, &x| a << 8 | x as u32))
        }
        None => None,
    };
    r.finish()?;
    Ok((ca, path))
}

// ---- nombres de servidor ----

enum Host {
    Dns(String),
    Ip(Vec<u8>),
    Invalid,
}

fn ipv4(s: &str) -> Option<Vec<u8>> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    parts
        .iter()
        .map(|p| {
            let ok = !p.is_empty() && p.len() <= 3 && p.bytes().all(|c| c.is_ascii_digit()) && (p.len() == 1 || !p.starts_with('0'));
            if ok { p.parse::<u8>().ok() } else { None }
        })
        .collect()
}

// shortcut: sin la forma con IPv4 al final (::ffff:1.2.3.4); añadirla si algún servidor la necesita.
fn ipv6(s: &str) -> Option<Vec<u8>> {
    let groups = |p: &str| -> Option<Vec<u16>> {
        if p.is_empty() {
            return Some(Vec::new());
        }
        p.split(':')
            .map(|g| if g.is_empty() || g.len() > 4 || !g.bytes().all(|c| c.is_ascii_hexdigit()) { None } else { u16::from_str_radix(g, 16).ok() })
            .collect()
    };
    let all = match s.find("::") {
        None => {
            let g = groups(s)?;
            if g.len() != 8 {
                return None;
            }
            g
        }
        Some(i) => {
            let (head, tail) = (groups(&s[..i])?, &s[i + 2..]);
            if tail.contains("::") {
                return None;
            }
            let tail = groups(tail)?;
            if head.len() + tail.len() > 7 {
                return None;
            }
            let mut g = head;
            g.resize(8 - tail.len(), 0);
            g.extend(tail);
            g
        }
    };
    Some(all.iter().flat_map(|g| g.to_be_bytes()).collect())
}

fn parse_host(h: &str) -> Host {
    let h = h.strip_prefix('[').and_then(|x| x.strip_suffix(']')).unwrap_or(h);
    if h.contains(':') {
        return ipv6(h).map_or(Host::Invalid, Host::Ip);
    }
    let h = h.strip_suffix('.').unwrap_or(h);
    if h.is_empty() || !h.is_ascii() {
        return Host::Invalid;
    }
    if h.bytes().all(|c| c.is_ascii_digit() || c == b'.') {
        return ipv4(h).map_or(Host::Invalid, Host::Ip); // parece IP: nunca casa con un nombre DNS
    }
    Host::Dns(h.to_ascii_lowercase())
}

/// Patrón de un SAN dNSName (ya en minúsculas) contra el host: comodín solo como primera etiqueta
/// completa «*.», con al menos dos etiquetas detrás, y sustituye exactamente una etiqueta.
fn dns_match(pat: &str, host: &str) -> bool {
    match pat.strip_prefix("*.") {
        None => pat == host,
        Some(rest) => {
            !rest.contains('*')
                && rest.contains('.')
                && host.split_once('.').is_some_and(|(first, tail)| !first.is_empty() && tail == rest)
        }
    }
}

fn host_ok(c: &Cert, host: &Host) -> bool {
    match host {
        Host::Dns(h) => c.dns.iter().any(|p| dns_match(p, h)),
        Host::Ip(ip) => c.ips.iter().any(|x| x == ip),
        Host::Invalid => false,
    }
}

/// ¿`name` está dentro del subárbol `base`? (RFC 5280 §4.2.1.10; «.x» = solo subdominios)
fn within(name: &str, base: &str) -> bool {
    if base.is_empty() {
        return true;
    }
    let sub = |b: &str| name.len() > b.len() && name.ends_with(b) && name.as_bytes()[name.len() - b.len() - 1] == b'.';
    match base.strip_prefix('.') {
        Some(b) => sub(b),
        None => name == base || sub(base),
    }
}

/// Los dNSName del SAN de la hoja frente a los nameConstraints de una CA.
fn nc_allows(nc: &NameConstraints, names: &[String]) -> bool {
    names.iter().all(|n| {
        let (base, wild) = match n.strip_prefix("*.") {
            Some(b) => (b, true),
            None => (n.as_str(), false),
        };
        // un comodín cubre los subdominios de `base`: también cae si una exclusión está dentro
        let excluded = nc.excluded.iter().any(|e| within(base, e) || (wild && within(e, base)));
        let permitted = nc.permitted.is_empty() || nc.permitted.iter().any(|p| within(base, p));
        permitted && !excluded
    })
}

// ---- raíces ----

#[derive(Default)]
pub struct RootStore {
    roots: Vec<Cert>,
    by_subject: BTreeMap<Vec<u8>, Vec<usize>>,
}

impl RootStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.roots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// Añade una raíz en DER. `Ok(false)` si ya estaba.
    pub fn add_der(&mut self, der: &[u8]) -> R<bool> {
        let c = Cert::parse(der)?;
        if self.roots.iter().any(|r| r.sha256 == c.sha256) {
            return Ok(false);
        }
        self.by_subject.entry(c.subject.clone()).or_default().push(self.roots.len());
        self.roots.push(c);
        Ok(true)
    }

    /// Carga un paquete `[u16 BE longitud][DER]…`. Devuelve cuántas raíces nuevas entraron; las
    /// entradas que no se pueden leer se saltan, pero un paquete cortado es `Formato`.
    pub fn load_bundle(&mut self, mut b: &[u8]) -> R<usize> {
        let mut n = 0;
        while !b.is_empty() {
            let [h, l, rest @ ..] = b else { return Err(CertError::Formato) };
            let len = u16::from_be_bytes([*h, *l]) as usize;
            if len == 0 || len > rest.len() {
                return Err(CertError::Formato);
            }
            n += self.add_der(&rest[..len]).unwrap_or(false) as usize;
            b = &rest[len..];
        }
        Ok(n)
    }

    /// Todas las raíces, para mostrarlas en Configuración.
    pub fn list(&self) -> Vec<CertInfo> {
        self.roots.iter().map(Cert::info).collect()
    }

    fn with_subject<'a>(&'a self, subject: &[u8]) -> impl Iterator<Item = &'a Cert> {
        self.by_subject.get(subject).into_iter().flatten().map(move |&i| &self.roots[i])
    }
}

// ---- validación de la cadena ----

type VerifyFn<'a> = &'a dyn Fn(&PubKey, SigAlg, &[u8], &[u8]) -> bool;

struct Ctx<'a> {
    host: Host,
    now: Time,
    verify: VerifyFn<'a>,
}

#[derive(Default)]
struct Search {
    err: Option<CertError>,
    too_long: bool,
    tries: usize,
}

/// Comprueba un camino completo: `path[0]` = hoja, el último = raíz.
fn check_path(path: &[&Cert], cx: &Ctx) -> R<()> {
    let n = path.len();
    for c in path {
        if let Some(e) = c.rechazo {
            return Err(e);
        }
        if cx.now < c.not_before {
            return Err(CertError::AunNoValido);
        }
        if cx.now > c.not_after {
            return Err(CertError::Caducado);
        }
    }
    // Hoja: EKU.
    if path[0].eku_server == Some(false) {
        return Err(CertError::UsoNoPermitido);
    }
    for (i, c) in path.iter().enumerate().skip(1) {
        let root = i == n - 1;
        // La raíz es de confianza por estar en el almacén: puede ser v1 (sin basicConstraints).
        match c.bc {
            Some((true, pl)) => {
                if pl.is_some_and(|p| (p as usize) < i - 1) {
                    return Err(CertError::UsoNoPermitido);
                }
            }
            None if root => {}
            _ => return Err(CertError::UsoNoPermitido),
        }
        if c.key_cert_sign == Some(false) || (!root && c.eku_server == Some(false)) {
            return Err(CertError::UsoNoPermitido);
        }
        if c.nc.as_ref().is_some_and(|nc| !nc_allows(nc, &path[0].dns)) {
            return Err(CertError::UsoNoPermitido);
        }
    }
    if !host_ok(path[0], &cx.host) {
        return Err(CertError::NombreNoCoincide);
    }
    // La firma de la raíz sobre sí misma no cuenta: es de confianza por estar en el almacén.
    for w in path.windows(2) {
        let (c, issuer) = (w[0], w[1]);
        let (Some(alg), Some(key)) = (c.sig_alg, issuer.key.as_ref()) else { return Err(CertError::NoSoportado) };
        if !(cx.verify)(key, alg, c.tbs(), &c.sig) {
            return Err(CertError::FirmaNoValida);
        }
    }
    Ok(())
}

fn search<'a>(path: &mut Vec<&'a Cert>, sent: &'a [Cert], used: &mut [bool], roots: &'a RootStore, cx: &Ctx, st: &mut Search) -> bool {
    if path.len() >= MAX_PATH {
        st.too_long = true;
        return false;
    }
    let cur = path[path.len() - 1];
    for r in roots.with_subject(&cur.issuer) {
        if st.tries >= MAX_TRIES {
            return false;
        }
        st.tries += 1;
        path.push(r);
        match check_path(path, cx) {
            Ok(()) => return true,
            Err(e) => {
                st.err.get_or_insert(e);
            }
        }
        path.pop();
    }
    for i in 0..sent.len() {
        if used[i] || sent[i].subject != cur.issuer {
            continue;
        }
        used[i] = true;
        path.push(&sent[i]);
        if search(path, sent, used, roots, cx, st) {
            return true;
        }
        path.pop();
        used[i] = false;
    }
    false
}

/// Valida lo que manda el servidor (`chain`, hoja primero) contra `roots`, para el nombre `host`
/// y la fecha `now`. `verify` comprueba una firma (normalmente `pk::verify`).
pub fn verify_server(chain: &[&[u8]], host: &str, now: Time, roots: &RootStore, verify: VerifyFn) -> Result<ChainInfo, CertError> {
    let leaf = Cert::parse(chain.first().ok_or(CertError::Formato)?)?;
    // Los certificados de más que no se entiendan se ignoran: no tienen por qué ser del camino.
    let sent: Vec<Cert> = chain[1..].iter().take(MAX_SENT).filter_map(|d| Cert::parse(d).ok()).collect();
    let cx = Ctx { host: parse_host(host), now, verify };
    let mut st = Search::default();
    let mut path = alloc::vec![&leaf];
    if !search(&mut path, &sent, &mut alloc::vec![false; sent.len()], roots, &cx, &mut st) {
        return Err(if let Some(e) = st.err {
            e
        } else if st.too_long {
            CertError::CadenaDemasiadoLarga
        } else if leaf.issuer == leaf.subject {
            CertError::Autofirmado
        } else {
            CertError::RaizDesconocida
        });
    }
    Ok(ChainInfo { leaf: path[0].info(), issuer: path[1].info() })
}
