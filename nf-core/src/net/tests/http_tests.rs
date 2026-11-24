// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! HTTP/1.1: URL, Content-Length, chunked, cierre, redirecciones, errores y cancelación.
use super::*;
use crate::net::http::{parse_head, parse_url, ChunkState, Chunked, HttpEvent, HttpGet, Url, MAX_HEAD, MAX_REDIRECTS};

fn lan() -> Lan {
    let mut l = Lan::up();
    l.router.names.push(("example.org".to_string(), alloc::vec![SRV], 300));
    l
}

/// Lanza la petición y deja correr el tiempo hasta `Done` o `Error` (o 120 s).
fn fetch(l: &mut Lan, url: &str, head: bool, follow: bool) -> Result<Vec<HttpEvent>, NetError> {
    let mut g = HttpGet::start(&mut l.net, url, head, follow)?;
    let mut ev = Vec::new();
    for _ in 0..6000 {
        l.run(20);
        ev.extend(g.poll(&mut l.net, l.now));
        if matches!(ev.last(), Some(HttpEvent::Done | HttpEvent::Error(_))) {
            assert!(g.poll(&mut l.net, l.now).is_empty(), "tras el final no hay más eventos");
            return Ok(ev);
        }
    }
    panic!("la petición no terminó: {ev:?}");
}

fn body(ev: &[HttpEvent]) -> Vec<u8> {
    ev.iter().filter_map(|e| if let HttpEvent::Body(b) = e { Some(b.clone()) } else { None }).flatten().collect()
}

fn status(ev: &[HttpEvent]) -> Option<u16> {
    ev.iter().find_map(|e| if let HttpEvent::Head { status, .. } = e { Some(*status) } else { None })
}

fn heads(ev: &[HttpEvent]) -> usize {
    ev.iter().filter(|e| matches!(e, HttpEvent::Head { .. })).count()
}

fn url(h: &str, p: u16, path: &str) -> Result<Url, NetError> {
    Ok(Url { https: false, host: h.to_string(), port: p, path: path.to_string() })
}

#[test]
fn analisis_de_urls() {
    assert_eq!(parse_url("http://example.org"), url("example.org", 80, "/"));
    assert_eq!(parse_url("example.org"), url("example.org", 80, "/"));
    assert_eq!(parse_url("http://example.org:8080/a/b?c=d#frag"), url("example.org", 8080, "/a/b?c=d"));
    assert_eq!(parse_url("HTTP://Example.org/x"), url("Example.org", 80, "/x"));
    assert_eq!(parse_url("http://example.org?x=1"), url("example.org", 80, "/?x=1"));
    assert_eq!(parse_url("http://example.org/ñ"), url("example.org", 80, "/%C3%B1"));
    assert_eq!(parse_url("localhost:3000/x"), url("localhost", 3000, "/x"));
    assert_eq!(parse_url("10.0.0.1/x#y"), url("10.0.0.1", 80, "/x"));
    assert_eq!(parse_url("host/redir?u=http://otra/"), url("host", 80, "/redir?u=http://otra/"));
    assert_eq!(parse_url("https://x"), Ok(Url { https: true, host: "x".to_string(), port: 443, path: "/".to_string() }));
    for m in ["", "ftp://x", "http://", "http://:80/", "http://a:0/", "http://a:65536/", "http://a:x/", "http://a:/", "http://u@a/", "http://[::1]/", "http://a b/", "http://a/\r\nX: y", " http://a", "http://.a/", "http://a|b/", "file:///etc/passwd"] {
        assert_eq!(parse_url(m), Err(NetError::BadUrl), "{m:?}");
    }
}

#[test]
fn content_length_y_peticion() {
    let mut l = lan();
    l.router.srv.listen(8080, b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhello", true, false);
    let ev = fetch(&mut l, "http://example.org:8080/x?y=1#f", false, true).unwrap();
    assert_eq!(ev.len(), 3, "{ev:?}");
    assert_eq!(
        ev[0],
        HttpEvent::Head { status: 200, reason: "OK".to_string(), headers: alloc::vec![("Content-Type".to_string(), "text/plain".to_string()), ("Content-Length".to_string(), "5".to_string())] }
    );
    assert_eq!((&ev[1], &ev[2]), (&HttpEvent::Body(b"hello".to_vec()), &HttpEvent::Done));
    let pedido = alloc::format!("GET /x?y=1 HTTP/1.1\r\nHost: example.org:8080\r\nUser-Agent: NightFury/{}\r\nAccept: */*\r\nAccept-Encoding: gzip, deflate\r\nConnection: close\r\n\r\n", crate::VERSION);
    assert_eq!(String::from_utf8_lossy(&l.router.srv.conns[0].got), pedido);
    l.run(6_000);
    assert!(l.net.socks.is_empty(), "la conexión se cerró sola");
    // el puerto 80 no lleva «:80» en Host
    l.router.srv.listen(80, b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n", false, false);
    let ev = fetch(&mut l, "example.org", false, true).unwrap();
    assert_eq!(ev.len(), 2, "Content-Length: 0 termina sin cuerpo: {ev:?}");
    assert!(l.router.srv.conns.last().unwrap().got.starts_with(b"GET / HTTP/1.1\r\nHost: example.org\r\n"));
}

#[test]
fn content_length_termina_sin_esperar_al_cierre() {
    let mut l = lan();
    l.router.srv.listen(80, b"HTTP/1.1 200 OK\r\nContent-Length: 3, 3\r\n\r\nabcEXTRA", false, false);
    let ev = fetch(&mut l, "http://example.org/", false, false).unwrap();
    assert_eq!(body(&ev), b"abc", "lo que sobra no es del cuerpo");
}

#[test]
fn cuerpo_grande() {
    let mut l = lan();
    let datos = pat_http(100_000);
    let mut r = b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\n\r\n".to_vec();
    r.extend_from_slice(&datos);
    l.router.srv.listen(80, &r, true, false);
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert!(body(&ev) == datos);
}

fn pat_http(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 13 + i / 7) as u8).collect()
}

#[test]
fn chunked_con_extensiones_y_trailers() {
    let mut l = lan();
    l.router.srv.listen(80, b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\nContent-Length: 999\r\n\r\n4;ext=1\r\nWiki\r\n5\r\npedia\r\nE\r\n in\r\n\r\nchunks.\r\n0\r\nTrailer: x\r\n\r\n", false, false);
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert_eq!(body(&ev), b"Wikipedia in\r\n\r\nchunks.");
    assert_eq!(ev.last(), Some(&HttpEvent::Done));
}

#[test]
fn chunked_a_trozos_cualesquiera() {
    let flujo = b"4;ext=1\r\nWiki\r\nA\r\npedia in c\r\nff\r\n".to_vec();
    let mut todo = flujo.clone();
    todo.extend_from_slice(&[b'z'; 255]);
    todo.extend_from_slice(b"\r\n0\r\nT: 1\r\nU: 2\r\n\r\n");
    let mut esperado = b"Wikipedia in c".to_vec();
    esperado.extend_from_slice(&[b'z'; 255]);
    let mut rng = Rng(7);
    for pasada in 0..200 {
        let mut c = Chunked::new();
        let mut out = Vec::new();
        let (mut i, mut fin) = (0, false);
        while i < todo.len() {
            let n = if pasada == 0 { 1 } else { 1 + rng.below(40) }.min(todo.len() - i);
            fin = c.feed(&todo[i..i + n], &mut out).unwrap();
            i += n;
        }
        assert!(fin && out == esperado, "pasada {pasada}");
    }
    // tras el final no se consume nada más
    let mut c = Chunked::new();
    let mut out = Vec::new();
    assert_eq!(c.feed(b"0\r\n\r\nSOBRA", &mut out), Ok(true));
    assert_eq!(c.feed(b"MAS", &mut out), Ok(true));
    assert!(out.is_empty());
}

#[test]
fn chunked_rechaza_basura() {
    let casos: [&[u8]; 8] = [b"xyz\r\n", b"\r\n", b"1234567890abcdef0\r\n", b"-1\r\n", b"3\r\nabcX\r\n", b"3\r\nabc\r\n\r\n", b";ext\r\n", b"0\r\nT: 1\r\n"];
    // los dos últimos: «;ext» sin tamaño es error; el trailer sin cerrar solo queda a medias (sin error)
    for (i, c) in casos.iter().enumerate() {
        let mut ch = Chunked::new();
        let r = ch.feed(c, &mut Vec::new());
        if i == 7 {
            assert_eq!(r, Ok(false));
        } else {
            assert!(r.is_err(), "{:?}", String::from_utf8_lossy(c));
        }
    }
    let mut ch = Chunked::new();
    assert!(ch.feed(&[b'a'; 2000], &mut Vec::new()).is_err(), "línea sin fin demasiado larga");
    let mut ch = Chunked::new();
    ch.feed(b"0\r\n", &mut Vec::new()).unwrap();
    let mut r = Ok(false);
    for _ in 0..200 {
        r = ch.feed(b"X: 0123456789012345678901234567890123456789\r\n", &mut Vec::new());
        if r.is_err() {
            break;
        }
    }
    assert!(r.is_err(), "trailers sin fin");
    assert!(ChunkState::End == ChunkState::End);
}

#[test]
fn hasta_que_el_servidor_cierra() {
    let mut l = lan();
    l.router.srv.listen(80, b"HTTP/1.0 200 OK\r\nServer: x\r\n\r\nabcdef", true, false);
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert_eq!((status(&ev), body(&ev)), (Some(200), b"abcdef".to_vec()));
    assert_eq!(ev.last(), Some(&HttpEvent::Done));
}

#[test]
fn head_204_304_y_100_continue() {
    let mut l = lan();
    l.router.srv.listen(80, b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n", false, false);
    let ev = fetch(&mut l, "http://example.org/", true, true).unwrap();
    assert_eq!((ev.len(), status(&ev)), (2, Some(200)));
    assert!(l.router.srv.conns[0].got.starts_with(b"HEAD / HTTP/1.1\r\n"));
    for (puerto, r) in [(81u16, &b"HTTP/1.1 204 No Content\r\n\r\n"[..]), (82, b"HTTP/1.1 304 Not Modified\r\nContent-Length: 50\r\n\r\n")] {
        l.router.srv.listen(puerto, r, false, false);
        let ev = fetch(&mut l, &alloc::format!("http://example.org:{puerto}/"), false, true).unwrap();
        assert_eq!((ev.len(), body(&ev).len()), (2, 0));
    }
    l.router.srv.listen(83, b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok", true, false);
    let ev = fetch(&mut l, "http://example.org:83/", false, true).unwrap();
    assert_eq!((heads(&ev), status(&ev), body(&ev)), (1, Some(200), b"ok".to_vec()));
}

#[test]
fn redirecciones_absoluta_sin_seguir_y_en_bucle() {
    let mut l = lan();
    l.router.srv.listen(80, b"HTTP/1.1 302 Found\r\nLocation: http://example.org:8080/z?q=1\r\nContent-Length: 7\r\n\r\nmovido!", true, false);
    l.router.srv.listen(8080, b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok", true, false);
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert_eq!((heads(&ev), status(&ev), body(&ev)), (1, Some(200), b"ok".to_vec()), "{ev:?}");
    assert!(l.router.srv.conns.iter().any(|c| c.port == 8080 && c.got.starts_with(b"GET /z?q=1 HTTP/1.1\r\nHost: example.org:8080\r\n")));
    // sin seguir: se devuelve la 302 con su Location
    let ev = fetch(&mut l, "http://example.org/", false, false).unwrap();
    match &ev[0] {
        HttpEvent::Head { status, headers, .. } => {
            assert_eq!(*status, 302);
            assert!(headers.iter().any(|(k, v)| k == "Location" && v.ends_with("/z?q=1")));
        }
        e => panic!("{e:?}"),
    }
    assert_eq!(body(&ev), b"movido!");
    // 301, 303, 307 y 308 también; sin Location, la respuesta se entrega tal cual
    for (puerto, cod) in [(91u16, 301), (92, 303), (93, 307), (94, 308)] {
        let r = alloc::format!("HTTP/1.1 {cod} X\r\nLocation: http://example.org:8080/\r\nContent-Length: 0\r\n\r\n");
        l.router.srv.listen(puerto, r.as_bytes(), false, false);
        let ev = fetch(&mut l, &alloc::format!("http://example.org:{puerto}/"), false, true).unwrap();
        assert_eq!(body(&ev), b"ok", "{cod}");
    }
    l.router.srv.listen(95, b"HTTP/1.1 302 Found\r\nContent-Length: 0\r\n\r\n", false, false);
    let ev = fetch(&mut l, "http://example.org:95/", false, true).unwrap();
    assert_eq!(status(&ev), Some(302));
}

#[test]
fn redireccion_relativa_y_tope() {
    let mut l = lan();
    // «c?x=1» respecto de /a/b es /a/c?x=1; el servidor la repite en bucle
    l.router.srv.listen(80, b"HTTP/1.1 301 Moved\r\nLocation: c?x=1\r\nContent-Length: 0\r\n\r\n", false, false);
    let ev = fetch(&mut l, "http://example.org/a/b?z=9", false, true).unwrap();
    assert_eq!(ev, [HttpEvent::Error(NetError::Protocol("demasiadas redirecciones"))]);
    let r = &l.router.srv.reqs;
    assert_eq!(r.len(), 1 + MAX_REDIRECTS as usize, "la inicial y 10 redirecciones");
    assert!(r[0].starts_with(b"GET /a/b?z=9 HTTP/1.1"));
    assert!(r[1].starts_with(b"GET /a/c?x=1 HTTP/1.1"));
    assert!(r[2].starts_with(b"GET /a/c?x=1 HTTP/1.1"));
    // ruta absoluta y de esquema relativo
    l.router.srv.reqs.clear();
    l.router.srv.listen(80, b"HTTP/1.1 302 Found\r\nLocation: /raiz\r\nContent-Length: 0\r\n\r\n", false, false);
    fetch(&mut l, "http://example.org/a/b", false, true).unwrap();
    assert!(l.router.srv.reqs[1].starts_with(b"GET /raiz HTTP/1.1"), "{:?}", l.router.srv.reqs.iter().map(|r| String::from_utf8_lossy(&r[..20.min(r.len())]).into_owned()).collect::<Vec<_>>());
    l.router.srv.listen(80, b"HTTP/1.1 302 Found\r\nLocation: //example.org:8081/q\r\nContent-Length: 0\r\n\r\n", false, false);
    l.router.srv.listen(8081, b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nQ", false, false);
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert_eq!(body(&ev), b"Q");
    // relativa con «://» en la consulta
    l.router.srv.reqs.clear();
    l.router.srv.listen(80, b"HTTP/1.1 302 Found\r\nLocation: /login?next=http://x\r\nContent-Length: 0\r\n\r\n", false, false);
    let _ = fetch(&mut l, "http://example.org/a", false, true);
    assert!(l.router.srv.reqs[1].starts_with(b"GET /login?next=http://x HTTP/1.1"));
}

#[test]
fn redirecciones_a_destinos_imposibles() {
    let mut l = lan();
    for (loc, esperado) in [
        ("https://example.org/", NetError::Tls("Este equipo no puede comprobar certificados.".to_string())),
        ("ftp://example.org/", NetError::BadUrl),
        ("http://no-existe.test/", NetError::NotFound),
        ("/con espacio", NetError::BadUrl),
    ] {
        let r = alloc::format!("HTTP/1.1 302 Found\r\nLocation: {loc}\r\nContent-Length: 0\r\n\r\n");
        l.router.srv.listen(80, r.as_bytes(), false, false);
        let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
        assert_eq!(ev, [HttpEvent::Error(esperado.clone())], "{loc}");
    }
}

#[test]
fn cabeceras_demasiado_grandes_y_justas() {
    let mut l = lan();
    let mut r = b"HTTP/1.1 200 OK\r\nX: ".to_vec();
    r.extend_from_slice(&alloc::vec![b'a'; 40_000]);
    l.router.srv.listen(80, &r, true, false);
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert_eq!(ev, [HttpEvent::Error(NetError::Protocol("cabeceras demasiado grandes"))]);
    // casi el tope: todavía vale
    let mut r = b"HTTP/1.1 200 OK\r\nX: ".to_vec();
    r.extend_from_slice(&alloc::vec![b'a'; MAX_HEAD - 100]);
    r.extend_from_slice(b"\r\nContent-Length: 2\r\n\r\nok");
    l.router.srv.listen(80, &r, true, false);
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert_eq!(body(&ev), b"ok");
}

#[test]
fn respuestas_mal_formadas() {
    let mut l = lan();
    let casos: [(&[u8], &str); 8] = [
        (b"garbage\r\n\r\n", "línea de estado no válida"),
        (b"HTTP/1.1 99 X\r\n\r\n", "línea de estado no válida"),
        (b"HTTP/2 200 OK\r\n\r\n", "línea de estado no válida"),
        (b"HTTP/1.1 200 OK\r\nsin-dos-puntos\r\n\r\n", "cabecera sin «:»"),
        (b"HTTP/1.1 200 OK\r\n : x\r\n\r\n", "nombre de cabecera no válido"),
        (b"HTTP/1.1 200 OK\r\nContent-Length: 5, 6\r\n\r\nabcdef", "Content-Length no válido"),
        (b"HTTP/1.1 200 OK\r\nContent-Length: abc\r\n\r\n", "Content-Length no válido"),
        (b"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n", "Content-Length no válido"),
    ];
    for (r, motivo) in casos {
        l.router.srv.listen(80, r, true, false);
        let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
        assert_eq!(ev.last(), Some(&HttpEvent::Error(NetError::Protocol(motivo))), "{:?}", String::from_utf8_lossy(r));
    }
}

#[test]
fn cuerpo_incompleto_o_conexion_cerrada_antes() {
    let mut l = lan();
    l.router.srv.listen(80, b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc", true, false);
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert_eq!((body(&ev), ev.last()), (b"abc".to_vec(), Some(&HttpEvent::Error(NetError::Protocol("respuesta incompleta")))));
    l.router.srv.listen(80, b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n", true, false);
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert_eq!(ev.last(), Some(&HttpEvent::Error(NetError::Protocol("respuesta incompleta"))));
    l.router.srv.listen(80, b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n", true, false);
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert_eq!(ev.last(), Some(&HttpEvent::Error(NetError::Protocol("tamaño de bloque no válido"))));
    l.router.srv.listen(80, b"HTTP/1.1 200 OK\r\nContent-Le", true, false);
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert_eq!(ev, [HttpEvent::Error(NetError::Protocol("conexión cerrada antes de las cabeceras"))]);
}

#[test]
fn errores_de_red_y_de_arranque() {
    let mut l = lan();
    assert_eq!(fetch(&mut l, "http://example.org:81/", false, true).unwrap(), [HttpEvent::Error(NetError::Refused)]);
    assert_eq!(fetch(&mut l, "http://no-existe.test/", false, true).unwrap(), [HttpEvent::Error(NetError::NotFound)]);
    assert!(matches!(HttpGet::start(&mut l.net, "ftp://x/", false, true), Err(NetError::BadUrl)));
    l.wire.borrow_mut().link = None;
    l.run(100);
    let r = HttpGet::start(&mut l.net, "http://otro.test/", false, true);
    assert!(matches!(r, Err(NetError::NoCable)), "{:?}", r.err());
}

#[test]
fn sin_respuesta_del_servidor_da_timeout_a_los_30_s() {
    let mut l = lan();
    l.router.srv.listen(80, b"", false, false); // acepta y calla
    let t0 = l.now;
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert_eq!(ev, [HttpEvent::Error(NetError::Timeout)]);
    assert!(l.now - t0 >= 30_000 && l.now - t0 < 31_000, "{}", l.now - t0);
    l.run(200);
    assert_eq!(l.router.srv.rst_seen, 1, "se corta la conexión");
}

#[test]
fn cancelar_libera_y_corta() {
    let mut l = lan();
    let datos = pat_http(300_000);
    let mut r = b"HTTP/1.1 200 OK\r\nContent-Length: 300000\r\n\r\n".to_vec();
    r.extend_from_slice(&datos);
    l.router.srv.listen(80, &r, true, false);
    let mut g = HttpGet::start(&mut l.net, "http://example.org/", false, true).unwrap();
    let mut n = 0;
    for _ in 0..3 {
        l.run(20);
        n += body(&g.poll(&mut l.net, l.now)).len();
    }
    assert!(n > 0 && n < 300_000, "{n}");
    g.cancel(&mut l.net);
    l.run(500);
    assert!(l.router.srv.rst_seen >= 1, "el servidor recibe RST");
    assert!(g.poll(&mut l.net, l.now).is_empty());
    assert!(l.net.socks.is_empty());
    // cancelar mientras resuelve o sin haber hecho nada tampoco rompe
    let mut g = HttpGet::start(&mut l.net, "http://example.org/", false, true).unwrap();
    g.cancel(&mut l.net);
    assert!(g.poll(&mut l.net, l.now).is_empty());
}

#[test]
fn analisis_de_cabeceras_directo() {
    let (st, rz, h) = parse_head(b"HTTP/1.1 404 No encontrado\r\nA: 1\r\nb:  dos  ").unwrap();
    assert_eq!((st, rz.as_str()), (404, "No encontrado"));
    assert_eq!(h, [("A".to_string(), "1".to_string()), ("b".to_string(), "dos".to_string())]);
    assert_eq!(parse_head(b"HTTP/1.1 200 ").unwrap().1, "");
}

// ---- v0.12.4: Content-Encoding gzip / deflate ----

fn con_cabecera(cab: &str, cuerpo: &[u8]) -> Vec<u8> {
    let mut r = alloc::format!("HTTP/1.1 200 OK\r\n{cab}\r\n\r\n").into_bytes();
    r.extend_from_slice(cuerpo);
    r
}

fn comprimido(cod: &str, z: &[u8]) -> Vec<u8> {
    con_cabecera(&alloc::format!("Content-Encoding: {cod}\r\nContent-Length: {}", z.len()), z)
}

#[test]
fn pide_gzip_y_deflate_y_descomprime() {
    let esperado = crate::img::tests::var(800);
    let casos: [(&str, &[u8]); 6] = [
        ("gzip", include_bytes!("../../img/testdata/var.gz")),
        ("GZIP", include_bytes!("../../img/testdata/var.gz")),
        ("x-gzip", include_bytes!("../../img/testdata/var.gz")),
        ("deflate", include_bytes!("../../img/testdata/var_l9.zlib")),
        ("deflate", include_bytes!("../../img/testdata/var_l6.raw")), // DEFLATE crudo, sin cabecera zlib
        ("identity", &[]),
    ];
    for (cod, z) in casos {
        let mut l = lan();
        let r = if z.is_empty() { comprimido(cod, &esperado) } else { comprimido(cod, z) };
        l.router.srv.listen(80, &r, true, false);
        let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
        assert_eq!(body(&ev), esperado, "{cod}");
        assert_eq!(ev.last(), Some(&HttpEvent::Done), "{cod}");
        let pedido = String::from_utf8_lossy(&l.router.srv.conns[0].got).into_owned();
        assert!(pedido.contains("\r\nAccept-Encoding: gzip, deflate\r\n"), "{pedido}");
        // las cabeceras que ve el usuario siguen siendo las del servidor (también el Content-Length comprimido)
        let HttpEvent::Head { headers, .. } = &ev[0] else { panic!("{ev:?}") };
        assert!(headers.iter().any(|(k, v)| k == "Content-Encoding" && v == cod));
    }
}

#[test]
fn gzip_con_chunked_y_sin_longitud() {
    let esperado = crate::img::tests::var(800);
    let gz = include_bytes!("../../img/testdata/var.gz");
    let mut r = b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    for t in gz.chunks(100) {
        r.extend_from_slice(alloc::format!("{:x}\r\n", t.len()).as_bytes());
        r.extend_from_slice(t);
        r.extend_from_slice(b"\r\n");
    }
    r.extend_from_slice(b"0\r\n\r\n");
    let mut l = lan();
    l.router.srv.listen(80, &r, true, false);
    assert_eq!(body(&fetch(&mut l, "http://example.org/", false, true).unwrap()), esperado);
    // sin Content-Length: termina al cerrar el servidor
    let r = con_cabecera("Content-Encoding: deflate", include_bytes!("../../img/testdata/var_l9.zlib"));
    l.router.srv.listen(80, &r, true, false);
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert_eq!((body(&ev), ev.last()), (esperado, Some(&HttpEvent::Done)));
}

#[test]
fn otros_codigos_y_casos_sin_cuerpo_pasan_tal_cual() {
    let mut l = lan();
    let r = comprimido("br", b"\x1b\x03datos de otro codec");
    l.router.srv.listen(80, &r, true, false);
    assert_eq!(body(&fetch(&mut l, "http://example.org/", false, true).unwrap()), b"\x1b\x03datos de otro codec");
    let r = comprimido("gzip, br", b"xyz");
    l.router.srv.listen(80, &r, true, false);
    assert_eq!(body(&fetch(&mut l, "http://example.org/", false, true).unwrap()), b"xyz");
    // HEAD y cuerpo vacío con la cabecera puesta: nada que descomprimir, ningún error
    let r = con_cabecera("Content-Encoding: gzip\r\nContent-Length: 50", b"");
    l.router.srv.listen(80, &r, true, false);
    let ev = fetch(&mut l, "http://example.org/", true, true).unwrap();
    assert_eq!(ev.last(), Some(&HttpEvent::Done));
    let r = con_cabecera("Content-Encoding: gzip\r\nContent-Length: 0", b"");
    l.router.srv.listen(80, &r, true, false);
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert_eq!((body(&ev).len(), ev.last()), (0, Some(&HttpEvent::Done)));
}

#[test]
fn cuerpo_comprimido_roto_incompleto_o_bomba() {
    let gz = include_bytes!("../../img/testdata/var.gz");
    let mut l = lan();
    let mut roto = gz.to_vec();
    let n = roto.len();
    roto[n - 6] ^= 0xFF; // el CRC-32 ya no cuadra
    l.router.srv.listen(80, &comprimido("gzip", &roto), true, false);
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert_eq!(ev.last(), Some(&HttpEvent::Error(NetError::Protocol("cuerpo comprimido no válido"))));
    // la longitud declarada casa, pero al flujo gzip le falta el final
    l.router.srv.listen(80, &comprimido("gzip", &gz[..gz.len() - 5]), true, false);
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert_eq!(ev.last(), Some(&HttpEvent::Error(NetError::Protocol("cuerpo comprimido incompleto"))));
    // basura que no es DEFLATE
    l.router.srv.listen(80, &comprimido("gzip", &[0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]), true, false);
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert!(matches!(ev.last(), Some(HttpEvent::Error(NetError::Protocol(_)))), "{:?}", ev.last());
    // una bomba (≈ 39 MB desde 250 KB) se corta en el tope de 32 MB
    let bomba = crate::img::tests::bomba(150_000);
    l.router.srv.listen(80, &comprimido("deflate", &bomba), true, false);
    let ev = fetch(&mut l, "http://example.org/", false, true).unwrap();
    assert_eq!(ev.last(), Some(&HttpEvent::Error(NetError::Protocol("cuerpo descomprimido demasiado grande"))));
    assert!(body(&ev).len() <= 32 * 1024 * 1024);
}
