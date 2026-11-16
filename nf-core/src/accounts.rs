//! Cuentas de usuario: formato del archivo de cuentas y comprobación de claves. Lógica pura;
//! quién puede hacer qué y dónde se guarda lo decide `files.rs`.
//!
//! Archivo (`files::ACCOUNTS_FILE`), una cuenta por línea:
//! `nombre|admin o usuario|vueltas|sal en hex|comprobación en hex`
//! Sin clave, los dos últimos campos van vacíos. Nunca se guarda la clave: solo un derivado
//! PBKDF2-HMAC-SHA-256 con sal propia, que no sirve para descifrar nada (lleva la etiqueta
//! `LOGIN`, distinta de la que se usará para las llaves de cifrado).
use crate::crypto::{ct_eq, from_hex, pbkdf2, to_hex, wipe};
use crate::files::valid_name;
use crate::fs::{same, FsError};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Longitud mínima de una clave.
pub const MIN_PASSWORD: usize = 6;
/// Longitud máxima del nombre de una cuenta.
pub const MAX_NAME: usize = 24;
/// Vueltas de PBKDF2 para claves nuevas. En las pruebas se baja para que sean rápidas.
pub const ITERS: u32 = if cfg!(test) { 50 } else { 100_000 };
const LOGIN: &[u8] = b"nightfury-login-v1";

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Account {
    pub name: String,
    pub admin: bool,
    iters: u32,
    salt: [u8; 16],
    check: Option<[u8; 32]>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AccountError {
    /// Nombre no válido (vacío, largo, con caracteres prohibidos o reservado).
    BadName,
    Exists,
    NotFound,
    WrongPassword,
    /// Clave demasiado corta (`MIN_PASSWORD`).
    WeakPassword,
    /// Hace falta ser administrador.
    NotAdmin,
    /// No se puede quitar la última cuenta administradora.
    LastAdmin,
    /// La cuenta tiene la sesión abierta.
    InUse,
    Fs(FsError),
}

impl From<FsError> for AccountError {
    fn from(e: FsError) -> AccountError {
        AccountError::Fs(e)
    }
}

impl AccountError {
    /// Texto para mostrar al usuario.
    pub fn text(self) -> &'static str {
        match self {
            AccountError::BadName => "El nombre no es válido.",
            AccountError::Exists => "Ya existe una cuenta con ese nombre.",
            AccountError::NotFound => "No existe esa cuenta.",
            AccountError::WrongPassword => "La clave no es correcta.",
            AccountError::WeakPassword => "La clave debe tener al menos 6 caracteres.",
            AccountError::NotAdmin => "Hace falta ser administrador.",
            AccountError::LastAdmin => "Tiene que quedar al menos una cuenta administradora.",
            AccountError::InUse => "Esa cuenta tiene la sesión abierta.",
            AccountError::Fs(_) => "No se han podido guardar los cambios en el disco.",
        }
    }
}

/// Nombre de cuenta válido: sirve como nombre de carpeta, sin punto inicial ni `|`, y corto.
pub fn valid_user_name(name: &str) -> bool {
    valid_name(name) && name.chars().count() <= MAX_NAME && !name.starts_with('.') && !name.contains('|') && name.trim() == name
}

/// Sal de 16 bytes a partir de dos lecturas de `Platform::entropy`.
pub fn salt_from(a: u64, b: u64) -> [u8; 16] {
    let mut s = [0u8; 16];
    s[..8].copy_from_slice(&a.to_le_bytes());
    s[8..].copy_from_slice(&b.to_le_bytes());
    s
}

fn derive(password: &str, salt: &[u8; 16], iters: u32) -> [u8; 32] {
    let mut s = [0u8; 16 + LOGIN.len()];
    s[..16].copy_from_slice(salt);
    s[16..].copy_from_slice(LOGIN);
    let mut out = [0u8; 32];
    pbkdf2(password.as_bytes(), &s, iters, &mut out);
    out
}

impl Account {
    /// Cuenta nueva sin clave.
    pub fn new(name: &str, admin: bool) -> Account {
        Account { name: String::from(name), admin, iters: ITERS, salt: [0; 16], check: None }
    }

    pub fn has_password(&self) -> bool {
        self.check.is_some()
    }

    /// `true` si la clave es correcta. Una cuenta sin clave solo acepta la clave vacía.
    pub fn verify(&self, password: &str) -> bool {
        match &self.check {
            None => password.is_empty(),
            Some(c) => {
                let mut d = derive(password, &self.salt, self.iters);
                let ok = ct_eq(&d, c);
                wipe(&mut d);
                ok
            }
        }
    }

    /// Pone (`Some`) o quita (`None`) la clave. `salt` debe ser nueva y aleatoria.
    pub fn set_password(&mut self, password: Option<&str>, salt: [u8; 16]) -> Result<(), AccountError> {
        match password {
            None => self.check = None,
            Some(p) if p.chars().count() < MIN_PASSWORD => return Err(AccountError::WeakPassword),
            Some(p) => {
                self.iters = ITERS;
                self.salt = salt;
                self.check = Some(derive(p, &salt, ITERS));
            }
        }
        Ok(())
    }
}

/// Lee el archivo de cuentas. Las líneas mal formadas se ignoran.
pub fn parse(text: &str) -> Vec<Account> {
    let mut v: Vec<Account> = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.trim().split('|').collect();
        if f.len() != 5 || !valid_user_name(f[0]) || v.iter().any(|a| same(&a.name, f[0])) {
            continue;
        }
        let admin = match f[1] {
            "admin" => true,
            "usuario" => false,
            _ => continue,
        };
        let Ok(iters) = f[2].parse::<u32>() else { continue };
        let (salt, check) = if f[4].is_empty() {
            ([0u8; 16], None)
        } else {
            let (Some(s), Some(c)) = (from_hex(f[3]), from_hex(f[4])) else { continue };
            let (Ok(s), Ok(c)) = (<[u8; 16]>::try_from(s), <[u8; 32]>::try_from(c)) else { continue };
            (s, Some(c))
        };
        v.push(Account { name: String::from(f[0]), admin, iters, salt, check });
    }
    v
}

pub fn serialize(accounts: &[Account]) -> String {
    let mut s = String::new();
    for a in accounts {
        let (salt, check) = match &a.check {
            Some(c) => (to_hex(&a.salt), to_hex(c)),
            None => (String::new(), String::new()),
        };
        s.push_str(&format!("{}|{}|{}|{}|{}\n", a.name, if a.admin { "admin" } else { "usuario" }, a.iters, salt, check));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nombres() {
        for ok in ["Ana", "Fury Nocturn", "ñandú_2"] {
            assert!(valid_user_name(ok), "{ok}");
        }
        for bad in ["", ".oculto", "a|b", " ana", "ana ", "a\\b", "x".repeat(25).as_str()] {
            assert!(!valid_user_name(bad), "{bad}");
        }
    }

    #[test]
    fn claves() {
        let mut a = Account::new("Ana", true);
        assert!(!a.has_password() && a.verify("") && !a.verify("algo"));
        assert_eq!(a.set_password(Some("corta"), [1; 16]), Err(AccountError::WeakPassword));
        a.set_password(Some("secreta1"), [1; 16]).unwrap();
        assert!(a.has_password() && a.verify("secreta1"));
        assert!(!a.verify("secreta2") && !a.verify(""));
        // La misma clave con otra sal da otra comprobación.
        let mut b = Account::new("Ana", true);
        b.set_password(Some("secreta1"), [2; 16]).unwrap();
        assert_ne!(a.check, b.check);
        a.set_password(None, [0; 16]).unwrap();
        assert!(a.verify(""));
    }

    #[test]
    fn ida_y_vuelta_del_archivo() {
        let mut a = Account::new("Ana", true);
        a.set_password(Some("secreta1"), [9; 16]).unwrap();
        let b = Account::new("Luis", false);
        let text = serialize(&[a.clone(), b.clone()]);
        assert!(!text.contains("secreta1"));
        assert_eq!(parse(&text), [a, b]);
        // Líneas rotas o repetidas se ignoran.
        let v = parse("Ana|admin|50||\nana|usuario|50||\nmal\nX|jefe|50||\nY|usuario|x||\nZ|usuario|50|zz|zz\n");
        assert_eq!(v.len(), 1);
        assert!(v[0].admin && !v[0].has_password());
    }
}
