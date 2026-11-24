// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Páginas que genera el propio navegador (errores y texto plano) como HTML para el motor web.
use alloc::format;
use alloc::string::String;
use nf_core::net::NetError;

/// Escapa lo que va dentro de HTML (el texto y las URL vienen de fuera).
pub fn escapar(s: &str) -> String {
    let mut r = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => r.push_str("&amp;"),
            '<' => r.push_str("&lt;"),
            '>' => r.push_str("&gt;"),
            '"' => r.push_str("&quot;"),
            c => r.push(c),
        }
    }
    r
}

/// Página de error: título, explicación y la dirección que falló.
pub fn error(titulo: &str, detalle: &str, url: &str) -> String {
    format!(
        "<html><head><title>{t}</title><style>\
body {{ font-family: sans-serif; background: #ffffff; color: #202124 }}\
.c {{ max-width: 560px; margin: 90px auto 0 auto; padding: 0 24px }}\
h1 {{ font-size: 24px; font-weight: normal; margin: 0 0 16px 0 }}\
p {{ color: #5f6368; font-size: 15px; margin: 8px 0 }}\
.u {{ color: #80868b; font-size: 13px; margin-top: 28px }}\
.i {{ color: #c5221f; font-size: 13px; margin-bottom: 8px }}\
</style></head><body><div class=c><div class=i>Error</div><h1>{t}</h1><p>{d}</p><p class=u>{u}</p></div></body></html>",
        t = escapar(titulo),
        d = escapar(detalle),
        u = escapar(url)
    )
}

/// Un texto plano como `<pre>` (el título es la dirección).
pub fn texto_plano(titulo: &str, texto: &str) -> String {
    format!("<html><head><title>{}</title></head><body><pre>{}</pre></body></html>", escapar(titulo), escapar(texto))
}

/// Título y explicación de un fallo de red.
pub fn error_de_red(e: &NetError, url: &str) -> (String, String) {
    let host = nf_core::net::http::parse_url(url).map(|u| u.host).unwrap_or_default();
    let (t, d): (&str, String) = match e {
        NetError::NotFound => ("No se encuentra el servidor", format!("No se ha podido encontrar la dirección de «{host}». Comprueba que está bien escrita y que hay conexión a Internet.")),
        NetError::Timeout => ("El servidor tarda demasiado en responder", format!("«{host}» no ha respondido a tiempo. Puede estar caído o la conexión es muy lenta.")),
        NetError::Refused => ("El servidor ha rechazado la conexión", format!("«{host}» está en marcha pero no acepta conexiones en ese puerto.")),
        NetError::Reset => ("Se ha interrumpido la conexión", String::from("El servidor ha cortado la conexión antes de terminar.")),
        NetError::Unreachable(_) | NetError::TtlExpired(_) => ("No se puede llegar al servidor", format!("No hay ruta hasta «{host}». Revisa la conexión de red.")),
        NetError::NoAdapter | NetError::NoCable | NetError::NoAddress => ("Sin conexión de red", String::from("Este equipo no está conectado a ninguna red. Revisa el cable y la configuración de red.")),
        NetError::BadUrl => ("La dirección no es válida", String::from("No se ha podido entender la dirección. Solo se admiten direcciones http y https.")),
        NetError::Tls(m) => ("La conexión no es privada", format!("No se puede garantizar que «{host}» sea quien dice ser, así que no se abre. {m}")),
        NetError::Protocol(m) => ("Respuesta no válida del servidor", format!("El servidor ha enviado una respuesta que no se entiende: {m}.")),
        NetError::Denied | NetError::Pending => ("Sin permiso para usar la red", String::from("Este programa no tiene permiso para usar la red. Puedes cambiarlo en Configuración → Red.")),
        NetError::Busy | NetError::Gone | NetError::InUse => ("La red está ocupada", String::from("Hay demasiadas conexiones abiertas. Vuelve a intentarlo en unos segundos.")),
    };
    (String::from(t), d)
}

/// Título y explicación de una respuesta HTTP de error sin contenido.
pub fn error_http(codigo: u16, motivo: &str) -> (String, String) {
    let t = match codigo {
        404 | 410 => "No se encuentra la página",
        401 | 403 => "No tienes acceso a esta página",
        500..=599 => "El servidor tiene un problema",
        _ => "No se puede abrir la página",
    };
    (String::from(t), format!("El servidor ha respondido con el código {codigo} {motivo}."))
}
