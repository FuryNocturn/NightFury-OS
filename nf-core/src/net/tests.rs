// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Pruebas de la pila de red: una tarjeta en memoria y un «router» simulado (ARP, DHCP, DNS, ICMP y
//! un servidor TCP mínimo) que contesta las tramas. El tiempo se simula pasando `now_ms` a `poll`.
use super::pkt::*;
use super::tcb::{self, Seg, ACK, FIN, RST, SYN};
use super::*;
use alloc::rc::Rc;
use alloc::string::ToString;
use core::cell::RefCell;
use std::collections::VecDeque;

mod basic;
mod dns_tests;
mod fuzz;
mod http_tests;
mod tcp_tests;

pub const HOST_MAC: [u8; 6] = [2, 0, 0, 0, 0, 1];
pub const R_MAC: [u8; 6] = [2, 0, 0, 0, 0, 0xAA];
pub const R_IP: Ipv4 = [192, 168, 1, 1];
pub const DNS2: Ipv4 = [192, 168, 1, 2];
pub const OFFER: Ipv4 = [192, 168, 1, 50];
pub const SRV: Ipv4 = [93, 184, 216, 34];

pub struct Wire {
    pub rx: VecDeque<Vec<u8>>,
    pub tx: Vec<Vec<u8>>,
    pub link: Option<Link>,
}

pub struct Dev(pub Rc<RefCell<Wire>>);

impl NetDev for Dev {
    fn name(&self) -> String {
        "Prueba".to_string()
    }
    fn mac(&self) -> [u8; 6] {
        HOST_MAC
    }
    fn link(&mut self) -> Option<Link> {
        self.0.borrow().link
    }
    fn send(&mut self, frame: &[u8]) -> bool {
        assert!((14..=1514).contains(&frame.len()), "trama de {} bytes", frame.len());
        self.0.borrow_mut().tx.push(frame.to_vec());
        true
    }
    fn recv(&mut self, buf: &mut [u8]) -> Option<usize> {
        let f = self.0.borrow_mut().rx.pop_front()?;
        buf[..f.len()].copy_from_slice(&f);
        Some(f.len())
    }
}

pub const LINK: Link = Link { speed_mbps: 1000, full_duplex: true };

/// Opción `code` de un mensaje DHCP.
pub fn dhcp_opt(m: &[u8], code: u8) -> Option<Vec<u8>> {
    let mut o = m.get(240..)?;
    while let Some(&c) = o.first() {
        if c == 255 {
            break;
        }
        if c == 0 {
            o = &o[1..];
            continue;
        }
        let l = o[1] as usize;
        if c == code {
            return Some(o[2..2 + l].to_vec());
        }
        o = &o[2 + l..];
    }
    None
}

pub fn dhcp_type(m: &[u8]) -> u8 {
    dhcp_opt(m, 53).map_or(0, |v| v[0])
}

/// Nombre de la pregunta (sin compresión).
pub fn qname(q: &[u8]) -> String {
    let (mut p, mut s) = (12, String::new());
    while q[p] != 0 {
        let l = q[p] as usize;
        if !s.is_empty() {
            s.push('.');
        }
        s.push_str(core::str::from_utf8(&q[p + 1..p + 1 + l]).unwrap());
        p += 1 + l;
    }
    s
}

/// Respuesta DNS: copia la pregunta, con `rcode` y los registros ya construidos.
pub fn dns_msg(q: &[u8], rcode: u8, rrs: &[Vec<u8>]) -> Vec<u8> {
    let mut v = q[..2].to_vec();
    v.extend_from_slice(&[0x81, 0x80 | rcode, 0, 1]);
    v.extend_from_slice(&(rrs.len() as u16).to_be_bytes());
    v.extend_from_slice(&[0, 0, 0, 0]);
    v.extend_from_slice(&q[12..]);
    for r in rrs {
        v.extend_from_slice(r);
    }
    v
}

/// Registro A con el nombre como puntero a la pregunta.
pub fn rr_a(ttl: u32, ip: Ipv4) -> Vec<u8> {
    let mut v = alloc::vec![0xC0, 0x0C, 0, 1, 0, 1];
    v.extend_from_slice(&ttl.to_be_bytes());
    v.extend_from_slice(&[0, 4]);
    v.extend_from_slice(&ip);
    v
}

pub type DnsHook = Box<dyn FnMut(Ipv4, &[u8], u16) -> Vec<(Ipv4, u16, Vec<u8>)>>;

pub struct Script {
    pub reply: Vec<u8>,
    pub close: bool,
    pub echo: bool,
}

pub struct Conn {
    pub port: u16,
    pub cport: u16,
    pub iss: u32,
    pub rcv_nxt: u32,
    pub got: Vec<u8>,
    pub out: Vec<u8>,
    pub close: bool,
    acked: usize,
    sent: usize,
    dup: u8,
    last_tx: u64,
    pub cw: u32,
    fin_sent: bool,
    pub fin_acked: bool,
    replied: bool,
    pub peer_fin: bool,
    pub max_unacked: usize,
}

/// Servidor TCP mínimo en `SRV`: responde, repite o calla según el guion del puerto. Reenvía lo
/// perdido por tiempo (500 ms) y por tres ACK repetidos.
pub struct Srv {
    pub scripts: Vec<(u16, Script)>,
    pub conns: Vec<Conn>,
    /// Ventana que anuncia; con 0 descarta los datos que lleguen.
    pub win: u16,
    /// Cuánto envía como mucho sin confirmar.
    pub swin: usize,
    pub syns: Vec<u64>,
    pub ignore_syn: u32,
    pub rst_seen: u32,
    iss_next: u32,
}

impl Srv {
    fn new() -> Srv {
        Srv { scripts: Vec::new(), conns: Vec::new(), win: 8192, swin: 8 * 1460, syns: Vec::new(), ignore_syn: 0, rst_seen: 0, iss_next: 0xFFFF_F000 }
    }

    pub fn listen(&mut self, port: u16, reply: &[u8], close: bool, echo: bool) {
        self.scripts.retain(|s| s.0 != port);
        self.scripts.push((port, Script { reply: reply.to_vec(), close, echo }));
    }

    fn push_new(c: &mut Conn, win: u16, swin: usize, now: u64, out: &mut Vec<Seg>) {
        let limit = (c.acked + swin.min(c.cw as usize)).min(c.out.len());
        while c.sent < limit {
            let n = (limit - c.sent).min(1460);
            let seq = c.iss.wrapping_add(1).wrapping_add(c.sent as u32);
            out.push(Seg { seq, ack: c.rcv_nxt, flags: ACK, win, mss: None, data: c.out[c.sent..c.sent + n].to_vec() });
            c.sent += n;
            c.last_tx = now;
        }
        c.max_unacked = c.max_unacked.max(c.sent - c.acked);
        if c.close && c.sent == c.out.len() && !c.fin_sent {
            let seq = c.iss.wrapping_add(1).wrapping_add(c.out.len() as u32);
            out.push(Seg { seq, ack: c.rcv_nxt, flags: FIN | ACK, win, mss: None, data: Vec::new() });
            c.fin_sent = true;
            c.last_tx = now;
        }
    }

    pub fn handle(&mut self, t: &tcb::TcpIn, now: u64) -> Vec<Seg> {
        let mut out = Vec::new();
        let Some(si) = self.scripts.iter().position(|s| s.0 == t.dp) else {
            if t.flags & RST == 0 {
                out.push(tcb::rst_for(t));
            }
            return out;
        };
        if t.flags & RST != 0 {
            self.conns.retain(|c| c.cport != t.sp);
            self.rst_seen += 1;
            return out;
        }
        if t.flags & SYN != 0 {
            self.syns.push(now);
            if self.ignore_syn > 0 {
                self.ignore_syn -= 1;
                return out;
            }
            self.conns.retain(|c| c.cport != t.sp);
            let iss = self.iss_next;
            self.iss_next = self.iss_next.wrapping_add(100_000);
            self.conns.push(Conn {
                port: t.dp,
                cport: t.sp,
                iss,
                rcv_nxt: t.seq.wrapping_add(1),
                got: Vec::new(),
                out: Vec::new(),
                close: false,
                acked: 0,
                sent: 0,
                dup: 0,
                last_tx: now,
                cw: 65535,
                fin_sent: false,
                fin_acked: false,
                replied: false,
                peer_fin: false,
                max_unacked: 0,
            });
            out.push(Seg { seq: iss, ack: t.seq.wrapping_add(1), flags: SYN | ACK, win: self.win, mss: Some(1460), data: Vec::new() });
            return out;
        }
        let Some(ci) = self.conns.iter().position(|c| c.cport == t.sp) else {
            out.push(tcb::rst_for(t));
            return out;
        };
        let (win, swin) = (self.win, self.swin);
        let script = &self.scripts[si].1;
        let c = &mut self.conns[ci];
        let need_ack = !t.data.is_empty() || t.flags & FIN != 0;
        if need_ack && t.seq == c.rcv_nxt && (win > 0 || t.data.is_empty()) {
            c.got.extend_from_slice(t.data);
            c.rcv_nxt = c.rcv_nxt.wrapping_add(t.data.len() as u32);
            if script.echo {
                c.out.extend_from_slice(t.data);
            } else if !c.replied && c.got.ends_with(b"\r\n\r\n") {
                c.out = script.reply.clone();
                c.replied = true;
                c.close = script.close;
            }
            if t.flags & FIN != 0 {
                c.rcv_nxt = c.rcv_nxt.wrapping_add(1);
                c.peer_fin = true;
                c.close |= script.echo;
            }
        }
        if need_ack {
            out.push(Seg { seq: c.iss.wrapping_add(1).wrapping_add(c.sent as u32), ack: c.rcv_nxt, flags: ACK, win, mss: None, data: Vec::new() });
        }
        if t.flags & ACK != 0 {
            let off = t.ack.wrapping_sub(c.iss.wrapping_add(1));
            if (off as i32) >= 0 {
                let off = off as usize;
                let a = off.min(c.out.len());
                let same = t.win as u32 == c.cw;
                c.cw = t.win as u32;
                if a > c.acked {
                    c.acked = a;
                    c.dup = 0;
                } else if a == c.acked && c.sent > c.acked && t.data.is_empty() && same {
                    c.dup += 1;
                    if c.dup == 3 {
                        let n = (c.sent - c.acked).min(1460);
                        let seq = c.iss.wrapping_add(1).wrapping_add(c.acked as u32);
                        out.push(Seg { seq, ack: c.rcv_nxt, flags: ACK, win, mss: None, data: c.out[c.acked..c.acked + n].to_vec() });
                    }
                }
                if c.fin_sent && off == c.out.len() + 1 {
                    c.fin_acked = true;
                }
            }
        }
        Self::push_new(c, win, swin, now, &mut out);
        out
    }

    pub fn tick(&mut self, now: u64) -> Vec<(u16, u16, Seg)> {
        let (win, swin) = (self.win, self.swin);
        let mut res = Vec::new();
        for c in self.conns.iter_mut() {
            if !(c.acked < c.out.len() || (c.fin_sent && !c.fin_acked)) || now.saturating_sub(c.last_tx) < 500 {
                continue;
            }
            let mut out = Vec::new();
            if c.cw == 0 && c.acked < c.out.len() {
                // sonda de ventana cero
                let seq = c.iss.wrapping_add(1).wrapping_add(c.acked as u32);
                out.push(Seg { seq, ack: c.rcv_nxt, flags: ACK, win, mss: None, data: c.out[c.acked..c.acked + 1].to_vec() });
                c.last_tx = now;
            } else {
                c.sent = c.acked;
                c.fin_sent = false;
                Self::push_new(c, win, swin, now, &mut out);
            }
            res.extend(out.into_iter().map(|s| (c.port, c.cport, s)));
        }
        res
    }

    /// ACK con la ventana nueva para todas las conexiones.
    pub fn update(&mut self) -> Vec<(u16, u16, Seg)> {
        let win = self.win;
        self.conns.iter().map(|c| (c.port, c.cport, Seg { seq: c.iss.wrapping_add(1).wrapping_add(c.sent as u32), ack: c.rcv_nxt, flags: ACK, win, mss: None, data: Vec::new() })).collect()
    }
}

pub struct Router {
    pub arp_on: bool,
    /// Peticiones ARP que ignora antes de contestar.
    pub arp_skip: u32,
    pub dhcp_on: bool,
    pub dhcp_drop: u32,
    pub lease: u32,
    pub t12: Option<(u32, u32)>,
    pub nak: bool,
    pub alive: Vec<Ipv4>,
    pub unreach: Vec<Ipv4>,
    pub dns_ips: Vec<Ipv4>,
    pub names: Vec<(String, Vec<Ipv4>, u32)>,
    pub dns_hook: Option<DnsHook>,
    pub log_dhcp: Vec<(u64, Ipv4, Vec<u8>)>,
    pub log_dns: Vec<(Ipv4, u16, Vec<u8>)>,
    pub arp_log: Vec<Ipv4>,
    pub srv: Srv,
    ip_id: u16,
}

impl Router {
    fn new() -> Router {
        Router {
            arp_on: true,
            arp_skip: 0,
            dhcp_on: true,
            dhcp_drop: 0,
            lease: 3600,
            t12: None,
            nak: false,
            alive: alloc::vec![R_IP, SRV],
            unreach: Vec::new(),
            dns_ips: alloc::vec![R_IP, DNS2],
            names: Vec::new(),
            dns_hook: None,
            log_dhcp: Vec::new(),
            log_dns: Vec::new(),
            arp_log: Vec::new(),
            srv: Srv::new(),
            ip_id: 0,
        }
    }

    pub fn ip_frame(&mut self, src: Ipv4, dst: Ipv4, proto: u8, p: &[u8]) -> Vec<u8> {
        self.ip_id = self.ip_id.wrapping_add(1);
        eth(HOST_MAC, R_MAC, ETH_IP, &ip_build(src, dst, proto, 64, self.ip_id, p))
    }

    pub fn tcp_frame(&mut self, sport: u16, dport: u16, s: &Seg) -> Vec<u8> {
        let p = tcb::build(SRV, OFFER, sport, dport, s);
        self.ip_frame(SRV, OFFER, P_TCP, &p)
    }

    fn dhcp_reply(&self, req: &[u8], ty: u8) -> Vec<u8> {
        let mut v = alloc::vec![0u8; 240];
        v[..3].copy_from_slice(&[2, 1, 6]);
        v[4..8].copy_from_slice(&req[4..8]);
        if ty != 6 {
            v[16..20].copy_from_slice(&OFFER);
        }
        v[20..24].copy_from_slice(&R_IP);
        v[28..34].copy_from_slice(&req[28..34]);
        v[236..240].copy_from_slice(&[99, 130, 83, 99]);
        v.extend_from_slice(&[53, 1, ty, 54, 4, 192, 168, 1, 1]);
        if ty != 6 {
            v.extend_from_slice(&[51, 4]);
            v.extend_from_slice(&self.lease.to_be_bytes());
            v.extend_from_slice(&[1, 4, 255, 255, 255, 0, 3, 4, 192, 168, 1, 1, 6, 8, 192, 168, 1, 1, 192, 168, 1, 2]);
            if let Some((a, b)) = self.t12 {
                v.extend_from_slice(&[58, 4]);
                v.extend_from_slice(&a.to_be_bytes());
                v.extend_from_slice(&[59, 4]);
                v.extend_from_slice(&b.to_be_bytes());
            }
        }
        v.push(255);
        v
    }

    fn dns_default(&mut self, q: &[u8]) -> Vec<u8> {
        let n = qname(q);
        match self.names.iter().find(|x| x.0 == n) {
            Some((_, ips, ttl)) => dns_msg(q, 0, &ips.iter().map(|i| rr_a(*ttl, *i)).collect::<Vec<_>>()),
            None => dns_msg(q, 3, &[]),
        }
    }

    /// Trama del PC → tramas que el router le devuelve.
    pub fn handle(&mut self, f: &[u8], now: u64) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        if f.len() < 14 {
            return out;
        }
        if rd16(f, 12) == ETH_ARP {
            let a = parse_arp(&f[14..]).expect("ARP válido del PC");
            self.arp_log.push(a.tpa);
            let known = a.tpa == R_IP || self.alive.contains(&a.tpa) || self.dns_ips.contains(&a.tpa);
            if a.op == 1 && known && self.arp_on {
                if self.arp_skip > 0 {
                    self.arp_skip -= 1;
                } else {
                    out.push(eth(a.sha, R_MAC, ETH_ARP, &arp_build(2, R_MAC, a.tpa, a.sha, a.spa)));
                }
            }
            return out;
        }
        let ip = parse_packet(&f[14..]).expect("IP válido del PC");
        let hdr = &f[14..14 + 28.min(f.len() - 14)];
        let on_link = ip.dst[..3] == [192, 168, 1];
        let err = |me: &mut Router, ty: u8, code: u8| me.ip_frame(R_IP, OFFER, P_ICMP, &icmp_build(ty, code, [0; 4], hdr));
        if self.unreach.contains(&ip.dst) {
            out.push(err(self, 3, 1));
            return out;
        }
        if !on_link && ip.dst != BCAST_IP && ip.ttl <= 1 {
            out.push(err(self, 11, 0));
            return out;
        }
        match ip.proto {
            P_ICMP => {
                let (ty, _, rest, data) = parse_icmp(ip.payload).expect("ICMP válido del PC");
                if ty == 8 && self.alive.contains(&ip.dst) {
                    out.push(self.ip_frame(ip.dst, OFFER, P_ICMP, &icmp_build(0, 0, rest, data)));
                }
            }
            P_UDP => {
                let (sp, dp, data) = parse_udp(&ip).expect("UDP válido del PC");
                if dp == 67 {
                    self.log_dhcp.push((now, ip.dst, data.to_vec()));
                    let ty = dhcp_type(data);
                    if !self.dhcp_on || self.dhcp_drop > 0 {
                        self.dhcp_drop = self.dhcp_drop.saturating_sub(1);
                        return out;
                    }
                    let reply = match ty {
                        1 => Some(2),
                        3 if self.nak => {
                            self.nak = false;
                            Some(6)
                        }
                        3 => Some(5),
                        _ => None,
                    };
                    if let Some(r) = reply {
                        let m = self.dhcp_reply(data, r);
                        // con ciaddr (renovación) responde en unicast; si no, en difusión
                        let unicast = data[12..16] != [0; 4];
                        let dst = if unicast { OFFER } else { BCAST_IP };
                        let u = udp_build(R_IP, dst, 67, 68, &m);
                        out.push(self.ip_frame(R_IP, dst, P_UDP, &u));
                    }
                } else if dp == 53 && self.dns_ips.contains(&ip.dst) {
                    self.log_dns.push((ip.dst, sp, data.to_vec()));
                    let replies = match self.dns_hook.as_mut() {
                        Some(h) => h(ip.dst, data, sp),
                        None => alloc::vec![(ip.dst, sp, self.dns_default(data))],
                    };
                    for (from, to_port, m) in replies {
                        let u = udp_build(from, OFFER, 53, to_port, &m);
                        out.push(self.ip_frame(from, OFFER, P_UDP, &u));
                    }
                }
            }
            P_TCP if ip.dst == SRV => {
                let t = tcb::parse(&ip).expect("TCP válido del PC");
                for s in self.srv.handle(&t, now) {
                    out.push(self.tcp_frame(t.dp, t.sp, &s));
                }
            }
            _ => {}
        }
        out
    }

    pub fn tick(&mut self, now: u64) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for (port, cport, s) in self.srv.tick(now) {
            out.push(self.tcp_frame(port, cport, &s));
        }
        out
    }
}

pub struct Lan {
    pub wire: Rc<RefCell<Wire>>,
    pub net: Net,
    pub router: Router,
    pub now: u64,
    pub up_log: Vec<Vec<u8>>,
    pub drop_up: Box<dyn FnMut(&[u8]) -> bool>,
    pub drop_down: Box<dyn FnMut(&[u8]) -> bool>,
    pub reorder: bool,
}

/// (puerto origen, puerto destino, banderas, bytes de datos, seq) de una trama TCP.
pub fn tcp_info(f: &[u8]) -> Option<(u16, u16, u8, usize, u32)> {
    if f.len() < 34 || rd16(f, 12) != ETH_IP {
        return None;
    }
    let ip = parse_packet(&f[14..])?;
    let t = tcb::parse(&ip).filter(|_| ip.proto == P_TCP)?;
    Some((t.sp, t.dp, t.flags, t.data.len(), t.seq))
}

impl Lan {
    /// Red con cable puesto pero sin arrancar (todavía no se ha hecho `poll`).
    pub fn new() -> Lan {
        let wire = Rc::new(RefCell::new(Wire { rx: VecDeque::new(), tx: Vec::new(), link: Some(LINK) }));
        let mut net = Net::new([7; 32]);
        net.attach(Box::new(Dev(wire.clone())));
        Lan { wire, net, router: Router::new(), now: 1000, up_log: Vec::new(), drop_up: Box::new(|_| false), drop_down: Box::new(|_| false), reorder: false }
    }

    /// Red con la dirección ya concedida y la puerta de enlace resuelta.
    pub fn up() -> Lan {
        let mut l = Lan::new();
        l.run(500);
        assert_eq!(l.net.status().state, State::Connected);
        l
    }

    pub fn pump(&mut self) {
        for _ in 0..200 {
            self.net.poll(self.now);
            let tx = core::mem::take(&mut self.wire.borrow_mut().tx);
            let mut replies = Vec::new();
            let quiet = tx.is_empty();
            for f in tx {
                self.up_log.push(f.clone());
                if (self.drop_up)(&f) {
                    continue;
                }
                let mut b = self.router.handle(&f, self.now);
                if self.reorder {
                    b.reverse();
                }
                replies.extend(b);
            }
            replies.extend(self.router.tick(self.now));
            let idle = quiet && replies.is_empty();
            for r in replies {
                if !(self.drop_down)(&r) {
                    self.wire.borrow_mut().rx.push_back(r);
                }
            }
            if idle {
                break;
            }
        }
    }

    /// Avanza `ms` de tiempo simulado en pasos de 20 ms.
    pub fn run(&mut self, ms: u64) {
        let end = self.now + ms;
        loop {
            self.pump();
            if self.now >= end {
                break;
            }
            self.now = (self.now + 20).min(end);
        }
    }

    pub fn inject(&mut self, f: Vec<u8>) {
        self.wire.borrow_mut().rx.push_back(f);
        self.pump();
    }

    pub fn state(&self) -> State {
        self.net.status().state
    }

    pub fn ping(&mut self, dst: Ipv4, bytes: u16, ttl: u8) -> Result<PingReply, NetError> {
        let id = self.net.ping(dst, bytes, ttl)?;
        for _ in 0..400 {
            self.run(20);
            if let Some(r) = self.net.ping_poll(id) {
                return r;
            }
        }
        panic!("el ping no terminó");
    }

    pub fn resolve(&mut self, name: &str) -> Result<Vec<Ipv4>, NetError> {
        let id = self.net.resolve(name)?;
        for _ in 0..500 {
            self.run(20);
            if let Some(r) = self.net.resolve_poll(id) {
                return r;
            }
        }
        panic!("la resolución no terminó");
    }

    /// Conecta con `SRV:port` y espera a que salga de `Connecting`.
    pub fn connect(&mut self, port: u16) -> SockId {
        let s = self.net.tcp_connect(SRV, port).unwrap();
        for _ in 0..1500 {
            self.run(20);
            if self.net.tcp_state(s) != TcpState::Connecting {
                break;
            }
        }
        s
    }

    /// Frames de TCP enviados por el PC (con datos) desde `up_log`.
    pub fn host_tcp(&self) -> Vec<(u16, u16, u8, usize, u32)> {
        self.up_log.iter().filter_map(|f| tcp_info(f)).collect()
    }

    /// Lee hasta `want` bytes (o hasta que pasen `max_ms`), dejando correr el tiempo.
    pub fn read_all(&mut self, s: SockId, want: usize, max_ms: u64) -> Vec<u8> {
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        let end = self.now + max_ms;
        while got.len() < want && self.now < end {
            self.run(20);
            while let Ok(n) = self.net.tcp_recv(s, &mut buf) {
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
            }
        }
        got
    }

    /// Envía todo `data`, dejando correr el tiempo hasta que cabe.
    pub fn send_all(&mut self, s: SockId, data: &[u8], max_ms: u64) {
        let (mut off, end) = (0, self.now + max_ms);
        while off < data.len() && self.now < end {
            off += self.net.tcp_send(s, &data[off..]).unwrap();
            self.run(20);
        }
        assert_eq!(off, data.len(), "no se pudo enviar todo");
    }
}

/// Generador pseudoaleatorio xorshift con semilla (pruebas deterministas).
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}
