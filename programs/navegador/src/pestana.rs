// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Una pestaña: historial, página mostrada y las descargas en curso (documento principal y recursos).
use crate::campo::Campo;
use crate::entrada::{es_ruta_local, local_de_url, recurso_permitido, sin_fragmento, tipo_de_extension, tipo_vista, url_de_local, Entrada, TipoVista, HOST_LOCAL};
use crate::fuentes::Letras;
use crate::paginas;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use nf_core::bmp::Picture;
use nf_core::fs::FsError;
use nf_core::fsapi::FilesApi;
use nf_core::img::{self, Limits};
use nf_core::net::http::{HttpEvent, HttpGet};
use nf_core::net::{NetApi, NetError};
use nf_core::platform::Platform;
use nf_web::{url, Documento, Recurso, TipoRecurso};

/// Descargas a la vez por pestaña (la principal cuenta).
pub const SIMULTANEAS: usize = 6;
const MAX_REDIRECCIONES: u8 = 10;
const TOPE_DOC: usize = 8 << 20;
const TOPE_CSS: usize = 2 << 20;
const TOPE_IMG: usize = 16 << 20;
/// Mientras carga, la página se vuelve a maquetar como mucho cada tanto.
const REMAQUETAR_MS: u64 = 250;
/// `<meta http-equiv="refresh">`: espera máxima que se respeta y redirecciones seguidas que se siguen.
const REFRESCO_MAX_S: u32 = 30;
const MAX_REFRESCOS: u8 = 5;
/// Margen alrededor de una imagen suelta.
pub const MARGEN_IMG: i32 = 20;
/// Base con la que se maquetan las páginas de error (no tienen recursos).
const BASE_ERROR: &str = "http://error.invalid/";

pub enum Vista {
    /// Página de inicio (la dibuja el programa).
    Inicio,
    Documento(Documento),
    /// Imagen suelta, centrada.
    Imagen(Picture),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Seguridad {
    Ninguna,
    Segura,
    NoSegura,
    Archivo,
}

struct Nav {
    url: String,
    scroll: i32,
}

enum Rol {
    Principal,
    Recurso(Recurso),
}

struct Estado {
    codigo: u16,
    motivo: String,
    tipo: Option<String>,
}

type Fallo = (String, String);

struct Descarga {
    get: HttpGet,
    rol: Rol,
    /// La que se está pidiendo ahora (cambia con las redirecciones).
    url: String,
    redirs: u8,
    estado: Option<Estado>,
    cuerpo: Vec<u8>,
    tope: usize,
    fin: Option<Result<(), Fallo>>,
}

fn tope(t: TipoRecurso) -> usize {
    match t {
        TipoRecurso::Css => TOPE_CSS,
        TipoRecurso::Imagen => TOPE_IMG,
    }
}

impl Descarga {
    fn nueva(get: HttpGet, rol: Rol, url: &str, tope: usize) -> Descarga {
        Descarga { get, rol, url: String::from(url), redirs: 0, estado: None, cuerpo: Vec::new(), tope, fin: None }
    }

    fn fallar(&mut self, net: &mut dyn NetApi, titulo: &str, detalle: &str) {
        self.get.cancel(net);
        self.fin = Some(Err((String::from(titulo), String::from(detalle))));
    }

    /// Avanza la descarga: cabeceras, redirecciones (a mano, para vigilar adónde van), cuerpo y fin.
    fn sondear(&mut self, net: &mut dyn NetApi, ahora: u64, pagina: &str) {
        for ev in self.get.poll(net, ahora) {
            match ev {
                HttpEvent::Head { status, reason, headers } => {
                    let cab = |n: &str| headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.clone());
                    if let (301 | 302 | 303 | 307 | 308, Some(loc)) = (status, cab("location")) {
                        return self.redirigir(net, &loc, pagina);
                    }
                    self.estado = Some(Estado { codigo: status, motivo: reason, tipo: cab("content-type") });
                }
                HttpEvent::Body(b) => {
                    if self.cuerpo.len() + b.len() > self.tope {
                        return self.fallar(net, "El archivo es demasiado grande", "Supera el tamaño máximo que admite el navegador.");
                    }
                    self.cuerpo.extend_from_slice(&b);
                }
                HttpEvent::Secure(_) => {}
                HttpEvent::Done => self.fin = Some(Ok(())),
                HttpEvent::Error(e) => {
                    self.fin = Some(Err(paginas::error_de_red(&e, &self.url)));
                    return;
                }
            }
        }
    }

    fn redirigir(&mut self, net: &mut dyn NetApi, loc: &str, pagina: &str) {
        if self.redirs >= MAX_REDIRECCIONES {
            return self.fallar(net, "Demasiadas redirecciones", "La página redirige una y otra vez sin llegar a ningún sitio.");
        }
        let Some(sig) = url::resolver(&self.url, loc) else {
            return self.fallar(net, "Redirección no válida", "La página redirige a una dirección que el navegador no admite (solo http y https).");
        };
        // Un recurso de una página pública no puede acabar en la red local por una redirección.
        if matches!(self.rol, Rol::Recurso(_)) && !recurso_permitido(pagina, &sig) {
            return self.fallar(net, "Bloqueado", "Una página de Internet no puede cargar recursos de la red local.");
        }
        self.get.cancel(net);
        match HttpGet::start(net, &sig, false, false) {
            Ok(g) => {
                self.get = g;
                self.url = sig;
                self.redirs += 1;
                self.estado = None;
                self.cuerpo.clear();
            }
            Err(e) => self.fin = Some(Err(paginas::error_de_red(&e, &sig))),
        }
    }
}

/// Entrega a la página lo descargado (o su fallo: sin imagen se pinta el texto alternativo).
fn dar_recurso(d: &mut Documento, r: &Recurso, datos: Option<&[u8]>) {
    match (r.tipo, datos) {
        (TipoRecurso::Css, Some(b)) => d.poner_css(&r.url, b),
        (TipoRecurso::Css, None) => {}
        (TipoRecurso::Imagen, Some(b)) => match img::dimensions(b) {
            Some((w, h)) => {
                d.tamano_imagen(&r.url, w, h);
                d.poner_imagen(&r.url, img::decode(b, Limits::default()).ok());
            }
            None => d.poner_imagen(&r.url, None),
        },
        (TipoRecurso::Imagen, None) => d.poner_imagen(&r.url, None),
    }
}

fn motivo_fs(e: FsError) -> &'static str {
    match e {
        FsError::NotFound | FsError::BadPath => "No existe ese archivo.",
        FsError::Denied => "No tienes permiso para abrir ese archivo.",
        FsError::IsDir => "Es una carpeta, no un archivo.",
        _ => "No se ha podido leer el archivo.",
    }
}

/// Último trozo de la ruta o de la URL (el título de un texto o una imagen sueltos).
fn nombre_de(url: &str) -> String {
    let sin = sin_fragmento(url).0.split('?').next().unwrap_or("");
    String::from(sin.rsplit(['/', '\\']).find(|s| !s.is_empty()).unwrap_or(url))
}

pub struct Pestana {
    pub id: u32,
    pub vista: Vista,
    pub titulo: String,
    /// La mostrada (la ruta, si es del disco); vacía en la página de inicio.
    pub url: String,
    /// La que se está cargando.
    pub pendiente: Option<String>,
    /// URL con la que se maquetó la página mostrada.
    pub base: String,
    /// Página del disco: sus recursos relativos se leen de disco.
    pub local: bool,
    /// Lo mostrado es una página de error propia: no lleva candado.
    pub es_error: bool,
    pub scroll: i32,
    /// Alto de lo mostrado, para el desplazamiento.
    pub alto: i32,
    /// Caja de búsqueda de la página de inicio.
    pub caja: Campo,
    /// Selección de texto, en coordenadas de la página.
    pub sel: Option<((i32, i32), (i32, i32))>,
    /// Elemento al que ir cuando haya maquetado (`#fragmento`) y desplazamiento a recuperar (atrás, recargar).
    pub ancla: Option<String>,
    pub restaurar: Option<i32>,
    historial: Vec<Nav>,
    pos: usize,
    descargas: Vec<Descarga>,
    cola: VecDeque<Recurso>,
    /// Hay algo nuevo que maquetar.
    sucia: bool,
    ancho_maquetado: i32,
    ultima_maqueta: u64,
    /// Redirección pedida por la página (`<meta http-equiv="refresh">`): cuándo (`uptime_ms`) y adónde.
    refresco: Option<(u64, String)>,
    refrescos: u8,
}

impl Pestana {
    pub fn nueva(id: u32) -> Pestana {
        Pestana {
            id,
            vista: Vista::Inicio,
            titulo: String::new(),
            url: String::new(),
            pendiente: None,
            base: String::new(),
            local: false,
            es_error: false,
            scroll: 0,
            alto: 0,
            caja: Campo::default(),
            sel: None,
            ancla: None,
            restaurar: None,
            historial: alloc::vec![Nav { url: String::new(), scroll: 0 }],
            pos: 0,
            descargas: Vec::new(),
            cola: VecDeque::new(),
            sucia: false,
            ancho_maquetado: 0,
            ultima_maqueta: 0,
            refresco: None,
            refrescos: 0,
        }
    }

    pub fn cargando(&self) -> bool {
        !self.descargas.is_empty() || !self.cola.is_empty()
    }

    pub fn titulo_visible(&self) -> String {
        if self.pendiente.is_some() {
            String::from("Cargando…")
        } else if !self.titulo.is_empty() {
            self.titulo.clone()
        } else if self.url.is_empty() {
            String::from("Nueva pestaña")
        } else {
            self.url.clone()
        }
    }

    /// La dirección que enseña la barra.
    pub fn url_visible(&self) -> &str {
        self.pendiente.as_deref().unwrap_or(&self.url)
    }

    pub fn seguridad(&self) -> Seguridad {
        let u = self.url_visible();
        if self.es_error && self.pendiente.is_none() {
            Seguridad::Ninguna
        } else if u.starts_with("https://") {
            Seguridad::Segura
        } else if u.starts_with("http://") {
            Seguridad::NoSegura
        } else if es_ruta_local(u) {
            Seguridad::Archivo
        } else {
            Seguridad::Ninguna
        }
    }

    pub fn puede_atras(&self) -> bool {
        self.pos > 0
    }

    pub fn puede_adelante(&self) -> bool {
        self.pos + 1 < self.historial.len()
    }

    // ------------------------------------------------------------ navegación

    /// Corta todas las descargas.
    pub fn cancelar(&mut self, sys: &mut dyn Platform) {
        if let Some(net) = sys.net() {
            for d in self.descargas.iter_mut() {
                d.get.cancel(net);
            }
        }
        self.descargas.clear();
        self.cola.clear();
        self.pendiente = None;
        self.refresco = None;
        self.refrescos = 0;
    }

    /// Navega a `e` y la apunta en el historial.
    pub fn ir(&mut self, sys: &mut dyn Platform, e: Entrada, ahora: u64) {
        let url = match e {
            Entrada::Web(u) => u,
            Entrada::Local(p) => p,
        };
        self.historial[self.pos].scroll = self.scroll;
        self.historial.truncate(self.pos + 1);
        self.historial.push(Nav { url: url.clone(), scroll: 0 });
        self.pos += 1;
        self.cargar(sys, &url, ahora, None);
    }

    pub fn atras(&mut self, sys: &mut dyn Platform, ahora: u64) {
        if self.puede_atras() {
            self.historial[self.pos].scroll = self.scroll;
            self.pos -= 1;
            self.cargar_actual(sys, ahora, None);
        }
    }

    pub fn adelante(&mut self, sys: &mut dyn Platform, ahora: u64) {
        if self.puede_adelante() {
            self.historial[self.pos].scroll = self.scroll;
            self.pos += 1;
            self.cargar_actual(sys, ahora, None);
        }
    }

    pub fn recargar(&mut self, sys: &mut dyn Platform, ahora: u64) {
        self.cargar_actual(sys, ahora, Some(self.scroll));
    }

    pub fn detener(&mut self, sys: &mut dyn Platform) {
        self.cancelar(sys);
    }

    fn cargar_actual(&mut self, sys: &mut dyn Platform, ahora: u64, scroll: Option<i32>) {
        let n = &self.historial[self.pos];
        let (url, scroll) = (n.url.clone(), scroll.unwrap_or(n.scroll));
        self.cargar(sys, &url, ahora, Some(scroll));
    }

    fn cargar(&mut self, sys: &mut dyn Platform, url: &str, ahora: u64, restaurar: Option<i32>) {
        let _ = ahora;
        self.cancelar(sys);
        self.restaurar = restaurar.filter(|&s| s > 0);
        self.ancla = None;
        if url.is_empty() {
            return self.mostrar_inicio();
        }
        if es_ruta_local(url) {
            return self.abrir_local(sys, url);
        }
        let (sin, frag) = sin_fragmento(url);
        self.ancla = frag.filter(|f| !f.is_empty()).map(String::from);
        self.pendiente = Some(String::from(url));
        let Some(net) = sys.net() else {
            let (t, d) = paginas::error_de_red(&NetError::NoAdapter, url);
            return self.error(&t, &d, url);
        };
        match HttpGet::start(net, sin, false, false) {
            Ok(get) => self.descargas.push(Descarga::nueva(get, Rol::Principal, sin, TOPE_DOC)),
            Err(e) => {
                let (t, d) = paginas::error_de_red(&e, url);
                self.error(&t, &d, url);
            }
        }
    }

    fn mostrar_inicio(&mut self) {
        *self = Pestana { id: self.id, historial: core::mem::take(&mut self.historial), pos: self.pos, caja: core::mem::take(&mut self.caja), ..Pestana::nueva(self.id) };
    }

    fn abrir_local(&mut self, sys: &mut dyn Platform, ruta: &str) {
        match sys.fs().read(ruta) {
            Ok(datos) => {
                let tipo = Some(tipo_de_extension(ruta).unwrap_or("application/octet-stream"));
                self.mostrar(sys, ruta, &url_de_local(ruta), true, tipo, datos);
            }
            Err(e) => self.error("No se puede abrir el archivo", motivo_fs(e), ruta),
        }
    }

    // ------------------------------------------------------------ lo que se ve

    /// Página de error en lugar de la actual.
    fn error(&mut self, titulo: &str, detalle: &str, url: &str) {
        self.poner_documento(url, BASE_ERROR, false, Documento::cargar(paginas::error(titulo, detalle, url).as_bytes(), Some("text/html; charset=utf-8"), BASE_ERROR));
        self.titulo = String::from(titulo);
        self.es_error = true;
    }

    fn poner_documento(&mut self, mostrada: &str, base: &str, local: bool, d: Documento) {
        self.titulo = d.titulo().unwrap_or_default();
        self.vista = Vista::Documento(d);
        self.fijar(mostrada, base, local);
    }

    fn fijar(&mut self, mostrada: &str, base: &str, local: bool) {
        self.url = String::from(mostrada);
        self.base = String::from(base);
        self.local = local;
        self.es_error = false;
        self.pendiente = None;
        self.scroll = 0;
        self.alto = 0;
        self.sel = None;
        self.sucia = false;
        self.ancho_maquetado = 0;
        self.historial[self.pos].url = String::from(sin_fragmento(mostrada).0);
    }

    /// Enseña una respuesta (de la red o del disco) según su tipo. Si es una página, encola sus recursos.
    fn mostrar(&mut self, sys: &mut dyn Platform, mostrada: &str, base: &str, local: bool, ct: Option<&str>, cuerpo: Vec<u8>) {
        match tipo_vista(ct, &cuerpo) {
            TipoVista::Html if cuerpo.len() <= TOPE_DOC => {
                let d = Documento::cargar(&cuerpo, ct, base);
                // Redirección sin JavaScript; una página pública no puede mandar a la red local.
                self.refresco = d
                    .refresco()
                    .filter(|&(s, u)| s <= REFRESCO_MAX_S && (u.starts_with("https://") || u.starts_with("http://")) && recurso_permitido(base, u))
                    .map(|(s, u)| (sys.uptime_ms() + s as u64 * 1000, String::from(u)));
                self.poner_documento(mostrada, base, local, d);
                self.encolar_recursos(sys);
            }
            TipoVista::Texto if cuerpo.len() <= TOPE_DOC => {
                let texto = nf_web::html::decodificar(&cuerpo, ct);
                let nombre = nombre_de(mostrada);
                self.poner_documento(mostrada, base, local, Documento::cargar(paginas::texto_plano(&nombre, &texto).as_bytes(), Some("text/html; charset=utf-8"), base));
                self.titulo = nombre;
            }
            TipoVista::Imagen if cuerpo.len() <= TOPE_IMG => match img::decode(&cuerpo, Limits::default()) {
                Ok(p) => {
                    self.vista = Vista::Imagen(p);
                    self.titulo = nombre_de(mostrada);
                    self.fijar(mostrada, base, local);
                }
                Err(_) => self.error("No se puede mostrar la imagen", "El archivo está dañado, no es de un formato admitido o es demasiado grande.", mostrada),
            },
            TipoVista::Html | TipoVista::Texto | TipoVista::Imagen => self.error("El archivo es demasiado grande", "Supera el tamaño máximo que admite el navegador.", mostrada),
            TipoVista::Otro => {
                let tipo = ct.map_or(String::from("desconocido"), |c| String::from(c.split(';').next().unwrap_or("").trim()));
                self.error("No se puede mostrar este tipo de archivo", &format!("El contenido es de tipo «{tipo}». De momento el navegador solo muestra páginas web, texto e imágenes PNG, JPEG y GIF."), mostrada);
            }
        }
    }

    /// Apunta los recursos de la página nueva: los del disco se leen ya; los de la red, a la cola.
    fn encolar_recursos(&mut self, sys: &mut dyn Platform) {
        let Vista::Documento(d) = &mut self.vista else { return };
        for r in d.recursos() {
            if !recurso_permitido(&self.base, &r.url) {
                dar_recurso(d, &r, None);
            } else if self.local && r.url.strip_prefix("http://").is_some_and(|u| u.starts_with(HOST_LOCAL)) {
                let datos = local_de_url(&r.url).and_then(|p| sys.fs().read(&p).ok()).filter(|b| b.len() <= tope(r.tipo));
                dar_recurso(d, &r, datos.as_deref());
            } else {
                self.cola.push_back(r);
            }
        }
    }

    // ------------------------------------------------------------ red

    /// Atiende la red. `true` si algo ha cambiado y hay que dibujar.
    pub fn tick(&mut self, sys: &mut dyn Platform, ahora: u64) -> bool {
        let mut cambio = false;
        if self.refresco.as_ref().is_some_and(|r| sys.uptime_ms() >= r.0) {
            let (_, url) = self.refresco.take().unwrap_or_default();
            let n = self.refrescos;
            if n < MAX_REFRESCOS {
                // Sustituye la entrada actual del historial (Atrás no vuelve a la página que redirige).
                self.historial[self.pos].url = url.clone();
                self.cargar(sys, &url, ahora, None);
                self.refrescos = n + 1;
                cambio = true;
            }
        }
        if !self.descargas.is_empty() {
            match sys.net() {
                Some(net) => {
                    for d in self.descargas.iter_mut() {
                        d.sondear(net, ahora, &self.base);
                    }
                }
                None => {
                    for d in self.descargas.iter_mut() {
                        d.fin = Some(Err(paginas::error_de_red(&NetError::NoAdapter, &d.url)));
                    }
                }
            }
            let (fin, vivas): (Vec<_>, Vec<_>) = core::mem::take(&mut self.descargas).into_iter().partition(|d| d.fin.is_some());
            self.descargas = vivas;
            for d in fin {
                cambio = true;
                self.terminar(sys, d);
            }
            // La redirección del documento principal cambia la dirección que se va a mostrar.
            if let Some(p) = self.descargas.iter().find(|d| matches!(d.rol, Rol::Principal)) {
                self.pendiente = Some(match sin_fragmento(self.pendiente.as_deref().unwrap_or("")).1 {
                    Some(f) => format!("{}#{f}", p.url),
                    None => p.url.clone(),
                });
            }
        }
        self.lanzar(sys);
        if self.sucia && (ahora.saturating_sub(self.ultima_maqueta) >= REMAQUETAR_MS || !self.cargando()) {
            self.ancho_maquetado = 0;
            self.sucia = false;
            cambio = true;
        }
        cambio
    }

    fn terminar(&mut self, sys: &mut dyn Platform, d: Descarga) {
        let Some(fin) = d.fin else { return };
        match (d.rol, fin) {
            (Rol::Principal, Err((t, m))) => {
                let url = self.pendiente.take().unwrap_or(d.url);
                self.error(&t, &m, &url);
            }
            (Rol::Principal, Ok(())) => {
                let (codigo, motivo, tipo) = d.estado.map_or((200, String::new(), None), |e| (e.codigo, e.motivo, e.tipo));
                let url = String::from(sin_fragmento(&d.url).0);
                if codigo >= 400 && d.cuerpo.is_empty() {
                    let (t, m) = paginas::error_http(codigo, &motivo);
                    return self.error(&t, &m, &url);
                }
                // La ancla de lo que se pidió vale para la página que llegue (aunque haya redirecciones).
                let ancla = self.ancla.take();
                self.mostrar(sys, &url, &url, false, tipo.as_deref(), d.cuerpo);
                self.ancla = ancla;
            }
            (Rol::Recurso(r), res) => {
                let bien = res.is_ok() && d.estado.as_ref().is_some_and(|e| (200..300).contains(&e.codigo));
                if let Vista::Documento(doc) = &mut self.vista {
                    dar_recurso(doc, &r, bien.then_some(&d.cuerpo[..]));
                    self.sucia = true;
                }
            }
        }
    }

    /// Pone en marcha recursos de la cola hasta llenar las descargas simultáneas.
    fn lanzar(&mut self, sys: &mut dyn Platform) {
        while self.descargas.len() < SIMULTANEAS {
            let Some(r) = self.cola.front().cloned() else { return };
            let Some(net) = sys.net() else { return };
            match HttpGet::start(net, &r.url, false, false) {
                Ok(get) => {
                    self.cola.pop_front();
                    self.descargas.push(Descarga::nueva(get, Rol::Recurso(r.clone()), &r.url, tope(r.tipo)));
                }
                Err(NetError::Busy) => return,
                Err(_) => {
                    self.cola.pop_front();
                    if let Vista::Documento(doc) = &mut self.vista {
                        dar_recurso(doc, &r, None);
                        self.sucia = true;
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ maquetado

    /// Maqueta (o mide la imagen) para `ancho` px si hace falta y deja `alto`. Carga las fuentes que pidan.
    pub fn preparar(&mut self, ancho: i32, letras: &mut Letras, fs: &mut dyn FilesApi, ahora: u64) {
        if self.ancho_maquetado == ancho {
            return;
        }
        match &mut self.vista {
            Vista::Inicio => return,
            Vista::Documento(d) => {
                for _ in 0..3 {
                    self.alto = d.maquetar(ancho, letras);
                    if !letras.faltan() || !letras.cargar(fs) {
                        break;
                    }
                }
                let aplicar = match (self.restaurar, &self.ancla) {
                    (Some(y), _) => Some(y),
                    (None, Some(a)) => d.ancla(a),
                    _ => None,
                };
                if let Some(y) = aplicar {
                    self.scroll = y;
                }
            }
            Vista::Imagen(p) => self.alto = ajustar_imagen(p, ancho).1 + 2 * MARGEN_IMG,
        }
        self.ancho_maquetado = ancho;
        self.ultima_maqueta = ahora;
        if !self.cargando() {
            self.restaurar = None;
            self.ancla = None;
        }
    }

    /// El usuario ha movido la página: ya no se le lleva a la ancla ni a la posición guardada.
    pub fn desplazar_a(&mut self, y: i32) {
        self.scroll = y;
        self.restaurar = None;
        self.ancla = None;
    }
}

/// Tamaño con el que se muestra una imagen suelta: la que cabe a lo ancho, sin ampliarla.
pub fn ajustar_imagen(p: &Picture, ancho: i32) -> (i32, i32) {
    let max = (ancho - 2 * MARGEN_IMG).max(1);
    if p.w <= max || p.w <= 0 {
        (p.w, p.h)
    } else {
        (max, ((p.h as i64 * max as i64 / p.w as i64) as i32).max(1))
    }
}
