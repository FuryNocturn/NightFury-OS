// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Servicio de archivos y del equipo para los programas en modo usuario (v0.10).
//!
//! El programa codifica una `Call` con `wire` y la manda con `SYS_SERVICE`; el escritorio la atiende
//! con `serve` sobre el sistema real y devuelve la respuesta. `RemoteFiles` es el lado del programa:
//! un `FilesApi` que hace ese viaje en cada método. Respuesta = byte 0 + valor (o byte 1 si la
//! petición estaba rota).
//!
//! Política (`serve`): un programa tiene los permisos del usuario de la sesión (los aplica `Files`) y
//! además
//! - nunca escribe ni borra bajo `C:\Sistema` (ni mueve o copia desde o hacia ahí);
//! - cuentas, claves, cifrado, registro, reiniciar y apagar solo valen si `system` (el programa es,
//!   byte a byte, uno de los del sistema);
//! - no hay llamada para instalar programas del sistema;
//! - la fecha, la sal y el acumulador de aleatoriedad los pone siempre el sistema.
use crate::accounts::{salt_from, Account, AccountError};
use crate::drives::{DriveInfo, FormatKind, FormatRequest};
use crate::files::{in_system, resolve, Files, TrashItem, PROGRAMS_DIR};
use crate::vol::FsKind;
use crate::fs::{Entry, FsError};
use crate::fsapi::FilesApi;
use crate::keys::{KeyError, Pool};
use crate::net::{ConnInfo, Datagram, Ipv4, ListenId, NetApi, NetError, NetStatus, PingId, PingReply, Proto, QueryId, SockId, State, TcpState, UdpId, MAX_LISTEN, MAX_PINGS, MAX_QUERIES, MAX_SOCKS, MAX_UDP, TCP_BUF, UDP_MAX};
use crate::netperm::{self, Kind};
use crate::platform::{DateTime, MemInfo, Platform};
use crate::wire::{decode, encode, Bad, Reader, Wire, WireResult, Writer};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Bytes tal cual (mismo formato que `Vec<u8>`, sin pasar byte a byte).
pub struct Blob(pub Vec<u8>);

impl Wire for Blob {
    fn put(&self, w: &mut Writer) {
        w.bytes(&self.0);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(Blob(r.bytes()?.to_vec()))
    }
}

macro_rules! calls {
    ($($v:ident $({ $($f:ident : $t:ty),* })? = $tag:literal,)*) => {
        /// Una petición al sistema: una por método de `FilesApi` (sin las fechas, sales ni
        /// acumuladores, que pone el sistema) y las del equipo.
        pub enum Call { $($v $({ $($f: $t),* })?),* }

        impl Wire for Call {
            fn put(&self, w: &mut Writer) {
                match self {
                    $(Call::$v $({ $($f),* })? => {
                        w.put(&($tag as u8));
                        $($(w.put($f);)*)?
                    })*
                }
            }
            fn get(r: &mut Reader) -> WireResult<Self> {
                Ok(match u8::get(r)? {
                    $($tag => Call::$v $({ $($f: r.get()?),* })?,)*
                    _ => return Err(Bad),
                })
            }
        }
    };
}

calls! {
    Persistent = 0,
    Generation = 1,
    List { dir: String } = 2,
    Stat { path: String } = 3,
    Exists { path: String } = 4,
    Read { path: String } = 5,
    Write { path: String, data: Blob } = 6,
    Mkdir { path: String } = 7,
    Rename { from: String, to: String } = 8,
    Copy { from: String, to: String } = 9,
    Delete { path: String } = 10,
    FreeName { dir: String, name: String } = 11,
    TrashList = 12,
    Restore { id: String } = 13,
    EmptyTrash = 14,
    Space = 15,
    SetClipMany { paths: Vec<String>, cut: bool } = 16,
    PasteAll { dir: String } = 17,
    Transfer { paths: Vec<String>, dir: String, mv: bool } = 18,
    UserRename { from: String, to: String } = 19,
    UserTrashMany { paths: Vec<String> } = 20,
    UserCreate { path: String, dir: bool } = 21,
    Undo = 22,
    Log { text: String } = 23,
    Home = 24,
    ExplorerPrefs = 25,
    Clip = 26,
    ClipPaths = 27,
    User = 28,
    UndoLabel = 29,
    Accounts = 30,
    IsAdmin = 31,
    VerifyPassword { password: String } = 32,
    HasPassword = 33,
    CreateAccount { name: String, password: Option<String>, admin: bool } = 34,
    DeleteAccount { name: String, keep_files: bool } = 35,
    RenameAccount { old: String, new: String } = 36,
    SetAdmin { name: String, admin: bool } = 37,
    SetPassword { old: String, new: Option<String> } = 38,
    DiskEncrypted = 39,
    DiskConversion = 40,
    ConvertStep { blocks: u32 } = 41,
    HomeEncrypted = 42,
    HomeConversion = 43,
    HomeConvertStep { files: u32 } = 44,
    EncryptDisk { password: String } = 45,
    DecryptDisk { password: String } = 46,
    ChangeDiskPassword { old: String, new: String } = 47,
    NewDiskRecovery { password: String } = 48,
    EncryptHome { password: String } = 49,
    DecryptHome { password: String } = 50,
    NewHomeRecovery { password: String } = 51,
    Mem = 52,
    Cpu = 53,
    Screen = 54,
    Entropy = 55,
    Reboot = 56,
    Shutdown = 57,
    Drives = 58,
    UnlockDrive { letter: char, secret: String, recovery: bool } = 59,
    DriveAction { letter: char, req: FormatRequest } = 60,
    DriveResult = 61,
    ListView { dir: String } = 62,
    NetStatus = 63,
    NetRenew = 64,
    NetPing { dst: Ipv4, bytes: u16, ttl: u8 } = 65,
    NetPingPoll { id: PingId } = 66,
    NetResolve { name: String } = 67,
    NetResolvePoll { id: QueryId } = 68,
    NetConnect { dst: Ipv4, port: u16 } = 69,
    NetState { id: SockId } = 70,
    NetSend { id: SockId, data: Blob } = 71,
    NetRecv { id: SockId, max: u32 } = 72,
    NetClose { id: SockId } = 73,
    NetAbort { id: SockId } = 74,
    UdpBind { port: u16 } = 75,
    UdpSend { id: UdpId, dst: Ipv4, port: u16, data: Blob } = 76,
    UdpRecv { id: UdpId } = 77,
    UdpClose { id: UdpId } = 78,
    TcpListen { port: u16 } = 79,
    TcpAccept { id: ListenId } = 80,
    TcpUnlisten { id: ListenId } = 81,
    NetConnections = 82,
    NetPerms = 83,
    SetNetPerm { hash: [u8; 32], out: Option<bool>, inn: Option<bool> } = 84,
    CustomRoots = 85,
    AddRoot { der: Blob } = 86,
    RemoveRoot { sha: [u8; 32] } = 87,
}

/// Lo más que viaja de una vez por `NetSend` y `NetRecv` (el búfer de una conexión). Un programa
/// que quiera más manda o lee en varias vueltas, como con cualquier `tcp_send`/`tcp_recv`.
pub const NET_CHUNK: usize = TCP_BUF;
/// Longitud máxima de un nombre DNS (RFC 1035).
const NET_NAME_MAX: usize = 253;

// ---------- Wire de los tipos que viajan ----------

impl<T: Wire, E: Wire> Wire for Result<T, E> {
    fn put(&self, w: &mut Writer) {
        match self {
            Ok(v) => {
                w.put(&0u8);
                w.put(v);
            }
            Err(e) => {
                w.put(&1u8);
                w.put(e);
            }
        }
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        match u8::get(r)? {
            0 => Ok(Ok(r.get()?)),
            1 => Ok(Err(r.get()?)),
            _ => Err(Bad),
        }
    }
}

const FS_ERRORS: [FsError; 12] = [
    FsError::NotFound,
    FsError::Exists,
    FsError::NotDir,
    FsError::IsDir,
    FsError::NotEmpty,
    FsError::NoSpace,
    FsError::BadName,
    FsError::BadPath,
    FsError::Disk,
    FsError::Corrupt,
    FsError::Invalid,
    FsError::Denied,
];

impl Wire for FsError {
    fn put(&self, w: &mut Writer) {
        w.put(&(FS_ERRORS.iter().position(|e| e == self).unwrap_or(8) as u8));
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        FS_ERRORS.get(u8::get(r)? as usize).copied().ok_or(Bad)
    }
}

const ACCOUNT_ERRORS: [AccountError; 9] = [
    AccountError::BadName,
    AccountError::Exists,
    AccountError::NotFound,
    AccountError::WrongPassword,
    AccountError::WeakPassword,
    AccountError::NotAdmin,
    AccountError::LastAdmin,
    AccountError::InUse,
    AccountError::Encrypted,
];

impl Wire for AccountError {
    fn put(&self, w: &mut Writer) {
        match self {
            AccountError::Fs(e) => {
                w.put(&255u8);
                w.put(e);
            }
            e => {
                w.put(&(ACCOUNT_ERRORS.iter().position(|x| x == e).unwrap_or(0) as u8));
            }
        }
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        match u8::get(r)? {
            255 => Ok(AccountError::Fs(r.get()?)),
            n => ACCOUNT_ERRORS.get(n as usize).copied().ok_or(Bad),
        }
    }
}

const KEY_ERRORS: [KeyError; 9] = [
    KeyError::WrongSecret,
    KeyError::Weak,
    KeyError::NotEncrypted,
    KeyError::AlreadyEncrypted,
    KeyError::NotAdmin,
    KeyError::NoRoom,
    KeyError::Busy,
    KeyError::Unavailable,
    KeyError::NotReady,
];

impl Wire for KeyError {
    fn put(&self, w: &mut Writer) {
        match self {
            KeyError::Fs(e) => {
                w.put(&255u8);
                w.put(e);
            }
            e => {
                w.put(&(KEY_ERRORS.iter().position(|x| x == e).unwrap_or(4) as u8));
            }
        }
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        match u8::get(r)? {
            255 => Ok(KeyError::Fs(r.get()?)),
            n => KEY_ERRORS.get(n as usize).copied().ok_or(Bad),
        }
    }
}

impl Wire for char {
    fn put(&self, w: &mut Writer) {
        w.put(&(*self as u32));
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        char::from_u32(u32::get(r)?).ok_or(Bad)
    }
}

// Solo se añade al final: los números ya publicados no cambian.
const FS_KINDS: [FsKind; 11] = [FsKind::Fat12, FsKind::Fat16, FsKind::Fat32, FsKind::ExFat, FsKind::Iso9660, FsKind::Udf, FsKind::NightFury, FsKind::Unknown, FsKind::Ntfs, FsKind::Ext, FsKind::BitLocker];

impl Wire for FsKind {
    fn put(&self, w: &mut Writer) {
        w.put(&(FS_KINDS.iter().position(|k| k == self).unwrap_or(7) as u8));
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        FS_KINDS.get(u8::get(r)? as usize).copied().ok_or(Bad)
    }
}

const FORMAT_KINDS: [FormatKind; 3] = [FormatKind::Fat32, FormatKind::ExFat, FormatKind::NightFury];

impl Wire for FormatKind {
    fn put(&self, w: &mut Writer) {
        w.put(&(FORMAT_KINDS.iter().position(|k| k == self).unwrap_or(0) as u8));
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        FORMAT_KINDS.get(u8::get(r)? as usize).copied().ok_or(Bad)
    }
}

impl Wire for FormatRequest {
    fn put(&self, w: &mut Writer) {
        match self {
            FormatRequest::Eject => {
                w.put(&0u8);
            }
            FormatRequest::Format { kind, label } => {
                w.put(&1u8).put(kind).put(label);
            }
            FormatRequest::Encrypt { label, password } => {
                w.put(&2u8).put(label).put(password);
            }
        }
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(match u8::get(r)? {
            0 => FormatRequest::Eject,
            1 => FormatRequest::Format { kind: r.get()?, label: r.get()? },
            2 => FormatRequest::Encrypt { label: r.get()?, password: r.get()? },
            _ => return Err(Bad),
        })
    }
}

impl Wire for DriveInfo {
    fn put(&self, w: &mut Writer) {
        w.put(&self.letter).put(&self.label).put(&self.device).put(&self.kind).put(&self.total).put(&self.free);
        w.put(&self.read_only).put(&self.removable).put(&self.locked);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(DriveInfo { letter: r.get()?, label: r.get()?, device: r.get()?, kind: r.get()?, total: r.get()?, free: r.get()?, read_only: r.get()?, removable: r.get()?, locked: r.get()? })
    }
}

impl Wire for Entry {
    fn put(&self, w: &mut Writer) {
        w.put(&self.name).put(&self.dir).put(&self.size).put(&self.modified);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(Entry { name: r.get()?, dir: r.get()?, size: r.get()?, modified: r.get()?, hidden: false })
    }
}

impl Wire for TrashItem {
    fn put(&self, w: &mut Writer) {
        w.put(&self.id).put(&self.name).put(&self.original).put(&self.dir).put(&self.size).put(&self.deleted);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(TrashItem { id: r.get()?, name: r.get()?, original: r.get()?, dir: r.get()?, size: r.get()?, deleted: r.get()? })
    }
}

/// Solo viajan el nombre y el tipo: la sal y la marca de la clave no salen del sistema, así que
/// `has_password` de una cuenta recibida no vale (para eso está `FilesApi::has_password`).
impl Wire for Account {
    fn put(&self, w: &mut Writer) {
        w.put(&self.name).put(&self.admin);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        let name: String = r.get()?;
        Ok(Account::new(&name, r.get()?))
    }
}

impl Wire for MemInfo {
    fn put(&self, w: &mut Writer) {
        w.put(&self.total).put(&self.heap_total).put(&self.heap_used);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(MemInfo { total: r.get()?, heap_total: r.get()?, heap_used: r.get()? })
    }
}

// ---------- Wire de la red ----------

impl Wire for [u8; 4] {
    fn put(&self, w: &mut Writer) {
        w.buf.extend_from_slice(self);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        let mut a = [0u8; 4];
        a.copy_from_slice(r.take(4)?);
        Ok(a)
    }
}

impl Wire for [u8; 6] {
    fn put(&self, w: &mut Writer) {
        w.buf.extend_from_slice(self);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        let mut a = [0u8; 6];
        a.copy_from_slice(r.take(6)?);
        Ok(a)
    }
}

impl Wire for [u8; 32] {
    fn put(&self, w: &mut Writer) {
        w.buf.extend_from_slice(self);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        let mut a = [0u8; 32];
        a.copy_from_slice(r.take(32)?);
        Ok(a)
    }
}

impl Wire for netperm::Perm {
    fn put(&self, w: &mut Writer) {
        w.put(&self.hash).put(&self.name).put(&self.out).put(&self.inn);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(netperm::Perm { hash: r.get()?, name: r.get()?, out: r.get()?, inn: r.get()? })
    }
}

macro_rules! net_id {
    ($($t:ident),*) => {$(
        impl Wire for $t {
            fn put(&self, w: &mut Writer) {
                w.put(&self.0);
            }
            fn get(r: &mut Reader) -> WireResult<Self> {
                Ok($t(r.get()?))
            }
        }
    )*};
}
net_id!(PingId, QueryId, SockId, UdpId, ListenId);

impl Wire for Datagram {
    fn put(&self, w: &mut Writer) {
        w.put(&self.from).put(&self.port).bytes(&self.data);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(Datagram { from: r.get()?, port: r.get()?, data: r.bytes()?.to_vec() })
    }
}

/// Textos de `ConnInfo::state`, por posición (como `PROTOCOL`).
const CONN_STATES: [&str; 6] = ["Conectando", "Abierta", "Cerrando", "Cerrada", "Escuchando", "—"];

impl Wire for ConnInfo {
    fn put(&self, w: &mut Writer) {
        let p: u8 = match self.proto {
            Proto::Tcp => 0,
            Proto::TcpListen => 1,
            Proto::Udp => 2,
        };
        w.put(&p).put(&self.id).put(&self.local_port).put(&self.remote);
        w.put(&(CONN_STATES.iter().position(|s| *s == self.state).unwrap_or(5) as u8));
        w.put(&self.rx).put(&self.tx).put(&self.owner);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        let proto = match u8::get(r)? {
            0 => Proto::Tcp,
            1 => Proto::TcpListen,
            2 => Proto::Udp,
            _ => return Err(Bad),
        };
        Ok(ConnInfo {
            proto,
            id: r.get()?,
            local_port: r.get()?,
            remote: r.get()?,
            state: CONN_STATES.get(u8::get(r)? as usize).copied().unwrap_or("—"),
            rx: r.get()?,
            tx: r.get()?,
            owner: r.get()?,
        })
    }
}

const STATES: [State; 5] = [State::NoAdapter, State::NoCable, State::Configuring, State::NoInternet, State::Connected];

impl Wire for State {
    fn put(&self, w: &mut Writer) {
        w.put(&(STATES.iter().position(|s| s == self).unwrap_or(0) as u8));
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        STATES.get(u8::get(r)? as usize).copied().ok_or(Bad)
    }
}

impl Wire for NetStatus {
    fn put(&self, w: &mut Writer) {
        w.put(&self.state).put(&self.adapter).put(&self.mac).put(&self.speed_mbps).put(&self.full_duplex);
        w.put(&self.ip).put(&self.mask).put(&self.gateway).put(&self.dns).put(&self.dhcp_server);
        w.put(&self.lease_s).put(&self.lease_since_ms);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(NetStatus {
            state: r.get()?,
            adapter: r.get()?,
            mac: r.get()?,
            speed_mbps: r.get()?,
            full_duplex: r.get()?,
            ip: r.get()?,
            mask: r.get()?,
            gateway: r.get()?,
            dns: r.get()?,
            dhcp_server: r.get()?,
            lease_s: r.get()?,
            lease_since_ms: r.get()?,
        })
    }
}

/// Motivos de `NetError::Protocol` que viajan por su posición (el texto es `&'static str`, así que
/// al recibir se vuelve a uno de estos y nunca se crea un texto nuevo). Solo se añade al final.
const PROTOCOL: [&str; 11] = [
    "Content-Length no válido",
    "cabeceras demasiado grandes",
    "conexión cerrada antes de las cabeceras",
    "conexión cerrada",
    "demasiadas redirecciones",
    "la conexión no está abierta",
    "respuesta incompleta",
    "sin servidor DNS",
    "tamaño de bloque no válido",
    "tamaño de eco excesivo",
    "datagrama demasiado grande",
];
const PROTOCOL_UNKNOWN: &str = "respuesta no válida";

impl Wire for NetError {
    fn put(&self, w: &mut Writer) {
        use NetError::*;
        match self {
            NoAdapter => w.put(&0u8),
            NoCable => w.put(&1u8),
            NoAddress => w.put(&2u8),
            NotFound => w.put(&3u8),
            Timeout => w.put(&4u8),
            Refused => w.put(&5u8),
            Reset => w.put(&6u8),
            Unreachable(ip) => w.put(&7u8).put(ip),
            TtlExpired(ip) => w.put(&8u8).put(ip),
            Busy => w.put(&9u8),
            Gone => w.put(&10u8),
            BadUrl => w.put(&11u8),
            HttpsNotYet => w.put(&12u8),
            Protocol(t) => w.put(&13u8).put(&(PROTOCOL.iter().position(|p| p == t).map_or(255, |i| i as u8))),
            Denied => w.put(&14u8),
            Pending => w.put(&15u8),
            InUse => w.put(&16u8),
        };
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        use NetError::*;
        Ok(match u8::get(r)? {
            0 => NoAdapter,
            1 => NoCable,
            2 => NoAddress,
            3 => NotFound,
            4 => Timeout,
            5 => Refused,
            6 => Reset,
            7 => Unreachable(r.get()?),
            8 => TtlExpired(r.get()?),
            9 => Busy,
            10 => Gone,
            11 => BadUrl,
            12 => HttpsNotYet,
            13 => Protocol(PROTOCOL.get(u8::get(r)? as usize).copied().unwrap_or(PROTOCOL_UNKNOWN)),
            14 => Denied,
            15 => Pending,
            16 => InUse,
            _ => return Err(Bad),
        })
    }
}

impl Wire for PingReply {
    fn put(&self, w: &mut Writer) {
        w.put(&self.from).put(&self.bytes).put(&self.rtt_ms).put(&self.ttl);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(PingReply { from: r.get()?, bytes: r.get()?, rtt_ms: r.get()?, ttl: r.get()? })
    }
}

impl Wire for TcpState {
    fn put(&self, w: &mut Writer) {
        match self {
            TcpState::Connecting => w.put(&0u8),
            TcpState::Open => w.put(&1u8),
            TcpState::PeerClosed => w.put(&2u8),
            TcpState::Closed => w.put(&3u8),
            TcpState::Failed(e) => w.put(&4u8).put(e),
        };
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(match u8::get(r)? {
            0 => TcpState::Connecting,
            1 => TcpState::Open,
            2 => TcpState::PeerClosed,
            3 => TcpState::Closed,
            4 => TcpState::Failed(r.get()?),
            _ => return Err(Bad),
        })
    }
}

/// Lo que un programa ha abierto en la pila: solo usa lo suyo, y al irse se libera.
#[derive(Default)]
pub struct NetOwn {
    pings: Vec<PingId>,
    queries: Vec<QueryId>,
    socks: Vec<SockId>,
    udps: Vec<UdpId>,
    listens: Vec<ListenId>,
    /// v0.12.1, programa de terceros: SHA-256 de su `.nfx` y nombre. Sin esto no tiene red.
    id: Option<([u8; 32], String)>,
    /// Sus permisos tal como estaban en la generación de archivos dada (se releen si cambia).
    cache: Option<(u64, Option<bool>, Option<bool>)>,
    /// Aviso pendiente de respuesta, y si ya está en pantalla.
    ask: Option<Kind>,
    shown: bool,
    /// Proceso dueño (`ProcInfo::id`) para el Administrador de tareas; 0 sin proceso.
    pid: u64,
}

impl NetOwn {
    /// Proceso al que se apunta lo que abra (`ProcApp`).
    pub fn set_pid(&mut self, pid: u64) {
        self.pid = pid;
    }

    /// Programa de terceros con esta identidad (`ProcApp`, al cargarlo).
    pub fn set_program(&mut self, hash: [u8; 32], name: &str) {
        self.id = Some((hash, String::from(name)));
    }

    /// Aviso que falta enseñar (y lo da por enseñado). `ProcApp` lo llama cuando el escritorio
    /// tiene el hueco de diálogo libre.
    pub fn prompt(&mut self) -> Option<(Kind, String)> {
        let (k, (_, name)) = (self.ask?, self.id.as_ref()?);
        if self.shown {
            return None;
        }
        self.shown = true;
        Some((k, name.clone()))
    }

    /// El usuario respondió al aviso: se guarda para todas las cuentas y se apunta en el registro.
    pub fn answered(&mut self, sys: &mut dyn Platform, allow: bool) {
        let (Some(k), Some((hash, name))) = (self.ask.take(), self.id.clone()) else { return };
        self.shown = false;
        let now = sys.now();
        let files = sys.files();
        let mut perms = files.net_perms();
        netperm::record(&mut perms, hash, &name, k, allow);
        // Aunque no se pueda guardar (disco en memoria o lleno), vale para esta vez.
        let _ = files.set_net_perms(&perms, now);
        let what = match k {
            Kind::Out => "usar la red",
            Kind::In => "aceptar conexiones",
        };
        files.log(&alloc::format!("Red: «{}» {} {what}", netperm::clean_name(&name), if allow { "puede" } else { "no puede" }), now);
        let p = perms.iter().find(|p| p.hash == hash);
        self.cache = Some((files.generation(), p.and_then(|p| p.out), p.and_then(|p| p.inn)));
    }

    /// ¿Puede el programa hacer algo de tipo `k`? Sin respuesta todavía, pide el aviso (`Pending`).
    fn check(&mut self, sys: &mut dyn Platform, k: Kind) -> Result<(), NetError> {
        let Some((hash, _)) = self.id.as_ref() else { return Err(NetError::NoAdapter) };
        let generation = sys.files().generation();
        if self.cache.is_none_or(|c| c.0 != generation) {
            let p = sys.files().net_perms().into_iter().find(|p| p.hash == *hash);
            self.cache = Some((generation, p.as_ref().and_then(|p| p.out), p.as_ref().and_then(|p| p.inn)));
        }
        let (_, out, inn) = self.cache.unwrap_or_default();
        match if k == Kind::Out { out } else { inn } {
            Some(true) => Ok(()),
            Some(false) => Err(NetError::Denied),
            None => {
                if self.ask.is_none() {
                    self.ask = Some(k);
                }
                Err(NetError::Pending)
            }
        }
    }

    /// El programa se va: corta sus conexiones y recoge sus pings y consultas (los que siguen en
    /// marcha vencen solos por plazo y la pila los borra poco después).
    pub fn release(&mut self, sys: &mut dyn Platform) {
        if let Some(net) = sys.net() {
            for s in self.socks.drain(..) {
                net.tcp_abort(s);
            }
            for p in self.pings.drain(..) {
                net.ping_poll(p);
            }
            for q in self.queries.drain(..) {
                net.resolve_poll(q);
            }
            for u in self.udps.drain(..) {
                net.udp_close(u);
            }
            for l in self.listens.drain(..) {
                net.tcp_unlisten(l);
            }
        }
        self.pings.clear();
        self.queries.clear();
        self.socks.clear();
        self.udps.clear();
        self.listens.clear();
    }
}

/// Qué permiso necesita una llamada de red de un programa de terceros (`None`: ninguno, porque solo
/// lee el estado o trabaja con identificadores que ya abrió con permiso).
fn needs(call: &Call) -> Option<Kind> {
    match call {
        Call::NetPing { .. } | Call::NetResolve { .. } | Call::NetConnect { .. } | Call::UdpSend { .. } | Call::UdpBind { port: 0 } => Some(Kind::Out),
        Call::UdpBind { .. } | Call::TcpListen { .. } | Call::TcpAccept { .. } => Some(Kind::In),
        _ => None,
    }
}

/// Lo que ve quien no tiene red (equipo sin pila o programa sin identidad): no hay tarjeta.
fn no_net(call: &Call) -> Vec<u8> {
    net_err(call, NetError::NoAdapter)
}

/// La respuesta de `call` cuando falla con `e`.
fn net_err(call: &Call, e: NetError) -> Vec<u8> {
    match call {
        Call::NetStatus => encode(&no_status()),
        Call::NetPing { .. } | Call::NetResolve { .. } | Call::NetConnect { .. } | Call::NetSend { .. } | Call::NetRecv { .. } => encode(&Err::<u8, _>(e)),
        Call::UdpBind { .. } | Call::UdpSend { .. } | Call::UdpRecv { .. } | Call::TcpListen { .. } | Call::TcpAccept { .. } => encode(&Err::<u8, _>(e)),
        Call::NetConnections => encode(&Vec::<ConnInfo>::new()),
        Call::NetPingPoll { .. } | Call::NetResolvePoll { .. } => encode(&Some(Err::<u8, _>(e))),
        Call::NetState { .. } => encode(&TcpState::Failed(e)),
        _ => encode(&()),
    }
}

fn no_status() -> NetStatus {
    NetStatus { state: State::NoAdapter, adapter: String::new(), mac: [0; 6], speed_mbps: 0, full_duplex: false, ip: None, mask: None, gateway: None, dns: Vec::new(), dhcp_server: None, lease_s: 0, lease_since_ms: 0 }
}

// ---------- Lado del sistema ----------

/// Atiende una petición. Respuesta: `0` + el valor, o `1` si no se entiende (nunca un pánico).
pub fn serve(sys: &mut dyn Platform, req: &[u8], system: bool, own: &mut NetOwn) -> Vec<u8> {
    let Ok(call) = decode::<Call>(req) else { return vec![1] };
    let mut out = vec![0];
    out.extend(run(sys, call, system, own));
    out
}

/// Ejecuta `f` si `ok`; si no, devuelve `deny` (el error de permiso o el valor neutro).
fn gate<T: Wire>(ok: bool, deny: T, f: impl FnOnce() -> T) -> Vec<u8> {
    encode(&if ok { f() } else { deny })
}

/// Ninguna de las rutas está en `C:\Sistema` (ni, para un programa de terceros, en `C:\Programas`).
fn outside(system: bool, paths: &[&str]) -> bool {
    !paths.iter().any(|p| blocked(system, p))
}

/// Ruta que el programa no puede cambiar: `C:\Sistema` para todos; `C:\Programas` también para
/// los de terceros (instalar o sustituir programas es cosa de Configuración).
pub(crate) fn blocked(system: bool, path: &str) -> bool {
    in_system(path) || (!system && in_programs(path))
}

/// Origen que un programa de terceros no puede copiar ni mover: lo de `blocked` y la raíz de C:,
/// que contiene `C:\Sistema` (copiarla a D: la leería entera).
fn src_blocked(system: bool, path: &str) -> bool {
    blocked(system, path) || (!system && resolve("C:\\", path).is_ok_and(|c| c.len() == 3 && c.starts_with('C')))
}

fn in_programs(path: &str) -> bool {
    // Solo en C:: `D:\Programas` es una carpeta cualquiera de una unidad extraíble.
    resolve("C:\\", path).is_ok_and(|c| c.starts_with('C') && c[3..].split('\\').next().is_some_and(|t| t.eq_ignore_ascii_case(&PROGRAMS_DIR[3..])))
}

fn with_pool<R>(sys: &mut dyn Platform, f: impl FnOnce(&mut Files, &mut Pool) -> R) -> R {
    let mut pool = core::mem::take(sys.pool());
    let r = f(sys.files(), &mut pool);
    *sys.pool() = pool;
    r
}

fn salt(sys: &mut dyn Platform) -> [u8; 16] {
    let (a, b) = (sys.entropy(), sys.entropy());
    salt_from(a, b)
}

fn run(sys: &mut dyn Platform, call: Call, system: bool, own: &mut NetOwn) -> Vec<u8> {
    use Call::*;
    let denied = FsError::Denied;
    let no_admin = AccountError::NotAdmin;
    let no_key = KeyError::NotAdmin;
    match call {
        Mem => encode(&sys.mem()),
        Cpu => encode(&sys.cpu()),
        Screen => encode(&sys.screen()),
        Entropy => encode(&sys.entropy()),
        Reboot => gate(system, (), || sys.reboot()),
        Shutdown => gate(system, (), || sys.shutdown()),
        Persistent => encode(&sys.files().persistent()),
        Generation => encode(&sys.files().generation()),
        List { dir } => encode(&sys.files().list(&dir)),
        ListView { dir } => encode(&sys.files().list_view(&dir)),
        Stat { path } => encode(&sys.files().stat(&path)),
        Exists { path } => encode(&sys.files().exists(&path)),
        // Un programa de terceros no lee `C:\Sistema` (con sesión de administrador vería las cuentas).
        Read { path } => gate(system || !in_system(&path), Err(denied), || sys.files().read(&path).map(Blob)),
        Write { path, data } => {
            let now = sys.now();
            gate(outside(system, &[&path]), Err(denied), || sys.files().write(&path, &data.0, now))
        }
        Mkdir { path } => {
            let now = sys.now();
            gate(outside(system, &[&path]), Err(denied), || sys.files().mkdir(&path, now))
        }
        Rename { from, to } => gate(outside(system, &[&from, &to]), Err(denied), || sys.files().rename(&from, &to)),
        Copy { from, to } => {
            let now = sys.now();
            gate(outside(system, &[&to]) && !src_blocked(system, &from), Err(denied), || sys.files().copy(&from, &to, now))
        }
        Delete { path } => gate(outside(system, &[&path]), Err(denied), || sys.files().delete(&path)),
        FreeName { dir, name } => encode(&sys.files().free_name(&dir, &name)),
        TrashList => encode(&sys.files().trash_list()),
        Restore { id } => {
            // Restaura donde estaba: si estaba en el sistema, no.
            let files = sys.files();
            let into_system = files.trash_list().is_ok_and(|l| l.iter().any(|t| t.id == id && blocked(system, &t.original)));
            gate(!into_system, Err(denied), || files.restore(&id))
        }
        EmptyTrash => encode(&sys.files().empty_trash()),
        Space => encode(&sys.files().space()),
        SetClipMany { paths, cut } => encode(&sys.files().set_clip_many(&paths, cut)),
        PasteAll { dir } => {
            let now = sys.now();
            let files = sys.files();
            let from_system = files.clip_paths().0.iter().any(|p| src_blocked(system, p));
            gate(outside(system, &[&dir]) && !from_system, Err(denied), || files.paste_all(&dir, now))
        }
        Transfer { paths, dir, mv } => {
            let now = sys.now();
            let ok = outside(system, &[&dir]) && !paths.iter().any(|p| src_blocked(system, p));
            gate(ok, Err(denied), || sys.files().transfer(&paths, &dir, mv, now))
        }
        UserRename { from, to } => gate(outside(system, &[&from, &to]), Err(denied), || sys.files().user_rename(&from, &to)),
        UserTrashMany { paths } => {
            let now = sys.now();
            gate(!paths.iter().any(|p| blocked(system, p)), Err(denied), || sys.files().user_trash_many(&paths, now))
        }
        UserCreate { path, dir } => {
            let now = sys.now();
            gate(outside(system, &[&path]), Err(denied), || sys.files().user_create(&path, dir, now))
        }
        Undo => {
            let now = sys.now();
            encode(&sys.files().undo(now))
        }
        Log { text } => {
            let now = sys.now();
            gate(system, (), || sys.files().log(&text, now))
        }
        Home => encode(&sys.files().home()),
        ExplorerPrefs => encode(&sys.files().explorer_prefs()),
        Clip => encode(&FilesApi::clip(sys.files())),
        ClipPaths => encode(&FilesApi::clip_paths(sys.files())),
        User => encode(&FilesApi::user(sys.files())),
        UndoLabel => encode(&sys.files().undo_label().map(String::from)),
        IsAdmin => encode(&sys.files().is_admin()),
        // ---- Cuentas y claves: solo programas del sistema ----
        Accounts => gate(system, Vec::new(), || sys.files().accounts()),
        VerifyPassword { password } => gate(system, false, || sys.files().verify_password(&password)),
        HasPassword => gate(system, false, || sys.files().has_password()),
        CreateAccount { name, password, admin } => {
            let (now, salt) = (sys.now(), salt(sys));
            gate(system, Err(no_admin), || sys.files().create_account(&name, password.as_deref(), admin, salt, now))
        }
        DeleteAccount { name, keep_files } => {
            let now = sys.now();
            gate(system, Err(no_admin), || sys.files().delete_account(&name, keep_files, now))
        }
        RenameAccount { old, new } => {
            let now = sys.now();
            gate(system, Err(no_admin), || sys.files().rename_account(&old, &new, now))
        }
        SetAdmin { name, admin } => {
            let now = sys.now();
            gate(system, Err(no_admin), || sys.files().set_admin(&name, admin, now))
        }
        SetPassword { old, new } => {
            let (now, salt) = (sys.now(), salt(sys));
            gate(system, Err(no_admin), || sys.files().set_password(&old, new.as_deref(), salt, now))
        }
        // ---- Cifrado: solo programas del sistema ----
        DiskEncrypted => gate(system, false, || sys.files().disk_encrypted()),
        DiskConversion => gate(system, None, || sys.files().disk_conversion()),
        ConvertStep { blocks } => {
            let now = sys.now();
            gate(system, Err(no_key), || sys.files().convert_step(blocks, now))
        }
        HomeEncrypted => gate(system, false, || sys.files().home_encrypted()),
        HomeConversion => gate(system, None, || sys.files().home_conversion()),
        HomeConvertStep { files } => {
            let now = sys.now();
            gate(system, Err(no_key), || sys.files().home_convert_step(files, now))
        }
        EncryptDisk { password } => {
            let now = sys.now();
            gate(system, Err(no_key), || with_pool(sys, |f, pool| f.encrypt_disk(&password, pool, now)))
        }
        DecryptDisk { password } => {
            let now = sys.now();
            gate(system, Err(no_key), || sys.files().decrypt_disk(&password, now))
        }
        ChangeDiskPassword { old, new } => {
            let now = sys.now();
            gate(system, Err(no_key), || with_pool(sys, |f, pool| f.change_disk_password(&old, &new, pool, now)))
        }
        NewDiskRecovery { password } => {
            let now = sys.now();
            gate(system, Err(no_key), || with_pool(sys, |f, pool| f.new_disk_recovery(&password, pool, now)))
        }
        EncryptHome { password } => {
            let now = sys.now();
            gate(system, Err(no_key), || with_pool(sys, |f, pool| f.encrypt_home(&password, pool, now)))
        }
        DecryptHome { password } => {
            let now = sys.now();
            gate(system, Err(no_key), || sys.files().decrypt_home(&password, now))
        }
        NewHomeRecovery { password } => {
            let now = sys.now();
            gate(system, Err(no_key), || with_pool(sys, |f, pool| f.new_home_recovery(&password, pool, now)))
        }
        // ---- Unidades extraíbles: ver, cualquiera; abrir, expulsar, formatear y cifrar, el sistema ----
        Drives => encode(&sys.files().drives()),
        UnlockDrive { letter, secret, recovery } => gate(system, Err(no_key), || sys.files().unlock_drive(letter, &secret, recovery)),
        DriveAction { letter, req } => gate(system, Err(denied), || sys.files().request_drive_action(letter, req)),
        DriveResult => gate(system, None, || sys.files().drive_result()),
        NetPerms => gate(system, Vec::new(), || sys.files().net_perms()),
        SetNetPerm { hash, out, inn } => {
            let now = sys.now();
            gate(system, Err(denied), || sys.files().set_net_perm(hash, out, inn, now))
        }
        CustomRoots => gate(system, Vec::new(), || sys.files().custom_roots()),
        AddRoot { der } => {
            let now = sys.now();
            gate(system, Err(denied), || sys.files().add_root(der.0, now))
        }
        RemoveRoot { sha } => {
            let now = sys.now();
            gate(system, Err(denied), || sys.files().remove_root(sha, now))
        }
        NetStatus | NetRenew | NetPing { .. } | NetPingPoll { .. } | NetResolve { .. } | NetResolvePoll { .. } | NetConnect { .. } | NetState { .. } | NetSend { .. } | NetRecv { .. } | NetClose { .. } | NetAbort { .. }
        | UdpBind { .. } | UdpSend { .. } | UdpRecv { .. } | UdpClose { .. } | TcpListen { .. } | TcpAccept { .. } | TcpUnlisten { .. } | NetConnections => {
            if !system {
                if own.id.is_none() {
                    return no_net(&call);
                }
                // Los puertos de menos de 1024 son del sistema, como en otros sistemas.
                if let UdpBind { port } | TcpListen { port } = call {
                    if (1..1024).contains(&port) {
                        return net_err(&call, NetError::InUse);
                    }
                }
                if let Some(k) = needs(&call) {
                    if let Err(e) = own.check(sys, k) {
                        return net_err(&call, e);
                    }
                }
            }
            match sys.net() {
                // `renew` cambia la red de todo el equipo: solo el sistema. La MAC identifica el equipo:
                // un programa de terceros no la ve.
                Some(_) if !system && matches!(call, NetRenew) => encode(&()),
                Some(net) if !system && matches!(call, NetStatus) => encode(&crate::net::NetStatus { mac: [0; 6], ..net.status() }),
                Some(net) => net_run(net, call, own, system),
                None => no_net(&call),
            }
        }
    }
}

/// Las llamadas de red, con la pila ya a mano. Cada programa solo toca los identificadores que
/// abrió él; uno ajeno es uno que «ya no existe».
fn net_run(net: &mut dyn NetApi, call: Call, own: &mut NetOwn, system: bool) -> Vec<u8> {
    use Call::*;
    match call {
        NetStatus => encode(&net.status()),
        NetRenew => encode(&net.renew()),
        NetPing { dst, bytes, ttl } => {
            // Con la lista llena se tiran los ya caducados (la pila los borra sola).
            if own.pings.len() >= MAX_PINGS {
                own.pings.retain(|&p| net.ping_poll(p).is_none());
            }
            let r = if own.pings.len() >= MAX_PINGS { Err(NetError::Busy) } else { net.ping(dst, bytes, ttl) };
            if let Ok(id) = r {
                own.pings.push(id);
            }
            encode(&r)
        }
        NetPingPoll { id } => {
            if !own.pings.contains(&id) {
                return encode(&Some(Err::<PingReply, _>(NetError::Gone)));
            }
            let r = net.ping_poll(id);
            if r.is_some() {
                own.pings.retain(|&p| p != id);
            }
            encode(&r)
        }
        NetResolve { name } => {
            if own.queries.len() >= MAX_QUERIES {
                own.queries.retain(|&q| net.resolve_poll(q).is_none());
            }
            let r = if name.len() > NET_NAME_MAX {
                Err(NetError::NotFound)
            } else if own.queries.len() >= MAX_QUERIES {
                Err(NetError::Busy)
            } else {
                net.resolve(&name)
            };
            if let Ok(id) = r {
                own.queries.push(id);
            }
            encode(&r)
        }
        NetResolvePoll { id } => {
            if !own.queries.contains(&id) {
                return encode(&Some(Err::<Vec<Ipv4>, _>(NetError::Gone)));
            }
            let r = net.resolve_poll(id);
            if r.is_some() {
                own.queries.retain(|&q| q != id);
            }
            encode(&r)
        }
        NetConnect { dst, port } => {
            if own.socks.len() >= MAX_SOCKS {
                own.socks.retain(|&s| net.tcp_state(s) != TcpState::Failed(NetError::Gone));
            }
            let r = if own.socks.len() >= MAX_SOCKS { Err(NetError::Busy) } else { net.tcp_connect(dst, port) };
            if let Ok(id) = r {
                own.socks.push(id);
                net.set_owner(Proto::Tcp, id.0, own.pid);
            }
            encode(&r)
        }
        NetState { id } => encode(&if own.socks.contains(&id) { net.tcp_state(id) } else { TcpState::Failed(NetError::Gone) }),
        NetSend { id, data } => {
            let n = data.0.len().min(NET_CHUNK);
            encode(&if own.socks.contains(&id) { net.tcp_send(id, &data.0[..n]).map(|n| n as u64) } else { Err(NetError::Gone) })
        }
        NetRecv { id, max } => {
            if !own.socks.contains(&id) {
                return encode(&Err::<Blob, _>(NetError::Gone));
            }
            let mut buf = vec![0u8; (max as usize).min(NET_CHUNK)];
            encode(&net.tcp_recv(id, &mut buf).map(|n| {
                buf.truncate(n);
                Blob(buf)
            }))
        }
        // Ambas liberan el identificador: el programa ya no lo tiene.
        NetClose { id } => {
            if let Some(i) = own.socks.iter().position(|&s| s == id) {
                own.socks.swap_remove(i);
                net.tcp_close(id);
            }
            encode(&())
        }
        NetAbort { id } => {
            if let Some(i) = own.socks.iter().position(|&s| s == id) {
                own.socks.swap_remove(i);
                net.tcp_abort(id);
            }
            encode(&())
        }
        UdpBind { port } => {
            let r = if own.udps.len() >= MAX_UDP { Err(NetError::Busy) } else { net.udp_bind(port) };
            if let Ok(id) = r {
                own.udps.push(id);
                net.set_owner(Proto::Udp, id.0, own.pid);
            }
            encode(&r)
        }
        UdpSend { id, dst, port, data } => {
            encode(&if !own.udps.contains(&id) {
                Err(NetError::Gone)
            } else if data.0.len() > UDP_MAX {
                Err(NetError::Protocol("datagrama demasiado grande"))
            } else {
                net.udp_send(id, dst, port, &data.0)
            })
        }
        UdpRecv { id } => encode(&if own.udps.contains(&id) { net.udp_recv(id) } else { Err(NetError::Gone) }),
        UdpClose { id } => {
            if let Some(i) = own.udps.iter().position(|&u| u == id) {
                own.udps.swap_remove(i);
                net.udp_close(id);
            }
            encode(&())
        }
        TcpListen { port } => {
            let r = if own.listens.len() >= MAX_LISTEN { Err(NetError::Busy) } else { net.tcp_listen(port) };
            if let Ok(id) = r {
                own.listens.push(id);
                net.set_owner(Proto::TcpListen, id.0, own.pid);
            }
            encode(&r)
        }
        TcpAccept { id } => {
            if !own.listens.contains(&id) {
                return encode(&Err::<Option<(SockId, Ipv4, u16)>, _>(NetError::Gone));
            }
            // Sin hueco en la lista del programa, la conexión espera en la cola de la pila.
            if own.socks.len() >= MAX_SOCKS {
                own.socks.retain(|&s| net.tcp_state(s) != TcpState::Failed(NetError::Gone));
                if own.socks.len() >= MAX_SOCKS {
                    return encode(&Ok::<Option<(SockId, Ipv4, u16)>, NetError>(None));
                }
            }
            let r = net.tcp_accept(id);
            if let Ok(Some((s, _, _))) = r {
                own.socks.push(s);
                net.set_owner(Proto::Tcp, s.0, own.pid);
            }
            encode(&r)
        }
        TcpUnlisten { id } => {
            if let Some(i) = own.listens.iter().position(|&l| l == id) {
                own.listens.swap_remove(i);
                net.tcp_unlisten(id);
            }
            encode(&())
        }
        // Un programa del sistema (el Administrador de tareas) ve todo; uno de terceros, solo lo suyo.
        NetConnections if system => encode(&net.connections()),
        NetConnections => {
            let mine = |c: &ConnInfo| match c.proto {
                Proto::Tcp => own.socks.contains(&SockId(c.id)),
                Proto::Udp => own.udps.contains(&UdpId(c.id)),
                Proto::TcpListen => own.listens.contains(&ListenId(c.id)),
            };
            encode(&net.connections().into_iter().filter(mine).collect::<Vec<_>>())
        }
        _ => encode(&()),
    }
}

// ---------- Lado del programa ----------

/// Cómo llega una petición al sistema (en un programa, `SYS_SERVICE` + `SYS_TAKE_REPLY`).
pub trait Transport {
    fn call(&mut self, req: &[u8]) -> Vec<u8>;
}

fn ask<T: Transport, R: Wire>(t: &mut T, c: Call, bad: R) -> R {
    match t.call(&encode(&c)).split_first() {
        Some((0, rest)) => decode(rest).unwrap_or(bad),
        _ => bad,
    }
}

/// `FilesApi` que lo pide todo al sistema. Con una respuesta rota o perdida devuelve un error de
/// E/S (o el valor neutro): nunca entra en pánico.
pub struct RemoteFiles<T: Transport>(pub T);

const UNDO_LABELS: [&str; 5] = ["Cambiar nombre", "Mover", "Copiar", "Nuevo", "Eliminar"];

impl<T: Transport> RemoteFiles<T> {
    /// Hace la petición; `bad` es lo que sale si la respuesta no vale.
    pub fn ask<R: Wire>(&mut self, c: Call, bad: R) -> R {
        ask(&mut self.0, c, bad)
    }
    fn fs<R: Wire>(&mut self, c: Call) -> Result<R, FsError> {
        self.ask(c, Err(FsError::Disk))
    }
    fn acc<R: Wire>(&mut self, c: Call) -> Result<R, AccountError> {
        self.ask(c, Err(AccountError::Fs(FsError::Disk)))
    }
    fn key<R: Wire>(&mut self, c: Call) -> Result<R, KeyError> {
        self.ask(c, Err(KeyError::Fs(FsError::Disk)))
    }
}

fn s(x: &str) -> String {
    String::from(x)
}

impl<T: Transport> FilesApi for RemoteFiles<T> {
    fn persistent(&mut self) -> bool {
        self.ask(Call::Persistent, false)
    }
    fn generation(&mut self) -> u64 {
        self.ask(Call::Generation, 0)
    }
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, FsError> {
        self.fs(Call::List { dir: s(dir) })
    }
    fn list_view(&mut self, dir: &str) -> Result<Vec<Entry>, FsError> {
        self.fs(Call::ListView { dir: s(dir) })
    }
    fn stat(&mut self, path: &str) -> Result<Entry, FsError> {
        self.fs(Call::Stat { path: s(path) })
    }
    fn exists(&mut self, path: &str) -> bool {
        self.ask(Call::Exists { path: s(path) }, false)
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        self.fs::<Blob>(Call::Read { path: s(path) }).map(|b| b.0)
    }
    fn write(&mut self, path: &str, data: &[u8], _now: DateTime) -> Result<(), FsError> {
        self.fs(Call::Write { path: s(path), data: Blob(data.to_vec()) })
    }
    fn mkdir(&mut self, path: &str, _now: DateTime) -> Result<(), FsError> {
        self.fs(Call::Mkdir { path: s(path) })
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        self.fs(Call::Rename { from: s(from), to: s(to) })
    }
    fn copy(&mut self, from: &str, to: &str, _now: DateTime) -> Result<(), FsError> {
        self.fs(Call::Copy { from: s(from), to: s(to) })
    }
    fn delete(&mut self, path: &str) -> Result<(), FsError> {
        self.fs(Call::Delete { path: s(path) })
    }
    fn free_name(&mut self, dir: &str, name: &str) -> String {
        self.ask(Call::FreeName { dir: s(dir), name: s(name) }, s(name))
    }
    fn trash_list(&mut self) -> Result<Vec<TrashItem>, FsError> {
        self.fs(Call::TrashList)
    }
    fn restore(&mut self, id: &str) -> Result<String, FsError> {
        self.fs(Call::Restore { id: s(id) })
    }
    fn empty_trash(&mut self) -> Result<(), FsError> {
        self.fs(Call::EmptyTrash)
    }
    fn space(&mut self) -> (u64, u64) {
        self.ask(Call::Space, (0, 0))
    }
    fn set_clip_many(&mut self, paths: &[String], cut: bool) {
        self.ask(Call::SetClipMany { paths: paths.to_vec(), cut }, ())
    }
    fn paste_all(&mut self, dir: &str, _now: DateTime) -> Result<Vec<String>, FsError> {
        self.fs(Call::PasteAll { dir: s(dir) })
    }
    fn transfer(&mut self, paths: &[String], dir: &str, mv: bool, _now: DateTime) -> Result<Vec<String>, FsError> {
        self.fs(Call::Transfer { paths: paths.to_vec(), dir: s(dir), mv })
    }
    fn user_rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        self.fs(Call::UserRename { from: s(from), to: s(to) })
    }
    fn user_trash_many(&mut self, paths: &[String], _now: DateTime) -> Result<(), FsError> {
        self.fs(Call::UserTrashMany { paths: paths.to_vec() })
    }
    fn user_create(&mut self, path: &str, dir: bool, _now: DateTime) -> Result<(), FsError> {
        self.fs(Call::UserCreate { path: s(path), dir })
    }
    fn undo(&mut self, _now: DateTime) -> Result<(), FsError> {
        self.fs(Call::Undo)
    }
    fn log(&mut self, text: &str, _now: DateTime) {
        self.ask(Call::Log { text: s(text) }, ())
    }
    fn home(&mut self) -> String {
        self.ask(Call::Home, String::new())
    }
    fn explorer_prefs(&mut self) -> String {
        self.ask(Call::ExplorerPrefs, String::new())
    }
    fn clip(&mut self) -> Option<(String, bool)> {
        self.ask(Call::Clip, None)
    }
    fn clip_paths(&mut self) -> (Vec<String>, bool) {
        self.ask(Call::ClipPaths, (Vec::new(), false))
    }
    fn user(&mut self) -> Option<String> {
        self.ask(Call::User, None)
    }
    fn undo_label(&mut self) -> Option<&'static str> {
        let l: Option<String> = self.ask(Call::UndoLabel, None);
        l.and_then(|l| UNDO_LABELS.iter().copied().find(|x| *x == l))
    }
    fn accounts(&mut self) -> Vec<Account> {
        self.ask(Call::Accounts, Vec::new())
    }
    fn is_admin(&mut self) -> bool {
        self.ask(Call::IsAdmin, false)
    }
    fn verify_password(&mut self, password: &str) -> bool {
        self.ask(Call::VerifyPassword { password: s(password) }, false)
    }
    fn has_password(&mut self) -> bool {
        self.ask(Call::HasPassword, false)
    }
    fn create_account(&mut self, name: &str, password: Option<&str>, admin: bool, _salt: [u8; 16], _now: DateTime) -> Result<(), AccountError> {
        self.acc(Call::CreateAccount { name: s(name), password: password.map(s), admin })
    }
    fn delete_account(&mut self, name: &str, keep_files: bool, _now: DateTime) -> Result<(), AccountError> {
        self.acc(Call::DeleteAccount { name: s(name), keep_files })
    }
    fn rename_account(&mut self, old: &str, new: &str, _now: DateTime) -> Result<(), AccountError> {
        self.acc(Call::RenameAccount { old: s(old), new: s(new) })
    }
    fn set_admin(&mut self, name: &str, admin: bool, _now: DateTime) -> Result<(), AccountError> {
        self.acc(Call::SetAdmin { name: s(name), admin })
    }
    fn set_password(&mut self, old: &str, new: Option<&str>, _salt: [u8; 16], _now: DateTime) -> Result<(), AccountError> {
        self.acc(Call::SetPassword { old: s(old), new: new.map(s) })
    }
    fn disk_encrypted(&mut self) -> bool {
        self.ask(Call::DiskEncrypted, false)
    }
    fn disk_conversion(&mut self) -> Option<(u64, u64, bool)> {
        self.ask(Call::DiskConversion, None)
    }
    fn convert_step(&mut self, blocks: u32, _now: DateTime) -> Result<bool, KeyError> {
        self.key(Call::ConvertStep { blocks })
    }
    fn home_encrypted(&mut self) -> bool {
        self.ask(Call::HomeEncrypted, false)
    }
    fn home_conversion(&mut self) -> Option<(u64, u64, bool)> {
        self.ask(Call::HomeConversion, None)
    }
    fn home_convert_step(&mut self, files: u32, _now: DateTime) -> Result<bool, KeyError> {
        self.key(Call::HomeConvertStep { files })
    }
    fn encrypt_disk(&mut self, password: &str, _pool: &mut Pool, _now: DateTime) -> Result<String, KeyError> {
        self.key(Call::EncryptDisk { password: s(password) })
    }
    fn decrypt_disk(&mut self, password: &str, _now: DateTime) -> Result<(), KeyError> {
        self.key(Call::DecryptDisk { password: s(password) })
    }
    fn change_disk_password(&mut self, old: &str, new: &str, _pool: &mut Pool, _now: DateTime) -> Result<(), KeyError> {
        self.key(Call::ChangeDiskPassword { old: s(old), new: s(new) })
    }
    fn new_disk_recovery(&mut self, password: &str, _pool: &mut Pool, _now: DateTime) -> Result<String, KeyError> {
        self.key(Call::NewDiskRecovery { password: s(password) })
    }
    fn encrypt_home(&mut self, password: &str, _pool: &mut Pool, _now: DateTime) -> Result<String, KeyError> {
        self.key(Call::EncryptHome { password: s(password) })
    }
    fn decrypt_home(&mut self, password: &str, _now: DateTime) -> Result<(), KeyError> {
        self.key(Call::DecryptHome { password: s(password) })
    }
    fn new_home_recovery(&mut self, password: &str, _pool: &mut Pool, _now: DateTime) -> Result<String, KeyError> {
        self.key(Call::NewHomeRecovery { password: s(password) })
    }
    fn drives(&mut self) -> Vec<DriveInfo> {
        self.ask(Call::Drives, Vec::new())
    }
    fn unlock_drive(&mut self, letter: char, secret: &str, recovery: bool) -> Result<(), KeyError> {
        self.key(Call::UnlockDrive { letter, secret: s(secret), recovery })
    }
    fn request_drive_action(&mut self, letter: char, req: FormatRequest) -> Result<(), FsError> {
        self.fs(Call::DriveAction { letter, req })
    }
    fn net_perms(&mut self) -> Vec<netperm::Perm> {
        self.ask(Call::NetPerms, Vec::new())
    }
    fn set_net_perm(&mut self, hash: [u8; 32], out: Option<bool>, inn: Option<bool>, _now: DateTime) -> Result<(), FsError> {
        self.fs(Call::SetNetPerm { hash, out, inn })
    }
    fn custom_roots(&mut self) -> Vec<Vec<u8>> {
        self.ask(Call::CustomRoots, Vec::new())
    }
    fn add_root(&mut self, der: Vec<u8>, _now: DateTime) -> Result<(), FsError> {
        self.fs(Call::AddRoot { der: Blob(der) })
    }
    fn remove_root(&mut self, sha: [u8; 32], _now: DateTime) -> Result<(), FsError> {
        self.fs(Call::RemoveRoot { sha })
    }
    fn drive_result(&mut self) -> Option<Result<Option<String>, FsError>> {
        self.ask(Call::DriveResult, None)
    }
}

/// `NetApi` de un programa: cada método es un viaje al sistema. Sin respuesta o con una rota, no
/// hay tarjeta de red (nunca un pánico).
pub struct RemoteNet<T: Transport>(pub T);

impl<T: Transport> RemoteNet<T> {
    fn net<R: Wire>(&mut self, c: Call) -> Result<R, NetError> {
        ask(&mut self.0, c, Err(NetError::NoAdapter))
    }
    fn poll<R: Wire>(&mut self, c: Call) -> Option<Result<R, NetError>> {
        ask(&mut self.0, c, Some(Err(NetError::NoAdapter)))
    }
}

impl<T: Transport> NetApi for RemoteNet<T> {
    fn status(&mut self) -> NetStatus {
        ask(&mut self.0, Call::NetStatus, no_status())
    }
    fn renew(&mut self) {
        ask(&mut self.0, Call::NetRenew, ())
    }
    fn ping(&mut self, dst: Ipv4, bytes: u16, ttl: u8) -> Result<PingId, NetError> {
        self.net(Call::NetPing { dst, bytes, ttl })
    }
    fn ping_poll(&mut self, id: PingId) -> Option<Result<PingReply, NetError>> {
        self.poll(Call::NetPingPoll { id })
    }
    fn resolve(&mut self, name: &str) -> Result<QueryId, NetError> {
        self.net(Call::NetResolve { name: s(name) })
    }
    fn resolve_poll(&mut self, id: QueryId) -> Option<Result<Vec<Ipv4>, NetError>> {
        self.poll(Call::NetResolvePoll { id })
    }
    fn tcp_connect(&mut self, dst: Ipv4, port: u16) -> Result<SockId, NetError> {
        self.net(Call::NetConnect { dst, port })
    }
    fn tcp_state(&mut self, id: SockId) -> TcpState {
        ask(&mut self.0, Call::NetState { id }, TcpState::Failed(NetError::NoAdapter))
    }
    fn tcp_send(&mut self, id: SockId, data: &[u8]) -> Result<usize, NetError> {
        let n = data.len().min(NET_CHUNK);
        self.net::<u64>(Call::NetSend { id, data: Blob(data[..n].to_vec()) }).map(|n| (n as usize).min(data.len()))
    }
    fn tcp_recv(&mut self, id: SockId, buf: &mut [u8]) -> Result<usize, NetError> {
        let max = buf.len().min(NET_CHUNK) as u32;
        let b: Blob = self.net(Call::NetRecv { id, max })?;
        // Un sistema que mintiera con más bytes de los pedidos no desborda el búfer.
        let n = b.0.len().min(buf.len());
        buf[..n].copy_from_slice(&b.0[..n]);
        Ok(n)
    }
    fn tcp_close(&mut self, id: SockId) {
        ask(&mut self.0, Call::NetClose { id }, ())
    }
    fn tcp_abort(&mut self, id: SockId) {
        ask(&mut self.0, Call::NetAbort { id }, ())
    }
    fn udp_bind(&mut self, port: u16) -> Result<UdpId, NetError> {
        self.net(Call::UdpBind { port })
    }
    fn udp_send(&mut self, id: UdpId, dst: Ipv4, port: u16, data: &[u8]) -> Result<(), NetError> {
        if data.len() > UDP_MAX {
            return Err(NetError::Protocol("datagrama demasiado grande"));
        }
        self.net(Call::UdpSend { id, dst, port, data: Blob(data.to_vec()) })
    }
    fn udp_recv(&mut self, id: UdpId) -> Result<Option<Datagram>, NetError> {
        let mut d: Option<Datagram> = self.net(Call::UdpRecv { id })?;
        // Un sistema que mintiera con un datagrama enorme no hace crecer la memoria del programa sin tope.
        if let Some(g) = d.as_mut() {
            g.data.truncate(UDP_MAX);
        }
        Ok(d)
    }
    fn udp_close(&mut self, id: UdpId) {
        ask(&mut self.0, Call::UdpClose { id }, ())
    }
    fn tcp_listen(&mut self, port: u16) -> Result<ListenId, NetError> {
        self.net(Call::TcpListen { port })
    }
    fn tcp_accept(&mut self, id: ListenId) -> Result<Option<(SockId, Ipv4, u16)>, NetError> {
        self.net(Call::TcpAccept { id })
    }
    fn tcp_unlisten(&mut self, id: ListenId) {
        ask(&mut self.0, Call::TcpUnlisten { id }, ())
    }
    fn connections(&mut self) -> Vec<ConnInfo> {
        ask(&mut self.0, Call::NetConnections, Vec::new())
    }
    fn set_owner(&mut self, _proto: Proto, _id: u32, _owner: u64) {}
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::apps::{self, AppKind, Ctx};
    use crate::dialogs::{Answer, DialogRequest, DialogResult};
    use crate::input::{Key, KeyEvent};
    use crate::platform::TestPlatform;
    use crate::surface::Surface;
    use crate::theme::Theme;
    use alloc::format;
    use alloc::rc::Rc;
    use core::cell::RefCell;

    const DOCS: &str = "C:\\Usuarios\\Usuario\\Documentos";
    const PICS: &str = "C:\\Usuarios\\Usuario\\Imágenes";
    const NOW: DateTime = DateTime { year: 2026, month: 11, day: 18, hour: 1, minute: 2, second: 3 };
    /// La fecha que pone el sistema (la de `TestPlatform`), no la que mande el programa.
    const NOW_REAL: DateTime = DateTime { year: 2026, month: 11, day: 15, hour: 21, minute: 37, second: 5 };

    type Real = Rc<RefCell<TestPlatform>>;

    /// Transporte de prueba: llama directamente a `serve` sobre la plataforma de verdad.
    struct Via {
        real: Real,
        system: bool,
        own: Rc<RefCell<NetOwn>>,
    }

    impl Transport for Via {
        fn call(&mut self, req: &[u8]) -> Vec<u8> {
            serve(&mut *self.real.borrow_mut(), req, self.system, &mut self.own.borrow_mut())
        }
    }

    /// Plataforma de un programa: lo suyo es local y los archivos van por `RemoteFiles`.
    pub(crate) struct Prog {
        real: Real,
        fs: RemoteFiles<Via>,
        dummy: Files,
        pool: Pool,
        net: RemoteNet<Via>,
        own: Rc<RefCell<NetOwn>>,
    }

    impl Platform for Prog {
        fn now(&mut self) -> DateTime {
            self.real.borrow_mut().now()
        }
        fn uptime_ms(&self) -> u64 {
            0
        }
        fn mem(&self) -> MemInfo {
            self.real.borrow().mem()
        }
        fn cpu(&self) -> String {
            self.real.borrow().cpu()
        }
        fn screen(&self) -> (u32, u32) {
            (1280, 800)
        }
        fn reboot(&mut self) {}
        fn shutdown(&mut self) {}
        fn files(&mut self) -> &mut Files {
            &mut self.dummy
        }
        fn fs(&mut self) -> &mut dyn FilesApi {
            &mut self.fs
        }
        fn entropy(&mut self) -> u64 {
            self.real.borrow_mut().entropy()
        }
        fn pool(&mut self) -> &mut Pool {
            &mut self.pool
        }
        fn net(&mut self) -> Option<&mut dyn NetApi> {
            Some(&mut self.net)
        }
    }

    pub(crate) fn prog(system: bool) -> (Real, Prog) {
        let real: Real = Rc::new(RefCell::new(TestPlatform::default()));
        let own = Rc::new(RefCell::new(NetOwn::default()));
        let via = || Via { real: real.clone(), system, own: own.clone() };
        let p = Prog { real: real.clone(), fs: RemoteFiles(via()), dummy: Files::default(), pool: Pool::new(), net: RemoteNet(via()), own: own.clone() };
        (real, p)
    }

    fn tecla(app: &mut Box<dyn apps::App>, p: &mut Prog, key: Key, ch: Option<char>, ctrl: bool) -> Option<DialogRequest> {
        let mut th = Theme::default();
        let mut cx = Ctx::new(p, &mut th, 0);
        app.key(&KeyEvent { key, ch, down: true, shift: false, ctrl, alt: false }, &mut cx);
        cx.dialog.take()
    }

    fn dibuja(app: &mut Box<dyn apps::App>, p: &mut Prog) {
        app.draw(&mut Surface::new(1000, 520, 0), &Theme::default(), p, true);
    }

    fn abre(app: &mut Box<dyn apps::App>, p: &mut Prog, ruta: &str) {
        let mut th = Theme::default();
        let mut cx = Ctx::new(p, &mut th, 0);
        app.open_path(ruta, &mut cx);
        dibuja(app, p);
    }

    #[test]
    fn mensajes_rotos_no_hacen_panico() {
        let mut sys = TestPlatform::default();
        assert_eq!(serve(&mut sys, &[], true, &mut NetOwn::default()), [1]);
        assert_eq!(serve(&mut sys, &[250], true, &mut NetOwn::default()), [1], "llamada que no existe");
        assert_eq!(serve(&mut sys, &[2, 255, 255, 255, 255], true, &mut NetOwn::default()), [1], "longitud que miente");
        assert_eq!(serve(&mut sys, &[15, 0], true, &mut NetOwn::default()), [1], "sobra un byte");
        // Respuestas rotas al otro lado: error de E/S o valor neutro.
        struct Roto(Vec<u8>);
        impl Transport for Roto {
            fn call(&mut self, _: &[u8]) -> Vec<u8> {
                self.0.clone()
            }
        }
        for r in [vec![], vec![1], vec![0], vec![0, 9, 9, 9], vec![7, 0]] {
            let mut f = RemoteFiles(Roto(r));
            assert_eq!(f.read("C:\\a"), Err(FsError::Disk));
            assert_eq!(f.write("C:\\a", b"x", NOW), Err(FsError::Disk));
            assert!(!f.exists("C:\\a") && !f.is_admin() && f.accounts().is_empty() && f.user().is_none());
            assert_eq!(f.encrypt_home("x", &mut Pool::new(), NOW), Err(KeyError::Fs(FsError::Disk)));
        }
    }

    #[test]
    fn tipos_ida_y_vuelta() {
        let e = Entry { name: s("ñ.txt"), dir: false, size: 7, modified: NOW, hidden: false };
        assert_eq!(decode::<Result<Entry, FsError>>(&encode(&Ok::<_, FsError>(e.clone()))), Ok(Ok(e)));
        for fe in FS_ERRORS {
            assert_eq!(decode::<FsError>(&encode(&fe)), Ok(fe));
        }
        for ae in ACCOUNT_ERRORS.into_iter().chain([AccountError::Fs(FsError::Denied)]) {
            assert_eq!(decode::<AccountError>(&encode(&ae)), Ok(ae));
        }
        for ke in KEY_ERRORS.into_iter().chain([KeyError::Fs(FsError::Corrupt)]) {
            assert_eq!(decode::<KeyError>(&encode(&ke)), Ok(ke));
        }
        let t = TrashItem { id: s("1"), name: s("a"), original: s("C:\\a"), dir: true, size: 3, deleted: NOW };
        assert_eq!(decode::<TrashItem>(&encode(&t)), Ok(t));
        let c = Call::Transfer { paths: vec![s("a"), s("b")], dir: s("C:\\d"), mv: true };
        let Ok(Call::Transfer { paths, dir, mv }) = decode::<Call>(&encode(&c)) else { panic!() };
        assert_eq!((paths.len(), dir.as_str(), mv), (2, "C:\\d", true));
        assert!(decode::<FsError>(&[99]).is_err() && decode::<AccountError>(&[99]).is_err());
    }

    #[test]
    fn bloc_de_notas_crea_guarda_y_lee_por_el_servicio() {
        let (real, mut p) = prog(false);
        let mut np = apps::create(AppKind::Notepad);
        tecla(&mut np, &mut p, Key::Char('h'), Some('h'), false);
        tecla(&mut np, &mut p, Key::Char('i'), Some('i'), false);
        let Some(DialogRequest::Save { tag, .. }) = tecla(&mut np, &mut p, Key::Char('s'), Some('s'), true) else { panic!("sin Guardar como") };
        let ruta = format!("{DOCS}\\hola.txt");
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut p, &mut th, 0);
        np.dialog_result(DialogResult::Path { tag, path: Some(ruta.clone()) }, &mut cx);
        assert!(cx.dialog.is_none(), "se guardó sin error");
        // Está en el sistema de archivos de verdad, con la fecha del sistema.
        assert_eq!(real.borrow_mut().files.read(&ruta).unwrap(), b"hi");
        assert_eq!(real.borrow_mut().files.stat(&ruta).unwrap().modified, NOW_REAL);
        // Otro Bloc de notas lo abre.
        let mut otro = apps::create(AppKind::Notepad);
        otro.open_path(&ruta, &mut cx);
        assert_eq!(otro.title(), "hola.txt: Bloc de notas");
        // Y escribir en el sistema no.
        let r = cx.sys.fs().write("C:\\Sistema\\config\\x", b"x", NOW);
        assert_eq!(r, Err(FsError::Denied));
    }

    #[test]
    fn explorador_lista_copia_y_manda_a_la_papelera_por_el_servicio() {
        let (real, mut p) = prog(false);
        real.borrow_mut().files.write(&format!("{DOCS}\\nota.txt"), b"hola", NOW).unwrap();
        let mut ex = apps::create(AppKind::Files);
        abre(&mut ex, &mut p, DOCS);
        assert!(p.fs().list(DOCS).unwrap().iter().any(|e| e.name == "nota.txt"));
        // Seleccionar todo y copiar: el portapapeles está en el sistema.
        tecla(&mut ex, &mut p, Key::Char('a'), None, true);
        tecla(&mut ex, &mut p, Key::Char('c'), None, true);
        assert_eq!(real.borrow_mut().files.clip_paths().0, [format!("{DOCS}\\nota.txt")]);
        abre(&mut ex, &mut p, PICS);
        tecla(&mut ex, &mut p, Key::Char('v'), None, true);
        assert!(real.borrow_mut().files.exists(&format!("{PICS}\\nota.txt")), "copiado");
        assert!(real.borrow_mut().files.exists(&format!("{DOCS}\\nota.txt")), "el original sigue");
        // Lo pegado queda seleccionado: Suprimir lo manda a la Papelera tras confirmar.
        dibuja(&mut ex, &mut p);
        let Some(DialogRequest::Message { tag, .. }) = tecla(&mut ex, &mut p, Key::Delete, None, false) else { panic!("sin confirmación") };
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut p, &mut th, 0);
        ex.dialog_result(DialogResult::Answer { tag, answer: Answer::Yes }, &mut cx);
        assert!(!real.borrow_mut().files.exists(&format!("{PICS}\\nota.txt")));
        let t = real.borrow_mut().files.trash_list().unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].original, format!("{PICS}\\nota.txt"));
    }

    #[test]
    fn un_programa_no_toca_el_sistema() {
        let (real, mut p) = prog(false);
        {
            let mut r = real.borrow_mut();
            r.files.write("C:\\Sistema\\config\\ajuste", b"1", NOW).unwrap();
            r.files.write(&format!("{DOCS}\\a.txt"), b"a", NOW).unwrap();
            r.files.write("C:\\Sistema\\temp\\viejo", b"v", NOW).unwrap();
            r.files.trash("C:\\Sistema\\temp\\viejo", NOW).unwrap();
        }
        let f = p.fs();
        let d = FsError::Denied;
        // Con rutas disfrazadas (mayúsculas, "..", "/") también.
        for sis in ["C:\\Sistema\\config\\nuevo", "c:\\SISTEMA\\x", "C:\\Usuarios\\..\\Sistema\\x", "C:/Sistema/x", "\\sistema\\x"] {
            assert_eq!(f.write(sis, b"x", NOW), Err(d), "{sis}");
            assert_eq!(f.mkdir(sis, NOW), Err(d), "{sis}");
            assert_eq!(f.delete(sis), Err(d), "{sis}");
            assert_eq!(f.user_create(sis, false, NOW), Err(d), "{sis}");
            assert_eq!(f.rename(&format!("{DOCS}\\a.txt"), sis), Err(d), "{sis}");
            assert_eq!(f.copy(&format!("{DOCS}\\a.txt"), sis, NOW), Err(d), "{sis}");
            assert_eq!(f.user_trash_many(&[s(sis)], NOW), Err(d), "{sis}");
        }
        assert_eq!(f.rename("C:\\Sistema\\config\\ajuste", &format!("{DOCS}\\b")), Err(d), "desde el sistema");
        assert_eq!(f.copy("C:\\Sistema\\config\\ajuste", &format!("{DOCS}\\b"), NOW), Err(d));
        assert_eq!(f.user_rename("C:\\Sistema\\config\\ajuste", &format!("{DOCS}\\b")), Err(d));
        assert_eq!(f.transfer(&[s("C:\\Sistema\\config\\ajuste")], DOCS, false, NOW), Err(d));
        assert_eq!(f.transfer(&[format!("{DOCS}\\a.txt")], "C:\\Sistema\\config", true, NOW), Err(d));
        // Pegar desde el sistema o hacia él.
        f.set_clip_many(&[s("C:\\Sistema\\config\\ajuste")], false);
        assert_eq!(f.paste_all(DOCS, NOW), Err(d));
        f.set_clip_many(&[format!("{DOCS}\\a.txt")], false);
        assert_eq!(f.paste_all("C:\\Sistema\\config", NOW), Err(d));
        // Restaurar algo que estaba en el sistema.
        let id = f.trash_list().unwrap()[0].id.clone();
        assert_eq!(f.restore(&id), Err(d));
        // Nada de eso cambió el disco. Leer el sistema tampoco se puede (con sesión de
        // administrador vería las cuentas); lo suyo, sí.
        assert_eq!(f.read("C:\\Sistema\\config\\ajuste"), Err(d));
        assert_eq!(f.read("c:/sistema/config/../config/ajuste"), Err(d));
        assert!(!f.exists("C:\\Sistema\\config\\nuevo") && !f.exists(&format!("{DOCS}\\b")));
        assert_eq!(f.write(&format!("{DOCS}\\c.txt"), b"c", NOW), Ok(()));
        assert_eq!(f.rename(&format!("{DOCS}\\c.txt"), &format!("{DOCS}\\d.txt")), Ok(()));
        assert_eq!(f.transfer(&[format!("{DOCS}\\d.txt")], PICS, false, NOW).unwrap().len(), 1);
        assert!(real.borrow_mut().files.exists(&format!("{PICS}\\d.txt")));
    }

    #[test]
    fn solo_el_sistema_toca_los_programas_y_lee_el_sistema() {
        let (real, mut p) = prog(false);
        real.borrow_mut().files.write("C:\\Sistema\\config\\ajuste", b"1", NOW).unwrap();
        let _ = real.borrow_mut().files.mkdir("C:\\Programas", NOW);
        real.borrow_mut().files.mkdir("C:\\Programas\\Reloj", NOW).unwrap();
        real.borrow_mut().files.write("C:\\Programas\\Reloj\\programa.nfx", b"bin", NOW).unwrap();
        let d = FsError::Denied;
        let f = p.fs();
        for pr in ["C:\\Programas\\Reloj\\programa.nfx", "c:/PROGRAMAS/Reloj/x", "C:\\Usuarios\\..\\Programas\\Nuevo"] {
            assert_eq!(f.write(pr, b"troyano", NOW), Err(d), "{pr}");
            assert_eq!(f.delete(pr), Err(d), "{pr}");
            assert_eq!(f.user_trash_many(&[s(pr)], NOW), Err(d), "{pr}");
        }
        assert_eq!(f.read("C:\\Programas\\Reloj\\programa.nfx").unwrap(), b"bin", "leer programas sí");
        // Un nombre que solo empieza igual no es la carpeta de programas.
        assert_eq!(f.write(&format!("{DOCS}\\Programas.txt"), b"x", NOW), Ok(()));
        assert!(!blocked(false, "C:\\ProgramasX\\a") && blocked(true, "C:\\Sistema\\a") && !blocked(true, "C:\\Programas\\a"));
        // Un programa del sistema (Configuración instala) sí.
        let (real, mut p) = prog(true);
        real.borrow_mut().files.write("C:\\Sistema\\config\\ajuste", b"1", NOW).unwrap();
        let f = p.fs();
        let _ = f.mkdir("C:\\Programas", NOW);
        assert_eq!(f.mkdir("C:\\Programas\\Nuevo", NOW), Ok(()));
        assert_eq!(f.write("C:\\Programas\\Nuevo\\app.txt", b"nombre=Nuevo", NOW), Ok(()));
        assert_eq!(f.read("C:\\Sistema\\config\\ajuste").unwrap(), b"1");
    }

    #[test]
    fn cuentas_claves_y_cifrado_solo_para_el_sistema() {
        let (real, mut p) = prog(false);
        let f = p.fs();
        assert!(f.accounts().is_empty() && !f.verify_password("") && !f.has_password());
        assert_eq!(f.create_account("Marta", None, false, [0; 16], NOW), Err(AccountError::NotAdmin));
        assert_eq!(f.set_password("", Some("clave-nueva"), [0; 16], NOW), Err(AccountError::NotAdmin));
        assert_eq!(f.delete_account("Usuario", false, NOW), Err(AccountError::NotAdmin));
        assert_eq!(f.encrypt_home("", &mut Pool::new(), NOW), Err(KeyError::NotAdmin));
        assert_eq!(f.decrypt_disk("", NOW), Err(KeyError::NotAdmin));
        assert!(!f.disk_encrypted() && f.home_conversion().is_none());
        f.log("intruso", NOW);
        f.set_admin("Usuario", false, NOW).ok();
        assert!(real.borrow_mut().files.is_admin(), "sigue siendo administrador");
        // Reiniciar y apagar tampoco.
        p.fs.ask(Call::Reboot, ());
        p.fs.ask(Call::Shutdown, ());
        assert!(!real.borrow().rebooted && !real.borrow().halted);
        // Lo básico del equipo y de la sesión sí.
        assert_eq!(p.fs.ask(Call::Cpu, String::new()), "CPU de pruebas");
        assert_eq!(p.fs().user().as_deref(), Some("Usuario"));
        assert!(p.fs().is_admin());

        // Un programa del sistema sí; la sal y el acumulador los pone el sistema.
        let (real, mut p) = prog(true);
        let f = p.fs();
        assert_eq!(f.create_account("Ana", None, true, [0; 16], NOW), Ok(()), "la primera cuenta siempre es administradora");
        assert_eq!(f.create_account("Marta", Some("clave-marta"), false, [0; 16], NOW), Ok(()));
        let cuentas = f.accounts();
        assert!(cuentas.iter().any(|a| a.name == "Marta" && !a.admin));
        assert!(f.verify_password("") && !f.verify_password("x"));
        let hay = real.borrow_mut().files.accounts();
        let marta = hay.iter().find(|a| a.name == "Marta").unwrap();
        assert!(marta.verify("clave-marta") && marta.has_password(), "la clave se guardó con una sal del sistema");
        p.fs.ask(Call::Reboot, ());
        p.fs.ask(Call::Shutdown, ());
        assert!(real.borrow().rebooted && real.borrow().halted);
    }

    fn usb() -> Box<dyn crate::vol::Volume> {
        use crate::vol::{self, MemSectors};
        let mut m = MemSectors::new(131072, 512);
        vol::format(&mut m, FsKind::Fat32, "USB", NOW).unwrap();
        let part = vol::partitions(&mut m).unwrap();
        vol::mount(Box::new(m), &part[0]).unwrap()
    }

    #[test]
    fn unidades_extraibles_por_el_servicio() {
        for system in [false, true] {
            let (real, mut p) = prog(system);
            assert_eq!(real.borrow_mut().files.attach(1, "Pendrive", usb()), Some('D'));
            let f = p.fs();
            let d = f.drives();
            assert_eq!(d.iter().map(|i| (i.letter, i.removable)).collect::<Vec<_>>(), [('C', false), ('D', true)]);
            assert_eq!((d[1].label.as_str(), d[1].device.as_str(), d[1].kind), ("USB", "Pendrive", FsKind::Fat16));
            // Los programas de terceros también leen y escriben en D:, también en una carpeta llamada Programas.
            f.mkdir("D:\\Programas", NOW).unwrap();
            f.write("D:\\Programas\\a.txt", b"hola", NOW).unwrap();
            assert_eq!(f.read("D:\\programas\\a.txt").unwrap(), b"hola");
            assert_eq!(f.list("D:\\").unwrap().len(), 1);
            // Pero C:\Sistema sigue cerrado, también desde una unidad extraíble.
            assert_eq!(f.copy("D:\\Programas\\a.txt", "C:\\Sistema\\temp\\a.txt", NOW), Err(FsError::Denied));
            assert_eq!(f.transfer(&[s("D:\\Programas\\a.txt")], "C:\\Sistema\\temp", false, NOW), Err(FsError::Denied));
            assert!(!f.exists("C:\\Sistema\\temp\\a.txt"));
            // Ni copiando la raíz de C:, que lleva dentro C:\Sistema.
            if !system {
                for root in ["C:\\", "c:", "C:\\Usuarios\\.."] {
                    assert_eq!(f.copy(root, "D:\\x", NOW), Err(FsError::Denied), "{root}");
                    assert_eq!(f.transfer(&[s(root)], "D:\\", false, NOW), Err(FsError::Denied), "{root}");
                    f.set_clip_many(&[s(root)], false);
                    assert_eq!(f.paste_all("D:\\", NOW), Err(FsError::Denied), "{root}");
                }
                assert!(!f.exists("D:\\x"));
            }
            // Abrir, expulsar, formatear y cifrar: solo el sistema.
            assert_eq!(f.unlock_drive('D', "x", false), Err(if system { KeyError::NotEncrypted } else { KeyError::NotAdmin }));
            assert_eq!(f.request_drive_action('C', FormatRequest::Eject), Err(if system { FsError::Invalid } else { FsError::Denied }));
            if !system {
                assert_eq!(f.request_drive_action('D', FormatRequest::Eject), Err(FsError::Denied));
                assert!(f.drive_result().is_none());
                assert!(real.borrow_mut().files.take_drive_actions().is_empty());
                assert_eq!(f.drives().len(), 2);
            } else {
                let req = FormatRequest::Format { kind: FormatKind::ExFat, label: s("Ñ") };
                assert_eq!(f.request_drive_action('D', req), Ok(()));
                assert_eq!(real.borrow_mut().files.take_drive_actions().len(), 1);
                real.borrow_mut().files.set_drive_result(Ok(Some(s("codigo"))));
                assert_eq!(p.fs().drive_result(), Some(Ok(Some(s("codigo")))));
                assert_eq!(p.fs().drives().len(), 1);
            }
        }
        let c = Call::DriveAction { letter: 'E', req: FormatRequest::Encrypt { label: s("x"), password: s("y") } };
        assert!(matches!(decode::<Call>(&encode(&c)), Ok(Call::DriveAction { letter: 'E', req: FormatRequest::Encrypt { .. } })));
        assert!(decode::<FormatRequest>(&[9]).is_err() && decode::<char>(&0xD800u32.to_le_bytes()).is_err());
    }

    // ---------- Red por el servicio ----------

    use crate::net::tests::{Lan, SRV};
    use crate::net::Net;

    /// Un programa con la pila de la `Lan` ya en el sistema. La pila vive en `real`; `lan` guarda una
    /// vacía y se la presta a `corre` para avanzar el tiempo.
    fn con_lan(system: bool) -> (Real, Prog, Lan) {
        let (real, p) = prog(system);
        let mut lan = Lan::up();
        lan.router.names.push((s("ejemplo.com"), vec![SRV], 300));
        lan.router.srv.listen(80, b"hola", true, false);
        real.borrow_mut().net = Some(core::mem::replace(&mut lan.net, Net::new([0; 32])));
        (real, p, lan)
    }

    fn corre(real: &Real, lan: &mut Lan, ms: u64) {
        let mut r = real.borrow_mut();
        let n = r.net.as_mut().unwrap();
        core::mem::swap(&mut lan.net, n);
        lan.run(ms);
        core::mem::swap(&mut lan.net, n);
    }

    /// Otro programa sobre el mismo sistema, con sus propios identificadores.
    fn otro(real: &Real, system: bool) -> RemoteNet<Via> {
        RemoteNet(Via { real: real.clone(), system, own: Rc::new(RefCell::new(NetOwn::default())) })
    }

    #[test]
    fn red_ping_dns_y_tcp_por_el_servicio() {
        let (real, mut p, mut lan) = con_lan(true);
        let st = p.net.status();
        assert_eq!((st.state, st.ip, st.adapter.as_str()), (State::Connected, Some([192, 168, 1, 50]), "Prueba"));
        let id = p.net.ping(SRV, 32, 64).unwrap();
        let rep = (0..200).find_map(|_| {
            corre(&real, &mut lan, 20);
            p.net.ping_poll(id)
        });
        assert_eq!(rep.unwrap().unwrap().from, SRV);
        assert_eq!(p.net.ping_poll(id), Some(Err(NetError::Gone)), "ya recogido");
        let q = p.net.resolve("ejemplo.com").unwrap();
        let ips = (0..300).find_map(|_| {
            corre(&real, &mut lan, 20);
            p.net.resolve_poll(q)
        });
        assert_eq!(ips, Some(Ok(vec![SRV])));
        assert_eq!(p.net.resolve(&"a".repeat(300)), Err(NetError::NotFound));
        let sk = p.net.tcp_connect(SRV, 80).unwrap();
        while p.net.tcp_state(sk) == TcpState::Connecting {
            corre(&real, &mut lan, 20);
        }
        assert_eq!(p.net.tcp_state(sk), TcpState::Open);
        assert_eq!(p.net.tcp_send(sk, b"GET / HTTP/1.1\r\n\r\n"), Ok(18));
        let mut got = Vec::new();
        let mut buf = [0u8; 16];
        for _ in 0..100 {
            corre(&real, &mut lan, 20);
            let n = p.net.tcp_recv(sk, &mut buf).unwrap();
            got.extend_from_slice(&buf[..n]);
        }
        assert_eq!(got, b"hola");
        p.net.tcp_close(sk);
        assert_eq!(p.net.tcp_state(sk), TcpState::Failed(NetError::Gone), "ya no es suyo");
        p.net.renew();
    }

    #[test]
    fn red_denegada_a_programas_de_terceros() {
        let (_real, mut p, _lan) = con_lan(false);
        assert_eq!(p.net.status().state, State::NoAdapter);
        assert_eq!(p.net.ping(SRV, 32, 64), Err(NetError::NoAdapter));
        assert_eq!(p.net.ping_poll(PingId(0)), Some(Err(NetError::NoAdapter)));
        assert_eq!(p.net.resolve("ejemplo.com"), Err(NetError::NoAdapter));
        assert_eq!(p.net.tcp_connect(SRV, 80), Err(NetError::NoAdapter));
        assert_eq!(p.net.tcp_state(SockId(0)), TcpState::Failed(NetError::NoAdapter));
        assert_eq!(p.net.tcp_send(SockId(0), b"x"), Err(NetError::NoAdapter));
        assert_eq!(p.net.tcp_recv(SockId(0), &mut [0u8; 4]), Err(NetError::NoAdapter));
        // Sin pila en el sistema, igual.
        let (_real, mut p) = prog(true);
        assert_eq!(p.net.status().state, State::NoAdapter);
        assert_eq!(p.net.tcp_connect(SRV, 80), Err(NetError::NoAdapter));
    }

    #[test]
    fn red_de_terceros_con_aviso_que_se_recuerda() {
        let (real, mut p, mut lan) = con_lan(false);
        p.own.borrow_mut().set_program([7; 32], "Mensajes");
        // El estado se ve sin preguntar, pero sin la MAC; renovar no hace nada.
        let st = p.net.status();
        assert_eq!((st.state, st.mac), (State::Connected, [0; 6]));
        // Primera vez: aviso de «usar la red», una sola vez aunque insista.
        assert_eq!(p.net.tcp_connect(SRV, 80), Err(NetError::Pending));
        assert_eq!(p.net.resolve("ejemplo.com"), Err(NetError::Pending));
        assert_eq!(p.own.borrow_mut().prompt(), Some((Kind::Out, s("Mensajes"))));
        assert_eq!(p.own.borrow_mut().prompt(), None, "ya en pantalla");
        p.own.borrow_mut().answered(&mut *real.borrow_mut(), true);
        let sk = p.net.tcp_connect(SRV, 80).unwrap();
        corre(&real, &mut lan, 200);
        assert_eq!(p.net.tcp_state(sk), TcpState::Open);
        // Escuchar es otro permiso; los puertos bajos no se dan; se dice que no y se recuerda.
        assert_eq!(p.net.tcp_listen(80), Err(NetError::InUse));
        assert_eq!(p.net.udp_bind(5000), Err(NetError::Pending));
        assert_eq!(p.own.borrow_mut().prompt(), Some((Kind::In, s("Mensajes"))));
        p.own.borrow_mut().answered(&mut *real.borrow_mut(), false);
        assert_eq!(p.net.tcp_listen(5000), Err(NetError::Denied));
        // Guardado para todas las cuentas y en el registro.
        let perms = real.borrow_mut().files().net_perms();
        assert_eq!(perms, vec![netperm::Perm { hash: [7; 32], name: s("Mensajes"), out: Some(true), inn: Some(false) }]);
        let log = real.borrow_mut().files().read(crate::files::LOG_FILE).unwrap();
        assert!(String::from_utf8_lossy(&log).contains("Red: «Mensajes» puede usar la red"));
        // Otra ventana del mismo programa ya no pregunta; uno con otro contenido, sí.
        let mut b = otro(&real, false);
        b.0.own.borrow_mut().set_program([7; 32], "Mensajes");
        assert!(b.tcp_connect(SRV, 80).is_ok());
        let mut c = otro(&real, false);
        c.0.own.borrow_mut().set_program([8; 32], "Mensajes");
        assert_eq!(c.tcp_connect(SRV, 80), Err(NetError::Pending));
        // Quitado en Configuración: lo nuevo se niega.
        let mut perms = real.borrow_mut().files().net_perms();
        perms[0].out = Some(false);
        real.borrow_mut().files().set_net_perms(&perms, NOW).unwrap();
        assert_eq!(p.net.tcp_connect(SRV, 80), Err(NetError::Denied));
    }

    #[test]
    fn red_un_programa_no_toca_lo_de_otro() {
        let (real, mut a, mut lan) = con_lan(true);
        let mut b = otro(&real, true);
        let sk = a.net.tcp_connect(SRV, 80).unwrap();
        let ping = a.net.ping(SRV, 8, 64).unwrap();
        let q = a.net.resolve("ejemplo.com").unwrap();
        corre(&real, &mut lan, 100);
        let gone = TcpState::Failed(NetError::Gone);
        assert_eq!((b.tcp_state(sk), b.tcp_send(sk, b"x"), b.tcp_recv(sk, &mut [0u8; 4])), (gone, Err(NetError::Gone), Err(NetError::Gone)));
        b.tcp_abort(sk);
        b.tcp_close(sk);
        assert_eq!(b.ping_poll(ping), Some(Err(NetError::Gone)));
        assert_eq!(b.resolve_poll(q), Some(Err(NetError::Gone)));
        // Lo de A sigue en pie.
        assert_eq!(a.net.tcp_state(sk), TcpState::Open);
        assert!(matches!(a.net.ping_poll(ping), Some(Ok(_))));
        assert_eq!(a.net.resolve_poll(q), Some(Ok(vec![SRV])));
    }

    #[test]
    fn red_al_quitar_la_ventana_se_libera_todo() {
        let (real, mut p, mut lan) = con_lan(true);
        let socks: Vec<SockId> = (0..3).map(|_| p.net.tcp_connect(SRV, 80).unwrap()).collect();
        let q = p.net.resolve("ejemplo.com").unwrap();
        let ping = p.net.ping(SRV, 8, 64).unwrap();
        corre(&real, &mut lan, 200);
        // Una consulta y un ping ya hechos que nadie recogió, y otro ping aún en marcha.
        p.net.ping([10, 9, 9, 9], 8, 64).unwrap();
        p.own.borrow_mut().release(&mut *real.borrow_mut());
        {
            let mut r = real.borrow_mut();
            let n = r.net.as_mut().unwrap();
            for sk in &socks {
                assert_eq!(n.tcp_state(*sk), TcpState::Failed(NetError::Gone));
            }
            assert_eq!(n.resolve_poll(q), Some(Err(NetError::Gone)));
            assert_eq!(n.ping_poll(ping), Some(Err(NetError::Gone)));
        }
        // El programa ya no tiene nada suyo y los huecos vuelven.
        assert_eq!(p.net.tcp_state(socks[0]), TcpState::Failed(NetError::Gone));
        assert!(p.own.borrow().socks.is_empty() && p.own.borrow().pings.is_empty() && p.own.borrow().queries.is_empty());
        for _ in 0..MAX_SOCKS {
            assert!(p.net.tcp_connect(SRV, 80).is_ok());
        }
        assert_eq!(p.net.tcp_connect(SRV, 80), Err(NetError::Busy));
    }

    #[test]
    fn red_mensajes_rotos_y_tipos() {
        let (real, _p, mut lan) = con_lan(true);
        // Un `max` descomunal no reserva memoria de más aunque la conexión sea suya.
        let mut b = otro(&real, true);
        let sk = b.tcp_connect(SRV, 80).unwrap();
        corre(&real, &mut lan, 100);
        let mut buf = vec![0u8; 3 * NET_CHUNK];
        assert!(b.tcp_recv(sk, &mut buf).is_ok());
        assert_eq!(b.tcp_send(sk, &vec![0u8; 3 * NET_CHUNK]).map(|n| n <= NET_CHUNK), Ok(true));
        let calls = [
            Call::NetStatus,
            Call::NetRenew,
            Call::NetPing { dst: SRV, bytes: 1, ttl: 2 },
            Call::NetPingPoll { id: PingId(1) },
            Call::NetResolve { name: s("ejemplo.com") },
            Call::NetResolvePoll { id: QueryId(1) },
            Call::NetConnect { dst: SRV, port: 80 },
            Call::NetState { id: SockId(1) },
            Call::NetSend { id: SockId(1), data: Blob(vec![1, 2]) },
            Call::NetRecv { id: SockId(1), max: u32::MAX },
            Call::NetClose { id: SockId(1) },
            Call::NetAbort { id: SockId(1) },
        ];
        for c in &calls {
            let bytes = encode(c);
            let mut own = NetOwn::default();
            // Ningún prefijo ni un byte de más se entiende; solo el mensaje entero.
            for n in 1..bytes.len() {
                assert_eq!(serve(&mut *real.borrow_mut(), &bytes[..n], true, &mut own), [1], "{n} de {}", bytes.len());
            }
            let mut largo = bytes.clone();
            largo.push(0);
            assert_eq!(serve(&mut *real.borrow_mut(), &largo, true, &mut own), [1]);
            assert_eq!(serve(&mut *real.borrow_mut(), &bytes, true, &mut own)[0], 0);
        }
        // Tipos.
        let st = NetStatus { dns: vec![[1, 1, 1, 1], [8, 8, 8, 8]], ip: Some([10, 0, 0, 2]), adapter: s("Ñ"), mac: [1, 2, 3, 4, 5, 6], ..no_status() };
        assert_eq!(decode::<NetStatus>(&encode(&st)), Ok(st));
        for e in [NetError::NoAdapter, NetError::Unreachable([1, 2, 3, 4]), NetError::TtlExpired([5, 6, 7, 8]), NetError::Busy, NetError::HttpsNotYet, NetError::Protocol("sin servidor DNS")] {
            assert_eq!(decode::<NetError>(&encode(&e)), Ok(e));
        }
        assert_eq!(decode::<NetError>(&encode(&NetError::Protocol("texto que no existe"))), Ok(NetError::Protocol(PROTOCOL_UNKNOWN)));
        assert_eq!(decode::<NetError>(&[13, 200]), Ok(NetError::Protocol(PROTOCOL_UNKNOWN)));
        assert!(decode::<NetError>(&[99]).is_err() && decode::<TcpState>(&[9]).is_err() && decode::<State>(&[9]).is_err());
        let t = TcpState::Failed(NetError::Reset);
        assert_eq!(decode::<TcpState>(&encode(&t)), Ok(t));
        let r = PingReply { from: SRV, bytes: 32, rtt_ms: 7, ttl: 60 };
        assert_eq!(decode::<PingReply>(&encode(&r)), Ok(r));
        // Respuestas rotas al otro lado: no hay tarjeta.
        struct Roto;
        impl Transport for Roto {
            fn call(&mut self, _: &[u8]) -> Vec<u8> {
                vec![0, 9]
            }
        }
        let mut n = RemoteNet(Roto);
        assert_eq!(n.status().state, State::NoAdapter);
        assert_eq!(n.tcp_connect(SRV, 80), Err(NetError::NoAdapter));
        assert_eq!(n.tcp_recv(SockId(1), &mut [0u8; 4]), Err(NetError::NoAdapter));
        assert_eq!(n.tcp_state(SockId(1)), TcpState::Failed(NetError::NoAdapter));
    }
}
