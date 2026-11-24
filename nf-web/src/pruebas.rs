// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Pruebas del motor con una fuente falsa de ancho fijo: 0,5·px por carácter, ascendente 0,8·px y
//! descendente 0,2·px (a 16 px: 8 px por letra, línea de 18 px).
use crate::css;
use crate::estilo::Display;
use crate::html::{self, Dato};
use crate::maqueta::Item;
use crate::url::resolver;
use crate::valores::{color, largo, Col, Ctx, Len};
use crate::*;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use nf_core::geom::Rect;

struct Falsa;

impl Fuentes for Falsa {
    fn medir(&mut self, _l: Letra, px: f32, t: &str) -> f32 {
        t.chars().count() as f32 * px * 0.5
    }
    fn metricas(&mut self, _l: Letra, px: f32) -> (f32, f32) {
        (px * 0.8, px * 0.2)
    }
    fn dibujar(&mut self, s: &mut Surface, l: Letra, px: f32, x: f32, base: f32, c: u32, t: &str) {
        let w = self.medir(l, px, t);
        s.fill_rect(Rect::new(x as i32, (base - px * 0.8) as i32, w as i32, px as i32), c);
    }
}

const SIN_MARGEN: &str = "<style>body{margin:0}</style>";

fn doc(h: &str) -> Documento {
    Documento::cargar(h.as_bytes(), None, "http://ej.com/dir/pag.html")
}

fn maq(d: &mut Documento, ancho: i32) -> i32 {
    d.maquetar(ancho, &mut Falsa)
}

fn items(d: &Documento) -> &Vec<Item> {
    &d.res.as_ref().unwrap().items
}

/// (texto, x, línea base) de cada trozo de texto.
fn textos(d: &Documento) -> Vec<(String, i32, i32)> {
    items(d).iter().filter_map(|i| if let Item::Texto(t) = i { Some((t.s.clone(), t.x, t.y)) } else { None }).collect()
}

fn cajas(d: &Documento) -> Vec<Rect> {
    items(d).iter().filter_map(|i| if let Item::Caja { r, .. } = i { Some(*r) } else { None }).collect()
}

// ---------------------------------------------------------------------------------------------------------
// URLs

#[test]
fn url_rfc3986_5_4() {
    let b = "http://a/b/c/d;p?q";
    let casos = [
        ("g", "http://a/b/c/g"),
        ("./g", "http://a/b/c/g"),
        ("g/", "http://a/b/c/g/"),
        ("/g", "http://a/g"),
        ("//g", "http://g/"),
        ("?y", "http://a/b/c/d;p?y"),
        ("g?y", "http://a/b/c/g?y"),
        ("#s", "http://a/b/c/d;p?q"),
        ("g#s", "http://a/b/c/g"),
        ("g?y#s", "http://a/b/c/g?y"),
        (";x", "http://a/b/c/;x"),
        ("g;x", "http://a/b/c/g;x"),
        ("g;x?y#s", "http://a/b/c/g;x?y"),
        ("", "http://a/b/c/d;p?q"),
        (".", "http://a/b/c/"),
        ("./", "http://a/b/c/"),
        ("..", "http://a/b/"),
        ("../", "http://a/b/"),
        ("../g", "http://a/b/g"),
        ("../..", "http://a/"),
        ("../../", "http://a/"),
        ("../../g", "http://a/g"),
        ("../../../g", "http://a/g"),
        ("../../../../g", "http://a/g"),
        ("/./g", "http://a/g"),
        ("/../g", "http://a/g"),
        ("g.", "http://a/b/c/g."),
        (".g", "http://a/b/c/.g"),
        ("g..", "http://a/b/c/g.."),
        ("..g", "http://a/b/c/..g"),
        ("./../g", "http://a/b/g"),
        ("./g/.", "http://a/b/c/g/"),
        ("g/./h", "http://a/b/c/g/h"),
        ("g/../h", "http://a/b/c/h"),
        ("g;x=1/./y", "http://a/b/c/g;x=1/y"),
        ("g;x=1/../y", "http://a/b/c/y"),
        ("g?y/./x", "http://a/b/c/g?y/./x"),
        ("g?y/../x", "http://a/b/c/g?y/../x"),
        ("g#s/./x", "http://a/b/c/g"),
        ("g#s/../x", "http://a/b/c/g"),
        ("https://x.org/a/../b", "https://x.org/b"),
        ("HTTP://X.ORG", "http://x.org/"),
        ("a b", "http://a/b/c/a%20b"),
    ];
    for (rel, esperado) in casos {
        assert_eq!(resolver(b, rel).as_deref(), Some(esperado), "rel = {rel:?}");
    }
    for rel in ["about:blank", "data:text/plain,hola", "javascript:void(0)", "mailto:a@b.c", "ftp://x/y", "file:///c"] {
        assert_eq!(resolver(b, rel), None, "rel = {rel:?}");
    }
    assert_eq!(resolver("about:blank", "x"), None);
    assert_eq!(resolver("http://a", "x").as_deref(), Some("http://a/x"));
}

// ---------------------------------------------------------------------------------------------------------
// HTML

fn texto_de(h: &str) -> String {
    let d = html::parsear(h);
    d.texto(0)
}

#[test]
fn entidades() {
    assert_eq!(texto_de("a&amp;b &lt;&gt; &quot;x&quot; &eacute;&Eacute;&ntilde;"), "a&b <> \"x\" éÉñ");
    assert_eq!(texto_de("&copy; &reg; &euro; &hellip; &mdash; &ndash; &lsquo;&rsquo;&ldquo;&rdquo; &nbsp;|"), "© ® € … — – ‘’“” \u{a0}|");
    assert_eq!(texto_de("&#233;&#xE9;&#x20AC;&#128512;"), "éé€😀");
    assert_eq!(texto_de("&#0;&#xD800;&#99999999;"), "\u{FFFD}\u{FFFD}\u{FFFD}");
    assert_eq!(texto_de("&#150;"), "–", "windows-1252 en referencias numéricas");
    assert_eq!(texto_de("&copy 2020 &amp"), "© 2020 &");
    assert_eq!(texto_de("&noexiste; & &#; &#x;"), "&noexiste; & &#; &#x;");
    assert_eq!(texto_de("&alpha;&Omega;"), "αΩ");
    let d = html::parsear("<a href=\"?a=1&amp;b=2&c=3\" title='&quot;x&quot;'>x</a>");
    let a = (0..d.nodos.len()).find(|&i| d.etq(i) == "a").unwrap();
    assert_eq!(d.attr(a, "href"), Some("?a=1&b=2&c=3"));
    assert_eq!(d.attr(a, "title"), Some("\"x\""));
}

#[test]
fn codificacion() {
    // ISO-8859-1 y windows-1252 por el Content-Type
    let latin = b"<p>caf\xe9 \x80 \x93x\x94";
    let d = Documento::cargar(latin, Some("text/html; charset=ISO-8859-1"), "http://a/");
    assert_eq!(d.dom.texto(0), "café € “x”");
    // por <meta charset>
    let d = Documento::cargar(b"<meta charset=\"windows-1252\"><p>a\xf1o", None, "http://a/");
    assert!(d.dom.texto(0).contains("año"));
    let d = Documento::cargar(b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=iso-8859-1\"><p>a\xf1o", None, "http://a/");
    assert!(d.dom.texto(0).contains("año"));
    // UTF-8 con BOM y sin él; bytes inválidos se sustituyen
    let d = Documento::cargar(b"\xEF\xBB\xBF<p>\xc3\xb1", None, "http://a/");
    assert_eq!(d.dom.texto(0), "ñ");
    let d = Documento::cargar(b"<p>\xc3\xb1\xff", None, "http://a/");
    assert_eq!(d.dom.texto(0), "ñ\u{FFFD}");
    // el Content-Type manda sobre el meta
    let d = Documento::cargar(b"<meta charset=iso-8859-1><p>\xc3\xb1", Some("text/html;charset=utf-8"), "http://a/");
    assert_eq!(d.dom.texto(0), "ñ");
}

fn etiquetas(d: &html::Dom) -> Vec<String> {
    (0..d.nodos.len()).filter_map(|i| d.elem(i).map(|e| e.etq.clone())).collect()
}

fn padre_de(d: &html::Dom, n: usize) -> String {
    d.etq(d.padre(n).unwrap()).to_string()
}

#[test]
fn arbol_mal_formado() {
    let d = html::parsear("<p>uno<p>dos<ul><li>a<li>b</ul><dl><dt>t<dd>d<dt>t2</dl><table><tr><td>x<td>y<tr><td>z</table>");
    let e = etiquetas(&d);
    assert_eq!(e.iter().filter(|x| *x == "p").count(), 2);
    // los dos p son hermanos, no anidados
    let ps: Vec<usize> = (0..d.nodos.len()).filter(|&i| d.etq(i) == "p").collect();
    assert_eq!(d.nodos[ps[0]].padre, d.nodos[ps[1]].padre);
    let lis: Vec<usize> = (0..d.nodos.len()).filter(|&i| d.etq(i) == "li").collect();
    assert_eq!(lis.len(), 2);
    assert_eq!(padre_de(&d, lis[1]), "ul");
    let tds: Vec<usize> = (0..d.nodos.len()).filter(|&i| d.etq(i) == "td").collect();
    assert_eq!(tds.len(), 3);
    assert!(tds.iter().all(|&t| padre_de(&d, t) == "tr"));
    let trs: Vec<usize> = (0..d.nodos.len()).filter(|&i| d.etq(i) == "tr").collect();
    assert_eq!(d.nodos[trs[0]].padre, d.nodos[trs[1]].padre);
    // dt/dd hermanos
    assert_eq!(e.iter().filter(|x| *x == "dt" || *x == "dd").count(), 3);

    // cierres sobrantes y mal anidados: sin pánico, el texto se conserva
    for h in ["</b></i></div>texto", "<b><i>a</b>b</i>c", "<div><span>x</div>y</span>", "<a><a>x</a></a>", "<<<>>><p<p>", "<p a='x"] {
        let d = html::parsear(h);
        let _ = d.texto(0);
    }
    assert_eq!(texto_de("<b><i>a</b>b</i>c"), "abc");
    // elementos vacíos
    let d = html::parsear("<p>a<br>b<img src=x>c<hr>d<input>e");
    assert!(d.nodos.iter().all(|n| match &n.dato {
        Dato::Elem(e) if matches!(e.etq.as_str(), "br" | "img" | "hr" | "input") => n.hijos.is_empty(),
        _ => true,
    }));
    assert_eq!(texto_de("<p>a<br>b<img src=x>c<hr>d<input>e"), "abcde");
}

#[test]
fn texto_crudo_y_comentarios() {
    let d = html::parsear("<script>if (a<b) {x='</p>'}</script><style>p{}</style><!-- <p>no</p> --><title>A &amp; B</title><textarea><b>x</b></textarea>ok<![CDATA[ <p>no</p> ]]>");
    assert!(!etiquetas(&d).contains(&"p".to_string()));
    assert_eq!(texto_de("<title>A &amp; B</title>").trim(), "A & B");
    let d = html::parsear("<textarea><b>x</b></textarea>");
    assert!(!etiquetas(&d).contains(&"b".to_string()));
    // <script> no se muestra, <noscript> sí, <template> y <head> no
    let mut d = doc(&format!("{SIN_MARGEN}<script>var x = 1;</script><noscript>sinjs</noscript><template>plantilla</template><p>visible"));
    maq(&mut d, 400);
    let t: Vec<String> = textos(&d).into_iter().map(|x| x.0).collect();
    assert!(t.contains(&"sinjs".to_string()) && t.contains(&"visible".to_string()));
    assert!(!t.iter().any(|x| x.contains("plantilla") || x.contains("var x")));
}

#[test]
fn titulo_base_y_recursos() {
    let h = "<html><head><title>  Mi \n  página </title><base href=\"/otra/\"><link rel=\"stylesheet\" href=\"a.css\">\
        <link rel=\"alternate stylesheet\" href=\"b.css\"><link rel=stylesheet href=\"p.css\" media=\"print\"><link rel=stylesheet href=\"s.css\" media=\"screen\">\
        <link rel=stylesheet href=\"a.css\"><link rel=icon href=i.png></head><body><img src=\"x.png\"><img src=\"x.png\"><img srcset=\"y.png 2x, z.png 4x\">\
        <img src=\"data:image/png;base64,AAAA\"><img src=\"//cdn.org/w.gif\"><a href=#x>x</a></body></html>";
    let d = doc(h);
    assert_eq!(d.titulo().as_deref(), Some("Mi página"));
    assert_eq!(d.url(), "http://ej.com/otra/");
    let r: Vec<(String, TipoRecurso)> = d.recursos().into_iter().map(|r| (r.url, r.tipo)).collect();
    assert_eq!(
        r,
        [
            ("http://ej.com/otra/a.css".to_string(), TipoRecurso::Css),
            ("http://ej.com/otra/s.css".to_string(), TipoRecurso::Css),
            ("http://ej.com/otra/x.png".to_string(), TipoRecurso::Imagen),
            ("http://ej.com/otra/y.png".to_string(), TipoRecurso::Imagen),
            ("http://cdn.org/w.gif".to_string(), TipoRecurso::Imagen),
        ]
    );
    assert_eq!(doc("<p>sin título").titulo(), None);
}

// ---------------------------------------------------------------------------------------------------------
// CSS

fn estilo_de(h: &str, ancho: i32, id: &str) -> estilo::Estilo {
    let mut d = doc(h);
    maq(&mut d, ancho);
    let n = (0..d.dom.nodos.len()).find(|&i| d.dom.attr(i, "id") == Some(id)).expect("id");
    d.estilos.as_ref().unwrap().0.de(n).clone()
}

#[test]
fn cascada_especificidad_e_importante() {
    let h = "<style>
        p { color: red } .c { color: blue } #i { color: green } p.c { color: yellow }
        div p { color: #111111 } #j { color: #222222 !important } #j { color: #333333 }
        .k { color: #444444 } p.k { color: #555555 }
        .m { color: #666666 } .m { color: #777777 }
        .n { color: #888888 !important } </style>
        <p id=a>x</p><p id=b class=c>x</p><p id=i class=c>x</p><div><p id=d>x</p></div><p id=j style=\"color:#999999\">x</p>
        <p id=k class=k>x</p><p id=m class=m>x</p><p id=n class=n style=\"color:#aaaaaa\">x</p><p id=o style=\"color:#abcdef\" class=c>x</p>";
    let c = |id| estilo_de(h, 400, id).color;
    assert_eq!(c("a"), 0xFF0000);
    assert_eq!(c("b"), 0xFFFF00, "p.c gana a .c y p");
    assert_eq!(c("i"), 0x008000, "el id gana");
    assert_eq!(c("d"), 0x111111, "div p gana a p");
    assert_eq!(c("j"), 0x222222, "!important gana al atributo style");
    assert_eq!(c("k"), 0x555555);
    assert_eq!(c("m"), 0x777777, "a igual especificidad, la última");
    assert_eq!(c("n"), 0x888888, "!important del autor gana al style normal");
    assert_eq!(c("o"), 0xABCDEF, "el atributo style gana a las reglas");
}

#[test]
fn selectores() {
    let h = "<style>
        li:first-child { color: #010101 } li:last-child { color: #020202 } ul > li + li { margin-left: 5px }
        a:link { color: #030303 } a:hover { color: #040404 } [data-x] { color: #050505 } [data-x=hola] { color: #060606 }
        [lang|=es] { background: #070707 } [href^=http] { border-left: 1px solid #080808 }
        h1, h2.t, .u { color: #0a0a0a } * { text-align: right } .p ~ .q { color: #0b0b0b } .x:not(.y) { color: #0c0c0c }
        div > p { color: #0d0d0d } @media (max-width: 300px) { #w { color: #0e0e0e } } @media print { #w { color: #0f0f0f } }
        @media screen and (min-width: 500px) { #w { color: #101010 } } @font-face { font-family: x } #w { }
        p::before { color: #111111 } </style>
        <ul><li id=l1>1<li id=l2>2<li id=l3>3</ul><a id=a1 href=x>l</a><a id=a2>n</a><p id=p1 data-x>d</p><p id=p2 data-x=hola>d</p>
        <p id=p3 lang=es-MX>e</p><a id=a3 href=\"http://x\">h</a><h1 id=h>h</h1><h2 id=h2 class=t>h</h2><span id=u class=u>u</span>
        <div><b class=p>p</b><b class=z>z</b><b class=q id=q>q</b></div><i class=x id=x1>x</i><i class=\"x y\" id=x2>x</i><div id=dv><p id=pp>p</p></div><i id=w>w</i>";
    let e = |id: &str| estilo_de(h, 400, id);
    assert_eq!(e("l1").color, 0x010101);
    assert_eq!(e("l3").color, 0x020202);
    assert_eq!(e("l2").margen[3], Len::Px(5.0));
    assert_eq!(e("l1").margen[3], Len::Px(0.0));
    assert_eq!(e("a1").color, 0x030303, ":link y :hover ignorado");
    assert_ne!(e("a2").color, 0x030303);
    assert_eq!(e("p1").color, 0x050505);
    assert_eq!(e("p2").color, 0x060606);
    assert_eq!(e("p3").fondo, Some(0x070707));
    assert_eq!(e("a3").borde[3], 1.0);
    assert_eq!(e("h").color, 0x0A0A0A);
    assert_eq!(e("h2").color, 0x0A0A0A);
    assert_eq!(e("u").color, 0x0A0A0A);
    assert_eq!(e("q").color, 0x0B0B0B);
    assert_eq!(e("x1").color, 0x0C0C0C);
    assert_ne!(e("x2").color, 0x0C0C0C);
    assert_eq!(e("pp").color, 0x0D0D0D);
    assert_eq!(e("h").alinear, estilo::Alin::Der);
    // @media: a 400 px ni max-width:300 ni min-width:500 ni print
    assert_ne!(e("w").color, 0x0E0E0E);
    assert_ne!(e("w").color, 0x0F0F0F);
    assert_ne!(e("w").color, 0x101010);
    let h300 = h.replace("max-width: 300px", "max-width: 450px");
    assert_eq!(estilo_de(&h300, 400, "w").color, 0x0E0E0E);
    assert_eq!(estilo_de(h, 600, "w").color, 0x101010);
}

#[test]
fn herencia() {
    let h = "<style>body{margin:0;color:#123456;font-family:Georgia,serif;font-size:20px;line-height:1.5;text-align:center;white-space:nowrap}
        div{margin:10px;padding:3px;border:2px solid red;background:#ffeedd;width:50px}</style>
        <body><div id=d><span id=s>x</span></div>";
    let s = estilo_de(h, 400, "s");
    assert_eq!(s.color, 0x123456);
    assert_eq!(s.px, 20.0);
    assert_eq!(s.familia, Familia::Serif);
    assert_eq!(s.alinear, estilo::Alin::Centro);
    assert_eq!(s.blanco, estilo::Blanco::NoWrap);
    assert_eq!(s.lh, estilo::Altura::Mult(1.5));
    // lo que no se hereda
    assert_eq!(s.margen, [Len::Px(0.0); 4]);
    assert_eq!(s.fondo, None);
    assert_eq!(s.borde, [0.0; 4]);
    assert_eq!(s.ancho, Len::Auto);
    let d = estilo_de(h, 400, "d");
    assert_eq!(d.fondo, Some(0xFFEEDD));
    assert_eq!(d.borde_color[0], 0xFF0000);
    assert_eq!(d.ancho, Len::Px(50.0));
}

#[test]
fn colores_y_unidades() {
    let c = |s: &str| color(s);
    assert_eq!(c("#fff"), Some(Col::Rgb(0xFFFFFF)));
    assert_eq!(c("#1a2B3c"), Some(Col::Rgb(0x1A2B3C)));
    assert_eq!(c("#0000"), Some(Col::Transp));
    assert_eq!(c("#00000000"), Some(Col::Transp));
    assert_eq!(c("#000f"), Some(Col::Rgb(0)));
    assert_eq!(c("rgb(255, 0, 128)"), Some(Col::Rgb(0xFF0080)));
    assert_eq!(c("rgb(100% 0% 0%)"), Some(Col::Rgb(0xFF0000)));
    assert_eq!(c("rgba(0,0,0,0)"), Some(Col::Transp));
    assert_eq!(c("rgba(0,0,0,0.5)"), Some(Col::Rgb(0x7F7F7F)), "alfa mezclado con blanco");
    assert_eq!(c("rgb(300, -5, 0)"), Some(Col::Rgb(0xFF0000)));
    assert_eq!(c("hsl(120, 100%, 50%)"), Some(Col::Rgb(0x00FF00)));
    assert_eq!(c("ReD"), Some(Col::Rgb(0xFF0000)));
    assert_eq!(c("cornflowerblue"), Some(Col::Rgb(0x6495ED)));
    assert_eq!(c("transparent"), Some(Col::Transp));
    assert_eq!(c("currentColor"), Some(Col::Actual));
    assert_eq!(c("#ggg"), None);
    assert_eq!(c("nada"), None);
    assert_eq!(c("#12345"), None);
    let x = Ctx { em: 10.0, rem: 20.0, vw: 1000.0, vh: 500.0 };
    let l = |s: &str| largo(s, &x);
    assert_eq!(l("12px"), Some(Len::Px(12.0)));
    assert_eq!(l("2em"), Some(Len::Px(20.0)));
    assert_eq!(l("1.5rem"), Some(Len::Px(30.0)));
    assert_eq!(l("50%"), Some(Len::Pct(50.0)));
    assert_eq!(l("12pt"), Some(Len::Px(16.0)));
    assert_eq!(l("10vw"), Some(Len::Px(100.0)));
    assert_eq!(l("10vh"), Some(Len::Px(50.0)));
    assert_eq!(l("0"), Some(Len::Px(0.0)));
    assert_eq!(l("auto"), Some(Len::Auto));
    assert_eq!(l("-3px"), Some(Len::Px(-3.0)));
    assert_eq!(l("12"), None);
    assert_eq!(l("1e3px"), None);
    assert_eq!(l("infpx"), None);
    // tamaños de letra relativos
    let e = estilo_de("<style>body{font-size:20px}.a{font-size:1.5em}.b{font-size:50%}.c{font-size:2rem}.d{font-size:larger}.e{font:italic bold 12px/2 Arial,sans-serif}</style>\
        <i id=a class=a>x</i><i id=b class=b>x</i><i id=c class=c>x</i><i id=d class=d>x</i><i id=e class=e>x</i>", 400, "a");
    assert_eq!(e.px, 30.0);
    assert_eq!(estilo_de("<style>body{font-size:20px}.b{font-size:50%}</style><i id=b class=b>x</i>", 400, "b").px, 10.0);
    assert_eq!(estilo_de("<style>.c{font-size:2rem}</style><i id=c class=c>x</i>", 400, "c").px, 32.0);
    let f = estilo_de("<style>.e{font:italic bold 12px/2 Arial,sans-serif}</style><i id=e class=e>x</i>", 400, "e");
    assert!(f.cursiva && f.negrita && f.px == 12.0 && f.familia == Familia::Sans);
    assert_eq!(f.lh, estilo::Altura::Mult(2.0));
    let _ = e;
    // familias
    let fam = |v: &str| estilo_de(&format!("<style>.f{{font-family:{v}}}</style><i id=f class=f>x</i>"), 400, "f").familia;
    assert_eq!(fam("Arial, Helvetica, sans-serif"), Familia::Sans);
    assert_eq!(fam("'Times New Roman', serif"), Familia::Serif);
    assert_eq!(fam("\"Courier New\", monospace"), Familia::Mono);
    assert_eq!(fam("Consolas"), Familia::Mono);
    assert_eq!(fam("system-ui"), Familia::Sans);
    assert_eq!(fam("Georgia"), Familia::Serif);
    assert_eq!(fam("Verdana"), Familia::Sans);
}

#[test]
fn css_con_errores_se_recupera() {
    let h = "<style>p { color: red; ;;; bogus: ; color }  }} p { margin: 0 } @media { div { color: blue } \
        @import url(x.css); .a { color: #00ff00 } p { /* comentario } */ background: #010203 } \
        b { color: rgb(1,2 } i { color: #0a0a0a } </style>\
        <p id=p>x</p><i id=i>x</i>";
    let p = estilo_de(h, 400, "p");
    assert_eq!(p.color, 0xFF0000);
    assert_eq!(p.margen[0], Len::Px(0.0));
    let i = estilo_de(h, 400, "i");
    assert_eq!(i.color, 0x0A0A0A);
    // hoja truncada o con basura
    for c in ["p{color:red", "p{", "@media", "@media screen{p{color:red}", "/*", "p{background:url(\"a)b\")}", "}}}{{{;;;", "\u{0}\u{1}\u{2}", "a{b:c}d{e:f}g"] {
        let _ = css::parsear_hoja(c, Vec::new());
    }
}

// ---------------------------------------------------------------------------------------------------------
// Maquetado

#[test]
fn salto_de_linea_y_alineacion() {
    let mut d = doc(&format!("{SIN_MARGEN}<div>aaaa bbbb cccc</div>"));
    let alto = maq(&mut d, 80);
    let t = textos(&d);
    assert_eq!(t.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["aaaa bbbb", "cccc"]);
    assert_eq!(t[0].1, 0);
    assert_eq!(t[1].2 - t[0].2, 18, "alto de línea: 1,15 × 16");
    assert_eq!(alto, 36);
    // centrado y a la derecha
    let mut d = doc(&format!("{SIN_MARGEN}<div style=\"text-align:center\">aaaa</div><div style=\"text-align:right\">aaaa</div>"));
    maq(&mut d, 100);
    let t = textos(&d);
    assert_eq!((t[0].1, t[1].1), (34, 68));
    // palabra más larga que la línea: se corta
    let mut d = doc(&format!("{SIN_MARGEN}<div>aaaaaaaaaaaaaaaaaaaa</div>"));
    maq(&mut d, 80);
    let t = textos(&d);
    assert_eq!(t.len(), 2);
    assert_eq!(t[0].0, "aaaaaaaaaa");
    // <br> y espacios colapsados
    let mut d = doc(&format!("{SIN_MARGEN}<div>a   b<br>c <br><br>d</div>"));
    let alto = maq(&mut d, 400);
    let t = textos(&d);
    assert_eq!(t[0].0, "a b");
    assert_eq!(t.len(), 3, "a b / c / d");
    assert_eq!(alto, 18 * 4, "hay una línea vacía por el segundo <br>");
    // pre conserva espacios y saltos; nowrap no parte
    let mut d = doc(&format!("{SIN_MARGEN}<pre>\nx  y\n  z</pre><div style=\"white-space:nowrap\">aaaa bbbb cccc dddd</div>"));
    maq(&mut d, 100);
    let t = textos(&d);
    assert_eq!(t[0].0, "x  y");
    assert_eq!(t[1].0, "  z");
    assert_eq!(t[2].0, "aaaa bbbb cccc dddd");
    // estilos en línea en la misma línea
    let mut d = doc(&format!("{SIN_MARGEN}<p>uno <b>dos</b> <i>tres</i></p>"));
    maq(&mut d, 400);
    let t = textos(&d);
    assert_eq!(t.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["uno ", "dos", " ", "tres"]);
    assert!(t.windows(2).all(|w| w[0].2 == w[1].2));
}

#[test]
fn margenes_y_cajas() {
    // margen auto con width centra
    let mut d = doc(&format!("{SIN_MARGEN}<div style=\"width:100px;margin:0 auto;background:#ff0000\">x</div>"));
    maq(&mut d, 300);
    let c = cajas(&d);
    assert_eq!((c[0].x, c[0].w), (100, 100));
    // padding y borde
    let mut d = doc(&format!("{SIN_MARGEN}<div style=\"padding:5px;border:2px solid #000;margin-left:10px\">x</div>"));
    let alto = maq(&mut d, 300);
    let c = cajas(&d);
    assert_eq!((c[0].x, c[0].w), (10, 290));
    assert_eq!(alto, 18 + 14);
    assert_eq!(textos(&d)[0].1, 10 + 2 + 5);
    // box-sizing, max-width, porcentajes
    let mut d = doc(&format!("{SIN_MARGEN}<div style=\"width:50%;padding:10px;box-sizing:border-box;background:#eee\">x</div><div style=\"max-width:120px;background:#eee\">y</div>"));
    maq(&mut d, 400);
    let c = cajas(&d);
    assert_eq!(c[0].w, 200);
    assert_eq!(c[1].w, 120);
    // márgenes verticales colapsados: 20 y 30 → 30 entre las cajas
    let mut d = doc(&format!("{SIN_MARGEN}<div style=\"margin-bottom:20px;background:#111\">a</div><div style=\"margin-top:30px;background:#222\">b</div>"));
    let alto = maq(&mut d, 300);
    let c = cajas(&d);
    assert_eq!(c[1].y - (c[0].y + c[0].h), 30);
    assert_eq!(alto, 18 + 30 + 18);
    // el margen del primer hijo sale del padre (sin relleno): el fondo del padre empieza después
    let mut d = doc(&format!("{SIN_MARGEN}<div style=\"background:#111\"><p style=\"margin:10px 0 0\">a</p></div>"));
    maq(&mut d, 300);
    assert_eq!(cajas(&d)[0].y, 10);
    // altura fija
    let mut d = doc(&format!("{SIN_MARGEN}<div style=\"height:50px;background:#111\">a</div><i>x</i>"));
    maq(&mut d, 300);
    assert_eq!(cajas(&d)[0].h, 50);
    assert_eq!(textos(&d)[1].2 - textos(&d)[0].2, 50 + 18 - 14 + 14 - 18 + 0 + 4 * 0 + 18 - 18, "la línea siguiente va a 50 px");
}

#[test]
fn listas() {
    let mut d = doc(&format!("{SIN_MARGEN}<ul><li>uno<li>dos</ul><ol start=3><li>tres<li>cuatro</ol>"));
    maq(&mut d, 300);
    let t = textos(&d);
    assert_eq!(t[0].1, 40, "sangría de 40 px");
    let vinetas = items(&d).iter().filter(|i| matches!(i, Item::Vineta { .. })).count();
    assert_eq!(vinetas, 2);
    let nums: Vec<&str> = t.iter().map(|x| x.0.as_str()).filter(|s| s.ends_with(". ")).collect();
    assert_eq!(nums, ["3. ", "4. "]);
    // list-style-type:none no pinta marcador
    let mut d = doc(&format!("{SIN_MARGEN}<ul style=\"list-style-type:none\"><li>uno</ul>"));
    maq(&mut d, 300);
    assert!(!items(&d).iter().any(|i| matches!(i, Item::Vineta { .. })));
    // listas anidadas con marcadores distintos
    let mut d = doc(&format!("{SIN_MARGEN}<ul><li>a<ul><li>b<ul><li>c</ul></ul></ul>"));
    maq(&mut d, 300);
    let tipos: Vec<_> = items(&d).iter().filter_map(|i| if let Item::Vineta { tipo, .. } = i { Some(*tipo) } else { None }).collect();
    assert_eq!(tipos, [estilo::Lista::Disco, estilo::Lista::Circulo, estilo::Lista::Cuadrado]);
}

#[test]
fn tabla_2x2() {
    let mut d = doc(&format!(
        "{SIN_MARGEN}<style>table{{border-spacing:0}} td{{padding:0}}</style><table><tr><td>aa</td><td>bbbb</td></tr><tr><td>cccccc</td><td>d</td></tr></table>"
    ));
    let alto = maq(&mut d, 400);
    let t = textos(&d);
    // columnas: max(2,6)=6 letras = 48 px, max(4,1)=4 letras = 32 px
    assert_eq!(t[0], ("aa".to_string(), 0, t[0].2));
    assert_eq!(t[1].1, 48);
    assert_eq!(t[2].1, 0);
    assert_eq!(t[3].1, 48);
    assert_eq!(t[2].2 - t[0].2, 18);
    assert_eq!(alto, 36);
    // colspan
    let mut d = doc(&format!("{SIN_MARGEN}<style>table{{border-spacing:0}} td{{padding:0}}</style><table><tr><td colspan=2>xxxxxxxxxx</td></tr><tr><td>a</td><td>b</td></tr></table>"));
    maq(&mut d, 400);
    let t = textos(&d);
    assert_eq!(t[0].0, "xxxxxxxxxx");
    assert!(t[2].1 >= 40, "la segunda columna empieza tras el reparto del colspan: {}", t[2].1);
    // la tabla se estrecha al ancho disponible sin romper el pánico: celdas con mucho texto parten
    let mut d = doc(&format!("{SIN_MARGEN}<table width=100%><tr><td>aaaa bbbb cccc dddd eeee</td><td>ffff gggg</td></tr></table>"));
    maq(&mut d, 160);
    let t = textos(&d);
    assert!(t.len() > 2);
    assert!(t.iter().all(|x| x.1 < 160));
    // fondos de celda y bgcolor
    let mut d = doc(&format!("{SIN_MARGEN}<table><tr><td bgcolor=\"#ff0000\">a</td></tr></table>"));
    maq(&mut d, 400);
    assert!(items(&d).iter().any(|i| matches!(i, Item::Caja { fondo: Some(0xFF0000), .. })));
}

#[test]
fn imagenes() {
    let h = format!("{SIN_MARGEN}<p><img src=\"a.png\" alt=\"texto alterno\"></p>");
    let mut d = doc(&h);
    // desconocida: no ocupa nada
    let alto0 = maq(&mut d, 300);
    assert!(items(&d).iter().all(|i| !matches!(i, Item::Imagen { .. })));
    let _ = alto0;
    // dimensiones conocidas antes de decodificar
    d.tamano_imagen("http://ej.com/dir/a.png", 100, 50);
    maq(&mut d, 300);
    let r = items(&d).iter().find_map(|i| if let Item::Imagen { r, .. } = i { Some(*r) } else { None }).unwrap();
    assert_eq!((r.w, r.h), (100, 50));
    // más ancha que la columna: se reduce en proporción
    let mut d2 = doc(&h);
    d2.tamano_imagen("http://ej.com/dir/a.png", 400, 200);
    maq(&mut d2, 100);
    let r = items(&d2).iter().find_map(|i| if let Item::Imagen { r, .. } = i { Some(*r) } else { None }).unwrap();
    assert_eq!((r.w, r.h), (100, 50));
    // atributos width/height mandan
    let mut d3 = doc(&format!("{SIN_MARGEN}<img src=a.png width=30 height=20 alt=x>"));
    maq(&mut d3, 300);
    let r = items(&d3).iter().find_map(|i| if let Item::Imagen { r, .. } = i { Some(*r) } else { None }).unwrap();
    assert_eq!((r.w, r.h), (30, 20));
    // solo width: la altura sale de la proporción
    let mut d4 = doc(&format!("{SIN_MARGEN}<img src=a.png style=\"width:50px\">"));
    d4.tamano_imagen("http://ej.com/dir/a.png", 100, 40);
    maq(&mut d4, 300);
    let r = items(&d4).iter().find_map(|i| if let Item::Imagen { r, .. } = i { Some(*r) } else { None }).unwrap();
    assert_eq!((r.w, r.h), (50, 20));
    // imagen decodificada: se pinta y usa su tamaño natural
    let mut d5 = doc(&format!("{SIN_MARGEN}<img src=a.png>"));
    d5.poner_imagen("http://ej.com/dir/a.png", Some(Picture { w: 4, h: 2, argb: alloc::vec![0xFFFF0000; 8] }));
    maq(&mut d5, 300);
    let mut s = Surface::new(20, 20, 0xFFFFFF);
    d5.pintar(&mut s, 0, &mut Falsa);
    assert!(s.px.iter().any(|&p| p == 0xFF0000));
    // falló: se dibuja el alt
    d.poner_imagen("http://ej.com/dir/a.png", None);
    maq(&mut d, 300);
    assert!(textos(&d).iter().any(|t| t.0 == "texto alterno"));
    assert!(items(&d).iter().all(|i| !matches!(i, Item::Imagen { .. })));
}

#[test]
fn flotantes() {
    let mut d = doc(&format!("{SIN_MARGEN}<div style=\"float:left;width:40px;height:30px;background:#f00\"></div><p style=\"margin:0\">aaaa bbbb cccc dddd</p>"));
    maq(&mut d, 100);
    let t = textos(&d);
    assert_eq!(t[0].1, 40, "el texto rodea al flotante");
    let mut d = doc(&format!("{SIN_MARGEN}<div style=\"float:right;width:40px;height:30px;background:#f00\"></div><p style=\"margin:0\">aaaa</p>"));
    maq(&mut d, 100);
    assert_eq!(cajas(&d)[0].x, 60);
    // un contenedor con overflow:hidden contiene a su flotante
    let mut d = doc(&format!("{SIN_MARGEN}<div style=\"overflow:hidden;background:#111\"><div style=\"float:left;width:40px;height:60px\">x</div></div><b>z</b>"));
    maq(&mut d, 100);
    assert_eq!(cajas(&d)[0].h, 60);
}

#[test]
fn enlaces_texto_y_anclas() {
    let h = format!(
        "{SIN_MARGEN}<h1 id=arriba>Titulo</h1><p>hola <a href=\"/x/y.html#z\">enlace uno</a> y <a href=\"#fin\">interno</a> <a href=\"javascript:f()\">js</a></p>\
         <p>segunda linea de texto</p><a name=\"fin\"></a><div id=\"abajo\" style=\"margin-top:100px\">fin</div><a href=\"http://o.org/\"><img src=i.png width=20 height=10></a>"
    );
    let mut d = doc(&h);
    maq(&mut d, 600);
    let t = textos(&d);
    let uno = t.iter().find(|x| x.0 == "enlace uno").unwrap();
    assert_eq!(d.enlace_en(uno.1 + 3, uno.2 - 3).as_deref(), Some("http://ej.com/x/y.html#z"));
    let interno = t.iter().find(|x| x.0 == "interno").unwrap();
    assert_eq!(d.enlace_en(interno.1 + 1, interno.2 - 2).as_deref(), Some("http://ej.com/dir/pag.html#fin"));
    let js = t.iter().find(|x| x.0 == "js").unwrap();
    assert_eq!(d.enlace_en(js.1 + 1, js.2 - 2), None);
    assert_eq!(d.enlace_en(uno.1 + 3, uno.2 + 500), None);
    let hola = t.iter().find(|x| x.0.starts_with("hola")).unwrap();
    assert_eq!(d.enlace_en(hola.1 + 3, hola.2 - 3), None);
    // el enlace de una imagen
    d.tamano_imagen("http://ej.com/dir/i.png", 20, 10);
    maq(&mut d, 600);
    let r = items(&d).iter().find_map(|i| if let Item::Imagen { r, .. } = i { Some(*r) } else { None }).unwrap();
    assert_eq!(d.enlace_en(r.x + 2, r.y + 2).as_deref(), Some("http://o.org/"));
    // anclas
    assert_eq!(d.ancla("arriba"), Some(21), "el margen superior del h1 (0,67 em) queda fuera de su caja");
    let abajo = d.ancla("abajo").unwrap();
    assert!(abajo > 60, "{abajo}");
    assert_eq!(d.ancla("fin").map(|y| y > 0), Some(true));
    assert_eq!(d.ancla("noexiste"), None);
    // texto seleccionado
    let t0 = t.iter().find(|x| x.0 == "Titulo").unwrap();
    let todo = d.texto_en(0, 0, 600, 10_000);
    assert!(todo.starts_with("Titulo\nhola enlace uno y interno js\nsegunda linea de texto"), "{todo:?}");
    // parte de una línea: del medio de «enlace uno» al final de «interno»
    let sel = d.texto_en(uno.1 + 7 * 8, uno.2 - 3, interno.1 + 56, interno.2 - 3);
    assert_eq!(sel, "uno y interno");
    // al revés da lo mismo
    assert_eq!(d.texto_en(interno.1 + 56, interno.2 - 3, uno.1 + 7 * 8, uno.2 - 3), sel);
    // fuera de todo texto
    assert_eq!(d.texto_en(500, 5000, 520, 5010), "");
    let _ = t0;
}

#[test]
fn poner_css_cambia_el_maquetado() {
    let mut d = doc(&format!("{SIN_MARGEN}<link rel=stylesheet href=\"b.css\"><link rel=stylesheet href=\"a.css\"><p id=p>x</p>"));
    let alto = maq(&mut d, 300);
    assert_eq!(alto, 18 + 32, "sin hoja: margen de 16 px del párrafo");
    // llegan desordenadas, pero valen en el orden del documento (a.css, la segunda, gana)
    d.poner_css("http://ej.com/dir/a.css", b"p { margin: 0; color: #00ff00 }");
    d.poner_css("http://ej.com/dir/b.css", b"p { margin: 0; color: #ff0000; padding-top: 10px }");
    let alto = maq(&mut d, 300);
    assert_eq!(alto, 28);
    assert_eq!(d.estilos.as_ref().unwrap().0.de(d.dom.nodos.iter().position(|n| matches!(&n.dato, Dato::Elem(e) if e.etq == "p")).unwrap()).color, 0x00FF00);
    // el fondo del lienzo viene de body
    let mut d = doc("<body style=\"background:#102030\">x");
    maq(&mut d, 300);
    assert_eq!(d.fondo(), 0x102030);
    let d = doc("x");
    assert_eq!(d.fondo(), 0xFFFFFF);
}

#[test]
fn pintar_con_scroll_y_clip() {
    let mut d = doc(&format!("{SIN_MARGEN}<div style=\"background:#ff0000;height:100px\"></div><div style=\"background:#00ff00;height:100px\"></div>"));
    maq(&mut d, 50);
    let mut s = Surface::new(50, 50, 0);
    d.pintar(&mut s, 75, &mut Falsa);
    assert_eq!(s.px[0], 0xFF0000);
    assert_eq!(s.px[30 * 50], 0x00FF00);
    // el recorte de la superficie se respeta
    let mut s = Surface::new(50, 50, 0);
    s.set_clip(Rect::new(10, 10, 10, 10));
    d.pintar(&mut s, 0, &mut Falsa);
    assert_eq!(s.px[0], 0);
    assert_eq!(s.px[15 * 50 + 15], 0xFF0000);
    // texto con subrayado
    let mut d = doc(&format!("{SIN_MARGEN}<u>aaaa</u>"));
    maq(&mut d, 100);
    let mut s = Surface::new(100, 30, 0xFFFFFF);
    d.pintar(&mut s, 0, &mut Falsa);
    assert!(s.px.iter().any(|&p| p == 0));
}

// ---------------------------------------------------------------------------------------------------------
// Robustez: entrada hostil

struct Azar(u64);

impl Azar {
    fn sig(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

const PIEZAS: &[&str] = &[
    "<div>", "</div>", "<p>", "</p>", "<table>", "<tr>", "<td>", "</td>", "</table>", "<ul>", "<li>", "<a href=\"x\">", "</a>", "<b>", "</i>", "<img src=a.png width=-5 height=99999999>",
    "<span style=\"float:left;width:50%;margin:-9999px auto;padding:1e9px\">", "texto ", "palabralarguísimaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ", "&amp;", "&#xFFFFFFFF;",
    "<", ">", "<!--", "-->", "\"", "'", "<br>", "<pre>", " \n ", "<style>p{color:red}</style>", "<td colspan=999999 rowspan=3>", "<h1>", "<center>", "<font size=+99 color=zz>",
    "<div style=\"display:inline-block;width:10px\">", "<div style=\"position:absolute;float:right;clear:both\">", "<li style=\"list-style:square\">", "<input>", "<textarea>", "</textarea>",
];

fn aleatorio(sem: u64, n: usize) -> String {
    let mut a = Azar(sem);
    (0..n).map(|_| PIEZAS[a.sig() as usize % PIEZAS.len()]).collect()
}

fn recorre(h: &str, ancho: i32) {
    let mut d = Documento::cargar(h.as_bytes(), None, "http://ej.com/");
    d.tamano_imagen("http://ej.com/a.png", 300, 200);
    let alto = maq(&mut d, ancho);
    assert!((0..=1_000_000).contains(&alto));
    let mut s = Surface::new(120, 90, 0xFFFFFF);
    d.pintar(&mut s, alto / 2, &mut Falsa);
    d.pintar(&mut s, -50, &mut Falsa);
    let _ = d.enlace_en(5, 5);
    let _ = d.texto_en(0, 0, 400, 400);
    let _ = d.ancla("x");
}

#[test]
fn html_aleatorio_no_entra_en_panico() {
    let t = std::time::Instant::now();
    for sem in 0..300u64 {
        let h = aleatorio(sem, 40 + (sem as usize % 7) * 60);
        recorre(&h, [1, 5, 40, 320, 1000, 100_000][sem as usize % 6]);
    }
    assert!(t.elapsed().as_secs() < 30, "demasiado lento: {:?}", t.elapsed());
}

#[test]
fn bytes_aleatorios_y_truncados() {
    let mut a = Azar(7);
    for _ in 0..100 {
        let n = (a.sig() % 600) as usize;
        let b: Vec<u8> = (0..n).map(|_| a.sig() as u8).collect();
        let mut d = Documento::cargar(&b, Some("text/html; charset=windows-1252"), "http://ej.com/");
        maq(&mut d, 300);
        let mut d = Documento::cargar(&b, None, "mal formada");
        maq(&mut d, 300);
        d.poner_css("x", &b);
        let _ = css::parsear_hoja(&String::from_utf8_lossy(&b), Vec::new());
    }
    // truncado en cada posición de un documento real
    let h = "<html><head><style>p.a > b:first-child { color: #123; margin: 1em auto } @media (max-width:300px){p{x:y}}</style></head>\
        <body><table><tr><td><ul><li>é<a href=\"q?x=1&amp;y=2\">enlace</a></ul></td></tr></table><img src=a.png alt=\"ñ\"><!-- c --></body></html>";
    for i in 0..h.len() {
        if h.is_char_boundary(i) {
            recorre(&h[..i], 200);
        }
    }
    for i in 0..h.len() {
        let mut b = h.as_bytes().to_vec();
        b.truncate(i);
        let mut d = Documento::cargar(&b, None, "http://ej.com/");
        maq(&mut d, 200);
    }
}

#[test]
fn profundidad_y_tamanos_extremos() {
    let t = std::time::Instant::now();
    // 100 000 divs anidados, y spans, y listas, y tablas
    for (ab, ce) in [("<div>", "</div>"), ("<span>", "</span>"), ("<ul><li>", "</li></ul>"), ("<table><tr><td>", "</td></tr></table>"), ("<b><i>", "</i></b>"), ("<a href=x>", "</a>")] {
        let h = format!("{}hola{}", ab.repeat(100_000), ce.repeat(100_000));
        recorre(&h, 300);
    }
    // muy profundo y hermanos: 200 000 nodos
    recorre(&"<p>x".repeat(300_000), 300);
    recorre(&"<li>x<br>".repeat(100_000), 300);
    // palabra de un millón de letras y texto de muchos MB
    recorre(&"a".repeat(1_000_000), 77);
    recorre(&"palabra ".repeat(500_000), 500);
    recorre(&"x ".repeat(5_000_000), 200);
    // CSS enorme y selectores patológicos
    let css = "a a a a a a a a b{color:red}".repeat(5000);
    recorre(&format!("<style>{css}</style>{}x{}", "<a>".repeat(250), "</a>".repeat(250)), 300);
    let css: String = (0..30_000).map(|i| format!(".c{i}{{margin:{i}px;color:#{:06x}}}", i * 37)).collect();
    recorre(&format!("<style>{css}</style><div class=c5>x</div>"), 300);
    recorre(&format!("<style>{}</style>x", "@media screen{".repeat(10_000)), 300);
    recorre(&format!("<style>p{{{}}}</style><p>x", "a:b;".repeat(500_000)), 300);
    // medidas absurdas
    recorre("<div style=\"width:99999999px;height:99999999px;margin:99999999px;padding:99999999px;border:99999999px solid red\">x</div>", 300);
    recorre("<table width=99999999><tr><td width=99999999 colspan=99999999>x</td><td>y</td></tr></table>", 300);
    recorre(&"<div style=\"height:900000px\">x</div>".repeat(50), 300);
    recorre(&"<img src=a.png width=99999 height=99999>".repeat(1000), 300);
    recorre(&format!("<table>{}</table>", "<tr><td>x</td></tr>".repeat(50_000)), 300);
    recorre(&format!("<tr>{}", "<td>x".repeat(50_000)), 300);
    recorre(&"<a name=x id=y>".repeat(100_000), 300);
    recorre(&format!("<p {}>x", "a=b ".repeat(100_000)), 300);
    assert!(t.elapsed().as_secs() < 90, "demasiado lento: {:?}", t.elapsed());
}

#[test]
fn display_none_y_visibility() {
    let mut d = doc(&format!("{SIN_MARGEN}<div style=\"display:none\">oculto</div><div style=\"visibility:hidden\">invisible</div><div hidden>otro</div><p>x</p>"));
    let alto = maq(&mut d, 300);
    let t: Vec<String> = textos(&d).into_iter().map(|x| x.0).collect();
    assert_eq!(t, ["x"]);
    assert_eq!(alto, 18 + 16 + 18 + 16, "la línea invisible ocupa su sitio; el p lleva márgenes de 16");
    let _ = Display::None;
}
