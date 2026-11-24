// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Descargas de la ventana: cuándo una respuesta se descarga en vez de mostrarse, el nombre del archivo
//! (Content-Disposition, RFC 6266 y 5987, o la URL), el aviso previo, el avance y la escritura en Descargas.
//! Las descargas pertenecen a la ventana: siguen aunque la pestaña navegue o se cierre.
use crate::entrada::{tipo_vista, TipoVista};
use crate::paginas;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use nf_core::files::{join, Std};
use nf_core::fsapi::FilesApi;
use nf_core::net::http::{HttpEvent, HttpGet};
use nf_core::net::NetApi;
use nf_core::platform::{DateTime, Platform};

/// Lo máximo que se acumula en memoria por descarga.
// shortcut: FilesApi solo escribe archivos enteros, así que se acumula todo en memoria (64 MiB como mucho) y se
// escribe al final; escribir por partes cuando FilesApi pueda añadir a un archivo.
pub const TOPE_MEM: usize = 64 << 20;
const MAX_NOMBRE: usize = 120;
const NOMBRE_VACIO: &str = "descarga";

// ------------------------------------------------------------ qué se descarga y cómo se llama

/// ¿La respuesta (2xx con contenido) se guarda en vez de mostrarse? Sí si el enlace lo pide (`download`), si trae
/// `Content-Disposition: attachment` o si su tipo no es de los que se muestran. Sin `Content-Type` se mira el contenido.
pub fn es_descarga(forzar: bool, status: u16, cd: Option<&str>, ct: Option<&str>) -> bool {
    if !(200..300).contains(&status) || status == 204 || status == 205 {
        return false;
    }
    forzar || cd.is_some_and(|c| c.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("attachment")) || (ct.is_some() && tipo_vista(ct, &[]) == TipoVista::Otro)
}

fn decodificar_pct(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut r = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (b[i], s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(v)) => {
                r.push(v);
                i += 3;
            }
            (c, _) => {
                r.push(c);
                i += 1;
            }
        }
    }
    r
}

/// Parámetros `nombre=valor` de una cabecera `Content-Disposition` (nombre en minúsculas, valor sin comillas).
fn parametros(cd: &str) -> Vec<(String, String)> {
    let mut v = Vec::new();
    let mut it = cd.chars().peekable();
    // El tipo («attachment», «inline») va antes del primer «;».
    while it.next_if(|&c| c != ';').is_some() {}
    while it.peek().is_some() {
        while it.next_if(|&c| c == ';' || c.is_whitespace()).is_some() {}
        let mut nombre = String::new();
        while let Some(c) = it.next_if(|&c| c != '=' && c != ';') {
            nombre.push(c);
        }
        let mut valor = String::new();
        if it.next_if_eq(&'=').is_some() {
            if it.next_if_eq(&'"').is_some() {
                while let Some(c) = it.next() {
                    match c {
                        '"' => break,
                        // Solo `\"` es un escape; otra barra se queda (`C:\x\y.exe` sigue siendo una ruta, que `sanear` recorta).
                        '\\' if it.peek() == Some(&'"') => valor.extend(it.next()),
                        '\\' if it.peek() == Some(&'\\') => valor.extend(it.next()),
                        c => valor.push(c),
                    }
                }
            } else {
                while let Some(c) = it.next_if(|&c| c != ';') {
                    valor.push(c);
                }
            }
        }
        let nombre = nombre.trim().to_ascii_lowercase();
        if !nombre.is_empty() {
            v.push((nombre, String::from(valor.trim())));
        }
    }
    v
}

/// `filename*=UTF-8''%C3%B1.txt` (RFC 5987): solo UTF-8 e ISO-8859-1.
fn nombre_extendido(v: &str) -> Option<String> {
    let mut p = v.splitn(3, '\'');
    let (charset, _idioma, texto) = (p.next()?, p.next()?, p.next()?);
    let bytes = decodificar_pct(texto);
    if charset.eq_ignore_ascii_case("utf-8") {
        String::from_utf8(bytes).ok()
    } else if charset.eq_ignore_ascii_case("iso-8859-1") {
        Some(bytes.iter().map(|&b| b as char).collect())
    } else {
        None
    }
}

/// Nombre limpio para guardar: sin rutas ni caracteres que Windows no admite, sin `..`, sin nombres reservados,
/// de 120 caracteres como mucho (conservando la extensión). Nunca vacío.
pub fn sanear(bruto: &str) -> String {
    // Una ruta («a/b\c.txt») se queda con el último trozo.
    let ultimo = bruto.rsplit(['/', '\\']).next().unwrap_or("");
    let mut s = String::new();
    for c in ultimo.chars() {
        let c = if c.is_control() || "\\/:*?\"<>|".contains(c) { '_' } else { c };
        // Una racha de puntos («..») se queda en uno.
        if c == '.' && s.ends_with('.') {
            continue;
        }
        s.push(c);
    }
    // Windows no admite puntos ni espacios al final.
    let s = s.trim_start().trim_end_matches(['.', ' ']);
    if s.is_empty() {
        return String::from(NOMBRE_VACIO);
    }
    let mut s = String::from(s);
    if s.chars().count() > MAX_NOMBRE {
        let ext = s.rfind('.').filter(|&i| i > 0 && s[i..].chars().count() <= 20).map_or("", |i| &s[i..]);
        let base: String = s.chars().take(MAX_NOMBRE - ext.chars().count()).collect();
        s = format!("{base}{ext}");
    }
    // CON, PRN, AUX, NUL, COM1-9 y LPT1-9 (con cualquier extensión) no se pueden crear en Windows.
    let raiz = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reservado = matches!(raiz.as_str(), "CON" | "PRN" | "AUX" | "NUL") || ((raiz.starts_with("COM") || raiz.starts_with("LPT")) && raiz.len() == 4 && matches!(raiz.as_bytes()[3], b'1'..=b'9'));
    if reservado {
        s.insert(0, '_');
    }
    s
}

/// Nombre del archivo: `filename*`, `filename` o el último trozo de la ruta de la URL; saneado.
pub fn nombre_descarga(cd: Option<&str>, url: &str) -> String {
    let p = cd.map(parametros).unwrap_or_default();
    let de = |n: &str| p.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    if let Some(n) = de("filename*").and_then(|v| nombre_extendido(&v)).or_else(|| de("filename")).filter(|n| !n.trim().is_empty()) {
        return sanear(&n);
    }
    let ruta = url.split(['#', '?']).next().unwrap_or("");
    let ruta = ruta.split_once("://").map_or(ruta, |(_, r)| r.split_once('/').map_or("", |(_, r)| r));
    sanear(&String::from_utf8_lossy(&decodificar_pct(ruta.rsplit('/').next().unwrap_or(""))))
}

/// «1,5 MB», «300 B»…
pub fn tamano(n: u64) -> String {
    const U: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let (mut v, mut i) = (n as f64 / 1024.0, 0);
    while v >= 1024.0 && i + 1 < U.len() {
        v /= 1024.0;
        i += 1;
    }
    // Un decimal, con coma; sin «,0».
    let d = (v * 10.0 + 0.5) as u64;
    if d % 10 == 0 || d >= 1000 {
        format!("{} {}", (d + 5) / 10, U[i])
    } else {
        format!("{},{} {}", d / 10, d % 10, U[i])
    }
}

/// Nombre libre en `dir`: «x.ext», «x (1).ext», «x (2).ext»… Nunca el de un archivo que existe.
pub fn nombre_libre(fs: &mut dyn FilesApi, dir: &str, nombre: &str) -> Option<String> {
    if !fs.exists(&join(dir, nombre)) {
        return Some(String::from(nombre));
    }
    let (base, ext) = match nombre.rfind('.') {
        Some(i) if i > 0 => nombre.split_at(i),
        _ => (nombre, ""),
    };
    (1..10_000).map(|n| format!("{base} ({n}){ext}")).find(|c| !fs.exists(&join(dir, c)))
}

// ------------------------------------------------------------ las descargas de la ventana

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Estado {
    /// Esperando el «Descargar» del usuario (la conexión queda parada).
    Pregunta,
    Bajando,
    Hecha,
    Cancelada,
    Fallo(String),
}

pub struct Item {
    pub id: u32,
    pub nombre: String,
    pub url: String,
    pub total: Option<u64>,
    pub recibido: u64,
    pub estado: Estado,
    /// Dónde quedó (al terminar).
    pub ruta: String,
    get: Option<HttpGet>,
    datos: Vec<u8>,
    /// El servidor ya envió todo.
    completo: bool,
}

impl Item {
    pub fn es_programa(&self) -> bool {
        self.nombre.to_ascii_lowercase().ends_with(".nfx")
    }

    pub fn viva(&self) -> bool {
        matches!(self.estado, Estado::Pregunta | Estado::Bajando)
    }

    pub fn sitio(&self) -> String {
        crate::datos::host(&self.url).unwrap_or_default()
    }

    /// Lo que se le pregunta al usuario.
    pub fn pregunta(&self) -> (String, Option<&'static str>) {
        let tam = self.total.map_or(String::from("tamaño desconocido"), tamano);
        (format!("¿Descargar {} ({tam}) de {}?", self.nombre, self.sitio()), self.es_programa().then_some("Es un programa. Ábrelo solo si confías en este sitio."))
    }

    /// Texto de estado de la tarjeta de la barra.
    pub fn texto_estado(&self) -> String {
        match &self.estado {
            Estado::Pregunta => String::from("Esperando…"),
            Estado::Bajando => match self.total {
                Some(t) => format!("{} de {}", tamano(self.recibido), tamano(t)),
                None => tamano(self.recibido),
            },
            Estado::Hecha => format!("Completada · {}", tamano(self.recibido)),
            Estado::Cancelada => String::from("Cancelada"),
            Estado::Fallo(m) => m.clone(),
        }
    }

    fn fallar(&mut self, motivo: &str) {
        self.estado = Estado::Fallo(String::from(motivo));
        self.datos = Vec::new();
        self.get = None;
    }

    /// Recoge lo que haya llegado.
    fn avanzar(&mut self, net: Option<&mut dyn NetApi>, ms: u64) {
        let (Some(g), Some(net)) = (self.get.as_mut(), net) else {
            return self.fallar("Se ha perdido la conexión de red.");
        };
        for ev in g.poll(net, ms) {
            match ev {
                HttpEvent::Body(b) => {
                    if self.datos.len() + b.len() > TOPE_MEM {
                        g.cancel(net);
                        return self.fallar("Supera los 64 MiB que admite el navegador.");
                    }
                    self.recibido += b.len() as u64;
                    self.datos.extend_from_slice(&b);
                }
                HttpEvent::Done => self.completo = true,
                HttpEvent::Error(e) => return self.fallar(&paginas::error_de_red(&e, &self.url).1),
                HttpEvent::Head { .. } | HttpEvent::Secure(_) => {}
            }
        }
    }

    /// Escribe el archivo en Descargas con un nombre libre.
    fn guardar(&mut self, fs: &mut dyn FilesApi, now: DateTime) {
        let dir = fs.std_dir(Std::Downloads);
        // Si la carpeta ya existe, falla y da igual.
        let _ = fs.mkdir(&dir, now);
        let Some(nombre) = nombre_libre(fs, &dir, &self.nombre) else {
            return self.fallar("No se ha encontrado un nombre libre en Descargas.");
        };
        let ruta = join(&dir, &nombre);
        match fs.write(&ruta, &self.datos, now) {
            Ok(()) => {
                self.ruta = ruta;
                self.estado = Estado::Hecha;
                self.datos = Vec::new();
                self.get = None;
            }
            Err(e) => self.fallar(&format!("No se ha podido guardar en Descargas: {}", e.message())),
        }
    }
}

#[derive(Default)]
pub struct Descargas {
    pub items: Vec<Item>,
    sig: u32,
    /// La barra de abajo se enseña (la cierra el usuario con la ×; una descarga nueva la abre).
    pub barra: bool,
}

impl Descargas {
    /// Una respuesta que se ha decidido descargar: queda esperando la respuesta del usuario. `previos` es lo que
    /// ya había llegado del cuerpo y `completo`, si el servidor había terminado.
    pub fn anadir(&mut self, get: HttpGet, nombre: String, url: String, total: Option<u64>, previos: Vec<u8>, completo: bool) {
        self.sig += 1;
        self.items.push(Item { id: self.sig, nombre, url, total, recibido: previos.len() as u64, estado: Estado::Pregunta, ruta: String::new(), get: Some(get), datos: previos, completo });
    }

    /// Una descarga que ni empieza (bloqueada, o sin red): sale en la barra como fallida.
    pub fn fallida(&mut self, nombre: String, url: String, motivo: &str) {
        self.sig += 1;
        self.items.push(Item { id: self.sig, nombre, url, total: None, recibido: 0, estado: Estado::Fallo(String::from(motivo)), ruta: String::new(), get: None, datos: Vec::new(), completo: false });
        self.barra = true;
    }

    /// La que espera respuesta (de una en una).
    pub fn pregunta(&self) -> Option<&Item> {
        self.items.iter().find(|i| i.estado == Estado::Pregunta)
    }

    /// El usuario responde a la pregunta: `Descargar` o `Cancelar` (que corta la conexión y no deja nada).
    pub fn responder(&mut self, sys: &mut dyn Platform, si: bool) {
        let Some(i) = self.items.iter().position(|i| i.estado == Estado::Pregunta) else { return };
        if si {
            self.items[i].estado = Estado::Bajando;
            self.barra = true;
            return;
        }
        let it = self.items.remove(i);
        if let (Some(mut g), Some(net)) = (it.get, sys.net()) {
            g.cancel(net);
        }
    }

    /// Cancela una descarga en curso (no guarda nada).
    pub fn cancelar(&mut self, sys: &mut dyn Platform, id: u32) {
        let Some(it) = self.items.iter_mut().find(|i| i.id == id && i.estado == Estado::Bajando) else { return };
        if let (Some(g), Some(net)) = (it.get.as_mut(), sys.net()) {
            g.cancel(net);
        }
        it.estado = Estado::Cancelada;
        it.datos = Vec::new();
        it.get = None;
    }

    /// Al cerrar la ventana: se corta todo lo que sigue en marcha.
    pub fn cortar_todo(&mut self, sys: &mut dyn Platform) {
        for it in self.items.iter_mut().filter(|i| i.viva()) {
            if let (Some(g), Some(net)) = (it.get.as_mut(), sys.net()) {
                g.cancel(net);
            }
            it.estado = Estado::Cancelada;
            it.datos = Vec::new();
            it.get = None;
        }
    }

    /// Avanza las que están bajando y guarda las que terminan. `true` si algo ha cambiado.
    pub fn tick(&mut self, sys: &mut dyn Platform, ms: u64) -> bool {
        let mut cambio = false;
        for i in 0..self.items.len() {
            if self.items[i].estado != Estado::Bajando {
                continue;
            }
            cambio = true;
            if !self.items[i].completo {
                self.items[i].avanzar(sys.net(), ms);
            }
            if self.items[i].estado == Estado::Bajando && self.items[i].completo {
                let now = sys.now();
                self.items[i].guardar(sys.fs(), now);
            }
        }
        cambio
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn mostrar_o_descargar() {
        let d = |forzar, st, cd, ct| es_descarga(forzar, st, cd, ct);
        assert!(!d(false, 200, None, Some("text/html; charset=utf-8")));
        assert!(!d(false, 200, None, Some("application/xhtml+xml")));
        assert!(!d(false, 200, None, Some("text/plain")));
        assert!(!d(false, 200, None, Some("image/png")) && !d(false, 200, None, Some("image/jpeg")) && !d(false, 200, None, Some("image/gif")));
        assert!(!d(false, 200, None, None), "sin tipo se mira el contenido: se muestra");
        assert!(d(false, 200, None, Some("application/pdf")));
        assert!(d(false, 200, None, Some("application/octet-stream")));
        assert!(d(false, 200, None, Some("image/svg+xml")));
        assert!(d(false, 200, Some("attachment; filename=a.txt"), Some("text/html")));
        assert!(d(false, 200, Some("Attachment"), None));
        assert!(!d(false, 200, Some("inline; filename=a.txt"), Some("text/html")));
        assert!(d(true, 200, None, Some("text/html")), "enlace con download");
        // Los errores se muestran como página, y 204 no trae nada que guardar.
        assert!(!d(true, 404, None, Some("application/pdf")) && !d(true, 204, None, None));
    }

    #[test]
    fn nombres_de_content_disposition() {
        let n = |cd: &str| nombre_descarga(Some(cd), "https://x.com/ruta/otro.bin?q=1");
        assert_eq!(n("attachment; filename=\"informe.pdf\""), "informe.pdf");
        assert_eq!(n("attachment; filename=informe.pdf"), "informe.pdf");
        assert_eq!(n("attachment; filename=\"a; b.txt\""), "a; b.txt", "el «;» entre comillas no corta");
        assert_eq!(n("attachment; filename=\"di\\\"cho.txt\""), "di\"cho.txt".replace('"', "_"));
        // filename* manda sobre filename (RFC 6266), en UTF-8 y en Latin-1.
        assert_eq!(n("attachment; filename=\"fallback.txt\"; filename*=UTF-8''a%C3%B1o%20nuevo.txt"), "año nuevo.txt");
        assert_eq!(n("attachment; filename*=iso-8859-1'es'a%F1o.txt"), "año.txt");
        // Con ruta: solo el último trozo.
        assert_eq!(n("attachment; filename=\"../../Windows/sistema.dll\""), "sistema.dll");
        assert_eq!(n("attachment; filename=\"C:\\Windows\\x.exe\""), "x.exe");
        // Sin nombre válido: la URL.
        assert_eq!(n("attachment"), "otro.bin");
        assert_eq!(n("attachment; filename=\"\""), "otro.bin");
        assert_eq!(nombre_descarga(None, "https://x.com/a/mi%20foto%C3%B1.png#z"), "mi fotoñ.png");
        assert_eq!(nombre_descarga(None, "https://x.com/"), "descarga");
        assert_eq!(nombre_descarga(None, "https://x.com"), "descarga");
    }

    #[test]
    fn nombres_saneados() {
        assert_eq!(sanear(".."), "descarga");
        assert_eq!(sanear("..."), "descarga");
        assert_eq!(sanear(""), "descarga");
        assert_eq!(sanear("   "), "descarga");
        assert_eq!(sanear("a..b.txt"), "a.b.txt");
        assert_eq!(sanear("a<b>c:d*e?f\"g|h.txt"), "a_b_c_d_e_f_g_h.txt");
        assert_eq!(sanear("con\u{7}trol\n.txt"), "con_trol_.txt");
        assert_eq!(sanear("fin. . "), "fin");
        for r in ["CON", "con.txt", "Nul.tar.gz", "COM1", "lpt9.log", "AUX"] {
            assert!(sanear(r).starts_with('_'), "{r}");
        }
        for ok in ["COM10.txt", "CONSOLA.txt", "com0", "auxiliar"] {
            assert_eq!(sanear(ok), ok);
        }
        // Largo: 120 caracteres como mucho, con la extensión.
        let largo = format!("{}.tar.gz", "ñ".repeat(300));
        let s = sanear(&largo);
        assert_eq!(s.chars().count(), 120);
        assert!(s.ends_with(".gz") && s.starts_with("ññ"));
        let sin_ext = sanear(&"a".repeat(500));
        assert_eq!(sin_ext.len(), 120);
        // Una «extensión» enorme no se conserva entera.
        assert_eq!(sanear(&format!("a.{}", "b".repeat(300))).chars().count(), 120);
    }

    #[test]
    fn tamanos() {
        assert_eq!(tamano(0), "0 B");
        assert_eq!(tamano(1023), "1023 B");
        assert_eq!(tamano(1024), "1 KB");
        assert_eq!(tamano(1536), "1,5 KB");
        assert_eq!(tamano(5 << 20), "5 MB");
        assert_eq!(tamano(1_288_490_189), "1,2 GB");
    }

    #[test]
    fn numera_sin_sobrescribir() {
        let mut sys = nf_core::platform::TestPlatform::default();
        let now = sys.now();
        let dir = "C:\\Pruebas";
        sys.files.mkdir(dir, now).unwrap();
        assert_eq!(nombre_libre(&mut sys.files, dir, "x.txt").as_deref(), Some("x.txt"));
        sys.files.write("C:\\Pruebas\\x.txt", b"1", now).unwrap();
        assert_eq!(nombre_libre(&mut sys.files, dir, "x.txt").as_deref(), Some("x (1).txt"));
        sys.files.write("C:\\Pruebas\\x (1).txt", b"1", now).unwrap();
        assert_eq!(nombre_libre(&mut sys.files, dir, "x.txt").as_deref(), Some("x (2).txt"));
        sys.files.write("C:\\Pruebas\\README", b"1", now).unwrap();
        assert_eq!(nombre_libre(&mut sys.files, dir, "README").as_deref(), Some("README (1)"));
        sys.files.write("C:\\Pruebas\\.ini", b"1", now).unwrap();
        assert_eq!(nombre_libre(&mut sys.files, dir, ".ini").as_deref(), Some(".ini (1)"));
    }

    #[test]
    fn el_aviso_dice_nombre_tamano_y_sitio() {
        let mut it = Item { id: 1, nombre: String::from("juego.nfx"), url: String::from("https://Descargas.Ejemplo.com/x"), total: Some(2 << 20), recibido: 0, estado: Estado::Pregunta, ruta: String::new(), get: None, datos: Vec::new(), completo: false };
        let (t, aviso) = it.pregunta();
        assert_eq!(t, "¿Descargar juego.nfx (2 MB) de descargas.ejemplo.com?");
        assert_eq!(aviso, Some("Es un programa. Ábrelo solo si confías en este sitio."));
        it.nombre = String::from("foto.png");
        it.total = None;
        let (t, aviso) = it.pregunta();
        assert_eq!(t, "¿Descargar foto.png (tamaño desconocido) de descargas.ejemplo.com?");
        assert_eq!(aviso, None);
    }
}
