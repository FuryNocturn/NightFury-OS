// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Unidades extraíbles: engancha los discos USB que aparecen y desaparecen a `Files` (letras D:, E:…)
//! y ejecuta las acciones sobre el disco entero que piden los programas del sistema (expulsar,
//! formatear, cifrar), que solo puede hacer el núcleo. En los discos internos formatear y cifrar
//! tocan solo una partición (por una `Window`); lo demás del disco queda intacto.
use crate::{ahci, nvme, pendrive::Pen, rtc, usb};
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
use nf_core::vol::instalar::{self, Esp};
use nf_core::vol::{self, nfvol, FsKind, SectorDev, Window};

/// Los discos internos (SATA y NVMe) llevan este bit en su id para no chocar con los de USB.
const INTERNO: u32 = 1 << 31;

/// Un disco o espacio de nombres interno; las particiones lo comparten por `Rc`.
pub enum Fisico {
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

pub struct Interno {
    id: u32,
    nombre: String,
    /// Número de serie (para identificar el disco cuando se formateen particiones internas).
    serie: String,
    pub fis: Rc<RefCell<Fisico>>,
}

/// Menos que esto no cabe (ESP de 256 MiB + datos): se rechaza antes de escribir nada.
const MIN_INSTALAR: u64 = 1 << 30;

/// Numera los discos internos (SATA primero, luego NVMe) y los deja listos para repartirlos.
pub fn preparar(sata: Vec<Box<ahci::Disco>>, nvme: Vec<nvme::Espacio>) -> Vec<Interno> {
    let todos = sata.into_iter().map(|d| (String::from(d.modelo()), String::from(d.serie()), Fisico::Sata(d))).chain(nvme.into_iter().map(|n| (String::from(n.modelo()), String::from(n.serie()), Fisico::Nvme(n))));
    todos.enumerate().map(|(i, (nombre, serie, f))| Interno { id: INTERNO | i as u32, nombre, serie, fis: Rc::new(RefCell::new(f)) }).collect()
}

impl Interno {
    pub fn modelo(&self) -> &str {
        &self.nombre
    }
    pub fn bytes(&self) -> u64 {
        let f = self.fis.borrow();
        f.sectors().saturating_mul(f.sector_size() as u64)
    }
    pub fn manejador(&self, solo_lectura: bool) -> Manejador {
        Manejador { fis: self.fis.clone(), ro: solo_lectura }
    }
    /// El disco SATA, si este lo es y nadie más lo comparte (para usarlo entero como C:, como siempre).
    pub fn en_sata(self) -> Option<Box<ahci::Disco>> {
        match Rc::try_unwrap(self.fis).ok()?.into_inner() {
            Fisico::Sata(d) => Some(d),
            Fisico::Nvme(_) => None,
        }
    }
    /// Si el disco lleva una instalación de NightFury: la partición NFFS que nombra `\EFI\NightFury\particion`
    /// de su ESP (o, sin ese archivo, la primera NFFS): su (primer sector, sectores).
    pub fn instalada(&self) -> Option<(u64, u64)> {
        instalar::particion_nightfury(&mut || Box::new(self.manejador(true)) as Box<dyn SectorDev>)
    }
    /// Lo que tiene, por partición (formato y bytes), para la lista del instalador. Vacío si está a cero.
    pub fn contenido(&self) -> Vec<(FsKind, u64)> {
        let mut m = self.manejador(true);
        let ss = m.sector_size();
        let ps = vol::partitions(&mut m).unwrap_or_default();
        // El primer MiB a cero (sin tabla ni cabeceras) y nada reconocible: disco vacío.
        let mut b = alloc::vec![1u8; (1 << 20).min(ss * m.sectors().min(u32::MAX as u64) as usize)];
        let a_cero = !b.is_empty() && m.read(0, &mut b).is_ok() && b.iter().all(|&x| x == 0);
        if a_cero && ps.iter().all(|p| p.kind == FsKind::Unknown) {
            return Vec::new();
        }
        if ps.is_empty() {
            return alloc::vec![(FsKind::Unknown, self.bytes())];
        }
        ps.iter().map(|p| (p.kind, p.sectors.saturating_mul(ss as u64))).collect()
    }
    /// Instala NightFury en TODO el disco con `efi` como cargador. `NoSpace` (sin escribir nada) si no llega a 1 GiB.
    pub fn instalar(&self, efi: &[u8], version: &str) -> Result<Esp, FsError> {
        if self.bytes() < MIN_INSTALAR {
            return Err(FsError::NoSpace);
        }
        let mut abrir = || Box::new(self.manejador(false)) as Box<dyn SectorDev>;
        // shortcut: sin `arranque.efi` aparte (va vacío y el núcleo hace de BOOTX64); pasarlo aquí cuando el pendrive lo lleve.
        let esp = instalar::instalar(&mut abrir, efi, &[], version, rtc::now(), &mut crate::entropia)?;
        abrir().flush()?;
        Ok(esp)
    }
}

/// Manejador de un disco interno; `ro` lo deja de solo lectura (todo lo que no es NightFury ni FAT/exFAT que NightFury formateó).
pub struct Manejador {
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
    /// Modo Probar: ningún disco interno se escribe, sea cual sea su formato.
    solo_lectura: bool,
}

impl Unidades {
    /// Engancha las particiones de los discos internos (SATA que no son el del sistema, y NVMe).
    /// Se llama una vez al arrancar, antes que ningún USB.
    pub fn internos(&mut self, files: &mut Files, lista: Vec<Interno>, solo_lectura: bool) {
        self.solo_lectura = solo_lectura;
        self.internos = lista;
        for k in 0..self.internos.len() {
            let it = &self.internos[k];
            traza!("NF interno: disco {} «{}» serie «{}»: {} sectores de {} bytes", it.id & !INTERNO, it.nombre, it.serie, it.fis.borrow().sectors(), it.fis.borrow().sector_size());
            self.montar_interno(files, k, None);
        }
    }

    /// Monta las particiones del disco interno `k`, o solo la que empieza en `solo`.
    fn montar_interno(&self, files: &mut Files, k: usize, solo: Option<u64>) {
        let it = &self.internos[k];
        let abrir = |ro| Some(Box::new(Manejador { fis: it.fis.clone(), ro: ro || self.solo_lectura }) as Box<dyn SectorDev>);
        conectar(files, it.id, &it.nombre, abrir, None, solo, &it.serie);
    }

    /// Altas y bajas de discos y acciones pendientes. Se llama en cada vuelta del bucle principal.
    pub fn sondear(&mut self, files: &mut Files, pool: &mut Pool, pen: &mut Pen) {
        for c in usb::cambios_discos() {
            match c {
                usb::CambioDisco::Conectado { id, nombre, dev } => {
                    // El pendrive del que se arrancó no se monta nunca (ni como D: ni su ESP).
                    if pen.es_arranque(id) {
                        continue;
                    }
                    traza!("NF usb: disco {} «{}»: {} sectores de {} bytes{}", id, nombre, dev.sectors(), dev.sector_size(), if dev.read_only() { ", solo lectura" } else { "" });
                    self.nombres.retain(|n| n.0 != id);
                    self.nombres.push((id, nombre.clone()));
                    conectar(files, id, &nombre, |_| usb::abrir_disco(id), None, None, "");
                }
                usb::CambioDisco::Quitado { id } => {
                    pen.quitado(id);
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
                // `Files` ya quitó solo la letra de esa partición. Expulsar un interno no llega (lo deniega `Files`).
                let r = match a {
                    DriveAction::Format { disk, part, kind, label } => self.rehacer_interno(files, disk, part, Some(kind), &label, None, pool),
                    DriveAction::Encrypt { disk, part, label, password } => self.rehacer_interno(files, disk, part, None, &label, Some(&password), pool),
                    DriveAction::Eject { .. } => Err(FsError::Denied),
                };
                if let Err(e) = &r {
                    traza!("NF interno: acción sobre la partición fallida: {:?}", e);
                }
                files.set_drive_result(r);
                continue;
            }
            let r = match a {
                // `Files` ya escribió y desmontó: no queda nada que hacer salvo anotarlo.
                DriveAction::Eject { disk } => {
                    traza!("NF usb: disco {} expulsado", disk);
                    Ok(None)
                }
                DriveAction::Format { disk, kind, label, .. } => self.rehacer(files, disk, Some(kind), &label, None, pool),
                DriveAction::Encrypt { disk, label, password, .. } => self.rehacer(files, disk, None, &label, Some(&password), pool),
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
        conectar(files, disk, &nombre, |_| usb::abrir_disco(disk), password, None, "");
        code
    }

    /// Formatea (`kind`, o NightFury cifrado si hay `password`) SOLO la partición interna que empieza en
    /// `part`: escribe dentro de sus sectores y, en la tabla, únicamente el tipo de partición. Después
    /// vuelve a montar esa partición. Si falla, vuelve a montar lo que hubiera.
    fn rehacer_interno(&self, files: &mut Files, disk: u32, part: u64, kind: Option<FormatKind>, label: &str, password: Option<&str>, pool: &mut Pool) -> Result<Option<String>, FsError> {
        // Defensa: solo discos de la lista de internos. El del sistema (C:) nunca está en ella (`elegir_disco` lo aparta).
        let k = self.internos.iter().position(|i| i.id == disk).ok_or(FsError::NotFound)?;
        let it = &self.internos[k];
        let man = |ro| Manejador { fis: it.fis.clone(), ro: ro || self.solo_lectura };
        let r = (|| {
            let p = vol::partitions(&mut man(true))?.into_iter().find(|p| p.start == part).ok_or(FsError::NotFound)?;
            let win = || Box::new(Window::new(Box::new(man(false)), p.start, p.sectors)) as Box<dyn SectorDev>;
            let now = rtc::now();
            let code = match (kind, password) {
                (Some(FormatKind::Fat32), None) => vol::format_in(&mut *win(), FsKind::Fat32, label, now).map(|()| None),
                (Some(FormatKind::ExFat), None) => vol::format_in(&mut *win(), FsKind::ExFat, label, now).map(|()| None),
                (Some(FormatKind::NightFury), None) => nfvol::format_in(win(), label, None, now).map_err(clave_a_fs),
                (None, Some(pw)) => nfvol::format_in(win(), label, Some((pw, pool)), now).map_err(clave_a_fs),
                _ => Err(FsError::Invalid),
            }?;
            // Tipo de partición según lo que quedó dentro; NightFury siempre escribe, FAT/exFAT desde ahora también.
            let nuevo = vol::partitions(&mut man(true))?.into_iter().find(|q| q.start == p.start).map_or(FsKind::Unknown, |q| q.kind);
            if let Err(e) = vol::set_partition_type(&mut man(false), p.start, nuevo) {
                traza!("NF interno: disco {}: no se pudo ajustar el tipo de partición ({:?})", disk & !INTERNO, e);
            }
            let escribible = matches!(nuevo, FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32 | FsKind::ExFat);
            if let Err(e) = files.set_internal_writable(&it.serie, p.start, escribible, now) {
                traza!("NF interno: no se pudo anotar unidades.cfg ({:?})", e);
            }
            Ok(code)
        })();
        self.montar_interno(files, k, Some(part));
        r
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
/// Los discos internos (bit `INTERNO`) no son extraíbles y solo se montan con escritura NightFury y las
/// FAT/exFAT apuntadas en `unidades.cfg` (las que NightFury formateó; `serie` identifica el disco).
/// `solo`: monta únicamente la partición que empieza en ese sector.
fn conectar(files: &mut Files, id: u32, nombre: &str, abrir: impl Fn(bool) -> Option<Box<dyn SectorDev>>, clave: Option<&str>, solo: Option<u64>, serie: &str) {
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
        if solo.is_some_and(|s| s != p.start) {
            continue;
        }
        // Cada partición pide su propio manejador: `mount` y `open` se quedan con el dispositivo.
        let disk = if interno { id | INTERNO } else { id };
        let rw = !interno || p.kind == FsKind::NightFury || (matches!(p.kind, FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32 | FsKind::ExFat) && files.internal_writable(serie, p.start));
        let Some(d) = abrir(!rw) else { break };
        let v = match p.kind {
            FsKind::NightFury => match nfvol::open(d, p) {
                Ok(nfvol::Opened::Ready(v)) => v,
                Ok(nfvol::Opened::Locked(drive)) => {
                    let drive = match clave {
                        Some(c) => match drive.unlock(c, false) {
                            Ok(v) => {
                                attach(files, disk, p.start, nombre, i, v);
                                continue;
                            }
                            Err((drive, e)) => {
                                traza!("NF {}: disco {} partición {}: no se abre con la contraseña ({:?})", t, id, i, e);
                                drive
                            }
                        },
                        None => drive,
                    };
                    let letra = files.attach_locked(Locked { disk, part: p.start, removable: !interno, device: String::from(nombre), drive });
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
                Ok(v) if interno && !rw => Box::new(vol::ReadOnly(v)),
                Ok(v) => v,
                Err(e) => {
                    traza!("NF {}: disco {} partición {} ({:?}, {} sectores desde {}): no se monta ({:?})", t, id, i, p.kind, p.sectors, p.start, e);
                    continue;
                }
            },
        };
        attach(files, disk, p.start, nombre, i, v);
    }
}

fn attach(files: &mut Files, disk: u32, start: u64, nombre: &str, i: usize, mut v: Box<dyn vol::Volume>) {
    let interno = disk & INTERNO != 0;
    let t = if interno { "interno" } else { "usb" };
    let (total, libre) = v.space();
    let kind = v.kind();
    let etiqueta = v.label();
    let ro = v.read_only();
    if let Some(aviso) = v.notice() {
        traza!("NF {}: disco {} partición {}: {}", t, disk & !INTERNO, i, aviso);
    }
    let letra = files.attach_part(disk, start, nombre, v, !interno);
    traza!("NF {}: disco {} partición {}: {:?} «{}» {} MiB, {} MiB libres{} -> {:?}", t, disk & !INTERNO, i, kind, etiqueta, total >> 20, libre >> 20, if ro { ", solo lectura" } else { "" }, letra);
}
