//! Cifrado total del disco (v0.6.0): capa de bloques cifrada con AES-256-XTS. Ver docs/PLAN-estructura.md.
//!
//! # Formato (dispositivo de N bloques)
//! - Bloques `0..N-2`: el NFFS (que mide como mucho N-2), cifrado bloque a bloque con AES-256-XTS;
//!   el sector del ajuste es el número de bloque.
//! - Bloque `N-2`: bloque de trabajo de la conversión (copia del bloque que se está convirtiendo).
//!   Al terminar de cifrar se pone a ceros porque puede contener texto en claro.
//! - Bloque `N-1`: cabecera en claro, texto `clave=valor` por líneas rellenado con ceros:
//!   ```text
//!   NFCRYPT1
//!   version=1
//!   estado=cifrado | cifrando | descifrando
//!   progreso=<siguiente bloque por convertir>
//!   pendiente=si | no          (si: el bloque `progreso` está copiado en el bloque de trabajo)
//!   clave=<Slot::to_text de la clave del disco>
//!   recuperacion=<Slot::to_text del código de recuperación>
//!   suma=<SHA-256 en hexadecimal de todo lo anterior, líneas incluidas>
//!   ```
//!
//! # Conversión en el sitio (cifrar: de 0 a N-3 en claro → cifrado; descifrar: al revés)
//! Por cada bloque i: (a) copiarlo al bloque de trabajo y escribir la cabecera con `pendiente=si`;
//! (b) escribir el bloque i convertido; (c) escribir la cabecera con `progreso=i+1, pendiente=no`.
//! Entre cada paso hay `flush`. Si se corta, al abrir con `pendiente=si` el bloque i se rehace a
//! partir del bloque de trabajo (la conversión es idempotente). Los bloques por debajo del
//! progreso ya están convertidos y los de encima no, así que el sistema se puede usar durante la
//! conversión (`CryptDev` elige según el bloque).
//!
//! Las llaves (la maestra de 64 bytes) solo viven dentro de `Xts`, que las borra al soltarse.
use crate::block::{BlockDev, DiskError, BLOCK};
use crate::cipher::Xts;
use crate::crypto::{ct_eq, sha256, to_hex, from_hex, wipe};
use crate::fs::FsError;
use crate::keys::{normalize_recovery, KeyError, Slot, MASTER};
use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use core::cell::RefCell;

/// Firma de la cabecera.
pub const SIGNATURE: &str = "NFCRYPT1";
/// Bloques del final del dispositivo que no son del NFFS (trabajo y cabecera).
pub const RESERVED: u64 = 2;
/// Mínimo de bloques de un dispositivo cifrado (el NFFS exige 64 y hay dos reservados).
const MIN_DEV: u64 = 66;

/// Estado del disco cifrado.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// Todo cifrado.
    Encrypted,
    /// Cifrando: los bloques por debajo de `next` están cifrados, el resto en claro.
    Encrypting,
    /// Descifrando: los bloques por debajo de `next` están en claro, el resto cifrado.
    Decrypting,
}

impl State {
    fn text(self) -> &'static str {
        match self {
            State::Encrypted => "cifrado",
            State::Encrypting => "cifrando",
            State::Decrypting => "descifrando",
        }
    }
}

/// Contenido de la cabecera.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Header {
    pub state: State,
    /// Siguiente bloque por convertir.
    pub next: u64,
    /// El bloque `next` está copiado en el bloque de trabajo y puede estar a medias.
    pub pending: bool,
    /// Llave maestra envuelta con la clave del disco.
    pub disk: Slot,
    /// Llave maestra envuelta con el código de recuperación.
    pub recovery: Slot,
}

impl Header {
    pub fn encode(&self) -> [u8; BLOCK] {
        let body = format!(
            "{SIGNATURE}\nversion=1\nestado={}\nprogreso={}\npendiente={}\nclave={}\nrecuperacion={}\n",
            self.state.text(),
            self.next,
            if self.pending { "si" } else { "no" },
            self.disk.to_text(),
            self.recovery.to_text()
        );
        let text = format!("{body}suma={}\n", to_hex(&sha256(body.as_bytes())));
        let mut b = [0u8; BLOCK];
        b[..text.len()].copy_from_slice(text.as_bytes());
        b
    }

    /// `None` si el bloque no es una cabecera íntegra (firma, suma, campos y relleno a cero).
    pub fn decode(b: &[u8]) -> Option<Header> {
        if b.len() != BLOCK || &b[..SIGNATURE.len()] != SIGNATURE.as_bytes() {
            return None;
        }
        let end = b.iter().position(|&x| x == 0).unwrap_or(BLOCK);
        if b[end..].iter().any(|&x| x != 0) {
            return None;
        }
        let text = core::str::from_utf8(&b[..end]).ok()?;
        let (body, sum) = text.rsplit_once("suma=")?;
        let sum = from_hex(sum.strip_suffix('\n')?)?;
        if !ct_eq(&sha256(body.as_bytes()), &sum) {
            return None;
        }
        let mut lines = body.lines();
        if lines.next()? != SIGNATURE || lines.next()? != "version=1" {
            return None;
        }
        let mut field = |name: &str| lines.next()?.strip_prefix(name)?.strip_prefix('=').map(String::from);
        let state = match field("estado")?.as_str() {
            "cifrado" => State::Encrypted,
            "cifrando" => State::Encrypting,
            "descifrando" => State::Decrypting,
            _ => return None,
        };
        let next = field("progreso")?.parse::<u64>().ok()?;
        let pending = match field("pendiente")?.as_str() {
            "si" => true,
            "no" => false,
            _ => return None,
        };
        let disk = Slot::parse(&field("clave")?)?;
        let recovery = Slot::parse(&field("recuperacion")?)?;
        if lines.next().is_some() {
            return None;
        }
        // Un disco ya cifrado del todo no tiene progreso ni bloque pendiente.
        if state == State::Encrypted && (next != 0 || pending) {
            return None;
        }
        Some(Header { state, next, pending, disk, recovery })
    }
}

/// ¿Tiene `dev` una cabecera de cifrado válida en su último bloque? No escribe nada.
pub fn detect(dev: &mut dyn BlockDev) -> Option<Header> {
    let n = dev.blocks();
    if n < MIN_DEV {
        return None;
    }
    let mut b = [0u8; BLOCK];
    dev.read(n - 1, &mut b).ok()?;
    let h = Header::decode(&b)?;
    (h.next <= n - RESERVED).then_some(h)
}

/// Escribe la cabecera en el último bloque de `dev` y hace `flush`.
pub fn write_header(dev: &mut dyn BlockDev, h: &Header) -> Result<(), DiskError> {
    let n = dev.blocks();
    if n < MIN_DEV {
        return Err(DiskError::Range);
    }
    dev.write(n - 1, &h.encode())?;
    dev.flush()
}

/// Comprueba la clave del disco (o el código de recuperación) contra la cabecera y devuelve la
/// llave maestra. El que llama debe borrarla (`crypto::wipe`).
pub fn unlock(h: &Header, secret: &str, recovery: bool) -> Result<[u8; MASTER], KeyError> {
    let m = if recovery {
        let mut code = normalize_recovery(secret).ok_or(KeyError::WrongSecret)?.into_bytes();
        let r = h.recovery.unwrap(&code);
        wipe(&mut code);
        r
    } else {
        h.disk.unwrap(secret.as_bytes())
    };
    m.ok_or(KeyError::WrongSecret)
}

fn ke(e: DiskError) -> KeyError {
    KeyError::Fs(FsError::from(e))
}

/// Estado compartido de un disco cifrado: el dispositivo de abajo, las llaves y la cabecera.
pub struct Core {
    dev: Option<Box<dyn BlockDev>>,
    xts: Xts,
    hdr: Header,
    /// Bloques del dispositivo (con los dos reservados).
    total: u64,
}

impl Core {
    /// `master` es la llave maestra ya comprobada; el que llama la borra después.
    pub fn new(dev: Box<dyn BlockDev>, hdr: Header, master: &[u8; MASTER]) -> Core {
        let total = dev.blocks();
        Core { dev: Some(dev), xts: Xts::new(master), hdr, total }
    }

    pub fn header(&self) -> &Header {
        &self.hdr
    }

    /// Bloques que ve el NFFS.
    pub fn data_blocks(&self) -> u64 {
        self.total - RESERVED
    }

    /// (bloques hechos, total, `true` = cifrando) mientras hay una conversión; `None` si no.
    pub fn progress(&self) -> Option<(u64, u64, bool)> {
        (self.hdr.state != State::Encrypted).then_some((self.hdr.next, self.data_blocks(), self.hdr.state == State::Encrypting))
    }

    /// Devuelve el dispositivo de abajo y deja el `Core` inservible (llaves incluidas).
    pub fn take_dev(&mut self) -> Option<Box<dyn BlockDev>> {
        self.dev.take()
    }

    fn dev(&mut self) -> Result<&mut Box<dyn BlockDev>, DiskError> {
        self.dev.as_mut().ok_or(DiskError::Io)
    }

    fn work(&self) -> u64 {
        self.total - 2
    }

    /// ¿Está el bloque `n` en disco en forma cifrada?
    fn is_cipher(&self, n: u64) -> bool {
        match self.hdr.state {
            State::Encrypted => true,
            State::Encrypting => n < self.hdr.next,
            State::Decrypting => n >= self.hdr.next,
        }
    }

    /// Escribe la cabecera nueva y, solo si llegó al disco, la adopta.
    fn put(&mut self, h: Header) -> Result<(), DiskError> {
        write_header(&mut **self.dev()?, &h)?;
        self.hdr = h;
        Ok(())
    }

    /// Cambia las llaves envueltas (cambio de clave o de código); no toca los datos.
    pub fn set_slots(&mut self, disk: Option<Slot>, recovery: Option<Slot>) -> Result<(), DiskError> {
        self.clean()?;
        let mut h = self.hdr.clone();
        if let Some(s) = disk {
            h.disk = s;
        }
        if let Some(s) = recovery {
            h.recovery = s;
        }
        self.put(h)
    }

    /// Empieza a descifrar (el disco debe estar cifrado del todo).
    pub fn start_decrypt(&mut self) -> Result<(), DiskError> {
        if self.hdr.state != State::Encrypted {
            return Err(DiskError::Io);
        }
        let mut h = self.hdr.clone();
        h.state = State::Decrypting;
        self.put(h)
    }

    /// Si quedó un bloque a medias, lo rehace desde el bloque de trabajo.
    fn clean(&mut self) -> Result<(), DiskError> {
        if !self.hdr.pending {
            return Ok(());
        }
        let (i, w) = (self.hdr.next, self.work());
        let mut buf = [0u8; BLOCK];
        let r = self.redo(i, w, &mut buf);
        wipe(&mut buf);
        r
    }

    fn redo(&mut self, i: u64, w: u64, buf: &mut [u8; BLOCK]) -> Result<(), DiskError> {
        self.dev()?.read(w, buf)?;
        self.redo_from(i, buf)
    }

    fn convert(&self, i: u64, buf: &mut [u8]) {
        if self.hdr.state == State::Encrypting {
            self.xts.encrypt(i, buf)
        } else {
            self.xts.decrypt(i, buf)
        }
    }

    /// Recupera tras un corte (rehace el bloque pendiente) y, si ya estaba todo convertido, remata.
    pub fn recover(&mut self) -> Result<(), DiskError> {
        self.clean()?;
        if self.hdr.state != State::Encrypted && self.hdr.next >= self.data_blocks() {
            self.finish()?;
        }
        Ok(())
    }

    /// Remata una conversión terminada: borra el bloque de trabajo (puede tener texto en claro) y,
    /// al cifrar, marca el disco como cifrado; al descifrar, borra la cabecera.
    fn finish(&mut self) -> Result<(), DiskError> {
        let w = self.work();
        let zero = [0u8; BLOCK];
        let n = self.total;
        match self.hdr.state {
            State::Encrypted => {}
            State::Encrypting => {
                let d = self.dev()?;
                d.write(w, &zero)?;
                d.flush()?;
                let mut h = self.hdr.clone();
                h.state = State::Encrypted;
                h.next = 0;
                self.put(h)?;
            }
            State::Decrypting => {
                let d = self.dev()?;
                d.write(w, &zero)?;
                d.flush()?;
                d.write(n - 1, &zero)?;
                d.flush()?;
            }
        }
        Ok(())
    }

    /// Convierte hasta `max` bloques más. `Ok(true)` cuando ha terminado del todo (al descifrar, el
    /// disco ya es un NFFS normal y quien lo usa puede quedarse con el dispositivo: `take_dev`).
    pub fn step(&mut self, max: u32) -> Result<bool, DiskError> {
        if self.hdr.state == State::Encrypted {
            return Ok(true);
        }
        self.clean()?;
        for _ in 0..max {
            if self.hdr.next >= self.data_blocks() {
                break;
            }
            self.convert_one()?;
        }
        if self.hdr.next >= self.data_blocks() {
            self.finish()?;
            return Ok(true);
        }
        Ok(false)
    }

    fn convert_one(&mut self) -> Result<(), DiskError> {
        let (i, w) = (self.hdr.next, self.work());
        let mut buf = [0u8; BLOCK];
        let r = (|| {
            self.dev()?.read(i, &mut buf)?;
            // (a) copia de seguridad del bloque y marca de pendiente.
            let d = self.dev()?;
            d.write(w, &buf)?;
            d.flush()?;
            let mut h = self.hdr.clone();
            h.pending = true;
            self.put(h)?;
            // (b) y (c) están en `redo`, que parte del bloque de trabajo.
            self.redo_from(i, &mut buf)
        })();
        wipe(&mut buf);
        r
    }

    /// (b) y (c) con el contenido ya en `buf` (el mismo que hay en el bloque de trabajo).
    fn redo_from(&mut self, i: u64, buf: &mut [u8; BLOCK]) -> Result<(), DiskError> {
        self.convert(i, buf);
        let d = self.dev()?;
        d.write(i, buf)?;
        d.flush()?;
        let mut h = self.hdr.clone();
        h.next = i + 1;
        h.pending = false;
        self.put(h)
    }

    pub fn read(&mut self, n: u64, buf: &mut [u8]) -> Result<(), DiskError> {
        if n >= self.data_blocks() || buf.len() != BLOCK {
            return Err(DiskError::Range);
        }
        self.clean()?;
        self.dev()?.read(n, buf)?;
        if self.is_cipher(n) {
            self.xts.decrypt(n, buf);
        }
        Ok(())
    }

    pub fn write(&mut self, n: u64, buf: &[u8]) -> Result<(), DiskError> {
        if n >= self.data_blocks() || buf.len() != BLOCK {
            return Err(DiskError::Range);
        }
        self.clean()?;
        if self.is_cipher(n) {
            let mut t = [0u8; BLOCK];
            t.copy_from_slice(buf);
            self.xts.encrypt(n, &mut t);
            let r = self.dev()?.write(n, &t);
            wipe(&mut t);
            r
        } else {
            self.dev()?.write(n, buf)
        }
    }

    pub fn flush(&mut self) -> Result<(), DiskError> {
        self.dev()?.flush()
    }
}

/// Dispositivo que el NFFS ve: `N-2` bloques en claro; por debajo, cifrados al vuelo.
pub struct CryptDev(Rc<RefCell<Core>>);

impl CryptDev {
    pub fn new(core: Rc<RefCell<Core>>) -> CryptDev {
        CryptDev(core)
    }
}

impl BlockDev for CryptDev {
    fn blocks(&self) -> u64 {
        self.0.borrow().data_blocks()
    }
    fn read(&mut self, n: u64, buf: &mut [u8]) -> Result<(), DiskError> {
        self.0.borrow_mut().read(n, buf)
    }
    fn write(&mut self, n: u64, buf: &[u8]) -> Result<(), DiskError> {
        self.0.borrow_mut().write(n, buf)
    }
    fn flush(&mut self) -> Result<(), DiskError> {
        self.0.borrow_mut().flush()
    }
}

/// Crea el `Core` de un dispositivo con cabecera, comprobando la clave. Si la clave es mala devuelve
/// el dispositivo intacto para que el que llama lo conserve.
pub fn open(
    dev: Box<dyn BlockDev>,
    hdr: Header,
    secret: &str,
    recovery: bool,
) -> Result<Core, (Box<dyn BlockDev>, Header, KeyError)> {
    match unlock(&hdr, secret, recovery) {
        Ok(mut m) => {
            let c = Core::new(dev, hdr, &m);
            wipe(&mut m);
            Ok(c)
        }
        Err(e) => Err((dev, hdr, e)),
    }
}

/// Convierte un error de disco en el de claves.
pub fn disk_err(e: DiskError) -> KeyError {
    ke(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Pool;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::vec::Vec;

    /// Disco en memoria con corte de luz simulado: a partir de la escritura `limit` se descartan.
    #[derive(Clone)]
    pub struct Cut {
        pub data: Rc<RefCell<Vec<u8>>>,
        pub writes: Rc<Cell<usize>>,
        pub limit: Rc<Cell<usize>>,
    }
    impl Cut {
        pub fn new(blocks: usize) -> Cut {
            Cut { data: Rc::new(RefCell::new(vec![0; blocks * BLOCK])), writes: Rc::new(Cell::new(0)), limit: Rc::new(Cell::new(usize::MAX)) }
        }
        pub fn bytes(&self) -> Vec<u8> {
            self.data.borrow().clone()
        }
    }
    impl BlockDev for Cut {
        fn blocks(&self) -> u64 {
            (self.data.borrow().len() / BLOCK) as u64
        }
        fn read(&mut self, n: u64, buf: &mut [u8]) -> Result<(), DiskError> {
            if n >= self.blocks() || buf.len() != BLOCK {
                return Err(DiskError::Range);
            }
            buf.copy_from_slice(&self.data.borrow()[n as usize * BLOCK..][..BLOCK]);
            Ok(())
        }
        fn write(&mut self, n: u64, buf: &[u8]) -> Result<(), DiskError> {
            if n >= self.blocks() || buf.len() != BLOCK {
                return Err(DiskError::Range);
            }
            let w = self.writes.get();
            self.writes.set(w + 1);
            if w < self.limit.get() {
                self.data.borrow_mut()[n as usize * BLOCK..][..BLOCK].copy_from_slice(buf);
            }
            Ok(())
        }
    }

    pub fn pool() -> Pool {
        let mut p = Pool::new();
        for i in 0..crate::keys::POOL_EVENTS_NEEDED as i32 {
            p.add_event(i, i * 3, i as u64, i as u64 * 7);
        }
        p
    }

    /// Cabecera y llave nuevas para un disco de prueba.
    pub fn new_header(pool: &mut Pool, state: State) -> (Header, [u8; MASTER]) {
        let m = pool.master();
        let (mut s1, mut i1, mut s2, mut i2) = ([0u8; 16], [0u8; 16], [0u8; 16], [0u8; 16]);
        pool.take(&mut s1);
        pool.take(&mut i1);
        pool.take(&mut s2);
        pool.take(&mut i2);
        let disk = Slot::wrap(&m, b"clave-disco", s1, i1);
        let recovery = Slot::wrap(&m, b"RECUPERACION", s2, i2);
        (Header { state, next: 0, pending: false, disk, recovery }, m)
    }

    fn block(i: u64) -> [u8; BLOCK] {
        let mut b = [0u8; BLOCK];
        for (k, x) in b.iter_mut().enumerate() {
            *x = (i as usize * 31 + k * 7 + 5) as u8;
        }
        b
    }

    /// Disco de 70 bloques (68 de datos) con un patrón distinto en cada bloque.
    fn patterned() -> Cut {
        let d = Cut::new(70);
        let mut dev = d.clone();
        for i in 0..68 {
            dev.write(i, &block(i)).unwrap();
        }
        d
    }

    fn start(d: &Cut, state: State) -> (Core, [u8; MASTER]) {
        let mut p = pool();
        let (h, m) = new_header(&mut p, state);
        let mut dev = d.clone();
        write_header(&mut dev, &h).unwrap();
        (Core::new(Box::new(d.clone()), h, &m), m)
    }

    fn all_ok(core: &mut Core) {
        for i in 0..68 {
            let mut b = [0u8; BLOCK];
            core.read(i, &mut b).unwrap();
            assert_eq!(b, block(i), "bloque {i}");
        }
    }

    #[test]
    fn header_roundtrip_and_tamper() {
        let mut p = pool();
        let (mut h, _) = new_header(&mut p, State::Encrypting);
        h.next = 12;
        h.pending = true;
        let b = h.encode();
        assert_eq!(Header::decode(&b), Some(h.clone()));
        // Cualquier byte alterado del texto invalida la cabecera (suma, firma o formato).
        let used = b.iter().position(|&x| x == 0).unwrap();
        for i in 0..used {
            let mut c = b;
            c[i] ^= 1;
            assert_eq!(Header::decode(&c), None, "byte {i}");
        }
        // Relleno que no es cero, basura y bloques normales.
        let mut c = b;
        c[BLOCK - 1] = 1;
        assert_eq!(Header::decode(&c), None);
        assert_eq!(Header::decode(&[0u8; BLOCK]), None);
        assert_eq!(Header::decode(&[0xa5u8; BLOCK]), None);
        assert_eq!(Header::decode(&b[..100]), None);
        // Cifrado del todo con progreso: incoherente.
        let mut e = h.clone();
        e.state = State::Encrypted;
        assert_eq!(Header::decode(&e.encode()), None);
    }

    #[test]
    fn detect_needs_valid_header_in_last_block() {
        let d = Cut::new(70);
        assert!(detect(&mut d.clone()).is_none());
        let mut p = pool();
        let (h, _) = new_header(&mut p, State::Encrypted);
        write_header(&mut d.clone(), &h).unwrap();
        assert_eq!(detect(&mut d.clone()), Some(h.clone()));
        // En otro bloque no vale.
        let e = Cut::new(70);
        e.clone().write(68, &h.encode()).unwrap();
        assert!(detect(&mut e.clone()).is_none());
        // Progreso fuera del disco.
        let mut bad = h.clone();
        bad.state = State::Encrypting;
        bad.next = 69;
        let f = Cut::new(70);
        write_header(&mut f.clone(), &bad).unwrap();
        assert!(detect(&mut f.clone()).is_none());
    }

    #[test]
    fn unlock_with_key_and_recovery() {
        let mut p = pool();
        let (h, m) = new_header(&mut p, State::Encrypted);
        assert_eq!(unlock(&h, "clave-disco", false), Ok(m));
        assert_eq!(unlock(&h, "otra", false), Err(KeyError::WrongSecret));
        assert_eq!(unlock(&h, "RECUPERACION", false), Err(KeyError::WrongSecret));
        // Código mal escrito o de otro formato.
        assert_eq!(unlock(&h, "ABC", true), Err(KeyError::WrongSecret));
    }

    #[test]
    fn encrypt_all_then_read_and_raw_is_not_plain() {
        let d = patterned();
        let (mut core, _) = start(&d, State::Encrypting);
        // Mientras se convierte, lo escrito y leído sigue siendo correcto en cada paso.
        let mut steps = 0;
        while !core.step(5).unwrap() {
            all_ok(&mut core);
            steps += 1;
            assert!(steps < 100);
        }
        assert_eq!(core.header().state, State::Encrypted);
        all_ok(&mut core);
        let raw = d.bytes();
        for i in 0..68usize {
            assert_ne!(&raw[i * BLOCK..(i + 1) * BLOCK], &block(i as u64)[..], "bloque {i} en claro");
        }
        // Bloque de trabajo a ceros y cabecera válida.
        assert!(raw[68 * BLOCK..69 * BLOCK].iter().all(|&x| x == 0));
        assert!(detect(&mut d.clone()).is_some_and(|h| h.state == State::Encrypted));
    }

    #[test]
    fn decrypt_all_leaves_plain_disk_without_header() {
        let d = patterned();
        let (mut core, _) = start(&d, State::Encrypting);
        while !core.step(7).unwrap() {}
        core.start_decrypt().unwrap();
        assert_eq!(core.progress(), Some((0, 68, false)));
        while !core.step(3).unwrap() {
            all_ok(&mut core);
        }
        let raw = d.bytes();
        for i in 0..68usize {
            assert_eq!(&raw[i * BLOCK..(i + 1) * BLOCK], &block(i as u64)[..]);
        }
        assert!(raw[68 * BLOCK..].iter().all(|&x| x == 0));
        assert!(detect(&mut d.clone()).is_none());
    }

    #[test]
    fn use_during_conversion_reads_and_writes_both_sides() {
        let d = patterned();
        let (mut core, _) = start(&d, State::Encrypting);
        assert!(!core.step(30).unwrap());
        // Escribir por debajo (cifrado) y por encima (en claro) del progreso.
        let (lo, hi) = (3u64, 50u64);
        core.write(lo, &block(200)).unwrap();
        core.write(hi, &block(201)).unwrap();
        let mut b = [0u8; BLOCK];
        core.read(lo, &mut b).unwrap();
        assert_eq!(b, block(200));
        core.read(hi, &mut b).unwrap();
        assert_eq!(b, block(201));
        // Por debajo, lo que hay en disco es cifrado; por encima, en claro.
        let raw = d.bytes();
        assert_ne!(&raw[lo as usize * BLOCK..][..BLOCK], &block(200)[..]);
        assert_eq!(&raw[hi as usize * BLOCK..][..BLOCK], &block(201)[..]);
        // Y sigue valiendo tras terminar y tras descifrar.
        while !core.step(9).unwrap() {}
        core.read(lo, &mut b).unwrap();
        assert_eq!(b, block(200));
        core.start_decrypt().unwrap();
        assert!(!core.step(20).unwrap());
        core.write(2, &block(202)).unwrap();
        core.write(60, &block(203)).unwrap();
        while !core.step(11).unwrap() {}
        let raw = d.bytes();
        assert_eq!(&raw[lo as usize * BLOCK..][..BLOCK], &block(200)[..]);
        assert_eq!(&raw[hi as usize * BLOCK..][..BLOCK], &block(201)[..]);
        assert_eq!(&raw[2 * BLOCK..][..BLOCK], &block(202)[..]);
        assert_eq!(&raw[60 * BLOCK..][..BLOCK], &block(203)[..]);
    }

    /// Corta la luz en CADA escritura de la conversión, reabre con la clave y termina.
    fn cuts(from: State) {
        // Cuenta las escrituras de la conversión entera en un disco sin cortes.
        let probe = patterned();
        let (mut core, _) = start(&probe, State::Encrypting);
        if from == State::Decrypting {
            while !core.step(100).unwrap() {}
            core.start_decrypt().unwrap();
        }
        let base = probe.writes.get();
        while !core.step(100).unwrap() {}
        let total = probe.writes.get() - base;
        assert!(total > 68);
        for k in 0..=total {
            let d = patterned();
            let (mut core, m) = start(&d, State::Encrypting);
            if from == State::Decrypting {
                while !core.step(100).unwrap() {}
                core.start_decrypt().unwrap();
            }
            d.limit.set(d.writes.get() + k);
            while !core.step(1).unwrap() {}
            // Reinicio: sin Core, con lo que quedó en el disco (también las escrituras tras el corte se perdieron).
            drop(core);
            let after = Cut { data: Rc::new(RefCell::new(d.bytes())), writes: Rc::new(Cell::new(0)), limit: Rc::new(Cell::new(usize::MAX)) };
            let mut dev = after.clone();
            match detect(&mut dev) {
                Some(h) => {
                    let mut core = Core::new(Box::new(after.clone()), h, &m);
                    core.recover().unwrap();
                    while !core.step(4).unwrap() {}
                    if from == State::Encrypting {
                        all_ok(&mut core);
                    } else {
                        // Al descifrar y terminar queda un NFFS plano.
                        let raw = after.bytes();
                        for i in 0..68usize {
                            assert_eq!(&raw[i * BLOCK..(i + 1) * BLOCK], &block(i as u64)[..], "k={k} bloque {i}");
                        }
                        assert!(detect(&mut after.clone()).is_none(), "k={k}");
                    }
                    assert!(after.bytes()[68 * BLOCK..69 * BLOCK].iter().all(|&x| x == 0), "k={k}");
                }
                None => {
                    // Solo puede ser al descifrar, con la cabecera ya borrada: todo en claro.
                    assert_eq!(from, State::Decrypting, "k={k}");
                    let raw = after.bytes();
                    for i in 0..68usize {
                        assert_eq!(&raw[i * BLOCK..(i + 1) * BLOCK], &block(i as u64)[..], "k={k} bloque {i}");
                    }
                }
            }
        }
    }

    #[test]
    fn power_cut_at_every_write_while_encrypting() {
        cuts(State::Encrypting);
    }

    #[test]
    fn power_cut_at_every_write_while_decrypting() {
        cuts(State::Decrypting);
    }

    #[test]
    fn io_error_midway_is_repaired_on_next_use() {
        // Un fallo de escritura en el paso (b) deja `pendiente`; la siguiente lectura lo rehace.
        struct Flaky(Cut, Rc<Cell<bool>>);
        impl BlockDev for Flaky {
            fn blocks(&self) -> u64 {
                self.0.blocks()
            }
            fn read(&mut self, n: u64, b: &mut [u8]) -> Result<(), DiskError> {
                self.0.read(n, b)
            }
            fn write(&mut self, n: u64, b: &[u8]) -> Result<(), DiskError> {
                if self.1.get() && n == 5 {
                    self.1.set(false);
                    return Err(DiskError::Io);
                }
                self.0.write(n, b)
            }
        }
        let d = patterned();
        let mut p = pool();
        let (h, m) = new_header(&mut p, State::Encrypting);
        write_header(&mut d.clone(), &h).unwrap();
        let fail = Rc::new(Cell::new(false));
        let mut core = Core::new(Box::new(Flaky(d.clone(), fail.clone())), h, &m);
        assert!(!core.step(5).unwrap());
        fail.set(true);
        assert!(core.step(1).is_err());
        assert!(core.header().pending);
        all_ok(&mut core);
        assert!(!core.header().pending);
    }

    #[test]
    fn set_slots_rewraps_same_master() {
        let d = patterned();
        let (mut core, m) = start(&d, State::Encrypting);
        while !core.step(100).unwrap() {}
        let new = Slot::wrap(&m, b"nueva", [9; 16], [8; 16]);
        core.set_slots(Some(new), None).unwrap();
        let h = detect(&mut d.clone()).unwrap();
        assert_eq!(unlock(&h, "nueva", false), Ok(m));
        assert_eq!(unlock(&h, "clave-disco", false), Err(KeyError::WrongSecret));
        all_ok(&mut core);
    }
}
