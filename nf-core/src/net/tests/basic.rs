// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Paquetes, DHCP, ARP e ICMP.
use super::*;

fn types(l: &Lan) -> Vec<u8> {
    l.router.log_dhcp.iter().map(|d| dhcp_type(&d.2)).collect()
}

// ---------- paquetes ----------

#[test]
fn suma_de_internet_rfc1071() {
    // ejemplo del RFC 1071: la suma de 00 01 f2 03 f4 f5 f6 f7 es ddf2
    let d = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
    assert_eq!(finish(csum(&d, 0)), !0xddf2u16);
    // longitud impar
    assert_eq!(finish(csum(&[0x12, 0x34, 0x56], 0)), !(0x1234u16 + 0x5600));
}

#[test]
fn ip_ida_y_vuelta_y_rechazos() {
    let p = ip_build([1, 2, 3, 4], [5, 6, 7, 8], P_UDP, 64, 9, b"datos");
    let ip = parse_packet(&p).unwrap();
    assert_eq!((ip.src, ip.dst, ip.proto, ip.ttl, ip.payload), ([1, 2, 3, 4], [5, 6, 7, 8], P_UDP, 64, &b"datos"[..]));
    // relleno de Ethernet detrás: se recorta por la longitud total
    let mut pad = p.clone();
    pad.extend_from_slice(&[0; 20]);
    assert_eq!(parse_packet(&pad).unwrap().payload, b"datos");
    let mut m = p.clone();
    m[0] = 0x65; // versión 6
    assert!(parse_packet(&m).is_none());
    let mut m = p.clone();
    m[0] = 0x44; // IHL 4
    assert!(parse_packet(&m).is_none());
    let mut m = p.clone();
    m[10] ^= 1; // suma
    assert!(parse_packet(&m).is_none());
    assert!(parse_packet(&p[..p.len() - 1]).is_none()); // más corto que la longitud total
    assert!(parse_packet(&p[..10]).is_none());
    let mut m = p.clone();
    m[6] = 0x20; // MF: fragmento
    let c = finish(csum(&{ let mut h = m[..20].to_vec(); h[10] = 0; h[11] = 0; h }, 0));
    m[10..12].copy_from_slice(&c.to_be_bytes());
    assert!(parse_packet(&m).is_none());
}

#[test]
fn udp_icmp_arp_formatos() {
    let u = udp_build([1, 1, 1, 1], [2, 2, 2, 2], 1000, 53, b"x");
    let p = ip_build([1, 1, 1, 1], [2, 2, 2, 2], P_UDP, 64, 1, &u);
    let ip = parse_packet(&p).unwrap();
    assert_eq!(parse_udp(&ip), Some((1000, 53, &b"x"[..])));
    let mut bad = p.clone();
    bad[28] ^= 0xFF; // dato
    assert!(parse_udp(&parse_packet(&bad).unwrap()).is_none());
    let i = icmp_build(8, 0, [0, 1, 0, 2], b"ab");
    assert_eq!(parse_icmp(&i), Some((8, 0, [0, 1, 0, 2], &b"ab"[..])));
    let mut bad = i.clone();
    bad[8] ^= 1;
    assert!(parse_icmp(&bad).is_none());
    let a = arp_build(1, HOST_MAC, OFFER, [0; 6], R_IP);
    let a = parse_arp(&a).unwrap();
    assert_eq!((a.op, a.sha, a.spa, a.tpa), (1, HOST_MAC, OFFER, R_IP));
    assert!(parse_arp(&[0; 27]).is_none());
}

#[test]
fn texto_de_ip() {
    assert_eq!(parse_ip("192.168.1.1"), Some([192, 168, 1, 1]));
    assert_eq!(parse_ip("0.0.0.0"), Some([0; 4]));
    for m in ["", "1.2.3", "1.2.3.4.5", "256.1.1.1", "01.2.3.4", "1..3.4", "a.b.c.d", " 1.2.3.4", "1.2.3.4 ", "1.2.3.-4", "1.2.3.4444", "+1.2.3.4"] {
        assert_eq!(parse_ip(m), None, "{m:?}");
    }
    assert_eq!(ip_text([10, 0, 0, 1]), "10.0.0.1");
    assert_eq!(mac_text(HOST_MAC), "02-00-00-00-00-01");
}

// ---------- estados y DHCP ----------

#[test]
fn sin_tarjeta_y_sin_cable() {
    let mut n = Net::new([1; 32]);
    assert_eq!(n.status().state, State::NoAdapter);
    n.poll(100);
    assert_eq!(n.ping(R_IP, 8, 64), Err(NetError::NoAdapter));
    assert_eq!(n.tcp_connect(R_IP, 80), Err(NetError::NoAdapter));

    let mut l = Lan::new();
    l.wire.borrow_mut().link = None;
    l.run(100);
    assert_eq!(l.state(), State::NoCable);
    assert_eq!(l.net.status().speed_mbps, 0);
    assert!(l.wire.borrow().tx.is_empty() && l.up_log.is_empty(), "sin cable no se envía nada");
    assert_eq!(l.net.ping(R_IP, 8, 64), Err(NetError::NoCable));
    l.wire.borrow_mut().link = Some(LINK);
    l.run(500);
    assert_eq!(l.state(), State::Connected);
}

#[test]
fn dhcp_completo() {
    let mut l = Lan::new();
    l.run(500);
    let s = l.net.status();
    assert_eq!(s.state, State::Connected);
    assert_eq!(s.adapter, "Prueba");
    assert_eq!((s.mac, s.speed_mbps, s.full_duplex), (HOST_MAC, 1000, true));
    assert_eq!((s.ip, s.mask, s.gateway), (Some(OFFER), Some([255, 255, 255, 0]), Some(R_IP)));
    assert_eq!((s.dns, s.dhcp_server, s.lease_s), (alloc::vec![R_IP, DNS2], Some(R_IP), 3600));
    assert_eq!(s.lease_since_ms, 1000);
    assert_eq!(types(&l), [1, 3]);
    let (disc, req) = (&l.router.log_dhcp[0], &l.router.log_dhcp[1]);
    assert_eq!(disc.1, BCAST_IP);
    assert!(disc.2.len() >= 300);
    assert_eq!(dhcp_opt(&disc.2, 12).unwrap(), b"NightFury");
    assert_eq!(dhcp_opt(&req.2, 12).unwrap(), b"NightFury");
    assert_eq!(dhcp_opt(&disc.2, 55).unwrap(), [1, 3, 6, 51, 54, 58, 59]);
    assert_eq!(disc.2[10] & 0x80, 0x80, "pide respuesta por difusión");
    assert_eq!(disc.2[4..8], req.2[4..8], "mismo xid en toda la conversación");
    assert_ne!(disc.2[4..8], [0; 4]);
    assert_eq!(dhcp_opt(&req.2, 50).unwrap(), OFFER);
    assert_eq!(dhcp_opt(&req.2, 54).unwrap(), R_IP);
    // otro secreto, otro xid
    let mut l2 = Lan::new();
    l2.net = Net::new([9; 32]);
    l2.net.attach(Box::new(Dev(l2.wire.clone())));
    l2.run(500);
    assert_ne!(l.router.log_dhcp[0].2[4..8], l2.router.log_dhcp[0].2[4..8]);
}

#[test]
fn dhcp_reintentos_y_sin_internet() {
    let mut l = Lan::new();
    l.router.dhcp_on = false;
    l.run(14_000);
    assert_eq!(l.state(), State::Configuring);
    assert_eq!(l.net.ping(R_IP, 8, 64), Err(NetError::NoAddress));
    l.run(1_000);
    assert_eq!(l.state(), State::NoInternet, "a los DHCP_TIMEOUT_MS");
    let t: Vec<u64> = l.router.log_dhcp.iter().map(|d| d.0).collect();
    assert_eq!(t, [1000, 3000, 7000, 15000], "espera creciente 2, 4, 8 s");
    assert!(types(&l).iter().all(|&x| x == 1));
    l.router.dhcp_on = true;
    l.run(16_000); // siguiente intento a los 16 s
    assert_eq!(l.state(), State::Connected);
}

#[test]
fn dhcp_pierde_el_primer_discover() {
    let mut l = Lan::new();
    l.router.dhcp_drop = 1;
    l.run(1000);
    assert_eq!(l.state(), State::Configuring);
    l.run(2500);
    assert_eq!(l.state(), State::Connected);
}

#[test]
fn dhcp_ignora_respuestas_ajenas() {
    let mut l = Lan::new();
    l.router.dhcp_on = false;
    l.run(100);
    let disc = l.router.log_dhcp[0].2.clone();
    let offer = |l: &mut Lan, req: &[u8]| {
        let m = l.router.dhcp_reply(req, 2);
        let u = udp_build(R_IP, BCAST_IP, 67, 68, &m);
        l.router.ip_frame(R_IP, BCAST_IP, P_UDP, &u)
    };
    let mut bad = disc.clone();
    bad[7] ^= 1; // xid
    let f = offer(&mut l, &bad);
    l.inject(f);
    let mut bad = disc.clone();
    bad[28] ^= 1; // chaddr
    let f = offer(&mut l, &bad);
    l.inject(f);
    assert_eq!(types(&l), [1], "no hay REQUEST: no era para nosotros");
    let f = offer(&mut l, &disc);
    l.inject(f);
    assert_eq!(types(&l), [1, 3]);
}

#[test]
fn dhcp_renueva_a_la_mitad() {
    let mut l = Lan::new();
    l.router.lease = 120;
    l.run(500);
    assert_eq!(l.state(), State::Connected);
    l.run(58_000);
    assert_eq!(types(&l), [1, 3], "todavía no");
    l.run(3_000); // T1 = 60 s tras la concesión
    assert_eq!(types(&l), [1, 3, 3]);
    let r = &l.router.log_dhcp[2];
    assert_eq!(r.1, R_IP, "unicast al servidor");
    assert_eq!(r.2[12..16], OFFER, "ciaddr");
    assert!(dhcp_opt(&r.2, 54).is_none() && dhcp_opt(&r.2, 50).is_none());
    assert_eq!(l.state(), State::Connected);
    let s = l.net.status();
    assert!(s.lease_since_ms >= 61_000 && s.lease_since_ms < 62_000, "{}", s.lease_since_ms);
    // la siguiente renovación cuenta desde la nueva concesión
    l.run(55_000);
    assert_eq!(types(&l).len(), 3);
    l.run(10_000);
    assert_eq!(types(&l).len(), 4);
}

#[test]
fn dhcp_usa_t1_y_t2_del_servidor() {
    let mut l = Lan::new();
    l.router.lease = 1000;
    l.router.t12 = Some((100, 200));
    l.run(500);
    l.run(98_000);
    assert_eq!(types(&l).len(), 2);
    l.run(3_000);
    assert_eq!(types(&l).len(), 3);
}

#[test]
fn dhcp_reenlaza_y_caduca() {
    let mut l = Lan::new();
    l.router.lease = 60; // T1 30 s, T2 52,5 s
    l.run(500);
    l.router.dhcp_on = false;
    l.run(32_000);
    let n = types(&l).len();
    assert!(n >= 3, "renovación en unicast");
    l.run(21_000);
    let rb = l.router.log_dhcp.iter().filter(|d| d.1 == BCAST_IP && d.2[12..16] == OFFER).count();
    assert!(rb >= 1, "a T2 reenlaza por difusión");
    assert_eq!(l.state(), State::Connected);
    l.run(9_000); // a los 60 s caduca
    let s = l.net.status();
    assert_eq!((s.ip, s.state), (None, State::Configuring));
    assert_eq!(l.net.ping(R_IP, 8, 64), Err(NetError::NoAddress));
    l.router.dhcp_on = true;
    l.run(4_000); // reintento a los 6 s de caducar
    assert_eq!(l.state(), State::Connected);
}

#[test]
fn dhcp_renew_suelta_y_pide_de_nuevo() {
    let mut l = Lan::up();
    l.net.renew();
    l.run(500);
    assert_eq!(types(&l), [1, 3, 7, 1, 3]);
    let rel = &l.router.log_dhcp[2];
    assert_eq!((rel.1, rel.2[12..16].to_vec(), dhcp_opt(&rel.2, 54).unwrap()), (R_IP, OFFER.to_vec(), R_IP.to_vec()));
    assert_eq!(l.state(), State::Connected);
    // sin cable no hace nada
    l.wire.borrow_mut().link = None;
    l.run(100);
    l.net.renew();
    assert_eq!(l.state(), State::NoCable);
}

#[test]
fn dhcp_nak_en_la_renovacion() {
    let mut l = Lan::new();
    l.router.lease = 120;
    l.run(500);
    l.router.nak = true;
    l.run(62_000);
    assert_eq!(types(&l), [1, 3, 3, 1, 3], "NAK: se empieza de nuevo");
    assert_eq!(l.state(), State::Connected);
}

#[test]
fn cable_fuera_pierde_la_direccion() {
    let mut l = Lan::up();
    l.wire.borrow_mut().link = None;
    l.run(100);
    let s = l.net.status();
    assert_eq!((s.state, s.ip, s.gateway, s.speed_mbps), (State::NoCable, None, None, 0));
    l.run(60_000);
    assert_eq!(types(&l), [1, 3], "sin cable no hay DHCP");
    l.wire.borrow_mut().link = Some(LINK);
    l.run(500);
    assert_eq!(l.state(), State::Connected);
    assert_eq!(types(&l), [1, 3, 1, 3], "DHCP desde cero");
}

// ---------- ARP ----------

#[test]
fn arp_puerta_de_enlace_sin_respuesta() {
    let mut l = Lan::new();
    l.router.arp_on = false;
    l.run(3_000);
    assert_eq!(l.state(), State::Configuring, "con dirección, pero la puerta aún no ha contestado");
    l.run(2_500);
    let s = l.net.status();
    assert_eq!((s.state, s.ip), (State::NoInternet, Some(OFFER)));
    assert_eq!(l.router.arp_log.iter().filter(|a| **a == R_IP).count(), 4);
    l.router.arp_on = true;
    l.run(5_000);
    assert_eq!(l.state(), State::Connected);
}

#[test]
fn arp_contesta_por_nuestra_ip() {
    let mut l = Lan::up();
    let n = l.up_log.len();
    let who = |tpa| eth(BCAST_MAC, R_MAC, ETH_ARP, &arp_build(1, R_MAC, R_IP, [0; 6], tpa));
    l.inject(who([192, 168, 1, 99]));
    assert_eq!(l.up_log.len(), n, "no es nuestra IP");
    l.inject(who(OFFER));
    let r = l.up_log.last().unwrap();
    assert_eq!((rd16(r, 12), r[..6].to_vec()), (ETH_ARP, R_MAC.to_vec()));
    let a = parse_arp(&r[14..]).unwrap();
    assert_eq!((a.op, a.sha, a.spa), (2, HOST_MAC, OFFER));
}

#[test]
fn arp_cache_caduca_y_se_refresca() {
    let mut l = Lan::up();
    assert_eq!(l.router.arp_log.iter().filter(|a| **a == R_IP).count(), 1);
    l.run(30_000);
    assert_eq!(l.router.arp_log.iter().filter(|a| **a == R_IP).count(), 1);
    l.run(32_000);
    assert_eq!(l.router.arp_log.iter().filter(|a| **a == R_IP).count(), 2);
    assert_eq!(l.state(), State::Connected);
}

#[test]
fn arp_cola_y_reintentos() {
    let mut l = Lan::up();
    let vecino = [192, 168, 1, 77];
    l.router.alive.push(vecino);
    l.router.arp_skip = 2;
    let r = l.ping(vecino, 16, 64).unwrap();
    assert!(r.rtt_ms >= 2000, "el eco esperó en la cola: {}", r.rtt_ms);
    assert_eq!(l.router.arp_log.iter().filter(|a| **a == vecino).count(), 3);
}

#[test]
fn arp_sin_respuesta_descarta_la_cola() {
    let mut l = Lan::up();
    let vecino = [192, 168, 1, 99];
    assert_eq!(l.ping(vecino, 16, 64), Err(NetError::Timeout));
    assert_eq!(l.router.arp_log.iter().filter(|a| **a == vecino).count(), 4);
    l.run(10_000);
    assert_eq!(l.router.arp_log.iter().filter(|a| **a == vecino).count(), 4);
    assert!(l.net.arp.held.is_empty() && l.net.arp.reqs.is_empty());
}

#[test]
fn arp_no_aprende_de_cualquiera() {
    let mut l = Lan::up();
    let n = l.net.arp.cache.len();
    // respuesta que nadie pidió, a otro destino
    l.inject(eth(HOST_MAC, [2, 0, 0, 0, 9, 9], ETH_ARP, &arp_build(2, [2, 0, 0, 0, 9, 9], [192, 168, 1, 200], HOST_MAC, [192, 168, 1, 201])));
    assert_eq!(l.net.arp.cache.len(), n);
    // quien dice ser nosotros
    l.inject(eth(BCAST_MAC, [2, 0, 0, 0, 9, 9], ETH_ARP, &arp_build(1, [2, 0, 0, 0, 9, 9], OFFER, [0; 6], R_IP)));
    // y quien manda a la puerta de enlace una MAC de difusión
    l.inject(eth(HOST_MAC, R_MAC, ETH_ARP, &arp_build(2, BCAST_MAC, R_IP, HOST_MAC, OFFER)));
    assert_eq!(l.ping(SRV, 8, 64).unwrap().from, SRV, "la puerta de enlace sigue siendo la buena");
}

#[test]
fn arp_no_cambia_la_puerta_ni_llena_la_cache() {
    let mut l = Lan::up();
    let malo = [2, 0, 0, 0, 6, 6];
    // petición a nosotros que dice que la puerta de enlace tiene otra MAC
    l.inject(eth(BCAST_MAC, malo, ETH_ARP, &arp_build(1, malo, R_IP, [0; 6], OFFER)));
    let gw = l.net.arp.cache.iter().find(|e| e.ip == R_IP).map(|e| e.mac);
    assert_eq!(gw, Some(R_MAC), "la MAC de la puerta no cambia");
    // muchas IP desconocidas preguntando por nosotros: se contesta, pero no se guardan
    let n = l.net.arp.cache.len();
    for i in 0..40u8 {
        l.inject(eth(BCAST_MAC, malo, ETH_ARP, &arp_build(1, malo, [192, 168, 1, 100 + i], [0; 6], OFFER)));
    }
    assert_eq!(l.net.arp.cache.len(), n);
    assert_eq!(l.ping(SRV, 8, 64).unwrap().from, SRV);
}

#[test]
fn ip_descarta_origenes_imposibles() {
    let mut l = Lan::up();
    let n = l.up_log.len();
    let eco = |l: &mut Lan, src: Ipv4| l.router.ip_frame(src, OFFER, P_ICMP, &icmp_build(8, 0, [0, 1, 0, 1], &[]));
    for src in [OFFER, [0, 0, 0, 0], [255, 255, 255, 255], [192, 168, 1, 255], [224, 0, 0, 1]] {
        let f = eco(&mut l, src);
        l.inject(f);
    }
    assert_eq!(l.up_log.len(), n, "no se contesta a ningún eco con origen imposible");
    let f = eco(&mut l, R_IP);
    l.inject(f);
    assert!(l.up_log.len() > n, "a un origen normal sí");
}

// ---------- ICMP ----------

#[test]
fn ping_con_respuesta() {
    let mut l = Lan::up();
    let r = l.ping(SRV, 32, 64).unwrap();
    assert_eq!((r.from, r.bytes, r.ttl), (SRV, 32, 64));
    assert!(r.rtt_ms <= 40);
    let id = l.net.ping(SRV, 0, 64).unwrap();
    l.run(100);
    assert_eq!(l.net.ping_poll(id).unwrap().unwrap().bytes, 0);
    assert_eq!(l.net.ping_poll(id), Some(Err(NetError::Gone)), "el identificador se libera");
    // el eco que enviamos es correcto: id, secuencia y datos
    let f = l.up_log.iter().rev().find(|f| rd16(f, 12) == ETH_IP && f[14 + 9] == P_ICMP).unwrap();
    assert_eq!(f[..6], R_MAC, "fuera de la subred va a la puerta de enlace");
    assert_eq!(f[14 + 8], 64);
}

#[test]
fn ping_sin_respuesta_da_timeout() {
    let mut l = Lan::up();
    let id = l.net.ping([10, 9, 9, 9], 8, 64).unwrap();
    l.run(3_900);
    assert_eq!(l.net.ping_poll(id), None);
    l.run(200);
    assert_eq!(l.net.ping_poll(id), Some(Err(NetError::Timeout)));
}

#[test]
fn ping_ttl_caducado_e_inaccesible() {
    let mut l = Lan::up();
    assert_eq!(l.ping(SRV, 8, 1), Err(NetError::TtlExpired(R_IP)));
    l.router.unreach.push([8, 8, 8, 8]);
    assert_eq!(l.ping([8, 8, 8, 8], 8, 64), Err(NetError::Unreachable(R_IP)));
}

#[test]
fn ping_errores_y_limites() {
    let mut l = Lan::up();
    assert!(matches!(l.net.ping(SRV, 1473, 64), Err(NetError::Protocol(_))));
    assert!(l.net.ping(SRV, 1472, 64).is_ok());
    for _ in 0..MAX_PINGS - 1 {
        l.net.ping([10, 9, 9, 9], 8, 64).unwrap();
    }
    assert_eq!(l.net.ping([10, 9, 9, 9], 8, 64), Err(NetError::Busy));
    l.run(5_000);
    assert!(l.net.ping([10, 9, 9, 9], 8, 64).is_err(), "los resultados sin recoger siguen ocupando sitio");
    l.run(60_000);
    assert!(l.net.ping([10, 9, 9, 9], 8, 64).is_ok(), "pero caducan");
    let r = l.ping(OFFER, 4, 64).unwrap();
    assert_eq!((r.from, r.rtt_ms), (OFFER, 0), "a uno mismo");
}

#[test]
fn ping_ignora_respuestas_que_no_casan() {
    let mut l = Lan::up();
    let id = l.net.ping([10, 9, 9, 9], 8, 64).unwrap();
    l.run(100);
    let req = l.up_log.iter().rev().find(|f| rd16(f, 12) == ETH_IP && f[14 + 9] == P_ICMP).unwrap().clone();
    let ip = parse_packet(&req[14..]).unwrap();
    let (_, _, rest, data) = parse_icmp(ip.payload).unwrap();
    let echo = |l: &mut Lan, src: Ipv4, rest: [u8; 4]| l.router.ip_frame(src, OFFER, P_ICMP, &icmp_build(0, 0, rest, data));
    let f = echo(&mut l, [10, 9, 9, 9], [rest[0], rest[1], rest[2], rest[3] ^ 1]); // otra secuencia
    l.inject(f);
    let f = echo(&mut l, [10, 9, 9, 9], [rest[0] ^ 1, rest[1], rest[2], rest[3]]); // otro identificador
    l.inject(f);
    let f = echo(&mut l, [10, 9, 9, 8], rest); // otro origen
    l.inject(f);
    let mut bad = icmp_build(0, 0, rest, data);
    bad[2] ^= 1; // suma
    let f = l.router.ip_frame([10, 9, 9, 9], OFFER, P_ICMP, &bad);
    l.inject(f);
    assert_eq!(l.net.ping_poll(id), None);
    // un error ICMP que cita otro origen o otro eco tampoco vale
    let inner = ip_build([1, 2, 3, 4], [10, 9, 9, 9], P_ICMP, 64, 1, &icmp_build(8, 0, rest, data));
    let f = l.router.ip_frame(R_IP, OFFER, P_ICMP, &icmp_build(3, 1, [0; 4], &inner));
    l.inject(f);
    assert_eq!(l.net.ping_poll(id), None);
    let f = echo(&mut l, [10, 9, 9, 9], rest);
    l.inject(f);
    assert!(l.net.ping_poll(id).unwrap().is_ok());
}

#[test]
fn contesta_a_pings_que_nos_hacen() {
    let mut l = Lan::up();
    let echo = icmp_build(8, 0, [0, 5, 0, 9], b"hola");
    let f = l.router.ip_frame(R_IP, OFFER, P_ICMP, &echo);
    l.inject(f);
    let r = l.up_log.last().unwrap();
    let ip = parse_packet(&r[14..]).unwrap();
    assert_eq!((ip.src, ip.dst, ip.proto), (OFFER, R_IP, P_ICMP));
    assert_eq!(parse_icmp(ip.payload), Some((0, 0, [0, 5, 0, 9], &b"hola"[..])));
    let n = l.up_log.len();
    // a la difusión, con la suma mal o fragmentado: nada
    let f = l.router.ip_frame(R_IP, [192, 168, 1, 255], P_ICMP, &echo);
    l.inject(f);
    let mut bad = echo.clone();
    bad[3] ^= 1;
    let f = l.router.ip_frame(R_IP, OFFER, P_ICMP, &bad);
    l.inject(f);
    let mut frag = f_with_mf(&mut l, &echo);
    l.inject(core::mem::take(&mut frag));
    assert_eq!(l.up_log.len(), n);
}

fn f_with_mf(l: &mut Lan, icmp: &[u8]) -> Vec<u8> {
    let mut f = l.router.ip_frame(R_IP, OFFER, P_ICMP, icmp);
    f[14 + 6] = 0x20;
    f[14 + 10] = 0;
    f[14 + 11] = 0;
    let c = finish(csum(&f[14..34], 0));
    f[14 + 10..14 + 12].copy_from_slice(&c.to_be_bytes());
    f
}
