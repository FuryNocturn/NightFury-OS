// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Controlador NVMe por sondeo, sin interrupciones. Una cola de administración y una pareja de colas
//! de E/S, con una sola orden en vuelo. Hecho con la NVM Express Base Specification y el NVM Command
//! Set públicos. Cada espacio de nombres útil (sectores de 512 o 4096 bytes) sale como un `Espacio`.
use crate::serial::traza;
use crate::usb::{esperar, Dma};
use crate::{paging, pci};
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering::SeqCst};
use nf_core::block::DiskError;

// Registros del controlador
const CC: usize = 0x14;
const CSTS: usize = 0x1C;
const AQA: usize = 0x24;
const ASQ: usize = 0x28;
const ACQ: usize = 0x30;
const TIMBRES: usize = 0x1000;

const CSTS_RDY: u32 = 1;
const CSTS_CFS: u32 = 2;

/// Plazos (ms) de las órdenes de administración y de E/S.
const ESPERA_ADMIN: u64 = 5_000;
const ESPERA_ES: u64 = 10_000;
/// Mayor transferencia por orden: 32 páginas, que caben en una sola lista de PRP.
const MAX_TRANSF: usize = 32 * 4096;

fn rd32(a: usize) -> u32 {
    // MMIO del controlador, mapeado por `sondear`.
    unsafe { read_volatile(a as *const u32) }
}
fn wr32(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}
fn rd64(a: usize) -> u64 {
    rd32(a) as u64 | (rd32(a + 4) as u64) << 32
}
fn wr64(a: usize, v: u64) {
    wr32(a, v as u32);
    wr32(a + 4, (v >> 32) as u32);
}

/// Texto de Identify: ASCII relleno de espacios.
fn texto(b: &[u8]) -> String {
    let s: String = b.iter().map(|&c| if (0x20..0x7F).contains(&c) { c as char } else { ' ' }).collect();
    String::from(s.trim())
}

/// CC.EN = 0 y espera a que el controlador lo confirme. `false` si no se detiene en el plazo.
fn apagar(regs: usize, ms: u64) -> bool {
    wr32(regs + CC, 0);
    esperar(ms, || rd32(regs + CSTS) & CSTS_RDY == 0)
}

struct Ctrl {
    regs: usize,
    dstrd: usize,
    /// Tras un fallo el controlador se apaga y no se vuelve a usar: así ya no puede escribir en memoria.
    roto: bool,
    cid: u16,
    /// Índice 0 = administración, 1 = E/S.
    prof: [u16; 2],
    tail: [u16; 2],
    head: [u16; 2],
    fase: [u32; 2],
    sq: [Dma; 2],
    cq: [Dma; 2],
    /// Búfer de datos (y de Identify) y página con la lista de PRP.
    datos: Dma,
    lista: Dma,
    max: usize,
}

impl Ctrl {
    fn timbre(&self, n: usize, v: u16) {
        wr32(self.regs + TIMBRES + n * (4 << self.dstrd), v as u32);
    }

    fn parar(&mut self) {
        self.roto = true;
        if !apagar(self.regs, 2_000) {
            traza!("NF nvme: el controlador no se detiene");
        }
    }

    /// Envía una orden por la cola `q` y espera su finalización. Devuelve el DW0 del resultado.
    fn orden(&mut self, q: usize, mut c: [u32; 16], ms: u64) -> Result<u32, DiskError> {
        if self.roto {
            return Err(DiskError::Io);
        }
        self.cid = self.cid.wrapping_add(1);
        c[0] = c[0] & 0xFFFF | (self.cid as u32) << 16;
        // Las colas son memoria nuestra de `prof[q]` entradas y los índices se mantienen por debajo.
        unsafe {
            let p = self.sq[q].ptr::<u32>().add(self.tail[q] as usize * 16);
            for (i, w) in c.iter().enumerate() {
                write_volatile(p.add(i), *w);
            }
        }
        self.tail[q] = (self.tail[q] + 1) % self.prof[q];
        fence(SeqCst);
        self.timbre(2 * q, self.tail[q]);
        let e = unsafe { self.cq[q].ptr::<u32>().add(self.head[q] as usize * 4) };
        let (fase, regs) = (self.fase[q], self.regs);
        let hecho = esperar(ms, || {
            // Entrada de la cola de finalización (memoria que escribe el controlador).
            (unsafe { read_volatile(e.add(3)) } >> 16) & 1 == fase || rd32(regs + CSTS) & CSTS_CFS != 0
        });
        fence(SeqCst);
        let dw3 = unsafe { read_volatile(e.add(3)) };
        if !hecho || (dw3 >> 16) & 1 != fase {
            traza!("NF nvme: orden {:#04x} sin respuesta o con fallo del controlador (CSTS {:#x})", c[0] & 0xFF, rd32(regs + CSTS));
            self.parar();
            return Err(DiskError::Io);
        }
        let dw0 = unsafe { read_volatile(e) };
        self.head[q] += 1;
        if self.head[q] == self.prof[q] {
            self.head[q] = 0;
            self.fase[q] ^= 1;
        }
        self.timbre(2 * q + 1, self.head[q]);
        if dw3 & 0xFFFF != self.cid as u32 {
            traza!("NF nvme: finalización de otra orden ({} en vez de {})", dw3 & 0xFFFF, self.cid);
            self.parar();
            return Err(DiskError::Io);
        }
        let estado = dw3 >> 17;
        if estado != 0 {
            traza!("NF nvme: orden {:#04x} rechazada: tipo {} código {:#04x}", c[0] & 0xFF, estado >> 8 & 7, estado & 0xFF);
            return Err(DiskError::Io);
        }
        Ok(dw0)
    }

    /// Identify (CNS `cns`) hacia el búfer de datos.
    fn identificar(&mut self, cns: u32, nsid: u32) -> Result<(), DiskError> {
        unsafe { core::ptr::write_bytes(self.datos.ptr::<u8>(), 0, 4096) };
        let mut c = [0u32; 16];
        c[0] = 0x06;
        c[1] = nsid;
        let d = self.datos.dir();
        (c[6], c[7]) = (d as u32, (d >> 32) as u32);
        c[10] = cns;
        self.orden(0, c, ESPERA_ADMIN).map(|_| ())
    }

    fn ident(&self) -> &[u8] {
        // 4096 bytes del búfer de datos, que es nuestro.
        unsafe { core::slice::from_raw_parts(self.datos.ptr::<u8>(), 4096) }
    }

    /// Lee o escribe `buf.len()` bytes (múltiplo del sector) desde `lba` con las órdenes que hagan falta.
    fn transferir(&mut self, nsid: u32, ss: usize, lba: u64, buf: &mut [u8], escribe: bool) -> Result<(), DiskError> {
        let mut l = lba;
        for c in buf.chunks_mut(self.max) {
            let paginas = c.len().div_ceil(4096);
            if escribe {
                unsafe { core::ptr::copy_nonoverlapping(c.as_ptr(), self.datos.ptr::<u8>(), c.len()) };
            }
            let d = self.datos.dir();
            // PRP2: nada, la segunda página o una lista con las demás.
            let prp2 = match paginas {
                1 => 0,
                2 => d + 4096,
                n => {
                    for k in 1..n {
                        // La lista es una página nuestra y n <= 32.
                        unsafe { self.lista.ptr::<u64>().add(k - 1).write_volatile(d + 4096 * k as u64) };
                    }
                    self.lista.dir()
                }
            };
            let mut o = [0u32; 16];
            o[0] = if escribe { 0x01 } else { 0x02 };
            o[1] = nsid;
            (o[6], o[7]) = (d as u32, (d >> 32) as u32);
            (o[8], o[9]) = (prp2 as u32, (prp2 >> 32) as u32);
            (o[10], o[11]) = (l as u32, (l >> 32) as u32);
            o[12] = (c.len() / ss - 1) as u32;
            self.orden(1, o, ESPERA_ES)?;
            if !escribe {
                c.copy_from_slice(unsafe { core::slice::from_raw_parts(self.datos.ptr::<u8>(), c.len()) });
            }
            l += (c.len() / ss) as u64;
        }
        Ok(())
    }
}

/// Un espacio de nombres NVM utilizable.
pub struct Espacio {
    ctrl: Rc<RefCell<Ctrl>>,
    nsid: u32,
    ss: usize,
    sectores: u64,
    modelo: String,
    serie: String,
}

impl Espacio {
    pub fn modelo(&self) -> &str {
        &self.modelo
    }
    pub fn serie(&self) -> &str {
        &self.serie
    }
    pub fn sector_size(&self) -> usize {
        self.ss
    }
    pub fn sectores(&self) -> u64 {
        self.sectores
    }
    /// El llamador comprueba el rango y que `buf` sea múltiplo del sector.
    pub fn leer(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), DiskError> {
        self.ctrl.borrow_mut().transferir(self.nsid, self.ss, lba, buf, false)
    }
    pub fn escribir(&mut self, lba: u64, buf: &[u8]) -> Result<(), DiskError> {
        // `transferir` solo lee `buf` al escribir; la copia evita pedir `&mut` a quien llama.
        let mut t = Vec::from(buf);
        self.ctrl.borrow_mut().transferir(self.nsid, self.ss, lba, &mut t, true)
    }
    pub fn vaciar(&mut self) -> Result<(), DiskError> {
        let mut c = [0u32; 16];
        c[1] = self.nsid;
        self.ctrl.borrow_mut().orden(1, c, ESPERA_ES).map(|_| ())
    }
}

/// Inicia el controlador cuyos registros están en `regs` y devuelve sus espacios de nombres útiles.
fn iniciar(regs: usize) -> Option<Vec<Espacio>> {
    let cap = rd64(regs);
    let mqes = (cap & 0xFFFF) as usize + 1;
    let to = (cap >> 24) & 0xFF;
    let dstrd = (cap >> 32 & 0xF) as usize;
    if cap >> 37 & 1 == 0 || cap >> 48 & 0xF != 0 {
        traza!("NF nvme: sin conjunto NVM o con página mínima mayor de 4 KiB (CAP {:#x}), descartado", cap);
        return None;
    }
    let espera = ((to + 1) * 500).min(20_000);
    if !apagar(regs, espera) {
        traza!("NF nvme: no se reinicia, descartado");
        return None;
    }
    let (adm, io) = (mqes.min(16), mqes.min(64));
    let mem = |t| Dma::nuevo(t, 4096);
    let (Some(sq0), Some(cq0), Some(sq1), Some(cq1), Some(datos), Some(lista)) = (mem(4096), mem(4096), mem(4096), mem(4096), mem(MAX_TRANSF), mem(4096)) else {
        traza!("NF nvme: sin memoria, descartado");
        return None;
    };
    wr32(regs + AQA, (adm - 1) as u32 | ((adm - 1) as u32) << 16);
    wr64(regs + ASQ, sq0.dir());
    wr64(regs + ACQ, cq0.dir());
    // CSS = 0 (NVM), MPS = 0 (4 KiB), entradas de 64 bytes (2^6) y de 16 (2^4).
    wr32(regs + CC, 1 | 6 << 16 | 4 << 20);
    let listo = esperar(espera, || rd32(regs + CSTS) & (CSTS_RDY | CSTS_CFS) != 0);
    if !listo || rd32(regs + CSTS) != CSTS_RDY {
        traza!("NF nvme: no llega a estar listo (CSTS {:#x}), descartado", rd32(regs + CSTS));
        apagar(regs, 2_000);
        return None;
    }
    let mut c = Ctrl { regs, dstrd, roto: false, cid: 0, prof: [adm as u16, io as u16], tail: [0; 2], head: [0; 2], fase: [1; 2], sq: [sq0, sq1], cq: [cq0, cq1], datos, lista, max: MAX_TRANSF };
    let r = (|| {
        c.identificar(1, 0)?;
        let id = c.ident();
        let (serie, modelo, mdts, nn) = (texto(&id[4..24]), texto(&id[24..64]), id[77], u32::from_le_bytes(id[516..520].try_into().unwrap()));
        if mdts != 0 {
            c.max = c.max.min(4096usize << mdts.min(20));
        }
        // CQ de E/S (contigua, sin interrupciones) y SQ de E/S asociada a ella.
        let mut o = [0u32; 16];
        o[0] = 0x05;
        (o[6], o[7]) = (c.cq[1].dir() as u32, (c.cq[1].dir() >> 32) as u32);
        (o[10], o[11]) = ((io as u32 - 1) << 16 | 1, 1);
        c.orden(0, o, ESPERA_ADMIN)?;
        let mut o = [0u32; 16];
        o[0] = 0x01;
        (o[6], o[7]) = (c.sq[1].dir() as u32, (c.sq[1].dir() >> 32) as u32);
        (o[10], o[11]) = ((io as u32 - 1) << 16 | 1, 1 << 16 | 1);
        c.orden(0, o, ESPERA_ADMIN)?;
        // Espacios activos: la lista (CNS 2) o, si el controlador no la entiende, 1..=NN.
        let mut nsids: Vec<u32> = Vec::new();
        if c.identificar(2, 0).is_ok() {
            nsids.extend(c.ident().chunks_exact(4).map(|x| u32::from_le_bytes(x.try_into().unwrap())).take_while(|&n| n != 0));
        } else {
            nsids.extend(1..=nn.clamp(1, 32));
        }
        nsids.truncate(32);
        Ok::<_, DiskError>((serie, modelo, nsids))
    })();
    let Ok((serie, modelo, nsids)) = r else {
        traza!("NF nvme: no se puede preparar el controlador, descartado");
        c.parar();
        return None;
    };
    traza!("NF nvme: «{}» serie «{}», transferencia máxima {} KiB, {} espacio(s) activo(s)", modelo, serie, c.max >> 10, nsids.len());
    let ctrl = Rc::new(RefCell::new(c));
    let mut v = Vec::new();
    for nsid in nsids {
        let mut cb = ctrl.borrow_mut();
        if cb.identificar(0, nsid).is_err() {
            continue;
        }
        let id = cb.ident();
        let nsze = u64::from_le_bytes(id[0..8].try_into().unwrap());
        let (nlbaf, flbas, dps) = (id[25] as usize, id[26] as usize, id[29]);
        let idx = (flbas & 0xF) | ((flbas >> 5) & 3) << 4;
        if nsze == 0 || idx > nlbaf {
            continue;
        }
        let f = u32::from_le_bytes(id[128 + 4 * idx..132 + 4 * idx].try_into().unwrap());
        let (ms, lbads) = (f & 0xFFFF, f >> 16 & 0xFF);
        if lbads != 9 && lbads != 12 {
            traza!("NF nvme: espacio {}: sector de 2^{} bytes, no se usa", nsid, lbads);
            continue;
        }
        // shortcut: no se manejan metadatos intercalados ni protección de extremo a extremo, cuando haga falta un disco así.
        if ms != 0 && (flbas & 0x10 != 0 || dps & 7 != 0) {
            traza!("NF nvme: espacio {}: metadatos intercalados o protección de extremo a extremo, no se usa", nsid);
            continue;
        }
        drop(cb);
        v.push(Espacio { ctrl: ctrl.clone(), nsid, ss: 1 << lbads, sectores: nsze, modelo: modelo.clone(), serie: serie.clone() });
    }
    Some(v)
}

/// Todos los controladores NVMe (clase 01h/08h/02h) y sus espacios de nombres útiles.
pub fn sondear() -> Vec<Espacio> {
    let mut v = Vec::new();
    for (bus, dev, fun) in pci::buscar(0x01_08_02) {
        let id = pci::leer(bus, dev, fun, 0);
        let b0 = pci::leer(bus, dev, fun, 0x10);
        let mut base = (b0 & !0xF) as u64;
        if (b0 >> 1) & 3 == 2 {
            base |= (pci::leer(bus, dev, fun, 0x14) as u64) << 32;
        }
        // Registros y timbres caben en 64 KiB; el mapeo sin caché va por páginas de 2 MiB.
        if b0 & 1 != 0 || base == 0 || base >= 1 << 46 || !paging::mapear_mmio(base, 0x1_0000) {
            traza!("NF nvme: {:02x}:{:02x}.{} {:04x}:{:04x} BAR0 inutilizable ({:#x}), se salta", bus, dev, fun, id & 0xFFFF, id >> 16, base);
            continue;
        }
        let cmd = pci::leer(bus, dev, fun, 0x04);
        pci::escribir(bus, dev, fun, 0x04, cmd | 0b110);
        match iniciar(base as usize) {
            Some(e) => {
                traza!("NF nvme: {:02x}:{:02x}.{} {:04x}:{:04x} base={:#x}: {} espacio(s) de nombres", bus, dev, fun, id & 0xFFFF, id >> 16, base, e.len());
                v.extend(e);
            }
            None => traza!("NF nvme: {:02x}:{:02x}.{} {:04x}:{:04x} no arranca", bus, dev, fun, id & 0xFFFF, id >> 16),
        }
    }
    v
}
