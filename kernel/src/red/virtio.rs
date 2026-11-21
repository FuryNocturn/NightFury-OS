// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! virtio-net por PCI (dispositivo moderno, virtio 1.2 de OASIS; el transitorio 1af4:1000 también
//! trae las capacidades modernas). Colas «split» por sondeo: la 0 recibe y la 1 envía. Solo se
//! negocian VERSION_1, MAC y STATUS (sin offloads).
use super::{rd, wr};
use crate::serial::traza;
use crate::usb::{esperar, Dma};
use crate::{paging, pci};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{fence, Ordering::SeqCst};
use nf_core::net::{Link, NetDev};

/// Descriptores por cola como mucho (el dispositivo puede ofrecer menos).
const MAX_COLA: u16 = 128;
/// Búfer por descriptor: cabecera virtio (12 bytes, con VERSION_1) y la trama.
const BUF: usize = 2048;
const HDR: usize = 12;
const PLAZO_MS: u64 = 1_000;

// Campos de la configuración común (virtio 1.2, 4.1.4.3)
const SEL_DISP: usize = 0;
const DISP: usize = 4;
const SEL_DRV: usize = 8;
const DRV: usize = 12;
const NUM_COLAS: usize = 18;
const ESTADO: usize = 20;
const COLA_SEL: usize = 22;
const COLA_TAM: usize = 24;
const COLA_MSIX: usize = 26;
const COLA_ON: usize = 28;
const COLA_NOTIF: usize = 30;
const COLA_DESC: usize = 32;
const COLA_AVAIL: usize = 40;
const COLA_USED: usize = 48;

const ST_ACK: u8 = 1;
const ST_DRIVER: u8 = 2;
const ST_DRIVER_OK: u8 = 4;
const ST_FEATURES_OK: u8 = 8;

const F_MAC: u32 = 1 << 5;
const F_STATUS: u32 = 1 << 16;
/// Bit 32 = bit 0 de la palabra alta.
const F_VERSION_1_ALTA: u32 = 1;

pub fn soporta(vendor: u16, dev: u16) -> bool {
    vendor == 0x1AF4 && matches!(dev, 0x1000 | 0x1041)
}

fn wr64(a: usize, v: u64) {
    wr::<u32>(a, v as u32);
    wr::<u32>(a + 4, (v >> 32) as u32);
}

/// Cola «split»: tabla de descriptores, anillo disponible y anillo usado.
struct Cola {
    n: u16,
    desc: Dma,
    avail: Dma,
    used: Dma,
    avail_idx: u16,
    visto: u16,
    /// Dirección del timbre de esta cola.
    timbre: usize,
    indice: u16,
}

impl Cola {
    /// Configura la cola `indice` en el dispositivo (sin habilitarla aún).
    fn nueva(cfg: usize, notif: usize, mult: usize, indice: u16) -> Option<Cola> {
        wr::<u16>(cfg + COLA_SEL, indice);
        let max = rd::<u16>(cfg + COLA_TAM);
        let n = max.min(MAX_COLA);
        if n < 2 || n & (n - 1) != 0 {
            return None;
        }
        let (desc, avail, used) = (Dma::nuevo(16 * n as usize, 4096)?, Dma::nuevo(6 + 2 * n as usize, 4096)?, Dma::nuevo(6 + 8 * n as usize, 4096)?);
        wr::<u16>(cfg + COLA_TAM, n);
        wr::<u16>(cfg + COLA_MSIX, 0xFFFF);
        wr64(cfg + COLA_DESC, desc.dir());
        wr64(cfg + COLA_AVAIL, avail.dir());
        wr64(cfg + COLA_USED, used.dir());
        let off = rd::<u16>(cfg + COLA_NOTIF) as usize;
        wr::<u16>(cfg + COLA_ON, 1);
        Some(Cola { n, desc, avail, used, avail_idx: 0, visto: 0, timbre: notif.checked_add(off.checked_mul(mult)?)?, indice })
    }

    fn describir(&self, id: u16, dir: u64, len: u32, flags: u16) {
        let d = self.desc.dir() as usize + 16 * id as usize;
        wr64(d, dir);
        wr::<u32>(d + 8, len);
        wr::<u16>(d + 12, flags);
        wr::<u16>(d + 14, 0);
    }

    /// Entrega el descriptor `id` al dispositivo y toca el timbre.
    fn publicar(&mut self, id: u16) {
        let a = self.avail.dir() as usize;
        wr::<u16>(a + 4 + 2 * (self.avail_idx % self.n) as usize, id);
        fence(SeqCst);
        self.avail_idx = self.avail_idx.wrapping_add(1);
        wr::<u16>(a + 2, self.avail_idx);
        fence(SeqCst);
        wr::<u16>(self.timbre, self.indice);
    }

    /// Siguiente descriptor devuelto por el dispositivo: (id, longitud escrita). `None` si no hay o
    /// si el id no es de esta cola (entonces se ignora la entrada).
    fn usado(&mut self) -> Option<(u16, u32)> {
        let u = self.used.dir() as usize;
        loop {
            if rd::<u16>(u + 2) == self.visto {
                return None;
            }
            fence(SeqCst);
            let e = u + 4 + 8 * (self.visto % self.n) as usize;
            let (id, len) = (rd::<u32>(e), rd::<u32>(e + 4));
            self.visto = self.visto.wrapping_add(1);
            if id < self.n as u32 {
                return Some((id as u16, len));
            }
            traza!("NF virtio: id usado {} fuera de la cola", id);
        }
    }
}

pub struct Virtio {
    cfg: usize,
    /// Configuración específica de net (MAC en 0..6, estado en 6).
    dev: usize,
    con_estado: bool,
    mac: [u8; 6],
    rx: Cola,
    tx: Cola,
    rxbuf: Dma,
    txbuf: Dma,
    /// Descriptores de envío libres.
    libres: Vec<u16>,
}

impl Drop for Virtio {
    /// El reinicio detiene el DMA antes de que se liberen las colas y los búferes.
    fn drop(&mut self) {
        wr::<u8>(self.cfg + ESTADO, 0);
        esperar(PLAZO_MS, || rd::<u8>(self.cfg + ESTADO) == 0);
    }
}

/// Región de una capacidad virtio ya mapeada; `mult` solo vale en la de notificación.
struct Region {
    dir: usize,
    mult: usize,
}

pub fn iniciar(bus: u8, dev: u8, fun: u8) -> Option<Box<dyn NetDev>> {
    // Capacidades de virtio: tipo 1 común, 2 notificación, 4 específica del dispositivo.
    let (mut comun, mut notif, mut espec) = (None, None, None);
    for p in pci::capacidades(bus, dev, fun) {
        let c0 = pci::leer(bus, dev, fun, p);
        if c0 & 0xFF != 9 {
            continue;
        }
        let c = [c0, pci::leer(bus, dev, fun, p + 4), pci::leer(bus, dev, fun, p + 8), pci::leer(bus, dev, fun, p + 12), if c0 >> 16 & 0xFF >= 20 { pci::leer(bus, dev, fun, p + 16) } else { 0 }];
        let (tipo, bar, off, len) = ((c[0] >> 24) as u8, (c[1] & 0xFF) as u8, c[2] as u64, c[3] as u64);
        let min = match tipo {
            1 => 56,
            2 => 2,
            4 => 8,
            _ => continue,
        };
        if bar > 5 || len < min || off + len > 1 << 32 {
            continue;
        }
        let Some(base) = pci::bar(bus, dev, fun, bar) else { continue };
        if !paging::mapear_mmio(base + off, len) {
            continue;
        }
        let r = Region { dir: (base + off) as usize, mult: c[4] as usize };
        match tipo {
            1 if comun.is_none() => comun = Some(r),
            2 if notif.is_none() => notif = Some(r),
            4 if espec.is_none() => espec = Some(r),
            _ => {}
        }
    }
    let (comun, notif, espec) = (comun?, notif?, espec?);
    pci::activar(bus, dev, fun);
    let r = montar(&comun, &notif, &espec);
    if r.is_none() {
        wr::<u8>(comun.dir + ESTADO, 0);
    }
    r.map(|v| Box::new(v) as Box<dyn NetDev>)
}

fn montar(comun: &Region, notif: &Region, espec: &Region) -> Option<Virtio> {
    let cfg = comun.dir;
    wr::<u8>(cfg + ESTADO, 0);
    if !esperar(PLAZO_MS, || rd::<u8>(cfg + ESTADO) == 0) {
        traza!("NF virtio: no se reinicia");
        return None;
    }
    wr::<u8>(cfg + ESTADO, ST_ACK | ST_DRIVER);
    wr::<u32>(cfg + SEL_DISP, 0);
    let baja = rd::<u32>(cfg + DISP);
    wr::<u32>(cfg + SEL_DISP, 1);
    if rd::<u32>(cfg + DISP) & F_VERSION_1_ALTA == 0 || baja & F_MAC == 0 {
        traza!("NF virtio: el dispositivo no ofrece VERSION_1 y MAC");
        return None;
    }
    let con_estado = baja & F_STATUS != 0;
    wr::<u32>(cfg + SEL_DRV, 0);
    wr::<u32>(cfg + DRV, F_MAC | if con_estado { F_STATUS } else { 0 });
    wr::<u32>(cfg + SEL_DRV, 1);
    wr::<u32>(cfg + DRV, F_VERSION_1_ALTA);
    wr::<u8>(cfg + ESTADO, ST_ACK | ST_DRIVER | ST_FEATURES_OK);
    if rd::<u8>(cfg + ESTADO) & ST_FEATURES_OK == 0 || rd::<u16>(cfg + NUM_COLAS) < 2 {
        traza!("NF virtio: características rechazadas");
        return None;
    }
    let rx = Cola::nueva(cfg, notif.dir, notif.mult, 0)?;
    let tx = Cola::nueva(cfg, notif.dir, notif.mult, 1)?;
    let (rxbuf, txbuf) = (Dma::nuevo(BUF * rx.n as usize, 4096)?, Dma::nuevo(BUF * tx.n as usize, 4096)?);
    let mut mac = [0u8; 6];
    for (i, m) in mac.iter_mut().enumerate() {
        *m = rd::<u8>(espec.dir + i);
    }
    let libres = (0..tx.n).rev().collect();
    let mut v = Virtio { cfg, dev: espec.dir, con_estado, mac, rx, tx, rxbuf, txbuf, libres };
    wr::<u8>(cfg + ESTADO, ST_ACK | ST_DRIVER | ST_FEATURES_OK | ST_DRIVER_OK);
    for id in 0..v.rx.n {
        v.rx.describir(id, v.rxbuf.dir() + (BUF * id as usize) as u64, BUF as u32, 2);
        v.rx.publicar(id);
    }
    Some(v)
}

impl NetDev for Virtio {
    fn name(&self) -> String {
        String::from("virtio-net")
    }

    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn link(&mut self) -> Option<Link> {
        // shortcut: virtio no dice la velocidad (salvo SPEED_DUPLEX, que no se negocia); se informa 1000 Mb/s dúplex completo, cuando haga falta el dato real.
        (!self.con_estado || rd::<u16>(self.dev + 6) & 1 != 0).then_some(Link { speed_mbps: 1000, full_duplex: true })
    }

    fn send(&mut self, frame: &[u8]) -> bool {
        if !(14..=1514).contains(&frame.len()) {
            return false;
        }
        while let Some((id, _)) = self.tx.usado() {
            self.libres.push(id);
        }
        let Some(id) = self.libres.pop() else { return false };
        let b = self.txbuf.dir() as usize + BUF * id as usize;
        // Cabecera a cero (sin offloads), trama y relleno hasta 60 bytes; todo cabe en BUF.
        let largo = frame.len().max(60);
        unsafe {
            core::ptr::write_bytes(b as *mut u8, 0, HDR + largo);
            core::ptr::copy_nonoverlapping(frame.as_ptr(), (b + HDR) as *mut u8, frame.len());
        }
        self.tx.describir(id, b as u64, (HDR + largo) as u32, 0);
        self.tx.publicar(id);
        true
    }

    fn recv(&mut self, buf: &mut [u8]) -> Option<usize> {
        while let Some((id, len)) = self.rx.usado() {
            let len = len as usize;
            let ok = (HDR + 14..=BUF).contains(&len) && len - HDR <= buf.len();
            if ok {
                let b = self.rxbuf.dir() as usize + BUF * id as usize + HDR;
                // Sobre un búfer propio de BUF bytes; `len` validada arriba.
                unsafe { core::ptr::copy_nonoverlapping(b as *const u8, buf.as_mut_ptr(), len - HDR) };
            }
            self.rx.publicar(id);
            if ok {
                return Some(len - HDR);
            }
        }
        None
    }
}
