// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Unidades extraíbles: engancha los discos USB que aparecen y desaparecen a `Files` (letras D:, E:…)
//! y ejecuta las acciones sobre el disco entero que piden los programas del sistema (expulsar,
//! formatear, cifrar), que solo puede hacer el núcleo.
use crate::{rtc, usb};
use crate::serial::traza;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use nf_core::drives::{DriveAction, FormatKind, Locked};
use nf_core::files::Files;
use nf_core::fs::FsError;
use nf_core::keys::{KeyError, Pool};
use nf_core::vol::{self, nfvol, FsKind, SectorDev};

/// Nombre de cada disco conectado (para volver a montarlo tras formatearlo o cifrarlo).
#[derive(Default)]
pub struct Unidades {
    nombres: Vec<(u32, String)>,
}

impl Unidades {
    /// Altas y bajas de discos y acciones pendientes. Se llama en cada vuelta del bucle principal.
    pub fn sondear(&mut self, files: &mut Files, pool: &mut Pool) {
        for c in usb::cambios_discos() {
            match c {
                usb::CambioDisco::Conectado { id, nombre, dev } => {
                    traza!("NF usb: disco {} «{}»: {} sectores de {} bytes{}", id, nombre, dev.sectors(), dev.sector_size(), if dev.read_only() { ", solo lectura" } else { "" });
                    self.nombres.retain(|n| n.0 != id);
                    self.nombres.push((id, nombre.clone()));
                    conectar(files, id, &nombre, dev, None);
                }
                usb::CambioDisco::Quitado { id } => {
                    self.nombres.retain(|n| n.0 != id);
                    files.detach(id);
                    traza!("NF usb: disco {} quitado", id);
                }
            }
        }
        for a in files.take_drive_actions() {
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
        let Some(dev) = usb::abrir_disco(disk) else { return Err(FsError::Disk) };
        // Un volumen recién cifrado queda bloqueado: se abre con la misma contraseña para que el
        // usuario no tenga que escribirla otra vez. Si falló (p. ej. contraseña débil, que se
        // comprueba antes de escribir), se vuelve a montar lo que había para que no desaparezca.
        conectar(files, disk, &nombre, dev, password);
        code
    }
}

fn clave_a_fs(e: KeyError) -> FsError {
    match e {
        KeyError::Fs(e) => e,
        _ => FsError::Invalid,
    }
}

/// Monta cada partición del disco y la conecta a `files`. `clave` abre las cifradas.
fn conectar(files: &mut Files, id: u32, nombre: &str, mut dev: Box<dyn SectorDev>, clave: Option<&str>) {
    let ps = match vol::partitions(dev.as_mut()) {
        Ok(ps) => ps,
        Err(e) => {
            traza!("NF usb: disco {}: no se leen las particiones ({:?})", id, e);
            return;
        }
    };
    drop(dev);
    for (i, p) in ps.iter().enumerate() {
        // Cada partición pide su propio manejador: `mount` y `open` se quedan con el dispositivo.
        let Some(d) = usb::abrir_disco(id) else { break };
        let v = match p.kind {
            FsKind::NightFury => match nfvol::open(d, p) {
                Ok(nfvol::Opened::Ready(v)) => v,
                Ok(nfvol::Opened::Locked(drive)) => {
                    let drive = match clave {
                        Some(c) => match drive.unlock(c, false) {
                            Ok(v) => {
                                attach(files, id, nombre, i, v);
                                continue;
                            }
                            Err((drive, e)) => {
                                traza!("NF usb: disco {} partición {}: no se abre con la contraseña ({:?})", id, i, e);
                                drive
                            }
                        },
                        None => drive,
                    };
                    let letra = files.attach_locked(Locked { disk: id, device: String::from(nombre), drive });
                    traza!("NF usb: disco {} partición {}: NightFury cifrada -> {:?}", id, i, letra);
                    continue;
                }
                Err(e) => {
                    traza!("NF usb: disco {} partición {}: no se abre ({:?})", id, i, e);
                    continue;
                }
            },
            // shortcut: las particiones de formato desconocido solo se trazan (no sale la unidad «no compatible»), cuando haga falta mostrarla.
            FsKind::Unknown => {
                traza!("NF usb: disco {} partición {}: formato no compatible ({} sectores desde {})", id, i, p.sectors, p.start);
                continue;
            }
            _ => match vol::mount(d, p) {
                Ok(v) => v,
                Err(e) => {
                    traza!("NF usb: disco {} partición {} ({:?}, {} sectores desde {}): no se monta ({:?})", id, i, p.kind, p.sectors, p.start, e);
                    continue;
                }
            },
        };
        attach(files, id, nombre, i, v);
    }
}

fn attach(files: &mut Files, id: u32, nombre: &str, i: usize, mut v: Box<dyn vol::Volume>) {
    let (total, libre) = v.space();
    let kind = v.kind();
    let etiqueta = v.label();
    let ro = v.read_only();
    let letra = files.attach(id, nombre, v);
    traza!("NF usb: disco {} partición {}: {:?} «{}» {} MiB, {} MiB libres{} -> {:?}", id, i, kind, etiqueta, total >> 20, libre >> 20, if ro { ", solo lectura" } else { "" }, letra);
}
