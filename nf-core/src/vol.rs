// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Volúmenes ajenos (pendrives, discos USB, CD/DVD): sectores, particiones y sistemas de archivos
//! FAT12/16/32, exFAT, ISO9660 y UDF.
//!
//! CONTRATO (v0.11): `SectorDev`, `MemSectors`, `Volume`, `FsKind`, `Partition`, `partitions`,
//! `mount` y `format` no cambian de firma. Las implementaciones van en los submódulos.
//!
//! Rutas dentro de un volumen: relativas a su raíz, con `\` como separador y sin letra de unidad:
//! `""` o `"\\"` es la raíz; `"Fotos\\a.jpg"` un archivo. Los nombres se comparan sin distinguir
//! mayúsculas (como Windows) y se conservan tal como se escribieron.
use crate::fs::{Entry, FsError};
use crate::platform::DateTime;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Disco por sectores (lo que da un pendrive: 512 bytes, 2048 en un CD, a veces 4096).
pub trait SectorDev {
    /// Tamaño de sector en bytes (potencia de dos, 512..=4096).
    fn sector_size(&self) -> usize;
    /// Número de sectores.
    fn sectors(&self) -> u64;
    /// Lee `buf.len() / sector_size()` sectores desde `lba`. `buf.len()` es múltiplo del sector.
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError>;
    /// Escribe sectores desde `lba`. `Err(FsError::Denied)` si el soporte es de solo lectura.
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError>;
    /// Asegura que lo escrito ha llegado al soporte.
    fn flush(&mut self) -> Result<(), FsError> {
        Ok(())
    }
    /// Soporte de solo lectura (CD, interruptor de protección).
    fn read_only(&self) -> bool {
        false
    }
}

/// Disco por sectores en memoria (tests y pruebas).
pub struct MemSectors {
    pub data: Vec<u8>,
    pub sector: usize,
    pub ro: bool,
}

impl MemSectors {
    pub fn new(sectors: u64, sector: usize) -> Self {
        MemSectors { data: vec![0; sectors as usize * sector], sector, ro: false }
    }
}

impl SectorDev for MemSectors {
    fn sector_size(&self) -> usize {
        self.sector
    }
    fn sectors(&self) -> u64 {
        (self.data.len() / self.sector) as u64
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        let o = lba as usize * self.sector;
        if buf.len() % self.sector != 0 || o + buf.len() > self.data.len() {
            return Err(FsError::Disk);
        }
        buf.copy_from_slice(&self.data[o..o + buf.len()]);
        Ok(())
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError> {
        if self.ro {
            return Err(FsError::Denied);
        }
        let o = lba as usize * self.sector;
        if buf.len() % self.sector != 0 || o + buf.len() > self.data.len() {
            return Err(FsError::Disk);
        }
        self.data[o..o + buf.len()].copy_from_slice(buf);
        Ok(())
    }
    fn read_only(&self) -> bool {
        self.ro
    }
}

/// Sistema de archivos reconocido.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FsKind {
    Fat12,
    Fat16,
    Fat32,
    ExFat,
    Iso9660,
    Udf,
    /// El formato propio (NFFS), en claro o cifrado: lo monta `files`, no este módulo.
    NightFury,
    /// NTFS, ext, HFS+, APFS…: se ve la unidad pero sale «Formato no compatible» (v0.11).
    Unknown,
}

/// Partición encontrada en el disco (o el disco entero si no hay tabla).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Partition {
    /// Primer sector (del disco) y número de sectores.
    pub start: u64,
    pub sectors: u64,
    /// Lo que hay dentro, mirando el sector de arranque o el superbloque.
    pub kind: FsKind,
}

/// Un sistema de archivos montado.
pub trait Volume {
    fn kind(&self) -> FsKind;
    /// Etiqueta del volumen ("" si no tiene).
    fn label(&self) -> String;
    /// (bytes totales, bytes libres).
    fn space(&mut self) -> (u64, u64);
    fn read_only(&self) -> bool;
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, FsError>;
    fn stat(&mut self, path: &str) -> Result<Entry, FsError>;
    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError>;
    /// Crea o sobrescribe un archivo (la carpeta padre debe existir).
    fn write(&mut self, path: &str, data: &[u8], now: DateTime) -> Result<(), FsError>;
    fn mkdir(&mut self, path: &str, now: DateTime) -> Result<(), FsError>;
    /// Renombra o mueve dentro del mismo volumen.
    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError>;
    /// Borra un archivo o una carpeta vacía.
    fn delete(&mut self, path: &str) -> Result<(), FsError>;
    /// Escribe lo pendiente y deja el volumen limpio (antes de expulsar).
    fn flush(&mut self) -> Result<(), FsError>;
}

/// Lee la tabla de particiones (GPT, si no MBR; si no hay, el disco entero es una partición) y
/// reconoce el sistema de archivos de cada una. Las particiones vacías o de tipo 0 no salen.
pub fn partitions(dev: &mut dyn SectorDev) -> Result<Vec<Partition>, FsError> {
    part::partitions(dev)
}

/// Monta la partición `p` de `dev`. `Err(FsError::Corrupt)` si el formato es `NightFury` o
/// `Unknown` o está dañado. Los de solo lectura (ISO, UDF) devuelven `Denied` al escribir.
pub fn mount(dev: Box<dyn SectorDev>, p: &Partition) -> Result<Box<dyn Volume>, FsError> {
    match p.kind {
        FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32 => fat::mount(dev, p),
        FsKind::ExFat => exfat::mount(dev, p),
        FsKind::Iso9660 => iso::mount(dev, p),
        FsKind::Udf => udf::mount(dev, p),
        FsKind::NightFury | FsKind::Unknown => Err(FsError::Corrupt),
    }
}

/// Formatea el disco entero con una sola partición MBR del tipo pedido (`Fat32` o `ExFat`; con
/// `Fat32` en discos de menos de 64 MiB se usa FAT16 o FAT12) y la etiqueta dada.
pub fn format(dev: &mut dyn SectorDev, kind: FsKind, label: &str, now: DateTime) -> Result<(), FsError> {
    match kind {
        FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32 => fat::format(dev, label, now),
        FsKind::ExFat => exfat::format(dev, label, now),
        _ => Err(FsError::Invalid),
    }
}

pub mod exfat;
pub mod fat;
pub mod iso;
pub mod part;
pub mod udf;
