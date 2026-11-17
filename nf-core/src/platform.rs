// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Tabla de llamadas entre las apps/escritorio y el núcleo. Las apps nunca tocan hardware:
//! todo pasa por aquí, lo que permite añadir otros "subsistemas" más adelante.
use crate::files::Files;
use crate::keys::Pool;
use crate::proc::Process;
use alloc::boxed::Box;
use alloc::string::String;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MemInfo {
    /// Memoria RAM de la máquina, en bytes.
    pub total: u64,
    /// Memoria reservada para el sistema (montón), en bytes.
    pub heap_total: u64,
    pub heap_used: u64,
}

pub trait Platform {
    fn now(&mut self) -> DateTime;
    fn uptime_ms(&self) -> u64;
    fn mem(&self) -> MemInfo;
    fn cpu(&self) -> String;
    fn screen(&self) -> (u32, u32);
    fn reboot(&mut self);
    fn shutdown(&mut self);
    /// Archivos del sistema (unidad C:).
    fn files(&mut self) -> &mut Files;
    /// 64 bits impredecibles (generador del procesador si existe, mezclado con el reloj fino).
    /// Para sales; las llaves de cifrado mezclan muchas lecturas con otras fuentes.
    fn entropy(&mut self) -> u64;
    /// Acumulador de aleatoriedad del sistema. El escritorio le echa cada movimiento del ratón
    /// y cada tecla; de él salen las llaves de cifrado.
    fn pool(&mut self) -> &mut Pool;
    /// Carga un programa `.nfx` en modo usuario (v0.8). `Err` con el motivo, en español.
    /// Devuelve el proceso y la memoria fija que ocupa (imagen + pila), para `ProcApp::new`.
    fn spawn(&mut self, _nfx: &[u8]) -> Result<(Box<dyn Process>, u64), String> {
        Err(String::from("Este equipo no puede ejecutar programas"))
    }
}

/// Plataforma simulada para pruebas y para la vista previa en el PC.
#[derive(Default)]
pub struct TestPlatform {
    pub ms: u64,
    pub rebooted: bool,
    pub halted: bool,
    pub size: (u32, u32),
    /// Unidad C: en memoria.
    pub files: Files,
    /// Contador para `entropy` (determinista en las pruebas).
    pub seed: u64,
    pub pool: Pool,
}

impl Platform for TestPlatform {
    fn now(&mut self) -> DateTime {
        DateTime { year: 2026, month: 11, day: 15, hour: 21, minute: 37, second: 5 }
    }
    fn uptime_ms(&self) -> u64 {
        self.ms
    }
    fn mem(&self) -> MemInfo {
        MemInfo { total: 1 << 30, heap_total: 256 << 20, heap_used: 20 << 20 }
    }
    fn cpu(&self) -> String {
        String::from("CPU de pruebas")
    }
    fn screen(&self) -> (u32, u32) {
        self.size
    }
    fn reboot(&mut self) {
        self.rebooted = true;
    }
    fn shutdown(&mut self) {
        self.halted = true;
    }
    fn files(&mut self) -> &mut Files {
        &mut self.files
    }
    fn entropy(&mut self) -> u64 {
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.seed
    }
    fn pool(&mut self) -> &mut Pool {
        &mut self.pool
    }
}
