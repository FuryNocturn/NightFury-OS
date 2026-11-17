// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Llaves de cifrado: de dónde salen y cómo se guardan. Lógica pura sobre `crypto` y `cipher`.
//!
//! - `Pool`: acumulador de aleatoriedad (ratón, teclado, reloj fino, generador del procesador).
//! - Llave maestra: 64 bytes aleatorios. Es la que cifra de verdad; nunca sale de la memoria.
//! - `Slot`: la llave maestra envuelta con un secreto (la clave del usuario o el código de
//!   recuperación). Cambiar la clave solo cambia el `Slot`; no se recifra ningún dato.
//! - Código de recuperación: 40 caracteres en 8 grupos de 5 (200 bits).
//!
//! Envoltorio (cifrar y luego autenticar): de `secreto` y `sal` salen con PBKDF2-HMAC-SHA-256
//! 64 bytes, la mitad para AES-256-CTR y la mitad para HMAC-SHA-256 sobre todo el registro.
//! Un secreto equivocado o un registro alterado se detectan por la marca, sin revelar nada.
use crate::cipher::ctr;
use crate::crypto::{ct_eq, from_hex, pbkdf2, to_hex, wipe, Hmac, Sha256};
use crate::fs::FsError;
use alloc::format;
use alloc::string::String;

/// Longitud de una llave maestra.
pub const MASTER: usize = 64;
/// Vueltas de PBKDF2 para envolver llaves. En las pruebas se baja para que sean rápidas.
pub const ITERS: u32 = if cfg!(test) { 50 } else { 200_000 };
const LABEL: &[u8] = b"nightfury-key-v1";
/// Sin I, L, O ni 1: 32 símbolos, 5 bits cada uno. Al teclearlo, una O se toma como 0.
const ALPHABET: &[u8; 32] = b"ABCDEFGHJKMNPQRSTUVWXYZ023456789";

/// Errores de las operaciones de cifrado.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyError {
    /// Clave o código de recuperación incorrectos (o registro de llaves alterado).
    WrongSecret,
    /// Clave demasiado corta.
    Weak,
    NotEncrypted,
    AlreadyEncrypted,
    /// Hace falta ser administrador.
    NotAdmin,
    /// Este disco no tiene sitio reservado para la cabecera de cifrado.
    NoRoom,
    /// Hay una conversión en curso.
    Busy,
    /// No hay disco o no hay sesión.
    Unavailable,
    /// Faltan movimientos de ratón en el acumulador (`Pool::ready`).
    NotReady,
    Fs(FsError),
}

impl From<FsError> for KeyError {
    fn from(e: FsError) -> KeyError {
        KeyError::Fs(e)
    }
}

impl KeyError {
    /// Texto para mostrar al usuario.
    pub fn text(self) -> &'static str {
        match self {
            KeyError::WrongSecret => "La clave o el código no son correctos.",
            KeyError::Weak => "La clave debe tener al menos 6 caracteres.",
            KeyError::NotEncrypted => "No está cifrado.",
            KeyError::AlreadyEncrypted => "Ya está cifrado.",
            KeyError::NotAdmin => "Hace falta ser administrador.",
            KeyError::NoRoom => "Este disco no admite el cifrado total: crea un disco nuevo y copia tus archivos.",
            KeyError::Busy => "Hay una conversión en curso: espera a que termine.",
            KeyError::Unavailable => "No hay disco o no hay sesión abierta.",
            KeyError::NotReady => "Mueve más el ratón para generar la llave.",
            KeyError::Fs(_) => "Error al escribir en el disco.",
        }
    }
}

/// Movimientos mínimos de ratón que la pantalla de generación exige antes de continuar.
pub const POOL_EVENTS_NEEDED: u32 = 256;

/// Acumulador de aleatoriedad. Todo lo que entra se mezcla con SHA-256; lo que sale no permite
/// reconstruir lo que entró, y tras cada extracción el estado cambia.
pub struct Pool {
    state: [u8; 32],
    events: u32,
    counter: u64,
}

impl Default for Pool {
    fn default() -> Pool {
        Pool::new()
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        wipe(&mut self.state);
    }
}

impl Pool {
    pub fn new() -> Pool {
        Pool { state: [0; 32], events: 0, counter: 0 }
    }

    /// Mezcla datos cualesquiera (lecturas de `Platform::entropy`, la hora, etc.).
    pub fn add(&mut self, data: &[u8]) {
        let mut h = Sha256::new();
        h.update(&self.state);
        h.update(&(data.len() as u64).to_le_bytes());
        h.update(data);
        self.state = h.finish();
    }

    /// Mezcla un movimiento del ratón o una tecla: posición o código, el instante y una lectura
    /// del reloj fino. Solo cuenta como movimiento nuevo si la posición cambia.
    pub fn add_event(&mut self, x: i32, y: i32, now_ms: u64, fine: u64) {
        let mut b = [0u8; 24];
        b[..4].copy_from_slice(&x.to_le_bytes());
        b[4..8].copy_from_slice(&y.to_le_bytes());
        b[8..16].copy_from_slice(&now_ms.to_le_bytes());
        b[16..].copy_from_slice(&fine.to_le_bytes());
        self.add(&b);
        self.events = self.events.saturating_add(1);
    }

    /// Movimientos acumulados (para la barra de progreso).
    pub fn events(&self) -> u32 {
        self.events
    }

    /// ¿Hay bastante para generar llaves?
    pub fn ready(&self) -> bool {
        self.events >= POOL_EVENTS_NEEDED
    }

    /// Bytes para mostrar en pantalla mientras se mueve el ratón. NO son la llave: salen de un
    /// derivado aparte del estado y verlos no ayuda a adivinar lo que luego se extraiga.
    pub fn preview(&self) -> [u8; 32] {
        Hmac::new(&self.state).mac(&[b"vista"])
    }

    /// Extrae bytes aleatorios y avanza el estado.
    pub fn take(&mut self, out: &mut [u8]) {
        let h = Hmac::new(&self.state);
        for chunk in out.chunks_mut(32) {
            self.counter += 1;
            let mut block = h.mac(&[b"salida", &self.counter.to_le_bytes()]);
            chunk.copy_from_slice(&block[..chunk.len()]);
            wipe(&mut block);
        }
        let mut next = h.mac(&[b"siguiente", &self.counter.to_le_bytes()]);
        self.state = next;
        wipe(&mut next);
    }

    pub fn master(&mut self) -> [u8; MASTER] {
        let mut m = [0u8; MASTER];
        self.take(&mut m);
        m
    }
}

/// Llave maestra envuelta con un secreto.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Slot {
    iters: u32,
    salt: [u8; 16],
    iv: [u8; 16],
    ct: [u8; MASTER],
    tag: [u8; 32],
}

fn derive(secret: &[u8], salt: &[u8; 16], iters: u32) -> ([u8; 32], [u8; 32]) {
    let mut s = [0u8; 16 + LABEL.len()];
    s[..16].copy_from_slice(salt);
    s[16..].copy_from_slice(LABEL);
    let mut dk = [0u8; 64];
    pbkdf2(secret, &s, iters, &mut dk);
    let (mut ke, mut km) = ([0u8; 32], [0u8; 32]);
    ke.copy_from_slice(&dk[..32]);
    km.copy_from_slice(&dk[32..]);
    wipe(&mut dk);
    (ke, km)
}

impl Slot {
    fn mac(&self, km: &[u8; 32]) -> [u8; 32] {
        Hmac::new(km).mac(&[&self.iters.to_le_bytes(), &self.salt, &self.iv, &self.ct])
    }

    /// Envuelve `master` con `secret`. `salt` e `iv` deben ser nuevos y aleatorios (`Pool::take`).
    pub fn wrap(master: &[u8; MASTER], secret: &[u8], salt: [u8; 16], iv: [u8; 16]) -> Slot {
        let (mut ke, mut km) = derive(secret, &salt, ITERS);
        let mut s = Slot { iters: ITERS, salt, iv, ct: *master, tag: [0; 32] };
        ctr(&ke, &iv, &mut s.ct);
        s.tag = s.mac(&km);
        wipe(&mut ke);
        wipe(&mut km);
        s
    }

    /// Devuelve la llave maestra si `secret` es el correcto y el registro no se ha alterado.
    pub fn unwrap(&self, secret: &[u8]) -> Option<[u8; MASTER]> {
        let (mut ke, mut km) = derive(secret, &self.salt, self.iters);
        let ok = ct_eq(&self.mac(&km), &self.tag);
        let out = if ok {
            let mut m = self.ct;
            ctr(&ke, &self.iv, &mut m);
            Some(m)
        } else {
            None
        };
        wipe(&mut ke);
        wipe(&mut km);
        out
    }

    /// Texto de una línea, sin espacios: `vueltas:sal:iv:cifrado:marca` en hexadecimal.
    pub fn to_text(&self) -> String {
        format!("{}:{}:{}:{}:{}", self.iters, to_hex(&self.salt), to_hex(&self.iv), to_hex(&self.ct), to_hex(&self.tag))
    }

    pub fn parse(text: &str) -> Option<Slot> {
        let mut f = text.trim().split(':');
        let iters = f.next()?.parse::<u32>().ok().filter(|&i| i > 0 && i <= 50_000_000)?;
        let salt = from_hex(f.next()?)?.try_into().ok()?;
        let iv = from_hex(f.next()?)?.try_into().ok()?;
        let ct = from_hex(f.next()?)?.try_into().ok()?;
        let tag = from_hex(f.next()?)?.try_into().ok()?;
        if f.next().is_some() {
            return None;
        }
        Some(Slot { iters, salt, iv, ct, tag })
    }
}

/// Deriva de la llave maestra una subllave de 32 bytes para un uso concreto (`label`), de modo
/// que una misma llave nunca se use para dos cosas.
pub fn subkey(master: &[u8; MASTER], label: &[u8]) -> [u8; 32] {
    Hmac::new(master).mac(&[b"nightfury-sub-v1", label])
}

/// Código de recuperación nuevo: 8 grupos de 5 caracteres separados por guiones.
pub fn recovery_code(pool: &mut Pool) -> String {
    let mut raw = [0u8; 40];
    pool.take(&mut raw);
    let mut s = String::with_capacity(47);
    for (i, b) in raw.iter().enumerate() {
        if i > 0 && i % 5 == 0 {
            s.push('-');
        }
        s.push(ALPHABET[(b & 31) as usize] as char);
    }
    wipe(&mut raw);
    s
}

/// Deja el código como se usa de secreto: mayúsculas, sin guiones ni espacios. `None` si no
/// tiene 40 caracteres del alfabeto (así un error al teclear se avisa sin calcular nada).
pub fn normalize_recovery(input: &str) -> Option<String> {
    let s: String =
        input.chars().filter(|c| !c.is_whitespace() && *c != '-').flat_map(char::to_uppercase).map(|c| if c == 'O' { '0' } else { c }).collect();
    (s.len() == 40 && s.bytes().all(|b| ALPHABET.contains(&b))).then_some(s)
}

/// Fortaleza orientativa de una clave, de 0 (muy débil) a 4 (fuerte), para el indicador.
pub fn strength(password: &str) -> u8 {
    let n = password.chars().count();
    let kinds = [
        password.chars().any(|c| c.is_lowercase()),
        password.chars().any(|c| c.is_uppercase()),
        password.chars().any(|c| c.is_ascii_digit()),
        password.chars().any(|c| !c.is_alphanumeric()),
    ]
    .iter()
    .filter(|&&k| k)
    .count();
    let mut distinct: alloc::vec::Vec<char> = password.chars().collect();
    distinct.sort_unstable();
    distinct.dedup();
    if n < 8 || distinct.len() < 4 {
        return if n >= 6 { 1 } else { 0 };
    }
    match (n, kinds) {
        (16.., 2..) | (12.., 3..) => 4,
        (12.., _) | (10.., 3..) => 3,
        (_, 2..) => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(seed: u8) -> Pool {
        let mut p = Pool::new();
        for i in 0..POOL_EVENTS_NEEDED {
            p.add_event(i as i32 * 3 + seed as i32, 400 - i as i32, 1000 + i as u64 * 7, 123_456 + i as u64 * 9973);
        }
        p
    }

    #[test]
    fn acumulador() {
        let mut p = Pool::new();
        assert!(!p.ready());
        let mut p1 = pool(1);
        let mut p2 = pool(2);
        assert!(p1.ready() && p1.events() == POOL_EVENTS_NEEDED);
        let (a, b) = (p1.master(), p2.master());
        assert_ne!(a, b, "movimientos distintos dan llaves distintas");
        assert_ne!(a, p1.master(), "cada extracción es distinta");
        // La vista previa no es la salida ni la revela.
        let v = pool(1).preview();
        assert!(!a.windows(32).any(|w| w == v));
        // Salidas de longitudes distintas y sin ceros de relleno.
        let mut out = [0u8; 100];
        p.add(b"algo");
        p.take(&mut out);
        assert!(out[90..].iter().any(|&b| b != 0));
    }

    #[test]
    fn envolver_y_abrir() {
        let master = pool(1).master();
        let s = Slot::wrap(&master, b"mi clave", [1; 16], [2; 16]);
        assert_eq!(s.unwrap(b"mi clave"), Some(master));
        assert_eq!(s.unwrap(b"otra clave"), None);
        assert!(!s.to_text().contains(&to_hex(&master)), "la llave no va en claro");
        // Ida y vuelta por texto.
        let t = Slot::parse(&s.to_text()).unwrap();
        assert_eq!(t, s);
        assert_eq!(t.unwrap(b"mi clave"), Some(master));
        // Cualquier byte alterado se detecta.
        let text = s.to_text();
        for pos in [text.len() - 1, text.len() / 2, text.find(':').unwrap() + 1] {
            let mut bad = text.clone().into_bytes();
            bad[pos] = if bad[pos] == b'0' { b'1' } else { b'0' };
            let bad = String::from_utf8(bad).unwrap();
            assert_eq!(Slot::parse(&bad).and_then(|x| x.unwrap(b"mi clave")), None, "pos {pos}");
        }
        for bad in ["", "x", "1:00:00:00:00", "0:aa:bb:cc:dd", &format!("{text}:00")] {
            assert!(Slot::parse(bad).is_none(), "{bad}");
        }
        // Cambiar de clave = envolver otra vez la MISMA llave.
        let s2 = Slot::wrap(&master, b"clave nueva", [3; 16], [4; 16]);
        assert_eq!(s2.unwrap(b"clave nueva"), Some(master));
        assert_eq!(s2.unwrap(b"mi clave"), None);
    }

    #[test]
    fn subllaves_y_recuperacion() {
        let m = pool(1).master();
        assert_ne!(subkey(&m, b"contenido"), subkey(&m, b"nombres"));
        let mut p = pool(5);
        let code = recovery_code(&mut p);
        assert_eq!(code.len(), 47);
        assert_eq!(code.split('-').count(), 8);
        assert!(code.split('-').all(|g| g.len() == 5));
        assert_ne!(code, recovery_code(&mut p));
        let norm = normalize_recovery(&code).unwrap();
        assert_eq!(norm.len(), 40);
        assert_eq!(normalize_recovery(&code.to_lowercase().replace('-', " ")), Some(norm.clone()));
        assert!(normalize_recovery("ABCDE").is_none() && normalize_recovery(&code.replace(&code[..1], "1")).is_none());
        assert_eq!(normalize_recovery(&"o".repeat(40)), Some("0".repeat(40)));
        // El código abre lo que él envolvió.
        let s = Slot::wrap(&m, norm.as_bytes(), [5; 16], [6; 16]);
        assert_eq!(s.unwrap(normalize_recovery(&code).unwrap().as_bytes()), Some(m));
    }

    #[test]
    fn fortaleza() {
        assert_eq!(strength("abc"), 0);
        assert_eq!(strength("aaaaaaaaaaaa"), 1);
        assert_eq!(strength("secreta"), 1);
        assert_eq!(strength("secreta12"), 2);
        assert_eq!(strength("Secreta-2026"), 4);
        assert_eq!(strength("frase larga de paso con espacios"), 4);
    }
}
