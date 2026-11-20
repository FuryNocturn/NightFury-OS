// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Cliente DNS (RFC 1035): consultas A por UDP, con identificador y puerto de origen aleatorios,
//! comprobación estricta de la respuesta, nombres comprimidos con tope, CNAME dentro de la misma
//! respuesta y caché por TTL.
use super::pkt::*;
use super::{parse_ip, Ipv4, Net, NetError, QueryId, DNS_TIMEOUT_MS, MAX_QUERIES};
use alloc::string::String;
use alloc::vec::Vec;

const CACHE_MAX: usize = 32;
const KEEP_DONE_MS: u64 = 60_000;
/// Cada intento al siguiente servidor, con este intervalo.
const RETRY_MS: u64 = 1_500;
const MAX_TTL_S: u32 = 86_400;
const MAX_ANSWERS: usize = 64;
const MAX_CNAME_HOPS: usize = 8;
const MAX_ADDRS: usize = 16;

pub(super) struct Query {
    id: u32,
    name: String,
    qid: u16,
    port: u16,
    start: u64,
    attempts: usize,
    next_tx: u64,
    done: Option<(u64, Result<Vec<Ipv4>, NetError>)>,
}

pub(super) struct CacheEntry {
    name: String,
    addrs: Vec<Ipv4>,
    expires: u64,
}

/// Qué hacer con un datagrama recibido.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Resp {
    /// No casa o está mal formada: se ignora y se sigue esperando.
    Ignore,
    Answer(Vec<Ipv4>, u32),
    Fail(NetError),
}

/// Nombre en minúsculas sin punto final, o `None` si no es un nombre de equipo válido.
fn clean_name(s: &str) -> Option<String> {
    let s = s.strip_suffix('.').unwrap_or(s);
    if s.is_empty() || s.len() > 253 {
        return None;
    }
    for label in s.split('.') {
        if label.is_empty() || label.len() > 63 || !label.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') {
            return None;
        }
    }
    Some(s.to_ascii_lowercase())
}

fn build_query(id: u16, name: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(18 + name.len());
    v.extend_from_slice(&id.to_be_bytes());
    v.extend_from_slice(&[1, 0, 0, 1, 0, 0, 0, 0, 0, 0]); // RD=1, una pregunta
    for l in name.split('.') {
        v.push(l.len() as u8);
        v.extend_from_slice(l.as_bytes());
    }
    v.extend_from_slice(&[0, 0, 1, 0, 1]); // tipo A, clase IN
    v
}

/// Lee un nombre (con compresión) en `pos`; devuelve el nombre en minúsculas y dónde sigue el
/// mensaje. Un puntero solo puede ir hacia atrás y se siguen 10 como mucho; el nombre, 255 bytes.
pub(super) fn read_name(msg: &[u8], pos: usize) -> Option<(String, usize)> {
    let (mut out, mut p, mut end, mut jumps) = (String::new(), pos, None, 0);
    loop {
        let l = *msg.get(p)? as usize;
        if l == 0 {
            p += 1;
            break;
        }
        if l & 0xC0 == 0xC0 {
            let ptr = ((l & 0x3F) << 8) | *msg.get(p + 1)? as usize;
            end.get_or_insert(p + 2);
            jumps += 1;
            if jumps > 10 || ptr >= p {
                return None;
            }
            p = ptr;
            continue;
        }
        if l & 0xC0 != 0 || out.len() + l + 1 > 255 {
            return None;
        }
        let lab = msg.get(p + 1..p + 1 + l)?;
        if !out.is_empty() {
            out.push('.');
        }
        for &c in lab {
            if !c.is_ascii() {
                return None;
            }
            out.push(c.to_ascii_lowercase() as char);
        }
        p += 1 + l;
    }
    Some((out, end.unwrap_or(p)))
}

enum Rec {
    A(Ipv4),
    Cname(String),
}

/// Valida la respuesta contra la consulta (identificador y pregunta) y extrae las direcciones A
/// del nombre, siguiendo los CNAME que traiga la propia respuesta.
pub(super) fn parse_response(m: &[u8], id: u16, name: &str) -> Resp {
    parse_inner(m, id, name).unwrap_or(Resp::Ignore)
}

fn parse_inner(m: &[u8], id: u16, name: &str) -> Option<Resp> {
    if m.len() < 12 || rd16(m, 0) != id {
        return None;
    }
    let flags = rd16(m, 2);
    // respuesta, opcode 0 y una sola pregunta
    if flags & 0x8000 == 0 || flags & 0x7800 != 0 || rd16(m, 4) != 1 {
        return None;
    }
    let (qname, p) = read_name(m, 12)?;
    if qname != name || rd16(m, p) != 1 || rd16(m, p + 2) != 1 {
        return None;
    }
    match flags & 15 {
        0 => {}
        3 => return Some(Resp::Fail(NetError::NotFound)),
        _ => return None, // SERVFAIL, REFUSED…: se prueba con el otro servidor
    }
    let mut pos = p + 4;
    let mut recs: Vec<(String, Rec, u32)> = Vec::new();
    for _ in 0..(rd16(m, 6) as usize).min(MAX_ANSWERS) {
        let (owner, q) = read_name(m, pos)?;
        let (ty, class, ttl, rdlen) = (rd16(m, q), rd16(m, q + 2), rd32(m, q + 4), rd16(m, q + 8) as usize);
        let rdata = m.get(q + 10..q + 10 + rdlen)?;
        pos = q + 10 + rdlen;
        if class != 1 {
            continue;
        }
        match ty {
            1 if rdlen == 4 => recs.push((owner, Rec::A(rd_ip(rdata, 0)), ttl)),
            5 => recs.push((owner, Rec::Cname(read_name(m, q + 10)?.0), ttl)),
            _ => {}
        }
    }
    let (mut cur, mut ttl) = (String::from(name), u32::MAX);
    for _ in 0..MAX_CNAME_HOPS {
        let addrs: Vec<Ipv4> = recs
            .iter()
            .filter_map(|r| match r {
                (o, Rec::A(a), t) if *o == cur => {
                    ttl = ttl.min(*t);
                    Some(*a)
                }
                _ => None,
            })
            .take(MAX_ADDRS)
            .collect();
        if !addrs.is_empty() {
            return Some(Resp::Answer(addrs, ttl.min(MAX_TTL_S)));
        }
        match recs.iter().find_map(|r| match r {
            (o, Rec::Cname(t), ttl) if *o == cur => Some((t.clone(), *ttl)),
            _ => None,
        }) {
            Some((t, tt)) => {
                cur = t;
                ttl = ttl.min(tt);
            }
            None => break,
        }
    }
    Some(Resp::Fail(NetError::NotFound))
}

impl Net {
    pub(super) fn dns_start(&mut self, name: &str) -> Result<QueryId, NetError> {
        if self.queries.len() >= MAX_QUERIES {
            return Err(NetError::Busy);
        }
        let now = self.now;
        let mut q = Query { id: 0, name: String::new(), qid: 0, port: 0, start: now, attempts: 0, next_tx: now, done: None };
        if let Some(ip) = parse_ip(name) {
            q.done = Some((now, Ok(alloc::vec![ip])));
        } else {
            let n = clean_name(name).ok_or(NetError::NotFound)?;
            if let Some(c) = self.dns_cache.iter().find(|c| c.name == n && c.expires > now) {
                q.done = Some((now, Ok(c.addrs.clone())));
            } else {
                self.ready()?;
                if self.cfg.as_ref().is_none_or(|c| c.dns.is_empty()) {
                    return Err(NetError::Protocol("sin servidor DNS"));
                }
                q.qid = self.rand() as u16;
                // puertos efímeros 49152-65535, sin repetir uno en uso
                q.port = loop {
                    let p = 49152 + (self.rand() % 16384) as u16;
                    if !self.queries.iter().any(|o| o.port == p) {
                        break p;
                    }
                };
                q.name = n;
            }
        }
        self.next_query = self.next_query.wrapping_add(1);
        q.id = self.next_query;
        self.queries.push(q);
        if self.queries.last().is_some_and(|q| q.done.is_none()) {
            self.dns_send(self.queries.len() - 1);
        }
        Ok(QueryId(self.next_query))
    }

    pub(super) fn dns_result(&mut self, id: QueryId) -> Option<Result<Vec<Ipv4>, NetError>> {
        let Some(i) = self.queries.iter().position(|q| q.id == id.0) else { return Some(Err(NetError::Gone)) };
        if self.queries[i].done.is_none() {
            return None;
        }
        self.queries.swap_remove(i).done.map(|d| d.1)
    }

    /// Un intento: al servidor `attempts % n` (primero, segundo, primero…).
    fn dns_send(&mut self, i: usize) {
        let Some((me, servers)) = self.cfg.as_ref().map(|c| (c.ip, c.dns.clone())) else { return };
        let now = self.now;
        let q = &mut self.queries[i];
        let Some(&srv) = servers.get(q.attempts % servers.len().max(1)) else { return };
        q.attempts += 1;
        q.next_tx = now + RETRY_MS;
        let u = udp_build(me, srv, q.port, 53, &build_query(q.qid, &q.name));
        let _ = self.send_ip(srv, P_UDP, 64, &u);
    }

    pub(super) fn dns_tick(&mut self) {
        let now = self.now;
        for i in 0..self.queries.len() {
            let q = &mut self.queries[i];
            if q.done.is_some() {
                continue;
            }
            if now.saturating_sub(q.start) >= DNS_TIMEOUT_MS {
                q.done = Some((now, Err(NetError::Timeout)));
            } else if now >= q.next_tx {
                self.dns_send(i);
            }
        }
        self.queries.retain(|q| q.done.as_ref().is_none_or(|d| now.saturating_sub(d.0) < KEEP_DONE_MS));
        self.dns_cache.retain(|c| c.expires > now);
    }

    pub(super) fn dns_input(&mut self, from: Ipv4, port: u16, data: &[u8]) {
        let Some(servers) = self.cfg.as_ref().map(|c| c.dns.clone()) else { return };
        let now = self.now;
        let Some(i) = self.queries.iter().position(|q| q.done.is_none() && q.port == port) else { return };
        let q = &mut self.queries[i];
        // solo vale quien ya ha recibido la consulta
        if !servers.iter().take(q.attempts).any(|s| *s == from) {
            return;
        }
        match parse_response(data, q.qid, &q.name) {
            Resp::Ignore => {}
            Resp::Fail(e) => q.done = Some((now, Err(e))),
            Resp::Answer(addrs, ttl) => {
                q.done = Some((now, Ok(addrs.clone())));
                if ttl > 0 {
                    let name = q.name.clone();
                    self.dns_cache.retain(|c| c.name != name);
                    if self.dns_cache.len() >= CACHE_MAX {
                        self.dns_cache.remove(0);
                    }
                    self.dns_cache.push(CacheEntry { name, addrs, expires: now + ttl as u64 * 1000 });
                }
            }
        }
    }
}
