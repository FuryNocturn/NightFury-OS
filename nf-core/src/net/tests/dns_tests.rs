// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! DNS: consultas, caché, compresión, CNAME, respuestas falsas y entrada hostil.
use super::*;
use crate::net::dns::{parse_response, read_name, Resp};

fn rr(owner: &[u8], ty: u16, ttl: u32, rdata: &[u8]) -> Vec<u8> {
    let mut v = owner.to_vec();
    v.extend_from_slice(&ty.to_be_bytes());
    v.extend_from_slice(&[0, 1]);
    v.extend_from_slice(&ttl.to_be_bytes());
    v.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
    v.extend_from_slice(rdata);
    v
}

fn lan_con(names: &[(&str, &[Ipv4], u32)]) -> Lan {
    let mut l = Lan::up();
    for (n, ips, ttl) in names {
        l.router.names.push((n.to_string(), ips.to_vec(), *ttl));
    }
    l
}

fn hook(l: &mut Lan, f: impl FnMut(Ipv4, &[u8], u16) -> Vec<(Ipv4, u16, Vec<u8>)> + 'static) {
    l.router.dns_hook = Some(Box::new(f));
}

#[test]
fn consulta_bien_formada_y_cache_por_ttl() {
    let mut l = lan_con(&[("example.org", &[SRV], 60)]);
    assert_eq!(l.resolve("example.org").unwrap(), [SRV]);
    assert_eq!(l.router.log_dns.len(), 1);
    let (dst, port, q) = l.router.log_dns[0].clone();
    assert_eq!(dst, R_IP);
    assert!((49152..=65535).contains(&port));
    assert_eq!(q[2..12], [1, 0, 0, 1, 0, 0, 0, 0, 0, 0], "RD y una pregunta");
    assert_eq!(qname(&q), "example.org");
    assert_eq!(q[q.len() - 5..], [0, 0, 1, 0, 1]);
    // desde la caché, sin red, y sin distinguir mayúsculas ni el punto final
    assert_eq!(l.resolve("EXAMPLE.org.").unwrap(), [SRV]);
    assert_eq!(l.router.log_dns.len(), 1);
    l.run(61_000);
    assert_eq!(l.resolve("example.org").unwrap(), [SRV]);
    assert_eq!(l.router.log_dns.len(), 2, "el TTL caducó");
    assert_ne!(l.router.log_dns[1].1, port, "puerto de origen distinto en cada consulta");
    assert_ne!(l.router.log_dns[1].2[..2], q[..2], "identificador distinto");
}

#[test]
fn ttl_cero_no_se_guarda() {
    let mut l = lan_con(&[("cero.test", &[SRV], 0)]);
    l.resolve("cero.test").unwrap();
    l.resolve("cero.test").unwrap();
    assert_eq!(l.router.log_dns.len(), 2);
}

#[test]
fn un_texto_que_es_ip_se_devuelve_tal_cual() {
    let mut l = Lan::new();
    l.router.dhcp_on = false;
    l.run(100);
    assert_eq!(l.resolve("1.2.3.4").unwrap(), [[1, 2, 3, 4]], "incluso sin dirección");
    assert_eq!(l.resolve("example.org"), Err(NetError::NoAddress));
    assert!(l.router.log_dns.is_empty());
    let id = l.net.resolve("9.9.9.9").unwrap();
    assert_eq!(l.net.resolve_poll(id), Some(Ok(alloc::vec![[9, 9, 9, 9]])));
    assert_eq!(l.net.resolve_poll(id), Some(Err(NetError::Gone)));
}

#[test]
fn nxdomain_y_sin_direcciones() {
    let mut l = lan_con(&[("hay.test", &[], 60)]);
    assert_eq!(l.resolve("no-existe.test"), Err(NetError::NotFound));
    assert_eq!(l.resolve("hay.test"), Err(NetError::NotFound), "respuesta sin registros A");
    for n in ["", "a..b", "a b.test", "ñ.test", &"a".repeat(64), &["abcdefghi"; 30].join(".")] {
        assert_eq!(l.resolve(n), Err(NetError::NotFound), "{n:?}");
    }
    assert_eq!(l.router.log_dns.len(), 2);
}

#[test]
fn cname_y_nombres_comprimidos() {
    let mut l = lan_con(&[]);
    hook(&mut l, |from, q, sp| {
        assert_eq!(qname(q), "www.example.org");
        // «cdn» + puntero a «example.org» de la pregunta (desplazamiento 16)
        let cname = rr(&[0xC0, 0x0C], 5, 300, &[3, b'c', b'd', b'n', 0xC0, 0x10]);
        // el propietario de los A es un puntero al dato del CNAME (desplazamiento 12 + 21 + 12)
        let owner = [0xC0, 12 + 17 + 4 + 12];
        let a1 = rr(&owner, 1, 120, &[1, 2, 3, 4]);
        let a2 = rr(&owner, 1, 90, &[1, 2, 3, 5]);
        alloc::vec![(from, sp, dns_msg(q, 0, &[cname, a1, a2]))]
    });
    assert_eq!(l.resolve("www.example.org").unwrap(), [[1, 2, 3, 4], [1, 2, 3, 5]]);
    l.run(89_000);
    assert_eq!(l.router.log_dns.len(), 1);
    l.resolve("www.example.org").unwrap();
    assert_eq!(l.router.log_dns.len(), 1, "TTL mínimo de la cadena: 90 s");
    l.run(2_000);
    l.resolve("www.example.org").unwrap();
    assert_eq!(l.router.log_dns.len(), 2);
}

#[test]
fn respuestas_falsas_se_ignoran() {
    let mut l = lan_con(&[]);
    hook(&mut l, |from, q, sp| {
        let falso = |q: &[u8]| dns_msg(q, 0, &[rr_a(60, [6, 6, 6, 6])]);
        let mut id_malo = falso(q);
        id_malo[1] ^= 1;
        let mut otra_pregunta = falso(q);
        otra_pregunta[13] = b'X';
        let mut es_consulta = falso(q);
        es_consulta[2] &= 0x7F; // QR = 0
        let mut opcode = falso(q);
        opcode[2] |= 0x08;
        let servfail = dns_msg(q, 2, &[]);
        alloc::vec![
            (from, sp.wrapping_add(1), falso(q)), // puerto de destino que no es el nuestro
            ([192, 168, 1, 200], sp, falso(q)),   // de quien no es el servidor
            (DNS2, sp, falso(q)),                 // del segundo servidor, al que aún no se ha preguntado
            (from, sp, id_malo),
            (from, sp, otra_pregunta),
            (from, sp, es_consulta),
            (from, sp, opcode),
            (from, sp, servfail),
            (from, sp, alloc::vec![0; 5]),
            (from, sp, dns_msg(q, 0, &[rr_a(60, [7, 7, 7, 7])])),
        ]
    });
    assert_eq!(l.resolve("example.org").unwrap(), [[7, 7, 7, 7]]);
    // el falso no quedó en la caché
    assert_eq!(l.resolve("example.org").unwrap(), [[7, 7, 7, 7]]);
}

#[test]
fn pasa_al_segundo_servidor() {
    let mut l = lan_con(&[("example.org", &[SRV], 60)]);
    hook(&mut l, |from, q, sp| {
        if from == R_IP {
            return Vec::new();
        }
        alloc::vec![(from, sp, dns_msg(q, 0, &[rr_a(60, SRV)]))]
    });
    let t0 = l.now;
    assert_eq!(l.resolve("example.org").unwrap(), [SRV]);
    assert!(l.now - t0 >= 1_500 && l.now - t0 < 2_500);
    let dst: Vec<Ipv4> = l.router.log_dns.iter().map(|d| d.0).collect();
    assert_eq!(dst, [R_IP, DNS2]);
    assert_eq!(l.router.log_dns[0].2[..2], l.router.log_dns[1].2[..2], "mismo identificador al reintentar");
}

#[test]
fn servfail_del_primero_prueba_con_el_segundo() {
    let mut l = lan_con(&[]);
    hook(&mut l, |from, q, sp| alloc::vec![(from, sp, if from == R_IP { dns_msg(q, 2, &[]) } else { dns_msg(q, 0, &[rr_a(60, SRV)]) })]);
    assert_eq!(l.resolve("example.org").unwrap(), [SRV]);
}

#[test]
fn sin_respuesta_da_timeout() {
    let mut l = lan_con(&[]);
    hook(&mut l, |_, _, _| Vec::new());
    let t0 = l.now;
    assert_eq!(l.resolve("example.org"), Err(NetError::Timeout));
    assert!(l.now - t0 >= DNS_TIMEOUT_MS && l.now - t0 < DNS_TIMEOUT_MS + 100);
    let dst: Vec<Ipv4> = l.router.log_dns.iter().map(|d| d.0).collect();
    assert_eq!(dst, [R_IP, DNS2, R_IP, DNS2]);
}

#[test]
fn limites_de_consultas_y_de_cache() {
    let mut l = lan_con(&[]);
    hook(&mut l, |_, _, _| Vec::new());
    for i in 0..MAX_QUERIES {
        l.net.resolve(&alloc::format!("n{i}.test")).unwrap();
    }
    assert_eq!(l.net.resolve("otro.test"), Err(NetError::Busy));
    l.run(70_000);
    l.router.dns_hook = None;
    for i in 0..40 {
        l.router.names.push((alloc::format!("c{i}.test"), alloc::vec![SRV], 300));
        l.resolve(&alloc::format!("c{i}.test")).unwrap();
    }
    assert_eq!(l.net.dns_cache.len(), 32, "tope de la caché");
    // la más antigua se fue; la última sigue
    let n = l.router.log_dns.len();
    l.resolve("c39.test").unwrap();
    assert_eq!(l.router.log_dns.len(), n);
    l.resolve("c0.test").unwrap();
    assert_eq!(l.router.log_dns.len(), n + 1);
}

// ---------- entrada hostil, directamente sobre el lector ----------

fn query(name: &str) -> Vec<u8> {
    let mut q = alloc::vec![0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for l in name.split('.') {
        q.push(l.len() as u8);
        q.extend_from_slice(l.as_bytes());
    }
    q.extend_from_slice(&[0, 0, 1, 0, 1]);
    q
}

fn reply(name: &str, rrs: &[Vec<u8>]) -> Vec<u8> {
    dns_msg(&query(name), 0, rrs)
}

#[test]
fn lector_de_respuestas_resiste_basura() {
    let n = "a.test";
    let ok = reply(n, &[rr_a(10, [1, 1, 1, 1])]);
    assert_eq!(parse_response(&ok, 0x1234, n), Resp::Answer(alloc::vec![[1, 1, 1, 1]], 10));
    assert_eq!(parse_response(&ok, 0x1235, n), Resp::Ignore, "otro identificador");
    assert_eq!(parse_response(&ok, 0x1234, "b.test"), Resp::Ignore, "otra pregunta");
    // puntero hacia sí mismo y hacia delante
    for p in [[0xC0, 12 + 8 + 4], [0xC0, 0xFF], [0xFF, 0xFF]] {
        assert_eq!(parse_response(&reply(n, &[rr(&p, 1, 5, &[1, 1, 1, 1])]), 0x1234, n), Resp::Ignore, "{p:?}");
    }
    // etiqueta de 64, con bits reservados, y nombre que se pasa de 255
    let mut larga = alloc::vec![64];
    larga.extend_from_slice(&[b'x'; 64]);
    larga.push(0);
    assert_eq!(parse_response(&reply(n, &[rr(&larga, 1, 5, &[1, 1, 1, 1])]), 0x1234, n), Resp::Ignore);
    assert_eq!(parse_response(&reply(n, &[rr(&[0x80, 0], 1, 5, &[1, 1, 1, 1])]), 0x1234, n), Resp::Ignore);
    let mut enorme = Vec::new();
    for _ in 0..70 {
        enorme.extend_from_slice(&[3, b'a', b'b', b'c']);
    }
    enorme.push(0);
    assert_eq!(parse_response(&reply(n, &[rr(&enorme, 1, 5, &[1, 1, 1, 1])]), 0x1234, n), Resp::Ignore);
    assert!(read_name(&enorme, 0).is_none());
    // datos cortados, rdlen mayor que el mensaje, ancount enorme
    let mut cortado = ok.clone();
    cortado.truncate(cortado.len() - 2);
    assert_eq!(parse_response(&cortado, 0x1234, n), Resp::Ignore);
    let mut muchos = ok.clone();
    muchos[6] = 0xFF;
    muchos[7] = 0xFF;
    assert_eq!(parse_response(&muchos, 0x1234, n), Resp::Ignore);
    // un A de 3 bytes y uno de otra clase no cuentan
    let mut clase = rr_a(5, [2, 2, 2, 2]);
    clase[5] = 3;
    assert_eq!(parse_response(&reply(n, &[rr(&[0xC0, 0x0C], 1, 5, &[1, 2, 3]), clase]), 0x1234, n), Resp::Fail(NetError::NotFound));
    // un CNAME que da vueltas no cuelga
    let a = rr(&[0xC0, 0x0C], 5, 5, &[1, b'b', 0xC0, 0x0E]);
    let b = rr(&[1, b'b', 0xC0, 0x0E], 5, 5, &[0xC0, 0x0C]);
    assert_eq!(parse_response(&reply("a.test", &[a, b]), 0x1234, n), Resp::Fail(NetError::NotFound));
    // el A de otro nombre no vale
    let otro = rr(&[1, b'z', 0], 1, 5, &[9, 9, 9, 9]);
    assert_eq!(parse_response(&reply(n, &[otro]), 0x1234, n), Resp::Fail(NetError::NotFound));
    // más de 16 direcciones: se recortan; TTL enorme: se recorta a un día
    let todos: Vec<Vec<u8>> = (0..30).map(|i| rr_a(u32::MAX, [8, 8, 8, i])).collect();
    match parse_response(&reply(n, &todos), 0x1234, n) {
        Resp::Answer(a, ttl) => assert_eq!((a.len(), ttl), (16, 86_400)),
        r => panic!("{r:?}"),
    }
    assert_eq!(read_name(&[3, b'a', b'B', b'c', 0], 0), Some(("abc".to_string(), 5)), "en minúsculas");
}
