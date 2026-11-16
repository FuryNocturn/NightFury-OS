//! Menú contextual (clic derecho). CONTRATO: estos tipos no cambian.
//! Lo pide una app con `cx.menu = Some(MenuRequest { .. })` y la elección llega por
//! `App::menu_result(tag, id, cx)`. Lo dibuja el escritorio, que también lo usa para sus menús.
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct MenuItem {
    /// Texto visible. Se ignora si `separator`.
    pub text: String,
    /// Identificador que se devuelve al elegirlo (lo decide quien pide el menú).
    pub id: u32,
    /// Acción por defecto: se dibuja en negrita.
    pub bold: bool,
    /// En gris y no se puede elegir.
    pub disabled: bool,
    /// Marca a la izquierda (opción activa de "Ver" u "Ordenar por").
    pub checked: bool,
    /// Atajo que se muestra a la derecha, p. ej. "Ctrl+Z". Vacío = ninguno.
    pub shortcut: &'static str,
    /// Línea separadora.
    pub separator: bool,
    /// Si no está vacío, el elemento abre este submenú (un solo nivel) y no se elige él mismo.
    pub sub: Vec<MenuItem>,
}

impl MenuItem {
    pub fn new(id: u32, text: &str) -> MenuItem {
        MenuItem { id, text: String::from(text), ..Default::default() }
    }
    pub fn sep() -> MenuItem {
        MenuItem { separator: true, ..Default::default() }
    }
    pub fn submenu(text: &str, sub: Vec<MenuItem>) -> MenuItem {
        MenuItem { text: String::from(text), sub, ..Default::default() }
    }
    pub fn bold(mut self) -> MenuItem {
        self.bold = true;
        self
    }
    pub fn disabled(mut self, d: bool) -> MenuItem {
        self.disabled = d;
        self
    }
    pub fn checked(mut self, c: bool) -> MenuItem {
        self.checked = c;
        self
    }
    pub fn shortcut(mut self, s: &'static str) -> MenuItem {
        self.shortcut = s;
        self
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MenuRequest {
    /// Para que la app sepa a qué menú responde.
    pub tag: u32,
    /// Esquina superior izquierda deseada, en coordenadas del contenido de la ventana que lo pide
    /// (el escritorio lo recoloca para que no se salga de la pantalla).
    pub x: i32,
    pub y: i32,
    pub items: Vec<MenuItem>,
}
