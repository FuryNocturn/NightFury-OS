// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Unidades extraíbles con el formato propio (NFFS), en claro o cifradas con `cryptdev`.
//!
//! CONTRATO (v0.11): `SectorBlocks`, `open`, `Opened`, `LockedDrive::unlock` y `format` no cambian
//! de firma.
use super::{Partition, SectorDev, Volume};
use crate::block::BlockDev;
use crate::fs::FsError;
use crate::keys::{KeyError, Pool};
use crate::platform::DateTime;
use alloc::boxed::Box;
use alloc::string::String;

/// Bloques de 4096 bytes (`BlockDev`) sobre un rango de sectores de un `SectorDev`
/// (la partición `start..start+sectors`). Los bloques que no caben enteros al final no se usan.
pub struct SectorBlocks {
    pub dev: Box<dyn SectorDev>,
    pub start: u64,
    pub sectors: u64,
}

impl BlockDev for SectorBlocks {
    fn blocks(&self) -> u64 {
        todo!()
    }
    fn read(&mut self, _n: u64, _buf: &mut [u8]) -> Result<(), crate::block::DiskError> {
        todo!()
    }
    fn write(&mut self, _n: u64, _buf: &[u8]) -> Result<(), crate::block::DiskError> {
        todo!()
    }
    fn flush(&mut self) -> Result<(), crate::block::DiskError> {
        todo!()
    }
}

/// Resultado de abrir una partición `FsKind::NightFury`.
pub enum Opened {
    Ready(Box<dyn Volume>),
    /// Cifrada: hace falta la contraseña (o el código de recuperación).
    Locked(LockedDrive),
}

/// Volumen NightFury cifrado, todavía cerrado.
pub struct LockedDrive {
    _p: (),
}

impl LockedDrive {
    /// Abre con la contraseña o, si `recovery`, con el código de recuperación. Si falla, devuelve
    /// la unidad para poder reintentar.
    pub fn unlock(self, _secret: &str, _recovery: bool) -> Result<Box<dyn Volume>, (LockedDrive, KeyError)> {
        todo!()
    }
}

/// Abre la partición `p` (de tipo `NightFury`) del disco.
pub fn open(_dev: Box<dyn SectorDev>, _p: &Partition) -> Result<Opened, FsError> {
    todo!()
}

/// Borra el disco entero y crea una partición MBR con NFFS dentro; con `encrypt`, cifrada con esa
/// contraseña. La etiqueta se guarda dentro del volumen. Devuelve el código de recuperación si se
/// cifró.
pub fn format(_dev: Box<dyn SectorDev>, _label: &str, _encrypt: Option<(&str, &mut Pool)>, _now: DateTime) -> Result<Option<String>, KeyError> {
    todo!()
}
