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
use alloc::string::String;
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
    let _ = i;
    todo!("paso 5: implementar")
}

/// Controlador de un mando: traduce sus informes a `PadState` y lleva su saludo si lo necesita.
pub struct Pad {
    kind: PadKind,
    // El implementador añade aquí lo que necesite (mapa del descriptor genérico, fase del saludo…).
}

impl Pad {
    /// Mando HID genérico a partir de su descriptor de informes. `None` si su colección de aplicación
    /// no es Joystick (Generic Desktop 0x04) ni Game Pad (0x05), o si no tiene botones ni ejes.
    pub fn from_descriptor(desc: &[u8]) -> Option<Pad> {
        let _ = desc;
        todo!("paso 5: implementar")
    }

    pub fn kind(&self) -> PadKind {
        self.kind
    }

    /// Nombre para cuando el dispositivo no da el suyo: «Mando USB», «Mando DualShock 4»…
    pub fn default_name(&self) -> &'static str {
        todo!("paso 5: implementar")
    }

    /// Informes de salida que hay que mandar nada más configurar el dispositivo (el saludo del Pro
    /// Controller, el encendido del de Xbox One). Vacío si no hace falta nada.
    pub fn start(&mut self, now_ms: u64) -> Vec<Vec<u8>> {
        let _ = now_ms;
        todo!("paso 5: implementar")
    }

    /// Un informe de entrada tal cual llega del endpoint (con su ID de informe si lo lleva).
    /// Devuelve el estado si ha cambiado y los informes de salida que tocan ahora.
    pub fn feed(&mut self, report: &[u8], now_ms: u64) -> (Option<PadState>, Vec<Vec<u8>>) {
        let _ = (report, now_ms);
        todo!("paso 5: implementar")
    }

    /// Paso de tiempo: reintentos del saludo si el mando no contesta. Vacío si no toca nada.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Vec<u8>> {
        let _ = now_ms;
        todo!("paso 5: implementar")
    }
}
