//! nfdisk: lee y escribe imágenes de disco NFFS (el `disco.img` de QEMU) desde Windows.
//! Solo escribe en archivos regulares; nunca en dispositivos ni unidades.
use nf_core::block::{BlockDev, DiskError, BLOCK};
use nf_core::files::{is_protected, Files};
use nf_core::fs::{Entry, FsError, Nffs};
use nf_core::platform::DateTime;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
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

fn mount(path: &str, write: bool) -> R<Nffs> {
    let mut dev = open(path, write)?;
    if !Nffs::is_formatted(&mut dev) {
        return Err(format!("«{path}» no contiene un disco NFFS."));
    }
    Nffs::mount(Box::new(dev)).map_err(fs_err)
}

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
    Nffs::format(Box::new(dev), t).map_err(fs_err)?;
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

fn list(fs: &mut Nffs, dir: &str, recurse: bool, depth: usize) -> R<()> {
    let mut v = fs.list(dir).map_err(fs_err)?;
    if dir.is_empty() {
        v.retain(|e| !e.name.eq_ignore_ascii_case("$Papelera"));
    }
    v.sort_by_key(|e| (!e.dir, e.name.to_lowercase()));
    for e in &v {
        println!("{}{}", "  ".repeat(depth), show(e));
        if recurse && e.dir {
            let sub = if dir.is_empty() { e.name.clone() } else { format!("{dir}\\{}", e.name) };
            list(fs, &sub, true, depth + 1)?;
        }
    }
    Ok(())
}

fn run(args: &[String]) -> R<()> {
    let flag = |f: &str| args.iter().any(|a| a == f);
    let pos: Vec<&str> = args.iter().skip(1).filter(|a| !a.starts_with("--") && a.as_str() != "-r").map(|s| s.as_str()).collect();
    let need = |n: usize| if pos.len() == n { Ok(()) } else { Err("Número de argumentos incorrecto. Prueba «nfdisk ayuda».".to_string()) };
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("ayuda");
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
            let (total, free) = mount(pos[0], false)?.space();
            println!("Tamaño: {}\nLibre:  {}\nUsado:  {}", fmt_size(total), fmt_size(free), fmt_size(total - free));
        }
        "comprobar" => {
            need(1)?;
            mount(pos[0], false)?.check().map_err(|e| format!("La imagen tiene errores: {}", e.message()))?;
            println!("La imagen está bien.");
        }
        "listar" => {
            if pos.len() != 1 && pos.len() != 2 {
                return need(1);
            }
            let mut fs = mount(pos[0], false)?;
            let dir = rel(pos.get(1).copied().unwrap_or(""))?;
            list(&mut fs, &dir, flag("-r"), 0)?;
        }
        "meter" => {
            need(3)?;
            let data = std::fs::read(pos[1]).map_err(|e| format!("No se puede leer «{}»: {e}", pos[1]))?;
            let dest = rel_item(pos[2])?;
            let mut fs = mount(pos[0], true)?;
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
            let data = mount(pos[0], false)?.read(&src).map_err(fs_err)?;
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
            mount(pos[0], true)?.mkdir(&p, now()).map_err(fs_err)?;
            println!("Carpeta creada.");
        }
        "borrar" => {
            need(2)?;
            let p = rel_item(pos[1])?;
            if is_protected(&p) {
                return Err(format!("«{p}» es una carpeta del sistema o de una cuenta y no se puede borrar."));
            }
            mount(pos[0], true)?.remove(&p).map_err(fs_err)?;
            println!("Borrado.");
        }
        "ver" => {
            need(2)?;
            let p = rel_item(pos[1])?;
            let data = mount(pos[0], false)?.read(&p).map_err(fs_err)?;
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

    fn go(a: &[&str]) -> R<()> {
        run(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
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
        let mut fs = mount(&img.0, false).unwrap();
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
        let mut fs = mount(&img.0, false).unwrap();
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
}
