// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Imágenes de pendrive para probar el almacenamiento masivo USB de NightFury en QEMU:
//!   nfdisk usb <imagen> <fat32|exfat|nightfury> <MiB>   crea y formatea una imagen con carpetas y archivos de prueba
//!   nfdisk usb <imagen> nightfury-cifrado <MiB> <contraseña>   ídem, NightFury cifrada (imprime el código de recuperación)
//!   nfdisk pen-arranque <salida.img> <BOOTX64.EFI> [versión]   imagen de 66 MiB de pendrive de arranque UEFI (se graba tal cual en un pendrive)
//!   nfdisk usb-ver <imagen>                   lista lo que hay (y el CRC de cada archivo) tras usarla en QEMU;
//!                                             las NightFury cifradas se abren con la variable NFDISK_CLAVE
//! Solo crea archivos regulares nuevos (`usb`) y nunca escribe en dispositivos ni unidades.
use nf_core::fs::FsError;
use nf_core::platform::DateTime;
use nf_core::vol::{self, nfvol, part::crc32, FsKind, SectorDev};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};

type R<T> = Result<T, String>;

/// Disco de sectores de 512 bytes sobre un archivo regular.
pub(crate) struct Pendrive {
    pub(crate) f: File,
    pub(crate) sectors: u64,
}

impl Pendrive {
    pub(crate) fn abrir(path: &str) -> R<Pendrive> {
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

fn montar(path: &str, clave: Option<&str>) -> R<Vec<(FsKind, Box<dyn vol::Volume>)>> {
    let mut d = Pendrive::abrir(path)?;
    let mut v = Vec::new();
    for p in vol::partitions(&mut d).map_err(fs_err)? {
        let m = match p.kind {
            FsKind::NightFury => match nfvol::open(Box::new(Pendrive::abrir(path)?), &p).map_err(fs_err)? {
                nfvol::Opened::Ready(m) => m,
                nfvol::Opened::Locked(d) => {
                    let clave = clave.ok_or("Unidad NightFury cifrada: la clave va en la variable NFDISK_CLAVE.")?;
                    d.unlock(clave, false).map_err(|(_, e)| e.text().to_string())?
                }
            },
            _ => vol::mount(Box::new(Pendrive::abrir(path)?), &p).map_err(fs_err)?,
        };
        v.push((p.kind, m));
    }
    Ok(v)
}

pub fn crear(args: &[&str], now: DateTime) -> R<()> {
    let (img, tipo, mib, clave) = match args {
        [img, tipo, mib] => (img, tipo, mib, None),
        [img, "nightfury-cifrado", mib, clave] => (img, &"nightfury-cifrado", mib, Some(*clave)),
        _ => return Err("Uso: nfdisk usb <imagen> <fat32|exfat|nightfury> <MiB>  o  nfdisk usb <imagen> nightfury-cifrado <MiB> <contraseña>".into()),
    };
    let kind = match *tipo {
        "fat32" => FsKind::Fat32,
        "exfat" => FsKind::ExFat,
        "nightfury" | "nightfury-cifrado" => FsKind::NightFury,
        _ => return Err("El sistema de archivos debe ser fat32, exfat, nightfury o nightfury-cifrado.".into()),
    };
    if *tipo == "nightfury-cifrado" && clave.is_none() {
        return Err("nightfury-cifrado necesita la contraseña: nfdisk usb <imagen> nightfury-cifrado <MiB> <contraseña>".into());
    }
    let mib: u64 = mib.parse().map_err(|_| "El tamaño debe ser un número de MiB.".to_string())?;
    if !(8..=65_536).contains(&mib) {
        return Err("El tamaño debe estar entre 8 y 65536 MiB.".into());
    }
    let f = OpenOptions::new().read(true).write(true).create_new(true).open(img).map_err(|e| format!("No se puede crear «{img}»: {e}"))?;
    f.set_len(mib << 20).map_err(|e| e.to_string())?;
    drop(f);
    let mut codigo = None;
    let r = (|| -> R<()> {
        if kind == FsKind::NightFury {
            // shortcut: imagen solo de prueba: el azar sale de la hora, no de un acumulador real; la contraseña va por argumento (para QEMU, no para datos reales).
            let mut pool = nf_core::keys::Pool::new();
            for i in 0..nf_core::keys::POOL_EVENTS_NEEDED as i32 {
                pool.add_event(i, i * 7, i as u64, i as u64 * 13);
            }
            pool.add(&std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()).to_le_bytes());
            codigo = nfvol::format(Box::new(Pendrive::abrir(img)?), "PRUEBA", clave.map(|c| (c, &mut pool)), now).map_err(|e| e.text().to_string())?;
        } else {
            vol::format(&mut Pendrive::abrir(img)?, kind, "PRUEBA", now).map_err(fs_err)?;
        }
        let mut m = montar(img, clave)?.pop().ok_or("La imagen formateada no tiene particiones.")?.1;
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
    if let Some(c) = codigo {
        println!("Código de recuperación: {c}");
    }
    Ok(())
}

/// Pendrive de arranque: imagen NUEVA de 66 MiB (ESP de 64 MiB + hueco para el GPT de respaldo).
/// shortcut: azar = hora + pid (basta para GUID únicos entre imágenes, no para secretos); splitmix64.
pub(crate) fn azar() -> impl FnMut() -> u64 {
    let mut x = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64) ^ (std::process::id() as u64) << 32;
    move || {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ z >> 30).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ z >> 27).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ z >> 31
    }
}

pub fn pen_arranque(args: &[&str], now: DateTime) -> R<()> {
    let (img, efi, version) = match args {
        [img, efi] => (img, efi, env!("CARGO_PKG_VERSION")),
        [img, efi, v] => (img, efi, *v),
        _ => return Err("Uso: nfdisk pen-arranque <salida.img> <BOOTX64.EFI> [versión]".into()),
    };
    let efi = std::fs::read(efi).map_err(|e| format!("No se puede leer «{efi}»: {e}"))?;
    let f = OpenOptions::new().read(true).write(true).create_new(true).open(img).map_err(|e| format!("No se puede crear «{img}»: {e}"))?;
    f.set_len(66 << 20).map_err(|e| e.to_string())?;
    let mut azar = azar();
    let sectors = 66 << 11;
    let mut abrir = || Box::new(Pendrive { f: f.try_clone().expect("duplicar el manejador"), sectors }) as Box<dyn SectorDev>;
    let r = vol::instalar::crear_pendrive(&mut abrir, &efi, version, vol::instalar::ESP_PENDRIVE, now, &mut azar).map_err(fs_err);
    if r.is_err() {
        let _ = std::fs::remove_file(img);
    }
    r?;
    println!("Imagen {img} creada (66 MiB, arranque UEFI). Grábala con un programa de imágenes (Rufus en modo DD, balenaEtcher…) o prueba: QEMU -drive format=raw,file={img}");
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
    for (kind, mut m) in montar(img, std::env::var("NFDISK_CLAVE").ok().as_deref())? {
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
        for t in ["fat32", "exfat", "nightfury"] {
            let p = std::env::temp_dir().join(format!("nfdisk-usb-{}-{t}.img", std::process::id())).to_string_lossy().into_owned();
            let _ = std::fs::remove_file(&p);
            crear(&[&p, t, "64"], now).unwrap();
            assert!(crear(&[&p, t, "64"], now).is_err(), "no pisa una imagen");
            let mut v = montar(&p, None).unwrap();
            let (_, m) = &mut v[0];
            assert_eq!(m.read("datos.bin").unwrap(), patron(300_000));
            assert_eq!(m.read("Fotos\\Viajes\\playa.txt").unwrap(), b"playa");
            ver(&[&p]).unwrap();
            std::fs::remove_file(&p).unwrap();
        }
    }
    #[test]
    fn pen_arranque_crea_y_no_pisa() {
        let now = DateTime { year: 2026, month: 11, day: 25, hour: 12, minute: 0, second: 0 };
        let dir = std::env::temp_dir();
        let (img, efi) = (dir.join(format!("nfdisk-pen-{}.img", std::process::id())), dir.join(format!("nfdisk-pen-{}.efi", std::process::id())));
        let (img, efi) = (img.to_string_lossy().into_owned(), efi.to_string_lossy().into_owned());
        let _ = std::fs::remove_file(&img);
        std::fs::write(&efi, patron(100_000)).unwrap();
        pen_arranque(&[&img, &efi, "9.9"], now).unwrap();
        assert!(pen_arranque(&[&img, &efi], now).is_err(), "no pisa una imagen");
        let f = OpenOptions::new().read(true).write(true).open(&img).unwrap();
        let mut abrir = || Box::new(Pendrive { f: f.try_clone().unwrap(), sectors: 66 << 11 }) as Box<dyn SectorDev>;
        assert!(vol::instalar::es_pendrive(&mut abrir));
        std::fs::remove_file(&img).unwrap();
        std::fs::remove_file(&efi).unwrap();
    }
    #[test]
    fn crea_y_lee_nightfury_cifrado() {
        let now = DateTime { year: 2026, month: 11, day: 18, hour: 12, minute: 0, second: 0 };
        let p = std::env::temp_dir().join(format!("nfdisk-usb-{}-cif.img", std::process::id())).to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&p);
        crear(&[&p, "nightfury-cifrado", "64", "clave-buena"], now).unwrap();
        assert!(montar(&p, None).is_err() && montar(&p, Some("mala-clave")).is_err());
        let mut v = montar(&p, Some("clave-buena")).unwrap();
        assert_eq!(v[0].1.label(), "PRUEBA");
        assert_eq!(v[0].1.read("datos.bin").unwrap(), patron(300_000));
        assert!(crear(&[&p, "nightfury-cifrado", "64"], now).is_err());
        std::fs::remove_file(&p).unwrap();
    }
}
