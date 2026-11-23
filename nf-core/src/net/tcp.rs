// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! TCP dentro de `Net`: tabla de conexiones, puertos, números de secuencia iniciales (RFC 6528),
//! reparto de segmentos recibidos y salida de los que producen los `Sock` (ver `tcb.rs`).
use super::pkt::*;
use super::tcb::{self, Seg, Sock, St, ACK, RST, SYN};
use super::{Ipv4, Net, NetError, SockId, LISTEN_BACKLOG, MAX_SOCKS};
use alloc::vec::Vec;

impl Net {
    fn sock_idx(&self, s: SockId) -> Option<usize> {
        self.socks.iter().position(|k| k.id == s.0 && !k.released)
    }

    /// ISN = reloj de 4 µs + SHA-256(secreto, cuádrupla) (RFC 6528).
    fn isn(&self, lip: Ipv4, lport: u16, rip: Ipv4, rport: u16) -> u32 {
        let mut m = [0u8; 44];
        m[..32].copy_from_slice(&self.secret);
        m[32..36].copy_from_slice(&lip);
        m[36..38].copy_from_slice(&lport.to_be_bytes());
        m[38..42].copy_from_slice(&rip);
        m[42..44].copy_from_slice(&rport.to_be_bytes());
        let h = crate::crypto::sha256(&m);
        u32::from_be_bytes([h[0], h[1], h[2], h[3]]).wrapping_add((self.now as u32).wrapping_mul(250))
    }

    pub(super) fn tcp_emit(&mut self, remote: Ipv4, rport: u16, lport: u16, segs: Vec<Seg>) {
        let Some(me) = self.cfg.as_ref().map(|c| c.ip) else { return };
        for s in segs {
            let b = tcb::build(me, remote, lport, rport, &s);
            let _ = self.send_ip(remote, P_TCP, 64, &b);
        }
    }

    pub(super) fn tcp_open(&mut self, dst: Ipv4, port: u16) -> Result<SockId, NetError> {
        let me = self.ready()?;
        // las entrantes sin aceptar (en la cola de un listener) no cuentan
        if self.socks.iter().filter(|s| s.listener.is_none()).count() >= MAX_SOCKS {
            return Err(NetError::Busy);
        }
        if port == 0 {
            return Err(NetError::Refused);
        }
        self.route(dst)?;
        let mut lport = 0;
        for _ in 0..64 {
            let p = 49152 + (self.rand() % 16384) as u16;
            if !self.socks.iter().any(|s| s.lport == p) && !self.listens.iter().any(|l| l.port == p) {
                lport = p;
                break;
            }
        }
        if lport == 0 {
            return Err(NetError::Busy);
        }
        self.next_sock = self.next_sock.wrapping_add(1);
        let mut s = Sock::new(self.next_sock, dst, port, lport, self.isn(me, lport, dst, port), self.now);
        let syn = s.syn_seg();
        self.socks.push(s);
        self.tcp_emit(dst, port, lport, alloc::vec![syn]);
        Ok(SockId(self.next_sock))
    }

    pub(super) fn tcp_write(&mut self, s: SockId, data: &[u8]) -> Result<usize, NetError> {
        let i = self.sock_idx(s).ok_or(NetError::Gone)?;
        if self.socks[i].tls.is_some() {
            return self.tls_write(i, data).unwrap_or(Err(NetError::Gone));
        }
        let now = self.now;
        let k = &mut self.socks[i];
        if let Some(e) = &k.fail {
            return Err(e.clone());
        }
        if !matches!(k.st, St::Est | St::CloseWait) {
            return Err(NetError::Protocol("la conexión no está abierta"));
        }
        let n = k.queue(data);
        let mut out = Vec::new();
        k.output(now, &mut out);
        let (r, rp, lp) = (k.remote, k.rport, k.lport);
        self.tcp_emit(r, rp, lp, out);
        Ok(n)
    }

    pub(super) fn tcp_read(&mut self, s: SockId, buf: &mut [u8]) -> Result<usize, NetError> {
        let i = self.sock_idx(s).ok_or(NetError::Gone)?;
        if self.socks[i].tls.is_some() {
            let r = self.tls_read(i, buf).unwrap_or(Err(NetError::Gone));
            // sin texto claro y con TCP caído, vale el error de TCP
            return match (r, &self.socks[i].fail) {
                (Ok(0), Some(e)) => Err(e.clone()),
                (r, _) => r,
            };
        }
        let k = &mut self.socks[i];
        let n = k.read(buf);
        if n == 0 {
            if let Some(e) = &k.fail {
                return Err(e.clone());
            }
        }
        let mut out = Vec::new();
        k.window_update(&mut out);
        let (r, rp, lp) = (k.remote, k.rport, k.lport);
        self.tcp_emit(r, rp, lp, out);
        Ok(n)
    }

    pub(super) fn tcp_shut(&mut self, s: SockId, abort: bool) {
        let Some(i) = self.sock_idx(s) else { return };
        let now = self.now;
        let k = &mut self.socks[i];
        let mut out = Vec::new();
        if abort {
            if !matches!(k.st, St::SynSent | St::Closed) {
                out.push(k.rst_seg());
            }
            k.kill(NetError::Reset);
            k.released = true;
        } else {
            Net::tls_close(k);
            k.close();
            k.output(now, &mut out);
        }
        let (r, rp, lp) = (k.remote, k.rport, k.lport);
        self.tcp_emit(r, rp, lp, out);
        self.socks.retain(|s| !s.dead());
    }

    pub(super) fn tcp_tick(&mut self) {
        self.tls_tick();
        let now = self.now;
        let mut jobs = Vec::new();
        for k in self.socks.iter_mut() {
            let mut out = Vec::new();
            k.on_timer(now, &mut out);
            if !out.is_empty() {
                jobs.push((k.remote, k.rport, k.lport, out));
            }
        }
        for (r, rp, lp, out) in jobs {
            self.tcp_emit(r, rp, lp, out);
        }
        self.socks.retain(|s| !s.dead());
    }

    pub(super) fn tcp_input(&mut self, ip: &Ip) {
        if self.cfg.as_ref().map(|c| c.ip) != Some(ip.dst) {
            return;
        }
        let Some(t) = tcb::parse(ip) else { return };
        let now = self.now;
        match self.socks.iter_mut().find(|s| s.st != St::Closed && s.remote == ip.src && s.rport == t.sp && s.lport == t.dp) {
            Some(k) => {
                let mut out = Vec::new();
                k.on_segment(&t, now, &mut out);
                self.tcp_emit(ip.src, t.sp, t.dp, out);
                self.socks.retain(|s| !s.dead());
            }
            None if t.flags & RST != 0 => {}
            None if t.flags & (SYN | ACK) == SYN => self.tcp_syn(ip, &t),
            // conexión que no existe: RST
            None => self.tcp_emit(ip.src, t.sp, t.dp, alloc::vec![tcb::rst_for(&t)]),
        }
    }

    /// SYN de una conexión nueva. A un puerto a la escucha y de la red local: SYN-ACK (con la cola
    /// llena, contando las de medio saludo, se ignora). Si no, RST como a un puerto cerrado.
    fn tcp_syn(&mut self, ip: &Ip, t: &tcb::TcpIn) {
        let l = self.listens.iter().find(|l| l.port == t.dp).map(|l| l.id).filter(|_| self.is_local(ip.src));
        let Some(l) = l else {
            return self.tcp_emit(ip.src, t.sp, t.dp, alloc::vec![tcb::rst_for(t)]);
        };
        if self.socks.iter().filter(|s| s.listener == Some(l)).count() >= LISTEN_BACKLOG {
            return;
        }
        let Some(me) = self.cfg.as_ref().map(|c| c.ip) else { return };
        self.next_sock = self.next_sock.wrapping_add(1);
        let mut s = Sock::passive(self.next_sock, ip.src, t.sp, t.dp, self.isn(me, t.dp, ip.src, t.sp), t, self.now, l);
        let synack = s.synack_seg();
        self.socks.push(s);
        self.tcp_emit(ip.src, t.sp, t.dp, alloc::vec![synack]);
    }
}
