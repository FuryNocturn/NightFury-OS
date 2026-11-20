// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Cabeceras de Ethernet II, ARP (RFC 826), IPv4 (RFC 791), UDP (RFC 768) e ICMP (RFC 792):
//! construir y leer. No guarda estado. Todo lo que se lee se comprueba antes de indexar.
use super::Ipv4;
use alloc::vec::Vec;

pub const ETH_IP: u16 = 0x0800;
pub const ETH_ARP: u16 = 0x0806;
pub const P_ICMP: u8 = 1;
pub const P_TCP: u8 = 6;
pub const P_UDP: u8 = 17;
pub const BCAST_MAC: [u8; 6] = [0xFF; 6];
pub const BCAST_IP: Ipv4 = [255; 4];

/// Lectura big-endian que nunca entra en pánico (0 fuera de rango; quien llama ya validó la longitud).
pub fn rd16(b: &[u8], i: usize) -> u16 {
    match b.get(i..i + 2) {
        Some(s) => u16::from_be_bytes([s[0], s[1]]),
        None => 0,
    }
}

pub fn rd32(b: &[u8], i: usize) -> u32 {
    match b.get(i..i + 4) {
        Some(s) => u32::from_be_bytes([s[0], s[1], s[2], s[3]]),
        None => 0,
    }
}

pub fn rd_ip(b: &[u8], i: usize) -> Ipv4 {
    match b.get(i..i + 4) {
        Some(s) => [s[0], s[1], s[2], s[3]],
        None => [0; 4],
    }
}

/// Acumula palabras de 16 bits (suma en complemento a uno, sin plegar).
pub fn csum(data: &[u8], mut acc: u32) -> u32 {
    let mut it = data.chunks_exact(2);
    for w in &mut it {
        acc = acc.wrapping_add(u16::from_be_bytes([w[0], w[1]]) as u32);
    }
    if let [last] = it.remainder() {
        acc = acc.wrapping_add((*last as u32) << 8);
    }
    acc
}

/// Pliega y complementa. Sobre datos con su suma incluida da 0 si es correcta.
pub fn finish(mut acc: u32) -> u16 {
    while acc >> 16 != 0 {
        acc = (acc & 0xFFFF) + (acc >> 16);
    }
    !(acc as u16)
}

/// Acumulado de la pseudocabecera de UDP/TCP.
pub fn pseudo(src: Ipv4, dst: Ipv4, proto: u8, len: usize) -> u32 {
    csum(&src, 0).wrapping_add(csum(&dst, 0)).wrapping_add(proto as u32).wrapping_add(len as u32)
}

pub fn eth(dst: [u8; 6], src: [u8; 6], ty: u16, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(14 + payload.len());
    v.extend_from_slice(&dst);
    v.extend_from_slice(&src);
    v.extend_from_slice(&ty.to_be_bytes());
    v.extend_from_slice(payload);
    v
}

pub struct Arp {
    pub op: u16,
    pub sha: [u8; 6],
    pub spa: Ipv4,
    pub tpa: Ipv4,
}

pub fn arp_build(op: u16, sha: [u8; 6], spa: Ipv4, tha: [u8; 6], tpa: Ipv4) -> Vec<u8> {
    let mut v = Vec::with_capacity(28);
    v.extend_from_slice(&[0, 1, 8, 0, 6, 4]);
    v.extend_from_slice(&op.to_be_bytes());
    v.extend_from_slice(&sha);
    v.extend_from_slice(&spa);
    v.extend_from_slice(&tha);
    v.extend_from_slice(&tpa);
    v
}

pub fn parse_arp(p: &[u8]) -> Option<Arp> {
    if p.len() < 28 || p[..6] != [0, 1, 8, 0, 6, 4] {
        return None;
    }
    Some(Arp { op: rd16(p, 6), sha: p[8..14].try_into().ok()?, spa: rd_ip(p, 14), tpa: rd_ip(p, 24) })
}

pub struct Ip<'a> {
    pub src: Ipv4,
    pub dst: Ipv4,
    pub proto: u8,
    pub ttl: u8,
    pub payload: &'a [u8],
}

/// Cabecera de 20 bytes (sin opciones), con «no fragmentar».
pub fn ip_build(src: Ipv4, dst: Ipv4, proto: u8, ttl: u8, id: u16, payload: &[u8]) -> Vec<u8> {
    let total = (20 + payload.len()) as u16;
    let mut v = Vec::with_capacity(20 + payload.len());
    v.extend_from_slice(&[0x45, 0]);
    v.extend_from_slice(&total.to_be_bytes());
    v.extend_from_slice(&id.to_be_bytes());
    v.extend_from_slice(&[0x40, 0, ttl, proto, 0, 0]);
    v.extend_from_slice(&src);
    v.extend_from_slice(&dst);
    let c = finish(csum(&v, 0));
    v[10..12].copy_from_slice(&c.to_be_bytes());
    v.extend_from_slice(payload);
    v
}

/// Valida versión, IHL, longitudes y suma de la cabecera. Descarta fragmentos (no se reensambla).
// shortcut: sin reensamblado de fragmentos IP, un datagrama fragmentado se pierde; hace falta si algún
// servidor lo envía (poco habitual con MTU de 1500 y TCP/DNS cortos).
pub fn parse_ip(p: &[u8]) -> Option<Ip<'_>> {
    if p.len() < 20 || p[0] >> 4 != 4 {
        return None;
    }
    let ihl = (p[0] & 15) as usize * 4;
    let total = rd16(p, 2) as usize;
    if ihl < 20 || total < ihl || total > p.len() || finish(csum(&p[..ihl], 0)) != 0 {
        return None;
    }
    if rd16(p, 6) & 0x3FFF != 0 {
        return None;
    }
    Some(Ip { src: rd_ip(p, 12), dst: rd_ip(p, 16), proto: p[9], ttl: p[8], payload: &p[ihl..total] })
}

/// Cabecera IP truncada de un mensaje ICMP de error (sin comprobar longitud total ni suma):
/// (protocolo, origen, destino, lo que sigue a la cabecera).
pub fn parse_inner(p: &[u8]) -> Option<(u8, Ipv4, Ipv4, &[u8])> {
    if p.len() < 20 || p[0] >> 4 != 4 {
        return None;
    }
    let ihl = (p[0] & 15) as usize * 4;
    if ihl < 20 || p.len() < ihl {
        return None;
    }
    Some((p[9], rd_ip(p, 12), rd_ip(p, 16), &p[ihl..]))
}

pub fn udp_build(src: Ipv4, dst: Ipv4, sp: u16, dp: u16, data: &[u8]) -> Vec<u8> {
    let len = 8 + data.len();
    let mut v = Vec::with_capacity(len);
    v.extend_from_slice(&sp.to_be_bytes());
    v.extend_from_slice(&dp.to_be_bytes());
    v.extend_from_slice(&(len as u16).to_be_bytes());
    v.extend_from_slice(&[0, 0]);
    v.extend_from_slice(data);
    let c = match finish(csum(&v, pseudo(src, dst, P_UDP, len))) {
        0 => 0xFFFF,
        c => c,
    };
    v[6..8].copy_from_slice(&c.to_be_bytes());
    v
}

/// (puerto origen, puerto destino, datos). La suma, si viene (≠ 0), debe cuadrar.
pub fn parse_udp<'a>(ip: &Ip<'a>) -> Option<(u16, u16, &'a [u8])> {
    let p = ip.payload;
    let len = rd16(p, 4) as usize;
    if p.len() < 8 || len < 8 || len > p.len() {
        return None;
    }
    if rd16(p, 6) != 0 && finish(csum(&p[..len], pseudo(ip.src, ip.dst, P_UDP, len))) != 0 {
        return None;
    }
    Some((rd16(p, 0), rd16(p, 2), &p[8..len]))
}

pub fn icmp_build(ty: u8, code: u8, rest: [u8; 4], data: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + data.len());
    v.extend_from_slice(&[ty, code, 0, 0]);
    v.extend_from_slice(&rest);
    v.extend_from_slice(data);
    let c = finish(csum(&v, 0));
    v[2..4].copy_from_slice(&c.to_be_bytes());
    v
}

/// (tipo, código, los 4 bytes siguientes a la suma, datos), con la suma comprobada.
pub fn parse_icmp(p: &[u8]) -> Option<(u8, u8, [u8; 4], &[u8])> {
    if p.len() < 8 || finish(csum(p, 0)) != 0 {
        return None;
    }
    Some((p[0], p[1], [p[4], p[5], p[6], p[7]], &p[8..]))
}
