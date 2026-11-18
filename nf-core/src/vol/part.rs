// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Tablas de particiones MBR y GPT. Pendiente (v0.11): se rellena por su encargo.
use super::{Partition, SectorDev};
use crate::fs::FsError;
use alloc::vec::Vec;

pub fn partitions(_dev: &mut dyn SectorDev) -> Result<Vec<Partition>, FsError> {
    Ok(Vec::new())
}
