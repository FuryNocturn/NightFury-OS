// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Pruebas de x509. La PKI de `testdata/` la genera `testdata/gen.sh` (OpenSSL); la fecha de
//! las pruebas es fija (2026-11-23) para que no caduquen.

use super::der::{Reader, MAX_DER};
use super::*;
use alloc::string::ToString;
use alloc::vec;
use core::cell::RefCell;

macro_rules! der {
    ($n:literal) => {
        include_bytes!(concat!("testdata/", $n, ".der")) as &[u8]
    };
}

const RAIZ: &[u8] = der!("raiz");
const RAIZ384: &[u8] = der!("raiz384");
const INTER: &[u8] = der!("inter");
const HOJA: &[u8] = der!("hoja");
const TODOS: [&[u8]; 20] = [
    der!("raiz"), der!("raiz384"), der!("inter"), der!("hoja"), der!("comodin"), der!("ip"), der!("caducada"),
    der!("futura"), der!("otrodominio"), der!("sinserver"), der!("critica"), der!("pss"), der!("autofirmada"),
    der!("nocaint"), der!("hoja_noca"), der!("ncexcl"), der!("hoja_ncexcl"), der!("ncip"), der!("hoja384"),
    der!("hoja_ncperm"),
];

fn t(year: u16, month: u8, day: u8) -> Time {
    Time { year, month, day, hour: 12, min: 0, sec: 0 }
}
fn ahora() -> Time {
    t(2026, 11, 23)
}
fn si(_: &PubKey, _: SigAlg, _: &[u8], _: &[u8]) -> bool {
    true
}
fn no(_: &PubKey, _: SigAlg, _: &[u8], _: &[u8]) -> bool {
    false
}
fn raices() -> RootStore {
    let mut r = RootStore::new();
    assert_eq!(r.add_der(RAIZ), Ok(true));
    assert_eq!(r.add_der(RAIZ384), Ok(true));
    r
}
fn ver(chain: &[&[u8]], host: &str) -> Result<ChainInfo, CertError> {
    verify_server(chain, host, ahora(), &raices(), &si)
}

// ---- DER ----

#[test]
fn der_longitudes_estrictas() {
    let rd = |b: &[u8]| Reader::new(b).read().map(|t| t.content.len());
    assert_eq!(rd(&[0x04, 0x01, 0xaa]), Ok(1));
    assert!(rd(&[0x04, 0x81, 0x01, 0xaa]).is_err()); // forma larga innecesaria
    assert!(rd(&[0x04, 0x82, 0x00, 0x80]).is_err()); // cero inicial
    assert!(rd(&[0x04, 0x80, 0xaa, 0, 0]).is_err()); // indefinida
    assert!(rd(&[0x04, 0xff]).is_err());
    assert!(rd(&[0x04, 0x05, 0xaa]).is_err()); // truncada
    assert!(rd(&[0x1f, 0x01, 0x00]).is_err()); // etiqueta larga
    assert!(rd(&[0x04]).is_err());
    assert!(rd(&[0x04, 0x84, 0x7f, 0xff, 0xff, 0xff]).is_err()); // enorme
    let mut r = Reader::new(&[0x04, 0x01, 0xaa, 0x00]);
    r.read().unwrap();
    assert!(r.finish().is_err()); // bytes sobrantes
}

#[test]
fn der_enteros_y_booleanos() {
    let int = |b: &[u8]| der::int_bytes(Reader::new(b).read().unwrap()).is_ok();
    assert!(int(&[2, 1, 0]) && int(&[2, 2, 0, 0x80]) && int(&[2, 1, 0xff]));
    assert!(!int(&[2, 2, 0, 0x7f]) && !int(&[2, 2, 0xff, 0x80]) && !int(&[2, 0]));
    let b = |b: &[u8]| der::bool_val(Reader::new(b).read().unwrap());
    assert_eq!(b(&[1, 1, 0xff]), Ok(true));
    assert_eq!(b(&[1, 1, 0]), Ok(false));
    assert!(b(&[1, 1, 1]).is_err());
}

fn time(tag: u8, s: &str) -> R<Time> {
    let mut v = vec![tag, s.len() as u8];
    v.extend_from_slice(s.as_bytes());
    parse_time(Reader::new(&v).read().unwrap())
}

#[test]
fn tiempos() {
    assert_eq!(time(0x17, "490101000000Z").unwrap().year, 2049);
    assert_eq!(time(0x17, "500101000000Z").unwrap().year, 1950);
    assert_eq!(time(0x18, "20500101235959Z").unwrap(), Time { year: 2050, month: 1, day: 1, hour: 23, min: 59, sec: 59 });
    assert!(time(0x17, "240229000000Z").is_ok());
    assert!(time(0x17, "230229000000Z").is_err());
    assert!(time(0x17, "241301000000Z").is_err());
    assert!(time(0x17, "240101240000Z").is_err());
    assert!(time(0x17, "240101000060Z").is_err());
    assert!(time(0x17, "2401010000Z").is_err()); // sin segundos
    assert!(time(0x17, "240101000000+").is_err());
    assert!(time(0x18, "20240101000000.5Z").is_err()); // fracciones
    assert!(time(0x18, "2024010100000aZ").is_err());
    assert!(t(2026, 1, 2) > t(2025, 12, 31));
    assert!(Time { sec: 1, ..t(2026, 1, 1) } > t(2026, 1, 1));
}

// ---- nombres ----

#[test]
fn comodines_y_nombres() {
    assert!(dns_match("prueba.nf", "prueba.nf"));
    assert!(dns_match("*.prueba.nf", "www.prueba.nf"));
    assert!(!dns_match("*.prueba.nf", "prueba.nf"));
    assert!(!dns_match("*.prueba.nf", "a.b.prueba.nf"));
    assert!(!dns_match("*.nf", "prueba.nf")); // una sola etiqueta detrás
    assert!(!dns_match("*.*.nf", "a.b.nf"));
    assert!(!dns_match("w*.prueba.nf", "www.prueba.nf")); // solo etiqueta completa
    assert!(!dns_match("*.prueba.nf", ".prueba.nf"));
    assert!(!dns_match("*.prueba.nf", "xprueba.nf"));
    assert!(matches!(parse_host("PRUEBA.nf."), Host::Dns(h) if h == "prueba.nf"));
    assert!(matches!(parse_host("10.0.0.1"), Host::Ip(v) if v == [10, 0, 0, 1]));
    assert!(matches!(parse_host("10.0.0"), Host::Invalid));
    assert!(matches!(parse_host("010.0.0.1"), Host::Invalid));
    assert!(matches!(parse_host("256.0.0.1"), Host::Invalid));
    assert!(matches!(parse_host("[::1]"), Host::Ip(v) if v.len() == 16 && v[15] == 1));
    assert!(matches!(parse_host("2001:db8::1"), Host::Ip(v) if v[..4] == [0x20, 1, 0x0d, 0xb8]));
    assert!(matches!(parse_host("1:2:3:4:5:6:7:8"), Host::Ip(_)));
    assert!(matches!(parse_host("1:2:3:4:5:6:7::8"), Host::Invalid));
    assert!(matches!(parse_host("1::2::3"), Host::Invalid));
    assert!(matches!(parse_host(""), Host::Invalid));
    assert!(matches!(parse_host("ñandú.nf"), Host::Invalid));
}

#[test]
fn subarboles() {
    assert!(within("a.prueba.nf", "prueba.nf") && within("prueba.nf", "prueba.nf"));
    assert!(!within("xprueba.nf", "prueba.nf") && !within("prueba.nf", "a.prueba.nf"));
    assert!(within("a.prueba.nf", ".prueba.nf") && !within("prueba.nf", ".prueba.nf"));
    assert!(within("lo.que.sea", ""));
    let nc = NameConstraints { permitted: vec!["prueba.nf".to_string()], excluded: vec!["malo.prueba.nf".to_string()] };
    let ok = |n: &str| nc_allows(&nc, &[n.to_string()]);
    assert!(ok("www.prueba.nf") && !ok("malo.prueba.nf") && !ok("x.malo.prueba.nf") && !ok("otro.nf"));
    assert!(!ok("*.prueba.nf")); // el comodín incluiría malo.prueba.nf
    assert!(ok("*.www.prueba.nf"));
}

// ---- certificados ----

#[test]
fn cadena_correcta_y_datos() {
    let info = ver(&[HOJA, INTER], "prueba.nf").unwrap();
    assert_eq!((info.leaf.cn.as_str(), info.leaf.org.as_str()), ("prueba.nf", "NightFury Prueba"));
    assert_eq!(info.issuer.cn, "NF Intermedio");
    assert_eq!(info.leaf.sha256, crate::crypto::sha256(HOJA));
    assert_eq!(info.leaf.not_before, Time { year: 2026, month: 1, day: 1, hour: 0, min: 0, sec: 0 });
    assert_eq!(info.leaf.not_after.year, 2030);
    // mayúsculas, punto final y orden: intermedios desordenados, repetidos o de sobra
    assert!(ver(&[HOJA, INTER], "PRUEBA.NF.").is_ok());
    assert!(ver(&[HOJA, RAIZ384, INTER, INTER, RAIZ], "prueba.nf").is_ok());
    assert!(ver(&[HOJA, &[1, 2, 3], INTER], "prueba.nf").is_ok());
    // raíz de P-384 directa
    assert_eq!(ver(&[der!("hoja384")], "prueba.nf").unwrap().issuer.cn, "NF Raiz 384");
}

#[test]
fn errores_de_cadena() {
    use CertError::*;
    let e = |c: &[&[u8]], h: &str| ver(c, h).unwrap_err();
    assert_eq!(e(&[HOJA], "prueba.nf"), RaizDesconocida); // falta el intermedio
    assert_eq!(e(&[der!("autofirmada")], "prueba.nf"), Autofirmado);
    assert_eq!(e(&[der!("caducada"), INTER], "prueba.nf"), Caducado);
    assert_eq!(e(&[der!("futura"), INTER], "prueba.nf"), AunNoValido);
    assert_eq!(e(&[der!("otrodominio"), INTER], "prueba.nf"), NombreNoCoincide); // el CN no vale
    assert_eq!(e(&[HOJA, INTER], "otro.nf"), NombreNoCoincide);
    assert_eq!(e(&[der!("sinserver"), INTER], "prueba.nf"), UsoNoPermitido);
    assert_eq!(e(&[der!("hoja_noca"), der!("nocaint")], "prueba.nf"), UsoNoPermitido);
    assert_eq!(e(&[der!("hoja_ncexcl"), der!("ncexcl")], "prueba.nf"), UsoNoPermitido);
    assert_eq!(e(&[der!("hoja_ncperm"), der!("ncperm")], "prueba.nf"), UsoNoPermitido);
    assert_eq!(e(&[der!("hoja_ncip"), der!("ncip")], "prueba.nf"), NoSoportado);
    assert_eq!(e(&[der!("critica"), INTER], "prueba.nf"), NoSoportado);
    assert_eq!(e(&[der!("hoja_pl"), der!("inter2"), INTER], "prueba.nf"), UsoNoPermitido); // pathLen 0
    let larga = [der!("hoja_larga"), der!("c5"), der!("c4"), der!("c3"), der!("c2"), der!("c1")];
    assert_eq!(e(&larga, "prueba.nf"), CadenaDemasiadoLarga);
    assert_eq!(e(&larga[..2], "prueba.nf"), RaizDesconocida);
    assert_eq!(e(&[], "prueba.nf"), Formato);
    assert_eq!(e(&[&HOJA[..HOJA.len() - 1]], "prueba.nf"), Formato);
    // fechas de la raíz y de toda la cadena
    let ahora_2051 = verify_server(&[HOJA, INTER], "prueba.nf", t(2051, 1, 1), &raices(), &si);
    assert_eq!(ahora_2051, Err(Caducado));
    assert_eq!(verify_server(&[HOJA, INTER], "prueba.nf", t(2025, 6, 1), &raices(), &si), Err(AunNoValido));
    // sin raíces de confianza
    assert_eq!(verify_server(&[HOJA, INTER], "prueba.nf", ahora(), &RootStore::new(), &si), Err(RaizDesconocida));
    for err in [Caducado, AunNoValido, NombreNoCoincide, Autofirmado, RaizDesconocida, FirmaNoValida, UsoNoPermitido, CadenaDemasiadoLarga, NoSoportado, Formato] {
        assert!(!err.mensaje().is_empty());
    }
}

#[test]
fn comodin_e_ip() {
    let com = der!("comodin");
    assert!(ver(&[com, INTER], "www.prueba.nf").is_ok());
    assert_eq!(ver(&[com, INTER], "prueba.nf"), Err(CertError::NombreNoCoincide));
    assert_eq!(ver(&[com, INTER], "a.b.prueba.nf"), Err(CertError::NombreNoCoincide));
    let ip = der!("ip");
    for h in ["192.168.1.10", "2001:db8::1", "[2001:DB8:0:0:0:0:0:1]"] {
        assert!(ver(&[ip, INTER], h).is_ok(), "{h}");
    }
    for h in ["192.168.1.11", "2001:db8::2", "prueba.nf", "192.168.1"] {
        assert_eq!(ver(&[ip, INTER], h), Err(CertError::NombreNoCoincide), "{h}");
    }
    assert_eq!(ver(&[HOJA, INTER], "192.168.1.10"), Err(CertError::NombreNoCoincide));
}

#[test]
fn firmas_y_algoritmos() {
    let log = RefCell::new(vec![]);
    let rec = |k: &PubKey, a: SigAlg, m: &[u8], s: &[u8]| {
        log.borrow_mut().push((k.clone(), a, m.to_vec(), s.len()));
        true
    };
    verify_server(&[HOJA, INTER], "prueba.nf", ahora(), &raices(), &rec).unwrap();
    {
        let l = log.borrow();
        assert_eq!(l.len(), 2);
        // hoja: firmada con la clave EC del intermedio; el mensaje es el tbs de la hoja
        assert!(matches!(&l[0].0, PubKey::EcP256(p) if p.len() == 65 && p[0] == 4));
        assert_eq!(l[0].1, SigAlg::EcdsaSha256);
        assert!(HOJA.windows(l[0].2.len()).any(|w| w == &l[0].2[..]) && l[0].2.len() > 100 && l[0].2[0] == 0x30);
        // intermedio: firmado con la clave RSA de la raíz
        assert!(matches!(&l[1].0, PubKey::Rsa { n, e } if n.len() == 256 && n[0] != 0 && e == &[1, 0, 1]));
        assert_eq!(l[1].1, SigAlg::RsaPkcs1Sha256);
    }
    log.borrow_mut().clear();
    verify_server(&[der!("pss")], "prueba.nf", ahora(), &raices(), &rec).unwrap();
    assert_eq!(log.borrow()[0].1, SigAlg::RsaPssSha256);
    log.borrow_mut().clear();
    verify_server(&[der!("hoja384")], "prueba.nf", ahora(), &raices(), &rec).unwrap();
    assert_eq!(log.borrow()[0].1, SigAlg::EcdsaSha384);
    assert!(matches!(&log.borrow()[0].0, PubKey::EcP384(p) if p.len() == 97));
    // firma que no vale
    assert_eq!(verify_server(&[HOJA, INTER], "prueba.nf", ahora(), &raices(), &no), Err(CertError::FirmaNoValida));
    assert_eq!(verify_server(&[der!("pss")], "prueba.nf", ahora(), &raices(), &no), Err(CertError::FirmaNoValida));
}

#[test]
fn raiz_de_confianza_y_paquete() {
    let mut b = Vec::new();
    for d in [RAIZ, &[0x30, 0x03, 1, 2, 3][..], RAIZ384] {
        b.extend_from_slice(&(d.len() as u16).to_be_bytes());
        b.extend_from_slice(d);
    }
    let mut s = RootStore::new();
    assert_eq!(s.load_bundle(&b), Ok(2)); // la basura se salta
    assert_eq!(s.load_bundle(&b), Ok(0)); // sin duplicados
    assert_eq!(s.len(), 2);
    let l = s.list();
    assert_eq!(l[0].cn, "NF Raiz");
    assert_eq!(l[0].not_after.year, 2050); // GeneralizedTime
    assert_eq!(l[1].sha256, crate::crypto::sha256(RAIZ384));
    assert!(verify_server(&[HOJA, INTER], "prueba.nf", ahora(), &s, &si).is_ok());
    assert_eq!(RootStore::new().load_bundle(&b[..b.len() - 1]), Err(CertError::Formato));
    assert_eq!(RootStore::new().load_bundle(&[0]), Err(CertError::Formato));
    assert_eq!(RootStore::new().load_bundle(&[0, 0]), Err(CertError::Formato));
    assert_eq!(RootStore::new().add_der(&vec![0x30; MAX_DER + 1]), Err(CertError::Formato));
}

// ---- PEM ----

fn b64(d: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in d.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= c.len() { A[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

#[test]
fn pem_y_base64() {
    for n in 0..20u8 {
        let d: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37)).collect();
        assert_eq!(pem_base64(&b64(&d)), Some(d));
    }
    assert!(pem_base64("QQ=").is_none() && pem_base64("QR==").is_none() && pem_base64("Q===").is_none());
    assert!(pem_base64("QQ==QQ==").is_none() && pem_base64("QQ$=").is_none() && pem_base64("Q").is_none());
    let bloque = |d: &[u8]| {
        let c = b64(d);
        let lineas: Vec<&str> = c.as_bytes().chunks(64).map(|l| core::str::from_utf8(l).unwrap()).collect();
        format!("-----BEGIN CERTIFICATE-----\r\n{}\r\n-----END CERTIFICATE-----\r\n", lineas.join("\r\n"))
    };
    let txt = format!("Subject: x\n{}texto suelto\n{}", bloque(RAIZ), bloque(INTER));
    assert_eq!(pem_or_der(txt.as_bytes()), vec![RAIZ.to_vec(), INTER.to_vec()]);
    assert_eq!(pem_or_der(RAIZ), vec![RAIZ.to_vec()]);
    assert!(pem_or_der(b"hola").is_empty() && pem_or_der(b"").is_empty());
    assert!(pem_or_der(b"-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----").is_empty());
    assert!(pem_or_der(b"-----BEGIN CERTIFICATE-----\nQUJD").is_empty()); // sin cierre
}

fn pem_base64(s: &str) -> Option<Vec<u8>> {
    super::pem::base64_decode(s.as_bytes())
}

// ---- entrada hostil ----

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn truncados_nunca_hacen_panic() {
    for d in TODOS {
        for n in 0..d.len() {
            let _ = Cert::parse(&d[..n]);
        }
        assert!(Cert::parse(d).is_ok());
    }
}

#[test]
fn fuzz_determinista() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let r = raices();
    for d in TODOS {
        for _ in 0..600 {
            let mut m = d.to_vec();
            for _ in 0..1 + rng.next() % 4 {
                let i = rng.next() as usize % m.len();
                m[i] = rng.next() as u8;
            }
            match rng.next() % 6 {
                0 => m.truncate(rng.next() as usize % m.len()),
                1 => m.push(rng.next() as u8),
                _ => {}
            }
            let _ = Cert::parse(&m);
            let _ = verify_server(&[&m, INTER], "prueba.nf", ahora(), &r, &si);
            let _ = verify_server(&[HOJA, &m], "prueba.nf", ahora(), &r, &si);
            let mut s = RootStore::new();
            let _ = s.add_der(&m);
            let _ = pem_or_der(&m);
        }
    }
}

// ---- firmas reales (pk::verify) y raíces de Mozilla ----

#[test]
fn cadenas_con_firmas_reales() {
    let r = raices();
    let v = |c: &[&[u8]]| verify_server(c, "prueba.nf", ahora(), &r, &crate::pk::verify);
    assert!(v(&[HOJA, INTER]).is_ok()); // RSA (raíz) → EC P-256 (intermedio)
    assert!(v(&[der!("hoja384")]).is_ok()); // P-384
    assert!(v(&[der!("pss"), INTER]).is_ok() || v(&[der!("pss")]).is_ok());
    let mut mala = HOJA.to_vec();
    let n = mala.len();
    mala[n - 5] ^= 1; // dentro de la firma
    assert_eq!(v(&[&mala, INTER]), Err(CertError::FirmaNoValida));
}

#[test]
fn raices_de_mozilla() {
    let bundle = include_bytes!("../../data/raices/raices.bin");
    let mut r = RootStore::new();
    let n = r.load_bundle(bundle).unwrap();
    assert_eq!(n, 109, "todas las raíces del paquete deben leerse");
    // Cada raíz va firmada por sí misma: sirve de prueba de RSA/ECDSA con datos reales.
    let mut b: &[u8] = bundle;
    let (mut ok, mut total) = (0, 0);
    while b.len() >= 2 {
        let l = (b[0] as usize) << 8 | b[1] as usize;
        let c = Cert::parse(&b[2..2 + l]).unwrap();
        b = &b[2 + l..];
        if let (Some(alg), Some(k)) = (c.sig_alg, c.key.as_ref()) {
            total += 1;
            if crate::pk::verify(k, alg, &c.der[c.tbs.0..c.tbs.1], &c.sig) {
                ok += 1;
            }
        }
    }
    assert_eq!(ok, total, "autofirmas válidas: {ok}/{total}");
    assert!(total >= 100);
}
