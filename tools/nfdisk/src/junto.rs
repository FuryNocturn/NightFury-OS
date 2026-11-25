// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Pruebas de «instalar junto a Windows/Linux» con QEMU:
//!   nfdisk disco-prueba <imagen> <windows|windows-datos|linux|bitlocker|mbr> <MiB> [efi_prueba]
//!       crea un disco falso (archivo nuevo, sectores de 512) con ese otro sistema; `efi_prueba` es lo que
//!       llevará como cargador de Windows o Linux (si falta, un texto corto)
//!   nfdisk analizar <imagen>   lo que NightFury ofrecería en ese disco (no escribe nada)
use super::usb::{azar, Pendrive};
use super::{fs_err, R};
use nf_core::platform::DateTime;
use nf_core::vol::imagenes::{self, Prueba};
use nf_core::vol::instalar::{self, Lugar};
use nf_core::vol::SectorDev;
use std::fs::OpenOptions;

const USO: &str = "Uso: nfdisk disco-prueba <imagen> <windows|windows-datos|linux|bitlocker|mbr> <MiB> [efi_prueba]";

pub fn disco_prueba(args: &[&str], now: DateTime) -> R<()> {
    let (img, tipo, mib, efi) = match args {
        [i, t, m] => (i, t, m, None),
        [i, t, m, e] => (i, t, m, Some(e)),
        _ => return Err(USO.into()),
    };
    let tipo = Prueba::de_texto(tipo).ok_or(USO)?;
    let mib: u64 = mib.parse().ok().filter(|m| (1..=1 << 22).contains(m)).ok_or("Los MiB deben ser un número entre 1 y 4194304.")?;
    let efi = match efi {
        Some(p) => std::fs::read(p).map_err(|e| format!("No se puede leer «{p}»: {e}"))?,
        None => b"MZ cargador de prueba (nfdisk disco-prueba)".to_vec(),
    };
    let f = OpenOptions::new().read(true).write(true).create_new(true).open(img).map_err(|e| format!("No se puede crear «{img}»: {e}"))?;
    f.set_len(mib << 20).map_err(|e| e.to_string())?;
    let mut abrir = || Box::new(Pendrive { f: f.try_clone().expect("duplicar el manejador"), sectors: mib << 11 }) as Box<dyn SectorDev>;
    let r = imagenes::crear(&mut abrir, tipo, &efi, now, &mut azar()).map_err(fs_err);
    if r.is_err() {
        let _ = std::fs::remove_file(img);
    }
    r?;
    println!("Imagen {img} creada ({mib} MiB, tipo {tipo:?}). Mira lo que ofrece con: nfdisk analizar {img}");
    Ok(())
}

pub fn analizar(args: &[&str]) -> R<()> {
    let [img] = args else { return Err("Uso: nfdisk analizar <imagen>".into()) };
    let p = Pendrive::abrir(img)?;
    let (f, sectors) = (p.f, p.sectors);
    let a = instalar::analizar(&mut || Box::new(Pendrive { f: f.try_clone().expect("duplicar el manejador"), sectors }) as Box<dyn SectorDev>);
    println!("Disco: {}", a.bloqueo_disco.map_or("se puede instalar junto a lo que hay".to_string(), |b| format!("NO se instala: {b}")));
    match &a.esp {
        Some(e) => println!("ESP: partición {} desde el sector {}, {} MiB", e.numero, e.inicio_lba, e.sectores / 2048),
        None => println!("ESP: no hay (se crearía una de 256 MiB al principio del hueco)"),
    }
    println!("NightFury ya instalado: {}", if a.nightfury { "sí" } else { "no" });
    for (i, s) in a.sitios.iter().enumerate() {
        let que = match &s.lugar {
            Lugar::Hueco => "hueco libre".to_string(),
            Lugar::Particion { numero, fs, etiqueta } => format!("partición {numero} ({fs:?}, «{etiqueta}»)"),
        };
        println!("  {}. {que}, desde el sector {}, {} MiB: {}", i + 1, s.inicio_lba, s.sectores / 2048, s.bloqueo.map_or("DISPONIBLE".to_string(), |b| format!("bloqueado, {b}")));
    }
    if a.sitios.is_empty() {
        println!("  (ningún sitio: hace falta un hueco o una partición de datos de 2 GiB como mínimo)");
    }
    Ok(())
}
