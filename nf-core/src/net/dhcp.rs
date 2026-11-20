// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Cliente DHCP (RFC 2131 y 2132): DISCOVER → OFFER → REQUEST → ACK, renovación a T1 (unicast al
//! servidor) y T2 (difusión), RELEASE al renovar a mano y vuelta a empezar al caducar o con NAK.
use super::pkt::*;
use super::{Cfg, Ipv4, Net};
use alloc::vec::Vec;

const MAGIC: [u8; 4] = [99, 130, 83, 99];
/// Reintentos de la renovación: la mitad del tiempo que queda, con este mínimo.
const RENEW_MIN_MS: u64 = 30_000;

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DState {
    /// Sin cable o sin tarjeta.
    #[default]
    Off,
    Select,
    Request,
    Bound,
    Renew,
    Rebind,
}

#[derive(Default)]
pub(super) struct Dhcp {
    pub st: DState,
    xid: u32,
    /// Desde cuándo se intenta obtener dirección (para el plazo de `NoInternet`).
    pub since: u64,
    next_tx: u64,
    tries: u32,
    /// Última oferta o confirmación: lo que falte en la siguiente se toma de aquí.
    last: Option<Msg>,
    t1_ms: u64,
    t2_ms: u64,
}

#[derive(Clone, Default)]
struct Msg {
    ty: u8,
    yiaddr: Ipv4,
    mask: Option<Ipv4>,
    router: Option<Ipv4>,
    dns: Vec<Ipv4>,
    lease: Option<u32>,
    server: Option<Ipv4>,
    t1: Option<u32>,
    t2: Option<u32>,
}

impl Msg {
    fn or(mut self, b: &Msg) -> Msg {
        self.mask = self.mask.or(b.mask);
        self.router = self.router.or(b.router);
        if self.dns.is_empty() {
            self.dns = b.dns.clone();
        }
        self.lease = self.lease.or(b.lease);
        self.server = self.server.or(b.server);
        self.t1 = self.t1.or(b.t1);
        self.t2 = self.t2.or(b.t2);
        self
    }
}

/// Mensaje BOOTP/DHCP del cliente. Con `ciaddr` vacío pide respuesta por difusión (RFC 2131, 4.1).
fn build(ty: u8, xid: u32, mac: [u8; 6], ciaddr: Ipv4, req: Option<Ipv4>, srv: Option<Ipv4>) -> Vec<u8> {
    let mut v = alloc::vec![0u8; 240];
    v[..3].copy_from_slice(&[1, 1, 6]);
    v[4..8].copy_from_slice(&xid.to_be_bytes());
    if ciaddr == [0; 4] {
        v[10] = 0x80;
    }
    v[12..16].copy_from_slice(&ciaddr);
    v[28..34].copy_from_slice(&mac);
    v[236..240].copy_from_slice(&MAGIC);
    v.extend_from_slice(&[53, 1, ty, 61, 7, 1]);
    v.extend_from_slice(&mac);
    v.extend_from_slice(&[12, 9]);
    v.extend_from_slice(b"NightFury");
    if let Some(r) = req {
        v.extend_from_slice(&[50, 4]);
        v.extend_from_slice(&r);
    }
    if let Some(s) = srv {
        v.extend_from_slice(&[54, 4]);
        v.extend_from_slice(&s);
    }
    v.extend_from_slice(&[55, 7, 1, 3, 6, 51, 54, 58, 59, 255]);
    v.resize(v.len().max(300), 0);
    v
}

/// Lee una respuesta del servidor; `None` si no es para nosotros o está mal formada.
fn parse(d: &[u8], xid: u32, mac: [u8; 6]) -> Option<Msg> {
    if d.len() < 240 || d[0] != 2 || d[1] != 1 || d[2] != 6 || rd32(d, 4) != xid || d[28..34] != mac || d[236..240] != MAGIC {
        return None;
    }
    let mut m = Msg { yiaddr: rd_ip(d, 16), ..Msg::default() };
    let mut o = &d[240..];
    while let Some(&c) = o.first() {
        if c == 255 {
            break;
        }
        if c == 0 {
            o = &o[1..];
            continue;
        }
        let l = *o.get(1)? as usize;
        let v = o.get(2..2 + l)?;
        match c {
            53 if l == 1 => m.ty = v[0],
            1 if l == 4 => m.mask = Some(rd_ip(v, 0)),
            3 if l >= 4 => m.router = Some(rd_ip(v, 0)),
            6 if l >= 4 => m.dns = v.chunks_exact(4).take(4).map(|c| rd_ip(c, 0)).filter(|a| *a != [0; 4]).collect(),
            51 if l == 4 => m.lease = Some(rd32(v, 0)),
            54 if l == 4 => m.server = Some(rd_ip(v, 0)),
            58 if l == 4 => m.t1 = Some(rd32(v, 0)),
            59 if l == 4 => m.t2 = Some(rd32(v, 0)),
            _ => {}
        }
        o = &o[2 + l..];
    }
    (m.ty != 0).then_some(m)
}

/// Dirección de equipo utilizable (ni 0, ni loopback, ni multicast/reservada).
fn host_ok(a: Ipv4) -> bool {
    a[0] != 0 && a[0] != 127 && a[0] < 224
}

/// Máscara de unos seguidos de ceros.
fn mask_ok(m: Ipv4) -> bool {
    let v = u32::from_be_bytes(m);
    v != 0 && (!v).wrapping_add(1) & !v == 0
}

fn classful(a: Ipv4) -> Ipv4 {
    match a[0] {
        0..=127 => [255, 0, 0, 0],
        128..=191 => [255, 255, 0, 0],
        _ => [255, 255, 255, 0],
    }
}

impl Net {
    pub(super) fn dhcp_start(&mut self) {
        let xid = self.rand();
        self.dhcp = Dhcp { st: DState::Select, xid, since: self.now, next_tx: self.now, ..Dhcp::default() };
    }

    /// Empieza de cero sin reiniciar el reloj del plazo de `NoInternet`.
    fn dhcp_restart(&mut self) {
        let since = self.dhcp.since;
        self.lose_config();
        self.dhcp_start();
        self.dhcp.since = since;
    }

    fn dhcp_bcast(&mut self, msg: &[u8]) {
        let u = udp_build([0; 4], BCAST_IP, 68, 67, msg);
        self.out_ip([0; 4], BCAST_IP, BCAST_IP, P_UDP, 64, &u);
    }

    /// REQUEST según el estado: pidiendo lo ofrecido (difusión con servidor), renovando (unicast al
    /// servidor) o reenlazando (difusión con ciaddr).
    fn dhcp_request(&mut self) {
        let (xid, mac) = (self.dhcp.xid, self.mac);
        match self.dhcp.st {
            DState::Request => {
                let Some(o) = &self.dhcp.last else { return };
                let m = build(3, xid, mac, [0; 4], Some(o.yiaddr), o.server);
                self.dhcp_bcast(&m);
            }
            DState::Renew => {
                let Some((me, srv)) = self.cfg.as_ref().map(|c| (c.ip, c.server)) else { return };
                let m = build(3, xid, mac, me, None, None);
                let u = udp_build(me, srv, 68, 67, &m);
                let _ = self.send_ip(srv, P_UDP, 64, &u);
            }
            DState::Rebind => {
                let Some(me) = self.cfg.as_ref().map(|c| c.ip) else { return };
                let m = build(3, xid, mac, me, None, None);
                let u = udp_build(me, BCAST_IP, 68, 67, &m);
                self.out_ip(me, BCAST_IP, BCAST_IP, P_UDP, 64, &u);
            }
            _ => {}
        }
    }

    /// RELEASE al servidor (si ya conocemos su MAC; si no, se pierde, y la concesión caduca sola).
    pub(super) fn dhcp_release(&mut self) {
        if !matches!(self.dhcp.st, DState::Bound | DState::Renew | DState::Rebind) {
            return;
        }
        let Some((me, srv)) = self.cfg.as_ref().map(|c| (c.ip, c.server)) else { return };
        let xid = self.rand();
        let m = build(7, xid, self.mac, me, None, Some(srv));
        let u = udp_build(me, srv, 68, 67, &m);
        let _ = self.send_ip(srv, P_UDP, 64, &u);
    }

    pub(super) fn dhcp_tick(&mut self) {
        let now = self.now;
        let (lease, since) = self.cfg.as_ref().map_or((0, 0), |c| (c.lease_s, c.since));
        if matches!(self.dhcp.st, DState::Bound | DState::Renew | DState::Rebind) && lease != u32::MAX && now >= since + lease as u64 * 1000 {
            // caducó: otra vez sin dirección, con el plazo de `NoInternet` desde ahora
            self.lose_config();
            self.dhcp_start();
        }
        if self.dhcp.st == DState::Bound && now >= since.saturating_add(self.dhcp.t1_ms) {
            self.dhcp.st = DState::Renew;
            self.dhcp.xid = self.rand();
            self.dhcp.next_tx = now;
        }
        if self.dhcp.st == DState::Renew && now >= since.saturating_add(self.dhcp.t2_ms) {
            self.dhcp.st = DState::Rebind;
            self.dhcp.next_tx = now;
        }
        if now < self.dhcp.next_tx {
            return;
        }
        match self.dhcp.st {
            DState::Select => {
                let m = build(1, self.dhcp.xid, self.mac, [0; 4], None, None);
                self.dhcp_bcast(&m);
                // espera creciente: 2, 4, 8, 16 y 32 s
                self.dhcp.next_tx = now + (2_000u64 << self.dhcp.tries.min(4));
                self.dhcp.tries += 1;
            }
            DState::Request => {
                if self.dhcp.tries >= 4 {
                    return self.dhcp_restart();
                }
                self.dhcp_request();
                self.dhcp.next_tx = now + (2_000u64 << self.dhcp.tries);
                self.dhcp.tries += 1;
            }
            DState::Renew | DState::Rebind => {
                self.dhcp_request();
                let end = if self.dhcp.st == DState::Renew { since.saturating_add(self.dhcp.t2_ms) } else { since + lease as u64 * 1000 };
                let rem = end.saturating_sub(now);
                self.dhcp.next_tx = now + (rem / 2).max(RENEW_MIN_MS).min(rem.max(1));
            }
            _ => {}
        }
    }

    pub(super) fn dhcp_input(&mut self, from: Ipv4, data: &[u8]) {
        let Some(m) = parse(data, self.dhcp.xid, self.mac) else { return };
        match (m.ty, self.dhcp.st) {
            (2, DState::Select) => {
                if !host_ok(m.yiaddr) {
                    return;
                }
                let mut o = m;
                o.server = o.server.or(Some(from));
                self.dhcp.last = Some(o);
                self.dhcp.st = DState::Request;
                self.dhcp.tries = 0;
                self.dhcp_request();
                self.dhcp.next_tx = self.now + 2_000;
                self.dhcp.tries = 1;
            }
            (5, DState::Request | DState::Renew | DState::Rebind) => {
                if self.dhcp.st == DState::Request && self.dhcp.last.as_ref().map(|o| o.yiaddr) != Some(m.yiaddr) {
                    return;
                }
                self.dhcp_bind(from, m);
            }
            (6, DState::Request) => self.dhcp_restart(),
            (6, DState::Renew | DState::Rebind) => {
                self.lose_config();
                self.dhcp_start();
            }
            _ => {}
        }
    }

    fn dhcp_bind(&mut self, from: Ipv4, m: Msg) {
        let m = match &self.dhcp.last {
            Some(b) => m.or(b),
            None => m,
        };
        if !host_ok(m.yiaddr) {
            return;
        }
        let lease = m.lease.unwrap_or(3600).max(10);
        let l = lease as u64 * 1000;
        let (t1_ms, t2_ms) = match (m.t1, m.t2) {
            (Some(a), Some(b)) if a > 0 && a < b && b < lease => (a as u64 * 1000, b as u64 * 1000),
            _ => (l / 2, l * 7 / 8),
        };
        if self.cfg.as_ref().is_some_and(|c| c.ip != m.yiaddr) {
            self.lose_config(); // otra dirección: lo anterior ya no vale
        }
        self.cfg = Some(Cfg {
            ip: m.yiaddr,
            mask: m.mask.filter(|k| mask_ok(*k)).unwrap_or_else(|| classful(m.yiaddr)),
            gw: m.router.filter(|r| host_ok(*r)),
            dns: m.dns.clone(),
            server: m.server.unwrap_or(from),
            lease_s: lease,
            since: self.now,
        });
        let inf = lease == u32::MAX;
        self.dhcp.t1_ms = if inf { u64::MAX } else { t1_ms };
        self.dhcp.t2_ms = if inf { u64::MAX } else { t2_ms };
        self.dhcp.st = DState::Bound;
        self.dhcp.last = Some(m);
    }
}
