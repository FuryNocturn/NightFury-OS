//! Tabla de llamadas entre las apps/escritorio y el núcleo. Las apps nunca tocan hardware:
//! todo pasa por aquí, lo que permite añadir otros "subsistemas" más adelante.
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
}

/// Plataforma simulada para pruebas y para la vista previa en el PC.
#[derive(Default)]
pub struct TestPlatform {
    pub ms: u64,
    pub rebooted: bool,
    pub halted: bool,
    pub size: (u32, u32),
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
}
