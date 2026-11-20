// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Cliente HTTP/1.1 (RFC 9110 y 9112) para `curl`, sobre `Net`. Sin bloquear: se crea con `start` y
//! se llama a `poll` en cada tick hasta que devuelve `Done` o `Error`.
//!
//! CONTRATO: las firmas públicas no cambian.
use super::{Net, NetError};
use alloc::string::String;
use alloc::vec::Vec;

/// URL `http://host[:puerto][/ruta][?consulta]`. El fragmento (`#…`) se descarta.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Url {
    pub host: String,
    pub port: u16,
    /// Empieza por `/` (vale `/` si la URL no trae ruta); incluye la consulta.
    pub path: String,
}

/// Sin esquema se supone `http://`. `https://` da `HttpsNotYet`; otro esquema, `BadUrl`.
pub fn parse_url(_s: &str) -> Result<Url, NetError> {
    todo!()
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HttpEvent {
    /// Cabeceras de la respuesta final (tras seguir las redirecciones, si se pidió).
    Head { status: u16, reason: String, headers: Vec<(String, String)> },
    /// Un trozo del cuerpo, ya sin la codificación «chunked».
    Body(Vec<u8>),
    Done,
    Error(NetError),
}

/// Topes: cabeceras de 32 KiB como mucho y 10 redirecciones.
pub const MAX_HEAD: usize = 32 * 1024;
pub const MAX_REDIRECTS: u8 = 10;

/// Una petición `GET` (o `HEAD` con `head_only`). Con `follow`, sigue las redirecciones 301, 302,
/// 303, 307 y 308 (una a `https://` termina en `HttpsNotYet`). Envía `Host`, `User-Agent:
/// NightFury/<versión>`, `Accept: */*` y `Connection: close`.
pub struct HttpGet {
    _todo: (),
}

impl HttpGet {
    pub fn start(_net: &mut Net, _url: &str, _head_only: bool, _follow: bool) -> Result<HttpGet, NetError> {
        todo!()
    }

    /// Avanza y devuelve lo que haya pasado desde la última llamada (vacío si nada). Tras `Done` o
    /// `Error`, la conexión ya está cerrada y no devuelve nada más.
    pub fn poll(&mut self, _net: &mut Net, _now_ms: u64) -> Vec<HttpEvent> {
        todo!()
    }

    /// Cancela (Ctrl+C en la Terminal) y libera la conexión.
    pub fn cancel(&mut self, _net: &mut Net) {
        todo!()
    }
}
