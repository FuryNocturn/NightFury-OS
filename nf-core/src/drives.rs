// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Unidades extraíbles D:, E:… (v0.11).
//!
//! CONTRATO: estos tipos y los métodos de unidades de `Files` (`attach`, `attach_locked`, `detach`,
//! `drives`, `unlock_drive`, `request_drive_action`, `take_drive_actions`) no cambian de firma.
//!
//! Reparto:
//! - `Files` guarda la tabla de unidades y reparte por letra todas sus operaciones de archivos
//!   (`list`, `read`, `write`, `copy`, `transfer`…). C: sigue siendo el NFFS de siempre.
//! - El núcleo conecta y quita unidades (`attach`/`detach`) y ejecuta las acciones que piden los
//!   programas del sistema (`take_drive_actions`): expulsar, formatear y cifrar necesitan el disco
//!   entero, que solo tiene el núcleo.
use crate::vol::{FsKind, LockedDrive};
use alloc::string::String;

/// Lo que se ve de una unidad (C: incluida) en «Este equipo».
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DriveInfo {
    /// 'C', 'D', 'E'…
    pub letter: char,
    /// Etiqueta del volumen ("" si no tiene).
    pub label: String,
    /// Nombre del dispositivo (fabricante y modelo) para el aviso y las propiedades.
    pub device: String,
    pub kind: FsKind,
    pub total: u64,
    pub free: u64,
    pub read_only: bool,
    /// Extraíble (todas menos C:).
    pub removable: bool,
    /// Cifrada y todavía sin contraseña: no se puede abrir hasta `unlock_drive`.
    pub locked: bool,
}

/// Formato al formatear una unidad.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormatKind {
    Fat32,
    ExFat,
    NightFury,
}

/// Acción sobre el disco de una unidad que ejecuta el núcleo.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DriveAction {
    /// Escribir lo pendiente y desmontar todas las particiones del disco.
    Eject { disk: u32 },
    /// USB: borrar el disco entero y crear una partición con el formato y la etiqueta dados.
    /// Disco interno: formatear solo la partición que empieza en el sector `part` (en USB vale 0).
    Format { disk: u32, part: u64, kind: FormatKind, label: String },
    /// Borrar el disco entero y crear un volumen NightFury cifrado con la contraseña dada. El código
    /// de recuperación se devuelve al programa con `Files::drive_result`. En un disco interno, solo
    /// la partición que empieza en el sector `part`.
    Encrypt { disk: u32, part: u64, label: String, password: String },
}

impl DriveInfo {
    /// Nombre que ven los usuarios: «Disco local (C:)», «Etiqueta (D:)» o «Unidad USB (D:)».
    pub fn name(&self) -> String {
        let l = self.letter;
        match (self.removable, self.label.is_empty()) {
            (false, _) => alloc::format!("Disco local ({l}:)"),
            (true, true) => alloc::format!("Unidad USB ({l}:)"),
            (true, false) => alloc::format!("{} ({l}:)", self.label),
        }
    }
}

/// Nombre del sistema de archivos para mostrar.
pub fn kind_name(k: FsKind) -> &'static str {
    match k {
        FsKind::Fat12 => "FAT12",
        FsKind::Fat16 => "FAT16",
        FsKind::Fat32 => "FAT32",
        FsKind::ExFat => "exFAT",
        FsKind::Iso9660 => "ISO 9660",
        FsKind::Udf => "UDF",
        FsKind::NightFury => "NightFury",
        FsKind::Ntfs => "NTFS",
        FsKind::Ext => "ext",
        FsKind::BitLocker => "BitLocker",
        FsKind::Unknown => "Formato no compatible",
    }
}

/// Unidad cifrada esperando contraseña (la guarda `Files`).
pub struct Locked {
    pub disk: u32,
    /// Primer sector de la partición (solo importa en los discos internos).
    pub part: u64,
    /// `false` si es una partición de un disco interno.
    pub removable: bool,
    pub device: String,
    pub drive: LockedDrive,
}

/// Lo que pide un programa del sistema sobre una unidad (sin el número de disco, que pone `Files`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FormatRequest {
    Eject,
    Format { kind: FormatKind, label: String },
    Encrypt { label: String, password: String },
}
