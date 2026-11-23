// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Mensajes de NightFury como programa en modo usuario. La lógica está en `lib.rs`.
#![no_std]
#![no_main]

// Tick cada 50 ms: sondea la red y las conexiones.
nf_app::programa!(mensajes_app::crear(), Some(50));
