//! Entrada: tipos de evento (CONTRATO, no cambian) y decodificadores de teclado y ratón PS/2.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    /// Tecla imprimible; lleva el carácter base sin modificadores (minúscula).
    Char(char),
    Escape,
    Backspace,
    Tab,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// F1..F12.
    F(u8),
    /// Tecla de sistema (la del logo).
    Super,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KeyEvent {
    pub key: Key,
    /// Carácter a escribir, ya con mayúsculas, AltGr y acentos. `None` si no escribe nada
    /// (teclas de control, o Ctrl/Alt pulsados).
    pub ch: Option<char>,
    /// true = pulsada, false = soltada.
    pub down: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Paquete de ratón: desplazamiento en píxeles de pantalla (dy positivo = hacia abajo)
/// y botones (bit 0 izquierdo, bit 1 derecho, bit 2 central).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MousePacket {
    pub dx: i32,
    pub dy: i32,
    pub buttons: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MousePacket),
    /// Paso de tiempo; lleva los milisegundos desde el arranque.
    Tick(u64),
}

/// Decodificador de teclado (scan code set 1, distribución española).
pub struct Keyboard {}

impl Default for Keyboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Keyboard {
    pub const fn new() -> Self {
        Keyboard {}
    }
    /// Recibe un byte del teclado; devuelve un evento cuando se completa una tecla.
    pub fn feed(&mut self, _byte: u8) -> Option<KeyEvent> {
        None
    }
}

/// Decodificador de ratón PS/2 (paquetes de 3 bytes).
pub struct Ps2Mouse {}

impl Default for Ps2Mouse {
    fn default() -> Self {
        Self::new()
    }
}

impl Ps2Mouse {
    pub const fn new() -> Self {
        Ps2Mouse {}
    }
    /// Recibe un byte del ratón; devuelve un paquete cuando se completa.
    pub fn feed(&mut self, _byte: u8) -> Option<MousePacket> {
        None
    }
}
