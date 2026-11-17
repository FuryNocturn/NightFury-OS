//! Formato de programa `.nfx` (F3, v0.8). CONTRATO: el formato en disco, `Kind`, `Section`,
//! `Image`, `parse` y `encode`.
//!
//! Todo little endian:
//! ```text
//! 0   "NFX1"
//! 4   versión de llamadas (u16, = abi::ABI_VERSION)
//! 6   número de secciones (u16, 1..=MAX_SECTIONS)
//! 8   punto de entrada (u64)
//! 16  reservado (u64, 0)
//! 24  secciones, 40 bytes cada una:
//!     tipo u32 · 0 u32 · dirección u64 · tamaño en memoria u64 · posición en el archivo u64 ·
//!     tamaño en el archivo u64
//! ```
//! Lo que pasa `parse` se puede cargar tal cual: cada sección empieza en página, cae entera entre
//! `abi::USER_BASE` y `abi::IMAGE_TOP`, no comparte páginas con otra y sus datos están dentro del
//! archivo. Lo que va de `tamaño en el archivo` a `tamaño en memoria` se rellena con ceros.
use crate::abi::{ABI_VERSION, IMAGE_TOP, MAX_MEM, PAGE, STACK_SIZE, USER_BASE};
use alloc::vec::Vec;

pub const MAGIC: &[u8; 4] = b"NFX1";
pub const MAX_SECTIONS: usize = 16;
const HEADER: usize = 24;
const SECTION: usize = 40;

/// Permisos de una sección: código (lectura + ejecución), solo lectura, datos (lectura +
/// escritura) y bss (lectura + escritura, sin datos en el archivo).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Code = 1,
    ReadOnly = 2,
    Data = 3,
    Bss = 4,
}

impl Kind {
    fn from_u32(v: u32) -> Option<Kind> {
        Some(match v {
            1 => Kind::Code,
            2 => Kind::ReadOnly,
            3 => Kind::Data,
            4 => Kind::Bss,
            _ => return None,
        })
    }
    pub fn writable(self) -> bool {
        matches!(self, Kind::Data | Kind::Bss)
    }
    pub fn executable(self) -> bool {
        self == Kind::Code
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Section<'a> {
    pub kind: Kind,
    pub vaddr: u64,
    pub memsz: u64,
    pub data: &'a [u8],
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Image<'a> {
    pub entry: u64,
    pub sections: Vec<Section<'a>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NfxError {
    /// No es un `.nfx` (firma o tamaño).
    NotNfx,
    /// Hecho para otra versión de las llamadas.
    Version,
    /// Cabecera o sección incoherente (fuera de zona, solapada, sin código, entrada mala…).
    Invalid,
    /// Pide más memoria de la permitida.
    TooBig,
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn u64_at(b: &[u8], i: usize) -> u64 {
    let mut t = [0u8; 8];
    t.copy_from_slice(&b[i..i + 8]);
    u64::from_le_bytes(t)
}

/// Comprueba y lee un `.nfx` entero.
pub fn parse(file: &[u8]) -> Result<Image<'_>, NfxError> {
    if file.len() < HEADER || &file[0..4] != MAGIC {
        return Err(NfxError::NotNfx);
    }
    if u16_at(file, 4) != ABI_VERSION {
        return Err(NfxError::Version);
    }
    let n = u16_at(file, 6) as usize;
    let entry = u64_at(file, 8);
    if n == 0 || n > MAX_SECTIONS || u64_at(file, 16) != 0 || file.len() < HEADER + n * SECTION {
        return Err(NfxError::Invalid);
    }
    let mut sections: Vec<Section> = Vec::with_capacity(n);
    let mut pages: Vec<(u64, u64)> = Vec::with_capacity(n);
    let mut total: u64 = 0;
    for i in 0..n {
        let o = HEADER + i * SECTION;
        let kind = Kind::from_u32(u32_at(file, o)).ok_or(NfxError::Invalid)?;
        let (vaddr, memsz, off, filesz) = (u64_at(file, o + 8), u64_at(file, o + 16), u64_at(file, o + 24), u64_at(file, o + 32));
        if u32_at(file, o + 4) != 0 || vaddr % PAGE != 0 || memsz == 0 || filesz > memsz || (kind == Kind::Bss && filesz != 0) {
            return Err(NfxError::Invalid);
        }
        let end = vaddr.checked_add(memsz).ok_or(NfxError::Invalid)?;
        if vaddr < USER_BASE || end > IMAGE_TOP {
            return Err(NfxError::Invalid);
        }
        let fend = off.checked_add(filesz).ok_or(NfxError::Invalid)?;
        if fend > file.len() as u64 {
            return Err(NfxError::Invalid);
        }
        let pend = end.div_ceil(PAGE) * PAGE;
        if pages.iter().any(|&(s, e)| vaddr < e && s < pend) {
            return Err(NfxError::Invalid);
        }
        pages.push((vaddr, pend));
        total += pend - vaddr;
        if total > MAX_MEM - STACK_SIZE {
            return Err(NfxError::TooBig);
        }
        sections.push(Section { kind, vaddr, memsz, data: &file[off as usize..fend as usize] });
    }
    if !sections.iter().any(|s| s.kind == Kind::Code && entry >= s.vaddr && entry < s.vaddr + s.memsz) {
        return Err(NfxError::Invalid);
    }
    Ok(Image { entry, sections })
}

/// Escribe un `.nfx` (lo usa `tools/nfpack`). No valida: pasa el resultado por `parse`.
pub fn encode(entry: u64, sections: &[Section]) -> Vec<u8> {
    let mut o = Vec::new();
    o.extend_from_slice(MAGIC);
    o.extend_from_slice(&ABI_VERSION.to_le_bytes());
    o.extend_from_slice(&(sections.len() as u16).to_le_bytes());
    o.extend_from_slice(&entry.to_le_bytes());
    o.extend_from_slice(&0u64.to_le_bytes());
    let mut off = (HEADER + sections.len() * SECTION) as u64;
    for s in sections {
        o.extend_from_slice(&(s.kind as u32).to_le_bytes());
        o.extend_from_slice(&0u32.to_le_bytes());
        o.extend_from_slice(&s.vaddr.to_le_bytes());
        o.extend_from_slice(&s.memsz.to_le_bytes());
        o.extend_from_slice(&(if s.data.is_empty() { 0 } else { off }).to_le_bytes());
        o.extend_from_slice(&(s.data.len() as u64).to_le_bytes());
        off += s.data.len() as u64;
    }
    for s in sections {
        o.extend_from_slice(s.data);
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: u64 = USER_BASE + 0x40_0000;

    fn sample() -> Vec<u8> {
        let code = [0x90u8; 100];
        let ro = [1u8; 10];
        let data = [2u8; 5];
        encode(
            B + 4,
            &[
                Section { kind: Kind::Code, vaddr: B, memsz: 100, data: &code },
                Section { kind: Kind::ReadOnly, vaddr: B + 0x1000, memsz: 10, data: &ro },
                Section { kind: Kind::Data, vaddr: B + 0x2000, memsz: 0x1800, data: &data },
                Section { kind: Kind::Bss, vaddr: B + 0x4000, memsz: 0x10, data: &[] },
            ],
        )
    }

    #[test]
    fn ida_y_vuelta() {
        let f = sample();
        let img = parse(&f).unwrap();
        assert_eq!(img.entry, B + 4);
        assert_eq!(img.sections.len(), 4);
        assert_eq!(img.sections[2].data, &[2u8; 5]);
        assert_eq!(img.sections[2].memsz, 0x1800);
        assert!(img.sections[3].data.is_empty());
    }

    /// Cambia el campo de 8 bytes de la sección `i` en la posición `field` (8 dirección, 16 tamaño,
    /// 24 posición, 32 tamaño en archivo).
    fn patch(f: &[u8], i: usize, field: usize, v: u64) -> Vec<u8> {
        let mut f = f.to_vec();
        let o = HEADER + i * SECTION + field;
        f[o..o + 8].copy_from_slice(&v.to_le_bytes());
        f
    }

    #[test]
    fn rechaza_lo_malo() {
        let f = sample();
        assert_eq!(parse(&f[..10]), Err(NfxError::NotNfx));
        assert_eq!(parse(b"MZ\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"), Err(NfxError::NotNfx));
        let mut v = f.clone();
        v[4] = 9;
        assert_eq!(parse(&v), Err(NfxError::Version));
        // Truncado: las secciones apuntan fuera del archivo.
        assert_eq!(parse(&f[..f.len() - 1]), Err(NfxError::Invalid));
        // Dirección del núcleo, sin alinear, más allá de la imagen, desbordando.
        assert_eq!(parse(&patch(&f, 0, 8, 0x10_0000)), Err(NfxError::Invalid));
        assert_eq!(parse(&patch(&f, 1, 8, B + 0x1001)), Err(NfxError::Invalid));
        assert_eq!(parse(&patch(&patch(&f, 3, 8, IMAGE_TOP - 0x1000), 3, 16, 0x1001)), Err(NfxError::Invalid));
        assert_eq!(parse(&patch(&f, 3, 16, u64::MAX - 0x100)), Err(NfxError::Invalid));
        // Datos fuera del archivo, desbordando, o más datos que memoria.
        assert_eq!(parse(&patch(&f, 0, 24, f.len() as u64)), Err(NfxError::Invalid));
        assert_eq!(parse(&patch(&f, 0, 24, u64::MAX)), Err(NfxError::Invalid));
        assert_eq!(parse(&patch(&f, 1, 32, 11)), Err(NfxError::Invalid));
        // Solapes: misma página que otra sección (aunque los bytes no se pisen).
        assert_eq!(parse(&patch(&f, 1, 8, B)), Err(NfxError::Invalid));
        assert_eq!(parse(&patch(&f, 3, 8, B + 0x3000)), Err(NfxError::Invalid));
        // Entrada fuera del código (en datos) o fuera de todo.
        let mut v = f.clone();
        v[8..16].copy_from_slice(&(B + 0x2000).to_le_bytes());
        assert_eq!(parse(&v), Err(NfxError::Invalid));
        v[8..16].copy_from_slice(&(B + 100).to_le_bytes());
        assert_eq!(parse(&v), Err(NfxError::Invalid));
        // Tipo inventado, bss con datos, sin secciones, demasiadas.
        let mut v = f.clone();
        v[HEADER] = 7;
        assert_eq!(parse(&v), Err(NfxError::Invalid));
        assert_eq!(parse(&patch(&f, 3, 32, 1)), Err(NfxError::Invalid));
        let mut v = f.clone();
        v[6] = 0;
        assert_eq!(parse(&v), Err(NfxError::Invalid));
        v[6] = 17;
        assert_eq!(parse(&v), Err(NfxError::Invalid));
        // Demasiada memoria.
        assert_eq!(parse(&patch(&f, 3, 16, MAX_MEM)), Err(NfxError::TooBig));
    }

    #[test]
    fn sin_codigo_no_vale() {
        let d = [0u8; 4];
        let f = encode(B, &[Section { kind: Kind::Data, vaddr: B, memsz: 4, data: &d }]);
        assert_eq!(parse(&f), Err(NfxError::Invalid));
    }
}
