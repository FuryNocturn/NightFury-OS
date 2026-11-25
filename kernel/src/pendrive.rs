// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Arranque desde un pendrive NightFury (v0.12.6): encuentra el propio pendrive entre los discos USB,
//! lo aparta (nunca se monta como unidad) y prepara el modo Portátil. Lo que hay que enseñar y elegir
//! está en `nf-core` (`login::pen`); aquí solo el hardware.
use crate::boot::ArranqueUsb;
use crate::serial::traza;
use crate::usb;
use alloc::boxed::Box;
use nf_core::block::BlockDev;
use nf_core::files::Files;
use nf_core::fs::FsError;
use nf_core::platform::{DateTime, PenEstado};
use nf_core::vol::instalar;
use nf_core::vol::nfvol::SectorBlocks;
use nf_core::vol::part::gpt;
use nf_core::vol::{MemSectors, SectorDev};

/// Plazo para que el pendrive de arranque aparezca entre los discos USB (el xHCI los enumera tras salir de UEFI).
const BUSCAR_MS: u64 = 15_000;

/// Lo que el usuario eligió en el menú; lo aplica el bucle principal (monta los discos internos).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Eleccion {
    Probar,
    Portatil,
}

pub struct Pen {
    arranque: Option<ArranqueUsb>,
    /// Hay que enseñar el menú (arrancó de un pendrive y no se ha elegido nada).
    pub menu: bool,
    pub eleccion: Option<Eleccion>,
    /// Identificador USB del pendrive de arranque, cuando ya apareció.
    disco: Option<u32>,
    /// (tiene la partición portátil, bytes libres donde crearla) del pendrive encontrado.
    listo: Option<(bool, u64)>,
    /// Instante (ms desde el arranque) en que el menú preguntó por primera vez.
    desde: Option<u64>,
}

/// Disco de cero sectores: el manejador que se da si el pendrive se quita justo al abrirlo (todo falla con `Disk`).
fn vacio() -> Box<dyn SectorDev> {
    Box::new(MemSectors::new(0, 512))
}

impl Pen {
    pub fn new(arranque: Option<ArranqueUsb>) -> Pen {
        if let Some(a) = &arranque {
            traza!("NF pendrive: arrancado desde USB (partición {} inicio {} tamaño {} GUID {})", a.particion, a.inicio, a.tamano, a.guid.is_some());
        }
        Pen { menu: arranque.is_some(), arranque, eleccion: None, disco: None, listo: None, desde: None }
    }

    /// ¿Es este disco USB el pendrive de arranque? El primero que sea un pendrive NightFury (y, si el firmware
    /// dio la GUID de la partición, cuya partición 1 la tenga) queda apartado. Con `true` no se monta.
    pub fn es_arranque(&mut self, id: u32) -> bool {
        let Some(a) = self.arranque else { return false };
        if self.disco.is_some() {
            return self.disco == Some(id);
        }
        let mut abrir = || usb::abrir_disco(id).unwrap_or_else(vacio);
        if !instalar::es_pendrive(&mut abrir) {
            return false;
        }
        if let Some(g) = a.guid {
            let primera = gpt::leer_gpt(&mut *abrir()).and_then(|(_, e)| e.first().map(|e| e.unica));
            if primera != Some(g) {
                return false;
            }
        }
        self.disco = Some(id);
        self.listo = Some(self.mirar(id));
        traza!("NF pendrive: el disco USB {} es el pendrive de arranque", id);
        true
    }

    pub fn quitado(&mut self, id: u32) {
        if self.disco == Some(id) {
            self.disco = None;
            self.listo = None;
        }
    }

    /// (¿tiene la partición portátil?, bytes libres tras la última partición).
    fn mirar(&self, id: u32) -> (bool, u64) {
        let mut abrir = || usb::abrir_disco(id).unwrap_or_else(vacio);
        let tiene = instalar::portatil(&mut abrir).is_some();
        let mut d = abrir();
        let ss = d.sector_size() as u64;
        (tiene, gpt::hueco_final(&mut *d).map_or(0, |(i, f)| (f - i + 1) * ss))
    }

    pub fn estado(&mut self, ahora_ms: u64) -> PenEstado {
        match self.listo {
            Some((portatil, hueco)) => PenEstado::Listo { portatil, hueco },
            None => {
                let desde = *self.desde.get_or_insert(ahora_ms);
                if ahora_ms.saturating_sub(desde) >= BUSCAR_MS {
                    PenEstado::NoEncontrado
                } else {
                    PenEstado::Buscando
                }
            }
        }
    }

    pub fn elegir(&mut self, e: Eleccion) {
        self.menu = false;
        self.eleccion = Some(e);
    }

    /// El cargador `BOOTX64.EFI` del propio pendrive de arranque (lo que se copia al instalar). No escribe nada.
    pub fn efi(&self) -> Result<alloc::vec::Vec<u8>, FsError> {
        let id = self.disco.ok_or(FsError::NotFound)?;
        instalar::leer_efi(&mut || usb::abrir_disco(id).unwrap_or_else(vacio))
    }

    /// Modo Portátil: crea la partición NFFS si falta y devuelve `C:` sobre ella. `true` si es nueva
    /// (queda como disco en blanco, para que el asistente pregunte el cifrado y formatee al final).
    pub fn portatil(&mut self, now: DateTime) -> Result<(bool, Files), FsError> {
        let id = self.disco.ok_or(FsError::NotFound)?;
        let mut abrir = || usb::abrir_disco(id).unwrap_or_else(vacio);
        let (rango, nuevo) = match instalar::portatil(&mut abrir) {
            Some(r) => (r, false),
            None => (instalar::crear_portatil(&mut abrir, now, &mut crate::entropia)?, true),
        };
        self.listo = Some((true, 0));
        let dev: Box<dyn BlockDev> = Box::new(SectorBlocks { dev: abrir(), start: rango.0, sectors: rango.1 - rango.0 + 1 });
        Ok((nuevo, if nuevo { Files::pendiente(dev, now) } else { Files::probe(Some(dev), now) }))
    }
}
