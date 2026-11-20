// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Controlador xHCI (especificación xHCI 1.2): anillos, órdenes, eventos, ranuras, contextos y
//! transferencias, por sondeo y con tiempos de espera. No sabe nada de clases de dispositivo.
use super::{esperar, espera_ms, Dma, Plazo, Uso};
use crate::serial::traza;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering::SeqCst};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Error {
    Tiempo,
    /// Código de finalización del xHCI distinto de éxito.
    Codigo(u8),
    Memoria,
    Datos,
    /// El dispositivo ya no existe.
    Ausente,
}

type R<T> = Result<T, Error>;

// PORTSC
pub const CCS: u32 = 1;
pub const PED: u32 = 1 << 1;
const PR: u32 = 1 << 4;
const PP: u32 = 1 << 9;
/// Bits de cambio (CSC..CEC): se borran escribiendo 1.
const CAMBIOS: u32 = 0x00FE_0000;
pub const CSC: u32 = 1 << 17;
const PRC: u32 = 1 << 21;
const WRC: u32 = 1 << 19;
/// Campos de lectura/escritura que hay que conservar al escribir PORTSC (PP, PIC, despertar).
const PRESERVA: u32 = 0x0E00_C200;

/// Anillo de TRB de un solo segmento; la última posición es un Link que vuelve al principio.
pub struct Anillo {
    m: Dma,
    n: usize,
    i: usize,
    ciclo: u32,
}

impl Anillo {
    fn de(m: Dma, n: usize) -> Anillo {
        Anillo { m, n, i: 0, ciclo: 1 }
    }
    fn escribe(&self, i: usize, t: [u32; 4]) {
        let p = unsafe { self.m.ptr::<u32>().add(i * 4) };
        // El dueño del TRB es el controlador cuando coincide el bit de ciclo: se escribe el último.
        unsafe {
            write_volatile(p, t[0]);
            write_volatile(p.add(1), t[1]);
            write_volatile(p.add(2), t[2]);
            fence(SeqCst);
            write_volatile(p.add(3), t[3]);
        }
    }
    /// Añade un TRB y devuelve su dirección.
    fn push(&mut self, mut t: [u32; 4]) -> u64 {
        let a = self.pos();
        t[3] = t[3] & !1 | self.ciclo;
        self.escribe(self.i, t);
        self.i += 1;
        if self.i == self.n - 1 {
            let d = self.m.dir();
            self.escribe(self.i, [d as u32, (d >> 32) as u32, 0, 6 << 10 | 1 << 1 | self.ciclo]);
            self.i = 0;
            self.ciclo ^= 1;
        }
        a
    }
    fn pos(&self) -> u64 {
        self.m.dir() + self.i as u64 * 16
    }
}

pub struct Ep {
    anillo: Anillo,
    pub mps: u16,
    /// Búfer propio de los endpoints de interrupción.
    pub buf: Option<Dma>,
    /// Último TRB del TD en curso (el que da el evento de fin).
    ult: u64,
    hecho: Option<(u8, u32)>,
    /// TD de control: un paquete corto en la fase de datos no termina la transferencia.
    ctl: bool,
    res: u32,
    pedido: u32,
    pub errores: u8,
    pub uso: Uso,
}

/// Lo que hace falta para crear el contexto de un endpoint.
pub struct CfgEp {
    pub dci: u8,
    /// Tipo de endpoint del contexto: 2 bulk OUT, 6 bulk IN, 3 interrupción OUT, 7 interrupción IN.
    pub tipo: u8,
    pub mps: u16,
    /// Ya en la forma del contexto (2^n × 125 µs).
    pub intervalo: u8,
    pub rafaga: u8,
}

pub struct Dispositivo {
    pub slot: u8,
    pub vel: u8,
    pub raiz: u8,
    pub ruta: u32,
    /// Hubs por encima (0 si cuelga de la raíz): también es el nivel de su ruta.
    pub prof: u8,
    pub padre: Option<(u8, u8)>,
    pub tt: Option<(u8, u8)>,
    /// Puertos, si es un hub.
    pub puertos: u8,
    salida: Dma,
    entrada: Dma,
    pub eps: [Option<Ep>; 31],
}

/// Dónde y cómo cuelga un dispositivo nuevo.
pub struct Origen {
    pub raiz: u8,
    pub ruta: u32,
    pub vel: u8,
    pub prof: u8,
    pub padre: Option<(u8, u8)>,
    pub tt: Option<(u8, u8)>,
}

pub struct Xhci {
    pub id: usize,
    op: usize,
    rt: usize,
    db: usize,
    /// Tamaño de un contexto: 32 o 64 bytes.
    cs: usize,
    ac64: bool,
    pub npuertos: u8,
    usb3: [bool; 256],
    dcbaa: Dma,
    _esc: Vec<Dma>,
    cmd: Anillo,
    cmd_pend: u64,
    cmd_res: Option<(u8, u8)>,
    ev: Dma,
    _erst: Dma,
    ev_i: usize,
    ev_ciclo: u32,
    /// Puertos con cambio de estado pendientes de atender.
    pub psc: Vec<u8>,
    pub devs: Vec<Option<Box<Dispositivo>>>,
    /// Ranura del dispositivo conectado a cada puerto raíz (0 = ninguno).
    pub puerto_slot: [u8; 256],
    /// Enumeraciones fallidas seguidas por puerto raíz.
    pub intentos: [u8; 256],
    rebote: Dma,
}

fn r32(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn w32(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}
fn w64(a: usize, v: u64) {
    w32(a, v as u32);
    w32(a + 4, (v >> 32) as u32);
}

fn trb_cmd(tipo: u32, ptr: u64, slot: u8, extra: u32) -> [u32; 4] {
    [ptr as u32, (ptr >> 32) as u32, 0, tipo << 10 | extra | (slot as u32) << 24]
}

impl Xhci {
    /// Traspaso desde la BIOS, reset y puesta en marcha. `None` (con traza) si no sirve.
    pub fn nuevo(id: usize, base: u64) -> Option<Xhci> {
        let b = base as usize;
        let (hcs1, hcs2, hcc) = (r32(b + 4), r32(b + 8), r32(b + 0x10));
        let op = b + (r32(b) & 0xFF) as usize;
        let ac64 = hcc & 1 != 0;
        let usb3 = capacidades(b, hcc);
        // Reset: parar, esperar a que pare, HCRST, esperar a que termine y a que esté listo (CNR).
        if r32(op + 4) & 1 == 0 {
            w32(op, r32(op) & !1);
            if !esperar(200, || r32(op + 4) & 1 != 0) {
                traza!("NF usb: controlador {}: no se detiene", id);
                return None;
            }
        }
        w32(op, r32(op) | 2);
        if !esperar(1000, || r32(op) & 2 == 0 && r32(op + 4) & (1 << 11) == 0) {
            traza!("NF usb: controlador {}: el reset no termina", id);
            return None;
        }
        let nslots = (hcs1 & 0xFF) as u8;
        let npuertos = (hcs1 >> 24) as u8;
        if nslots == 0 || npuertos == 0 || r32(op + 8) & 1 == 0 {
            traza!("NF usb: controlador {}: parámetros no soportados (ranuras {} puertos {})", id, nslots, npuertos);
            return None;
        }
        // Memoria: por debajo de 4 GiB si el controlador no es de 64 bits.
        let mem = |t: usize, a: usize| Dma::nuevo(t, a).filter(|d| ac64 || d.dir() >> 32 == 0);
        let dcbaa = mem(4096, 4096)?;
        let mut esc = Vec::new();
        let nesc = ((hcs2 >> 21) & 0x1F) << 5 | (hcs2 >> 27) & 0x1F;
        if nesc > 0 {
            let lista = mem(nesc as usize * 8, 4096)?;
            for i in 0..nesc as usize {
                let p = mem(4096, 4096)?;
                unsafe { write_volatile(lista.ptr::<u64>().add(i), p.dir()) };
                esc.push(p);
            }
            unsafe { write_volatile(dcbaa.ptr::<u64>(), lista.dir()) };
            esc.push(lista);
        }
        let cmd = Anillo::de(mem(256 * 16, 4096)?, 256);
        let ev = mem(256 * 16, 4096)?;
        let erst = mem(16, 64)?;
        unsafe {
            write_volatile(erst.ptr::<u64>(), ev.dir());
            write_volatile(erst.ptr::<u32>().add(2), 256);
        }
        let rt = b + (r32(b + 0x18) & !0x1F) as usize;
        // Programación: ranuras, DCBAA, anillo de órdenes y el único interruptor (sondeado, sin MSI).
        w32(op + 0x38, nslots as u32);
        w64(op + 0x30, dcbaa.dir());
        w64(op + 0x18, cmd.m.dir() | 1);
        w32(rt + 0x28, 1);
        w64(rt + 0x38, ev.dir());
        w64(rt + 0x30, erst.dir());
        w32(rt + 0x20, 2);
        let x = Xhci {
            id,
            op,
            rt,
            db: b + (r32(b + 0x14) & !3) as usize,
            cs: if hcc & 4 != 0 { 64 } else { 32 },
            ac64,
            npuertos,
            usb3,
            dcbaa,
            _esc: esc,
            cmd,
            cmd_pend: 0,
            cmd_res: None,
            ev,
            _erst: erst,
            ev_i: 0,
            ev_ciclo: 1,
            psc: Vec::new(),
            devs: (0..=nslots as usize).map(|_| None).collect(),
            puerto_slot: [0; 256],
            intentos: [0; 256],
            rebote: mem(4096, 4096)?,
        };
        w32(op, r32(op) | 1);
        if !esperar(200, || r32(op + 4) & 1 == 0) {
            traza!("NF usb: controlador {}: no arranca", id);
            return None;
        }
        for p in 1..=npuertos {
            if r32(x.pa(p)) & PP == 0 {
                x.pw(p, PP);
            }
        }
        traza!("NF usb: controlador {}: v{:x}.{:02x} {} puertos {} ranuras ({} USB3) contextos de {} bytes", id, r32(b) >> 24, r32(b) >> 16 & 0xFF, npuertos, nslots, x.usb3.iter().filter(|&&u| u).count(), x.cs);
        Some(x)
    }

    /// Memoria para DMA que el controlador alcanza (por debajo de 4 GiB si no es de 64 bits).
    pub fn mem(&self, t: usize, a: usize) -> R<Dma> {
        Dma::nuevo(t, a).filter(|d| self.ac64 || d.dir() >> 32 == 0).ok_or(Error::Memoria)
    }

    // ---------- Puertos raíz ----------

    fn pa(&self, p: u8) -> usize {
        self.op + 0x400 + (p as usize - 1) * 0x10
    }
    pub fn portsc(&self, p: u8) -> u32 {
        r32(self.pa(p))
    }
    /// Escribe PORTSC conservando sus campos de configuración y sin tocar los bits de "borrar con 1".
    fn pw(&self, p: u8, v: u32) {
        let a = self.pa(p);
        w32(a, r32(a) & PRESERVA | v);
    }
    /// Borra los bits de cambio de un puerto raíz y devuelve el PORTSC leído antes.
    pub fn ack_puerto(&self, p: u8) -> u32 {
        let sc = self.portsc(p);
        self.pw(p, CAMBIOS);
        sc
    }

    /// Reset del puerto raíz. `Some(velocidad)` si queda habilitado (códigos de velocidad del xHCI:
    /// 1 completa, 2 baja, 3 alta, 4 super).
    pub fn reset_puerto(&mut self, p: u8) -> Option<u8> {
        let sc = self.portsc(p);
        if sc & CCS == 0 {
            return None;
        }
        // Un puerto USB 3 que ya entrenó el enlace (U0) está habilitado sin reset.
        if !(self.usb3[p as usize] && sc & PED != 0 && (sc >> 5) & 0xF == 0) {
            if !self.usb3[p as usize] {
                espera_ms(100); // estabilización de la conexión USB 2
                if self.portsc(p) & CCS == 0 {
                    return None;
                }
            }
            self.pw(p, PR);
            if !esperar(500, || self.portsc(p) & (PRC | WRC) != 0) {
                return None;
            }
            self.pw(p, CAMBIOS);
            espera_ms(10);
        }
        let sc = self.portsc(p);
        (sc & (CCS | PED) == CCS | PED).then_some((sc >> 10 & 0xF) as u8)
    }

    // ---------- Anillo de eventos ----------

    fn timbre(&self, slot: u8, dest: u32) {
        w32(self.db + slot as usize * 4, dest);
    }

    /// Procesa todos los eventos pendientes.
    pub fn eventos(&mut self) {
        let mut n = 0;
        while n < 512 {
            let p = unsafe { self.ev.ptr::<u32>().add(self.ev_i * 4) };
            let d3 = unsafe { read_volatile(p.add(3)) };
            if d3 & 1 != self.ev_ciclo {
                break;
            }
            fence(SeqCst);
            let t = unsafe { [read_volatile(p), read_volatile(p.add(1)), read_volatile(p.add(2)), d3] };
            self.evento(t);
            self.ev_i += 1;
            if self.ev_i == 256 {
                self.ev_i = 0;
                self.ev_ciclo ^= 1;
            }
            n += 1;
        }
        if n > 0 {
            // Nueva posición de lectura; el bit 3 (EHB) se borra escribiendo 1.
            w64(self.rt + 0x38, self.ev.dir() + self.ev_i as u64 * 16 | 8);
        }
    }

    fn evento(&mut self, t: [u32; 4]) {
        let ptr = t[0] as u64 | (t[1] as u64) << 32;
        let cod = (t[2] >> 24) as u8;
        match t[3] >> 10 & 0x3F {
            32 => {
                let (slot, dci) = ((t[3] >> 24) as u8, (t[3] >> 16 & 0x1F) as u8);
                let Some(ep) = self.ep_mut(slot, dci) else { return };
                let resid = t[2] & 0xFF_FFFF;
                match cod {
                    13 if ep.ctl && ptr != ep.ult => ep.res = resid,
                    1 if ptr != ep.ult => {}
                    _ => ep.hecho = Some((cod, if ep.ctl { ep.res } else { resid })),
                }
            }
            33 if ptr == self.cmd_pend => self.cmd_res = Some((cod, (t[3] >> 24) as u8)),
            34 => {
                let p = (t[0] >> 24) as u8;
                if !self.psc.contains(&p) {
                    self.psc.push(p);
                }
            }
            _ => {}
        }
    }

    // ---------- Órdenes ----------

    /// Envía una orden y espera su fin. Devuelve (código de finalización, ranura).
    fn comando(&mut self, t: [u32; 4]) -> R<(u8, u8)> {
        self.cmd_res = None;
        self.cmd_pend = self.cmd.push(t);
        self.timbre(0, 0);
        let p = Plazo::ms(1000);
        loop {
            self.eventos();
            if let Some(r) = self.cmd_res.take() {
                return Ok(r);
            }
            if p.vencido() {
                break;
            }
            core::hint::spin_loop();
        }
        // Abortar la orden y reanudar el anillo desde la posición actual.
        w32(self.op + 0x18, 4);
        esperar(500, || r32(self.op + 0x18) & 8 == 0);
        self.eventos();
        w64(self.op + 0x18, self.cmd.pos() | self.cmd.ciclo as u64);
        traza!("NF usb: controlador {}: una orden no responde", self.id);
        Err(Error::Tiempo)
    }

    fn orden_ok(&mut self, t: [u32; 4]) -> R<u8> {
        match self.comando(t)? {
            (1, s) => Ok(s),
            (c, _) => Err(Error::Codigo(c)),
        }
    }

    // ---------- Ranuras y contextos ----------

    pub fn ep_mut(&mut self, slot: u8, dci: u8) -> Option<&mut Ep> {
        self.devs.get_mut(slot as usize)?.as_mut()?.eps.get_mut((dci as usize).checked_sub(1)?)?.as_mut()
    }
    pub fn dev(&self, slot: u8) -> Option<&Dispositivo> {
        self.devs.get(slot as usize)?.as_deref()
    }
    pub fn dev_mut(&mut self, slot: u8) -> Option<&mut Dispositivo> {
        self.devs.get_mut(slot as usize)?.as_deref_mut()
    }

    /// Palabra `dw` de la entrada `e` de un bloque de contextos.
    fn c(&self, base: &Dma, e: usize, dw: usize) -> *mut u32 {
        unsafe { base.ptr::<u8>().add(e * self.cs + dw * 4) as *mut u32 }
    }

    /// Enable Slot + Address Device. Devuelve la ranura, ya direccionada.
    pub fn alta(&mut self, o: Origen) -> R<u8> {
        let slot = self.orden_ok([0, 0, 0, 9 << 10])?;
        if slot == 0 || slot as usize >= self.devs.len() {
            return Err(Error::Datos);
        }
        let (Ok(anillo), Ok(salida), Ok(entrada)) = (self.mem(64 * 16, 1024), self.mem(self.cs * 32, 4096), self.mem(self.cs * 33, 4096)) else {
            let _ = self.comando(trb_cmd(10, 0, slot, 0));
            return Err(Error::Memoria);
        };
        let anillo = Anillo::de(anillo, 64);
        let mps0: u32 = match o.vel {
            3 => 64,
            4 | 5 => 512,
            _ => 8, // baja y completa: el real se lee después
        };
        let dcbaa_slot = unsafe { self.dcbaa.ptr::<u64>().add(slot as usize) };
        unsafe {
            *self.c(&entrada, 0, 1) = 3; // A0 (ranura) y A1 (EP0)
            *self.c(&entrada, 1, 0) = o.ruta | (o.vel as u32) << 20 | 1 << 27;
            *self.c(&entrada, 1, 1) = (o.raiz as u32) << 16;
            if let Some((h, p)) = o.tt {
                *self.c(&entrada, 1, 2) = h as u32 | (p as u32) << 8;
            }
            *self.c(&entrada, 2, 1) = 3 << 1 | 4 << 3 | mps0 << 16;
            *self.c(&entrada, 2, 2) = anillo.m.dir() as u32 | 1;
            *self.c(&entrada, 2, 3) = (anillo.m.dir() >> 32) as u32;
            *self.c(&entrada, 2, 4) = 8;
            write_volatile(dcbaa_slot, salida.dir());
        }
        let ep0 = Ep { anillo, mps: mps0 as u16, buf: None, ult: 0, hecho: None, ctl: true, res: 0, pedido: 0, errores: 0, uso: Uso::Nada };
        let mut eps: [Option<Ep>; 31] = core::array::from_fn(|_| None);
        eps[0] = Some(ep0);
        let ent = entrada.dir();
        self.devs[slot as usize] = Some(Box::new(Dispositivo {
            slot,
            vel: o.vel,
            raiz: o.raiz,
            ruta: o.ruta,
            prof: o.prof,
            padre: o.padre,
            tt: o.tt,
            puertos: 0,
            salida,
            entrada,
            eps,
        }));
        if o.padre.is_none() {
            self.puerto_slot[o.raiz as usize] = slot;
        }
        if let Err(e) = self.orden_ok(trb_cmd(11, ent, slot, 0)) {
            self.quitar(slot);
            return Err(e);
        }
        Ok(slot)
    }

    /// Disable Slot de un dispositivo y de todo lo que cuelgue de él; libera su memoria.
    pub fn quitar(&mut self, slot: u8) {
        let hijos: Vec<u8> = self.devs.iter().flatten().filter(|d| matches!(d.padre, Some((h, _)) if h == slot)).map(|d| d.slot).collect();
        for h in hijos {
            self.quitar(h);
        }
        let Some(mut d) = self.devs.get_mut(slot as usize).and_then(|d| d.take()) else { return };
        let fuera = self.comando(trb_cmd(10, 0, slot, 0)).is_ok();
        unsafe { write_volatile(self.dcbaa.ptr::<u64>().add(slot as usize), 0) };
        super::masivo::quitado(self.id, slot, !fuera);
        for e in d.eps.iter_mut().flatten() {
            match &mut e.uso {
                Uso::Mando(m) => {
                    super::evento_mando(nf_core::gamepad::PadEvent::Removed(m.id));
                    traza!("NF usb: mando {} quitado", m.id);
                }
                // Lo que estuviera pulsado se suelta: si no, Mayús o un botón quedarían pegados.
                Uso::Teclado(kb) => kb.feed(&[0; 8], 0, super::soltar_teclas()),
                Uso::Raton => super::soltar_raton(),
                _ => {}
            }
        }
        if d.padre.is_none() && self.puerto_slot[d.raiz as usize] == slot {
            self.puerto_slot[d.raiz as usize] = 0;
        }
        if !fuera {
            traza!("NF usb: ranura {}: Disable Slot falla, su memoria no se libera", slot);
            core::mem::forget(d);
        }
    }

    /// Evaluate Context: nuevo tamaño máximo de paquete del EP0 (se conoce tras leer el descriptor).
    pub fn ajustar_mps0(&mut self, slot: u8, mps: u16) -> R<()> {
        let d = self.dev_mut(slot).ok_or(Error::Ausente)?;
        let (ent, sal) = (d.entrada.dir(), d.salida.dir());
        let cs = self.cs;
        unsafe {
            core::ptr::write_bytes(ent as *mut u8, 0, cs * 33);
            *(ent as *mut u32).add(1) = 1 << 1; // A1
            core::ptr::copy_nonoverlapping((sal as *const u8).add(cs), (ent as *mut u8).add(2 * cs), cs); // EP0
            *((ent as *mut u8).add(2 * cs + 4) as *mut u32) = *((sal as *const u8).add(cs + 4) as *const u32) & 0xFFFF | (mps as u32) << 16;
        }
        self.orden_ok(trb_cmd(13, ent, slot, 0))?;
        if let Some(e) = self.ep_mut(slot, 1) {
            e.mps = mps;
        }
        Ok(())
    }

    /// Configure Endpoint: añade endpoints (con su `Uso`) y, si es un hub, sus datos en el contexto de ranura.
    pub fn configurar(&mut self, slot: u8, eps: Vec<(CfgEp, Uso)>, hub: Option<(u8, u8)>) -> R<()> {
        let mut nuevos = Vec::new();
        for (c, uso) in eps {
            if !(2..=31).contains(&c.dci) {
                return Err(Error::Datos);
            }
            let anillo = Anillo::de(self.mem(64 * 16, 1024)?, 64);
            let tam = (c.mps.max(8) as usize).next_power_of_two();
            let buf = if c.tipo == 7 { Some(self.mem(tam, tam)?) } else { None };
            let ep = Ep { anillo, mps: c.mps, buf, ult: 0, hecho: None, ctl: false, res: 0, pedido: 0, errores: 0, uso };
            nuevos.push((c, ep));
        }
        let cs = self.cs;
        let d = self.dev(slot).ok_or(Error::Ausente)?;
        let (ent, sal) = (d.entrada.dir() as *mut u8, d.salida.dir() as *const u8);
        let mut add = 1u32;
        let mut max = unsafe { *(sal as *const u32) >> 27 };
        unsafe {
            core::ptr::write_bytes(ent, 0, cs * 33);
            core::ptr::copy_nonoverlapping(sal, ent.add(cs), cs); // contexto de ranura actual
            for (c, e) in &nuevos {
                add |= 1 << c.dci;
                max = max.max(c.dci as u32);
                let k = ent.add((1 + c.dci as usize) * cs) as *mut u32;
                // Los endpoints bulk no son periódicos: su carga por intervalo es 0.
                let esit = if matches!(c.tipo, 2 | 6) { 0 } else { c.mps as u32 * (c.rafaga as u32 + 1) };
                *k = (c.intervalo as u32) << 16;
                *k.add(1) = 3 << 1 | (c.tipo as u32) << 3 | (c.rafaga as u32) << 8 | (c.mps as u32) << 16;
                *k.add(2) = e.anillo.m.dir() as u32 | 1;
                *k.add(3) = (e.anillo.m.dir() >> 32) as u32;
                *k.add(4) = c.mps as u32 | esit << 16;
            }
            let s = ent.add(cs) as *mut u32;
            *s = *s & !(0x1F << 27) | max << 27;
            if let Some((puertos, ttt)) = hub {
                *s |= 1 << 26;
                *s.add(1) = *s.add(1) & 0x00FF_FFFF | (puertos as u32) << 24;
                *s.add(2) = *s.add(2) & !(3 << 16) | (ttt as u32) << 16;
            }
            *(ent as *mut u32).add(1) = add;
        }
        self.orden_ok(trb_cmd(12, ent as u64, slot, 0))?;
        let d = self.dev_mut(slot).ok_or(Error::Ausente)?;
        if let Some((puertos, _)) = hub {
            d.puertos = puertos;
        }
        for (c, e) in nuevos {
            d.eps[c.dci as usize - 1] = Some(e);
        }
        Ok(())
    }

    // ---------- Transferencias ----------

    /// Transferencia de control en el EP0 (bloqueante). `datos` ≤ 4 KiB. Devuelve los bytes recibidos
    /// (o enviados). Con un error de endpoint lo recupera; con `Tiempo` el dispositivo queda dudoso.
    pub fn control(&mut self, slot: u8, tipo: u8, req: u8, val: u16, idx: u16, datos: &mut [u8]) -> R<usize> {
        let len = datos.len();
        if len > 4096 {
            return Err(Error::Datos);
        }
        let entrada = tipo & 0x80 != 0;
        let rb = self.rebote.dir();
        if !entrada && len > 0 {
            unsafe { core::ptr::copy_nonoverlapping(datos.as_ptr(), rb as *mut u8, len) };
        }
        {
            let ep = self.ep_mut(slot, 1).ok_or(Error::Ausente)?;
            ep.ctl = true;
            ep.res = 0;
            ep.hecho = None;
            ep.pedido = len as u32;
            let trt = if len == 0 { 0 } else if entrada { 3 } else { 2 };
            ep.anillo.push([tipo as u32 | (req as u32) << 8 | (val as u32) << 16, idx as u32 | (len as u32) << 16, 8, 2 << 10 | 1 << 6 | trt << 16]);
            if len > 0 {
                // En la fase de datos de entrada se pide evento también con paquete corto (ISP).
                ep.anillo.push([rb as u32, (rb >> 32) as u32, len as u32, 3 << 10 | if entrada { 1 << 16 | 1 << 2 } else { 0 }]);
            }
            ep.ult = ep.anillo.push([0, 0, 0, 4 << 10 | 1 << 5 | if len == 0 || !entrada { 1 << 16 } else { 0 }]);
        }
        self.timbre(slot, 1);
        let n = match self.esperar(slot, 1, 1000) {
            Err(Error::Tiempo) => {
                // Sin abortar, el TD quedaría en el anillo y su evento tardío respondería a la siguiente.
                self.abortar(slot, 1);
                return Err(Error::Tiempo);
            }
            r => r?,
        };
        if entrada {
            unsafe { core::ptr::copy_nonoverlapping(rb as *const u8, datos.as_mut_ptr(), n.min(len)) };
        }
        Ok(n)
    }

    /// Transferencia bulk o de interrupción: encola `len` bytes en `buf` (dirección física = virtual,
    /// sin condiciones de alineación) y toca el timbre. Se espera con `esperar`.
    pub fn normal(&mut self, slot: u8, dci: u8, buf: u64, len: usize, entrada: bool) -> R<()> {
        let ep = self.ep_mut(slot, dci).ok_or(Error::Ausente)?;
        ep.ctl = false;
        ep.hecho = None;
        ep.res = 0;
        ep.pedido = len as u32;
        let mps = ep.mps.max(1) as usize;
        let (mut resta, mut a) = (len, buf);
        loop {
            // Un TRB no puede cruzar un límite de 64 KiB.
            let n = resta.min(0x1_0000 - (a & 0xFFFF) as usize);
            resta -= n;
            let ultimo = resta == 0;
            let td = (resta.div_ceil(mps)).min(31) as u32;
            let isp = if entrada { 1 << 2 } else { 0 };
            ep.ult = ep.anillo.push([a as u32, (a >> 32) as u32, n as u32 | td << 17, 1 << 10 | isp | if ultimo { 1 << 5 } else { 1 << 4 }]);
            a += n as u64;
            if ultimo {
                break;
            }
        }
        self.timbre(slot, dci as u32);
        Ok(())
    }

    /// Espera el fin de la transferencia en curso de un endpoint. Devuelve los bytes movidos.
    /// Un error de endpoint (stall, etc.) se recupera aquí y se devuelve como `Codigo`.
    pub fn esperar(&mut self, slot: u8, dci: u8, ms: u64) -> R<usize> {
        let p = Plazo::ms(ms);
        loop {
            self.eventos();
            let ep = self.ep_mut(slot, dci).ok_or(Error::Ausente)?;
            if let Some((cod, res)) = ep.hecho.take() {
                return self.fin(slot, dci, cod, res);
            }
            if p.vencido() {
                return Err(Error::Tiempo);
            }
            core::hint::spin_loop();
        }
    }

    fn fin(&mut self, slot: u8, dci: u8, cod: u8, res: u32) -> R<usize> {
        if cod == 1 || cod == 13 {
            let ep = self.ep_mut(slot, dci).ok_or(Error::Ausente)?;
            return Ok(ep.pedido.saturating_sub(res) as usize);
        }
        self.recuperar(slot, dci);
        Err(Error::Codigo(cod))
    }

    /// Reset Endpoint + Set TR Dequeue Pointer: el endpoint vuelve a aceptar transferencias nuevas.
    pub fn recuperar(&mut self, slot: u8, dci: u8) {
        let _ = self.comando(trb_cmd(14, 0, slot, (dci as u32) << 16));
        let Some(ep) = self.ep_mut(slot, dci) else { return };
        let (p, c) = (ep.anillo.pos(), ep.anillo.ciclo as u64);
        let _ = self.comando(trb_cmd(16, p | c, slot, (dci as u32) << 16));
    }

    /// Una transferencia no termina (plazo agotado): Stop Endpoint y vuelta al anillo limpio, para poder
    /// seguir usando el endpoint. El evento de «parado» que deja el TD pendiente se descarta al encolar de nuevo.
    pub fn abortar(&mut self, slot: u8, dci: u8) {
        let _ = self.comando(trb_cmd(15, 0, slot, (dci as u32) << 16));
        self.recuperar(slot, dci);
        if let Some(ep) = self.ep_mut(slot, dci) {
            ep.hecho = None;
        }
    }

    // ---------- Endpoints de interrupción (lo que usan las clases) ----------

    /// Endpoints con una transferencia terminada: (ranura, dci).
    pub fn completados(&self) -> Vec<(u8, u8)> {
        let mut v = Vec::new();
        for d in self.devs.iter().flatten() {
            for (i, e) in d.eps.iter().enumerate() {
                if matches!(e, Some(e) if e.hecho.is_some() && !matches!(e.uso, Uso::Nada)) {
                    v.push((d.slot, i as u8 + 1));
                }
            }
        }
        v
    }

    /// Recoge lo recibido por un endpoint de interrupción terminado: los datos o el código de error.
    pub fn recoger(&mut self, slot: u8, dci: u8) -> Option<Result<Vec<u8>, u8>> {
        let ep = self.ep_mut(slot, dci)?;
        let (cod, res) = ep.hecho.take()?;
        if cod != 1 && cod != 13 {
            ep.errores = ep.errores.saturating_add(1);
            return Some(Err(cod));
        }
        ep.errores = 0;
        let n = ep.pedido.saturating_sub(res) as usize;
        let b = ep.buf.as_ref()?;
        Some(Ok(unsafe { core::slice::from_raw_parts(b.ptr::<u8>(), n) }.to_vec()))
    }

    /// Encola (otra vez) la lectura de un endpoint de interrupción IN.
    pub fn armar(&mut self, slot: u8, dci: u8) -> R<()> {
        let ep = self.ep_mut(slot, dci).ok_or(Error::Ausente)?;
        let b = ep.buf.as_ref().ok_or(Error::Datos)?;
        let (a, n) = (b.dir(), ep.mps.max(1) as usize);
        self.normal(slot, dci, a, n, true)
    }
}

/// Recorre las capacidades extendidas: traspaso desde la BIOS (ID 1) y reparto de puertos USB 3 (ID 2).
fn capacidades(base: usize, hcc: u32) -> [bool; 256] {
    let mut usb3 = [false; 256];
    let mut a = base + ((hcc >> 16) as usize) * 4;
    if hcc >> 16 == 0 {
        return usb3;
    }
    for _ in 0..64 {
        let h = r32(a);
        match h & 0xFF {
            1 => {
                // USB Legacy Support: pedir la propiedad al SO y apagar las SMI.
                if h & (1 << 16) != 0 {
                    w32(a, h | 1 << 24);
                    if !esperar(1000, || r32(a) & (1 << 16) == 0) {
                        traza!("NF usb: la BIOS no cede el controlador, se continúa");
                    }
                }
                let c = r32(a + 4);
                w32(a + 4, c & !0xE011 | 0xE000_0000);
            }
            2 if h >> 24 >= 3 => {
                let (off, n) = (r32(a + 8) & 0xFF, r32(a + 8) >> 8 & 0xFF);
                for p in off..off + n {
                    if let Some(u) = usb3.get_mut(p as usize) {
                        *u = true;
                    }
                }
            }
            _ => {}
        }
        let sig = (h >> 8 & 0xFF) as usize;
        if sig == 0 {
            break;
        }
        a += sig * 4;
    }
    usb3
}

/// Datos de configuración de endpoint: intervalo en la forma del contexto (2^n × 125 µs).
pub fn intervalo(vel: u8, b_interval: u8) -> u8 {
    if vel >= 3 {
        b_interval.clamp(1, 16) - 1
    } else {
        // Baja/completa: milisegundos -> 2^n × 125 µs, entre 2^3 y 2^10.
        (b_interval.max(1).ilog2() as u8 + 3).clamp(3, 10)
    }
}
