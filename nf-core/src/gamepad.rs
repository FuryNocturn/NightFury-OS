// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Mandos (v0.11): estado común de cualquier mando y los controladores de cada familia, sin hardware
//! (el núcleo les pasa los informes USB y manda lo que piden). Se prueban con informes de ejemplo.
//!
//! CONTRATO: los tipos públicos, las constantes y las firmas de `detect` y de `Pad` no cambian.
//!
//! Recorrido:
//! - El núcleo (`kernel/src/usb`) llama a `detect` con cada interfaz. Si da `NeedsDescriptor` lee el
//!   descriptor de informes HID y llama a `Pad::from_descriptor`.
//! - Al configurar el dispositivo manda por el endpoint de interrupción OUT (o por SET_REPORT si no lo
//!   hay) lo que devuelve `Pad::start`; con cada informe de entrada llama a `Pad::feed` y cada ~10 ms
//!   a `Pad::tick`, y manda lo que devuelvan.
//! - Cada cambio sale como `PadEvent` hacia el escritorio (`Desktop::gamepad`), que entrega el
//!   estado a la ventana activa (`EV_GAMEPAD`) y la lista de mandos a todas (`EV_GAMEPADS`).
//!
//! Clean room: los formatos salen de las especificaciones HID 1.11 y de documentación pública de los
//! informes de cada mando, no de código de otros sistemas.
use crate::wire::{Bad, Reader, Wire, WireResult, Writer};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// Botones por POSICIÓN, no por letra: el de abajo del rombo es `BTN_SOUTH` en todos los mandos
// (A en Xbox, Cruz en PlayStation, B en Nintendo).
pub const BTN_SOUTH: u32 = 1 << 0;
pub const BTN_EAST: u32 = 1 << 1;
pub const BTN_WEST: u32 = 1 << 2;
pub const BTN_NORTH: u32 = 1 << 3;
/// LB / L1 / L.
pub const BTN_L1: u32 = 1 << 4;
/// RB / R1 / R.
pub const BTN_R1: u32 = 1 << 5;
/// Gatillo izquierdo pulsado (LT / L2 / ZL); el recorrido va en `AXIS_LT` si el mando lo tiene.
pub const BTN_L2: u32 = 1 << 6;
pub const BTN_R2: u32 = 1 << 7;
/// View / Share / Create / − .
pub const BTN_SELECT: u32 = 1 << 8;
/// Menu / Options / + .
pub const BTN_START: u32 = 1 << 9;
/// Xbox / PS / Home.
pub const BTN_HOME: u32 = 1 << 10;
/// Pulsar el stick izquierdo.
pub const BTN_L3: u32 = 1 << 11;
pub const BTN_R3: u32 = 1 << 12;
pub const BTN_UP: u32 = 1 << 13;
pub const BTN_DOWN: u32 = 1 << 14;
pub const BTN_LEFT: u32 = 1 << 15;
pub const BTN_RIGHT: u32 = 1 << 16;
/// Panel táctil (PlayStation) / Captura (Nintendo) / Compartir (Xbox Series).
pub const BTN_EXTRA: u32 = 1 << 17;

/// Índices de `PadState::axes`. Sticks: -32768..=32767, con X negativa a la izquierda e Y negativa
/// ARRIBA. Gatillos: 0 (suelto) ..= 32767 (a fondo); los mandos con gatillos digitales dan 0 o 32767.
pub const AXIS_LX: usize = 0;
pub const AXIS_LY: usize = 1;
pub const AXIS_RX: usize = 2;
pub const AXIS_RY: usize = 3;
pub const AXIS_LT: usize = 4;
pub const AXIS_RT: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PadState {
    /// `BTN_*`.
    pub buttons: u32,
    /// `AXIS_*`.
    pub axes: [i16; 6],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PadKind {
    /// HID con descriptor de informes de joystick o gamepad.
    Generic,
    DualShock4,
    DualSense,
    SwitchPro,
    Xbox360,
    XboxOne,
}

/// Un mando conectado, como lo ven los programas.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PadInfo {
    /// Único mientras el sistema esté encendido (no se reutiliza al desconectar).
    pub id: u32,
    pub kind: PadKind,
    /// Nombre del producto (descriptor USB) o, si no lo da, `Pad::default_name`.
    pub name: String,
}

/// Lo que el núcleo comunica al escritorio.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PadEvent {
    Connected(PadInfo),
    Removed(u32),
    /// Estado nuevo de un mando (solo cuando cambia).
    State(u32, PadState),
}

/// Lo que el núcleo sabe de una interfaz USB antes de reclamarla.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PadIface {
    pub vid: u16,
    pub pid: u16,
    pub class: u8,
    pub sub: u8,
    pub proto: u8,
}

pub enum Detect {
    No,
    Pad(Pad),
    /// HID que puede ser un mando genérico: leer su descriptor de informes y `Pad::from_descriptor`.
    NeedsDescriptor,
}

/// ¿Esta interfaz es un mando? Por VID/PID (Sony, Nintendo) o por clase (Xbox: 0xFF/0x5D/0x01 y
/// 0xFF/0x47/0xD0). Un HID (clase 3) que no sea de arranque ni conocido da `NeedsDescriptor`; el
/// teclado y el ratón de arranque (3/1/1 y 3/1/2) dan `No`.
pub fn detect(i: &PadIface) -> Detect {
    let kind = match (i.vid, i.pid, i.class, i.sub, i.proto) {
        (0x054C, 0x05C4 | 0x09CC, ..) => PadKind::DualShock4,
        (0x054C, 0x0CE6 | 0x0DF2, ..) => PadKind::DualSense,
        (0x057E, 0x2009, ..) => PadKind::SwitchPro,
        (_, _, 0xFF, 0x5D, 0x01) => PadKind::Xbox360,
        (_, _, 0xFF, 0x47, 0xD0) => PadKind::XboxOne,
        (_, _, 3, 1, 1 | 2) => return Detect::No,
        (_, _, 3, ..) => return Detect::NeedsDescriptor,
        _ => return Detect::No,
    };
    Detect::Pad(Pad::new(kind, None))
}

// ---------------------------------------------------------------------------------------------
// Utilidades de normalización

/// Más que esto en un gatillo analógico cuenta como pulsado (≈25 %).
const UMBRAL: i16 = 8192;

/// Hat de 8 direcciones (0 = arriba, en sentido horario) a botones; fuera de rango = centro.
fn hat_btns(h: u8) -> u32 {
    match h {
        0 => BTN_UP,
        1 => BTN_UP | BTN_RIGHT,
        2 => BTN_RIGHT,
        3 => BTN_RIGHT | BTN_DOWN,
        4 => BTN_DOWN,
        5 => BTN_DOWN | BTN_LEFT,
        6 => BTN_LEFT,
        7 => BTN_LEFT | BTN_UP,
        _ => 0,
    }
}

/// Junta los `BTN_*` de los bits de `v` según la tabla (bit, botón).
fn map(v: u32, table: &[(u32, u32)]) -> u32 {
    table.iter().filter(|(m, _)| v & m != 0).fold(0, |a, (_, b)| a | b)
}

/// Stick de un byte (0..=255, centro ~128) a -32768..=32767.
fn stick8(v: u8) -> i16 {
    (v as i32 * 257 - 32768) as i16
}

fn trig8(v: u8) -> i16 {
    (v as i32 * 32767 / 255) as i16
}

fn le16(r: &[u8], i: usize) -> i16 {
    i16::from_le_bytes([r[i], r[i + 1]])
}

fn trig_btns(lt: i16, rt: i16) -> u32 {
    (if lt > UMBRAL { BTN_L2 } else { 0 }) | (if rt > UMBRAL { BTN_R2 } else { 0 })
}

// ---------------------------------------------------------------------------------------------
// Informes de cada familia (todos devuelven `None` si el informe no es el de estado)

/// DualShock 4, informe USB 0x01.
fn ds4(r: &[u8]) -> Option<PadState> {
    if r.len() < 10 || r[0] != 1 {
        return None;
    }
    let buttons = hat_btns(r[5] & 15)
        | map(r[5] as u32, &[(0x10, BTN_WEST), (0x20, BTN_SOUTH), (0x40, BTN_EAST), (0x80, BTN_NORTH)])
        | map(r[6] as u32, &[(1, BTN_L1), (2, BTN_R1), (4, BTN_L2), (8, BTN_R2), (0x10, BTN_SELECT), (0x20, BTN_START), (0x40, BTN_L3), (0x80, BTN_R3)])
        | map(r[7] as u32, &[(1, BTN_HOME), (2, BTN_EXTRA)]);
    Some(PadState { buttons, axes: [stick8(r[1]), stick8(r[2]), stick8(r[3]), stick8(r[4]), trig8(r[8]), trig8(r[9])] })
}

/// DualSense (y Edge), informe USB 0x01.
fn dualsense(r: &[u8]) -> Option<PadState> {
    if r.len() < 11 || r[0] != 1 {
        return None;
    }
    let buttons = hat_btns(r[8] & 15)
        | map(r[8] as u32, &[(0x10, BTN_WEST), (0x20, BTN_SOUTH), (0x40, BTN_EAST), (0x80, BTN_NORTH)])
        | map(r[9] as u32, &[(1, BTN_L1), (2, BTN_R1), (4, BTN_L2), (8, BTN_R2), (0x10, BTN_SELECT), (0x20, BTN_START), (0x40, BTN_L3), (0x80, BTN_R3)])
        | map(r[10] as u32, &[(1, BTN_HOME), (2, BTN_EXTRA)]);
    Some(PadState { buttons, axes: [stick8(r[1]), stick8(r[2]), stick8(r[3]), stick8(r[4]), trig8(r[5]), trig8(r[6])] })
}

/// Xbox 360 con cable: informe de 20 bytes de tipo 0x00. En el mando la Y va hacia arriba.
fn xbox360(r: &[u8]) -> Option<PadState> {
    if r.len() < 14 || r[0] != 0 {
        return None;
    }
    let mut buttons = map(r[2] as u32, &[(1, BTN_UP), (2, BTN_DOWN), (4, BTN_LEFT), (8, BTN_RIGHT), (0x10, BTN_START), (0x20, BTN_SELECT), (0x40, BTN_L3), (0x80, BTN_R3)])
        | map(r[3] as u32, &[(1, BTN_L1), (2, BTN_R1), (4, BTN_HOME), (0x10, BTN_SOUTH), (0x20, BTN_EAST), (0x40, BTN_WEST), (0x80, BTN_NORTH)]);
    let (lt, rt) = (trig8(r[4]), trig8(r[5]));
    buttons |= trig_btns(lt, rt);
    Some(PadState { buttons, axes: [le16(r, 6), le16(r, 8).saturating_neg(), le16(r, 10), le16(r, 12).saturating_neg(), lt, rt] })
}

/// Xbox One / Series (GIP), informe 0x20: cabecera de 4 bytes y 14 de carga.
fn xbox_one(r: &[u8]) -> Option<PadState> {
    if r.len() < 18 || r[0] != 0x20 {
        return None;
    }
    let p = &r[4..];
    let mut buttons = map(p[0] as u32, &[(4, BTN_START), (8, BTN_SELECT), (0x10, BTN_SOUTH), (0x20, BTN_EAST), (0x40, BTN_WEST), (0x80, BTN_NORTH)])
        | map(p[1] as u32, &[(1, BTN_UP), (2, BTN_DOWN), (4, BTN_LEFT), (8, BTN_RIGHT), (0x10, BTN_L1), (0x20, BTN_R1), (0x40, BTN_L3), (0x80, BTN_R3)]);
    // Gatillos de 10 bits.
    let trig = |i: usize| ((u16::from_le_bytes([p[i], p[i + 1]]) & 0x3FF) as i32 * 32767 / 1023) as i16;
    let (lt, rt) = (trig(2), trig(4));
    buttons |= trig_btns(lt, rt);
    Some(PadState { buttons, axes: [le16(p, 6), le16(p, 8).saturating_neg(), le16(p, 10), le16(p, 12).saturating_neg(), lt, rt] })
}

/// Pro Controller, informes 0x30 (y 0x21, que tienen la misma forma): sticks de 12 bits.
fn switch_pro(r: &[u8]) -> Option<PadState> {
    if r.len() < 12 || !matches!(r[0], 0x30 | 0x21) {
        return None;
    }
    // Botones por posición: B (abajo) = SOUTH, A (derecha) = EAST, Y (izquierda) = WEST, X (arriba) = NORTH.
    let buttons = map(r[3] as u32, &[(1, BTN_WEST), (2, BTN_NORTH), (4, BTN_SOUTH), (8, BTN_EAST), (0x40, BTN_R1), (0x80, BTN_R2)])
        | map(r[4] as u32, &[(1, BTN_SELECT), (2, BTN_START), (4, BTN_R3), (8, BTN_L3), (0x10, BTN_HOME), (0x20, BTN_EXTRA)])
        | map(r[5] as u32, &[(1, BTN_DOWN), (2, BTN_UP), (4, BTN_RIGHT), (8, BTN_LEFT), (0x40, BTN_L1), (0x80, BTN_L2)]);
    // shortcut: sin la calibración de fábrica (centro nominal 2048) puede quedar algo de deriva; leerla de la SPI del mando.
    let st = |v: u32, inv: bool| {
        let d = if inv { 2048 - v as i32 } else { v as i32 - 2048 };
        (d * 16).clamp(-32768, 32767) as i16
    };
    let x = |i: usize| r[i] as u32 | (r[i + 1] as u32 & 15) << 8;
    let y = |i: usize| (r[i + 1] as u32 >> 4) | (r[i + 2] as u32) << 4;
    // ZL/ZR son digitales: ejes a 0 o 32767.
    let (lt, rt) = (if buttons & BTN_L2 != 0 { 32767 } else { 0 }, if buttons & BTN_R2 != 0 { 32767 } else { 0 });
    Some(PadState { buttons, axes: [st(x(6), false), st(y(6), true), st(x(9), false), st(y(9), true), lt, rt] })
}

// ---------------------------------------------------------------------------------------------
// Saludo del Pro Controller por USB

/// Códigos de los informes de salida 0x80: estado, handshake, velocidad, handshake, sin tiempo límite.
const SW_STEPS: [u8; 5] = [1, 2, 3, 2, 4];
const SW_RETRY_MS: u64 = 500;
const SW_TRIES: u8 = 10;

#[derive(Default)]
struct Switch {
    /// Índice en `SW_STEPS` del último paso mandado.
    phase: usize,
    ready: bool,
    sent_ms: u64,
    tries: u8,
    pkt: u8,
}

impl Switch {
    /// Informes del paso actual; el último añade el subcomando 0x03 que pide el informe 0x30.
    fn out(&mut self, now_ms: u64) -> Vec<Vec<u8>> {
        self.sent_ms = now_ms;
        let mut v = vec![vec![0x80, SW_STEPS[self.phase]]];
        if self.phase == SW_STEPS.len() - 1 {
            v.push(vec![0x01, self.pkt & 15, 0, 1, 0x40, 0x40, 0, 1, 0x40, 0x40, 0x03, 0x30]);
            self.pkt = self.pkt.wrapping_add(1);
        }
        v
    }
}

// ---------------------------------------------------------------------------------------------
// HID genérico: descriptor de informes (HID 1.11, 6.2.2)

#[derive(Clone, Copy, PartialEq, Debug)]
enum Slot {
    Axis(usize),
    Hat,
    /// Máscara `BTN_*`.
    Button(u32),
    // Ejes que se resuelven al final según cuáles tenga el mando.
    Z,
    Rz,
    Rx,
    Ry,
}

#[derive(Clone, Debug)]
struct Field {
    rid: u8,
    /// Posición del primer bit dentro del informe (contando el byte del ID si lo hay).
    bit: u32,
    size: u32,
    lmin: i32,
    lmax: i32,
    slot: Slot,
}

struct Generic {
    /// ¿Los informes empiezan con su ID?
    ids: bool,
    fields: Vec<Field>,
}

/// Orden de botones de `Button page` (1..) a `BTN_*`.
const BTN_ORDER: [u32; 14] = [BTN_SOUTH, BTN_EAST, BTN_WEST, BTN_NORTH, BTN_L1, BTN_R1, BTN_L2, BTN_R2, BTN_SELECT, BTN_START, BTN_L3, BTN_R3, BTN_HOME, BTN_EXTRA];

fn classify(page: u32, usage: u32) -> Option<Slot> {
    Some(match (page, usage) {
        (9, n @ 1..=14) => Slot::Button(BTN_ORDER[n as usize - 1]),
        (1, 0x30) => Slot::Axis(AXIS_LX),
        (1, 0x31) => Slot::Axis(AXIS_LY),
        (1, 0x32) => Slot::Z,
        (1, 0x33) => Slot::Rx,
        (1, 0x34) => Slot::Ry,
        (1, 0x35) => Slot::Rz,
        (1, 0x39) => Slot::Hat,
        (2, 0xC5) => Slot::Axis(AXIS_LT),
        (2, 0xC4) => Slot::Axis(AXIS_RT),
        _ => return None,
    })
}

#[derive(Clone, Default)]
struct Global {
    page: u32,
    lmin: i32,
    lmax: i32,
    size: u32,
    count: u32,
    rid: u8,
}

/// Una `Usage` con página extendida (4 bytes) lleva la página arriba; si no, la página actual.
fn full(page: u32, u: u32) -> (u32, u32) {
    if u > 0xFFFF { (u >> 16, u & 0xFFFF) } else { (page, u) }
}

impl Generic {
    fn parse(desc: &[u8]) -> Option<Generic> {
        let mut g = Global::default();
        let mut stack: Vec<Global> = Vec::new();
        let mut usages: Vec<u32> = Vec::new();
        let (mut umin, mut umax): (Option<u32>, Option<u32>) = (None, None);
        // Por colección abierta: ¿está dentro de una aplicación Joystick / Game Pad?
        let mut cols: Vec<bool> = Vec::new();
        let mut offs: Vec<(u8, u32)> = Vec::new();
        let mut fields: Vec<Field> = Vec::new();
        let (mut ids, mut pad_app) = (false, false);
        let mut i = 0;
        while i < desc.len() {
            let p = desc[i];
            if p == 0xFE {
                // Ítem largo: no define nada que nos sirva.
                i += 3 + desc.get(i + 1).copied().unwrap_or(0) as usize;
                continue;
            }
            let n = match p & 3 {
                3 => 4,
                s => s as usize,
            };
            let data = desc.get(i + 1..i + 1 + n)?;
            i += 1 + n;
            let u = data.iter().enumerate().fold(0u32, |a, (k, b)| a | (*b as u32) << (8 * k));
            let s = if n == 0 { 0 } else { ((u << (32 - 8 * n)) as i32) >> (32 - 8 * n) };
            match ((p >> 2) & 3, p >> 4) {
                // Globales.
                (1, 0) => g.page = u,
                (1, 1) => g.lmin = s,
                // Muchos mandos escriben 0xFF (-1 con signo) queriendo 255: si queda por debajo del mínimo, sin signo.
                (1, 2) => g.lmax = if s < g.lmin { u as i32 } else { s },
                (1, 7) => g.size = u,
                (1, 8) => {
                    g.rid = u as u8;
                    ids = true;
                }
                (1, 9) => g.count = u,
                (1, 10) => stack.push(g.clone()),
                (1, 11) => {
                    if let Some(x) = stack.pop() {
                        g = x;
                    }
                }
                // Locales.
                (2, 0) => {
                    if usages.len() < 256 {
                        usages.push(u)
                    }
                }
                (2, 1) => umin = Some(u),
                (2, 2) => umax = Some(u),
                // Principales.
                (0, t) => {
                    let inside = cols.last().copied().unwrap_or(false);
                    match t {
                        8 => {
                            let rid = g.rid;
                            let at = match offs.iter().position(|o| o.0 == rid) {
                                Some(k) => k,
                                None => {
                                    offs.push((rid, if rid != 0 { 8 } else { 0 }));
                                    offs.len() - 1
                                }
                            };
                            let off = offs[at].1;
                            // Variable (bit 1) y no constante (bit 0); los arrays no sirven para mandos.
                            if inside && u & 1 == 0 && u & 2 != 0 && g.size >= 1 && g.size <= 32 {
                                for k in 0..g.count.min(4096) {
                                    let us = if !usages.is_empty() {
                                        Some(usages[(k as usize).min(usages.len() - 1)])
                                    } else if let (Some(a), Some(b)) = (umin, umax) {
                                        Some((a + k).min(b))
                                    } else {
                                        None
                                    };
                                    let Some(us) = us else { continue };
                                    let (page, usage) = full(g.page, us);
                                    if let Some(slot) = classify(page, usage) {
                                        fields.push(Field { rid, bit: off + k * g.size, size: g.size, lmin: g.lmin, lmax: g.lmax, slot });
                                    }
                                }
                            }
                            offs[at].1 = off.saturating_add(g.size.saturating_mul(g.count));
                        }
                        0xA => {
                            let pad = u == 1 && cols.is_empty() && usages.first().is_some_and(|x| matches!(full(g.page, *x), (1, 4 | 5)));
                            pad_app |= pad;
                            cols.push(inside || pad);
                        }
                        0xC => {
                            cols.pop();
                        }
                        _ => {}
                    }
                    usages.clear();
                    umin = None;
                    umax = None;
                }
                _ => {}
            }
        }
        if !pad_app || fields.is_empty() {
            return None;
        }
        let has_z = fields.iter().any(|f| matches!(f.slot, Slot::Z | Slot::Rz));
        for f in fields.iter_mut() {
            f.slot = match f.slot {
                Slot::Z => Slot::Axis(AXIS_RX),
                Slot::Rz => Slot::Axis(AXIS_RY),
                Slot::Rx => Slot::Axis(if has_z { AXIS_LT } else { AXIS_RX }),
                Slot::Ry => Slot::Axis(if has_z { AXIS_RT } else { AXIS_RY }),
                s => s,
            };
        }
        Some(Generic { ids, fields })
    }

    /// Estado nuevo a partir de un informe; lo que no está en este informe se queda como estaba.
    fn read(&self, r: &[u8], last: PadState) -> Option<PadState> {
        let id = if self.ids { *r.first()? } else { 0 };
        let mine = || self.fields.iter().filter(|f| f.rid == id);
        // Un informe más corto de lo que dice el descriptor no se fía (no suelta botones a medias).
        if mine().any(|f| get_bits(r, f.bit, f.size).is_none()) {
            return None;
        }
        let mut st = last;
        let mut clear = 0;
        for f in mine() {
            clear |= match f.slot {
                Slot::Button(m) => m,
                Slot::Hat => BTN_UP | BTN_DOWN | BTN_LEFT | BTN_RIGHT,
                Slot::Axis(AXIS_LT) => BTN_L2,
                Slot::Axis(AXIS_RT) => BTN_R2,
                _ => 0,
            };
        }
        st.buttons &= !clear;
        for f in mine() {
            let Some(raw) = get_bits(r, f.bit, f.size) else { continue };
            let v = if f.lmin < 0 { ((raw << (32 - f.size)) as i32 >> (32 - f.size)) as i64 } else { raw as i64 };
            let span = f.lmax as i64 - f.lmin as i64;
            match f.slot {
                Slot::Button(m) => {
                    if v != 0 {
                        st.buttons |= m
                    }
                }
                Slot::Hat => {
                    // Fuera de [lmin, lmax] (el «sin pulsar» de 8) es centro; de 4 posiciones, N E S O.
                    let h = v - f.lmin as i64;
                    let h = if h < 0 || h > span { 8 } else if span == 3 { h * 2 } else { h };
                    st.buttons |= hat_btns(h as u8);
                }
                Slot::Axis(a) if span > 0 => {
                    let t = v.clamp(f.lmin as i64, f.lmax as i64) - f.lmin as i64;
                    st.axes[a] = if a >= AXIS_LT { t * 32767 / span } else { t * 65535 / span - 32768 } as i16;
                }
                _ => {}
            }
        }
        let has = |a| self.fields.iter().any(|f| f.slot == Slot::Axis(a));
        for (a, m) in [(AXIS_LT, BTN_L2), (AXIS_RT, BTN_R2)] {
            if !has(a) {
                st.axes[a] = if st.buttons & m != 0 { 32767 } else { 0 };
            } else if st.axes[a] > UMBRAL {
                st.buttons |= m;
            }
        }
        Some(st)
    }
}

/// `size` bits (hasta 32) desde el bit `off`, en el orden de los informes HID (bit 0 del byte 0 primero).
fn get_bits(r: &[u8], off: u32, size: u32) -> Option<u32> {
    if size == 0 || size > 32 || (off as usize + size as usize) > r.len() * 8 {
        return None;
    }
    let mut v = 0u64;
    for k in 0..size {
        let b = off + k;
        v |= (((r[(b / 8) as usize] >> (b % 8)) & 1) as u64) << k;
    }
    Some(v as u32)
}

// ---------------------------------------------------------------------------------------------

/// Controlador de un mando: traduce sus informes a `PadState` y lleva su saludo si lo necesita.
pub struct Pad {
    kind: PadKind,
    last: PadState,
    gen: Option<Generic>,
    sw: Switch,
    /// Botón Xbox de los mandos GIP: llega en un informe aparte.
    home: bool,
}

impl Pad {
    fn new(kind: PadKind, gen: Option<Generic>) -> Pad {
        Pad { kind, last: PadState::default(), gen, sw: Switch::default(), home: false }
    }

    /// Mando HID genérico a partir de su descriptor de informes. `None` si su colección de aplicación
    /// no es Joystick (Generic Desktop 0x04) ni Game Pad (0x05), o si no tiene botones ni ejes.
    pub fn from_descriptor(desc: &[u8]) -> Option<Pad> {
        Generic::parse(desc).map(|g| Pad::new(PadKind::Generic, Some(g)))
    }

    pub fn kind(&self) -> PadKind {
        self.kind
    }

    /// Nombre para cuando el dispositivo no da el suyo: «Mando USB», «Mando DualShock 4»…
    pub fn default_name(&self) -> &'static str {
        match self.kind {
            PadKind::Generic => "Mando USB",
            PadKind::DualShock4 => "Mando DualShock 4",
            PadKind::DualSense => "Mando DualSense",
            PadKind::SwitchPro => "Mando Switch Pro",
            PadKind::Xbox360 => "Mando Xbox 360",
            PadKind::XboxOne => "Mando Xbox",
        }
    }

    /// Informes de salida que hay que mandar nada más configurar el dispositivo (el saludo del Pro
    /// Controller, el encendido del de Xbox One). Vacío si no hace falta nada.
    pub fn start(&mut self, now_ms: u64) -> Vec<Vec<u8>> {
        match self.kind {
            PadKind::SwitchPro => {
                self.sw = Switch::default();
                self.sw.out(now_ms)
            }
            // GIP: orden 0x05 (encender) con carga de un byte.
            PadKind::XboxOne => vec![vec![0x05, 0x20, 0x00, 0x01, 0x00]],
            _ => Vec::new(),
        }
    }

    /// Un informe de entrada tal cual llega del endpoint (con su ID de informe si lo lleva).
    /// Devuelve el estado si ha cambiado y los informes de salida que tocan ahora.
    pub fn feed(&mut self, report: &[u8], now_ms: u64) -> (Option<PadState>, Vec<Vec<u8>>) {
        let mut out = Vec::new();
        let new = match self.kind {
            PadKind::Generic => self.gen.as_ref().and_then(|g| g.read(report, self.last)),
            PadKind::DualShock4 => ds4(report),
            PadKind::DualSense => dualsense(report),
            PadKind::Xbox360 => xbox360(report),
            PadKind::XboxOne => match report {
                // Botón Xbox: llega aparte; se mezcla con el último estado.
                [0x07, _, _, _, b, ..] => {
                    self.home = b & 1 != 0;
                    let mut s = self.last;
                    s.buttons = if self.home { s.buttons | BTN_HOME } else { s.buttons & !BTN_HOME };
                    Some(s)
                }
                _ => xbox_one(report).map(|mut s| {
                    if self.home {
                        s.buttons |= BTN_HOME;
                    }
                    s
                }),
            },
            PadKind::SwitchPro => {
                let sw = &mut self.sw;
                if report.len() >= 2 && report[0] == 0x81 && !sw.ready && sw.phase < SW_STEPS.len() - 1 && report[1] == SW_STEPS[sw.phase] {
                    sw.phase += 1;
                    sw.tries = 0;
                    out = sw.out(now_ms);
                }
                let s = switch_pro(report);
                if s.is_some() {
                    sw.ready = true;
                }
                s
            }
        };
        match new {
            Some(s) if s != self.last => {
                self.last = s;
                (Some(s), out)
            }
            _ => (None, out),
        }
    }

    /// Paso de tiempo: reintentos del saludo si el mando no contesta. Vacío si no toca nada.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Vec<u8>> {
        let sw = &mut self.sw;
        if self.kind == PadKind::SwitchPro && !sw.ready && sw.tries < SW_TRIES && now_ms.saturating_sub(sw.sent_ms) >= SW_RETRY_MS {
            sw.tries += 1;
            return sw.out(now_ms);
        }
        Vec::new()
    }
}

// ---------------------------------------------------------------------------------------------
// Viaje entre sistema y programas (EV_GAMEPADS)

const PAD_KINDS: [PadKind; 6] = [PadKind::Generic, PadKind::DualShock4, PadKind::DualSense, PadKind::SwitchPro, PadKind::Xbox360, PadKind::XboxOne];

impl Wire for PadKind {
    fn put(&self, w: &mut Writer) {
        w.put(&(PAD_KINDS.iter().position(|k| k == self).unwrap_or(0) as u8));
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        PAD_KINDS.get(u8::get(r)? as usize).copied().ok_or(Bad)
    }
}

impl Wire for PadInfo {
    fn put(&self, w: &mut Writer) {
        w.put(&self.id).put(&self.kind).put(&self.name);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(PadInfo { id: r.get()?, kind: r.get()?, name: r.get()? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{decode, encode};
    use alloc::string::ToString;

    fn pad(kind: PadKind) -> Pad {
        Pad::new(kind, None)
    }

    fn feed1(p: &mut Pad, r: &[u8]) -> PadState {
        p.feed(r, 0).0.expect("estado nuevo")
    }

    #[test]
    fn detecta_por_vid_pid_y_clase() {
        let i = |vid, pid, class, sub, proto| PadIface { vid, pid, class, sub, proto };
        let kind = |x: PadIface| match detect(&x) {
            Detect::Pad(p) => Some(p.kind()),
            _ => None,
        };
        assert_eq!(kind(i(0x054C, 0x05C4, 3, 0, 0)), Some(PadKind::DualShock4));
        assert_eq!(kind(i(0x054C, 0x09CC, 3, 0, 0)), Some(PadKind::DualShock4));
        assert_eq!(kind(i(0x054C, 0x0CE6, 3, 0, 0)), Some(PadKind::DualSense));
        assert_eq!(kind(i(0x054C, 0x0DF2, 3, 0, 0)), Some(PadKind::DualSense));
        assert_eq!(kind(i(0x057E, 0x2009, 3, 0, 0)), Some(PadKind::SwitchPro));
        assert_eq!(kind(i(0x045E, 0x028E, 0xFF, 0x5D, 0x01)), Some(PadKind::Xbox360));
        assert_eq!(kind(i(0x045E, 0x02EA, 0xFF, 0x47, 0xD0)), Some(PadKind::XboxOne));
        assert!(matches!(detect(&i(1, 2, 3, 1, 1)), Detect::No));
        assert!(matches!(detect(&i(1, 2, 3, 1, 2)), Detect::No));
        assert!(matches!(detect(&i(1, 2, 3, 0, 0)), Detect::NeedsDescriptor));
        assert!(matches!(detect(&i(1, 2, 8, 6, 80)), Detect::No));
        assert!(matches!(detect(&i(1, 2, 0xFF, 0x5D, 0x02)), Detect::No));
    }

    #[test]
    fn dualshock4() {
        let mut p = pad(PadKind::DualShock4);
        let mut r = [0u8; 64];
        r[0] = 1;
        r[1..5].copy_from_slice(&[0, 255, 128, 128]);
        r[5] = 0x20 | 2; // cruz + hat al este
        r[6] = 0x01 | 0x20; // L1 + Options
        r[7] = 0x01 | 0x02; // PS + touchpad
        r[8] = 255;
        let s = feed1(&mut p, &r);
        assert_eq!(s.buttons, BTN_SOUTH | BTN_RIGHT | BTN_L1 | BTN_START | BTN_HOME | BTN_EXTRA);
        assert_eq!(s.axes[AXIS_LX], -32768);
        assert_eq!(s.axes[AXIS_LY], 32767);
        assert_eq!(s.axes[AXIS_LT], 32767);
        assert_eq!(s.axes[AXIS_RT], 0);
        // El mismo informe no es un cambio; el hat 8 es centro.
        assert!(p.feed(&r, 1).0.is_none());
        r[5] = 8;
        assert_eq!(feed1(&mut p, &r).buttons & (BTN_UP | BTN_RIGHT), 0);
        // Un informe que no es el 0x01 se ignora.
        assert!(p.feed(&[0x11, 0, 0], 2).0.is_none());
    }

    #[test]
    fn dualsense() {
        let mut p = pad(PadKind::DualSense);
        let mut r = [0u8; 64];
        r[0] = 1;
        r[1..7].copy_from_slice(&[128, 128, 255, 0, 0, 200]);
        r[8] = 0x80 | 7; // triángulo + hat arriba-izquierda
        r[9] = 0x10 | 0x80; // Create + R3
        r[10] = 1;
        let s = feed1(&mut p, &r);
        assert_eq!(s.buttons, BTN_NORTH | BTN_UP | BTN_LEFT | BTN_SELECT | BTN_R3 | BTN_HOME);
        assert_eq!(s.axes[AXIS_RX], 32767);
        assert_eq!(s.axes[AXIS_RY], -32768);
        assert_eq!(s.axes[AXIS_RT], (200i32 * 32767 / 255) as i16);
    }

    #[test]
    fn xbox360() {
        let mut p = pad(PadKind::Xbox360);
        let mut r = [0u8; 20];
        r[1] = 20;
        r[2] = 0x01 | 0x10; // arriba + Start
        r[3] = 0x10 | 0x80 | 0x04; // A + Y + Guide
        r[4] = 255;
        r[6..8].copy_from_slice(&(-32768i16).to_le_bytes());
        r[8..10].copy_from_slice(&32767i16.to_le_bytes()); // arriba en el mando
        let s = feed1(&mut p, &r);
        assert_eq!(s.buttons, BTN_UP | BTN_START | BTN_SOUTH | BTN_NORTH | BTN_HOME | BTN_L2);
        assert_eq!(s.axes[AXIS_LX], -32768);
        assert_eq!(s.axes[AXIS_LY], -32767, "Y hacia arriba es negativa");
        assert_eq!(s.axes[AXIS_LT], 32767);
        // Mensajes de otro tipo (conexión) no son estado.
        assert!(p.feed(&[0x08, 0x00, 0x00], 1).0.is_none());
    }

    #[test]
    fn xbox_one() {
        let mut p = pad(PadKind::XboxOne);
        assert_eq!(p.start(0), vec![vec![0x05, 0x20, 0x00, 0x01, 0x00]]);
        let mut r = [0u8; 18];
        r[0] = 0x20;
        r[3] = 14;
        r[4] = 0x20 | 0x08; // B + View
        r[5] = 0x10 | 0x08; // LB + derecha
        r[6..8].copy_from_slice(&1023u16.to_le_bytes());
        r[10..12].copy_from_slice(&1000i16.to_le_bytes());
        r[12..14].copy_from_slice(&(-1000i16).to_le_bytes());
        let s = feed1(&mut p, &r);
        assert_eq!(s.buttons, BTN_EAST | BTN_SELECT | BTN_L1 | BTN_RIGHT | BTN_L2);
        assert_eq!(s.axes[AXIS_LT], 32767);
        assert_eq!(s.axes[AXIS_LX], 1000);
        assert_eq!(s.axes[AXIS_LY], 1000);
        // Botón Xbox: informe 0x07 aparte; se mantiene al llegar otro 0x20.
        let s = feed1(&mut p, &[0x07, 0x20, 0, 2, 1, 0x5B]);
        assert_ne!(s.buttons & BTN_HOME, 0);
        r[4] = 0;
        assert_ne!(feed1(&mut p, &r).buttons & BTN_HOME, 0);
        let s = feed1(&mut p, &[0x07, 0x20, 0, 2, 0, 0x5B]);
        assert_eq!(s.buttons & BTN_HOME, 0);
    }

    /// Informe 0x30 del Pro Controller con los sticks dados (12 bits).
    fn pro(b3: u8, b4: u8, b5: u8, l: (u16, u16), r: (u16, u16)) -> [u8; 12] {
        let pack = |(x, y): (u16, u16)| [x as u8, (x >> 8) as u8 | (y << 4) as u8, (y >> 4) as u8];
        let mut o = [0u8; 12];
        o[0] = 0x30;
        o[3..6].copy_from_slice(&[b3, b4, b5]);
        o[6..9].copy_from_slice(&pack(l));
        o[9..12].copy_from_slice(&pack(r));
        o
    }

    #[test]
    fn switch_pro_saludo_y_reintentos() {
        let mut p = pad(PadKind::SwitchPro);
        assert_eq!(p.start(100), vec![vec![0x80, 0x01]]);
        // No contesta: reintenta pasados 500 ms, no antes.
        assert!(p.tick(300).is_empty());
        assert_eq!(p.tick(600), vec![vec![0x80, 0x01]]);
        // Una respuesta que no toca no avanza; las que tocan, sí, en orden.
        assert!(p.feed(&[0x81, 0x03], 700).1.is_empty());
        let mut sent = vec![];
        for code in [1u8, 2, 3, 2] {
            sent.push(p.feed(&[0x81, code, 0, 0], 700).1);
        }
        assert_eq!(sent[0], vec![vec![0x80, 2]]);
        assert_eq!(sent[1], vec![vec![0x80, 3]]);
        assert_eq!(sent[2], vec![vec![0x80, 2]]);
        assert_eq!(sent[3][0], vec![0x80, 4]);
        assert_eq!(sent[3][1][0], 0x01);
        assert_eq!(&sent[3][1][10..], &[0x03, 0x30]);
        // Sigue sin informes: reintenta el último paso. Con el primero 0x30, listo y sin más reintentos.
        assert_eq!(p.tick(1300).len(), 2);
        p.feed(&pro(0, 0, 0, (2048, 2048), (2048, 2048)), 1400);
        assert!(p.tick(100_000).is_empty());
    }

    #[test]
    fn switch_pro_se_rinde() {
        let mut p = pad(PadKind::SwitchPro);
        p.start(0);
        let n = (1..=100).filter(|i| !p.tick(*i * 500).is_empty()).count();
        assert_eq!(n, SW_TRIES as usize);
    }

    #[test]
    fn switch_pro_informe() {
        let mut p = pad(PadKind::SwitchPro);
        // B (abajo) + A (derecha) + ZR; Capture + Plus; arriba + ZL. Stick izquierdo a la derecha y arriba.
        let r = pro(0x04 | 0x08 | 0x80, 0x20 | 0x02, 0x02 | 0x80, (4095, 4095), (0, 2048));
        let s = feed1(&mut p, &r);
        assert_eq!(s.buttons, BTN_SOUTH | BTN_EAST | BTN_R2 | BTN_EXTRA | BTN_START | BTN_UP | BTN_L2);
        assert_eq!(s.axes[AXIS_LX], 32752);
        assert_eq!(s.axes[AXIS_LY], -32752, "Y arriba es negativa");
        assert_eq!((s.axes[AXIS_LT], s.axes[AXIS_RT]), (32767, 32767));
        assert_eq!(s.axes[AXIS_RX], -32768);
        assert_eq!(s.axes[AXIS_RY], 0);
    }

    // Descriptor de mando clásico: 8 botones, hat de 8 posiciones, X Y Z Rz de 8 bits.
    const GAMEPAD: &[u8] = &[
        0x05, 0x01, 0x09, 0x05, 0xA1, 0x01, // Generic Desktop, Game Pad, Application
        0x05, 0x09, 0x19, 0x01, 0x29, 0x08, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, // 8 botones
        0x05, 0x01, 0x09, 0x39, 0x15, 0x00, 0x25, 0x07, 0x75, 0x04, 0x95, 0x01, 0x81, 0x42, // hat
        0x75, 0x04, 0x95, 0x01, 0x81, 0x03, // relleno
        0x09, 0x30, 0x09, 0x31, 0x09, 0x32, 0x09, 0x35, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x04, 0x81, 0x02, // ejes
        0xC0,
    ];

    #[test]
    fn generico_sin_id() {
        let mut p = Pad::from_descriptor(GAMEPAD).expect("es un mando");
        assert_eq!(p.kind(), PadKind::Generic);
        assert_eq!(p.default_name(), "Mando USB");
        // botones 1 y 6, hat al sur (4), X=0, Y=255, Z=128, Rz=255
        let s = feed1(&mut p, &[0b0010_0001, 0x04, 0, 255, 128, 255]);
        assert_eq!(s.buttons, BTN_SOUTH | BTN_R1 | BTN_DOWN);
        assert_eq!((s.axes[AXIS_LX], s.axes[AXIS_LY]), (-32768, 32767));
        assert_eq!(s.axes[AXIS_RX], (128i32 * 65535 / 255 - 32768) as i16);
        assert_eq!(s.axes[AXIS_RY], 32767);
        // Informe corto: se ignora, no suelta nada.
        assert!(p.feed(&[1], 0).0.is_none());
        // Hat fuera de rango = centro; los botones se sueltan.
        let s = feed1(&mut p, &[0, 0x08, 0, 255, 128, 255]);
        assert_eq!(s.buttons, 0);
    }

    #[test]
    fn generico_con_ids_y_gatillos() {
        // Informe 1: X, Y, Z, Rz con signo; informe 2: 4 botones y Rx/Ry (gatillos, porque hay Z/Rz).
        let d: &[u8] = &[
            0x05, 0x01, 0x09, 0x04, 0xA1, 0x01, // Joystick
            0x85, 0x01, 0x09, 0x30, 0x09, 0x31, 0x09, 0x32, 0x09, 0x35, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x04, 0x81, 0x02,
            0x85, 0x02, 0x05, 0x09, 0x19, 0x01, 0x29, 0x04, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x04, 0x81, 0x02, 0x75, 0x04, 0x95, 0x01, 0x81, 0x03,
            0x05, 0x01, 0x09, 0x33, 0x09, 0x34, 0x16, 0x00, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x10, 0x95, 0x02, 0x81, 0x02,
            0xC0,
        ];
        let mut p = Pad::from_descriptor(d).unwrap();
        let s = feed1(&mut p, &[1, 0x80, 0x7F, 0, 0]);
        assert_eq!((s.axes[AXIS_LX], s.axes[AXIS_LY]), (-32768, 32767));
        let s = feed1(&mut p, &[2, 0b0101, 0xFF, 0x00, 0, 0]);
        assert_eq!(s.buttons, BTN_SOUTH | BTN_WEST | BTN_L2);
        assert_eq!(s.axes[AXIS_LT], 32767);
        // El informe 1 no pisa los botones del 2.
        let s = feed1(&mut p, &[1, 0x10, 0x10, 0, 0]);
        assert_eq!(s.buttons, BTN_SOUTH | BTN_WEST | BTN_L2);
        // ID desconocido: nada.
        assert!(p.feed(&[9, 1, 1, 1, 1], 0).0.is_none());
    }

    #[test]
    fn descriptor_que_no_es_mando() {
        // Teclado: Generic Desktop, Keyboard (0x06).
        let kb: &[u8] = &[0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x05, 0x09, 0x19, 0x01, 0x29, 0x08, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0xC0];
        assert!(Pad::from_descriptor(kb).is_none());
        // Joystick sin nada que leer.
        assert!(Pad::from_descriptor(&[0x05, 0x01, 0x09, 0x04, 0xA1, 0x01, 0xC0]).is_none());
        // Truncado y basura.
        assert!(Pad::from_descriptor(&GAMEPAD[..9]).is_none());
        assert!(Pad::from_descriptor(&[0xFF; 40]).is_none());
        assert!(Pad::from_descriptor(&[]).is_none());
    }

    #[test]
    fn wire_de_mandos() {
        let v = vec![
            PadInfo { id: 3, kind: PadKind::SwitchPro, name: "Pro Controller".to_string() },
            PadInfo { id: 4, kind: PadKind::Generic, name: String::new() },
        ];
        assert_eq!(decode::<Vec<PadInfo>>(&encode(&v)), Ok(v));
        // Un tipo inventado no se acepta.
        assert!(decode::<PadKind>(&[9]).is_err());
    }
}
