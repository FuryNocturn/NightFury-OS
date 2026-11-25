// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Escritura de tablas GPT según la especificación UEFI 2.x, capítulo 5: MBR protector, cabecera y
//! tabla primarias, y las de respaldo al final del disco. Siempre 128 entradas de 128 bytes.
use super::{crc32, detect_kind, gpt_raw, u32le, u64le};
use crate::fs::FsError;
use crate::vol::{Partition, SectorDev};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Tipo «sistema EFI» (C12A7328-F81F-11D2-BA4B-00A0C93EC93B), tal como va en el disco.
pub const TIPO_ESP: [u8; 16] = [0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B];
/// Tipo propio de las particiones NFFS (6B3F2D8E-5C41-4A97-B0E2-7D9A1F4C8E35), tal como va en el disco.
pub const TIPO_NFFS: [u8; 16] = [0x8E, 0x2D, 0x3F, 0x6B, 0x41, 0x5C, 0x97, 0x4A, 0xB0, 0xE2, 0x7D, 0x9A, 0x1F, 0x4C, 0x8E, 0x35];

/// Entradas de la tabla (siempre 128).
pub const ENTRADAS: usize = 128;
const TAM_ENTRADA: usize = 128;
const TABLA: usize = ENTRADAS * TAM_ENTRADA;

/// Una partición a escribir. `inicio_lba` y `fin_lba` incluidos.
pub struct Particion<'a> {
    pub tipo: [u8; 16],
    pub unica: [u8; 16],
    pub nombre: &'a str,
    pub inicio_lba: u64,
    pub fin_lba: u64,
}

/// Una entrada leída de la tabla.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entrada {
    /// Puesto en la tabla, desde 1 (el «número de partición» que usan UEFI y los sistemas).
    pub numero: usize,
    pub tipo: [u8; 16],
    pub unica: [u8; 16],
    pub inicio_lba: u64,
    pub fin_lba: u64,
    /// Entrada completa (128 bytes) para reescribirla sin perder nombre ni atributos.
    pub cruda: Vec<u8>,
}

impl Entrada {
    pub fn sectores(&self) -> u64 {
        self.fin_lba - self.inicio_lba + 1
    }
}

/// GUID aleatorio (versión 4) a partir de una fuente de números que pone el llamador.
pub fn guid_nuevo(aleatorio: &mut dyn FnMut() -> u64) -> [u8; 16] {
    let mut g = [0u8; 16];
    g[..8].copy_from_slice(&aleatorio().to_le_bytes());
    g[8..].copy_from_slice(&aleatorio().to_le_bytes());
    g[7] = g[7] & 0x0F | 0x40;
    g[8] = g[8] & 0x3F | 0x80;
    g
}

/// Primer sector de `lba` alineado a 1 MiB que no sea menor que `lba`.
pub fn alinear_mib(lba: u64, ss: usize) -> u64 {
    let m = (1u64 << 20) / ss as u64;
    lba.div_ceil(m) * m
}

/// Primer sector usable por particiones: tras el MBR, la cabecera y la tabla primarias.
pub fn primer_lba_usable(ss: usize) -> u64 {
    2 + (TABLA / ss) as u64
}

/// Último sector usable en un disco de `sectores`: antes de la tabla y la cabecera de respaldo.
pub fn ultimo_lba_usable(sectores: u64, ss: usize) -> u64 {
    sectores.saturating_sub(2 + (TABLA / ss) as u64)
}

/// Entrada de 128 bytes de una partición (nombre en UTF-16, 36 unidades como mucho).
fn entrada(p: &Particion) -> [u8; TAM_ENTRADA] {
    let mut e = [0u8; TAM_ENTRADA];
    e[0..16].copy_from_slice(&p.tipo);
    e[16..32].copy_from_slice(&p.unica);
    e[32..40].copy_from_slice(&p.inicio_lba.to_le_bytes());
    e[40..48].copy_from_slice(&p.fin_lba.to_le_bytes());
    for (i, u) in p.nombre.encode_utf16().take(36).enumerate() {
        e[56 + 2 * i..58 + 2 * i].copy_from_slice(&u.to_le_bytes());
    }
    e
}

/// Cabecera de 92 bytes en un sector entero, con sus dos CRC.
fn cabecera(ss: usize, mi: u64, otra: u64, tabla_lba: u64, n: u64, guid: &[u8; 16], tabla: &[u8]) -> Vec<u8> {
    let mut h = vec![0u8; ss];
    h[0..8].copy_from_slice(b"EFI PART");
    h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
    h[12..16].copy_from_slice(&92u32.to_le_bytes());
    h[24..32].copy_from_slice(&mi.to_le_bytes());
    h[32..40].copy_from_slice(&otra.to_le_bytes());
    h[40..48].copy_from_slice(&primer_lba_usable(ss).to_le_bytes());
    h[48..56].copy_from_slice(&ultimo_lba_usable(n, ss).to_le_bytes());
    h[56..72].copy_from_slice(guid);
    h[72..80].copy_from_slice(&tabla_lba.to_le_bytes());
    h[80..84].copy_from_slice(&(ENTRADAS as u32).to_le_bytes());
    h[84..88].copy_from_slice(&(TAM_ENTRADA as u32).to_le_bytes());
    h[88..92].copy_from_slice(&crc32(tabla).to_le_bytes());
    let c = crc32(&h[..92]);
    h[16..20].copy_from_slice(&c.to_le_bytes());
    h
}

/// Escribe MBR protector, y las dos cabeceras y tablas. Primero respaldo, luego tabla primaria y al
/// final su cabecera: un corte a medias deja la GPT vieja o la de respaldo, nunca una mezcla válida.
fn escribir(dev: &mut dyn SectorDev, tabla: &[u8], guid: &[u8; 16]) -> Result<(), FsError> {
    if dev.read_only() {
        return Err(FsError::Denied);
    }
    let (ss, n) = (dev.sector_size(), dev.sectors());
    if ss < 512 || n <= primer_lba_usable(ss) + 2 + (TABLA / ss) as u64 {
        return Err(FsError::NoSpace);
    }
    // La cabecera de respaldo de una GPT anterior (otro tamaño de disco) no debe quedar suelta.
    let vieja = gpt_raw(dev, 1).map(|(h, _)| u64le(&h, 32));
    let tsec = (TABLA / ss) as u64;
    let mut mbr = vec![0u8; ss];
    mbr[446 + 2] = 0x02; // CHS inicial 0x000200 (sector 1 = LBA 1)
    mbr[446 + 4] = 0xEE;
    mbr[446 + 5..446 + 8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    mbr[446 + 8..446 + 12].copy_from_slice(&1u32.to_le_bytes());
    mbr[446 + 12..446 + 16].copy_from_slice(&((n - 1).min(u32::MAX as u64) as u32).to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    if let Some(v) = vieja.filter(|&v| v != n - 1 && v < n) {
        dev.write(v, &vec![0u8; ss])?;
    }
    dev.write(n - 1 - tsec, tabla)?;
    dev.write(n - 1, &cabecera(ss, n - 1, 1, n - 1 - tsec, n, guid, tabla))?;
    dev.write(2, tabla)?;
    dev.write(1, &cabecera(ss, 1, n - 1, 2, n, guid, tabla))?;
    dev.write(0, &mbr)?;
    dev.flush()
}

/// Crea una GPT nueva con estas particiones (borra la tabla anterior; no toca el contenido de las
/// particiones). Las particiones deben caber entre `primer_lba_usable` y `ultimo_lba_usable`, sin
/// solaparse; a lo sumo 128.
pub fn crear_gpt(dev: &mut dyn SectorDev, partes: &[Particion], disco_guid: [u8; 16]) -> Result<(), FsError> {
    let (ss, n) = (dev.sector_size(), dev.sectors());
    let (primero, ultimo) = (primer_lba_usable(ss), ultimo_lba_usable(n, ss));
    let mut orden: Vec<(u64, u64)> = partes.iter().map(|p| (p.inicio_lba, p.fin_lba)).collect();
    orden.sort_unstable();
    let mal = partes.len() > ENTRADAS
        || orden.iter().any(|&(i, f)| i < primero || f > ultimo || i > f)
        || orden.windows(2).any(|w| w[1].0 <= w[0].1)
        || partes.iter().any(|p| p.tipo == [0; 16]);
    if mal {
        return Err(FsError::Invalid);
    }
    let mut tabla = vec![0u8; TABLA];
    for (i, p) in partes.iter().enumerate() {
        tabla[i * TAM_ENTRADA..(i + 1) * TAM_ENTRADA].copy_from_slice(&entrada(p));
    }
    escribir(dev, &tabla, &disco_guid)
}

/// Entradas no vacías de la GPT y el GUID del disco; `None` si no hay GPT válida (primaria o respaldo).
pub fn leer_gpt(dev: &mut dyn SectorDev) -> Option<([u8; 16], Vec<Entrada>)> {
    let n = dev.sectors();
    let (h, t) = gpt_raw(dev, 1).or_else(|| gpt_raw(dev, n.checked_sub(1)?))?;
    let esz = u32le(&h, 84) as usize;
    let mut out = Vec::new();
    for (k, e) in t.chunks(esz).enumerate() {
        let (i, f) = (u64le(e, 32), u64le(e, 40));
        if e[..16].iter().any(|&b| b != 0) && i <= f && f < n {
            out.push(Entrada { numero: k + 1, tipo: e[..16].try_into().ok()?, unica: e[16..32].try_into().ok()?, inicio_lba: i, fin_lba: f, cruda: e[..TAM_ENTRADA].to_vec() });
        }
    }
    Some((h[56..72].try_into().ok()?, out))
}

/// Tabla de 128 entradas con cada una en su puesto (los números de partición no cambian).
fn tabla_de(ent: &[Entrada]) -> Result<Vec<u8>, FsError> {
    let mut t = vec![0u8; TABLA];
    for e in ent {
        if e.numero == 0 || e.numero > ENTRADAS || e.cruda.len() != TAM_ENTRADA {
            return Err(FsError::Invalid);
        }
        t[(e.numero - 1) * TAM_ENTRADA..e.numero * TAM_ENTRADA].copy_from_slice(&e.cruda);
    }
    Ok(t)
}

/// ¿Cabe `nueva` en el disco sin pisar a ninguna de `otras`?
fn cabe(dev: &dyn SectorDev, otras: &[Entrada], nueva: &Particion) -> bool {
    let (ss, n) = (dev.sector_size(), dev.sectors());
    let solapa = otras.iter().any(|e| nueva.inicio_lba <= e.fin_lba && e.inicio_lba <= nueva.fin_lba);
    !(nueva.inicio_lba < primer_lba_usable(ss) || nueva.fin_lba > ultimo_lba_usable(n, ss) || nueva.inicio_lba > nueva.fin_lba || solapa || nueva.tipo == [0; 16])
}

/// Añade una partición a la GPT existente en el primer puesto libre (la reescribe entera, con la copia
/// de respaldo en el final real del disco). Conserva las demás entradas tal cual y en su puesto.
pub fn anadir_particion(dev: &mut dyn SectorDev, nueva: &Particion) -> Result<(), FsError> {
    let (guid, ent) = leer_gpt(dev).ok_or(FsError::Corrupt)?;
    if !cabe(dev, &ent, nueva) {
        return Err(FsError::Invalid);
    }
    let mut tabla = tabla_de(&ent)?;
    let sitio = tabla.chunks(TAM_ENTRADA).position(|c| c.iter().all(|&b| b == 0)).ok_or(FsError::Invalid)?;
    tabla[sitio * TAM_ENTRADA..(sitio + 1) * TAM_ENTRADA].copy_from_slice(&entrada(nueva));
    escribir(dev, &tabla, &guid)
}

/// Cambia la entrada número `numero` por `nueva` (en el mismo puesto), o la borra si es `None`. Las demás
/// quedan tal cual, con las dos cabeceras y tablas rehechas.
pub fn reemplazar(dev: &mut dyn SectorDev, numero: usize, nueva: Option<&Particion>) -> Result<(), FsError> {
    let (guid, ent) = leer_gpt(dev).ok_or(FsError::Corrupt)?;
    if !ent.iter().any(|e| e.numero == numero) {
        return Err(FsError::NotFound);
    }
    let resto: Vec<Entrada> = ent.into_iter().filter(|e| e.numero != numero).collect();
    let mut tabla = tabla_de(&resto)?;
    if let Some(p) = nueva {
        if !cabe(dev, &resto, p) {
            return Err(FsError::Invalid);
        }
        tabla[(numero - 1) * TAM_ENTRADA..numero * TAM_ENTRADA].copy_from_slice(&entrada(p));
    }
    escribir(dev, &tabla, &guid)
}

/// Hueco libre tras la última partición: (primer sector alineado a 1 MiB, último usable), si hay.
pub fn hueco_final(dev: &mut dyn SectorDev) -> Option<(u64, u64)> {
    let ss = dev.sector_size();
    let (_, ent) = leer_gpt(dev)?;
    let desde = ent.iter().map(|e| e.fin_lba + 1).max().unwrap_or(0).max(primer_lba_usable(ss));
    let (i, f) = (alinear_mib(desde, ss), ultimo_lba_usable(dev.sectors(), ss));
    (i <= f).then_some((i, f))
}

/// La partición como la ve `partitions()` (sistema de archivos incluido), también la ESP.
pub fn como_particion(dev: &mut dyn SectorDev, inicio_lba: u64, sectores: u64) -> Partition {
    Partition { start: inicio_lba, sectors: sectores, kind: detect_kind(dev, inicio_lba, sectores) }
}

/// Nombre de la partición (UTF-16 de la entrada cruda).
pub fn nombre(e: &Entrada) -> String {
    let u: Vec<u16> = e.cruda[56..128].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
    String::from_utf16_lossy(&u)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vol::part::guid;
    use crate::vol::MemSectors;

    fn azar() -> impl FnMut() -> u64 {
        let mut x = 0x1234_5678_9ABC_DEF1u64;
        move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        }
    }

    #[test]
    fn guids_propios_en_disco() {
        assert_eq!(TIPO_ESP, guid("C12A7328-F81F-11D2-BA4B-00A0C93EC93B"));
        assert_eq!(TIPO_NFFS, guid("6B3F2D8E-5C41-4A97-B0E2-7D9A1F4C8E35"));
        let g = guid_nuevo(&mut azar());
        assert_eq!(g[7] >> 4, 4);
        assert_eq!(g[8] >> 6, 2);
    }

    #[test]
    fn crea_gpt_y_se_lee_con_las_dos_cabeceras() {
        for ss in [512usize, 4096] {
            let n = (64 << 20) / ss as u64;
            let mut d = MemSectors::new(n, ss);
            let ini = alinear_mib(primer_lba_usable(ss), ss);
            let fin = ini + (8 << 20) / ss as u64 - 1;
            let mut a = azar();
            let u = guid_nuevo(&mut a);
            crear_gpt(&mut d, &[Particion { tipo: TIPO_NFFS, unica: u, nombre: "Datos ñ", inicio_lba: ini, fin_lba: fin }], guid_nuevo(&mut a)).unwrap();
            // Las dos cabeceras y tablas pasan el CRC; la de respaldo está en el último sector.
            for lba in [1, n - 1] {
                let (h, _) = gpt_raw(&mut d, lba).expect("CRC válido");
                assert_eq!(u64le(&h, 48), ultimo_lba_usable(n, ss));
            }
            let (_, e) = leer_gpt(&mut d).unwrap();
            assert_eq!((e.len(), e[0].inicio_lba, e[0].fin_lba, e[0].unica, nombre(&e[0])), (1, ini, fin, u, "Datos ñ".into()));
            assert_eq!(d.data[510], 0x55);
            assert_eq!(d.data[446 + 4], 0xEE);
            let p = crate::vol::partitions(&mut d).unwrap();
            assert_eq!((p.len(), p[0].start, p[0].sectors), (1, ini, fin - ini + 1));
            // Solo con la de respaldo también se lee.
            d.data[ss + 20] ^= 1;
            assert_eq!(leer_gpt(&mut d).unwrap().1.len(), 1);
            // Añadir una segunda parte la reescribe y rechaza solapes.
            let h = hueco_final(&mut d).unwrap();
            assert_eq!(h.0 % ((1 << 20) / ss as u64), 0);
            let nueva = Particion { tipo: TIPO_NFFS, unica: u, nombre: "", inicio_lba: h.0, fin_lba: h.1 };
            assert_eq!(anadir_particion(&mut d, &Particion { inicio_lba: fin, ..nueva }), Err(FsError::Invalid));
            anadir_particion(&mut d, &nueva).unwrap();
            assert_eq!(crate::vol::partitions(&mut d).unwrap().len(), 2);
            assert!(gpt_raw(&mut d, 1).is_some());
        }
    }

    #[test]
    fn reemplazar_conserva_numeros_y_crc() {
        for ss in [512usize, 4096] {
            let n = (64 << 20) / ss as u64;
            let mut d = MemSectors::new(n, ss);
            let m = (1 << 20) / ss as u64;
            let ini = alinear_mib(primer_lba_usable(ss), ss);
            let p = |k: u64, tipo| Particion { tipo, unica: [k as u8; 16], nombre: "x", inicio_lba: ini + k * 4 * m, fin_lba: ini + k * 4 * m + 2 * m - 1 };
            crear_gpt(&mut d, &[p(0, TIPO_ESP), p(1, TIPO_ESP), p(2, TIPO_ESP)], [9; 16]).unwrap();
            // Borrar la 2 deja hueco: la 3 sigue siendo la 3, y añadir ocupa el puesto libre.
            reemplazar(&mut d, 2, None).unwrap();
            let (_, e) = leer_gpt(&mut d).unwrap();
            assert_eq!(e.iter().map(|e| e.numero).collect::<Vec<_>>(), [1, 3]);
            anadir_particion(&mut d, &p(1, TIPO_NFFS)).unwrap();
            let (g, e) = leer_gpt(&mut d).unwrap();
            assert_eq!((g, e.iter().map(|e| (e.numero, e.tipo == TIPO_NFFS)).collect::<Vec<_>>()), ([9; 16], vec![(1, false), (2, true), (3, false)]));
            // Cambiar de tipo y nombre en el sitio; solapar o pedir un puesto inexistente falla.
            reemplazar(&mut d, 3, Some(&Particion { tipo: TIPO_NFFS, nombre: "NightFury", ..p(2, TIPO_NFFS) })).unwrap();
            assert_eq!(reemplazar(&mut d, 3, Some(&Particion { inicio_lba: ini, ..p(2, TIPO_NFFS) })), Err(FsError::Invalid));
            assert_eq!(reemplazar(&mut d, 9, None), Err(FsError::NotFound));
            let (_, e) = leer_gpt(&mut d).unwrap();
            assert_eq!((e[2].tipo, nombre(&e[2]), e[2].unica), (TIPO_NFFS, "NightFury".into(), [2; 16]));
            for lba in [1, n - 1] {
                assert!(gpt_raw(&mut d, lba).is_some());
            }
        }
    }

    #[test]
    fn rechaza_particiones_mal_puestas() {
        let mut d = MemSectors::new(4096, 512);
        let p = |i, f| Particion { tipo: TIPO_NFFS, unica: [1; 16], nombre: "", inicio_lba: i, fin_lba: f };
        assert_eq!(crear_gpt(&mut d, &[p(1, 100)], [0; 16]), Err(FsError::Invalid));
        assert_eq!(crear_gpt(&mut d, &[p(100, 99)], [0; 16]), Err(FsError::Invalid));
        assert_eq!(crear_gpt(&mut d, &[p(100, 4095)], [0; 16]), Err(FsError::Invalid));
        assert_eq!(crear_gpt(&mut d, &[p(100, 200), p(200, 300)], [0; 16]), Err(FsError::Invalid));
        assert!(crear_gpt(&mut d, &[p(100, 200), p(201, 300)], [0; 16]).is_ok());
    }
}
