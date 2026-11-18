// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Reloj: hora y fecha, cronómetro con vueltas y temporizador. Programa en modo usuario (ver
//! `main.rs`); la lógica vive aquí para probarla y fotografiarla en el PC.
#![cfg_attr(not(test), no_std)]
extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use nf_core::apps::{App, AppKind, Ctx, MouseEv};
use nf_core::font::{self, Align};
use nf_core::geom::Rect;
use nf_core::input::{Key, KeyEvent};
use nf_core::platform::{DateTime, Platform};
use nf_core::surface::{blend, darken, Surface};
use nf_core::theme::Theme;

const TAB_H: i32 = 44;
const INK: u32 = 0x1A1A1A;
const MUTED: u32 = 0x6B6B6B;
const LINE: u32 = 0xE0E0E0;
const BTN_H: i32 = 36;
const MAX_VUELTAS: usize = 999;
/// Cada cuánto cambia el color del temporizador acabado.
const PARPADEO_MS: u64 = 400;

const DIAS: [&str; 7] = ["domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado"];
const MESES: [&str; 12] = ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"];

// ---------------------------------------------------------------- lógica pura

/// Día de la semana (0 = domingo) de una fecha del calendario gregoriano.
fn dia_semana(y: i32, m: u32, d: u32) -> usize {
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let m = m.clamp(1, 12);
    let y = if m < 3 { y - 1 } else { y };
    ((y + y / 4 - y / 100 + y / 400 + T[m as usize - 1] + d as i32).rem_euclid(7)) as usize
}

/// "martes, 17 de noviembre de 2026".
fn fecha_larga(t: &DateTime) -> String {
    let (m, d) = ((t.month as usize).clamp(1, 12), (t.day as u32).max(1));
    format!("{}, {} de {} de {}", DIAS[dia_semana(t.year as i32, m as u32, d)], d, MESES[m - 1], t.year)
}

/// "mm:ss,cc" (con horas, "h:mm:ss,cc", a partir de la hora).
fn texto_crono(ms: u64) -> String {
    let (cs, s) = (ms / 10 % 100, ms / 1000);
    if s >= 3600 {
        format!("{}:{:02}:{:02},{:02}", s / 3600, s / 60 % 60, s % 60, cs)
    } else {
        format!("{:02}:{:02},{:02}", s / 60, s % 60, cs)
    }
}

/// "mm:ss" de un número de segundos.
fn texto_minseg(s: u64) -> String {
    format!("{:02}:{:02}", s / 60, s % 60)
}

#[derive(Default)]
struct Crono {
    /// Tiempo ya contado cuando está parado (o hasta el último inicio).
    acc: u64,
    inicio: Option<u64>,
    /// Tiempo total en cada vuelta, en orden.
    vueltas: Vec<u64>,
}

impl Crono {
    fn tiempo(&self, ahora: u64) -> u64 {
        self.acc + self.inicio.map_or(0, |i| ahora.saturating_sub(i))
    }
    fn marcha(&self) -> bool {
        self.inicio.is_some()
    }
    fn alternar(&mut self, ahora: u64) {
        match self.inicio.take() {
            Some(i) => self.acc += ahora.saturating_sub(i),
            None => self.inicio = Some(ahora),
        }
    }
    fn vuelta(&mut self, ahora: u64) {
        if self.marcha() && self.vueltas.len() < MAX_VUELTAS {
            self.vueltas.push(self.tiempo(ahora));
        }
    }
    fn reiniciar(&mut self) {
        *self = Crono::default();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fase {
    /// Sin empezar: se ajustan minutos y segundos.
    Parado,
    /// Cuenta atrás; acaba en `fin` (reloj del sistema, ms).
    Marcha { fin: u64 },
    /// En pausa con `resta` ms por delante.
    Pausa { resta: u64 },
    /// Ha llegado a cero.
    Acabado,
}

struct Temporizador {
    min: u32,
    seg: u32,
    fase: Fase,
}

impl Temporizador {
    fn total_ms(&self) -> u64 {
        (self.min as u64 * 60 + self.seg as u64) * 1000
    }
    /// ¿Ha llegado a cero? (también antes de que el tick lo anote).
    fn acabado(&self, ahora: u64) -> bool {
        match self.fase {
            Fase::Acabado => true,
            Fase::Marcha { fin } => ahora >= fin,
            _ => false,
        }
    }
    fn resta_ms(&self, ahora: u64) -> u64 {
        match self.fase {
            Fase::Parado => self.total_ms(),
            Fase::Marcha { fin } => fin.saturating_sub(ahora),
            Fase::Pausa { resta } => resta,
            Fase::Acabado => 0,
        }
    }
    fn alternar(&mut self, ahora: u64) {
        self.fase = match self.fase {
            Fase::Parado if self.total_ms() > 0 => Fase::Marcha { fin: ahora + self.total_ms() },
            Fase::Marcha { .. } => Fase::Pausa { resta: self.resta_ms(ahora) },
            Fase::Pausa { resta } => Fase::Marcha { fin: ahora + resta },
            f => f,
        };
    }
    /// Pasa a `Acabado` si toca. `true` si acaba de cambiar.
    fn actualizar(&mut self, ahora: u64) -> bool {
        if matches!(self.fase, Fase::Marcha { .. }) && self.acabado(ahora) {
            self.fase = Fase::Acabado;
            return true;
        }
        false
    }
    fn reiniciar(&mut self) {
        self.fase = Fase::Parado;
    }
    /// Cambia minutos (`dm`) o segundos (`ds`) mientras está parado.
    fn ajustar(&mut self, dm: i32, ds: i32) {
        if self.fase == Fase::Parado {
            self.min = (self.min as i32 + dm).clamp(0, 99) as u32;
            self.seg = (self.seg as i32 + ds).clamp(0, 59) as u32;
        }
    }
}

/// ¿En qué mitad del parpadeo estamos?
fn fase_parpadeo(ahora: u64) -> bool {
    ahora / PARPADEO_MS % 2 == 0
}

// ---------------------------------------------------------------- la app

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pestana {
    Reloj,
    Cronometro,
    Temporizador,
}

const PESTANAS: [(Pestana, &str); 3] = [(Pestana::Reloj, "Reloj"), (Pestana::Cronometro, "Cronómetro"), (Pestana::Temporizador, "Temporizador")];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Btn {
    Tab(Pestana),
    /// Iniciar / Pausar.
    Marcha,
    Vuelta,
    Reiniciar,
    MinMenos,
    MinMas,
    SegMenos,
    SegMas,
}

struct Reloj {
    pestana: Pestana,
    crono: Crono,
    temp: Temporizador,
    size: (i32, i32),
    hover: Option<Btn>,
    abajo: Option<Btn>,
    /// Lo último dibujado (para pedir redibujo solo si cambia algo visible).
    clave: (u64, u64, u64),
}

/// App lista para `nf_app::programa!`.
pub fn crear() -> Box<dyn App> {
    Box::new(Reloj::new())
}

impl Reloj {
    pub fn new() -> Self {
        Reloj { pestana: Pestana::Reloj, crono: Crono::default(), temp: Temporizador { min: 5, seg: 0, fase: Fase::Parado }, size: (0, 0), hover: None, abajo: None, clave: (0, 0, 0) }
    }

    /// Botones de la pestaña actual (y las pestañas), con su zona y si se pueden pulsar.
    fn botones(&self) -> Vec<(Btn, Rect, bool)> {
        let (w, _) = self.size;
        let mut v = Vec::new();
        for (i, (p, _)) in PESTANAS.iter().enumerate() {
            let (x0, x1) = (i as i32 * w / 3, (i as i32 + 1) * w / 3);
            v.push((Btn::Tab(*p), Rect::new(x0, 0, x1 - x0, TAB_H), true));
        }
        let y0 = TAB_H;
        // Fila de n botones centrada, de ancho `bw` (se estrechan si la ventana es pequeña).
        let fila = |y: i32, n: i32| -> Vec<Rect> {
            let bw = ((w - 16 - (n - 1) * 10) / n).clamp(40, 120);
            let x0 = (w - n * bw - (n - 1) * 10) / 2;
            (0..n).map(|i| Rect::new(x0 + i * (bw + 10), y, bw, BTN_H)).collect()
        };
        match self.pestana {
            Pestana::Reloj => {}
            Pestana::Cronometro => {
                let r = fila(y0 + 112, 3);
                v.push((Btn::Marcha, r[0], true));
                v.push((Btn::Vuelta, r[1], self.crono.marcha()));
                v.push((Btn::Reiniciar, r[2], self.crono.acc > 0 || self.crono.marcha()));
            }
            Pestana::Temporizador => {
                let parado = self.temp.fase == Fase::Parado;
                let gw = ((w - 16 - 20) / 2).clamp(120, 170);
                for (g, (menos, mas)) in [(Btn::MinMenos, Btn::MinMas), (Btn::SegMenos, Btn::SegMas)].into_iter().enumerate() {
                    let x = if g == 0 { w / 2 - gw - 10 } else { w / 2 + 10 };
                    let y = y0 + 148;
                    v.push((menos, Rect::new(x, y, 40, 40), parado));
                    v.push((mas, Rect::new(x + gw - 40, y, 40, 40), parado));
                }
                let r = fila(y0 + 212, 2);
                v.push((Btn::Marcha, r[0], self.temp.fase != Fase::Acabado && (self.temp.fase != Fase::Parado || self.temp.total_ms() > 0)));
                v.push((Btn::Reiniciar, r[1], self.temp.fase != Fase::Parado));
            }
        }
        v
    }

    fn pulsar(&mut self, b: Btn, ahora: u64) {
        match b {
            Btn::Tab(p) => self.pestana = p,
            Btn::Marcha => match self.pestana {
                Pestana::Cronometro => self.crono.alternar(ahora),
                _ => self.temp.alternar(ahora),
            },
            Btn::Vuelta => self.crono.vuelta(ahora),
            Btn::Reiniciar => match self.pestana {
                Pestana::Cronometro => self.crono.reiniciar(),
                _ => self.temp.reiniciar(),
            },
            Btn::MinMenos => self.temp.ajustar(-1, 0),
            Btn::MinMas => self.temp.ajustar(1, 0),
            Btn::SegMenos => self.temp.ajustar(0, -1),
            Btn::SegMas => self.temp.ajustar(0, 1),
        }
    }

    fn en(&self, x: i32, y: i32) -> Option<Btn> {
        self.botones().into_iter().find(|(_, r, on)| *on && r.contains(x, y)).map(|(b, _, _)| b)
    }

    /// Resumen de lo que se ve; si cambia entre ticks, hay que redibujar.
    fn clave(&self, ahora: u64, t: &DateTime) -> (u64, u64, u64) {
        let hora = t.hour as u64 * 3600 + t.minute as u64 * 60 + t.second as u64;
        match self.pestana {
            Pestana::Reloj => (0, hora, 0),
            Pestana::Cronometro => (1, self.crono.tiempo(ahora) / 10, 0),
            Pestana::Temporizador => (2, self.temp.resta_ms(ahora).div_ceil(1000), (self.temp.acabado(ahora) && fase_parpadeo(ahora)) as u64 + 2 * self.temp.acabado(ahora) as u64),
        }
    }
}

impl App for Reloj {
    fn kind(&self) -> AppKind {
        AppKind::Program
    }
    fn title(&self) -> String {
        if self.temp.fase == Fase::Acabado {
            String::from("¡Tiempo! - Reloj")
        } else {
            String::from("Reloj")
        }
    }
    fn min_size(&self) -> (i32, i32) {
        (360, 300)
    }

    fn draw(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, _focused: bool) {
        self.size = (s.w, s.h);
        let ahora = sys.uptime_ms();
        let fecha = sys.now();
        // Temporizador acabado: la ventana entera alterna entre blanco y un tono del color de énfasis.
        let fin = self.temp.acabado(ahora);
        let fondo = if fin && fase_parpadeo(ahora) { blend(0xFFFFFF, theme.accent, 70) } else { 0xFFFFFF };
        s.fill_rect(s.bounds(), fondo);
        // Pestañas.
        let botones = self.botones();
        for (b, r, _) in botones.iter().filter(|(b, ..)| matches!(b, Btn::Tab(_))) {
            let Btn::Tab(p) = b else { continue };
            let activa = *p == self.pestana;
            if self.hover == Some(*b) && !activa {
                s.fill_rect(*r, 0xEDEDED);
            }
            let t = PESTANAS.iter().find(|x| x.0 == *p).map_or("", |x| x.1);
            font::draw_in(s, *r, t, 10, activa, if activa { INK } else { MUTED }, Align::Center);
            if activa {
                s.fill_rect(Rect::new(r.x + 12, r.bottom() - 3, r.w - 24, 3), theme.accent);
            }
        }
        s.fill_rect(Rect::new(0, TAB_H - 1, s.w, 1), LINE);
        let y0 = TAB_H;
        let pantalla = Rect::new(12, y0 + 12, s.w - 24, 84);
        match self.pestana {
            Pestana::Reloj => {
                let hora = format!("{:02}:{:02}:{:02}", fecha.hour, fecha.minute, fecha.second);
                let zona = Rect::new(12, y0 + 20, s.w - 24, (s.h - y0 - 40).max(60));
                let alto = zona.h * 55 / 100;
                gran(s, Rect::new(zona.x, zona.y + zona.h / 2 - alto - 4, zona.w, alto), &hora, INK);
                font::draw_in(s, Rect::new(zona.x, zona.y + zona.h / 2 + 4, zona.w, 28), &fecha_larga(&fecha), 14, false, MUTED, Align::Center);
            }
            Pestana::Cronometro => {
                gran(s, pantalla, &texto_crono(self.crono.tiempo(ahora)), INK);
                self.dibuja_botones(s, theme, &botones, ahora);
                self.dibuja_vueltas(s, y0 + 168);
            }
            Pestana::Temporizador => {
                let texto = if fin { String::from("¡Tiempo!") } else { texto_minseg(self.temp.resta_ms(ahora).div_ceil(1000)) };
                gran(s, pantalla, &texto, if fin { theme.accent } else { INK });
                // Barra de lo que queda.
                let barra = Rect::new(w_centro(s.w, 260), y0 + 100, 260.min(s.w - 24).max(0), 4);
                s.fill_rect(barra, LINE);
                let total = self.temp.total_ms().max(1);
                let queda = if self.temp.fase == Fase::Parado { total } else { self.temp.resta_ms(ahora).min(total) };
                s.fill_rect(Rect::new(barra.x, barra.y, (barra.w as u64 * queda / total) as i32, barra.h), theme.accent);
                // Selectores de minutos y segundos.
                let gw = ((s.w - 16 - 20) / 2).clamp(120, 170);
                for (g, (etq, val)) in [("Minutos", self.temp.min), ("Segundos", self.temp.seg)].into_iter().enumerate() {
                    let x = if g == 0 { s.w / 2 - gw - 10 } else { s.w / 2 + 10 };
                    font::draw_in(s, Rect::new(x, y0 + 124, gw, 20), etq, 9, false, MUTED, Align::Center);
                    font::draw_in(s, Rect::new(x + 40, y0 + 148, gw - 80, 40), &format!("{val:02}"), 20, true, INK, Align::Center);
                }
                self.dibuja_botones(s, theme, &botones, ahora);
            }
        }
    }

    fn key(&mut self, ev: &KeyEvent, cx: &mut Ctx) {
        let ahora = cx.now_ms;
        let tab = |p: Pestana| PESTANAS.iter().position(|x| x.0 == p).unwrap_or(0);
        match ev.key {
            Key::Left => self.pestana = PESTANAS[(tab(self.pestana) + 2) % 3].0,
            Key::Right => self.pestana = PESTANAS[(tab(self.pestana) + 1) % 3].0,
            Key::Char(' ') | Key::Enter if self.pestana != Pestana::Reloj => {
                if self.botones().iter().any(|(b, _, on)| *b == Btn::Marcha && *on) {
                    self.pulsar(Btn::Marcha, ahora);
                }
            }
            Key::Char('v') if self.pestana == Pestana::Cronometro => self.pulsar(Btn::Vuelta, ahora),
            _ => return,
        }
        cx.redraw = true;
    }

    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
        let (hover, abajo) = match ev {
            MouseEv::Move { x, y } => (self.en(x, y), self.abajo),
            MouseEv::Down { x, y } => {
                let b = self.en(x, y);
                (b, b)
            }
            MouseEv::Up { x, y } => {
                let b = self.en(x, y);
                if let (Some(b), true) = (b, b == self.abajo) {
                    self.pulsar(b, cx.now_ms);
                    cx.redraw = true;
                }
                (b, None)
            }
            MouseEv::Leave => (None, None),
            MouseEv::Wheel { .. } | MouseEv::RightDown { .. } => return,
        };
        if (hover, abajo) != (self.hover, self.abajo) {
            cx.redraw = true;
        }
        self.hover = hover;
        self.abajo = abajo;
    }

    fn tick(&mut self, cx: &mut Ctx) {
        let ahora = cx.now_ms;
        if self.temp.actualizar(ahora) {
            cx.redraw = true;
        }
        let k = self.clave(ahora, &cx.sys.now());
        if k != self.clave {
            self.clave = k;
            cx.redraw = true;
        }
    }
}

impl Reloj {
    fn dibuja_botones(&self, s: &mut Surface, theme: &Theme, botones: &[(Btn, Rect, bool)], ahora: u64) {
        for (b, r, on) in botones.iter().filter(|(b, ..)| !matches!(b, Btn::Tab(_))) {
            let texto = match b {
                Btn::Marcha if self.pestana == Pestana::Cronometro => if self.crono.marcha() { "Pausar" } else { "Iniciar" },
                Btn::Marcha => if matches!(self.temp.fase, Fase::Marcha { .. }) && !self.temp.acabado(ahora) { "Pausar" } else { "Iniciar" },
                Btn::Vuelta => "Vuelta",
                Btn::Reiniciar => "Restablecer",
                Btn::MinMenos | Btn::SegMenos => "−",
                Btn::MinMas | Btn::SegMas => "+",
                Btn::Tab(_) => continue,
            };
            let principal = *b == Btn::Marcha;
            let base = if principal { theme.accent } else { 0xE6E6E6 };
            let (fondo, tinta) = match (*on, self.hover == Some(*b)) {
                (false, _) => (0xF0F0F0, 0xA8A8A8),
                (true, true) if self.abajo == Some(*b) => (darken(base, 40), if principal { 0xFFFFFF } else { INK }),
                (true, true) => (darken(base, 18), if principal { 0xFFFFFF } else { INK }),
                (true, false) => (base, if principal { 0xFFFFFF } else { INK }),
            };
            s.fill_rect(*r, fondo);
            let (tam, negrita) = if r.w == 40 { (14, true) } else { (10, false) };
            font::draw_in(s, *r, texto, tam, negrita, tinta, Align::Center);
        }
    }

    /// Lista de vueltas (la más reciente arriba): número, tiempo de la vuelta y tiempo total.
    fn dibuja_vueltas(&self, s: &mut Surface, y: i32) {
        let col = Rect::new(w_centro(s.w, 360), y, 360.min(s.w - 24).max(0), (s.h - y - 8).max(0));
        if col.h < 26 || self.crono.vueltas.is_empty() {
            return;
        }
        font::draw_in(s, Rect::new(col.x, y, col.w, 22), "Vuelta", 9, false, MUTED, Align::Left);
        font::draw_in(s, Rect::new(col.x, y, col.w * 2 / 3, 22), "Tiempo", 9, false, MUTED, Align::Right);
        font::draw_in(s, Rect::new(col.x, y, col.w, 22), "Total", 9, false, MUTED, Align::Right);
        s.fill_rect(Rect::new(col.x, y + 23, col.w, 1), LINE);
        let filas = ((col.h - 26) / 24) as usize;
        let n = self.crono.vueltas.len();
        for (i, total) in self.crono.vueltas.iter().enumerate().rev().take(filas) {
            let r = Rect::new(col.x, y + 26 + (n - 1 - i) as i32 * 24, col.w, 24);
            let parcial = total - if i == 0 { 0 } else { self.crono.vueltas[i - 1] };
            font::draw_in(s, r, &format!("{}", i + 1), 10, false, INK, Align::Left);
            font::draw_in(s, Rect::new(r.x, r.y, r.w * 2 / 3, r.h), &texto_crono(parcial), 10, false, INK, Align::Right);
            font::draw_in(s, r, &texto_crono(*total), 10, false, INK, Align::Right);
        }
    }
}

fn w_centro(w: i32, ancho: i32) -> i32 {
    (w - ancho.min(w - 24).max(0)) / 2
}

/// Texto grande y en negrita, del mayor tamaño que quepa en `r`.
fn gran(s: &mut Surface, r: Rect, texto: &str, color: u32) {
    let tam = [72u32, 60, 48, 40, 32, 26, 20, 16, 12].into_iter().find(|&z| font::width(texto, z, true) <= r.w && font::line_h(z) <= r.h).unwrap_or(12);
    font::draw_in(s, r, texto, tam, true, color, Align::Center);
}

#[cfg(test)]
mod tests {
    use super::*;
    use nf_core::platform::TestPlatform;

    #[test]
    fn fechas_en_espanol() {
        let t = |y, m, d| DateTime { year: y, month: m, day: d, ..Default::default() };
        assert_eq!(fecha_larga(&t(2026, 11, 17)), "martes, 17 de noviembre de 2026");
        assert_eq!(fecha_larga(&t(2000, 1, 1)), "sábado, 1 de enero de 2000");
        assert_eq!(fecha_larga(&t(2024, 2, 29)), "jueves, 29 de febrero de 2024");
        assert_eq!(fecha_larga(&t(2026, 12, 31)), "jueves, 31 de diciembre de 2026");
        // Una fecha sin rellenar no rompe nada.
        assert!(fecha_larga(&DateTime::default()).contains("enero"));
    }

    #[test]
    fn formatos_de_tiempo() {
        assert_eq!(texto_crono(0), "00:00,00");
        assert_eq!(texto_crono(61_230), "01:01,23");
        assert_eq!(texto_crono(3_600_000 + 5_000 + 70), "1:00:05,07");
        assert_eq!(texto_minseg(125), "02:05");
        assert_eq!(texto_minseg(99 * 60 + 59), "99:59");
    }

    #[test]
    fn cronometro() {
        let mut c = Crono::default();
        c.alternar(1000);
        assert_eq!(c.tiempo(1500), 500);
        c.vuelta(1500);
        c.vuelta(2000);
        c.alternar(2500);
        assert_eq!(c.tiempo(9999), 1500, "parado no cuenta");
        c.vuelta(3000);
        assert_eq!(c.vueltas, [500, 1000], "sin marcha no hay vueltas");
        c.alternar(4000);
        assert_eq!(c.tiempo(4100), 1600, "sigue donde estaba");
        c.reiniciar();
        assert_eq!((c.tiempo(5000), c.vueltas.len(), c.marcha()), (0, 0, false));
    }

    #[test]
    fn temporizador() {
        let mut t = Temporizador { min: 0, seg: 0, fase: Fase::Parado };
        t.alternar(0);
        assert_eq!(t.fase, Fase::Parado, "a cero no arranca");
        t.ajustar(1, 30);
        assert_eq!(t.total_ms(), 90_000);
        t.ajustar(-5, 100);
        assert_eq!((t.min, t.seg), (0, 59));
        t.ajustar(2, -59);
        t.alternar(1000);
        assert_eq!(t.fase, Fase::Marcha { fin: 121_000 });
        t.ajustar(1, 1);
        assert_eq!((t.min, t.seg), (2, 0), "en marcha no se ajusta");
        assert!(!t.actualizar(120_999));
        t.alternar(61_000);
        assert_eq!(t.fase, Fase::Pausa { resta: 60_000 });
        t.alternar(100_000);
        assert_eq!(t.resta_ms(101_000), 59_000);
        assert!(t.actualizar(160_000) && t.fase == Fase::Acabado);
        assert!(!t.actualizar(170_000), "solo avisa una vez");
        t.alternar(200_000);
        assert_eq!(t.fase, Fase::Acabado, "acabado solo se quita con Restablecer");
        t.reiniciar();
        assert_eq!((t.fase, t.resta_ms(0)), (Fase::Parado, 120_000));
        assert_ne!(fase_parpadeo(0), fase_parpadeo(PARPADEO_MS));
    }

    fn con<R>(f: impl FnOnce(&mut Ctx) -> R, ms: u64) -> R {
        let mut sys = TestPlatform::default();
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut sys, &mut th, ms);
        f(&mut cx)
    }

    fn clic(app: &mut Reloj, b: Btn, ms: u64) {
        let r = app.botones().into_iter().find(|x| x.0 == b).unwrap().1;
        let (x, y) = (r.x + r.w / 2, r.y + r.h / 2);
        con(
            |cx| {
                app.mouse(MouseEv::Down { x, y }, cx);
                app.mouse(MouseEv::Up { x, y }, cx);
            },
            ms,
        );
    }

    fn dibuja(app: &mut Reloj, w: i32, h: i32, ms: u64) -> Surface {
        let mut sys = TestPlatform { ms, ..Default::default() };
        let mut s = Surface::new(w, h, 0);
        app.draw(&mut s, &Theme::default(), &mut sys, true);
        s
    }

    #[test]
    fn pestanas_botones_y_acabado() {
        let mut app = Reloj::new();
        dibuja(&mut app, 420, 360, 0);
        clic(&mut app, Btn::Tab(Pestana::Cronometro), 0);
        assert_eq!(app.pestana, Pestana::Cronometro);
        dibuja(&mut app, 420, 360, 0);
        clic(&mut app, Btn::Marcha, 1000);
        clic(&mut app, Btn::Vuelta, 1500);
        assert_eq!((app.crono.marcha(), app.crono.vueltas.clone()), (true, vec![500]));
        // Los botones desactivados no responden: Vuelta con el cronómetro parado.
        clic(&mut app, Btn::Marcha, 2000);
        let (x, y) = {
            let r = app.botones().into_iter().find(|b| b.0 == Btn::Vuelta).unwrap().1;
            (r.x + 3, r.y + 3)
        };
        assert!(app.en(x, y).is_none());
        // Temporizador: ajustar, iniciar y que acabe parpadeando.
        clic(&mut app, Btn::Tab(Pestana::Temporizador), 0);
        dibuja(&mut app, 420, 360, 0);
        clic(&mut app, Btn::SegMas, 0);
        clic(&mut app, Btn::MinMenos, 0);
        assert_eq!((app.temp.min, app.temp.seg), (4, 1));
        clic(&mut app, Btn::Marcha, 10_000);
        assert!(matches!(app.temp.fase, Fase::Marcha { fin: 251_000 }));
        // El tick pide redibujar al cambiar el segundo y al acabar; el título avisa.
        con(|cx| app.tick(cx), 10_000);
        let redibuja = con(
            |cx| {
                app.tick(cx);
                cx.redraw
            },
            10_000,
        );
        assert!(!redibuja, "mismo segundo: nada nuevo");
        assert!(con(|cx| { app.tick(cx); cx.redraw }, 11_000));
        con(|cx| app.tick(cx), 251_000);
        assert_eq!(app.title(), "¡Tiempo! - Reloj");
        let (a, b) = (dibuja(&mut app, 420, 360, 251_000), dibuja(&mut app, 420, 360, 251_000 + PARPADEO_MS));
        assert_ne!(a.px[5 * 420 + 300], b.px[5 * 420 + 300], "el fondo parpadea");
        clic(&mut app, Btn::Reiniciar, 252_000);
        assert_eq!((app.temp.fase, app.title().as_str()), (Fase::Parado, "Reloj"));
    }

    #[test]
    fn teclado_y_tamanos_raros() {
        let mut app = Reloj::new();
        let tecla = |k, ch| KeyEvent { key: k, ch, down: true, shift: false, ctrl: false, alt: false };
        con(
            |cx| {
                app.key(&tecla(Key::Right, None), cx);
                assert_eq!(app.pestana, Pestana::Cronometro);
                app.key(&tecla(Key::Char(' '), Some(' ')), cx);
                assert!(app.crono.marcha());
                app.key(&tecla(Key::Left, None), cx);
                app.key(&tecla(Key::Left, None), cx);
                assert_eq!(app.pestana, Pestana::Temporizador, "da la vuelta");
            },
            100,
        );
        for p in [Pestana::Reloj, Pestana::Cronometro, Pestana::Temporizador] {
            app.pestana = p;
            for (w, h) in [(360, 300), (1, 1), (0, 0), (40, 600), (1000, 100)] {
                dibuja(&mut app, w, h, 3_723_450);
            }
        }
        // Con muchas vueltas y poco alto solo se dibujan las que caben.
        app.pestana = Pestana::Cronometro;
        app.crono.inicio = Some(0);
        for i in 0..50 {
            app.crono.vuelta(i * 100);
        }
        dibuja(&mut app, 420, 360, 10_000);
    }
}
