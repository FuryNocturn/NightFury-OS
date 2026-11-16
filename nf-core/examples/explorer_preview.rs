//! Vista previa del Explorador de archivos:
//! `cargo run -p nf-core --example explorer_preview -- carpeta`
//! Guarda BMP de una carpeta con contenido, de la Papelera y del tamaño mínimo.
use nf_core::apps::{self, AppKind, Ctx};
use nf_core::files;
use nf_core::platform::{Platform, TestPlatform};
use nf_core::surface::Surface;
use nf_core::theme::Theme;

fn guarda(dir: &str, nombre: &str, s: &Surface) {
    std::fs::write(format!("{dir}/{nombre}.bmp"), nf_core::bmp::encode(s)).unwrap();
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| String::from("explorer_preview"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut sys = TestPlatform { ms: 1000, size: (1280, 800), ..Default::default() };
    let now = sys.now();
    let f = &mut sys.files;
    f.mkdir("C:\\Documentos\\Trabajo", now).unwrap();
    f.mkdir("C:\\Documentos\\Fotos de viaje", now).unwrap();
    f.write("C:\\Documentos\\nota.txt", &[b'x'; 1500], now).unwrap();
    f.write("C:\\Documentos\\Lista de la compra con un nombre bastante largo.txt", b"leche", now).unwrap();
    f.write("C:\\Documentos\\datos.bin", &vec![7u8; 3 << 20], now).unwrap();
    f.write("C:\\Documentos\\vacio", b"", now).unwrap();
    f.write("C:\\Imágenes\\viejo.txt", b"a", now).unwrap();
    f.trash("C:\\Imágenes\\viejo.txt", now).unwrap();
    f.write("C:\\Documentos\\borrador.txt", b"hola", now).unwrap();
    f.trash("C:\\Documentos\\borrador.txt", now).unwrap();
    let mut theme = Theme::default();
    let mut app = apps::create(AppKind::Files);
    app.open_path("C:\\Documentos", &mut Ctx::new(&mut sys, &mut theme, 0));
    for (nombre, (w, h)) in [("def", apps::default_size(AppKind::Files)), ("min", app.min_size())] {
        let mut s = Surface::new(w, h, 0);
        app.draw(&mut s, &theme, &mut sys, true);
        guarda(&dir, &format!("carpeta_{nombre}"), &s);
    }
    app.open_path(files::TRASH, &mut Ctx::new(&mut sys, &mut theme, 0));
    for (nombre, (w, h)) in [("def", apps::default_size(AppKind::Files)), ("min", app.min_size())] {
        let mut s = Surface::new(w, h, 0);
        app.draw(&mut s, &theme, &mut sys, true);
        guarda(&dir, &format!("papelera_{nombre}"), &s);
    }
    // Carpeta con muchas filas: barra de desplazamiento.
    for i in 0..30 {
        sys.files.mkdir(&format!("C:\\Escritorio\\Carpeta {i:02}"), now).unwrap();
    }
    app.open_path("C:\\Escritorio", &mut Ctx::new(&mut sys, &mut theme, 0));
    let (w, h) = apps::default_size(AppKind::Files);
    let mut s = Surface::new(w, h, 0);
    app.draw(&mut s, &theme, &mut sys, true);
    guarda(&dir, "escritorio_barra", &s);
}
