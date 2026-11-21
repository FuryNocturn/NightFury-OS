// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Capa de enlace y red de `Net`: tramas Ethernet, ARP con caché (RFC 826), enrutado IPv4 y reparto
//! de lo recibido a ICMP, UDP (DHCP y DNS) y TCP.
use super::pkt::*;
use super::{Ipv4, Net, NetError};
use alloc::vec::Vec;

const ARP_TTL_MS: u64 = 60_000;
const ARP_MAX: usize = 32;
const REQ_MAX: usize = 8;
const HELD_MAX: usize = 8;
const ARP_TRIES: u8 = 4;
const ARP_GAP_MS: u64 = 1_000;
/// Tras fallar la puerta de enlace, se vuelve a probar a los 5 s.
const GW_RETRY_MS: u64 = 5_000;

pub(super) struct ArpEntry {
    pub(super) ip: Ipv4,
    pub(super) mac: [u8; 6],
    expires: u64,
}

pub(super) struct ArpReq {
    ip: Ipv4,
    tries: u8,
    next: u64,
}

/// Paquete IP esperando la MAC de su siguiente salto.
pub(super) struct Held {
    hop: Ipv4,
    pkt: Vec<u8>,
}

#[derive(Default)]
pub(super) struct ArpState {
    pub(super) cache: Vec<ArpEntry>,
    pub(super) reqs: Vec<ArpReq>,
    pub(super) held: Vec<Held>,
    /// La puerta de enlace ha contestado a ARP alguna vez con esta concesión.
    pub gw_ok: bool,
    /// Se agotaron los reintentos de ARP hacia la puerta de enlace.
    pub gw_fail: bool,
    gw_retry: u64,
}

impl Net {
    pub(super) fn send_eth(&mut self, dst: [u8; 6], ty: u16, payload: &[u8]) -> bool {
        let f = eth(dst, self.mac, ty, payload);
        if f.len() > 1514 {
            return false;
        }
        self.dev.as_mut().is_some_and(|d| d.send(&f))
    }

    fn arp_lookup(&self, ip: Ipv4) -> Option<[u8; 6]> {
        self.arp.cache.iter().find(|e| e.ip == ip && e.expires > self.now).map(|e| e.mac)
    }

    fn arp_send_request(&mut self, ip: Ipv4) {
        let me = self.cfg.as_ref().map_or([0; 4], |c| c.ip);
        let p = arp_build(1, self.mac, me, [0; 6], ip);
        self.send_eth(BCAST_MAC, ETH_ARP, &p);
    }

    /// Pide la MAC de `ip` si nadie la ha pedido ya.
    fn arp_want(&mut self, ip: Ipv4) {
        if self.arp.reqs.len() >= REQ_MAX || self.arp.reqs.iter().any(|r| r.ip == ip) {
            return;
        }
        self.arp.reqs.push(ArpReq { ip, tries: 1, next: self.now + ARP_GAP_MS });
        self.arp_send_request(ip);
    }

    fn arp_learn(&mut self, ip: Ipv4, mac: [u8; 6]) {
        let expires = self.now + ARP_TTL_MS;
        match self.arp.cache.iter_mut().find(|e| e.ip == ip) {
            Some(e) => {
                e.mac = mac;
                e.expires = expires;
            }
            None => {
                if self.arp.cache.len() >= ARP_MAX {
                    if let Some(i) = (0..self.arp.cache.len()).min_by_key(|&i| self.arp.cache[i].expires) {
                        self.arp.cache.swap_remove(i);
                    }
                }
                self.arp.cache.push(ArpEntry { ip, mac, expires });
            }
        }
        self.arp.reqs.retain(|r| r.ip != ip);
        if self.cfg.as_ref().is_some_and(|c| c.gw == Some(ip)) {
            self.arp.gw_ok = true;
            self.arp.gw_fail = false;
        }
        for h in core::mem::take(&mut self.arp.held) {
            if h.hop == ip {
                self.send_eth(mac, ETH_IP, &h.pkt);
            } else {
                self.arp.held.push(h);
            }
        }
    }

    pub(super) fn arp_tick(&mut self) {
        let now = self.now;
        self.arp.cache.retain(|e| e.expires > now);
        let gw = self.cfg.as_ref().and_then(|c| c.gw);
        if let Some(gw) = gw {
            if now >= self.arp.gw_retry && self.arp_lookup(gw).is_none() {
                self.arp_want(gw);
            }
        }
        let mut i = 0;
        while i < self.arp.reqs.len() {
            if self.arp.reqs[i].next <= now {
                if self.arp.reqs[i].tries >= ARP_TRIES {
                    let r = self.arp.reqs.remove(i);
                    self.arp.held.retain(|h| h.hop != r.ip);
                    if Some(r.ip) == gw {
                        self.arp.gw_fail = true;
                        self.arp.gw_retry = now + GW_RETRY_MS;
                    }
                    continue;
                }
                self.arp.reqs[i].tries += 1;
                self.arp.reqs[i].next = now + ARP_GAP_MS;
                let ip = self.arp.reqs[i].ip;
                self.arp_send_request(ip);
            }
            i += 1;
        }
    }

    /// Difusión limitada o dirigida a nuestra subred.
    pub(super) fn is_bcast(&self, ip: Ipv4) -> bool {
        ip == BCAST_IP || self.cfg.as_ref().is_some_and(|c| c.mask != BCAST_IP && (0..4).all(|i| ip[i] == c.ip[i] | !c.mask[i]))
    }

    /// Siguiente salto: el propio destino si es de nuestra subred; si no, la puerta de enlace.
    pub(super) fn route(&self, dst: Ipv4) -> Result<Ipv4, NetError> {
        let c = self.cfg.as_ref().ok_or(NetError::NoAddress)?;
        if self.is_bcast(dst) || (0..4).all(|i| dst[i] & c.mask[i] == c.ip[i] & c.mask[i]) {
            return Ok(dst);
        }
        c.gw.ok_or(NetError::Unreachable(c.ip))
    }

    /// Envía un datagrama IP desde nuestra dirección, enrutado.
    pub(super) fn send_ip(&mut self, dst: Ipv4, proto: u8, ttl: u8, payload: &[u8]) -> Result<(), NetError> {
        let hop = self.route(dst)?;
        let src = self.cfg.as_ref().map_or([0; 4], |c| c.ip);
        self.out_ip(src, dst, hop, proto, ttl, payload);
        Ok(())
    }

    /// Envía con origen y siguiente salto dados (DHCP: origen 0.0.0.0, difusión).
    pub(super) fn out_ip(&mut self, src: Ipv4, dst: Ipv4, hop: Ipv4, proto: u8, ttl: u8, payload: &[u8]) {
        self.ip_id = self.ip_id.wrapping_add(1);
        let pkt = ip_build(src, dst, proto, ttl, self.ip_id, payload);
        if self.is_bcast(hop) {
            self.send_eth(BCAST_MAC, ETH_IP, &pkt);
        } else if let Some(mac) = self.arp_lookup(hop) {
            self.send_eth(mac, ETH_IP, &pkt);
        } else {
            if self.arp.held.len() >= HELD_MAX {
                self.arp.held.remove(0);
            }
            self.arp.held.push(Held { hop, pkt });
            self.arp_want(hop);
        }
    }

    pub(super) fn rx(&mut self, frame: &[u8]) {
        if frame.len() < 14 || (frame[..6] != self.mac && frame[..6] != BCAST_MAC) {
            return;
        }
        let p = &frame[14..];
        match rd16(frame, 12) {
            ETH_ARP => self.arp_input(p),
            ETH_IP => self.ip_input(p),
            _ => {}
        }
    }

    fn arp_input(&mut self, p: &[u8]) {
        let Some(a) = parse_arp(p) else { return };
        let me = self.cfg.as_ref().map(|c| c.ip);
        if a.sha == self.mac || a.sha == BCAST_MAC || a.spa == [0; 4] || Some(a.spa) == me {
            return;
        }
        let ours = Some(a.tpa) == me;
        // Se aprende de quien esperamos. De quien ya conocemos, solo si repite su MAC o si su entrada
        // caducó: así una petición o respuesta suelta no cambia la MAC de la puerta de enlace ni llena la
        // caché. A quien solo nos pregunta se le contesta sin guardarlo.
        let esperado = self.arp.reqs.iter().any(|r| r.ip == a.spa);
        let conocido = self.arp.cache.iter().any(|e| e.ip == a.spa && (e.mac == a.sha || e.expires <= self.now));
        if esperado || conocido {
            self.arp_learn(a.spa, a.sha);
        }
        if a.op == 1 && ours {
            let r = arp_build(2, self.mac, a.tpa, a.sha, a.spa);
            self.send_eth(a.sha, ETH_ARP, &r);
        }
    }

    fn ip_input(&mut self, p: &[u8]) {
        let Some(ip) = parse_packet(p) else { return };
        match &self.cfg {
            Some(c) => {
                if ip.dst != c.ip && !self.is_bcast(ip.dst) {
                    return;
                }
                // orígenes imposibles: nosotros, 0.0.0.0, multidifusión o difusión
                if ip.src == c.ip || ip.src == [0; 4] || ip.src[0] >= 224 || self.is_bcast(ip.src) {
                    return;
                }
            }
            // sin dirección solo se atiende a DHCP, que puede llegar a cualquier destino
            None => {
                if ip.proto != P_UDP {
                    return;
                }
            }
        }
        match ip.proto {
            P_ICMP => self.icmp_input(&ip),
            P_UDP => {
                if let Some((sp, dp, data)) = parse_udp(&ip) {
                    if sp == 67 && dp == 68 {
                        self.dhcp_input(ip.src, data);
                    } else if self.dns_port(dp) {
                        self.dns_input(ip.src, dp, data);
                    } else {
                        self.udp_input(&ip, sp, dp, data);
                    }
                }
            }
            P_TCP => self.tcp_input(&ip),
            _ => {}
        }
    }
}
