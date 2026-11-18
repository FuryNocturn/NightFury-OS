// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Imágenes de pendrive para probar el almacenamiento masivo USB de NightFury en QEMU:
//!   nfdisk usb <imagen> <fat32|exfat> <MiB>   crea y formatea una imagen con carpetas y archivos de prueba
//!   nfdisk usb-ver <imagen>                   lista lo que hay (y el CRC de cada archivo) tras usarla en QEMU
//! Solo crea archivos regulares nuevos (`usb`) y nunca escribe en dispositivos ni unidades.
use nf_core::fs::FsError;
use nf_core::platform::DateTime;
use nf_core::vol::{self, part::crc32, FsKind, SectorDev};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};

type R<T> = Result<T, String>;

/// Disco de sectores de 512 bytes sobre un archivo regular.
struct Pendrive {
    f: File,
    sectors: u64,
}

impl Pendrive {
    fn abrir(path: &str) -> R<Pendrive> {
        let f = OpenOptions::new().read(true).write(true).open(path).map_err(|e| format!("No se puede abrir «{path}»: {e}"))?;
        let m = f.metadata().map_err(|e| e.to_string())?;
        if !m.is_file() || m.len() == 0 || m.len() % 512 != 0 {
            return Err(format!("«{path}» no es un archivo regular de sectores de 512 bytes."));
        }
        Ok(Pendrive { f, sectors: m.len() / 512 })
    }
}

impl SectorDev for Pendrive {
    fn sector_size(&self) -> usize {
        512
    }
    fn sectors(&self) -> u64 {
        self.sectors
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        if buf.len() % 512 != 0 || lba + (buf.len() / 512) as u64 > self.sectors {
            return Err(FsError::Disk);
        }
        self.f.seek(SeekFrom::Start(lba * 512)).map_err(|_| FsError::Disk)?;
        self.f.read_exact(buf).map_err(|_| FsError::Disk)
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError> {
        if buf.len() % 512 != 0 || lba + (buf.len() / 512) as u64 > self.sectors {
            return Err(FsError::Disk);
        }
        self.f.seek(SeekFrom::Start(lba * 512)).map_err(|_| FsError::Disk)?;
        self.f.write_all(buf).map_err(|_| FsError::Disk)
    }
    fn flush(&mut self) -> Result<(), FsError> {
        self.f.sync_all().map_err(|_| FsError::Disk)
    }
}

fn fs_err(e: FsError) -> String {
    e.message().to_string()
}

/// Bytes de prueba reproducibles (para comparar lecturas de varios trozos de 64 KiB).
fn patron(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i.wrapping_mul(31) ^ (i >> 8)) as u8).collect()
}

fn montar(path: &str) -> R<Vec<(FsKind, Box<dyn vol::Volume>)>> {
    let mut d = Pendrive::abrir(path)?;
    let mut v = Vec::new();
    for p in vol::partitions(&mut d).map_err(fs_err)? {
        let m = vol::mount(Box::new(Pendrive::abrir(path)?), &p).map_err(fs_err)?;
        v.push((p.kind, m));
    }
    Ok(v)
}

pub fn crear(args: &[&str], now: DateTime) -> R<()> {
    let [img, tipo, mib] = args else { return Err("Uso: nfdisk usb <imagen> <fat32|exfat> <MiB>".into()) };
    let kind = match *tipo {
        "fat32" => FsKind::Fat32,
        "exfat" => FsKind::ExFat,
        _ => return Err("El sistema de archivos debe ser fat32 o exfat.".into()),
    };
    let mib: u64 = mib.parse().map_err(|_| "El tamaño debe ser un número de MiB.".to_string())?;
    if !(8..=65_536).contains(&mib) {
        return Err("El tamaño debe estar entre 8 y 65536 MiB.".into());
    }
    let f = OpenOptions::new().read(true).write(true).create_new(true).open(img).map_err(|e| format!("No se puede crear «{img}»: {e}"))?;
    f.set_len(mib << 20).map_err(|e| e.to_string())?;
    drop(f);
    let r = (|| -> R<()> {
        vol::format(&mut Pendrive::abrir(img)?, kind, "PRUEBA", now).map_err(fs_err)?;
        let mut m = montar(img)?.pop().ok_or("La imagen formateada no tiene particiones.")?.1;
        for d in ["Documentos", "Fotos", "Fotos\\Viajes"] {
            m.mkdir(d, now).map_err(fs_err)?;
        }
        let ficheros: [(&str, Vec<u8>); 4] = [
            ("LEEME.TXT", b"Pendrive de prueba de NightFury.\r\n".to_vec()),
            ("Documentos\\nota.txt", "Nota con acentos: canción, niño.\r\n".as_bytes().to_vec()),
            ("Fotos\\Viajes\\playa.txt", b"playa".to_vec()),
            ("datos.bin", patron(300_000)),
        ];
        for (ruta, datos) in &ficheros {
            m.write(ruta, datos, now).map_err(fs_err)?;
        }
        m.flush().map_err(fs_err)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(img);
    }
    r?;
    println!("Imagen {img} creada ({tipo}, {mib} MiB). QEMU: run.ps1 -Usb {img}");
    Ok(())
}

fn listar(m: &mut dyn vol::Volume, dir: &str, nivel: usize) -> R<()> {
    let mut es = m.list(dir).map_err(fs_err)?;
    es.sort_by(|a, b| a.name.cmp(&b.name));
    for e in es {
        let ruta = if dir.is_empty() { e.name.clone() } else { format!("{dir}\\{}", e.name) };
        if e.dir {
            println!("{:n$}{}\\", "", e.name, n = nivel * 2);
            listar(m, &ruta, nivel + 1)?;
        } else {
            let crc = m.read(&ruta).map(|d| format!("{:08x}", crc32(&d))).unwrap_or_else(|e| e.message().to_string());
            println!("{:n$}{}  {} bytes  crc32 {crc}", "", e.name, e.size, n = nivel * 2);
        }
    }
    Ok(())
}

pub fn ver(args: &[&str]) -> R<()> {
    let [img] = args else { return Err("Uso: nfdisk usb-ver <imagen>".into()) };
    for (kind, mut m) in montar(img)? {
        let (total, libre) = m.space();
        println!("{:?} «{}»: {} bytes, {} libres", kind, m.label(), total, libre);
        listar(m.as_mut(), "", 1)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crea_y_lee_fat32_y_exfat() {
        let now = DateTime { year: 2026, month: 11, day: 18, hour: 12, minute: 0, second: 0 };
        for t in ["fat32", "exfat"] {
            let p = std::env::temp_dir().join(format!("nfdisk-usb-{}-{t}.img", std::process::id())).to_string_lossy().into_owned();
            let _ = std::fs::remove_file(&p);
            crear(&[&p, t, "64"], now).unwrap();
            assert!(crear(&[&p, t, "64"], now).is_err(), "no pisa una imagen");
            let mut v = montar(&p).unwrap();
            let (_, m) = &mut v[0];
            assert_eq!(m.read("datos.bin").unwrap(), patron(300_000));
            assert_eq!(m.read("Fotos\\Viajes\\playa.txt").unwrap(), b"playa");
            ver(&[&p]).unwrap();
            std::fs::remove_file(&p).unwrap();
        }
    }
}
