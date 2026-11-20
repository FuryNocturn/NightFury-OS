// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Contrato entre el núcleo y los programas en modo usuario (F3, v0.8). CONTRATO: los números de
//! llamada, `WireEvent`, sus `kind` y las constantes de memoria no cambian sin subir `ABI_VERSION`.
//!
//! Llamada: `syscall` con el número en `rax` y los argumentos en `rdi, rsi, rdx, r10, r8`.
//! Vuelve en `rax`: >= 0 bien, < 0 un `E_*`. Se conservan todos los registros salvo `rax, rcx, r11`.
//! Arranque: el programa empieza en su punto de entrada sin argumentos, con `rsp` = `STACK_TOP`
//! (alineada a 16). Lo primero que recibe al llamar a `WAIT_EVENT` es `EV_THEME`, `EV_RESIZE` y
//! `EV_FOCUS`. Para cerrarse llama a `EXIT`; `EV_CLOSE` solo pide que lo haga.
use crate::apps::MouseEv;
use crate::input::{Key, KeyEvent};

pub const ABI_VERSION: u16 = 1;

/// Zona del programa: una entrada entera de la tabla de páginas de nivel 4 (índice 32, 512 GiB).
/// El núcleo vive por debajo (identidad, solo supervisor).
pub const USER_BASE: u64 = 0x0000_1000_0000_0000;
pub const USER_TOP: u64 = USER_BASE + (1 << 39);
/// Pila: crece hacia abajo desde `STACK_TOP`; por debajo de `STACK_TOP - STACK_SIZE` queda sin
/// mapear (página de guarda). Por encima de `STACK_TOP` hay otra página sin mapear.
pub const STACK_SIZE: u64 = 1 << 20;
pub const STACK_TOP: u64 = USER_TOP - 0x1000;
/// Las secciones del `.nfx` van por debajo de `IMAGE_TOP`; el montón (`MEM_GROW`) empieza en
/// `HEAP_BASE`.
pub const IMAGE_TOP: u64 = USER_BASE + (1 << 32);
pub const HEAP_BASE: u64 = USER_BASE + (1 << 36);
/// Memoria máxima de un programa (imagen + montón + pila).
pub const MAX_MEM: u64 = 256 << 20;
pub const PAGE: u64 = 0x1000;

// ---------- Números de llamada ----------
/// `EXIT(code)`: termina el programa. No vuelve.
pub const SYS_EXIT: u64 = 0;
/// `LOG(ptr, len)`: texto UTF-8 al registro de depuración (puerto serie). Máx. `LOG_MAX` bytes.
pub const SYS_LOG: u64 = 1;
/// `WAIT_EVENT(ptr, timeout_ms)`: espera el siguiente evento y lo escribe en `ptr`
/// (`WireEvent`, `WIRE_EVENT_SIZE` bytes). `timeout_ms = u64::MAX` = sin límite. Si vence, escribe
/// `EV_NONE`. Devuelve 0.
pub const SYS_WAIT_EVENT: u64 = 2;
/// `PRESENT(ptr, w, h, stride)`: entrega la imagen del contenido, `h` filas de `stride` píxeles
/// u32 0x00RRGGBB (`stride >= w`). `w`, `h` hasta `MAX_DIM`.
pub const SYS_PRESENT: u64 = 3;
/// `SET_TITLE(ptr, len)`: título de la ventana, UTF-8, máx. `TITLE_MAX` bytes.
pub const SYS_SET_TITLE: u64 = 4;
/// `MEM_GROW(bytes)`: amplía el montón en `bytes` (se redondea a páginas, a cero). Devuelve la
/// dirección donde empieza lo nuevo. `MEM_GROW(0)` devuelve el final actual.
pub const SYS_MEM_GROW: u64 = 5;
/// `UPTIME()`: milisegundos desde el arranque.
pub const SYS_UPTIME: u64 = 6;
/// `DATETIME(ptr)`: fecha y hora local en `ptr`, 8 bytes: año u16, mes, día, hora, min, seg, 0.
pub const SYS_DATETIME: u64 = 7;
/// `SET_TIMER(ms)`: pide un `EV_TIMER` dentro de `ms` (sustituye al anterior; 0 = quitarlo).
pub const SYS_SET_TIMER: u64 = 8;
/// `SET_MIN_SIZE(w, h)`: tamaño mínimo del contenido.
pub const SYS_SET_MIN_SIZE: u64 = 9;
/// `SAVE_AS(ptr, len, name_ptr, name_len)` (v0.9): el sistema pregunta al usuario dónde guardar esos
/// `len` bytes (máx. `SAVE_MAX`) proponiendo el nombre dado (UTF-8, sin `\`, `/` ni `:`) y los
/// escribe con sus permisos. El programa nunca ve la ruta. La respuesta llega como `EV_SAVED`.
pub const SYS_SAVE_AS: u64 = 10;
pub const SAVE_MAX: u64 = 32 << 20;
/// `SERVICE(ptr, len)` (v0.10): petición al sistema codificada con `wire` (archivos, cuentas, datos
/// del equipo; ver `service`). El programa queda parado hasta que el escritorio la atiende y vuelve
/// con la longitud de la respuesta (>= 0), que se recoge con `TAKE_REPLY`. Máx. `SERVICE_MAX` bytes.
pub const SYS_SERVICE: u64 = 11;
/// `TAKE_REPLY(ptr, cap)` (v0.10): copia la respuesta del último `SERVICE` y la descarta. Devuelve su
/// longitud; `E_INVAL` si `cap` no llega (la respuesta se conserva) o no hay ninguna.
pub const SYS_TAKE_REPLY: u64 = 12;
/// `REQUEST(ptr, len)` (v0.10): petición al escritorio codificada con `wire` (abrir una ruta, un
/// diálogo, un menú, empezar a arrastrar, cambiar el tema, respuestas de arrastre). No espera: lo que
/// conteste llega como evento. Máx. `REQUEST_MAX` bytes.
pub const SYS_REQUEST: u64 = 13;
/// `EVENT_DATA(ptr, cap)` (v0.10): datos del último evento entregado que los lleve (`d` = su
/// longitud). Valen hasta el siguiente `WAIT_EVENT`. Devuelve la longitud; `E_INVAL` si `cap` no llega.
pub const SYS_EVENT_DATA: u64 = 14;
/// Tamaño máximo de una petición o respuesta de `SERVICE` (cabe un `SAVE_MAX` con su ruta).
pub const SERVICE_MAX: u64 = SAVE_MAX + (1 << 20);
pub const REQUEST_MAX: u64 = 1 << 20;

pub const E_INVAL: i64 = -1;
/// Puntero fuera de la zona del programa o sin mapear.
pub const E_FAULT: i64 = -2;
pub const E_NOMEM: i64 = -3;
pub const E_NOSYS: i64 = -4;
/// Ya hay una petición igual en curso (p. ej. otro `SAVE_AS` sin responder).
pub const E_BUSY: i64 = -5;

pub const LOG_MAX: usize = 1024;
pub const TITLE_MAX: usize = 256;
pub const MAX_DIM: u64 = 4096;

// ---------- Eventos ----------
pub const EV_NONE: u32 = 0;
/// Tecla pulsada: `a` = código (`key_code`), `d` = carácter (0 si no hay), `flags` = `F_*`.
pub const EV_KEY: u32 = 1;
/// Ratón (`a`, `b` = x, y relativos al contenido).
pub const EV_MOUSE_DOWN: u32 = 2;
pub const EV_MOUSE_UP: u32 = 3;
pub const EV_MOUSE_MOVE: u32 = 4;
pub const EV_MOUSE_RIGHT: u32 = 5;
/// `c` = muescas (positivo = hacia abajo).
pub const EV_MOUSE_WHEEL: u32 = 6;
pub const EV_MOUSE_LEAVE: u32 = 7;
/// Tamaño del contenido: `a` = ancho, `b` = alto. El programa debe volver a dibujar (`PRESENT`).
pub const EV_RESIZE: u32 = 8;
/// `a` = 1 con foco, 0 sin foco.
pub const EV_FOCUS: u32 = 9;
/// `d` = color de énfasis, `a` = fondo.
pub const EV_THEME: u32 = 10;
pub const EV_TIMER: u32 = 11;
/// El usuario quiere cerrar la ventana.
pub const EV_CLOSE: u32 = 12;
/// Resultado de `SAVE_AS`: `a` = 1 guardado, 0 cancelado, -1 error.
pub const EV_SAVED: u32 = 13;
/// Eventos de la v0.10. Los que llevan datos (`wire`) ponen su longitud en `d` y se leen con
/// `EVENT_DATA`.
/// Respuesta a un diálogo pedido con `REQUEST`; datos: `DialogResult`.
pub const EV_DIALOG: u32 = 14;
/// Elemento elegido en un menú: `a` = tag, `b` = id.
pub const EV_MENU: u32 = 15;
/// Se arrastran archivos sobre la ventana: `a, b` = punto, `c` = 1 dentro / 0 salió o terminó;
/// datos: las rutas. El programa contesta con `Request::DragAnswer`.
pub const EV_DRAG_OVER: u32 = 16;
/// El escritorio pregunta qué se arrastraría con el botón derecho desde `a, b`. Se contesta con
/// `Request::DragSourceAnswer`.
pub const EV_DRAG_SOURCE: u32 = 17;
/// Archivos soltados en `a, b`; `c` = 1 copiar; datos: las rutas.
pub const EV_DROP: u32 = 18;
/// Abrir una ruta en esta ventana (`App::open_path`); datos: la ruta.
pub const EV_OPEN: u32 = 19;
/// Estado de un mando (solo a la ventana activa, cuando cambia y al ganar el foco). `a` = id del
/// mando, `b` = botones (`gamepad::BTN_*`), y los ejes de dos en dos como i16 (bajo | alto << 16):
/// `c` = LX, LY; `d` = RX, RY; `flags` = LT, RT. Ver `WireEvent::gamepad` y `to_gamepad`.
/// Al perder el foco, el programa debe dar todo por suelto.
pub const EV_GAMEPAD: u32 = 20;
/// Lista de mandos conectados (lleva datos: `Vec<gamepad::PadInfo>`). Llega a todas las ventanas
/// al abrirse y cada vez que se conecta o quita uno.
pub const EV_GAMEPADS: u32 = 21;

pub const F_SHIFT: u32 = 1;
pub const F_CTRL: u32 = 2;
pub const F_ALT: u32 = 4;

/// Evento tal cual viaja a la memoria del programa (32 bytes, little endian, sin huecos).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct WireEvent {
    pub kind: u32,
    pub a: i32,
    pub b: i32,
    pub c: i32,
    pub d: u32,
    pub flags: u32,
    /// Reloj (`UPTIME`) al entregar el evento.
    pub now_ms: u64,
}

pub const WIRE_EVENT_SIZE: usize = 32;

impl WireEvent {
    pub fn new(kind: u32, now_ms: u64) -> Self {
        WireEvent { kind, now_ms, ..Default::default() }
    }

    pub fn to_bytes(&self) -> [u8; WIRE_EVENT_SIZE] {
        let mut o = [0u8; WIRE_EVENT_SIZE];
        o[0..4].copy_from_slice(&self.kind.to_le_bytes());
        o[4..8].copy_from_slice(&self.a.to_le_bytes());
        o[8..12].copy_from_slice(&self.b.to_le_bytes());
        o[12..16].copy_from_slice(&self.c.to_le_bytes());
        o[16..20].copy_from_slice(&self.d.to_le_bytes());
        o[20..24].copy_from_slice(&self.flags.to_le_bytes());
        o[24..32].copy_from_slice(&self.now_ms.to_le_bytes());
        o
    }

    pub fn from_bytes(b: &[u8; WIRE_EVENT_SIZE]) -> Self {
        let u = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let mut t = [0u8; 8];
        t.copy_from_slice(&b[24..32]);
        WireEvent { kind: u(0), a: u(4) as i32, b: u(8) as i32, c: u(12) as i32, d: u(16), flags: u(20), now_ms: u64::from_le_bytes(t) }
    }

    pub fn key(ev: &KeyEvent, now_ms: u64) -> Self {
        let mut flags = 0;
        if ev.shift {
            flags |= F_SHIFT;
        }
        if ev.ctrl {
            flags |= F_CTRL;
        }
        if ev.alt {
            flags |= F_ALT;
        }
        let (code, kc) = key_code(ev.key);
        let d = ev.ch.or(kc).map_or(0, |c| c as u32);
        WireEvent { kind: EV_KEY, a: code, d, flags, now_ms, ..Default::default() }
    }

    /// `EV_GAMEPAD` del mando `id`.
    pub fn gamepad(id: u32, st: &crate::gamepad::PadState, now_ms: u64) -> Self {
        let pair = |i: usize| (st.axes[i] as u16 as u32 | (st.axes[i + 1] as u16 as u32) << 16) as i32;
        WireEvent { kind: EV_GAMEPAD, a: id as i32, b: st.buttons as i32, c: pair(0), d: pair(2) as u32, flags: pair(4) as u32, now_ms }
    }

    /// Vuelve a (id, estado). `None` si no es `EV_GAMEPAD`.
    pub fn to_gamepad(&self) -> Option<(u32, crate::gamepad::PadState)> {
        if self.kind != EV_GAMEPAD {
            return None;
        }
        let (c, d, f) = (self.c as u32, self.d, self.flags);
        let lo = |v: u32| v as u16 as i16;
        let hi = |v: u32| (v >> 16) as u16 as i16;
        Some((self.a as u32, crate::gamepad::PadState { buttons: self.b as u32, axes: [lo(c), hi(c), lo(d), hi(d), lo(f), hi(f)] }))
    }

    /// Vuelve a `KeyEvent` (siempre `down = true`). `None` si no es `EV_KEY` o el código no existe.
    pub fn to_key(&self) -> Option<KeyEvent> {
        if self.kind != EV_KEY {
            return None;
        }
        let ch = char::from_u32(self.d).filter(|_| self.d != 0);
        let key = key_from_code(self.a, ch)?;
        Some(KeyEvent { key, ch, down: true, shift: self.flags & F_SHIFT != 0, ctrl: self.flags & F_CTRL != 0, alt: self.flags & F_ALT != 0 })
    }

    pub fn mouse(ev: MouseEv, now_ms: u64) -> Self {
        let (kind, a, b, c) = match ev {
            MouseEv::Down { x, y } => (EV_MOUSE_DOWN, x, y, 0),
            MouseEv::Up { x, y } => (EV_MOUSE_UP, x, y, 0),
            MouseEv::Move { x, y } => (EV_MOUSE_MOVE, x, y, 0),
            MouseEv::RightDown { x, y } => (EV_MOUSE_RIGHT, x, y, 0),
            MouseEv::Wheel { x, y, dz } => (EV_MOUSE_WHEEL, x, y, dz),
            MouseEv::Leave => (EV_MOUSE_LEAVE, 0, 0, 0),
        };
        WireEvent { kind, a, b, c, now_ms, ..Default::default() }
    }

    pub fn to_mouse(&self) -> Option<MouseEv> {
        let (x, y) = (self.a, self.b);
        Some(match self.kind {
            EV_MOUSE_DOWN => MouseEv::Down { x, y },
            EV_MOUSE_UP => MouseEv::Up { x, y },
            EV_MOUSE_MOVE => MouseEv::Move { x, y },
            EV_MOUSE_RIGHT => MouseEv::RightDown { x, y },
            EV_MOUSE_WHEEL => MouseEv::Wheel { x, y, dz: self.c },
            EV_MOUSE_LEAVE => MouseEv::Leave,
            _ => return None,
        })
    }
}

/// Código de tecla y, para `Key::Char`, su carácter.
fn key_code(k: Key) -> (i32, Option<char>) {
    match k {
        Key::Char(c) => (0, Some(c)),
        Key::Escape => (1, None),
        Key::Backspace => (2, None),
        Key::Tab => (3, None),
        Key::Enter => (4, None),
        Key::Left => (5, None),
        Key::Right => (6, None),
        Key::Up => (7, None),
        Key::Down => (8, None),
        Key::Home => (9, None),
        Key::End => (10, None),
        Key::PageUp => (11, None),
        Key::PageDown => (12, None),
        Key::Insert => (13, None),
        Key::Delete => (14, None),
        Key::Super => (15, None),
        Key::Other => (16, None),
        Key::F(n) => (100 + n as i32, None),
    }
}

fn key_from_code(code: i32, ch: Option<char>) -> Option<Key> {
    Some(match code {
        0 => Key::Char(ch?),
        1 => Key::Escape,
        2 => Key::Backspace,
        3 => Key::Tab,
        4 => Key::Enter,
        5 => Key::Left,
        6 => Key::Right,
        7 => Key::Up,
        8 => Key::Down,
        9 => Key::Home,
        10 => Key::End,
        11 => Key::PageUp,
        12 => Key::PageDown,
        13 => Key::Insert,
        14 => Key::Delete,
        15 => Key::Super,
        16 => Key::Other,
        101..=124 => Key::F((code - 100) as u8),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evento_ida_y_vuelta() {
        let e = WireEvent { kind: EV_MOUSE_WHEEL, a: -5, b: 7, c: 3, d: 0xABCD, flags: 6, now_ms: 0x1122_3344_5566 };
        assert_eq!(WireEvent::from_bytes(&e.to_bytes()), e);
        assert_eq!(core::mem::size_of::<WireEvent>(), WIRE_EVENT_SIZE);
    }

    #[test]
    fn mando_ida_y_vuelta() {
        use crate::gamepad::*;
        let st = PadState { buttons: BTN_SOUTH | BTN_EXTRA | BTN_RIGHT, axes: [-32768, 32767, -1, 1, 0, 32767] };
        let e = WireEvent::gamepad(7, &st, 99);
        assert_eq!((e.kind, e.a, e.now_ms), (EV_GAMEPAD, 7, 99));
        assert_eq!(WireEvent::from_bytes(&e.to_bytes()).to_gamepad(), Some((7, st)));
        assert_eq!(WireEvent::new(EV_KEY, 0).to_gamepad(), None);
    }

    #[test]
    fn teclas_ida_y_vuelta() {
        let keys = [Key::Char('ñ'), Key::Escape, Key::Enter, Key::Delete, Key::F(1), Key::F(12), Key::Super, Key::Other];
        for k in keys {
            let ch = if let Key::Char(c) = k { Some(c) } else { None };
            let ev = KeyEvent { key: k, ch, down: true, shift: true, ctrl: false, alt: true };
            assert_eq!(WireEvent::key(&ev, 9).to_key(), Some(ev), "{k:?}");
        }
        let enter = KeyEvent { key: Key::Enter, ch: Some('\n'), down: true, shift: false, ctrl: true, alt: false };
        assert_eq!(WireEvent::key(&enter, 0).to_key(), Some(enter));
        // Códigos inventados o un Char sin carácter no se aceptan.
        assert_eq!(WireEvent { kind: EV_KEY, a: 99, ..Default::default() }.to_key(), None);
        assert_eq!(WireEvent { kind: EV_KEY, a: 0, d: 0, ..Default::default() }.to_key(), None);
        assert_eq!(WireEvent { kind: EV_KEY, a: 0, d: 0xD800, ..Default::default() }.to_key(), None);
    }

    #[test]
    fn raton_ida_y_vuelta() {
        for m in [MouseEv::Down { x: 1, y: 2 }, MouseEv::Up { x: -1, y: 0 }, MouseEv::Move { x: 3, y: 4 }, MouseEv::RightDown { x: 5, y: 6 }, MouseEv::Wheel { x: 7, y: 8, dz: -2 }, MouseEv::Leave] {
            assert_eq!(WireEvent::mouse(m, 0).to_mouse(), Some(m));
        }
        assert_eq!(WireEvent::new(EV_RESIZE, 0).to_mouse(), None);
    }

    #[test]
    fn zonas_de_memoria_ordenadas() {
        assert!(USER_BASE < IMAGE_TOP && IMAGE_TOP < HEAP_BASE && HEAP_BASE + MAX_MEM < STACK_TOP - STACK_SIZE);
        assert_eq!(USER_BASE >> 39, 32);
        assert_eq!((USER_TOP - 1) >> 39, 32);
    }
}
