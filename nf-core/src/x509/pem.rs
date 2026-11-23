// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! PEM (RFC 7468) y base64 estricto para los .crt/.cer/.pem que instala el administrador.

use alloc::vec::Vec;

const BEGIN: &[u8] = b"-----BEGIN CERTIFICATE-----";
const END: &[u8] = b"-----END CERTIFICATE-----";

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

/// Uno o varios certificados: bloques «BEGIN CERTIFICATE» en base64, o un DER directo (empieza por
/// SEQUENCE). Un bloque con base64 inválido se descarta; el texto entre bloques se ignora.
pub fn pem_or_der(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if find(bytes, BEGIN).is_none() {
        if bytes.first() == Some(&0x30) {
            out.push(bytes.to_vec());
        }
        return out;
    }
    let mut rest = bytes;
    while let Some(i) = find(rest, BEGIN) {
        let body = &rest[i + BEGIN.len()..];
        let Some(j) = find(body, END) else { break };
        if let Some(d) = base64_decode(&body[..j]) {
            if !d.is_empty() {
                out.push(d);
            }
        }
        rest = &body[j + END.len()..];
    }
    out
}

fn val(c: u8) -> Option<u32> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    } as u32)
}

/// Base64 estricto: alfabeto estándar, relleno `=` exacto y bits sobrantes a cero. Ignora solo
/// espacios, tabuladores y saltos de línea.
pub fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    let v: Vec<u8> = input.iter().copied().filter(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n')).collect();
    if v.len() % 4 != 0 {
        return None;
    }
    let pad = v.iter().rev().take_while(|&&b| b == b'=').count();
    if pad > 2 {
        return None;
    }
    let body = &v[..v.len() - pad];
    let mut out = Vec::with_capacity(body.len() / 4 * 3 + 2);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &c in body {
        acc = acc << 6 | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    if acc != 0 {
        return None; // bits de relleno no canónicos
    }
    Some(out)
}
