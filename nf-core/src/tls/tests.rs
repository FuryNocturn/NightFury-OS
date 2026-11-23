// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Pruebas de TLS 1.3.
//!
//! - RFC 8448 §3: programa de claves con el ClientHello y el ServerHello del documento.
//! - `vuelo_*.bin` (testdata): lo que respondió un `openssl s_server` a nuestro ClientHello fijo (que es
//!   siempre el mismo bytes a bytes), para reproducirlo sin red. Se regraban con
//!   `NF_TLS_GRABAR=1 cargo test -p nf-core -- --ignored tls` (las claves y certificados de `testdata`
//!   son SOLO de prueba; los genera `testdata/gen.sh`).
//! - Interoperabilidad con OpenSSL (`#[ignore]`): `cargo test -p nf-core -- --ignored tls`.

use super::hs13::claves;
use super::*;
use crate::crypto::{from_hex, Rng};
use alloc::string::ToString;
use alloc::vec;

const CA: &[u8] = include_bytes!("testdata/ca.der");
const VUELOS: [(&str, &[u8], &str, &str); 4] = [
    ("chacha_ec", include_bytes!("testdata/vuelo_chacha_ec.bin"), "TLS_CHACHA20_POLY1305_SHA256", "x25519"),
    ("aes128_rsa", include_bytes!("testdata/vuelo_aes128_rsa.bin"), "TLS_AES_128_GCM_SHA256", "x25519"),
    ("aes256_p384", include_bytes!("testdata/vuelo_aes256_p384.bin"), "TLS_AES_256_GCM_SHA384", "x25519"),
    ("hrr_ec", include_bytes!("testdata/vuelo_hrr_ec.bin"), "TLS_CHACHA20_POLY1305_SHA256", "secp256r1"),
];

fn hex(s: &str) -> Vec<u8> {
    from_hex(&s.split_whitespace().collect::<String>()).expect("hex")
}

fn ahora() -> Time {
    Time { year: 2026, month: 11, day: 23, hour: 12, min: 0, sec: 0 }
}

fn almacen() -> Arc<RootStore> {
    let mut r = RootStore::new();
    r.add_der(CA).expect("ca");
    Arc::new(r)
}

const RANDOM: [u8; 32] = [7; 32];
const SID: [u8; 32] = [9; 32];
const PRIV: [u8; 32] = [5; 32];

/// Cliente con todos los valores fijos: su ClientHello es siempre el mismo.
fn fijo(host: &str, insecure: bool) -> Client {
    Client::con_valores(host, ahora(), almacen(), insecure, Rng::new([3; 32]), RANDOM, SID.to_vec(), PRIV)
}

/// Registros de un flujo: (inicio, tipo, longitud del cuerpo).
fn registros(b: &[u8]) -> Vec<(usize, u8, usize)> {
    let (mut v, mut p) = (Vec::new(), 0);
    while p + 5 <= b.len() {
        let n = u16::from_be_bytes([b[p + 3], b[p + 4]]) as usize;
        v.push((p, b[p], n));
        p += 5 + n;
    }
    v
}

// ---------------------------------------------------------------------------------------------
// RFC 8448 §3 (Simple 1-RTT Handshake)
// ---------------------------------------------------------------------------------------------

const RFC_CH: &str = "01 00 00 c0 03 03 cb 34 ec b1 e7 81 63 ba 1c 38 c6 da cb 19 6a 6d ff a2 1a 8d 99 12 ec 18 a2 ef 62 83 02 4d ec e7
    00 00 06 13 01 13 03 13 02 01 00 00 91 00 00 00 0b 00 09 00 00 06 73 65 72 76 65 72 ff 01 00 01 00 00 0a 00 14 00 12
    00 1d 00 17 00 18 00 19 01 00 01 01 01 02 01 03 01 04 00 23 00 00 00 33 00 26 00 24 00 1d 00 20 99 38 1d e5 60 e4 bd
    43 d2 3d 8e 43 5a 7d ba fe b3 c0 6e 51 c1 3c ae 4d 54 13 69 1e 52 9a af 2c 00 2b 00 03 02 03 04 00 0d 00 20 00 1e 04
    03 05 03 06 03 02 03 08 04 08 05 08 06 04 01 05 01 06 01 02 01 04 02 05 02 06 02 02 02 00 2d 00 02 01 01 00 1c 00 02
    40 01";
const RFC_SH: &str = "02 00 00 56 03 03 a6 af 06 a4 12 18 60 dc 5e 6e 60 24 9c d3 4c 95 93 0c 8a c5 cb 14 34 da c1 55 77 2e d3 e2 69 28
    00 13 01 00 00 2e 00 33 00 24 00 1d 00 20 c9 82 88 76 11 20 95 fe 66 76 2b db f7 c6 72 e1 56 d6 cc 25 3b 83 3d f1 dd
    69 b1 b0 4e 75 1f 0f 00 2b 00 02 03 04";

#[test]
fn rfc8448_programa_de_claves() {
    let random: [u8; 32] = hex("cb34ecb1e78163ba1c38c6dacb196a6dffa21a8d9912ec18a2ef6283024dece7").try_into().unwrap();
    let privada: [u8; 32] = hex("49af42ba7f7994852d713ef2784bcbcaa7911de26adc5642cb634540e7ea5005").try_into().unwrap();
    let mut c = Client::con_valores("server", ahora(), Arc::new(RootStore::new()), true, Rng::new([1; 32]), random, vec![], privada);
    // Nuestro ClientHello lleva la misma clave pública x25519 del documento.
    let ch_propio = c.take_output();
    let publica = hex("99381de560e4bd43d23d8e435a7dbafeb3c06e51c13cae4d5413691e529aaf2c");
    assert!(ch_propio.windows(32).any(|w| w == &publica[..]));
    // Para el transcript se usa el ClientHello del documento (otras extensiones que las nuestras).
    c.transcript = hex(RFC_CH);
    let sh = hex(RFC_SH);
    let mut rec = vec![22, 3, 3, 0, sh.len() as u8];
    rec.extend_from_slice(&sh);
    c.feed(&rec).expect("ServerHello del RFC");
    assert_eq!(&*c.hs_secret, &hex("1dc826e93606aa6fdc0aadc12f741b01046aa6b99f691ed221a9f0ca043fbeac")[..]);
    assert_eq!(&*c.c_hs, &hex("b3eddb126e067f35a780b3abf45e2d8f3b1a950738f52e9600746a0e27a55a21")[..]);
    assert_eq!(&*c.s_hs, &hex("b67b7d690cc16c4e75e54213cb2d37b4e9c912bcded9105d42befd59d391ad38")[..]);
    let rx = c.rx.as_ref().unwrap();
    assert_eq!(&*rx.key, &hex("3fce516009c21727d0f2e4e86ee403bc")[..]);
    assert_eq!(&rx.iv[..], &hex("5d313eb2671276ee13000b30")[..]);
    let h = c.suite.unwrap().hash;
    let k = claves(h, c.suite.unwrap().aead, &c.c_hs);
    assert_eq!(&*k.key, &hex("dbfaa693d1762c5b666af5d950258d01")[..]);
    assert_eq!(&k.iv[..], &hex("5bd3c71b836e0b76bb73265f")[..]);
}

// ---------------------------------------------------------------------------------------------
// Reproducción de los vuelos grabados
// ---------------------------------------------------------------------------------------------

fn reproducir(host: &str, insecure: bool, vuelo: &[u8], trozo: usize) -> (Client, Result<(), TlsError>) {
    let mut c = fijo(host, insecure);
    c.take_output();
    for t in vuelo.chunks(trozo) {
        if let Err(e) = c.feed(t) {
            return (c, Err(e));
        }
    }
    (c, Ok(()))
}

#[test]
fn reproduce_vuelos() {
    for (nombre, v, suite, grupo) in VUELOS {
        for trozo in [v.len(), 1, 7] {
            let (mut c, r) = reproducir("prueba.nf", false, v, trozo);
            r.unwrap_or_else(|e| panic!("{nombre}/{trozo}: {}", e.mensaje()));
            assert!(c.is_ready(), "{nombre}");
            let i = c.info().unwrap();
            assert_eq!((i.version, i.suite, i.group), ("TLS 1.3", suite, grupo), "{nombre}");
            assert_eq!(i.chain.as_ref().unwrap().leaf.cn, "prueba.nf");
            assert!(i.cert_error.is_none());
            assert!(c.read().starts_with(b"HTTP/1.0 200 ok"), "{nombre}");
            assert!(c.peer_closed(), "{nombre}");
        }
    }
}

#[test]
fn certificado_de_otro_nombre() {
    // Vuelo de un servidor cuyo certificado es de «otro.nf», al que llamamos «prueba.nf».
    let v: &[u8] = include_bytes!("testdata/vuelo_otro_host.bin");
    let (mut c, r) = reproducir("prueba.nf", false, v, 4096);
    assert_eq!(r, Err(TlsError::Cert(CertError::NombreNoCoincide)));
    // Sale la alerta bad_certificate (42) y todo queda cerrado.
    assert!(c.take_output().ends_with(&[21, 3, 3, 0, 2, 2, 42]));
    assert_eq!(c.feed(&[0]), Err(TlsError::Cerrado));
    // Con -k sigue y lo apunta.
    let (c, r) = reproducir("prueba.nf", true, v, 4096);
    r.unwrap();
    assert!(c.is_ready());
    let i = c.info().unwrap();
    assert_eq!(i.cert_error, Some(CertError::NombreNoCoincide));
    assert!(i.chain.is_none());
}

#[test]
fn sin_raices_falla() {
    let v = VUELOS[0].1;
    let mut c = Client::con_valores("prueba.nf", ahora(), Arc::new(RootStore::new()), false, Rng::new([3; 32]), RANDOM, SID.to_vec(), PRIV);
    assert!(matches!(c.feed(v), Err(TlsError::Cert(_))));
}

#[test]
fn clienthello_bien_formado() {
    let mut c = fijo("prueba.nf", false);
    let o = c.take_output();
    assert_eq!(&o[..3], &[22, 3, 3]);
    assert_eq!(o.len(), 5 + u16::from_be_bytes([o[3], o[4]]) as usize);
    assert_eq!(&o[5..9], &[1, 0, ((o.len() - 9) >> 8) as u8, (o.len() - 9) as u8]);
    assert_eq!(&o[9..11], &[3, 3]);
    assert_eq!(&o[11..43], &RANDOM);
    assert_eq!(o[43], 32);
    assert_eq!(&o[44..76], &SID);
    // Suites: ChaCha20, AES-128, AES-256 (en ese orden).
    assert_eq!(&o[76..84], &[0, 6, 0x13, 3, 0x13, 1, 0x13, 2]);
    assert!(o.windows(9).any(|w| w == b"prueba.nf"));
    assert!(o.windows(8).any(|w| w == b"http/1.1"));
    // Con una IP literal no hay SNI.
    let o = fijo("127.0.0.1", false).take_output();
    assert!(!o.windows(9).any(|w| w == b"127.0.0.1"));
    let o = fijo("[::1]", false).take_output();
    assert!(!o.windows(3).any(|w| w == b"::1"));
}

// ---------------------------------------------------------------------------------------------
// Robustez
// ---------------------------------------------------------------------------------------------

/// Cliente que ya terminó el saludo (se le dan registros hasta que `is_ready`) y desde dónde sigue el vuelo.
fn conectado_en() -> (Client, usize) {
    let v = VUELOS[0].1;
    let mut c = fijo("prueba.nf", false);
    c.take_output();
    for (p, _, n) in registros(v) {
        c.feed(&v[p..p + 5 + n]).unwrap();
        if c.is_ready() {
            c.take_output();
            return (c, p + 5 + n);
        }
    }
    panic!("el vuelo no termina el saludo");
}

fn conectado() -> Client {
    conectado_en().0
}

#[test]
fn registro_truncado_espera_y_longitudes_absurdas_fallan() {
    let v = VUELOS[0].1;
    for n in [0, 1, 4, 5, 6, 70, conectado_en().1 - 1] {
        let mut c = fijo("prueba.nf", false);
        c.feed(&v[..n]).unwrap_or(());
        assert!(!c.is_ready());
    }
    for len in [0xffffu16, 0x4101, 0x6000] {
        let mut c = fijo("prueba.nf", false);
        c.take_output();
        let r = c.feed(&[22, 3, 3, (len >> 8) as u8, len as u8]);
        assert!(matches!(r, Err(TlsError::Protocolo(_))), "{len:x}");
        assert_eq!(c.take_output(), [21, 3, 3, 0, 2, 2, 22]);
    }
    // Un registro de saludo vacío y algo que no es TLS.
    let mut c = fijo("prueba.nf", false);
    assert!(c.feed(&[22, 3, 3, 0, 0]).is_err());
    let mut c = fijo("prueba.nf", false);
    assert!(c.feed(b"HTTP/1.1 400 Bad Request\r\n\r\n").is_err());
}

#[test]
fn alerta_del_servidor() {
    let mut c = fijo("prueba.nf", false);
    c.take_output();
    let e = c.feed(&[21, 3, 3, 0, 2, 2, 40]).unwrap_err();
    assert_eq!(e, TlsError::Alerta(40));
    assert!(e.mensaje().contains("40"));
    assert!(c.take_output().is_empty()); // a una alerta no se contesta
    assert_eq!(c.write(b"x"), Err(TlsError::Cerrado));
}

fn con_parche(f: impl Fn(&mut Vec<u8>)) -> (Client, Result<(), TlsError>) {
    let mut v = VUELOS[0].1.to_vec();
    f(&mut v);
    let mut c = fijo("prueba.nf", false);
    c.take_output();
    let r = c.feed(&v);
    (c, r)
}

// Posiciones en el ServerHello del vuelo: cabecera de registro (5) + de mensaje (4) + versión (2).
const POS_RANDOM: usize = 11;
const POS_SID: usize = 44;
const POS_SUITE: usize = 76;

#[test]
fn serverhello_incorrecto() {
    // Suite que no se ofreció.
    let (mut c, r) = con_parche(|v| v[POS_SUITE..POS_SUITE + 2].copy_from_slice(&[0xc0, 0x2f]));
    assert!(matches!(r, Err(TlsError::Protocolo(_))));
    assert_eq!(c.take_output(), [21, 3, 3, 0, 2, 2, 47]);
    // Eco del session_id distinto.
    let (_, r) = con_parche(|v| v[POS_SID] ^= 1);
    assert!(matches!(r, Err(TlsError::Protocolo(_))));
    // Centinelas de degradación.
    for ultimo in [0u8, 1] {
        let (_, r) = con_parche(|v| {
            v[POS_RANDOM + 24..POS_RANDOM + 31].copy_from_slice(b"DOWNGRD");
            v[POS_RANDOM + 31] = ultimo;
        });
        assert!(matches!(r, Err(TlsError::Protocolo(m)) if m.contains("degradación")));
    }
    // Compresión distinta de cero.
    let (_, r) = con_parche(|v| v[POS_SUITE + 2] = 1);
    assert!(r.is_err());
    // Sin supported_versions (un servidor TLS 1.2): se cambia el tipo de esa extensión.
    let (_, r) = con_parche(|v| {
        let n = v.windows(4).position(|w| w == [0x00, 0x2b, 0x00, 0x02]).unwrap();
        v[n + 1] = 0x2a;
    });
    assert!(matches!(r, Err(TlsError::NoSoportado(_)) | Err(TlsError::Protocolo(_))));
}

/// ServerHello/HelloRetryRequest a mano (sesión de 32 bytes `SID`, suite ChaCha20).
fn hello_servidor(random: &[u8], exts: &[u8]) -> Vec<u8> {
    let mut b = vec![3, 3];
    b.extend_from_slice(random);
    b.push(32);
    b.extend_from_slice(&SID);
    b.extend_from_slice(&[0x13, 3, 0]);
    b.extend_from_slice(&(exts.len() as u16).to_be_bytes());
    b.extend_from_slice(exts);
    let mut m = vec![2, 0, (b.len() >> 8) as u8, b.len() as u8];
    m.extend_from_slice(&b);
    let mut r = vec![22, 3, 3, (m.len() >> 8) as u8, m.len() as u8];
    r.extend_from_slice(&m);
    r
}

#[test]
fn hello_retry_request() {
    let hrr_random = crate::crypto::sha256(b"HelloRetryRequest");
    let sv = [0x00, 0x2b, 0x00, 0x02, 3, 4];
    let p256 = [sv.as_slice(), &[0x00, 0x33, 0x00, 0x02, 0x00, 0x17]].concat();
    // Cambia el grupo: sale un segundo ClientHello con key_share P-256 y un solo CCS.
    let mut c = fijo("prueba.nf", false);
    let ch1 = c.take_output();
    c.feed(&hello_servidor(&hrr_random, &p256)).unwrap();
    let o = c.take_output();
    assert_eq!(&o[..6], &[20, 3, 3, 0, 1, 1]);
    assert_eq!(&o[6..9], &[22, 3, 3]);
    assert!(o.windows(6).any(|w| w == [0x00, 0x33, 0x00, 0x47, 0x00, 0x45]));
    assert!(o.windows(4).any(|w| w == [0x00, 0x17, 0x00, 0x41]) && ch1 != o);
    // Un segundo HRR es un error.
    assert!(c.feed(&hello_servidor(&hrr_random, &p256)).is_err());
    // HRR que no cambia nada (grupo ya enviado) o con grupo desconocido.
    for g in [[0x00u8, 0x1d], [0x00, 0x18]] {
        let ext = [sv.as_slice(), &[0x00, 0x33, 0x00, 0x02, g[0], g[1]]].concat();
        assert!(fijo("prueba.nf", false).feed(&hello_servidor(&hrr_random, &ext)).is_err());
    }
    assert!(fijo("prueba.nf", false).feed(&hello_servidor(&hrr_random, &sv)).is_err());
    // Con cookie solo: vale y la repite.
    let mut c = fijo("prueba.nf", false);
    c.take_output();
    let ext = [sv.as_slice(), &[0x00, 0x2c, 0x00, 0x05, 0x00, 0x03, 1, 2, 3]].concat();
    c.feed(&hello_servidor(&hrr_random, &ext)).unwrap();
    assert!(c.take_output().windows(9).any(|w| w == &[0x00, 0x2c, 0x00, 0x05, 0x00, 0x03, 1, 2, 3][..]));
}

#[test]
fn registros_en_claro_tras_el_cambio_de_claves() {
    let v = VUELOS[0].1;
    let r = registros(v);
    // Tras el ServerHello, un registro de saludo sin cifrar es un error.
    let sh = r[0].2 + 5;
    let mut c = fijo("prueba.nf", false);
    c.take_output();
    c.feed(&v[..sh]).unwrap();
    assert!(c.feed(&[22, 3, 3, 0, 4, 20, 0, 0, 0]).is_err());
    // CCS: se tolera el de compatibilidad, no uno raro ni uno con el saludo ya terminado.
    let mut c = fijo("prueba.nf", false);
    c.take_output();
    c.feed(&v[..sh]).unwrap();
    c.feed(&[20, 3, 3, 0, 1, 1]).unwrap();
    assert!(c.feed(&[20, 3, 3, 0, 1, 2]).is_err());
    let mut c = conectado();
    assert!(c.feed(&[20, 3, 3, 0, 1, 1]).is_err());
}

#[test]
fn ningun_cambio_del_saludo_se_acepta() {
    // Cambiar cualquier byte del saludo (hasta el Finished del servidor) tiene que dar error o dejarlo
    // sin terminar; solo se salva la versión de cada cabecera de registro, que se ignora a propósito.
    let v = VUELOS[0].1;
    let r = registros(v);
    let fin = conectado_en().1;
    let cab: Vec<usize> = r.iter().map(|x| x.0 + 2).collect();
    for p in (0..fin).filter(|p| !cab.contains(p)) {
        let mut m = v[..fin].to_vec();
        m[p] ^= 0x01;
        let (c, e) = reproducir("prueba.nf", false, &m, 1 << 20);
        assert!(e.is_err() || !c.is_ready(), "byte {p} aceptado");
    }
}

/// Servidor de pruebas: claves de aplicación del servidor, tomadas de las del cliente.
fn claves_servidor(c: &Client) -> Keys {
    let s = c.suite.unwrap();
    let mut k = claves(s.hash, s.aead, &c.rx_secret);
    k.seq = c.rx.as_ref().unwrap().seq;
    k
}

fn claves_cliente(c: &Client) -> Keys {
    let s = c.suite.unwrap();
    let mut k = claves(s.hash, s.aead, &c.tx_secret);
    k.seq = c.tx.as_ref().unwrap().seq;
    k
}

#[test]
fn escritura_en_registros_de_16_kib() {
    let mut c = conectado();
    c.take_output();
    let datos: Vec<u8> = (0..40_000u32).map(|i| i as u8).collect();
    c.write(&datos).unwrap();
    let o = c.take_output();
    let r = registros(&o);
    assert_eq!(r.len(), 3);
    let mut k = claves_cliente(&c);
    k.seq -= 3; // el contador de la copia de pruebas parte de antes de los tres registros
    let mut claro = Vec::new();
    for (p, _, n) in r {
        assert!(n <= MAX_CIFRADO);
        let (t, d) = k.open(&o[p..p + 5], &o[p + 5..p + 5 + n]).unwrap();
        assert_eq!(t, CT_DATOS);
        assert!(d.len() <= MAX_CLARO);
        claro.extend(d);
    }
    assert_eq!(claro, datos);
}

#[test]
fn key_update_y_cierre() {
    let mut c = conectado();
    let (h, a) = (c.suite.unwrap().hash, c.suite.unwrap().aead);
    let (rx_viejo, tx_viejo) = (c.rx_secret.0.clone(), c.tx_secret.0.clone());
    let mut s = claves_servidor(&c);
    let mut kc = claves_cliente(&c);
    // El servidor pide actualizar: respondemos con un KeyUpdate (con las claves viejas) y pasamos a las nuevas.
    c.feed(&s.seal(CT_SALUDO, &[24, 0, 0, 1, 1]).unwrap()).unwrap();
    let o = c.take_output();
    let (t, d) = kc.open(&o[..5], &o[5..]).unwrap();
    assert_eq!((t, &d[..]), (CT_SALUDO, &[24u8, 0, 0, 1, 0][..]));
    c.write(b"hola").unwrap();
    let o = c.take_output();
    let mut kn = claves(h, a, &h.expand_label(&tx_viejo, "traffic upd", &[], h.len()));
    assert_eq!(kn.open(&o[..5], &o[5..]).unwrap(), (CT_DATOS, b"hola".to_vec()));
    // Y lo del servidor llega con las suyas nuevas.
    let mut sn = claves(h, a, &h.expand_label(&rx_viejo, "traffic upd", &[], h.len()));
    c.feed(&sn.seal(CT_DATOS, b"respuesta").unwrap()).unwrap();
    assert_eq!(c.read(), b"respuesta");
    // Valor de KeyUpdate no válido.
    let mut c = conectado();
    let mut s = claves_servidor(&c);
    assert!(c.feed(&s.seal(CT_SALUDO, &[24, 0, 0, 1, 7]).unwrap()).is_err());
    // close_notify del servidor: se anota y se contesta con el nuestro.
    let mut c = conectado();
    let mut s = claves_servidor(&c);
    c.feed(&s.seal(CT_ALERTA, &[1, 0]).unwrap()).unwrap();
    assert!(c.peer_closed());
    assert!(!c.take_output().is_empty());
    assert_eq!(c.write(b"x"), Err(TlsError::Cerrado));
    // Un registro de datos corrupto: error y todo cerrado.
    let mut c = conectado();
    let mut s = claves_servidor(&c);
    let mut r = s.seal(CT_DATOS, b"datos").unwrap();
    let n = r.len() - 1;
    r[n] ^= 1;
    assert!(matches!(c.feed(&r), Err(TlsError::Protocolo(_))));
    assert_eq!(c.feed(&[0]), Err(TlsError::Cerrado));
}

#[test]
fn escribir_antes_de_tiempo_y_cerrar() {
    let mut c = fijo("prueba.nf", false);
    assert!(matches!(c.write(b"x"), Err(TlsError::Protocolo(_))));
    let mut c = conectado();
    c.close();
    assert_eq!(c.write(b"x"), Err(TlsError::Cerrado));
}

#[test]
fn mensajes_de_error_en_espanol() {
    for e in [TlsError::Cert(CertError::Caducado), TlsError::Alerta(48), TlsError::Protocolo("x"), TlsError::NoSoportado("y"), TlsError::Cerrado] {
        assert!(!e.mensaje().is_empty());
    }
    assert!(TlsError::Alerta(48).mensaje().contains("entidad certificadora"));
    assert_eq!(TlsError::Protocolo("x").mensaje().to_string(), "Error en la conexión segura: x.");
}

/// Generador xorshift para el fuzz determinista.
struct Xs(u64);
impl Xs {
    fn n(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn fuzz_sin_panico() {
    let mut x = Xs(0x9e3779b97f4a7c15);
    for (_, v, _, _) in VUELOS {
        for _ in 0..150 {
            let mut m = v.to_vec();
            for _ in 0..1 + x.n() % 3 {
                let p = (x.n() as usize) % m.len();
                m[p] = x.n() as u8;
            }
            match x.n() % 4 {
                0 => m.truncate((x.n() as usize) % m.len()),
                1 => {
                    let p = (x.n() as usize) % m.len();
                    m.drain(p..(p + 1 + (x.n() as usize) % 50).min(m.len()));
                }
                _ => {}
            }
            let mut c = fijo("prueba.nf", x.n() % 2 == 0);
            c.take_output();
            let trozo = 1 + (x.n() as usize) % 700;
            for t in m.chunks(trozo) {
                if c.feed(t).is_err() {
                    break;
                }
            }
            c.read();
            let _ = c.write(b"x");
        }
    }
    // Bytes al azar como si fueran del servidor.
    for _ in 0..200 {
        let n = (x.n() % 400) as usize;
        let m: Vec<u8> = (0..n).map(|_| x.n() as u8).collect();
        let mut c = fijo("prueba.nf", true);
        let _ = c.feed(&m);
    }
}

// ---------------------------------------------------------------------------------------------
// Interoperabilidad con OpenSSL (`cargo test -p nf-core -- --ignored tls`)
// ---------------------------------------------------------------------------------------------

mod openssl {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;

    struct Servidor(Child);
    impl Drop for Servidor {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn datos() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/tls/testdata")
    }

    fn lanzar(cert: &str, extra: &[&str]) -> (Servidor, u16) {
        let puerto = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let d = datos();
        let exe = std::env::var("OPENSSL").unwrap_or_else(|_| "openssl".into());
        let hijo = Command::new(exe)
            .args(["s_server", "-tls1_3", "-www", "-accept", &format!("127.0.0.1:{puerto}")])
            .arg("-cert")
            .arg(d.join(format!("{cert}.pem")))
            .arg("-key")
            .arg(d.join(format!("{cert}.key")))
            .args(extra)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("no se pudo lanzar openssl (variable OPENSSL)");
        let s = Servidor(hijo);
        for _ in 0..100 {
            if TcpStream::connect(("127.0.0.1", puerto)).is_ok() {
                return (s, puerto);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("openssl s_server no arrancó");
    }

    /// Hace un GET por TLS; devuelve la respuesta y, si `grabar`, los bytes que mandó el servidor.
    fn get(c: &mut Client, puerto: u16) -> (Result<Vec<u8>, TlsError>, Vec<u8>) {
        let mut s = TcpStream::connect(("127.0.0.1", puerto)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let (mut recibido, mut resp, mut buf, mut pedido) = (Vec::new(), Vec::new(), [0u8; 8192], false);
        let r = loop {
            let o = c.take_output();
            if !o.is_empty() {
                let _ = s.write_all(&o);
            }
            if c.is_ready() && !pedido {
                pedido = true;
                if let Err(e) = c.write(b"GET / HTTP/1.0\r\n\r\n") {
                    break Err(e);
                }
                continue;
            }
            if c.peer_closed() {
                break Ok(());
            }
            let n = s.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break Ok(());
            }
            recibido.extend_from_slice(&buf[..n]);
            if let Err(e) = c.feed(&buf[..n]) {
                let _ = s.write_all(&c.take_output());
                break Err(e);
            }
            resp.extend(c.read());
        };
        resp.extend(c.read());
        (r.map(|_| resp), recibido)
    }

    /// Una combinación: certificado, opciones de s_server, suite y grupo esperados. Con `grabar` guarda el vuelo.
    fn caso(nombre: &str, cert: &str, extra: &[&str], suite: &str, grupo: &str) {
        let (_s, puerto) = lanzar(cert, extra);
        let mut c = fijo("prueba.nf", false);
        let (r, recibido) = get(&mut c, puerto);
        let resp = r.unwrap_or_else(|e| panic!("{nombre}: {}", e.mensaje()));
        let i = c.info().expect("info");
        assert_eq!((i.suite, i.group), (suite, grupo), "{nombre}");
        let texto = String::from_utf8_lossy(&resp);
        assert!(texto.starts_with("HTTP/1.0 200 ok"), "{nombre}: {texto}");
        assert!(texto.contains(&format!("Cipher is {suite}")), "{nombre}");
        if std::env::var("NF_TLS_GRABAR").is_ok() && VUELOS.iter().any(|v| v.0 == nombre) {
            std::fs::write(datos().join(format!("vuelo_{nombre}.bin")), recibido).unwrap();
        }
    }

    #[test]
    #[ignore]
    fn tls_openssl_chacha_ecdsa() {
        caso("chacha_ec", "srv_ec", &["-ciphersuites", "TLS_CHACHA20_POLY1305_SHA256"], "TLS_CHACHA20_POLY1305_SHA256", "x25519");
    }

    #[test]
    #[ignore]
    fn tls_openssl_aes128_rsa() {
        caso("aes128_rsa", "srv_rsa", &["-ciphersuites", "TLS_AES_128_GCM_SHA256"], "TLS_AES_128_GCM_SHA256", "x25519");
    }

    #[test]
    #[ignore]
    fn tls_openssl_aes256_p384() {
        caso("aes256_p384", "srv_p384", &["-ciphersuites", "TLS_AES_256_GCM_SHA384"], "TLS_AES_256_GCM_SHA384", "x25519");
    }

    #[test]
    #[ignore]
    fn tls_openssl_hello_retry_p256() {
        caso("hrr_ec", "srv_ec", &["-ciphersuites", "TLS_CHACHA20_POLY1305_SHA256", "-groups", "P-256"], "TLS_CHACHA20_POLY1305_SHA256", "secp256r1");
    }

    #[test]
    #[ignore]
    fn tls_openssl_rsa_con_p256_y_aes128() {
        caso("sin_grabar_rsa_hrr", "srv_rsa", &["-ciphersuites", "TLS_AES_128_GCM_SHA256", "-groups", "P-256"], "TLS_AES_128_GCM_SHA256", "secp256r1");
    }

    #[test]
    #[ignore]
    fn tls_openssl_certificado_de_otro_host() {
        let (_s, puerto) = lanzar("srv_otro", &[]);
        let (r, recibido) = get(&mut fijo("prueba.nf", false), puerto);
        assert_eq!(r, Err(TlsError::Cert(CertError::NombreNoCoincide)));
        if std::env::var("NF_TLS_GRABAR").is_ok() {
            std::fs::write(datos().join("vuelo_otro_host.bin"), recibido).unwrap();
        }
        let mut c = fijo("prueba.nf", true);
        let (r, _) = get(&mut c, puerto);
        assert!(r.unwrap().starts_with(b"HTTP/1.0 200 ok"));
        assert_eq!(c.info().unwrap().cert_error, Some(CertError::NombreNoCoincide));
    }

    #[test]
    #[ignore]
    fn tls_openssl_con_new_y_semilla_distinta() {
        let (_s, puerto) = lanzar("srv_ec", &[]);
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let mut seed = [0u8; 32];
        seed[..16].copy_from_slice(&t.to_le_bytes());
        let mut c = Client::new("prueba.nf", ahora(), almacen(), false, seed);
        let (r, _) = get(&mut c, puerto);
        assert!(r.unwrap().starts_with(b"HTTP/1.0 200 ok"));
    }

    #[test]
    #[ignore]
    fn tls_openssl_ip_literal_sin_sni() {
        // El certificado solo tiene DNS: con la IP debe fallar por nombre, no por el saludo.
        let (_s, puerto) = lanzar("srv_ec", &[]);
        let mut c = Client::con_valores("127.0.0.1", ahora(), almacen(), false, Rng::new([3; 32]), RANDOM, SID.to_vec(), PRIV);
        let (r, _) = get(&mut c, puerto);
        assert_eq!(r, Err(TlsError::Cert(CertError::NombreNoCoincide)));
    }
}
