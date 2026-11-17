// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! nfdisk: lee y escribe imágenes de disco NFFS (el `disco.img` de QEMU) desde Windows.
//! Solo escribe en archivos regulares; nunca en dispositivos ni unidades.
use nf_core::block::{BlockDev, DiskError, BLOCK};
use nf_core::cryptdev::{self, CryptDev, Header};
use nf_core::files::{is_protected, Files};
use nf_core::fs::{Entry, FsError, Nffs};
use nf_core::platform::DateTime;
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, IsTerminal, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::rc::Rc;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

const AYUDA: &str = "nfdisk: imágenes de disco NightFury (NFFS). Cierra QEMU antes de usarlo.\n\
  nfdisk crear <imagen> <MiB>\n\
  nfdisk formatear <imagen> [--forzar]\n\
  nfdisk info <imagen>\n\
  nfdisk comprobar <imagen>\n\
  nfdisk listar <imagen> [ruta] [-r]\n\
  nfdisk meter <imagen> <archivo de Windows> <ruta destino> [--reemplazar]\n\
  nfdisk sacar <imagen> <ruta en la imagen> <archivo de Windows> [--reemplazar]\n\
  nfdisk carpeta <imagen> <ruta>\n\
  nfdisk borrar <imagen> <ruta>\n\
  nfdisk ver <imagen> <ruta>\n\
  nfdisk ayuda\n\
Carpetas de usuario cifradas: sin clave se ven tal como están en disco (nombres cifrados, «(carpeta cifrada)»). Con --usuario <nombre>\n\
y la clave de esa cuenta en la variable NFDISK_CLAVE_USUARIO (o por teclado; nunca por argumento), «listar», «ver», «sacar», «meter»,\n\
«carpeta» y «borrar» trabajan en claro dentro de su carpeta, con las rutas de siempre (C:\\Usuarios\\Ana\\Documentos\\nota.txt).\n\
Sin --usuario, meter, ver, sacar, carpeta y borrar se niegan a tocar dentro de una carpeta cifrada.\n\
Discos con cifrado total: sin clave, «info» y «comprobar» solo dicen «disco cifrado de NightFury». Las demás órdenes piden la clave\n\
del disco (variable de entorno NFDISK_CLAVE, o se pregunta por teclado; nunca por argumento) y con ella trabajan como siempre, también\n\
sobre un disco a medio convertir. --recuperacion: la clave es el código de recuperación del disco. --clave: pregunta la clave en «info» y\n\
«comprobar». nfdisk no activa ni quita el cifrado: eso se hace desde NightFury (Configuración › Disco).\n\
Rutas de la imagen: C:\\Usuarios\\Ana\\Documentos\\nota.txt, \\Usuarios\\Ana\\Documentos\\nota.txt o Usuarios/Ana/Documentos/nota.txt.\n\
Estructura: Usuarios\\<nombre>\\{Escritorio,Documentos,Descargas,Imágenes,Música,Vídeos,.config,.papelera} y Sistema\\{config,temp,lib,drivers,logs}.\n\
nfdisk trabaja directamente sobre el volumen, sin cuentas ni permisos (como administrador): no crea cuentas ni migra discos antiguos; eso lo hace NightFury al arrancar.";

type R<T> = Result<T, String>;

fn fs_err(e: FsError) -> String {
    e.message().to_string()
}

fn disk_err<T>(r: Result<T, DiskError>) -> R<T> {
    r.map_err(|_| "Error de disco.".to_string())
}

/// Disco de bloques sobre un archivo regular.
struct FileDev {
    f: File,
    blocks: u64,
}

impl BlockDev for FileDev {
    fn blocks(&self) -> u64 {
        self.blocks
    }
    fn read(&mut self, n: u64, buf: &mut [u8]) -> Result<(), DiskError> {
        if n >= self.blocks || buf.len() != BLOCK {
            return Err(DiskError::Range);
        }
        self.f.seek(SeekFrom::Start(n * BLOCK as u64)).map_err(|_| DiskError::Io)?;
        self.f.read_exact(buf).map_err(|_| DiskError::Io)
    }
    fn write(&mut self, n: u64, buf: &[u8]) -> Result<(), DiskError> {
        if n >= self.blocks || buf.len() != BLOCK {
            return Err(DiskError::Range);
        }
        self.f.seek(SeekFrom::Start(n * BLOCK as u64)).map_err(|_| DiskError::Io)?;
        self.f.write_all(buf).map_err(|_| DiskError::Io)
    }
    fn flush(&mut self) -> Result<(), DiskError> {
        self.f.sync_all().map_err(|_| DiskError::Io)
    }
}

/// Rechaza dispositivos (`\\.\`, `\\?\`), raíces de unidad y nombres reservados de Windows.
fn check_path(p: &str) -> R<()> {
    let s = p.replace('/', "\\");
    let b = s.as_bytes();
    let drive_root = b.len() >= 2 && b[1] == b':' && s[2..].chars().all(|c| c == '\\');
    if s.starts_with("\\\\.\\") || s.starts_with("\\\\?\\") || drive_root || s.trim_matches('\\').is_empty() {
        return Err(format!("«{p}» es un dispositivo o una unidad, no un archivo de imagen. No se toca."));
    }
    let name = s.rsplit('\\').next().unwrap_or("");
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];
    let com_lpt = (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit();
    if reserved.contains(&stem.as_str()) || com_lpt {
        return Err(format!("«{p}» es un nombre de dispositivo de Windows. No se toca."));
    }
    Ok(())
}

/// Si el archivo existe debe ser regular.
fn check_regular(p: &str) -> R<()> {
    check_path(p)?;
    match std::fs::metadata(p) {
        Ok(m) if !m.is_file() => Err(format!("«{p}» no es un archivo regular.")),
        _ => Ok(()),
    }
}

fn open(path: &str, write: bool) -> R<FileDev> {
    check_regular(path)?;
    let f = OpenOptions::new().read(true).write(write).open(path).map_err(|e| format!("No se puede abrir «{path}»: {e}"))?;
    dev_from(f, path)
}

fn dev_from(f: File, path: &str) -> R<FileDev> {
    let m = f.metadata().map_err(|e| format!("No se puede leer «{path}»: {e}"))?;
    if !m.is_file() {
        return Err(format!("«{path}» no es un archivo regular."));
    }
    if m.len() % BLOCK as u64 != 0 {
        return Err(format!("El tamaño de «{path}» no es múltiplo de {BLOCK} bytes: no es una imagen válida."));
    }
    Ok(FileDev { f, blocks: m.len() / BLOCK as u64 })
}

/// Cabecera de cifrado total del dispositivo, si la tiene (y su último bloque no es del NFFS).
fn crypt_header(dev: &mut FileDev) -> Option<Header> {
    let h = cryptdev::detect(dev)?;
    (Nffs::volume_blocks_of(dev) != Some(dev.blocks)).then_some(h)
}

const CIFRADO: &str = "Disco cifrado de NightFury. Sin la clave no se puede ver nada más.";

/// De dónde sale la clave: el argumento dice si se puede preguntar por teclado.
type Secret<'a> = &'a dyn Fn(bool) -> Option<String>;

/// Cómo se consigue la clave de un disco cifrado.
#[derive(Clone, Copy)]
struct Opts<'a> {
    secret: Secret<'a>,
    /// La clave es el código de recuperación.
    recovery: bool,
    /// Se puede preguntar por teclado si no está en el entorno.
    ask: bool,
}

/// Abre el volumen de un disco cifrado con la clave. Un bloque a medias de una conversión se
/// rehace al abrir (escribe), y entonces la imagen se abre también para escritura.
fn mount_crypt(path: &str, blocks: u64, h: Header, write: bool, o: Opts) -> R<Nffs> {
    let secret = (o.secret)(o.ask).ok_or("Este disco está cifrado: hace falta la clave (variable NFDISK_CLAVE o por teclado).")?;
    let dev = open(path, write || cryptdev::needs_repair(&h, blocks))?;
    let mut core = match cryptdev::open(Box::new(dev), h, &secret, o.recovery) {
        Ok(c) => c,
        Err((_, _, e)) => return Err(e.text().to_string()),
    };
    core.recover().map_err(|_| "Error de disco.".to_string())?;
    Nffs::mount(Box::new(CryptDev::new(Rc::new(RefCell::new(core))))).map_err(fs_err)
}

/// `Ok(None)` si el disco es cifrado y no se ha dado la clave (para «info» y «comprobar»).
fn mount_opt(path: &str, write: bool, o: Opts) -> R<Option<Nffs>> {
    let mut dev = open(path, false)?;
    if let Some(h) = crypt_header(&mut dev) {
        if !o.ask && (o.secret)(false).is_none() {
            return Ok(None);
        }
        return mount_crypt(path, dev.blocks, h, write, o).map(Some);
    }
    if !Nffs::is_formatted(&mut dev) {
        return Err(format!("«{path}» no contiene un disco NFFS."));
    }
    drop(dev);
    let dev = open(path, write)?;
    Nffs::mount(Box::new(dev)).map(Some).map_err(fs_err)
}

fn mount(path: &str, write: bool, o: Opts) -> R<Nffs> {
    mount_opt(path, write, o)?.ok_or_else(|| CIFRADO.to_string())
}

/// Apaga el eco de la consola mientras se escribe la clave (solo Windows; sin consola no hace nada).
#[cfg(windows)]
mod noecho {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleMode(h: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(h: *mut c_void, mode: u32) -> i32;
    }
    const ECHO_INPUT: u32 = 0x0004;

    pub struct Guard(*mut c_void, u32);

    pub fn off() -> Option<Guard> {
        let h = std::io::stdin().as_raw_handle();
        let mut mode = 0u32;
        // SAFETY: llamadas a la API de consola con el manejador de la entrada estándar y un puntero válido.
        unsafe {
            (GetConsoleMode(h, &mut mode) != 0 && SetConsoleMode(h, mode & !ECHO_INPUT) != 0).then_some(Guard(h, mode))
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            // SAFETY: restaura el modo que tenía la consola.
            unsafe {
                SetConsoleMode(self.0, self.1);
            }
        }
    }
}

/// Clave de la variable de entorno `env` o, si `ask`, preguntada por teclado (sin eco si se puede).
fn read_line_secret(env: &str, prompt: &str, ask: bool) -> Option<String> {
    if let Some(v) = std::env::var_os(env) {
        return v.into_string().ok().filter(|s| !s.is_empty());
    }
    if !ask {
        return None;
    }
    let tty = std::io::stdin().is_terminal();
    #[cfg(windows)]
    let _guard = if tty { noecho::off() } else { None };
    #[cfg(windows)]
    let hidden = _guard.is_some();
    #[cfg(not(windows))]
    let hidden = false;
    if tty {
        eprint!("{prompt}{}: ", if hidden { "" } else { " (se verá lo que escribes)" });
        let _ = std::io::stderr().flush();
    }
    let mut line = String::new();
    let r = std::io::stdin().lock().read_line(&mut line);
    if tty {
        eprintln!();
    }
    let line = line.trim_end_matches(['\r', '\n']).to_string();
    (r.is_ok() && !line.is_empty()).then_some(line)
}

/// Clave del disco: `NFDISK_CLAVE` o teclado.
fn read_secret(ask: bool) -> Option<String> {
    read_line_secret("NFDISK_CLAVE", "Clave del disco", ask)
}

/// Clave de una cuenta: `NFDISK_CLAVE_USUARIO` o teclado. Nunca por argumento.
fn read_user_secret(user: &str, ask: bool) -> Option<String> {
    read_line_secret("NFDISK_CLAVE_USUARIO", &format!("Clave de {user}"), ask)
}

/// De dónde sale la clave de una cuenta (las pruebas la dan a mano).
type UserSecret<'a> = &'a dyn Fn(&str, bool) -> Option<String>;

/// Fecha UTC de ahora (días desde 1970 → calendario civil, a mano).
fn now() -> DateTime {
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = ((t / 86400) as i64, t % 86400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + (month <= 2) as i64;
    DateTime { year: year as u16, month: month as u8, day: day as u8, hour: (rem / 3600) as u8, minute: (rem % 3600 / 60) as u8, second: (rem % 60) as u8 }
}

/// Ruta de imagen → ruta relativa de `Nffs` ("" = raíz).
fn rel(p: &str) -> R<String> {
    let mut s = p.replace('/', "\\");
    if s.len() >= 2 && s.as_bytes()[1] == b':' && s.as_bytes()[0].eq_ignore_ascii_case(&b'C') {
        s.drain(..2);
    }
    let r = s.trim_matches('\\').to_string();
    if r.split('\\').next().unwrap_or("").eq_ignore_ascii_case("$Papelera") {
        return Err("La carpeta $Papelera es interna del sistema.".into());
    }
    Ok(r)
}

fn rel_item(p: &str) -> R<String> {
    let r = rel(p)?;
    if r.is_empty() {
        return Err("Hay que indicar un archivo o carpeta, no la raíz.".into());
    }
    Ok(r)
}

/// Formatea la imagen y deja las carpetas iniciales del sistema (reutilizando `Files`).
fn format_dev(dev: FileDev) -> R<()> {
    let f = dev.f.try_clone().map_err(|e| e.to_string())?;
    let blocks = dev.blocks;
    let t = now();
    // Se dejan sin usar los dos últimos bloques: sitio para el cifrado total.
    Nffs::format_with(Box::new(dev), blocks.saturating_sub(cryptdev::RESERVED), t).map_err(fs_err)?;
    let files = Files::probe(Some(Box::new(FileDev { f, blocks })), t);
    if !files.persistent() {
        return Err("No se pudieron crear las carpetas iniciales.".into());
    }
    Ok(())
}

fn fmt_size(b: u64) -> String {
    format!("{:.1} MiB", b as f64 / 1_048_576.0)
}

fn show(e: &Entry) -> String {
    let m = e.modified;
    let kind = if e.dir { "<DIR>".to_string() } else { e.size.to_string() };
    format!("{:04}-{:02}-{:02} {:02}:{:02}  {:>9}  {}", m.year, m.month, m.day, m.hour, m.minute, kind, e.name)
}

/// Nombre de la cuenta si `rel` es (o está dentro de) la carpeta de una cuenta cifrada.
fn encrypted_home(fs: &mut Nffs, rel: &str) -> Option<String> {
    let mut c = rel.split('\\').filter(|c| !c.is_empty());
    let (top, name) = (c.next()?, c.next()?);
    (top.eq_ignore_ascii_case("Usuarios") && fs.stat(&format!("Sistema\\config\\llaves\\{name}")).is_ok()).then(|| name.to_string())
}

/// Sin `--usuario` no se toca dentro de una carpeta cifrada: lo que se escribiera iría en claro.
fn not_encrypted(fs: &mut Nffs, rel: &str) -> R<()> {
    match encrypted_home(fs, rel) {
        Some(n) if rel.split('\\').filter(|c| !c.is_empty()).count() >= 3 => {
            Err(format!("«{rel}» está en la carpeta cifrada de {n}: usa --usuario {n} (clave en NFDISK_CLAVE_USUARIO)."))
        }
        _ => Ok(()),
    }
}

fn list(fs: &mut Nffs, dir: &str, recurse: bool, depth: usize, out: &mut Vec<String>) -> R<()> {
    let mut v = fs.list(dir).map_err(fs_err)?;
    if dir.is_empty() {
        v.retain(|e| !e.name.eq_ignore_ascii_case("$Papelera"));
    }
    v.sort_by_key(|e| (!e.dir, e.name.to_lowercase()));
    if depth == 0 && encrypted_home(fs, dir).is_some() {
        out.push("(carpeta cifrada: se muestra tal como está en el disco; usa --usuario para verla en claro)".into());
    }
    for e in &v {
        let sub = if dir.is_empty() { e.name.clone() } else { format!("{dir}\\{}", e.name) };
        let enc = e.dir && dir.eq_ignore_ascii_case("Usuarios") && encrypted_home(fs, &sub).is_some();
        out.push(format!("{}{}{}", "  ".repeat(depth), show(e), if enc { "  (carpeta cifrada)" } else { "" }));
        if recurse && e.dir {
            list(fs, &sub, true, depth + 1, out)?;
        }
    }
    Ok(())
}

/// Abre la imagen y la sesión de `user` (con su clave) para trabajar en claro dentro de su carpeta.
fn open_user(path: &str, write: bool, o: Opts, user: &str, usecret: UserSecret) -> R<Files> {
    let mut f = Files::on_volume(mount(path, write, o)?);
    let has_pw = f.accounts().iter().find(|a| a.name.eq_ignore_ascii_case(user)).ok_or_else(|| format!("No existe la cuenta «{user}»."))?.has_password();
    let pw = if has_pw { usecret(user, true).ok_or("Hace falta la clave de la cuenta (variable NFDISK_CLAVE_USUARIO o por teclado).")? } else { String::new() };
    f.login(user, &pw).map_err(|e| e.text().to_string())?;
    Ok(f)
}

/// Órdenes con `--usuario`: lo mismo que sin él, pero en claro dentro de la carpeta cifrada de `user`
/// (con los permisos de esa cuenta).
fn user_cmd(cmd: &str, pos: &[&str], flags: (bool, bool), user: &str, o: Opts, usecret: UserSecret) -> R<()> {
    let (recurse, replace) = flags;
    let abs = |p: &str| rel(p).map(|r| format!("C:\\{r}"));
    let need = |n: usize| if pos.len() == n { Ok(()) } else { Err("Número de argumentos incorrecto. Prueba «nfdisk ayuda».".to_string()) };
    match cmd {
        "listar" => {
            if pos.len() != 1 && pos.len() != 2 {
                return need(1);
            }
            let mut f = open_user(pos[0], false, o, user, usecret)?;
            let start = match pos.get(1) {
                Some(p) => abs(p)?,
                None => format!("C:\\Usuarios\\{user}"),
            };
            let mut stack = vec![(start, 0usize)];
            while let Some((dir, depth)) = stack.pop() {
                let mut kids = Vec::new();
                for e in f.list(&dir).map_err(fs_err)? {
                    println!("{}{}", "  ".repeat(depth), show(&e));
                    if recurse && e.dir {
                        kids.push((nf_core::files::join(&dir, &e.name), depth + 1));
                    }
                }
                kids.reverse();
                stack.extend(kids);
            }
        }
        "ver" => {
            need(2)?;
            let data = open_user(pos[0], false, o, user, usecret)?.read(&abs(pos[1])?).map_err(fs_err)?;
            let mut out = std::io::stdout();
            let _ = out.write_all(String::from_utf8_lossy(&data).as_bytes());
            let _ = out.flush();
        }
        "sacar" => {
            need(3)?;
            let data = open_user(pos[0], false, o, user, usecret)?.read(&abs(pos[1])?).map_err(fs_err)?;
            check_regular(pos[2])?;
            if Path::new(pos[2]).exists() && !replace {
                return Err(format!("«{}» ya existe. Usa --reemplazar para sustituirlo.", pos[2]));
            }
            std::fs::write(pos[2], &data).map_err(|e| format!("No se puede escribir «{}»: {e}", pos[2]))?;
            println!("Copiados {} bytes a {}.", data.len(), pos[2]);
        }
        "meter" => {
            need(3)?;
            let data = std::fs::read(pos[1]).map_err(|e| format!("No se puede leer «{}»: {e}", pos[1]))?;
            let dest = abs(pos[2])?;
            let mut f = open_user(pos[0], true, o, user, usecret)?;
            match f.stat(&dest) {
                Ok(e) if e.dir => return Err(fs_err(FsError::IsDir)),
                Ok(_) if !replace => return Err("Ya existe en la imagen. Usa --reemplazar para sustituirlo.".into()),
                _ => {}
            }
            f.write(&dest, &data, now()).map_err(fs_err)?;
            println!("Copiados {} bytes a {}.", data.len(), pos[2]);
        }
        "carpeta" => {
            need(2)?;
            open_user(pos[0], true, o, user, usecret)?.mkdir(&abs(pos[1])?, now()).map_err(fs_err)?;
            println!("Carpeta creada.");
        }
        "borrar" => {
            need(2)?;
            let p = abs(pos[1])?;
            if is_protected(&p) {
                return Err(format!("«{}» es una carpeta del sistema o de una cuenta y no se puede borrar.", pos[1]));
            }
            open_user(pos[0], true, o, user, usecret)?.delete(&p).map_err(fs_err)?;
            println!("Borrado.");
        }
        _ => return Err("--usuario solo vale con listar, ver, sacar, meter, carpeta y borrar.".into()),
    }
    Ok(())
}

fn run(args: &[String]) -> R<()> {
    run_with(args, &read_secret, &read_user_secret)
}

fn run_with(args: &[String], secret: Secret, usecret: UserSecret) -> R<()> {
    // `--usuario <nombre>` se quita antes: el nombre no es un argumento de posición.
    let (mut user, mut rest) = (None, Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--usuario" {
            user = Some(it.next().ok_or("--usuario necesita el nombre de la cuenta.")?.clone());
        } else {
            rest.push(a.clone());
        }
    }
    let args = &rest[..];
    let flag = |f: &str| args.iter().any(|a| a == f);
    let opts = |ask: bool| Opts { secret, recovery: flag("--recuperacion"), ask };
    let all = opts(true);
    let quiet = opts(flag("--clave"));
    let pos: Vec<&str> = args.iter().skip(1).filter(|a| !a.starts_with("--") && a.as_str() != "-r").map(|s| s.as_str()).collect();
    let need = |n: usize| if pos.len() == n { Ok(()) } else { Err("Número de argumentos incorrecto. Prueba «nfdisk ayuda».".to_string()) };
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("ayuda");
    if let Some(u) = &user {
        return user_cmd(cmd, &pos, (flag("-r"), flag("--reemplazar")), u, all, usecret);
    }
    match cmd {
        "ayuda" | "--ayuda" | "-h" | "--help" => println!("{AYUDA}"),
        "crear" => {
            need(2)?;
            let mib: u64 = pos[1].parse().map_err(|_| "El tamaño debe ser un número de MiB.".to_string())?;
            if !(1..=1_048_576).contains(&mib) {
                return Err("El tamaño debe estar entre 1 y 1048576 MiB.".into());
            }
            check_path(pos[0])?;
            let f = OpenOptions::new().read(true).write(true).create_new(true).open(pos[0]).map_err(|e| format!("No se puede crear «{}»: {e}", pos[0]))?;
            let r = f.set_len(mib * 1_048_576).map_err(|e| e.to_string()).and_then(|_| dev_from(f, pos[0])).and_then(format_dev);
            if let Err(e) = r {
                let _ = std::fs::remove_file(pos[0]);
                return Err(e);
            }
            println!("Imagen creada: {} ({mib} MiB).", pos[0]);
        }
        "formatear" => {
            need(1)?;
            let mut dev = open(pos[0], true)?;
            if crypt_header(&mut dev).is_some() {
                // Formatear un disco cifrado destruye lo que contiene: antes hay que demostrar que se tiene la clave.
                drop(dev);
                mount(pos[0], false, all)?;
                dev = open(pos[0], true)?;
            }
            if !Nffs::is_blank(&mut dev) && !flag("--forzar") {
                return Err("La imagen no está en blanco (tiene datos). Para borrarlo todo y formatear usa --forzar.".into());
            }
            if flag("--forzar") {
                // Pone a cero lo que `is_blank` examina: no queda un NFFS viejo a medias.
                let (n, z) = (dev.blocks, [0u8; BLOCK]);
                for b in (0..n.min(256)).chain(n.saturating_sub(256).max(n.min(256))..n) {
                    disk_err(dev.write(b, &z))?;
                }
                disk_err(dev.flush())?;
            }
            format_dev(dev)?;
            println!("Imagen formateada.");
        }
        "info" => {
            need(1)?;
            let Some(mut fs) = mount_opt(pos[0], false, quiet)? else { return Ok(println!("{CIFRADO}")) };
            let (total, free) = fs.space();
            println!("Tamaño: {}\nLibre:  {}\nUsado:  {}", fmt_size(total), fmt_size(free), fmt_size(total - free));
        }
        "comprobar" => {
            need(1)?;
            let Some(mut fs) = mount_opt(pos[0], false, quiet)? else { return Ok(println!("{CIFRADO}")) };
            fs.check().map_err(|e| format!("La imagen tiene errores: {}", e.message()))?;
            println!("La imagen está bien.");
        }
        "listar" => {
            if pos.len() != 1 && pos.len() != 2 {
                return need(1);
            }
            let mut fs = mount(pos[0], false, all)?;
            let dir = rel(pos.get(1).copied().unwrap_or(""))?;
            let mut lines = Vec::new();
            let r = list(&mut fs, &dir, flag("-r"), 0, &mut lines);
            lines.iter().for_each(|l| println!("{l}"));
            r?;
        }
        "meter" => {
            need(3)?;
            let data = std::fs::read(pos[1]).map_err(|e| format!("No se puede leer «{}»: {e}", pos[1]))?;
            let dest = rel_item(pos[2])?;
            let mut fs = mount(pos[0], true, all)?;
            not_encrypted(&mut fs, &dest)?;
            match fs.stat(&dest) {
                Ok(e) if e.dir => return Err(fs_err(FsError::IsDir)),
                Ok(_) if !flag("--reemplazar") => return Err("Ya existe en la imagen. Usa --reemplazar para sustituirlo.".into()),
                _ => {}
            }
            fs.write(&dest, &data, now()).map_err(fs_err)?;
            println!("Copiados {} bytes a {}.", data.len(), pos[2]);
        }
        "sacar" => {
            need(3)?;
            let src = rel_item(pos[1])?;
            let mut fs = mount(pos[0], false, all)?;
            not_encrypted(&mut fs, &src)?;
            let data = fs.read(&src).map_err(fs_err)?;
            check_regular(pos[2])?;
            if Path::new(pos[2]).exists() && !flag("--reemplazar") {
                return Err(format!("«{}» ya existe. Usa --reemplazar para sustituirlo.", pos[2]));
            }
            std::fs::write(pos[2], &data).map_err(|e| format!("No se puede escribir «{}»: {e}", pos[2]))?;
            println!("Copiados {} bytes a {}.", data.len(), pos[2]);
        }
        "carpeta" => {
            need(2)?;
            let p = rel_item(pos[1])?;
            let mut fs = mount(pos[0], true, all)?;
            not_encrypted(&mut fs, &p)?;
            fs.mkdir(&p, now()).map_err(fs_err)?;
            println!("Carpeta creada.");
        }
        "borrar" => {
            need(2)?;
            let p = rel_item(pos[1])?;
            if is_protected(&p) {
                return Err(format!("«{p}» es una carpeta del sistema o de una cuenta y no se puede borrar."));
            }
            let mut fs = mount(pos[0], true, all)?;
            not_encrypted(&mut fs, &p)?;
            fs.remove(&p).map_err(fs_err)?;
            println!("Borrado.");
        }
        "ver" => {
            need(2)?;
            let p = rel_item(pos[1])?;
            let mut fs = mount(pos[0], false, all)?;
            not_encrypted(&mut fs, &p)?;
            let data = fs.read(&p).map_err(fs_err)?;
            let mut o = std::io::stdout();
            let _ = o.write_all(String::from_utf8_lossy(&data).as_bytes());
            let _ = o.flush();
        }
        _ => return Err(format!("Orden desconocida: «{cmd}». Prueba «nfdisk ayuda».")),
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    /// Archivo temporal único que se borra al soltarlo.
    struct Tmp(String);
    impl Tmp {
        fn new(ext: &str) -> Tmp {
            let n = N.fetch_add(1, Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!("nfdisk-test-{}-{n}.{ext}", std::process::id()));
            Tmp(p.to_string_lossy().into_owned())
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Sin clave disponible (para discos sin cifrar).
    fn plain() -> Opts<'static> {
        Opts { secret: &|_| None, recovery: false, ask: false }
    }

    /// Las pruebas no tocan la variable de entorno (es de todo el proceso): la clave se da a mano.
    fn go(a: &[&str]) -> R<()> {
        go_key(a, None)
    }

    fn go_key(a: &[&str], key: Option<&str>) -> R<()> {
        let key = key.map(String::from);
        run_with(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &move |_| key.clone(), &|_, _| None)
    }

    fn go_user(a: &[&str], ukey: Option<&str>) -> R<()> {
        let ukey = ukey.map(String::from);
        run_with(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &|_| None, &move |_, _| ukey.clone())
    }

    #[test]
    fn ciclo_completo() {
        let (img, src, out) = (Tmp::new("img"), Tmp::new("bin"), Tmp::new("out"));
        let datos: Vec<u8> = (0..20_000u32).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(&src.0, &datos).unwrap();
        go(&["crear", &img.0, "2"]).unwrap();
        assert!(go(&["crear", &img.0, "2"]).is_err());
        go(&["info", &img.0]).unwrap();
        go(&["comprobar", &img.0]).unwrap();
        let mut fs = mount(&img.0, false, plain()).unwrap();
        for c in ["Usuarios", "Sistema\\config", "Sistema\\temp", "Sistema\\lib", "Sistema\\drivers", "Sistema\\logs"] {
            assert!(fs.stat(c).unwrap().dir, "{c}");
        }
        assert!(fs.list("Usuarios").unwrap().is_empty(), "sin cuentas: las crea el sistema");
        assert!(fs.stat("Documentos").is_err());
        drop(fs);
        go(&["carpeta", &img.0, "C:\\Usuarios\\Ana"]).unwrap();
        go(&["carpeta", &img.0, "C:\\Usuarios\\Ana\\Documentos"]).unwrap();
        go(&["carpeta", &img.0, "C:\\Usuarios\\Ana\\Documentos\\sub"]).unwrap();
        go(&["meter", &img.0, &src.0, "Usuarios/Ana/Documentos/sub/a.bin"]).unwrap();
        assert!(go(&["meter", &img.0, &src.0, "\\Usuarios\\Ana\\Documentos\\sub\\a.bin"]).is_err());
        go(&["meter", &img.0, &src.0, "C:\\Usuarios\\Ana\\Documentos\\sub\\a.bin", "--reemplazar"]).unwrap();
        go(&["listar", &img.0, "C:\\Usuarios\\Ana", "-r"]).unwrap();
        go(&["sacar", &img.0, "C:\\Usuarios\\Ana\\Documentos\\sub\\a.bin", &out.0]).unwrap();
        assert_eq!(std::fs::read(&out.0).unwrap(), datos);
        assert!(go(&["sacar", &img.0, "Usuarios/Ana/Documentos/sub/a.bin", &out.0]).is_err());
        go(&["sacar", &img.0, "Usuarios/Ana/Documentos/sub/a.bin", &out.0, "--reemplazar"]).unwrap();
        std::fs::write(&src.0, "hola").unwrap();
        go(&["meter", &img.0, &src.0, "Usuarios/Ana/Documentos/sub/t.txt"]).unwrap();
        go(&["ver", &img.0, "Usuarios/Ana/Documentos/sub/t.txt"]).unwrap();
        go(&["borrar", &img.0, "Usuarios/Ana/Documentos/sub/t.txt"]).unwrap();
        assert!(go(&["borrar", &img.0, "Usuarios/Ana/Documentos/sub"]).is_err());
        go(&["borrar", &img.0, "Usuarios/Ana/Documentos/sub/a.bin"]).unwrap();
        go(&["borrar", &img.0, "Usuarios/Ana/Documentos/sub"]).unwrap();
        // Carpetas del sistema y de las cuentas: no se borran.
        for p in ["Usuarios\\Ana\\Documentos", "Usuarios\\Ana", "Usuarios", "Sistema", "Sistema\\logs", "Usuarios\\Ana\\.config"] {
            assert!(go(&["borrar", &img.0, p]).is_err(), "{p}");
        }
        assert!(go(&["borrar", &img.0, "C:\\"]).is_err());
        assert!(go(&["meter", &img.0, &src.0, "$Papelera\\x"]).is_err());
        go(&["comprobar", &img.0]).unwrap();
    }

    #[test]
    fn formatear_protege_datos() {
        let img = Tmp::new("img");
        go(&["crear", &img.0, "1"]).unwrap();
        assert!(go(&["formatear", &img.0]).is_err());
        go(&["carpeta", &img.0, "Usuarios\\x"]).unwrap();
        assert!(go(&["formatear", &img.0]).is_err());
        go(&["formatear", &img.0, "--forzar"]).unwrap();
        let mut fs = mount(&img.0, false, plain()).unwrap();
        assert!(fs.stat("Usuarios\\x").is_err());
        assert!(fs.stat("Sistema\\logs").is_ok());
        drop(fs);
        let en_blanco = Tmp::new("img");
        std::fs::write(&en_blanco.0, vec![0u8; 1 << 20]).unwrap();
        go(&["formatear", &en_blanco.0]).unwrap();
        go(&["comprobar", &en_blanco.0]).unwrap();
        let otro = Tmp::new("img");
        std::fs::write(&otro.0, vec![9u8; 1 << 20]).unwrap();
        assert!(go(&["formatear", &otro.0]).is_err());
        assert_eq!(std::fs::read(&otro.0).unwrap(), vec![9u8; 1 << 20]);
    }

    #[test]
    fn rechaza_dispositivos() {
        for p in ["\\\\.\\PhysicalDrive0", "\\\\.\\C:", "\\\\?\\C:\\x.img", "//./PhysicalDrive0", "C:", "D:\\", "\\", "NUL", "COM1"] {
            for c in ["info", "comprobar", "formatear"] {
                assert!(go(&[c, p, "--forzar"]).is_err(), "{c} {p}");
            }
            assert!(go(&["crear", p, "1"]).is_err(), "crear {p}");
        }
        let d = std::env::temp_dir().to_string_lossy().into_owned();
        assert!(go(&["formatear", &d, "--forzar"]).is_err());
        let img = Tmp::new("img");
        go(&["crear", &img.0, "1"]).unwrap();
        assert!(go(&["sacar", &img.0, "Sistema", "\\\\.\\PhysicalDrive0"]).is_err());
    }

    #[test]
    fn tamano_no_multiplo_se_rechaza() {
        let img = Tmp::new("img");
        std::fs::write(&img.0, vec![0u8; 5000]).unwrap();
        assert!(go(&["info", &img.0]).is_err());
    }

    #[test]
    fn fecha_razonable() {
        let t = now();
        assert!(t.year >= 2026 && (1..=12).contains(&t.month) && (1..=31).contains(&t.day));
    }

    // ---- Discos con cifrado total ----

    const CLAVE: &str = "clave-de-prueba";
    const SECRETO: &str = "TEXTO-SECRETO-DE-PRUEBA-PARA-NFDISK";
    const RUTA: &str = "C:\\Usuarios\\Ana\\Documentos\\secreto.txt";

    /// Imagen de 2 MiB con una cuenta y un archivo, cifrada (entera o a medias) desde el sistema.
    /// Devuelve el código de recuperación.
    fn imagen_cifrada(img: &Tmp, terminar: bool) -> String {
        go(&["crear", &img.0, "2"]).unwrap();
        let t = now();
        let mut f = Files::probe(Some(Box::new(open(&img.0, true).unwrap())), t);
        f.create_account("Ana", None, true, [3; 16], t).unwrap();
        f.login("Ana", "").unwrap();
        f.write(RUTA, SECRETO.repeat(50).as_bytes(), t).unwrap();
        let mut pool = nf_core::keys::Pool::new();
        for i in 0..nf_core::keys::POOL_EVENTS_NEEDED as i32 {
            pool.add_event(i, i * 7, i as u64, i as u64 * 13);
        }
        let code = f.encrypt_disk(CLAVE, &mut pool, t).unwrap();
        if terminar {
            while !f.convert_step(64, t).unwrap() {}
        } else {
            assert!(!f.convert_step(100, t).unwrap());
        }
        code
    }

    fn hay(bytes: &[u8], aguja: &str) -> bool {
        bytes.windows(aguja.len()).any(|w| w == aguja.as_bytes())
    }

    #[test]
    fn imagen_nueva_deja_dos_bloques_sin_usar() {
        let img = Tmp::new("img");
        go(&["crear", &img.0, "1"]).unwrap();
        let fs = mount(&img.0, false, plain()).unwrap();
        assert_eq!(fs.volume_blocks(), 254);
        assert_eq!(fs.dev_blocks(), 256);
    }

    #[test]
    fn disco_cifrado_sin_clave_no_ensena_nada_ni_se_toca() {
        let img = Tmp::new("img");
        imagen_cifrada(&img, true);
        let antes = std::fs::read(&img.0).unwrap();
        assert!(!hay(&antes, SECRETO) && !hay(&antes, "secreto.txt") && !hay(&antes, "Usuarios"));
        go(&["info", &img.0]).unwrap();
        go(&["comprobar", &img.0]).unwrap();
        for orden in [
            vec!["listar", &img.0],
            vec!["ver", &img.0, RUTA],
            vec!["carpeta", &img.0, "Usuarios\\Ana\\x"],
            vec!["borrar", &img.0, RUTA],
            vec!["formatear", &img.0, "--forzar"],
            vec!["sacar", &img.0, RUTA, "no-debe-existir.bin"],
        ] {
            assert!(go(&orden).is_err(), "{orden:?}");
        }
        // Clave mala o código que no es: se rechaza y la imagen queda igual.
        assert!(go_key(&["listar", &img.0], Some("clave-mala")).is_err());
        assert!(go_key(&["info", &img.0], Some("clave-mala")).is_err());
        assert!(go_key(&["listar", &img.0, "--recuperacion"], Some(CLAVE)).is_err());
        assert_eq!(std::fs::read(&img.0).unwrap(), antes);
        assert!(!Path::new("no-debe-existir.bin").exists());
    }

    #[test]
    fn disco_cifrado_con_clave_funciona_como_siempre() {
        let (img, src, out) = (Tmp::new("img"), Tmp::new("bin"), Tmp::new("out"));
        let codigo = imagen_cifrada(&img, true);
        let k = Some(CLAVE);
        go_key(&["info", &img.0], k).unwrap();
        go_key(&["comprobar", &img.0], k).unwrap();
        go_key(&["listar", &img.0, "C:\\Usuarios", "-r"], k).unwrap();
        go_key(&["sacar", &img.0, RUTA, &out.0], k).unwrap();
        assert_eq!(std::fs::read(&out.0).unwrap(), SECRETO.repeat(50).as_bytes());
        std::fs::write(&src.0, "nuevo-contenido-largo-para-buscar").unwrap();
        go_key(&["carpeta", &img.0, "Usuarios\\Ana\\Documentos\\Nueva"], k).unwrap();
        go_key(&["meter", &img.0, &src.0, "Usuarios/Ana/Documentos/Nueva/n.txt"], k).unwrap();
        go_key(&["sacar", &img.0, "Usuarios/Ana/Documentos/Nueva/n.txt", &out.0, "--reemplazar"], k).unwrap();
        assert_eq!(std::fs::read(&out.0).unwrap(), b"nuevo-contenido-largo-para-buscar");
        go_key(&["borrar", &img.0, "Usuarios/Ana/Documentos/Nueva/n.txt"], k).unwrap();
        go_key(&["comprobar", &img.0], k).unwrap();
        // Lo escrito con nfdisk tampoco queda en claro.
        let crudo = std::fs::read(&img.0).unwrap();
        assert!(!hay(&crudo, "nuevo-contenido") && !hay(&crudo, SECRETO) && !hay(&crudo, "Nueva"));
        // Con el código de recuperación (con y sin guiones).
        go_key(&["listar", &img.0, "--recuperacion"], Some(&codigo)).unwrap();
        go_key(&["listar", &img.0, "--recuperacion"], Some(&codigo.replace('-', ""))).unwrap();
        assert!(go_key(&["listar", &img.0, "--recuperacion"], Some(CLAVE)).is_err());
        // Y el sistema sigue pudiendo abrirlo con la clave.
        let t = now();
        let mut f = Files::probe(Some(Box::new(open(&img.0, true).unwrap())), t);
        assert!(f.disk_locked());
        f.unlock_disk(CLAVE, false, t).unwrap();
        f.login("Ana", "").unwrap();
        assert_eq!(f.read(RUTA).unwrap(), SECRETO.repeat(50).as_bytes());
    }

    #[test]
    fn disco_a_medio_convertir_se_abre_con_clave() {
        let (img, src, out) = (Tmp::new("img"), Tmp::new("bin"), Tmp::new("out"));
        imagen_cifrada(&img, false);
        let k = Some(CLAVE);
        // Sin clave no se ve nada; con ella se lee y se escribe a ambos lados del progreso.
        go(&["info", &img.0]).unwrap();
        go_key(&["comprobar", &img.0], k).unwrap();
        go_key(&["sacar", &img.0, RUTA, &out.0], k).unwrap();
        assert_eq!(std::fs::read(&out.0).unwrap(), SECRETO.repeat(50).as_bytes());
        std::fs::write(&src.0, "escrito-con-nfdisk-a-medias").unwrap();
        go_key(&["meter", &img.0, &src.0, "Usuarios/Ana/Documentos/m.txt"], k).unwrap();
        go_key(&["listar", &img.0, "Usuarios/Ana/Documentos"], k).unwrap();
        // El sistema termina la conversión y todo está.
        let t = now();
        let mut f = Files::probe(Some(Box::new(open(&img.0, true).unwrap())), t);
        f.unlock_disk(CLAVE, false, t).unwrap();
        f.login("Ana", "").unwrap();
        while !f.convert_step(64, t).unwrap() {}
        assert_eq!(f.read("C:\\Usuarios\\Ana\\Documentos\\m.txt").unwrap(), b"escrito-con-nfdisk-a-medias");
        assert_eq!(f.read(RUTA).unwrap(), SECRETO.repeat(50).as_bytes());
    }

    #[test]
    fn formatear_un_disco_cifrado_pide_clave_y_forzar() {
        let img = Tmp::new("img");
        imagen_cifrada(&img, true);
        let k = Some(CLAVE);
        assert!(go_key(&["formatear", &img.0], k).is_err());
        assert!(go_key(&["formatear", &img.0, "--forzar"], Some("mala")).is_err());
        go_key(&["formatear", &img.0, "--forzar"], k).unwrap();
        // Ahora es un NFFS normal, sin cabecera y sin lo de antes.
        go(&["comprobar", &img.0]).unwrap();
        assert!(go(&["ver", &img.0, RUTA]).is_err());
        let mut dev = open(&img.0, false).unwrap();
        assert!(crypt_header(&mut dev).is_none());
    }

    #[test]
    fn clave_por_variable_de_entorno() {
        let img = Tmp::new("img");
        imagen_cifrada(&img, true);
        // Es la única prueba que toca la variable (es de todo el proceso).
        std::env::set_var("NFDISK_CLAVE", CLAVE);
        let ok = run(&["listar".to_string(), img.0.clone(), "C:\\Usuarios".to_string()]);
        let info = run(&["info".to_string(), img.0.clone()]);
        std::env::set_var("NFDISK_CLAVE", "otra");
        let mala = run(&["listar".to_string(), img.0.clone()]);
        std::env::remove_var("NFDISK_CLAVE");
        ok.unwrap();
        info.unwrap();
        assert!(mala.is_err());
    }

    #[test]
    fn carpeta_cifrada_de_usuario_en_claro_solo_con_la_clave_de_la_cuenta() {
        use nf_core::keys::{Pool, POOL_EVENTS_NEEDED};
        let (img, src, out) = (Tmp::new("img"), Tmp::new("bin"), Tmp::new("out"));
        go(&["crear", &img.0, "2"]).unwrap();
        let t = now();
        {
            let dev = open(&img.0, true).unwrap();
            let mut f = Files::probe(Some(Box::new(dev)), t);
            f.create_account("Ana", Some("clave-ana-1"), true, [1; 16], t).unwrap();
            f.login("Ana", "clave-ana-1").unwrap();
            f.write("C:\\Usuarios\\Ana\\Documentos\\secreto-largo-nombre.txt", b"CONTENIDO-SECRETO-NFDISK", t).unwrap();
            let mut pool = Pool::new();
            for i in 0..POOL_EVENTS_NEEDED as i32 {
                pool.add_event(i, i * 5, i as u64, i as u64 * 11);
            }
            f.encrypt_home("clave-ana-1", &mut pool, t).unwrap();
            while !f.home_convert_step(100, t).unwrap() {}
        }
        let raw = std::fs::read(&img.0).unwrap();
        for needle in ["CONTENIDO-SECRETO-NFDISK", "secreto-largo-nombre", "Documentos"] {
            assert!(!raw.windows(needle.len()).any(|w| w == needle.as_bytes()), "{needle}");
        }
        // Sin clave de usuario: se ve tal como está en disco y no se toca por dentro.
        let mut fs = mount(&img.0, false, plain()).unwrap();
        let mut lines = Vec::new();
        list(&mut fs, "Usuarios", false, 0, &mut lines).unwrap();
        assert!(lines.iter().any(|l| l.contains("Ana") && l.contains("(carpeta cifrada)")), "{lines:?}");
        let mut inside = Vec::new();
        list(&mut fs, "Usuarios\\Ana", true, 0, &mut inside).unwrap();
        assert!(inside[0].contains("carpeta cifrada") && inside.len() > 8 && inside.iter().all(|l| !l.contains("Documentos")), "{inside:?}");
        drop(fs);
        std::fs::write(&src.0, "NUEVO-CONTENIDO-META").unwrap();
        let ruta = "C:\\Usuarios\\Ana\\Documentos\\secreto-largo-nombre.txt";
        for c in [
            vec!["ver", &img.0, ruta],
            vec!["sacar", &img.0, ruta, &out.0],
            vec!["meter", &img.0, &src.0, "C:\\Usuarios\\Ana\\Documentos\\nuevo.txt"],
            vec!["carpeta", &img.0, "C:\\Usuarios\\Ana\\Documentos\\sub"],
            vec!["borrar", &img.0, ruta],
        ] {
            assert!(go(&c).is_err(), "{c:?}");
        }
        assert!(!Path::new(&out.0).exists());
        // Con --usuario y su clave: en claro.
        let con = |c: &[&str], k: Option<&str>| {
            let mut v = c.to_vec();
            v.extend(["--usuario", "Ana"]);
            go_user(&v, k)
        };
        let k = Some("clave-ana-1");
        assert!(con(&["ver", &img.0, ruta], None).is_err());
        assert!(con(&["ver", &img.0, ruta], Some("mala")).is_err());
        assert!(go_user(&["ver", &img.0, ruta, "--usuario", "Nadie"], k).is_err());
        assert!(go_user(&["info", &img.0, "--usuario", "Ana"], k).is_err());
        con(&["ver", &img.0, ruta], k).unwrap();
        con(&["sacar", &img.0, ruta, &out.0], k).unwrap();
        assert_eq!(std::fs::read(&out.0).unwrap(), b"CONTENIDO-SECRETO-NFDISK");
        con(&["carpeta", &img.0, "C:\\Usuarios\\Ana\\Documentos\\sub"], k).unwrap();
        con(&["meter", &img.0, &src.0, "Usuarios/Ana/Documentos/sub/nuevo.txt"], k).unwrap();
        assert!(con(&["meter", &img.0, &src.0, "Usuarios/Ana/Documentos/sub/nuevo.txt"], k).is_err());
        con(&["meter", &img.0, &src.0, "Usuarios/Ana/Documentos/sub/nuevo.txt", "--reemplazar"], k).unwrap();
        con(&["listar", &img.0, "C:\\Usuarios\\Ana\\Documentos", "-r"], k).unwrap();
        con(&["listar", &img.0], k).unwrap();
        let raw = std::fs::read(&img.0).unwrap();
        for needle in ["NUEVO-CONTENIDO-META", "nuevo.txt", "sub"] {
            assert!(!raw.windows(needle.len()).any(|w| w == needle.as_bytes()), "{needle}");
        }
        assert!(con(&["borrar", &img.0, "C:\\Usuarios\\Ana\\Documentos"], k).is_err());
        con(&["borrar", &img.0, "C:\\Usuarios\\Ana\\Documentos\\sub"], k).unwrap();
        go(&["comprobar", &img.0]).unwrap();
        // NightFury lo ve igual: lo que quedó es lo de antes.
        let dev = open(&img.0, false).unwrap();
        let mut f = Files::probe(Some(Box::new(dev)), t);
        f.login("Ana", "clave-ana-1").unwrap();
        assert_eq!(f.read("C:\\Usuarios\\Ana\\Documentos\\secreto-largo-nombre.txt").unwrap(), b"CONTENIDO-SECRETO-NFDISK");
        assert!(!f.exists("C:\\Usuarios\\Ana\\Documentos\\sub"));
    }

    #[test]
    fn usuario_sin_clave_propia_entra_sin_pedirla() {
        let img = Tmp::new("img");
        go(&["crear", &img.0, "2"]).unwrap();
        let t = now();
        {
            let dev = open(&img.0, true).unwrap();
            let mut f = Files::probe(Some(Box::new(dev)), t);
            f.create_account("Luis", None, true, [1; 16], t).unwrap();
        }
        go_user(&["carpeta", &img.0, "C:\\Usuarios\\Luis\\Documentos\\x", "--usuario", "Luis"], None).unwrap();
        let mut fs = mount(&img.0, false, plain()).unwrap();
        assert!(fs.stat("Usuarios\\Luis\\Documentos\\x").unwrap().dir);
        // Sin cifrar, sin --usuario sigue como siempre.
        drop(fs);
        go(&["carpeta", &img.0, "C:\\Usuarios\\Luis\\Documentos\\y"]).unwrap();
    }
}
