// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Lo que las apps piden a los archivos y a las cuentas (v0.10). Lo implementan `Files` (el
//! sistema, directo) y `RemoteFiles` (un programa en modo usuario, que lo pide por `SYS_SERVICE`;
//! ver `service`). Todo es `&mut self` y devuelve valores propios, porque cada llamada puede ser un
//! viaje al sistema. Las fechas, sales y el acumulador de aleatoriedad que piden algunas llamadas
//! solo valen en `Files`: por el servicio los pone el sistema.
use crate::accounts::{Account, AccountError};
use crate::drives::{DriveInfo, FormatRequest};
use crate::files::{self, Files, Std, TrashItem};
use crate::fs::{Entry, FsError};
use crate::keys::{KeyError, Pool};
use crate::platform::DateTime;
use alloc::string::String;
use alloc::vec::Vec;

/// Declara el trait y la implementación para `Files` (que solo reenvía) a la vez.
macro_rules! api {
    ($($(#[$m:meta])* fn $n:ident(&mut self $(, $a:ident : $t:ty)*) -> $r:ty;)*) => {
        pub trait FilesApi {
            $($(#[$m])* fn $n(&mut self $(, $a: $t)*) -> $r;)*
            /// Carpeta estándar del usuario: `<home>\<nombre>`.
            fn std_dir(&mut self, d: Std) -> String {
                files::join(&self.home(), d.name())
            }
            /// Portapapeles de archivos: primera ruta y si es "cortar".
            fn clip(&mut self) -> Option<(String, bool)>;
            /// Todas las rutas del portapapeles y si es "cortar".
            fn clip_paths(&mut self) -> (Vec<String>, bool);
            /// Cuenta de la sesión abierta.
            fn user(&mut self) -> Option<String>;
            /// Nombre de la última operación que se puede deshacer.
            fn undo_label(&mut self) -> Option<&'static str>;
        }
        impl FilesApi for Files {
            $(fn $n(&mut self $(, $a: $t)*) -> $r { Files::$n(self $(, $a)*) })*
            fn clip(&mut self) -> Option<(String, bool)> {
                Files::clip(self).map(|(p, c)| (String::from(p), c))
            }
            fn clip_paths(&mut self) -> (Vec<String>, bool) {
                let (p, c) = Files::clip_paths(self);
                (p.to_vec(), c)
            }
            fn user(&mut self) -> Option<String> {
                Files::user(self).map(String::from)
            }
            fn undo_label(&mut self) -> Option<&'static str> {
                Files::undo_label(self)
            }
        }
    };
}

api! {
    fn persistent(&mut self) -> bool;
    fn generation(&mut self) -> u64;
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, FsError>;
    fn list_view(&mut self, dir: &str) -> Result<Vec<Entry>, FsError>;
    fn stat(&mut self, path: &str) -> Result<Entry, FsError>;
    fn exists(&mut self, path: &str) -> bool;
    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError>;
    fn write(&mut self, path: &str, data: &[u8], now: DateTime) -> Result<(), FsError>;
    fn mkdir(&mut self, path: &str, now: DateTime) -> Result<(), FsError>;
    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError>;
    fn copy(&mut self, from: &str, to: &str, now: DateTime) -> Result<(), FsError>;
    fn delete(&mut self, path: &str) -> Result<(), FsError>;
    fn free_name(&mut self, dir: &str, name: &str) -> String;
    fn trash_list(&mut self) -> Result<Vec<TrashItem>, FsError>;
    fn restore(&mut self, id: &str) -> Result<String, FsError>;
    fn empty_trash(&mut self) -> Result<(), FsError>;
    fn space(&mut self) -> (u64, u64);
    fn set_clip_many(&mut self, paths: &[String], cut: bool) -> ();
    fn paste_all(&mut self, dir: &str, now: DateTime) -> Result<Vec<String>, FsError>;
    fn transfer(&mut self, paths: &[String], dir: &str, mv: bool, now: DateTime) -> Result<Vec<String>, FsError>;
    fn user_rename(&mut self, from: &str, to: &str) -> Result<(), FsError>;
    fn user_trash_many(&mut self, paths: &[String], now: DateTime) -> Result<(), FsError>;
    fn user_create(&mut self, path: &str, dir: bool, now: DateTime) -> Result<(), FsError>;
    fn undo(&mut self, now: DateTime) -> Result<(), FsError>;
    /// Línea en el registro del sistema.
    fn log(&mut self, text: &str, now: DateTime) -> ();
    fn home(&mut self) -> String;
    fn explorer_prefs(&mut self) -> String;
    // ---- Cuentas y claves (solo los programas del sistema) ----
    fn accounts(&mut self) -> Vec<Account>;
    fn is_admin(&mut self) -> bool;
    fn verify_password(&mut self, password: &str) -> bool;
    fn has_password(&mut self) -> bool;
    fn create_account(&mut self, name: &str, password: Option<&str>, admin: bool, salt: [u8; 16], now: DateTime) -> Result<(), AccountError>;
    fn delete_account(&mut self, name: &str, keep_files: bool, now: DateTime) -> Result<(), AccountError>;
    fn rename_account(&mut self, old: &str, new: &str, now: DateTime) -> Result<(), AccountError>;
    fn set_admin(&mut self, name: &str, admin: bool, now: DateTime) -> Result<(), AccountError>;
    fn set_password(&mut self, old: &str, new: Option<&str>, salt: [u8; 16], now: DateTime) -> Result<(), AccountError>;
    // ---- Cifrado (solo los programas del sistema) ----
    fn disk_encrypted(&mut self) -> bool;
    fn disk_conversion(&mut self) -> Option<(u64, u64, bool)>;
    fn convert_step(&mut self, blocks: u32, now: DateTime) -> Result<bool, KeyError>;
    fn home_encrypted(&mut self) -> bool;
    fn home_conversion(&mut self) -> Option<(u64, u64, bool)>;
    fn home_convert_step(&mut self, files: u32, now: DateTime) -> Result<bool, KeyError>;
    fn encrypt_disk(&mut self, password: &str, pool: &mut Pool, now: DateTime) -> Result<String, KeyError>;
    fn decrypt_disk(&mut self, password: &str, now: DateTime) -> Result<(), KeyError>;
    fn change_disk_password(&mut self, old: &str, new: &str, pool: &mut Pool, now: DateTime) -> Result<(), KeyError>;
    fn new_disk_recovery(&mut self, password: &str, pool: &mut Pool, now: DateTime) -> Result<String, KeyError>;
    fn encrypt_home(&mut self, password: &str, pool: &mut Pool, now: DateTime) -> Result<String, KeyError>;
    fn decrypt_home(&mut self, password: &str, now: DateTime) -> Result<(), KeyError>;
    fn new_home_recovery(&mut self, password: &str, pool: &mut Pool, now: DateTime) -> Result<String, KeyError>;
    // ---- Unidades extraíbles (v0.11): `drives` lo puede pedir cualquiera; lo demás, solo el sistema ----
    fn drives(&mut self) -> Vec<DriveInfo>;
    fn unlock_drive(&mut self, letter: char, secret: &str, recovery: bool) -> Result<(), KeyError>;
    fn request_drive_action(&mut self, letter: char, req: FormatRequest) -> Result<(), FsError>;
    fn drive_result(&mut self) -> Option<Result<Option<String>, FsError>>;
}
