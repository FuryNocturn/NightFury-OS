// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! La Calculadora del sistema, ejecutada como programa en modo usuario. No necesita tick.
#![no_std]
#![no_main]

nf_app::programa!(nf_core::apps::create(nf_core::apps::AppKind::Calc), None);
