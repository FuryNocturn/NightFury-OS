// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Unidades extraíbles: engancha los discos USB que aparecen y desaparecen a `Files` (letras D:, E:…)
//! y ejecuta las acciones sobre el disco entero que piden los programas del sistema (expulsar,
//! formatear, cifrar), que solo puede hacer el núcleo.
use crate::{ahci, nvme, rtc, usb};
use crate::serial::traza;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use nf_core::block::DiskError;
use nf_core::drives::{DriveAction, FormatKind, Locked};
use nf_core::files::Files;
use nf_core::fs::FsError;
use nf_core::keys::{KeyError, Pool};
use nf_core::vol::{self, nfvol, FsKind, SectorDev};

/// Los discos internos (SATA y NVMe) llevan este bit en su id para no chocar con los de USB.
const INTERNO: u32 = 1 << 31;

/// Un disco o espacio de nombres interno; las particiones lo comparten por `Rc`.
enum Fisico {
    Sata(Box<ahci::Disco>),
    Nvme(nvme::Espacio),
}

impl Fisico {
    fn sector_size(&self) -> usize {
        match self {
            Fisico::Sata(_) => 512,
            Fisico::Nvme(n) => n.sector_size(),
        }
    }
    fn sectors(&self) -> u64 {
        match self {
            Fisico::Sata(d) => d.sectores(),
            Fisico::Nvme(n) => n.sectores(),
        }
    }
}

struct Interno {
    id: u32,
    nombre: String,
    /// Número de serie (para identificar el disco cuando se formateen particiones internas).
    serie: String,
    fis: Rc<RefCell<Fisico>>,
}

/// Manejador de una partición de un disco interno; `ro` lo deja de solo lectura (todo lo que no es NightFury).
struct Manejador {
    fis: Rc<RefCell<Fisico>>,
    ro: bool,
}

impl Manejador {
    fn comprobar(&self, lba: u64, len: usize) -> Result<(), FsError> {
        let f = self.fis.borrow();
        let ss = f.sector_size();
        if len % ss != 0 || lba.checked_add((len / ss) as u64).map_or(true, |x| x > f.sectors()) {
            return Err(FsError::Disk);
        }
        Ok(())
    }
}

fn disco(e: DiskError) -> FsError {
    let _ = e;
    FsError::Disk
}

impl SectorDev for Manejador {
    fn sector_size(&self) -> usize {
        self.fis.borrow().sector_size()
    }
    fn sectors(&self) -> u64 {
        self.fis.borrow().sectors()
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        self.comprobar(lba, buf.len())?;
        match &mut *self.fis.borrow_mut() {
            Fisico::Sata(d) => d.leer_sectores(lba, buf),
            Fisico::Nvme(n) => n.leer(lba, buf),
        }
        .map_err(disco)
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError> {
        if self.ro {
            return Err(FsError::Denied);
        }
        self.comprobar(lba, buf.len())?;
        match &mut *self.fis.borrow_mut() {
            Fisico::Sata(d) => d.escribir_sectores(lba, buf),
            Fisico::Nvme(n) => n.escribir(lba, buf),
        }
        .map_err(disco)
    }
    fn flush(&mut self) -> Result<(), FsError> {
        if self.ro {
            return Ok(());
        }
        match &mut *self.fis.borrow_mut() {
            Fisico::Sata(d) => d.vaciar(),
            Fisico::Nvme(n) => n.vaciar(),
        }
        .map_err(disco)
    }
    fn read_only(&self) -> bool {
        self.ro
    }
}

/// Nombre de cada disco conectado (para volver a montarlo tras formatearlo o cifrarlo).
#[derive(Default)]
pub struct Unidades {
    nombres: Vec<(u32, String)>,
    internos: Vec<Interno>,
}

impl Unidades {
    /// Engancha las particiones de los discos internos (SATA que no son el del sistema, y NVMe).
    /// Se llama una vez al arrancar, antes que ningún USB.
    pub fn internos(&mut self, files: &mut Files, sata: Vec<Box<ahci::Disco>>, nvme: Vec<nvme::Espacio>) {
        let todos = sata.into_iter().map(|d| (String::from(d.modelo()), String::from(d.serie()), Fisico::Sata(d))).chain(nvme.into_iter().map(|n| (String::from(n.modelo()), String::from(n.serie()), Fisico::Nvme(n))));
        for (i, (nombre, serie, f)) in todos.enumerate() {
            let id = INTERNO | i as u32;
            self.internos.push(Interno { id, nombre, serie, fis: Rc::new(RefCell::new(f)) });
        }
        for k in 0..self.internos.len() {
            self.montar_interno(files, k);
        }
    }

    fn montar_interno(&self, files: &mut Files, k: usize) {
        let it = &self.internos[k];
        traza!("NF interno: disco {} «{}» serie «{}»: {} sectores de {} bytes", it.id & !INTERNO, it.nombre, it.serie, it.fis.borrow().sectors(), it.fis.borrow().sector_size());
        let abrir = |ro| Some(Box::new(Manejador { fis: it.fis.clone(), ro }) as Box<dyn SectorDev>);
        conectar(files, it.id, &it.nombre, abrir, None);
    }

    /// Altas y bajas de discos y acciones pendientes. Se llama en cada vuelta del bucle principal.
    pub fn sondear(&mut self, files: &mut Files, pool: &mut Pool) {
        for c in usb::cambios_discos() {
            match c {
                usb::CambioDisco::Conectado { id, nombre, dev } => {
                    traza!("NF usb: disco {} «{}»: {} sectores de {} bytes{}", id, nombre, dev.sectors(), dev.sector_size(), if dev.read_only() { ", solo lectura" } else { "" });
                    self.nombres.retain(|n| n.0 != id);
                    self.nombres.push((id, nombre.clone()));
                    conectar(files, id, &nombre, |_| usb::abrir_disco(id), None);
                }
                usb::CambioDisco::Quitado { id } => {
                    self.nombres.retain(|n| n.0 != id);
                    files.detach(id);
                    traza!("NF usb: disco {} quitado", id);
                }
            }
        }
        for a in files.take_drive_actions() {
            let disk = match &a {
                DriveAction::Eject { disk } | DriveAction::Format { disk, .. } | DriveAction::Encrypt { disk, .. } => *disk,
            };
            if disk & INTERNO != 0 {
                // `Files` ya quitó las letras del disco: se vuelve a montar tal cual. Formatear internos vendrá después.
                traza!("NF interno: disco {}: acción rechazada", disk & !INTERNO);
                if let Some(k) = self.internos.iter().position(|i| i.id == disk) {
                    self.montar_interno(files, k);
                }
                files.set_drive_result(Err(FsError::Denied));
                continue;
            }
            let r = match a {
                // `Files` ya escribió y desmontó: no queda nada que hacer salvo anotarlo.
                DriveAction::Eject { disk } => {
                    traza!("NF usb: disco {} expulsado", disk);
                    Ok(None)
                }
                DriveAction::Format { disk, kind, label } => self.rehacer(files, disk, Some(kind), &label, None, pool),
                DriveAction::Encrypt { disk, label, password } => self.rehacer(files, disk, None, &label, Some(&password), pool),
            };
            if let Err(e) = &r {
                traza!("NF usb: acción sobre la unidad fallida: {:?}", e);
            }
            files.set_drive_result(r);
        }
    }

    /// Formatea el disco entero (`kind`, o NightFury cifrado si hay `password`) y lo vuelve a montar.
    fn rehacer(&mut self, files: &mut Files, disk: u32, kind: Option<FormatKind>, label: &str, password: Option<&str>, pool: &mut Pool) -> Result<Option<String>, FsError> {
        let nombre = self.nombres.iter().find(|n| n.0 == disk).map(|n| n.1.clone()).ok_or(FsError::NotFound)?;
        // Por si `Files` no lo había soltado: nadie debe tener montado el disco mientras se reescribe.
        files.detach(disk);
        let mut dev = usb::abrir_disco(disk).ok_or(FsError::Disk)?;
        let now = rtc::now();
        let code = match (kind, password) {
            (Some(FormatKind::Fat32), None) => vol::format(&mut *dev, FsKind::Fat32, label, now).map(|()| None),
            (Some(FormatKind::ExFat), None) => vol::format(&mut *dev, FsKind::ExFat, label, now).map(|()| None),
            (Some(FormatKind::NightFury), None) => nfvol::format(dev, label, None, now).map_err(clave_a_fs),
            (None, Some(p)) => nfvol::format(dev, label, Some((p, pool)), now).map_err(clave_a_fs),
            _ => Err(FsError::Invalid),
        };
        if usb::abrir_disco(disk).is_none() {
            return Err(FsError::Disk);
        }
        // Un volumen recién cifrado queda bloqueado: se abre con la misma contraseña para que el
        // usuario no tenga que escribirla otra vez. Si falló (p. ej. contraseña débil, que se
        // comprueba antes de escribir), se vuelve a montar lo que había para que no desaparezca.
        conectar(files, disk, &nombre, |_| usb::abrir_disco(disk), password);
        code
    }
}

fn clave_a_fs(e: KeyError) -> FsError {
    match e {
        KeyError::Fs(e) => e,
        _ => FsError::Invalid,
    }
}

/// Monta cada partición del disco y la conecta a `files`. `clave` abre las cifradas. `abrir(ro)` da un
/// manejador nuevo del disco (cada montaje se queda con el suyo); `ro` pide uno de solo lectura.
/// Los discos internos (bit `INTERNO`) no son extraíbles y solo NightFury se monta con escritura.
fn conectar(files: &mut Files, id: u32, nombre: &str, abrir: impl Fn(bool) -> Option<Box<dyn SectorDev>>, clave: Option<&str>) {
    let interno = id & INTERNO != 0;
    let t = if interno { "interno" } else { "usb" };
    let id = id & !INTERNO;
    let Some(mut dev) = abrir(true) else { return };
    let ps = match vol::partitions(dev.as_mut()) {
        Ok(ps) => ps,
        Err(e) => {
            traza!("NF {}: disco {}: no se leen las particiones ({:?})", t, id, e);
            return;
        }
    };
    drop(dev);
    for (i, p) in ps.iter().enumerate() {
        // Cada partición pide su propio manejador: `mount` y `open` se quedan con el dispositivo.
        let disk = if interno { id | INTERNO } else { id };
        let Some(d) = abrir(interno && p.kind != FsKind::NightFury) else { break };
        let v = match p.kind {
            FsKind::NightFury => match nfvol::open(d, p) {
                Ok(nfvol::Opened::Ready(v)) => v,
                Ok(nfvol::Opened::Locked(drive)) => {
                    let drive = match clave {
                        Some(c) => match drive.unlock(c, false) {
                            Ok(v) => {
                                attach(files, disk, nombre, i, v);
                                continue;
                            }
                            Err((drive, e)) => {
                                traza!("NF {}: disco {} partición {}: no se abre con la contraseña ({:?})", t, id, i, e);
                                drive
                            }
                        },
                        None => drive,
                    };
                    let letra = files.attach_locked(Locked { disk, removable: !interno, device: String::from(nombre), drive });
                    traza!("NF {}: disco {} partición {}: NightFury cifrada -> {:?}", t, id, i, letra);
                    continue;
                }
                Err(e) => {
                    traza!("NF {}: disco {} partición {}: no se abre ({:?})", t, id, i, e);
                    continue;
                }
            },
            // shortcut: las particiones de formato desconocido solo se trazan (no sale la unidad «no compatible»), cuando haga falta mostrarla.
            FsKind::Unknown => {
                traza!("NF {}: disco {} partición {}: formato no compatible ({} sectores desde {})", t, id, i, p.sectors, p.start);
                continue;
            }
            _ => match vol::mount(d, p) {
                // El dispositivo ya es de solo lectura; el envoltorio cubre además los volúmenes que no lo respetan.
                Ok(v) if interno => Box::new(vol::ReadOnly(v)),
                Ok(v) => v,
                Err(e) => {
                    traza!("NF {}: disco {} partición {} ({:?}, {} sectores desde {}): no se monta ({:?})", t, id, i, p.kind, p.sectors, p.start, e);
                    continue;
                }
            },
        };
        attach(files, disk, nombre, i, v);
    }
}

fn attach(files: &mut Files, disk: u32, nombre: &str, i: usize, mut v: Box<dyn vol::Volume>) {
    let interno = disk & INTERNO != 0;
    let t = if interno { "interno" } else { "usb" };
    let (total, libre) = v.space();
    let kind = v.kind();
    let etiqueta = v.label();
    let ro = v.read_only();
    if let Some(aviso) = v.notice() {
        traza!("NF {}: disco {} partición {}: {}", t, disk & !INTERNO, i, aviso);
    }
    let letra = files.attach_as(disk, nombre, v, !interno);
    traza!("NF {}: disco {} partición {}: {:?} «{}» {} MiB, {} MiB libres{} -> {:?}", t, disk & !INTERNO, i, kind, etiqueta, total >> 20, libre >> 20, if ro { ", solo lectura" } else { "" }, letra);
}
