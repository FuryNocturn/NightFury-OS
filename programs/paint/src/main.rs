// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Paint de NightFury como programa en modo usuario. La lógica está en `lib.rs`.
#![no_std]
#![no_main]

// Tick cada 50 ms: solo para repetir el desplazamiento al mantener pulsada una barra.
nf_app::programa!(paint_app::crear(), Some(50));
