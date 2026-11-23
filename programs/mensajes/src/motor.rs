// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! El motor de Mensajes: identidad, equipos vistos, conexiones y conversaciones. No dibuja nada:
//! la ventana (`lib.rs`) lo mueve en cada tick y lee su estado. Todo lo que llega por la red es
//! hostil: topes en todas las tablas y búferes, y ninguna ruta que dependa de datos de fuera entra
//! en pánico.
use crate::almacen::{contactos_a_texto, contactos_de_texto, historial_a_texto, historial_de_texto, Estado, Ficha, Mensaje, MAX_HISTORIAL};
use crate::proto::*;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use nf_core::crypto::{ct_eq, from_hex, sha256, to_hex, wipe};
use nf_core::fs::FsError;
use nf_core::net::{Ipv4, ListenId, NetApi, NetError, SockId, State, TcpState, UdpId};
use nf_core::platform::{DateTime, Platform};

/// Equipos vistos sin guardar a la vez (tabla de anuncios) y contactos guardados.
pub const MAX_VISTOS: usize = 64;
pub const MAX_GUARDADOS: usize = 256;
const ENTRANTES_MAX: usize = 8;
const SALIENTES_MAX: usize = 4;
/// Sin anuncios en este tiempo, un equipo pasa a «desconectado».
const CADUCA_MS: u64 = 20_000;
const ANUNCIO_MS: u64 = 5_000;
const EXTRA_MS: u64 = 1_000;
const REINTENTO_MS: u64 = 30_000;
const SALUDO_MS: u64 = 5_000;
const CONEXION_MS: u64 = 10_000;
/// Un equipo visto y no guardado se olvida pasado este tiempo sin anuncios.
const OLVIDO_MS: u64 = 120_000;
const MAX_POR_CONEXION: usize = 32;
const MAX_TEXTOS_ENTRANTES: usize = 256;
/// Tope del búfer de recepción de cada conexión (una trama entera cabe de sobra).
const ENTRADA_MAX: usize = 8192;
const GUARDAR_MS: u64 = 1000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Permiso {
    /// «Usar la red».
    Usar,
    /// «Aceptar conexiones».
    Aceptar,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Red {
    Iniciando,
    Esperando(Permiso),
    Denegado(Permiso),
    SinRed,
    Error(&'static str),
    Activo,
}

pub struct Visto {
    pub ip: Ipv4,
    pub puerto: u16,
    pub ms: u64,
}

pub struct Contacto {
    pub pk: Clave,
    pub nombre: String,
    pub verificado: bool,
    /// Se guarda en disco (tiene conversación o está verificado).
    pub guardado: bool,
    pub visto: Option<Visto>,
    pub msgs: Vec<Mensaje>,
    /// Ha llegado algo desde la última vez que se abrió su conversación.
    pub sin_leer: bool,
    sucio: bool,
    /// No intentar conectar antes de este instante.
    proximo: u64,
}

impl Contacto {
    pub(crate) fn nuevo(pk: Clave, nombre: String, guardado: bool) -> Contacto {
        Contacto { pk, nombre, verificado: false, guardado, visto: None, msgs: Vec::new(), sin_leer: false, sucio: false, proximo: 0 }
    }
    pub fn en_linea(&self, ahora: u64) -> bool {
        self.visto.as_ref().is_some_and(|v| ahora < v.ms.saturating_add(CADUCA_MS))
    }
    pub fn pendientes(&self) -> usize {
        self.msgs.iter().filter(|m| m.saliente && m.estado == Estado::Pendiente).count()
    }
    fn recortar(&mut self) {
        let sobran = self.msgs.len().saturating_sub(MAX_HISTORIAL);
        self.msgs.drain(..sobran);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Etapa {
    Conectando,
    EsperaHola,
    Abierta,
}

enum Fin {
    Sigue,
    Bien,
    Mal,
}

/// Una conexión TCP en marcha. `inicia` = somos *I*.
struct Conn {
    sock: SockId,
    inicia: bool,
    /// *I*: el contacto al que escribimos. *R*: la identidad que dice ser *I* (tras el saludo).
    destino: Option<Clave>,
    etapa: Etapa,
    inicio: u64,
    efimera: Clave,
    hola_propio: [u8; HOLA_LEN],
    entrada: Vec<u8>,
    salida: Vec<u8>,
    sesion: Option<Sesion>,
    /// *I*: ids enviados que esperan su «recibido».
    esperando: Vec<u64>,
    /// *I*: ya se encoló el adiós. *R*: llegó el adiós.
    fin: bool,
    textos: usize,
}

impl Drop for Conn {
    fn drop(&mut self) {
        wipe(&mut self.efimera);
    }
}

impl Conn {
    fn nueva(sock: SockId, inicia: bool, destino: Option<Clave>, efimera: Clave, hola_propio: [u8; HOLA_LEN], ahora: u64) -> Conn {
        let etapa = if inicia { Etapa::Conectando } else { Etapa::EsperaHola };
        Conn { sock, inicia, destino, etapa, inicio: ahora, efimera, hola_propio, entrada: Vec::new(), salida: Vec::new(), sesion: None, esperando: Vec::new(), fin: false, textos: 0 }
    }

    /// Sella una trama y la deja en la cola de salida. `false` si no se pudo (hay que cortar).
    fn enviar(&mut self, t: &Trama) -> bool {
        let (Some(cuerpo), Some(s)) = (codificar(t), self.sesion.as_mut()) else { return false };
        match s.sellar(&cuerpo) {
            Some(f) => {
                self.salida.extend_from_slice(&f);
                true
            }
            None => false,
        }
    }
}

/// 32 bytes impredecibles: `sha256` de varias lecturas de `entropy()` y la hora.
fn azar(sys: &mut dyn Platform) -> Clave {
    let mut b = Vec::with_capacity(48);
    for _ in 0..4 {
        b.extend_from_slice(&sys.entropy().to_le_bytes());
    }
    b.extend_from_slice(&sys.uptime_ms().to_le_bytes());
    let t = sys.now();
    b.extend_from_slice(&t.year.to_be_bytes());
    b.extend_from_slice(&[t.month, t.day, t.hour, t.minute, t.second]);
    let h = sha256(&b);
    wipe(&mut b);
    h
}

fn escribir(sys: &mut dyn Platform, ruta: &str, datos: &[u8]) {
    let now = sys.now();
    // shortcut: un fallo al escribir (disco lleno o sin permiso) no se avisa, se reintenta en el siguiente guardado.
    let _ = sys.fs().write(ruta, datos, now);
}

fn leer_texto(sys: &mut dyn Platform, ruta: &str) -> Option<String> {
    String::from_utf8(sys.fs().read(ruta).ok()?).ok()
}

pub struct Motor {
    privada: Clave,
    pub publica: Clave,
    pub nombre: String,
    carpeta: String,
    pub contactos: Vec<Contacto>,
    pub red: Red,
    pub ahora: u64,
    /// La conversación que se está mirando (para marcar lo que llega como leído o no).
    pub seleccionado: Option<Clave>,
    /// Algo visible ha cambiado: la ventana debe redibujarse.
    pub cambio: bool,
    udp_tx: Option<UdpId>,
    udp_rx: Option<UdpId>,
    escucha: Option<ListenId>,
    conns: Vec<Conn>,
    prox_red: u64,
    prox_anuncio: u64,
    ult_anuncio: u64,
    anuncio_extra: bool,
    sucio: bool,
    ult_guardado: u64,
    en_linea: usize,
}

impl Motor {
    /// Abre la carpeta `Mensajes` del usuario y carga identidad, contactos y conversaciones.
    pub fn nuevo(sys: &mut dyn Platform, ahora: u64) -> Motor {
        let carpeta = format!("{}\\Mensajes", sys.fs().home());
        let now = sys.now();
        let _ = sys.fs().mkdir(&carpeta, now);
        let privada = Self::cargar_identidad(sys, &carpeta);
        let nombre = Self::nombre_equipo(sys);
        let mut contactos = Vec::new();
        for f in leer_texto(sys, &format!("{carpeta}\\contactos")).map(|t| contactos_de_texto(&t)).unwrap_or_default().into_iter().take(MAX_GUARDADOS) {
            let mut c = Contacto::nuevo(f.pk, f.nombre, true);
            c.verificado = f.verificado;
            c.msgs = leer_texto(sys, &format!("{carpeta}\\{}.txt", huella(&f.pk))).map(|t| historial_de_texto(&t)).unwrap_or_default();
            contactos.push(c);
        }
        Motor {
            publica: publica(&privada),
            privada,
            nombre,
            carpeta,
            contactos,
            red: Red::Iniciando,
            ahora,
            seleccionado: None,
            cambio: true,
            udp_tx: None,
            udp_rx: None,
            escucha: None,
            conns: Vec::new(),
            prox_red: 0,
            prox_anuncio: 0,
            ult_anuncio: 0,
            anuncio_extra: false,
            sucio: false,
            ult_guardado: 0,
            en_linea: 0,
        }
    }

    fn cargar_identidad(sys: &mut dyn Platform, carpeta: &str) -> Clave {
        let ruta = format!("{carpeta}\\identidad");
        let leida = sys.fs().read(&ruta);
        if let Ok(b) = &leida {
            if let Some(k) = core::str::from_utf8(b).ok().and_then(|t| from_hex(t.trim())).and_then(|v| Clave::try_from(v).ok()) {
                return k;
            }
        }
        let k = azar(sys);
        // Si no se pudo leer por otra causa que «no existe», no se pisa lo que haya.
        if matches!(leida, Ok(_) | Err(FsError::NotFound)) {
            escribir(sys, &ruta, to_hex(&k).as_bytes());
        }
        k
    }

    /// El nombre que se anuncia: el de la cuenta, acotado a 32 bytes y sin caracteres de control.
    fn nombre_equipo(sys: &mut dyn Platform) -> String {
        let mut n: String = sys.fs().user().unwrap_or_default().chars().filter(|c| !c.is_control()).collect();
        while n.len() > MAX_NOMBRE {
            n.pop();
        }
        if n.is_empty() {
            n = String::from("NightFury");
        }
        n
    }

    /// Para las capturas: dos contactos (uno conectado y verificado) y una conversación de ejemplo, sin red.
    #[doc(hidden)]
    pub fn con_ejemplo(mut self) -> Motor {
        let f = |hour, minute| DateTime { year: 2026, month: 11, day: 23, hour, minute, second: 0 };
        let m = |saliente, id, estado, hh, mm, t: &str| Mensaje { saliente, fecha: f(hh, mm), id, estado, texto: String::from(t) };
        let mut ana = Contacto::nuevo([1; 32], String::from("Ana (portátil)"), true);
        ana.verificado = true;
        ana.visto = Some(Visto { ip: [192, 168, 1, 20], puerto: PUERTO_TCP, ms: 0 });
        ana.msgs = vec![
            m(false, 1, Estado::Recibido, 10, 41, "Hola, ¿ya tienes lista la presentación?"),
            m(true, 2, Estado::Entregado, 10, 42, "Casi. Te la paso en cinco minutos."),
            m(false, 3, Estado::Recibido, 10, 43, "Perfecto, gracias. Avísame cuando la tengas y la miramos juntos antes de la reunión."),
            m(true, 4, Estado::Entregado, 10, 49, "Hecho, ya la tienes en la carpeta compartida."),
            m(true, 5, Estado::Pendiente, 10, 50, "¿Quedamos mañana a las nueve?"),
        ];
        self.contactos = vec![ana, Contacto::nuevo([2; 32], String::from("Luis"), true)];
        self.red = Red::Activo;
        self.ahora = 0;
        self
    }

    pub fn contacto(&self, pk: &Clave) -> Option<&Contacto> {
        self.contactos.iter().find(|c| c.pk == *pk)
    }

    /// Hay otro contacto verificado con el mismo nombre y otra clave.
    pub fn otra_clave(&self, c: &Contacto) -> bool {
        !c.verificado && self.contactos.iter().any(|o| o.verificado && o.pk != c.pk && o.nombre.eq_ignore_ascii_case(&c.nombre))
    }

    #[cfg(test)]
    pub fn conexiones(&self) -> usize {
        self.conns.len()
    }

    #[cfg(test)]
    pub fn privada_borrada(&self) -> bool {
        self.privada == [0; 32]
    }

    pub fn equipos_en_linea(&self) -> usize {
        self.contactos.iter().filter(|c| c.en_linea(self.ahora)).count()
    }

    // ------------------------------------------------------------ acciones de la ventana

    /// Añade un mensaje saliente pendiente. `false` si el texto no cumple los topes o no hay sitio.
    pub fn enviar(&mut self, sys: &mut dyn Platform, pk: &Clave, texto: &str) -> bool {
        let texto = texto.trim();
        let guardados = self.contactos.iter().filter(|c| c.guardado).count();
        let Some(c) = self.contactos.iter_mut().find(|c| c.pk == *pk) else { return false };
        if !texto_valido(texto) || (!c.guardado && guardados >= MAX_GUARDADOS) {
            return false;
        }
        let id = u64::from_be_bytes(azar(sys)[..8].try_into().unwrap_or([0; 8]));
        c.msgs.push(Mensaje { saliente: true, fecha: sys.now(), id, estado: Estado::Pendiente, texto: limpiar(texto) });
        c.recortar();
        c.guardado = true;
        c.sucio = true;
        c.proximo = 0;
        self.sucio = true;
        self.cambio = true;
        true
    }

    pub fn verificar(&mut self, pk: &Clave, si: bool) {
        let guardados = self.contactos.iter().filter(|c| c.guardado).count();
        let Some(c) = self.contactos.iter_mut().find(|c| c.pk == *pk) else { return };
        if si && !c.guardado && guardados >= MAX_GUARDADOS {
            return;
        }
        c.verificado = si;
        c.guardado |= si;
        self.sucio = true;
        self.cambio = true;
    }

    pub fn leido(&mut self, pk: &Clave) {
        if let Some(c) = self.contactos.iter_mut().find(|c| c.pk == *pk) {
            c.sin_leer = false;
        }
        self.cambio = true;
    }

    /// Tras `Denegado`: cierra lo abierto y vuelve a pedir los sockets (por si el usuario ya cambió el permiso).
    pub fn reintentar(&mut self, sys: &mut dyn Platform) {
        self.cerrar_red(sys);
        self.red = Red::Iniciando;
        self.prox_red = 0;
        self.cambio = true;
    }

    fn cerrar_red(&mut self, sys: &mut dyn Platform) {
        if let Some(net) = sys.net() {
            for c in self.conns.drain(..) {
                net.tcp_abort(c.sock);
            }
            if let Some(u) = self.udp_tx.take() {
                net.udp_close(u);
            }
            if let Some(u) = self.udp_rx.take() {
                net.udp_close(u);
            }
            if let Some(l) = self.escucha.take() {
                net.tcp_unlisten(l);
            }
        }
        self.conns.clear();
        (self.udp_tx, self.udp_rx, self.escucha) = (None, None, None);
    }

    /// La ventana se cierra: suelta la red, guarda y borra la clave privada de la memoria.
    pub fn liberar(&mut self, sys: &mut dyn Platform) {
        self.cerrar_red(sys);
        self.guardar(sys, true);
        wipe(&mut self.privada);
    }

    // ------------------------------------------------------------ tick

    pub fn tick(&mut self, sys: &mut dyn Platform, ahora: u64) {
        self.ahora = ahora;
        if ahora >= self.prox_red && !matches!(self.red, Red::Denegado(_)) {
            self.comprobar_red(sys);
            self.prox_red = ahora + if matches!(self.red, Red::Esperando(_)) { 500 } else { 2000 };
        }
        if self.red == Red::Activo {
            self.recibir_anuncios(sys);
            self.anunciar(sys);
            self.aceptar(sys);
            self.conducir_todas(sys);
            self.iniciar(sys);
            self.olvidar();
        }
        let n = self.equipos_en_linea();
        if n != self.en_linea {
            self.en_linea = n;
            self.cambio = true;
        }
        self.guardar(sys, false);
    }

    fn poner_red(&mut self, r: Red) {
        if self.red != r {
            self.red = r;
            self.cambio = true;
        }
    }

    fn comprobar_red(&mut self, sys: &mut dyn Platform) {
        let Some(net) = sys.net() else { return self.poner_red(Red::SinRed) };
        if net.status().state != State::Connected {
            return self.poner_red(Red::SinRed);
        }
        match self.abrir_sockets(net) {
            Ok(()) => self.poner_red(Red::Activo),
            Err((e, p)) => self.poner_red(match e {
                NetError::Pending => Red::Esperando(p),
                NetError::Denied => Red::Denegado(p),
                NetError::InUse => Red::Error("El puerto de Mensajes ya lo usa otro programa."),
                NetError::NoAdapter | NetError::NoCable | NetError::NoAddress => Red::SinRed,
                _ => Red::Error("No se pudo preparar la red. Se vuelve a intentar."),
            }),
        }
    }

    /// Primero «usar la red» (el socket de envío) y luego «aceptar conexiones» (47320 y 47321).
    fn abrir_sockets(&mut self, net: &mut dyn NetApi) -> Result<(), (NetError, Permiso)> {
        if self.udp_tx.is_none() {
            self.udp_tx = Some(net.udp_bind(0).map_err(|e| (e, Permiso::Usar))?);
        }
        if self.udp_rx.is_none() {
            self.udp_rx = Some(net.udp_bind(PUERTO_UDP).map_err(|e| (e, Permiso::Aceptar))?);
        }
        if self.escucha.is_none() {
            self.escucha = Some(net.tcp_listen(PUERTO_TCP).map_err(|e| (e, Permiso::Aceptar))?);
        }
        Ok(())
    }

    fn fallo_permiso(&mut self, e: &NetError, p: Permiso) {
        if *e == NetError::Denied {
            self.poner_red(Red::Denegado(p));
        }
    }

    fn recibir_anuncios(&mut self, sys: &mut dyn Platform) {
        for _ in 0..16 {
            let Some(rx) = self.udp_rx else { return };
            let Some(net) = sys.net() else { return };
            match net.udp_recv(rx) {
                Ok(Some(d)) => self.anuncio_recibido(d.from, &d.data),
                Ok(None) => return,
                Err(_) => {
                    // El socket ya no existe: `comprobar_red` lo vuelve a abrir.
                    self.udp_rx = None;
                    return;
                }
            }
        }
    }

    fn anuncio_recibido(&mut self, ip: Ipv4, datos: &[u8]) {
        let Some(a) = leer_anuncio(datos, &self.publica) else { return };
        let ahora = self.ahora;
        let visto = Visto { ip, puerto: a.puerto, ms: ahora };
        if let Some(c) = self.contactos.iter_mut().find(|c| c.pk == a.pk) {
            if !c.en_linea(ahora) {
                self.anuncio_extra = true;
                c.proximo = 0;
            }
            c.visto = Some(visto);
            // Un contacto verificado conserva su nombre: el anuncio no está autenticado.
            if !c.verificado && c.nombre != a.nombre {
                c.nombre = a.nombre;
                if c.guardado {
                    self.sucio = true;
                }
            }
            self.cambio = true;
            return;
        }
        // Equipo nuevo: la tabla de no guardados tiene tope; si está llena, sale el más viejo desconectado.
        let sin_guardar = self.contactos.iter().filter(|c| !c.guardado).count();
        if sin_guardar >= MAX_VISTOS {
            let viejo = self.contactos.iter().enumerate().filter(|(_, c)| !c.guardado && !c.en_linea(ahora)).min_by_key(|(_, c)| c.visto.as_ref().map_or(0, |v| v.ms)).map(|(i, _)| i);
            match viejo {
                Some(i) => {
                    self.contactos.remove(i);
                }
                None => return,
            }
        }
        let mut c = Contacto::nuevo(a.pk, a.nombre, false);
        c.visto = Some(visto);
        self.contactos.push(c);
        self.anuncio_extra = true;
        self.cambio = true;
    }

    fn anunciar(&mut self, sys: &mut dyn Platform) {
        let ahora = self.ahora;
        let extra = self.anuncio_extra && ahora >= self.ult_anuncio.saturating_add(EXTRA_MS);
        if ahora < self.prox_anuncio && !extra {
            return;
        }
        let (Some(tx), datos) = (self.udp_tx, anuncio(&self.publica, &self.nombre)) else { return };
        let Some(net) = sys.net() else { return };
        if let Err(e) = net.udp_send(tx, [255, 255, 255, 255], PUERTO_UDP, &datos) {
            self.fallo_permiso(&e, Permiso::Usar);
        }
        self.ult_anuncio = ahora;
        self.prox_anuncio = ahora + ANUNCIO_MS;
        self.anuncio_extra = false;
    }

    fn aceptar(&mut self, sys: &mut dyn Platform) {
        for _ in 0..8 {
            let Some(l) = self.escucha else { return };
            let Some(net) = sys.net() else { return };
            match net.tcp_accept(l) {
                Ok(Some((sock, _ip, _puerto))) => {
                    if self.conns.iter().filter(|c| !c.inicia).count() >= ENTRANTES_MAX {
                        net.tcp_abort(sock);
                        continue;
                    }
                    let mut e = azar(sys);
                    self.conns.push(Conn::nueva(sock, false, None, e, [0; HOLA_LEN], self.ahora));
                    wipe(&mut e);
                }
                Ok(None) => return,
                Err(e) => return self.fallo_permiso(&e, Permiso::Aceptar),
            }
        }
    }

    /// Abre una conexión a cada contacto en línea que tenga mensajes pendientes.
    fn iniciar(&mut self, sys: &mut dyn Platform) {
        let ahora = self.ahora;
        for i in 0..self.contactos.len() {
            if self.conns.iter().filter(|c| c.inicia).count() >= SALIENTES_MAX {
                return;
            }
            let c = &self.contactos[i];
            let Some(v) = c.visto.as_ref().filter(|_| c.en_linea(ahora) && ahora >= c.proximo && c.pendientes() > 0) else { continue };
            let (pk, ip, puerto) = (c.pk, v.ip, v.puerto);
            if self.conns.iter().any(|x| x.inicia && x.destino == Some(pk)) {
                continue;
            }
            let mut e = azar(sys);
            let Some(net) = sys.net() else { return };
            match net.tcp_connect(ip, puerto) {
                Ok(sock) => {
                    let h = hola(HOLA_I, &self.publica, &publica(&e));
                    self.conns.push(Conn::nueva(sock, true, Some(pk), e, h, ahora));
                }
                // El aviso de permiso está abierto: se reintenta en otro tick.
                Err(NetError::Pending) => {}
                Err(err) => {
                    self.fallo_permiso(&err, Permiso::Usar);
                    self.contactos[i].proximo = ahora + REINTENTO_MS;
                }
            }
            wipe(&mut e);
        }
    }

    fn olvidar(&mut self) {
        let (ahora, sel) = (self.ahora, self.seleccionado);
        let antes = self.contactos.len();
        self.contactos.retain(|c| c.guardado || sel == Some(c.pk) || c.visto.as_ref().is_some_and(|v| ahora < v.ms.saturating_add(OLVIDO_MS)));
        if self.contactos.len() != antes {
            self.cambio = true;
        }
    }

    // ------------------------------------------------------------ conexiones

    fn conducir_todas(&mut self, sys: &mut dyn Platform) {
        for _ in 0..self.conns.len() {
            let mut c = self.conns.remove(0);
            let fin = self.conducir(sys, &mut c);
            if matches!(fin, Fin::Sigue) {
                self.conns.push(c);
                continue;
            }
            let bien = matches!(fin, Fin::Bien);
            if let Some(net) = sys.net() {
                if bien {
                    net.tcp_close(c.sock);
                } else {
                    net.tcp_abort(c.sock);
                }
            }
            if let (true, Some(pk)) = (c.inicia, c.destino) {
                let ahora = self.ahora;
                if let Some(ct) = self.contactos.iter_mut().find(|x| x.pk == pk) {
                    // Si queda algo pendiente tras una sesión buena, se vuelve a intentar enseguida (con un respiro).
                    ct.proximo = ahora + if bien { 1000 } else { REINTENTO_MS };
                }
            }
        }
    }

    fn conducir(&mut self, sys: &mut dyn Platform, c: &mut Conn) -> Fin {
        let ahora = self.ahora;
        let tarde = ahora.saturating_sub(c.inicio);
        if tarde > CONEXION_MS || (c.etapa == Etapa::EsperaHola && !c.inicia && tarde > SALUDO_MS) {
            return Fin::Mal;
        }
        let Some(net) = sys.net() else { return Fin::Mal };
        let st = net.tcp_state(c.sock);
        if c.etapa == Etapa::Conectando {
            match st {
                TcpState::Connecting => return Fin::Sigue,
                TcpState::Open => {
                    c.salida.extend_from_slice(&c.hola_propio);
                    c.etapa = Etapa::EsperaHola;
                }
                _ => return Fin::Mal,
            }
        }
        if matches!(st, TcpState::Failed(_) | TcpState::Closed) {
            return Fin::Mal;
        }
        // Recibir, con tope en el búfer: lo que sobre espera en el de la pila.
        let mut vacio = false;
        for _ in 0..4 {
            let libre = ENTRADA_MAX - c.entrada.len();
            if libre == 0 {
                break;
            }
            let mut buf = [0u8; 2048];
            let k = libre.min(buf.len());
            match net.tcp_recv(c.sock, &mut buf[..k]) {
                Ok(0) => {
                    vacio = true;
                    break;
                }
                Ok(n) => c.entrada.extend_from_slice(&buf[..n.min(k)]),
                Err(_) => return Fin::Mal,
            }
        }
        if c.etapa == Etapa::EsperaHola && c.entrada.len() >= HOLA_LEN {
            let Ok(h) = <[u8; HOLA_LEN]>::try_from(&c.entrada[..HOLA_LEN]) else { return Fin::Mal };
            c.entrada.drain(..HOLA_LEN);
            if !self.saludo(c, &h) {
                return Fin::Mal;
            }
        }
        while c.etapa == Etapa::Abierta {
            let Some(s) = c.sesion.as_mut() else { return Fin::Mal };
            match s.abrir(&c.entrada) {
                Ok(Some((cuerpo, usados))) => {
                    c.entrada.drain(..usados);
                    let Some(t) = decodificar(&cuerpo) else { return Fin::Mal };
                    if !self.trama(c, t) {
                        return Fin::Mal;
                    }
                }
                Ok(None) => break,
                Err(()) => return Fin::Mal,
            }
        }
        let Some(net) = sys.net() else { return Fin::Mal };
        while !c.salida.is_empty() {
            match net.tcp_send(c.sock, &c.salida) {
                Ok(0) => break,
                Ok(n) => {
                    c.salida.drain(..n.min(c.salida.len()));
                }
                Err(_) => return Fin::Mal,
            }
        }
        if c.fin && c.salida.is_empty() {
            return Fin::Bien;
        }
        if matches!(st, TcpState::PeerClosed) && vacio {
            return Fin::Mal;
        }
        Fin::Sigue
    }

    /// Procesa el saludo del otro lado. `false` = cortar.
    fn saludo(&mut self, c: &mut Conn, h: &[u8; HOLA_LEN]) -> bool {
        if c.inicia {
            let (Some((s_r, _)), Some(destino)) = (leer_hola(h, HOLA_R), c.destino) else { return false };
            if !ct_eq(&s_r, &destino) {
                return false;
            }
            c.sesion = derivar(true, &self.privada, &c.efimera, &c.hola_propio, h);
        } else {
            let Some((s_i, _)) = leer_hola(h, HOLA_I) else { return false };
            if ct_eq(&s_i, &self.publica) {
                return false;
            }
            let hr = hola(HOLA_R, &self.publica, &publica(&c.efimera));
            c.sesion = derivar(false, &self.privada, &c.efimera, h, &hr);
            c.destino = Some(s_i);
            c.salida.extend_from_slice(&hr);
        }
        wipe(&mut c.efimera);
        if c.sesion.is_none() {
            return false;
        }
        c.etapa = Etapa::Abierta;
        if c.inicia {
            self.encolar(c);
        }
        true
    }

    /// *I*: los mensajes pendientes del contacto, y el adiós si no hay nada que esperar.
    fn encolar(&mut self, c: &mut Conn) {
        let Some(pk) = c.destino else { return };
        if let Some(ct) = self.contactos.iter().find(|x| x.pk == pk) {
            for m in ct.msgs.iter().filter(|m| m.saliente && m.estado == Estado::Pendiente).take(MAX_POR_CONEXION) {
                if c.enviar(&Trama::Texto { id: m.id, fecha: m.fecha, texto: m.texto.clone() }) {
                    c.esperando.push(m.id);
                }
            }
        }
        if c.esperando.is_empty() && c.enviar(&Trama::Adios) {
            c.fin = true;
        }
    }

    /// Una trama ya descifrada. `false` = cortar.
    fn trama(&mut self, c: &mut Conn, t: Trama) -> bool {
        match (c.inicia, t) {
            (true, Trama::Recibido(id)) => {
                let Some(pos) = c.esperando.iter().position(|x| *x == id) else { return false };
                c.esperando.swap_remove(pos);
                if let Some(m) = c.destino.and_then(|pk| self.contactos.iter_mut().find(|x| x.pk == pk)).and_then(|ct| {
                    ct.sucio = true;
                    ct.msgs.iter_mut().find(|m| m.saliente && m.id == id)
                }) {
                    m.estado = Estado::Entregado;
                }
                self.cambio = true;
                if c.esperando.is_empty() && !c.fin {
                    c.fin = true;
                    return c.enviar(&Trama::Adios);
                }
                true
            }
            (false, Trama::Texto { id, fecha, texto }) => {
                c.textos += 1;
                let Some(pk) = c.destino else { return false };
                c.textos <= MAX_TEXTOS_ENTRANTES && self.recibir_texto(pk, id, fecha, texto) && c.enviar(&Trama::Recibido(id))
            }
            (false, Trama::Adios) => {
                c.fin = true;
                true
            }
            _ => false,
        }
    }

    fn recibir_texto(&mut self, pk: Clave, id: u64, fecha: DateTime, texto: String) -> bool {
        let guardados = self.contactos.iter().filter(|c| c.guardado).count();
        let i = match self.contactos.iter().position(|c| c.pk == pk) {
            Some(i) => i,
            None => {
                self.contactos.push(Contacto::nuevo(pk, format!("Equipo {}", &huella(&pk)[..8]), false));
                self.contactos.len() - 1
            }
        };
        let sel = self.seleccionado == Some(pk);
        let c = &mut self.contactos[i];
        // Un id ya visto se confirma igual pero no se guarda otra vez.
        if c.msgs.iter().any(|m| !m.saliente && m.id == id) {
            return true;
        }
        if !c.guardado && guardados >= MAX_GUARDADOS {
            return false;
        }
        c.guardado = true;
        c.sin_leer = !sel;
        c.msgs.push(Mensaje { saliente: false, fecha, id, estado: Estado::Recibido, texto });
        c.recortar();
        c.sucio = true;
        self.sucio = true;
        self.cambio = true;
        true
    }

    // ------------------------------------------------------------ disco

    /// Escribe lo que haya cambiado, como mucho una vez por segundo (o ya, con `forzar`).
    pub fn guardar(&mut self, sys: &mut dyn Platform, forzar: bool) {
        if !self.sucio && !self.contactos.iter().any(|c| c.sucio) {
            return;
        }
        if !forzar && self.ahora < self.ult_guardado.saturating_add(GUARDAR_MS) {
            return;
        }
        self.ult_guardado = self.ahora;
        if self.sucio {
            self.sucio = false;
            let fichas: Vec<Ficha> = self.contactos.iter().filter(|c| c.guardado).map(|c| Ficha { pk: c.pk, nombre: c.nombre.clone(), verificado: c.verificado }).collect();
            escribir(sys, &format!("{}\\contactos", self.carpeta), contactos_a_texto(&fichas).as_bytes());
        }
        for c in self.contactos.iter_mut().filter(|c| c.sucio && c.guardado) {
            c.sucio = false;
            escribir(sys, &format!("{}\\{}.txt", self.carpeta, huella(&c.pk)), historial_a_texto(&c.msgs).as_bytes());
        }
    }
}
