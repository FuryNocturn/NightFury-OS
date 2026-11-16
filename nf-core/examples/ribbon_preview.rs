//! Vista previa de la cinta del Explorador: `cargo run -p nf-core --example ribbon_preview -- carpeta`
//! Guarda BMP de las pestañas Inicio y Vista (760 px), con hover y con botones desactivados.
use nf_core::apps::MouseEv;
use nf_core::icons::Icon;
use nf_core::ribbon::{Group, Ribbon, RibbonItem, Tab};
use nf_core::surface::Surface;
use nf_core::theme::Theme;

fn item(id: u32, t: &'static str, i: Icon) -> RibbonItem {
    RibbonItem::new(id, t, i)
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| String::from("ribbon_preview"));
    std::fs::create_dir_all(&dir).unwrap();
    let inicio = Tab {
        title: "Inicio",
        groups: vec![
            Group { title: "Portapapeles", items: vec![item(1, "Copiar", Icon::Copiar).large(), item(2, "Pegar", Icon::Pegar).large(), item(3, "Cortar", Icon::Cortar)] },
            Group {
                title: "Organizar",
                items: vec![item(4, "Mover a", Icon::MoverA), item(5, "Copiar a", Icon::CopiarA), item(6, "Eliminar", Icon::Eliminar), item(7, "Cambiar nombre", Icon::CambiarNombre)],
            },
            Group { title: "Nuevo", items: vec![item(8, "Nueva carpeta", Icon::CarpetaNueva).large(), item(9, "Documento de texto", Icon::DocumentoNuevo)] },
            Group { title: "Abrir", items: vec![item(10, "Propiedades", Icon::Propiedades).large(), item(11, "Abrir", Icon::Abrir)] },
            Group {
                title: "Seleccionar",
                items: vec![item(12, "Seleccionar todo", Icon::SeleccionarTodo), item(13, "No seleccionar nada", Icon::SeleccionarNada), item(14, "Invertir selección", Icon::InvertirSeleccion)],
            },
        ],
    };
    let vista = Tab {
        title: "Vista",
        groups: vec![
            Group {
                title: "Diseño",
                items: vec![
                    item(20, "Iconos grandes", Icon::VistaIconosGrandes),
                    item(21, "Iconos medianos", Icon::VistaIconosMedianos),
                    item(22, "Iconos pequeños", Icon::VistaIconosPequenos),
                    item(23, "Lista", Icon::VistaLista),
                    item(24, "Detalles", Icon::VistaDetalles),
                ],
            },
            Group { title: "Vista actual", items: vec![item(25, "Ordenar por", Icon::Ordenar).large().dropdown()] },
            Group { title: "Mostrar u ocultar", items: vec![item(26, "Extensiones de nombre de archivo", Icon::FileBlank).checkbox()] },
        ],
    };
    let mut rb = Ribbon::new(Some("Archivo"), vec![inicio, vista]);
    let theme = Theme::default();
    let w = 760;
    let guarda = |rb: &Ribbon, nombre: &str, w: i32| {
        let mut s = Surface::new(w, rb.height(), 0xFFFFFF);
        rb.draw(&mut s, w, &theme);
        std::fs::write(format!("{dir}/{nombre}.bmp"), nf_core::bmp::encode(&s)).unwrap();
    };
    rb.set_disabled(3, true);
    rb.set_disabled(12, true);
    let r = rb.item_rect(5, w).unwrap();
    rb.mouse(MouseEv::Move { x: r.x + 4, y: r.y + 4 }, w);
    guarda(&rb, "inicio", w);
    guarda(&rb, "inicio_ancho", 1000);
    rb.set_active(1);
    rb.set_checked(22, true);
    rb.set_checked(26, true);
    guarda(&rb, "vista", w);
}
