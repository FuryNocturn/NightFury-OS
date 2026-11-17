//! Cifrado de la carpeta de usuario (v0.6.0): lógica pura, sin disco. La unión con los archivos
//! está en `files.rs`.
//!
//! Contenido de cada archivo: `"NFH1"(4) | sal(8) | iv(16) | cifrado`. `iv` son los 16 primeros
//! bytes de `HMAC(k_marca, sal || longitud || contenido)` y `cifrado = AES-256-CTR(k_contenido, iv)`:
//! al leer se descifra y se recalcula esa marca; si no coincide, el archivo no es de fiar. Así no
//! hace falta azar al escribir: el mismo `iv` solo sale con la misma sal y el mismo contenido.
//!
//! Nombres: determinista, `iv = HMAC(k_nombre_marca, nombre)[..16]`, `iv || AES-CTR(k_nombre, iv, nombre)`
//! en base32 (solo mayúsculas, porque NFFS no distingue mayúsculas) con el prefijo `$e`.
//!
//! Archivo de llaves (`C:\Sistema\config\llaves\<cuenta>`): texto con `estado=`, una o dos líneas
//! `clave=` (dos solo mientras se cambia la clave), `recuperacion=` y `marca=` (HMAC del estado con
//! una subllave: un editor de discos no puede cambiar el estado sin que se note).
use crate::cipher::ctr;
use crate::crypto::{ct_eq, sha256, to_hex, wipe, Hmac};
use crate::keys::{self, Slot, MASTER};
use crate::platform::DateTime;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub const MAGIC: [u8; 4] = *b"NFH1";
/// Firma + sal + iv.
pub const HEADER: usize = 28;
/// Prefijo de los nombres cifrados en disco.
pub const NAME_PREFIX: &str = "$e";
const MAX_NAME: usize = 255;
const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Subllaves de una carpeta cifrada, derivadas de su llave maestra. Se borran al soltarlas.
pub struct Keys {
    content: [u8; 32],
    mark: [u8; 32],
    nmac: [u8; 32],
    nenc: [u8; 32],
    state: [u8; 32],
}

impl Keys {
    pub fn new(master: &[u8; MASTER]) -> Keys {
        Keys {
            content: keys::subkey(master, b"home-content"),
            mark: keys::subkey(master, b"home-mark"),
            nmac: keys::subkey(master, b"home-name-mac"),
            nenc: keys::subkey(master, b"home-name-enc"),
            state: keys::subkey(master, b"home-state"),
        }
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        for k in [&mut self.content, &mut self.mark, &mut self.nmac, &mut self.nenc, &mut self.state] {
            wipe(k);
        }
    }
}

/// Sal de una escritura: mezcla de la fecha y hora y de un contador en memoria.
pub fn make_sal(now: &DateTime, counter: u32) -> [u8; 8] {
    let mut b = [0u8; 12];
    b[..2].copy_from_slice(&now.year.to_le_bytes());
    b[2..6].copy_from_slice(&[now.month, now.day, now.hour, now.minute]);
    b[6] = now.second;
    b[7] = 0x5a;
    b[8..].copy_from_slice(&counter.to_le_bytes());
    let h = sha256(&b);
    let mut s = [0u8; 8];
    s.copy_from_slice(&h[..8]);
    s
}

fn mark(k: &Keys, sal: &[u8], data: &[u8]) -> [u8; 16] {
    let m = Hmac::new(&k.mark).mac(&[sal, &(data.len() as u64).to_le_bytes(), data]);
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&m[..16]);
    iv
}

/// Contenido tal como va al disco.
pub fn seal(k: &Keys, sal: [u8; 8], data: &[u8]) -> Vec<u8> {
    let iv = mark(k, &sal, data);
    let mut out = Vec::with_capacity(HEADER + data.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&sal);
    out.extend_from_slice(&iv);
    out.extend_from_slice(data);
    ctr(&k.content, &iv, &mut out[HEADER..]);
    out
}

/// Contenido en claro, o `None` si no tiene la firma, está truncado o alterado.
pub fn open(k: &Keys, raw: &[u8]) -> Option<Vec<u8>> {
    if raw.len() < HEADER || raw[..4] != MAGIC {
        return None;
    }
    let (sal, iv) = (&raw[4..12], &raw[12..HEADER]);
    let mut data = raw[HEADER..].to_vec();
    let mut ivv = [0u8; 16];
    ivv.copy_from_slice(iv);
    ctr(&k.content, &ivv, &mut data);
    if ct_eq(&mark(k, sal, &data), iv) {
        Some(data)
    } else {
        wipe(&mut data);
        None
    }
}

/// Tamaño del contenido en claro a partir del tamaño en disco.
pub fn plain_len(raw_len: u64) -> u64 {
    raw_len.saturating_sub(HEADER as u64)
}

fn b32(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 8 / 5 + 1);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in data {
        acc = acc << 8 | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            s.push(B32[(acc >> bits & 31) as usize] as char);
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        s.push(B32[(acc << (5 - bits) & 31) as usize] as char);
    }
    s
}

fn unb32(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = B32.iter().position(|&x| x == c.to_ascii_uppercase())? as u32;
        acc = acc << 5 | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// ¿Empieza como un nombre cifrado? (no comprueba que sea válido)
pub fn has_name_prefix(raw: &str) -> bool {
    raw.get(..NAME_PREFIX.len()).is_some_and(|p| p.eq_ignore_ascii_case(NAME_PREFIX))
}

/// Nombre en disco de `name`. `None` si no cabe en un nombre de NFFS (máximo 255 bytes) o si
/// empieza por el prefijo reservado.
pub fn encode_name(k: &Keys, name: &str) -> Option<String> {
    if has_name_prefix(name) {
        return None;
    }
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&Hmac::new(&k.nmac).mac(&[name.as_bytes()])[..16]);
    let mut ct = name.as_bytes().to_vec();
    ctr(&k.nenc, &iv, &mut ct);
    let mut buf = iv.to_vec();
    buf.extend_from_slice(&ct);
    let s = format!("{NAME_PREFIX}{}", b32(&buf));
    (s.len() <= MAX_NAME).then_some(s)
}

/// Nombre en claro de un nombre de disco, o `None` si no es de los nuestros, está alterado o no
/// sale de esta llave.
pub fn decode_name(k: &Keys, raw: &str) -> Option<String> {
    if !has_name_prefix(raw) {
        return None;
    }
    let body = &raw[NAME_PREFIX.len()..];
    let buf = unb32(body)?;
    if buf.len() < 17 {
        return None;
    }
    let (iv, ct) = buf.split_at(16);
    let mut pt = ct.to_vec();
    let mut ivv = [0u8; 16];
    ivv.copy_from_slice(iv);
    ctr(&k.nenc, &ivv, &mut pt);
    let ok = ct_eq(&Hmac::new(&k.nmac).mac(&[&pt])[..16], iv);
    let name = String::from_utf8(pt).ok().filter(|_| ok)?;
    // Una sola forma de escribirlo: sin sobrantes ni variantes.
    (b32(&buf).eq_ignore_ascii_case(body) && crate::files::valid_name(&name)).then_some(name)
}

// ---- Archivo de llaves ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HState {
    Encrypted,
    Encrypting,
    Decrypting,
}

impl HState {
    fn word(self) -> &'static str {
        match self {
            HState::Encrypted => "cifrado",
            HState::Encrypting => "cifrando",
            HState::Decrypting => "descifrando",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KeyFile {
    pub state: HState,
    /// La llave envuelta con la clave de la cuenta (dos solo mientras se cambia la clave).
    pub pass: Vec<Slot>,
    pub recovery: Slot,
    mark: Option<[u8; 32]>,
}

impl KeyFile {
    pub fn new(state: HState, pass: Vec<Slot>, recovery: Slot) -> KeyFile {
        KeyFile { state, pass, recovery, mark: None }
    }

    fn state_mark(k: &Keys, state: HState) -> [u8; 32] {
        Hmac::new(&k.state).mac(&[b"nightfury-home-state-v1", state.word().as_bytes()])
    }

    /// Texto del archivo, con la marca del estado calculada con `k`.
    pub fn to_text(&self, k: &Keys) -> String {
        let mut s = format!("estado={}\n", self.state.word());
        for p in &self.pass {
            s.push_str(&format!("clave={}\n", p.to_text()));
        }
        s.push_str(&format!("recuperacion={}\n", self.recovery.to_text()));
        s.push_str(&format!("marca={}\n", to_hex(&Self::state_mark(k, self.state))));
        s
    }

    pub fn parse(text: &str) -> Option<KeyFile> {
        let (mut state, mut pass, mut rec, mut mark) = (None, Vec::new(), None, None);
        for l in text.lines() {
            let Some((k, v)) = l.split_once('=') else { continue };
            match k {
                "estado" => {
                    state = Some(match v {
                        "cifrado" => HState::Encrypted,
                        "cifrando" => HState::Encrypting,
                        "descifrando" => HState::Decrypting,
                        _ => return None,
                    })
                }
                "clave" => pass.push(Slot::parse(v)?),
                "recuperacion" => rec = Some(Slot::parse(v)?),
                "marca" => mark = Some(crate::crypto::from_hex(v)?.try_into().ok()?),
                _ => {}
            }
        }
        if pass.is_empty() || pass.len() > 2 {
            return None;
        }
        Some(KeyFile { state: state?, pass, recovery: rec?, mark })
    }

    /// Llave maestra con la clave de la cuenta, y solo si el estado no ha sido alterado.
    pub fn open_pass(&self, password: &str) -> Option<[u8; MASTER]> {
        self.open_with(|| self.pass.iter().find_map(|s| s.unwrap(password.as_bytes())))
    }

    /// Lo mismo con el código de recuperación (ya normalizado).
    pub fn open_recovery(&self, code: &str) -> Option<[u8; MASTER]> {
        self.open_with(|| self.recovery.unwrap(code.as_bytes()))
    }

    fn open_with(&self, f: impl FnOnce() -> Option<[u8; MASTER]>) -> Option<[u8; MASTER]> {
        let mut m = f()?;
        let k = Keys::new(&m);
        let ok = self.mark.is_some_and(|x| ct_eq(&x, &Self::state_mark(&k, self.state)));
        if ok {
            Some(m)
        } else {
            wipe(&mut m);
            None
        }
    }
}

/// Segunda parte (iv) de un `Slot::wrap` que se hace sin `Pool`: sale de la sal, que es nueva
/// para cada envoltorio (con una clave nueva por sal, repetir el iv entre sales no importa).
pub fn iv_from_salt(salt: &[u8; 16]) -> [u8; 16] {
    let mut b = [0u8; 24];
    b[..16].copy_from_slice(salt);
    b[16..].copy_from_slice(b"wrapiv-1");
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&sha256(&b)[..16]);
    iv
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(n: u8) -> Keys {
        Keys::new(&[n; MASTER])
    }

    fn dt() -> DateTime {
        DateTime { year: 2026, month: 11, day: 17, hour: 1, minute: 2, second: 3 }
    }

    #[test]
    fn content_roundtrip_and_tamper() {
        let k = keys(1);
        for data in [&b""[..], b"a", &[7u8; 16], &[9u8; 100]] {
            let raw = seal(&k, make_sal(&dt(), 1), data);
            assert_eq!(raw.len(), HEADER + data.len());
            assert_eq!(plain_len(raw.len() as u64), data.len() as u64);
            assert_eq!(open(&k, &raw).as_deref(), Some(data));
            assert!(open(&keys(2), &raw).is_none());
            if !data.is_empty() {
                assert_ne!(&raw[HEADER..], data);
            }
            // Un bit en cualquier sitio, o truncado: nada.
            for i in 0..raw.len() {
                let mut r = raw.clone();
                r[i] ^= 1;
                assert!(open(&k, &r).is_none(), "byte {i}");
            }
            for n in 0..raw.len() {
                assert!(open(&k, &raw[..n]).is_none());
            }
            let mut longer = raw.clone();
            longer.push(0);
            assert!(open(&k, &longer).is_none());
        }
    }

    #[test]
    fn same_content_different_sal_differs() {
        let k = keys(1);
        let a = seal(&k, make_sal(&dt(), 1), b"igual");
        let b = seal(&k, make_sal(&dt(), 2), b"igual");
        assert_ne!(a[HEADER..], b[HEADER..]);
        assert_ne!(a[12..HEADER], b[12..HEADER]);
        assert_eq!(a, seal(&k, make_sal(&dt(), 1), b"igual"));
    }

    #[test]
    fn names_roundtrip_and_alterations() {
        let k = keys(1);
        for n in ["a", "Documentos", "ñandú ü.txt", "x y (2).txt"] {
            let e = encode_name(&k, n).unwrap();
            assert!(crate::files::valid_name(&e) && e.starts_with("$e"));
            assert_eq!(decode_name(&k, &e).as_deref(), Some(n));
            assert_eq!(decode_name(&k, &e.to_lowercase()).as_deref(), Some(n));
            assert_eq!(encode_name(&k, n).unwrap(), e);
            assert!(decode_name(&keys(2), &e).is_none());
            // Alterar cualquier carácter del cuerpo.
            for i in 2..e.len() {
                let mut b = e.clone().into_bytes();
                b[i] = if b[i] == b'A' { b'B' } else { b'A' };
                assert!(decode_name(&k, &String::from_utf8(b).unwrap()).is_none(), "{n} {i}");
            }
            assert!(decode_name(&k, &e[..e.len() - 1]).is_none());
            assert!(decode_name(&k, &format!("{e}A")).is_none());
        }
        assert_ne!(encode_name(&k, "a"), encode_name(&k, "A"));
        assert!(decode_name(&k, "texto.txt").is_none() && decode_name(&k, "$e").is_none());
        // Longitud: el máximo exacto cabe y uno más no.
        let ok = "a".repeat(142);
        assert!(encode_name(&k, &ok).is_some());
        assert!(encode_name(&k, &"a".repeat(143)).is_none());
        assert!(encode_name(&k, "$eABC").is_none());
    }

    #[test]
    fn keyfile_roundtrip_state_mark_and_slots() {
        let master = [5u8; MASTER];
        let k = Keys::new(&master);
        let s1 = Slot::wrap(&master, b"clave-uno", [1; 16], [2; 16]);
        let s2 = Slot::wrap(&master, b"clave-dos", [3; 16], [4; 16]);
        let r = Slot::wrap(&master, b"ABCDE", [5; 16], [6; 16]);
        let kf = KeyFile::new(HState::Encrypting, vec![s1.clone(), s2.clone()], r.clone());
        let text = kf.to_text(&k);
        assert!(!text.contains("clave-uno"));
        let back = KeyFile::parse(&text).unwrap();
        assert_eq!(back.state, HState::Encrypting);
        assert_eq!(back.open_pass("clave-uno"), Some(master));
        assert_eq!(back.open_pass("clave-dos"), Some(master));
        assert_eq!(back.open_pass("otra"), None);
        assert_eq!(back.open_recovery("ABCDE"), Some(master));
        assert_eq!(back.open_recovery("ABCDF"), None);
        // Cambiar el estado a mano invalida la apertura.
        let forged = text.replace("estado=cifrando", "estado=descifrando");
        assert_eq!(KeyFile::parse(&forged).unwrap().open_pass("clave-uno"), None);
        assert_eq!(KeyFile::parse(&text.replace("estado=cifrando", "estado=raro")), None);
        assert_eq!(KeyFile::parse(&text.replace("marca=", "m=")).unwrap().open_pass("clave-uno"), None);
        assert!(KeyFile::parse("estado=cifrado\n").is_none());
    }
}
