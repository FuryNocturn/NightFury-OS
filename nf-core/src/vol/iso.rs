// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Pendiente (v0.11): se rellena por su encargo.
use super::{Partition, SectorDev, Volume};
use crate::fs::FsError;
use alloc::boxed::Box;

pub fn mount(_dev: Box<dyn SectorDev>, _p: &Partition) -> Result<Box<dyn Volume>, FsError> {
    Err(FsError::Corrupt)
}
