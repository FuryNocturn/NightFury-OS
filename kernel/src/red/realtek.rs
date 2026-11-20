// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Tarjetas Realtek de cable por sondeo, una cola de recepción y una de envío, sin interrupciones.
//! Escrito a partir de la especificación de sala limpia `especificaciones\realtek`.
//!
//! Primera pasada: RTL8111/8168 de las revisiones G a H (V40, V42, V44 y V46 de la especificación),
//! ruta mínima y sin firmware. Las demás revisiones se reconocen pero no arrancan todavía: las
//! 8168 antiguas añadirían su secuencia en `secuencia`/`preparar`/`parar`; las 8125/8126 (otros
//! registros de aviso e interrupciones, y firmware) necesitarán su propio arranque. Los anillos
//! (descriptores de 16 bytes, mismo formato en todas) no cambian.
use super::{rd, wr};
use crate::serial::traza;
use crate::usb::{esperar, espera_ms, Dma};
use crate::{paging, pci};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use core::sync::atomic::{fence, Ordering::SeqCst};
use nf_core::net::{Link, NetDev};

/// Descriptores por anillo (potencia de 2: los contadores de envío dan la vuelta solos) y búfer.
const N: usize = 128;
const BUF: usize = 2048;

// Registros del 8168 (MMIO, BAR2)
const MAC0: usize = 0x00;
const MAR0: usize = 0x08;
const TNPDS: usize = 0x20;
const CMD: usize = 0x37;
const TXPOLL: usize = 0x38;
const CFGTX: usize = 0x40;
const RXCFG: usize = 0x44;
const CFG9346: usize = 0x50;
const CONFIG2: usize = 0x53;
const CONFIG3: usize = 0x54;
const CONFIG5: usize = 0x56;
const PHYSTATUS: usize = 0x6C;
const ERIDR: usize = 0x70;
const ERIAR: usize = 0x74;
const EPHYAR: usize = 0x80;
const OCPDR: usize = 0xB0;
const OCPPHY: usize = 0xB8;
const DLLPR: usize = 0xD0;
const MCU: usize = 0xD3;
const RMS: usize = 0xDA;
const CPLUS: usize = 0xE0;
const INTMIT: usize = 0xE2;
const RDSAR: usize = 0xE4;
const MAXTX: usize = 0xEC;
const MISC: usize = 0xF0;
const MISC1: usize = 0xF2;

const MISC_RXDV: u32 = 1 << 19;
const OWN: u32 = 1 << 31;
const EOR: u32 = 1 << 30;
const FS: u32 = 1 << 29;
const LS: u32 = 1 << 28;

/// Revisiones por (máscara, valor) sobre `CfgTX >> 20 & 0xFCF`, en el orden de la especificación (la
/// primera que cuadra manda): nombre, etiqueta V y firmware que pediría (vacío = ninguno).
const REVS: &[(u16, u16, &str, u8, &str)] = &[
    (0x7C8, 0x380, "RTL8168b", 17, ""),
    (0x7CF, 0x3C8, "RTL8168cp", 18, ""),
    (0x7CF, 0x3C0, "RTL8168c", 19, ""),
    (0x7CF, 0x3C2, "RTL8168c", 20, ""),
    (0x7CF, 0x3C3, "RTL8168c", 21, ""),
    (0x7C8, 0x3C0, "RTL8168c", 22, ""),
    (0x7CF, 0x3C9, "RTL8168cp", 23, ""),
    (0x7C8, 0x3C8, "RTL8168cp", 24, ""),
    (0x7CF, 0x281, "RTL8168d", 25, "rtl8168d-1"),
    (0x7C8, 0x280, "RTL8168d", 26, "rtl8168d-2"),
    (0x7CF, 0x28A, "RTL8168dp", 28, ""),
    (0x7CF, 0x28B, "RTL8168dp", 31, ""),
    (0x7CF, 0x2C1, "RTL8168e", 32, "rtl8168e-1"),
    (0x7C8, 0x2C0, "RTL8168e", 33, "rtl8168e-2"),
    (0x7C8, 0x2C8, "RTL8168e-vl", 34, "rtl8168e-3"),
    (0x7CF, 0x480, "RTL8168f", 35, "rtl8168f-1"),
    (0x7CF, 0x481, "RTL8168f", 36, "rtl8168f-2"),
    (0x7C8, 0x488, "RTL8411", 38, "rtl8411-1"),
    (0x7CF, 0x4C0, "RTL8168g", 40, "rtl8168g-2"),
    (0x7CF, 0x509, "RTL8168gu", 42, "rtl8168g-3"),
    (0x7CF, 0x5C8, "RTL8411b", 44, "rtl8411-2"),
    (0x7CF, 0x541, "RTL8168h", 46, "rtl8168h-2"),
    (0x7CF, 0x6C0, "RTL8168m", 46, "rtl8168h-2"),
    (0x7CF, 0x502, "RTL8168ep", 51, ""),
    (0x7CF, 0x54A, "RTL8168fp", 52, "rtl8168fp-3"),
    (0x7CF, 0x54B, "RTL8168fp", 52, ""),
    (0x7CF, 0x609, "RTL8125a", 61, "rtl8125a-3"),
    (0x7CF, 0x641, "RTL8125b", 63, "rtl8125b-2"),
    (0x7CF, 0x688, "RTL8125d", 64, "rtl8125d-1"),
    (0x7CF, 0x689, "RTL8125d", 64, "rtl8125d-2"),
    (0x7CF, 0x68A, "RTL8125k", 64, "rtl8125k-1"),
    (0x7CF, 0x68B, "RTL9151a", 64, "rtl9151a-1"),
    (0x7CF, 0x708, "RTL8125cp", 65, "rtl8125cp-1"),
    (0x7CF, 0x681, "RTL8125bp", 66, "rtl8125bp-2"),
    (0x7CF, 0x649, "RTL8126a", 70, "rtl8126a-2"),
    (0x7CF, 0x64A, "RTL8126a", 70, "rtl8126a-3"),
    (0x7CF, 0x6C9, "RTL8127a", 80, "rtl8127a-1"),
];

/// PCI (fabricante, dispositivo) que se prueban; la revisión real sale siempre de CfgTX.
/// shortcut: sin los reetiquetados que la especificación liga a un subdispositivo (Linksys 1737:1032,
/// 0001:8168), ni el 10EC:8136 (100 Mb), ni el 8169/8110 PCI; añadir si aparece una tarjeta real.
const IDS: &[(u16, u16)] = &[
    (0x10EC, 0x8168),
    (0x10EC, 0x8161),
    (0x10EC, 0x2502),
    (0x10EC, 0x2600),
    (0x10EC, 0x8125),
    (0x10EC, 0x3000),
    (0x10EC, 0x5000),
    (0x10EC, 0x8126),
    (0x10EC, 0x0E10),
    (0x1186, 0x4300),
    (0x1186, 0x4302),
    (0x1259, 0xC107),
    (0x16EC, 0x0116),
];

pub fn soporta(vendor: u16, dev: u16) -> bool {
    IDS.contains(&(vendor, dev))
}

/// Tablas del EPHY: (registro, bits a limpiar, bits a poner).
const EPHY40: &[(u8, u16, u16)] = &[(0x00, 0x0008, 0), (0x0C, 0x3FF0, 0x0820), (0x1E, 0, 0x0001), (0x19, 0x8000, 0)];
const EPHY42: &[(u8, u16, u16)] = &[
    (0x00, 0x0008, 0),
    (0x0C, 0x3FF0, 0x0820),
    (0x19, 0xFFFF, 0x7C00),
    (0x1E, 0xFFFF, 0x20EB),
    (0x0D, 0xFFFF, 0x1666),
    (0x00, 0xFFFF, 0x10A3),
    (0x06, 0xFFFF, 0xF050),
    (0x04, 0, 0x0010),
    (0x1D, 0x4000, 0),
];
const EPHY44: &[(u8, u16, u16)] = &[
    (0x00, 0x0008, 0),
    (0x0C, 0x37D0, 0x0820),
    (0x1E, 0, 0x0001),
    (0x19, 0x8021, 0),
    (0x1E, 0, 0x2000),
    (0x0D, 0x0100, 0x0200),
    (0x00, 0, 0x0080),
    (0x06, 0, 0x0010),
    (0x04, 0, 0x0010),
    (0x1D, 0, 0x4000),
];
const EPHY46: &[(u8, u16, u16)] = &[
    (0x1E, 0x0800, 0x0001),
    (0x1D, 0, 0x0800),
    (0x05, 0xFFFF, 0x2089),
    (0x06, 0xFFFF, 0x5881),
    (0x04, 0xFFFF, 0x854A),
    (0x01, 0xFFFF, 0x068B),
];

/// Dirección OCP del registro estándar `r` del PHY (página 0).
fn std_phy(r: u16) -> u16 {
    0xA400 + 2 * r
}

fn mac_valida(m: &[u8; 6]) -> bool {
    m[0] & 1 == 0 && *m != [0; 6]
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
    /// EOR del último descriptor: el chip vuelve al primero.
    fn eor(i: usize) -> u32 {
        if i == N - 1 {
            EOR
        } else {
            0
        }
    }
}

pub struct Realtek {
    r: usize,
    bdf: (u8, u8, u8),
    nombre: String,
    v: u8,
    mac: [u8; 6],
    rx: Anillo,
    tx: Anillo,
    rx_sig: usize,
    /// Contadores de envío que dan la vuelta (N divide a 2^32): ocupados = prod - cons.
    tx_prod: u32,
    tx_cons: u32,
}

impl Realtek {
    fn r8(&self, o: usize) -> u8 {
        rd(self.r + o)
    }
    fn r16(&self, o: usize) -> u16 {
        rd(self.r + o)
    }
    fn r32(&self, o: usize) -> u32 {
        rd(self.r + o)
    }
    fn w8(&self, o: usize, v: u8) {
        wr(self.r + o, v)
    }
    fn w16(&self, o: usize, v: u16) {
        wr(self.r + o, v)
    }
    fn w32(&self, o: usize, v: u32) {
        wr(self.r + o, v)
    }

    // ---------- Accesos auxiliares (F4) ----------

    /// ERI (tipo 0, ExGMAC): `mask` = bytes de 4 que se tocan.
    fn eri_w(&self, a: u32, v: u32, mask: u32) -> Option<()> {
        self.w32(ERIDR, v);
        self.w32(ERIAR, OWN | mask << 12 | a);
        esperar(10, || self.r32(ERIAR) & OWN == 0).then_some(())
    }
    fn eri_r(&self, a: u32) -> Option<u32> {
        self.w32(ERIAR, 0xF << 12 | a);
        esperar(10, || self.r32(ERIAR) & OWN != 0).then(|| self.r32(ERIDR))
    }
    fn eri_rmw(&self, a: u32, quitar: u32, poner: u32) -> Option<()> {
        let v = self.eri_r(a)?;
        self.eri_w(a, v & !quitar | poner, 0xF)
    }

    fn ephy_rmw(&self, reg: u8, quitar: u16, poner: u16) -> Option<()> {
        let r = (reg & 0x1F) as u32;
        self.w32(EPHYAR, r << 16);
        let mut v = 0;
        esperar(2, || {
            v = self.r32(EPHYAR);
            v & OWN != 0
        })
        .then_some(())?;
        self.w32(EPHYAR, OWN | r << 16 | (v as u16 & !quitar | poner) as u32);
        let ok = esperar(2, || self.r32(EPHYAR) & OWN == 0);
        espera_ms(1); // la especificación pide 10 us más tras cada escritura
        ok.then_some(())
    }

    /// OCP del MAC: sin espera descrita, dirección par de 16 bits.
    fn ocp_r(&self, a: u16) -> u16 {
        self.w32(OCPDR, (a as u32) << 15);
        self.r32(OCPDR) as u16
    }
    fn ocp_w(&self, a: u16, v: u16) {
        self.w32(OCPDR, OWN | (a as u32) << 15 | v as u32);
    }
    fn ocp_rmw(&self, a: u16, quitar: u16, poner: u16) {
        self.ocp_w(a, self.ocp_r(a) & !quitar | poner);
    }

    /// PHY por OCP (V40 en adelante): `a` es la dirección OCP (`std_phy` para los registros estándar).
    fn phy_r(&self, a: u16) -> Option<u16> {
        self.w32(OCPPHY, (a as u32) << 15);
        let mut v = 0;
        esperar(2, || {
            v = self.r32(OCPPHY);
            v & OWN != 0
        })
        .then_some(v as u16)
    }
    fn phy_w(&self, a: u16, d: u16) -> Option<()> {
        if self.v == 40 && a == std_phy(0) {
            // Ajuste de V40: reinicio o encendido del PHY ponen los bits altos de ERI 0x1A8; apagarlo los quita.
            let poner = d & 1 << 15 != 0 || d & 1 << 11 == 0;
            self.eri_rmw(0x1A8, if poner { 0 } else { 0xFC00_0000 }, if poner { 0xFC00_0000 } else { 0 })?;
        }
        self.w32(OCPPHY, OWN | (a as u32) << 15 | d as u32);
        esperar(2, || self.r32(OCPPHY) & OWN == 0).then_some(())
    }
    fn phy_rmw(&self, a: u16, poner: u16, quitar: u16) -> Option<()> {
        let v = self.phy_r(a)?;
        self.phy_w(a, v & !quitar | poner)
    }

    // ---------- Parada, reinicio y arranque (F3) ----------

    /// Antes del primer reinicio en V40 en adelante (paso 2 de 3.7).
    fn preparar(&self) {
        self.w32(MISC, self.r32(MISC) | MISC_RXDV);
        self.w8(CMD, self.r8(CMD) & !0x0C);
        espera_ms(1);
        self.w8(MCU, self.r8(MCU) & !0x80);
        if self.v <= 48 {
            self.ocp_rmw(0xE8DE, 1 << 14, 0);
            esperar(20, || self.r8(MCU) & 2 != 0);
            self.ocp_rmw(0xE8DE, 0, 1 << 15);
            esperar(20, || self.r8(MCU) & 2 != 0);
        }
    }

    /// Cierra RXDV, vacía los FIFO y reinicia. `false` si el reinicio no termina en 10 ms. Sin la
    /// petición de parada: en el 8168 G+ puede colgar el MAC.
    fn parar(&self) -> bool {
        self.w32(RXCFG, self.r32(RXCFG) & !0x3F);
        self.w32(MISC, self.r32(MISC) | MISC_RXDV);
        espera_ms(2);
        // Sin vaciar no es fatal: el reinicio de abajo manda.
        if !esperar(5, || self.r32(CFGTX) & 1 << 11 != 0) || !esperar(5, || self.r8(MCU) & 0x30 == 0x30) {
            traza!("NF realtek: FIFO sin vaciar antes del reinicio");
        }
        self.w8(CMD, 0x10);
        esperar(10, || self.r8(CMD) & 0x10 == 0)
    }

    /// MAC: ERI 0xE0/0xE4, luego los registros 0x00..0x05, y si ninguna vale, una local aleatoria
    /// (que se escribe en el chip). El resultado se deja también en el chip si difería.
    fn leer_mac(&mut self) {
        let eri = self.eri_r(0xE0).zip(self.eri_r(0xE4)).map(|(a, b)| [a as u8, (a >> 8) as u8, (a >> 16) as u8, (a >> 24) as u8, b as u8, (b >> 8) as u8]);
        let (l, h) = (self.r32(MAC0), self.r16(MAC0 + 4) as u32);
        let regs = [l as u8, (l >> 8) as u8, (l >> 16) as u8, (l >> 24) as u8, h as u8, (h >> 8) as u8];
        self.mac = match eri.filter(mac_valida).or(Some(regs).filter(mac_valida)) {
            Some(m) => m,
            None => {
                let e = crate::entropia().to_le_bytes();
                let m = [e[0] & 0xFE | 2, e[1], e[2], e[3], e[4], e[5]]; // administrada localmente, unicast
                traza!("NF realtek: MAC invalida {:02x?}, se usa la local {:02x?}", regs, m);
                m
            }
        };
        if self.mac != regs {
            self.w8(CFG9346, 0xC0);
            self.w32(MAC0 + 4, self.mac[4] as u32 | (self.mac[5] as u32) << 8);
            self.w32(MAC0, u32::from_le_bytes([self.mac[0], self.mac[1], self.mac[2], self.mac[3]]));
            self.r32(MAC0);
            self.w8(CFG9346, 0);
        }
    }

    /// Autonegociación de todo lo que el chip anuncia (F7, ruta mínima); el enlace lo espera `sondear`.
    fn phy(&self) -> Option<()> {
        self.phy_w(std_phy(0), 1 << 15)?;
        if !esperar(600, || self.phy_r(std_phy(0)).is_some_and(|v| v & 1 << 15 == 0)) {
            traza!("NF realtek: la PHY no sale del reinicio");
            return None;
        }
        self.phy_w(std_phy(4), 0x05E1)?; // selector, 10/100 half y full, pausa
        self.phy_w(std_phy(9), 0x0300)?; // 1000 half y full
        self.phy_rmw(0xA430, 0, 1 << 2)?; // sin ALDPS (página 0x0A43, registro 0x10)
        self.phy_rmw(0xA442, 1 << 11, 0)?; // 10 Mb/s habilitado (página 0x0A44, registro 0x11)
        self.phy_w(std_phy(0), 0x1200) // autonegociación activa y reiniciada, sin apagado
    }

    /// Apaga ASPM y CLKREQ en el control de enlace PCIe del dispositivo (F2.6).
    fn sin_aspm_pcie(&self) {
        let (b, d, f) = self.bdf;
        let cap = pci::capacidades(b, d, f).into_iter().find(|&c| pci::leer(b, d, f, c) as u8 == 0x10);
        if let Some(c) = cap.and_then(|c| c.checked_add(0x10)) {
            // Bits 0-1 ASPM y 8 CLKREQ del control; la mitad alta (estado, RW1C) se escribe a 0.
            pci::escribir(b, d, f, c, pci::leer(b, d, f, c) & 0xFEFC);
        }
    }

    /// Latencia ASPM: byte de configuración 0x70F = 0x27, por el ECAM.
    /// shortcut: sin la vía CSI de respaldo si no hay ECAM; añadirla si una placa no tiene MCFG.
    fn latencia_aspm(&self) {
        let (b, d, f) = self.bdf;
        let ok = pci::leer_ext(b, d, f, 0x70C).is_some_and(|v| pci::escribir_ext(b, d, f, 0x70C, v & 0x00FF_FFFF | 0x27 << 24));
        if !ok {
            traza!("NF realtek: latencia ASPM sin escribir (sin ECAM)");
        }
    }

    /// Tamaños de FIFO, umbrales de pausa y latencia ASPM, comunes a V40 a V46 (4.F3-X).
    fn fifo(&self) -> Option<()> {
        self.eri_w(0xC8, 0x0008_0002, 0xF)?;
        self.eri_w(0xE8, 0x0010_0006, 0xF)?;
        self.eri_w(0xCC, 0x38, 1)?;
        self.eri_w(0xD0, 0x48, 1)?;
        self.latencia_aspm();
        self.eri_rmw(0xDC, 1, 0)?; // reinicio del filtro de paquetes: a 0 y otra vez a 1
        self.eri_rmw(0xDC, 0, 1)
    }

    /// Secuencia propia de la revisión (4.F3-X, V40 a V46), con la configuración desbloqueada.
    /// shortcut: sin el parche del V44 (107 palabras sin transcribir en la especificación, omitible);
    /// se añade si aparece una 8411b con la unidad RX confundida tras apagar el PHY.
    fn secuencia(&self) -> Option<()> {
        let v = self.v;
        if v == 46 {
            for &(r, q, p) in EPHY46 {
                self.ephy_rmw(r, q, p)?;
            }
            self.fifo()?;
            self.eri_rmw(0xDC, 0, 0x1C)?;
            self.eri_w(0x5F0, 0x4F87, 3)?;
            self.w32(MISC, self.r32(MISC) & !MISC_RXDV); // reabre RXDV
            self.eri_w(0xC0, 0, 3)?;
            self.eri_w(0xB8, 0, 3)?;
            self.w8(DLLPR, self.r8(DLLPR) & !(1 << 6));
            self.w8(MISC1, self.r8(MISC1) & !(1 << 6));
            self.w8(DLLPR, self.r8(DLLPR) & !(1 << 7));
            self.eri_rmw(0x1B0, 1 << 12, 0)?;
            self.w8(CONFIG3, self.r8(CONFIG3) & !2);
            let cnt = self.phy_r(0xC426)? & 0x3FFF; // página 0x0C42, registro 0x13
            if cnt > 0 {
                self.ocp_rmw(0xD412, 0x0FFF, (16_000_000 / cnt as u32) as u16 & 0x0FFF);
            }
            self.ocp_rmw(0xE056, 0x00F0, 0);
            self.ocp_rmw(0xE052, 0x6000, 0x8008);
            self.ocp_rmw(0xE0D6, 0x01FF, 0x017F);
            self.ocp_rmw(0xD420, 0x0FFF, 0x047F);
            self.ocp_w(0xE63E, 1);
            self.ocp_w(0xE63E, 0);
            self.ocp_w(0xC094, 0);
            self.ocp_w(0xC09E, 0);
        } else {
            self.fifo()?;
            self.eri_w(0x2F8, 0x1D8F, 3)?;
            self.w32(MISC, self.r32(MISC) & !MISC_RXDV);
            self.eri_w(0xC0, 0, 3)?;
            self.eri_w(0xB8, 0, 3)?;
            self.eri_rmw(0x2FC, 0b110, 1)?;
            self.eri_rmw(0x1B0, 1 << 12, 0)?;
            self.w8(CONFIG3, self.r8(CONFIG3) & !2);
            let tabla = match v {
                40 => EPHY40,
                42 => EPHY42,
                _ => EPHY44,
            };
            for &(r, q, p) in tabla {
                self.ephy_rmw(r, q, p)?;
            }
        }
        self.eri_rmw(0x1B0, 3, 0) // EEE apagado (F15, ERI 0x1B0 bits 0-1)
    }

    fn armar_rx(&self, i: usize) {
        let d = self.rx.d(i);
        wr::<u64>(d + 8, self.rx.buf(i) as u64);
        wr::<u32>(d + 4, 0);
        fence(SeqCst);
        wr::<u32>(d, OWN | Anillo::eor(i) | BUF as u32);
    }

    /// Arranque completo (3.7). Si falla, el `Drop` detiene el DMA.
    fn arrancar(&mut self) -> Option<()> {
        self.sin_aspm_pcie();
        self.preparar();
        if !self.parar() {
            traza!("NF realtek: {} reinicio fallido", self.nombre);
            return None;
        }
        self.leer_mac();
        self.phy()?;
        for i in 0..N {
            self.armar_rx(i);
        }
        wr::<u32>(self.tx.d(N - 1), EOR);
        self.w8(CFG9346, 0xC0);
        // Antes del EPHY: sin ASPM (Config5 bit 0) ni CLKREQ (Config2 bit 7); en V46, también OCP 0xE092.
        self.w8(CONFIG5, self.r8(CONFIG5) & !1);
        self.w8(CONFIG2, self.r8(CONFIG2) & !0x80);
        if self.v >= 46 {
            self.ocp_rmw(0xE092, 0x00FF, 0);
            self.ocp_w(0xE048, 1500 + 14 + 0x20); // temporizador de inactividad de EEE
        }
        // C+: sin extracción de VLAN ni suma en RX; PCIDAC solo si algo queda por encima de 4 GiB.
        let alto = [&self.rx.desc, &self.rx.bufs, &self.tx.desc, &self.tx.bufs].iter().any(|d| d.dir() + (BUF * N) as u64 >> 32 != 0);
        self.w16(CPLUS, self.r16(CPLUS) & !(1 << 5 | 1 << 6) | 1 << 3 | (alto as u16) << 4);
        self.w8(MAXTX, 0x27);
        self.secuencia()?;
        self.w16(INTMIT, 0);
        self.w16(RMS, BUF as u16);
        // Parte alta antes que la baja: hay placas que lo exigen.
        let (t, r) = (self.tx.desc.dir(), self.rx.desc.dir());
        self.w32(TNPDS + 4, (t >> 32) as u32);
        self.w32(TNPDS, t as u32);
        self.w32(RDSAR + 4, (r >> 32) as u32);
        self.w32(RDSAR, r as u32);
        self.w8(CFG9346, 0);
        self.r8(CMD);
        // V40 en adelante: primero CfgTX y filtros, luego habilitar TX y RX.
        self.w32(CFGTX, 7 << 8 | 3 << 24 | 1 << 7); // ráfaga DMA ilimitada, hueco 3, FIFO automático
        self.w32(RXCFG, 1 << 15 | 1 << 14 | 7 << 8 | 1 << 11 | 1 << 1 | 1 << 3); // RX128, MULTI, ráfaga, EARLY_OFF; mi dirección y difusión
        self.w32(MAR0, 0);
        self.w32(MAR0 + 4, 0);
        self.w32(MISC, self.r32(MISC) & !MISC_RXDV);
        self.w8(CMD, 0x0C);
        self.w16(0x3C, 0); // máscara de interrupciones, siempre a 0
        Some(())
    }

    fn tx_limpiar(&mut self) {
        while self.tx_cons != self.tx_prod && rd::<u32>(self.tx.d(self.tx_cons as usize % N)) & OWN == 0 {
            self.tx_cons = self.tx_cons.wrapping_add(1);
        }
    }
}

pub fn iniciar(bus: u8, dev: u8, fun: u8) -> Option<Box<dyn NetDev>> {
    if pci::leer(bus, dev, fun, 0) as u16 == 0xFFFF {
        traza!("NF realtek: el dispositivo no responde");
        return None;
    }
    let base = pci::bar(bus, dev, fun, 2)?;
    if !paging::mapear_mmio(base, 0x1000) {
        return None;
    }
    // Solo memoria hasta saber qué chip es: no se activa el DMA de uno que no se va a usar.
    pci::escribir(bus, dev, fun, 4, pci::leer(bus, dev, fun, 4) & 0xFFFF | 2);
    let cfgtx: u32 = rd(base as usize + CFGTX);
    if cfgtx == !0 {
        traza!("NF realtek: fallo de bus (CfgTX = ffffffff)");
        return None;
    }
    let ident = (cfgtx >> 20) as u16 & 0xFCF;
    let Some(&(_, _, nombre, v, fw)) = REVS.iter().find(|r| ident & r.0 == r.1) else {
        traza!("NF realtek: revision no soportada {:#x}", ident);
        return None;
    };
    if !matches!(v, 40 | 42 | 44 | 46) {
        traza!("NF realtek: {} (revision {:#x}) revision no soportada todavia", nombre, ident);
        return None;
    }
    pci::activar(bus, dev, fun);
    let mut t = Realtek {
        r: base as usize,
        bdf: (bus, dev, fun),
        nombre: format!("Realtek {}", nombre),
        v,
        mac: [0; 6],
        rx: Anillo::nuevo()?,
        tx: Anillo::nuevo()?,
        rx_sig: 0,
        tx_prod: 0,
        tx_cons: 0,
    };
    if !fw.is_empty() {
        traza!("NF realtek: {} sin firmware", nombre);
    }
    if t.arrancar().is_none() {
        traza!("NF realtek: {} no arranca", nombre);
        return None;
    }
    traza!("NF realtek: {} revision {:#x}", nombre, ident);
    Some(Box::new(t))
}

impl Drop for Realtek {
    /// F14: RX cerrado, FIFO vaciados, chip en reinicio y sin maestro de bus.
    fn drop(&mut self) {
        self.parar();
        let (b, d, f) = self.bdf;
        pci::escribir(b, d, f, 4, pci::leer(b, d, f, 4) & 0xFFFF & !4);
    }
}

impl NetDev for Realtek {
    fn name(&self) -> String {
        self.nombre.clone()
    }

    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn link(&mut self) -> Option<Link> {
        let s = self.r8(PHYSTATUS);
        if s == 0xFF || s & 2 == 0 {
            return None;
        }
        let mbps = match s {
            _ if s & 0x10 != 0 => 1000,
            _ if s & 0x08 != 0 => 100,
            _ if s & 0x04 != 0 => 10,
            _ => return None,
        };
        Some(Link { speed_mbps: mbps, full_duplex: s & 1 != 0 })
    }

    fn send(&mut self, frame: &[u8]) -> bool {
        if !(14..=1514).contains(&frame.len()) {
            return false;
        }
        self.tx_limpiar();
        if self.tx_prod.wrapping_sub(self.tx_cons) as usize >= N {
            return false;
        }
        let i = self.tx_prod as usize % N;
        let (b, d) = (self.tx.buf(i), self.tx.d(i));
        let largo = frame.len().max(60); // el chip no siempre rellena: se rellena siempre
        // Búfer propio de BUF bytes; `largo` ≤ 1514.
        unsafe {
            core::ptr::write_bytes(b as *mut u8, 0, largo);
            core::ptr::copy_nonoverlapping(frame.as_ptr(), b as *mut u8, frame.len());
        }
        wr::<u64>(d + 8, b as u64);
        wr::<u32>(d + 4, 0);
        fence(SeqCst);
        wr::<u32>(d, OWN | FS | LS | Anillo::eor(i) | largo as u32);
        fence(SeqCst);
        self.tx_prod = self.tx_prod.wrapping_add(1);
        self.w8(TXPOLL, 0x40);
        true
    }

    fn recv(&mut self, buf: &mut [u8]) -> Option<usize> {
        // El chip puede perder avisos si las tramas van muy juntas: con envíos pendientes se repite.
        self.tx_limpiar();
        if self.tx_cons != self.tx_prod {
            self.w8(TXPOLL, 0x40);
        }
        for _ in 0..64 {
            let i = self.rx_sig;
            let d = self.rx.d(i);
            let o: u32 = rd(d);
            if o & OWN != 0 {
                return None;
            }
            fence(SeqCst);
            let largo = (o & 0x3FFF) as usize; // incluye los 4 bytes de FCS
            let bueno = o & (FS | LS) == FS | LS && o & 1 << 21 == 0 && (18..=BUF).contains(&largo) && largo - 4 <= buf.len();
            if bueno {
                // Sobre el búfer propio de BUF bytes; `largo` validado arriba.
                unsafe { core::ptr::copy_nonoverlapping(self.rx.buf(i) as *const u8, buf.as_mut_ptr(), largo - 4) };
            }
            self.armar_rx(i);
            self.rx_sig = (i + 1) % N;
            if bueno {
                return Some(largo - 4);
            }
        }
        None
    }
}
