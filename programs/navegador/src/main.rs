// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Navegador de NightFury como programa en modo usuario. La lógica está en `lib.rs`.
#![no_std]
#![no_main]

// Tick cada 30 ms: sondea las descargas y anima el indicador de carga.
nf_app::programa!(navegador_app::crear(), Some(30));
