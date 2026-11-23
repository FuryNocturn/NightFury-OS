// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! v0.12.2: cliente TLS 1.3 (RFC 8446) como máquina de estados sin E/S.
//!
//! Escrito desde las normas: RFC 8446, 8448 (vectores), 6066 (SNI), 7301 (ALPN), 8422 y 5869; y para
//! TLS 1.2: RFC 5246, 5289, 7905, 7627 y 5746.
//! Quien lo usa mueve los bytes: lo que llega por TCP entra en `feed`, lo que hay que mandar sale de
//! `take_output`. Capas: `record` (registros y AEAD), `keys` (HKDF y programa de claves),
//! `hs13` (mensajes de saludo 1.3) y `hs12` (saludo, PRF y registros de TLS 1.2: el cliente ofrece las dos
//! versiones y sigue por la que elija el servidor).

mod hs12;
mod hs13;
mod keys;
mod record;
#[cfg(test)]
mod tests;

use crate::crypto::Rng;
use crate::pk::PubKey;
use crate::x509::{CertError, ChainInfo, RootStore, Time};
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use keys::Secret;
use record::{Keys, CT_ALERTA, CT_CCS, CT_DATOS, CT_SALUDO, MAX_CIFRADO, MAX_CLARO};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsError {
    /// La cadena de certificados no vale (solo con `insecure = false`).
    Cert(CertError),
    /// Alerta recibida del servidor (código de RFC 8446 §6).
    Alerta(u8),
    /// El otro lado incumple el protocolo (o la entrada está mal formada).
    Protocolo(&'static str),
    NoSoportado(&'static str),
    /// La conexión ya está cerrada o en error.
    Cerrado,
}

impl TlsError {
    /// Texto en español para el usuario.
    pub fn mensaje(&self) -> String {
        match self {
            TlsError::Cert(e) => String::from(e.mensaje()),
            TlsError::Alerta(a) => format!("El servidor ha rechazado la conexión segura: {} (alerta TLS {}).", nombre_alerta(*a), a),
            TlsError::Protocolo(m) => format!("Error en la conexión segura: {}.", m),
            TlsError::NoSoportado(m) => format!("La conexión segura no es posible: {}.", m),
            TlsError::Cerrado => String::from("La conexión segura está cerrada."),
        }
    }
}

fn nombre_alerta(a: u8) -> &'static str {
    match a {
        0 => "cierre normal",
        10 => "mensaje inesperado",
        20 => "registro corrupto",
        22 => "registro demasiado grande",
        40 => "no hay parámetros comunes",
        42 => "certificado incorrecto",
        43 => "certificado no admitido",
        44 => "certificado revocado",
        45 => "certificado caducado",
        46 => "certificado desconocido",
        47 => "parámetro ilegal",
        48 => "entidad certificadora desconocida",
        49 => "acceso denegado",
        50 => "mensaje mal formado",
        51 => "error de descifrado",
        70 => "versión de protocolo no admitida",
        71 => "seguridad insuficiente",
        80 => "error interno del servidor",
        86 => "degradación no permitida",
        90 => "cancelado",
        109 => "falta una extensión",
        110 => "extensión no admitida",
        112 => "nombre no reconocido",
        116 => "se exige certificado",
        120 => "sin protocolo de aplicación común",
        _ => "alerta desconocida",
    }
}

/// Resultado del saludo, para mostrar al usuario.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    pub version: &'static str,
    pub suite: &'static str,
    /// Cadena validada (`None` si falló con `insecure`).
    pub chain: Option<ChainInfo>,
    /// Por qué falló la cadena (solo con `insecure`).
    pub cert_error: Option<CertError>,
    pub group: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum St {
    EsperaSh,
    EsperaEe,
    EsperaCert,
    EsperaCv,
    EsperaFin,
    Esp12Cert,
    Esp12Ske,
    Esp12Hd,
    Esp12Ccs,
    Esp12Fin,
    Conectado,
    Fallido,
}

/// Fallo interno: alerta a mandar (0 = ninguna) y error para quien llama.
pub(crate) struct Fallo(pub u8, pub TlsError);

pub(crate) const UNEXP: Fallo = Fallo(10, TlsError::Protocolo("mensaje inesperado"));
pub(crate) const DEC: Fallo = Fallo(50, TlsError::Protocolo("mensaje mal formado"));
pub(crate) fn ilegal(m: &'static str) -> Fallo {
    Fallo(47, TlsError::Protocolo(m))
}

/// Lector de bytes que no hace panic: todo devuelve `None` si falta.
pub(crate) struct Rd<'a>(pub &'a [u8]);

impl<'a> Rd<'a> {
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (a, b) = self.0.split_at_checked(n)?;
        self.0 = b;
        Some(a)
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        let b = self.take(2)?;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }
    pub fn u24(&mut self) -> Option<usize> {
        let b = self.take(3)?;
        Some(((b[0] as usize) << 16) | ((b[1] as usize) << 8) | b[2] as usize)
    }
    pub fn v8(&mut self) -> Option<&'a [u8]> {
        let n = self.u8()? as usize;
        self.take(n)
    }
    pub fn v16(&mut self) -> Option<&'a [u8]> {
        let n = self.u16()? as usize;
        self.take(n)
    }
    pub fn v24(&mut self) -> Option<&'a [u8]> {
        let n = self.u24()?;
        self.take(n)
    }
}

pub struct Client {
    host: String,
    ip: bool,
    now: Time,
    roots: Arc<RootStore>,
    insecure: bool,
    rng: Rng,
    inbuf: Vec<u8>,
    out: Vec<u8>,
    app_in: Vec<u8>,
    /// Fragmentos de mensajes de saludo aún incompletos.
    hsbuf: Vec<u8>,
    /// Todos los mensajes de saludo (con el cambio por message_hash tras un HRR).
    transcript: Vec<u8>,
    st: St,
    rx: Option<Keys>,
    tx: Option<Keys>,
    // ClientHello
    random: [u8; 32],
    sid: Vec<u8>,
    x_priv: [u8; 32],
    p_priv: Option<[u8; 32]>,
    /// Grupo del key_share enviado: 0x001d (x25519) o 0x0017 (secp256r1).
    grupo: u16,
    cookie: Vec<u8>,
    hrr_visto: bool,
    ccs_enviado: bool,
    ccs_vistos: u8,
    // saludo
    suite: Option<hs13::Suite>,
    hs_secret: Secret,
    c_hs: Secret,
    s_hs: Secret,
    rx_secret: Secret,
    tx_secret: Secret,
    leaf: Option<PubKey>,
    chain: Option<ChainInfo>,
    cert_error: Option<CertError>,
    /// El servidor pidió certificado: contexto a devolver en el Certificate vacío.
    cert_ctx: Option<Vec<u8>>,
    s12: hs12::S12,
    info: Option<SessionInfo>,
    listo: bool,
    cierre_enviado: bool,
    peer_closed: bool,
}

impl Client {
    /// Prepara el ClientHello (queda en la salida). `insecure` = curl -k: si la cadena falla, sigue pero lo
    /// apunta en `info().cert_error`. `seed` siembra el Rng (32 bytes de la plataforma).
    pub fn new(host: &str, now: Time, roots: Arc<RootStore>, insecure: bool, seed: [u8; 32]) -> Client {
        let mut rng = Rng::new(seed);
        let (mut random, mut sid, mut x) = ([0u8; 32], [0u8; 32], [0u8; 32]);
        rng.fill(&mut random);
        rng.fill(&mut sid);
        rng.fill(&mut x);
        let c = Client::con_valores(host, now, roots, insecure, rng, random, sid.to_vec(), x);
        crate::crypto::wipe(&mut x);
        c
    }

    /// Como `new` pero con random, session_id y privada x25519 fijos (vectores de prueba).
    pub(crate) fn con_valores(host: &str, now: Time, roots: Arc<RootStore>, insecure: bool, rng: Rng, random: [u8; 32], sid: Vec<u8>, x_priv: [u8; 32]) -> Client {
        let h = host.strip_prefix('[').and_then(|x| x.strip_suffix(']')).unwrap_or(host);
        let mut c = Client {
            host: String::from(host),
            ip: h.parse::<core::net::IpAddr>().is_ok(),
            now,
            roots,
            insecure,
            rng,
            inbuf: Vec::new(),
            out: Vec::new(),
            app_in: Vec::new(),
            hsbuf: Vec::new(),
            transcript: Vec::new(),
            st: St::EsperaSh,
            rx: None,
            tx: None,
            random,
            sid,
            x_priv,
            p_priv: None,
            grupo: 0x001d,
            cookie: Vec::new(),
            hrr_visto: false,
            ccs_enviado: false,
            ccs_vistos: 0,
            suite: None,
            hs_secret: Secret::vacio(),
            c_hs: Secret::vacio(),
            s_hs: Secret::vacio(),
            rx_secret: Secret::vacio(),
            tx_secret: Secret::vacio(),
            leaf: None,
            chain: None,
            cert_error: None,
            cert_ctx: None,
            s12: hs12::S12::nuevo(),
            info: None,
            listo: false,
            cierre_enviado: false,
            peer_closed: false,
        };
        let ch = c.armar_hello();
        // El primer mensaje cabe de sobra: no puede fallar.
        let _ = c.enviar_saludo(&ch);
        c
    }

    /// Bytes recibidos por TCP.
    pub fn feed(&mut self, data: &[u8]) -> Result<(), TlsError> {
        if self.st == St::Fallido {
            return Err(TlsError::Cerrado);
        }
        if self.peer_closed {
            return Ok(());
        }
        self.inbuf.extend_from_slice(data);
        loop {
            if self.inbuf.len() < 5 {
                return Ok(());
            }
            let (ct, len) = (self.inbuf[0], u16::from_be_bytes([self.inbuf[3], self.inbuf[4]]) as usize);
            if !(CT_CCS..=CT_DATOS).contains(&ct) || self.inbuf[1] != 3 {
                return Err(self.fallar(UNEXP_NO_TLS));
            }
            if len > MAX_CIFRADO || (self.rx.is_none() && len > MAX_CLARO) {
                return Err(self.fallar(Fallo(22, TlsError::Protocolo("registro demasiado grande"))));
            }
            if self.inbuf.len() < 5 + len {
                return Ok(());
            }
            let rec: Vec<u8> = self.inbuf.drain(..5 + len).collect();
            if let Err(f) = self.en_registro(ct, &rec[..5], &rec[5..]) {
                return Err(self.fallar(f));
            }
            if self.peer_closed {
                self.inbuf.clear();
                return Ok(());
            }
        }
    }

    /// Bytes a mandar por TCP (vacía el búfer).
    pub fn take_output(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.out)
    }

    /// ¿Saludo terminado? Ya se puede `write`.
    pub fn is_ready(&self) -> bool {
        self.listo && self.st != St::Fallido
    }

    /// Cifra `plaintext` en registros de hasta 16 KiB.
    pub fn write(&mut self, plaintext: &[u8]) -> Result<(), TlsError> {
        if self.st == St::Fallido || self.cierre_enviado {
            return Err(TlsError::Cerrado);
        }
        if !self.listo {
            return Err(TlsError::Protocolo("el saludo aún no ha terminado"));
        }
        self.enviar(CT_DATOS, plaintext).map_err(|f| self.fallar(f))
    }

    /// Texto claro recibido (vacía el búfer).
    pub fn read(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.app_in)
    }

    /// Manda close_notify; después ya no se puede escribir.
    pub fn close(&mut self) {
        if self.st != St::Fallido && !self.cierre_enviado {
            let _ = self.enviar(CT_ALERTA, &[1, 0]);
            self.cierre_enviado = true;
        }
    }

    /// ¿Llegó close_notify del servidor?
    pub fn peer_closed(&self) -> bool {
        self.peer_closed
    }

    /// Datos de la sesión, tras el saludo.
    pub fn info(&self) -> Option<&SessionInfo> {
        self.info.as_ref()
    }

    /// Pone `data` en la salida como registros de tipo `ct`, cifrados si ya hay claves de envío.
    fn enviar(&mut self, ct: u8, data: &[u8]) -> Result<(), Fallo> {
        for ch in data.chunks(MAX_CLARO) {
            match &mut self.tx {
                Some(k) => {
                    let r = k.seal(ct, ch).ok_or(Fallo(0, TlsError::Protocolo("contador de registros agotado")))?;
                    self.out.extend_from_slice(&r);
                }
                None => {
                    self.out.extend_from_slice(&[ct, 3, 3, (ch.len() >> 8) as u8, ch.len() as u8]);
                    self.out.extend_from_slice(ch);
                }
            }
        }
        Ok(())
    }

    /// Queda en error, manda la alerta y devuelve el error.
    fn fallar(&mut self, f: Fallo) -> TlsError {
        if f.0 != 0 && self.st != St::Fallido {
            let _ = self.enviar(CT_ALERTA, &[2, f.0]);
        }
        self.st = St::Fallido;
        f.1
    }

    fn en_registro(&mut self, ct: u8, header: &[u8], body: &[u8]) -> Result<(), Fallo> {
        if self.s12.activo {
            return self.en_registro12(ct, header, body);
        }
        match ct {
            CT_CCS => {
                // §5: solo el de compatibilidad (un 1), tras el ClientHello y antes del Finished del servidor.
                if body != [1] || self.listo || !self.hsbuf.is_empty() || self.ccs_vistos >= 2 {
                    return Err(UNEXP);
                }
                self.ccs_vistos += 1;
                Ok(())
            }
            CT_ALERTA if self.rx.is_none() => self.en_alerta(body),
            CT_SALUDO if self.rx.is_none() => {
                if body.is_empty() {
                    return Err(DEC);
                }
                self.hsbuf.extend_from_slice(body);
                self.tratar_saludo()
            }
            CT_DATOS if self.rx.is_some() => {
                let rx = self.rx.as_mut().ok_or(UNEXP)?;
                let (ict, data) = rx.open(header, body).ok_or(Fallo(20, TlsError::Protocolo("registro corrupto o con relleno vacío")))?;
                if data.len() > MAX_CLARO {
                    return Err(Fallo(22, TlsError::Protocolo("registro demasiado grande")));
                }
                match ict {
                    CT_SALUDO => {
                        if data.is_empty() {
                            return Err(DEC);
                        }
                        self.hsbuf.extend_from_slice(&data);
                        self.tratar_saludo()
                    }
                    CT_ALERTA => self.en_alerta(&data),
                    CT_DATOS if self.listo => {
                        self.app_in.extend_from_slice(&data);
                        Ok(())
                    }
                    _ => Err(UNEXP),
                }
            }
            _ => Err(UNEXP),
        }
    }

    pub(super) fn en_alerta(&mut self, d: &[u8]) -> Result<(), Fallo> {
        let [_nivel, desc] = *d else { return Err(DEC) };
        match desc {
            0 => {
                self.peer_closed = true;
                if !self.listo {
                    return Err(Fallo(0, TlsError::Cerrado));
                }
                self.close(); // §6.1: responder con el nuestro
                Ok(())
            }
            90 => Ok(()), // user_canceled: no cierra nada
            a => Err(Fallo(0, TlsError::Alerta(a))),
        }
    }
}

const UNEXP_NO_TLS: Fallo = Fallo(10, TlsError::Protocolo("lo recibido no parece TLS"));

impl Drop for Client {
    fn drop(&mut self) {
        crate::crypto::wipe(&mut self.x_priv);
        if let Some(p) = self.p_priv.as_mut() {
            crate::crypto::wipe(p);
        }
    }
}
