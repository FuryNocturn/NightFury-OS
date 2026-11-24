// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Páginas internas del navegador (`nightfury://historial` y `nightfury://favoritos`), hechas como HTML para
//! nf-web. Se abren desde la barra, los atajos y el menú. Una página web no puede abrirlas ni enlazarlas: el
//! navegador ignora ese destino salvo que venga de otra página interna (ver `Navegador::interna_desde`).
//! Sus enlaces y formularios usan `BASE` (nf-web solo entiende http/https) y se traducen a `nightfury://`.
//! Las acciones (borrar, mover…) viajan en la consulta, pero solo `aplicar` las ejecuta, al navegar a propósito:
//! la página que queda en el historial no lleva la acción y volver atrás no la repite.
use crate::datos::{host, Datos, Fav};
use crate::descargas::Descargas;
use crate::entrada::codificar_consulta;
use crate::paginas::{self, escapar};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use nf_core::hora;

pub const ESQUEMA: &str = "nightfury://";
/// Servidor ficticio de los enlaces de las páginas internas (`.invalid` nunca resuelve).
pub const BASE: &str = "http://interno.invalid/";
/// Entradas del historial que se enseñan de una vez.
const MAX_MOSTRADAS: usize = 400;

const ESTILO: &str = "body { font-family: sans-serif; background: #ffffff; color: #202124 }\
.c { max-width: 780px; margin: 28px auto 0 auto; padding: 0 24px 40px 24px }\
h1 { font-size: 24px; font-weight: normal; margin: 0 0 14px 0 }\
h2 { font-size: 14px; color: #5f6368; margin: 22px 0 6px 0 }\
p { font-size: 14px; margin: 6px 0 }\
.h { color: #80868b; margin-right: 14px }\
.u { color: #80868b; font-size: 12px; margin-left: 10px }\
.x { color: #c5221f; font-size: 12px; margin-left: 12px }\
.a { font-size: 12px; margin-left: 12px }\
.s { padding-left: 28px }";

fn empieza(u: &str, p: &str) -> bool {
    u.get(..p.len()).is_some_and(|x| x.eq_ignore_ascii_case(p))
}

pub fn es_interna(u: &str) -> bool {
    empieza(u, ESQUEMA)
}

/// `nightfury://x` o un enlace de página interna (`http://interno.invalid/x`) como `nightfury://x`.
pub fn de_enlace(u: &str) -> Option<String> {
    let r = if empieza(u, ESQUEMA) {
        &u[ESQUEMA.len()..]
    } else if empieza(u, BASE) {
        &u[BASE.len()..]
    } else {
        return None;
    };
    Some(format!("{ESQUEMA}{}", r.split('#').next().unwrap_or("")))
}

fn decodificar(s: &str) -> String {
    let b = s.as_bytes();
    let mut r: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => r.push(b' '),
            b'%' if s.get(i + 1..i + 3).is_some_and(|h| h.bytes().all(|c| c.is_ascii_hexdigit())) => {
                r.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'?'));
                i += 2;
            }
            c => r.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&r).into_owned()
}

/// Página y parámetros de una dirección interna.
fn partir(url: &str) -> (String, Vec<(String, String)>) {
    let r = de_enlace(url).unwrap_or_default();
    let r = r.strip_prefix(ESQUEMA).unwrap_or("");
    let (ruta, q) = r.split_once('?').unwrap_or((r, ""));
    let p = q.split('&').filter(|x| !x.is_empty()).map(|x| x.split_once('=').map_or((decodificar(x), String::new()), |(k, v)| (decodificar(k), decodificar(v)))).collect();
    (ruta.trim_end_matches('/').to_ascii_lowercase(), p)
}

fn param<'a>(p: &'a [(String, String)], k: &str) -> Option<&'a str> {
    p.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

fn num<T: core::str::FromStr>(p: &[(String, String)], k: &str) -> Option<T> {
    param(p, k).and_then(|v| v.parse().ok())
}

/// Enlace a una página interna con parámetros, listo para ir en un atributo HTML.
fn enlace(ruta: &str, par: &[(&str, &str)]) -> String {
    let mut s = format!("{BASE}{ruta}");
    for (i, (k, v)) in par.iter().enumerate() {
        s.push(if i == 0 { '?' } else { '&' });
        s.push_str(k);
        s.push('=');
        s.push_str(&codificar_consulta(v));
    }
    escapar(&s)
}

fn marco(titulo: &str, cuerpo: &str) -> String {
    format!("<html><head><title>{}</title><style>{ESTILO}</style></head><body><div class=c>{cuerpo}</div></body></html>", escapar(titulo))
}

fn corto(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return String::from(s);
    }
    let mut r: String = s.chars().take(n.saturating_sub(1)).collect();
    r.push('…');
    r
}

/// Pregunta antes de una acción que no se puede deshacer: un formulario con la acción y `ok=1`.
fn confirmar(titulo: &str, pregunta: &str, ruta: &str, campos: &[(&str, &str)], extra: &str) -> String {
    let mut ocultos = String::new();
    for (k, v) in campos.iter().chain(&[("ok", "1")]) {
        ocultos.push_str(&format!("<input type=hidden name=\"{k}\" value=\"{}\">", escapar(v)));
    }
    marco(titulo, &format!("<h1>{}</h1><p>{}</p><form action=\"{BASE}{ruta}\">{ocultos}{extra}<p><input type=submit value=\"Borrar\"> <a href=\"{BASE}{ruta}\">Cancelar</a></p></form>", escapar(titulo), escapar(pregunta)))
}

// ------------------------------------------------------------ historial

const SEMANA: [&str; 7] = ["domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado"];
const MESES: [&str; 12] = ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"];

fn dia_texto(dia: i64, hoy: i64) -> String {
    let f = hora::de_segundos(dia * 86_400);
    let fecha = format!("{}, {} de {} de {}", SEMANA[(dia + 4).rem_euclid(7) as usize], f.day, MESES[(f.month as usize).clamp(1, 12) - 1], f.year);
    match hoy - dia {
        0 => format!("Hoy - {fecha}"),
        1 => format!("Ayer - {fecha}"),
        _ => fecha,
    }
}

fn historial(d: &Datos, p: &[(String, String)], ahora: i64) -> String {
    let q = param(p, "q").unwrap_or("");
    if let Some(c) = param(p, "limpiar") {
        let que = match c {
            "hora" => "los datos de la última hora",
            "dia" => "los datos del último día",
            _ => "todo el historial",
        };
        return confirmar("Borrar datos de navegación", &format!("¿Borrar {que}? No se puede deshacer."), "historial", &[("limpiar", c)], "<p><label><input type=checkbox name=cookies value=1> También las cookies</label></p>");
    }
    let mut s = String::from("<h1>Historial</h1>");
    s.push_str(&format!("<form action=\"{BASE}historial\"><input type=search name=q value=\"{}\" placeholder=\"Buscar en el historial\"> <input type=submit value=\"Buscar\"></form>", escapar(q)));
    s.push_str(&format!(
        "<p>Borrar datos de navegación: <a href=\"{}\">última hora</a> · <a href=\"{}\">último día</a> · <a href=\"{}\">todo</a></p>",
        enlace("historial", &[("limpiar", "hora")]),
        enlace("historial", &[("limpiar", "dia")]),
        enlace("historial", &[("limpiar", "todo")])
    ));
    if d.incog {
        s.push_str("<p class=u>En incógnito no se guarda historial.</p>");
    }
    let todas = d.hist.buscar(q);
    if todas.is_empty() {
        s.push_str(if q.trim().is_empty() { "<p>No hay páginas en el historial.</p>" } else { "<p>No hay nada que coincida con lo que buscas.</p>" });
    }
    let (hoy, mut dia_ant) = (ahora.div_euclid(86_400), i64::MIN);
    for v in todas.iter().take(MAX_MOSTRADAS) {
        let dia = v.ts.div_euclid(86_400);
        if dia != dia_ant {
            s.push_str(&format!("<h2>{}</h2>", escapar(&dia_texto(dia, hoy))));
            dia_ant = dia;
        }
        let t = hora::de_segundos(v.ts);
        let titulo = if v.titulo.is_empty() { &v.url } else { &v.titulo };
        let borrar = if q.is_empty() { enlace("historial", &[("borrar", &format!("{}", v.ts))]) } else { enlace("historial", &[("borrar", &format!("{}", v.ts)), ("q", q)]) };
        s.push_str(&format!(
            "<p><span class=h>{:02}:{:02}</span><a href=\"{}\">{}</a><span class=u>{}</span><a class=x href=\"{borrar}\">Eliminar</a></p>",
            t.hour,
            t.minute,
            escapar(&v.url),
            escapar(&corto(titulo, 90)),
            escapar(&host(&v.url).unwrap_or_default())
        ));
    }
    if todas.len() > MAX_MOSTRADAS {
        s.push_str(&format!("<p class=u>Se muestran las {MAX_MOSTRADAS} más recientes. Usa el buscador para ver el resto.</p>"));
    }
    marco("Historial", &s)
}

// ------------------------------------------------------------ favoritos

fn fila(f: &Fav) -> String {
    let id = format!("{}", f.id);
    let titulo = if f.titulo.is_empty() { &f.url } else { &f.titulo };
    format!(
        "<p><a href=\"{}\">{}</a><span class=u>{}</span><a class=a href=\"{}\">Subir</a><a class=a href=\"{}\">Bajar</a><a class=a href=\"{}\">Mover</a><a class=x href=\"{}\">Eliminar</a></p>",
        escapar(&f.url),
        escapar(&corto(titulo, 80)),
        escapar(&host(&f.url).unwrap_or_default()),
        enlace("favoritos", &[("subir", &id)]),
        enlace("favoritos", &[("bajar", &id)]),
        enlace("favoritos", &[("mover", &id)]),
        enlace("favoritos", &[("quitar", &id)])
    )
}

fn favoritos(d: &Datos, p: &[(String, String)]) -> String {
    let f = &d.favs;
    if let Some(id) = num::<u32>(p, "quitar") {
        // Solo llega aquí una carpeta con cosas dentro (si no, `aplicar` ya la ha borrado).
        let nombre = f.por_id(id).map_or("", |x| x.titulo.as_str());
        return confirmar("Eliminar carpeta", &format!("¿Eliminar la carpeta «{nombre}» y todos los favoritos que contiene?"), "favoritos", &[("quitar", &format!("{id}"))], "");
    }
    if let Some(x) = num::<u32>(p, "mover").and_then(|id| f.por_id(id)).filter(|x| !x.carpeta) {
        let id = format!("{}", x.id);
        let mut s = format!("<h1>Mover «{}»</h1><p><a href=\"{}\">Barra de favoritos</a></p>", escapar(&corto(&x.titulo, 60)), enlace("favoritos", &[("mv", &id), ("a", "0")]));
        for c in f.hijos(0).into_iter().filter(|c| c.carpeta) {
            s.push_str(&format!("<p><a href=\"{}\">{}</a></p>", enlace("favoritos", &[("mv", &id), ("a", &format!("{}", c.id))]), escapar(&corto(&c.titulo, 60))));
        }
        s.push_str(&format!("<p><a href=\"{BASE}favoritos\">Cancelar</a></p>"));
        return marco("Mover favorito", &s);
    }
    let mut s = String::from("<h1>Favoritos</h1>");
    s.push_str(&format!("<form action=\"{BASE}favoritos\"><input type=text name=nueva placeholder=\"Nombre de la carpeta nueva\"> <input type=submit value=\"Crear carpeta\"></form>"));
    if f.v.is_empty() {
        s.push_str("<p>Todavía no tienes favoritos. Pulsa la estrella de la barra de direcciones (Ctrl+D) para añadir uno.</p>");
    }
    for x in f.hijos(0) {
        if !x.carpeta {
            s.push_str(&fila(x));
            continue;
        }
        let id = format!("{}", x.id);
        s.push_str(&format!(
            "<h2>Carpeta</h2><form action=\"{BASE}favoritos\"><input type=hidden name=ren value=\"{id}\"><input type=text name=nombre value=\"{}\"> <input type=submit value=\"Renombrar\"><a class=a href=\"{}\">Subir</a><a class=a href=\"{}\">Bajar</a><a class=x href=\"{}\">Eliminar</a></form>",
            escapar(&x.titulo),
            enlace("favoritos", &[("subir", &id)]),
            enlace("favoritos", &[("bajar", &id)]),
            enlace("favoritos", &[("quitar", &id)])
        ));
        let hijos = f.hijos(x.id);
        s.push_str("<div class=s>");
        if hijos.is_empty() {
            s.push_str("<p class=u>Carpeta vacía.</p>");
        }
        for h in hijos {
            s.push_str(&fila(h));
        }
        s.push_str("</div>");
    }
    marco("Favoritos", &s)
}

// ------------------------------------------------------------ descargas

/// La página `nightfury://descargas`: las descargas de esta ventana (solo en memoria), la última arriba.
pub fn descargas(d: &Descargas) -> String {
    let mut s = String::from("<h1>Descargas</h1><p class=u>Esta lista es de esta ventana y no se guarda. Recarga la página para ver cómo avanzan.</p>");
    if d.items.is_empty() {
        s.push_str("<p>No has descargado nada en esta ventana.</p>");
    }
    for it in d.items.iter().rev() {
        let donde = if it.ruta.is_empty() { String::new() } else { format!("<br><span class=u>{}</span>", escapar(&it.ruta)) };
        s.push_str(&format!("<p>{} <span class=u>{} · {}</span>{donde}</p>", escapar(&it.nombre), escapar(&it.texto_estado()), escapar(&it.sitio())));
    }
    marco("Descargas", &s)
}

/// Nombre de la página de una dirección interna (`historial`, `descargas`…).
pub fn ruta(url: &str) -> String {
    partir(url).0
}

/// El HTML de la página interna `url` (con los datos de ahora; `ahora` en segundos del reloj).
pub fn pagina(d: &Datos, url: &str, ahora: i64) -> String {
    let (ruta, p) = partir(url);
    match ruta.as_str() {
        "historial" => historial(d, &p, ahora),
        "favoritos" => favoritos(d, &p),
        _ => paginas::error("No se encuentra la página", "Esa página interna del navegador no existe.", url),
    }
}

/// Ejecuta la acción que trae la dirección, si trae una y está confirmada. `Some(destino)` = hecha: se muestra
/// `destino` (la misma página sin la acción); `None` = no hay acción (o falta confirmarla): se muestra `url`.
pub fn aplicar(d: &mut Datos, url: &str, ahora: i64, ms: u64) -> Option<String> {
    let (ruta, p) = partir(url);
    let ok = param(&p, "ok") == Some("1");
    match ruta.as_str() {
        "historial" => {
            let q = param(&p, "q").unwrap_or("");
            let destino = if q.is_empty() { format!("{ESQUEMA}historial") } else { format!("{ESQUEMA}historial?q={}", codificar_consulta(q)) };
            if let Some(ts) = num::<i64>(&p, "borrar") {
                if d.hist.borrar(ts) {
                    d.cambio_historial(ms);
                }
                return Some(destino);
            }
            let desde = match param(&p, "limpiar") {
                Some("hora") => ahora - 3600,
                Some("dia") => ahora - 86_400,
                Some("todo") => 0,
                _ => return None,
            };
            if !ok {
                return None;
            }
            // El navegador recoge este aviso y vacía el tarro (también en disco).
            d.borrar_cookies = param(&p, "cookies") == Some("1");
            if d.hist.borrar_desde(desde) {
                d.cambio_historial(ms);
            }
            Some(format!("{ESQUEMA}historial"))
        }
        "favoritos" => {
            let destino = Some(format!("{ESQUEMA}favoritos"));
            let f = &mut d.favs;
            let hecho = if let Some(n) = param(&p, "nueva") {
                f.nueva_carpeta(n).is_some()
            } else if let (Some(id), Some(n)) = (num(&p, "ren"), param(&p, "nombre")) {
                f.renombrar(id, n)
            } else if let Some(id) = num(&p, "subir") {
                f.mover_pos(id, -1)
            } else if let Some(id) = num(&p, "bajar") {
                f.mover_pos(id, 1)
            } else if let (Some(id), Some(a)) = (num(&p, "mv"), num(&p, "a")) {
                f.mover_a(id, a)
            } else if let Some(id) = num::<u32>(&p, "quitar") {
                // Una carpeta con cosas dentro pide confirmación; un favorito o una vacía, no.
                if f.por_id(id).is_some_and(|x| x.carpeta) && !ok && !f.hijos(id).is_empty() {
                    return None;
                }
                f.quitar(id)
            } else {
                return None;
            };
            if hecho {
                d.cambio_favoritos(ms);
            }
            destino
        }
        _ => None,
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    const AHORA: i64 = 20_000 * 86_400 + 3600 * 15;

    fn datos() -> Datos {
        let mut d = Datos::default();
        d.cargado = true;
        d.hist.apuntar(AHORA - 18 * 3600, "https://b.com/", "Ayer");
        d.hist.apuntar(AHORA - 60, "https://a.com/x?y=1&z=2", "Título <b>&\"raro\"</b>");
        let c = d.favs.nueva_carpeta("Mi <carpeta>").unwrap();
        d.favs.anadir("https://f.com/", "Fav \"1\"", c).unwrap();
        d.favs.anadir("https://g.com/", "Fav 2", 0).unwrap();
        d
    }

    #[test]
    fn direcciones_internas() {
        assert!(es_interna("nightfury://historial") && es_interna("NightFury://x") && !es_interna("https://nightfury://"));
        assert_eq!(de_enlace("http://interno.invalid/historial?q=a+b#z").as_deref(), Some("nightfury://historial?q=a+b"));
        assert_eq!(de_enlace("HTTP://INTERNO.INVALID/favoritos").as_deref(), Some("nightfury://favoritos"));
        assert_eq!(de_enlace("https://interno.invalid/"), None);
        assert_eq!(de_enlace("http://ejemplo.com/"), None);
        let (r, p) = partir("nightfury://Historial/?q=a%20%C3%B1+b&borrar=5&x");
        assert_eq!(r, "historial");
        assert_eq!((param(&p, "q"), num::<i64>(&p, "borrar"), param(&p, "x")), (Some("a ñ b"), Some(5), Some("")));
    }

    #[test]
    fn el_html_escapa_lo_de_fuera() {
        let d = datos();
        let h = pagina(&d, "nightfury://historial", AHORA);
        assert!(!h.contains("<b>") && h.contains("&lt;b&gt;&amp;&quot;raro&quot;&lt;/b&gt;"), "{h}");
        assert!(h.contains("Hoy - ") && h.contains("Ayer - "), "agrupado por día");
        assert!(h.contains("https://a.com/x?y=1&amp;z=2"));
        let f = pagina(&d, "nightfury://favoritos", AHORA);
        assert!(!f.contains("<carpeta>") && f.contains("Mi &lt;carpeta&gt;") && f.contains("Fav &quot;1&quot;"));
        // El buscador filtra y no inyecta.
        let b = pagina(&d, "nightfury://historial?q=%22%3E%3Cscript%3E", AHORA);
        assert!(!b.contains("<script>") && b.contains("No hay nada que coincida"));
        assert!(pagina(&d, "nightfury://otra", AHORA).contains("No se encuentra"));
    }

    #[test]
    fn acciones_del_historial() {
        let mut d = datos();
        let ts = d.hist.v[0].ts;
        let h = pagina(&d, "nightfury://historial", AHORA);
        assert!(h.contains(&format!("borrar={ts}")));
        assert_eq!(aplicar(&mut d, &format!("nightfury://historial?q=a&borrar={ts}"), AHORA, 0).as_deref(), Some("nightfury://historial?q=a"));
        assert_eq!(d.hist.v.len(), 1);
        // Sin confirmar no borra: enseña la pregunta.
        assert_eq!(aplicar(&mut d, "nightfury://historial?limpiar=todo", AHORA, 0), None);
        assert_eq!(d.hist.v.len(), 1);
        assert!(pagina(&d, "nightfury://historial?limpiar=todo", AHORA).contains("name=\"ok\""));
        assert_eq!(aplicar(&mut d, "nightfury://historial?limpiar=hora&ok=1", AHORA, 0).as_deref(), Some("nightfury://historial"));
        assert!(d.hist.v.is_empty());
        assert_eq!(aplicar(&mut d, "nightfury://historial?limpiar=raro&ok=1", AHORA, 0), None);
    }

    #[test]
    fn la_casilla_de_cookies_pide_borrarlas() {
        let mut d = datos();
        assert!(pagina(&d, "nightfury://historial?limpiar=todo", AHORA).contains("También las cookies"));
        assert!(aplicar(&mut d, "nightfury://historial?limpiar=todo&ok=1", AHORA, 0).is_some());
        assert!(!d.borrar_cookies);
        assert!(aplicar(&mut d, "nightfury://historial?limpiar=todo&ok=1&cookies=1", AHORA, 0).is_some());
        assert!(d.borrar_cookies);
    }

    #[test]
    fn acciones_de_favoritos() {
        let mut d = datos();
        assert!(aplicar(&mut d, "nightfury://favoritos?nueva=Otra+carpeta", AHORA, 0).is_some());
        assert_eq!(d.favs.v.last().unwrap().titulo, "Otra carpeta");
        assert!(aplicar(&mut d, "nightfury://favoritos?nueva=", AHORA, 0).is_some(), "vacío: no crea, pero vuelve a la lista");
        assert_eq!(d.favs.v.len(), 4);
        assert!(aplicar(&mut d, "nightfury://favoritos?ren=1&nombre=Casa", AHORA, 0).is_some());
        assert_eq!(d.favs.por_id(1).unwrap().titulo, "Casa");
        assert!(pagina(&d, "nightfury://favoritos?mover=3", AHORA).contains("mv=3&amp;a=1"));
        assert!(aplicar(&mut d, "nightfury://favoritos?mv=3&a=1", AHORA, 0).is_some());
        assert_eq!(d.favs.hijos(1).len(), 2);
        // Carpeta con cosas: pide confirmar; vacía y favoritos, no.
        assert_eq!(aplicar(&mut d, "nightfury://favoritos?quitar=1", AHORA, 0), None);
        assert!(pagina(&d, "nightfury://favoritos?quitar=1", AHORA).contains("Casa"));
        assert!(aplicar(&mut d, "nightfury://favoritos?quitar=1&ok=1", AHORA, 0).is_some());
        assert_eq!(d.favs.v.len(), 1);
        assert!(aplicar(&mut d, "nightfury://favoritos?quitar=4", AHORA, 0).is_some());
        assert!(d.favs.v.is_empty());
        assert!(pagina(&d, "nightfury://favoritos", AHORA).contains("Todavía no tienes favoritos"));
    }
}
