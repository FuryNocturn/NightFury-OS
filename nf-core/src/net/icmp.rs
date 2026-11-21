// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! ICMP (RFC 792): eco (pedir y contestar) y errores «destino inaccesible» / «tiempo excedido»,
//! que se asocian al ping o a la conexión TCP que los provocó por la cabecera original.
use super::pkt::*;
use super::tcb::St;
use super::{Ipv4, Net, NetError, PingId, PingReply, MAX_PINGS, PING_TIMEOUT_MS};
use alloc::vec::Vec;

/// Un resultado que nadie recoge en este tiempo se descarta.
const KEEP_DONE_MS: u64 = 60_000;
/// 1500 de MTU - 20 de IP - 8 de ICMP.
const MAX_ECHO: u16 = 1472;

pub(super) struct PingOp {
    id: u32,
    ident: u16,
    seq: u16,
    dst: Ipv4,
    sent: u64,
    done: Option<(u64, Result<PingReply, NetError>)>,
}

impl Net {
    pub(super) fn ping_start(&mut self, dst: Ipv4, bytes: u16, ttl: u8) -> Result<PingId, NetError> {
        let me = self.ready()?;
        if bytes > MAX_ECHO {
            return Err(NetError::Protocol("tamaño de eco excesivo"));
        }
        if self.pings.len() >= MAX_PINGS {
            return Err(NetError::Busy);
        }
        let ident = self.rand() as u16;
        self.ping_seq = self.ping_seq.wrapping_add(1);
        let seq = self.ping_seq;
        self.next_ping = self.next_ping.wrapping_add(1);
        let (id, now) = (self.next_ping, self.now);
        let mut op = PingOp { id, ident, seq, dst, sent: now, done: None };
        if dst == me {
            // a uno mismo no se sale a la red
            op.done = Some((now, Ok(PingReply { from: me, bytes, rtt_ms: 0, ttl })));
        } else {
            let data: Vec<u8> = (0..bytes).map(|i| b'a' + (i % 23) as u8).collect();
            let m = icmp_build(8, 0, [(ident >> 8) as u8, ident as u8, (seq >> 8) as u8, seq as u8], &data);
            self.send_ip(dst, P_ICMP, ttl, &m)?;
        }
        self.pings.push(op);
        Ok(PingId(id))
    }

    pub(super) fn ping_result(&mut self, id: PingId) -> Option<Result<PingReply, NetError>> {
        let Some(i) = self.pings.iter().position(|p| p.id == id.0) else { return Some(Err(NetError::Gone)) };
        if self.pings[i].done.is_none() {
            return None;
        }
        self.pings.swap_remove(i).done.map(|d| d.1)
    }

    pub(super) fn icmp_tick(&mut self) {
        let now = self.now;
        for p in self.pings.iter_mut() {
            if p.done.is_none() && now.saturating_sub(p.sent) >= PING_TIMEOUT_MS {
                p.done = Some((now, Err(NetError::Timeout)));
            }
        }
        self.pings.retain(|p| p.done.as_ref().is_none_or(|d| now.saturating_sub(d.0) < KEEP_DONE_MS));
    }

    pub(super) fn icmp_input(&mut self, ip: &Ip) {
        let Some((ty, _code, rest, data)) = parse_icmp(ip.payload) else { return };
        let Some(me) = self.cfg.as_ref().map(|c| c.ip) else { return };
        match ty {
            // eco pedido a nosotros (no a una difusión): se devuelve igual
            8 if ip.dst == me => {
                let r = icmp_build(0, 0, rest, data);
                let _ = self.send_ip(ip.src, P_ICMP, 64, &r);
            }
            0 => {
                let (ident, seq, now) = (rd16(&rest, 0), rd16(&rest, 2), self.now);
                if let Some(p) = self.pings.iter_mut().find(|p| p.done.is_none() && p.ident == ident && p.seq == seq && p.dst == ip.src) {
                    let rtt_ms = now.saturating_sub(p.sent);
                    p.done = Some((now, Ok(PingReply { from: ip.src, bytes: data.len() as u16, rtt_ms, ttl: ip.ttl })));
                }
            }
            3 | 11 => {
                let e = if ty == 3 { NetError::Unreachable(ip.src) } else { NetError::TtlExpired(ip.src) };
                self.icmp_error(me, e, data);
            }
            _ => {}
        }
    }

    /// `data` trae la cabecera IP original y al menos 8 bytes de lo que enviamos.
    fn icmp_error(&mut self, me: Ipv4, e: NetError, data: &[u8]) {
        let Some((proto, isrc, idst, rest)) = parse_inner(data) else { return };
        if isrc != me || rest.len() < 8 {
            return;
        }
        let now = self.now;
        match proto {
            P_ICMP if rest[0] == 8 => {
                let (ident, seq) = (rd16(rest, 4), rd16(rest, 6));
                if let Some(p) = self.pings.iter_mut().find(|p| p.done.is_none() && p.ident == ident && p.seq == seq && p.dst == idst) {
                    p.done = Some((now, Err(e)));
                }
            }
            P_TCP => {
                let (sp, dp, seq) = (rd16(rest, 0), rd16(rest, 2), rd32(rest, 4));
                // el número de secuencia del SYN evita que un tercero corte conexiones a ciegas
                if let Some(s) = self.socks.iter_mut().find(|s| s.st == St::SynSent && s.lport == sp && s.rport == dp && s.remote == idst && s.iss == seq) {
                    s.fail = Some(e);
                    s.st = St::Closed;
                }
            }
            _ => {}
        }
    }
}
