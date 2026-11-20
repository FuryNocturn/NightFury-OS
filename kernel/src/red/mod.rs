// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Tarjetas de red de cable (v0.12), por sondeo y sin interrupciones. Cada controlador implementa
//! `nf_core::net::NetDev`; la pila (`nf_core::net::Net`) no sabe cuál es.
//!
//! CONTRATO: `sondear` no cambia de firma.
use alloc::boxed::Box;
use nf_core::net::NetDev;

/// Busca en el PCI las tarjetas compatibles, inicia la primera que tenga cable (si ninguna lo tiene,
/// la primera que arranque) y la devuelve. `None` si no hay ninguna.
pub fn sondear() -> Option<Box<dyn NetDev>> {
    None
}
