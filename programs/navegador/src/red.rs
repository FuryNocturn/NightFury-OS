// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Lo que comparten las pestañas de una ventana: el tarro de cookies (con lo que se guarda en disco) y las
//! descargas. Una ventana de incógnito nunca lee ni escribe las cookies en disco.
use crate::cookies::{sitio, Tarro};
use crate::datos::Datos;
use crate::descargas::Descargas;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use nf_core::fsapi::FilesApi;
use nf_core::hora::{self, Zona};
use nf_core::platform::{DateTime, Platform};

/// Cuánto se espera tras un cambio antes de escribir (se juntan varios).
const RETRASO_MS: u64 = 2_000;
const REINTENTO_MS: u64 = 30_000;
/// Sitios con «Recordar sesión».
const MAX_RECORDAR: usize = 500;

pub type RedRc = Rc<RefCell<Red>>;

#[derive(Default)]
pub struct Red {
    pub tarro: Tarro,
    pub descargas: Descargas,
    pub incognito: bool,
    /// Sitios (`cookies::sitio`) cuyas cookies persistentes se guardan en disco aunque no estén en favoritos.
    pub recordar: Vec<String>,
    /// Zona horaria del sistema (`Some(None)` = no se pudo leer: se usa la hora local tal cual).
    zona: Option<Option<&'static Zona>>,
    cargado: bool,
    c_en: Option<u64>,
    r_en: Option<u64>,
    /// Último texto escrito o leído de cada fichero: si no cambia, no se escribe.
    escrito_c: String,
    escrito_r: String,
}

impl Red {
    pub fn nueva() -> RedRc {
        Rc::new(RefCell::new(Red::default()))
    }

    /// Segundos Unix en UTC (las cookies caducan en UTC; el reloj del equipo es local).
    // shortcut: si el programa no puede leer la zona de `C:\Sistema\config\hora`, se usa la hora local como si
    // fuera UTC: solo falla un `Expires` absoluto (hasta ±14 h); `Max-Age` es relativo y sale bien.
    pub fn ahora_utc(&mut self, sys: &mut dyn Platform) -> i64 {
        let local: DateTime = sys.now();
        let z = *self.zona.get_or_insert_with(|| sys.fs().read(hora::FILE).ok().map(|b| hora::parse(&b).zona()));
        hora::segundos(z.map_or(local, |z| hora::local_a_utc(local, z)))
    }

    /// Lee las cookies y la lista de sitios recordados (una vez; nada en incógnito) y quita las caducadas.
    pub fn cargar(&mut self, fs: &mut dyn FilesApi, utc: i64) {
        if self.cargado || self.incognito {
            return;
        }
        self.cargado = true;
        let leer = |fs: &mut dyn FilesApi, n: &str| {
            let ruta = Datos::ruta(fs, n);
            fs.read(&ruta).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
        };
        self.escrito_c = leer(fs, "cookies");
        self.tarro.cargar(&self.escrito_c, utc);
        self.tarro.purgar(utc);
        self.escrito_r = leer(fs, "recordar");
        self.recordar = self.escrito_r.lines().map(|l| String::from(l.trim())).filter(|l| !l.is_empty()).take(MAX_RECORDAR).collect();
    }

    /// Hay cookies nuevas, borradas o de otros sitios: tocará escribir.
    pub fn cambio_cookies(&mut self, ms: u64) {
        if !self.incognito {
            self.c_en.get_or_insert(ms + RETRASO_MS);
        }
    }

    pub fn es_recordado(&self, host: &str) -> bool {
        let s = sitio(host);
        self.recordar.iter().any(|r| *r == s)
    }

    /// Pone o quita «Recordar sesión» de un sitio. No hace nada en incógnito.
    pub fn alternar_recordar(&mut self, host: &str, ms: u64) {
        if self.incognito {
            return;
        }
        let s = sitio(host);
        match self.recordar.iter().position(|r| *r == s) {
            Some(i) => {
                self.recordar.remove(i);
            }
            None if self.recordar.len() < MAX_RECORDAR => self.recordar.push(s),
            None => return,
        }
        self.r_en.get_or_insert(ms + RETRASO_MS);
        self.cambio_cookies(ms);
    }

    pub fn borrar_sitio(&mut self, host: &str, ms: u64) {
        self.tarro.borrar_sitio(host);
        self.cambio_cookies(ms);
    }

    /// Todas, en memoria y (al escribir) en disco.
    pub fn borrar_todo(&mut self, ms: u64) {
        self.tarro.borrar_todo();
        self.cambio_cookies(ms);
    }

    /// Cookies de un sitio (para el panel del candado).
    pub fn cuantas(&self, host: &str) -> usize {
        let s = sitio(host);
        self.tarro.cookies().iter().filter(|c| sitio(&c.dominio) == s).count()
    }

    /// Escribe lo pendiente (todo, con `forzar`). Solo las cookies persistentes de sitios en favoritos (`es_fav`) o
    /// recordados. En incógnito no hace nada.
    pub fn guardar(&mut self, fs: &mut dyn FilesApi, ms: u64, now: DateTime, forzar: bool, es_fav: impl Fn(&str) -> bool) {
        if self.incognito || !self.cargado {
            return;
        }
        let toca = |t: Option<u64>| t.is_some_and(|t| forzar || ms >= t);
        if toca(self.r_en) {
            let texto = self.recordar.iter().map(|s| alloc::format!("{s}\n")).collect::<String>();
            let ok = texto == self.escrito_r || Datos::escribir(fs, "recordar", &texto, now);
            if ok {
                self.escrito_r = texto;
            }
            self.r_en = (!ok).then_some(ms + REINTENTO_MS);
        }
        if toca(self.c_en) {
            let recordar = &self.recordar;
            let texto = self.tarro.serializar(|d| es_fav(d) || recordar.contains(&sitio(d)));
            let ok = texto == self.escrito_c || Datos::escribir(fs, "cookies", &texto, now);
            if ok {
                self.escrito_c = texto;
            }
            self.c_en = (!ok).then_some(ms + REINTENTO_MS);
        }
    }
}
