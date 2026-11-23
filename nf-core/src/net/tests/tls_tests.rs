// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! HTTPS sobre la pila: el «servidor» simulado devuelve un saludo TLS grabado con OpenSSL
//! (`tls/testdata`), que solo cuadra con el ClientHello de valores fijos de `set_tls_fijo`.
use super::*;
use crate::net::http::{HttpEvent, HttpGet};
use crate::x509::{RootStore, Time};
use alloc::sync::Arc;

const CA: &[u8] = include_bytes!("../../tls/testdata/ca.der");
const VUELO: &[u8] = include_bytes!("../../tls/testdata/vuelo_aes128_rsa.bin");
const VUELO_OTRO: &[u8] = include_bytes!("../../tls/testdata/vuelo_otro_host.bin");

fn lan_tls(vuelo: &[u8]) -> Lan {
    let mut l = Lan::up();
    l.router.names.push(("prueba.nf".to_string(), alloc::vec![SRV], 300));
    l.router.srv.listen_any(443, vuelo);
    let mut r = RootStore::new();
    r.add_der(CA).unwrap();
    l.net.set_tls_env(Arc::new(r), Time { year: 2026, month: 11, day: 23, hour: 12, min: 0, sec: 0 });
    l.net.set_tls_fijo([7; 32], [9; 32], [5; 32]);
    l
}

fn https(l: &mut Lan, url: &str, follow: bool, insecure: bool) -> Vec<HttpEvent> {
    let mut g = HttpGet::start_tls(&mut l.net, url, false, follow, insecure).unwrap();
    let mut ev = Vec::new();
    for _ in 0..3000 {
        l.run(20);
        ev.extend(g.poll(&mut l.net, l.now));
        if matches!(ev.last(), Some(HttpEvent::Done | HttpEvent::Error(_))) {
            return ev;
        }
    }
    panic!("no terminó: {ev:?}");
}

#[test]
fn https_con_saludo_grabado() {
    let mut l = lan_tls(VUELO);
    let ev = https(&mut l, "https://prueba.nf/", false, false);
    let HttpEvent::Secure(i) = &ev[0] else { panic!("{ev:?}") };
    assert_eq!((i.version.as_str(), i.suite.as_str(), i.group.as_str()), ("TLS 1.3", "TLS_AES_128_GCM_SHA256", "x25519"));
    assert_eq!(i.subject, "prueba.nf (NightFury Prueba)");
    assert_eq!(i.issuer, "NF TLS Raiz de Prueba (NightFury Prueba)");
    assert_eq!((i.expires, i.cert_error.clone()), ((2050, 1, 1), None));
    // El saludo grabado trae ya la respuesta y el cierre del servidor (antes de que mandemos nada): al
    // pedir se ve «cerrada». Aquí basta con que el saludo valga; el texto claro se lee abajo.
    // En la tabla de conexiones se ve como TLS.
    let mut l = lan_tls(VUELO);
    let s = l.net.tcp_connect_tls(SRV, 443, "prueba.nf", false).unwrap();
    assert_eq!(l.net.tcp_state(s), TcpState::Connecting);
    for _ in 0..100 {
        l.run(20);
    }
    assert_eq!(l.net.tcp_state(s), TcpState::PeerClosed);
    let mut buf = [0u8; 64];
    let n = l.net.tcp_recv(s, &mut buf).unwrap();
    assert!(buf[..n].starts_with(b"HTTP/1.0 200 ok"), "{:?}", &buf[..n]);
    assert_eq!(l.net.connections().iter().find(|c| c.id == s.0).map(|c| c.proto), Some(Proto::Tls));
}

#[test]
fn https_certificado_malo_se_niega_y_con_k_sigue() {
    let mut l = lan_tls(VUELO_OTRO);
    let ev = https(&mut l, "https://prueba.nf/", false, false);
    assert_eq!(ev, [HttpEvent::Error(NetError::Tls(crate::x509::CertError::NombreNoCoincide.mensaje().to_string()))]);
    let mut l = lan_tls(VUELO_OTRO);
    let ev = https(&mut l, "https://prueba.nf/", false, true);
    let HttpEvent::Secure(i) = &ev[0] else { panic!("{ev:?}") };
    assert_eq!(i.cert_error.as_deref(), Some(crate::x509::CertError::NombreNoCoincide.mensaje()));
}

#[test]
fn https_servidor_que_cierra_en_el_saludo() {
    let mut l = lan_tls(b"");
    let ev = https(&mut l, "https://prueba.nf/", false, false);
    assert!(matches!(&ev[..], [HttpEvent::Error(NetError::Tls(m))] if m.contains("durante el saludo")), "{ev:?}");
}

#[test]
fn https_redireccion_de_http_a_https() {
    let mut l = lan_tls(VUELO);
    l.router.srv.listen(80, b"HTTP/1.1 301 Moved\r\nLocation: https://prueba.nf/x\r\nContent-Length: 0\r\n\r\n", true, false);
    let ev = https(&mut l, "http://prueba.nf/", true, false);
    assert!(matches!(ev[0], HttpEvent::Secure(_)), "{ev:?}");
}

#[test]
fn analisis_de_urls_https() {
    let u = http::parse_url("HTTPS://Prueba.nf/a?b").unwrap();
    assert_eq!((u.https, u.port, u.path.as_str()), (true, 443, "/a?b"));
    assert_eq!(http::parse_url("https://x:8443/").unwrap().port, 8443);
    assert!(!http::parse_url("http://x").unwrap().https);
}
