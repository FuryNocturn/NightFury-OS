// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! v0.12.1: sockets UDP, puertos TCP a la escucha y lista de conexiones. Reglas en `mod.rs`.
//! Todo lo que llega es hostil: colas con tope y nada que entre en pánico.
use super::pkt::*;
use super::tcb::St;
use super::*;
use alloc::collections::VecDeque;

/// A cuántos (IP, puerto) distintos se recuerda haber enviado desde un socket (el más viejo sale).
// shortcut: 16 destinos por socket; un programa que hable con más de 16 servidores a la vez a
// través de un solo socket perdería sus respuestas, subir el tope si hace falta.
const SENT_MAX: usize = 16;

pub(super) struct Udp {
    id: u32,
    pub(super) port: u16,
    /// Abierto con puerto fijo: recibe también de la red local y por difusión.
    fixed: bool,
    queue: VecDeque<Datagram>,
    sent_to: Vec<(Ipv4, u16)>,
    rx: u64,
    tx: u64,
    owner: u64,
}

pub(super) struct Listener {
    pub(super) id: u32,
    pub(super) port: u16,
    owner: u64,
}

impl Net {
    /// Misma subred que nuestra IP según la máscara de DHCP (sin dirección, nunca).
    pub(super) fn is_local(&self, ip: Ipv4) -> bool {
        self.cfg.as_ref().is_some_and(|c| (0..4).all(|i| ip[i] & c.mask[i] == c.ip[i] & c.mask[i]))
    }

    pub(super) fn udp_open(&mut self, port: u16) -> Result<UdpId, NetError> {
        if self.udps.len() >= MAX_UDP {
            return Err(NetError::Busy);
        }
        let fixed = port != 0;
        let port = if port == 0 {
            let mut found = 0;
            for _ in 0..64 {
                let p = 49152 + (self.rand() % 16384) as u16;
                if !self.udps.iter().any(|u| u.port == p) && !self.dns_port(p) {
                    found = p;
                    break;
                }
            }
            if found == 0 {
                return Err(NetError::Busy);
            }
            found
        } else if port == 68 || self.udps.iter().any(|u| u.port == port) || self.dns_port(port) {
            return Err(NetError::InUse);
        } else {
            port
        };
        self.next_udp = self.next_udp.wrapping_add(1);
        self.udps.push(Udp { id: self.next_udp, port, fixed, queue: VecDeque::new(), sent_to: Vec::new(), rx: 0, tx: 0, owner: 0 });
        Ok(UdpId(self.next_udp))
    }

    pub(super) fn udp_write(&mut self, s: UdpId, dst: Ipv4, port: u16, data: &[u8]) -> Result<(), NetError> {
        let i = self.udps.iter().position(|u| u.id == s.0).ok_or(NetError::Gone)?;
        if data.len() > UDP_MAX {
            return Err(NetError::Protocol("datagrama demasiado grande"));
        }
        let me = self.ready()?;
        if (224..=239).contains(&dst[0]) {
            return Err(NetError::Unreachable(dst));
        }
        let b = udp_build(me, dst, self.udps[i].port, port, data);
        self.send_ip(dst, P_UDP, 64, &b)?;
        let bcast = self.is_bcast(dst);
        let u = &mut self.udps[i];
        u.tx += data.len() as u64;
        // a una difusión no contesta «ese» destino, sino cada equipo con su IP: no se recuerda
        if !bcast && !u.sent_to.contains(&(dst, port)) {
            if u.sent_to.len() >= SENT_MAX {
                u.sent_to.remove(0);
            }
            u.sent_to.push((dst, port));
        }
        Ok(())
    }

    pub(super) fn udp_read(&mut self, s: UdpId) -> Result<Option<Datagram>, NetError> {
        let u = self.udps.iter_mut().find(|u| u.id == s.0).ok_or(NetError::Gone)?;
        Ok(u.queue.pop_front())
    }

    pub(super) fn udp_shut(&mut self, s: UdpId) {
        self.udps.retain(|u| u.id != s.0);
    }

    /// Un datagrama que no es de DHCP ni de una consulta DNS en curso: al socket de ese puerto si la
    /// regla de «solo red local» lo deja pasar.
    pub(super) fn udp_input(&mut self, ip: &Ip, sp: u16, dp: u16, data: &[u8]) {
        if self.cfg.is_none() {
            return;
        }
        let local = self.is_local(ip.src) || self.is_bcast(ip.dst);
        let Some(u) = self.udps.iter_mut().find(|u| u.port == dp) else { return };
        if u.queue.len() >= UDP_QUEUE || !(u.sent_to.contains(&(ip.src, sp)) || (u.fixed && local)) {
            return;
        }
        u.rx += data.len() as u64;
        u.queue.push_back(Datagram { from: ip.src, port: sp, data: data.to_vec() });
    }

    pub(super) fn listen_open(&mut self, port: u16) -> Result<ListenId, NetError> {
        if port == 0 || self.listens.iter().any(|l| l.port == port) {
            return Err(NetError::InUse);
        }
        if self.listens.len() >= MAX_LISTEN {
            return Err(NetError::Busy);
        }
        self.next_listen = self.next_listen.wrapping_add(1);
        self.listens.push(Listener { id: self.next_listen, port, owner: 0 });
        Ok(ListenId(self.next_listen))
    }

    pub(super) fn listen_accept(&mut self, l: ListenId) -> Result<Option<(SockId, Ipv4, u16)>, NetError> {
        if !self.listens.iter().any(|x| x.id == l.0) {
            return Err(NetError::Gone);
        }
        let Some(i) = self.socks.iter().position(|k| k.listener == Some(l.0) && !matches!(k.st, St::SynRcvd | St::Closed)) else {
            return Ok(None);
        };
        if self.socks.iter().filter(|k| k.listener.is_none()).count() >= MAX_SOCKS {
            return Ok(None);
        }
        let k = &mut self.socks[i];
        k.listener = None;
        Ok(Some((SockId(k.id), k.remote, k.rport)))
    }

    pub(super) fn listen_shut(&mut self, l: ListenId) {
        self.listens.retain(|x| x.id != l.0);
        let mut jobs = Vec::new();
        for k in self.socks.iter_mut().filter(|k| k.listener == Some(l.0)) {
            if k.st != St::Closed {
                jobs.push((k.remote, k.rport, k.lport, alloc::vec![k.rst_seg()]));
            }
        }
        self.socks.retain(|k| k.listener != Some(l.0));
        for (r, rp, lp, segs) in jobs {
            self.tcp_emit(r, rp, lp, segs);
        }
    }

    pub(super) fn owner_set(&mut self, proto: Proto, id: u32, owner: u64) {
        match proto {
            Proto::Tcp | Proto::Tls => self.socks.iter_mut().filter(|k| k.id == id).for_each(|k| k.owner = owner),
            Proto::Udp => self.udps.iter_mut().filter(|u| u.id == id).for_each(|u| u.owner = owner),
            Proto::TcpListen => self.listens.iter_mut().filter(|l| l.id == id).for_each(|l| l.owner = owner),
        }
    }

    pub(super) fn conn_list(&self) -> Vec<ConnInfo> {
        let mut v = Vec::new();
        for k in self.socks.iter().filter(|k| !k.released && k.listener.is_none()) {
            let state = match k.st {
                St::SynSent | St::SynRcvd => "Conectando",
                St::Est => "Abierta",
                St::Closed => "Cerrada",
                _ => "Cerrando",
            };
            v.push(ConnInfo { proto: if k.tls.is_some() { Proto::Tls } else { Proto::Tcp }, id: k.id, local_port: k.lport, remote: Some((k.remote, k.rport)), state, rx: k.rx, tx: k.tx, owner: k.owner });
        }
        for l in &self.listens {
            v.push(ConnInfo { proto: Proto::TcpListen, id: l.id, local_port: l.port, remote: None, state: "Escuchando", rx: 0, tx: 0, owner: l.owner });
        }
        for u in &self.udps {
            v.push(ConnInfo { proto: Proto::Udp, id: u.id, local_port: u.port, remote: None, state: "—", rx: u.rx, tx: u.tx, owner: u.owner });
        }
        v
    }
}
