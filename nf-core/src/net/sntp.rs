// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! v0.12.3: cliente SNTP (RFC 4330, formato de NTPv4 en RFC 5905) para poner la hora real.
//!
//! Una consulta por UDP 123 a `pool.ntp.org`: se manda un paquete de 48 bytes en modo cliente con un
//! «transmit» que no se puede adivinar y solo vale la respuesta que lo devuelve en `originate`, viene de
//! quien preguntamos, en modo servidor, con estrato 1..15 y sin alarma de reloj (LI ≠ 3). El estrato 0 es
//! «Kiss-o'-Death» (el servidor pide que se calle): no se usa y la próxima prueba es a los 10 minutos.
//! Lo manda el escritorio (`Sntp::tick`): al tener dirección por DHCP, cada 24 h y con «Sincronizar ahora».
use super::{Ipv4, NetApi, QueryId, State, UdpId};
use crate::hora::{self, SyncInfo};
use crate::platform::Platform;

pub const SERVIDOR: &str = "pool.ntp.org";
const PUERTO: u16 = 123;
/// Segundos de 1900-01-01 a 1970-01-01.
const NTP_A_UNIX: i64 = 2_208_988_800;
/// Si la hora del equipo difiere menos que esto, no se toca.
const UMBRAL_MS: i64 = 2_000;
const REENVIO_MS: u64 = 4_000;
const ESPERA_MS: u64 = 8_000;
const CADA_MS: u64 = 24 * 3600 * 1000;
const REINTENTO_MS: u64 = 10 * 60 * 1000;

/// Paquete de consulta: LI 0, versión 4, modo 3 (cliente), con `tx` como «transmit».
pub fn peticion(tx: u64) -> [u8; 48] {
    let mut p = [0u8; 48];
    p[0] = 0x23;
    p[40..48].copy_from_slice(&tx.to_be_bytes());
    p
}

#[derive(Debug, PartialEq, Eq)]
pub enum Rechazo {
    Corta,
    NoEsServidor,
    Version,
    /// LI = 3: el servidor dice que no está sincronizado.
    Alarma,
    /// Estrato 0 (Kiss-o'-Death u otro aviso) o mayor que 15.
    Estrato,
    /// El `originate` no es el `transmit` que enviamos.
    Origen,
    SinHora,
}

/// Comprueba la respuesta y devuelve la hora del servidor al contestar, en ms Unix.
pub fn comprobar(r: &[u8], tx: u64) -> Result<i64, Rechazo> {
    if r.len() < 48 {
        return Err(Rechazo::Corta);
    }
    if r[0] & 7 != 4 {
        return Err(Rechazo::NoEsServidor);
    }
    if !(3..=4).contains(&(r[0] >> 3 & 7)) {
        return Err(Rechazo::Version);
    }
    if r[0] >> 6 == 3 {
        return Err(Rechazo::Alarma);
    }
    if !(1..=15).contains(&r[1]) {
        return Err(Rechazo::Estrato);
    }
    if r[24..32] != tx.to_be_bytes() {
        return Err(Rechazo::Origen);
    }
    let (s, f) = (u32::from_be_bytes([r[40], r[41], r[42], r[43]]) as i64, u32::from_be_bytes([r[44], r[45], r[46], r[47]]) as i64);
    if s == 0 {
        return Err(Rechazo::SinHora);
    }
    // Con el bit alto a 0 la fecha es de la era 1 (desde 2036).
    let unix = if s & 0x8000_0000 != 0 { s - NTP_A_UNIX } else { s + (1 << 32) - NTP_A_UNIX };
    Ok(unix * 1000 + (f * 1000 >> 32))
}

enum Fase {
    Libre,
    Resolviendo(QueryId),
    Esperando { id: UdpId, dst: Ipv4, tx: u64, envio: u64, desde: u64, reenviado: bool },
}

enum Fin {
    /// Hora del servidor (ms Unix).
    Hora(i64),
    Error,
}

pub struct Sntp {
    fase: Fase,
    /// Desde cuándo (ms desde el arranque) toca la próxima consulta automática.
    proxima: u64,
    pedida: bool,
}

impl Default for Sntp {
    fn default() -> Self {
        Sntp { fase: Fase::Libre, proxima: 0, pedida: false }
    }
}

impl Sntp {
    /// Un paso: atiende las órdenes de hora de `Files`, lleva la consulta en marcha y aplica el resultado.
    pub fn tick(&mut self, sys: &mut dyn Platform, ms: u64) {
        let (poner, pedida) = sys.files().hora_pendiente();
        if let Some(t) = poner {
            sys.set_now(t);
        }
        self.pedida |= pedida;
        let cfg = sys.files().time_cfg();
        let (utc, azar) = (hora::segundos(hora::local_a_utc(sys.now(), cfg.zona())), sys.entropy());
        let libre = matches!(self.fase, Fase::Libre);
        let fin = match sys.net() {
            Some(net) => self.paso(net, ms, cfg.auto, utc, azar),
            None if self.pedida => Some(Fin::Error),
            None => None,
        };
        let estado = match fin {
            Some(Fin::Hora(servidor)) => Some(aplicar(sys, servidor)),
            Some(Fin::Error) => Some(4),
            None => None,
        };
        if let Some(estado) = estado {
            self.fase = Fase::Libre;
            self.pedida = false;
            self.proxima = ms + if estado == 4 { REINTENTO_MS } else { CADA_MS };
            let info = SyncInfo { estado, fin: sys.now() };
            sys.files().hora_sync_info(info);
        } else if libre && !matches!(self.fase, Fase::Libre) {
            let info = SyncInfo { estado: 1, ..sys.files().time_sync_info() };
            sys.files().hora_sync_info(info);
        }
    }

    fn paso(&mut self, net: &mut dyn NetApi, ms: u64, auto: bool, utc: i64, azar: u64) -> Option<Fin> {
        match self.fase {
            Fase::Libre => {
                if !(self.pedida || auto && ms >= self.proxima) {
                    return None;
                }
                if net.status().state != State::Connected {
                    return self.pedida.then_some(Fin::Error);
                }
                match net.resolve(SERVIDOR) {
                    Ok(q) => self.fase = Fase::Resolviendo(q),
                    Err(_) => return Some(Fin::Error),
                }
            }
            Fase::Resolviendo(q) => match net.resolve_poll(q)? {
                Ok(ips) if !ips.is_empty() => {
                    // Segundos NTP de la hora actual y 32 bits de azar: nadie que no vea el paquete lo adivina.
                    let tx = ((utc + NTP_A_UNIX) as u64 & 0xFFFF_FFFF) << 32 | azar & 0xFFFF_FFFF;
                    let Ok(id) = net.udp_bind(0) else { return Some(Fin::Error) };
                    let _ = net.udp_send(id, ips[0], PUERTO, &peticion(tx));
                    self.fase = Fase::Esperando { id, dst: ips[0], tx, envio: ms, desde: ms, reenviado: false };
                }
                _ => return Some(Fin::Error),
            },
            Fase::Esperando { id, dst, tx, envio, desde, reenviado } => {
                loop {
                    match net.udp_recv(id) {
                        Ok(Some(d)) if d.from == dst && d.port == PUERTO => {
                            if let Ok(t) = comprobar(&d.data, tx) {
                                net.udp_close(id);
                                // La hora del servidor es la de su respuesta: se suma medio viaje.
                                return Some(Fin::Hora(t + (ms - envio) as i64 / 2));
                            }
                        }
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                        Err(_) => return Some(Fin::Error),
                    }
                }
                if ms - desde >= ESPERA_MS {
                    net.udp_close(id);
                    return Some(Fin::Error);
                }
                if !reenviado && ms - desde >= REENVIO_MS {
                    let _ = net.udp_send(id, dst, PUERTO, &peticion(tx));
                    self.fase = Fase::Esperando { id, dst, tx, envio: ms, desde, reenviado: true };
                }
            }
        }
        None
    }
}

/// Pone el reloj según la hora del servidor (ms Unix). Devuelve el estado de `SyncInfo`: 2 ajustada, 3 sin cambios.
fn aplicar(sys: &mut dyn Platform, servidor_ms: i64) -> u8 {
    let z = sys.files().time_cfg().zona();
    let propia_ms = hora::segundos(hora::local_a_utc(sys.now(), z)) * 1000;
    if (servidor_ms - propia_ms).abs() < UMBRAL_MS {
        return 3;
    }
    sys.set_now(hora::utc_a_local(hora::de_segundos(servidor_ms.div_euclid(1000)), z));
    2
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{DateTime, TestPlatform};

    const TX: u64 = 0xE9A1_0000_1234_5678;

    /// Respuesta de servidor válida con la hora Unix `unix` s: LI 0, v4, modo 4, estrato 2.
    fn respuesta(unix: i64) -> [u8; 48] {
        let mut r = [0u8; 48];
        r[0] = 0x24;
        r[1] = 2;
        r[24..32].copy_from_slice(&TX.to_be_bytes());
        let s = (unix + NTP_A_UNIX) as u32;
        r[40..44].copy_from_slice(&s.to_be_bytes());
        r[44..48].copy_from_slice(&0x8000_0000u32.to_be_bytes()); // medio segundo
        r
    }

    #[test]
    fn consulta_bien_formada() {
        let p = peticion(TX);
        assert_eq!((p[0] >> 6, p[0] >> 3 & 7, p[0] & 7), (0, 4, 3));
        assert_eq!(&p[40..48], &TX.to_be_bytes());
        assert!(p[1..40].iter().all(|&b| b == 0));
    }

    #[test]
    fn respuesta_valida() {
        // 2023-11-14 22:13:20 UTC = 1_700_000_000.
        assert_eq!(comprobar(&respuesta(1_700_000_000), TX), Ok(1_700_000_000_500));
        // Desde 2036 el bit alto de los segundos vuelve a 0 (era 1): 2040-01-01 = 2_208_988_800 s Unix.
        assert_eq!(comprobar(&respuesta(2_208_988_800), TX), Ok(2_208_988_800_500));
    }

    #[test]
    fn respuestas_que_se_rechazan() {
        let ok = respuesta(1_700_000_000);
        // «originate» distinto: un paquete falso o de otra consulta.
        assert_eq!(comprobar(&ok, TX ^ 1), Err(Rechazo::Origen));
        // Estrato 0: Kiss-o'-Death (RATE) y también «sin definir».
        let mut kod = ok;
        kod[1] = 0;
        kod[12..16].copy_from_slice(b"RATE");
        assert_eq!(comprobar(&kod, TX), Err(Rechazo::Estrato));
        let mut e16 = ok;
        e16[1] = 16;
        assert_eq!(comprobar(&e16, TX), Err(Rechazo::Estrato));
        // LI = 3: servidor sin sincronizar.
        let mut li = ok;
        li[0] |= 0xC0;
        assert_eq!(comprobar(&li, TX), Err(Rechazo::Alarma));
        // Modo cliente (3) o difusión (5) en vez de servidor.
        for modo in [3, 5, 0] {
            let mut m = ok;
            m[0] = m[0] & !7 | modo;
            assert_eq!(comprobar(&m, TX), Err(Rechazo::NoEsServidor));
        }
        let mut v = ok;
        v[0] = v[0] & !0x38 | 2 << 3;
        assert_eq!(comprobar(&v, TX), Err(Rechazo::Version));
        assert_eq!(comprobar(&ok[..47], TX), Err(Rechazo::Corta));
        let mut sin = ok;
        sin[40..44].fill(0);
        assert_eq!(comprobar(&sin, TX), Err(Rechazo::SinHora));
    }

    #[test]
    fn aplicar_respeta_el_umbral_y_la_zona() {
        // El reloj de pruebas marca 2026-11-15 21:37:05 en Madrid (invierno): 20:37:05 UTC.
        let mut sys = TestPlatform::default();
        let utc = hora::segundos(DateTime { year: 2026, month: 11, day: 15, hour: 20, minute: 37, second: 5 });
        assert_eq!(aplicar(&mut sys, utc * 1000 + 1_900), 3);
        assert_eq!(aplicar(&mut sys, utc * 1000 - 1_900), 3);
        assert_eq!(sys.reloj, None);
        assert_eq!(aplicar(&mut sys, (utc + 600) * 1000), 2);
        assert_eq!(sys.reloj, Some(DateTime { year: 2026, month: 11, day: 15, hour: 21, minute: 47, second: 5 }));
    }
}
