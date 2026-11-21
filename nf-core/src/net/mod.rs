// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Red (v0.12): pila IPv4 propia sobre una tarjeta de cable cualquiera.
//!
//! CONTRATO: los tipos y firmas públicos de este archivo y de `http.rs` no cambian.
//!
//! Reparto:
//! - El núcleo busca la tarjeta, la entrega con `Net::attach` y llama a `Net::poll` en cada vuelta
//!   del bucle principal. La pila no sabe qué tarjeta es.
//! - Las apps del sistema (Terminal, bandeja, Configuración) usan `Platform::net()`: estado,
//!   `ping`, `resolve` y TCP. Todo es asíncrono: se empieza una operación y se pregunta por ella
//!   en los ticks siguientes, sin bloquear el escritorio.
//! - Todo lo que llega por la red es entrada hostil: se valida y nunca provoca pánico ni bucles.
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

pub mod http;
mod dhcp;
mod dns;
mod icmp;
mod ip;
mod pkt;
mod tcb;
mod tcp;
mod udp;
#[cfg(test)]
pub(crate) mod tests;

/// Dirección IPv4, en el orden en que se escribe (`[192, 168, 1, 1]`).
pub type Ipv4 = [u8; 4];

/// Enlace de la tarjeta cuando hay cable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Link {
    pub speed_mbps: u32,
    pub full_duplex: bool,
}

/// Una tarjeta de red a nivel de tramas Ethernet. La implementa el núcleo para cada controlador;
/// los tests, con tarjetas en memoria.
pub trait NetDev {
    /// Nombre para Configuración e `ipconfig /all` («Intel I225-V», «Realtek RTL8168h»…).
    fn name(&self) -> String;
    fn mac(&self) -> [u8; 6];
    /// `None` si no hay cable (o no hay enlace).
    fn link(&mut self) -> Option<Link>;
    /// Envía una trama completa sin CRC (cabecera Ethernet incluida, 14..=1514 bytes; la tarjeta
    /// rellena las cortas). `false` si la cola de envío está llena o hay un fallo.
    fn send(&mut self, frame: &[u8]) -> bool;
    /// Copia en `buf` (al menos 1514 bytes) la siguiente trama recibida, sin CRC, y devuelve su
    /// longitud. `None` si no hay ninguna.
    fn recv(&mut self, buf: &mut [u8]) -> Option<usize>;
}

/// Estado de la red, para la bandeja y Configuración.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// No hay ninguna tarjeta compatible.
    NoAdapter,
    /// Hay tarjeta pero no cable.
    NoCable,
    /// Pidiendo la dirección por DHCP.
    Configuring,
    /// Sin dirección tras el plazo de DHCP, o con dirección pero la puerta de enlace no responde a ARP.
    NoInternet,
    Connected,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NetStatus {
    pub state: State,
    /// Nombre de la tarjeta ("" si no hay).
    pub adapter: String,
    pub mac: [u8; 6],
    /// 0 sin enlace.
    pub speed_mbps: u32,
    pub full_duplex: bool,
    pub ip: Option<Ipv4>,
    pub mask: Option<Ipv4>,
    pub gateway: Option<Ipv4>,
    pub dns: Vec<Ipv4>,
    pub dhcp_server: Option<Ipv4>,
    /// Duración de la concesión DHCP, en segundos (0 sin concesión).
    pub lease_s: u32,
    /// `now_ms` de `poll` cuando se obtuvo la concesión.
    pub lease_since_ms: u64,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NetError {
    NoAdapter,
    NoCable,
    /// Todavía no hay dirección IP.
    NoAddress,
    /// El nombre no existe (DNS «NXDOMAIN» o sin direcciones A).
    NotFound,
    Timeout,
    /// TCP: el destino respondió con RST al conectar.
    Refused,
    /// TCP: la conexión se cortó con RST.
    Reset,
    /// ICMP «destino inaccesible» (red, equipo o puerto), con quien lo envió.
    Unreachable(Ipv4),
    /// ICMP «tiempo excedido» (TTL caducado en tránsito), con quien lo envió.
    TtlExpired(Ipv4),
    /// Demasiadas operaciones o conexiones a la vez.
    Busy,
    /// Identificador que ya no existe (operación terminada o liberada).
    Gone,
    BadUrl,
    /// `https://`: llega en v0.12.2.
    HttpsNotYet,
    /// Respuesta mal formada, con un motivo corto en español.
    Protocol(&'static str),
    /// v0.12.1: el usuario no ha dado permiso a este programa (o lo ha quitado en Configuración).
    Denied,
    /// v0.12.1: el aviso de permiso está en pantalla. El programa vuelve a intentarlo más tarde.
    Pending,
    /// v0.12.1: el puerto ya lo usa otro (o, para un programa de terceros, es menor que 1024).
    InUse,
}

/// v0.12.1: un socket UDP y un puerto TCP a la escucha.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub struct UdpId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub struct ListenId(pub u32);

/// Un datagrama UDP recibido.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Datagram {
    pub from: Ipv4,
    pub port: u16,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Proto {
    Tcp,
    /// Puerto TCP a la escucha (sin otro extremo).
    TcpListen,
    Udp,
}

/// Una conexión o socket abierto, para el Administrador de tareas.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ConnInfo {
    pub proto: Proto,
    /// El número de `SockId`, `UdpId` o `ListenId` según `proto`.
    pub id: u32,
    pub local_port: u16,
    /// Otro extremo (TCP); `None` en UDP y a la escucha.
    pub remote: Option<(Ipv4, u16)>,
    /// Texto corto en español: «Conectando», «Abierta», «Cerrando», «Cerrada», «Escuchando», «—».
    pub state: &'static str,
    /// Bytes de datos recibidos y enviados (sin cabeceras) desde que se abrió.
    pub rx: u64,
    pub tx: u64,
}

/// Respuesta a un `ping`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PingReply {
    pub from: Ipv4,
    /// Bytes de datos del eco (sin cabeceras).
    pub bytes: u16,
    pub rtt_ms: u64,
    pub ttl: u8,
}

/// Identificadores de operaciones en marcha.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PingId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct QueryId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub struct SockId(pub u32);

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TcpState {
    Connecting,
    Open,
    /// El otro extremo cerró (FIN): se puede seguir leyendo lo que queda en el búfer.
    PeerClosed,
    /// Cerrada limpiamente por los dos lados.
    Closed,
    Failed(NetError),
}

/// Plazos (ms).
pub const PING_TIMEOUT_MS: u64 = 4_000;
pub const DNS_TIMEOUT_MS: u64 = 5_000;
pub const CONNECT_TIMEOUT_MS: u64 = 20_000;
/// Sin dirección tras este plazo, el estado pasa a `NoInternet` (DHCP sigue reintentando).
pub const DHCP_TIMEOUT_MS: u64 = 15_000;
/// Topes: conexiones TCP, pings y consultas DNS a la vez; búferes de cada conexión.
pub const MAX_SOCKS: usize = 16;
pub const MAX_PINGS: usize = 16;
pub const MAX_QUERIES: usize = 16;
pub const TCP_BUF: usize = 64 * 1024;
/// v0.12.1: sockets UDP y puertos a la escucha a la vez (en toda la pila), datagramas en cola por
/// socket UDP, conexiones esperando `tcp_accept` por puerto, y datos máximos de un datagrama (sin
/// fragmentar: 1500 − 20 − 8).
pub const MAX_UDP: usize = 32;
pub const MAX_LISTEN: usize = 16;
pub const UDP_QUEUE: usize = 32;
pub const LISTEN_BACKLOG: usize = 8;
pub const UDP_MAX: usize = 1472;

/// Configuración obtenida por DHCP.
pub(crate) struct Cfg {
    pub ip: Ipv4,
    pub mask: Ipv4,
    pub gw: Option<Ipv4>,
    pub dns: Vec<Ipv4>,
    pub server: Ipv4,
    pub lease_s: u32,
    pub since: u64,
}

/// La pila de red. Un solo adaptador.
pub struct Net {
    secret: [u8; 32],
    ctr: u64,
    now: u64,
    dev: Option<Box<dyn NetDev>>,
    mac: [u8; 6],
    link: Option<Link>,
    rxbuf: Vec<u8>,
    cfg: Option<Cfg>,
    dhcp: dhcp::Dhcp,
    arp: ip::ArpState,
    ip_id: u16,
    pings: Vec<icmp::PingOp>,
    next_ping: u32,
    ping_seq: u16,
    queries: Vec<dns::Query>,
    next_query: u32,
    dns_cache: Vec<dns::CacheEntry>,
    socks: Vec<tcb::Sock>,
    next_sock: u32,
}

impl Net {
    /// `secret`: 32 bytes impredecibles del acumulador del sistema (para los números de secuencia
    /// de TCP, el identificador de DHCP y los puertos e identificadores de DNS).
    pub fn new(secret: [u8; 32]) -> Net {
        Net {
            secret,
            ctr: 0,
            now: 0,
            dev: None,
            mac: [0; 6],
            link: None,
            rxbuf: alloc::vec![0; 1514],
            cfg: None,
            dhcp: dhcp::Dhcp::default(),
            arp: ip::ArpState::default(),
            ip_id: 0,
            pings: Vec::new(),
            next_ping: 0,
            ping_seq: 0,
            queries: Vec::new(),
            next_query: 0,
            dns_cache: Vec::new(),
            socks: Vec::new(),
            next_sock: 0,
        }
    }

    /// Engancha la tarjeta. Con cable, empieza DHCP en el siguiente `poll`.
    pub fn attach(&mut self, dev: Box<dyn NetDev>) {
        self.mac = dev.mac();
        self.dev = Some(dev);
        self.link = None;
        self.lose_config();
        self.dhcp = dhcp::Dhcp::default();
    }

    /// Recibe y procesa las tramas pendientes, envía lo que toque (DHCP, ARP, retransmisiones de TCP)
    /// y vence los plazos. Mira el enlace: si se quita el cable, se pierde la dirección; al volver,
    /// DHCP empieza de nuevo. Renueva la concesión a la mitad de su duración.
    pub fn poll(&mut self, now_ms: u64) {
        self.now = now_ms;
        let Some(dev) = self.dev.as_mut() else { return };
        let link = dev.link();
        if link.is_some() != self.link.is_some() {
            if link.is_some() {
                self.dhcp_start();
            } else {
                self.lose_config();
                self.dhcp.st = dhcp::DState::Off;
            }
        }
        self.link = link;
        if link.is_none() {
            return;
        }
        self.rx_all();
        self.arp_tick();
        self.dhcp_tick();
        self.icmp_tick();
        self.dns_tick();
        self.tcp_tick();
    }

    pub fn status(&self) -> NetStatus {
        let state = if self.dev.is_none() {
            State::NoAdapter
        } else if self.link.is_none() {
            State::NoCable
        } else if let Some(c) = &self.cfg {
            if self.arp.gw_fail {
                State::NoInternet
            } else if c.gw.is_some() && !self.arp.gw_ok {
                State::Configuring
            } else {
                State::Connected
            }
        } else if self.now.saturating_sub(self.dhcp.since) >= DHCP_TIMEOUT_MS {
            State::NoInternet
        } else {
            State::Configuring
        };
        let c = self.cfg.as_ref();
        NetStatus {
            state,
            adapter: self.dev.as_ref().map(|d| d.name()).unwrap_or_default(),
            mac: self.mac,
            speed_mbps: self.link.map_or(0, |l| l.speed_mbps),
            full_duplex: self.link.is_some_and(|l| l.full_duplex),
            ip: c.map(|c| c.ip),
            mask: c.map(|c| c.mask),
            gateway: c.and_then(|c| c.gw),
            dns: c.map(|c| c.dns.clone()).unwrap_or_default(),
            dhcp_server: c.map(|c| c.server),
            lease_s: c.map_or(0, |c| c.lease_s),
            lease_since_ms: c.map_or(0, |c| c.since),
        }
    }

    /// `ipconfig /renew` y el botón «Renovar dirección»: suelta la concesión y empieza DHCP de nuevo.
    pub fn renew(&mut self) {
        self.dhcp_release();
        self.lose_config();
        if self.link.is_some() {
            self.dhcp_start();
        }
    }

    /// Envía un eco ICMP con `bytes` de datos y el `ttl` dado. El resultado llega por `ping_poll`.
    pub fn ping(&mut self, dst: Ipv4, bytes: u16, ttl: u8) -> Result<PingId, NetError> {
        self.ping_start(dst, bytes, ttl)
    }

    /// `None` mientras espera; después, la respuesta o el error (`Timeout` a los `PING_TIMEOUT_MS`).
    /// Tras devolver `Some`, el identificador se libera.
    pub fn ping_poll(&mut self, id: PingId) -> Option<Result<PingReply, NetError>> {
        self.ping_result(id)
    }

    /// Resuelve un nombre (registros A) con el DNS de DHCP, con caché según el TTL. Un texto que ya
    /// es una IP se devuelve tal cual.
    pub fn resolve(&mut self, name: &str) -> Result<QueryId, NetError> {
        self.dns_start(name)
    }

    /// Como `ping_poll`.
    pub fn resolve_poll(&mut self, id: QueryId) -> Option<Result<Vec<Ipv4>, NetError>> {
        self.dns_result(id)
    }

    /// Abre una conexión TCP. El estado se mira con `tcp_state`.
    pub fn tcp_connect(&mut self, dst: Ipv4, port: u16) -> Result<SockId, NetError> {
        self.tcp_open(dst, port)
    }

    pub fn tcp_state(&self, s: SockId) -> TcpState {
        match self.socks.iter().find(|k| k.id == s.0 && !k.released) {
            Some(k) => k.state(),
            None => TcpState::Failed(NetError::Gone),
        }
    }

    /// Copia en el búfer de envío lo que quepa y devuelve cuánto (0 si está lleno). Error si la
    /// conexión no está abierta.
    pub fn tcp_send(&mut self, s: SockId, data: &[u8]) -> Result<usize, NetError> {
        self.tcp_write(s, data)
    }

    /// Copia en `buf` lo recibido y devuelve cuánto (0 si no hay nada todavía: mira `tcp_state` para
    /// saber si ya no llegará más).
    pub fn tcp_recv(&mut self, s: SockId, buf: &mut [u8]) -> Result<usize, NetError> {
        self.tcp_read(s, buf)
    }

    /// Cierre ordenado (FIN tras enviar lo pendiente). El identificador queda libre; la pila termina
    /// el cierre por su cuenta.
    pub fn tcp_close(&mut self, s: SockId) {
        self.tcp_shut(s, false)
    }

    /// Corta la conexión con RST y libera el identificador.
    pub fn tcp_abort(&mut self, s: SockId) {
        self.tcp_shut(s, true)
    }

    // ---------- v0.12.1: UDP, TCP a la escucha y lista de conexiones (`udp.rs`, `tcb.rs`) ----------
    //
    // Regla de «solo red local» (la aplica la pila, no el servicio): «red local» = misma subred que
    // nuestra IP según la máscara de DHCP (sin dirección no hay nada que recibir).
    // - Un socket UDP recibe de quien le haya enviado él antes (misma IP y puerto: la respuesta de un
    //   servidor de Internet entra) y, si se abrió con un puerto fijo, además de cualquier equipo de la
    //   red local, también por difusión (255.255.255.255 o la de la subred). Lo demás se descarta.
    // - Un puerto a la escucha solo acepta SYN de la red local; a los de fuera, RST.
    // - Los puertos que usa la pila (68 de DHCP, los de las consultas DNS en curso) no se dan; un
    //   datagrama a uno de esos va a la pila, no al socket.

    /// Abre un socket UDP. `port` 0 = uno efímero libre (49152-65535). `InUse` si el puerto está
    /// cogido; `Busy` con `MAX_UDP` abiertos. Sin dirección también se puede abrir.
    pub fn udp_bind(&mut self, port: u16) -> Result<UdpId, NetError> {
        self.udp_open(port)
    }

    /// Envía un datagrama de hasta `UDP_MAX` bytes (más: `Protocol("datagrama demasiado grande")`).
    /// `dst` puede ser 255.255.255.255 o la difusión de la subred; multidifusión no
    /// (`Unreachable(dst)`). Si hay que preguntar la MAC por ARP, sale cuando llegue la respuesta (como
    /// el resto de la pila). Sin dirección, `NoAddress`.
    pub fn udp_send(&mut self, s: UdpId, dst: Ipv4, port: u16, data: &[u8]) -> Result<(), NetError> {
        self.udp_write(s, dst, port, data)
    }

    /// El siguiente datagrama de la cola (`UDP_QUEUE` como mucho: los que llegan con la cola llena se
    /// tiran). `Ok(None)` si no hay; `Gone` si el identificador no existe.
    pub fn udp_recv(&mut self, s: UdpId) -> Result<Option<Datagram>, NetError> {
        self.udp_read(s)
    }

    pub fn udp_close(&mut self, s: UdpId) {
        self.udp_shut(s)
    }

    /// Pone un puerto TCP a la escucha (`port` ≠ 0). `InUse` si ya escucha otro; `Busy` con
    /// `MAX_LISTEN`. Hasta `LISTEN_BACKLOG` conexiones esperan `tcp_accept`; con la cola llena, el SYN
    /// se ignora (el otro reintenta).
    pub fn tcp_listen(&mut self, port: u16) -> Result<ListenId, NetError> {
        self.listen_open(port)
    }

    /// Una conexión ya establecida (tras el saludo de tres pasos) con su IP y puerto de origen, o
    /// `Ok(None)` si no hay ninguna. Desde aquí es una `SockId` normal (cuenta en `MAX_SOCKS`; si no
    /// cabe, se queda en la cola). `Gone` si el identificador no existe.
    pub fn tcp_accept(&mut self, l: ListenId) -> Result<Option<(SockId, Ipv4, u16)>, NetError> {
        self.listen_accept(l)
    }

    /// Deja de escuchar; las conexiones en cola sin aceptar se cortan con RST.
    pub fn tcp_unlisten(&mut self, l: ListenId) {
        self.listen_shut(l)
    }

    /// Todo lo abierto (TCP sin liberar, UDP y a la escucha), con sus contadores.
    pub fn connections(&self) -> Vec<ConnInfo> {
        self.conn_list()
    }

    /// 32 bits impredecibles: SHA-256(secreto ‖ contador).
    pub(crate) fn rand(&mut self) -> u32 {
        let mut m = [0u8; 40];
        m[..32].copy_from_slice(&self.secret);
        m[32..].copy_from_slice(&self.ctr.to_be_bytes());
        self.ctr = self.ctr.wrapping_add(1);
        let h = crate::crypto::sha256(&m);
        u32::from_be_bytes([h[0], h[1], h[2], h[3]])
    }

    /// Nuestra IP si se puede operar (hay tarjeta, cable y dirección).
    pub(crate) fn ready(&self) -> Result<Ipv4, NetError> {
        if self.dev.is_none() {
            return Err(NetError::NoAdapter);
        }
        if self.link.is_none() {
            return Err(NetError::NoCable);
        }
        self.cfg.as_ref().map(|c| c.ip).ok_or(NetError::NoAddress)
    }

    /// Se pierde la dirección (cable, caducidad o `renew`): caen ARP y conexiones.
    pub(crate) fn lose_config(&mut self) {
        self.cfg = None;
        self.arp = ip::ArpState::default();
        self.dns_cache.clear();
        for s in self.socks.iter_mut() {
            s.kill(NetError::Reset);
        }
        self.socks.retain(|s| !s.released);
    }

    fn rx_all(&mut self) {
        let mut buf = core::mem::take(&mut self.rxbuf);
        for _ in 0..128 {
            let Some(n) = self.dev.as_mut().and_then(|d| d.recv(&mut buf)) else { break };
            if n <= buf.len() {
                self.rx(&buf[..n]);
            }
        }
        self.rxbuf = buf;
    }
}

/// Lo que las apps usan de la pila. La implementa `Net` (el sistema) y `RemoteNet` (un programa en
/// modo usuario, que lo pide al sistema por `SERVICE`). Mismas reglas que los métodos de `Net`.
pub trait NetApi {
    fn status(&mut self) -> NetStatus;
    fn renew(&mut self);
    fn ping(&mut self, dst: Ipv4, bytes: u16, ttl: u8) -> Result<PingId, NetError>;
    fn ping_poll(&mut self, id: PingId) -> Option<Result<PingReply, NetError>>;
    fn resolve(&mut self, name: &str) -> Result<QueryId, NetError>;
    fn resolve_poll(&mut self, id: QueryId) -> Option<Result<Vec<Ipv4>, NetError>>;
    fn tcp_connect(&mut self, dst: Ipv4, port: u16) -> Result<SockId, NetError>;
    fn tcp_state(&mut self, s: SockId) -> TcpState;
    fn tcp_send(&mut self, s: SockId, data: &[u8]) -> Result<usize, NetError>;
    fn tcp_recv(&mut self, s: SockId, buf: &mut [u8]) -> Result<usize, NetError>;
    fn tcp_close(&mut self, s: SockId);
    fn tcp_abort(&mut self, s: SockId);
    fn udp_bind(&mut self, port: u16) -> Result<UdpId, NetError>;
    fn udp_send(&mut self, s: UdpId, dst: Ipv4, port: u16, data: &[u8]) -> Result<(), NetError>;
    fn udp_recv(&mut self, s: UdpId) -> Result<Option<Datagram>, NetError>;
    fn udp_close(&mut self, s: UdpId);
    fn tcp_listen(&mut self, port: u16) -> Result<ListenId, NetError>;
    fn tcp_accept(&mut self, l: ListenId) -> Result<Option<(SockId, Ipv4, u16)>, NetError>;
    fn tcp_unlisten(&mut self, l: ListenId);
    fn connections(&mut self) -> Vec<ConnInfo>;
}

impl NetApi for Net {
    fn status(&mut self) -> NetStatus {
        Net::status(self)
    }
    fn renew(&mut self) {
        Net::renew(self)
    }
    fn ping(&mut self, dst: Ipv4, bytes: u16, ttl: u8) -> Result<PingId, NetError> {
        Net::ping(self, dst, bytes, ttl)
    }
    fn ping_poll(&mut self, id: PingId) -> Option<Result<PingReply, NetError>> {
        Net::ping_poll(self, id)
    }
    fn resolve(&mut self, name: &str) -> Result<QueryId, NetError> {
        Net::resolve(self, name)
    }
    fn resolve_poll(&mut self, id: QueryId) -> Option<Result<Vec<Ipv4>, NetError>> {
        Net::resolve_poll(self, id)
    }
    fn tcp_connect(&mut self, dst: Ipv4, port: u16) -> Result<SockId, NetError> {
        Net::tcp_connect(self, dst, port)
    }
    fn tcp_state(&mut self, s: SockId) -> TcpState {
        Net::tcp_state(self, s)
    }
    fn tcp_send(&mut self, s: SockId, data: &[u8]) -> Result<usize, NetError> {
        Net::tcp_send(self, s, data)
    }
    fn tcp_recv(&mut self, s: SockId, buf: &mut [u8]) -> Result<usize, NetError> {
        Net::tcp_recv(self, s, buf)
    }
    fn tcp_close(&mut self, s: SockId) {
        Net::tcp_close(self, s)
    }
    fn tcp_abort(&mut self, s: SockId) {
        Net::tcp_abort(self, s)
    }
    fn udp_bind(&mut self, port: u16) -> Result<UdpId, NetError> {
        Net::udp_bind(self, port)
    }
    fn udp_send(&mut self, s: UdpId, dst: Ipv4, port: u16, data: &[u8]) -> Result<(), NetError> {
        Net::udp_send(self, s, dst, port, data)
    }
    fn udp_recv(&mut self, s: UdpId) -> Result<Option<Datagram>, NetError> {
        Net::udp_recv(self, s)
    }
    fn udp_close(&mut self, s: UdpId) {
        Net::udp_close(self, s)
    }
    fn tcp_listen(&mut self, port: u16) -> Result<ListenId, NetError> {
        Net::tcp_listen(self, port)
    }
    fn tcp_accept(&mut self, l: ListenId) -> Result<Option<(SockId, Ipv4, u16)>, NetError> {
        Net::tcp_accept(self, l)
    }
    fn tcp_unlisten(&mut self, l: ListenId) {
        Net::tcp_unlisten(self, l)
    }
    fn connections(&mut self) -> Vec<ConnInfo> {
        Net::connections(self)
    }
}

/// Lee «a.b.c.d» (cuatro números de 0 a 255, sin ceros a la izquierda raros ni espacios).
pub fn parse_ip(s: &str) -> Option<Ipv4> {
    let mut ip = [0u8; 4];
    let mut it = s.split('.');
    for b in ip.iter_mut() {
        let p = it.next()?;
        // solo cifras, sin ceros a la izquierda («01» no vale) y 3 como mucho
        if p.is_empty() || p.len() > 3 || !p.bytes().all(|c| c.is_ascii_digit()) || (p.len() > 1 && p.starts_with('0')) {
            return None;
        }
        *b = p.parse().ok()?;
    }
    if it.next().is_some() {
        return None;
    }
    Some(ip)
}

/// «a.b.c.d».
pub fn ip_text(ip: Ipv4) -> String {
    alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

/// «AA-BB-CC-DD-EE-FF», como `ipconfig /all`.
pub fn mac_text(m: [u8; 6]) -> String {
    alloc::format!("{:02X}-{:02X}-{:02X}-{:02X}-{:02X}-{:02X}", m[0], m[1], m[2], m[3], m[4], m[5])
}
