// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Pruebas de TLS 1.3 y 1.2.
//!
//! - RFC 8448 §3: programa de claves con el ClientHello y el ServerHello del documento.
//! - `vuelo_*.bin` (testdata): lo que respondió un `openssl s_server` a nuestro ClientHello fijo (que es
//!   siempre el mismo bytes a bytes), para reproducirlo sin red. Se regraban con
//!   `NF_TLS_GRABAR=1 cargo test -p nf-core -- --ignored tls` (las claves y certificados de `testdata`
//!   son SOLO de prueba; los genera `testdata/gen.sh`).
//! - Interoperabilidad con OpenSSL (`#[ignore]`): `cargo test -p nf-core -- --ignored tls`.

use super::hs12::prf;
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
/// Vuelos de TLS 1.2: (nombre, bytes, suite, grupo, ¿extended_master_secret?).
const VUELOS12: [(&str, &[u8], &str, &str, bool); 7] = [
    ("c12_chacha_ec", include_bytes!("testdata/vuelo_c12_chacha_ec.bin"), "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256", "x25519", true),
    ("c12_chacha_rsa_pss", include_bytes!("testdata/vuelo_c12_chacha_rsa_pss.bin"), "TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256", "x25519", true),
    ("c12_aes128_ec_sha384", include_bytes!("testdata/vuelo_c12_aes128_ec_sha384.bin"), "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256", "x25519", true),
    ("c12_aes128_rsa_pkcs1", include_bytes!("testdata/vuelo_c12_aes128_rsa_pkcs1.bin"), "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256", "secp256r1", true),
    ("c12_aes256_ec", include_bytes!("testdata/vuelo_c12_aes256_ec.bin"), "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384", "secp256r1", true),
    ("c12_aes256_rsa", include_bytes!("testdata/vuelo_c12_aes256_rsa.bin"), "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384", "x25519", true),
    ("c12_sin_ems", include_bytes!("testdata/vuelo_c12_sin_ems.bin"), "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256", "x25519", false),
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
    // Suites: las de 1.3 (ChaCha20, AES-128, AES-256) y luego las de 1.2.
    assert_eq!(&o[76..96], &[0, 18, 0x13, 3, 0x13, 1, 0x13, 2, 0xcc, 0xa9, 0xcc, 0xa8, 0xc0, 0x2b, 0xc0, 0x2f, 0xc0, 0x2c, 0xc0, 0x30]);
    // supported_versions: 1.3 y 1.2; renegotiation_info vacía y extended_master_secret.
    assert!(o.windows(7).any(|w| w == [0x00, 0x2b, 0x00, 0x05, 4, 3, 4]));
    assert!(o.windows(5).any(|w| w == [0xff, 0x01, 0x00, 0x01, 0x00]));
    assert!(o.windows(4).any(|w| w == [0x00, 0x17, 0x00, 0x00]));
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
// TLS 1.2
// ---------------------------------------------------------------------------------------------

#[test]
fn prf_de_tls12() {
    // Vectores de PRF (secreto, etiqueta, semilla); la salida de referencia viene de `openssl kdf TLS1-PRF`.
    let sal = prf(super::keys::Hash::S256, &hex("9bbe436ba940f017b17652849a71db35"), "test label", &hex("a0ba9f936cda311827a6f796ffd5198c"), 100);
    assert_eq!(&*sal, &hex("e3f229ba727be17b8d122620557cd453c2aab21d07c3d495329b52d4e61edb5a6b301791e90d35c9c9a46b4e14baf9af0fa022f7077def17abfd3797c0564bab4fbc91666e9def9b97fce34f796789baa48082d122ee42c5a72e5a5110fff70187347b66")[..]);
    let sal = prf(super::keys::Hash::S384, &hex("b80b733d6ceefcdc71566ea48e5567df"), "test label", &hex("d4640e12e4bcdbfb437f03e6ae418ee5"), 48);
    assert_eq!(&*sal, &hex("4080bbcbdd175e5f6bfda2777b8ef4279074f9e398429a73e1dfd004a41ba996bf4a19c7922ed22cc4157fa6f3d41b18")[..]);
    // La salida corta es prefijo de la larga.
    let a = prf(super::keys::Hash::S256, b"k", "l", b"s", 10);
    let b = prf(super::keys::Hash::S256, b"k", "l", b"s", 70);
    assert_eq!(&a[..], &b[..10]);
}

/// Copia de las claves de un sentido (para hacer de servidor o comprobar lo que escribió el cliente).
fn espejo(k: &Keys, a: record::Aead) -> Keys {
    let il = if a == record::Aead::Chacha { 12 } else { 4 };
    let mut n = Keys::new12(a, Secret(k.key.to_vec()), &k.iv[..il]);
    n.seq = k.seq;
    n
}

#[test]
fn reproduce_vuelos_tls12() {
    for (nombre, v, suite, grupo, ems) in VUELOS12 {
        for t in [v.len(), 1, 7] {
            let (mut c, r) = reproducir("prueba.nf", false, v, t);
            r.unwrap_or_else(|e| panic!("{nombre}/{t}: {}", e.mensaje()));
            assert!(c.is_ready(), "{nombre}");
            let i = c.info().unwrap();
            assert_eq!((i.version, i.suite, i.group), ("TLS 1.2", suite, grupo), "{nombre}");
            assert_eq!(c.s12.ems, ems, "{nombre}");
            assert_eq!(i.chain.as_ref().unwrap().leaf.cn, "prueba.nf");
            assert!(c.read().starts_with(b"HTTP/1.0 200 ok"), "{nombre}");
            assert!(c.peer_closed(), "{nombre}");
        }
    }
}

/// Cliente que acaba de terminar el saludo de un vuelo, y el desplazamiento justo después.
fn conectado12(v: &[u8]) -> (Client, usize) {
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

/// (inicio del registro, inicio del mensaje) del primer mensaje de saludo de tipo `t` en claro.
fn mensaje12(v: &[u8], t: u8) -> (usize, usize) {
    for (p, ct, n) in registros(v) {
        if ct != 22 {
            break;
        }
        let mut q = p + 5;
        while q + 4 <= p + 5 + n {
            if v[q] == t {
                return (p, q);
            }
            q += 4 + (((v[q + 1] as usize) << 16) | ((v[q + 2] as usize) << 8) | v[q + 3] as usize);
        }
    }
    panic!("sin mensaje {t}");
}

fn con_parche12(i: usize, f: impl Fn(&mut Vec<u8>)) -> (Client, Result<(), TlsError>) {
    let mut v = VUELOS12[i].1.to_vec();
    f(&mut v);
    let mut c = fijo("prueba.nf", false);
    c.take_output();
    let r = c.feed(&v);
    (c, r)
}

#[test]
fn serverhello12_incorrecto() {
    let suite = 44 + VUELOS12[0].1[43] as usize;
    // Suite que no se ofreció (CBC).
    let (mut c, r) = con_parche12(0, |v| v[suite..suite + 2].copy_from_slice(&[0xc0, 0x13]));
    assert!(matches!(r, Err(TlsError::Protocolo(m)) if m.contains("suite")));
    assert_eq!(c.take_output(), [21, 3, 3, 0, 2, 2, 47]);
    // Una suite de 1.3 sin supported_versions tampoco vale.
    assert!(con_parche12(0, |v| v[suite..suite + 2].copy_from_slice(&[0x13, 0x01])).1.is_err());
    // El servidor solo admite 1.1: error claro y alerta protocol_version.
    let (mut c, r) = con_parche12(0, |v| v[9..11].copy_from_slice(&[3, 2]));
    assert!(matches!(r, Err(TlsError::NoSoportado(m)) if m.contains("TLS 1.1")));
    assert_eq!(c.take_output(), [21, 3, 3, 0, 2, 2, 70]);
    assert!(matches!(con_parche12(0, |v| v[9..11].copy_from_slice(&[3, 5])).1, Err(TlsError::Protocolo(_))));
    // Centinelas de degradación con TLS 1.2 negociado (ofrecimos 1.3).
    for ultimo in [0u8, 1] {
        let (_, r) = con_parche12(0, |v| {
            v[11 + 24..11 + 31].copy_from_slice(b"DOWNGRD");
            v[11 + 31] = ultimo;
        });
        assert!(matches!(r, Err(TlsError::Protocolo(m)) if m.contains("degradación")));
    }
    // Compresión, renegotiation_info no vacía y una extensión que no se pidió.
    assert!(con_parche12(0, |v| v[suite + 2] = 1).1.is_err());
    let reneg = |v: &Vec<u8>| v.windows(5).position(|w| w == [0xff, 0x01, 0x00, 0x01, 0x00]).unwrap();
    let (mut c, r) = con_parche12(0, |v| {
        let n = reneg(v);
        v[n + 4] = 1;
    });
    assert!(matches!(r, Err(TlsError::Protocolo(m)) if m.contains("renegociación")));
    assert_eq!(c.take_output(), [21, 3, 3, 0, 2, 2, 40]);
    assert!(con_parche12(0, |v| {
        let n = reneg(v);
        v[n] = 0xfe;
    }).1.is_err());
}

#[test]
fn serverkeyexchange_incorrecto() {
    for i in [0, 1, 3] {
        let v = VUELOS12[i].1;
        let (_, ske) = mensaje12(v, 12);
        let long = ((v[ske + 1] as usize) << 16) | ((v[ske + 2] as usize) << 8) | v[ske + 3] as usize;
        let alg = ske + 4 + 4 + v[ske + 4 + 3] as usize;
        // Firma mala (último byte) y punto cambiado (la firma ya no cuadra).
        let (mut c, r) = con_parche12(i, |v| v[ske + 4 + long - 1] ^= 1);
        assert!(matches!(r, Err(TlsError::Protocolo(m)) if m.contains("ServerKeyExchange")), "{i}");
        assert_eq!(c.take_output(), [21, 3, 3, 0, 2, 2, 51]);
        assert!(con_parche12(i, |v| v[ske + 4 + 4] ^= 1).1.is_err());
        // Curva que no se ofreció (secp384r1), tipo de curva que no es «con nombre», punto con longitud rara.
        let (_, r) = con_parche12(i, |v| v[ske + 5..ske + 7].copy_from_slice(&[0, 0x18]));
        assert!(matches!(r, Err(TlsError::Protocolo(m)) if m.contains("curva")), "{i}");
        assert!(con_parche12(i, |v| v[ske + 4] = 1).1.is_err());
        assert!(con_parche12(i, |v| v[ske + 7] = 31).1.is_err());
        // Algoritmo de firma no ofrecido (RSA PKCS#1 con SHA-1), o que no casa con la clave del certificado.
        let (_, r) = con_parche12(i, |v| v[alg..alg + 2].copy_from_slice(&[0x02, 0x01]));
        assert!(matches!(r, Err(TlsError::Protocolo(m)) if m.contains("firma")), "{i}");
        let otro: [u8; 2] = if i == 0 { [0x08, 0x04] } else { [0x04, 0x03] };
        assert!(con_parche12(i, |v| v[alg..alg + 2].copy_from_slice(&otro)).1.is_err(), "{i}");
    }
}

#[test]
fn certificate_request_y_finished_malo() {
    let v = VUELOS12[2].1;
    // CertificateRequest antes del ServerHelloDone: el cliente contesta con un Certificate vacío
    // (el Finished del servidor ya no cuadra porque el transcript cambió: acaba en error).
    let (rec, msg) = mensaje12(v, 14);
    assert_eq!(rec + 5, msg);
    let cr = [13, 0, 0, 8, 1, 0x40, 0, 2, 4, 3, 0, 0];
    let mut m = v[..rec].to_vec();
    m.extend_from_slice(&[22, 3, 3, 0, cr.len() as u8]);
    m.extend_from_slice(&cr);
    m.extend_from_slice(&v[rec..]);
    let mut c = fijo("prueba.nf", false);
    c.take_output();
    assert!(c.feed(&m).is_err());
    assert!(c.take_output().windows(12).any(|w| w == [22, 3, 3, 0, 7, 11, 0, 0, 3, 0, 0, 0]));
    // Finished con verify_data incorrecto, bien cifrado, y datos antes del Finished: se rechazan.
    for (i, f) in VUELOS12.iter().enumerate() {
        let ccs = registros(f.1).into_iter().find(|r| r.1 == 20).unwrap();
        for ct in [22u8, 23] {
            let mut c = fijo("prueba.nf", false);
            c.take_output();
            c.feed(&f.1[..ccs.0 + 5 + ccs.2]).unwrap();
            let mut k = espejo(c.rx.as_ref().unwrap(), c.suite.unwrap().aead);
            let datos: &[u8] = if ct == 22 { &[20, 0, 0, 12, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12] } else { b"hola" };
            let e = c.feed(&k.seal(ct, datos).unwrap()).unwrap_err();
            assert!(matches!(e, TlsError::Protocolo(_)), "{i}/{ct}");
            assert!(!c.is_ready());
        }
    }
    // Un ChangeCipherSpec repetido, o con el servidor aún en medio del saludo en claro tras el ServerHello.
    let ccs = registros(v).into_iter().find(|r| r.1 == 20).unwrap();
    let mut c = fijo("prueba.nf", false);
    c.take_output();
    c.feed(&v[..ccs.0 + 5 + ccs.2]).unwrap();
    assert!(c.feed(&[20, 3, 3, 0, 1, 1]).is_err());
    let sh = registros(v)[0];
    let mut c = fijo("prueba.nf", false);
    c.take_output();
    c.feed(&v[..sh.0 + 5 + sh.2]).unwrap();
    assert!(c.feed(&[20, 3, 3, 0, 1, 1]).is_err());
}

#[test]
fn tls12_ya_conectado() {
    for (nombre, v, _, _, _) in VUELOS12 {
        let (mut c, _) = conectado12(v);
        let a = c.suite.unwrap().aead;
        let mut s = espejo(c.rx.as_ref().unwrap(), a);
        // HelloRequest: se ignora y todo sigue.
        c.feed(&s.seal(CT_SALUDO, &[0, 0, 0, 0]).unwrap()).unwrap();
        assert!(c.is_ready() && c.take_output().is_empty(), "{nombre}");
        // Escritura en registros de hasta 16 KiB que descifra una copia de las claves.
        let mut k = espejo(c.tx.as_ref().unwrap(), a);
        let datos: Vec<u8> = (0..40_000u32).map(|i| (i * 7) as u8).collect();
        c.write(&datos).unwrap();
        let o = c.take_output();
        let r = registros(&o);
        assert_eq!(r.len(), 3, "{nombre}");
        let mut claro = Vec::new();
        for (p, ct, n) in r {
            assert!(n <= MAX_CIFRADO);
            assert_eq!(ct, CT_DATOS);
            let (t, d) = k.open(&o[p..p + 5], &o[p + 5..p + 5 + n]).unwrap();
            assert_eq!(t, CT_DATOS);
            claro.extend(d);
        }
        assert_eq!(claro, datos, "{nombre}");
        // Lectura, también de un registro vacío.
        c.feed(&s.seal(CT_DATOS, b"respuesta").unwrap()).unwrap();
        c.feed(&s.seal(CT_DATOS, b"").unwrap()).unwrap();
        assert_eq!(c.read(), b"respuesta");
        // Mensajes que en 1.2 no existen o no vienen a cuento: HelloRequest con cuerpo, NewSessionTicket, KeyUpdate, ServerHelloDone.
        for m in [&[0u8, 0, 0, 1, 0][..], &[4, 0, 0, 0], &[24, 0, 0, 1, 0], &[14, 0, 0, 0]] {
            let (mut c, _) = conectado12(v);
            let mut s = espejo(c.rx.as_ref().unwrap(), a);
            assert!(c.feed(&s.seal(CT_SALUDO, m).unwrap()).is_err(), "{nombre}");
        }
        // Registro corrupto: error y todo cerrado.
        let (mut c, _) = conectado12(v);
        let mut s = espejo(c.rx.as_ref().unwrap(), a);
        let mut r = s.seal(CT_DATOS, b"datos").unwrap();
        let n = r.len() - 1;
        r[n] ^= 1;
        assert!(matches!(c.feed(&r), Err(TlsError::Protocolo(_))));
        assert_eq!(c.feed(&[0]), Err(TlsError::Cerrado));
        // close_notify del servidor: se anota y se contesta con el nuestro.
        let (mut c, _) = conectado12(v);
        let mut s = espejo(c.rx.as_ref().unwrap(), a);
        let mut k = espejo(c.tx.as_ref().unwrap(), a);
        c.feed(&s.seal(CT_ALERTA, &[1, 0]).unwrap()).unwrap();
        assert!(c.peer_closed());
        let o = c.take_output();
        assert_eq!(k.open(&o[..5], &o[5..]).unwrap(), (CT_ALERTA, vec![1, 0]), "{nombre}");
        assert_eq!(c.write(b"x"), Err(TlsError::Cerrado));
        // close propio.
        let (mut c, _) = conectado12(v);
        c.close();
        assert!(!c.take_output().is_empty());
        assert_eq!(c.write(b"x"), Err(TlsError::Cerrado));
    }
}

#[test]
fn ningun_cambio_del_saludo12_se_acepta() {
    // Como en 1.3: cambiar un byte del saludo (hasta el Finished del servidor) da error o lo deja sin terminar,
    // salvo la versión de cada cabecera de registro, que se ignora a propósito.
    for i in [0, 3] {
        let v = VUELOS12[i].1;
        let fin = conectado12(v).1;
        let cab: Vec<usize> = registros(v).iter().map(|x| x.0 + 2).collect();
        for p in (0..fin).filter(|p| !cab.contains(p)) {
            let mut m = v[..fin].to_vec();
            m[p] ^= 0x01;
            let (c, e) = reproducir("prueba.nf", false, &m, 1 << 20);
            assert!(e.is_err() || !c.is_ready(), "vuelo {i}, byte {p} aceptado");
        }
    }
}

#[test]
fn fuzz12_sin_panico() {
    let mut x = Xs(0x1234_5678_9abc_def1);
    for (_, v, _, _, _) in VUELOS12 {
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

    /// `proto`: "-tls1_3", "-tls1_2" o "" (los dos).
    fn lanzar(proto: &str, cert: &str, extra: &[&str]) -> (Servidor, u16) {
        let puerto = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let d = datos();
        let exe = std::env::var("OPENSSL").unwrap_or_else(|_| "openssl".into());
        let hijo = Command::new(exe)
            .args(["s_server", "-www", "-accept", &format!("127.0.0.1:{puerto}")])
            .args(Some(proto).filter(|p| !p.is_empty()))
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
        let (_s, puerto) = lanzar("-tls1_3", cert, extra);
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
        let (_s, puerto) = lanzar("-tls1_3", "srv_otro", &[]);
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
        let (_s, puerto) = lanzar("-tls1_3", "srv_ec", &[]);
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
        let (_s, puerto) = lanzar("-tls1_3", "srv_ec", &[]);
        let mut c = Client::con_valores("127.0.0.1", ahora(), almacen(), false, Rng::new([3; 32]), RANDOM, SID.to_vec(), PRIV);
        let (r, _) = get(&mut c, puerto);
        assert_eq!(r, Err(TlsError::Cert(CertError::NombreNoCoincide)));
    }

    /// Una combinación de TLS 1.2: `cifrado` es el nombre OpenSSL de la suite.
    fn caso12(nombre: &str, cert: &str, cifrado: &str, extra: &[&str], suite: &str, grupo: &str, ems: bool) {
        let mut args = vec!["-cipher", cifrado];
        args.extend_from_slice(extra);
        let (_s, puerto) = lanzar("-tls1_2", cert, &args);
        let mut c = fijo("prueba.nf", false);
        let (r, recibido) = get(&mut c, puerto);
        let resp = r.unwrap_or_else(|e| panic!("{nombre}: {}", e.mensaje()));
        let i = c.info().expect("info");
        assert_eq!((i.version, i.suite, i.group), ("TLS 1.2", suite, grupo), "{nombre}");
        assert_eq!(c.s12.ems, ems, "{nombre}");
        let texto = String::from_utf8_lossy(&resp);
        assert!(texto.starts_with("HTTP/1.0 200 ok"), "{nombre}: {texto}");
        assert!(texto.contains(&format!("Cipher is {cifrado}")) && texto.contains("TLSv1.2"), "{nombre}: {texto}");
        if std::env::var("NF_TLS_GRABAR").is_ok() && VUELOS12.iter().any(|v| v.0 == nombre) {
            std::fs::write(datos().join(format!("vuelo_{nombre}.bin")), recibido).unwrap();
        }
    }

    #[test]
    #[ignore]
    fn tls12_openssl_chacha_ecdsa() {
        caso12("c12_chacha_ec", "srv_ec", "ECDHE-ECDSA-CHACHA20-POLY1305", &[], "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256", "x25519", true);
    }

    #[test]
    #[ignore]
    fn tls12_openssl_chacha_rsa_pss() {
        caso12("c12_chacha_rsa_pss", "srv_rsa", "ECDHE-RSA-CHACHA20-POLY1305", &["-sigalgs", "RSA-PSS+SHA256"], "TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256", "x25519", true);
    }

    #[test]
    #[ignore]
    fn tls12_openssl_aes128_ecdsa_con_sha384() {
        caso12("c12_aes128_ec_sha384", "srv_ec", "ECDHE-ECDSA-AES128-GCM-SHA256", &["-sigalgs", "ECDSA+SHA384"], "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256", "x25519", true);
    }

    #[test]
    #[ignore]
    fn tls12_openssl_aes128_rsa_pkcs1_p256() {
        caso12("c12_aes128_rsa_pkcs1", "srv_rsa", "ECDHE-RSA-AES128-GCM-SHA256", &["-sigalgs", "RSA+SHA256", "-groups", "P-256"], "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256", "secp256r1", true);
    }

    #[test]
    #[ignore]
    fn tls12_openssl_aes256_ecdsa_p256() {
        caso12("c12_aes256_ec", "srv_ec", "ECDHE-ECDSA-AES256-GCM-SHA384", &["-groups", "P-256"], "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384", "secp256r1", true);
    }

    #[test]
    #[ignore]
    fn tls12_openssl_aes256_rsa() {
        caso12("c12_aes256_rsa", "srv_rsa", "ECDHE-RSA-AES256-GCM-SHA384", &[], "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384", "x25519", true);
    }

    #[test]
    #[ignore]
    fn tls12_openssl_sin_extended_master_secret() {
        caso12("c12_sin_ems", "srv_ec", "ECDHE-ECDSA-AES128-GCM-SHA256", &["-no_ems"], "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256", "x25519", false);
        caso12("sin_grabar_rsa_sin_ems", "srv_rsa", "ECDHE-RSA-CHACHA20-POLY1305", &["-no_ems", "-groups", "P-256"], "TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256", "secp256r1", false);
    }

    #[test]
    #[ignore]
    fn tls12_openssl_certificado_de_otro_host() {
        let (_s, puerto) = lanzar("-tls1_2", "srv_otro", &[]);
        let (r, _) = get(&mut fijo("prueba.nf", false), puerto);
        assert_eq!(r, Err(TlsError::Cert(CertError::NombreNoCoincide)));
        let mut c = fijo("prueba.nf", true);
        let (r, _) = get(&mut c, puerto);
        assert!(r.unwrap().starts_with(b"HTTP/1.0 200 ok"));
        let i = c.info().unwrap();
        assert_eq!((i.version, i.cert_error), ("TLS 1.2", Some(CertError::NombreNoCoincide)));
    }

    #[test]
    #[ignore]
    fn tls12_openssl_con_new_y_semilla_distinta() {
        let (_s, puerto) = lanzar("-tls1_2", "srv_ec", &[]);
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let mut seed = [0u8; 32];
        seed[..16].copy_from_slice(&t.to_le_bytes());
        let mut c = Client::new("prueba.nf", ahora(), almacen(), false, seed);
        let (r, _) = get(&mut c, puerto);
        assert!(r.unwrap().starts_with(b"HTTP/1.0 200 ok"));
        assert_eq!(c.info().unwrap().version, "TLS 1.2");
    }

    #[test]
    #[ignore]
    fn tls12_openssl_servidor_con_1_2_y_1_3_negocia_1_3() {
        let (_s, puerto) = lanzar("", "srv_ec", &[]);
        let mut c = fijo("prueba.nf", false);
        let (r, _) = get(&mut c, puerto);
        assert!(r.unwrap().starts_with(b"HTTP/1.0 200 ok"));
        assert_eq!(c.info().unwrap().version, "TLS 1.3");
    }
}
