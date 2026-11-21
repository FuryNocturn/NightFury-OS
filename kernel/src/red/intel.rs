// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Tarjetas Intel de cable por sondeo, una cola de recepción y una de envío, sin interrupciones.
//! - Clásicas (82540EM, 82574L): descriptores legados, según el manual del 8254x y la hoja del 82574.
//! - 82576 e I210/I211: descriptores avanzados de un búfer, según las hojas del 82576 y del I210.
//! - I225/I226: descriptores avanzados, a partir de la especificación de sala limpia
//!   (`especificaciones\intel-ethernet`, familia B, con sus erratas).
//! - I217/I218/I219 (familia A, integrados en el PCH): recepción extendida y envío legado; su
//!   arranque, PHY y ajustes de enlace están en `intel/pch.rs`.
use super::{rd, wr};
use crate::serial::traza;
use crate::usb::{esperar, espera_ms, Dma};
use crate::{paging, pci};
use alloc::boxed::Box;
use alloc::string::String;
use core::cell::Cell;
use core::sync::atomic::{fence, Ordering::SeqCst};
use nf_core::net::{Link, NetDev};

/// Descriptores por anillo y tamaño de búfer.
const N: usize = 128;
const BUF: usize = 2048;

// Registros comunes
const CTRL: usize = 0x0;
const STATUS: usize = 0x8;
const EECD: usize = 0x10;
const CTRL_EXT: usize = 0x18;
const MDIC: usize = 0x20;
const RCTL: usize = 0x100;
const TCTL: usize = 0x400;
const TIPG: usize = 0x410;
const RFCTL: usize = 0x5008;
const MTA: usize = 0x5200;
const RAL0: usize = 0x5400;
const MANC: usize = 0x5820;
const UTA: usize = 0xA000;
const RXPBS: usize = 0x2404;
const TXPBS: usize = 0x3404;
const SWSM: usize = 0x5B50;
const SW_FW_SYNC: usize = 0x5B5C;

mod pch;

const CTRL_SLU: u32 = 1 << 6;
const CTRL_ASDE: u32 = 1 << 5;
const CTRL_FRCSPD_DPX: u32 = 1 << 11 | 1 << 12;
const CTRL_MASTER_OFF: u32 = 1 << 2;
const CTRL_RST: u32 = 1 << 26;
const CTRL_RFCE_TFCE_VME: u32 = 1 << 27 | 1 << 28 | 1 << 30;
const STATUS_MASTER_ON: u32 = 1 << 19;
const DRV_LOAD: u32 = 1 << 28;
const RCTL_EN_BAM_SECRC: u32 = 1 << 1 | 1 << 15 | 1 << 26;
/// EN | PSP | CT = 15 | COLD = 63 | RTLC (el valor típico 0x0103F0FA de la especificación).
const TCTL_VALOR: u32 = 1 << 1 | 1 << 3 | 15 << 4 | 63 << 12 | 1 << 24;
const TIPG_COBRE: u32 = 0x0060_2008;
const AV: u32 = 1 << 31;

#[derive(Clone, Copy, PartialEq)]
enum Fam {
    E8254x,
    E82574,
    Igb,
    I225,
    I226,
    /// I217/I218/I219; el número es la generación de la plataforma (0 LPT … 6 PTP).
    Pch(u8),
}

impl Fam {
    fn pch(self) -> bool {
        matches!(self, Fam::Pch(_))
    }
    /// Formato de reescritura de recepción con estado en +8 y longitud en +12.
    fn rx_ext(self) -> bool {
        self.avanzado() || self.pch()
    }
    fn avanzado(self) -> bool {
        matches!(self, Fam::Igb | Fam::I225 | Fam::I226)
    }
    fn b(self) -> bool {
        matches!(self, Fam::I225 | Fam::I226)
    }
}

const MODELOS: &[(u16, &str, Fam)] = &[
    (0x100E, "Intel 82540EM", Fam::E8254x),
    (0x10D3, "Intel 82574L", Fam::E82574),
    (0x10C9, "Intel 82576", Fam::Igb),
    (0x1533, "Intel I210", Fam::Igb),
    (0x157B, "Intel I210", Fam::Igb),
    (0x1539, "Intel I211", Fam::Igb),
    (0x15F3, "Intel I225-V", Fam::I225),
    (0x15F2, "Intel I225-LM", Fam::I225),
    (0x15F8, "Intel I225-I", Fam::I225),
    (0x15F7, "Intel I220-V", Fam::I225),
    (0x3100, "Intel I225-K", Fam::I225),
    (0x3101, "Intel I225-K2", Fam::I225),
    (0x5502, "Intel I225-LMVP", Fam::I225),
    (0x0D9F, "Intel I225-IT", Fam::I225),
    (0x15FD, "Intel I225", Fam::I225),
    (0x125E, "Intel I221-V", Fam::I225),
    (0x125B, "Intel I226-LM", Fam::I226),
    (0x125C, "Intel I226-V", Fam::I226),
    (0x3102, "Intel I226-K", Fam::I226),
    (0x5503, "Intel I226-LMVP", Fam::I226),
    (0x125D, "Intel I226-IT", Fam::I226),
    (0x125F, "Intel I226", Fam::I226),
];

fn modelo(dev: u16) -> Option<(&'static str, Fam)> {
    MODELOS.iter().find(|m| m.0 == dev).map(|m| (m.1, m.2)).or_else(|| pch::modelo(dev).map(|(n, g)| (n, Fam::Pch(g))))
}

pub fn soporta(vendor: u16, dev: u16) -> bool {
    vendor == 0x8086 && modelo(dev).is_some()
}

fn wr64(a: usize, v: u64) {
    wr::<u32>(a, v as u32);
    wr::<u32>(a + 4, (v >> 32) as u32);
}

/// Un anillo de 16 bytes por descriptor con un búfer de `BUF` bytes cada uno.
struct Anillo {
    desc: Dma,
    bufs: Dma,
}

impl Anillo {
    fn nuevo() -> Option<Anillo> {
        Some(Anillo { desc: Dma::nuevo(16 * N, 4096)?, bufs: Dma::nuevo(BUF * N, 4096)? })
    }
    fn d(&self, i: usize) -> usize {
        self.desc.dir() as usize + 16 * i
    }
    fn buf(&self, i: usize) -> usize {
        self.bufs.dir() as usize + BUF * i
    }
}

pub struct Intel {
    r: usize,
    nombre: &'static str,
    fam: Fam,
    /// Revisión PCI (el I225 anterior a la 2 necesita otro IPGT a 2,5 Gb/s).
    rev: u8,
    mac: [u8; 6],
    /// Base de los registros de cola 0: RDBAL en `rq`, TDBAL en `tq`.
    rq: usize,
    tq: usize,
    rx: Anillo,
    tx: Anillo,
    /// Siguiente descriptor de RX por mirar; si se está saltando una trama partida en varios.
    rx_sig: usize,
    saltando: bool,
    tx_sig: usize,
    tx_limpio: usize,
    ipgt: u32,
    /// Id de dispositivo y posición PCI (la familia A los necesita para sus erratas).
    dev: u16,
    bdf: (u8, u8, u8),
    /// Familia A: reintentos de los accesos a la PHY (se apagan donde un error es esperado).
    reint: Cell<bool>,
    /// Familia A: enlace por el que ya se hicieron los ajustes de F9.
    ultimo: Option<(u32, bool)>,
}

impl Intel {
    fn rd(&self, o: usize) -> u32 {
        rd(self.r + o)
    }
    fn wr(&self, o: usize, v: u32) {
        wr(self.r + o, v)
    }

    /// Detiene el DMA y devuelve el control al firmware (F16 de la especificación).
    fn parar(&self) {
        self.wr(RCTL, 0);
        self.wr(TCTL, 1 << 3);
        espera_ms(10);
        self.wr(CTRL, self.rd(CTRL) | CTRL_MASTER_OFF);
        if !esperar(100, || self.rd(STATUS) & STATUS_MASTER_ON == 0) {
            traza!("NF intel: el maestro PCIe no se detiene");
        }
        if self.fam != Fam::E8254x {
            self.wr(CTRL_EXT, self.rd(CTRL_EXT) & !DRV_LOAD);
        }
    }

    // ---------- Familia B: semáforos de la PHY y MDIC (F3, F7) ----------

    /// Lectura con adquisición de SWSM.SMBI (se lee 0 = es nuestro) y SWESMBI. Con un borrado
    /// forzado y un reintento si el firmware se quedó con él.
    fn swsm_tomar(&self) -> bool {
        for _ in 0..2 {
            if esperar(40, || self.rd(SWSM) & 1 == 0) {
                self.wr(SWSM, self.rd(SWSM) | 2);
                if self.rd(SWSM) & 2 != 0 {
                    return true;
                }
            }
            self.wr(SWSM, self.rd(SWSM) & !3);
        }
        false
    }
    fn swsm_soltar(&self) {
        self.wr(SWSM, self.rd(SWSM) & !3);
    }

    /// Toma la PHY0 (bit de software 1 de SW_FW_SYNC, el del firmware 17), con plazo.
    fn phy_tomar(&self) -> bool {
        for _ in 0..200 {
            if !self.swsm_tomar() {
                return false;
            }
            let s = self.rd(SW_FW_SYNC);
            let libre = s & (2 | 2 << 16) == 0;
            if libre {
                self.wr(SW_FW_SYNC, s | 2);
            }
            self.swsm_soltar();
            if libre {
                return true;
            }
            espera_ms(5);
        }
        false
    }
    fn phy_soltar(&self) {
        if self.swsm_tomar() {
            self.wr(SW_FW_SYNC, self.rd(SW_FW_SYNC) & !2);
            self.swsm_soltar();
        }
    }

    /// Dirección MDIO de la PHY interna: 0 en el I225/I226, 1 en el 82576 y el I210.
    fn phy_dir(&self) -> u32 {
        !self.fam.b() as u32
    }

    /// Un acceso MDIC a la PHY con su semáforo (que se suelta siempre): escribe `escribir` o, si es `None`, lee.
    fn mdic(&self, reg: u32, escribir: Option<u16>) -> Option<u16> {
        if !self.phy_tomar() {
            return None;
        }
        let op = match escribir {
            Some(d) => 1 << 26 | d as u32,
            None => 1 << 27,
        };
        self.wr(MDIC, op | reg << 16 | self.phy_dir() << 21);
        let mut v = 0;
        let listo = esperar(100, || {
            v = self.rd(MDIC);
            v & 1 << 28 != 0
        });
        self.phy_soltar();
        (listo && v & 1 << 30 == 0).then_some(v as u16)
    }
    fn phy_rmw(&self, reg: u32, poner: u16, quitar: u16) -> Option<()> {
        let v = self.mdic(reg, None)?;
        self.mdic(reg, Some(v & !quitar | poner)).map(|_| ())
    }
    /// Registro `a` del dispositivo MMD 7 (anuncio de 2,5 Gb/s): accesos 13/14 de la cláusula 45.
    fn mmd7_rmw(&self, a: u16, poner: u16) -> Option<()> {
        self.mdic(13, Some(7))?;
        self.mdic(14, Some(a))?;
        self.mdic(13, Some(0x4007))?;
        let v = self.mdic(14, None)?;
        self.mdic(14, Some(v | poner))?;
        self.mdic(13, Some(0)).map(|_| ())
    }

    /// F8: anuncia todas las velocidades y reinicia la autonegociación. Todo con lectura y
    /// modificación para no tocar lo que la NVM dejó puesto; si algo falla, se deja como esté.
    fn negociar(&self) {
        if self.rd(MANC) & 1 << 18 != 0 {
            traza!("NF intel: PHY bloqueada por el firmware, se deja como esta");
            return;
        }
        let id = (self.mdic(2, None), self.mdic(3, None));
        if id.0 != Some(0x67C9) || id.1.map(|v| v & 0xFFF0) != Some(0xDC00) {
            traza!("NF intel: PHY desconocida {:04x?}, no se toca", id);
            return;
        }
        // reg 4: 10/100 (0x01E0) sin pausa; reg 9: 1000 FD; MMD 7 0x20: 2,5 Gb/s FD; reg 0: AN activa y reiniciada.
        let ok = self.phy_rmw(4, 0x01E0, 0x0C00).and_then(|_| self.phy_rmw(9, 0x0200, 0)).and_then(|_| self.mmd7_rmw(0x20, 0x80)).and_then(|_| self.phy_rmw(0, 0x1200, 0));
        if ok.is_none() {
            traza!("NF intel: no se pudo reiniciar la autonegociacion");
        }
    }

    /// E1: borra L1.2 del control de PM de L1 (capacidad extendida 0x1E, registro +8): en el I225
    /// ASPM L1.2 (bit 2) y PCI-PM L1.2 (bit 0); en el I226 solo ASPM L1.2.
    fn sin_l12(&self, bus: u8, dev: u8, fun: u8) {
        let quitar = if self.fam == Fam::I225 { 0b101 } else { 0b100 };
        let hecho = pci::cap_ext(bus, dev, fun, 0x1E).and_then(|c| {
            let v = pci::leer_ext(bus, dev, fun, c + 8)?;
            pci::escribir_ext(bus, dev, fun, c + 8, v & !quitar).then_some(v & quitar)
        });
        match hecho {
            Some(antes) => traza!("NF intel: L1.2 desactivado (antes {:#x})", antes),
            None => traza!("NF intel: no se pudo desactivar L1.2 (sin ECAM o sin capacidad)"),
        }
    }

    fn armar_rx(&self, i: usize) {
        let d = self.rx.d(i);
        wr64(d, self.rx.buf(i) as u64);
        wr64(d + 8, 0); // quita el estado de la vuelta anterior y, en el avanzado, la dirección de cabecera
    }

    fn tx_hecho(&self, i: usize) -> bool {
        rd::<u32>(self.tx.d(i) + 12) & 1 != 0 // DD: byte de estado (legado) o bit 0 de olinfo_status (avanzado)
    }
}

pub fn iniciar(bus: u8, dev: u8, fun: u8) -> Option<Box<dyn NetDev>> {
    let id = pci::leer(bus, dev, fun, 0);
    let (nombre, fam) = modelo((id >> 16) as u16)?;
    let base = pci::bar(bus, dev, fun, 0)?;
    if !paging::mapear_mmio(base, 0x2_0000) {
        return None;
    }
    pci::activar(bus, dev, fun);
    let (rq, tq) = if fam.avanzado() { (0xC000, 0xE000) } else { (0x2800, 0x3800) };
    let mut t = Intel {
        r: base as usize,
        nombre,
        fam,
        rev: pci::leer(bus, dev, fun, 8) as u8,
        mac: [0; 6],
        rq,
        tq,
        rx: Anillo::nuevo()?,
        tx: Anillo::nuevo()?,
        rx_sig: 0,
        saltando: false,
        tx_sig: 0,
        tx_limpio: 0,
        ipgt: 8,
        dev: (id >> 16) as u16,
        bdf: (bus, dev, fun),
        reint: Cell::new(true),
        ultimo: None,
    };
    let pch = fam.pch();
    // Si algo falla a partir de aquí, el `Drop` detiene el DMA.
    if fam.b() {
        t.sin_l12(bus, dev, fun);
    }
    // Reinicio (F4): sin maestro PCIe, sin interrupciones, RX/TX parados y CTRL.RST. No se escribe IMS jamás.
    let (icr, imc) = if fam.avanzado() { (0x1500, 0x150C) } else { (0xC0, 0xD8) };
    if pch {
        if !t.pch_antes() {
            traza!("NF intel: {} no deja la PHY accesible", nombre);
            return None;
        }
        t.pch_reset();
    } else {
        t.wr(CTRL, t.rd(CTRL) | CTRL_MASTER_OFF);
        esperar(50, || t.rd(STATUS) & STATUS_MASTER_ON == 0); // sin respuesta no es fatal
        t.wr(imc, !0);
        t.wr(RCTL, 0);
        t.wr(TCTL, 1 << 3);
        t.rd(STATUS);
        espera_ms(15);
        t.wr(CTRL, t.rd(CTRL) | CTRL_RST);
        espera_ms(20);
        if !esperar(1_000, || t.rd(CTRL) & CTRL_RST == 0) {
            traza!("NF intel: {} tarda en salir del reinicio", nombre); // si de verdad no responde, la MAC lo delata
        }
        esperar(10, || t.rd(EECD) & 1 << 9 != 0); // carga automática; sin NVM no es fatal
    }
    t.wr(imc, !0);
    t.rd(icr);

    // MAC que dejó el hardware en RAR[0] (F6).
    let (l, h) = (t.rd(RAL0), t.rd(RAL0 + 4));
    t.mac = [l as u8, (l >> 8) as u8, (l >> 16) as u8, (l >> 24) as u8, h as u8, (h >> 8) as u8];
    if t.mac[0] & 1 != 0 || t.mac == [0; 6] {
        traza!("NF intel: {} MAC invalida {:02x?}", nombre, t.mac);
        return None;
    }

    // Filtros: solo RAR[0] (con AV), sin multidifusión por hash.
    t.wr(RAL0, l);
    t.wr(RAL0 + 4, h & 0xFFFF | AV);
    let otras = if fam.avanzado() { 15 } else { 0 };
    for i in 1..=otras {
        t.wr(RAL0 + 8 * i, 0);
        t.wr(RAL0 + 8 * i + 4, 0);
    }
    for i in 0..if pch { 32 } else { 128 } {
        t.wr(MTA + 4 * i, 0);
        if fam.avanzado() {
            t.wr(UTA + 4 * i, 0);
        }
    }
    if fam.b() {
        t.wr(RXPBS, 0xA2);
        t.wr(TXPBS, 0x0400_0014);
        t.wr(RFCTL, 1 << 16); // E6: cabeceras IPv6 mal formadas
    }
    if pch {
        t.pch_config();
    }
    t.wr(TIPG, TIPG_COBRE);
    // Enlace automático (SLU), sin forzar velocidad ni dúplex, sin control de flujo ni VLAN.
    let ctrl = t.rd(CTRL) & !(CTRL_FRCSPD_DPX | CTRL_RFCE_TFCE_VME) | CTRL_SLU;
    t.wr(CTRL, if fam.rx_ext() { ctrl } else { ctrl | CTRL_ASDE });
    if fam != Fam::E8254x {
        t.wr(CTRL_EXT, t.rd(CTRL_EXT) | DRV_LOAD);
    }

    // Anillo de recepción.
    for i in 0..N {
        t.armar_rx(i);
    }
    wr64(t.r + rq, t.rx.desc.dir());
    t.wr(rq + 8, (16 * N) as u32);
    t.wr(rq + 0x10, 0);
    t.wr(rq + 0x18, 0);
    if fam.avanzado() {
        t.wr(rq + 0xC, 2 | 1 << 25 | 1 << 31); // búfer de 2 KiB, un búfer avanzado, sin bloqueo si falta descriptor
        t.wr(rq + 0x28, 0x0200_0808); // ENABLE, PTHRESH 8, HTHRESH 8, WTHRESH 0
        if !esperar(10, || t.rd(rq + 0x28) & 1 << 25 != 0) {
            traza!("NF intel: {} cola RX sin habilitar", nombre);
            return None;
        }
    }
    t.wr(RCTL, RCTL_EN_BAM_SECRC);
    t.wr(rq + 0x18, (N - 1) as u32);

    // Anillo de envío.
    wr64(t.r + tq, t.tx.desc.dir());
    t.wr(tq + 8, (16 * N) as u32);
    t.wr(tq + 0x10, 0);
    t.wr(tq + 0x18, 0);
    if fam.avanzado() {
        t.wr(tq + 0x28, 0x0200_0108); // ENABLE, PTHRESH 8, HTHRESH 1, WTHRESH 0 (E8)
        if !esperar(10, || t.rd(tq + 0x28) & 1 << 25 != 0) {
            traza!("NF intel: {} cola TX sin habilitar", nombre);
            return None;
        }
    } else if fam == Fam::E82574 {
        // shortcut: valores de la familia PCH del 82574 (bit 22, GRAN, WTHRESH 1, PTHRESH 31) sin probar en hardware real; revisar con la hoja del 82574.
        t.wr(tq + 0x28, 1 << 22 | 1 << 24 | 1 << 16 | 0x1F);
    }
    t.wr(TCTL, TCTL_VALOR);
    if fam.b() {
        t.negociar();
    } else if fam == Fam::Igb && t.phy_rmw(0, 0x1200, 0).is_none() {
        traza!("NF intel: {} no se pudo reiniciar la autonegociacion", nombre); // AN activa y reiniciada
    } else if pch {
        t.pch_enlace_inicio();
    }
    Some(Box::new(t))
}

impl Drop for Intel {
    fn drop(&mut self) {
        self.parar();
    }
}

impl NetDev for Intel {
    fn name(&self) -> String {
        String::from(self.nombre)
    }

    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn link(&mut self) -> Option<Link> {
        let s = self.rd(STATUS);
        if s == !0 || s & 2 == 0 {
            if self.ultimo.take().is_some() {
                self.pch_sin_enlace();
            }
            return None;
        }
        let mbps = match s {
            _ if self.fam.b() && s & 1 << 22 != 0 => 2500,
            _ if s & 1 << 7 != 0 => 1000,
            _ if s & 1 << 6 != 0 => 100,
            _ => 10,
        };
        let full = s & 1 != 0;
        if self.fam.pch() {
            if self.ultimo != Some((mbps, full)) {
                self.pch_enlace(mbps, full);
                self.ultimo = Some((mbps, full));
            }
            return Some(Link { speed_mbps: mbps, full_duplex: full });
        }
        // E2: el I225 anterior a la revisión 2 necesita IPGT = 0xB a 2,5 Gb/s; el resto, 8.
        let ipgt = if self.fam == Fam::I225 && self.rev < 2 && mbps == 2500 { 0xB } else { 8 };
        if ipgt != self.ipgt {
            self.wr(TIPG, TIPG_COBRE & !0x3FF | ipgt);
            self.ipgt = ipgt;
        }
        Some(Link { speed_mbps: mbps, full_duplex: s & 1 != 0 })
    }

    fn send(&mut self, frame: &[u8]) -> bool {
        if !(14..=1514).contains(&frame.len()) {
            return false;
        }
        while self.tx_limpio != self.tx_sig && self.tx_hecho(self.tx_limpio) {
            self.tx_limpio = (self.tx_limpio + 1) % N;
        }
        let sig = (self.tx_sig + 1) % N;
        if sig == self.tx_limpio {
            return false;
        }
        let (b, d) = (self.tx.buf(self.tx_sig), self.tx.d(self.tx_sig));
        let largo = frame.len().max(60);
        // Búfer propio de BUF bytes; `largo` ≤ 1514.
        unsafe {
            core::ptr::write_bytes(b as *mut u8, 0, largo);
            core::ptr::copy_nonoverlapping(frame.as_ptr(), b as *mut u8, frame.len());
        }
        wr64(d, b as u64);
        let l = largo as u32;
        if self.fam.avanzado() {
            // longitud | tipo datos | EOP | IFCS | RS | DEXT, y PAYLEN = longitud << 14.
            wr::<u32>(d + 8, l | 0x0030_0000 | 1 << 24 | 1 << 25 | 1 << 27 | 1 << 29);
            wr::<u32>(d + 12, l << 14);
        } else {
            wr::<u32>(d + 8, l | (1 | 2 | 8) << 24); // longitud, CSO 0, EOP | IFCS | RS
            wr::<u32>(d + 12, 0);
        }
        fence(SeqCst);
        self.tx_sig = sig;
        self.wr(self.tq + 0x18, sig as u32);
        true
    }

    fn recv(&mut self, buf: &mut [u8]) -> Option<usize> {
        let adv = self.fam.rx_ext();
        for _ in 0..N {
            let i = self.rx_sig;
            let d = self.rx.d(i);
            let est = if adv { rd::<u32>(d + 8) } else { rd::<u8>(d + 12) as u32 };
            if est & 1 == 0 {
                return None;
            }
            fence(SeqCst);
            let largo = rd::<u16>(d + if adv { 12 } else { 8 }) as usize;
            // Error de trama: RXE (bit 31) en el avanzado; cualquier bit del byte de errores en el legado.
            let error = match self.fam {
                Fam::Pch(_) => est & 0x9700_0000 != 0, // CE, SE, SEQ, CXE y RXE
                _ if adv => est >> 31 != 0,
                _ => rd::<u8>(d + 13) != 0,
            };
            let eop = est & 2 != 0;
            let bueno = eop && !self.saltando && !error && (14..=BUF).contains(&largo) && largo <= buf.len();
            if bueno {
                // Sobre el búfer propio de BUF bytes; `largo` validado arriba.
                unsafe { core::ptr::copy_nonoverlapping(self.rx.buf(i) as *const u8, buf.as_mut_ptr(), largo) };
            }
            self.saltando = !eop; // una trama en varios descriptores no debería darse con búferes de 2 KiB
            self.armar_rx(i);
            self.wr(self.rq + 0x18, i as u32);
            self.rx_sig = (i + 1) % N;
            if bueno {
                return Some(largo);
            }
        }
        None
    }
}
