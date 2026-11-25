// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Tabla de llamadas entre las apps/escritorio y el núcleo. Las apps nunca tocan hardware:
//! todo pasa por aquí, lo que permite añadir otros "subsistemas" más adelante.
use crate::arranque::{self, Ajustes, InfoArranque};
use crate::files::Files;
use crate::fs::FsError;
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

/// Dónde está el pendrive del que arrancó el sistema (v0.12.6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PenEstado {
    /// Todavía no ha aparecido (los USB tardan unos segundos).
    Buscando,
    /// No ha aparecido en el plazo.
    NoEncontrado,
    /// Encontrado. Con `portatil` ya tiene la partición del modo Portátil; si no, `hueco` son los bytes libres donde se crearía.
    Listo { portatil: bool, hueco: u64 },
}

/// Un disco interno que se puede elegir para instalar NightFury (v0.12.6).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PenDisco {
    pub modelo: String,
    pub bytes: u64,
    /// Lo que tiene, por partición: (formato, bytes). Vacío = disco sin particiones ni datos.
    pub partes: Vec<(crate::vol::FsKind, u64)>,
    /// Bytes por sector (para dar tamaño a los sitios de `analisis`).
    pub sector: u32,
    /// Qué se puede hacer con el disco (v0.12.7): dónde instalar junto a lo que hay y si ya lleva NightFury.
    pub analisis: crate::vol::instalar::Analisis,
}

/// Dónde instala `Platform::pen_instalar` (v0.12.7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Donde {
    /// Todo el disco, borrándolo.
    DiscoEntero,
    /// El sitio `k` de `PenDisco::analisis.sitios`.
    Sitio(usize),
}

pub trait Platform {
    fn now(&mut self) -> DateTime;
    /// Pone el reloj del equipo (hora LOCAL, como la que da `now`). Por defecto no hace nada (programas, pruebas).
    fn set_now(&mut self, _t: DateTime) {}
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
    /// La red (v0.12), solo para las apps del sistema. `None` si el equipo no tiene pila de red.
    fn net(&mut self) -> Option<&mut dyn crate::net::NetApi> {
        None
    }
    /// Se arrancó desde un pendrive y aún no se ha elegido modo: el escritorio enseña antes de nada el menú de arranque.
    fn pen_menu(&self) -> bool {
        false
    }
    /// Estado del pendrive de arranque (para los modos Portátil e Instalar).
    fn pen_estado(&mut self) -> PenEstado {
        PenEstado::NoEncontrado
    }
    /// Modo Probar: C: en memoria, los discos del equipo y el pendrive de arranque solo se leen.
    fn pen_probar(&mut self) {}
    /// Modo Portátil: crea la partición si falta y C: pasa a ser ella. `Ok(true)` si es nueva (disco en blanco:
    /// el asistente pregunta el cifrado y las cuentas); `Ok(false)` si ya existía.
    fn pen_portatil(&mut self) -> Result<bool, FsError> {
        Err(FsError::Invalid)
    }
    /// Discos internos (SATA y NVMe, nunca USB ni el pendrive de arranque) donde se puede instalar.
    fn pen_discos(&mut self) -> Vec<PenDisco> {
        Vec::new()
    }
    /// Instala NightFury en el disco `i` de `pen_discos`, en `donde`. `Ok(true)` si además quedó creada
    /// la entrada de arranque UEFI; `Ok(false)` si no (el equipo debe arrancarlo desde su menú de arranque).
    fn pen_instalar(&mut self, _i: usize, _donde: Donde) -> Result<bool, FsError> {
        Err(FsError::Invalid)
    }
    /// Quita NightFury del disco `i` de `pen_discos` (sus particiones y su entrada de arranque; lo demás no se toca).
    /// `Ok(true)` si también se quitó la entrada UEFI; `Ok(false)` si el firmware no dejó (el disco ya está limpio).
    fn pen_quitar(&mut self, _i: usize) -> Result<bool, FsError> {
        Err(FsError::Invalid)
    }
    /// Menú de arranque (v0.12.7): sus ajustes y entradas. `None` si no se arrancó desde él o no hay servicios de ejecución.
    fn arranque_leer(&mut self) -> Option<InfoArranque> {
        None
    }
    /// Guarda los ajustes del menú de arranque (los valida: ver `arranque::validar`).
    fn arranque_guardar(&mut self, _a: &Ajustes) -> Result<(), &'static str> {
        Err(arranque::ERRORES[0])
    }
    /// Pone la entrada «NightFury» la primera de `BootOrder`, sin quitar ni repetir ninguna.
    fn arranque_primero(&mut self) -> Result<(), &'static str> {
        Err(arranque::ERRORES[0])
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
    /// Red de pruebas (los tests le enganchan una tarjeta en memoria).
    pub net: Option<crate::net::Net>,
    /// Reloj puesto con `set_now`; sin él, `now` da una fecha fija.
    pub reloj: Option<DateTime>,
    /// Menú de arranque de pruebas; `None` = no instalado.
    pub arranque: Option<InfoArranque>,
}

impl Platform for TestPlatform {
    fn now(&mut self) -> DateTime {
        self.reloj.unwrap_or(DateTime { year: 2026, month: 11, day: 15, hour: 21, minute: 37, second: 5 })
    }
    fn set_now(&mut self, t: DateTime) {
        self.reloj = Some(t);
    }
    fn arranque_leer(&mut self) -> Option<InfoArranque> {
        self.arranque.clone()
    }
    fn arranque_guardar(&mut self, a: &Ajustes) -> Result<(), &'static str> {
        arranque::validar(a)?;
        self.arranque.as_mut().ok_or(arranque::ERRORES[0])?.ajustes = a.clone();
        Ok(())
    }
    fn arranque_primero(&mut self) -> Result<(), &'static str> {
        self.arranque.as_mut().ok_or(arranque::ERRORES[0])?.primero = true;
        Ok(())
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
    fn net(&mut self) -> Option<&mut dyn crate::net::NetApi> {
        self.net.as_mut().map(|n| n as &mut dyn crate::net::NetApi)
    }
}
