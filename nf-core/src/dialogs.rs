//! Cuadros de diálogo modales. CONTRATO: estos tipos no cambian. Una app pide un diálogo con
//! `cx.dialog = Some(..)` y recibe la respuesta en `App::dialog_result`. `tag` lo elige la app
//! para saber a qué petición corresponde la respuesta.
use alloc::string::String;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Buttons {
    /// "Aceptar".
    Ok,
    /// "Aceptar" y "Cancelar".
    OkCancel,
    /// "Sí" y "No".
    YesNo,
    /// "Sí", "No" y "Cancelar".
    YesNoCancel,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    Ok,
    Yes,
    No,
    /// También al cerrar el diálogo con su aspa o con Escape.
    Cancel,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DialogRequest {
    /// Mensaje o pregunta.
    Message { tag: u32, title: String, text: String, buttons: Buttons },
    /// Pide una línea de texto (p. ej. un nombre). `value` es el texto inicial.
    Input { tag: u32, title: String, label: String, value: String },
    /// Elegir un archivo existente. `dir` es la carpeta inicial ("C:\\Documentos").
    Open { tag: u32, title: String, dir: String },
    /// Elegir dónde guardar. `name` es el nombre propuesto. Si el archivo ya existe, el propio
    /// diálogo pregunta si se reemplaza antes de responder.
    Save { tag: u32, title: String, dir: String, name: String },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DialogResult {
    /// Respuesta a `Message`.
    Answer { tag: u32, answer: Answer },
    /// Respuesta a `Input`; `None` si se canceló.
    Text { tag: u32, value: Option<String> },
    /// Respuesta a `Open` y `Save`: ruta absoluta; `None` si se canceló.
    Path { tag: u32, path: Option<String> },
}
