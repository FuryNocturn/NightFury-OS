// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Entrada hostil con semilla fija: tramas al azar y tramas válidas estropeadas contra una red
//! conectada y con operaciones en curso, segmentos TCP al azar contra un `Sock` y los analizadores.
//! Lo único que se exige es no entrar en pánico, no colgarse y poder recuperarse después.
use super::*;
use crate::net::dhcp;
use crate::net::dns::{parse_response, read_name};
use crate::net::http::{parse_head, parse_url, Chunked, HttpGet};
use crate::net::tcb::{Sock, TcpIn};

fn bytes(r: &mut Rng, max: usize) -> Vec<u8> {
    let n = r.below(max + 1);
    (0..n).map(|_| r.next() as u8).collect()
}

/// Estropea una trama válida: cambia bytes, la recorta o le añade cola.
fn estropear(r: &mut Rng, f: &[u8]) -> Vec<u8> {
    let mut v = f.to_vec();
    for _ in 0..1 + r.below(4) {
        if v.is_empty() {
            break;
        }
        let i = r.below(v.len());
        match r.below(4) {
            0 => v[i] = r.next() as u8,
            1 => v[i] ^= 1 << r.below(8),
            2 => v[i] = [0, 0xFF, 0x7F, 0x80][r.below(4)],
            _ => v.truncate(i),
        }
    }
    if r.below(8) == 0 {
        v.extend(bytes(r, 40));
    }
    v
}

/// Redirige una trama que salió del equipo para que parezca que llega a él.
fn de_vuelta(mut f: Vec<u8>) -> Vec<u8> {
    if f.len() >= 14 {
        f[0..6].copy_from_slice(&HOST_MAC);
        f[6..12].copy_from_slice(&R_MAC);
    }
    f
}

#[test]
fn tramas_al_azar_y_estropeadas_contra_una_red_viva() {
    for semilla in [1u64, 0xDEAD_BEEF, 77_777, 123_456_789] {
        let mut r = Rng(semilla);
        let mut l = Lan::up();
        l.router.names.push(("example.org".to_string(), alloc::vec![SRV], 300));
        l.router.srv.listen(80, b"HTTP/1.1 200 OK\r\nContent-Length: 20000\r\n\r\nxxxx", false, false);
        l.router.srv.listen(7, b"", false, true);
        let sock = l.connect(7);
        let _ = l.net.tcp_send(sock, &[1; 3000]);
        let _ = l.net.ping([8, 8, 8, 8], 56, 64);
        let _ = l.net.resolve("otro.test");
        let mut http = HttpGet::start(&mut l.net, "http://example.org/", false, true).unwrap();
        // tramas válidas de todo tipo para estropear
        l.run(200);
        let base: Vec<Vec<u8>> = l.up_log.iter().cloned().map(de_vuelta).collect();
        assert!(base.len() > 5, "{}", base.len());
        let mut buf = [0u8; 512];
        for i in 0..3000 {
            let f = match r.below(4) {
                0 => bytes(&mut r, 1600),
                1 => {
                    // cabecera Ethernet buena y resto al azar
                    let mut v = de_vuelta(bytes(&mut r, 120));
                    v.resize(v.len().max(14), 0);
                    v[12..14].copy_from_slice(&[[8, 0], [8, 6]][r.below(2)]);
                    v
                }
                _ => {
                    let b = &base[r.below(base.len())];
                    estropear(&mut r, b)
                }
            };
            l.inject(f[..f.len().min(1514)].to_vec()); // la tarjeta nunca entrega más de 1514
            if i % 50 == 0 {
                l.run(20 + r.below(300) as u64);
                let _ = l.net.tcp_recv(sock, &mut buf);
                let _ = http.poll(&mut l.net, l.now);
                let _ = l.net.status();
            }
        }
        http.cancel(&mut l.net);
        l.net.tcp_abort(sock);
        // recuperación: sin conexiones, la pila vuelve a funcionar tras renovar y esperar
        l.run(5_000);
        l.net.renew();
        l.run(130_000);
        assert_eq!(l.state(), State::Connected, "semilla {semilla}");
        assert!(l.ping(SRV, 32, 64).is_ok(), "semilla {semilla}: el ping vuelve a funcionar");
        assert!(l.resolve("example.org").is_ok(), "semilla {semilla}");
    }
}

#[test]
fn tramas_estropeadas_sin_configuracion() {
    // antes de tener dirección solo cuentan DHCP y ARP; nada debe romperse
    let mut r = Rng(4242);
    let mut l = Lan::new();
    l.router.dhcp_on = false;
    l.run(100);
    let base: Vec<Vec<u8>> = l.up_log.iter().cloned().map(de_vuelta).collect();
    assert!(!base.is_empty());
    for _ in 0..2000 {
        let i = r.below(base.len());
        let f = if r.below(3) == 0 { bytes(&mut r, 400) } else { estropear(&mut r, &base[i]) };
        l.inject(f);
    }
    l.run(10_000);
    l.router.dhcp_on = true;
    l.net.renew();
    l.run(40_000);
    assert_eq!(l.state(), State::Connected);
}

fn seg_azar<'a>(r: &mut Rng, base: (u32, u32), datos: &'a [u8]) -> TcpIn<'a> {
    let cerca = |r: &mut Rng, b: u32| match r.below(4) {
        0 => r.next() as u32,
        1 => b.wrapping_add(r.below(100_000) as u32),
        2 => b.wrapping_sub(r.below(100_000) as u32),
        _ => b.wrapping_add(r.below(3) as u32),
    };
    let n = r.below(datos.len() + 1);
    TcpIn { sp: 80, dp: 5000, seq: cerca(r, base.0), ack: cerca(r, base.1), flags: r.next() as u8 & 0x3F, win: r.next() as u16, mss: [None, Some(0), Some(536), Some(65535)][r.below(4)], data: &datos[..n] }
}

#[test]
fn sock_con_segmentos_y_tiempo_al_azar() {
    for semilla in 1..=40u64 {
        let mut r = Rng(semilla * 0x9E37_79B9);
        let (iss, irs) = (r.next() as u32, r.next() as u32);
        let mut s = Sock::new(1, [1, 1, 1, 1], 80, 5000, iss, 0);
        let datos: Vec<u8> = (0..3000).map(|i| i as u8).collect();
        let mut now = 0u64;
        let mut out = Vec::new();
        let mut buf = [0u8; 700];
        let _ = s.syn_seg();
        for paso in 0..2000 {
            now += r.below(400) as u64;
            out.clear();
            match r.below(10) {
                0 => {
                    s.queue(&datos[..r.below(datos.len())]);
                    s.output(now, &mut out);
                }
                1 => {
                    s.read(&mut buf);
                    s.window_update(&mut out);
                }
                2 => s.on_timer(now, &mut out),
                3 if paso > 1500 => s.close(),
                4 => {
                    // un SYN-ACK correcto de vez en cuando para llegar a estar abierto
                    let t = TcpIn { sp: 80, dp: 5000, seq: irs, ack: iss.wrapping_add(1), flags: SYN | ACK, win: 65535, mss: Some(1460), data: &[] };
                    s.on_segment(&t, now, &mut out);
                }
                _ => {
                    let t = seg_azar(&mut r, (irs.wrapping_add(1), iss.wrapping_add(1)), &datos);
                    s.on_segment(&t, now, &mut out);
                }
            }
            assert!(out.iter().all(|g| g.data.len() <= 1460), "semilla {semilla} paso {paso}");
            assert!(s.send_free() <= 65536 * 4);
        }
        // al final el tiempo solo lo mata o lo cierra, nunca lo deja colgado para siempre
        for _ in 0..200 {
            now += 60_000;
            out.clear();
            s.on_timer(now, &mut out);
        }
    }
}

#[test]
fn analizadores_con_basura() {
    let mut r = Rng(99);
    let mut c = Chunked::new();
    for i in 0..20_000 {
        let b = bytes(&mut r, 300);
        let ip = Ip { src: SRV, dst: OFFER, proto: [P_TCP, P_UDP, P_ICMP, 99][r.below(4)], ttl: 64, payload: &b };
        let _ = (parse_arp(&b), parse_packet(&b), parse_inner(&b), parse_icmp(&b), parse_udp(&ip), tcb::parse(&ip));
        let _ = parse_response(&b, r.next() as u16, "example.org");
        let _ = read_name(&b, r.below(b.len() + 1));
        let _ = dhcp::parse(&b, r.next() as u32, HOST_MAC);
        let _ = parse_head(&b);
        let _ = parse_url(&String::from_utf8_lossy(&b));
        if i % 500 == 0 {
            c = Chunked::new();
        }
        let _ = c.feed(&b, &mut Vec::new());
    }
}

#[test]
fn analizadores_con_mensajes_validos_estropeados() {
    let mut r = Rng(31337);
    let mut l = Lan::up();
    l.router.names.push(("example.org".to_string(), alloc::vec![SRV, [1, 2, 3, 4]], 300));
    let _ = l.resolve("example.org");
    // DHCP y DNS reales salidos del equipo (con la trama Ethernet, IP y UDP por delante)
    let mut cargas: Vec<Vec<u8>> = Vec::new();
    for f in l.up_log.iter().chain(l.router.log_dhcp.iter().map(|d| &d.2)) {
        if let Some(ip) = f.get(14..).and_then(parse_packet) {
            if let Some((_, _, d)) = parse_udp(&ip) {
                cargas.push(d.to_vec());
            }
        } else {
            cargas.push(f.clone());
        }
    }
    cargas.extend(l.router.log_dns.iter().map(|d| d.2.clone()));
    assert!(!cargas.is_empty());
    let resp = dns_msg(&cargas[cargas.len() - 1], 0, &[rr_a(60, SRV)]);
    cargas.push(resp);
    for _ in 0..30_000 {
        let i = r.below(cargas.len());
        let m = estropear(&mut r, &cargas[i]);
        let _ = parse_response(&m, r.next() as u16, "example.org");
        let _ = dhcp::parse(&m, r.next() as u32, HOST_MAC);
    }
    // chunked estropeado, por trozos
    let bueno = b"4;e=1\r\nWiki\r\n5\r\npedia\r\n0\r\nT: x\r\n\r\n";
    for _ in 0..5_000 {
        let m = estropear(&mut r, bueno);
        let mut c = Chunked::new();
        let mut i = 0;
        while i < m.len() {
            let n = (1 + r.below(8)).min(m.len() - i);
            if c.feed(&m[i..i + n], &mut Vec::new()).is_err() {
                break;
            }
            i += n;
        }
    }
    let cab = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nX: y";
    for _ in 0..5_000 {
        let _ = parse_head(&estropear(&mut r, cab));
    }
}
