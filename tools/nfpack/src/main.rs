// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! nfpack: convierte el ELF de un programa (x86_64-unknown-none, enlazado con programs/link.ld a
//! su dirección fija) en un `.nfx` que carga el núcleo. Uso: `nfpack <elf> <salida.nfx>`.
//!
//! Solo lee lo necesario del ELF64: la cabecera, las cabeceras de programa `PT_LOAD` y, para
//! rechazarlas, las secciones con relocaciones. El `.nfx` no admite reubicaciones al cargar:
//! el programa tiene que venir ya enlazado en su sitio.
use nf_core::abi::PAGE;
use nf_core::nfx::{self, Kind, Section};
use std::process::ExitCode;

/// Un `PT_LOAD` del ELF.
#[derive(Debug, PartialEq, Eq)]
struct Segmento {
    vaddr: u64,
    memsz: u64,
    off: u64,
    filesz: u64,
    flags: u32,
}

#[derive(Debug)]
struct Elf {
    entry: u64,
    segs: Vec<Segmento>,
}

const PF_X: u32 = 1;
const PF_W: u32 = 2;

fn u16_at(b: &[u8], i: usize) -> Result<u64, String> {
    b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as u64).ok_or_else(|| "ELF truncado".to_string())
}
fn u32_at(b: &[u8], i: usize) -> Result<u64, String> {
    b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as u64).ok_or_else(|| "ELF truncado".to_string())
}
fn u64_at(b: &[u8], i: usize) -> Result<u64, String> {
    b.get(i..i + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap())).ok_or_else(|| "ELF truncado".to_string())
}

/// Lee un ELF64 little endian de x86-64 ejecutable (no reubicable) y sus `PT_LOAD`.
fn leer_elf(b: &[u8]) -> Result<Elf, String> {
    if b.len() < 64 || &b[0..4] != b"\x7fELF" {
        return Err("no es un ELF".into());
    }
    if b[4] != 2 || b[5] != 1 {
        return Err("el ELF no es de 64 bits little endian".into());
    }
    if u16_at(b, 18)? != 62 {
        return Err("el ELF no es de x86-64".into());
    }
    match u16_at(b, 16)? {
        2 => {}
        3 => return Err("ejecutable reubicable (PIE): no se admiten reubicaciones al cargar; enlaza con programs/link.ld (sin -pie)".into()),
        t => return Err(format!("tipo de ELF {t} no soportado (hace falta ejecutable)")),
    }
    let entry = u64_at(b, 24)?;
    // Secciones con relocaciones que ocupan memoria: el .nfx no las aplica.
    let (shoff, shentsize, shnum) = (u64_at(b, 40)?, u16_at(b, 58)?, u16_at(b, 60)?);
    for i in 0..shnum {
        let o = (shoff + i * shentsize) as usize;
        let (tipo, flags) = (u32_at(b, o + 4)?, u64_at(b, o + 8)?);
        if (tipo == 4 || tipo == 9) && flags & 2 != 0 {
            return Err("el ELF tiene relocaciones que se aplican al cargar; el .nfx no las admite".into());
        }
    }
    let (phoff, phentsize, phnum) = (u64_at(b, 32)?, u16_at(b, 54)?, u16_at(b, 56)?);
    if phentsize < 56 {
        return Err("cabecera de programa de tamaño inválido".into());
    }
    let mut segs = Vec::new();
    for i in 0..phnum {
        let o = (phoff + i * phentsize) as usize;
        match u32_at(b, o)? {
            1 => {
                let (flags, off, vaddr, filesz, memsz) = (u32_at(b, o + 4)? as u32, u64_at(b, o + 8)?, u64_at(b, o + 16)?, u64_at(b, o + 32)?, u64_at(b, o + 40)?);
                if filesz > memsz || off.checked_add(filesz).is_none_or(|e| e > b.len() as u64) {
                    return Err("segmento PT_LOAD fuera del archivo".into());
                }
                if memsz > 0 {
                    segs.push(Segmento { vaddr, memsz, off, filesz, flags });
                }
            }
            // Un PT_DYNAMIC solo existe en ejecutables enlazados dinámicamente.
            2 => return Err("el ELF es dinámico (PT_DYNAMIC); no se admite".into()),
            _ => {}
        }
    }
    if segs.is_empty() {
        return Err("el ELF no tiene segmentos PT_LOAD".into());
    }
    Ok(Elf { entry, segs })
}

/// Del ELF al `.nfx` ya comprobado con `nfx::parse`.
fn empaquetar(elf_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let elf = leer_elf(elf_bytes)?;
    let mut secciones: Vec<Section> = Vec::new();
    for s in &elf.segs {
        if s.flags & PF_W != 0 && s.flags & PF_X != 0 {
            return Err(format!("el segmento en {:#x} es escribible y ejecutable (W+X)", s.vaddr));
        }
        if s.vaddr % PAGE != 0 {
            return Err(format!("el segmento en {:#x} no empieza en página (revisa link.ld)", s.vaddr));
        }
        let datos = &elf_bytes[s.off as usize..(s.off + s.filesz) as usize];
        if s.flags & PF_X != 0 {
            secciones.push(Section { kind: Kind::Code, vaddr: s.vaddr, memsz: s.memsz, data: datos });
        } else if s.flags & PF_W == 0 {
            secciones.push(Section { kind: Kind::ReadOnly, vaddr: s.vaddr, memsz: s.memsz, data: datos });
        } else {
            // Datos con su cola de bss. Las páginas enteras de bss van en una sección Bss aparte;
            // lo que cae en la última página de los datos queda en Data (se rellena con ceros).
            let paginas = s.filesz.div_ceil(PAGE) * PAGE;
            let bss = s.memsz > paginas;
            if s.filesz > 0 {
                secciones.push(Section { kind: Kind::Data, vaddr: s.vaddr, memsz: if bss { s.filesz } else { s.memsz }, data: datos });
            }
            if bss {
                secciones.push(Section { kind: Kind::Bss, vaddr: s.vaddr + paginas, memsz: s.memsz - paginas, data: &[] });
            }
        }
    }
    secciones.sort_by_key(|s| s.vaddr);
    let out = nfx::encode(elf.entry, &secciones);
    nfx::parse(&out).map_err(|e| format!("el .nfx resultante no es válido ({e:?}): revisa direcciones y secciones en link.ld"))?;
    Ok(out)
}

/// Una línea por sección, para ver qué se ha empaquetado.
fn resumen(nfx_bytes: &[u8]) -> String {
    let img = nfx::parse(nfx_bytes).expect("ya comprobado");
    let mut t = format!("entrada {:#x}, {} bytes\n", img.entry, nfx_bytes.len());
    for s in &img.sections {
        t += &format!("  {:<9} dirección {:#014x}  memoria {:>7}  archivo {:>7}\n", format!("{:?}", s.kind), s.vaddr, s.memsz, s.data.len());
    }
    t
}

fn main() -> ExitCode {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() != 2 {
        eprintln!("uso: nfpack <elf> <salida.nfx>");
        return ExitCode::from(2);
    }
    let r = std::fs::read(&a[0]).map_err(|e| format!("no puedo leer {}: {e}", a[0])).and_then(|b| empaquetar(&b)).and_then(|o| {
        std::fs::write(&a[1], &o).map_err(|e| format!("no puedo escribir {}: {e}", a[1]))?;
        Ok(o)
    });
    match r {
        Ok(o) => {
            print!("{} -> {}\n{}", a[0], a[1], resumen(&o));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("nfpack: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nf_core::abi::USER_BASE;

    const B: u64 = USER_BASE + 0x40_0000;

    /// ELF mínimo a mano: (flags, vaddr, datos, memsz) por segmento. `tipo` = e_type; con `rela`
    /// añade una tabla de secciones con una `.rela.dyn` asignable.
    fn elf(tipo: u16, entry: u64, segs: &[(u32, u64, Vec<u8>, u64)], rela: bool) -> Vec<u8> {
        let ph = 64usize;
        let datos0 = ph + segs.len() * 56;
        let mut o = vec![0u8; datos0];
        o[0..4].copy_from_slice(b"\x7fELF");
        o[4] = 2;
        o[5] = 1;
        o[16..18].copy_from_slice(&tipo.to_le_bytes());
        o[18..20].copy_from_slice(&62u16.to_le_bytes());
        o[24..32].copy_from_slice(&entry.to_le_bytes());
        o[32..40].copy_from_slice(&(ph as u64).to_le_bytes());
        o[54..56].copy_from_slice(&56u16.to_le_bytes());
        o[56..58].copy_from_slice(&(segs.len() as u16).to_le_bytes());
        o[58..60].copy_from_slice(&64u16.to_le_bytes());
        let mut off = datos0;
        for (i, (flags, vaddr, d, memsz)) in segs.iter().enumerate() {
            let p = ph + i * 56;
            o[p..p + 4].copy_from_slice(&1u32.to_le_bytes());
            o[p + 4..p + 8].copy_from_slice(&flags.to_le_bytes());
            o[p + 8..p + 16].copy_from_slice(&(off as u64).to_le_bytes());
            o[p + 16..p + 24].copy_from_slice(&vaddr.to_le_bytes());
            o[p + 32..p + 40].copy_from_slice(&(d.len() as u64).to_le_bytes());
            o[p + 40..p + 48].copy_from_slice(&memsz.to_le_bytes());
            off += d.len();
        }
        for (_, _, d, _) in segs {
            o.extend_from_slice(d);
        }
        if rela {
            let sh = o.len() as u64;
            o[40..48].copy_from_slice(&sh.to_le_bytes());
            o[60..62].copy_from_slice(&1u16.to_le_bytes());
            let mut s = [0u8; 64];
            s[4..8].copy_from_slice(&4u32.to_le_bytes());
            s[8..16].copy_from_slice(&2u64.to_le_bytes());
            o.extend_from_slice(&s);
        }
        o
    }

    fn bueno() -> Vec<u8> {
        elf(2, B, &[(5, B, vec![0x90; 100], 100), (4, B + 0x1000, vec![1; 10], 10), (6, B + 0x2000, vec![2; 5], 0x1800 + 0x2000)], false)
    }

    #[test]
    fn empaqueta_codigo_datos_y_bss() {
        let n = empaquetar(&bueno()).unwrap();
        let img = nfx::parse(&n).unwrap();
        assert_eq!(img.entry, B);
        let t: Vec<_> = img.sections.iter().map(|s| (s.kind, s.vaddr - B, s.memsz, s.data.len())).collect();
        // Datos: 5 bytes de archivo en una página; el resto de la memoria (0x3800) cae en páginas
        // propias desde 0x3000 y va como Bss.
        assert_eq!(t, vec![(Kind::Code, 0, 100, 100), (Kind::ReadOnly, 0x1000, 10, 10), (Kind::Data, 0x2000, 5, 5), (Kind::Bss, 0x3000, 0x2800, 0)]);
    }

    #[test]
    fn cola_de_bss_en_la_misma_pagina_queda_en_data() {
        let e = elf(2, B, &[(5, B, vec![0x90; 4], 4), (6, B + 0x1000, vec![7; 8], 0x100)], false);
        let img = nfx::parse(&empaquetar(&e).unwrap()).map(|i| i.sections.iter().map(|s| (s.kind, s.memsz)).collect::<Vec<_>>());
        assert_eq!(img, Ok(vec![(Kind::Code, 4), (Kind::Data, 0x100)]));
    }

    #[test]
    fn bss_solo_va_como_bss() {
        let e = elf(2, B, &[(5, B, vec![0x90; 4], 4), (6, B + 0x1000, vec![], 0x2000)], false);
        let n = empaquetar(&e).unwrap();
        let img = nfx::parse(&n).unwrap();
        assert_eq!((img.sections[1].kind, img.sections[1].memsz), (Kind::Bss, 0x2000));
    }

    #[test]
    fn rechaza_lo_malo() {
        let msg = |e: Vec<u8>| empaquetar(&e).unwrap_err();
        assert!(msg(b"MZ".to_vec()).contains("no es un ELF"));
        assert!(msg(elf(3, B, &[(5, B, vec![0x90], 1)], false)).contains("PIE"));
        assert!(msg(elf(2, B, &[(7, B, vec![0x90], 1)], false)).contains("W+X"));
        assert!(msg(elf(2, B, &[(5, B, vec![0x90], 1)], true)).contains("relocaciones"));
        assert!(msg(elf(2, B, &[(5, B + 8, vec![0x90], 1)], false)).contains("página"));
        // Fuera de la zona de programas o entrada fuera del código: lo para nfx::parse.
        assert!(msg(elf(2, 0x1000, &[(5, 0x1000, vec![0x90], 1)], false)).contains("no es válido"));
        assert!(msg(elf(2, B + 64, &[(5, B, vec![0x90], 1)], false)).contains("no es válido"));
        // Truncado.
        let mut t = bueno();
        t.truncate(t.len() - 3);
        assert!(msg(t).contains("fuera del archivo"));
        assert!(msg(bueno()[..30].to_vec()).contains("no es un ELF"));
    }
}
