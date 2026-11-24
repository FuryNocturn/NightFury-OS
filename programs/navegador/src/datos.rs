// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Historial y favoritos: modelo, formato en disco (texto, una entrada por línea, campos con tabulador),
//! purga de 90 días y sugerencias de la barra. Sin dibujo ni ventana: se prueba sola.
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use nf_core::files::join;
use nf_core::fsapi::FilesApi;
use nf_core::platform::DateTime;

pub const DIAS: i64 = 90;
pub const MAX_HISTORIAL: usize = 20_000;
pub const MAX_FAVORITOS: usize = 5_000;
pub const MAX_URL: usize = 2048;
const MAX_TITULO: usize = 200;
/// Dos visitas seguidas a la misma dirección en menos de esto cuentan como una (recargar).
const REPETIDA_S: i64 = 60;
/// Espera entre un cambio y su escritura en disco, y tras un fallo al escribir.
const RETRASO_MS: u64 = 2_000;
const REINTENTO_MS: u64 = 30_000;
const CARPETA: &str = "AppData\\Navegador";

// ------------------------------------------------------------ formato de texto

/// Escapa `\`, tabulador y saltos de línea para que quepan en un campo de una línea.
pub fn escapar(s: &str) -> String {
    let mut r = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => r.push_str("\\\\"),
            '\t' => r.push_str("\\t"),
            '\n' => r.push_str("\\n"),
            '\r' => r.push_str("\\r"),
            c => r.push(c),
        }
    }
    r
}

pub fn desescapar(s: &str) -> String {
    let mut r = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            r.push(c);
            continue;
        }
        match it.next() {
            Some('t') => r.push('\t'),
            Some('n') => r.push('\n'),
            Some('r') => r.push('\r'),
            Some('\\') => r.push('\\'),
            Some(o) => {
                r.push('\\');
                r.push(o);
            }
            None => r.push('\\'),
        }
    }
    r
}

fn recortar(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(MAX_TITULO).collect()
}

/// Host en minúsculas de una URL http o https.
pub fn host(url: &str) -> Option<String> {
    nf_core::net::http::parse_url(url).ok().map(|u| u.host.to_ascii_lowercase())
}

fn sin_www(h: &str) -> &str {
    h.strip_prefix("www.").unwrap_or(h)
}

// ------------------------------------------------------------ historial

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Visita {
    /// Segundos del reloj; es único y crece (sirve de identificador).
    pub ts: i64,
    pub url: String,
    pub titulo: String,
}

#[derive(Default)]
pub struct Historial {
    /// De la más antigua a la más reciente.
    pub v: Vec<Visita>,
}

impl Historial {
    /// Lee el texto del disco: las líneas rotas se saltan.
    pub fn leer(texto: &str) -> Historial {
        let mut v: Vec<Visita> = Vec::new();
        for l in texto.lines() {
            let mut c = l.split('\t');
            let (Some(ts), Some(url), Some(t), None) = (c.next(), c.next(), c.next(), c.next()) else { continue };
            let (Ok(ts), url) = (ts.parse::<i64>(), desescapar(url)) else { continue };
            if url.is_empty() || url.len() > MAX_URL {
                continue;
            }
            v.push(Visita { ts, url, titulo: recortar(&desescapar(t)) });
        }
        v.sort_by_key(|x| x.ts);
        let mut ant = i64::MIN;
        for x in v.iter_mut() {
            x.ts = x.ts.max(ant.saturating_add(1));
            ant = x.ts;
        }
        if v.len() > MAX_HISTORIAL {
            v.drain(..v.len() - MAX_HISTORIAL);
        }
        Historial { v }
    }

    pub fn escribir(&self) -> String {
        let mut s = String::new();
        for x in &self.v {
            s.push_str(&format!("{}\t{}\t{}\n", x.ts, escapar(&x.url), escapar(&x.titulo)));
        }
        s
    }

    /// Apunta una visita. Solo http y https; la repetida de hace menos de un minuto solo pone el título.
    pub fn apuntar(&mut self, ts: i64, url: &str, titulo: &str) -> bool {
        if url.len() > MAX_URL || !(url.starts_with("http://") || url.starts_with("https://")) {
            return false;
        }
        let titulo = recortar(titulo);
        if let Some(u) = self.v.last_mut() {
            if u.url == url && ts - u.ts < REPETIDA_S {
                let cambia = u.titulo != titulo && !titulo.is_empty();
                if cambia {
                    u.titulo = titulo;
                }
                return cambia;
            }
        }
        let ts = ts.max(self.v.last().map_or(i64::MIN, |u| u.ts.saturating_add(1)));
        self.v.push(Visita { ts, url: String::from(url), titulo });
        if self.v.len() > MAX_HISTORIAL {
            self.v.remove(0);
        }
        true
    }

    /// Quita lo de más de 90 días. `true` si ha quitado algo.
    pub fn purgar(&mut self, ahora: i64) -> bool {
        let n = self.v.len();
        let corte = ahora - DIAS * 86_400;
        self.v.retain(|x| x.ts >= corte);
        self.v.len() != n
    }

    /// Quita las visitas desde `desde` en adelante (0 = todo).
    pub fn borrar_desde(&mut self, desde: i64) -> bool {
        let n = self.v.len();
        self.v.retain(|x| x.ts < desde);
        self.v.len() != n
    }

    pub fn borrar(&mut self, ts: i64) -> bool {
        let n = self.v.len();
        self.v.retain(|x| x.ts != ts);
        self.v.len() != n
    }

    /// Las que contienen todas las palabras de `q` en la dirección o el título; la más reciente primero.
    pub fn buscar(&self, q: &str) -> Vec<&Visita> {
        let pal = palabras(q);
        self.v.iter().rev().filter(|x| coincide(&pal, &x.url, &x.titulo)).collect()
    }
}

fn palabras(q: &str) -> Vec<String> {
    q.split_whitespace().map(|p| p.to_lowercase()).collect()
}

fn coincide(pal: &[String], url: &str, titulo: &str) -> bool {
    let h = format!("{} {}", url.to_lowercase(), titulo.to_lowercase());
    pal.iter().all(|p| h.contains(p.as_str()))
}

// ------------------------------------------------------------ favoritos

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fav {
    /// Distinto de cero y único (favoritos y carpetas comparten la numeración).
    pub id: u32,
    /// 0 = la barra; si no, el id de una carpeta (solo hay un nivel de carpetas).
    pub padre: u32,
    pub carpeta: bool,
    /// Vacía en una carpeta.
    pub url: String,
    /// El nombre, si es una carpeta.
    pub titulo: String,
}

#[derive(Default)]
pub struct Favoritos {
    /// En el orden en que se enseñan (el de cada carpeta es el de sus hijos aquí dentro).
    pub v: Vec<Fav>,
}

impl Favoritos {
    pub fn leer(texto: &str) -> Favoritos {
        let mut v: Vec<Fav> = Vec::new();
        for l in texto.lines() {
            let mut c = l.split('\t');
            let (Some(t), Some(id), Some(padre), Some(url), Some(titulo), None) = (c.next(), c.next(), c.next(), c.next(), c.next(), c.next()) else { continue };
            let (Ok(id), Ok(padre), url) = (id.parse::<u32>(), padre.parse::<u32>(), desescapar(url)) else { continue };
            let carpeta = match t {
                "F" => false,
                "C" => true,
                _ => continue,
            };
            if id == 0 || v.iter().any(|f| f.id == id) || (!carpeta && (url.is_empty() || url.len() > MAX_URL)) {
                continue;
            }
            if v.len() >= MAX_FAVORITOS {
                break;
            }
            v.push(Fav { id, padre, carpeta, url: if carpeta { String::new() } else { url }, titulo: recortar(&desescapar(titulo)) });
        }
        // Un padre que no existe, o que no está en la barra, deja al hijo en la barra.
        let carpetas: Vec<u32> = v.iter().filter(|f| f.carpeta && f.padre == 0).map(|f| f.id).collect();
        for f in v.iter_mut() {
            if f.padre != 0 && (f.carpeta || !carpetas.contains(&f.padre)) {
                f.padre = 0;
            }
        }
        Favoritos { v }
    }

    pub fn escribir(&self) -> String {
        let mut s = String::new();
        for f in &self.v {
            s.push_str(&format!("{}\t{}\t{}\t{}\t{}\n", if f.carpeta { 'C' } else { 'F' }, f.id, f.padre, escapar(&f.url), escapar(&f.titulo)));
        }
        s
    }

    pub fn por_id(&self, id: u32) -> Option<&Fav> {
        self.v.iter().find(|f| f.id == id)
    }

    /// Lo que cuelga de `padre` (0 = la barra), en orden.
    pub fn hijos(&self, padre: u32) -> Vec<&Fav> {
        self.v.iter().filter(|f| f.padre == padre).collect()
    }

    /// Id del favorito (no carpeta) con esa dirección.
    pub fn buscar_url(&self, url: &str) -> Option<u32> {
        self.v.iter().find(|f| !f.carpeta && f.url == url).map(|f| f.id)
    }

    fn id_nuevo(&self) -> u32 {
        self.v.iter().map(|f| f.id).max().unwrap_or(0) + 1
    }

    pub fn anadir(&mut self, url: &str, titulo: &str, padre: u32) -> Option<u32> {
        if self.v.len() >= MAX_FAVORITOS || url.is_empty() || url.len() > MAX_URL {
            return None;
        }
        let id = self.id_nuevo();
        let titulo = recortar(titulo);
        self.v.push(Fav { id, padre, carpeta: false, url: String::from(url), titulo: if titulo.is_empty() { String::from(url) } else { titulo } });
        Some(id)
    }

    pub fn nueva_carpeta(&mut self, nombre: &str) -> Option<u32> {
        let nombre = recortar(nombre);
        if self.v.len() >= MAX_FAVORITOS || nombre.trim().is_empty() {
            return None;
        }
        let id = self.id_nuevo();
        self.v.push(Fav { id, padre: 0, carpeta: true, url: String::new(), titulo: nombre });
        Some(id)
    }

    /// Quita un favorito, o una carpeta con todo lo que lleva dentro.
    pub fn quitar(&mut self, id: u32) -> bool {
        let n = self.v.len();
        self.v.retain(|f| f.id != id && f.padre != id);
        self.v.len() != n
    }

    pub fn renombrar(&mut self, id: u32, nombre: &str) -> bool {
        let nombre = recortar(nombre);
        match self.v.iter_mut().find(|f| f.id == id) {
            Some(f) if !nombre.trim().is_empty() => {
                f.titulo = nombre;
                true
            }
            _ => false,
        }
    }

    /// Sube (`-1`) o baja (`1`) entre sus hermanos.
    pub fn mover_pos(&mut self, id: u32, d: i32) -> bool {
        let Some(i) = self.v.iter().position(|f| f.id == id) else { return false };
        let padre = self.v[i].padre;
        let j = if d < 0 { self.v[..i].iter().rposition(|f| f.padre == padre) } else { self.v[i + 1..].iter().position(|f| f.padre == padre).map(|k| i + 1 + k) };
        let Some(j) = j else { return false };
        self.v.swap(i, j);
        true
    }

    /// Cambia un favorito a la barra (`0`) o a una carpeta; va al final. Las carpetas no se anidan.
    pub fn mover_a(&mut self, id: u32, padre: u32) -> bool {
        let destino_vale = padre == 0 || self.por_id(padre).is_some_and(|f| f.carpeta);
        let Some(i) = self.v.iter().position(|f| f.id == id) else { return false };
        if !destino_vale || self.v[i].carpeta || self.v[i].padre == padre {
            return false;
        }
        let mut f = self.v.remove(i);
        f.padre = padre;
        self.v.push(f);
        true
    }

    /// Los favoritos (sin carpetas) en el orden de la barra, con lo de cada carpeta tras ella.
    pub fn planos(&self) -> Vec<&Fav> {
        let mut r = Vec::new();
        for f in self.hijos(0) {
            if f.carpeta {
                r.extend(self.hijos(f.id));
            } else {
                r.push(f);
            }
        }
        r
    }

    /// ¿Hay un favorito de ese sitio? Vale el mismo host o uno de sus subdominios (y al revés).
    pub fn es_sitio(&self, h: &str) -> bool {
        let h = h.trim_end_matches('.').to_ascii_lowercase();
        // `b.contains('.')`: «com» no hace favorito a todo el .com.
        let es_sub = |a: &str, b: &str| a == b || (b.contains('.') && a.strip_suffix(b).is_some_and(|p| p.ends_with('.')));
        !h.is_empty() && self.v.iter().filter(|f| !f.carpeta).filter_map(|f| host(&f.url)).any(|f| es_sub(&f, &h) || es_sub(&h, &f))
    }
}

// ------------------------------------------------------------ sugerencias

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sugerencia {
    pub url: String,
    pub titulo: String,
    pub favorito: bool,
}

/// Hasta `max` direcciones del historial y los favoritos que contienen todas las palabras de `texto`:
/// primero las que empiezan por lo escrito, luego los favoritos y luego lo más reciente.
pub fn sugerir(hist: &Historial, favs: &Favoritos, texto: &str, max: usize) -> Vec<Sugerencia> {
    let pal = palabras(texto);
    let Some(primera) = pal.first() else { return Vec::new() };
    let mut r: Vec<Sugerencia> = Vec::new();
    let mut nuevas = |url: &str, titulo: &str, favorito: bool| {
        if coincide(&pal, url, titulo) && !r.iter().any(|s| s.url == url) {
            r.push(Sugerencia { url: String::from(url), titulo: String::from(titulo), favorito });
        }
    };
    for f in favs.v.iter().filter(|f| !f.carpeta) {
        nuevas(&f.url, &f.titulo, true);
    }
    for x in hist.v.iter().rev() {
        nuevas(&x.url, &x.titulo, false);
    }
    let empieza = |s: &Sugerencia| host(&s.url).is_some_and(|h| sin_www(&h).starts_with(primera.as_str()));
    r.sort_by_key(|s| !empieza(s));
    r.truncate(max);
    r
}

// ------------------------------------------------------------ en disco

/// Lo del usuario que se guarda: historial y favoritos, cargados una vez y escritos con retraso.
#[derive(Default)]
pub struct Datos {
    pub hist: Historial,
    pub favs: Favoritos,
    pub cargado: bool,
    /// Ventana de incógnito: no se lee ni se escribe el historial.
    pub incog: bool,
    /// Reloj (ms) a partir del cual toca escribir cada fichero.
    h_en: Option<u64>,
    f_en: Option<u64>,
}

impl Datos {
    fn ruta(fs: &mut dyn FilesApi, nombre: &str) -> String {
        join(&join(&fs.home(), CARPETA), nombre)
    }

    /// Lee los ficheros (una sola vez) y purga lo de más de 90 días.
    pub fn cargar(&mut self, fs: &mut dyn FilesApi, ahora: i64, ms: u64) {
        if self.cargado {
            return;
        }
        self.cargado = true;
        let leer = |fs: &mut dyn FilesApi, n: &str| {
            let ruta = Self::ruta(fs, n);
            fs.read(&ruta).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
        };
        self.favs = Favoritos::leer(&leer(fs, "favoritos"));
        if !self.incog {
            self.hist = Historial::leer(&leer(fs, "historial"));
            if self.hist.purgar(ahora) {
                self.h_en = Some(ms + RETRASO_MS);
            }
        }
    }

    pub fn cambio_historial(&mut self, ms: u64) {
        if !self.incog {
            self.h_en.get_or_insert(ms + RETRASO_MS);
        }
    }

    pub fn cambio_favoritos(&mut self, ms: u64) {
        self.f_en.get_or_insert(ms + RETRASO_MS);
    }

    /// Escribe lo que toque (o todo lo pendiente, con `forzar`: al cerrar).
    pub fn guardar(&mut self, fs: &mut dyn FilesApi, ms: u64, now: DateTime, forzar: bool) {
        let toca = |t: Option<u64>| t.is_some_and(|t| forzar || ms >= t);
        if toca(self.h_en) {
            let ok = Self::escribir(fs, "historial", &self.hist.escribir(), now);
            self.h_en = (!ok).then_some(ms + REINTENTO_MS);
        }
        if toca(self.f_en) {
            let ok = Self::escribir(fs, "favoritos", &self.favs.escribir(), now);
            self.f_en = (!ok).then_some(ms + REINTENTO_MS);
        }
    }

    // `FilesApi::write` ya es atómico (sustituye el fichero entero o no toca nada): no hace falta temporal.
    fn escribir(fs: &mut dyn FilesApi, nombre: &str, texto: &str, now: DateTime) -> bool {
        let ruta = Self::ruta(fs, nombre);
        if fs.write(&ruta, texto.as_bytes(), now).is_ok() {
            return true;
        }
        // Falta la carpeta del programa: se crea (y las de arriba, por si acaso) y se prueba otra vez.
        let casa = fs.home();
        let appdata = join(&casa, "AppData");
        for d in [casa, appdata.clone(), join(&appdata, "Navegador")] {
            let _ = fs.mkdir(&d, now);
        }
        fs.write(&ruta, texto.as_bytes(), now).is_ok()
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    const D: i64 = 86_400;

    #[test]
    fn escapes_ida_y_vuelta_con_caracteres_raros() {
        for s in ["", "a\tb\nc\rd\\e", "\\t literal", "fin\\", "ñandú 日本 \u{1F600}", "\\\\n"] {
            assert_eq!(desescapar(&escapar(s)), s);
            assert!(!escapar(s).contains(['\t', '\n', '\r']));
        }
    }

    #[test]
    fn historial_ida_y_vuelta_y_lineas_rotas() {
        let mut h = Historial::default();
        assert!(h.apuntar(1000, "https://a.com/x?q=1\ty", "Título\ncon\ttabs \\ y ñ"));
        assert!(h.apuntar(2000, "http://b.org/", ""));
        let t = h.escribir();
        let l = Historial::leer(&format!("basura\n\n{t}1\t2\n3\t\tx\nabc\thttp://c/\tt\n5\thttps://x/\tt\textra\n"));
        assert_eq!(l.v, h.v, "lo roto se salta y lo bueno vuelve igual");
        assert_eq!(l.v[0].titulo, "Títulocontabs \\ y ñ", "los caracteres de control se quitan al recortar");
    }

    #[test]
    fn historial_limites_y_repetidas() {
        let mut h = Historial::default();
        assert!(!h.apuntar(1, "ftp://x/", "t"), "solo http y https");
        assert!(!h.apuntar(1, &format!("https://a.com/{}", "x".repeat(MAX_URL)), "t"), "url demasiado larga");
        assert!(h.apuntar(100, "https://a.com/", ""));
        assert!(h.apuntar(110, "https://a.com/", "Título"), "la repetida solo pone el título");
        assert_eq!(h.v.len(), 1);
        assert_eq!(h.v[0].titulo, "Título");
        assert!(h.apuntar(300, "https://a.com/", "x"), "pasado un minuto es otra visita");
        assert!(h.apuntar(50, "https://b.com/", "x"));
        assert!(h.v[2].ts > h.v[1].ts, "el id crece aunque el reloj vaya hacia atrás");
        // Tope de entradas: se pierden las más antiguas.
        let mut t = String::new();
        for i in 0..MAX_HISTORIAL + 5 {
            t.push_str(&format!("{i}\thttps://a.com/{i}\tt\n"));
        }
        let l = Historial::leer(&t);
        assert_eq!(l.v.len(), MAX_HISTORIAL);
        assert_eq!(l.v[0].ts, 5);
    }

    #[test]
    fn purga_de_90_dias_y_borrar_por_intervalo() {
        let ahora = 200 * D;
        let mut h = Historial::default();
        for (ts, u) in [(ahora - 91 * D, "https://viejo.com/"), (ahora - 89 * D, "https://casi.com/"), (ahora - 2 * D, "https://ayer.com/"), (ahora - 1800, "https://hace-media-hora.com/"), (ahora - 10, "https://ahora.com/")] {
            h.apuntar(ts, u, "t");
        }
        assert!(h.purgar(ahora));
        assert_eq!(h.v.len(), 4);
        assert!(!h.purgar(ahora));
        // Última hora.
        assert!(h.borrar_desde(ahora - 3600));
        assert_eq!(h.v.len(), 2);
        // Último día: no toca lo de anteayer.
        assert!(!h.borrar_desde(ahora - D));
        assert!(h.borrar(h.v[0].ts));
        assert_eq!(h.v.len(), 1);
        assert!(h.borrar_desde(0));
        assert!(h.v.is_empty());
    }

    #[test]
    fn busqueda_en_historial() {
        let mut h = Historial::default();
        h.apuntar(1, "https://es.wikipedia.org/wiki/Gato", "Gato - Wikipedia");
        h.apuntar(2, "https://ejemplo.com/perro", "Mi PERRO");
        let r = h.buscar("perro ejemplo");
        assert_eq!(r.len(), 1);
        assert_eq!(h.buscar("").len(), 2);
        assert_eq!(h.buscar("wiki gato")[0].ts, 1);
        assert!(h.buscar("pez").is_empty());
    }

    fn favs() -> Favoritos {
        let mut f = Favoritos::default();
        let c = f.nueva_carpeta("Trabajo\tx").unwrap();
        f.anadir("https://a.com/", "A", 0).unwrap();
        f.anadir("https://b.com/", "B", c).unwrap();
        f.anadir("https://c.com/", "C", 0).unwrap();
        f
    }

    #[test]
    fn favoritos_con_carpetas_ida_y_vuelta() {
        let mut f = favs();
        assert_eq!(f.hijos(0).len(), 3);
        assert_eq!(f.hijos(1).len(), 1);
        assert_eq!(f.planos().iter().map(|x| x.titulo.as_str()).collect::<Vec<_>>(), ["B", "A", "C"]);
        let l = Favoritos::leer(&format!("{}roto\nF\t9\t0\t\tsin url\nF\t2\t0\thttps://dup/\tx\nF\t8\t77\thttps://huerfano/\th\nC\t9\t1\tcarpeta en carpeta\t\n", f.escribir()));
        assert_eq!(l.v[..4], f.v[..], "lo bueno vuelve igual");
        assert_eq!(l.por_id(8).unwrap().padre, 0, "padre inexistente: a la barra");
        assert_eq!(l.por_id(9).unwrap().padre, 0, "las carpetas no se anidan");
        assert!(l.por_id(2).unwrap().url == "https://a.com/", "id repetido: gana el primero");
        // mover, subir, bajar, renombrar, borrar
        assert!(f.mover_a(2, 1));
        assert_eq!(f.hijos(1).iter().map(|x| x.titulo.as_str()).collect::<Vec<_>>(), ["B", "A"]);
        assert!(!f.mover_a(1, 1), "una carpeta no entra en otra");
        assert!(f.mover_pos(2, -1));
        assert_eq!(f.hijos(1)[0].titulo, "A");
        assert!(!f.mover_pos(2, -1), "ya es el primero");
        assert!(f.mover_a(2, 0));
        assert!(f.renombrar(1, "Casa") && !f.renombrar(1, "  "));
        assert!(f.quitar(1), "borrar la carpeta se lleva lo de dentro");
        assert_eq!(f.v.iter().map(|x| x.titulo.as_str()).collect::<Vec<_>>(), ["C", "A"]);
        assert_eq!(f.buscar_url("https://c.com/"), Some(4));
        assert!(f.anadir("", "x", 0).is_none());
    }

    #[test]
    fn favorito_de_un_sitio() {
        let f = favs();
        assert!(f.es_sitio("a.com") && f.es_sitio("WWW.A.COM.") && f.es_sitio("x.a.com"));
        assert!(!f.es_sitio("ba.com") && !f.es_sitio("com") && !f.es_sitio(""));
    }

    #[test]
    fn sugerencias() {
        let f = favs();
        let mut h = Historial::default();
        h.apuntar(1, "https://www.avisos.net/", "Avisos");
        h.apuntar(2, "https://a.com/", "A repetido");
        h.apuntar(3, "https://otra.com/a", "contiene a");
        let s = sugerir(&h, &f, "a", 8);
        assert_eq!(s[0].url, "https://a.com/", "empieza por lo escrito y es favorito");
        assert!(s[0].favorito && s.iter().filter(|x| x.url == "https://a.com/").count() == 1, "sin repetidas");
        assert_eq!(s[1].url, "https://www.avisos.net/", "el host empieza por «a» (sin www)");
        assert!(sugerir(&h, &f, "  ", 8).is_empty());
        assert_eq!(sugerir(&h, &f, "a", 2).len(), 2);
        assert_eq!(sugerir(&h, &f, "contiene a", 8)[0].url, "https://otra.com/a");
        assert!(sugerir(&Historial::default(), &f, "avisos", 8).is_empty());
    }
}
