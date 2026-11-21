// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! v0.12.1: sockets UDP, puertos TCP a la escucha y lista de conexiones, contra la LAN simulada.
//! El «equipo de la LAN» (`PEER`) y el de fuera (`FAR`) se simulan inyectando tramas a mano.
use super::*;

const PEER: Ipv4 = [192, 168, 1, 77];
const FAR: Ipv4 = [8, 8, 8, 8];
const SUBNET_BCAST: Ipv4 = [192, 168, 1, 255];

fn lan() -> Lan {
    let mut l = Lan::up();
    l.router.alive.push(PEER);
    l
}

/// Datagramas UDP enviados por el equipo (sin DHCP ni DNS): (destino, MAC destino, puerto origen,
/// puerto destino, datos).
fn udp_out(l: &Lan) -> Vec<(Ipv4, [u8; 6], u16, u16, Vec<u8>)> {
    l.up_log
        .iter()
        .filter_map(|f| {
            if f.len() < 34 || rd16(f, 12) != ETH_IP {
                return None;
            }
            let ip = parse_packet(&f[14..])?;
            let (sp, dp, d) = parse_udp(&ip).filter(|_| ip.proto == P_UDP)?;
            if dp == 67 || dp == 53 {
                return None;
            }
            Some((ip.dst, f[..6].try_into().unwrap(), sp, dp, d.to_vec()))
        })
        .collect()
}

fn udp_in(l: &mut Lan, from: Ipv4, sp: u16, dst: Ipv4, dp: u16, data: &[u8]) {
    let u = udp_build(from, dst, sp, dp, data);
    let f = l.router.ip_frame(from, dst, P_UDP, &u);
    l.inject(f);
}

/// Segmentos TCP enviados por el equipo: (destino, puerto origen, puerto destino, banderas, seq, ack, datos).
type Out = (Ipv4, u16, u16, u8, u32, u32, Vec<u8>);
fn tcp_out(l: &Lan) -> Vec<Out> {
    l.up_log
        .iter()
        .filter_map(|f| {
            if f.len() < 34 || rd16(f, 12) != ETH_IP {
                return None;
            }
            let ip = parse_packet(&f[14..])?;
            let t = tcb::parse(&ip).filter(|_| ip.proto == P_TCP)?;
            Some((ip.dst, t.sp, t.dp, t.flags, t.seq, t.ack, t.data.to_vec()))
        })
        .collect()
}

/// Lo que el equipo ha enviado a `(dst, puerto destino)`.
fn to(l: &Lan, dst: Ipv4, dp: u16) -> Vec<Out> {
    tcp_out(l).into_iter().filter(|o| o.0 == dst && o.2 == dp).collect()
}

fn seg_in(l: &mut Lan, from: Ipv4, sp: u16, dp: u16, seq: u32, ack: u32, flags: u8, data: &[u8]) {
    let s = Seg { seq, ack, flags, win: 65535, mss: if flags & SYN != 0 { Some(1460) } else { None }, data: data.to_vec() };
    let b = tcb::build(from, OFFER, sp, dp, &s);
    let f = l.router.ip_frame(from, OFFER, P_TCP, &b);
    l.inject(f);
}

/// Saludo de tres pasos del equipo `from:sp` al puerto `dp`; devuelve (seq del SYN-ACK, seq del otro tras el SYN)
/// o `None` si el equipo no contestó con SYN-ACK. Con `acabar` a `false` se queda a medias.
fn saludo(l: &mut Lan, from: Ipv4, sp: u16, dp: u16, acabar: bool) -> Option<(u32, u32)> {
    seg_in(l, from, sp, dp, 1000, 0, SYN, &[]);
    let (_, _, _, fl, seq, ack, _) = to(l, from, sp).into_iter().rev().find(|o| o.3 & (SYN | RST) != 0)?;
    if fl != SYN | ACK || ack != 1001 {
        return None;
    }
    if acabar {
        seg_in(l, from, sp, dp, 1001, seq.wrapping_add(1), ACK, &[]);
    }
    Some((seq, 1001))
}

fn port_de(l: &Lan, id: UdpId) -> u16 {
    l.net.connections().iter().find(|c| c.proto == Proto::Udp && c.id == id.0).unwrap().local_port
}

#[test]
fn udp_ida_y_vuelta_con_un_equipo_de_la_lan() {
    let mut l = lan();
    let s = l.net.udp_bind(0).unwrap();
    let p = port_de(&l, s);
    assert!((49152..=65535).contains(&p));
    l.net.udp_send(s, PEER, 5000, b"hola").unwrap();
    l.run(100);
    let o = udp_out(&l);
    assert_eq!(o.len(), 1);
    assert_eq!((o[0].0, o[0].2, o[0].3, o[0].4.as_slice()), (PEER, p, 5000, &b"hola"[..]));
    udp_in(&mut l, PEER, 5000, OFFER, p, b"que tal");
    let d = l.net.udp_recv(s).unwrap().unwrap();
    assert_eq!((d.from, d.port, d.data.as_slice()), (PEER, 5000, &b"que tal"[..]));
    assert_eq!(l.net.udp_recv(s), Ok(None));
    let c = l.net.connections().into_iter().find(|c| c.proto == Proto::Udp).unwrap();
    assert_eq!((c.state, c.remote, c.rx, c.tx), ("—", None, 7, 4));
    l.net.udp_close(s);
    assert_eq!(l.net.udp_recv(s), Err(NetError::Gone));
    assert!(l.net.connections().is_empty());
}

#[test]
fn respuesta_de_fuera_solo_si_se_le_envio_antes() {
    let mut l = lan();
    let s = l.net.udp_bind(0).unwrap();
    let p = port_de(&l, s);
    udp_in(&mut l, FAR, 7777, OFFER, p, b"sin pedir");
    assert_eq!(l.net.udp_recv(s), Ok(None));
    l.net.udp_send(s, FAR, 7777, b"x").unwrap();
    l.run(100);
    // otro puerto u otra IP de fuera: no vale; el mismo (IP, puerto): sí
    udp_in(&mut l, FAR, 7778, OFFER, p, b"otro puerto");
    udp_in(&mut l, [1, 1, 1, 1], 7777, OFFER, p, b"otra ip");
    assert_eq!(l.net.udp_recv(s), Ok(None));
    udp_in(&mut l, FAR, 7777, OFFER, p, b"respuesta");
    assert_eq!(l.net.udp_recv(s).unwrap().unwrap().data, b"respuesta");
}

#[test]
fn efimero_no_recibe_de_la_lan_si_no_ha_enviado() {
    let mut l = lan();
    let s = l.net.udp_bind(0).unwrap();
    let p = port_de(&l, s);
    udp_in(&mut l, PEER, 9, OFFER, p, b"intruso");
    udp_in(&mut l, PEER, 9, BCAST_IP, p, b"difusion");
    assert_eq!(l.net.udp_recv(s), Ok(None));
}

#[test]
fn puerto_fijo_recibe_de_la_lan_y_por_difusion_pero_no_de_fuera() {
    let mut l = lan();
    let s = l.net.udp_bind(5000).unwrap();
    udp_in(&mut l, PEER, 9, OFFER, 5000, b"a");
    udp_in(&mut l, PEER, 9, BCAST_IP, 5000, b"b");
    udp_in(&mut l, PEER, 9, SUBNET_BCAST, 5000, b"c");
    udp_in(&mut l, FAR, 9, OFFER, 5000, b"fuera");
    let mut got = Vec::new();
    while let Some(d) = l.net.udp_recv(s).unwrap() {
        got.push(d.data[0]);
    }
    assert_eq!(got, b"abc");
}

#[test]
fn cola_llena_tira_los_que_sobran() {
    let mut l = lan();
    let s = l.net.udp_bind(5000).unwrap();
    for i in 0..UDP_QUEUE + 10 {
        udp_in(&mut l, PEER, 9, OFFER, 5000, &[i as u8]);
    }
    let mut n = 0;
    while let Some(d) = l.net.udp_recv(s).unwrap() {
        assert_eq!(d.data, [n as u8], "los primeros se quedan, en orden");
        n += 1;
    }
    assert_eq!(n, UDP_QUEUE);
}

#[test]
fn dns_sigue_funcionando_con_sockets_udp_y_el_puerto_53_llega_al_usuario() {
    let mut l = lan();
    l.router.names.push(("example.org".to_string(), alloc::vec![SRV], 300));
    let fijo = l.net.udp_bind(5000).unwrap();
    let ef = l.net.udp_bind(0).unwrap();
    assert_eq!(l.resolve("example.org").unwrap(), [SRV]);
    let dns_port = l.router.log_dns[0].1;
    assert!(dns_port != 5000 && dns_port != port_de(&l, ef), "la consulta no usa un puerto de un socket");
    // un datagrama desde el puerto 53 a un socket de usuario le llega a él
    udp_in(&mut l, R_IP, 53, OFFER, 5000, b"no es dns");
    assert_eq!(l.net.udp_recv(fijo).unwrap().unwrap().data, b"no es dns");
}

#[test]
fn el_puerto_de_una_consulta_dns_en_curso_no_se_da() {
    let mut l = lan();
    l.router.dns_hook = Some(Box::new(|_, _, _| Vec::new()));
    let _ = l.net.resolve("lento.test").unwrap();
    l.run(100);
    let p = l.router.log_dns[0].1;
    assert_eq!(l.net.udp_bind(p), Err(NetError::InUse));
    // y un datagrama a ese puerto va a la pila, no a un socket
    let s = l.net.udp_bind(5000).unwrap();
    udp_in(&mut l, R_IP, 53, OFFER, p, b"basura");
    assert_eq!(l.net.udp_recv(s), Ok(None));
}

#[test]
fn errores_de_udp() {
    let mut l = lan();
    assert_eq!(l.net.udp_bind(68), Err(NetError::InUse));
    let s = l.net.udp_bind(5000).unwrap();
    assert_eq!(l.net.udp_bind(5000), Err(NetError::InUse));
    assert_eq!(l.net.udp_send(s, [224, 0, 0, 251], 5353, b"x"), Err(NetError::Unreachable([224, 0, 0, 251])));
    assert_eq!(l.net.udp_send(s, PEER, 1, &[0; UDP_MAX + 1]), Err(NetError::Protocol("datagrama demasiado grande")));
    assert_eq!(l.net.udp_send(s, PEER, 1, &[0; UDP_MAX]), Ok(()));
    assert_eq!(l.net.udp_send(UdpId(999), PEER, 1, b"x"), Err(NetError::Gone));
    // difusión: a la MAC de difusión
    l.net.udp_send(s, BCAST_IP, 6000, b"hola a todos").unwrap();
    l.run(50);
    assert_eq!(udp_out(&l).iter().rev().find(|o| o.0 == BCAST_IP).unwrap().1, BCAST_MAC);
    for _ in 0..MAX_UDP - 1 {
        l.net.udp_bind(0).unwrap();
    }
    assert_eq!(l.net.udp_bind(0), Err(NetError::Busy));
    assert_eq!(l.net.udp_bind(6001), Err(NetError::Busy));
}

#[test]
fn sin_direccion_se_abre_pero_no_se_envia_y_el_socket_sobrevive() {
    let mut l = lan();
    let s = l.net.udp_bind(5000).unwrap();
    let ls = l.net.tcp_listen(8080).unwrap();
    l.router.dhcp_on = false;
    l.net.renew();
    l.run(100);
    assert_eq!(l.net.udp_send(s, PEER, 1, b"x"), Err(NetError::NoAddress));
    assert!(l.net.udp_bind(5001).is_ok());
    l.router.dhcp_on = true;
    l.run(40_000);
    assert_eq!(l.state(), State::Connected);
    assert_eq!(l.net.udp_send(s, PEER, 1, b"x"), Ok(()));
    assert_eq!(l.net.tcp_accept(ls), Ok(None));
}

#[test]
fn escuchar_y_aceptar_con_datos_en_los_dos_sentidos() {
    let mut l = lan();
    let ls = l.net.tcp_listen(8080).unwrap();
    assert_eq!(l.net.tcp_accept(ls), Ok(None));
    // a medias: el SYN-ACK sale, pero aún no hay nada que aceptar
    let (seq, _) = saludo(&mut l, PEER, 40000, 8080, false).expect("SYN-ACK");
    assert_eq!(l.net.tcp_accept(ls), Ok(None));
    seg_in(&mut l, PEER, 40000, 8080, 1001, seq.wrapping_add(1), ACK, &[]);
    let (s, ip, port) = l.net.tcp_accept(ls).unwrap().unwrap();
    assert_eq!((ip, port), (PEER, 40000));
    assert_eq!(l.net.tcp_state(s), TcpState::Open);
    assert_eq!(l.net.tcp_accept(ls), Ok(None));
    // el otro envía, nosotros recibimos y respondemos
    seg_in(&mut l, PEER, 40000, 8080, 1001, seq.wrapping_add(1), ACK | tcb::PSH, b"hola");
    let mut buf = [0u8; 64];
    assert_eq!(l.net.tcp_recv(s, &mut buf), Ok(4));
    assert_eq!(&buf[..4], b"hola");
    assert_eq!(l.net.tcp_send(s, b"adios!"), Ok(6));
    l.run(100);
    let datos: Vec<u8> = to(&l, PEER, 40000).into_iter().flat_map(|o| o.6).collect();
    assert_eq!(datos, b"adios!");
    let c = l.net.connections().into_iter().find(|c| c.proto == Proto::Tcp).unwrap();
    assert_eq!((c.id, c.local_port, c.remote, c.state, c.rx, c.tx), (s.0, 8080, Some((PEER, 40000)), "Abierta", 4, 6));
    // el otro cierra: «Cerrando» y aún se puede leer; luego reset: «Cerrada»
    seg_in(&mut l, PEER, 40000, 8080, 1005, seq.wrapping_add(7), FIN | ACK, &[]);
    assert_eq!(l.net.tcp_state(s), TcpState::PeerClosed);
    assert_eq!(l.net.connections().iter().find(|c| c.proto == Proto::Tcp).unwrap().state, "Cerrando");
    l.net.tcp_abort(s);
    assert!(l.net.connections().iter().all(|c| c.proto != Proto::Tcp));
}

#[test]
fn syn_de_fuera_de_la_subred_recibe_rst_y_a_un_puerto_sin_escuchar_tambien() {
    let mut l = lan();
    let ls = l.net.tcp_listen(8080).unwrap();
    assert!(saludo(&mut l, FAR, 40000, 8080, false).is_none());
    let o = to(&l, FAR, 40000);
    assert_eq!(o.len(), 1);
    assert_eq!(o[0].3, RST | ACK);
    assert_eq!(o[0].5, 1001);
    assert!(saludo(&mut l, PEER, 40001, 9999, false).is_none());
    assert_eq!(to(&l, PEER, 40001)[0].3, RST | ACK);
    assert_eq!(l.net.tcp_accept(ls), Ok(None));
}

#[test]
fn con_la_cola_llena_el_syn_se_ignora() {
    let mut l = lan();
    let ls = l.net.tcp_listen(8080).unwrap();
    for i in 0..LISTEN_BACKLOG as u16 {
        assert!(saludo(&mut l, PEER, 40000 + i, 8080, i % 2 == 0).is_some(), "{i}");
    }
    let p = 40000 + LISTEN_BACKLOG as u16;
    seg_in(&mut l, PEER, p, 8080, 1000, 0, SYN, &[]);
    l.run(100);
    assert!(to(&l, PEER, p).is_empty(), "ni SYN-ACK ni RST");
    // aceptar una libera un sitio
    assert!(l.net.tcp_accept(ls).unwrap().is_some());
    assert!(saludo(&mut l, PEER, p, 8080, false).is_some());
}

#[test]
fn dejar_de_escuchar_corta_las_de_la_cola() {
    let mut l = lan();
    let ls = l.net.tcp_listen(8080).unwrap();
    saludo(&mut l, PEER, 40000, 8080, true).unwrap();
    saludo(&mut l, PEER, 40001, 8080, false).unwrap();
    l.net.tcp_unlisten(ls);
    l.run(100);
    for p in [40000, 40001] {
        assert!(to(&l, PEER, p).iter().any(|o| o.3 & RST != 0), "RST a {p}");
    }
    assert_eq!(l.net.tcp_accept(ls), Err(NetError::Gone));
    assert!(l.net.connections().is_empty());
    assert!(l.net.tcp_listen(8080).is_ok());
}

#[test]
fn puertos_a_la_escucha_en_uso_y_tope() {
    let mut l = lan();
    assert_eq!(l.net.tcp_listen(0), Err(NetError::InUse));
    l.net.tcp_listen(8080).unwrap();
    assert_eq!(l.net.tcp_listen(8080), Err(NetError::InUse));
    for i in 1..MAX_LISTEN as u16 {
        l.net.tcp_listen(8080 + i).unwrap();
    }
    assert_eq!(l.net.tcp_listen(9000), Err(NetError::Busy));
}

#[test]
fn aceptar_con_max_socks_lleno_la_deja_en_la_cola() {
    let mut l = lan();
    let ls = l.net.tcp_listen(8080).unwrap();
    saludo(&mut l, PEER, 40000, 8080, true).unwrap();
    let mut socks = Vec::new();
    for _ in 0..MAX_SOCKS {
        socks.push(l.net.tcp_connect(SRV, 99).unwrap()); // el servidor de pruebas lo rechaza; el socket sigue sin liberar
    }
    assert_eq!(l.net.tcp_connect(SRV, 99), Err(NetError::Busy));
    l.run(100);
    assert_eq!(l.net.tcp_accept(ls), Ok(None));
    l.net.tcp_abort(socks[0]);
    assert!(l.net.tcp_accept(ls).unwrap().is_some());
}

#[test]
fn lista_de_conexiones_con_textos() {
    let mut l = lan();
    l.router.srv.listen(7, b"", false, true);
    let u = l.net.udp_bind(5000).unwrap();
    let ls = l.net.tcp_listen(8080).unwrap();
    let conectando = l.net.tcp_connect(SRV, 7).unwrap();
    let c = l.net.connections();
    assert_eq!(c.iter().find(|c| c.id == conectando.0 && c.proto == Proto::Tcp).unwrap().state, "Conectando");
    let abierta = l.connect(7);
    l.send_all(abierta, b"12345", 1000);
    assert_eq!(l.read_all(abierta, 5, 2000), b"12345");
    let c = l.net.connections();
    let t = c.iter().find(|c| c.id == abierta.0 && c.proto == Proto::Tcp).unwrap();
    assert_eq!((t.state, t.remote, t.local_port > 0, t.rx, t.tx), ("Abierta", Some((SRV, 7)), true, 5, 5));
    let e = c.iter().find(|c| c.proto == Proto::TcpListen).unwrap();
    assert_eq!((e.id, e.local_port, e.remote, e.state), (ls.0, 8080, None, "Escuchando"));
    let d = c.iter().find(|c| c.proto == Proto::Udp).unwrap();
    assert_eq!((d.id, d.local_port, d.remote, d.state), (u.0, 5000, None, "—"));
    // reset del otro: «Cerrada» mientras no se libere
    let rst = l.net.tcp_connect(SRV, 99).unwrap();
    l.run(200);
    assert_eq!(l.net.connections().iter().find(|c| c.id == rst.0 && c.proto == Proto::Tcp).unwrap().state, "Cerrada");
    l.net.tcp_close(rst);
    assert!(l.net.connections().iter().all(|c| !(c.id == rst.0 && c.proto == Proto::Tcp)));
}

#[test]
fn entrada_hostil_contra_sockets_y_puertos_a_la_escucha() {
    for semilla in [3u64, 99, 2026] {
        let mut r = Rng(semilla);
        let mut l = lan();
        let u = l.net.udp_bind(5000).unwrap();
        let e = l.net.udp_bind(0).unwrap();
        let _ = l.net.udp_send(e, PEER, 5000, b"x");
        let ls = l.net.tcp_listen(8080).unwrap();
        let mut aceptadas = Vec::new();
        for i in 0..3000 {
            let from = [PEER, FAR, [192, 168, 1, 1 + r.below(250) as u8], [r.next() as u8, 1, 2, 3]][r.below(4)];
            let mut f = if r.below(2) == 0 {
                let dst = [OFFER, BCAST_IP, SUBNET_BCAST][r.below(3)];
                let dp = [5000, port_de(&l, e), r.next() as u16, 53, 68][r.below(5)];
                let n = r.below(UDP_MAX + 1);
                let data: Vec<u8> = (0..n).map(|_| r.next() as u8).collect();
                let u = udp_build(from, dst, [53, 5000, r.next() as u16][r.below(3)], dp, &data);
                l.router.ip_frame(from, dst, P_UDP, &u)
            } else {
                let n = r.below(30);
                let data: Vec<u8> = (0..n).map(|_| r.next() as u8).collect();
                let s = Seg { seq: r.next() as u32 % 3000, ack: r.next() as u32 % 3000, flags: r.next() as u8 & 0x3F, win: r.next() as u16, mss: [None, Some(0), Some(1460)][r.below(3)], data };
                let b = tcb::build(from, OFFER, 40000 + r.below(20) as u16, [8080, 8080, 9][r.below(3)], &s);
                l.router.ip_frame(from, OFFER, P_TCP, &b)
            };
            if r.below(6) == 0 {
                let k = r.below(f.len());
                f[k] ^= 1 << r.below(8);
            }
            l.inject(f);
            if i % 40 == 0 {
                l.run(r.below(400) as u64);
                while let Ok(Some(d)) = l.net.udp_recv(u) {
                    assert!(d.data.len() <= UDP_MAX);
                }
                if let Ok(Some((s, _, _))) = l.net.tcp_accept(ls) {
                    aceptadas.push(s);
                }
            }
            assert!(l.net.connections().len() <= MAX_SOCKS + LISTEN_BACKLOG + 2 + 2, "semilla {semilla}");
        }
        for s in aceptadas {
            l.net.tcp_abort(s);
        }
        l.net.tcp_unlisten(ls);
        // sigue funcionando
        l.run(30_000);
        assert!(l.ping(SRV, 32, 64).is_ok(), "semilla {semilla}");
        l.net.udp_send(e, PEER, 5000, b"otra vez").unwrap();
    }
}
