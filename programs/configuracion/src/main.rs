// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Configuración del sistema, ejecutado como programa en modo usuario (v0.10). El código es el de
//! `nf-core/src/apps`: aquí solo se arranca.
#![no_std]
#![no_main]

nf_app::programa!(nf_core::apps::create(nf_core::apps::AppKind::Settings), Some(20));
