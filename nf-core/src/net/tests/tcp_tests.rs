// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! TCP: conexión, datos, pérdidas, desorden, ventanas, cierres, RST, ICMP y el `Sock` por dentro.
use super::*;
use crate::net::tcb::{Sock, St, TcpIn, PSH};

const REQ: &[u8] = b"GET / HTTP/1.1\r\n\r\n";

pub fn pat(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 7 + i / 251) as u8).collect()
}

/// Puerto de origen de la primera conexión del PC.
fn cport(l: &Lan) -> u16 {
    l.host_tcp().iter().find(|t| t.2 & SYN != 0).unwrap().0
}

/// Ventana anunciada en la última trama TCP del PC.
fn last_win(l: &Lan) -> u16 {
    let f = l.up_log.iter().rev().find(|f| tcp_info(f).is_some()).unwrap();
    tcb::parse(&parse_packet(&f[14..]).unwrap()).unwrap().win
}

fn drop_data_every(n: u32) -> Box<dyn FnMut(&[u8]) -> bool> {
    let mut c = 0;
    Box::new(move |f| {
        if tcp_info(f).is_some_and(|t| t.3 > 0) {
            c += 1;
            c % n == 0
        } else {
            false
        }
    })
}

fn inject_srv(l: &mut Lan, sport: u16, dport: u16, seg: Seg) {
    let f = l.router.tcp_frame(sport, dport, &seg);
    l.inject(f);
}

#[test]
fn conecta_envia_recibe_y_cierra() {
    let mut l = Lan::up();
    l.router.srv.listen(7, b"", false, true);
    let s = l.connect(7);
    assert_eq!(l.net.tcp_state(s), TcpState::Open);
    let syn = l.up_log.iter().filter_map(|f| parse_packet(&f[14..]).and_then(|ip| tcb::parse(&ip).map(|t| (t.sp, t.dp, t.flags, t.mss, t.win, t.seq)))).find(|t| t.2 & SYN != 0).unwrap();
    assert!((49152..=65535).contains(&syn.0));
    assert_eq!((syn.1, syn.2, syn.3, syn.4), (7, SYN, Some(1460), 65535));
    assert_eq!(l.net.tcp_send(s, b"hola").unwrap(), 4);
    assert_eq!(l.read_all(s, 4, 2000), b"hola");
    l.net.tcp_close(s);
    assert_eq!(l.net.tcp_state(s), TcpState::Failed(NetError::Gone), "el identificador queda libre");
    l.run(100);
    assert!(l.router.srv.conns[0].peer_fin && l.router.srv.conns[0].fin_acked);
    assert_eq!(l.net.socks.len(), 1, "TIME-WAIT");
    l.run(4_500);
    assert!(l.net.socks.is_empty());
    // otra conexión: otro puerto y otro número de secuencia inicial
    let s2 = l.connect(7);
    assert_eq!(l.net.tcp_state(s2), TcpState::Open);
    let syns: Vec<_> = l.host_tcp().into_iter().filter(|t| t.2 & SYN != 0).collect();
    assert_eq!(syns.len(), 2);
    assert_ne!(syns[0].0, syns[1].0);
    assert_ne!(syns[0].4, syns[1].4);
}

#[test]
fn refused_si_el_puerto_esta_cerrado() {
    let mut l = Lan::up();
    let s = l.connect(81);
    assert_eq!(l.net.tcp_state(s), TcpState::Failed(NetError::Refused));
    assert_eq!(l.net.tcp_send(s, b"x"), Err(NetError::Refused));
    assert_eq!(l.net.tcp_recv(s, &mut [0; 4]), Err(NetError::Refused));
    l.net.tcp_close(s);
    assert!(l.net.socks.is_empty());
    assert_eq!(l.net.tcp_connect(SRV, 0), Err(NetError::Refused));
}

#[test]
fn timeout_al_conectar_con_syn_retransmitidos() {
    let mut l = Lan::up();
    l.router.srv.listen(7, b"", false, true);
    l.router.srv.ignore_syn = 100;
    let t0 = l.now;
    let s = l.connect(7);
    assert_eq!(l.net.tcp_state(s), TcpState::Failed(NetError::Timeout));
    assert!(l.now - t0 >= CONNECT_TIMEOUT_MS && l.now - t0 < CONNECT_TIMEOUT_MS + 100);
    let gaps: Vec<u64> = l.router.srv.syns.windows(2).map(|w| w[1] - w[0]).collect();
    assert_eq!(gaps, [1000, 2000, 4000, 8000], "RTO inicial de 1 s y doblando");
}

#[test]
fn syn_perdido_se_retransmite() {
    let mut l = Lan::up();
    l.router.srv.listen(7, b"", false, true);
    l.router.srv.ignore_syn = 1;
    let s = l.connect(7);
    assert_eq!(l.net.tcp_state(s), TcpState::Open);
    assert_eq!(l.router.srv.syns.len(), 2);
    assert_eq!(l.router.srv.syns[1] - l.router.srv.syns[0], 1000);
}

#[test]
fn descarga_con_perdidas() {
    let mut l = Lan::up();
    let data = pat(20_000);
    l.router.srv.listen(80, &data, true, false);
    l.drop_down = drop_data_every(7);
    let s = l.connect(80);
    l.send_all(s, REQ, 1000);
    let got = l.read_all(s, data.len(), 60_000);
    assert!(got == data, "recibidos {} bytes, distintos o incompletos", got.len());
    l.run(1000);
    assert_eq!(l.net.tcp_state(s), TcpState::PeerClosed);
    l.net.tcp_close(s);
    l.run(5000);
    assert!(l.net.socks.is_empty());
}

#[test]
fn descarga_con_desorden_y_perdidas() {
    let mut l = Lan::up();
    let data = pat(30_000);
    l.router.srv.listen(80, &data, true, false);
    l.reorder = true;
    l.drop_down = drop_data_every(11);
    let s = l.connect(80);
    l.send_all(s, REQ, 1000);
    let got = l.read_all(s, data.len(), 60_000);
    assert!(got == data, "recibidos {} bytes, distintos o incompletos", got.len());
}

#[test]
fn desorden_sin_perdidas_no_retransmite_de_mas() {
    let mut l = Lan::up();
    let data = pat(14_600);
    l.router.srv.listen(80, &data, true, false);
    l.reorder = true;
    let s = l.connect(80);
    l.send_all(s, REQ, 1000);
    let got = l.read_all(s, data.len(), 10_000);
    assert!(got == data);
    // los segmentos adelantados se guardaron: el servidor no tuvo que reenviar nada por tiempo
    let datos = l.up_log.iter().filter_map(|f| tcp_info(f)).filter(|t| t.3 > 0).count();
    assert_eq!(datos, 1, "solo la petición");
}

#[test]
fn subida_con_perdidas() {
    let mut l = Lan::up();
    l.router.srv.listen(9, b"", false, false);
    l.drop_up = drop_data_every(5);
    let s = l.connect(9);
    let data = pat(30_000);
    l.send_all(s, &data, 60_000);
    l.run(60_000);
    assert!(l.router.srv.conns[0].got == data, "el servidor recibió {} bytes", l.router.srv.conns[0].got.len());
    // se retransmitió con RTO, y lo adaptable bajó el mínimo de 1 s
    assert_eq!(l.net.tcp_state(s), TcpState::Open);
}

#[test]
fn retransmision_rapida_con_tres_acks_repetidos() {
    let mut l = Lan::up();
    l.router.srv.listen(9, b"", false, false);
    let s = l.connect(9);
    // se pierde solo la primera trama de datos grande: los siguientes provocan ACK repetidos
    let mut vistos = 0;
    l.drop_up = Box::new(move |f| {
        if tcp_info(f).is_some_and(|t| t.3 == 1460) {
            vistos += 1;
            vistos == 1
        } else {
            false
        }
    });
    let data = pat(8 * 1460);
    l.send_all(s, &data, 1000);
    l.run(100); // mucho antes de que venza el RTO
    let primero = l.host_tcp().iter().find(|t| t.3 == 1460).unwrap().4;
    let veces = l.host_tcp().iter().filter(|t| t.4 == primero && t.3 == 1460).count();
    assert_eq!(veces, 2, "retransmitida por los ACK repetidos, no por tiempo");
    l.run(30_000);
    assert!(l.router.srv.conns[0].got == data, "recibió {}", l.router.srv.conns[0].got.len());
}

#[test]
fn ventana_del_otro_se_respeta() {
    let mut l = Lan::up();
    l.router.srv.listen(9, b"", false, false);
    l.router.srv.win = 2920;
    let s = l.connect(9);
    l.drop_down = Box::new(|_| true); // no llega ningún ACK
    let data = pat(10_000);
    l.send_all(s, &data, 100);
    l.run(500);
    let enviados: usize = l.host_tcp().iter().filter(|t| t.2 & SYN == 0).map(|t| t.3).sum();
    assert!(enviados <= 2920 + 1460, "sin ACK no pasa de la ventana (más una retransmisión): {enviados}");
    assert_eq!(l.router.srv.conns[0].got.len(), 2920, "el servidor recibió exactamente su ventana");
    l.drop_down = Box::new(|_| false);
    l.run(20_000);
    assert!(l.router.srv.conns[0].got == data);
}

#[test]
fn ventana_cero_del_otro_sondea_y_reanuda() {
    let mut l = Lan::up();
    l.router.srv.listen(9, b"", false, false);
    l.router.srv.win = 0;
    let s = l.connect(9);
    l.send_all(s, &pat(5000), 100);
    l.run(5_000);
    assert!(l.router.srv.conns[0].got.is_empty());
    let datos: Vec<usize> = l.host_tcp().iter().filter(|t| t.3 > 0).map(|t| t.3).collect();
    assert!(!datos.is_empty() && datos.iter().all(|&n| n == 1), "solo sondas de un byte: {datos:?}");
    assert_eq!(l.net.tcp_state(s), TcpState::Open, "el otro está vivo: no se rinde");
    l.router.srv.win = 8192;
    for (p, c, sg) in l.router.srv.update() {
        inject_srv(&mut l, p, c, sg);
    }
    l.run(5_000);
    assert!(l.router.srv.conns[0].got == pat(5000));
}

#[test]
fn ventana_cero_para_siempre_acaba_en_timeout() {
    let mut l = Lan::up();
    l.router.srv.listen(9, b"", false, false);
    l.router.srv.win = 0;
    let s = l.connect(9);
    l.send_all(s, &pat(100), 100);
    l.run(600_000);
    assert_eq!(l.net.tcp_state(s), TcpState::Failed(NetError::Timeout), "no retiene el socket para siempre");
}

#[test]
fn nuestra_ventana_se_llena_y_se_reabre() {
    let mut l = Lan::up();
    let data = pat(120_000);
    l.router.srv.listen(80, &data, true, false);
    let s = l.connect(80);
    l.send_all(s, REQ, 1000);
    l.run(15_000); // sin leer nada
    assert_eq!(last_win(&l), 0, "búfer lleno: ventana cero");
    assert_eq!(l.net.socks[0].state(), TcpState::Open);
    let c = &l.router.srv.conns[0];
    assert!(c.out.len() == data.len());
    let got = l.read_all(s, data.len(), 60_000);
    assert!(got == data, "recibidos {}", got.len());
}

#[test]
fn el_otro_cierra_primero() {
    let mut l = Lan::up();
    l.router.srv.listen(80, b"hola", true, false);
    let s = l.connect(80);
    l.send_all(s, REQ, 1000);
    l.run(500);
    assert_eq!(l.net.tcp_state(s), TcpState::PeerClosed);
    assert_eq!(l.net.tcp_send(s, b"x").unwrap(), 1, "se puede seguir enviando tras el FIN del otro");
    assert_eq!(l.read_all(s, 4, 500), b"hola");
    l.net.tcp_close(s);
    l.run(500);
    assert!(l.net.socks.is_empty(), "LAST-ACK terminó sin TIME-WAIT");
    assert!(l.router.srv.conns[0].fin_acked);
}

#[test]
fn rst_en_conexion_abierta() {
    let mut l = Lan::up();
    l.router.srv.listen(7, b"", false, true);
    let s = l.connect(7);
    let (cp, iss) = (cport(&l), l.router.srv.conns[0].iss);
    let rst = |seq: u32| Seg { seq, ack: 0, flags: RST, win: 0, mss: None, data: Vec::new() };
    let n = l.up_log.len();
    inject_srv(&mut l, 7, cp, rst(iss.wrapping_add(1 + 1_000_000))); // fuera de la ventana
    assert_eq!((l.net.tcp_state(s), l.up_log.len()), (TcpState::Open, n));
    inject_srv(&mut l, 7, cp, rst(iss.wrapping_add(1 + 5))); // dentro, pero no exacto: ACK de desafío
    assert_eq!(l.net.tcp_state(s), TcpState::Open);
    assert_eq!(l.up_log.len(), n + 1);
    inject_srv(&mut l, 7, cp, rst(iss.wrapping_add(1)));
    assert_eq!(l.net.tcp_state(s), TcpState::Failed(NetError::Reset));
    assert_eq!(l.net.tcp_send(s, b"x"), Err(NetError::Reset));
}

#[test]
fn lo_recibido_antes_del_rst_se_puede_leer() {
    let mut l = Lan::up();
    l.router.srv.listen(7, b"", false, true);
    let s = l.connect(7);
    let (cp, iss) = (cport(&l), l.router.srv.conns[0].iss);
    let ack = l.router.srv.conns[0].rcv_nxt;
    inject_srv(&mut l, 7, cp, Seg { seq: iss.wrapping_add(1), ack, flags: ACK, win: 8192, mss: None, data: b"abc".to_vec() });
    inject_srv(&mut l, 7, cp, Seg { seq: iss.wrapping_add(4), ack: 0, flags: RST, win: 0, mss: None, data: Vec::new() });
    let mut b = [0; 8];
    assert_eq!(l.net.tcp_recv(s, &mut b), Ok(3));
    assert_eq!(l.net.tcp_recv(s, &mut b), Err(NetError::Reset));
}

#[test]
fn rst_a_segmentos_de_conexiones_que_no_existen() {
    let mut l = Lan::up();
    let n = l.up_log.len();
    // con ACK: RST con seq = ack
    inject_srv(&mut l, 80, 5555, Seg { seq: 100, ack: 777, flags: ACK, win: 100, mss: None, data: b"hola".to_vec() });
    let t = {
        let f = l.up_log.last().unwrap();
        tcb::parse(&parse_packet(&f[14..]).unwrap()).unwrap().flags_seq_ack()
    };
    assert_eq!(l.up_log.len(), n + 1);
    assert_eq!(t, (RST, 777, 0));
    // un SYN (sin ACK): RST|ACK con ack = seq + 1
    inject_srv(&mut l, 80, 5555, Seg { seq: 100, ack: 0, flags: SYN, win: 100, mss: None, data: Vec::new() });
    let t = {
        let f = l.up_log.last().unwrap();
        tcb::parse(&parse_packet(&f[14..]).unwrap()).unwrap().flags_seq_ack()
    };
    assert_eq!(t, (RST | ACK, 0, 101));
    // a un RST no se contesta
    let n = l.up_log.len();
    inject_srv(&mut l, 80, 5555, Seg { seq: 100, ack: 0, flags: RST, win: 0, mss: None, data: Vec::new() });
    assert_eq!(l.up_log.len(), n);
    // a otra dirección, tampoco
    let mut f = l.router.tcp_frame(80, 5555, &Seg { seq: 1, ack: 0, flags: SYN, win: 0, mss: None, data: Vec::new() });
    f[14 + 19] ^= 1;
    l.inject(f);
    assert_eq!(l.up_log.len(), n);
}

impl TcpIn<'_> {
    fn flags_seq_ack(&self) -> (u8, u32, u32) {
        (self.flags, self.seq, self.ack)
    }
}

#[test]
fn abort_envia_rst_y_libera() {
    let mut l = Lan::up();
    l.router.srv.listen(7, b"", false, true);
    let s = l.connect(7);
    l.net.tcp_abort(s);
    l.run(100);
    assert_eq!(l.router.srv.rst_seen, 1);
    assert_eq!(l.net.tcp_state(s), TcpState::Failed(NetError::Gone));
    assert!(l.net.socks.is_empty());
    // abortar mientras conecta no envía RST
    l.router.srv.ignore_syn = 1;
    let s = l.net.tcp_connect(SRV, 7).unwrap();
    l.run(100);
    l.net.tcp_abort(s);
    l.run(100);
    assert_eq!(l.router.srv.rst_seen, 1);
}

fn icmp_error_para_syn(l: &mut Lan, ty: u8, ajuste_seq: u32) {
    let f = l.up_log.iter().rev().find(|f| tcp_info(f).is_some_and(|t| t.2 & SYN != 0)).unwrap().clone();
    let mut inner = f[14..14 + 28].to_vec();
    let seq = rd32(&inner, 24).wrapping_add(ajuste_seq);
    inner[24..28].copy_from_slice(&seq.to_be_bytes());
    let m = l.router.ip_frame(R_IP, OFFER, P_ICMP, &icmp_build(ty, 1, [0; 4], &inner));
    l.inject(m);
}

#[test]
fn icmp_inaccesible_y_tiempo_excedido_cortan_la_conexion() {
    let mut l = Lan::up();
    l.router.srv.listen(7, b"", false, true);
    l.router.srv.ignore_syn = 100;
    let s = l.net.tcp_connect(SRV, 7).unwrap();
    l.run(100);
    icmp_error_para_syn(&mut l, 3, 1); // número de secuencia que no es el nuestro: se ignora
    assert_eq!(l.net.tcp_state(s), TcpState::Connecting);
    icmp_error_para_syn(&mut l, 3, 0);
    assert_eq!(l.net.tcp_state(s), TcpState::Failed(NetError::Unreachable(R_IP)));
    let s = l.net.tcp_connect(SRV, 7).unwrap();
    l.run(100);
    icmp_error_para_syn(&mut l, 11, 0);
    assert_eq!(l.net.tcp_state(s), TcpState::Failed(NetError::TtlExpired(R_IP)));
    // el router lo hace de verdad
    l.router.unreach.push(SRV);
    let s = l.connect(7);
    assert_eq!(l.net.tcp_state(s), TcpState::Failed(NetError::Unreachable(R_IP)));
}

#[test]
fn cable_fuera_corta_las_conexiones() {
    let mut l = Lan::up();
    l.router.srv.listen(7, b"", false, true);
    let s = l.connect(7);
    let s2 = l.net.tcp_connect(SRV, 7).unwrap();
    l.wire.borrow_mut().link = None;
    l.run(100);
    assert_eq!(l.net.tcp_state(s), TcpState::Failed(NetError::Reset));
    assert_eq!(l.net.tcp_state(s2), TcpState::Failed(NetError::Reset));
    l.net.tcp_close(s);
    l.net.tcp_close(s2);
    assert!(l.net.socks.is_empty());
}

#[test]
fn cierre_nuestro_sin_respuesta_caduca_y_datos_tardios_dan_rst() {
    let mut l = Lan::up();
    l.router.srv.listen(9, b"", false, false);
    let s = l.connect(9);
    let (cp, iss, ack) = (cport(&l), l.router.srv.conns[0].iss, l.router.srv.conns[0].rcv_nxt);
    l.net.tcp_close(s);
    l.run(200);
    assert_eq!(l.net.socks.len(), 1, "FIN-WAIT-2: el otro no ha cerrado");
    // datos que ya no podemos entregar: se corta
    let n = l.up_log.len();
    inject_srv(&mut l, 9, cp, Seg { seq: iss.wrapping_add(1), ack: ack.wrapping_add(1), flags: ACK, win: 100, mss: None, data: b"tarde".to_vec() });
    let f = l.up_log.last().unwrap();
    assert_eq!(l.up_log.len(), n + 1);
    assert_eq!(tcb::parse(&parse_packet(&f[14..]).unwrap()).unwrap().flags & RST, RST);
    assert!(l.net.socks.is_empty());
    // y si el otro no vuelve a hablar, FIN-WAIT-2 caduca
    let s = l.connect(9);
    l.net.tcp_close(s);
    l.run(500);
    assert_eq!(l.net.socks.len(), 1);
    l.run(31_000);
    assert!(l.net.socks.is_empty());
}

#[test]
fn tope_de_conexiones() {
    let mut l = Lan::up();
    l.router.srv.listen(7, b"", false, true);
    let ids: Vec<SockId> = (0..MAX_SOCKS).map(|_| l.net.tcp_connect(SRV, 7).unwrap()).collect();
    assert_eq!(l.net.tcp_connect(SRV, 7), Err(NetError::Busy));
    let mut puertos: Vec<u16> = l.net.socks.iter().map(|s| s.lport).collect();
    puertos.sort();
    puertos.dedup();
    assert_eq!(puertos.len(), MAX_SOCKS, "puertos de origen distintos");
    l.net.tcp_abort(ids[0]);
    assert!(l.net.tcp_connect(SRV, 7).is_ok());
    assert_eq!(l.net.tcp_send(SockId(9999), b"x"), Err(NetError::Gone));
    assert_eq!(l.net.tcp_recv(SockId(9999), &mut [0; 1]), Err(NetError::Gone));
}

#[test]
fn sin_ruta_a_otra_red_sin_puerta_de_enlace() {
    let mut l = Lan::up();
    l.net.cfg.as_mut().unwrap().gw = None;
    assert_eq!(l.net.tcp_connect(SRV, 7), Err(NetError::Unreachable(OFFER)));
    assert_eq!(l.net.ping(SRV, 8, 64), Err(NetError::Unreachable(OFFER)));
    assert!(l.net.tcp_connect([192, 168, 1, 77], 7).is_ok());
}

// ---------- el Sock por dentro ----------

const R: Ipv4 = [1, 1, 1, 1];

fn seg_in<'a>(seq: u32, ack: u32, flags: u8, win: u16, data: &'a [u8]) -> TcpIn<'a> {
    TcpIn { sp: 80, dp: 5000, seq, ack, flags, win, mss: Some(1460), data }
}

/// Un Sock ya establecido (SYN-ACK recibido), con números cercanos al desbordamiento.
fn establecido(iss: u32, irs: u32) -> Sock {
    let mut s = Sock::new(1, R, 80, 5000, iss, 0);
    let mut out = Vec::new();
    s.on_segment(&seg_in(irs, iss.wrapping_add(1), SYN | ACK, 65535, &[]), 10, &mut out);
    assert_eq!(s.state(), TcpState::Open);
    assert_eq!((out.len(), out[0].flags, out[0].ack), (1, ACK, irs.wrapping_add(1)));
    s
}

#[test]
fn sock_numeros_de_secuencia_con_desbordamiento() {
    let (iss, irs) = (0xFFFF_FFF0u32, 0xFFFF_FF00u32);
    let mut s = establecido(iss, irs);
    assert_eq!(s.queue(&pat(3000)), 3000);
    let mut out = Vec::new();
    assert!(s.output(20, &mut out));
    let v: Vec<(u32, usize)> = out.iter().map(|g| (g.seq, g.data.len())).collect();
    assert_eq!(v, [(iss.wrapping_add(1), 1460), (iss.wrapping_add(1461), 1460), (iss.wrapping_add(2921), 80)]);
    // se confirma por partes, cruzando el cero
    let mut o = Vec::new();
    s.on_segment(&seg_in(irs.wrapping_add(1), iss.wrapping_add(1461), ACK, 65535, &[]), 30, &mut o);
    s.on_segment(&seg_in(irs.wrapping_add(1), iss.wrapping_add(3001), ACK, 65535, &[]), 31, &mut o);
    assert!(o.is_empty(), "los ACK puros no se contestan");
    assert_eq!(s.send_free(), TCP_BUF, "todo confirmado");
    // datos recibidos cruzando el cero
    let datos = pat(300);
    s.on_segment(&seg_in(irs.wrapping_add(1), iss.wrapping_add(3001), ACK | PSH, 65535, &datos[..200]), 40, &mut o);
    assert_eq!(o.len(), 1);
    assert_eq!(o[0].ack, irs.wrapping_add(201));
    s.on_segment(&seg_in(irs.wrapping_add(201), iss.wrapping_add(3001), ACK, 65535, &datos[200..]), 41, &mut o);
    let mut b = [0; 400];
    assert_eq!(s.read(&mut b), 300);
    assert_eq!(&b[..300], &datos[..]);
    // cierre nuestro y confirmación
    s.close();
    o.clear();
    s.output(50, &mut o);
    assert_eq!((o[0].flags, o[0].seq), (FIN | ACK, iss.wrapping_add(3001)));
    s.on_segment(&seg_in(irs.wrapping_add(301), iss.wrapping_add(3002), ACK, 65535, &[]), 60, &mut o);
    assert_eq!(s.st, St::FinWait2);
}

#[test]
fn sock_segmentos_adelantados_duplicados_y_fuera_de_ventana() {
    let irs = 1000;
    let mut s = establecido(500, irs);
    let d = pat(400);
    let mut o = Vec::new();
    // adelantado: se guarda y se pide lo que falta
    s.on_segment(&seg_in(1101, 501, ACK, 100, &d[100..200]), 20, &mut o);
    assert_eq!((o.len(), o[0].ack), (1, 1001), "ACK repetido");
    let mut b = [0; 1000];
    assert_eq!(s.read(&mut b), 0);
    // lo mismo otra vez: no se duplica
    s.on_segment(&seg_in(1101, 501, ACK, 100, &d[100..200]), 21, &mut o);
    // fuera de la ventana (más allá del búfer): se descarta
    s.on_segment(&seg_in(1001 + TCP_BUF as u32 + 50, 501, ACK, 100, &d[..10]), 22, &mut o);
    // y lo que falta, solapando con lo guardado
    s.on_segment(&seg_in(1001, 501, ACK, 100, &d[..150]), 23, &mut o);
    assert_eq!(s.read(&mut b), 200);
    assert_eq!(&b[..200], &d[..200]);
    assert_eq!(o.last().unwrap().ack, 1201);
    // un duplicado viejo no cambia nada y se confirma
    s.on_segment(&seg_in(1001, 501, ACK, 100, &d[..100]), 24, &mut o);
    assert_eq!(o.last().unwrap().ack, 1201);
    assert_eq!(s.read(&mut b), 0);
    // solapado con lo ya recibido: solo entra lo nuevo
    s.on_segment(&seg_in(1101, 501, ACK, 100, &d[100..300]), 25, &mut o);
    assert_eq!(s.read(&mut b), 100);
    assert_eq!(&b[..100], &d[200..300]);
    // un FIN adelantado espera su turno
    s.on_segment(&seg_in(1401, 501, ACK | FIN, 100, &[]), 26, &mut o);
    assert_eq!(s.state(), TcpState::Open);
    s.on_segment(&seg_in(1301, 501, ACK, 100, &d[..100]), 27, &mut o);
    assert_eq!(s.state(), TcpState::PeerClosed);
    assert_eq!(o.last().unwrap().ack, 1402);
}

#[test]
fn sock_no_guarda_mas_de_lo_que_cabe() {
    let mut s = establecido(500, 1000);
    let mut o = Vec::new();
    let trozo = pat(1400);
    // 60 segmentos adelantados distintos: como mucho 16 y nunca más que el búfer
    for i in 0..60u32 {
        s.on_segment(&seg_in(1101 + i * 1400, 501, ACK, 100, &trozo), 20 + i as u64, &mut o);
    }
    assert!(o.iter().all(|g| g.ack == 1001));
    // al llegar el hueco entra lo guardado, nada más
    let hueco = pat(100);
    s.on_segment(&seg_in(1001, 501, ACK, 100, &hueco), 100, &mut o);
    let mut b = alloc::vec![0; TCP_BUF * 2];
    let n = s.read(&mut b);
    assert_eq!(n, 100 + 16 * 1400, "se guardaron 16 segmentos adelantados y no más");
}

#[test]
fn sock_rto_adaptable_y_abandono() {
    let mut s = establecido(500, 1000);
    // la confirmación del SYN tardó 10 ms: RTO al mínimo de 200 ms
    s.queue(b"abc");
    let mut o = Vec::new();
    s.output(100, &mut o);
    s.on_timer(299, &mut o);
    assert_eq!(o.len(), 1, "todavía no");
    s.on_timer(300, &mut o);
    assert_eq!(o.len(), 2);
    assert_eq!((o[1].seq, o[1].data.clone()), (501, b"abc".to_vec()));
    // el siguiente espera el doble
    s.on_timer(699, &mut o);
    assert_eq!(o.len(), 2);
    s.on_timer(700, &mut o);
    assert_eq!(o.len(), 3);
    // y a los 9 vencimientos se rinde con RST
    let mut t = 700;
    while s.state() == TcpState::Open {
        t += 40_000;
        s.on_timer(t, &mut o);
    }
    assert_eq!(s.state(), TcpState::Failed(NetError::Timeout));
    assert_eq!(o.last().unwrap().flags & RST, RST);
    assert_eq!(o.len(), 3 + 6 + 1, "8 retransmisiones y el RST");
}

#[test]
fn sock_tres_acks_repetidos_retransmiten() {
    let mut s = establecido(500, 1000);
    s.queue(&pat(5 * 1460));
    let mut o = Vec::new();
    s.output(20, &mut o);
    assert_eq!(o.len(), 5);
    o.clear();
    for i in 0..2 {
        s.on_segment(&seg_in(1001, 501, ACK, 65535, &[]), 21 + i, &mut o);
    }
    assert!(o.is_empty());
    s.on_segment(&seg_in(1001, 501, ACK, 65535, &[]), 30, &mut o);
    assert_eq!((o.len(), o[0].seq, o[0].data.len()), (1, 501, 1460));
}

#[test]
fn sock_rst_y_syn_en_estado_sincronizado() {
    let mut s = establecido(500, 1000);
    let mut o = Vec::new();
    s.on_segment(&seg_in(1001, 501, SYN | ACK, 65535, &[]), 20, &mut o);
    assert_eq!((o.len(), s.state()), (1, TcpState::Open), "SYN repetido: ACK de desafío, sigue abierta");
    // un ACK de algo que no enviamos
    let mut o = Vec::new();
    s.on_segment(&seg_in(1001, 9999, ACK, 65535, &[]), 21, &mut o);
    assert_eq!(o.len(), 1);
    // sin ACK, se ignora
    let mut o = Vec::new();
    s.on_segment(&seg_in(1001, 0, 0, 65535, b"x"), 22, &mut o);
    assert!(o.is_empty());
    let mut b = [0; 4];
    assert_eq!(s.read(&mut b), 0);
}

#[test]
fn sock_syn_ack_con_ack_incorrecto_da_rst() {
    let mut s = Sock::new(1, R, 80, 5000, 500, 0);
    let mut o = Vec::new();
    s.on_segment(&seg_in(1000, 777, SYN | ACK, 100, &[]), 5, &mut o);
    assert_eq!((o[0].flags, o[0].seq, s.state()), (RST, 777, TcpState::Connecting));
    // un RST con ACK que no es el nuestro no cuenta
    let mut o = Vec::new();
    s.on_segment(&seg_in(0, 777, RST | ACK, 0, &[]), 6, &mut o);
    assert_eq!(s.state(), TcpState::Connecting);
    s.on_segment(&seg_in(0, 501, RST | ACK, 0, &[]), 7, &mut o);
    assert_eq!(s.state(), TcpState::Failed(NetError::Refused));
}

#[test]
fn sock_mss_del_otro_y_ventana() {
    let mut s = Sock::new(1, R, 80, 5000, 500, 0);
    let mut o = Vec::new();
    let mut t = seg_in(1000, 501, SYN | ACK, 3000, &[]);
    t.mss = Some(536);
    s.on_segment(&t, 5, &mut o);
    s.queue(&pat(5000));
    o.clear();
    s.output(6, &mut o);
    let v: Vec<usize> = o.iter().map(|g| g.data.len()).collect();
    assert_eq!(v, [536, 536, 536, 536, 536, 320], "MSS 536 y ventana de 3000");
    // sin la opción MSS se supone 536; con un MSS de 0 no se atasca
    let mut s = Sock::new(1, R, 80, 5000, 500, 0);
    let mut t = seg_in(1000, 501, SYN | ACK, 3000, &[]);
    t.mss = Some(0);
    s.on_segment(&t, 5, &mut o);
    s.queue(&pat(100));
    o.clear();
    s.output(6, &mut o);
    assert!(!o.is_empty() && o.len() <= 100);
}
