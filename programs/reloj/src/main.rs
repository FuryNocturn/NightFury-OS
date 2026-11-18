// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Reloj de NightFury como programa en modo usuario. La lógica está en `lib.rs`.
#![no_std]
#![no_main]

// Tick cada 50 ms: el cronómetro y el temporizador se mueven con él.
nf_app::programa!(reloj_app::crear(), Some(50));
