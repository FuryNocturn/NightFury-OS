// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Servicio de archivos y del equipo para los programas en modo usuario (v0.10).
//!
//! El programa codifica una `Call` con `wire` y la manda con `SYS_SERVICE`; el escritorio la atiende
//! con `serve` sobre el sistema real y devuelve la respuesta. `RemoteFiles` es el lado del programa:
//! un `FilesApi` que hace ese viaje en cada método. Respuesta = byte 0 + valor (o byte 1 si la
//! petición estaba rota).
//!
//! Política (`serve`): un programa tiene los permisos del usuario de la sesión (los aplica `Files`) y
//! además
//! - nunca escribe ni borra bajo `C:\Sistema` (ni mueve o copia desde o hacia ahí);
//! - cuentas, claves, cifrado, registro, reiniciar y apagar solo valen si `system` (el programa es,
//!   byte a byte, uno de los del sistema);
//! - no hay llamada para instalar programas del sistema;
//! - la fecha, la sal y el acumulador de aleatoriedad los pone siempre el sistema.
use crate::accounts::{salt_from, Account, AccountError};
use crate::files::{in_system, Files, TrashItem};
use crate::fs::{Entry, FsError};
use crate::fsapi::FilesApi;
use crate::keys::{KeyError, Pool};
use crate::platform::{DateTime, MemInfo, Platform};
use crate::wire::{decode, encode, Bad, Reader, Wire, WireResult, Writer};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Bytes tal cual (mismo formato que `Vec<u8>`, sin pasar byte a byte).
pub struct Blob(pub Vec<u8>);

impl Wire for Blob {
    fn put(&self, w: &mut Writer) {
        w.bytes(&self.0);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(Blob(r.bytes()?.to_vec()))
    }
}

macro_rules! calls {
    ($($v:ident $({ $($f:ident : $t:ty),* })? = $tag:literal,)*) => {
        /// Una petición al sistema: una por método de `FilesApi` (sin las fechas, sales ni
        /// acumuladores, que pone el sistema) y las del equipo.
        pub enum Call { $($v $({ $($f: $t),* })?),* }

        impl Wire for Call {
            fn put(&self, w: &mut Writer) {
                match self {
                    $(Call::$v $({ $($f),* })? => {
                        w.put(&($tag as u8));
                        $($(w.put($f);)*)?
                    })*
                }
            }
            fn get(r: &mut Reader) -> WireResult<Self> {
                Ok(match u8::get(r)? {
                    $($tag => Call::$v $({ $($f: r.get()?),* })?,)*
                    _ => return Err(Bad),
                })
            }
        }
    };
}

calls! {
    Persistent = 0,
    Generation = 1,
    List { dir: String } = 2,
    Stat { path: String } = 3,
    Exists { path: String } = 4,
    Read { path: String } = 5,
    Write { path: String, data: Blob } = 6,
    Mkdir { path: String } = 7,
    Rename { from: String, to: String } = 8,
    Copy { from: String, to: String } = 9,
    Delete { path: String } = 10,
    FreeName { dir: String, name: String } = 11,
    TrashList = 12,
    Restore { id: String } = 13,
    EmptyTrash = 14,
    Space = 15,
    SetClipMany { paths: Vec<String>, cut: bool } = 16,
    PasteAll { dir: String } = 17,
    Transfer { paths: Vec<String>, dir: String, mv: bool } = 18,
    UserRename { from: String, to: String } = 19,
    UserTrashMany { paths: Vec<String> } = 20,
    UserCreate { path: String, dir: bool } = 21,
    Undo = 22,
    Log { text: String } = 23,
    Home = 24,
    ExplorerPrefs = 25,
    Clip = 26,
    ClipPaths = 27,
    User = 28,
    UndoLabel = 29,
    Accounts = 30,
    IsAdmin = 31,
    VerifyPassword { password: String } = 32,
    HasPassword = 33,
    CreateAccount { name: String, password: Option<String>, admin: bool } = 34,
    DeleteAccount { name: String, keep_files: bool } = 35,
    RenameAccount { old: String, new: String } = 36,
    SetAdmin { name: String, admin: bool } = 37,
    SetPassword { old: String, new: Option<String> } = 38,
    DiskEncrypted = 39,
    DiskConversion = 40,
    ConvertStep { blocks: u32 } = 41,
    HomeEncrypted = 42,
    HomeConversion = 43,
    HomeConvertStep { files: u32 } = 44,
    EncryptDisk { password: String } = 45,
    DecryptDisk { password: String } = 46,
    ChangeDiskPassword { old: String, new: String } = 47,
    NewDiskRecovery { password: String } = 48,
    EncryptHome { password: String } = 49,
    DecryptHome { password: String } = 50,
    NewHomeRecovery { password: String } = 51,
    Mem = 52,
    Cpu = 53,
    Screen = 54,
    Entropy = 55,
    Reboot = 56,
    Shutdown = 57,
}

// ---------- Wire de los tipos que viajan ----------

impl<T: Wire, E: Wire> Wire for Result<T, E> {
    fn put(&self, w: &mut Writer) {
        match self {
            Ok(v) => {
                w.put(&0u8);
                w.put(v);
            }
            Err(e) => {
                w.put(&1u8);
                w.put(e);
            }
        }
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        match u8::get(r)? {
            0 => Ok(Ok(r.get()?)),
            1 => Ok(Err(r.get()?)),
            _ => Err(Bad),
        }
    }
}

const FS_ERRORS: [FsError; 12] = [
    FsError::NotFound,
    FsError::Exists,
    FsError::NotDir,
    FsError::IsDir,
    FsError::NotEmpty,
    FsError::NoSpace,
    FsError::BadName,
    FsError::BadPath,
    FsError::Disk,
    FsError::Corrupt,
    FsError::Invalid,
    FsError::Denied,
];

impl Wire for FsError {
    fn put(&self, w: &mut Writer) {
        w.put(&(FS_ERRORS.iter().position(|e| e == self).unwrap_or(8) as u8));
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        FS_ERRORS.get(u8::get(r)? as usize).copied().ok_or(Bad)
    }
}

const ACCOUNT_ERRORS: [AccountError; 9] = [
    AccountError::BadName,
    AccountError::Exists,
    AccountError::NotFound,
    AccountError::WrongPassword,
    AccountError::WeakPassword,
    AccountError::NotAdmin,
    AccountError::LastAdmin,
    AccountError::InUse,
    AccountError::Encrypted,
];

impl Wire for AccountError {
    fn put(&self, w: &mut Writer) {
        match self {
            AccountError::Fs(e) => {
                w.put(&255u8);
                w.put(e);
            }
            e => {
                w.put(&(ACCOUNT_ERRORS.iter().position(|x| x == e).unwrap_or(0) as u8));
            }
        }
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        match u8::get(r)? {
            255 => Ok(AccountError::Fs(r.get()?)),
            n => ACCOUNT_ERRORS.get(n as usize).copied().ok_or(Bad),
        }
    }
}

const KEY_ERRORS: [KeyError; 9] = [
    KeyError::WrongSecret,
    KeyError::Weak,
    KeyError::NotEncrypted,
    KeyError::AlreadyEncrypted,
    KeyError::NotAdmin,
    KeyError::NoRoom,
    KeyError::Busy,
    KeyError::Unavailable,
    KeyError::NotReady,
];

impl Wire for KeyError {
    fn put(&self, w: &mut Writer) {
        match self {
            KeyError::Fs(e) => {
                w.put(&255u8);
                w.put(e);
            }
            e => {
                w.put(&(KEY_ERRORS.iter().position(|x| x == e).unwrap_or(4) as u8));
            }
        }
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        match u8::get(r)? {
            255 => Ok(KeyError::Fs(r.get()?)),
            n => KEY_ERRORS.get(n as usize).copied().ok_or(Bad),
        }
    }
}

impl Wire for Entry {
    fn put(&self, w: &mut Writer) {
        w.put(&self.name).put(&self.dir).put(&self.size).put(&self.modified);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(Entry { name: r.get()?, dir: r.get()?, size: r.get()?, modified: r.get()? })
    }
}

impl Wire for TrashItem {
    fn put(&self, w: &mut Writer) {
        w.put(&self.id).put(&self.name).put(&self.original).put(&self.dir).put(&self.size).put(&self.deleted);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(TrashItem { id: r.get()?, name: r.get()?, original: r.get()?, dir: r.get()?, size: r.get()?, deleted: r.get()? })
    }
}

/// Solo viajan el nombre y el tipo: la sal y la marca de la clave no salen del sistema, así que
/// `has_password` de una cuenta recibida no vale (para eso está `FilesApi::has_password`).
impl Wire for Account {
    fn put(&self, w: &mut Writer) {
        w.put(&self.name).put(&self.admin);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        let name: String = r.get()?;
        Ok(Account::new(&name, r.get()?))
    }
}

impl Wire for MemInfo {
    fn put(&self, w: &mut Writer) {
        w.put(&self.total).put(&self.heap_total).put(&self.heap_used);
    }
    fn get(r: &mut Reader) -> WireResult<Self> {
        Ok(MemInfo { total: r.get()?, heap_total: r.get()?, heap_used: r.get()? })
    }
}

// ---------- Lado del sistema ----------

/// Atiende una petición. Respuesta: `0` + el valor, o `1` si no se entiende (nunca un pánico).
pub fn serve(sys: &mut dyn Platform, req: &[u8], system: bool) -> Vec<u8> {
    let Ok(call) = decode::<Call>(req) else { return vec![1] };
    let mut out = vec![0];
    out.extend(run(sys, call, system));
    out
}

/// Ejecuta `f` si `ok`; si no, devuelve `deny` (el error de permiso o el valor neutro).
fn gate<T: Wire>(ok: bool, deny: T, f: impl FnOnce() -> T) -> Vec<u8> {
    encode(&if ok { f() } else { deny })
}

/// Ninguna de las rutas está en `C:\Sistema`.
fn outside(paths: &[&str]) -> bool {
    !paths.iter().any(|p| in_system(p))
}

fn with_pool<R>(sys: &mut dyn Platform, f: impl FnOnce(&mut Files, &mut Pool) -> R) -> R {
    let mut pool = core::mem::take(sys.pool());
    let r = f(sys.files(), &mut pool);
    *sys.pool() = pool;
    r
}

fn salt(sys: &mut dyn Platform) -> [u8; 16] {
    let (a, b) = (sys.entropy(), sys.entropy());
    salt_from(a, b)
}

fn run(sys: &mut dyn Platform, call: Call, system: bool) -> Vec<u8> {
    use Call::*;
    let denied = FsError::Denied;
    let no_admin = AccountError::NotAdmin;
    let no_key = KeyError::NotAdmin;
    match call {
        Mem => encode(&sys.mem()),
        Cpu => encode(&sys.cpu()),
        Screen => encode(&sys.screen()),
        Entropy => encode(&sys.entropy()),
        Reboot => gate(system, (), || sys.reboot()),
        Shutdown => gate(system, (), || sys.shutdown()),
        Persistent => encode(&sys.files().persistent()),
        Generation => encode(&sys.files().generation()),
        List { dir } => encode(&sys.files().list(&dir)),
        Stat { path } => encode(&sys.files().stat(&path)),
        Exists { path } => encode(&sys.files().exists(&path)),
        Read { path } => encode(&sys.files().read(&path).map(Blob)),
        Write { path, data } => {
            let now = sys.now();
            gate(outside(&[&path]), Err(denied), || sys.files().write(&path, &data.0, now))
        }
        Mkdir { path } => {
            let now = sys.now();
            gate(outside(&[&path]), Err(denied), || sys.files().mkdir(&path, now))
        }
        Rename { from, to } => gate(outside(&[&from, &to]), Err(denied), || sys.files().rename(&from, &to)),
        Copy { from, to } => {
            let now = sys.now();
            gate(outside(&[&from, &to]), Err(denied), || sys.files().copy(&from, &to, now))
        }
        Delete { path } => gate(outside(&[&path]), Err(denied), || sys.files().delete(&path)),
        FreeName { dir, name } => encode(&sys.files().free_name(&dir, &name)),
        TrashList => encode(&sys.files().trash_list()),
        Restore { id } => {
            // Restaura donde estaba: si estaba en el sistema, no.
            let files = sys.files();
            let into_system = files.trash_list().is_ok_and(|l| l.iter().any(|t| t.id == id && in_system(&t.original)));
            gate(!into_system, Err(denied), || files.restore(&id))
        }
        EmptyTrash => encode(&sys.files().empty_trash()),
        Space => encode(&sys.files().space()),
        SetClipMany { paths, cut } => encode(&sys.files().set_clip_many(&paths, cut)),
        PasteAll { dir } => {
            let now = sys.now();
            let files = sys.files();
            let from_system = files.clip_paths().0.iter().any(|p| in_system(p));
            gate(outside(&[&dir]) && !from_system, Err(denied), || files.paste_all(&dir, now))
        }
        Transfer { paths, dir, mv } => {
            let now = sys.now();
            let ok = outside(&[&dir]) && !paths.iter().any(|p| in_system(p));
            gate(ok, Err(denied), || sys.files().transfer(&paths, &dir, mv, now))
        }
        UserRename { from, to } => gate(outside(&[&from, &to]), Err(denied), || sys.files().user_rename(&from, &to)),
        UserTrashMany { paths } => {
            let now = sys.now();
            gate(!paths.iter().any(|p| in_system(p)), Err(denied), || sys.files().user_trash_many(&paths, now))
        }
        UserCreate { path, dir } => {
            let now = sys.now();
            gate(outside(&[&path]), Err(denied), || sys.files().user_create(&path, dir, now))
        }
        Undo => {
            let now = sys.now();
            encode(&sys.files().undo(now))
        }
        Log { text } => {
            let now = sys.now();
            gate(system, (), || sys.files().log(&text, now))
        }
        Home => encode(&sys.files().home()),
        ExplorerPrefs => encode(&sys.files().explorer_prefs()),
        Clip => encode(&FilesApi::clip(sys.files())),
        ClipPaths => encode(&FilesApi::clip_paths(sys.files())),
        User => encode(&FilesApi::user(sys.files())),
        UndoLabel => encode(&sys.files().undo_label().map(String::from)),
        IsAdmin => encode(&sys.files().is_admin()),
        // ---- Cuentas y claves: solo programas del sistema ----
        Accounts => gate(system, Vec::new(), || sys.files().accounts()),
        VerifyPassword { password } => gate(system, false, || sys.files().verify_password(&password)),
        HasPassword => gate(system, false, || sys.files().has_password()),
        CreateAccount { name, password, admin } => {
            let (now, salt) = (sys.now(), salt(sys));
            gate(system, Err(no_admin), || sys.files().create_account(&name, password.as_deref(), admin, salt, now))
        }
        DeleteAccount { name, keep_files } => {
            let now = sys.now();
            gate(system, Err(no_admin), || sys.files().delete_account(&name, keep_files, now))
        }
        RenameAccount { old, new } => {
            let now = sys.now();
            gate(system, Err(no_admin), || sys.files().rename_account(&old, &new, now))
        }
        SetAdmin { name, admin } => {
            let now = sys.now();
            gate(system, Err(no_admin), || sys.files().set_admin(&name, admin, now))
        }
        SetPassword { old, new } => {
            let (now, salt) = (sys.now(), salt(sys));
            gate(system, Err(no_admin), || sys.files().set_password(&old, new.as_deref(), salt, now))
        }
        // ---- Cifrado: solo programas del sistema ----
        DiskEncrypted => gate(system, false, || sys.files().disk_encrypted()),
        DiskConversion => gate(system, None, || sys.files().disk_conversion()),
        ConvertStep { blocks } => {
            let now = sys.now();
            gate(system, Err(no_key), || sys.files().convert_step(blocks, now))
        }
        HomeEncrypted => gate(system, false, || sys.files().home_encrypted()),
        HomeConversion => gate(system, None, || sys.files().home_conversion()),
        HomeConvertStep { files } => {
            let now = sys.now();
            gate(system, Err(no_key), || sys.files().home_convert_step(files, now))
        }
        EncryptDisk { password } => {
            let now = sys.now();
            gate(system, Err(no_key), || with_pool(sys, |f, pool| f.encrypt_disk(&password, pool, now)))
        }
        DecryptDisk { password } => {
            let now = sys.now();
            gate(system, Err(no_key), || sys.files().decrypt_disk(&password, now))
        }
        ChangeDiskPassword { old, new } => {
            let now = sys.now();
            gate(system, Err(no_key), || with_pool(sys, |f, pool| f.change_disk_password(&old, &new, pool, now)))
        }
        NewDiskRecovery { password } => {
            let now = sys.now();
            gate(system, Err(no_key), || with_pool(sys, |f, pool| f.new_disk_recovery(&password, pool, now)))
        }
        EncryptHome { password } => {
            let now = sys.now();
            gate(system, Err(no_key), || with_pool(sys, |f, pool| f.encrypt_home(&password, pool, now)))
        }
        DecryptHome { password } => {
            let now = sys.now();
            gate(system, Err(no_key), || sys.files().decrypt_home(&password, now))
        }
        NewHomeRecovery { password } => {
            let now = sys.now();
            gate(system, Err(no_key), || with_pool(sys, |f, pool| f.new_home_recovery(&password, pool, now)))
        }
    }
}

// ---------- Lado del programa ----------

/// Cómo llega una petición al sistema (en un programa, `SYS_SERVICE` + `SYS_TAKE_REPLY`).
pub trait Transport {
    fn call(&mut self, req: &[u8]) -> Vec<u8>;
}

/// `FilesApi` que lo pide todo al sistema. Con una respuesta rota o perdida devuelve un error de
/// E/S (o el valor neutro): nunca entra en pánico.
pub struct RemoteFiles<T: Transport>(pub T);

const UNDO_LABELS: [&str; 5] = ["Cambiar nombre", "Mover", "Copiar", "Nuevo", "Eliminar"];

impl<T: Transport> RemoteFiles<T> {
    /// Hace la petición; `bad` es lo que sale si la respuesta no vale.
    pub fn ask<R: Wire>(&mut self, c: Call, bad: R) -> R {
        match self.0.call(&encode(&c)).split_first() {
            Some((0, rest)) => decode(rest).unwrap_or(bad),
            _ => bad,
        }
    }
    fn fs<R: Wire>(&mut self, c: Call) -> Result<R, FsError> {
        self.ask(c, Err(FsError::Disk))
    }
    fn acc<R: Wire>(&mut self, c: Call) -> Result<R, AccountError> {
        self.ask(c, Err(AccountError::Fs(FsError::Disk)))
    }
    fn key<R: Wire>(&mut self, c: Call) -> Result<R, KeyError> {
        self.ask(c, Err(KeyError::Fs(FsError::Disk)))
    }
}

fn s(x: &str) -> String {
    String::from(x)
}

impl<T: Transport> FilesApi for RemoteFiles<T> {
    fn persistent(&mut self) -> bool {
        self.ask(Call::Persistent, false)
    }
    fn generation(&mut self) -> u64 {
        self.ask(Call::Generation, 0)
    }
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, FsError> {
        self.fs(Call::List { dir: s(dir) })
    }
    fn stat(&mut self, path: &str) -> Result<Entry, FsError> {
        self.fs(Call::Stat { path: s(path) })
    }
    fn exists(&mut self, path: &str) -> bool {
        self.ask(Call::Exists { path: s(path) }, false)
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        self.fs::<Blob>(Call::Read { path: s(path) }).map(|b| b.0)
    }
    fn write(&mut self, path: &str, data: &[u8], _now: DateTime) -> Result<(), FsError> {
        self.fs(Call::Write { path: s(path), data: Blob(data.to_vec()) })
    }
    fn mkdir(&mut self, path: &str, _now: DateTime) -> Result<(), FsError> {
        self.fs(Call::Mkdir { path: s(path) })
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        self.fs(Call::Rename { from: s(from), to: s(to) })
    }
    fn copy(&mut self, from: &str, to: &str, _now: DateTime) -> Result<(), FsError> {
        self.fs(Call::Copy { from: s(from), to: s(to) })
    }
    fn delete(&mut self, path: &str) -> Result<(), FsError> {
        self.fs(Call::Delete { path: s(path) })
    }
    fn free_name(&mut self, dir: &str, name: &str) -> String {
        self.ask(Call::FreeName { dir: s(dir), name: s(name) }, s(name))
    }
    fn trash_list(&mut self) -> Result<Vec<TrashItem>, FsError> {
        self.fs(Call::TrashList)
    }
    fn restore(&mut self, id: &str) -> Result<String, FsError> {
        self.fs(Call::Restore { id: s(id) })
    }
    fn empty_trash(&mut self) -> Result<(), FsError> {
        self.fs(Call::EmptyTrash)
    }
    fn space(&mut self) -> (u64, u64) {
        self.ask(Call::Space, (0, 0))
    }
    fn set_clip_many(&mut self, paths: &[String], cut: bool) {
        self.ask(Call::SetClipMany { paths: paths.to_vec(), cut }, ())
    }
    fn paste_all(&mut self, dir: &str, _now: DateTime) -> Result<Vec<String>, FsError> {
        self.fs(Call::PasteAll { dir: s(dir) })
    }
    fn transfer(&mut self, paths: &[String], dir: &str, mv: bool, _now: DateTime) -> Result<Vec<String>, FsError> {
        self.fs(Call::Transfer { paths: paths.to_vec(), dir: s(dir), mv })
    }
    fn user_rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        self.fs(Call::UserRename { from: s(from), to: s(to) })
    }
    fn user_trash_many(&mut self, paths: &[String], _now: DateTime) -> Result<(), FsError> {
        self.fs(Call::UserTrashMany { paths: paths.to_vec() })
    }
    fn user_create(&mut self, path: &str, dir: bool, _now: DateTime) -> Result<(), FsError> {
        self.fs(Call::UserCreate { path: s(path), dir })
    }
    fn undo(&mut self, _now: DateTime) -> Result<(), FsError> {
        self.fs(Call::Undo)
    }
    fn log(&mut self, text: &str, _now: DateTime) {
        self.ask(Call::Log { text: s(text) }, ())
    }
    fn home(&mut self) -> String {
        self.ask(Call::Home, String::new())
    }
    fn explorer_prefs(&mut self) -> String {
        self.ask(Call::ExplorerPrefs, String::new())
    }
    fn clip(&mut self) -> Option<(String, bool)> {
        self.ask(Call::Clip, None)
    }
    fn clip_paths(&mut self) -> (Vec<String>, bool) {
        self.ask(Call::ClipPaths, (Vec::new(), false))
    }
    fn user(&mut self) -> Option<String> {
        self.ask(Call::User, None)
    }
    fn undo_label(&mut self) -> Option<&'static str> {
        let l: Option<String> = self.ask(Call::UndoLabel, None);
        l.and_then(|l| UNDO_LABELS.iter().copied().find(|x| *x == l))
    }
    fn accounts(&mut self) -> Vec<Account> {
        self.ask(Call::Accounts, Vec::new())
    }
    fn is_admin(&mut self) -> bool {
        self.ask(Call::IsAdmin, false)
    }
    fn verify_password(&mut self, password: &str) -> bool {
        self.ask(Call::VerifyPassword { password: s(password) }, false)
    }
    fn has_password(&mut self) -> bool {
        self.ask(Call::HasPassword, false)
    }
    fn create_account(&mut self, name: &str, password: Option<&str>, admin: bool, _salt: [u8; 16], _now: DateTime) -> Result<(), AccountError> {
        self.acc(Call::CreateAccount { name: s(name), password: password.map(s), admin })
    }
    fn delete_account(&mut self, name: &str, keep_files: bool, _now: DateTime) -> Result<(), AccountError> {
        self.acc(Call::DeleteAccount { name: s(name), keep_files })
    }
    fn rename_account(&mut self, old: &str, new: &str, _now: DateTime) -> Result<(), AccountError> {
        self.acc(Call::RenameAccount { old: s(old), new: s(new) })
    }
    fn set_admin(&mut self, name: &str, admin: bool, _now: DateTime) -> Result<(), AccountError> {
        self.acc(Call::SetAdmin { name: s(name), admin })
    }
    fn set_password(&mut self, old: &str, new: Option<&str>, _salt: [u8; 16], _now: DateTime) -> Result<(), AccountError> {
        self.acc(Call::SetPassword { old: s(old), new: new.map(s) })
    }
    fn disk_encrypted(&mut self) -> bool {
        self.ask(Call::DiskEncrypted, false)
    }
    fn disk_conversion(&mut self) -> Option<(u64, u64, bool)> {
        self.ask(Call::DiskConversion, None)
    }
    fn convert_step(&mut self, blocks: u32, _now: DateTime) -> Result<bool, KeyError> {
        self.key(Call::ConvertStep { blocks })
    }
    fn home_encrypted(&mut self) -> bool {
        self.ask(Call::HomeEncrypted, false)
    }
    fn home_conversion(&mut self) -> Option<(u64, u64, bool)> {
        self.ask(Call::HomeConversion, None)
    }
    fn home_convert_step(&mut self, files: u32, _now: DateTime) -> Result<bool, KeyError> {
        self.key(Call::HomeConvertStep { files })
    }
    fn encrypt_disk(&mut self, password: &str, _pool: &mut Pool, _now: DateTime) -> Result<String, KeyError> {
        self.key(Call::EncryptDisk { password: s(password) })
    }
    fn decrypt_disk(&mut self, password: &str, _now: DateTime) -> Result<(), KeyError> {
        self.key(Call::DecryptDisk { password: s(password) })
    }
    fn change_disk_password(&mut self, old: &str, new: &str, _pool: &mut Pool, _now: DateTime) -> Result<(), KeyError> {
        self.key(Call::ChangeDiskPassword { old: s(old), new: s(new) })
    }
    fn new_disk_recovery(&mut self, password: &str, _pool: &mut Pool, _now: DateTime) -> Result<String, KeyError> {
        self.key(Call::NewDiskRecovery { password: s(password) })
    }
    fn encrypt_home(&mut self, password: &str, _pool: &mut Pool, _now: DateTime) -> Result<String, KeyError> {
        self.key(Call::EncryptHome { password: s(password) })
    }
    fn decrypt_home(&mut self, password: &str, _now: DateTime) -> Result<(), KeyError> {
        self.key(Call::DecryptHome { password: s(password) })
    }
    fn new_home_recovery(&mut self, password: &str, _pool: &mut Pool, _now: DateTime) -> Result<String, KeyError> {
        self.key(Call::NewHomeRecovery { password: s(password) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::{self, AppKind, Ctx};
    use crate::dialogs::{Answer, DialogRequest, DialogResult};
    use crate::input::{Key, KeyEvent};
    use crate::platform::TestPlatform;
    use crate::surface::Surface;
    use crate::theme::Theme;
    use alloc::format;
    use alloc::rc::Rc;
    use core::cell::RefCell;

    const DOCS: &str = "C:\\Usuarios\\Usuario\\Documentos";
    const PICS: &str = "C:\\Usuarios\\Usuario\\Imágenes";
    const NOW: DateTime = DateTime { year: 2026, month: 11, day: 18, hour: 1, minute: 2, second: 3 };
    /// La fecha que pone el sistema (la de `TestPlatform`), no la que mande el programa.
    const NOW_REAL: DateTime = DateTime { year: 2026, month: 11, day: 15, hour: 21, minute: 37, second: 5 };

    type Real = Rc<RefCell<TestPlatform>>;

    /// Transporte de prueba: llama directamente a `serve` sobre la plataforma de verdad.
    struct Via {
        real: Real,
        system: bool,
    }

    impl Transport for Via {
        fn call(&mut self, req: &[u8]) -> Vec<u8> {
            serve(&mut *self.real.borrow_mut(), req, self.system)
        }
    }

    /// Plataforma de un programa: lo suyo es local y los archivos van por `RemoteFiles`.
    struct Prog {
        real: Real,
        fs: RemoteFiles<Via>,
        dummy: Files,
        pool: Pool,
    }

    impl Platform for Prog {
        fn now(&mut self) -> DateTime {
            self.real.borrow_mut().now()
        }
        fn uptime_ms(&self) -> u64 {
            0
        }
        fn mem(&self) -> MemInfo {
            self.real.borrow().mem()
        }
        fn cpu(&self) -> String {
            self.real.borrow().cpu()
        }
        fn screen(&self) -> (u32, u32) {
            (1280, 800)
        }
        fn reboot(&mut self) {}
        fn shutdown(&mut self) {}
        fn files(&mut self) -> &mut Files {
            &mut self.dummy
        }
        fn fs(&mut self) -> &mut dyn FilesApi {
            &mut self.fs
        }
        fn entropy(&mut self) -> u64 {
            self.real.borrow_mut().entropy()
        }
        fn pool(&mut self) -> &mut Pool {
            &mut self.pool
        }
    }

    fn prog(system: bool) -> (Real, Prog) {
        let real: Real = Rc::new(RefCell::new(TestPlatform::default()));
        let p = Prog { real: real.clone(), fs: RemoteFiles(Via { real: real.clone(), system }), dummy: Files::default(), pool: Pool::new() };
        (real, p)
    }

    fn tecla(app: &mut Box<dyn apps::App>, p: &mut Prog, key: Key, ch: Option<char>, ctrl: bool) -> Option<DialogRequest> {
        let mut th = Theme::default();
        let mut cx = Ctx::new(p, &mut th, 0);
        app.key(&KeyEvent { key, ch, down: true, shift: false, ctrl, alt: false }, &mut cx);
        cx.dialog.take()
    }

    fn dibuja(app: &mut Box<dyn apps::App>, p: &mut Prog) {
        app.draw(&mut Surface::new(1000, 520, 0), &Theme::default(), p, true);
    }

    fn abre(app: &mut Box<dyn apps::App>, p: &mut Prog, ruta: &str) {
        let mut th = Theme::default();
        let mut cx = Ctx::new(p, &mut th, 0);
        app.open_path(ruta, &mut cx);
        dibuja(app, p);
    }

    #[test]
    fn mensajes_rotos_no_hacen_panico() {
        let mut sys = TestPlatform::default();
        assert_eq!(serve(&mut sys, &[], true), [1]);
        assert_eq!(serve(&mut sys, &[250], true), [1], "llamada que no existe");
        assert_eq!(serve(&mut sys, &[2, 255, 255, 255, 255], true), [1], "longitud que miente");
        assert_eq!(serve(&mut sys, &[15, 0], true), [1], "sobra un byte");
        // Respuestas rotas al otro lado: error de E/S o valor neutro.
        struct Roto(Vec<u8>);
        impl Transport for Roto {
            fn call(&mut self, _: &[u8]) -> Vec<u8> {
                self.0.clone()
            }
        }
        for r in [vec![], vec![1], vec![0], vec![0, 9, 9, 9], vec![7, 0]] {
            let mut f = RemoteFiles(Roto(r));
            assert_eq!(f.read("C:\\a"), Err(FsError::Disk));
            assert_eq!(f.write("C:\\a", b"x", NOW), Err(FsError::Disk));
            assert!(!f.exists("C:\\a") && !f.is_admin() && f.accounts().is_empty() && f.user().is_none());
            assert_eq!(f.encrypt_home("x", &mut Pool::new(), NOW), Err(KeyError::Fs(FsError::Disk)));
        }
    }

    #[test]
    fn tipos_ida_y_vuelta() {
        let e = Entry { name: s("ñ.txt"), dir: false, size: 7, modified: NOW };
        assert_eq!(decode::<Result<Entry, FsError>>(&encode(&Ok::<_, FsError>(e.clone()))), Ok(Ok(e)));
        for fe in FS_ERRORS {
            assert_eq!(decode::<FsError>(&encode(&fe)), Ok(fe));
        }
        for ae in ACCOUNT_ERRORS.into_iter().chain([AccountError::Fs(FsError::Denied)]) {
            assert_eq!(decode::<AccountError>(&encode(&ae)), Ok(ae));
        }
        for ke in KEY_ERRORS.into_iter().chain([KeyError::Fs(FsError::Corrupt)]) {
            assert_eq!(decode::<KeyError>(&encode(&ke)), Ok(ke));
        }
        let t = TrashItem { id: s("1"), name: s("a"), original: s("C:\\a"), dir: true, size: 3, deleted: NOW };
        assert_eq!(decode::<TrashItem>(&encode(&t)), Ok(t));
        let c = Call::Transfer { paths: vec![s("a"), s("b")], dir: s("C:\\d"), mv: true };
        let Ok(Call::Transfer { paths, dir, mv }) = decode::<Call>(&encode(&c)) else { panic!() };
        assert_eq!((paths.len(), dir.as_str(), mv), (2, "C:\\d", true));
        assert!(decode::<FsError>(&[99]).is_err() && decode::<AccountError>(&[99]).is_err());
    }

    #[test]
    fn bloc_de_notas_crea_guarda_y_lee_por_el_servicio() {
        let (real, mut p) = prog(false);
        let mut np = apps::create(AppKind::Notepad);
        tecla(&mut np, &mut p, Key::Char('h'), Some('h'), false);
        tecla(&mut np, &mut p, Key::Char('i'), Some('i'), false);
        let Some(DialogRequest::Save { tag, .. }) = tecla(&mut np, &mut p, Key::Char('s'), Some('s'), true) else { panic!("sin Guardar como") };
        let ruta = format!("{DOCS}\\hola.txt");
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut p, &mut th, 0);
        np.dialog_result(DialogResult::Path { tag, path: Some(ruta.clone()) }, &mut cx);
        assert!(cx.dialog.is_none(), "se guardó sin error");
        // Está en el sistema de archivos de verdad, con la fecha del sistema.
        assert_eq!(real.borrow_mut().files.read(&ruta).unwrap(), b"hi");
        assert_eq!(real.borrow_mut().files.stat(&ruta).unwrap().modified, NOW_REAL);
        // Otro Bloc de notas lo abre.
        let mut otro = apps::create(AppKind::Notepad);
        otro.open_path(&ruta, &mut cx);
        assert_eq!(otro.title(), "hola.txt: Bloc de notas");
        // Y escribir en el sistema no.
        let r = cx.sys.fs().write("C:\\Sistema\\config\\x", b"x", NOW);
        assert_eq!(r, Err(FsError::Denied));
    }

    #[test]
    fn explorador_lista_copia_y_manda_a_la_papelera_por_el_servicio() {
        let (real, mut p) = prog(false);
        real.borrow_mut().files.write(&format!("{DOCS}\\nota.txt"), b"hola", NOW).unwrap();
        let mut ex = apps::create(AppKind::Files);
        abre(&mut ex, &mut p, DOCS);
        assert!(p.fs().list(DOCS).unwrap().iter().any(|e| e.name == "nota.txt"));
        // Seleccionar todo y copiar: el portapapeles está en el sistema.
        tecla(&mut ex, &mut p, Key::Char('a'), None, true);
        tecla(&mut ex, &mut p, Key::Char('c'), None, true);
        assert_eq!(real.borrow_mut().files.clip_paths().0, [format!("{DOCS}\\nota.txt")]);
        abre(&mut ex, &mut p, PICS);
        tecla(&mut ex, &mut p, Key::Char('v'), None, true);
        assert!(real.borrow_mut().files.exists(&format!("{PICS}\\nota.txt")), "copiado");
        assert!(real.borrow_mut().files.exists(&format!("{DOCS}\\nota.txt")), "el original sigue");
        // Lo pegado queda seleccionado: Suprimir lo manda a la Papelera tras confirmar.
        dibuja(&mut ex, &mut p);
        let Some(DialogRequest::Message { tag, .. }) = tecla(&mut ex, &mut p, Key::Delete, None, false) else { panic!("sin confirmación") };
        let mut th = Theme::default();
        let mut cx = Ctx::new(&mut p, &mut th, 0);
        ex.dialog_result(DialogResult::Answer { tag, answer: Answer::Yes }, &mut cx);
        assert!(!real.borrow_mut().files.exists(&format!("{PICS}\\nota.txt")));
        let t = real.borrow_mut().files.trash_list().unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].original, format!("{PICS}\\nota.txt"));
    }

    #[test]
    fn un_programa_no_toca_el_sistema() {
        let (real, mut p) = prog(false);
        {
            let mut r = real.borrow_mut();
            r.files.write("C:\\Sistema\\config\\ajuste", b"1", NOW).unwrap();
            r.files.write(&format!("{DOCS}\\a.txt"), b"a", NOW).unwrap();
            r.files.write("C:\\Sistema\\temp\\viejo", b"v", NOW).unwrap();
            r.files.trash("C:\\Sistema\\temp\\viejo", NOW).unwrap();
        }
        let f = p.fs();
        let d = FsError::Denied;
        // Con rutas disfrazadas (mayúsculas, "..", "/") también.
        for sis in ["C:\\Sistema\\config\\nuevo", "c:\\SISTEMA\\x", "C:\\Usuarios\\..\\Sistema\\x", "C:/Sistema/x", "\\sistema\\x"] {
            assert_eq!(f.write(sis, b"x", NOW), Err(d), "{sis}");
            assert_eq!(f.mkdir(sis, NOW), Err(d), "{sis}");
            assert_eq!(f.delete(sis), Err(d), "{sis}");
            assert_eq!(f.user_create(sis, false, NOW), Err(d), "{sis}");
            assert_eq!(f.rename(&format!("{DOCS}\\a.txt"), sis), Err(d), "{sis}");
            assert_eq!(f.copy(&format!("{DOCS}\\a.txt"), sis, NOW), Err(d), "{sis}");
            assert_eq!(f.user_trash_many(&[s(sis)], NOW), Err(d), "{sis}");
        }
        assert_eq!(f.rename("C:\\Sistema\\config\\ajuste", &format!("{DOCS}\\b")), Err(d), "desde el sistema");
        assert_eq!(f.copy("C:\\Sistema\\config\\ajuste", &format!("{DOCS}\\b"), NOW), Err(d));
        assert_eq!(f.user_rename("C:\\Sistema\\config\\ajuste", &format!("{DOCS}\\b")), Err(d));
        assert_eq!(f.transfer(&[s("C:\\Sistema\\config\\ajuste")], DOCS, false, NOW), Err(d));
        assert_eq!(f.transfer(&[format!("{DOCS}\\a.txt")], "C:\\Sistema\\config", true, NOW), Err(d));
        // Pegar desde el sistema o hacia él.
        f.set_clip_many(&[s("C:\\Sistema\\config\\ajuste")], false);
        assert_eq!(f.paste_all(DOCS, NOW), Err(d));
        f.set_clip_many(&[format!("{DOCS}\\a.txt")], false);
        assert_eq!(f.paste_all("C:\\Sistema\\config", NOW), Err(d));
        // Restaurar algo que estaba en el sistema.
        let id = f.trash_list().unwrap()[0].id.clone();
        assert_eq!(f.restore(&id), Err(d));
        // Nada de eso cambió el disco; leer sí se puede, y lo suyo también.
        assert_eq!(f.read("C:\\Sistema\\config\\ajuste").unwrap(), b"1");
        assert!(!f.exists("C:\\Sistema\\config\\nuevo") && !f.exists(&format!("{DOCS}\\b")));
        assert_eq!(f.write(&format!("{DOCS}\\c.txt"), b"c", NOW), Ok(()));
        assert_eq!(f.rename(&format!("{DOCS}\\c.txt"), &format!("{DOCS}\\d.txt")), Ok(()));
        assert_eq!(f.transfer(&[format!("{DOCS}\\d.txt")], PICS, false, NOW).unwrap().len(), 1);
        assert!(real.borrow_mut().files.exists(&format!("{PICS}\\d.txt")));
    }

    #[test]
    fn cuentas_claves_y_cifrado_solo_para_el_sistema() {
        let (real, mut p) = prog(false);
        let f = p.fs();
        assert!(f.accounts().is_empty() && !f.verify_password("") && !f.has_password());
        assert_eq!(f.create_account("Marta", None, false, [0; 16], NOW), Err(AccountError::NotAdmin));
        assert_eq!(f.set_password("", Some("clave-nueva"), [0; 16], NOW), Err(AccountError::NotAdmin));
        assert_eq!(f.delete_account("Usuario", false, NOW), Err(AccountError::NotAdmin));
        assert_eq!(f.encrypt_home("", &mut Pool::new(), NOW), Err(KeyError::NotAdmin));
        assert_eq!(f.decrypt_disk("", NOW), Err(KeyError::NotAdmin));
        assert!(!f.disk_encrypted() && f.home_conversion().is_none());
        f.log("intruso", NOW);
        f.set_admin("Usuario", false, NOW).ok();
        assert!(real.borrow_mut().files.is_admin(), "sigue siendo administrador");
        // Reiniciar y apagar tampoco.
        p.fs.ask(Call::Reboot, ());
        p.fs.ask(Call::Shutdown, ());
        assert!(!real.borrow().rebooted && !real.borrow().halted);
        // Lo básico del equipo y de la sesión sí.
        assert_eq!(p.fs.ask(Call::Cpu, String::new()), "CPU de pruebas");
        assert_eq!(p.fs().user().as_deref(), Some("Usuario"));
        assert!(p.fs().is_admin());

        // Un programa del sistema sí; la sal y el acumulador los pone el sistema.
        let (real, mut p) = prog(true);
        let f = p.fs();
        assert_eq!(f.create_account("Ana", None, true, [0; 16], NOW), Ok(()), "la primera cuenta siempre es administradora");
        assert_eq!(f.create_account("Marta", Some("clave-marta"), false, [0; 16], NOW), Ok(()));
        let cuentas = f.accounts();
        assert!(cuentas.iter().any(|a| a.name == "Marta" && !a.admin));
        assert!(f.verify_password("") && !f.verify_password("x"));
        let hay = real.borrow_mut().files.accounts();
        let marta = hay.iter().find(|a| a.name == "Marta").unwrap();
        assert!(marta.verify("clave-marta") && marta.has_password(), "la clave se guardó con una sal del sistema");
        p.fs.ask(Call::Reboot, ());
        p.fs.ask(Call::Shutdown, ());
        assert!(real.borrow().rebooted && real.borrow().halted);
    }
}
