// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Tabla de llamadas entre las apps/escritorio y el núcleo. Las apps nunca tocan hardware:
//! todo pasa por aquí, lo que permite añadir otros "subsistemas" más adelante.
use crate::files::Files;
use crate::fsapi::FilesApi;
use crate::keys::Pool;
use crate::proc::{ProcInfo, Process};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

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
    /// Los archivos como los ven las apps (v0.10): el sistema, `files()`; un programa en modo
    /// usuario, un cliente que los pide al sistema.
    fn fs(&mut self) -> &mut dyn FilesApi {
        self.files()
    }
    /// 64 bits impredecibles (generador del procesador si existe, mezclado con el reloj fino).
    /// Para sales; las llaves de cifrado mezclan muchas lecturas con otras fuentes.
    fn entropy(&mut self) -> u64;
    /// Acumulador de aleatoriedad del sistema. El escritorio le echa cada movimiento del ratón
    /// y cada tecla; de él salen las llaves de cifrado.
    fn pool(&mut self) -> &mut Pool;
    /// Carga un programa `.nfx` en modo usuario y lo pone en marcha (v0.9). `name` es para el
    /// Administrador de tareas. `Err` con el motivo, en español.
    fn spawn(&mut self, _nfx: &[u8], _name: &str) -> Result<Box<dyn Process>, String> {
        Err(String::from("Este equipo no puede ejecutar programas"))
    }
    /// Programas en marcha (v0.9), para el Administrador de tareas.
    fn processes(&mut self) -> Vec<ProcInfo> {
        Vec::new()
    }
    /// Finaliza el programa `id` (queda `Killed(Kill::Ended)`; su ventana se cierra sola).
    fn kill_process(&mut self, _id: u64) {}
    /// Deja correr a los programas listos hasta que se paren o les toque el turno al escritorio
    /// (v0.10). `ProcApp` lo usa para atender seguidas las peticiones de un programa sin esperar
    /// al siguiente tick. Sin programas no hace nada.
    fn run_programs(&mut self) {}
    /// `true` si `nfx` es, byte a byte, uno de los programas que trae el sistema (v0.10). Solo
    /// esos pueden usar las llamadas de cuentas, claves, cifrado, reiniciar y apagar.
    fn is_system_binary(&self, _nfx: &[u8]) -> bool {
        false
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
