//! La Calculadora del sistema, ejecutada como programa en modo usuario. No necesita tick.
#![no_std]
#![no_main]

nf_app::programa!(nf_core::apps::create(nf_core::apps::AppKind::Calc), None);
