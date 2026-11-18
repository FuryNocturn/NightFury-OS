// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Enumeración de dispositivos USB (descriptores, configuración), hubs y clases (HID de arranque, hub).
//! Las clases se enchufan en `Uso::de` (qué interfaz reclaman), en `preparar` (qué piden al dispositivo
//! antes de configurar) y en `atender` (qué hacen con cada transferencia terminada).
use super::xhci::{intervalo, CfgEp, Error, Origen, Xhci, CCS, CSC};
use super::{espera_ms, Plazo, Uso};
use crate::serial::traza;
use alloc::vec::Vec;
use nf_core::input::MousePacket;
use nf_core::usbhid::boot_mouse;

type R<T> = Result<T, Error>;

struct EpD {
    addr: u8,
    attr: u8,
    mps: u16,
    intervalo: u8,
    rafaga: u8,
}

struct Iface {
    num: u8,
    clase: u8,
    sub: u8,
    proto: u8,
    eps: Vec<EpD>,
}

/// Interfaces (solo el ajuste alternativo 0) y sus endpoints, del descriptor de configuración completo.
fn interfaces(cfg: &[u8]) -> Vec<Iface> {
    let mut v: Vec<Iface> = Vec::new();
    let mut activa = false;
    let mut i = 0;
    while i + 2 <= cfg.len() {
        let l = cfg[i] as usize;
        if l < 2 || i + l > cfg.len() {
            break;
        }
        let d = &cfg[i..i + l];
        match d[1] {
            4 if l >= 9 => {
                activa = d[3] == 0;
                if activa {
                    v.push(Iface { num: d[2], clase: d[5], sub: d[6], proto: d[7], eps: Vec::new() });
                }
            }
            5 if l >= 7 && activa => {
                if let Some(f) = v.last_mut() {
                    f.eps.push(EpD { addr: d[2], attr: d[3], mps: u16::from_le_bytes([d[4], d[5]]), intervalo: d[6], rafaga: 0 });
                }
            }
            // Companion de endpoint SuperSpeed: ráfaga máxima del endpoint anterior.
            0x30 if l >= 3 && activa => {
                if let Some(e) = v.last_mut().and_then(|f| f.eps.last_mut()) {
                    e.rafaga = d[2];
                }
            }
            _ => {}
        }
        i += l;
    }
    v
}

fn nombre(u: &Uso) -> &'static str {
    match u {
        Uso::Nada => "-",
        Uso::Teclado(_) => "teclado",
        Uso::Raton => "raton",
        Uso::Hub => "hub",
        Uso::Masivo => "almacenamiento",
    }
}

// ---------- Enumeración ----------

/// Con el controlador ya en marcha: enumera todo lo que haya conectado.
pub fn explorar(x: &mut Xhci) {
    espera_ms(100);
    x.eventos();
    x.psc.clear();
    for p in 1..=x.npuertos {
        if x.portsc(p) & CCS != 0 {
            cambio_raiz(x, p, true);
        } else {
            x.ack_puerto(p);
        }
    }
}

/// Un puerto raíz cambió de estado: baja del dispositivo que había y/o alta del nuevo.
fn cambio_raiz(x: &mut Xhci, p: u8, inicial: bool) {
    let sc = x.ack_puerto(p);
    let conectado = sc & CCS != 0;
    let slot = x.puerto_slot[p as usize];
    if slot != 0 && (!conectado || sc & CSC != 0) {
        traza!("NF usb: controlador {} puerto {}: desconectado", x.id, p);
        x.quitar(slot);
    }
    if !conectado {
        x.intentos[p as usize] = 0;
    } else if x.puerto_slot[p as usize] == 0 && (inicial || sc & CSC != 0) && x.intentos[p as usize] < 3 {
        // Si falla tres veces seguidas se deja hasta que se desconecte, para no atascar el sondeo.
        match enumerar_raiz(x, p) {
            Ok(()) => x.intentos[p as usize] = 0,
            Err(e) => {
                x.intentos[p as usize] += 1;
                traza!("NF usb: controlador {} puerto {}: no se pudo enumerar ({:?})", x.id, p, e);
            }
        }
    }
}

fn enumerar_raiz(x: &mut Xhci, p: u8) -> R<()> {
    let vel = x.reset_puerto(p).ok_or(Error::Tiempo)?;
    let slot = x.alta(Origen { raiz: p, ruta: 0, vel, prof: 0, padre: None, tt: None })?;
    identificar(x, slot).inspect_err(|_| x.quitar(slot))
}

/// Lee los descriptores, reclama las interfaces que conocemos y deja el dispositivo configurado.
fn identificar(x: &mut Xhci, slot: u8) -> R<()> {
    let vel = x.dev(slot).ok_or(Error::Ausente)?.vel;
    // Los 8 primeros bytes dan el tamaño máximo de paquete del EP0 (algunos dispositivos tardan en responder).
    let mut d = [0u8; 18];
    let mut ultimo = Error::Datos;
    let mut leido = false;
    for _ in 0..3 {
        match x.control(slot, 0x80, 6, 0x0100, 0, &mut d[..8]) {
            Ok(8) => {
                leido = true;
                break;
            }
            Ok(_) => ultimo = Error::Datos,
            Err(e) => ultimo = e,
        }
        espera_ms(20);
    }
    if !leido {
        return Err(ultimo);
    }
    let mps = if vel >= 4 { if d[7] == 9 { 512 } else { 0 } } else { d[7] as u16 };
    if !matches!(mps, 8 | 16 | 32 | 64 | 512) {
        return Err(Error::Datos);
    }
    if x.ep_mut(slot, 1).map(|e| e.mps) != Some(mps) {
        x.ajustar_mps0(slot, mps)?;
    }
    if x.control(slot, 0x80, 6, 0x0100, 0, &mut d)? < 18 {
        return Err(Error::Datos);
    }
    let (vid, pid) = (u16::from_le_bytes([d[8], d[9]]), u16::from_le_bytes([d[10], d[11]]));
    let dv = x.dev(slot).ok_or(Error::Ausente)?;
    traza!("NF usb: controlador {} puerto {} ruta {:#x} ranura {}: {:04x}:{:04x} clase {:02x} velocidad {}", x.id, dv.raiz, dv.ruta, slot, vid, pid, d[4], vel);
    let mut c9 = [0u8; 9];
    if x.control(slot, 0x80, 6, 0x0200, 0, &mut c9)? < 9 {
        return Err(Error::Datos);
    }
    let total = (u16::from_le_bytes([c9[2], c9[3]]) as usize).clamp(9, 4096);
    let mut cfg = alloc::vec![0u8; total];
    let n = x.control(slot, 0x80, 6, 0x0200, 0, &mut cfg)?;
    cfg.truncate(n);
    preparar(x, slot, c9[5], &cfg)
}

/// Reclama las interfaces conocidas: SET_CONFIGURATION, órdenes propias de cada clase, Configure Endpoint
/// y primera lectura. Un dispositivo sin interfaces conocidas se deja direccionado, sin configurar.
fn preparar(x: &mut Xhci, slot: u8, valor_cfg: u8, cfg: &[u8]) -> R<()> {
    let vel = x.dev(slot).ok_or(Error::Ausente)?.vel;
    let mut planes: Vec<(u8, CfgEp, Uso)> = Vec::new();
    // Interfaces de almacenamiento: (interfaz, dci bulk IN, dci bulk OUT, direcciones de los endpoints).
    let mut masivos: Vec<(u8, u8, u8, u8, u8)> = Vec::new();
    for f in interfaces(cfg) {
        if f.clase == 8 && f.proto == 0x62 {
            // UAS sin alternativa BOT en el ajuste 0 (con ella, `interfaces` ya ve la BOT): no se soporta.
            traza!("NF usb: ranura {}: interfaz {} es UAS sin alternativa BOT, se ignora", slot, f.num);
        }
        let Some(uso) = Uso::de(f.clase, f.sub, f.proto) else { continue };
        if let Uso::Masivo = uso {
            let bulk = |dir_in: bool| f.eps.iter().find(|e| e.attr & 3 == 2 && (e.addr & 0x80 != 0) == dir_in);
            let (Some(ei), Some(eo)) = (bulk(true), bulk(false)) else {
                traza!("NF usb: ranura {}: interfaz {} de almacenamiento sin endpoints bulk IN y OUT", slot, f.num);
                continue;
            };
            let (di, d_o) = ((ei.addr & 0xF) * 2 + 1, (eo.addr & 0xF) * 2);
            if di == 1 || d_o == 0 || planes.iter().any(|p| p.1.dci == di || p.1.dci == d_o) {
                continue;
            }
            for (e, dci, tipo) in [(ei, di, 6), (eo, d_o, 2)] {
                let rafaga = if vel >= 4 { e.rafaga } else { 0 };
                planes.push((f.num, CfgEp { dci, tipo, mps: e.mps & 0x7FF, intervalo: 0, rafaga }, Uso::Nada));
            }
            masivos.push((f.num, di, d_o, ei.addr, eo.addr));
            continue;
        }
        let Some(e) = f.eps.iter().find(|e| e.attr & 3 == 3 && e.addr & 0x80 != 0) else { continue };
        let dci = (e.addr & 0xF) * 2 + 1;
        if planes.iter().any(|p| p.1.dci == dci) {
            continue;
        }
        let rafaga = if vel >= 4 { e.rafaga } else { (e.mps >> 11 & 3) as u8 };
        planes.push((f.num, CfgEp { dci, tipo: 7, mps: e.mps & 0x7FF, intervalo: intervalo(vel, e.intervalo), rafaga }, uso));
    }
    if planes.is_empty() {
        traza!("NF usb: ranura {}: ninguna interfaz conocida", slot);
        return Ok(());
    }
    x.control(slot, 0, 9, valor_cfg as u16, 0, &mut [])?;
    let mut hub = None;
    let mut espera_hub = 0;
    for (num, _, uso) in &planes {
        match uso {
            Uso::Teclado(_) | Uso::Raton => {
                // Protocolo de arranque y sin repeticiones por silencio; algunos dispositivos las rechazan.
                let _ = x.control(slot, 0x21, 0x0B, 0, *num as u16, &mut []);
                let _ = x.control(slot, 0x21, 0x0A, 0, *num as u16, &mut []);
            }
            Uso::Hub => {
                let prof = x.dev(slot).ok_or(Error::Ausente)?.prof;
                if vel >= 4 {
                    x.control(slot, 0x20, 12, prof as u16, 0, &mut [])?; // SET_HUB_DEPTH
                }
                let mut h = [0u8; 16];
                let n = x.control(slot, 0xA0, 6, if vel >= 4 { 0x2A00 } else { 0x2900 }, 0, &mut h)?;
                if n < 7 || h[2] == 0 {
                    return Err(Error::Datos);
                }
                hub = Some((h[2], if vel >= 4 { 0 } else { h[3] >> 5 & 3 }));
                espera_hub = h[5] as u64 * 2;
            }
            Uso::Nada | Uso::Masivo => {}
        }
    }
    // Solo los endpoints de interrupción se arman: los bulk los usa `masivo` bajo demanda.
    let dcis: Vec<(u8, bool)> = planes.iter().filter(|p| p.1.tipo == 7).map(|p| (p.1.dci, matches!(p.2, Uso::Hub))).collect();
    let nombres: Vec<&str> = planes.iter().map(|p| nombre(&p.2)).collect();
    x.configurar(slot, planes.into_iter().map(|(_, c, u)| (c, u)).collect(), hub)?;
    traza!("NF usb: ranura {} configurada: {:?}{}", slot, nombres, hub.map_or(alloc::string::String::new(), |h| alloc::format!(" ({} puertos)", h.0)));
    for (dci, es_hub) in dcis {
        x.armar(slot, dci)?;
        if es_hub {
            hub_listar(x, slot, espera_hub);
        }
    }
    for (num, di, d_o, ai, ao) in masivos {
        super::masivo::alta(x, slot, num, (di, ai), (d_o, ao));
    }
    Ok(())
}

// ---------- Hubs ----------

fn hub_peticion(x: &mut Xhci, hub: u8, req: u8, val: u16, puerto: u8) -> R<()> {
    x.control(hub, 0x23, req, val, puerto as u16, &mut []).map(|_| ())
}

/// (estado, cambios) de un puerto de hub.
fn hub_estado(x: &mut Xhci, hub: u8, p: u8) -> R<(u16, u16)> {
    let mut b = [0u8; 4];
    if x.control(hub, 0xA3, 0, 0, p as u16, &mut b)? < 4 {
        return Err(Error::Datos);
    }
    Ok((u16::from_le_bytes([b[0], b[1]]), u16::from_le_bytes([b[2], b[3]])))
}

/// Da corriente a los puertos de un hub recién configurado y enumera lo que ya haya enchufado.
fn hub_listar(x: &mut Xhci, hub: u8, espera: u64) {
    let Some(puertos) = x.dev(hub).map(|d| d.puertos) else { return };
    for p in 1..=puertos {
        let _ = hub_peticion(x, hub, 3, 8, p); // SET_FEATURE PORT_POWER
    }
    espera_ms(espera.max(20) + 100);
    for p in 1..=puertos {
        hub_puerto(x, hub, p, true);
    }
}

/// Cambios en un puerto de hub: baja del hijo que había y/o alta del nuevo.
fn hub_puerto(x: &mut Xhci, hub: u8, p: u8, forzar: bool) {
    let Ok((st, ch)) = hub_estado(x, hub, p) else { return };
    let usb3 = x.dev(hub).map_or(false, |d| d.vel >= 4);
    for bit in 0..8u8 {
        if ch >> bit & 1 != 0 {
            let f = match bit {
                0..=4 => 16 + bit,
                5 if usb3 => 29, // reset de BH
                6 if usb3 => 25, // estado del enlace
                7 if usb3 => 26, // error de configuración
                _ => continue,
            };
            let _ = hub_peticion(x, hub, 1, f as u16, p); // CLEAR_FEATURE C_PORT_*
        }
    }
    let hijo = x.devs.iter().flatten().find(|d| d.padre == Some((hub, p))).map(|d| d.slot);
    let conectado = st & 1 != 0;
    if let Some(h) = hijo.filter(|_| ch & 1 != 0 || !conectado) {
        traza!("NF usb: controlador {} hub (ranura {}) puerto {}: desconectado", x.id, hub, p);
        x.quitar(h);
    }
    let hay = x.devs.iter().flatten().any(|d| d.padre == Some((hub, p)));
    if conectado && !hay && (ch & 1 != 0 || forzar) {
        if let Err(e) = hub_hijo(x, hub, p) {
            traza!("NF usb: controlador {} hub (ranura {}) puerto {}: no se pudo enumerar ({:?})", x.id, hub, p, e);
        }
    }
}

/// Reset del puerto del hub y alta del dispositivo que cuelga de él.
fn hub_hijo(x: &mut Xhci, hub: u8, p: u8) -> R<()> {
    let h = x.dev(hub).ok_or(Error::Ausente)?;
    let (hvel, hruta, hprof, hraiz, htt) = (h.vel, h.ruta, h.prof, h.raiz, h.tt);
    if hprof >= 5 || p > 15 {
        return Err(Error::Datos); // la ruta solo tiene 5 niveles de 4 bits
    }
    let usb3 = hvel >= 4;
    if !usb3 {
        espera_ms(100); // estabilización de la conexión USB 2
    }
    hub_peticion(x, hub, 3, 4, p)?; // SET_FEATURE PORT_RESET
    let plazo = Plazo::ms(500);
    let (st, _) = loop {
        let (st, ch) = hub_estado(x, hub, p)?;
        if ch & 1 << 4 != 0 {
            break (st, ch);
        }
        if plazo.vencido() {
            return Err(Error::Tiempo);
        }
    };
    let _ = hub_peticion(x, hub, 1, 20, p); // C_PORT_RESET
    if usb3 {
        let _ = hub_peticion(x, hub, 1, 29, p);
    }
    espera_ms(10);
    let (st2, _) = hub_estado(x, hub, p)?;
    if (st | st2) & 1 == 0 || st2 & 2 == 0 {
        return Err(Error::Datos);
    }
    let vel = if usb3 { 4 } else if st2 & 1 << 10 != 0 { 3 } else if st2 & 1 << 9 != 0 { 2 } else { 1 };
    // Un dispositivo lento o completo detrás de un hub de alta velocidad habla con él por su TT.
    let tt = if vel <= 2 { if hvel == 3 { Some((hub, p)) } else { htt } } else { None };
    let slot = x.alta(Origen { raiz: hraiz, ruta: hruta | (p as u32) << (4 * hprof), vel, prof: hprof + 1, padre: Some((hub, p)), tt })?;
    identificar(x, slot).inspect_err(|_| x.quitar(slot))
}

// ---------- Sondeo ----------

/// Atiende un endpoint de interrupción cuya transferencia terminó y lo vuelve a encolar.
fn atender(x: &mut Xhci, slot: u8, dci: u8, ahora: u64, tec: &mut Vec<u8>, rat: &mut Vec<MousePacket>) {
    match x.recoger(slot, dci) {
        None => {}
        Some(Ok(datos)) => {
            let mut hub = false;
            match x.ep_mut(slot, dci).map(|e| &mut e.uso) {
                Some(Uso::Teclado(kb)) => kb.feed(&datos, ahora, tec),
                Some(Uso::Raton) => rat.extend(boot_mouse(&datos)),
                Some(Uso::Hub) => hub = true,
                _ => {}
            }
            if hub {
                // Mapa de bits: bit n = el puerto n ha cambiado.
                let puertos = x.dev(slot).map_or(0, |d| d.puertos);
                for p in 1..=puertos {
                    if datos.get(p as usize / 8).is_some_and(|b| b >> (p % 8) & 1 != 0) {
                        hub_puerto(x, slot, p, false);
                    }
                }
            }
            let _ = x.armar(slot, dci);
        }
        Some(Err(cod)) => {
            // Tres errores seguidos y se deja el endpoint quieto (suele ser que lo han desenchufado).
            let n = x.ep_mut(slot, dci).map_or(0, |e| e.errores);
            if n <= 3 {
                x.recuperar(slot, dci);
                let _ = x.armar(slot, dci);
            } else if n == 4 {
                traza!("NF usb: ranura {} endpoint {}: errores seguidos (código {}), se deja", slot, dci, cod);
            }
        }
    }
}

/// Un paso de sondeo de un controlador.
pub fn servir(x: &mut Xhci, ahora: u64, tec: &mut Vec<u8>, rat: &mut Vec<MousePacket>) {
    x.eventos();
    while let Some(p) = x.psc.pop() {
        if (1..=x.npuertos).contains(&p) {
            cambio_raiz(x, p, false);
        }
    }
    for (slot, dci) in x.completados() {
        atender(x, slot, dci, ahora, tec, rat);
    }
    for d in x.devs.iter_mut().flatten() {
        for e in d.eps.iter_mut().flatten() {
            if let Uso::Teclado(kb) = &mut e.uso {
                kb.tick(ahora, tec);
            }
        }
    }
}
