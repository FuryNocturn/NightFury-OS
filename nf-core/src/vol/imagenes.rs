// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Discos de prueba para instalar NightFury «junto a» otro sistema (solo con `cargo test` o la
//! característica `imagenes`): un Windows o un Linux falsos con lo mínimo que mira `instalar::analizar`.
use super::instalar::{escribir_en_esp, llenar_esp, Abrir, GUID_MSR};
use super::part::gpt::{self, Particion, TIPO_ESP};
use super::part::guid;
use super::{ext, ntfs, SectorDev, Window};
use crate::fs::FsError;
use crate::platform::DateTime;
use alloc::vec;

const MIB: u64 = 1 << 20;

/// Qué disco se crea.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prueba {
    /// GPT: ESP de Windows (100 MiB), MSR (16), «Windows» NTFS con `\Windows\System32` (64) y hueco libre.
    Windows,
    /// Igual, con una partición de datos FAT32 de 2 GiB tras Windows.
    WindowsDatos,
    /// GPT: ESP con `\EFI\ubuntu`, ext con `/etc/os-release` (64 MiB) y hueco libre.
    Linux,
    /// Como `Windows`, pero la partición de Windows está cifrada con BitLocker.
    BitLocker,
    /// Disco con tabla MBR y una partición NTFS.
    Mbr,
}

impl Prueba {
    pub fn de_texto(s: &str) -> Option<Prueba> {
        Some(match s {
            "windows" => Prueba::Windows,
            "windows-datos" => Prueba::WindowsDatos,
            "linux" => Prueba::Linux,
            "bitlocker" => Prueba::BitLocker,
            "mbr" => Prueba::Mbr,
            _ => return None,
        })
    }
}

/// Crea el disco de prueba `tipo` en el dispositivo (que debe estar a cero). `efi` es el contenido del
/// cargador falso del otro sistema (`bootmgfw.efi` o `shimx64.efi`).
pub fn crear(abrir: Abrir, tipo: Prueba, efi: &[u8], now: DateTime, aleatorio: &mut dyn FnMut() -> u64) -> Result<(), FsError> {
    let mut d = abrir();
    let ss = d.sector_size();
    if tipo == Prueba::Mbr {
        let mut s0 = vec![0u8; ss];
        s0[446 + 4] = 7;
        s0[446 + 8..446 + 12].copy_from_slice(&((MIB / ss as u64) as u32).to_le_bytes());
        s0[446 + 12..446 + 16].copy_from_slice(&((d.sectors() - MIB / ss as u64).min(u32::MAX as u64) as u32).to_le_bytes());
        s0[510..512].copy_from_slice(&[0x55, 0xAA]);
        return d.write(0, &s0);
    }
    let mut cursor = gpt::alinear_mib(gpt::primer_lba_usable(ss), ss);
    let mut poner = |mib: u64, tipo: [u8; 16], nombre: &'static str| {
        let p = Particion { tipo, unica: gpt::guid_nuevo(aleatorio), nombre, inicio_lba: cursor, fin_lba: cursor + mib * MIB / ss as u64 - 1 };
        cursor = p.fin_lba + 1;
        p
    };
    let datos = guid("EBD0A0A2-B9E5-4433-87C0-68B6B72699C7");
    let mut todas = vec![poner(100, TIPO_ESP, "EFI system partition")];
    if tipo == Prueba::Linux {
        todas.push(poner(64, guid("0FC63DAF-8483-4772-8E79-3D69D8477DE4"), "root"));
    } else {
        todas.push(poner(16, guid(GUID_MSR), "Microsoft reserved partition"));
        todas.push(poner(64, datos, "Windows"));
    }
    if tipo == Prueba::WindowsDatos {
        todas.push(poner(2048, datos, "Datos"));
    }
    gpt::crear_gpt(&mut *d, &todas, gpt::guid_nuevo(aleatorio))?;
    drop(d);
    let (carpeta, bootloader) = if tipo == Prueba::Linux { ("EFI\\ubuntu\\shimx64.efi", efi) } else { ("EFI\\Microsoft\\Boot\\bootmgfw.efi", efi) };
    let tam = |p: &Particion| p.fin_lba - p.inicio_lba + 1;
    let sistema = &todas[if tipo == Prueba::Linux { 1 } else { 2 }];
    llenar_esp(abrir, todas[0].inicio_lba, tam(&todas[0]), &[(carpeta, bootloader)], now)?;
    let mut w = Window::new(abrir(), sistema.inicio_lba, tam(sistema));
    match tipo {
        Prueba::Linux => w.write(0, &ext::pruebas::linux_falso())?,
        Prueba::BitLocker => {
            let mut s0 = vec![0u8; ss];
            s0[3..11].copy_from_slice(b"-FVE-FS-");
            s0[510..512].copy_from_slice(&[0x55, 0xAA]);
            w.write(0, &s0)?;
        }
        _ => w.write(0, &ntfs::pruebas::windows_falso())?,
    }
    w.flush()?;
    if let Some(e) = todas.get(3) {
        super::format_in(&mut Window::new(abrir(), e.inicio_lba, tam(e)), super::FsKind::Fat32, "DATOS", now)?;
        escribir_en_esp(abrir, e.inicio_lba, tam(e), &[("datos.txt", b"datos de prueba")], now)?;
    }
    abrir().flush()
}
