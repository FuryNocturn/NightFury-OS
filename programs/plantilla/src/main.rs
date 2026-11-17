#![no_std]
#![no_main]
nf_app::programa!(nf_core::apps::create(nf_core::apps::AppKind::Calc), None);
