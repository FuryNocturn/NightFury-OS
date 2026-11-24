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
    sys: TestPlatform,
    th: Theme,
    ms: u64,
}

impl Banco {
    fn nuevo() -> Banco {
        let mut n = Navegador::nuevo();
        con_letras(&mut n);
        Banco { n, sys: sistema(), th: Theme::default(), ms: 1000 }
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
    let mut t = Pestana::nueva(0);
    let mut sys = sistema();
    t.base = String::from("https://ejemplo.com/f.html");
    t.enviar(&mut sys, Envio { metodo: Metodo::Post, url: String::from("http://192.168.1.1/admin"), cuerpo: Some(b"a=1".to_vec()), tiene_password: false }, 0);
    assert!(t.titulo.contains("Bloqueado") && !t.cargando(), "{:?}", t.titulo);
}
