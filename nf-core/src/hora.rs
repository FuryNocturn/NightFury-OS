// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! v0.12.3: zona horaria y hora real.
//!
//! El reloj del PC (CMOS) guarda la hora LOCAL, como Windows. La UTC sale de restarle el desplazamiento de
//! la zona en ese instante. La tabla es corta (zonas habituales) y las reglas de verano están escritas desde
//! las normas legales de cada región, no copiadas de ninguna base de datos de zonas:
//! - UE: del último domingo de marzo a las 01:00 UTC al último domingo de octubre a las 01:00 UTC.
//! - EE. UU.: del 2.º domingo de marzo a las 02:00 locales al 1.er domingo de noviembre a las 02:00 locales.
//! - Chile: del primer sábado de septiembre a las 24:00 al primer sábado de abril a las 24:00 (locales).
//! - Sídney: del 1.er domingo de octubre (02:00) al 1.er domingo de abril (03:00), hora local.
//! - Nueva Zelanda: del último domingo de septiembre (02:00) al 1.er domingo de abril (03:00), hora local.
//!
//! De local a UTC, la hora que se repite al retrasar el reloj vale la PRIMERA vez (la de verano), y la que no
//! existe al adelantarlo se toma como hora de invierno (sale una hora más tarde en local).
//!
//! El ajuste (zona + «automático») va en `C:\Sistema\config\hora`; solo un administrador lo cambia.
use crate::platform::DateTime;
use alloc::string::String;
use alloc::vec::Vec;

pub const FILE: &str = "C:\\Sistema\\config\\hora";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Regla {
    Fija,
    Ue,
    Eeuu,
    Chile,
    Sidney,
    NuevaZelanda,
}

#[derive(Clone, Copy, Debug)]
pub struct Zona {
    /// Identificador que se guarda en el archivo (sin espacios).
    pub id: &'static str,
    /// Nombre visible: «(UTC+01:00) Madrid» (el desplazamiento es el de invierno / hora estándar).
    pub nombre: &'static str,
    /// Minutos respecto a UTC en hora estándar.
    pub std: i16,
    pub regla: Regla,
}

const fn z(id: &'static str, nombre: &'static str, std: i16, regla: Regla) -> Zona {
    Zona { id, nombre, std, regla }
}

use Regla::*;

/// La primera (Madrid) es la de por defecto.
// shortcut: Egipto y Marruecos van fijos (el verano de Egipto y el parón de Marruecos en Ramadán no se
// calculan; fallaría 1 h unas semanas al año), y las zonas son solo ~40; ampliar con la tabla si hace falta.
pub const ZONAS: &[Zona] = &[
    z("Madrid", "(UTC+01:00) Madrid", 60, Ue),
    z("Canarias", "(UTC+00:00) Islas Canarias", 0, Ue),
    z("Lisboa", "(UTC+00:00) Lisboa", 0, Ue),
    z("Londres", "(UTC+00:00) Londres", 0, Ue),
    z("Paris", "(UTC+01:00) París", 60, Ue),
    z("Berlin", "(UTC+01:00) Berlín", 60, Ue),
    z("Roma", "(UTC+01:00) Roma", 60, Ue),
    z("Atenas", "(UTC+02:00) Atenas", 120, Ue),
    z("Helsinki", "(UTC+02:00) Helsinki", 120, Ue),
    z("Moscu", "(UTC+03:00) Moscú", 180, Fija),
    z("Estambul", "(UTC+03:00) Estambul", 180, Fija),
    z("NuevaYork", "(UTC-05:00) Nueva York", -300, Eeuu),
    z("Chicago", "(UTC-06:00) Chicago", -360, Eeuu),
    z("Denver", "(UTC-07:00) Denver", -420, Eeuu),
    z("Phoenix", "(UTC-07:00) Phoenix", -420, Fija),
    z("LosAngeles", "(UTC-08:00) Los Ángeles", -480, Eeuu),
    z("Anchorage", "(UTC-09:00) Anchorage", -540, Eeuu),
    z("Honolulu", "(UTC-10:00) Honolulu", -600, Fija),
    z("Mexico", "(UTC-06:00) Ciudad de México", -360, Fija),
    z("Bogota", "(UTC-05:00) Bogotá", -300, Fija),
    z("Lima", "(UTC-05:00) Lima", -300, Fija),
    z("Caracas", "(UTC-04:00) Caracas", -240, Fija),
    z("Santiago", "(UTC-04:00) Santiago de Chile", -240, Chile),
    z("BuenosAires", "(UTC-03:00) Buenos Aires", -180, Fija),
    z("SaoPaulo", "(UTC-03:00) São Paulo", -180, Fija),
    z("Casablanca", "(UTC+01:00) Casablanca", 60, Fija),
    z("Lagos", "(UTC+01:00) Lagos", 60, Fija),
    z("ElCairo", "(UTC+02:00) El Cairo", 120, Fija),
    z("Johannesburgo", "(UTC+02:00) Johannesburgo", 120, Fija),
    z("Dubai", "(UTC+04:00) Dubái", 240, Fija),
    z("Karachi", "(UTC+05:00) Karachi", 300, Fija),
    z("India", "(UTC+05:30) India", 330, Fija),
    z("Daca", "(UTC+06:00) Daca", 360, Fija),
    z("Bangkok", "(UTC+07:00) Bangkok", 420, Fija),
    z("Pekin", "(UTC+08:00) Pekín / Shanghái", 480, Fija),
    z("Tokio", "(UTC+09:00) Tokio", 540, Fija),
    z("Seul", "(UTC+09:00) Seúl", 540, Fija),
    z("Brisbane", "(UTC+10:00) Brisbane", 600, Fija),
    z("Sidney", "(UTC+10:00) Sídney", 600, Sidney),
    z("Auckland", "(UTC+12:00) Auckland", 720, NuevaZelanda),
    z("UTC", "(UTC+00:00) UTC", 0, Fija),
];

// ---------------------------------------------------------------- calendario (días desde 1970-01-01)

/// Días desde 1970-01-01 de una fecha civil (algoritmo de días con eras de 400 años).
fn dias(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

/// Día de la semana: 0 domingo … 6 sábado.
fn semana(dias: i64) -> i64 {
    (dias + 4).rem_euclid(7)
}

/// El día `n` (1, 2…) con día de la semana `wd` del mes.
fn enesimo(y: i64, m: i64, wd: i64, n: i64) -> i64 {
    let primero = dias(y, m, 1);
    primero + (wd - semana(primero)).rem_euclid(7) + 7 * (n - 1)
}

/// El último día con día de la semana `wd` del mes.
fn ultimo(y: i64, m: i64, wd: i64) -> i64 {
    let fin = dias(if m == 12 { y + 1 } else { y }, if m == 12 { 1 } else { m + 1 }, 1) - 1;
    fin - (semana(fin) - wd).rem_euclid(7)
}

const DOM: i64 = 0;
const SAB: i64 = 6;

/// Segundos Unix de una fecha y hora (sin zona: quien llama dice si es local o UTC).
pub fn segundos(d: DateTime) -> i64 {
    dias(d.year as i64, d.month as i64, d.day as i64) * 86_400 + d.hour as i64 * 3600 + d.minute as i64 * 60 + d.second as i64
}

/// Lo contrario de `segundos`.
pub fn de_segundos(s: i64) -> DateTime {
    let (y, m, d) = civil(s.div_euclid(86_400));
    let t = s.rem_euclid(86_400);
    DateTime { year: y.clamp(0, 65535) as u16, month: m as u8, day: d as u8, hour: (t / 3600) as u8, minute: (t % 3600 / 60) as u8, second: (t % 60) as u8 }
}

/// `true` si la fecha y hora existen en el calendario y caben en el reloj del PC (2000..=2099).
pub fn valida(d: DateTime) -> bool {
    let bis = d.year % 4 == 0 && (d.year % 100 != 0 || d.year % 400 == 0);
    let dm = match d.month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => 28 + bis as u8,
        _ => return false,
    };
    (2000..=2099).contains(&d.year) && d.day >= 1 && d.day <= dm && d.hour < 24 && d.minute < 60 && d.second < 60
}

// ---------------------------------------------------------------- zonas

impl Zona {
    /// Instantes UTC (segundos Unix) en que empieza y acaba el verano del año `y`, si la zona lo tiene.
    fn verano(&self, y: i64) -> Option<(i64, i64)> {
        let (std, dst) = (self.std as i64 * 60, self.std as i64 * 60 + 3600);
        Some(match self.regla {
            Fija => return None,
            Ue => (ultimo(y, 3, DOM) * 86_400 + 3600, ultimo(y, 10, DOM) * 86_400 + 3600),
            Eeuu => (enesimo(y, 3, DOM, 2) * 86_400 + 7200 - std, enesimo(y, 11, DOM, 1) * 86_400 + 7200 - dst),
            // El sábado a las 24:00 es el domingo a las 00:00.
            Chile => ((enesimo(y, 9, SAB, 1) + 1) * 86_400 - std, (enesimo(y, 4, SAB, 1) + 1) * 86_400 - dst),
            Sidney => (enesimo(y, 10, DOM, 1) * 86_400 + 7200 - std, enesimo(y, 4, DOM, 1) * 86_400 + 10_800 - dst),
            NuevaZelanda => (ultimo(y, 9, DOM) * 86_400 + 7200 - std, enesimo(y, 4, DOM, 1) * 86_400 + 10_800 - dst),
        })
    }

    /// ¿Es horario de verano en el instante UTC `utc` (segundos Unix)?
    pub fn es_verano(&self, utc: i64) -> bool {
        let y = civil(utc.div_euclid(86_400)).0;
        match self.verano(y) {
            None => false,
            // Hemisferio norte: dentro del año; sur: empieza en un año y acaba en el siguiente.
            Some((ini, fin)) if ini < fin => utc >= ini && utc < fin,
            Some((ini, fin)) => utc >= ini || utc < fin,
        }
    }

    /// Minutos que hay que sumar a la UTC para tener la hora local en el instante UTC `utc`.
    pub fn desplazamiento(&self, utc: i64) -> i32 {
        self.std as i32 + if self.es_verano(utc) { 60 } else { 0 }
    }
}

/// UTC → hora local de la zona.
pub fn utc_a_local(utc: DateTime, z: &Zona) -> DateTime {
    let s = segundos(utc);
    de_segundos(s + z.desplazamiento(s) as i64 * 60)
}

/// Hora local de la zona → UTC (ver las reglas de la hora repetida y de la que no existe arriba).
pub fn local_a_utc(local: DateTime, z: &Zona) -> DateTime {
    let l = segundos(local);
    let con_verano = l - (z.std as i64 + 60) * 60;
    de_segundos(if z.es_verano(con_verano) { con_verano } else { l - z.std as i64 * 60 })
}

/// La zona con ese identificador, o Madrid si no existe.
pub fn zona(id: &str) -> &'static Zona {
    ZONAS.iter().find(|z| z.id == id).unwrap_or(&ZONAS[0])
}

// ---------------------------------------------------------------- ajuste guardado

/// Zona (posición en `ZONAS`) y si la hora se ajusta sola por internet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ajuste {
    pub zona: u8,
    pub auto: bool,
}

impl Default for Ajuste {
    fn default() -> Self {
        Ajuste { zona: 0, auto: true }
    }
}

impl Ajuste {
    pub fn zona(&self) -> &'static Zona {
        ZONAS.get(self.zona as usize).unwrap_or(&ZONAS[0])
    }
}

/// Lee `zona=<id>` y `automatico=si|no`; lo que falta o no se entiende queda por defecto.
pub fn parse(bytes: &[u8]) -> Ajuste {
    let mut a = Ajuste::default();
    for l in String::from_utf8_lossy(bytes).lines() {
        match l.split_once('=') {
            Some(("zona", v)) => a.zona = ZONAS.iter().position(|z| z.id == v.trim()).unwrap_or(0) as u8,
            Some(("automatico", v)) => a.auto = v.trim() != "no",
            _ => {}
        }
    }
    a
}

pub fn render(a: &Ajuste) -> Vec<u8> {
    alloc::format!("zona={}\nautomatico={}\n", a.zona().id, if a.auto { "si" } else { "no" }).into_bytes()
}

/// Última sincronización con internet, para Configuración.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SyncInfo {
    /// 0 nunca, 1 en curso, 2 hora ajustada, 3 la hora ya era correcta, 4 sin respuesta.
    pub estado: u8,
    /// Hora local del equipo cuando acabó (vacía si nunca).
    pub fin: DateTime,
}

/// Lo que `Files` recuerda de la hora: ajuste leído, orden de poner el reloj, petición de «Sincronizar ahora»
/// y resultado. El escritorio (que es quien tiene el reloj y la red) lo atiende en cada tick.
#[derive(Default)]
pub struct Estado {
    pub ajuste: Option<Ajuste>,
    pub poner: Option<DateTime>,
    pub sync_pedida: bool,
    pub info: SyncInfo,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> DateTime {
        DateTime { year, month, day, hour, minute, second: 0 }
    }

    /// Hora local que da `utc` en la zona `id`.
    fn local(id: &str, utc: DateTime) -> DateTime {
        utc_a_local(utc, zona(id))
    }

    #[test]
    fn calendario_ida_y_vuelta() {
        assert_eq!(segundos(dt(1970, 1, 1, 0, 0)), 0);
        assert_eq!(segundos(dt(2023, 11, 14, 22, 13)), 1_699_999_980);
        assert_eq!(de_segundos(1_699_999_980), dt(2023, 11, 14, 22, 13));
        assert_eq!(de_segundos(segundos(dt(2096, 2, 29, 23, 59))), dt(2096, 2, 29, 23, 59));
        assert_eq!(semana(dias(2026, 3, 29)), DOM);
        assert_eq!(ultimo(2026, 3, DOM), dias(2026, 3, 29));
        assert_eq!(ultimo(2026, 10, DOM), dias(2026, 10, 25));
        assert_eq!(enesimo(2026, 3, DOM, 2), dias(2026, 3, 8));
        assert_eq!(enesimo(2026, 11, DOM, 1), dias(2026, 11, 1));
    }

    #[test]
    fn fechas_validas() {
        assert!(valida(dt(2026, 2, 28, 23, 59)));
        assert!(!valida(dt(2026, 2, 29, 0, 0)));
        assert!(valida(dt(2028, 2, 29, 0, 0)));
        assert!(!valida(dt(1999, 12, 31, 0, 0)));
        assert!(!valida(dt(2026, 13, 1, 0, 0)));
        assert!(!valida(dt(2026, 4, 31, 0, 0)));
        assert!(!valida(DateTime { second: 60, ..dt(2026, 1, 1, 0, 0) }));
    }

    #[test]
    fn ue_cambio_de_marzo() {
        // 2026-03-29 01:00 UTC: Madrid pasa de 02:00 a 03:00.
        assert_eq!(local("Madrid", dt(2026, 3, 29, 0, 59)), dt(2026, 3, 29, 1, 59));
        assert_eq!(local("Madrid", dt(2026, 3, 29, 1, 0)), dt(2026, 3, 29, 3, 0));
        // Lisboa (UTC+0) cambia en el mismo instante UTC.
        assert_eq!(local("Lisboa", dt(2026, 3, 29, 1, 0)), dt(2026, 3, 29, 2, 0));
        // Hora que no existe: 02:30 se toma como invierno y sale 03:30.
        assert_eq!(local_a_utc(dt(2026, 3, 29, 2, 30), zona("Madrid")), dt(2026, 3, 29, 1, 30));
        assert_eq!(local_a_utc(dt(2026, 3, 29, 1, 59), zona("Madrid")), dt(2026, 3, 29, 0, 59));
        assert_eq!(local_a_utc(dt(2026, 3, 29, 3, 0), zona("Madrid")), dt(2026, 3, 29, 1, 0));
    }

    #[test]
    fn ue_cambio_de_octubre() {
        // 2026-10-25 01:00 UTC: Madrid pasa de 03:00 a 02:00.
        assert_eq!(local("Madrid", dt(2026, 10, 25, 0, 59)), dt(2026, 10, 25, 2, 59));
        assert_eq!(local("Madrid", dt(2026, 10, 25, 1, 0)), dt(2026, 10, 25, 2, 0));
        // 02:30 se repite: vale la primera vez (verano, 00:30 UTC).
        assert_eq!(local_a_utc(dt(2026, 10, 25, 2, 30), zona("Madrid")), dt(2026, 10, 25, 0, 30));
        assert_eq!(local_a_utc(dt(2026, 10, 25, 3, 0), zona("Madrid")), dt(2026, 10, 25, 2, 0));
        assert_eq!(local_a_utc(dt(2026, 7, 1, 12, 0), zona("Madrid")), dt(2026, 7, 1, 10, 0));
        assert_eq!(local_a_utc(dt(2026, 1, 1, 12, 0), zona("Madrid")), dt(2026, 1, 1, 11, 0));
    }

    #[test]
    fn eeuu_cambios() {
        // 2026-03-08 02:00 locales (EST, 07:00 UTC): salta a 03:00.
        assert_eq!(local("NuevaYork", dt(2026, 3, 8, 6, 59)), dt(2026, 3, 8, 1, 59));
        assert_eq!(local("NuevaYork", dt(2026, 3, 8, 7, 0)), dt(2026, 3, 8, 3, 0));
        // 2026-11-01 02:00 locales (EDT, 06:00 UTC): vuelve a 01:00.
        assert_eq!(local("NuevaYork", dt(2026, 11, 1, 5, 59)), dt(2026, 11, 1, 1, 59));
        assert_eq!(local("NuevaYork", dt(2026, 11, 1, 6, 0)), dt(2026, 11, 1, 1, 0));
        // Los Ángeles: el mismo día, a otra hora UTC.
        assert_eq!(local("LosAngeles", dt(2026, 3, 8, 10, 0)), dt(2026, 3, 8, 3, 0));
        // Phoenix no cambia.
        assert_eq!(local("Phoenix", dt(2026, 7, 1, 12, 0)), dt(2026, 7, 1, 5, 0));
        assert_eq!(local_a_utc(dt(2026, 7, 1, 12, 0), zona("NuevaYork")), dt(2026, 7, 1, 16, 0));
    }

    #[test]
    fn hemisferio_sur() {
        // Sídney: verano (UTC+11) en enero, invierno (UTC+10) en julio.
        assert_eq!(local("Sidney", dt(2026, 1, 15, 0, 0)), dt(2026, 1, 15, 11, 0));
        assert_eq!(local("Sidney", dt(2026, 7, 15, 0, 0)), dt(2026, 7, 15, 10, 0));
        // Empieza el 1.er domingo de octubre (2026-10-04) a las 02:00 locales = sábado 16:00 UTC.
        assert_eq!(local("Sidney", dt(2026, 10, 3, 15, 59)), dt(2026, 10, 4, 1, 59));
        assert_eq!(local("Sidney", dt(2026, 10, 3, 16, 0)), dt(2026, 10, 4, 3, 0));
        // Acaba el 1.er domingo de abril (2026-04-05) a las 03:00 locales = sábado 16:00 UTC.
        assert_eq!(local("Sidney", dt(2026, 4, 4, 15, 59)), dt(2026, 4, 5, 2, 59));
        assert_eq!(local("Sidney", dt(2026, 4, 4, 16, 0)), dt(2026, 4, 5, 2, 0));
        // Nueva Zelanda: empieza el último domingo de septiembre (2026-09-27) y acaba el 1.º de abril.
        assert_eq!(local("Auckland", dt(2026, 9, 26, 13, 59)), dt(2026, 9, 27, 1, 59));
        assert_eq!(local("Auckland", dt(2026, 9, 26, 14, 0)), dt(2026, 9, 27, 3, 0));
        assert_eq!(local("Auckland", dt(2026, 1, 15, 0, 0)), dt(2026, 1, 15, 13, 0));
        assert_eq!(local("Auckland", dt(2026, 7, 15, 0, 0)), dt(2026, 7, 15, 12, 0));
        // Chile: verano (UTC-3) en enero; acaba el sábado 2026-04-04 a las 24:00 (03:00 UTC del domingo).
        assert_eq!(local("Santiago", dt(2026, 1, 15, 12, 0)), dt(2026, 1, 15, 9, 0));
        assert_eq!(local("Santiago", dt(2026, 4, 5, 2, 59)), dt(2026, 4, 4, 23, 59));
        assert_eq!(local("Santiago", dt(2026, 4, 5, 3, 0)), dt(2026, 4, 4, 23, 0));
        // Empieza el sábado 2026-09-05 a las 24:00 (04:00 UTC del domingo).
        assert_eq!(local("Santiago", dt(2026, 9, 6, 3, 59)), dt(2026, 9, 5, 23, 59));
        assert_eq!(local("Santiago", dt(2026, 9, 6, 4, 0)), dt(2026, 9, 6, 1, 0));
        assert_eq!(local_a_utc(dt(2026, 1, 15, 11, 0), zona("Sidney")), dt(2026, 1, 15, 0, 0));
    }

    #[test]
    fn ida_y_vuelta_en_todas_las_zonas() {
        // Fuera de la hora repetida o inexistente, local→UTC deshace UTC→local.
        for z in ZONAS {
            for (m, d) in [(1, 15), (4, 20), (7, 15), (10, 15), (12, 20)] {
                let u = dt(2026, m, d, 12, 34);
                assert_eq!(local_a_utc(utc_a_local(u, z), z), u, "{}", z.id);
            }
        }
        assert_eq!(ZONAS.len(), 41);
    }

    #[test]
    fn ajuste_guardado() {
        assert_eq!(parse(b""), Ajuste { zona: 0, auto: true });
        let a = Ajuste { zona: ZONAS.iter().position(|z| z.id == "Tokio").unwrap() as u8, auto: false };
        assert_eq!(parse(&render(&a)), a);
        assert_eq!(parse(b"zona=Marte\nautomatico=no\nbasura"), Ajuste { zona: 0, auto: false });
    }
}
