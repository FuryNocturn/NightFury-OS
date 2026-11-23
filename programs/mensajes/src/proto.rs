// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Protocolo de Mensajes (docs/Mensajes-protocolo.md): anuncios, saludo, claves, tramas y palabras de
//! verificación. Es lógica pura: todo lo que llega por la red es hostil, así que nada aquí entra en
//! pánico con datos de fuera (todo devuelve `Option`/`Result`). Enteros de la red en big endian,
//! salvo el contador del nonce (LE), que fija el protocolo.
use crate::palabras::LISTA;
use alloc::string::String;
use alloc::vec::Vec;
use nf_core::crypto::{aead_open, aead_seal, ct_eq, hkdf_expand, hkdf_extract, sha256, to_hex, wipe, x25519, X25519_BASE};
use nf_core::platform::DateTime;

pub type Clave = [u8; 32];

pub const PUERTO_UDP: u16 = 47320;
pub const PUERTO_TCP: u16 = 47321;
pub const MAX_TRAMA: usize = 4112;
pub const MAX_NOMBRE: usize = 32;
pub const MAX_TEXTO_BYTES: usize = 4000;
pub const MAX_TEXTO_CHARS: usize = 2000;
pub const HOLA_LEN: usize = 69;
pub const HOLA_I: u8 = 1;
pub const HOLA_R: u8 = 2;
const ETIQUETA: usize = 16;

pub fn publica(privada: &Clave) -> Clave {
    x25519(privada, &X25519_BASE)
}

/// Las 16 primeras cifras hex de `sha256(clave pública)`.
pub fn huella(pk: &Clave) -> String {
    to_hex(&sha256(pk)[..8])
}

/// Las 6 palabras de verificación de una conversación: iguales en los dos lados.
pub fn palabras(a: &Clave, b: &Clave) -> [&'static str; 6] {
    let (menor, mayor) = if a <= b { (a, b) } else { (b, a) };
    let mut m = Vec::from(&b"NFM1 verificar"[..]);
    m.extend_from_slice(menor);
    m.extend_from_slice(mayor);
    let h = sha256(&m);
    core::array::from_fn(|i| LISTA[h[i] as usize])
}

pub fn nombre_valido(n: &str) -> bool {
    !n.is_empty() && n.len() <= MAX_NOMBRE && !n.chars().any(char::is_control)
}

pub fn texto_valido(t: &str) -> bool {
    !t.is_empty() && t.len() <= MAX_TEXTO_BYTES && t.chars().count() <= MAX_TEXTO_CHARS
}

/// Cambia por `?` los caracteres de control salvo el salto de línea y el tabulador.
pub fn limpiar(t: &str) -> String {
    t.chars().map(|c| if c.is_control() && c != '\n' && c != '\t' { '?' } else { c }).collect()
}

// ---------------------------------------------------------------- anuncio (UDP)

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Anuncio {
    pub pk: Clave,
    pub puerto: u16,
    pub nombre: String,
}

/// El anuncio de esta identidad (el puerto es siempre `PUERTO_TCP`).
pub fn anuncio(pk: &Clave, nombre: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(40 + nombre.len());
    v.extend_from_slice(b"NFM1\x10");
    v.extend_from_slice(pk);
    v.extend_from_slice(&PUERTO_TCP.to_be_bytes());
    v.push(nombre.len() as u8);
    v.extend_from_slice(nombre.as_bytes());
    v
}

/// `None` si no cumple el formato exacto, si trae nuestra clave o si excede los topes.
pub fn leer_anuncio(d: &[u8], propia: &Clave) -> Option<Anuncio> {
    if d.len() < 41 || d.len() > 40 + MAX_NOMBRE || &d[..5] != b"NFM1\x10" {
        return None;
    }
    let pk: Clave = d[5..37].try_into().ok()?;
    let puerto = u16::from_be_bytes([d[37], d[38]]);
    let n = d[39] as usize;
    if puerto == 0 || n != d.len() - 40 || ct_eq(&pk, propia) || ct_eq(&pk, &[0; 32]) {
        return None;
    }
    let nombre = core::str::from_utf8(&d[40..]).ok()?;
    nombre_valido(nombre).then(|| Anuncio { pk, puerto, nombre: String::from(nombre) })
}

// ---------------------------------------------------------------- saludo y claves (TCP)

/// `NFM1 ‖ tipo ‖ s ‖ e`.
pub fn hola(tipo: u8, s: &Clave, e: &Clave) -> [u8; HOLA_LEN] {
    let mut h = [0u8; HOLA_LEN];
    h[..4].copy_from_slice(b"NFM1");
    h[4] = tipo;
    h[5..37].copy_from_slice(s);
    h[37..].copy_from_slice(e);
    h
}

/// (clave de identidad, efímera) de un saludo del tipo dado; `None` si no es exactamente eso.
pub fn leer_hola(b: &[u8], tipo: u8) -> Option<(Clave, Clave)> {
    if b.len() != HOLA_LEN || &b[..4] != b"NFM1" || b[4] != tipo {
        return None;
    }
    Some((b[5..37].try_into().ok()?, b[37..].try_into().ok()?))
}

/// Claves y contadores de una conexión. Se borran al soltarla.
pub struct Sesion {
    enviar: Clave,
    recibir: Clave,
    n_env: u64,
    n_rec: u64,
}

impl Drop for Sesion {
    fn drop(&mut self) {
        wipe(&mut self.enviar);
        wipe(&mut self.recibir);
    }
}

fn nonce(n: u64) -> [u8; 12] {
    let mut v = [0u8; 12];
    v[4..].copy_from_slice(&n.to_le_bytes());
    v
}

/// Saca las claves de la conexión. `inicia` = somos *I*. `s_priv` es nuestra identidad y `e_priv` la
/// efímera de esta conexión; `hi` y `hr` son los dos saludos tal como viajaron. `None` si algún
/// secreto compartido sale todo ceros (clave de orden pequeño).
pub fn derivar(inicia: bool, s_priv: &Clave, e_priv: &Clave, hi: &[u8; HOLA_LEN], hr: &[u8; HOLA_LEN]) -> Option<Sesion> {
    let (otra_s, otra_e): (Clave, Clave) = if inicia { (hr[5..37].try_into().ok()?, hr[37..].try_into().ok()?) } else { (hi[5..37].try_into().ok()?, hi[37..].try_into().ok()?) };
    let ee = x25519(e_priv, &otra_e);
    // I: es = X25519(e_I, s_R), se = X25519(s_I, e_R). R calcula los mismos con sus privadas.
    let (es, se) = if inicia { (x25519(e_priv, &otra_s), x25519(s_priv, &otra_e)) } else { (x25519(s_priv, &otra_e), x25519(e_priv, &otra_s)) };
    let mut ikm = Vec::with_capacity(96);
    for x in [&ee, &es, &se] {
        ikm.extend_from_slice(x);
    }
    let cero = [ee, es, se].iter().any(|x| ct_eq(x, &[0; 32]));
    let mut th_in = Vec::from(&b"NightFury Mensajes 1"[..]);
    th_in.extend_from_slice(hi);
    th_in.extend_from_slice(hr);
    let th = sha256(&th_in);
    let mut prk = hkdf_extract(&th, &ikm);
    let (mut k_ir, mut k_ri) = ([0u8; 32], [0u8; 32]);
    hkdf_expand(&prk, b"NFM1 i->r", &mut k_ir);
    hkdf_expand(&prk, b"NFM1 r->i", &mut k_ri);
    wipe(&mut prk);
    let (mut ee, mut es, mut se) = (ee, es, se);
    wipe(&mut ee);
    wipe(&mut es);
    wipe(&mut se);
    wipe(&mut ikm);
    if cero {
        wipe(&mut k_ir);
        wipe(&mut k_ri);
        return None;
    }
    Some(if inicia { Sesion { enviar: k_ir, recibir: k_ri, n_env: 0, n_rec: 0 } } else { Sesion { enviar: k_ri, recibir: k_ir, n_env: 0, n_rec: 0 } })
}

impl Sesion {
    /// `longitud (u16) ‖ sellado`. `None` si no cabe en una trama o se agotó el contador.
    pub fn sellar(&mut self, cuerpo: &[u8]) -> Option<Vec<u8>> {
        let largo = cuerpo.len() + ETIQUETA;
        if largo > MAX_TRAMA || self.n_env == u64::MAX {
            return None;
        }
        let cab = (largo as u16).to_be_bytes();
        let sellado = aead_seal(&self.enviar, &nonce(self.n_env), &cab, cuerpo);
        self.n_env += 1;
        let mut v = Vec::with_capacity(2 + largo);
        v.extend_from_slice(&cab);
        v.extend_from_slice(&sellado);
        Some(v)
    }

    /// Abre la primera trama de `buf`: `Ok(Some((cuerpo, bytes usados)))`, `Ok(None)` si falta por llegar
    /// y `Err` si hay que cortar (longitud fuera de tope o no se descifra).
    pub fn abrir(&mut self, buf: &[u8]) -> Result<Option<(Vec<u8>, usize)>, ()> {
        if buf.len() < 2 {
            return Ok(None);
        }
        let largo = u16::from_be_bytes([buf[0], buf[1]]) as usize;
        if largo > MAX_TRAMA || largo <= ETIQUETA || self.n_rec == u64::MAX {
            return Err(());
        }
        if buf.len() < 2 + largo {
            return Ok(None);
        }
        let cuerpo = aead_open(&self.recibir, &nonce(self.n_rec), &buf[..2], &buf[2..2 + largo]).ok_or(())?;
        self.n_rec += 1;
        Ok(Some((cuerpo, 2 + largo)))
    }
}

// ---------------------------------------------------------------- contenido de las tramas

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Trama {
    Texto { id: u64, fecha: DateTime, texto: String },
    Recibido(u64),
    Adios,
}

/// `None` si es un texto que no cumple los topes.
pub fn codificar(t: &Trama) -> Option<Vec<u8>> {
    Some(match t {
        Trama::Texto { id, fecha, texto } => {
            if !texto_valido(texto) {
                return None;
            }
            let mut v = Vec::with_capacity(19 + texto.len());
            v.push(1);
            v.extend_from_slice(&id.to_be_bytes());
            v.extend_from_slice(&fecha.year.to_be_bytes());
            v.extend_from_slice(&[fecha.month, fecha.day, fecha.hour, fecha.minute, fecha.second]);
            v.extend_from_slice(&(texto.len() as u16).to_be_bytes());
            v.extend_from_slice(texto.as_bytes());
            v
        }
        Trama::Recibido(id) => {
            let mut v = Vec::from([2u8]);
            v.extend_from_slice(&id.to_be_bytes());
            v
        }
        Trama::Adios => Vec::from([3u8]),
    })
}

/// `None` si no es exactamente una trama válida. Los caracteres de control del texto (salvo `\n` y
/// `\t`) se cambian por `?`.
pub fn decodificar(b: &[u8]) -> Option<Trama> {
    match *b.first()? {
        1 if b.len() >= 19 => {
            let id = u64::from_be_bytes(b[1..9].try_into().ok()?);
            let fecha = DateTime { year: u16::from_be_bytes([b[9], b[10]]), month: b[11], day: b[12], hour: b[13], minute: b[14], second: b[15] };
            let n = u16::from_be_bytes([b[16], b[17]]) as usize;
            if b.len() != 18 + n || !(1..=12).contains(&fecha.month) || !(1..=31).contains(&fecha.day) || fecha.hour > 23 || fecha.minute > 59 || fecha.second > 59 {
                return None;
            }
            let texto = core::str::from_utf8(&b[18..]).ok()?;
            texto_valido(texto).then(|| Trama::Texto { id, fecha, texto: limpiar(texto) })
        }
        2 if b.len() == 9 => Some(Trama::Recibido(u64::from_be_bytes(b[1..9].try_into().ok()?))),
        3 if b.len() == 1 => Some(Trama::Adios),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn priv_(n: u8) -> Clave {
        [n; 32]
    }

    /// Un saludo completo entre dos identidades fijas: devuelve las dos sesiones.
    fn saludo() -> (Sesion, Sesion, [u8; HOLA_LEN], [u8; HOLA_LEN]) {
        let (si, ei, sr, er) = (priv_(1), priv_(2), priv_(3), priv_(4));
        let hi = hola(HOLA_I, &publica(&si), &publica(&ei));
        let hr = hola(HOLA_R, &publica(&sr), &publica(&er));
        let i = derivar(true, &si, &ei, &hi, &hr).unwrap();
        let r = derivar(false, &sr, &er, &hi, &hr).unwrap();
        (i, r, hi, hr)
    }

    #[test]
    fn el_saludo_da_las_mismas_claves() {
        let (i, r, _, _) = saludo();
        assert_eq!(i.enviar, r.recibir);
        assert_eq!(i.recibir, r.enviar);
        assert_ne!(i.enviar, i.recibir);
    }

    #[test]
    fn tramas_de_ida_y_vuelta_en_los_dos_sentidos() {
        let (mut i, mut r, _, _) = saludo();
        let t = Trama::Texto { id: 7, fecha: DateTime { year: 2026, month: 11, day: 23, hour: 9, minute: 5, second: 59 }, texto: String::from("hola\nñandú\tok") };
        for n in 0..3 {
            let f = i.sellar(&codificar(&t).unwrap()).unwrap();
            let (c, usados) = r.abrir(&f).unwrap().unwrap();
            assert_eq!(usados, f.len(), "trama {n}");
            assert_eq!(decodificar(&c), Some(t.clone()));
            let f = r.sellar(&codificar(&Trama::Recibido(7)).unwrap()).unwrap();
            let (c, _) = i.abrir(&f).unwrap().unwrap();
            assert_eq!(decodificar(&c), Some(Trama::Recibido(7)));
        }
    }

    #[test]
    fn una_trama_alterada_o_repetida_corta() {
        let (mut i, mut r, _, _) = saludo();
        let f = i.sellar(&[3]).unwrap();
        for k in 0..f.len() {
            let mut m = f.clone();
            m[k] ^= 1;
            let (mut i2, mut r2, _, _) = saludo();
            let _ = i2.sellar(&[3]);
            assert!(matches!(r2.abrir(&m), Err(()) | Ok(None)), "byte {k}");
        }
        assert!(r.abrir(&f).unwrap().is_some());
        // Repetir la misma trama: el contador ya subió, no se descifra.
        assert!(r.abrir(&f).is_err());
        // Longitud fuera de tope o demasiado corta.
        assert!(r.abrir(&[0x20, 0x00, 1, 2]).is_err());
        assert!(r.abrir(&[0x00, 0x05, 1, 2, 3, 4, 5]).is_err());
        // Falta por llegar.
        assert_eq!(r.abrir(&[0x00]).unwrap(), None);
        assert_eq!(r.abrir(&[0x00, 0x20, 1]).unwrap(), None);
    }

    #[test]
    fn quien_no_tiene_la_privada_no_llega_a_las_claves() {
        let (si, ei, sr, er) = (priv_(1), priv_(2), priv_(3), priv_(4));
        let hi = hola(HOLA_I, &publica(&si), &publica(&ei));
        let hr = hola(HOLA_R, &publica(&sr), &publica(&er));
        let mut i = derivar(true, &si, &ei, &hi, &hr).unwrap();
        // Un impostor que anuncia la identidad de R pero usa otra privada.
        let mut falso = derivar(false, &priv_(9), &er, &hi, &hr).unwrap();
        let f = i.sellar(&[3]).unwrap();
        assert!(falso.abrir(&f).is_err());
    }

    #[test]
    fn un_secreto_de_ceros_corta() {
        let (si, ei) = (priv_(1), priv_(2));
        let hi = hola(HOLA_I, &publica(&si), &publica(&ei));
        // Efímera de R de orden pequeño (todo ceros): X25519 da ceros.
        let hr = hola(HOLA_R, &publica(&priv_(3)), &[0; 32]);
        assert!(derivar(true, &si, &ei, &hi, &hr).is_none());
    }

    #[test]
    fn saludos_mal_formados() {
        let h = hola(HOLA_I, &priv_(1), &priv_(2));
        assert!(leer_hola(&h, HOLA_I).is_some());
        assert!(leer_hola(&h, HOLA_R).is_none());
        assert!(leer_hola(&h[..68], HOLA_I).is_none());
        let mut m = h;
        m[0] = b'X';
        assert!(leer_hola(&m, HOLA_I).is_none());
    }

    #[test]
    fn anuncios() {
        let (pk, propia) = (priv_(5), priv_(6));
        let a = anuncio(&pk, "Ana");
        assert_eq!(leer_anuncio(&a, &propia), Some(Anuncio { pk, puerto: PUERTO_TCP, nombre: String::from("Ana") }));
        // Nuestra propia clave, malformados, demasiado largos, nombre con control, UTF-8 roto.
        assert_eq!(leer_anuncio(&a, &pk), None);
        assert_eq!(leer_anuncio(&a[..a.len() - 1], &propia), None);
        let mut largo = a.clone();
        largo.push(b'x');
        assert_eq!(leer_anuncio(&largo, &propia), None);
        assert_eq!(leer_anuncio(&anuncio(&pk, &"a".repeat(33)), &propia), None);
        assert!(leer_anuncio(&anuncio(&pk, &"a".repeat(32)), &propia).is_some());
        assert_eq!(leer_anuncio(&anuncio(&pk, "a\nb"), &propia), None);
        assert_eq!(leer_anuncio(&anuncio(&pk, ""), &propia), None);
        let mut roto = anuncio(&pk, "ab");
        roto[40] = 0xFF;
        assert_eq!(leer_anuncio(&roto, &propia), None);
        let mut mal = a.clone();
        mal[0] = b'X';
        assert_eq!(leer_anuncio(&mal, &propia), None);
        let mut cero = a.clone();
        cero[37] = 0;
        cero[38] = 0;
        assert_eq!(leer_anuncio(&cero, &propia), None);
        assert_eq!(leer_anuncio(&[], &propia), None);
    }

    #[test]
    fn contenido_de_las_tramas() {
        let f = DateTime { year: 2026, month: 1, day: 2, hour: 3, minute: 4, second: 5 };
        let ok = codificar(&Trama::Texto { id: 1, fecha: f, texto: String::from("a") }).unwrap();
        assert_eq!(decodificar(&ok[..ok.len() - 1]), None, "longitud que no cuadra");
        let mut mal = ok.clone();
        mal[11] = 13;
        assert_eq!(decodificar(&mal), None, "mes");
        let mut con_control = ok.clone();
        *con_control.last_mut().unwrap() = 7;
        assert_eq!(decodificar(&con_control), Some(Trama::Texto { id: 1, fecha: f, texto: String::from("?") }));
        assert!(codificar(&Trama::Texto { id: 1, fecha: f, texto: "a".repeat(MAX_TEXTO_BYTES + 1) }).is_none());
        assert!(codificar(&Trama::Texto { id: 1, fecha: f, texto: "é".repeat(2001) }).is_none());
        assert!(codificar(&Trama::Texto { id: 1, fecha: f, texto: String::new() }).is_none());
        assert_eq!(decodificar(&[]), None);
        assert_eq!(decodificar(&[9]), None);
        assert_eq!(decodificar(&[2, 0]), None);
        assert_eq!(decodificar(&[3, 0]), None);
    }

    #[test]
    fn la_lista_de_palabras() {
        assert_eq!(LISTA.len(), 256);
        for (i, w) in LISTA.iter().enumerate() {
            assert!(!w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase()), "{w}");
            assert!(LISTA[..i].iter().all(|o| o != w), "repetida: {w}");
        }
    }

    #[test]
    fn las_palabras_son_iguales_en_los_dos_lados() {
        let (a, b) = (publica(&priv_(1)), publica(&priv_(2)));
        assert_eq!(palabras(&a, &b), palabras(&b, &a));
        assert_ne!(palabras(&a, &b), palabras(&a, &publica(&priv_(3))));
        assert_eq!(huella(&a).len(), 16);
    }
}
