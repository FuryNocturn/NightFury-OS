//! Vista previa del Explorador de archivos:
//! `cargo run -p nf-core --example explorer_preview -- carpeta`
//! Guarda BMP de una carpeta con contenido, de la Papelera y del tamaño mínimo.
use nf_core::apps::{self, AppKind, Ctx, MouseEv};
use nf_core::files::{self, join, Std};
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
    let (docs, escritorio, imagenes) = (f.std_dir(Std::Documents), f.std_dir(Std::Desktop), f.std_dir(Std::Pictures));
    f.mkdir(&join(&docs, "Trabajo"), now).unwrap();
    f.mkdir(&join(&docs, "Fotos de viaje"), now).unwrap();
    f.write(&join(&docs, "nota.txt"), &[b'x'; 1500], now).unwrap();
    f.write(&join(&docs, "Lista de la compra con un nombre bastante largo.txt"), b"leche", now).unwrap();
    f.write(&join(&docs, "datos.bin"), &vec![7u8; 3 << 20], now).unwrap();
    f.write(&join(&docs, "vacio"), b"", now).unwrap();
    f.write(&join(&imagenes, "viejo.txt"), b"a", now).unwrap();
    f.trash(&join(&imagenes, "viejo.txt"), now).unwrap();
    f.write(&join(&docs, "borrador.txt"), b"hola", now).unwrap();
    f.trash(&join(&docs, "borrador.txt"), now).unwrap();
    let mut theme = Theme::default();
    let mut app = apps::create(AppKind::Files);
    app.open_path(&docs, &mut Ctx::new(&mut sys, &mut theme, 0));
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
        sys.files.mkdir(&join(&escritorio, &format!("Carpeta {i:02}")), now).unwrap();
    }
    app.open_path(&escritorio, &mut Ctx::new(&mut sys, &mut theme, 0));
    let (w, h) = apps::default_size(AppKind::Files);
    let mut s = Surface::new(w, h, 0);
    app.draw(&mut s, &theme, &mut sys, true);
    guarda(&dir, "escritorio_barra", &s);
    // Vistas Iconos grandes y Lista (menú de zona vacía: Ver; 2 = menú de zona vacía, 22/21 = Iconos/Lista),
    // con un elemento seleccionado y con la carpeta de muchas filas (barra de desplazamiento).
    let mut ms = 0; // cada clic en un instante distinto para que no cuente como doble clic
    for (nombre, id) in [("iconos", 22), ("lista", 21)] {
        app.menu_result(2, id, &mut Ctx::new(&mut sys, &mut theme, 0));
        for (carpeta, sufijo) in [(&docs, "carpeta"), (&escritorio, "barra")] {
            app.open_path(carpeta, &mut Ctx::new(&mut sys, &mut theme, 0));
            let mut s = Surface::new(w, h, 0);
            app.draw(&mut s, &theme, &mut sys, true); // las zonas activas se calculan al dibujar
            ms += 10_000;
            app.mouse(MouseEv::Down { x: 160, y: 80 }, &mut Ctx::new(&mut sys, &mut theme, ms));
            app.draw(&mut s, &theme, &mut sys, true);
            guarda(&dir, &format!("{nombre}_{sufijo}"), &s);
        }
    }
    // Raíz y carpeta de la cuenta: entradas de Usuarios y las seis carpetas estándar con su icono.
    app.menu_result(2, 20, &mut Ctx::new(&mut sys, &mut theme, 0)); // Detalles
    for (nombre, ruta) in [("raiz", String::from("C:\\")), ("cuenta", sys.files.home())] {
        app.open_path(&ruta, &mut Ctx::new(&mut sys, &mut theme, 0));
        let mut s = Surface::new(w, h, 0);
        app.draw(&mut s, &theme, &mut sys, true);
        guarda(&dir, &format!("ruta_{nombre}"), &s);
    }
}
