// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Familia A (I217, I218, I219; MAC y PHY integradas en el PCH), a partir de la especificación de
//! sala limpia (`especificaciones\intel-ethernet`, familia A: F3 a F9, F12 y F17).
//! Recepción extendida y envío legado, por sondeo. Los anillos y `NetDev` son los de `intel.rs`.
use super::*;
use crate::usb::ahora_us;

// Generaciones de plataforma (orden de F1).
const SPT: u8 = 1;
const TGP: u8 = 3;
const MTP: u8 = 5;
const PTP: u8 = 6;

// Registros propios de la familia A (secciones 7.2 y 7.5).
const FEXTNVM6: usize = 0x10;
const FEXTNVM4: usize = 0x24;
const FEXTNVM3: usize = 0x3C;
const KMRNCTRLSTA: usize = 0x34;
const FEXTNVM7: usize = 0xE4;
const EXTCNF_CTRL: usize = 0xF00;
const IOSFPC: usize = 0xF28;
const PBA: usize = 0x1000;
const PBECCSTS: usize = 0x100C;
const FCRTL: usize = 0x2160;
const FCRTH: usize = 0x2168;
const RDBAL0: usize = 0x2800;
const RDH0: usize = 0x2810;
const RDT0: usize = 0x2818;
const RXDCTL0: usize = 0x2828;
const KABGTXD: usize = 0x3004;
const TDBAL0: usize = 0x3800;
const TDLEN0: usize = 0x3808;
const TDH0: usize = 0x3810;
const TDT0: usize = 0x3818;
const TXDCTL0: usize = 0x3828;
const TARC0: usize = 0x3840;
const TXDCTL1: usize = 0x3928;
const TARC1: usize = 0x3940;
const RXCSUM: usize = 0x5000;
const WUC: usize = 0x5800;
const H2ME: usize = 0x5B50;
const FWSM: usize = 0x5B54;
const FEXTNVM9: usize = 0x5BB4;
const FEXTNVM11: usize = 0x5BBC;
const FEXTNVM12: usize = 0x5BC0;
const FFLT_DBG: usize = 0x5F04;
const VET: usize = 0x38;
const IMC_A: usize = 0xD8;
const ICR_A: usize = 0xC0;

const SWFLAG: u32 = 1 << 5;
const PHY_RST: u32 = 1 << 31;

/// I217 e I218 con su nombre; el resto de ids de I219 va por generación (la especificación no
/// dice cuáles son -V y cuáles -LM, así que se llaman solo «I219»).
const I217_I218: &[(u16, &str, u8)] = &[
    (0x153A, "Intel I217-LM", 0),
    (0x153B, "Intel I217-V", 0),
    (0x155A, "Intel I218-LM", 0),
    (0x1559, "Intel I218-V", 0),
    (0x15A0, "Intel I218-LM2", 0),
    (0x15A1, "Intel I218-V2", 0),
    (0x15A2, "Intel I218-LM3", 0),
    (0x15A3, "Intel I218-V3", 0),
];
const I219: &[(u8, &[u16])] = &[
    (1, &[0x156F, 0x1570, 0x15B7, 0x15B8, 0x15B9, 0x15D7, 0x15D8, 0x15E3, 0x15D6, 0x0D53, 0x0D55]),
    (2, &[0x15BD, 0x15BE, 0x15BB, 0x15BC, 0x15DF, 0x15E0, 0x15E1, 0x15E2, 0x0D4E, 0x0D4F, 0x0D4C, 0x0D4D]),
    (3, &[0x15FB, 0x15FC, 0x15F9, 0x15FA, 0x15F4, 0x15F5]),
    (4, &[0x0DC5, 0x0DC6, 0x1A1E, 0x1A1F, 0x1A1C, 0x1A1D, 0x0DC7, 0x0DC8, 0x550C, 0x550D]),
    (5, &[0x550A, 0x550B, 0x550E, 0x550F, 0x5510, 0x5511, 0x57A0, 0x57A1]),
    (6, &[0x57B3, 0x57B4, 0x57B7, 0x57B8, 0x57B9, 0x57BA]),
];

/// Nombre y generación de la plataforma (0 LPT … 6 PTP) del id de dispositivo.
pub fn modelo(dev: u16) -> Option<(&'static str, u8)> {
    if let Some(m) = I217_I218.iter().find(|m| m.0 == dev) {
        return Some((m.1, m.2));
    }
    I219.iter().find(|g| g.1.contains(&dev)).map(|g| ("Intel I219", g.0))
}

fn us(n: u64) {
    let fin = ahora_us() + n;
    while ahora_us() < fin {
        core::hint::spin_loop();
    }
}

impl Intel {
    fn gen(&self) -> u8 {
        match self.fam {
            Fam::Pch(g) => g,
            _ => 0,
        }
    }
    /// ULP solo se sale en I219 y en I218-LM/V y LM3/V3 (F5).
    fn con_ulp(&self) -> bool {
        !matches!(self.dev, 0x153A | 0x153B | 0x15A0 | 0x15A1)
    }
    /// I218 de las plataformas LPT-LP (arreglo de K1, E13).
    fn lpt_lp(&self) -> bool {
        matches!(self.dev, 0x155A | 0x1559 | 0x15A2 | 0x15A3)
    }

    // ---------- F3: propiedad de la PHY ----------

    /// Bandera de software de la PHY (EXTCNF_CTRL.SWFLAG): hasta 100 ms a que esté libre y 1000 ms a que se confirme.
    fn bandera(&self) -> bool {
        if !esperar(100, || self.rd(EXTCNF_CTRL) & SWFLAG == 0) {
            return false;
        }
        self.wr(EXTCNF_CTRL, self.rd(EXTCNF_CTRL) | SWFLAG);
        if esperar(1000, || self.rd(EXTCNF_CTRL) & SWFLAG != 0) {
            return true;
        }
        self.bandera_soltar();
        false
    }
    fn bandera_soltar(&self) {
        self.wr(EXTCNF_CTRL, self.rd(EXTCNF_CTRL) & !SWFLAG);
    }
    /// FWSM bit 6 en 0: el firmware impide reiniciar la PHY. Espera hasta 300 ms a que se libere.
    fn phy_bloqueada(&self) -> bool {
        !esperar(300, || self.rd(FWSM) & 0x40 != 0)
    }

    // ---------- F7: acceso a la PHY (con la bandera tomada) ----------

    /// Un acceso MDIC con la dirección MDIO dada; comprueba además que vuelve el mismo registro.
    fn mdic_a(&self, dir: u32, reg: u32, w: Option<u16>) -> Option<u16> {
        let op = match w {
            Some(d) => 1 << 26 | d as u32,
            None => 1 << 27,
        };
        self.wr(MDIC, op | reg << 16 | dir << 21);
        let mut v = 0;
        let listo = esperar(100, || {
            v = self.rd(MDIC);
            v & 1 << 28 != 0
        });
        (listo && v & 1 << 30 == 0 && v >> 16 & 0x1F == reg).then_some(v as u16)
    }
    /// Registro `reg` de la página `pag`. Página 0: dirección MDIO 2; páginas ≥ 768: dirección 1; el
    /// registro de página (31) siempre con dirección 1. En MTP y posteriores, hasta 3 intentos.
    fn phy(&self, pag: u16, reg: u32, w: Option<u16>) -> Option<u16> {
        let (dir, sel) = if pag >= 768 { (1, if pag == 768 { 0 } else { pag * 32 }) } else { (2, 0) };
        let intentos = if self.gen() >= MTP && self.reint.get() { 3 } else { 1 };
        for i in 0..intentos {
            if i > 0 {
                espera_ms(10);
            }
            let r = (|| {
                if reg >= 16 {
                    self.mdic_a(1, 31, Some(sel))?;
                }
                self.mdic_a(dir, reg, w)
            })();
            if r.is_some() {
                return r;
            }
        }
        None
    }
    fn pg_rmw(&self, pag: u16, reg: u32, poner: u16, quitar: u16) -> Option<()> {
        let v = self.phy(pag, reg, None)?;
        self.phy(pag, reg, Some(v & !quitar | poner)).map(|_| ())
    }
    /// Registro extendido (EMI): dirección en el registro 16 de la página 0 y dato en el 17.
    fn emi(&self, dir: u16, v: u16) -> Option<()> {
        self.phy(0, 16, Some(dir))?;
        self.phy(0, 17, Some(v)).map(|_| ())
    }
    fn kmrn_rd(&self, off: u32) -> u32 {
        self.wr(KMRNCTRLSTA, off << 16 | 1 << 21);
        us(2);
        self.rd(KMRNCTRLSTA) & 0xFFFF
    }
    fn kmrn_wr(&self, off: u32, v: u32) {
        self.wr(KMRNCTRLSTA, off << 16 | v);
        us(2);
    }

    /// Flanco de CTRL.PHY_RST y espera de «configuración completada» (F4 paso 8). No hace nada si el firmware lo bloquea.
    fn phy_reset(&self) {
        if self.phy_bloqueada() {
            traza!("NF intel: reinicio de PHY bloqueado por el firmware");
            return;
        }
        let tomada = self.bandera();
        let c = self.rd(CTRL);
        self.wr(CTRL, c | PHY_RST);
        self.rd(STATUS);
        espera_ms(1); // duda: la especificación no da el tiempo de PHY_RST en la familia A (en la B, ~100 us)
        self.wr(CTRL, c & !PHY_RST);
        self.rd(STATUS);
        if tomada {
            self.bandera_soltar();
        }
        self.config_hecha();
    }
    /// Tras reiniciar la PHY: espera LAN_INIT_DONE (con límite) y lo borra, y borra PHYRA.
    fn config_hecha(&self) {
        espera_ms(10);
        if !esperar(150, || self.rd(STATUS) & 1 << 9 != 0) {
            traza!("NF intel: la PHY no avisa de configuracion terminada");
        }
        self.wr(STATUS, self.rd(STATUS) & !(1 << 9));
        if self.rd(STATUS) & 1 << 10 != 0 {
            self.wr(STATUS, self.rd(STATUS) & !(1 << 10));
        }
    }

    // ---------- F5: PHY accesible ----------

    /// Conmuta LANPHYPC para dejar la PHY en estado limpio.
    fn lanphypc(&self) {
        self.wr(FEXTNVM3, self.rd(FEXTNVM3) & !0x0C00_0000 | 0x0800_0000);
        self.wr(CTRL, self.rd(CTRL) & !(1 << 17) | 1 << 16);
        self.rd(STATUS);
        espera_ms(1);
        self.wr(CTRL, self.rd(CTRL) & !(1 << 16));
        self.rd(STATUS);
        if !esperar(120, || self.rd(CTRL_EXT) & 1 << 2 != 0) {
            traza!("NF intel: LANPHYPC sin LPCD");
        }
        espera_ms(30);
    }

    /// Sale del modo de bajo consumo (ULP) forzándolo, con o sin firmware que lo haga.
    fn salir_ulp(&self) {
        if self.rd(FWSM) & 1 << 15 != 0 {
            self.wr(H2ME, self.rd(H2ME) & !0x800 | 0x1000);
            if !esperar(2500, || self.rd(FWSM) & 1 << 10 == 0) {
                traza!("NF intel: el firmware no termina de sacar la PHY de ULP");
            }
            self.wr(H2ME, self.rd(H2ME) & !0x1000);
            return;
        }
        if !self.bandera() {
            traza!("NF intel: sin bandera de PHY para salir de ULP");
            return;
        }
        if !self.phy_bloqueada() {
            self.lanphypc();
        }
        self.reint.set(false); // al cambiar de interfaz el primer acceso falla
        let mut smb = self.phy(769, 23, None);
        if smb.is_none() {
            self.wr(CTRL_EXT, self.rd(CTRL_EXT) | 1 << 11);
            espera_ms(50);
            smb = self.phy(769, 23, None);
        }
        if let Some(v) = smb {
            self.phy(769, 23, Some(v & !1));
        }
        self.reint.set(true);
        self.wr(CTRL_EXT, self.rd(CTRL_EXT) & !(1 << 11));
        self.pg_rmw(770, 17, 0x4000, 0); // K1 habilitado
        if let Some(v) = self.phy(779, 16, None) {
            let v = v & !0x1D74;
            self.phy(779, 16, Some(v));
            self.phy(779, 16, Some(v | 1));
        }
        self.wr(FEXTNVM7, self.rd(FEXTNVM7) & !0x20);
        self.bandera_soltar();
        self.phy_reset();
        espera_ms(50);
    }

    /// ¿Responde la PHY con un identificador válido? (con la bandera tomada). Si lee 0 o todo unos,
    /// activa el modo MDIO lento y repite.
    fn id_ok(&self) -> bool {
        for _ in 0..2 {
            let (a, b) = (self.phy(0, 2, None), self.phy(0, 3, None));
            let (Some(a), Some(b)) = (a, b) else { return false };
            let id = (a as u32) << 16 | (b & 0xFFF0) as u32;
            if id == 0 || id == 0xFFFF_FFF0 {
                self.pg_rmw(769, 16, 1 << 10, 0);
                continue;
            }
            if id != 0x0154_00A0 {
                traza!("NF intel: PHY con identificador {:08x}, se sigue igualmente", id); // duda: otras PHY de generaciones nuevas
            }
            return true;
        }
        false
    }

    /// Deja la PHY respondiendo (F5 «garantizar que la PHY responde»).
    fn phy_accesible(&self) -> bool {
        if !self.bandera() {
            return false;
        }
        self.reint.set(false);
        let mut ok = self.id_ok();
        if !ok {
            self.wr(CTRL_EXT, self.rd(CTRL_EXT) | 1 << 11);
            espera_ms(50);
            ok = self.id_ok();
        }
        if !ok {
            if self.rd(FWSM) & 0x40 != 0 {
                self.lanphypc();
            } else {
                self.wr(CTRL_EXT, self.rd(CTRL_EXT) & !(1 << 11));
            }
            ok = self.id_ok();
        }
        if ok && self.rd(FWSM) & 1 << 15 == 0 {
            if let Some(v) = self.phy(769, 23, None) {
                self.phy(769, 23, Some(v & !1));
            }
            self.wr(CTRL_EXT, self.rd(CTRL_EXT) & !(1 << 11));
        }
        self.reint.set(true);
        self.bandera_soltar();
        if !ok {
            return false;
        }
        self.phy_reset();
        if self.gen() >= MTP && self.bandera() {
            self.k1_mtp();
            self.bandera_soltar();
        }
        true
    }

    /// F2 punto 4: sin ASPM en el enlace PCIe del dispositivo (capacidad PCIe, control de enlace).
    fn sin_aspm(&self) {
        let (b, d, f) = self.bdf;
        let cap = pci::capacidades(b, d, f).into_iter().find(|&c| c <= 0xEC && pci::leer(b, d, f, c) & 0xFF == 0x10);
        if let Some(c) = cap {
            // La mitad alta (estado del enlace) se escribe a 0 para no borrar bits que se borran escribiendo 1.
            pci::escribir(b, d, f, c + 0x10, pci::leer(b, d, f, c + 0x10) & 0xFFFC);
        }
    }

    /// Antes del reinicio: sin ASPM, configuración hardware de PHY bloqueada, fuera de ULP y PHY accesible.
    pub(super) fn pch_antes(&self) -> bool {
        self.sin_aspm();
        self.wr(EXTCNF_CTRL, self.rd(EXTCNF_CTRL) | 0x80);
        if self.con_ulp() {
            self.salir_ulp();
        }
        self.phy_accesible()
    }

    // ---------- F4: reinicio ----------

    /// E11 (SPT y posteriores): vaciado de anillos antes de reiniciar, con nuestros propios anillos
    /// (los que dejó el firmware pueden estar en memoria que ya no es suya). El RX queda sin descriptores.
    fn vaciar(&self) {
        self.wr(FEXTNVM11, self.rd(FEXTNVM11) | 0x2000);
        let (b, d, f) = self.bdf;
        let pendiente = || self.rd(TDLEN0) != 0 && pci::leer(b, d, f, 0xE4) & 0x100 != 0;
        if !pendiente() {
            return;
        }
        wr64(self.r + TDBAL0, self.tx.desc.dir());
        self.wr(TDLEN0, (16 * N) as u32);
        self.wr(TDH0, 0);
        self.wr(TDT0, 0);
        let desc = self.tx.d(0);
        wr64(desc, self.tx.desc.dir());
        wr::<u32>(desc + 8, 512 | 2 << 24); // 512 bytes, IFCS
        wr::<u32>(desc + 12, 0);
        self.wr(TCTL, self.rd(TCTL) | 1 << 1);
        fence(SeqCst);
        self.wr(TDT0, 1);
        us(250);
        if pendiente() {
            self.wr(RCTL, 0);
            us(150);
            wr64(self.r + RDBAL0, self.rx.desc.dir());
            self.wr(RDBAL0 + 8, (16 * N) as u32);
            self.wr(RDH0, 0);
            self.wr(RDT0, 0);
            self.wr(RXDCTL0, self.rd(RXDCTL0) & !0x3FFF | 0x1F | 1 << 8 | 1 << 24);
            self.wr(RCTL, 1 << 1);
            us(150);
            self.wr(RCTL, 0);
        }
    }

    /// Reinicio completo del MAC (y de la PHY si el firmware lo permite).
    pub(super) fn pch_reset(&self) {
        let g = self.gen();
        self.wr(CTRL, self.rd(CTRL) | CTRL_MASTER_OFF);
        esperar(100, || self.rd(STATUS) & STATUS_MASTER_ON == 0); // sin respuesta no es fatal
        self.wr(IMC_A, !0);
        self.wr(RCTL, 0);
        self.wr(TCTL, 1 << 3);
        self.rd(STATUS);
        espera_ms(11);
        if g >= SPT {
            self.vaciar();
        }
        let bloq = self.phy_bloqueada();
        let tomada = self.bandera();
        // Sin leer justo después: en esta generación puede colgar el hardware.
        self.wr(CTRL, self.rd(CTRL) | CTRL_RST | if bloq { 0 } else { PHY_RST });
        espera_ms(20);
        if tomada {
            self.bandera_soltar();
        }
        if !bloq {
            self.config_hecha();
            if self.bandera() {
                self.pg_rmw(769, 17, 0, 0x10); // borra «despertar de host»
                self.bandera_soltar();
            }
        }
        self.wr(IMC_A, !0);
        self.rd(ICR_A);
        self.wr(KABGTXD, self.rd(KABGTXD) | 0x0005_0000);
        if g >= PTP {
            self.wr(CTRL_EXT, self.rd(CTRL_EXT) & !(1 << 3)); // E17: DPG_EN
        }
        if g >= MTP && self.bandera() {
            self.k1_mtp();
            self.bandera_soltar();
        }
    }

    /// K1 en MTP y posteriores (E16): tras cada reinicio. Con la bandera tomada.
    fn k1_mtp(&self) -> Option<()> {
        self.wr(FEXTNVM12, self.rd(FEXTNVM12) & !0x00C0_0000 | 0x0080_0000);
        espera_ms(1);
        self.pg_rmw(770, 21, 0x0F00, 0x0FC0)
    }

    // ---------- F12: configuración global ----------

    /// Todo lo que vuelve a su valor por defecto en cada reinicio; con los anillos aún sin activar.
    pub(super) fn pch_config(&self) {
        let g = self.gen();
        self.wr(CTRL_EXT, self.rd(CTRL_EXT) | 1 << 22 | 1 << 20);
        self.wr(CTRL, self.rd(CTRL) | 1 << 19); // MEHE
        self.wr(PBECCSTS, self.rd(PBECCSTS) | 1 << 16);
        for q in [TXDCTL0, TXDCTL1] {
            let v = self.rd(q) | 1 << 22;
            self.wr(q, v & !(0x3F << 16) & !0x3F | 1 << 24 | 1 << 16 | 0x1F);
        }
        self.wr(TARC0, self.rd(TARC0) | 1 << 23 | 1 << 24 | 1 << 26 | 1 << 27);
        self.wr(TARC1, self.rd(TARC1) | 1 << 24 | 1 << 26 | 1 << 30 | 1 << 28); // MULR está a 0
        if g == SPT {
            self.wr(TARC0, self.rd(TARC0) & !(3 << 28) | 1 << 29); // E14
            self.wr(IOSFPC, self.rd(IOSFPC) | 1 << 16);
        }
        if g >= TGP {
            self.wr(FFLT_DBG, self.rd(FFLT_DBG) | 0x1000); // E15
        }
        self.wr(FEXTNVM7, self.rd(FEXTNVM7) | 4);
        if g >= SPT {
            self.wr(FEXTNVM9, self.rd(FEXTNVM9) | 0x1800); // sin MSI-X
        }
        self.wr(PBA, 26);
        self.wr(VET, 0x8100);
        self.wr(RXCSUM, 0);
        self.wr(WUC, 0);
        self.wr(FCRTL, 0);
        self.wr(FCRTH, 0);
        self.wr(RFCTL, 1 << 15 | 1 << 6 | 1 << 7); // EXTEN, sin filtrado NFS
    }

    // ---------- F8: enlace ----------

    /// Anuncia 10/100/1000 sin pausa, MDI-X automático y bajada automática de velocidad, y reinicia la
    /// autonegociación. Después libera la configuración hardware de PHY (duda D5) y cierra F12.
    pub(super) fn pch_enlace_inicio(&self) {
        let ok = self.bandera() && {
            let r = self
                .pg_rmw(0, 22, 0x8C00, 0)
                .and_then(|_| self.pg_rmw(0, 18, 0x0400, 0x0600))
                .and_then(|_| self.pg_rmw(0, 9, 0x0200, 0x1000))
                .and_then(|_| self.pg_rmw(0, 4, 0x01E0, 0x0C00))
                .and_then(|_| self.pg_rmw(0, 0, 0x1200, 0));
            self.bandera_soltar();
            r.is_some()
        };
        if !ok {
            traza!("NF intel: {} no se pudo reiniciar la autonegociacion", self.nombre);
        }
        self.wr(EXTCNF_CTRL, self.rd(EXTCNF_CTRL) & !0x80);
        self.wr(CTRL_EXT, self.rd(CTRL_EXT) | 1 << 17); // RO_DIS, al final de la configuración
    }

    // ---------- F9: ajustes en cada transición a «enlace arriba» ----------

    pub(super) fn pch_enlace(&self, mbps: u32, full: bool) {
        let g = self.gen();
        let hd10 = mbps == 10 && !full;
        let ipgt = if hd10 {
            0xFF
        } else if full && g >= SPT && mbps != 1000 {
            0x0C
        } else {
            8
        };
        self.wr(TIPG, self.rd(TIPG) & !0x3FF | ipgt);
        self.wr(FEXTNVM4, self.rd(FEXTNVM4) & !7 | 7);
        // shortcut: sin el LTR de plataforma (LTRV, F9 punto 7) ni FEXTNVM6.31 de SPT; la
        // especificación los da por no esenciales. Añadirlos si hay pérdidas o consumo alto en hardware real.
        if !self.bandera() {
            traza!("NF intel: sin bandera de PHY para los ajustes de enlace");
            return;
        }
        let r = (|| {
            self.emi(0xB20C, !hd10 as u16)?;
            let pll = if mbps < 1000 {
                0x3E8
            } else if g >= MTP {
                0x1D5
            } else {
                0x0FA
            };
            self.pg_rmw(772, 28, pll, 0x7FF)?;
            if mbps == 1000 {
                self.pg_rmw(770, 17, 0x200, 0)?;
            }
            if g >= SPT {
                if mbps == 1000 {
                    let v = self.phy(776, 20, None)?;
                    if v >> 2 & 0x3FF < 0x18 {
                        self.phy(776, 20, Some(v & !0xFFC | 0x18 << 2))?;
                    }
                } else {
                    self.phy(776, 20, Some(0xC023))?;
                }
            }
            if g >= MTP {
                self.k1_mtp()?;
            }
            if self.lpt_lp() {
                self.k1_lpt(mbps, full)?;
            }
            Some(())
        })();
        self.bandera_soltar();
        if r.is_none() {
            traza!("NF intel: fallo en los ajustes de enlace de la PHY");
        }
    }

    /// E13: arreglo de K1 del I218 de LPT-LP. Con la bandera tomada.
    fn k1_lpt(&self, mbps: u32, full: bool) -> Option<()> {
        if mbps == 1000 {
            let k = self.kmrn_rd(7);
            self.kmrn_wr(7, k & !2);
            us(20);
            self.wr(FEXTNVM6, self.rd(FEXTNVM6) | 0x100);
            self.kmrn_wr(7, k);
            return Some(());
        }
        self.wr(FEXTNVM6, self.rd(FEXTNVM6) & !0x100);
        if self.phy(0, 3, None)? & 0xF <= 5 && !(mbps == 100 && full) {
            let t = if mbps == 100 { 5 } else { 50 };
            self.pg_rmw(770, 18, t << 8, 0x3F00)?;
            self.wr(FEXTNVM6, self.rd(FEXTNVM6) & !0x200 | if mbps == 10 { 0x200 } else { 0 });
        }
        Some(())
    }

    /// Enlace caído: en el I218 de LPT-LP, suelta la petición de reloj del PLL.
    pub(super) fn pch_sin_enlace(&self) {
        if self.lpt_lp() {
            self.wr(FEXTNVM6, self.rd(FEXTNVM6) & !0x100);
        }
    }
}
