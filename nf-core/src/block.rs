// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Dispositivo de bloques: lo único que el sistema de archivos sabe del disco.
//! CONTRATO: `BLOCK`, `DiskError`, `BlockDev` y `RamDisk` no cambian.
use alloc::vec;
use alloc::vec::Vec;

/// Tamaño de bloque en bytes.
pub const BLOCK: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiskError {
    /// Bloque fuera del disco o búfer de tamaño incorrecto.
    Range,
    /// Fallo del dispositivo.
    Io,
}

pub trait BlockDev {
    /// Número de bloques de `BLOCK` bytes.
    fn blocks(&self) -> u64;
    /// Lee el bloque `n` en `buf` (exactamente `BLOCK` bytes).
    fn read(&mut self, n: u64, buf: &mut [u8]) -> Result<(), DiskError>;
    /// Escribe el bloque `n` desde `buf` (exactamente `BLOCK` bytes).
    fn write(&mut self, n: u64, buf: &[u8]) -> Result<(), DiskError>;
    /// Asegura que lo escrito ha llegado al soporte.
    fn flush(&mut self) -> Result<(), DiskError> {
        Ok(())
    }
}

/// Disco en memoria (pruebas y unidad C: sin disco real).
pub struct RamDisk {
    data: Vec<u8>,
}

impl RamDisk {
    pub fn new(blocks: u64) -> Self {
        RamDisk { data: vec![0; blocks as usize * BLOCK] }
    }
    pub fn from_bytes(mut data: Vec<u8>) -> Self {
        data.truncate(data.len() / BLOCK * BLOCK);
        RamDisk { data }
    }
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }
}

impl BlockDev for RamDisk {
    fn blocks(&self) -> u64 {
        (self.data.len() / BLOCK) as u64
    }
    fn read(&mut self, n: u64, buf: &mut [u8]) -> Result<(), DiskError> {
        if n >= self.blocks() || buf.len() != BLOCK {
            return Err(DiskError::Range);
        }
        let o = n as usize * BLOCK;
        buf.copy_from_slice(&self.data[o..o + BLOCK]);
        Ok(())
    }
    fn write(&mut self, n: u64, buf: &[u8]) -> Result<(), DiskError> {
        if n >= self.blocks() || buf.len() != BLOCK {
            return Err(DiskError::Range);
        }
        let o = n as usize * BLOCK;
        self.data[o..o + BLOCK].copy_from_slice(buf);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramdisk_roundtrip_and_limits() {
        let mut d = RamDisk::new(4);
        let mut b = [7u8; BLOCK];
        d.write(3, &b).unwrap();
        b.fill(0);
        d.read(3, &mut b).unwrap();
        assert!(b.iter().all(|&x| x == 7));
        assert_eq!(d.read(4, &mut b), Err(DiskError::Range));
        assert_eq!(d.write(0, &b[..10]), Err(DiskError::Range));
    }
}
