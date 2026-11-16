//! NFFS: sistema de archivos propio de NightFury. CONTRATO: `FsError`, `Entry` y las firmas
//! públicas de `Nffs` no cambian.
//!
//! Las rutas de `Nffs` son relativas a la raíz del volumen, con `\` como separador y sin letra
//! de unidad: "" es la raíz, "Documentos\\nota.txt" un archivo. Los nombres no distinguen
//! mayúsculas pero las conservan.
//!
//! # Formato en disco (diseño propio, versión 1; enteros en little endian)
//! - Bloques de 4096 bytes. Los bloques 0 y 1 son las dos copias del SUPERBLOQUE:
//!   `"NFFS"`, versión u32, generación u64, bloques totales u64, máximo de nodos u32,
//!   nº de bloques de metadatos u32, primer bloque de metadatos u64, longitud de metadatos u32,
//!   (4 bytes de relleno), suma de los metadatos u64 y suma del propio superbloque u64 (offset 56).
//!   La suma es FNV-1a de 64 bits. Al montar vale la copia válida de mayor generación.
//! - METADATOS = mapa de bloques (1 bit por bloque, 1 = usado) + tabla de nodos. Es una cadena de
//!   bloques de datos: cada uno empieza con el número del siguiente (u64, 0 = último) y el
//!   resto es contenido. La tabla de nodos: nº de nodos u32 y, por nodo, número u32, tipo u8
//!   (1 = carpeta), fecha (año u16, mes, día, hora, minuto, segundo), tamaño u64 y después
//!   una lista: en carpetas, entradas (nº de nodo u32, longitud u16, nombre UTF-8); en archivos,
//!   extensiones (bloque inicial u64, longitud en bloques u32). El nodo 0 es la raíz. No hay
//!   bloques de extensiones adicionales: la lista de un nodo es tan larga como haga falta.
//! - Los datos de los archivos ocupan extensiones de bloques contiguos, fuera de los metadatos.
//!
//! # Atomicidad
//! Nada se sobrescribe en sitio salvo los dos superbloques, y estos de forma alterna. Cada
//! operación (1) reserva bloques que el estado confirmado considera libres para los datos y la
//! nueva cadena de metadatos, (2) los escribe y hace `flush`, y (3) confirma con UNA sola
//! escritura de bloque: el superbloque de la otra copia con generación mayor, seguida de `flush`.
//! Los bloques que la operación libera (datos viejos, metadatos viejos) no se reutilizan hasta
//! la operación siguiente. Un corte antes de (3) deja el estado anterior; después, el nuevo.
//!
//! Se mantiene una reserva de bloques libres para poder borrar siempre aunque el disco esté lleno.
//!
//! shortcut: los metadatos enteros se reescriben en cada operación (coste proporcional al número
//! de nodos y a la fragmentación), los archivos se leen y escriben enteros en memoria y el
//! máximo de nodos se fija al formatear (bloques/2, entre 64 y 65 536); cambiar a metadatos
//! por bloques con copia al escribir si hay discos con decenas de miles de archivos.
use crate::block::{BlockDev, DiskError, BLOCK};
use crate::files::valid_name;
use crate::platform::DateTime;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FsError {
    NotFound,
    Exists,
    NotDir,
    IsDir,
    NotEmpty,
    NoSpace,
    /// Nombre vacío, demasiado largo o con caracteres no permitidos.
    BadName,
    /// Ruta mal formada o unidad desconocida.
    BadPath,
    /// Fallo del disco.
    Disk,
    /// El disco no tiene un NFFS válido.
    Corrupt,
    /// Operación no permitida (p. ej. mover una carpeta dentro de sí misma).
    Invalid,
}

impl FsError {
    /// Mensaje para el usuario.
    pub fn message(&self) -> &'static str {
        match self {
            FsError::NotFound => "No se encuentra el archivo o la carpeta.",
            FsError::Exists => "Ya existe un elemento con ese nombre.",
            FsError::NotDir => "No es una carpeta.",
            FsError::IsDir => "Es una carpeta, no un archivo.",
            FsError::NotEmpty => "La carpeta no está vacía.",
            FsError::NoSpace => "No queda espacio en el disco.",
            FsError::BadName => "El nombre no es válido.",
            FsError::BadPath => "La ruta no es válida.",
            FsError::Disk => "Error de disco.",
            FsError::Corrupt => "El disco está dañado o no tiene formato.",
            FsError::Invalid => "La operación no se puede hacer.",
        }
    }
}

impl From<DiskError> for FsError {
    fn from(_: DiskError) -> FsError {
        FsError::Disk
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    /// Tamaño en bytes (0 en carpetas).
    pub size: u64,
    pub modified: DateTime,
}

const MAGIC: &[u8; 4] = b"NFFS";
const VERSION: u32 = 1;
/// Mínimo de bloques para formatear.
const MIN_BLOCKS: u64 = 64;
/// Bytes útiles de un bloque de metadatos (los 8 primeros son el enlace al siguiente).
const PAYLOAD: usize = BLOCK - 8;
/// Primer bloque que puede tener datos o metadatos (0 y 1 son los superbloques).
const FIRST: u64 = 2;
/// Bloques libres que se dejan siempre de más para poder confirmar un borrado.
const RESERVE: u64 = 2;
const FNV0: u64 = 0xcbf2_9ce4_8422_2325;

fn fnv(data: &[u8], mut h: u64) -> u64 {
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn bit(map: &[u8], i: u64) -> bool {
    map[(i / 8) as usize] >> (i % 8) & 1 == 1
}

fn set_bit(map: &mut [u8], i: u64, v: bool) {
    let m = 1u8 << (i % 8);
    let b = &mut map[(i / 8) as usize];
    if v {
        *b |= m
    } else {
        *b &= !m
    }
}

/// Comparación de nombres sin distinguir mayúsculas.
pub(crate) fn same(a: &str, b: &str) -> bool {
    a.chars().flat_map(char::to_lowercase).eq(b.chars().flat_map(char::to_lowercase))
}

fn lower(a: &str) -> String {
    a.chars().flat_map(char::to_lowercase).collect()
}

#[derive(Clone)]
struct Node {
    dir: bool,
    mtime: DateTime,
    size: u64,
    /// Archivos: (bloque inicial, nº de bloques).
    extents: Vec<(u64, u32)>,
    /// Carpetas: (nombre, nº de nodo).
    entries: Vec<(String, u32)>,
}

#[derive(Clone, Copy)]
struct Sb {
    gen: u64,
    total: u64,
    max_nodes: u32,
    meta_count: u32,
    meta_first: u64,
    meta_len: u32,
    meta_sum: u64,
}

impl Sb {
    fn encode(&self) -> [u8; BLOCK] {
        let mut b = [0u8; BLOCK];
        b[0..4].copy_from_slice(MAGIC);
        b[4..8].copy_from_slice(&VERSION.to_le_bytes());
        b[8..16].copy_from_slice(&self.gen.to_le_bytes());
        b[16..24].copy_from_slice(&self.total.to_le_bytes());
        b[24..28].copy_from_slice(&self.max_nodes.to_le_bytes());
        b[28..32].copy_from_slice(&self.meta_count.to_le_bytes());
        b[32..40].copy_from_slice(&self.meta_first.to_le_bytes());
        b[40..44].copy_from_slice(&self.meta_len.to_le_bytes());
        b[48..56].copy_from_slice(&self.meta_sum.to_le_bytes());
        let s = fnv(&b[..56], FNV0);
        b[56..64].copy_from_slice(&s.to_le_bytes());
        b
    }

    fn decode(b: &[u8]) -> Option<Sb> {
        if b.len() != BLOCK || &b[0..4] != MAGIC {
            return None;
        }
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        if u32_at(4) != VERSION || u64_at(56) != fnv(&b[..56], FNV0) {
            return None;
        }
        let sb = Sb {
            gen: u64_at(8),
            total: u64_at(16),
            max_nodes: u32_at(24),
            meta_count: u32_at(28),
            meta_first: u64_at(32),
            meta_len: u32_at(40),
            meta_sum: u64_at(48),
        };
        let ok = sb.total >= MIN_BLOCKS
            && sb.max_nodes >= 1
            && sb.meta_count >= 1
            && (sb.meta_count as u64) < sb.total
            && sb.meta_first >= FIRST
            && sb.meta_first < sb.total;
        ok.then_some(sb)
    }
}

/// Lector con límites: nunca entra en pánico con datos cortos.
struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let e = self.p.checked_add(n)?;
        let s = self.b.get(self.p..e)?;
        self.p = e;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}

fn ser_nodes(nodes: &BTreeMap<u32, Node>) -> Vec<u8> {
    let mut o = Vec::new();
    o.extend_from_slice(&(nodes.len() as u32).to_le_bytes());
    for (id, n) in nodes {
        o.extend_from_slice(&id.to_le_bytes());
        o.push(n.dir as u8);
        o.extend_from_slice(&n.mtime.year.to_le_bytes());
        o.extend_from_slice(&[n.mtime.month, n.mtime.day, n.mtime.hour, n.mtime.minute, n.mtime.second]);
        o.extend_from_slice(&n.size.to_le_bytes());
        if n.dir {
            o.extend_from_slice(&(n.entries.len() as u32).to_le_bytes());
            for (name, child) in &n.entries {
                o.extend_from_slice(&child.to_le_bytes());
                o.extend_from_slice(&(name.len() as u16).to_le_bytes());
                o.extend_from_slice(name.as_bytes());
            }
        } else {
            o.extend_from_slice(&(n.extents.len() as u32).to_le_bytes());
            for (s, l) in &n.extents {
                o.extend_from_slice(&s.to_le_bytes());
                o.extend_from_slice(&l.to_le_bytes());
            }
        }
    }
    o
}

fn parse_nodes(b: &[u8]) -> Option<BTreeMap<u32, Node>> {
    let mut r = Rd { b, p: 0 };
    let count = r.u32()?;
    let mut nodes = BTreeMap::new();
    let mut last: Option<u32> = None;
    for _ in 0..count {
        let id = r.u32()?;
        if last.is_some_and(|l| id <= l) {
            return None;
        }
        last = Some(id);
        let flags = r.u8()?;
        if flags > 1 {
            return None;
        }
        let mtime = DateTime {
            year: r.u16()?,
            month: r.u8()?,
            day: r.u8()?,
            hour: r.u8()?,
            minute: r.u8()?,
            second: r.u8()?,
        };
        let size = r.u64()?;
        let n = r.u32()?;
        let mut node = Node { dir: flags == 1, mtime, size, extents: Vec::new(), entries: Vec::new() };
        for _ in 0..n {
            if node.dir {
                let child = r.u32()?;
                let len = r.u16()? as usize;
                let name = String::from_utf8(r.take(len)?.to_vec()).ok()?;
                node.entries.push((name, child));
            } else {
                node.extents.push((r.u64()?, r.u32()?));
            }
        }
        nodes.insert(id, node);
    }
    (r.p == b.len()).then_some(nodes)
}

fn blocks_for(size: u64) -> u64 {
    size.div_ceil(BLOCK as u64)
}

/// Comprobación completa de coherencia. Devuelve el nº de bloques libres.
fn verify(
    total: u64,
    max_nodes: u32,
    free: &[u8],
    nodes: &BTreeMap<u32, Node>,
    meta: &[u64],
) -> Result<u64, FsError> {
    let bad = FsError::Corrupt;
    if free.len() != total.div_ceil(8) as usize || nodes.len() as u64 > max_nodes as u64 {
        return Err(bad);
    }
    let mut seen = vec![0u8; free.len()];
    set_bit(&mut seen, 0, true);
    set_bit(&mut seen, 1, true);
    for &b in meta {
        if b < FIRST || b >= total || bit(&seen, b) {
            return Err(bad);
        }
        set_bit(&mut seen, b, true);
    }
    if !nodes.get(&0).is_some_and(|n| n.dir) {
        return Err(bad);
    }
    let mut visited = BTreeSet::new();
    visited.insert(0u32);
    let mut stack = vec![0u32];
    while let Some(id) = stack.pop() {
        let n = nodes.get(&id).ok_or(bad)?;
        if n.dir {
            if n.size != 0 || !n.extents.is_empty() {
                return Err(bad);
            }
            let mut names: Vec<String> = Vec::with_capacity(n.entries.len());
            for (name, child) in &n.entries {
                if !valid_name(name) || !nodes.contains_key(child) || !visited.insert(*child) {
                    return Err(bad);
                }
                names.push(lower(name));
                stack.push(*child);
            }
            names.sort_unstable();
            if names.windows(2).any(|w| w[0] == w[1]) {
                return Err(bad);
            }
        } else {
            if !n.entries.is_empty() {
                return Err(bad);
            }
            let mut sum = 0u64;
            for &(s, l) in &n.extents {
                let end = s.checked_add(l as u64).ok_or(bad)?;
                if l == 0 || s < FIRST || end > total {
                    return Err(bad);
                }
                for b in s..end {
                    if bit(&seen, b) {
                        return Err(bad);
                    }
                    set_bit(&mut seen, b, true);
                }
                sum += l as u64;
            }
            if sum != blocks_for(n.size) {
                return Err(bad);
            }
        }
    }
    if visited.len() != nodes.len() || seen != free {
        return Err(bad);
    }
    let used: u64 = seen.iter().map(|b| b.count_ones() as u64).sum();
    Ok(total - used)
}

/// Cambios pendientes de una operación, para poder deshacerlos en memoria si falla.
struct Tx {
    free: Vec<u8>,
    free_count: u64,
    undo: Vec<(u32, Option<Node>)>,
    /// Extensiones que quedan libres al confirmar.
    release: Vec<(u64, u32)>,
}

pub struct Nffs {
    dev: Box<dyn BlockDev>,
    /// Superbloque confirmado y copia (0 o 1) donde está.
    sb: Sb,
    slot: u64,
    /// Mapa de bloques usados confirmado (1 = usado).
    free: Vec<u8>,
    free_count: u64,
    nodes: BTreeMap<u32, Node>,
    /// Bloques de la cadena de metadatos confirmada.
    meta: Vec<u64>,
}

fn zero_block(dev: &mut dyn BlockDev, n: u64) -> bool {
    let mut b = [1u8; BLOCK];
    dev.read(n, &mut b).is_ok() && b.iter().all(|&x| x == 0)
}

fn read_sb(dev: &mut dyn BlockDev, slot: u64) -> Option<Sb> {
    let mut b = [0u8; BLOCK];
    dev.read(slot, &mut b).ok()?;
    Sb::decode(&b).filter(|sb| sb.total <= dev.blocks())
}

impl Nffs {
    /// ¿Tiene `dev` un NFFS (firma y superbloque válidos)?
    pub fn is_formatted(dev: &mut dyn BlockDev) -> bool {
        dev.blocks() >= MIN_BLOCKS && (read_sb(dev, 0).is_some() || read_sb(dev, 1).is_some())
    }

    /// ¿Está `dev` en blanco? (primeros y últimos 256 bloques a cero). Solo un disco en blanco
    /// puede formatearse; uno con cualquier otro contenido se ignora siempre.
    pub fn is_blank(dev: &mut dyn BlockDev) -> bool {
        let n = dev.blocks();
        if n < 512 {
            (0..n).all(|i| zero_block(dev, i))
        } else {
            (0..256).all(|i| zero_block(dev, i)) && (n - 256..n).all(|i| zero_block(dev, i))
        }
    }

    /// Crea un NFFS vacío en `dev` y lo monta.
    pub fn format(dev: Box<dyn BlockDev>, now: DateTime) -> Result<Nffs, FsError> {
        let total = dev.blocks();
        if total < MIN_BLOCKS {
            return Err(FsError::NoSpace);
        }
        let mut free = vec![0u8; total.div_ceil(8) as usize];
        set_bit(&mut free, 0, true);
        set_bit(&mut free, 1, true);
        let mut nodes = BTreeMap::new();
        nodes.insert(0, Node { dir: true, mtime: now, size: 0, extents: vec![], entries: vec![] });
        let sb = Sb {
            gen: 0,
            total,
            max_nodes: (total / 2).clamp(64, 65_536) as u32,
            meta_count: 0,
            meta_first: 0,
            meta_len: 0,
            meta_sum: 0,
        };
        let mut fs = Nffs { dev, sb, slot: 0, free, free_count: total - 2, nodes, meta: vec![] };
        let tx = Tx { free: vec![], free_count: 0, undo: vec![], release: vec![] };
        fs.commit(&tx, None, true)?;
        // Segunda copia del superbloque (la primera quedó en la copia 1).
        let b = fs.sb.encode();
        fs.dev.write(0, &b)?;
        fs.dev.flush()?;
        Ok(fs)
    }

    /// Monta un NFFS existente.
    pub fn mount(mut dev: Box<dyn BlockDev>) -> Result<Nffs, FsError> {
        if dev.blocks() < MIN_BLOCKS {
            return Err(FsError::Corrupt);
        }
        let mut cands: Vec<(u64, Sb)> = (0..2).filter_map(|s| read_sb(&mut *dev, s).map(|sb| (s, sb))).collect();
        // Primero la generación mayor; si no carga (copia a medias), la otra.
        cands.sort_by(|a, b| b.1.gen.cmp(&a.1.gen));
        for (slot, sb) in cands {
            if let Ok((free, nodes, meta, free_count)) = Nffs::load(&mut *dev, &sb) {
                return Ok(Nffs { dev, sb, slot, free, free_count, nodes, meta });
            }
        }
        Err(FsError::Corrupt)
    }

    #[allow(clippy::type_complexity)]
    fn load(dev: &mut dyn BlockDev, sb: &Sb) -> Result<(Vec<u8>, BTreeMap<u32, Node>, Vec<u64>, u64), FsError> {
        let bad = FsError::Corrupt;
        let count = sb.meta_count as usize;
        let len = sb.meta_len as usize;
        let bm = sb.total.div_ceil(8) as usize;
        if len > count * PAYLOAD || len <= (count - 1) * PAYLOAD || len < bm + 4 {
            return Err(bad);
        }
        let mut meta = Vec::with_capacity(len);
        let mut blocks = Vec::with_capacity(count);
        let mut b = sb.meta_first;
        let mut buf = [0u8; BLOCK];
        for _ in 0..count {
            if b < FIRST || b >= sb.total {
                return Err(bad);
            }
            dev.read(b, &mut buf)?;
            blocks.push(b);
            meta.extend_from_slice(&buf[8..]);
            b = u64::from_le_bytes(buf[..8].try_into().unwrap());
        }
        if b != 0 {
            return Err(bad);
        }
        meta.truncate(len);
        if fnv(&meta, FNV0) != sb.meta_sum {
            return Err(bad);
        }
        let nodes = parse_nodes(&meta[bm..]).ok_or(bad)?;
        let free = meta[..bm].to_vec();
        let free_count = verify(sb.total, sb.max_nodes, &free, &nodes, &blocks)?;
        Ok((free, nodes, blocks, free_count))
    }

    // ---- consulta ----

    fn get(&self, id: u32) -> Result<&Node, FsError> {
        self.nodes.get(&id).ok_or(FsError::Corrupt)
    }

    fn find(&self, dir: u32, name: &str) -> Result<Option<usize>, FsError> {
        Ok(self.get(dir)?.entries.iter().position(|(n, _)| same(n, name)))
    }

    fn comps(path: &str) -> Result<Vec<&str>, FsError> {
        let v: Vec<&str> = path.split('\\').filter(|c| !c.is_empty()).collect();
        if v.iter().all(|c| valid_name(c)) {
            Ok(v)
        } else {
            Err(FsError::BadName)
        }
    }

    fn lookup(&self, path: &str) -> Result<u32, FsError> {
        let mut id = 0;
        for c in Self::comps(path)? {
            let n = self.get(id)?;
            if !n.dir {
                return Err(FsError::NotDir);
            }
            id = n.entries.iter().find(|(e, _)| same(e, c)).ok_or(FsError::NotFound)?.1;
        }
        Ok(id)
    }

    /// (carpeta que contiene, nombre final, carpetas recorridas incluida esa).
    fn split_parent(&self, path: &str) -> Result<(u32, String, Vec<u32>), FsError> {
        let mut cs = Self::comps(path)?;
        let name = cs.pop().ok_or(FsError::Invalid)?;
        let mut id = 0;
        let mut chain = vec![0];
        for c in cs {
            let n = self.get(id)?;
            if !n.dir {
                return Err(FsError::NotDir);
            }
            id = n.entries.iter().find(|(e, _)| same(e, c)).ok_or(FsError::NotFound)?.1;
            chain.push(id);
        }
        if !self.get(id)?.dir {
            return Err(FsError::NotDir);
        }
        Ok((id, String::from(name), chain))
    }

    fn entry(&self, name: &str, id: u32) -> Result<Entry, FsError> {
        let n = self.get(id)?;
        Ok(Entry { name: String::from(name), dir: n.dir, size: if n.dir { 0 } else { n.size }, modified: n.mtime })
    }

    /// Entradas de una carpeta, sin orden garantizado.
    pub fn list(&mut self, path: &str) -> Result<Vec<Entry>, FsError> {
        let id = self.lookup(path)?;
        let n = self.get(id)?;
        if !n.dir {
            return Err(FsError::NotDir);
        }
        n.entries.iter().map(|(name, c)| self.entry(name, *c)).collect()
    }

    pub fn stat(&mut self, path: &str) -> Result<Entry, FsError> {
        if Self::comps(path)?.is_empty() {
            return self.entry("", 0);
        }
        let (p, name, _) = self.split_parent(path)?;
        let i = self.find(p, &name)?.ok_or(FsError::NotFound)?;
        let (stored, id) = &self.get(p)?.entries[i];
        self.entry(stored, *id)
    }

    /// Contenido completo de un archivo.
    pub fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        let id = self.lookup(path)?;
        let n = self.get(id)?;
        if n.dir {
            return Err(FsError::IsDir);
        }
        let (size, extents) = (n.size as usize, n.extents.clone());
        let mut out = Vec::with_capacity(size);
        let mut buf = [0u8; BLOCK];
        for (s, l) in extents {
            for b in s..s + l as u64 {
                self.dev.read(b, &mut buf)?;
                let take = (size - out.len()).min(BLOCK);
                out.extend_from_slice(&buf[..take]);
            }
        }
        Ok(out)
    }

    // ---- modificación ----

    fn begin(&self) -> Tx {
        Tx { free: self.free.clone(), free_count: self.free_count, undo: vec![], release: vec![] }
    }

    fn rollback(&mut self, tx: Tx) {
        for (id, old) in tx.undo.into_iter().rev() {
            match old {
                Some(n) => self.nodes.insert(id, n),
                None => self.nodes.remove(&id),
            };
        }
        self.free = tx.free;
        self.free_count = tx.free_count;
    }

    /// Guarda el estado previo de un nodo antes de tocarlo.
    fn touch(&self, tx: &mut Tx, id: u32) {
        if !tx.undo.iter().any(|(i, _)| *i == id) {
            tx.undo.push((id, self.nodes.get(&id).cloned()));
        }
    }

    fn node_mut(&mut self, tx: &mut Tx, id: u32) -> Result<&mut Node, FsError> {
        self.touch(tx, id);
        self.nodes.get_mut(&id).ok_or(FsError::Corrupt)
    }

    fn new_node(&mut self, tx: &mut Tx, node: Node) -> Result<u32, FsError> {
        if self.nodes.len() as u64 >= self.sb.max_nodes as u64 {
            return Err(FsError::NoSpace);
        }
        let mut id = 1;
        for &k in self.nodes.keys().skip(1) {
            if k != id {
                break;
            }
            id += 1;
        }
        self.touch(tx, id);
        self.nodes.insert(id, node);
        Ok(id)
    }

    /// Reserva `count` bloques libres, en las extensiones que haga falta (primer hueco).
    fn alloc(&mut self, count: u64) -> Result<Vec<(u64, u32)>, FsError> {
        if count > self.free_count {
            return Err(FsError::NoSpace);
        }
        let total = self.sb.total;
        let mut out = Vec::new();
        let mut need = count;
        let mut i = FIRST;
        while need > 0 && i < total {
            if i % 8 == 0 && self.free[(i / 8) as usize] == 0xff {
                i += 8;
                continue;
            }
            if bit(&self.free, i) {
                i += 1;
                continue;
            }
            let start = i;
            let mut len = 0u64;
            while i < total && need > 0 && len < u32::MAX as u64 && !bit(&self.free, i) {
                set_bit(&mut self.free, i, true);
                i += 1;
                len += 1;
                need -= 1;
            }
            out.push((start, len as u32));
        }
        if need > 0 {
            return Err(FsError::NoSpace);
        }
        self.free_count -= count;
        Ok(out)
    }

    fn free_range(&mut self, s: u64, l: u64) {
        for b in s..s + l {
            set_bit(&mut self.free, b, false);
        }
        self.free_count += l;
    }

    /// Termina una operación: si falla, deshace los cambios en memoria.
    fn finish(&mut self, tx: Tx, payload: Option<(&[(u64, u32)], &[u8])>, grow: bool) -> Result<(), FsError> {
        let r = self.commit(&tx, payload, grow);
        if r.is_err() {
            self.rollback(tx);
        }
        r
    }

    /// Escribe datos y metadatos nuevos y confirma con la escritura del superbloque.
    fn commit(&mut self, tx: &Tx, payload: Option<(&[(u64, u32)], &[u8])>, grow: bool) -> Result<(), FsError> {
        let nodes = ser_nodes(&self.nodes);
        let meta_len = self.free.len() + nodes.len();
        let n = meta_len.div_ceil(PAYLOAD) as u64;
        if self.free_count < n + if grow { RESERVE } else { 0 } {
            return Err(FsError::NoSpace);
        }
        let blocks: Vec<u64> = self.alloc(n)?.iter().flat_map(|&(s, l)| s..s + l as u64).collect();
        if let Some((ext, data)) = payload {
            let mut buf = [0u8; BLOCK];
            let mut off = 0;
            for &(s, l) in ext {
                for b in s..s + l as u64 {
                    let end = (off + BLOCK).min(data.len());
                    buf.fill(0);
                    buf[..end - off].copy_from_slice(&data[off..end]);
                    off = end;
                    self.dev.write(b, &buf)?;
                }
            }
        }
        for &(s, l) in &tx.release {
            self.free_range(s, l as u64);
        }
        for b in self.meta.clone() {
            self.free_range(b, 1);
        }
        let mut meta = Vec::with_capacity(meta_len);
        meta.extend_from_slice(&self.free);
        meta.extend_from_slice(&nodes);
        for (k, &b) in blocks.iter().enumerate() {
            let mut buf = [0u8; BLOCK];
            let next = blocks.get(k + 1).copied().unwrap_or(0);
            buf[..8].copy_from_slice(&next.to_le_bytes());
            let chunk = &meta[k * PAYLOAD..((k + 1) * PAYLOAD).min(meta.len())];
            buf[8..8 + chunk.len()].copy_from_slice(chunk);
            self.dev.write(b, &buf)?;
        }
        self.dev.flush()?;
        let sb = Sb {
            gen: self.sb.gen + 1,
            meta_count: n as u32,
            meta_first: blocks[0],
            meta_len: meta_len as u32,
            meta_sum: fnv(&meta, FNV0),
            ..self.sb
        };
        let slot = 1 - self.slot;
        self.dev.write(slot, &sb.encode())?;
        self.dev.flush()?;
        self.sb = sb;
        self.slot = slot;
        self.meta = blocks;
        Ok(())
    }

    /// Crea el archivo o sustituye su contenido de forma atómica: tras un corte queda el
    /// contenido viejo o el nuevo, nunca una mezcla.
    pub fn write(&mut self, path: &str, data: &[u8], now: DateTime) -> Result<(), FsError> {
        let (parent, name, _) = self.split_parent(path)?;
        let existing = match self.find(parent, &name)? {
            Some(i) => Some(self.get(parent)?.entries[i].1),
            None => None,
        };
        if let Some(id) = existing {
            if self.get(id)?.dir {
                return Err(FsError::IsDir);
            }
        }
        let mut tx = self.begin();
        let r = (|| {
            let ext = if data.is_empty() { vec![] } else { self.alloc(blocks_for(data.len() as u64))? };
            match existing {
                Some(id) => {
                    let n = self.node_mut(&mut tx, id)?;
                    let old = core::mem::replace(&mut n.extents, ext.clone());
                    tx.release.extend(old);
                    n.size = data.len() as u64;
                    n.mtime = now;
                }
                None => {
                    let node = Node { dir: false, mtime: now, size: data.len() as u64, extents: ext.clone(), entries: vec![] };
                    let id = self.new_node(&mut tx, node)?;
                    self.node_mut(&mut tx, parent)?.entries.push((name.clone(), id));
                }
            }
            Ok(ext)
        })();
        match r {
            Ok(ext) => self.finish(tx, Some((&ext, data)), true),
            Err(e) => {
                self.rollback(tx);
                Err(e)
            }
        }
    }

    pub fn mkdir(&mut self, path: &str, now: DateTime) -> Result<(), FsError> {
        let (parent, name, _) = self.split_parent(path)?;
        if self.find(parent, &name)?.is_some() {
            return Err(FsError::Exists);
        }
        let mut tx = self.begin();
        let r = (|| {
            let node = Node { dir: true, mtime: now, size: 0, extents: vec![], entries: vec![] };
            let id = self.new_node(&mut tx, node)?;
            self.node_mut(&mut tx, parent)?.entries.push((name.clone(), id));
            Ok(())
        })();
        match r {
            Ok(()) => self.finish(tx, None, true),
            Err(e) => {
                self.rollback(tx);
                Err(e)
            }
        }
    }

    /// Borra un archivo o una carpeta vacía.
    pub fn remove(&mut self, path: &str) -> Result<(), FsError> {
        let (parent, name, _) = self.split_parent(path)?;
        let idx = self.find(parent, &name)?.ok_or(FsError::NotFound)?;
        let id = self.get(parent)?.entries[idx].1;
        let node = self.get(id)?;
        if node.dir && !node.entries.is_empty() {
            return Err(FsError::NotEmpty);
        }
        let mut tx = self.begin();
        self.node_mut(&mut tx, parent)?.entries.remove(idx);
        self.touch(&mut tx, id);
        if let Some(n) = self.nodes.remove(&id) {
            tx.release.extend(n.extents);
        }
        self.finish(tx, None, false)
    }

    /// Mueve o renombra dentro del volumen. Falla con `Exists` si el destino existe.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        let (sp, sname, _) = self.split_parent(from)?;
        let sidx = self.find(sp, &sname)?.ok_or(FsError::NotFound)?;
        let sid = self.get(sp)?.entries[sidx].1;
        let (dp, dname, chain) = self.split_parent(to)?;
        if self.get(sid)?.dir && chain.contains(&sid) {
            return Err(FsError::Invalid);
        }
        let didx = self.find(dp, &dname)?;
        let in_place = dp == sp && didx == Some(sidx);
        if didx.is_some() && !in_place {
            return Err(FsError::Exists);
        }
        if in_place && self.get(sp)?.entries[sidx].0 == dname {
            return Ok(());
        }
        let mut tx = self.begin();
        let entry = if in_place {
            let e = &mut self.node_mut(&mut tx, sp)?.entries[sidx];
            e.0 = dname;
            None
        } else {
            let e = self.node_mut(&mut tx, sp)?.entries.remove(sidx);
            Some((dname, e.1))
        };
        if let Some(e) = entry {
            self.node_mut(&mut tx, dp)?.entries.push(e);
        }
        self.finish(tx, None, false)
    }

    /// (bytes totales, bytes libres). Los libres descuentan la reserva (los bloques de metadatos
    /// y dos más) que permite confirmar siempre un borrado aunque el disco esté lleno.
    pub fn space(&mut self) -> (u64, u64) {
        let usable = self.free_count.saturating_sub(self.meta.len() as u64 + RESERVE);
        (self.sb.total * BLOCK as u64, usable * BLOCK as u64)
    }

    /// Comprueba la coherencia del volumen entero.
    pub fn check(&mut self) -> Result<(), FsError> {
        let free = verify(self.sb.total, self.sb.max_nodes, &self.free, &self.nodes, &self.meta)?;
        if free != self.free_count {
            return Err(FsError::Corrupt);
        }
        Ok(())
    }

    /// Devuelve el dispositivo (para inspeccionarlo en pruebas y herramientas).
    pub fn into_dev(self) -> Box<dyn BlockDev> {
        self.dev
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::RamDisk;
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap as Map;
    use std::rc::Rc;

    fn t() -> DateTime {
        DateTime { year: 2026, month: 11, day: 16, hour: 10, minute: 30, second: 15 }
    }

    /// Generador pseudoaleatorio propio (xorshift64*), de semilla fija.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
        fn bytes(&mut self, n: usize) -> Vec<u8> {
            (0..n).map(|_| self.next() as u8).collect()
        }
    }

    /// Disco compartido con cuenta de lecturas/escrituras y corte de luz simulado: a partir de
    /// la escritura número `limit` (empezando en 0) las escrituras se descartan.
    #[derive(Clone)]
    struct Shared {
        data: Rc<RefCell<Vec<u8>>>,
        writes: Rc<Cell<usize>>,
        reads: Rc<Cell<usize>>,
        limit: Rc<Cell<usize>>,
    }
    impl Shared {
        fn new(blocks: u64) -> Shared {
            Shared::from_bytes(vec![0; blocks as usize * BLOCK])
        }
        fn from_bytes(v: Vec<u8>) -> Shared {
            Shared {
                data: Rc::new(RefCell::new(v)),
                writes: Rc::new(Cell::new(0)),
                reads: Rc::new(Cell::new(0)),
                limit: Rc::new(Cell::new(usize::MAX)),
            }
        }
        fn bytes(&self) -> Vec<u8> {
            self.data.borrow().clone()
        }
    }
    impl BlockDev for Shared {
        fn blocks(&self) -> u64 {
            (self.data.borrow().len() / BLOCK) as u64
        }
        fn read(&mut self, n: u64, buf: &mut [u8]) -> Result<(), DiskError> {
            if n >= self.blocks() || buf.len() != BLOCK {
                return Err(DiskError::Range);
            }
            self.reads.set(self.reads.get() + 1);
            let o = n as usize * BLOCK;
            buf.copy_from_slice(&self.data.borrow()[o..o + BLOCK]);
            Ok(())
        }
        fn write(&mut self, n: u64, buf: &[u8]) -> Result<(), DiskError> {
            if n >= self.blocks() || buf.len() != BLOCK {
                return Err(DiskError::Range);
            }
            let w = self.writes.get();
            self.writes.set(w + 1);
            if w < self.limit.get() {
                let o = n as usize * BLOCK;
                self.data.borrow_mut()[o..o + BLOCK].copy_from_slice(buf);
            }
            Ok(())
        }
    }

    fn fresh(blocks: u64) -> (Shared, Nffs) {
        let sh = Shared::new(blocks);
        let fs = Nffs::format(Box::new(sh.clone()), t()).unwrap();
        (sh, fs)
    }

    fn remount(fs: Nffs) -> Nffs {
        Nffs::mount(fs.into_dev()).unwrap()
    }

    fn dump(fs: &mut Nffs) -> Map<String, Option<Vec<u8>>> {
        let mut out = Map::new();
        let mut work = vec![String::new()];
        while let Some(d) = work.pop() {
            for e in fs.list(&d).unwrap() {
                let p = if d.is_empty() { e.name.clone() } else { format!("{d}\\{}", e.name) };
                if e.dir {
                    out.insert(p.clone(), None);
                    work.push(p);
                } else {
                    let data = fs.read(&p).unwrap();
                    assert_eq!(data.len() as u64, e.size);
                    out.insert(p, Some(data));
                }
            }
        }
        out
    }

    fn pattern(n: usize, seed: u8) -> Vec<u8> {
        (0..n).map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed)).collect()
    }

    #[test]
    fn format_and_mount() {
        let (_, mut fs) = fresh(256);
        assert!(fs.list("").unwrap().is_empty());
        fs.check().unwrap();
        let (total, free) = fs.space();
        assert_eq!(total, 256 * BLOCK as u64);
        assert!(free > 200 * BLOCK as u64 && free < total);
        let mut dev = fs.into_dev();
        assert!(Nffs::is_formatted(&mut *dev));
        assert!(!Nffs::is_blank(&mut *dev));
        let mut fs = Nffs::mount(dev).unwrap();
        fs.check().unwrap();
        assert!(fs.list("").unwrap().is_empty());
        assert!(fs.stat("").unwrap().dir);
    }

    #[test]
    fn blank_and_size_limits() {
        let mut small = RamDisk::new(100);
        assert!(Nffs::is_blank(&mut small));
        assert!(!Nffs::is_formatted(&mut small));
        small.write(99, &[1u8; BLOCK]).unwrap();
        assert!(!Nffs::is_blank(&mut small));
        let mut big = RamDisk::new(1000);
        assert!(Nffs::is_blank(&mut big));
        big.write(500, &[1u8; BLOCK]).unwrap(); // en medio: no cuenta
        assert!(Nffs::is_blank(&mut big));
        big.write(999, &[1u8; BLOCK]).unwrap();
        assert!(!Nffs::is_blank(&mut big));
        let mut big = RamDisk::new(1000);
        big.write(255, &[1u8; BLOCK]).unwrap();
        assert!(!Nffs::is_blank(&mut big));
        assert_eq!(Nffs::format(Box::new(RamDisk::new(10)), t()).err(), Some(FsError::NoSpace));
        assert_eq!(Nffs::format(Box::new(RamDisk::new(63)), t()).err(), Some(FsError::NoSpace));
        assert!(Nffs::format(Box::new(RamDisk::new(64)), t()).is_ok());
        assert_eq!(Nffs::mount(Box::new(RamDisk::new(100))).err(), Some(FsError::Corrupt));
        assert_eq!(Nffs::mount(Box::new(RamDisk::new(0))).err(), Some(FsError::Corrupt));
    }

    #[test]
    fn create_read_replace() {
        let (_, mut fs) = fresh(512);
        fs.write("a.txt", b"hola", t()).unwrap();
        assert_eq!(fs.read("a.txt").unwrap(), b"hola");
        let e = fs.stat("a.txt").unwrap();
        assert_eq!((e.name.as_str(), e.dir, e.size, e.modified), ("a.txt", false, 4, t()));
        let big = pattern(5 * BLOCK + 17, 3);
        fs.write("a.txt", &big, t()).unwrap();
        assert_eq!(fs.read("a.txt").unwrap(), big);
        fs.write("a.txt", b"x", t()).unwrap();
        assert_eq!(fs.read("a.txt").unwrap(), b"x");
        fs.write("a.txt", b"", t()).unwrap();
        assert_eq!(fs.read("a.txt").unwrap(), b"");
        assert_eq!(fs.stat("a.txt").unwrap().size, 0);
        fs.write("vacio", b"", t()).unwrap();
        assert_eq!(fs.read("vacio").unwrap(), b"");
        for n in [BLOCK - 1, BLOCK, BLOCK + 1, 2 * BLOCK] {
            let d = pattern(n, 9);
            fs.write("lim", &d, t()).unwrap();
            assert_eq!(fs.read("lim").unwrap(), d);
        }
        fs.check().unwrap();
        let mut fs = remount(fs);
        assert_eq!(fs.read("lim").unwrap().len(), 2 * BLOCK);
        fs.check().unwrap();
    }

    #[test]
    fn modified_date_is_kept() {
        let (_, mut fs) = fresh(128);
        let d2 = DateTime { year: 2027, month: 1, day: 2, hour: 3, minute: 4, second: 5 };
        fs.mkdir("d", d2).unwrap();
        fs.write("d\\f", b"1", d2).unwrap();
        let mut fs = remount(fs);
        assert_eq!(fs.stat("d").unwrap().modified, d2);
        assert_eq!(fs.stat("d\\f").unwrap().modified, d2);
        fs.write("d\\f", b"2", t()).unwrap();
        assert_eq!(fs.stat("d\\f").unwrap().modified, t());
    }

    #[test]
    fn folders_and_errors() {
        let (_, mut fs) = fresh(256);
        fs.mkdir("a", t()).unwrap();
        fs.mkdir("a\\b", t()).unwrap();
        fs.write("a\\b\\f", b"datos", t()).unwrap();
        assert_eq!(fs.mkdir("a", t()), Err(FsError::Exists));
        assert_eq!(fs.mkdir("x\\y", t()), Err(FsError::NotFound));
        assert_eq!(fs.write("x\\y", b"1", t()), Err(FsError::NotFound));
        assert_eq!(fs.write("a\\b\\f\\g", b"1", t()), Err(FsError::NotDir));
        assert_eq!(fs.mkdir("a\\b\\f\\g", t()), Err(FsError::NotDir));
        assert_eq!(fs.list("a\\b\\f"), Err(FsError::NotDir));
        assert_eq!(fs.read("a"), Err(FsError::IsDir));
        assert_eq!(fs.write("a", b"1", t()), Err(FsError::IsDir));
        assert_eq!(fs.read("nada"), Err(FsError::NotFound));
        assert_eq!(fs.stat("a\\nada"), Err(FsError::NotFound));
        assert_eq!(fs.list("nada"), Err(FsError::NotFound));
        assert_eq!(fs.write("", b"1", t()), Err(FsError::Invalid));
        assert_eq!(fs.mkdir("", t()), Err(FsError::Invalid));
        let names: Vec<String> = fs.list("a").unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["b"]);
        assert!(fs.stat("a\\b").unwrap().dir);
        // barras repetidas y al principio se toleran
        assert_eq!(fs.read("\\a\\\\b\\f").unwrap(), b"datos");
        fs.check().unwrap();
    }

    #[test]
    fn names_case_and_validity() {
        let (_, mut fs) = fresh(256);
        fs.write("Ñandú.txt", b"1", t()).unwrap();
        assert_eq!(fs.read("ñANDÚ.TXT").unwrap(), b"1");
        assert_eq!(fs.stat("ÑANDÚ.txt").unwrap().name, "Ñandú.txt");
        assert_eq!(fs.mkdir("ñandú.TXT", t()), Err(FsError::Exists));
        fs.mkdir("ÁRBOL", t()).unwrap();
        assert_eq!(fs.mkdir("árbol", t()), Err(FsError::Exists));
        fs.write("árbol\\Ü", b"2", t()).unwrap();
        fs.write("ÁRBOL\\ü", b"3", t()).unwrap(); // sustituye al mismo
        assert_eq!(fs.read("Árbol\\Ü").unwrap(), b"3");
        let e = fs.list("ÁRBOL").unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].name, "Ü");
        let long = "a".repeat(255);
        fs.write(&long, b"1", t()).unwrap();
        let multi = format!("{}x", "ñ".repeat(127)); // 255 bytes
        assert_eq!(multi.len(), 255);
        fs.write(&multi, b"1", t()).unwrap();
        let too_long = "a".repeat(256);
        let too_long_multi = "ñ".repeat(128);
        for bad in ["a*b", "a?b", "a\"b", "a<b", "a>b", "a|b", "a:b", "con\u{1}trol", "..", "a\\..", &too_long, &too_long_multi] {
            assert_eq!(fs.write(bad, b"1", t()), Err(FsError::BadName), "{bad:?}");
            assert_eq!(fs.mkdir(bad, t()), Err(FsError::BadName), "{bad:?}");
        }
        assert_eq!(fs.write("a\\.", b"1", t()), Err(FsError::BadName));
        fs.check().unwrap();
        let mut fs = remount(fs);
        assert_eq!(fs.read(&long).unwrap(), b"1");
        assert_eq!(fs.read(&multi.to_uppercase()).unwrap(), b"1");
        fs.check().unwrap();
    }

    #[test]
    fn rename_cases() {
        let (_, mut fs) = fresh(512);
        fs.mkdir("a", t()).unwrap();
        fs.mkdir("a\\b", t()).unwrap();
        fs.write("a\\b\\f", &pattern(9000, 1), t()).unwrap();
        fs.write("x", b"x", t()).unwrap();
        fs.mkdir("d", t()).unwrap();
        // renombrar en la misma carpeta
        fs.rename("x", "y").unwrap();
        assert_eq!(fs.read("x"), Err(FsError::NotFound));
        assert_eq!(fs.read("y").unwrap(), b"x");
        // mover a otra carpeta con otro nombre
        fs.rename("y", "d\\z").unwrap();
        assert_eq!(fs.read("d\\z").unwrap(), b"x");
        // mover una carpeta con contenido
        fs.rename("a\\b", "d\\b2").unwrap();
        assert_eq!(fs.read("d\\b2\\f").unwrap(), pattern(9000, 1));
        assert!(fs.list("a").unwrap().is_empty());
        // dentro de sí misma
        fs.mkdir("d\\b2\\c", t()).unwrap();
        assert_eq!(fs.rename("d", "d\\b2\\c\\q"), Err(FsError::Invalid));
        assert_eq!(fs.rename("d", "d\\q"), Err(FsError::Invalid));
        assert_eq!(fs.rename("D\\B2", "d\\b2\\c\\B2"), Err(FsError::Invalid));
        // existe el destino (también con otra capitalización, archivo o carpeta)
        fs.write("e", b"e", t()).unwrap();
        assert_eq!(fs.rename("e", "D\\Z"), Err(FsError::Exists));
        assert_eq!(fs.rename("e", "D"), Err(FsError::Exists));
        assert_eq!(fs.rename("d", "E"), Err(FsError::Exists));
        // solo mayúsculas
        fs.rename("e", "E").unwrap();
        assert_eq!(fs.list("").unwrap().iter().filter(|x| x.name == "E").count(), 1);
        fs.rename("d\\b2", "d\\B2").unwrap();
        assert_eq!(fs.stat("d\\b2").unwrap().name, "B2");
        // mismo nombre exacto: nada
        fs.rename("E", "E").unwrap();
        // no encontrados y raíz
        assert_eq!(fs.rename("nada", "otro"), Err(FsError::NotFound));
        assert_eq!(fs.rename("E", "nada\\otro"), Err(FsError::NotFound));
        assert_eq!(fs.rename("E", "d\\z\\otro"), Err(FsError::NotDir));
        assert_eq!(fs.rename("", "otro"), Err(FsError::Invalid));
        assert_eq!(fs.rename("E", ""), Err(FsError::Invalid));
        assert_eq!(fs.rename("E", "a*"), Err(FsError::BadName));
        fs.check().unwrap();
        let mut fs = remount(fs);
        assert_eq!(fs.read("d\\B2\\f").unwrap(), pattern(9000, 1));
        fs.check().unwrap();
    }

    #[test]
    fn remove_cases() {
        let (_, mut fs) = fresh(256);
        let (_, free0) = fs.space();
        fs.mkdir("a", t()).unwrap();
        fs.write("a\\f", &pattern(20_000, 4), t()).unwrap();
        assert_eq!(fs.remove("a"), Err(FsError::NotEmpty));
        assert_eq!(fs.remove(""), Err(FsError::Invalid));
        assert_eq!(fs.remove("nada"), Err(FsError::NotFound));
        assert_eq!(fs.remove("nada\\x"), Err(FsError::NotFound));
        assert_eq!(fs.remove("a\\f\\x"), Err(FsError::NotDir));
        fs.remove("A\\F").unwrap();
        assert_eq!(fs.read("a\\f"), Err(FsError::NotFound));
        fs.remove("a").unwrap();
        assert!(fs.list("").unwrap().is_empty());
        assert_eq!(fs.space().1, free0);
        fs.check().unwrap();
        // los números de nodo liberados se reutilizan
        for i in 0..50 {
            fs.write(&format!("f{i}"), b"1", t()).unwrap();
            fs.remove(&format!("f{i}")).unwrap();
        }
        assert_eq!(fs.nodes.len(), 1);
    }

    #[test]
    fn fill_disk_until_nospace() {
        let (_, mut fs) = fresh(256);
        let (_, free0) = fs.space();
        let data = pattern(8 * BLOCK, 5);
        let mut n = 0;
        loop {
            match fs.write(&format!("f{n}"), &data, t()) {
                Ok(()) => n += 1,
                Err(e) => {
                    assert_eq!(e, FsError::NoSpace);
                    break;
                }
            }
        }
        assert!(n > 20);
        fs.check().unwrap();
        let before = dump(&mut fs);
        assert_eq!(before.len(), n);
        // lo que falla no deja rastro
        assert_eq!(fs.write("grande", &pattern(200 * BLOCK, 1), t()), Err(FsError::NoSpace));
        assert_eq!(fs.write("f0", &pattern(200 * BLOCK, 1), t()), Err(FsError::NoSpace));
        assert_eq!(dump(&mut fs), before);
        fs.check().unwrap();
        let mut fs = remount(fs);
        assert_eq!(dump(&mut fs), before);
        // con el disco lleno aún se puede borrar, y se recupera todo
        for i in 0..n {
            fs.remove(&format!("f{i}")).unwrap();
        }
        fs.check().unwrap();
        assert_eq!(fs.space().1, free0);
        fs.write("otra", &data, t()).unwrap();
    }

    #[test]
    fn rename_and_remove_on_full_disk() {
        let (_, mut fs) = fresh(128);
        let mut n = 0;
        while fs.write(&format!("f{n}"), &pattern(3 * BLOCK, 1), t()).is_ok() {
            n += 1;
        }
        fs.rename("f0", "otro-nombre-algo-mas-largo").unwrap();
        fs.remove("otro-nombre-algo-mas-largo").unwrap();
        fs.check().unwrap();
        let mut fs = remount(fs);
        fs.check().unwrap();
        assert_eq!(fs.list("").unwrap().len(), n - 1);
    }

    #[test]
    fn fragmented_disk_and_big_files() {
        let (_, mut fs) = fresh(4096);
        for i in 0..400 {
            fs.write(&format!("p{i}"), &pattern(BLOCK, i as u8), t()).unwrap();
        }
        for i in (1..400).step_by(2) {
            fs.remove(&format!("p{i}")).unwrap();
        }
        let big = pattern(150 * BLOCK + 123, 11);
        fs.write("grande", &big, t()).unwrap();
        let id = fs.lookup("grande").unwrap();
        assert!(fs.nodes[&id].extents.len() > 100, "{}", fs.nodes[&id].extents.len());
        assert_eq!(fs.read("grande").unwrap(), big);
        fs.check().unwrap();
        let mut fs = remount(fs);
        assert_eq!(fs.read("grande").unwrap(), big);
        for i in (0..400).step_by(2) {
            assert_eq!(fs.read(&format!("p{i}")).unwrap(), pattern(BLOCK, i as u8));
        }
        // varios MiB
        let huge = pattern(3 * 1024 * 1024 + 5, 77);
        fs.write("enorme", &huge, t()).unwrap();
        assert_eq!(fs.read("enorme").unwrap(), huge);
        fs.write("enorme", b"corto", t()).unwrap();
        fs.write("enorme2", &huge, t()).unwrap();
        fs.check().unwrap();
        assert_eq!(remount(fs).read("enorme2").unwrap(), huge);
    }

    #[test]
    fn many_nodes_need_several_meta_blocks() {
        let (_, mut fs) = fresh(2048);
        fs.mkdir("d", t()).unwrap();
        for i in 0..300 {
            fs.write(&format!("d\\archivo-con-nombre-bastante-largo-{i:05}.txt"), &[i as u8], t()).unwrap();
        }
        assert!(fs.meta.len() > 2);
        fs.check().unwrap();
        let mut fs = remount(fs);
        assert_eq!(fs.list("d").unwrap().len(), 300);
        assert_eq!(fs.read("d\\archivo-con-nombre-bastante-largo-00042.txt").unwrap(), [42]);
        fs.check().unwrap();
    }

    #[test]
    fn max_nodes_limit() {
        let (_, mut fs) = fresh(64); // 64 nodos como mucho
        let mut n = 1;
        let r = loop {
            match fs.write(&format!("n{n}"), b"", t()) {
                Ok(()) => n += 1,
                Err(e) => break e,
            }
        };
        assert_eq!(r, FsError::NoSpace);
        assert_eq!(n, 64);
        fs.check().unwrap();
        fs.remove("n1").unwrap();
        fs.write("otra", b"", t()).unwrap();
    }

    #[test]
    fn older_copy_is_used_when_newest_is_damaged() {
        let (sh, mut fs) = fresh(256);
        fs.write("a", b"1", t()).unwrap();
        fs.write("b", b"2", t()).unwrap();
        let slot = fs.slot as usize;
        drop(fs);
        // se rompe la copia más reciente: se monta la anterior (estado tras escribir "a")
        let mut bytes = sh.bytes();
        bytes[slot * BLOCK + 20] ^= 0xff;
        let mut fs = Nffs::mount(Box::new(RamDisk::from_bytes(bytes.clone()))).unwrap();
        assert_eq!(fs.read("a").unwrap(), b"1");
        assert_eq!(fs.read("b"), Err(FsError::NotFound));
        fs.check().unwrap();
        fs.write("c", b"3", t()).unwrap();
        let mut fs = remount(fs);
        assert_eq!(fs.read("c").unwrap(), b"3");
        fs.check().unwrap();
        // las dos copias rotas
        bytes[(1 - slot) * BLOCK + 20] ^= 0xff;
        assert_eq!(Nffs::mount(Box::new(RamDisk::from_bytes(bytes))).err(), Some(FsError::Corrupt));
    }

    #[test]
    fn mount_reads_few_blocks_on_big_disk() {
        let sh = Shared::new(65_536); // 256 MiB
        let mut fs = Nffs::format(Box::new(sh.clone()), t()).unwrap();
        fs.mkdir("d", t()).unwrap();
        for i in 0..20 {
            fs.write(&format!("d\\f{i}"), &pattern(100_000, i as u8), t()).unwrap();
        }
        drop(fs);
        sh.reads.set(0);
        let mut fs = Nffs::mount(Box::new(sh.clone())).unwrap();
        assert!(sh.reads.get() <= 10, "lecturas al montar: {}", sh.reads.get());
        sh.reads.set(0);
        fs.write("nuevo", b"x", t()).unwrap();
        assert_eq!(sh.reads.get(), 0, "una escritura no debe leer el disco");
        let (total, free) = fs.space();
        assert_eq!(total, 256 << 20);
        assert!(free < total && free > total - (4 << 20));
        fs.check().unwrap();
    }

    #[test]
    fn space_is_exact_on_data() {
        let (_, mut fs) = fresh(512);
        let f0 = fs.space().1;
        fs.write("a", &pattern(10 * BLOCK, 1), t()).unwrap();
        assert_eq!(f0 - fs.space().1, 10 * BLOCK as u64);
        fs.write("a", &pattern(3 * BLOCK + 1, 1), t()).unwrap();
        assert_eq!(f0 - fs.space().1, 4 * BLOCK as u64);
        fs.write("b", b"", t()).unwrap();
        assert_eq!(f0 - fs.space().1, 4 * BLOCK as u64);
        fs.remove("a").unwrap();
        fs.remove("b").unwrap();
        assert_eq!(fs.space().1, f0);
    }

    // ---- cortes de luz ----

    enum Op {
        Write(&'static str, Vec<u8>),
        Mkdir(&'static str),
        Remove(&'static str),
        Rename(&'static str, &'static str),
    }

    fn apply(fs: &mut Nffs, op: &Op) -> Result<(), FsError> {
        match op {
            Op::Write(p, d) => fs.write(p, d, t()),
            Op::Mkdir(p) => fs.mkdir(p, t()),
            Op::Remove(p) => fs.remove(p),
            Op::Rename(a, b) => fs.rename(a, b),
        }
    }

    #[test]
    fn power_cuts_at_every_write() {
        // disco preparado: fragmentado y con metadatos de varios bloques
        let sh = Shared::new(512);
        let mut fs = Nffs::format(Box::new(sh.clone()), t()).unwrap();
        fs.mkdir("s", t()).unwrap();
        for i in 0..120 {
            fs.write(&format!("s\\relleno-largo-{i:04}-xxxxxxxxxxxx"), &pattern(BLOCK / 2 + i, i as u8), t()).unwrap();
        }
        for i in (0..120).step_by(3) {
            fs.remove(&format!("s\\relleno-largo-{i:04}-xxxxxxxxxxxx")).unwrap();
        }
        assert!(fs.meta.len() >= 2);
        let ops = vec![
            Op::Mkdir("a"),
            Op::Mkdir("a\\b"),
            Op::Write("a\\f1", pattern(3 * BLOCK, 1)),
            Op::Write("a\\b\\f2", vec![42]),
            Op::Write("a\\f1", pattern(60 * BLOCK + 9, 2)),
            Op::Write("a\\f1", pattern(10, 3)),
            Op::Write("z", vec![]),
            Op::Mkdir("a"),
            Op::Remove("a"),
            Op::Rename("a\\b", "c"),
            Op::Rename("z", "Z"),
            Op::Rename("a", "a2"),
            Op::Rename("a2", "c\\dentro"),
            Op::Rename("c", "c\\x\\y"),
            Op::Write("grande", pattern(40 * BLOCK, 9)),
            Op::Write("grande", pattern(41 * BLOCK, 8)),
            Op::Remove("c\\dentro\\f1"),
            Op::Remove("c\\f2"),
            Op::Remove("c\\dentro"),
            Op::Remove("c"),
            Op::Remove("grande"),
            Op::Write("s", vec![1]),
        ];
        let mut cuts = 0;
        for (i, op) in ops.iter().enumerate() {
            let before_bytes = sh.bytes();
            let mut a = Nffs::mount(Box::new(Shared::from_bytes(before_bytes.clone()))).unwrap();
            let dump_a = dump(&mut a);
            // ejecución normal: cuántas escrituras hace y qué resulta
            sh.writes.set(0);
            let res = apply(&mut fs, op);
            let w = sh.writes.get();
            fs.check().unwrap();
            let dump_b = dump(&mut fs);
            if res.is_err() {
                assert_eq!(w, 0, "una operación fallida no escribe (op {i})");
                assert_eq!(dump_a, dump_b);
            }
            for n in 0..=w {
                let cut = Shared::from_bytes(before_bytes.clone());
                let mut f = Nffs::mount(Box::new(cut.clone())).unwrap();
                cut.limit.set(cut.writes.get() + n);
                let _ = apply(&mut f, op);
                drop(f);
                let mut after = Nffs::mount(Box::new(Shared::from_bytes(cut.bytes())))
                    .unwrap_or_else(|e| panic!("no monta tras cortar en {n}/{w} de la op {i}: {e:?}"));
                after.check().unwrap_or_else(|e| panic!("incoherente tras cortar en {n}/{w} de la op {i}: {e:?}"));
                let d = dump(&mut after);
                if n < w {
                    assert_eq!(d, dump_a, "corte {n}/{w} de la op {i} debe dejar el estado anterior");
                } else {
                    assert_eq!(d, dump_b, "sin corte (op {i}) debe dejar el posterior");
                }
                cuts += 1;
            }
        }
        assert!(cuts > 100, "{cuts}");
    }

    #[test]
    fn nospace_leaves_disk_untouched_and_full_disk_cuts_are_clean() {
        let sh = Shared::new(96);
        let mut fs = Nffs::format(Box::new(sh.clone()), t()).unwrap();
        let mut n = 0;
        while fs.write(&format!("f{n}"), &pattern(5 * BLOCK, 1), t()).is_ok() {
            n += 1;
        }
        let before = sh.bytes();
        // el intento fallido no escribe nada en el disco
        assert_eq!(fs.write("zz", &pattern(50 * BLOCK, 1), t()), Err(FsError::NoSpace));
        assert_eq!(sh.bytes(), before);
        // y cortando el borrado en cada escritura queda el estado anterior
        sh.writes.set(0);
        fs.remove("f0").unwrap();
        let w = sh.writes.get();
        for k in 0..w {
            let cut = Shared::from_bytes(before.clone());
            let mut f = Nffs::mount(Box::new(cut.clone())).unwrap();
            cut.limit.set(k);
            let _ = f.remove("f0");
            let mut a = Nffs::mount(Box::new(Shared::from_bytes(cut.bytes()))).unwrap();
            a.check().unwrap();
            assert_eq!(a.list("").unwrap().len(), n);
        }
    }

    // ---- basura ----

    fn walk_all(fs: &mut Nffs) {
        fs.check().expect("si monta, debe ser coherente");
        let _ = dump(fs);
        let _ = fs.write("zz-prueba", b"x", t());
        let _ = fs.mkdir("zz-dir", t());
        let _ = fs.rename("zz-dir", "zz-dir2");
        let _ = fs.remove("zz-prueba");
        fs.check().unwrap();
    }

    fn populated() -> Vec<u8> {
        let sh = Shared::new(200);
        let mut fs = Nffs::format(Box::new(sh.clone()), t()).unwrap();
        fs.mkdir("a", t()).unwrap();
        fs.mkdir("a\\b", t()).unwrap();
        for i in 0..30 {
            fs.write(&format!("a\\b\\ñ{i}"), &pattern(BLOCK + i * 100, i as u8), t()).unwrap();
        }
        fs.write("raiz", b"hola", t()).unwrap();
        drop(fs);
        sh.bytes()
    }

    #[test]
    fn random_disks_never_panic() {
        let mut rng = Rng(0x1234_5678_9abc_def1);
        for i in 0..300 {
            let blocks = 64 + rng.below(40);
            let mut v = rng.bytes(blocks * BLOCK);
            if i % 3 == 0 {
                v[..4].copy_from_slice(MAGIC);
            }
            assert!(Nffs::mount(Box::new(RamDisk::from_bytes(v))).is_err());
        }
    }

    #[test]
    fn random_valid_looking_superblocks_never_panic() {
        let mut rng = Rng(77);
        for _ in 0..3000 {
            let blocks = 64 + rng.below(20) as u64;
            let mut dev = RamDisk::new(blocks);
            for b in 2..blocks {
                if rng.below(2) == 0 {
                    let mut blk = [0u8; BLOCK];
                    blk.copy_from_slice(&rng.bytes(BLOCK));
                    dev.write(b, &blk).unwrap();
                }
            }
            let sb = Sb {
                gen: rng.next() % 5,
                total: if rng.below(2) == 0 { blocks } else { rng.next() % (blocks + 3) },
                max_nodes: rng.next() as u32,
                meta_count: (rng.next() % 6) as u32,
                meta_first: rng.next() % (blocks + 3),
                meta_len: (rng.next() % 30_000) as u32,
                meta_sum: rng.next(),
            };
            dev.write(rng.below(2) as u64, &sb.encode()).unwrap();
            let _ = Nffs::is_formatted(&mut dev);
            if let Ok(mut fs) = Nffs::mount(Box::new(dev)) {
                walk_all(&mut fs);
            }
        }
    }

    #[test]
    fn random_byte_flips_in_metadata() {
        let base = populated();
        let mut rng = Rng(0xdead_beef);
        let (mut ok, mut err) = (0, 0);
        // bloques de metadatos y superbloques; los demás son datos o libres
        let mut hot: Vec<usize> = Nffs::mount(Box::new(RamDisk::from_bytes(base.clone()))).unwrap().meta.iter().map(|&b| b as usize).collect();
        hot.extend([0, 1]);
        for _ in 0..4000 {
            let mut v = base.clone();
            for _ in 0..1 + rng.below(8) {
                let blk = if rng.below(5) == 0 { rng.below(200) } else { hot[rng.below(hot.len())] };
                let off = if rng.below(2) == 0 { rng.below(80) } else { rng.below(BLOCK) };
                v[blk * BLOCK + off] = rng.next() as u8;
            }
            match Nffs::mount(Box::new(RamDisk::from_bytes(v))) {
                Ok(mut fs) => {
                    ok += 1;
                    walk_all(&mut fs)
                }
                Err(_) => err += 1,
            }
        }
        assert!(err > 100 && ok + err == 4000, "{ok} {err}");
    }

    /// Altera los metadatos y recalcula las sumas, para que lleguen al analizador.
    fn tamper(base: &[u8], rng: &mut Rng) -> Vec<u8> {
        let mut dev = RamDisk::from_bytes(base.to_vec());
        let sb = (0..2).filter_map(|s| read_sb(&mut dev, s)).max_by_key(|s| s.gen).unwrap();
        let mut blocks = vec![];
        let mut meta = vec![];
        let mut b = sb.meta_first;
        let mut buf = [0u8; BLOCK];
        for _ in 0..sb.meta_count {
            dev.read(b, &mut buf).unwrap();
            blocks.push(b);
            meta.extend_from_slice(&buf[8..]);
            b = u64::from_le_bytes(buf[..8].try_into().unwrap());
        }
        meta.truncate(sb.meta_len as usize);
        for _ in 0..1 + rng.below(6) {
            let i = if rng.below(2) == 0 { rng.below(meta.len()) } else { sb.total.div_ceil(8) as usize + rng.below(200) };
            let i = i.min(meta.len() - 1);
            meta[i] = if rng.below(2) == 0 { rng.next() as u8 } else { meta[i] ^ (1 << rng.below(8)) };
        }
        for (k, &blk) in blocks.iter().enumerate() {
            dev.read(blk, &mut buf).unwrap();
            let chunk = &meta[k * PAYLOAD..((k + 1) * PAYLOAD).min(meta.len())];
            buf[8..8 + chunk.len()].copy_from_slice(chunk);
            dev.write(blk, &buf).unwrap();
        }
        let new = Sb { meta_sum: fnv(&meta, FNV0), ..sb };
        dev.write(0, &new.encode()).unwrap();
        dev.write(1, &new.encode()).unwrap();
        dev.bytes().to_vec()
    }

    #[test]
    fn tampered_metadata_with_valid_checksums() {
        let base = populated();
        let mut rng = Rng(424242);
        let (mut ok, mut err) = (0, 0);
        for _ in 0..6000 {
            let v = tamper(&base, &mut rng);
            match Nffs::mount(Box::new(RamDisk::from_bytes(v))) {
                Ok(mut fs) => {
                    ok += 1;
                    walk_all(&mut fs)
                }
                Err(e) => {
                    assert_eq!(e, FsError::Corrupt);
                    err += 1
                }
            }
        }
        assert!(err > 2000, "{ok} {err}");
    }

    // ---- prueba larga contra un modelo ----

    type Model = Map<String, Option<Vec<u8>>>;

    fn model_walk(m: &Model, comps: &[String]) -> Result<(), FsError> {
        for k in 1..comps.len() {
            match m.get(&comps[..k].join("\\")) {
                None => return Err(FsError::NotFound),
                Some(Some(_)) => return Err(FsError::NotDir),
                Some(None) => {}
            }
        }
        Ok(())
    }

    fn lcomps(p: &str) -> Vec<String> {
        p.split('\\').map(lower).collect()
    }

    fn lowered(d: Map<String, Option<Vec<u8>>>) -> Model {
        d.into_iter().map(|(k, v)| (lower(&k), v)).collect()
    }

    #[test]
    fn long_random_run_against_model() {
        const POOL: [&str; 6] = ["a", "B", "Ñu", "ñU", "c", "Dé"];
        let mut rng = Rng(0xfeed_f00d);
        let mut fs = Nffs::format(Box::new(Shared::new(4096)), t()).unwrap();
        let mut m: Model = Map::new();
        let path = |rng: &mut Rng| -> String {
            let n = 1 + rng.below(3);
            (0..n).map(|_| POOL[rng.below(POOL.len())]).collect::<Vec<_>>().join("\\")
        };
        let mut done = Map::<&str, u32>::new();
        let mut errors = 0;
        for step in 0..6000 {
            let p = path(&mut rng);
            let lc = lcomps(&p);
            let key = lc.join("\\");
            let kind = rng.below(10);
            let (res, want): (Result<(), FsError>, Result<(), FsError>) = match kind {
                0..=3 => {
                    let len = [0, 1, 100, 4095, 4096, 4097, 9000, 30000][rng.below(8)];
                    let data = rng.bytes(len);
                    let want = model_walk(&m, &lc).and_then(|_| match m.get(&key) {
                        Some(None) => Err(FsError::IsDir),
                        _ => {
                            m.insert(key.clone(), Some(data.clone()));
                            Ok(())
                        }
                    });
                    *done.entry("write").or_insert(0) += 1;
                    (fs.write(&p, &data, t()), want)
                }
                4 | 5 => {
                    let want = model_walk(&m, &lc).and_then(|_| {
                        if m.contains_key(&key) {
                            Err(FsError::Exists)
                        } else {
                            m.insert(key.clone(), None);
                            Ok(())
                        }
                    });
                    *done.entry("mkdir").or_insert(0) += 1;
                    (fs.mkdir(&p, t()), want)
                }
                6 | 7 => {
                    let pre = format!("{key}\\");
                    let want = model_walk(&m, &lc).and_then(|_| match m.get(&key) {
                        None => Err(FsError::NotFound),
                        Some(None) if m.keys().any(|k| k.starts_with(&pre)) => Err(FsError::NotEmpty),
                        _ => {
                            m.remove(&key);
                            Ok(())
                        }
                    });
                    *done.entry("remove").or_insert(0) += 1;
                    (fs.remove(&p), want)
                }
                _ => {
                    let q = path(&mut rng);
                    let lq = lcomps(&q);
                    let qkey = lq.join("\\");
                    let pre = format!("{key}\\");
                    let want = model_walk(&m, &lc)
                        .and_then(|_| if m.contains_key(&key) { Ok(()) } else { Err(FsError::NotFound) })
                        .and_then(|_| model_walk(&m, &lq))
                        .and_then(|_| {
                            let src_dir = m[&key].is_none();
                            let dparent = lq[..lq.len() - 1].join("\\");
                            if src_dir && (dparent == key || dparent.starts_with(&pre)) {
                                return Err(FsError::Invalid);
                            }
                            if key == qkey {
                                return Ok(());
                            }
                            if m.contains_key(&qkey) {
                                return Err(FsError::Exists);
                            }
                            let moved: Vec<(String, Option<Vec<u8>>)> = m
                                .iter()
                                .filter(|(k, _)| **k == key || k.starts_with(&pre))
                                .map(|(k, v)| (format!("{qkey}{}", &k[key.len()..]), v.clone()))
                                .collect();
                            m.retain(|k, _| !(*k == key || k.starts_with(&pre)));
                            m.extend(moved);
                            Ok(())
                        });
                    *done.entry("rename").or_insert(0) += 1;
                    (fs.rename(&p, &q), want)
                }
            };
            if res.is_err() {
                errors += 1;
            }
            assert_eq!(res, want, "paso {step}, tipo {kind}, ruta {p:?}");
            if step % 150 == 149 {
                fs.check().unwrap();
                assert_eq!(lowered(dump(&mut fs)), m, "paso {step}");
            }
            if step % 400 == 399 {
                fs = remount(fs);
            }
        }
        assert_eq!(lowered(dump(&mut fs)), m);
        assert!(m.len() > 5, "el modelo no debería acabar vacío: {}", m.len());
        assert!(done.values().all(|&n| n > 200), "{done:?}");
        assert!(errors > 300 && errors < 5000, "{errors}");
    }
}
