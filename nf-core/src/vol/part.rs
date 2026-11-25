// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Tablas de particiones GPT y MBR (UEFI y formato MBR clásico) y reconocimiento del sistema de
//! archivos de cada partición. Los datos del disco son entrada hostil: todo se valida.
use super::{fat, FsKind, Partition, SectorDev};
use crate::fs::FsError;
use alloc::vec;
use alloc::vec::Vec;

pub mod gpt;

pub(super) fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub(super) fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
pub(super) fn u64le(b: &[u8], o: usize) -> u64 {
    u32le(b, o) as u64 | (u32le(b, o + 4) as u64) << 32
}

/// CRC-32 estándar (IEEE 802.3, el de GPT), bit a bit.
/// shortcut: sin tabla (lento por byte), basta para cabeceras y tablas de unos KB; tabla si hiciera falta más.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = (c >> 1) ^ (0xEDB8_8320 & (c & 1).wrapping_neg());
        }
    }
    !c
}

/// Lee `len` bytes desde el byte `off` del disco (cualquier alineación).
fn read_bytes(dev: &mut dyn SectorDev, off: u64, len: usize) -> Result<Vec<u8>, FsError> {
    let ss = dev.sector_size() as u64;
    let first = off / ss;
    let end = (off + len as u64).div_ceil(ss);
    if end > dev.sectors() {
        return Err(FsError::Disk);
    }
    let mut buf = vec![0; ((end - first) * ss) as usize];
    dev.read(first, &mut buf)?;
    let s = (off - first * ss) as usize;
    Ok(buf[s..s + len].to_vec())
}

/// Bytes `[off, off+len)` de la partición (dentro de ella), o `None`.
fn rd(dev: &mut dyn SectorDev, base: u64, bytes: u64, off: u64, len: usize) -> Option<Vec<u8>> {
    if off.checked_add(len as u64)? > bytes {
        return None;
    }
    read_bytes(dev, base + off, len).ok()
}

/// ¿Es el sector 0 el arranque de un sistema de archivos (y no una tabla de particiones)?
fn fs_boot(s0: &[u8], ss: usize) -> bool {
    fat::detect(s0, ss).is_some() || matches!(&s0[3..11], b"EXFAT   " | b"NTFS    " | b"-FVE-FS-") || &s0[0..4] == b"NFFS"
}

/// Qué hay dentro de la partición `[start, start+sectors)`.
fn detect_kind(dev: &mut dyn SectorDev, start: u64, sectors: u64) -> FsKind {
    let ss = dev.sector_size();
    let base = start.saturating_mul(ss as u64);
    let bytes = sectors.saturating_mul(ss as u64);
    if let Some(s0) = rd(dev, base, bytes, 0, 512) {
        if let Some(k) = fat::detect(&s0, ss) {
            return k;
        }
        if &s0[3..11] == b"EXFAT   " && s0[510] == 0x55 && s0[511] == 0xAA {
            return FsKind::ExFat;
        }
        if &s0[0..4] == b"NFFS" {
            return FsKind::NightFury;
        }
        if &s0[3..11] == b"NTFS    " && s0[510] == 0x55 && s0[511] == 0xAA {
            return FsKind::Ntfs;
        }
        // BitLocker pone su firma en el lugar del nombre OEM del sector de arranque.
        if &s0[3..11] == b"-FVE-FS-" {
            return FsKind::BitLocker;
        }
    }
    // ext2/3/4: superbloque en el byte 1024, firma 0xEF53 en su desplazamiento 56.
    if let Some(sb) = rd(dev, base, bytes, 1024 + 56, 2) {
        if sb == [0x53, 0xEF] {
            return FsKind::Ext;
        }
    }
    // Disco NightFury cifrado: la cabecera está en el último bloque de 4096 bytes.
    if bytes >= 4096 * 2 {
        let sig = crate::cryptdev::SIGNATURE.as_bytes();
        if let Some(h) = rd(dev, base, bytes, (bytes / 4096 - 1) * 4096, sig.len()) {
            if h == sig {
                return FsKind::NightFury;
            }
        }
    }
    // ISO9660: "CD001" en el descriptor del sector (de 2048) número 16.
    if let Some(d) = rd(dev, base, bytes, 32768, 6) {
        if &d[1..6] == b"CD001" {
            return FsKind::Iso9660;
        }
    }
    // UDF: secuencia de reconocimiento BEA01 ... NSR02/NSR03 en descriptores de 2048 desde 32768.
    let mut bea = false;
    for i in 0..32u64 {
        let Some(d) = rd(dev, base, bytes, 32768 + i * 2048, 6) else { break };
        match &d[1..6] {
            b"BEA01" => bea = true,
            b"NSR02" | b"NSR03" if bea => return FsKind::Udf,
            b"TEA01" => break,
            _ => {}
        }
    }
    FsKind::Unknown
}

/// Cabecera GPT en `lba` y su tabla de entradas, si las dos son válidas (CRC incluidos).
fn gpt_raw(dev: &mut dyn SectorDev, lba: u64) -> Option<(Vec<u8>, Vec<u8>)> {
    let ss = dev.sector_size() as u64;
    let n = dev.sectors();
    let h = read_bytes(dev, lba * ss, 92).ok()?;
    let hsize = u32le(&h, 12) as usize;
    if &h[0..8] != b"EFI PART" || hsize < 92 || hsize as u64 > ss {
        return None;
    }
    let h = read_bytes(dev, lba * ss, hsize).ok()?;
    let mut z = h.clone();
    z[16..20].fill(0);
    if crc32(&z) != u32le(&h, 16) || u64le(&h, 24) != lba {
        return None;
    }
    let (tlba, num, esz) = (u64le(&h, 72), u32le(&h, 80) as u64, u32le(&h, 84) as u64);
    if num == 0 || esz < 128 || !esz.is_power_of_two() || num * esz > 1 << 20 {
        return None;
    }
    if tlba.checked_add((num * esz).div_ceil(ss))? > n {
        return None;
    }
    let t = read_bytes(dev, tlba * ss, (num * esz) as usize).ok()?;
    (crc32(&t) == u32le(&h, 88)).then_some((h, t))
}

/// Particiones de una tabla GPT con cabecera en `lba`: (primer, último) sectores. `None` si no es válida.
fn gpt(dev: &mut dyn SectorDev, lba: u64) -> Option<Vec<(u64, u64)>> {
    let n = dev.sectors();
    let (h, t) = gpt_raw(dev, lba)?;
    let mut out = Vec::new();
    for e in t.chunks(u32le(&h, 84) as usize) {
        let (f, l) = (u64le(e, 32), u64le(e, 40));
        if e[..16].iter().any(|&b| b != 0) && f <= l && l < n && !NOT_DATA_GPT.iter().any(|g| guid(g) == e[..16]) {
            out.push((f, l));
        }
    }
    Some(out)
}

/// Tipos GPT que no son de datos y no reciben letra (especificación UEFI y tipos publicados por
/// Microsoft y freedesktop): sistema EFI, reservada de Microsoft, recuperación de Windows, metadatos
/// de discos dinámicos, swap de Linux, LVM y RAID de Linux.
const NOT_DATA_GPT: [&str; 8] = [
    "C12A7328-F81F-11D2-BA4B-00A0C93EC93B",
    "E3C9E316-0B5C-4DB8-817D-F92DF00215AE",
    "DE94BBA4-06D1-4D40-A16A-BFD50179D6AC",
    "5808C8AA-7E8F-42E0-85D2-E1E90434CFB3",
    "AF9B60A0-1431-4F62-BC68-3311714A69AD",
    "0657FD6D-A4AB-43C4-84E5-0933C84B4F4F",
    "E6D6D379-F507-44C2-A23C-238F2A3DF928",
    "A19D880F-05FC-4D3B-A006-743F0F84911E",
];

/// Tipos MBR equivalentes: sistema EFI, recuperación de Windows, swap de Linux, LVM, RAID y
/// discos dinámicos de Windows.
const NOT_DATA_MBR: [u8; 6] = [0xEF, 0x27, 0x82, 0x8E, 0xFD, 0x42];

/// GUID en texto a los 16 bytes del disco (los tres primeros grupos van al revés).
pub(crate) fn guid(s: &str) -> [u8; 16] {
    let h: Vec<u8> = s.bytes().filter(|c| *c != b'-').collect();
    let mut b = [0u8; 16];
    for (i, x) in b.iter_mut().enumerate() {
        let d = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
        *x = d(h[2 * i]) << 4 | d(h[2 * i + 1]);
    }
    b[0..4].reverse();
    b[4..6].reverse();
    b[6..8].reverse();
    b
}

fn is_ext(t: u8) -> bool {
    matches!(t, 0x05 | 0x0F | 0x85)
}

/// Particiones (inicio, sectores) de un MBR; `None` si el sector 0 no es un MBR válido.
fn mbr(dev: &mut dyn SectorDev, s0: &[u8]) -> Option<Vec<(u64, u64)>> {
    let n = dev.sectors();
    if s0[510] != 0x55 || s0[511] != 0xAA || (0..4).any(|i| !matches!(s0[446 + 16 * i], 0 | 0x80)) {
        return None;
    }
    let mut out = Vec::new();
    let mut ext = None;
    for i in 0..4 {
        let e = &s0[446 + 16 * i..462 + 16 * i];
        let (t, s, l) = (e[4], u32le(e, 8) as u64, u32le(e, 12) as u64);
        if t == 0 || l == 0 || s >= n {
            continue;
        }
        if is_ext(t) {
            ext = Some(s);
        } else if t != 0xEE && !NOT_DATA_MBR.contains(&t) {
            out.push((s, l.min(n - s)));
        }
    }
    // Lógicas: cadena de EBR; el siguiente cuelga del inicio de la extendida. Tope contra bucles.
    if let Some(base) = ext {
        let mut lba = base;
        for _ in 0..128 {
            let off = lba * dev.sector_size() as u64;
            let Ok(b) = read_bytes(dev, off, 512) else { break };
            if b[510] != 0x55 || b[511] != 0xAA {
                break;
            }
            let (t, s, l) = (b[450], lba + u32le(&b, 454) as u64, u32le(&b, 458) as u64);
            if t != 0 && !is_ext(t) && !NOT_DATA_MBR.contains(&t) && l != 0 && s < n {
                out.push((s, l.min(n - s)));
            }
            let (t2, s2) = (b[466], u32le(&b, 470) as u64);
            if !is_ext(t2) || s2 == 0 {
                break;
            }
            // El siguiente EBR siempre está más adelante: así la cadena no puede dar vueltas.
            if base + s2 <= lba || base + s2 >= n {
                break;
            }
            lba = base + s2;
            if lba >= n {
                break;
            }
        }
    }
    Some(out)
}

pub fn partitions(dev: &mut dyn SectorDev) -> Result<Vec<Partition>, FsError> {
    let n = dev.sectors();
    let ss = dev.sector_size();
    if n == 0 {
        return Ok(Vec::new());
    }
    let s0 = read_bytes(dev, 0, 512)?;
    let ranges: Vec<(u64, u64)> = if let Some(g) = gpt(dev, 1).or_else(|| if n > 2 { gpt(dev, n - 1) } else { None }) {
        g.into_iter().map(|(f, l)| (f, l - f + 1)).collect()
    } else if fs_boot(&s0, ss) {
        vec![(0, n)]
    } else if let Some(m) = mbr(dev, &s0) {
        m
    } else {
        vec![(0, n)]
    };
    Ok(ranges.into_iter().map(|(start, sectors)| Partition { start, sectors, kind: detect_kind(dev, start, sectors) }).collect())
}

/// Prepara el disco para formatear con una sola partición: calcula (inicio, sectores) alineados a
/// 1 MiB y borra las cabeceras GPT para que no tapen al MBR. No escribe todavía el MBR.
/// shortcut: en discos de menos de 8 MiB el inicio es de 4 KiB (no 1 MiB) para que quepa algo; MBR limita a 2^32 sectores.
pub(super) fn prepare(dev: &mut dyn SectorDev) -> Result<(u64, u64), FsError> {
    if dev.read_only() {
        return Err(FsError::Denied);
    }
    let ss = dev.sector_size() as u64;
    let n = dev.sectors();
    let start = if n * ss >= 8 << 20 { (1 << 20) / ss } else { (4096 / ss).max(2) };
    if n <= start + 16 {
        return Err(FsError::NoSpace);
    }
    let zero = vec![0u8; ss as usize];
    dev.write(1, &zero)?;
    dev.write(n - 1, &zero)?;
    Ok((start, (n - start).min(u32::MAX as u64 - start)))
}

/// Tipo GPT «Basic data» (el de las particiones de Windows y de los pendrives).
const BASIC_DATA: &str = "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7";

/// Ajusta el tipo de la partición que empieza en `start` a lo que ahora contiene (`kind`), sin tocar
/// nada más de la tabla: en MBR el byte de tipo (0x0C FAT32, 0x0E FAT16, 0x01 FAT12, 0x07 exFAT y
/// NightFury); en GPT el GUID «Basic data», con los CRC de la tabla y de las dos cabeceras rehechos.
/// Si el disco no tiene tabla o no hay partición que empiece ahí, no hace nada.
pub fn set_type(dev: &mut dyn SectorDev, start: u64, kind: FsKind) -> Result<(), FsError> {
    let ss = dev.sector_size() as u64;
    let n = dev.sectors();
    let mut gpt_found = false;
    for lba in [1, n.saturating_sub(1)] {
        let Some((mut h, mut t)) = gpt_raw(dev, lba) else { continue };
        gpt_found = true;
        let esz = u32le(&h, 84) as usize;
        let mut changed = false;
        for e in t.chunks_mut(esz) {
            if e[..16].iter().any(|&b| b != 0) && u64le(e, 32) == start {
                e[..16].copy_from_slice(&guid(BASIC_DATA));
                changed = true;
            }
        }
        if changed {
            let tlba = u64le(&h, 72);
            h[88..92].copy_from_slice(&crc32(&t).to_le_bytes());
            h[16..20].fill(0);
            let c = crc32(&h);
            h[16..20].copy_from_slice(&c.to_le_bytes());
            // Primero la tabla, luego la cabecera: un corte deja la cabecera vieja, que no cuadra y se ignora.
            write_bytes(dev, tlba * ss, &t)?;
            write_bytes(dev, lba * ss, &h)?;
        }
    }
    if gpt_found {
        return dev.flush();
    }
    let t = match kind {
        FsKind::Fat32 => 0x0C,
        FsKind::Fat16 => 0x0E,
        FsKind::Fat12 => 0x01,
        _ => 0x07,
    };
    let mut s0 = read_bytes(dev, 0, ss as usize)?;
    if fs_boot(&s0, ss as usize) || s0[510] != 0x55 || s0[511] != 0xAA {
        return Ok(());
    }
    for i in 0..4 {
        let o = 446 + 16 * i;
        if s0[o + 4] != 0 && !is_ext(s0[o + 4]) && u32le(&s0, o + 8) as u64 == start {
            s0[o + 4] = t;
            dev.write(0, &s0)?;
            return dev.flush();
        }
    }
    // Lógicas: cadena de EBR como en `mbr`.
    let base = (0..4).find_map(|i| (is_ext(s0[446 + 16 * i + 4])).then(|| u32le(&s0, 446 + 16 * i + 8) as u64));
    let mut lba = base.unwrap_or(n);
    for _ in 0..128 {
        if lba >= n {
            break;
        }
        let mut b = read_bytes(dev, lba * ss, ss as usize)?;
        if b[510] != 0x55 || b[511] != 0xAA {
            break;
        }
        if b[450] != 0 && lba + u32le(&b, 454) as u64 == start {
            b[450] = t;
            dev.write(lba, &b)?;
            return dev.flush();
        }
        let s2 = u32le(&b, 470) as u64;
        if !is_ext(b[466]) || s2 == 0 || base.unwrap_or(0) + s2 <= lba {
            break;
        }
        lba = base.unwrap_or(0) + s2;
    }
    Ok(())
}

/// Escribe `data` desde el byte `off` (alineado a sector) rellenando el último sector con lo que había.
fn write_bytes(dev: &mut dyn SectorDev, off: u64, data: &[u8]) -> Result<(), FsError> {
    let ss = dev.sector_size() as u64;
    let secs = (data.len() as u64).div_ceil(ss);
    let mut buf = read_bytes(dev, off, (secs * ss) as usize)?;
    buf[..data.len()].copy_from_slice(data);
    dev.write(off / ss, &buf)
}

/// Como `prepare` pero para formatear DENTRO de una partición (el dispositivo ya es una ventana):
/// ocupa todo y no toca ninguna tabla.
/// shortcut: FAT y MBR limitan a 2^32 sectores; una partición mayor se queda en esos sectores.
pub(super) fn whole(dev: &mut dyn SectorDev) -> Result<(u64, u64), FsError> {
    if dev.read_only() {
        return Err(FsError::Denied);
    }
    let n = dev.sectors();
    if n <= 16 {
        return Err(FsError::NoSpace);
    }
    Ok((0, n.min(u32::MAX as u64)))
}

/// Escribe un MBR con una única partición.
pub(super) fn write_mbr(dev: &mut dyn SectorDev, start: u64, sectors: u64, ptype: u8) -> Result<(), FsError> {
    let mut b = vec![0u8; dev.sector_size()];
    b[446 + 1..446 + 4].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    b[446 + 4] = ptype;
    b[446 + 5..446 + 8].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    b[446 + 8..446 + 12].copy_from_slice(&(start as u32).to_le_bytes());
    b[446 + 12..446 + 16].copy_from_slice(&(sectors as u32).to_le_bytes());
    b[510] = 0x55;
    b[511] = 0xAA;
    dev.write(0, &b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vol::MemSectors;

    #[test]
    fn guid_en_disco() {
        // Sistema EFI tal como aparece en una tabla GPT real.
        assert_eq!(guid("C12A7328-F81F-11D2-BA4B-00A0C93EC93B"), [0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B]);
    }

    #[test]
    fn ntfs_bitlocker_y_ext_se_reconocen() {
        let mut d = MemSectors::new(4096, 512);
        d.data[3..11].copy_from_slice(b"NTFS    ");
        d.data[510] = 0x55;
        d.data[511] = 0xAA;
        assert_eq!(detect_kind(&mut d, 0, 4096), FsKind::Ntfs);
        d.data[3..11].copy_from_slice(b"-FVE-FS-");
        assert_eq!(detect_kind(&mut d, 0, 4096), FsKind::BitLocker);
        let mut d = MemSectors::new(4096, 512);
        d.data[1024 + 56] = 0x53;
        d.data[1024 + 57] = 0xEF;
        assert_eq!(detect_kind(&mut d, 0, 4096), FsKind::Ext);
    }

    #[test]
    fn particiones_que_no_son_de_datos_no_salen() {
        let mut d = MemSectors::new(1000, 512);
        put_mbr(&mut d, 0, 0x07, 10, 100);
        put_mbr(&mut d, 1, 0x27, 200, 100);
        put_mbr(&mut d, 2, 0x82, 400, 100);
        put_mbr(&mut d, 3, 0x83, 600, 100);
        let p = partitions(&mut d).unwrap();
        assert_eq!(p.iter().map(|p| p.start).collect::<Vec<_>>(), vec![10, 600]);
    }

    #[test]
    fn crc32_known() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    fn put_mbr(d: &mut MemSectors, i: usize, t: u8, s: u32, l: u32) {
        let o = 446 + 16 * i;
        d.data[o + 4] = t;
        d.data[o + 8..o + 12].copy_from_slice(&s.to_le_bytes());
        d.data[o + 12..o + 16].copy_from_slice(&l.to_le_bytes());
        d.data[510] = 0x55;
        d.data[511] = 0xAA;
    }

    #[test]
    fn superfloppy_and_blank() {
        let mut d = MemSectors::new(100, 512);
        let p = partitions(&mut d).unwrap();
        assert_eq!(p, vec![Partition { start: 0, sectors: 100, kind: FsKind::Unknown }]);
        d.data[0..4].copy_from_slice(b"NFFS");
        assert_eq!(partitions(&mut d).unwrap()[0].kind, FsKind::NightFury);
    }

    #[test]
    fn mbr_primary_and_logical() {
        let mut d = MemSectors::new(1000, 512);
        put_mbr(&mut d, 0, 0x0C, 10, 100);
        put_mbr(&mut d, 1, 0x00, 0, 0);
        put_mbr(&mut d, 2, 0x83, 200, 50);
        put_mbr(&mut d, 3, 0x05, 300, 600);
        // EBR en 300: lógica en 300+1 (20 sectores), siguiente EBR en 300+40.
        let e = 300 * 512;
        d.data[e + 446 + 4] = 0x83;
        d.data[e + 446 + 8..e + 446 + 12].copy_from_slice(&1u32.to_le_bytes());
        d.data[e + 446 + 12..e + 446 + 16].copy_from_slice(&20u32.to_le_bytes());
        d.data[e + 462 + 4] = 0x05;
        d.data[e + 462 + 8..e + 462 + 12].copy_from_slice(&40u32.to_le_bytes());
        d.data[e + 462 + 12..e + 462 + 16].copy_from_slice(&30u32.to_le_bytes());
        d.data[e + 510] = 0x55;
        d.data[e + 511] = 0xAA;
        let e2 = 340 * 512;
        d.data[e2 + 446 + 4] = 0x07;
        d.data[e2 + 446 + 8..e2 + 446 + 12].copy_from_slice(&1u32.to_le_bytes());
        d.data[e2 + 446 + 12..e2 + 446 + 16].copy_from_slice(&25u32.to_le_bytes());
        // Segundo EBR apunta a sí mismo: no debe haber bucle.
        d.data[e2 + 462 + 4] = 0x05;
        d.data[e2 + 462 + 8..e2 + 462 + 12].copy_from_slice(&40u32.to_le_bytes());
        d.data[e2 + 510] = 0x55;
        d.data[e2 + 511] = 0xAA;
        let p = partitions(&mut d).unwrap();
        let r: Vec<(u64, u64)> = p.iter().map(|p| (p.start, p.sectors)).collect();
        assert_eq!(r, vec![(10, 100), (200, 50), (301, 20), (341, 25)]);
    }

    /// GPT mínima a mano; devuelve el disco. Particiones: (primer, último).
    fn make_gpt(parts: &[(u64, u64)], ss: usize, n: u64) -> MemSectors {
        let mut d = MemSectors::new(n, ss);
        let mut t = vec![0u8; 128 * 128];
        for (i, &(f, l)) in parts.iter().enumerate() {
            t[i * 128] = 0xAF; // GUID de tipo no nulo
            t[i * 128 + 32..i * 128 + 40].copy_from_slice(&f.to_le_bytes());
            t[i * 128 + 40..i * 128 + 48].copy_from_slice(&l.to_le_bytes());
        }
        let tsec = (t.len() / ss) as u64;
        let hdr = |my: u64, alt: u64, tl: u64| {
            let mut h = vec![0u8; 92];
            h[0..8].copy_from_slice(b"EFI PART");
            h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
            h[12..16].copy_from_slice(&92u32.to_le_bytes());
            h[24..32].copy_from_slice(&my.to_le_bytes());
            h[32..40].copy_from_slice(&alt.to_le_bytes());
            h[72..80].copy_from_slice(&tl.to_le_bytes());
            h[80..84].copy_from_slice(&128u32.to_le_bytes());
            h[84..88].copy_from_slice(&128u32.to_le_bytes());
            h[88..92].copy_from_slice(&crc32(&t).to_le_bytes());
            let c = crc32(&h);
            h[16..20].copy_from_slice(&c.to_le_bytes());
            h
        };
        let h1 = hdr(1, n - 1, 2);
        d.data[ss..ss + 92].copy_from_slice(&h1);
        d.data[2 * ss..2 * ss + t.len()].copy_from_slice(&t);
        let tl2 = n - 1 - tsec;
        let h2 = hdr(n - 1, 1, tl2);
        d.data[(n as usize - 1) * ss..(n as usize - 1) * ss + 92].copy_from_slice(&h2);
        d.data[tl2 as usize * ss..tl2 as usize * ss + t.len()].copy_from_slice(&t);
        // MBR protector
        put_mbr(&mut d, 0, 0xEE, 1, (n - 1) as u32);
        d
    }

    #[test]
    fn gpt_primary_and_backup() {
        for ss in [512usize, 4096] {
            let n = 400;
            let mut d = make_gpt(&[(40, 99), (100, 199)], ss, n);
            let p = partitions(&mut d).unwrap();
            assert_eq!(p.iter().map(|p| (p.start, p.sectors)).collect::<Vec<_>>(), vec![(40, 60), (100, 100)]);
            // Rompe la tabla principal (un bit de la tabla): usa la de respaldo.
            d.data[2 * ss + 33] ^= 1;
            let p = partitions(&mut d).unwrap();
            assert_eq!(p.len(), 2, "ss={ss}");
            // Rompe también la cabecera de respaldo: no queda GPT; el MBR protector no cuenta.
            let last = (n as usize - 1) * ss;
            d.data[last + 20] ^= 1;
            assert!(partitions(&mut d).unwrap().is_empty());
        }
    }

    #[test]
    fn set_type_mbr_solo_cambia_el_byte_de_tipo() {
        let mut d = MemSectors::new(1000, 512);
        put_mbr(&mut d, 0, 0x07, 10, 100);
        put_mbr(&mut d, 1, 0x83, 200, 50);
        put_mbr(&mut d, 2, 0x07, 300, 600);
        let antes = d.data[..512].to_vec();
        set_type(&mut d, 200, FsKind::Fat32).unwrap();
        set_type(&mut d, 300, FsKind::ExFat).unwrap();
        assert_eq!(d.data[446 + 16 + 4], 0x0C);
        assert_eq!(d.data[446 + 32 + 4], 0x07);
        // Nada más cambió: solo ese byte.
        let ahora = d.data[..512].to_vec();
        let dif: Vec<usize> = (0..512).filter(|&i| antes[i] != ahora[i]).collect();
        assert_eq!(dif, vec![446 + 16 + 4]);
        // Una partición que no existe no cambia nada.
        set_type(&mut d, 999, FsKind::Fat32).unwrap();
        assert_eq!(&d.data[..512], &ahora[..]);
    }

    #[test]
    fn set_type_logica() {
        let mut d = MemSectors::new(1000, 512);
        put_mbr(&mut d, 0, 0x05, 300, 600);
        let e = 300 * 512;
        d.data[e + 446 + 4] = 0x83;
        d.data[e + 446 + 8..e + 446 + 12].copy_from_slice(&1u32.to_le_bytes());
        d.data[e + 446 + 12..e + 446 + 16].copy_from_slice(&20u32.to_le_bytes());
        d.data[e + 510] = 0x55;
        d.data[e + 511] = 0xAA;
        set_type(&mut d, 301, FsKind::Fat32).unwrap();
        assert_eq!(d.data[e + 446 + 4], 0x0C);
        assert_eq!(d.data[446 + 4], 0x05);
    }

    #[test]
    fn set_type_gpt_rehace_los_crc() {
        for ss in [512usize, 4096] {
            let n = 400;
            let mut d = make_gpt(&[(40, 99), (100, 199)], ss, n);
            set_type(&mut d, 100, FsKind::ExFat).unwrap();
            // Las dos cabeceras y tablas siguen siendo válidas y la segunda partición es «Basic data».
            for lba in [1, n - 1] {
                let (h, t) = gpt_raw(&mut d, lba).expect("CRC válido");
                let esz = u32le(&h, 84) as usize;
                assert_eq!(&t[esz..esz + 16], &guid(BASIC_DATA));
                assert_eq!(t[0], 0xAF, "la primera partición no cambia");
            }
            assert_eq!(partitions(&mut d).unwrap().len(), 2);
        }
    }

    #[test]
    fn kinds_iso_udf_crypt() {
        let mut d = MemSectors::new(100, 2048);
        d.data[16 * 2048] = 1;
        d.data[16 * 2048 + 1..16 * 2048 + 6].copy_from_slice(b"CD001");
        assert_eq!(partitions(&mut d).unwrap()[0].kind, FsKind::Iso9660);
        let mut u = MemSectors::new(100, 512);
        u.data[32768 + 1..32768 + 6].copy_from_slice(b"BEA01");
        u.data[32768 + 2048 + 1..32768 + 2048 + 6].copy_from_slice(b"NSR03");
        assert_eq!(partitions(&mut u).unwrap()[0].kind, FsKind::Udf);
        let mut c = MemSectors::new(64, 512);
        let last = c.data.len() - 4096;
        c.data[last..last + 8].copy_from_slice(b"NFCRYPT1");
        assert_eq!(partitions(&mut c).unwrap()[0].kind, FsKind::NightFury);
    }
}
