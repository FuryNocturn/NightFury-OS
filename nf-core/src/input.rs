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

/// Paquete de ratón: desplazamiento en píxeles de pantalla (dy positivo = hacia abajo),
/// botones (bit 0 izquierdo, bit 1 derecho, bit 2 central) y rueda `dz` en muescas
/// (positivo = hacia abajo; siempre 0 si el ratón no tiene rueda).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MousePacket {
    pub dx: i32,
    pub dy: i32,
    pub buttons: u8,
    pub dz: i32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MousePacket),
    /// Paso de tiempo; lleva los milisegundos desde el arranque.
    Tick(u64),
}

/// Decodificador de teclado (scan code set 1, distribución española).
pub struct Keyboard {
    /// Se acaba de recibir el prefijo 0xE0.
    ext: bool,
    /// Bytes que quedan por ignorar de una secuencia 0xE1 (Pausa).
    skip: u8,
    lshift: bool,
    rshift: bool,
    lctrl: bool,
    rctrl: bool,
    alt: bool,
    altgr: bool,
    caps: bool,
    caps_held: bool,
    /// Tecla muerta pendiente (` ´ ¨ ^).
    dead: Option<char>,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self::new()
    }
}

/// Teclas imprimibles: (normal, con Mayús, con AltGr; '\0' = sin AltGr).
fn layout(code: u8) -> Option<(char, char, char)> {
    let t = match code {
        0x29 => ('º', 'ª', '\\'),
        0x02 => ('1', '!', '|'),
        0x03 => ('2', '"', '@'),
        0x04 => ('3', '·', '#'),
        0x05 => ('4', '$', '~'),
        0x06 => ('5', '%', '€'),
        0x07 => ('6', '&', '¬'),
        0x08 => ('7', '/', '\0'),
        0x09 => ('8', '(', '\0'),
        0x0A => ('9', ')', '\0'),
        0x0B => ('0', '=', '\0'),
        0x0C => ('\'', '?', '\0'),
        0x0D => ('¡', '¿', '\0'),
        0x1A => ('`', '^', '['),
        0x1B => ('+', '*', ']'),
        0x27 => ('ñ', 'Ñ', '\0'),
        0x28 => ('´', '¨', '{'),
        0x2B => ('ç', 'Ç', '}'),
        0x56 => ('<', '>', '\0'),
        0x33 => (',', ';', '\0'),
        0x34 => ('.', ':', '\0'),
        0x35 => ('-', '_', '\0'),
        0x10..=0x19 | 0x1E..=0x26 | 0x2C..=0x32 => {
            let c = match code {
                0x10..=0x19 => b"qwertyuiop"[(code - 0x10) as usize],
                0x1E..=0x26 => b"asdfghjkl"[(code - 0x1E) as usize],
                _ => b"zxcvbnm"[(code - 0x2C) as usize],
            } as char;
            (c, c.to_ascii_uppercase(), if c == 'e' { '€' } else { '\0' })
        }
        _ => return None,
    };
    Some(t)
}

/// Combina una tecla muerta con la letra siguiente (solo vocales).
fn compose(dead: char, c: char) -> Option<char> {
    let i = "aeiouAEIOU".chars().position(|v| v == c)?;
    let set = match dead {
        '´' => "áéíóúÁÉÍÓÚ",
        '`' => "àèìòùÀÈÌÒÙ",
        '¨' => "äëïöüÄËÏÖÜ",
        _ => "âêîôûÂÊÎÔÛ",
    };
    set.chars().nth(i)
}

impl Keyboard {
    pub const fn new() -> Self {
        Keyboard {
            ext: false,
            skip: 0,
            lshift: false,
            rshift: false,
            lctrl: false,
            rctrl: false,
            alt: false,
            altgr: false,
            caps: false,
            caps_held: false,
            dead: None,
        }
    }
    /// Recibe un byte del teclado; devuelve un evento cuando se completa una tecla.
    pub fn feed(&mut self, byte: u8) -> Option<KeyEvent> {
        if self.skip > 0 {
            self.skip -= 1;
            return None;
        }
        match byte {
            0xE0 => {
                self.ext = true;
                return None;
            }
            0xE1 => {
                // Pausa: E1 1D 45 E1 9D C5; cada E1 va seguido de 2 bytes.
                self.ext = false;
                self.skip = 2;
                return None;
            }
            _ => {}
        }
        let ext = self.ext;
        self.ext = false;
        let down = byte & 0x80 == 0;
        let code = byte & 0x7F;
        // Candidato a carácter; `dead` indica que es una tecla muerta.
        let mut cand: Option<char> = None;
        let mut dead = false;
        let key = match (ext, code) {
            (true, 0x2A | 0x36) => return None, // mayúsculas falsas
            (false, 0x2A) => {
                self.lshift = down;
                Key::Other
            }
            (false, 0x36) => {
                self.rshift = down;
                Key::Other
            }
            (_, 0x1D) => {
                if ext {
                    self.rctrl = down;
                } else {
                    self.lctrl = down;
                }
                Key::Other
            }
            (false, 0x38) => {
                self.alt = down;
                Key::Other
            }
            (true, 0x38) => {
                self.altgr = down;
                Key::Other
            }
            (false, 0x3A) => {
                // Solo al pulsar, no con la repetición automática de la tecla mantenida.
                if down && !self.caps_held {
                    self.caps = !self.caps;
                }
                self.caps_held = down;
                Key::Other
            }
            (false, 0x01) => Key::Escape,
            (false, 0x0E) => Key::Backspace,
            (false, 0x0F) => Key::Tab,
            (_, 0x1C) => Key::Enter,
            (false, 0x3B..=0x44) => Key::F(code - 0x3A),
            (false, 0x57) => Key::F(11),
            (false, 0x58) => Key::F(12),
            (true, 0x48) => Key::Up,
            (true, 0x50) => Key::Down,
            (true, 0x4B) => Key::Left,
            (true, 0x4D) => Key::Right,
            (true, 0x47) => Key::Home,
            (true, 0x4F) => Key::End,
            (true, 0x49) => Key::PageUp,
            (true, 0x51) => Key::PageDown,
            (true, 0x52) => Key::Insert,
            (true, 0x53) => Key::Delete,
            (true, 0x5B | 0x5C) => Key::Super,
            (true, 0x35) => {
                cand = Some('/');
                Key::Char('/')
            }
            (true, _) => return None,
            (false, 0x39) => {
                cand = Some(' ');
                Key::Char(' ')
            }
            (false, 0x37 | 0x47..=0x53) => {
                // Teclado numérico: siempre números.
                let c = match code {
                    0x37 => '*',
                    0x47 => '7',
                    0x48 => '8',
                    0x49 => '9',
                    0x4A => '-',
                    0x4B => '4',
                    0x4C => '5',
                    0x4D => '6',
                    0x4E => '+',
                    0x4F => '1',
                    0x50 => '2',
                    0x51 => '3',
                    0x52 => '0',
                    _ => ',',
                };
                cand = Some(c);
                Key::Char(c)
            }
            (false, _) => {
                let (n, s, a) = layout(code)?;
                let shift = self.lshift || self.rshift;
                if self.altgr && a != '\0' {
                    cand = Some(a);
                } else if n.is_alphabetic() {
                    cand = Some(if shift != self.caps { s } else { n });
                } else {
                    cand = Some(if shift { s } else { n });
                    dead = matches!(code, 0x1A | 0x28) && !self.altgr;
                }
                Key::Char(n)
            }
        };
        let shift = self.lshift || self.rshift;
        let ctrl = self.lctrl || self.rctrl;
        let alt = self.alt;
        let mut ch = None;
        if down && !ctrl && !alt {
            if let Some(c) = cand {
                ch = if dead {
                    self.dead = Some(c);
                    None
                } else if let Some(d) = self.dead.take() {
                    if c == ' ' {
                        Some(d)
                    } else {
                        compose(d, c).or(Some(c))
                    }
                } else {
                    Some(c)
                };
            }
        }
        Some(KeyEvent { key, ch, down, shift, ctrl, alt })
    }
}

/// Decodificador de ratón PS/2: paquetes de 3 bytes, o de 4 si el ratón tiene rueda
/// (protocolo PS/2 con rueda, identificador 3).
pub struct Ps2Mouse {
    buf: [u8; 4],
    n: usize,
    wheel: bool,
}

impl Default for Ps2Mouse {
    fn default() -> Self {
        Self::new()
    }
}

impl Ps2Mouse {
    pub const fn new() -> Self {
        Ps2Mouse { buf: [0; 4], n: 0, wheel: false }
    }
    /// Activa el paquete de 4 bytes (cuarto byte = rueda). Solo si el ratón respondió con el id 3.
    pub fn set_wheel(&mut self, on: bool) {
        self.wheel = on;
        self.n = 0;
    }
    /// Recibe un byte del ratón; devuelve un paquete cuando se completa.
    pub fn feed(&mut self, byte: u8) -> Option<MousePacket> {
        // El bit 3 del primer byte siempre es 1: sirve para resincronizar.
        if self.n == 0 && byte & 0x08 == 0 {
            return None;
        }
        self.buf[self.n] = byte;
        self.n += 1;
        if self.n < if self.wheel { 4 } else { 3 } {
            return None;
        }
        self.n = 0;
        let b0 = self.buf[0];
        let (mut dx, mut dy) = (0, 0);
        if b0 & 0xC0 == 0 {
            // 9 bits en complemento a dos: el signo está en el byte 0.
            dx = self.buf[1] as i32 - if b0 & 0x10 != 0 { 256 } else { 0 };
            dy = self.buf[2] as i32 - if b0 & 0x20 != 0 { 256 } else { 0 };
        }
        // Rueda: el cuarto byte es un entero con signo. Convención del protocolo: positivo = la
        // rueda gira hacia el usuario (el contenido sube), que es nuestro `dz` positivo = abajo.
        let dz = if self.wheel { self.buf[3] as i8 as i32 } else { 0 };
        // El ratón da Y positivo hacia arriba; la pantalla, hacia abajo.
        Some(MousePacket { dx, dy: -dy, buttons: b0 & 7, dz })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Envía bytes y devuelve los eventos producidos.
    fn feed(kb: &mut Keyboard, bytes: &[u8]) -> Vec<KeyEvent> {
        bytes.iter().filter_map(|&b| kb.feed(b)).collect()
    }

    /// Pulsa y suelta un código (sin prefijo) y devuelve el carácter de la pulsación.
    fn tap(kb: &mut Keyboard, code: u8) -> Option<char> {
        let ev = feed(kb, &[code, code | 0x80]);
        assert_eq!(ev.len(), 2);
        assert!(ev[0].down && !ev[1].down && ev[1].ch.is_none());
        ev[0].ch
    }

    const LSHIFT: u8 = 0x2A;
    const ALTGR: [u8; 2] = [0xE0, 0x38];

    #[test]
    fn letras_y_mayusculas() {
        let mut kb = Keyboard::new();
        assert_eq!(tap(&mut kb, 0x1E), Some('a'));
        kb.feed(LSHIFT);
        assert_eq!(tap(&mut kb, 0x1E), Some('A'));
        let ev = feed(&mut kb, &[0x1E]);
        assert!(ev[0].shift);
        kb.feed(0x1E | 0x80);
        kb.feed(LSHIFT | 0x80);
        assert_eq!(tap(&mut kb, 0x1E), Some('a'));
        assert_eq!(feed(&mut kb, &[0x1E])[0].key, Key::Char('a'));
    }

    #[test]
    fn bloq_mayus() {
        let mut kb = Keyboard::new();
        tap(&mut kb, 0x3A);
        assert_eq!(tap(&mut kb, 0x1E), Some('A'));
        assert_eq!(tap(&mut kb, 0x27), Some('Ñ'));
        assert_eq!(tap(&mut kb, 0x02), Some('1')); // no afecta a símbolos
        kb.feed(LSHIFT);
        assert_eq!(tap(&mut kb, 0x1E), Some('a')); // se invierte
        kb.feed(LSHIFT | 0x80);
        tap(&mut kb, 0x3A);
        assert_eq!(tap(&mut kb, 0x1E), Some('a'));
    }

    #[test]
    fn altgr() {
        let mut kb = Keyboard::new();
        feed(&mut kb, &ALTGR);
        let ev = feed(&mut kb, &[0x03]);
        assert_eq!(ev[0].ch, Some('@'));
        assert!(!ev[0].alt && !ev[0].ctrl);
        kb.feed(0x03 | 0x80);
        assert_eq!(tap(&mut kb, 0x12), Some('€'));
        assert_eq!(tap(&mut kb, 0x1A), Some('['));
        assert_eq!(tap(&mut kb, 0x2B), Some('}'));
        feed(&mut kb, &[0xE0, 0xB8]);
        assert_eq!(tap(&mut kb, 0x03), Some('2'));
    }

    #[test]
    fn enie_y_simbolos() {
        let mut kb = Keyboard::new();
        assert_eq!(tap(&mut kb, 0x27), Some('ñ'));
        assert_eq!(tap(&mut kb, 0x2B), Some('ç'));
        assert_eq!(tap(&mut kb, 0x56), Some('<'));
        assert_eq!(tap(&mut kb, 0x33), Some(','));
        kb.feed(LSHIFT);
        assert_eq!(tap(&mut kb, 0x27), Some('Ñ'));
        assert_eq!(tap(&mut kb, 0x03), Some('"'));
        assert_eq!(tap(&mut kb, 0x0D), Some('¿'));
        assert_eq!(feed(&mut kb, &[0x27])[0].key, Key::Char('ñ'));
    }

    #[test]
    fn teclas_muertas() {
        let mut kb = Keyboard::new();
        assert_eq!(tap(&mut kb, 0x28), None);
        assert_eq!(tap(&mut kb, 0x1E), Some('á'));
        // ´ + Mayús + e = É
        assert_eq!(tap(&mut kb, 0x28), None);
        kb.feed(LSHIFT);
        assert_eq!(tap(&mut kb, 0x12), Some('É'));
        // ¨ + u = ü (¨ es Mayús + 0x28)
        assert_eq!(tap(&mut kb, 0x28), None);
        kb.feed(LSHIFT | 0x80);
        assert_eq!(tap(&mut kb, 0x16), Some('ü'));
        // ´ + espacio = ´
        tap(&mut kb, 0x28);
        assert_eq!(tap(&mut kb, 0x39), Some('´'));
        // ´ + t = t
        tap(&mut kb, 0x28);
        assert_eq!(tap(&mut kb, 0x14), Some('t'));
        assert_eq!(tap(&mut kb, 0x14), Some('t'));
        // ` + o = ò, ^ + i = î
        tap(&mut kb, 0x1A);
        assert_eq!(tap(&mut kb, 0x18), Some('ò'));
        kb.feed(LSHIFT);
        tap(&mut kb, 0x1A);
        kb.feed(LSHIFT | 0x80);
        assert_eq!(tap(&mut kb, 0x17), Some('î'));
        // ^ + espacio = ^
        kb.feed(LSHIFT);
        tap(&mut kb, 0x1A);
        kb.feed(LSHIFT | 0x80);
        assert_eq!(tap(&mut kb, 0x39), Some('^'));
    }

    #[test]
    fn extendidas() {
        let mut kb = Keyboard::new();
        let casos: [(u8, Key); 11] = [
            (0x48, Key::Up),
            (0x50, Key::Down),
            (0x4B, Key::Left),
            (0x4D, Key::Right),
            (0x47, Key::Home),
            (0x4F, Key::End),
            (0x49, Key::PageUp),
            (0x51, Key::PageDown),
            (0x52, Key::Insert),
            (0x53, Key::Delete),
            (0x5B, Key::Super),
        ];
        for (c, k) in casos {
            let ev = feed(&mut kb, &[0xE0, c, 0xE0, c | 0x80]);
            assert_eq!(ev.len(), 2);
            assert_eq!(ev[0].key, k);
            assert!(ev[0].down && ev[0].ch.is_none());
            assert!(!ev[1].down);
        }
        assert_eq!(feed(&mut kb, &[0xE0, 0x5C])[0].key, Key::Super);
        // Mayúsculas falsas ignoradas.
        assert!(feed(&mut kb, &[0xE0, 0x2A, 0xE0, 0xAA, 0xE0, 0x36]).is_empty());
        // Control y escape.
        assert_eq!(feed(&mut kb, &[0x01])[0].key, Key::Escape);
        assert_eq!(feed(&mut kb, &[0x0E])[0].key, Key::Backspace);
        assert_eq!(feed(&mut kb, &[0x0F])[0].key, Key::Tab);
        assert_eq!(feed(&mut kb, &[0x1C])[0].key, Key::Enter);
        assert_eq!(feed(&mut kb, &[0xE0, 0x1C])[0].key, Key::Enter);
    }

    #[test]
    fn ctrl_no_escribe() {
        let mut kb = Keyboard::new();
        kb.feed(0x1D);
        let ev = feed(&mut kb, &[0x2E]);
        assert_eq!(ev[0].key, Key::Char('c'));
        assert_eq!(ev[0].ch, None);
        assert!(ev[0].ctrl);
        kb.feed(0x2E | 0x80);
        kb.feed(0x1D | 0x80);
        assert_eq!(tap(&mut kb, 0x2E), Some('c'));
        // Ctrl derecho y Alt izquierdo.
        feed(&mut kb, &[0xE0, 0x1D]);
        assert!(feed(&mut kb, &[0x2E])[0].ctrl);
        kb.feed(0x2E | 0x80);
        feed(&mut kb, &[0xE0, 0x9D]);
        kb.feed(0x38);
        let ev = feed(&mut kb, &[0x2E]);
        assert!(ev[0].alt && ev[0].ch.is_none());
    }

    #[test]
    fn numerico_y_funciones() {
        let mut kb = Keyboard::new();
        let num = [
            (0x47, '7'),
            (0x48, '8'),
            (0x49, '9'),
            (0x4A, '-'),
            (0x4B, '4'),
            (0x4C, '5'),
            (0x4D, '6'),
            (0x4E, '+'),
            (0x4F, '1'),
            (0x50, '2'),
            (0x51, '3'),
            (0x52, '0'),
            (0x53, ','),
            (0x37, '*'),
        ];
        for (c, ch) in num {
            assert_eq!(tap(&mut kb, c), Some(ch));
        }
        assert_eq!(feed(&mut kb, &[0xE0, 0x35])[0].ch, Some('/'));
        assert_eq!(feed(&mut kb, &[0x3B])[0].key, Key::F(1));
        assert_eq!(feed(&mut kb, &[0x44])[0].key, Key::F(10));
        assert_eq!(feed(&mut kb, &[0x57])[0].key, Key::F(11));
        assert_eq!(feed(&mut kb, &[0x58])[0].key, Key::F(12));
        assert_eq!(feed(&mut kb, &[0x3B])[0].ch, None);
    }

    #[test]
    fn pausa_no_desincroniza() {
        let mut kb = Keyboard::new();
        assert!(feed(&mut kb, &[0xE1, 0x1D, 0x45, 0xE1, 0x9D, 0xC5]).is_empty());
        assert_eq!(tap(&mut kb, 0x1E), Some('a'));
    }

    #[test]
    fn raton_movimiento() {
        let mut m = Ps2Mouse::new();
        assert_eq!(m.feed(0x08), None);
        assert_eq!(m.feed(5), None);
        // Y arriba (+3) = dy -3 en pantalla.
        assert_eq!(m.feed(3), Some(MousePacket { dx: 5, dy: -3, buttons: 0, dz: 0 }));
        // X = -2 (0xFE con signo), Y = -4 (0xFC con signo) -> dy +4.
        let p = [0x08 | 0x10 | 0x20, 0xFE, 0xFC].map(|b| m.feed(b));
        assert_eq!(p[2], Some(MousePacket { dx: -2, dy: 4, buttons: 0, dz: 0 }));
        // Valor extremo -256 y +255.
        let p = [0x08 | 0x10, 0x00, 0xFF].map(|b| m.feed(b));
        assert_eq!(p[2], Some(MousePacket { dx: -256, dy: -255, buttons: 0, dz: 0 }));
    }

    #[test]
    fn raton_rueda_cuatro_bytes() {
        let mut m = Ps2Mouse::new();
        m.set_wheel(true);
        assert_eq!([0x08, 1, 2].map(|b| m.feed(b)), [None; 3]);
        // Cuarto byte +1 = hacia abajo; 0xFF = -1 = hacia arriba.
        assert_eq!(m.feed(1), Some(MousePacket { dx: 1, dy: -2, buttons: 0, dz: 1 }));
        let p = [0x0A, 0, 0, 0xFF].map(|b| m.feed(b));
        assert_eq!(p[3], Some(MousePacket { dx: 0, dy: 0, buttons: 2, dz: -1 }));
        let p = [0x08, 0, 0, 0xFD].map(|b| m.feed(b));
        assert_eq!(p[3].unwrap().dz, -3);
        // Sin rueda, el decodificador sigue en 3 bytes y dz = 0.
        m.set_wheel(false);
        m.feed(0x08);
        m.feed(0);
        assert_eq!(m.feed(0).unwrap().dz, 0);
    }

    #[test]
    fn raton_botones_resync_desbordamiento() {
        let mut m = Ps2Mouse::new();
        for (b, esperado) in [(0x09, 1), (0x0A, 2), (0x0C, 4), (0x0F, 7)] {
            m.feed(b);
            m.feed(0);
            assert_eq!(m.feed(0).unwrap().buttons, esperado);
        }
        // Un byte suelto sin el bit 3 se descarta y se recupera la alineación.
        assert_eq!(m.feed(0x00), None);
        m.feed(0x08);
        m.feed(1);
        assert_eq!(m.feed(2), Some(MousePacket { dx: 1, dy: -2, buttons: 0, dz: 0 }));
        // Desbordamiento: sin movimiento, con botones.
        for ov in [0x40, 0x80, 0xC0] {
            m.feed(0x08 | 0x01 | ov);
            m.feed(100);
            let p = m.feed(100).unwrap();
            assert_eq!(p, MousePacket { dx: 0, dy: 0, buttons: 1, dz: 0 });
        }
    }
}
