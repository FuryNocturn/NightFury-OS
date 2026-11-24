// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Respuesta OCSP adjunta al saludo TLS (RFC 6960; la extensión status_request es la de RFC 6066 §8).
//!
//! Solo se mira lo que el servidor adjunta: nunca se consulta a nadie. La respuesta vale si es
//! `successful` con BasicOCSPResponse, está firmada por el emisor de la hoja o por un respondedor
//! delegado (certificado del mismo emisor con id-kp-OCSPSigning y vigente), trae un CertID de ESTA hoja
//! y está en fechas (con unos minutos de margen). Todo lo demás es `Revocacion::Invalida`: nunca tumba
//! la conexión ni entra en pánico. El ResponderID no se compara: lo que garantiza quién firma es la
//! firma comprobada con una clave autorizada.

use super::der::{int_bytes, Reader, Tlv};
use super::{alg_parts, is_null, parse_sig_alg, parse_time, Cert, CertError, Revocacion, Time, VerifyFn, OID_SHA256, OID_SHA384, OID_SHA512, R};
use crate::crypto::{ct_eq, sha1, sha256, sha384, sha512};
use alloc::vec::Vec;

/// id-pkix-ocsp-basic (1.3.6.1.5.5.7.48.1.1).
const OID_OCSP_BASIC: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01, 0x01];
/// SHA-1 (1.3.14.3.2.26): solo para el CertID, que la norma define con él.
const OID_SHA1: &[u8] = &[0x2b, 0x0e, 0x03, 0x02, 0x1a];
/// Margen de reloj (s) en thisUpdate y nextUpdate.
const MARGEN: i64 = 300;
/// Certificados del respondedor que se prueban como máximo.
const MAX_CERTS: usize = 8;

/// Comprueba la respuesta OCSP `resp` para la hoja `hoja` emitida por `emisor` (los dos en DER).
pub fn comprobar_ocsp(resp: &[u8], hoja: &[u8], emisor: &[u8], now: Time, verify: VerifyFn) -> Revocacion {
    let (Ok(hoja), Ok(emisor)) = (Cert::parse(hoja), Cert::parse(emisor)) else { return Revocacion::Invalida };
    analizar(resp, &hoja, &emisor, now, verify).unwrap_or(Revocacion::Invalida)
}

/// Segundos desde 1970-01-01 (calendario gregoriano; el día se cuenta desde el 1 de marzo).
fn segundos(t: Time) -> i64 {
    let (m, d) = (t.month as i64, t.day as i64);
    let y = t.year as i64 - (m <= 2) as i64;
    let (era, yoe) = (y.div_euclid(400), y.rem_euclid(400));
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let dias = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    dias * 86_400 + t.hour as i64 * 3600 + t.min as i64 * 60 + t.sec as i64
}

fn malo() -> CertError {
    CertError::Formato
}

fn analizar(resp: &[u8], hoja: &Cert, emisor: &Cert, now: Time, verify: VerifyFn) -> R<Revocacion> {
    // OCSPResponse ::= SEQUENCE { responseStatus ENUMERATED, responseBytes [0] EXPLICIT ResponseBytes }
    let mut top = Reader::new(resp);
    let r = top.expect(0x30)?;
    top.finish()?;
    let mut r = Reader::new(r.content);
    if r.expect(0x0a)?.content != [0] {
        return Err(malo()); // solo `successful`
    }
    let rb = Reader::new(r.expect(0xa0)?.content).expect(0x30)?;
    r.finish()?;
    let mut rb = Reader::new(rb.content);
    if rb.expect(0x06)?.content != OID_OCSP_BASIC {
        return Err(malo());
    }
    let basica = rb.expect(0x04)?.content;
    rb.finish()?;

    // BasicOCSPResponse ::= SEQUENCE { tbsResponseData, signatureAlgorithm, signature, certs [0] OPTIONAL }
    let mut b = Reader::new(basica);
    let b = b.expect(0x30)?;
    let mut b = Reader::new(b.content);
    let tbs = b.expect(0x30)?;
    let alg = parse_sig_alg(b.expect(0x30)?)?.ok_or_else(malo)?;
    let [0, sig @ ..] = b.expect(0x03)?.content else { return Err(malo()) };
    let certs = b.optional(0xa0)?;
    b.finish()?;

    let firmada_por = |k: Option<&crate::pk::PubKey>| k.is_some_and(|k| verify(k, alg, tbs.raw, sig));
    let mut ok = firmada_por(emisor.key.as_ref());
    if let (false, Some(c)) = (ok, certs) {
        // Respondedor delegado (§4.2.2.2): certificado del mismo emisor, con OCSPSigning y vigente.
        let mut l = Reader::new(Reader::new(c.content).expect(0x30)?.content);
        for _ in 0..MAX_CERTS {
            if l.is_empty() || ok {
                break;
            }
            let Ok(d) = Cert::parse(l.read()?.raw) else { continue };
            let del_emisor = d.issuer == emisor.subject
                && d.rechazo.is_none()
                && d.eku_ocsp
                && d.not_before <= now
                && now <= d.not_after
                && matches!((d.sig_alg, emisor.key.as_ref()), (Some(a), Some(k)) if verify(k, a, d.tbs(), &d.sig));
            ok = del_emisor && firmada_por(d.key.as_ref());
        }
    }
    if !ok {
        return Err(malo());
    }

    // ResponseData ::= SEQUENCE { version [0] OPTIONAL, responderID, producedAt, responses, extensions [1] OPTIONAL }
    let mut d = Reader::new(tbs.content);
    d.optional(0xa0)?;
    if !matches!(d.read()?.tag, 0xa1 | 0xa2) {
        return Err(malo());
    }
    parse_time(d.expect(0x18)?)?;
    let mut lista = Reader::new(d.expect(0x30)?.content);
    d.optional(0xa1)?;
    d.finish()?;

    let ahora = segundos(now);
    while !lista.is_empty() {
        // SingleResponse ::= SEQUENCE { certID, certStatus, thisUpdate, nextUpdate [0] OPTIONAL, extensions [1] OPTIONAL }
        let mut s = Reader::new(lista.expect(0x30)?.content);
        let cid = s.expect(0x30)?;
        let estado = s.read()?;
        let desde = parse_time(s.expect(0x18)?)?;
        let hasta = match s.optional(0xa0)? {
            Some(n) => Some(parse_time(Reader::new(n.content).expect(0x18)?)?),
            None => None,
        };
        s.optional(0xa1)?;
        s.finish()?;
        if !cert_id_coincide(cid, hoja, emisor)? {
            continue;
        }
        // Sin nextUpdate no hay forma de saber hasta cuándo vale: no se acepta.
        let hasta = hasta.ok_or_else(malo)?;
        if ahora < segundos(desde) - MARGEN || ahora > segundos(hasta) + MARGEN {
            return Err(malo());
        }
        return match (estado.tag, estado.content) {
            (0x80, []) => Ok(Revocacion::Buena),
            (0xa1, c) => {
                Reader::new(c).expect(0x18)?; // revocationTime
                Ok(Revocacion::Revocada)
            }
            _ => Err(malo()), // unknown u otra cosa
        };
    }
    Err(malo())
}

/// CertID ::= SEQUENCE { hashAlgorithm, issuerNameHash, issuerKeyHash, serialNumber }
fn cert_id_coincide(cid: Tlv, hoja: &Cert, emisor: &Cert) -> R<bool> {
    let mut c = Reader::new(cid.content);
    let alg = c.expect(0x30)?;
    let nombre = c.expect(0x04)?.content;
    let clave = c.expect(0x04)?.content;
    let serie = int_bytes(c.expect(0x02)?)?;
    c.finish()?;
    let (oid, p) = alg_parts(alg)?;
    let h: fn(&[u8]) -> Vec<u8> = if !is_null(p) {
        return Ok(false);
    } else if oid == OID_SHA1 {
        |d| sha1(d).to_vec()
    } else if oid == OID_SHA256 {
        |d| sha256(d).to_vec()
    } else if oid == OID_SHA384 {
        |d| sha384(d).to_vec()
    } else if oid == OID_SHA512 {
        |d| sha512(d).to_vec()
    } else {
        return Ok(false);
    };
    // El nombre es el del campo `issuer` de la hoja, tal como viene codificado.
    Ok(ct_eq(serie, &hoja.serial) && ct_eq(nombre, &h(&hoja.issuer)) && ct_eq(clave, &h(&emisor.key_bits)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segundos_epoca() {
        let t = |year, month, day, hour, min, sec| Time { year, month, day, hour, min, sec };
        assert_eq!(segundos(t(1970, 1, 1, 0, 0, 0)), 0);
        assert_eq!(segundos(t(2000, 3, 1, 0, 0, 0)), 951_868_800);
        assert_eq!(segundos(t(2024, 2, 29, 12, 0, 1)), 1_709_208_001);
        assert_eq!(segundos(t(2026, 11, 24, 0, 0, 0)), 1_795_478_400);
    }
}
