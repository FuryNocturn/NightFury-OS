// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
use super::*;
use crate::entrada::*;
use nf_core::platform::TestPlatform;
use nf_core::ttf::Font;

fn web(s: &str) -> Option<Entrada> {
    Some(Entrada::Web(String::from(s)))
}

#[test]
fn clasifica_lo_escrito() {
    assert_eq!(clasificar("   "), None);
    assert_eq!(clasificar("https://ejemplo.com/a b"), web("https://ejemplo.com/a%20b"));
    assert_eq!(clasificar("HTTP://Ejemplo.com"), web("HTTP://Ejemplo.com"));
    assert_eq!(clasificar("ejemplo.com"), web("https://ejemplo.com"));
    assert_eq!(clasificar("ejemplo.com/ruta?q=1.5"), web("https://ejemplo.com/ruta?q=1.5"));
    assert_eq!(clasificar("localhost:8080/x"), web("http://localhost:8080/x"));
    assert_eq!(clasificar("192.168.1.5"), web("http://192.168.1.5"));
    assert_eq!(clasificar("ftp://x.com"), web("ftp://x.com"));
    assert_eq!(clasificar("c:/Pruebas/a.html"), Some(Entrada::Local(String::from("C:\\Pruebas\\a.html"))));
    assert_eq!(clasificar("D:\\x\\y.htm"), Some(Entrada::Local(String::from("D:\\x\\y.htm"))));
    // Con espacios, o sin punto, es una búsqueda.
    assert_eq!(clasificar("gatos graciosos"), web("https://html.duckduckgo.com/html/?q=gatos+graciosos"));
    assert_eq!(clasificar("hola"), web("https://html.duckduckgo.com/html/?q=hola"));
    assert_eq!(clasificar("a. b"), web("https://html.duckduckgo.com/html/?q=a.+b"));
    assert_eq!(clasificar("..."), web("https://html.duckduckgo.com/html/?q=..."));
}

#[test]
fn codifica_la_consulta() {
    assert_eq!(codificar_consulta("año 2026 & más/otro=1#x"), "a%C3%B1o+2026+%26+m%C3%A1s%2Fotro%3D1%23x");
    assert_eq!(codificar_consulta("a-b_c.d~e"), "a-b_c.d~e");
}

#[test]
fn rutas_locales_como_url() {
    let u = url_de_local("C:\\Mis páginas\\a#b.html");
    assert_eq!(u, "http://archivo.invalid/C:/Mis%20p%C3%A1ginas/a%23b.html");
    assert_eq!(local_de_url(&u).as_deref(), Some("C:\\Mis páginas\\a#b.html"));
    assert_eq!(local_de_url("http://archivo.invalid/C:/x/y.png?v=1#z").as_deref(), Some("C:\\x\\y.png"));
    assert_eq!(local_de_url("http://archivo.invalid/etc/passwd"), None);
    assert_eq!(local_de_url("http://otro.com/C:/x"), None);
    // Los «..» no se salen de la unidad hacia una ruta sin letra.
    let r = nf_web::url::resolver("http://archivo.invalid/C:/a/b.html", "../../../x.png").unwrap();
    assert_eq!(local_de_url(&r), None);
}

#[test]
fn pagina_publica_no_toca_la_red_local() {
    let publica = "https://ejemplo.com/index.html";
    for malo in ["http://localhost/x.png", "http://127.0.0.1:8080/a.css", "http://10.0.0.3/x", "http://192.168.1.1/x", "http://172.16.0.9/x", "http://169.254.1.1/x", "http://router/x", "http://impresora.local/x", "http://0x7f.0.0.1/x", "http://archivo.invalid/C:/x.png", "http://[::1]/x"] {
        let r = nf_web::url::resolver(publica, malo);
        // `[::1]` ni siquiera se admite como URL: da igual cómo, pero nunca se carga.
        assert!(r.is_none_or(|u| !recurso_permitido(publica, &u)), "{malo}");
    }
    assert!(recurso_permitido(publica, "https://cdn.otro.com/x.css"));
    assert!(recurso_permitido(publica, "http://172.32.0.1/x"), "172.32 ya es público");
    // Desde una página de la red local o del disco, todo vale.
    assert!(recurso_permitido("http://192.168.1.2/", "http://192.168.1.3/x.png"));
    assert!(recurso_permitido("http://localhost:3000/", "http://localhost:3000/x.png"));
    assert!(recurso_permitido("http://archivo.invalid/C:/a.html", "http://archivo.invalid/C:/x.png"));
    assert!(recurso_permitido("http://archivo.invalid/C:/a.html", "https://cdn.otro.com/x.css"));
}

#[test]
fn tipo_de_vista_segun_content_type() {
    assert_eq!(tipo_vista(Some("text/html; charset=utf-8"), b""), TipoVista::Html);
    assert_eq!(tipo_vista(Some("Application/XHTML+xml"), b""), TipoVista::Html);
    assert_eq!(tipo_vista(Some("text/plain"), b""), TipoVista::Texto);
    assert_eq!(tipo_vista(Some("image/png"), b""), TipoVista::Imagen);
    assert_eq!(tipo_vista(Some("image/jpeg"), b""), TipoVista::Imagen);
    assert_eq!(tipo_vista(Some("image/gif"), b""), TipoVista::Imagen);
    assert_eq!(tipo_vista(Some("application/pdf"), b"%PDF"), TipoVista::Otro);
    assert_eq!(tipo_vista(Some("image/svg+xml"), b"<svg"), TipoVista::Otro);
    assert_eq!(tipo_vista(None, b"GIF89a...."), TipoVista::Imagen);
    assert_eq!(tipo_vista(None, b"<p>hola"), TipoVista::Html);
}

// ---------------------------------------------------------------- la ventana

fn sistema() -> TestPlatform {
    let mut sys = TestPlatform { size: (1280, 800), ..Default::default() };
    let ahora = sys.now();
    sys.files.mkdir("C:\\Pruebas", ahora).unwrap();
    sys
}

fn escribe(sys: &mut TestPlatform, ruta: &str, datos: &str) {
    let ahora = sys.now();
    sys.files.write(ruta, datos.as_bytes(), ahora).unwrap();
}

fn con_letras(n: &mut Navegador) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../nf-core/data/fuentes/");
    for (i, nombre) in ["LiberationSans-Regular", "LiberationSans-Bold"].into_iter().enumerate() {
        n.letras.poner(i, Font::parse(std::fs::read(format!("{dir}{nombre}.ttf")).unwrap()).unwrap());
    }
}

struct Banco {
    n: Navegador,
    sys: PlatRed,
    th: Theme,
    ms: u64,
}

impl Banco {
    fn nuevo() -> Banco {
        let mut n = Navegador::nuevo();
        con_letras(&mut n);
        Banco { n, sys: PlatRed { base: sistema(), red: None }, th: Theme::default(), ms: 1000 }
    }
    fn tecla(&mut self, key: Key, ctrl: bool, shift: bool, alt: bool) {
        let ev = KeyEvent { key, ch: None, down: true, shift, ctrl, alt };
        let mut cx = Ctx::new(&mut self.sys, &mut self.th, self.ms);
        self.n.key(&ev, &mut cx);
    }
    fn escribe(&mut self, t: &str) {
        for c in t.chars() {
            let ev = KeyEvent { key: Key::Char(c), ch: Some(c), down: true, shift: false, ctrl: false, alt: false };
            let mut cx = Ctx::new(&mut self.sys, &mut self.th, self.ms);
            self.n.key(&ev, &mut cx);
        }
    }
    fn raton(&mut self, ev: MouseEv) -> bool {
        let mut cx = Ctx::new(&mut self.sys, &mut self.th, self.ms);
        self.n.mouse(ev, &mut cx);
        cx.close
    }
    fn tick(&mut self) {
        self.ms += 30;
        self.sys.ms = self.ms;
        let mut cx = Ctx::new(&mut self.sys, &mut self.th, self.ms);
        self.n.tick(&mut cx);
    }
    fn foto(&mut self, w: i32, h: i32) -> Surface {
        self.sys.ms = self.ms;
        let mut s = Surface::new(w, h, 0);
        self.n.draw(&mut s, &self.th, &mut self.sys, true);
        s
    }
    fn abrir(&mut self, ruta: &str) {
        let mut cx = Ctx::new(&mut self.sys, &mut self.th, self.ms);
        self.n.open_path(ruta, &mut cx);
    }
}

#[test]
fn historial_atras_y_adelante_por_pestana() {
    let mut b = Banco::nuevo();
    for (f, t) in [("a", "A"), ("b", "B"), ("c", "C")] {
        escribe(&mut b.sys, &format!("C:\\Pruebas\\{f}.html"), &format!("<title>{t}</title><p>{t}"));
    }
    for f in ["a", "b", "c"] {
        b.abrir(&format!("C:\\Pruebas\\{f}.html"));
    }
    assert_eq!(b.n.pest[0].titulo, "C");
    b.tecla(Key::Left, false, false, true);
    assert_eq!((b.n.pest[0].titulo.as_str(), b.n.pest[0].url.as_str()), ("B", "C:\\Pruebas\\b.html"));
    b.tecla(Key::Left, false, false, true);
    b.tecla(Key::Left, false, false, true);
    assert_eq!(b.n.pest[0].url, "", "la página de inicio es la primera entrada");
    assert!(!b.n.pest[0].puede_atras());
    b.tecla(Key::Right, false, false, true);
    b.tecla(Key::Right, false, false, true);
    assert_eq!(b.n.pest[0].titulo, "B");
    // Retroceso no navega.
    b.tecla(Key::Backspace, false, false, false);
    assert_eq!(b.n.pest[0].titulo, "B");
    // Una navegación nueva corta lo que había por delante.
    b.abrir("C:\\Pruebas\\a.html");
    assert!(!b.n.pest[0].puede_adelante());
    // Otra pestaña tiene su propio historial.
    b.tecla(Key::Char('t'), true, false, false);
    assert!(!b.n.pest[1].puede_atras());
    assert!(b.n.pest[0].puede_atras());
}

#[test]
fn gestion_de_pestanas() {
    let mut b = Banco::nuevo();
    b.tecla(Key::Char('t'), true, false, false);
    b.tecla(Key::Char('t'), true, false, false);
    assert_eq!((b.n.pest.len(), b.n.act), (3, 2));
    b.tecla(Key::Tab, true, false, false);
    assert_eq!(b.n.act, 0, "Ctrl+Tab da la vuelta");
    b.tecla(Key::Tab, true, true, false);
    assert_eq!(b.n.act, 2, "Ctrl+Mayús+Tab vuelve atrás");
    let ids: Vec<u32> = b.n.pest.iter().map(|t| t.id).collect();
    assert_eq!(ids, [0, 1, 2]);
    b.tecla(Key::Char('w'), true, false, false);
    assert_eq!((b.n.pest.len(), b.n.act), (2, 1));
    // Clic en la ✕ de la primera pestaña.
    let r = rect_cerrar(rect_pestana(0, 2, b.n.size.0));
    b.raton(MouseEv::Down { x: r.x + 9, y: r.y + 9 });
    assert_eq!((b.n.pest.len(), b.n.act), (1, 0));
    // Cerrar la última cierra la ventana.
    let mut cx = Ctx::new(&mut b.sys, &mut b.th, 0);
    b.n.cerrar(0, &mut cx);
    assert!(cx.close);
}

#[test]
fn ctrl_l_y_escape_en_la_barra() {
    let mut b = Banco::nuevo();
    b.abrir("C:\\Pruebas\\no-existe.html");
    assert!(b.n.foco == Foco::Pagina);
    b.tecla(Key::Char('l'), true, false, false);
    assert!(b.n.foco == Foco::Barra && b.n.barra.todo && b.n.barra.texto() == "C:\\Pruebas\\no-existe.html");
    b.escribe("hola mundo");
    assert_eq!(b.n.barra.texto(), "hola mundo");
    b.tecla(Key::Escape, false, false, false);
    assert!(b.n.foco == Foco::Pagina);
    b.tecla(Key::F(6), false, false, false);
    assert_eq!(b.n.barra.texto(), "C:\\Pruebas\\no-existe.html", "Escape restaura la dirección");
}

#[test]
fn sin_red_sale_una_pagina_de_error() {
    let mut b = Banco::nuevo();
    b.escribe("ejemplo.com");
    b.tecla(Key::Enter, false, false, false);
    let t = &b.n.pest[0];
    assert_eq!((t.titulo.as_str(), t.url.as_str()), ("Sin conexión de red", "https://ejemplo.com"));
    assert_eq!(t.seguridad(), Seguridad::Ninguna, "una página de error no lleva candado");
    b.abrir("C:\\Pruebas\\falta.html");
    assert_eq!(b.n.pest[0].titulo, "No se puede abrir el archivo");
    // Un tipo desconocido da su aviso.
    escribe(&mut b.sys, "C:\\Pruebas\\x.xyz", "datos");
    b.abrir("C:\\Pruebas\\x.xyz");
    assert_eq!(b.n.pest[0].titulo, "No se puede mostrar este tipo de archivo");
    b.foto(900, 600);
}

#[test]
fn pagina_local_con_hoja_maqueta_y_pinta() {
    let mut b = Banco::nuevo();
    escribe(&mut b.sys, "C:\\Pruebas\\e.css", "body { background: #ff0000 } a { color: #00aa00 }");
    let mut html = String::from("<title>Prueba</title><link rel=stylesheet href='e.css'><img src='no.png' alt='sin imagen'><h1>Hola</h1><a href='#fin'>ir al final</a>");
    for i in 0..80 {
        html.push_str(&format!("<p>Párrafo número {i}, con texto de relleno suficiente para ocupar bastante sitio."));
    }
    html.push_str("<h2 id=fin>Final</h2><p>fin");
    escribe(&mut b.sys, "C:\\Pruebas\\p.html", &html);
    b.abrir("C:\\Pruebas\\p.html");
    let s = b.foto(900, 600);
    assert_eq!(b.n.title(), "Prueba - Navegador");
    assert!(b.n.pest[0].alto > 1500);
    assert_eq!(s.px[(300 * 900 + 600) as usize], 0xFF0000, "la hoja de estilos se ha leído del disco");
    // Rueda, teclas y ancla.
    b.raton(MouseEv::Wheel { x: 400, y: 300, dz: 3 });
    assert_eq!(b.n.pest[0].scroll, 180);
    b.tecla(Key::End, false, false, false);
    assert_eq!(b.n.pest[0].scroll, b.n.max_scroll());
    b.tecla(Key::Home, false, false, false);
    assert_eq!(b.n.pest[0].scroll, 0);
    b.tecla(Key::Char(' '), false, false, false);
    assert_eq!(b.n.pest[0].scroll, 600 - TAB_H - TOOL_H - PASO_FLECHA);
    // Clic en el enlace «#fin»: baja a la ancla sin recargar.
    let u = b.n.pest[0].url.clone();
    let mut cx = Ctx::new(&mut b.sys, &mut b.th, 0);
    b.n.abrir_enlace(&format!("{}#fin", url_de_local(&u)), false, &mut cx);
    assert!(b.n.pest[0].scroll > 1500 - 600);
    assert_eq!(b.n.pest[0].url, u);
    // Selección con el ratón y copia.
    b.tecla(Key::Home, false, false, false);
    b.foto(900, 600);
    let y = TAB_H + TOOL_H;
    b.raton(MouseEv::Down { x: 5, y: y + 5 });
    b.raton(MouseEv::Move { x: 800, y: y + 120 });
    b.raton(MouseEv::Up { x: 800, y: y + 120 });
    b.foto(900, 600);
    b.tecla(Key::Char('c'), true, false, false);
    assert!(b.n.copiado.contains("Hola"), "copiado: {:?}", b.n.copiado);
    // Cambiar el ancho de la ventana re-maqueta sin pánico.
    for w in [200, 480, 1400] {
        b.foto(w, 400);
    }
}

#[test]
fn imagen_suelta_y_texto_suelto() {
    let mut b = Banco::nuevo();
    escribe(&mut b.sys, "C:\\Pruebas\\t.txt", "línea <uno>\n  línea dos");
    b.abrir("C:\\Pruebas\\t.txt");
    b.foto(700, 500);
    assert_eq!(b.n.pest[0].titulo, "t.txt");
    escribe(&mut b.sys, "C:\\Pruebas\\roto.png", "no soy un png");
    b.abrir("C:\\Pruebas\\roto.png");
    assert_eq!(b.n.pest[0].titulo, "No se puede mostrar la imagen");
    // Una imagen cualquiera, con el tamaño ajustado al ancho.
    let p = nf_core::bmp::Picture { w: 800, h: 400, argb: vec![0xFF3366CC; 800 * 400] };
    assert_eq!(pestana::ajustar_imagen(&p, 440), (400, 200));
    assert_eq!(pestana::ajustar_imagen(&p, 2000), (800, 400));
}

#[test]
fn inicio_dibuja_y_busca() {
    let mut b = Banco::nuevo();
    let s = b.foto(1024, 700);
    assert!(s.px.iter().any(|&p| p == 0xDFE1E5), "la caja de búsqueda se ve");
    assert_eq!(b.n.title(), "Nueva pestaña - Navegador");
    // Se escribe en la barra (foco inicial) y Enter navega a la búsqueda.
    b.escribe("gatos");
    b.tecla(Key::Enter, false, false, false);
    assert_eq!(b.n.pest[0].url, "https://html.duckduckgo.com/html/?q=gatos");
    // Nueva pestaña: la caja del centro también escribe.
    b.tecla(Key::Char('t'), true, false, false);
    b.raton(MouseEv::Down { x: 500, y: TAB_H + TOOL_H + (700 - TAB_H - TOOL_H) * 45 / 100 + 20 });
    assert!(b.n.foco == Foco::Pagina);
    b.escribe("perros");
    assert_eq!(b.n.pest[1].caja.texto(), "perros");
    b.foto(1024, 700);
    b.tick();
}

#[test]
fn formulario_get_de_pagina_local_con_teclado() {
    let mut b = Banco::nuevo();
    escribe(&mut b.sys, "C:\\Pruebas\\dest.html", "<title>Destino</title><p>llegaste");
    escribe(&mut b.sys, "C:\\Pruebas\\f.html", "<title>Form</title><form action='dest.html'><input name=q autofocus><input type=submit></form>");
    b.abrir("C:\\Pruebas\\f.html");
    b.foto(900, 600);
    b.escribe("hola ñ");
    // Retroceso borra en el campo (no navega) y Enter envía el formulario: se abre el archivo de destino
    b.tecla(Key::Backspace, false, false, false);
    b.tecla(Key::Backspace, false, false, false);
    assert_eq!(b.n.pest[0].titulo, "Form");
    b.tecla(Key::Enter, false, false, false);
    assert_eq!((b.n.pest[0].titulo.as_str(), b.n.pest[0].url.as_str()), ("Destino", "C:\\Pruebas\\dest.html"));
    assert!(b.n.pest[0].puede_atras());
    // Tab pasa al botón y Espacio lo pulsa
    b.tecla(Key::Left, false, false, true);
    b.foto(900, 600);
    assert_eq!(b.n.pest[0].titulo, "Form");
    b.tecla(Key::Tab, false, false, false);
    b.escribe(" ");
    assert_eq!(b.n.pest[0].titulo, "Destino");
}

#[test]
fn post_avisa_de_contrasena_y_pide_confirmacion_al_repetir() {
    let mut b = Banco::nuevo();
    escribe(&mut b.sys, "C:\\Pruebas\\f.html", "<title>Form</title><p>x");
    b.abrir("C:\\Pruebas\\f.html");
    b.foto(900, 600);
    let envio = |url: &str, pass: bool| Envio { metodo: Metodo::Post, url: String::from(url), cuerpo: Some(b"u=a&p=b".to_vec()), tiene_password: pass };
    // contraseña por http: aviso y no se envía hasta que se acepte; Escape cancela
    {
        let mut cx = Ctx::new(&mut b.sys, &mut b.th, 0);
        b.n.enviar(envio("http://pagina.test/login", true), &mut cx);
    }
    assert!(matches!(b.n.aviso, Some(Aviso::Password(_))));
    assert_eq!(b.n.pest[0].url, "C:\\Pruebas\\f.html");
    b.foto(900, 600);
    b.tecla(Key::Escape, false, false, false);
    assert!(b.n.aviso.is_none() && b.n.pest[0].url == "C:\\Pruebas\\f.html");
    // por https no hay aviso y el POST queda en el historial
    {
        let mut cx = Ctx::new(&mut b.sys, &mut b.th, 0);
        b.n.enviar(envio("https://pagina.test/login", true), &mut cx);
    }
    assert!(b.n.aviso.is_none() && b.n.pest[0].pide_post(0));
    // recargar pregunta antes de reenviar; Cancelar lo deja como estaba
    b.tecla(Key::F(5), false, false, false);
    assert!(matches!(b.n.aviso, Some(Aviso::Reenviar(0))));
    b.foto(900, 600);
    let r = rect_aviso_no(rect_aviso(b.n.pagina()));
    b.raton(MouseEv::Down { x: r.x + 4, y: r.y + 4 });
    assert!(b.n.aviso.is_none());
    // atrás, a una página que no vino de un POST, no pregunta; adelante sí
    b.tecla(Key::Left, false, false, true);
    assert!(b.n.aviso.is_none() && b.n.pest[0].titulo == "Form");
    b.tecla(Key::Right, false, false, true);
    assert!(matches!(b.n.aviso, Some(Aviso::Reenviar(1))));
    b.foto(900, 600);
    let r = rect_aviso_si(rect_aviso(b.n.pagina()));
    b.raton(MouseEv::Down { x: r.x + 4, y: r.y + 4 });
    assert!(b.n.aviso.is_none() && b.n.pest[0].pide_post(0), "se reenvió: la entrada sigue siendo un POST");
}

#[test]
fn una_pagina_publica_no_envia_a_la_red_local() {
    let mut t = Pestana::nueva(0, crate::red::Red::nueva());
    let mut sys = sistema();
    t.base = String::from("https://ejemplo.com/f.html");
    t.enviar(&mut sys, Envio { metodo: Metodo::Post, url: String::from("http://192.168.1.1/admin"), cuerpo: Some(b"a=1".to_vec()), tiene_password: false }, 0);
    assert!(t.titulo.contains("Bloqueado") && !t.cargando(), "{:?}", t.titulo);
}

// ---------------------------------------------------------------- historial, favoritos e incógnito

const DIR_DATOS: &str = "C:\\Usuarios\\Usuario\\AppData\\Navegador";

/// Una página de la red recién mostrada (lo que deja `Pestana::terminar`) y un tick para que el navegador la recoja.
fn visita(b: &mut Banco, url: &str, titulo: &str) {
    let a = b.n.act;
    let t = &mut b.n.pest[a];
    t.url = String::from(url);
    t.titulo = String::from(titulo);
    t.visita = true;
    b.tick();
}

/// Deja pasar el retraso de escritura en disco.
fn espera_disco(b: &mut Banco) {
    for _ in 0..100 {
        b.tick();
    }
}

/// Ejecuta `f` con un `Ctx` y devuelve lo que ha pedido (menú y ruta a abrir).
fn pide(b: &mut Banco, f: impl FnOnce(&mut Navegador, &mut Ctx)) -> (Option<MenuRequest>, Option<String>) {
    let mut cx = Ctx::new(&mut b.sys, &mut b.th, b.ms);
    f(&mut b.n, &mut cx);
    (cx.menu.take(), cx.open.take())
}

#[test]
fn el_historial_se_apunta_se_guarda_y_se_recupera() {
    let mut b = Banco::nuevo();
    visita(&mut b, "https://a.com/x", "Página\tuno ñ");
    visita(&mut b, "https://b.org/", "Dos");
    assert_eq!(b.n.datos.hist.v.len(), 2);
    assert!(b.sys.files.read(&format!("{DIR_DATOS}\\historial")).is_err(), "no se escribe en cada cambio");
    espera_disco(&mut b);
    let texto = String::from_utf8(b.sys.files.read(&format!("{DIR_DATOS}\\historial")).unwrap()).unwrap();
    assert_eq!(texto.lines().count(), 2);
    let mut otro = Navegador::nuevo();
    otro.cargar_datos(&mut b.sys);
    assert_eq!(otro.datos.hist.v, b.n.datos.hist.v);
    // Una página de error, un formulario POST o una página del disco no se apuntan: no ponen `visita`.
    assert!(!b.n.pest[0].visita);
}

#[test]
fn incognito_no_escribe_historial_ni_lo_sugiere() {
    let mut b = Banco::nuevo();
    visita(&mut b, "https://a.com/", "Antes");
    espera_disco(&mut b);
    // Una ventana nueva avisada con la marca del escritorio (comparte el disco).
    let mut c = Banco::nuevo();
    c.sys = std::mem::take(&mut b.sys);
    c.abrir(&format!("{DIR_DATOS}\\incognito.nfinc"));
    assert!(c.n.es_incognito() && c.n.title().ends_with("(incógnito)"));
    visita(&mut c, "https://secreto.com/", "Secreto");
    assert!(c.n.datos.hist.v.is_empty(), "ni se lee ni se apunta");
    espera_disco(&mut c);
    let texto = String::from_utf8(c.sys.files.read(&format!("{DIR_DATOS}\\historial")).unwrap()).unwrap();
    assert!(texto.contains("a.com") && !texto.contains("secreto"), "{texto}");
    // Los favoritos sí se pueden añadir y usar; el aspecto es oscuro.
    c.tecla(Key::Char('d'), true, false, false);
    assert_eq!(c.n.datos.favs.v.len(), 1);
    assert!(c.n.es_favorito_sitio("secreto.com"));
    let s = c.foto(1024, 700);
    assert!(s.px.iter().any(|&p| p == 0x202124) && s.px.iter().any(|&p| p == 0x35363A), "tira y barra oscuras");
    // Las sugerencias solo traen favoritos.
    c.tecla(Key::Char('l'), true, false, false);
    c.escribe("a");
    assert!(c.n.sug.is_empty());
    c.tecla(Key::Char('l'), true, false, false);
    c.escribe("sec");
    assert_eq!(c.n.sug.len(), 1);
}

#[test]
fn ctrl_mayus_n_y_el_menu_piden_una_ventana_de_incognito() {
    let mut b = Banco::nuevo();
    b.foto(1024, 700);
    let ev = KeyEvent { key: Key::Char('n'), ch: None, down: true, shift: true, ctrl: true, alt: false };
    let (_, abre) = pide(&mut b, |n, cx| n.key(&ev, cx));
    assert_eq!(abre.as_deref(), Some(format!("{DIR_DATOS}\\incognito.nfinc").as_str()));
    // Desde el menú de los tres puntos.
    let m = rect_menu(1024);
    let (menu, _) = pide(&mut b, |n, cx| n.mouse(MouseEv::Down { x: m.x + 5, y: m.y + 5 }, cx));
    let menu = menu.expect("el botón abre el menú");
    assert!(menu.items.iter().any(|i| i.id == M_INCOG && i.text.contains("incógnito")));
    let (_, abre) = pide(&mut b, |n, cx| n.menu_result(TAG_MENU, M_INCOG, cx));
    assert!(abre.is_some_and(|r| r.ends_with(".nfinc")));
    // La página de incógnito explica lo que pasa.
    let mut c = Banco::nuevo();
    c.n.poner_incognito();
    c.foto(1024, 700);
    assert!(c.n.lineas_incognito(c.n.pagina()).join(" ").contains("las cookies se borran al cerrar"));
}

#[test]
fn una_pagina_web_no_abre_ni_enlaza_las_internas() {
    let mut b = Banco::nuevo();
    visita(&mut b, "https://a.com/", "Antes");
    b.n.pest[0].url = String::from("https://web.com/");
    b.foto(900, 600);
    let n = b.n.datos.hist.v.len();
    for u in ["http://interno.invalid/historial?limpiar=todo&ok=1", "http://INTERNO.invalid/favoritos", "nightfury://historial"] {
        pide(&mut b, |n, cx| n.abrir_enlace(u, false, cx));
        let e = Envio { metodo: Metodo::Get, url: String::from(u), cuerpo: None, tiene_password: false };
        pide(&mut b, |n, cx| n.enviar(e, cx));
        assert!(b.n.pest[0].interna.is_none() && b.n.pest[0].url == "https://web.com/", "{u}");
    }
    assert_eq!(b.n.datos.hist.v.len(), n, "y no ha borrado nada");
    // Escrito en la barra por el usuario, sí se abre.
    b.tecla(Key::Char('l'), true, false, false);
    b.escribe("nightfury://historial");
    b.tecla(Key::Enter, false, false, false);
    b.foto(900, 600);
    assert_eq!((b.n.pest[0].url.as_str(), b.n.pest[0].titulo.as_str()), ("nightfury://historial", "Historial"));
}

#[test]
fn pagina_de_historial_busca_borra_y_limpia() {
    let mut b = Banco::nuevo();
    visita(&mut b, "https://a.com/", "Gatos");
    visita(&mut b, "https://b.org/", "Perros");
    b.n.pest[0].url = String::new();
    b.tecla(Key::Char('h'), true, false, false);
    b.foto(900, 600);
    assert_eq!(b.n.pest.len(), 1, "la pestaña vacía se aprovecha");
    assert_eq!(b.n.pest[0].url, "nightfury://historial");
    pide(&mut b, |n, cx| n.abrir_enlace("http://interno.invalid/historial?q=gat", false, cx));
    b.foto(900, 600);
    assert_eq!(b.n.pest[0].url, "nightfury://historial?q=gat");
    // Eliminar una entrada la quita y reemplaza la entrada del historial de la pestaña (no añade otra).
    let ts = b.n.datos.hist.v[0].ts;
    let antes = b.n.pest[0].puede_atras();
    pide(&mut b, |n, cx| n.abrir_enlace(&format!("http://interno.invalid/historial?borrar={ts}&q=gat"), false, cx));
    b.foto(900, 600);
    assert_eq!((b.n.datos.hist.v.len(), b.n.pest[0].url.as_str(), b.n.pest[0].puede_atras()), (1, "nightfury://historial?q=gat", antes));
    // Borrar todo pide confirmación antes.
    pide(&mut b, |n, cx| n.abrir_enlace("http://interno.invalid/historial?limpiar=todo", false, cx));
    b.foto(900, 600);
    assert_eq!((b.n.datos.hist.v.len(), b.n.pest[0].titulo.as_str()), (1, "Borrar datos de navegación"));
    pide(&mut b, |n, cx| n.abrir_enlace("http://interno.invalid/historial?limpiar=todo&ok=1", false, cx));
    b.foto(900, 600);
    assert!(b.n.datos.hist.v.is_empty());
    assert_eq!(b.n.pest[0].url, "nightfury://historial");
}

#[test]
fn favoritos_estrella_barra_accesos_y_carpetas() {
    let mut b = Banco::nuevo();
    b.foto(1024, 700);
    assert_eq!(b.n.pagina().y, TAB_H + TOOL_H, "sin favoritos no hay barra");
    b.n.pest[0].url = String::from("https://a.com/");
    b.n.pest[0].titulo = String::from("Sitio A");
    b.foto(1024, 700);
    let e = rect_estrella(rect_omnibox(1024));
    b.raton(MouseEv::Down { x: e.x + 10, y: e.y + 10 });
    assert_eq!(b.n.datos.favs.v.len(), 1);
    assert_eq!(b.n.pagina().y, TAB_H + TOOL_H + FAV_H, "ya hay barra de favoritos");
    assert!(b.n.es_favorito_sitio("a.com") && !b.n.es_favorito_sitio("b.com"));
    b.foto(1024, 700);
    b.raton(MouseEv::Down { x: e.x + 10, y: e.y + 10 });
    assert!(b.n.datos.favs.v.is_empty());
    b.tecla(Key::Char('d'), true, false, false);
    assert_eq!(b.n.datos.favs.v.len(), 1, "Ctrl+D también");
    // Ctrl+Mayús+B oculta y muestra la barra.
    b.tecla(Key::Char('b'), true, true, false);
    assert_eq!(b.n.pagina().y, TAB_H + TOOL_H);
    b.tecla(Key::Char('b'), true, true, false);
    assert_eq!(b.n.pagina().y, TAB_H + TOOL_H + FAV_H);
    // Una carpeta con dos favoritos: el clic abre su menú; el elemento elegido abre el favorito.
    let c = b.n.datos.favs.nueva_carpeta("Trabajo").unwrap();
    let f1 = b.n.datos.favs.anadir("https://c.com/", "C", c).unwrap();
    b.n.datos.favs.anadir("https://d.com/", "D", c).unwrap();
    b.foto(1024, 700);
    let (items, _) = b.n.items_fav();
    let r = items.iter().find(|(_, id)| *id == c).unwrap().0;
    let (menu, _) = pide(&mut b, |n, cx| n.mouse(MouseEv::Down { x: r.x + 5, y: r.y + 5 }, cx));
    assert_eq!(menu.unwrap().items.iter().map(|i| i.id).collect::<Vec<_>>(), [f1, f1 + 1]);
    b.tecla(Key::Char('t'), true, false, false);
    pide(&mut b, |n, cx| n.menu_result(TAG_FAV, f1, cx));
    assert_eq!(b.n.pest[1].url_visible(), "https://c.com/");
    // La pestaña nueva enseña los accesos (los favoritos, no las carpetas).
    b.tecla(Key::Char('t'), true, false, false);
    b.foto(1024, 700);
    let accesos = b.n.accesos(b.n.pagina());
    assert_eq!(accesos.len(), 3);
    let t = accesos[0].0;
    // Soltar Ctrl (el escritorio lo avisa con `Key::Other`): el clic abre en la misma pestaña.
    b.tecla(Key::Other, false, false, false);
    b.raton(MouseEv::Down { x: t.x + 10, y: t.y + 10 });
    assert_eq!(b.n.pest[2].url_visible(), "https://a.com/", "el primero en el orden de la barra");
}

#[test]
fn sugerencias_de_la_barra_con_flechas_y_enter() {
    let mut b = Banco::nuevo();
    visita(&mut b, "https://www.ejemplo.com/gatos", "Gatos graciosos");
    visita(&mut b, "https://otra.org/", "Otra");
    b.n.datos.favs.anadir("https://favorita.net/", "Mi favorita", 0);
    b.tecla(Key::Char('t'), true, false, false);
    b.foto(1024, 700);
    b.escribe("g");
    assert_eq!(b.n.sug.len(), 2, "«gatos» y «org»");
    b.escribe("a");
    assert_eq!(b.n.sug.len(), 1, "solo «gatos»");
    b.tecla(Key::Down, false, false, false);
    assert_eq!(b.n.sug_sel, Some(0));
    b.foto(1024, 700);
    b.tecla(Key::Enter, false, false, false);
    assert_eq!(b.n.pest[b.n.act].url_visible(), "https://www.ejemplo.com/gatos");
    assert!(b.n.sug.is_empty());
    // Escape cierra la lista sin salir de la barra; un clic en una sugerencia la abre.
    b.tecla(Key::Char('t'), true, false, false);
    b.escribe("fav");
    assert_eq!(b.n.sug.len(), 1);
    b.tecla(Key::Escape, false, false, false);
    assert!(b.n.sug.is_empty() && b.n.foco == Foco::Barra);
    b.escribe("o");
    b.foto(1024, 700);
    let r = rect_sug(0, 1024);
    b.raton(MouseEv::Down { x: r.x + 20, y: r.y + 10 });
    assert_eq!(b.n.pest[b.n.act].url_visible(), "https://favorita.net/");
}

// ---------------------------------------------------------------- red simulada (cookies y descargas)

use crate::cookies::Contexto;
use nf_core::net::{ConnInfo, Datagram, ListenId, NetApi, NetError, NetStatus, PingId, PingReply, Proto, QueryId, SockId, State, TcpState, UdpId};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Lo que ha recibido el servidor: (nombre, ruta, petición completa).
type Registro = Rc<RefCell<Vec<(String, String, String)>>>;

struct Conex {
    host: String,
    req: Vec<u8>,
    resp: Option<Vec<u8>>,
    pos: usize,
    cuelga: bool,
}

/// Una pila de red de mentira: cada petición se contesta con lo que devuelva `sirve(nombre, ruta)`. Si la respuesta
/// lleva la cabecera `X-Cuelga`, el servidor no cierra (la descarga queda a medias).
struct FalsoNet {
    sirve: Box<dyn Fn(&str, &str) -> Vec<u8>>,
    hosts: Vec<String>,
    socks: Vec<Conex>,
    log: Registro,
    cortes: Rc<Cell<u32>>,
}

impl FalsoNet {
    fn nuevo(sirve: impl Fn(&str, &str) -> Vec<u8> + 'static, log: Registro, cortes: Rc<Cell<u32>>) -> FalsoNet {
        FalsoNet { sirve: Box::new(sirve), hosts: Vec::new(), socks: Vec::new(), log, cortes }
    }
    fn sock(&mut self, s: SockId) -> &mut Conex {
        &mut self.socks[s.0 as usize]
    }
    fn conectar(&mut self, dst: [u8; 4]) -> Result<SockId, NetError> {
        let host = self.hosts[dst[3] as usize].clone();
        self.socks.push(Conex { host, req: Vec::new(), resp: None, pos: 0, cuelga: false });
        Ok(SockId(self.socks.len() as u32 - 1))
    }
}

impl NetApi for FalsoNet {
    fn status(&mut self) -> NetStatus {
        NetStatus { state: State::NoAdapter, adapter: String::new(), mac: [0; 6], speed_mbps: 0, full_duplex: false, ip: None, mask: None, gateway: None, dns: Vec::new(), dhcp_server: None, lease_s: 0, lease_since_ms: 0 }
    }
    fn renew(&mut self) {}
    fn ping(&mut self, _: [u8; 4], _: u16, _: u8) -> Result<PingId, NetError> {
        Err(NetError::NoAdapter)
    }
    fn ping_poll(&mut self, _: PingId) -> Option<Result<PingReply, NetError>> {
        None
    }
    fn resolve(&mut self, name: &str) -> Result<QueryId, NetError> {
        self.hosts.push(String::from(name));
        Ok(QueryId(self.hosts.len() as u32 - 1))
    }
    fn resolve_poll(&mut self, id: QueryId) -> Option<Result<Vec<[u8; 4]>, NetError>> {
        Some(Ok(vec![[10, 0, 0, id.0 as u8]]))
    }
    fn tcp_connect(&mut self, dst: [u8; 4], _: u16) -> Result<SockId, NetError> {
        self.conectar(dst)
    }
    fn tcp_connect_tls(&mut self, dst: [u8; 4], _: u16, _: &str, _: bool) -> Result<SockId, NetError> {
        self.conectar(dst)
    }
    fn tcp_state(&mut self, s: SockId) -> TcpState {
        let c = self.sock(s);
        match &c.resp {
            Some(r) if c.pos >= r.len() && !c.cuelga => TcpState::PeerClosed,
            _ => TcpState::Open,
        }
    }
    fn tcp_send(&mut self, s: SockId, data: &[u8]) -> Result<usize, NetError> {
        self.sock(s).req.extend_from_slice(data);
        let c = &self.socks[s.0 as usize];
        if c.resp.is_none() && c.req.windows(4).any(|w| w == b"\r\n\r\n") {
            let texto = String::from_utf8_lossy(&c.req).into_owned();
            let ruta = String::from(texto.split_whitespace().nth(1).unwrap_or("/"));
            let host = c.host.clone();
            let r = (self.sirve)(&host, &ruta);
            self.log.borrow_mut().push((host, ruta, texto));
            let c = self.sock(s);
            c.cuelga = r.windows(8).any(|w| w == b"X-Cuelga");
            c.resp = Some(r);
        }
        Ok(data.len())
    }
    fn tcp_recv(&mut self, s: SockId, buf: &mut [u8]) -> Result<usize, NetError> {
        let c = self.sock(s);
        let Some(r) = &c.resp else { return Ok(0) };
        let n = (r.len() - c.pos).min(buf.len()).min(700);
        buf[..n].copy_from_slice(&r[c.pos..c.pos + n]);
        c.pos += n;
        Ok(n)
    }
    fn tcp_close(&mut self, _: SockId) {}
    fn tcp_abort(&mut self, _: SockId) {
        self.cortes.set(self.cortes.get() + 1);
    }
    fn udp_bind(&mut self, _: u16) -> Result<UdpId, NetError> {
        Err(NetError::NoAdapter)
    }
    fn udp_send(&mut self, _: UdpId, _: [u8; 4], _: u16, _: &[u8]) -> Result<(), NetError> {
        Err(NetError::NoAdapter)
    }
    fn udp_recv(&mut self, _: UdpId) -> Result<Option<Datagram>, NetError> {
        Ok(None)
    }
    fn udp_close(&mut self, _: UdpId) {}
    fn tcp_listen(&mut self, _: u16) -> Result<ListenId, NetError> {
        Err(NetError::NoAdapter)
    }
    fn tcp_accept(&mut self, _: ListenId) -> Result<Option<(SockId, [u8; 4], u16)>, NetError> {
        Ok(None)
    }
    fn tcp_unlisten(&mut self, _: ListenId) {}
    fn connections(&mut self) -> Vec<ConnInfo> {
        Vec::new()
    }
    fn set_owner(&mut self, _: Proto, _: u32, _: u64) {}
}

/// `TestPlatform` con una red de mentira enganchada (el resto, tal cual).
#[derive(Default)]
struct PlatRed {
    base: TestPlatform,
    red: Option<FalsoNet>,
}

impl std::ops::Deref for PlatRed {
    type Target = TestPlatform;
    fn deref(&self) -> &TestPlatform {
        &self.base
    }
}
impl std::ops::DerefMut for PlatRed {
    fn deref_mut(&mut self) -> &mut TestPlatform {
        &mut self.base
    }
}

impl Platform for PlatRed {
    fn now(&mut self) -> nf_core::platform::DateTime {
        self.base.now()
    }
    fn uptime_ms(&self) -> u64 {
        self.base.uptime_ms()
    }
    fn mem(&self) -> nf_core::platform::MemInfo {
        self.base.mem()
    }
    fn cpu(&self) -> String {
        self.base.cpu()
    }
    fn screen(&self) -> (u32, u32) {
        self.base.screen()
    }
    fn reboot(&mut self) {}
    fn shutdown(&mut self) {}
    fn files(&mut self) -> &mut nf_core::files::Files {
        &mut self.base.files
    }
    fn entropy(&mut self) -> u64 {
        self.base.entropy()
    }
    fn pool(&mut self) -> &mut nf_core::keys::Pool {
        &mut self.base.pool
    }
    fn net(&mut self) -> Option<&mut dyn NetApi> {
        self.red.as_mut().map(|n| n as &mut dyn NetApi)
    }
}

fn resp(estado: &str, cab: &[&str], cuerpo: &[u8]) -> Vec<u8> {
    let mut r = format!("HTTP/1.1 {estado}\r\n");
    for c in cab {
        r.push_str(c);
        r.push_str("\r\n");
    }
    r.push_str(&format!("Content-Length: {}\r\n\r\n", cuerpo.len()));
    let mut v = r.into_bytes();
    v.extend_from_slice(cuerpo);
    v
}

/// Una respuesta que anuncia 100 000 bytes y envía solo `cuerpo`; `X-Cuelga` hace que el servidor no cierre.
fn cuelga(cab: &[&str], cuerpo: &[u8]) -> Vec<u8> {
    let mut r = String::from("HTTP/1.1 200 OK\r\n");
    for c in cab {
        r.push_str(c);
        r.push_str("\r\n");
    }
    r.push_str("X-Cuelga: 1\r\nContent-Length: 100000\r\n\r\n");
    let mut v = r.into_bytes();
    v.extend_from_slice(cuerpo);
    v
}

fn html(cuerpo: &str) -> Vec<u8> {
    resp("200 OK", &["Content-Type: text/html"], cuerpo.as_bytes())
}

impl Banco {
    /// Un banco con la red de mentira; devuelve también el registro de peticiones y el contador de conexiones cortadas.
    fn con_red(sirve: impl Fn(&str, &str) -> Vec<u8> + 'static) -> (Banco, Registro, Rc<Cell<u32>>) {
        let mut b = Banco::nuevo();
        let (log, cortes) = (Registro::default(), Rc::new(Cell::new(0)));
        b.sys.red = Some(FalsoNet::nuevo(sirve, log.clone(), cortes.clone()));
        b.tick();
        (b, log, cortes)
    }
    /// Escribe la URL en la pestaña como si se tecleara y espera a que termine de cargar.
    fn ve_a(&mut self, url: &str) {
        self.ir_desde(url, None, false);
    }
    fn ir_desde(&mut self, url: &str, ini: Option<&str>, forzar: bool) {
        let ms = self.ms;
        self.n.pest[0].ir_desde(&mut self.sys, Entrada::Web(String::from(url)), ms, ini.map(String::from), forzar);
        self.espera_carga();
    }
    fn espera_carga(&mut self) {
        for _ in 0..200 {
            self.tick();
            if !self.n.pest[0].cargando() {
                return;
            }
        }
        panic!("la carga no termina");
    }
}

/// La cabecera `Cookie` de la última petición a `host` + `ruta`.
fn cookie_enviada(log: &Registro, host: &str, ruta: &str) -> Option<String> {
    let l = log.borrow();
    let (_, _, req) = l.iter().rev().find(|(h, r, _)| h == host && r == ruta)?;
    req.lines().find_map(|x| x.strip_prefix("Cookie: ")).map(String::from)
}

fn siembra(b: &mut Banco, url: &str, set: &[&str]) {
    let utc = b.n.red.borrow_mut().ahora_utc(&mut b.sys);
    for s in set {
        assert!(b.n.red.borrow_mut().tarro.guardar_desde_respuesta(url, s, utc, Contexto::PrimeraParte), "{s}");
    }
}

fn descargas_dir(b: &mut Banco) -> String {
    b.sys.fs().std_dir(nf_core::files::Std::Downloads)
}

#[test]
fn las_cookies_viajan_segun_el_contexto() {
    let (mut b, log, _) = Banco::con_red(|host, ruta| match (host, ruta) {
        ("b.com", "/") => html("<title>B</title><img src=\"https://a.com/i.png\">"),
        (_, "/i.png") => resp("404 No", &[], b""),
        _ => html("<title>x</title><p>x"),
    });
    siembra(&mut b, "https://a.com/", &["s=1; SameSite=Strict", "l=1; SameSite=Lax", "n=1; SameSite=None; Secure"]);
    // Escrita en la barra: todas.
    b.ve_a("https://a.com/p1");
    assert_eq!(cookie_enviada(&log, "a.com", "/p1").as_deref(), Some("s=1; l=1; n=1"));
    // Enlace desde otro sitio: sin Strict.
    b.ir_desde("https://a.com/p2", Some("https://b.com/"), false);
    assert_eq!(cookie_enviada(&log, "a.com", "/p2").as_deref(), Some("l=1; n=1"));
    // Misma web: todas otra vez.
    b.ir_desde("https://a.com/p3", Some("https://a.com/p1"), false);
    assert_eq!(cookie_enviada(&log, "a.com", "/p3").as_deref(), Some("s=1; l=1; n=1"));
    // POST a otro sitio: solo SameSite=None.
    b.n.pest[0].base = String::from("https://b.com/");
    let ms = b.ms;
    b.n.pest[0].enviar(&mut b.sys, Envio { metodo: Metodo::Post, url: String::from("https://a.com/p4"), cuerpo: Some(b"x=1".to_vec()), tiene_password: false }, ms);
    b.espera_carga();
    assert_eq!(cookie_enviada(&log, "a.com", "/p4").as_deref(), Some("n=1"));
    // La imagen de un tercero no lleva cookies, aunque la página sea de otro sitio.
    b.ve_a("https://b.com/");
    for _ in 0..20 {
        b.tick();
    }
    assert!(log.borrow().iter().any(|(h, r, _)| h == "a.com" && r == "/i.png"), "se pidió la imagen");
    assert_eq!(cookie_enviada(&log, "a.com", "/i.png"), None);
}

#[test]
fn set_cookie_de_una_redireccion_se_guarda_y_se_envia_al_siguiente_salto() {
    let (mut b, log, _) = Banco::con_red(|host, ruta| match (host, ruta) {
        ("r.com", "/a") => resp("302 Found", &["Location: https://r.com/b", "Set-Cookie: sid=9; Max-Age=100", "Set-Cookie: otra=2; Max-Age=100"], b""),
        _ => html("<title>fin</title><p>fin"),
    });
    b.ve_a("https://r.com/a");
    assert_eq!(b.n.pest[0].titulo, "fin");
    assert_eq!(cookie_enviada(&log, "r.com", "/a"), None);
    assert_eq!(cookie_enviada(&log, "r.com", "/b").as_deref(), Some("sid=9; otra=2"));
    assert_eq!(b.n.red.borrow().cuantas("r.com"), 2);
}

#[test]
fn solo_se_guardan_en_disco_las_cookies_de_favoritos_y_recordados() {
    let (mut b, _, _) = Banco::con_red(|_, _| resp("200 OK", &["Content-Type: text/html", "Set-Cookie: k=1; Max-Age=1000", "Set-Cookie: sesion=1"], b"<p>x"));
    for u in ["https://fav.com/", "https://rec.com/", "https://otro.com/"] {
        b.ve_a(u);
    }
    assert_eq!(b.n.red.borrow().tarro.len(), 6);
    b.n.datos.favs.anadir("https://fav.com/", "F", 0);
    let ms = b.ms;
    b.n.red.borrow_mut().alternar_recordar("rec.com", ms);
    espera_disco(&mut b);
    let texto = String::from_utf8(b.sys.files.read(&format!("{DIR_DATOS}\\cookies")).unwrap()).unwrap();
    assert!(texto.contains("fav.com") && texto.contains("rec.com") && !texto.contains("otro.com"), "{texto}");
    assert!(!texto.contains("sesion"), "las de sesión nunca van a disco");
    assert!(String::from_utf8(b.sys.files.read(&format!("{DIR_DATOS}\\recordar")).unwrap()).unwrap().contains("rec.com"));
    // Una ventana nueva las recupera.
    let mut otro = Navegador::nuevo();
    otro.cargar_datos(&mut b.sys);
    assert_eq!(otro.red.borrow().cuantas("fav.com"), 1);
    assert_eq!(otro.red.borrow().cuantas("otro.com"), 0);
}

#[test]
fn incognito_no_lee_ni_escribe_cookies() {
    let (mut b, _, _) = Banco::con_red(|_, _| resp("200 OK", &["Content-Type: text/html", "Set-Cookie: k=1; Max-Age=1000"], b"<p>x"));
    b.ve_a("https://a.com/");
    b.n.datos.favs.anadir("https://a.com/", "A", 0);
    espera_disco(&mut b);
    let antes = b.sys.files.read(&format!("{DIR_DATOS}\\cookies")).unwrap();
    assert!(String::from_utf8(antes.clone()).unwrap().contains("a.com"));
    // Ventana de incógnito sobre el mismo disco.
    let mut c = Banco::nuevo();
    c.sys = std::mem::take(&mut b.sys);
    let log = Registro::default();
    c.sys.red = Some(FalsoNet::nuevo(|_, _| resp("200 OK", &["Content-Type: text/html", "Set-Cookie: z=1; Max-Age=1000"], b"<p>x"), log.clone(), Rc::default()));
    c.abrir(&format!("{DIR_DATOS}\\incognito.nfinc"));
    c.tick();
    assert_eq!(c.n.red.borrow().cuantas("a.com"), 0, "no lee las de la ventana normal");
    c.ve_a("https://a.com/");
    c.n.datos.favs.anadir("https://a.com/", "A", 0);
    espera_disco(&mut c);
    assert_eq!(c.n.red.borrow().cuantas("a.com"), 1);
    assert_eq!(c.sys.files.read(&format!("{DIR_DATOS}\\cookies")).unwrap(), antes, "el disco no cambia");
    assert!(!log.borrow().iter().any(|(_, _, r)| r.contains("Cookie: k=1")));
}

fn servidor_de_descargas(host: &str, ruta: &str) -> Vec<u8> {
    let cuerpo = vec![b'x'; 3000];
    match (host, ruta) {
        ("d.com", "/f.zip") => resp("200 OK", &["Content-Type: application/zip", "Content-Disposition: attachment; filename=\"../a b.zip\""], &cuerpo),
        // Estas dos prometen más de lo que envían y el servidor no cierra: la descarga queda a medias.
        ("d.com", "/prog") => cuelga(&["Content-Type: application/octet-stream", "Content-Disposition: attachment; filename*=UTF-8''juego%20%C3%B1.nfx"], &cuerpo),
        ("d.com", "/largo") => cuelga(&["Content-Type: application/zip", "Content-Disposition: attachment; filename=\"g.zip\""], &cuerpo),
        _ => html("<title>Inicio</title><p>hola"),
    }
}

#[test]
fn una_descarga_pregunta_y_se_guarda_en_descargas_sin_pisar_nada() {
    let (mut b, _, _) = Banco::con_red(servidor_de_descargas);
    b.ve_a("https://otro.com/");
    assert_eq!(b.n.pest[0].titulo, "Inicio");
    let dir = descargas_dir(&mut b);
    for ronda in 0..2 {
        b.ve_a("https://d.com/f.zip");
        assert_eq!(b.n.pest[0].titulo, "Inicio", "la página no cambia");
        assert_eq!(b.n.pest[0].url, "https://otro.com/");
        let (txt, aviso) = b.n.red.borrow().descargas.pregunta().map(|i| i.pregunta()).expect("pregunta");
        assert_eq!(txt, "¿Descargar a b.zip (2,9 KB) de d.com?");
        assert!(aviso.is_none());
        for _ in 0..10 {
            b.tick();
        }
        if ronda == 0 {
            assert!(!b.sys.files.exists(&format!("{dir}\\a b.zip")), "nada se escribe antes de aceptar");
        }
        b.n.red.borrow_mut().descargas.responder(&mut b.sys, true);
        for _ in 0..50 {
            b.tick();
        }
        assert_eq!(b.n.red.borrow().descargas.items.last().unwrap().estado, Estado::Hecha);
    }
    assert_eq!(b.sys.files.read(&format!("{dir}\\a b.zip")).unwrap(), vec![b'x'; 3000]);
    assert_eq!(b.sys.files.read(&format!("{dir}\\a b (1).zip")).unwrap().len(), 3000);
}

#[test]
fn un_programa_avisa_y_cancelar_no_escribe_nada() {
    let (mut b, _, cortes) = Banco::con_red(servidor_de_descargas);
    b.ve_a("https://d.com/prog");
    let (txt, aviso) = b.n.red.borrow().descargas.pregunta().map(|i| i.pregunta()).expect("pregunta");
    assert!(txt.contains("juego ñ.nfx") && txt.contains("d.com"), "{txt}");
    assert_eq!(aviso, Some("Es un programa. Ábrelo solo si confías en este sitio."));
    let antes = cortes.get();
    b.n.red.borrow_mut().descargas.responder(&mut b.sys, false);
    assert!(cortes.get() > antes, "se corta la conexión");
    for _ in 0..20 {
        b.tick();
    }
    let dir = descargas_dir(&mut b);
    assert!(b.sys.files.list(&dir).map_or(true, |l| l.is_empty()), "no se escribió nada");
    assert!(b.n.red.borrow().descargas.items.is_empty());
}

#[test]
fn cancelar_una_descarga_en_curso_no_deja_fichero() {
    let (mut b, _, _) = Banco::con_red(servidor_de_descargas);
    b.ve_a("https://d.com/largo");
    b.n.red.borrow_mut().descargas.responder(&mut b.sys, true);
    for _ in 0..10 {
        b.tick();
    }
    let id = {
        let r = b.n.red.borrow();
        let it = r.descargas.items.last().unwrap();
        assert_eq!(it.estado, Estado::Bajando);
        it.id
    };
    b.n.red.borrow_mut().descargas.cancelar(&mut b.sys, id);
    for _ in 0..10 {
        b.tick();
    }
    assert_eq!(b.n.red.borrow().descargas.items.last().unwrap().estado, Estado::Cancelada);
    let dir = descargas_dir(&mut b);
    assert!(b.sys.files.list(&dir).map_or(true, |l| l.is_empty()));
}

#[test]
fn lo_mostrable_se_muestra_y_el_atributo_download_fuerza_la_descarga() {
    let (mut b, _, _) = Banco::con_red(servidor_de_descargas);
    b.ve_a("https://d.com/");
    assert_eq!(b.n.pest[0].titulo, "Inicio");
    assert!(b.n.red.borrow().descargas.pregunta().is_none(), "una página HTML no se descarga");
    // Un enlace marcado `download` descarga aunque el tipo sea mostrable.
    b.ir_desde("https://d.com/", None, true);
    assert!(b.n.red.borrow().descargas.pregunta().is_some());
}

#[test]
fn descarga_hacia_la_red_local_desde_una_pagina_publica_se_bloquea() {
    let (mut b, log, _) = Banco::con_red(servidor_de_descargas);
    b.ve_a("https://d.com/");
    b.ir_desde("http://192.168.1.1/f.zip", Some("https://d.com/"), true);
    assert!(b.n.red.borrow().descargas.pregunta().is_none());
    assert!(!log.borrow().iter().any(|(h, _, _)| h == "192.168.1.1"), "ni se conecta");
}

#[test]
fn borrar_todo_vacia_el_tarro_y_el_disco() {
    let (mut b, _, _) = Banco::con_red(|_, _| resp("200 OK", &["Content-Type: text/html", "Set-Cookie: k=1; Max-Age=1000"], b"<p>x"));
    b.ve_a("https://a.com/");
    b.n.datos.favs.anadir("https://a.com/", "A", 0);
    espera_disco(&mut b);
    assert!(String::from_utf8(b.sys.files.read(&format!("{DIR_DATOS}\\cookies")).unwrap()).unwrap().contains("a.com"));
    let ms = b.ms;
    b.n.red.borrow_mut().borrar_todo(ms);
    espera_disco(&mut b);
    assert_eq!(b.n.red.borrow().tarro.len(), 0);
    assert!(!String::from_utf8(b.sys.files.read(&format!("{DIR_DATOS}\\cookies")).unwrap()).unwrap().contains("a.com"));
}
