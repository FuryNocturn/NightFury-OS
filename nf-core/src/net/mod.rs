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

/// La pila de red. Un solo adaptador.
pub struct Net {
    _todo: (),
}

impl Net {
    /// `secret`: 32 bytes impredecibles del acumulador del sistema (para los números de secuencia
    /// de TCP, el identificador de DHCP y los puertos e identificadores de DNS).
    pub fn new(_secret: [u8; 32]) -> Net {
        todo!()
    }

    /// Engancha la tarjeta. Con cable, empieza DHCP en el siguiente `poll`.
    pub fn attach(&mut self, _dev: Box<dyn NetDev>) {
        todo!()
    }

    /// Recibe y procesa las tramas pendientes, envía lo que toque (DHCP, ARP, retransmisiones de TCP)
    /// y vence los plazos. Mira el enlace: si se quita el cable, se pierde la dirección; al volver,
    /// DHCP empieza de nuevo. Renueva la concesión a la mitad de su duración.
    pub fn poll(&mut self, _now_ms: u64) {
        todo!()
    }

    pub fn status(&self) -> NetStatus {
        todo!()
    }

    /// `ipconfig /renew` y el botón «Renovar dirección»: suelta la concesión y empieza DHCP de nuevo.
    pub fn renew(&mut self) {
        todo!()
    }

    /// Envía un eco ICMP con `bytes` de datos y el `ttl` dado. El resultado llega por `ping_poll`.
    pub fn ping(&mut self, _dst: Ipv4, _bytes: u16, _ttl: u8) -> Result<PingId, NetError> {
        todo!()
    }

    /// `None` mientras espera; después, la respuesta o el error (`Timeout` a los `PING_TIMEOUT_MS`).
    /// Tras devolver `Some`, el identificador se libera.
    pub fn ping_poll(&mut self, _id: PingId) -> Option<Result<PingReply, NetError>> {
        todo!()
    }

    /// Resuelve un nombre (registros A) con el DNS de DHCP, con caché según el TTL. Un texto que ya
    /// es una IP se devuelve tal cual.
    pub fn resolve(&mut self, _name: &str) -> Result<QueryId, NetError> {
        todo!()
    }

    /// Como `ping_poll`.
    pub fn resolve_poll(&mut self, _id: QueryId) -> Option<Result<Vec<Ipv4>, NetError>> {
        todo!()
    }

    /// Abre una conexión TCP. El estado se mira con `tcp_state`.
    pub fn tcp_connect(&mut self, _dst: Ipv4, _port: u16) -> Result<SockId, NetError> {
        todo!()
    }

    pub fn tcp_state(&self, _s: SockId) -> TcpState {
        todo!()
    }

    /// Copia en el búfer de envío lo que quepa y devuelve cuánto (0 si está lleno). Error si la
    /// conexión no está abierta.
    pub fn tcp_send(&mut self, _s: SockId, _data: &[u8]) -> Result<usize, NetError> {
        todo!()
    }

    /// Copia en `buf` lo recibido y devuelve cuánto (0 si no hay nada todavía: mira `tcp_state` para
    /// saber si ya no llegará más).
    pub fn tcp_recv(&mut self, _s: SockId, _buf: &mut [u8]) -> Result<usize, NetError> {
        todo!()
    }

    /// Cierre ordenado (FIN tras enviar lo pendiente). El identificador queda libre; la pila termina
    /// el cierre por su cuenta.
    pub fn tcp_close(&mut self, _s: SockId) {
        todo!()
    }

    /// Corta la conexión con RST y libera el identificador.
    pub fn tcp_abort(&mut self, _s: SockId) {
        todo!()
    }
}

/// Lee «a.b.c.d» (cuatro números de 0 a 255, sin ceros a la izquierda raros ni espacios).
pub fn parse_ip(_s: &str) -> Option<Ipv4> {
    todo!()
}

/// «a.b.c.d».
pub fn ip_text(ip: Ipv4) -> String {
    alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

/// «AA-BB-CC-DD-EE-FF», como `ipconfig /all`.
pub fn mac_text(m: [u8; 6]) -> String {
    alloc::format!("{:02X}-{:02X}-{:02X}-{:02X}-{:02X}-{:02X}", m[0], m[1], m[2], m[3], m[4], m[5])
}
