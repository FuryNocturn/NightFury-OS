// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! v0.12.2: TLS sobre una conexión TCP de la pila. El `tls::Client` (sin E/S) vive dentro del `Sock`:
//! lo que llega por TCP entra en él y lo que cifra sale al búfer de envío. Todo avanza en `poll` y en las
//! propias llamadas (`tcp_send`, `tcp_recv`), sin bloquear. Las claves no salen de aquí: la app solo ve texto claro.
use super::tcb::{Seg, Sock, St};
use super::{Net, NetError, SockId, TcpState, TlsInfo, TCP_BUF};
use crate::tls::Client;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

const SALUDO_CORTADO: &str = "El servidor cerró la conexión durante el saludo seguro. Puede que solo admita versiones antiguas de TLS.";

pub struct TlsConn {
    client: Client,
    /// Cifrado que aún no cabe en el búfer de envío de TCP.
    out: Vec<u8>,
    /// Texto claro recibido y aún sin leer.
    plain: Vec<u8>,
    err: Option<NetError>,
}

impl TlsConn {
    pub fn state(&self, k: &Sock) -> TcpState {
        if let Some(e) = &self.err {
            return TcpState::Failed(e.clone());
        }
        let ready = self.client.is_ready();
        match k.state() {
            TcpState::Connecting => TcpState::Connecting,
            TcpState::Failed(e) => TcpState::Failed(e),
            // shortcut: un FIN sin close_notify cuenta como cierre normal (muchos servidores lo hacen); la
            // longitud del cuerpo HTTP ya detecta el truncado. Exigir close_notify si algún día importa.
            TcpState::Open | TcpState::PeerClosed if ready => {
                if self.client.peer_closed() || k.st == St::CloseWait {
                    TcpState::PeerClosed
                } else {
                    TcpState::Open
                }
            }
            TcpState::Open => TcpState::Connecting,
            TcpState::PeerClosed | TcpState::Closed if !ready => TcpState::Failed(NetError::Tls(String::from(SALUDO_CORTADO))),
            _ => TcpState::Closed,
        }
    }
}

/// Un paso: TCP → TLS (descifra), TLS → TCP (lo cifrado entra en el búfer de envío) y aviso de ventana.
fn pump(k: &mut Sock, t: &mut TlsConn, now: u64, segs: &mut Vec<Seg>) {
    if t.err.is_none() {
        let mut b = [0u8; 4096];
        while t.plain.len() < TCP_BUF {
            let n = k.read(&mut b);
            if n == 0 {
                break;
            }
            if let Err(e) = t.client.feed(&b[..n]) {
                t.err = Some(NetError::Tls(e.mensaje()));
                break;
            }
            t.plain.extend(t.client.read());
        }
    }
    t.out.extend(t.client.take_output());
    if !t.out.is_empty() && matches!(k.st, St::Est | St::CloseWait) {
        let n = k.queue(&t.out);
        t.out.drain(..n);
        k.output(now, segs);
    }
    k.window_update(segs);
}

impl Net {
    /// Abre la conexión TCP y le pone el cliente TLS (el ClientHello sale al establecerse TCP).
    pub(super) fn tls_open(&mut self, dst: super::Ipv4, port: u16, host: &str, insecure: bool) -> Result<SockId, NetError> {
        let (roots, time) = self.tls_env.clone().ok_or_else(|| NetError::Tls(String::from("Este equipo no puede comprobar certificados.")))?;
        // Semilla: 256 bits derivados del secreto de la pila, que sale del acumulador de aleatoriedad del sistema.
        let mut seed = [0u8; 32];
        for c in seed.chunks_mut(4) {
            c.copy_from_slice(&self.rand().to_be_bytes());
        }
        let id = self.tcp_open(dst, port)?;
        #[cfg(test)]
        let client = match self.tls_fijo {
            Some((r, s, x)) => Client::con_valores(host, time, roots, insecure, crate::crypto::Rng::new([3; 32]), r, s.to_vec(), x),
            None => Client::new(host, time, roots, insecure, seed),
        };
        #[cfg(not(test))]
        let client = Client::new(host, time, roots, insecure, seed);
        if let Some(k) = self.socks.iter_mut().find(|k| k.id == id.0) {
            k.tls = Some(Box::new(TlsConn { client, out: Vec::new(), plain: Vec::new(), err: None }));
        }
        Ok(id)
    }

    pub(super) fn tls_info(&self, s: SockId) -> Option<TlsInfo> {
        let i = self.socks.iter().find(|k| k.id == s.0 && !k.released)?.tls.as_ref()?.client.info()?;
        let name = |cn: &str, o: &str| match (cn.is_empty(), o.is_empty()) {
            (_, true) => String::from(cn),
            (true, _) => String::from(o),
            _ => alloc::format!("{cn} ({o})"),
        };
        let (subject, issuer, expires) = match &i.chain {
            Some(c) => (name(&c.leaf.cn, &c.leaf.org), name(&c.leaf.issuer.cn, &c.leaf.issuer.org), (c.leaf.not_after.year, c.leaf.not_after.month, c.leaf.not_after.day)),
            None => (String::new(), String::new(), (0, 0, 0)),
        };
        Some(TlsInfo { version: String::from(i.version), suite: String::from(i.suite), group: String::from(i.group), subject, issuer, expires, cert_error: i.cert_error.map(|e| String::from(e.mensaje())) })
    }

    /// Hace avanzar una conexión TLS y emite lo que salga.
    fn tls_run<R>(&mut self, i: usize, f: impl FnOnce(&mut Sock, &mut TlsConn) -> R) -> Option<R> {
        let now = self.now;
        let k = &mut self.socks[i];
        let mut t = k.tls.take()?;
        let mut segs = Vec::new();
        let r = f(k, &mut t);
        pump(k, &mut t, now, &mut segs);
        let (r_ip, rp, lp) = (k.remote, k.rport, k.lport);
        k.tls = Some(t);
        self.tcp_emit(r_ip, rp, lp, segs);
        Some(r)
    }

    pub(super) fn tls_tick(&mut self) {
        for i in 0..self.socks.len() {
            if !self.socks[i].released {
                self.tls_run(i, |_, _| ());
            }
        }
    }

    /// `tcp_send` con TLS: cifra lo que quepa (como máximo lo que deja libre el búfer) y devuelve cuánto.
    pub(super) fn tls_write(&mut self, i: usize, data: &[u8]) -> Option<Result<usize, NetError>> {
        self.tls_run(i, |k, t| {
            if let Some(e) = &t.err {
                return Err(e.clone());
            }
            if let Some(e) = &k.fail {
                return Err(e.clone());
            }
            if !t.client.is_ready() || !matches!(k.st, St::Est | St::CloseWait) {
                return Err(NetError::Protocol("la conexión no está abierta"));
            }
            let buffered = t.out.len() + TCP_BUF - k.send_free();
            let n = data.len().min(TCP_BUF.saturating_sub(buffered));
            if n > 0 {
                if let Err(e) = t.client.write(&data[..n]) {
                    let e = NetError::Tls(e.mensaje());
                    t.err = Some(e.clone());
                    return Err(e);
                }
            }
            Ok(n)
        })
    }

    /// `tcp_recv` con TLS: texto claro ya descifrado.
    pub(super) fn tls_read(&mut self, i: usize, buf: &mut [u8]) -> Option<Result<usize, NetError>> {
        self.tls_run(i, |_, t| {
            let n = buf.len().min(t.plain.len());
            buf[..n].copy_from_slice(&t.plain[..n]);
            t.plain.drain(..n);
            match &t.err {
                Some(e) if n == 0 => Err(e.clone()),
                _ => Ok(n),
            }
        })
    }

    /// Cierre ordenado: close_notify antes del FIN (si cabe en el búfer; si no, se pierde y ya).
    pub(super) fn tls_close(k: &mut Sock) {
        if let Some(mut t) = k.tls.take() {
            if t.err.is_none() && t.client.is_ready() {
                t.client.close();
                k.queue(&t.client.take_output());
            }
        }
    }
}
