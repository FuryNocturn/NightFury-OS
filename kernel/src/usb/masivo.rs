// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Almacenamiento masivo USB: Bulk-Only Transport 1.0 (CBW/CSW) con órdenes SCSI (SPC/SBC).
//! Un `Bot` por interfaz y un `Disco` (`SectorDev`) por LUN. El dispositivo es entrada hostil: todo lo
//! que devuelve se valida (firma y tag del CSW, residuo, tamaños) y ante la duda se recupera o se deja
//! el disco inutilizable (`FsError::Disk`), nunca se confía en él.
//!
//! Los `Disco` no guardan punteros: solo un identificador único (nunca se reutiliza), que se busca en el
//! registro `BOTS` en cada operación. Al quitar el dispositivo se borra del registro y desde entonces
//! todo da `Disk`. Un solo núcleo y solo desde el bucle principal (ver `usb/mod.rs`).
use super::xhci::{Error, Xhci};
use super::{espera_ms, CambioDisco, Dma, Plazo};
use crate::serial::traza;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
use nf_core::fs::FsError;
use nf_core::vol::SectorDev;

/// Bytes por orden de lectura o escritura (un solo TRB: el búfer va alineado a 64 KiB).
const TAM_TRANSF: usize = 64 * 1024;
/// Dentro de `Bot::cmd`: el CBW en 0, el CSW en 64 y la respuesta de REQUEST SENSE en 128.
const CSW_OFF: u64 = 64;
const SENSE_OFF: u64 = 128;

struct G<T>(UnsafeCell<T>);
// Un solo núcleo; solo el bucle principal (sondeo USB y operaciones de disco) lo toca.
unsafe impl<T> Sync for G<T> {}
static BOTS: G<Vec<Bot>> = G(UnsafeCell::new(Vec::new()));
static CAMBIOS: G<Vec<CambioDisco>> = G(UnsafeCell::new(Vec::new()));
static SIG_ID: AtomicU32 = AtomicU32::new(1);

struct Lun {
    id: u32,
    lun: u8,
    ss: usize,
    nsec: u64,
    ro: bool,
}

struct Bot {
    ctrl: usize,
    slot: u8,
    iface: u8,
    /// (dci, dirección del endpoint) bulk IN y OUT.
    ent: (u8, u8),
    sal: (u8, u8),
    tag: u32,
    datos: Dma,
    cmd: Dma,
    /// La recuperación falló: no se vuelve a hablar con el dispositivo.
    roto: bool,
    luns: Vec<Lun>,
}

#[derive(Clone, Copy, PartialEq)]
enum Dir {
    Nada,
    In,
    Out,
}

/// Resultado de una transferencia bulk.
enum Fallo {
    Ausente,
    Stall,
    Otro,
}

/// Resultado de una orden BOT completa.
enum E {
    Ausente,
    /// Falló sin remedio (ya se intentó la recuperación).
    Fallo,
    /// CSW con estado 1: el dispositivo informa de un error (hay que pedir los datos de sentido).
    Check,
}

#[derive(Clone, Copy)]
struct Sense {
    clave: u8,
    asc: u8,
}

enum Sc {
    Ausente,
    Fallo,
    Sense(Sense),
}

// ---------- Interfaz con `usb` ----------

pub fn tomar_cambios() -> Vec<CambioDisco> {
    core::mem::take(unsafe { &mut *CAMBIOS.0.get() })
}

pub fn abrir(id: u32) -> Option<Box<dyn SectorDev>> {
    let l = unsafe { &*BOTS.0.get() }.iter().flat_map(|b| b.luns.iter()).find(|l| l.id == id)?;
    Some(Box::new(Disco { id, ss: l.ss, nsec: l.nsec, ro: l.ro }))
}

/// Un dispositivo se ha quitado (se llama desde `Xhci::quitar`, con la ranura ya deshabilitada).
/// Con `fuga`, el controlador no confirmó el Disable Slot y aún podría escribir en sus búferes: no se liberan.
pub fn quitado(ctrl: usize, slot: u8, fuga: bool) {
    let bots = unsafe { &mut *BOTS.0.get() };
    let cola = unsafe { &mut *CAMBIOS.0.get() };
    while let Some(i) = bots.iter().position(|b| b.ctrl == ctrl && b.slot == slot) {
        let b = bots.remove(i);
        for l in &b.luns {
            cola.push(CambioDisco::Quitado { id: l.id });
        }
        if fuga {
            core::mem::forget(b);
        }
    }
}

/// Una interfaz BOT recién configurada: Get Max LUN, y por cada LUN las órdenes de arranque del disco.
pub fn alta(x: &mut Xhci, slot: u8, iface: u8, ent: (u8, u8), sal: (u8, u8)) {
    let (Ok(datos), Ok(cmd)) = (x.mem(TAM_TRANSF, TAM_TRANSF), x.mem(4096, 4096)) else {
        traza!("NF usb: ranura {}: sin memoria para el almacenamiento", slot);
        return;
    };
    let mut b = Bot { ctrl: x.id, slot, iface, ent, sal, tag: 0, datos, cmd, roto: false, luns: Vec::new() };
    // Get Max LUN: un stall (o cualquier error) significa un solo LUN. Más de 15 no es válido.
    let mut m = [0u8; 1];
    let max = match x.control(slot, 0xA1, 0xFE, 0, iface as u16, &mut m) {
        Ok(1) if m[0] <= 15 => m[0],
        _ => 0,
    };
    // Un plazo para todos los LUN: un lector de 16 ranuras que nunca está listo no para el sistema 16 × 5 s.
    let plazo = Plazo::ms(8000);
    for lun in 0..=max {
        if b.roto || x.dev(slot).is_none() || plazo.vencido() {
            break;
        }
        match iniciar(x, &mut b, lun, &plazo) {
            Ok((nombre, ss, nsec, ro)) => {
                let id = SIG_ID.fetch_add(1, Relaxed);
                traza!("NF usb: ranura {} LUN {}: «{}» {} sectores de {} bytes ({} MiB){}", slot, lun, nombre, nsec, ss, (nsec * ss as u64) >> 20, if ro { ", solo lectura" } else { "" });
                b.luns.push(Lun { id, lun, ss, nsec, ro });
                unsafe { &mut *CAMBIOS.0.get() }.push(CambioDisco::Conectado { id, nombre, dev: Box::new(Disco { id, ss, nsec, ro }) });
            }
            Err(motivo) => traza!("NF usb: ranura {} LUN {}: se ignora ({})", slot, lun, motivo),
        }
    }
    if !b.luns.is_empty() {
        unsafe { &mut *BOTS.0.get() }.push(b);
    }
}

// ---------- Transporte BOT ----------

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn datos(b: &Bot, n: usize) -> &[u8] {
    unsafe { core::slice::from_raw_parts(b.datos.ptr::<u8>(), n.min(TAM_TRANSF)) }
}

/// Una transferencia bulk bloqueante. Con plazo agotado aborta el endpoint para poder reutilizarlo.
fn xfer(x: &mut Xhci, slot: u8, dci: u8, entrada: bool, addr: u64, len: usize, ms: u64) -> Result<usize, Fallo> {
    x.normal(slot, dci, addr, len, entrada).map_err(|_| Fallo::Ausente)?;
    match x.esperar(slot, dci, ms) {
        Ok(n) => Ok(n),
        Err(Error::Codigo(6)) => Err(Fallo::Stall), // STALL: `esperar` ya ha reiniciado el endpoint en el host
        Err(Error::Tiempo) => {
            x.abortar(slot, dci);
            Err(Fallo::Otro)
        }
        Err(Error::Ausente) => Err(Fallo::Ausente),
        Err(_) => Err(Fallo::Otro),
    }
}

/// Clear Feature ENDPOINT_HALT en el dispositivo.
fn quitar_halt(x: &mut Xhci, b: &Bot, addr: u8) {
    let _ = x.control(b.slot, 0x02, 1, 0, addr as u16, &mut []);
}

/// Recuperación del BOT (1.0, 5.3.4): Bulk-Only Mass Storage Reset y Clear HALT de ambos endpoints.
// shortcut: tras un reset sin halt en el host el bit de datos de los endpoints bulk puede quedar desfasado
// (hardware real, USB 2), cuando se pruebe con pendrives reales hay que reconfigurar el endpoint.
fn recuperacion(x: &mut Xhci, b: &Bot) -> bool {
    traza!("NF usb: ranura {}: recuperación BOT", b.slot);
    let ok = x.control(b.slot, 0x21, 0xFF, 0, b.iface as u16, &mut []).is_ok();
    quitar_halt(x, b, b.ent.1);
    quitar_halt(x, b, b.sal.1);
    ok
}

fn fallo(x: &mut Xhci, b: &mut Bot) -> E {
    if x.dev(b.slot).is_none() {
        return E::Ausente;
    }
    if !recuperacion(x, b) {
        b.roto = true;
        traza!("NF usb: ranura {}: el dispositivo no se recupera, se abandona", b.slot);
    }
    E::Fallo
}

/// CBW, fase de datos opcional (en `addr`) y CSW. `Ok(bytes de datos útiles)`.
fn bot(x: &mut Xhci, b: &mut Bot, lun: u8, cdb: &[u8], dir: Dir, len: usize, addr: u64, ms: u64) -> Result<usize, E> {
    if b.roto {
        return Err(E::Fallo);
    }
    if x.dev(b.slot).is_none() {
        return Err(E::Ausente);
    }
    debug_assert!(cdb.len() <= 16 && len <= TAM_TRANSF && (dir == Dir::Nada) == (len == 0));
    b.tag = b.tag.wrapping_add(1);
    let tag = b.tag;
    let mut cbw = [0u8; 31];
    cbw[..4].copy_from_slice(b"USBC");
    cbw[4..8].copy_from_slice(&tag.to_le_bytes());
    cbw[8..12].copy_from_slice(&(len as u32).to_le_bytes());
    cbw[12] = if dir == Dir::In { 0x80 } else { 0 };
    cbw[13] = lun;
    cbw[14] = cdb.len() as u8;
    cbw[15..15 + cdb.len()].copy_from_slice(cdb);
    let (cmd, slot) = (b.cmd.dir(), b.slot);
    unsafe {
        core::ptr::copy_nonoverlapping(cbw.as_ptr(), cmd as *mut u8, 31);
        core::ptr::write_bytes((cmd + CSW_OFF) as *mut u8, 0, 13);
    }
    match xfer(x, slot, b.sal.0, false, cmd, 31, 2000) {
        Ok(31) => {}
        Err(Fallo::Ausente) => return Err(E::Ausente),
        _ => return Err(fallo(x, b)),
    }
    let mut movido = None;
    if len > 0 {
        let (dci, ad) = if dir == Dir::In { b.ent } else { b.sal };
        match xfer(x, slot, dci, dir == Dir::In, addr, len, ms) {
            Ok(n) => movido = Some(n),
            // Stall en la fase de datos: se limpia el halt y se sigue con el CSW (BOT 6.7.2).
            Err(Fallo::Stall) => quitar_halt(x, b, ad),
            Err(Fallo::Ausente) => return Err(E::Ausente),
            Err(Fallo::Otro) => return Err(fallo(x, b)),
        }
    }
    // El CSW se lee con un reintento si el endpoint IN dio stall.
    let mut leido = None;
    for _ in 0..2 {
        match xfer(x, slot, b.ent.0, true, cmd + CSW_OFF, 13, 2000) {
            Ok(n) => {
                leido = Some(n);
                break;
            }
            Err(Fallo::Stall) => quitar_halt(x, b, b.ent.1),
            Err(Fallo::Ausente) => return Err(E::Ausente),
            Err(Fallo::Otro) => return Err(fallo(x, b)),
        }
    }
    let mut csw = [0u8; 13];
    unsafe { core::ptr::copy_nonoverlapping((cmd + CSW_OFF) as *const u8, csw.as_mut_ptr(), 13) };
    let residuo = u32::from_le_bytes([csw[8], csw[9], csw[10], csw[11]]) as usize;
    // CSW válido: 13 bytes, firma, tag igual, estado 0..=2 y residuo no mayor que lo pedido.
    if leido != Some(13) || &csw[..4] != b"USBS" || u32::from_le_bytes([csw[4], csw[5], csw[6], csw[7]]) != tag || csw[12] > 2 || residuo > len {
        return Err(fallo(x, b));
    }
    match csw[12] {
        0 => Ok(movido.unwrap_or(len).min(len - residuo)),
        1 => Err(E::Check),
        _ => Err(fallo(x, b)), // error de fase
    }
}

/// Datos de sentido tras un CHECK CONDITION (REQUEST SENSE, 18 bytes).
fn sentido(x: &mut Xhci, b: &mut Bot, lun: u8) -> Sense {
    let a = b.cmd.dir() + SENSE_OFF;
    let mut d = [0u8; 18];
    let n = bot(x, b, lun, &[0x03, 0, 0, 0, 18, 0], Dir::In, 18, a, 2000).unwrap_or(0).min(18);
    unsafe { core::ptr::copy_nonoverlapping(a as *const u8, d.as_mut_ptr(), n) };
    match d[0] & 0x7F {
        0x70 | 0x71 if n >= 14 => Sense { clave: d[2] & 0xF, asc: d[12] },
        0x72 | 0x73 if n >= 4 => Sense { clave: d[1] & 0xF, asc: d[2] },
        _ => Sense { clave: 0xFF, asc: 0 },
    }
}

/// Una orden SCSI con sentido: ante CHECK CONDITION pide el sentido (y repite una vez si es UNIT ATTENTION).
fn scsi(x: &mut Xhci, b: &mut Bot, lun: u8, cdb: &[u8], dir: Dir, len: usize, ms: u64) -> Result<usize, Sc> {
    for intento in 0..2 {
        let a = b.datos.dir();
        match bot(x, b, lun, cdb, dir, len, a, ms) {
            Ok(n) => return Ok(n),
            Err(E::Ausente) => return Err(Sc::Ausente),
            Err(E::Fallo) => return Err(Sc::Fallo),
            Err(E::Check) => {
                let s = sentido(x, b, lun);
                if s.clave == 6 && intento == 0 {
                    continue;
                }
                return Err(Sc::Sense(s));
            }
        }
    }
    Err(Sc::Fallo)
}

// ---------- Arranque de un LUN ----------

/// Texto del INQUIRY: solo ASCII imprimible, sin espacios de relleno.
fn texto(b: &[u8]) -> String {
    let s: String = b.iter().map(|&c| if (0x20..0x7F).contains(&c) { c as char } else { '?' }).collect();
    String::from(s.trim())
}

/// INQUIRY, TEST UNIT READY, READ CAPACITY y MODE SENSE. (nombre, tamaño de sector, sectores, solo lectura).
fn iniciar(x: &mut Xhci, b: &mut Bot, lun: u8, plazo: &Plazo) -> Result<(String, usize, u64, bool), &'static str> {
    let n = scsi(x, b, lun, &[0x12, 0, 0, 0, 36, 0], Dir::In, 36, 2000).map_err(|_| "INQUIRY falla")?;
    if n < 36 {
        return Err("respuesta de INQUIRY corta");
    }
    let mut inq = [0u8; 36];
    inq.copy_from_slice(datos(b, 36));
    let tipo = inq[0] & 0x1F;
    // Debe haber un dispositivo (calificador 0) de acceso directo (0), CD/DVD (5), óptico (7) o RBC (14).
    if inq[0] >> 5 != 0 || !matches!(tipo, 0 | 5 | 7 | 0xE) {
        return Err("no es un disco");
    }
    let optico = matches!(tipo, 5 | 7);
    let (v, p) = (texto(&inq[8..16]), texto(&inq[16..32]));
    let nombre = if v.is_empty() && p.is_empty() { String::from("USB") } else { alloc::format!("{} {}", v, p).trim().into() };
    // Mientras el medio «se prepara» (NOT READY) o tras un reinicio (UNIT ATTENTION) se reintenta
    // hasta el plazo del dispositivo.
    loop {
        match scsi(x, b, lun, &[0; 6], Dir::Nada, 0, 2000) {
            Ok(_) => break,
            Err(Sc::Sense(Sense { asc: 0x3A, .. })) => return Err("sin medio"),
            Err(Sc::Sense(Sense { clave: 2 | 6, .. })) => {}
            Err(_) => return Err("TEST UNIT READY falla"),
        }
        if plazo.vencido() {
            return Err("el medio no se prepara");
        }
        espera_ms(200);
    }
    let n = scsi(x, b, lun, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], Dir::In, 8, 2000).map_err(|_| "READ CAPACITY falla")?;
    if n != 8 {
        return Err("respuesta de READ CAPACITY corta");
    }
    let d = datos(b, 8);
    let (mut ultimo, mut bl) = (be32(&d[..4]) as u64, be32(&d[4..8]));
    if ultimo == 0xFFFF_FFFF {
        // Más de 2 TiB: READ CAPACITY(16) (SERVICE ACTION IN, acción 0x10).
        let mut c = [0u8; 16];
        c[0] = 0x9E;
        c[1] = 0x10;
        c[10..14].copy_from_slice(&32u32.to_be_bytes());
        let n = scsi(x, b, lun, &c, Dir::In, 32, 2000).map_err(|_| "READ CAPACITY(16) falla")?;
        if n < 12 {
            return Err("respuesta de READ CAPACITY(16) corta");
        }
        let d = datos(b, 12);
        ultimo = u64::from_be_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]);
        bl = be32(&d[8..12]);
    }
    if !(512..=4096).contains(&bl) || !bl.is_power_of_two() {
        return Err("tamaño de sector no soportado");
    }
    let nsec = ultimo.checked_add(1).filter(|&n| n <= u64::MAX / 4096).ok_or("capacidad absurda")?;
    // Protección contra escritura (bit 7 de los parámetros del dispositivo). Si falla, se asume escribible.
    let ro = optico
        || match scsi(x, b, lun, &[0x1A, 0, 0x3F, 0, 4, 0], Dir::In, 4, 2000) {
            Ok(n) if n >= 3 => datos(b, 3)[2] & 0x80 != 0,
            _ => false,
        };
    Ok((nombre, bl as usize, nsec, ro))
}

// ---------- El disco ----------

struct Disco {
    id: u32,
    ss: usize,
    nsec: u64,
    ro: bool,
}

/// Una orden READ/WRITE de `n` sectores desde `lba` por `b.datos`.
fn trozo(x: &mut Xhci, b: &mut Bot, lun: u8, lba: u64, n: usize, ss: usize, escribir: bool) -> Result<(), FsError> {
    let mut c = [0u8; 16];
    let l = if lba + n as u64 <= 1 << 32 && n <= 0xFFFF {
        c[0] = if escribir { 0x2A } else { 0x28 };
        c[2..6].copy_from_slice(&(lba as u32).to_be_bytes());
        c[7..9].copy_from_slice(&(n as u16).to_be_bytes());
        10
    } else {
        c[0] = if escribir { 0x8A } else { 0x88 };
        c[2..10].copy_from_slice(&lba.to_be_bytes());
        c[10..14].copy_from_slice(&(n as u32).to_be_bytes());
        16
    };
    let len = n * ss;
    match scsi(x, b, lun, &c[..l], if escribir { Dir::Out } else { Dir::In }, len, 10_000) {
        Ok(m) if m == len => Ok(()),
        Err(Sc::Sense(Sense { clave: 7, .. })) if escribir => Err(FsError::Denied),
        _ => Err(FsError::Disk),
    }
}

impl Disco {
    /// Busca el dispositivo por identificador; `Disk` si ya no está o está roto.
    fn con<R>(&self, f: impl FnOnce(&mut Xhci, &mut Bot, u8) -> Result<R, FsError>) -> Result<R, FsError> {
        let b = unsafe { &mut *BOTS.0.get() }.iter_mut().find(|b| b.luns.iter().any(|l| l.id == self.id)).ok_or(FsError::Disk)?;
        let lun = b.luns.iter().find(|l| l.id == self.id).map_or(0, |l| l.lun);
        let x = unsafe { &mut *super::USB.0.get() }.get_mut(b.ctrl).ok_or(FsError::Disk)?;
        if b.roto || x.dev(b.slot).is_none() {
            return Err(FsError::Disk);
        }
        f(x, b, lun)
    }

    fn comprobar(&self, lba: u64, len: usize) -> Result<usize, FsError> {
        let n = len / self.ss;
        if len % self.ss != 0 || lba.checked_add(n as u64).map_or(true, |f| f > self.nsec) {
            return Err(FsError::Disk);
        }
        Ok(n)
    }
}

impl SectorDev for Disco {
    fn sector_size(&self) -> usize {
        self.ss
    }
    fn sectors(&self) -> u64 {
        self.nsec
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        if self.comprobar(lba, buf.len())? == 0 {
            return Ok(());
        }
        let ss = self.ss;
        self.con(|x, b, lun| {
            let mut l = lba;
            for c in buf.chunks_mut(TAM_TRANSF / ss * ss) {
                trozo(x, b, lun, l, c.len() / ss, ss, false)?;
                c.copy_from_slice(datos(b, c.len()));
                l += (c.len() / ss) as u64;
            }
            Ok(())
        })
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError> {
        if self.ro {
            return Err(FsError::Denied);
        }
        if self.comprobar(lba, buf.len())? == 0 {
            return Ok(());
        }
        let ss = self.ss;
        self.con(|x, b, lun| {
            let mut l = lba;
            for c in buf.chunks(TAM_TRANSF / ss * ss) {
                unsafe { core::ptr::copy_nonoverlapping(c.as_ptr(), b.datos.ptr::<u8>(), c.len()) };
                trozo(x, b, lun, l, c.len() / ss, ss, true)?;
                l += (c.len() / ss) as u64;
            }
            Ok(())
        })
    }
    fn flush(&mut self) -> Result<(), FsError> {
        if self.ro {
            return Ok(());
        }
        self.con(|x, b, lun| match scsi(x, b, lun, &[0x35, 0, 0, 0, 0, 0, 0, 0, 0, 0], Dir::Nada, 0, 10_000) {
            Ok(_) => Ok(()),
            // ILLEGAL REQUEST: el dispositivo no tiene caché que vaciar.
            Err(Sc::Sense(Sense { clave: 5, .. })) => Ok(()),
            Err(_) => Err(FsError::Disk),
        })
    }
    fn read_only(&self) -> bool {
        self.ro
    }
}
