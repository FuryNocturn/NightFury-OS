// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! TCP (RFC 9293), lado cliente: una conexión (`Sock`) sin red por debajo. Recibe segmentos ya
//! leídos y deja los que hay que enviar en un `Vec<Seg>`; `tcp.rs` los lleva a la tarjeta.
//! RTO adaptable según RFC 6298; retransmisión rápida a los 3 ACK repetidos.
use super::pkt::*;
use super::{Ipv4, NetError, TcpState, CONNECT_TIMEOUT_MS, TCP_BUF};
use alloc::collections::VecDeque;
use alloc::vec::Vec;

pub const FIN: u8 = 1;
pub const SYN: u8 = 2;
pub const RST: u8 = 4;
pub const PSH: u8 = 8;
pub const ACK: u8 = 16;

pub const MY_MSS: u16 = 1460;
const RTO_INIT: u64 = 1_000;
const RTO_MIN: u64 = 200;
const RTO_MAX: u64 = 30_000;
const MAX_RETRIES: u8 = 8;
/// TIME-WAIT corto (RFC 9293 pide 2×MSL; aquí 4 s: solo hay clientes y puertos aleatorios).
const TIME_WAIT_MS: u64 = 4_000;
const FIN_WAIT2_MS: u64 = 30_000;
const MAX_OOO: usize = 16;

pub fn lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}
pub fn le(a: u32, b: u32) -> bool {
    !lt(b, a)
}

/// Un segmento por enviar.
#[derive(Clone, Debug)]
pub struct Seg {
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    pub win: u16,
    pub mss: Option<u16>,
    pub data: Vec<u8>,
}

pub fn build(src: Ipv4, dst: Ipv4, sp: u16, dp: u16, s: &Seg) -> Vec<u8> {
    let hl = if s.mss.is_some() { 24 } else { 20 };
    let mut v = Vec::with_capacity(hl + s.data.len());
    v.extend_from_slice(&sp.to_be_bytes());
    v.extend_from_slice(&dp.to_be_bytes());
    v.extend_from_slice(&s.seq.to_be_bytes());
    v.extend_from_slice(&s.ack.to_be_bytes());
    v.extend_from_slice(&[(hl as u8 / 4) << 4, s.flags]);
    v.extend_from_slice(&s.win.to_be_bytes());
    v.extend_from_slice(&[0, 0, 0, 0]);
    if let Some(m) = s.mss {
        v.extend_from_slice(&[2, 4]);
        v.extend_from_slice(&m.to_be_bytes());
    }
    v.extend_from_slice(&s.data);
    let c = finish(csum(&v, pseudo(src, dst, P_TCP, v.len())));
    v[16..18].copy_from_slice(&c.to_be_bytes());
    v
}

pub struct TcpIn<'a> {
    pub sp: u16,
    pub dp: u16,
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    pub win: u16,
    pub mss: Option<u16>,
    pub data: &'a [u8],
}

/// Lee un segmento TCP: longitudes y suma comprobadas; opciones leídas con tope (solo MSS).
pub fn parse<'a>(ip: &Ip<'a>) -> Option<TcpIn<'a>> {
    let p = ip.payload;
    if p.len() < 20 {
        return None;
    }
    let off = (p[12] >> 4) as usize * 4;
    if off < 20 || off > p.len() || finish(csum(p, pseudo(ip.src, ip.dst, P_TCP, p.len()))) != 0 {
        return None;
    }
    let mut o = &p[20..off];
    let mut mss = None;
    while let Some(&k) = o.first() {
        match k {
            0 => break,
            1 => o = &o[1..],
            _ => {
                let Some(&l) = o.get(1) else { break };
                let l = l as usize;
                if l < 2 || l > o.len() {
                    break;
                }
                if k == 2 && l == 4 {
                    mss = Some(rd16(o, 2));
                }
                o = &o[l..];
            }
        }
    }
    Some(TcpIn { sp: rd16(p, 0), dp: rd16(p, 2), seq: rd32(p, 4), ack: rd32(p, 8), flags: p[13], win: rd16(p, 14), mss, data: &p[off..] })
}

/// RST para un segmento que no pertenece a ninguna conexión (RFC 9293, 3.10.7.1).
pub fn rst_for(t: &TcpIn) -> Seg {
    if t.flags & ACK != 0 {
        Seg { seq: t.ack, ack: 0, flags: RST, win: 0, mss: None, data: Vec::new() }
    } else {
        let len = t.data.len() as u32 + (t.flags & SYN != 0) as u32 + (t.flags & FIN != 0) as u32;
        Seg { seq: 0, ack: t.seq.wrapping_add(len), flags: RST | ACK, win: 0, mss: None, data: Vec::new() }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum St {
    SynSent,
    /// Lado pasivo: llegó el SYN y se envió el SYN-ACK; falta el ACK.
    SynRcvd,
    Est,
    FinWait1,
    FinWait2,
    Closing,
    TimeWait,
    CloseWait,
    LastAck,
    Closed,
}

pub struct Sock {
    pub id: u32,
    pub remote: Ipv4,
    pub rport: u16,
    pub lport: u16,
    pub st: St,
    pub fail: Option<NetError>,
    /// La app ya soltó el identificador: la pila termina el cierre sola.
    pub released: bool,
    /// v0.12.1: conexión entrante aún en la cola del puerto a la escucha con ese id (sin aceptar).
    pub listener: Option<u32>,
    /// v0.12.1: proceso dueño, para el Administrador de tareas (0 = el sistema).
    pub owner: u64,
    /// Bytes de datos recibidos y puestos en el búfer de envío, para `connections`.
    pub rx: u64,
    pub tx: u64,
    /// v0.12.2: TLS por encima de esta conexión.
    pub tls: Option<alloc::boxed::Box<super::tlsconn::TlsConn>>,
    pub iss: u32,
    snd_una: u32,
    snd_nxt: u32,
    snd_wnd: u32,
    rcv_nxt: u32,
    mss: usize,
    /// Bytes desde `snd_una` (enviados sin confirmar y por enviar).
    sendq: Vec<u8>,
    recvq: VecDeque<u8>,
    ooo: Vec<(u32, Vec<u8>, bool)>,
    fin_pending: bool,
    fin_sent: bool,
    have_rtt: bool,
    srtt: u64,
    rttvar: u64,
    rto: u64,
    timer: Option<u64>,
    retries: u8,
    probe: Option<(u32, u64)>,
    dup: u8,
    /// Recuperando pérdidas: hasta qué número de secuencia se había enviado al retransmitir.
    recover: Option<u32>,
    last_wnd: u16,
    created: u64,
    wait_until: u64,
}

impl Sock {
    pub fn new(id: u32, remote: Ipv4, rport: u16, lport: u16, iss: u32, now: u64) -> Sock {
        Sock {
            id,
            remote,
            rport,
            lport,
            st: St::SynSent,
            fail: None,
            released: false,
            listener: None,
            owner: 0,
            rx: 0,
            tx: 0,
            tls: None,
            iss,
            snd_una: iss,
            snd_nxt: iss.wrapping_add(1),
            snd_wnd: 0,
            rcv_nxt: 0,
            mss: 536,
            sendq: Vec::new(),
            recvq: VecDeque::new(),
            ooo: Vec::new(),
            fin_pending: false,
            fin_sent: false,
            have_rtt: false,
            srtt: 0,
            rttvar: 0,
            rto: RTO_INIT,
            timer: Some(now + RTO_INIT),
            retries: 0,
            probe: Some((iss.wrapping_add(1), now)),
            dup: 0,
            recover: None,
            last_wnd: 0,
            created: now,
            wait_until: 0,
        }
    }

    /// Lado pasivo: nace en SYN_RECEIVED con el SYN del otro (`irs` = su número de secuencia).
    pub fn passive(id: u32, remote: Ipv4, rport: u16, lport: u16, iss: u32, t: &TcpIn, now: u64, listener: u32) -> Sock {
        let mut s = Sock::new(id, remote, rport, lport, iss, now);
        s.st = St::SynRcvd;
        s.listener = Some(listener);
        s.rcv_nxt = t.seq.wrapping_add(1);
        s.snd_wnd = t.win as u32;
        s.mss = (t.mss.unwrap_or(536) as usize).clamp(64, MY_MSS as usize);
        s
    }

    /// Cola de entrantes sin aceptar que ya murió (reset, plazo…): se puede quitar.
    pub fn dead(&self) -> bool {
        self.st == St::Closed && (self.released || self.listener.is_some())
    }

    /// Como `state`, pero con TLS el saludo cuenta: sigue «Conectando» hasta que acaba.
    pub fn tls_state(&self) -> TcpState {
        match &self.tls {
            Some(t) => t.state(self),
            None => self.state(),
        }
    }

    pub fn state(&self) -> TcpState {
        if let Some(e) = &self.fail {
            return TcpState::Failed(e.clone());
        }
        match self.st {
            St::SynSent | St::SynRcvd => TcpState::Connecting,
            St::Est => TcpState::Open,
            St::CloseWait => TcpState::PeerClosed,
            _ => TcpState::Closed,
        }
    }

    /// Ventana de recepción: búfer libre (sin escalado de ventana, 65535 como mucho).
    fn adv_win(&self) -> u16 {
        TCP_BUF.saturating_sub(self.recvq.len()).min(65535) as u16
    }

    fn mk(&mut self, seq: u32, flags: u8, data: Vec<u8>) -> Seg {
        self.last_wnd = self.adv_win();
        Seg { seq, ack: self.rcv_nxt, flags, win: self.last_wnd, mss: None, data }
    }

    pub fn syn_seg(&mut self) -> Seg {
        let mut s = self.mk(self.iss, SYN, Vec::new());
        s.ack = 0;
        s.mss = Some(MY_MSS);
        s
    }

    pub fn synack_seg(&mut self) -> Seg {
        let mut s = self.mk(self.iss, SYN | ACK, Vec::new());
        s.mss = Some(MY_MSS);
        s
    }

    fn ack_seg(&mut self) -> Seg {
        self.mk(self.snd_nxt, ACK, Vec::new())
    }

    pub fn rst_seg(&mut self) -> Seg {
        self.mk(self.snd_nxt, RST | ACK, Vec::new())
    }

    pub fn send_free(&self) -> usize {
        TCP_BUF.saturating_sub(self.sendq.len())
    }

    /// Mete en el búfer de envío lo que quepa; quien llama ejecuta `output`.
    pub fn queue(&mut self, data: &[u8]) -> usize {
        let n = data.len().min(self.send_free());
        self.sendq.extend_from_slice(&data[..n]);
        self.tx += n as u64;
        n
    }

    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let n = buf.len().min(self.recvq.len());
        for (b, v) in buf.iter_mut().zip(self.recvq.drain(..n)) {
            *b = v;
        }
        n
    }

    /// Tras leer: avisa al otro si la ventana estaba casi cerrada y ahora se ha abierto.
    pub fn window_update(&mut self, out: &mut Vec<Seg>) {
        let w = self.adv_win();
        if matches!(self.st, St::Est | St::CloseWait) && (self.last_wnd < MY_MSS && w >= MY_MSS || w - self.last_wnd.min(w) >= (TCP_BUF / 2) as u16) {
            out.push(self.ack_seg());
        }
    }

    /// Cierre ordenado pedido por la app.
    pub fn close(&mut self) {
        self.released = true;
        match self.st {
            St::SynSent => self.st = St::Closed,
            St::Est => {
                self.fin_pending = true;
                self.st = St::FinWait1;
            }
            St::CloseWait => {
                self.fin_pending = true;
                self.st = St::LastAck;
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        if !self.released {
            self.fail = Some(NetError::Reset);
        }
        self.st = St::Closed;
    }

    /// Pérdida de la dirección o del cable: todo se corta sin avisar al otro.
    pub fn kill(&mut self, e: NetError) {
        if !self.released && self.st != St::Closed {
            self.fail = Some(e);
        }
        self.st = St::Closed;
    }

    fn inflight(&self) -> usize {
        self.snd_nxt.wrapping_sub(self.snd_una) as usize
    }

    fn sample(&mut self, r: u64) {
        if !self.have_rtt {
            self.srtt = r;
            self.rttvar = r / 2;
            self.have_rtt = true;
        } else {
            self.rttvar = (3 * self.rttvar + self.srtt.abs_diff(r)) / 4;
            self.srtt = (7 * self.srtt + r) / 8;
        }
        self.rto = self.base_rto();
    }

    fn base_rto(&self) -> u64 {
        if self.have_rtt {
            (self.srtt + (4 * self.rttvar).max(1)).clamp(RTO_MIN, RTO_MAX)
        } else {
            RTO_INIT
        }
    }

    fn arm(&mut self, now: u64) {
        if self.timer.is_none() {
            self.timer = Some(now + self.rto);
        }
    }

    /// Envía datos nuevos (cabe en la ventana del otro) y el FIN pendiente. `true` si salió algo
    /// (todo segmento lleva el ACK al día).
    pub fn output(&mut self, now: u64, out: &mut Vec<Seg>) -> bool {
        if !matches!(self.st, St::Est | St::CloseWait | St::FinWait1 | St::Closing | St::LastAck) {
            return false;
        }
        let mut sent = false;
        for _ in 0..64 {
            let inflight = self.inflight();
            let data_in = inflight.saturating_sub(self.fin_sent as usize);
            let unsent = self.sendq.len().saturating_sub(data_in);
            let left = (self.snd_wnd as usize).saturating_sub(inflight);
            if unsent > 0 && left > 0 {
                let n = unsent.min(left).min(self.mss);
                let data = self.sendq[data_in..data_in + n].to_vec();
                let flags = if n == unsent { ACK | PSH } else { ACK };
                let seq = self.snd_nxt;
                let seg = self.mk(seq, flags, data);
                out.push(seg);
                self.snd_nxt = seq.wrapping_add(n as u32);
                if self.probe.is_none() {
                    self.probe = Some((self.snd_nxt, now));
                }
                self.arm(now);
                sent = true;
            } else if unsent == 0 && self.fin_pending && !self.fin_sent {
                let seq = self.snd_nxt;
                let seg = self.mk(seq, FIN | ACK, Vec::new());
                out.push(seg);
                self.fin_sent = true;
                self.snd_nxt = seq.wrapping_add(1);
                self.arm(now);
                sent = true;
            } else {
                if unsent > 0 {
                    self.arm(now); // ventana cerrada: el temporizador hará de sonda
                }
                break;
            }
        }
        sent
    }

    fn retransmit(&mut self, out: &mut Vec<Seg>) {
        self.probe = None;
        if self.st == St::SynSent {
            let s = self.syn_seg();
            out.push(s);
            return;
        }
        if self.st == St::SynRcvd {
            let s = self.synack_seg();
            out.push(s);
            return;
        }
        let inflight = self.inflight();
        if inflight > 0 {
            self.recover = Some(self.snd_nxt);
        }
        let data_in = inflight.saturating_sub(self.fin_sent as usize);
        if data_in > 0 {
            let n = self.mss.min(data_in).min(self.sendq.len());
            let seg = self.mk(self.snd_una, ACK, self.sendq[..n].to_vec());
            out.push(seg);
        } else if self.fin_sent && inflight > 0 {
            let seg = self.mk(self.snd_una, FIN | ACK, Vec::new());
            out.push(seg);
        }
    }

    pub fn on_timer(&mut self, now: u64, out: &mut Vec<Seg>) {
        match self.st {
            St::Closed => return,
            St::TimeWait | St::FinWait2 => {
                if now >= self.wait_until {
                    self.st = St::Closed;
                }
                return;
            }
            St::SynSent | St::SynRcvd if now.saturating_sub(self.created) >= CONNECT_TIMEOUT_MS => {
                self.fail = Some(NetError::Timeout);
                self.st = St::Closed;
                return;
            }
            _ => {}
        }
        let Some(t) = self.timer else { return };
        if now < t {
            return;
        }
        let inflight = self.inflight();
        let unsent = self.sendq.len().saturating_sub(inflight.saturating_sub(self.fin_sent as usize));
        // también cuentan las sondas de ventana cero: un otro que no abre nunca la ventana acaba en
        // Timeout y no retiene el socket para siempre
        self.retries += 1;
        if self.retries > MAX_RETRIES {
            let rst = self.rst_seg();
            out.push(rst);
            self.fail = Some(NetError::Timeout);
            self.st = St::Closed;
            return;
        }
        if inflight == 0 && unsent > 0 {
            // sonda de ventana cero: un byte fuera de ventana, que el otro contesta con su ventana
            let seq = self.snd_nxt;
            let seg = self.mk(seq, ACK, self.sendq[..1].to_vec());
            out.push(seg);
            self.snd_nxt = seq.wrapping_add(1);
        } else {
            self.retransmit(out);
        }
        self.rto = (self.rto * 2).min(RTO_MAX);
        self.timer = Some(now + self.rto);
    }

    pub fn on_segment(&mut self, t: &TcpIn, now: u64, out: &mut Vec<Seg>) {
        let (rst, syn, ack_f) = (t.flags & RST != 0, t.flags & SYN != 0, t.flags & ACK != 0);
        if self.st == St::SynSent {
            return self.syn_sent(t, now, out);
        }
        if self.st == St::Closed {
            return;
        }
        if self.st == St::SynRcvd && !self.syn_rcvd(t, now, out) {
            return;
        }
        let wnd = self.adv_win() as u32;
        if rst {
            // RFC 5961: solo vale el RST con el número exacto; si cae en la ventana, ACK de desafío
            let d = t.seq.wrapping_sub(self.rcv_nxt);
            if d == 0 {
                self.reset();
            } else if d < wnd {
                let s = self.ack_seg();
                out.push(s);
            }
            return;
        }
        if syn {
            let s = self.ack_seg();
            out.push(s);
            return;
        }
        if !ack_f {
            return;
        }
        if self.st == St::TimeWait {
            if !t.data.is_empty() || t.flags & FIN != 0 {
                let s = self.ack_seg();
                out.push(s);
                self.wait_until = now + TIME_WAIT_MS;
            }
            return;
        }
        // 1. número de secuencia: recorta lo ya recibido y lo que no cabe en la ventana
        let mut data = t.data;
        let mut seq = t.seq;
        let mut fin = t.flags & FIN != 0;
        let need_ack = !data.is_empty() || fin;
        let behind = self.rcv_nxt.wrapping_sub(seq);
        if (behind as i32) > 0 {
            let skip = behind as usize;
            if skip > data.len() {
                fin = false;
            }
            data = data.get(skip..).unwrap_or(&[]);
            seq = self.rcv_nxt;
        }
        let off = seq.wrapping_sub(self.rcv_nxt);
        if off > wnd || ((!data.is_empty() || fin) && off >= wnd) {
            let s = self.ack_seg();
            out.push(s);
            return;
        }
        let room = (wnd - off) as usize;
        if data.len() > room {
            data = &data[..room];
            fin = false;
        }
        // 2. ACK
        if lt(self.snd_nxt, t.ack) {
            let s = self.ack_seg();
            out.push(s);
            return;
        }
        if le(self.snd_una, t.ack) {
            let same_wnd = t.win as u32 == self.snd_wnd;
            self.snd_wnd = t.win as u32;
            let newly = t.ack.wrapping_sub(self.snd_una) as usize;
            if newly > 0 {
                // tras una pérdida, cada ACK parcial pide ya lo siguiente (sin esperar al RTO)
                if self.new_ack(t.ack, newly, now) {
                    self.retransmit(out);
                }
            } else if t.data.is_empty() && t.flags & FIN == 0 && same_wnd && self.snd_nxt != self.snd_una {
                self.dup = self.dup.saturating_add(1);
                if self.dup == 3 {
                    self.retransmit(out);
                }
            }
        }
        // 3. datos y FIN
        if !data.is_empty() || fin {
            if self.released && !data.is_empty() && matches!(self.st, St::FinWait1 | St::FinWait2) {
                // la app ya no puede leer: RFC 9293 3.6.1, se corta
                let s = self.rst_seg();
                out.push(s);
                self.st = St::Closed;
                return;
            }
            if seq == self.rcv_nxt {
                self.deliver(data, fin, now);
                self.drain_ooo(now);
            } else if matches!(self.st, St::Est | St::FinWait1 | St::FinWait2) {
                self.store_ooo(seq, data, fin);
            }
        }
        let sent = self.output(now, out);
        if need_ack && !sent && self.st != St::Closed {
            let s = self.ack_seg();
            out.push(s);
        }
    }

    /// Segmento recibido en SYN_RECEIVED. `true` si ya está establecida y hay que seguir procesándolo
    /// (el ACK puede traer datos).
    fn syn_rcvd(&mut self, t: &TcpIn, now: u64, out: &mut Vec<Seg>) -> bool {
        if t.flags & RST != 0 {
            if t.seq == self.rcv_nxt {
                self.reset();
            }
            return false;
        }
        if t.flags & SYN != 0 {
            let s = self.synack_seg();
            out.push(s);
            return false;
        }
        if t.flags & ACK == 0 {
            return false;
        }
        if t.ack != self.snd_nxt {
            out.push(Seg { seq: t.ack, ack: 0, flags: RST, win: 0, mss: None, data: Vec::new() });
            return false;
        }
        self.snd_una = t.ack;
        self.snd_wnd = t.win as u32;
        if let Some((_, t0)) = self.probe.take() {
            self.sample(now.saturating_sub(t0));
        }
        self.timer = None;
        self.retries = 0;
        self.st = St::Est;
        true
    }

    fn syn_sent(&mut self, t: &TcpIn, now: u64, out: &mut Vec<Seg>) {
        let ack_ok = t.flags & ACK != 0 && lt(self.iss, t.ack) && le(t.ack, self.snd_nxt);
        if t.flags & ACK != 0 && !ack_ok {
            if t.flags & RST == 0 {
                out.push(Seg { seq: t.ack, ack: 0, flags: RST, win: 0, mss: None, data: Vec::new() });
            }
            return;
        }
        if t.flags & RST != 0 {
            if ack_ok {
                self.fail = Some(NetError::Refused);
                self.st = St::Closed;
            }
            return;
        }
        // shortcut: sin apertura simultánea (SYN sin ACK se ignora); hace falta solo con pares que conecten entre sí.
        if t.flags & SYN != 0 && ack_ok {
            self.rcv_nxt = t.seq.wrapping_add(1);
            self.snd_una = t.ack;
            self.snd_wnd = t.win as u32;
            self.mss = (t.mss.unwrap_or(536) as usize).clamp(64, MY_MSS as usize);
            if let Some((e, t0)) = self.probe.take() {
                if le(e, t.ack) {
                    self.sample(now.saturating_sub(t0));
                }
            }
            self.timer = None;
            self.retries = 0;
            self.st = St::Est;
            let s = self.ack_seg();
            out.push(s);
        }
    }

    /// Procesa un ACK que avanza; `true` si estamos recuperando y falta por confirmar.
    fn new_ack(&mut self, ack: u32, newly: usize, now: u64) -> bool {
        let fin_acked = self.fin_sent && ack == self.snd_nxt;
        let data_acked = newly - fin_acked as usize;
        self.sendq.drain(..data_acked.min(self.sendq.len()));
        self.snd_una = ack;
        self.dup = 0;
        self.retries = 0;
        if let Some((e, t0)) = self.probe {
            if le(e, ack) {
                self.sample(now.saturating_sub(t0));
                self.probe = None;
            }
        }
        self.rto = self.base_rto(); // el ACK de datos nuevos deshace la espera exponencial
        self.timer = if self.snd_nxt != self.snd_una { Some(now + self.rto) } else { None };
        let partial = match self.recover {
            Some(r) if lt(ack, r) => true,
            Some(_) => {
                self.recover = None;
                false
            }
            None => false,
        };
        if fin_acked {
            match self.st {
                St::FinWait1 => {
                    self.st = St::FinWait2;
                    self.wait_until = now + FIN_WAIT2_MS;
                }
                St::Closing => {
                    self.st = St::TimeWait;
                    self.wait_until = now + TIME_WAIT_MS;
                }
                St::LastAck => self.st = St::Closed,
                _ => {}
            }
        }
        partial && self.snd_nxt != self.snd_una
    }

    /// Datos en orden (`seq == rcv_nxt`) al búfer de lectura; si traen FIN, avanza el estado.
    fn deliver(&mut self, data: &[u8], fin: bool, now: u64) {
        if !matches!(self.st, St::Est | St::FinWait1 | St::FinWait2) {
            return;
        }
        let n = data.len().min(TCP_BUF.saturating_sub(self.recvq.len()));
        self.recvq.extend(data[..n].iter().copied());
        self.rx += n as u64;
        self.rcv_nxt = self.rcv_nxt.wrapping_add(n as u32);
        if fin && n == data.len() {
            self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
            match self.st {
                St::Est => self.st = St::CloseWait,
                St::FinWait1 => self.st = St::Closing,
                St::FinWait2 => {
                    self.st = St::TimeWait;
                    self.wait_until = now + TIME_WAIT_MS;
                }
                _ => {}
            }
        }
    }

    fn drain_ooo(&mut self, now: u64) {
        while let Some(i) = self.ooo.iter().position(|o| (self.rcv_nxt.wrapping_sub(o.0) as i32) >= 0) {
            let (s, d, f) = self.ooo.swap_remove(i);
            let skip = self.rcv_nxt.wrapping_sub(s) as usize;
            if skip <= d.len() {
                self.deliver(&d[skip..], f, now);
            }
        }
    }

    /// Guarda el segmento adelantado si cabe en el búfer libre (y hay sitio en la lista); si no, se
    /// descarta y el otro lo retransmite.
    fn store_ooo(&mut self, seq: u32, data: &[u8], fin: bool) {
        let used: usize = self.ooo.iter().map(|o| o.1.len()).sum();
        let room = TCP_BUF.saturating_sub(self.recvq.len());
        if self.ooo.len() >= MAX_OOO || used + data.len() > room || self.ooo.iter().any(|o| o.0 == seq && o.1.len() >= data.len()) {
            return;
        }
        self.ooo.push((seq, data.to_vec(), fin));
    }
}
