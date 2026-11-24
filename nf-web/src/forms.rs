// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Formularios (HTML Living Standard, §4.10): los controles de un documento con su estado, la edición de
//! texto y la construcción del envío (conjunto de datos y `application/x-www-form-urlencoded`).
//! Toda la entrada es hostil: límites de controles, de largo de valor y de tamaño del cuerpo.
use crate::html::{self, Dom};
use alloc::string::String;
use alloc::vec::Vec;

/// «Este nodo no es un control» (y «sin imagen» en las listas de dibujo).
pub const SIN_CTL: u32 = u32::MAX;
/// Controles por documento como mucho (el resto se ignora).
pub const MAX_CONTROLES: usize = 4000;
/// Opciones de `<select>` entre todo el documento.
const MAX_OPCIONES: usize = 20_000;
/// Letras de un valor (más largo se corta al cargar y al escribir).
pub const MAX_VALOR: usize = 65_536;
/// Bytes del cuerpo (o de la consulta) de un envío; más grande no se envía.
const MAX_CUERPO: usize = 1 << 20;
/// Largo máximo de la URL de un envío por GET (más allá, los servidores la rechazan).
const MAX_URL_GET: usize = 8192;
const FORMS_MAX: usize = 2000;

/// Alto de una línea de texto dentro de un control (el maquetado y el pintado deben coincidir).
pub fn alto_linea(px: f32) -> i32 {
    nf_core::math::iround(px.clamp(0.0, 1000.0) * 1.15).max(1)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tipo {
    /// text, search, email, url, tel, number y cualquier tipo desconocido.
    Texto,
    Password,
    Casilla,
    Radio,
    /// `input submit` y `button` de tipo submit.
    Enviar,
    Restablecer,
    Boton,
    /// `input image`.
    Imagen,
    Select,
    Area,
    Oculto,
}

#[derive(Clone, Debug)]
pub struct Opcion {
    pub etiqueta: String,
    pub valor: String,
    pub deshab: bool,
}

#[derive(Clone, Debug)]
pub struct Control {
    pub nodo: usize,
    pub tipo: Tipo,
    pub nombre: String,
    /// Texto actual (campos y área); el atributo `value` en el resto.
    pub valor: String,
    pub valor0: String,
    pub marcado: bool,
    pub marcado0: bool,
    pub deshab: bool,
    pub lectura: bool,
    /// 0 = sin límite.
    pub max_largo: usize,
    /// `size` (campos) o `cols` (área).
    pub tam: usize,
    pub filas: usize,
    pub marcador: String,
    /// Rótulo de los botones.
    pub etiqueta: String,
    /// `<form>` al que pertenece (nodo).
    pub form: Option<usize>,
    pub opciones: Vec<Opcion>,
    pub sel: usize,
    pub sel0: usize,
    /// Cursor y ancla de la selección, en letras.
    pub cur: usize,
    pub ancla: Option<usize>,
    pub accion: Option<String>,
    pub metodo: Option<String>,
    pub auto: bool,
}

impl Control {
    pub fn es_texto(&self) -> bool {
        matches!(self.tipo, Tipo::Texto | Tipo::Password | Tipo::Area)
    }

    pub fn n(&self) -> usize {
        self.valor.chars().count()
    }

    /// Posición en bytes de la letra `i` (el final si se pasa).
    pub fn byte(&self, i: usize) -> usize {
        self.valor.char_indices().nth(i).map_or(self.valor.len(), |(b, _)| b)
    }

    pub fn seleccion(&self) -> Option<(usize, usize)> {
        let a = self.ancla?;
        (a != self.cur).then(|| (a.min(self.cur), a.max(self.cur)))
    }

    fn quitar_seleccion(&mut self) -> bool {
        let Some((a, b)) = self.seleccion() else {
            self.ancla = None;
            return false;
        };
        let (ba, bb) = (self.byte(a), self.byte(b));
        self.valor.replace_range(ba..bb, "");
        self.cur = a;
        self.ancla = None;
        true
    }

    pub fn insertar(&mut self, c: char) {
        if self.lectura || !(!c.is_control() || (c == '\n' && self.tipo == Tipo::Area)) {
            return;
        }
        self.quitar_seleccion();
        let tope = if self.max_largo > 0 { self.max_largo.min(MAX_VALOR) } else { MAX_VALOR };
        if self.n() >= tope {
            return;
        }
        let b = self.byte(self.cur);
        self.valor.insert(b, c);
        self.cur += 1;
    }

    /// Retroceso (`atras`) o Supr.
    pub fn borrar(&mut self, atras: bool) {
        if self.lectura || self.quitar_seleccion() {
            return;
        }
        let i = if atras {
            if self.cur == 0 {
                return;
            }
            self.cur -= 1;
            self.cur
        } else {
            self.cur
        };
        let b = self.byte(i);
        if b < self.valor.len() {
            self.valor.remove(b);
        }
    }

    pub fn mover(&mut self, dest: usize, extender: bool) {
        if extender {
            self.ancla.get_or_insert(self.cur);
        } else {
            self.ancla = None;
        }
        self.cur = dest.min(self.n());
    }

    /// Letra donde empieza cada línea (el área tiene varias; el resto, una).
    pub fn inicios(&self) -> Vec<usize> {
        let mut v = alloc::vec![0];
        if self.tipo == Tipo::Area {
            v.extend(self.valor.chars().enumerate().filter(|&(_, c)| c == '\n').map(|(i, _)| i + 1));
        }
        v
    }

    /// (línea, columna) del cursor.
    pub fn linea_cursor(&self) -> (usize, usize) {
        let st = self.inicios();
        let l = st.iter().rposition(|&s| s <= self.cur).unwrap_or(0);
        (l, self.cur - st[l])
    }

    /// Mueve el cursor `d` líneas (área) conservando la columna.
    pub fn mover_linea(&mut self, d: isize, extender: bool) {
        let st = self.inicios();
        let (l, col) = self.linea_cursor();
        let t = l as isize + d;
        if t < 0 || t as usize >= st.len() {
            return;
        }
        let t = t as usize;
        let largo = if t + 1 < st.len() { st[t + 1] - st[t] - 1 } else { self.n() - st[t] };
        self.mover(st[t] + col.min(largo), extender);
    }

    /// Principio o fin de la línea del cursor.
    pub fn a_extremo(&mut self, fin: bool, extender: bool) {
        let st = self.inicios();
        let (l, _) = self.linea_cursor();
        let d = if !fin {
            st[l]
        } else if l + 1 < st.len() {
            st[l + 1] - 1
        } else {
            self.n()
        };
        self.mover(d, extender);
    }

    fn reiniciar(&mut self) {
        self.valor = self.valor0.clone();
        self.marcado = self.marcado0;
        self.sel = self.sel0;
        self.cur = 0;
        self.ancla = None;
    }
}

/// Los controles del documento y su relación con los nodos.
pub struct Forms {
    pub ctl: Vec<Control>,
    /// Control de cada nodo (`SIN_CTL` si no es uno).
    pub ctl_de: Vec<u32>,
    /// Control al que apunta la etiqueta (`<label>`) que contiene cada nodo: índice + 1; 0 = ninguno.
    pub etiqueta_de: Vec<u32>,
}

fn recortar(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Texto de una sola línea (sin saltos).
fn una_linea(s: &str, max: usize) -> String {
    s.chars().filter(|c| !matches!(c, '\n' | '\r')).take(max).collect()
}

/// Saltos de línea del área como `\n`.
fn lf(s: &str, max: usize) -> String {
    let mut r = String::new();
    let mut it = s.chars().take(max).peekable();
    while let Some(c) = it.next() {
        match c {
            '\r' => {
                if it.peek() == Some(&'\n') {
                    it.next();
                }
                r.push('\n');
            }
            c => r.push(c),
        }
    }
    r
}

fn opciones_de(dom: &Dom, sel: usize, total: &mut usize) -> (Vec<Opcion>, usize) {
    let mut ops = Vec::new();
    let mut elegida = None;
    let mut pila: Vec<usize> = dom.nodos[sel].hijos.iter().rev().map(|&h| h as usize).collect();
    while let Some(n) = pila.pop() {
        if *total >= MAX_OPCIONES || ops.len() >= 2000 {
            break;
        }
        match dom.etq(n) {
            "option" => {
                *total += 1;
                let texto = html::compactar(&recortar(&dom.texto(n), 512));
                let etiqueta = dom.attr(n, "label").map(|l| html::compactar(&recortar(l, 512))).filter(|l| !l.is_empty()).unwrap_or_else(|| texto.clone());
                let valor = dom.attr(n, "value").map(|v| recortar(v, MAX_VALOR)).unwrap_or(texto);
                let deshab = dom.attr(n, "disabled").is_some();
                if dom.attr(n, "selected").is_some() && !deshab {
                    elegida = Some(ops.len());
                }
                ops.push(Opcion { etiqueta, valor, deshab });
            }
            "optgroup" | "select" if n != sel => pila.extend(dom.nodos[n].hijos.iter().rev().map(|&h| h as usize)),
            _ => {}
        }
    }
    let k = elegida.or_else(|| ops.iter().position(|o| !o.deshab)).unwrap_or(0);
    (ops, k)
}

fn nuevo_control(dom: &Dom, i: usize, opciones: &mut usize) -> Option<Control> {
    let attr = |k: &str| dom.attr(i, k);
    let tipo_attr = attr("type").map(|t| t.trim().to_ascii_lowercase()).unwrap_or_default();
    let tipo = match dom.etq(i) {
        "input" => match tipo_attr.as_str() {
            "hidden" => Tipo::Oculto,
            "checkbox" => Tipo::Casilla,
            "radio" => Tipo::Radio,
            "submit" => Tipo::Enviar,
            "reset" => Tipo::Restablecer,
            "button" => Tipo::Boton,
            "image" => Tipo::Imagen,
            "password" => Tipo::Password,
            "file" => return None,
            _ => Tipo::Texto,
        },
        "button" => match tipo_attr.as_str() {
            "reset" => Tipo::Restablecer,
            "button" => Tipo::Boton,
            _ => Tipo::Enviar,
        },
        "select" => Tipo::Select,
        _ => Tipo::Area,
    };
    let num = |k: &str, def: usize, max: usize| attr(k).and_then(|v| v.trim().parse::<usize>().ok()).filter(|&v| v > 0).map_or(def, |v| v.min(max));
    let max_largo = num("maxlength", 0, MAX_VALOR);
    let tope = if max_largo > 0 { max_largo } else { MAX_VALOR };
    let valor = match tipo {
        Tipo::Texto | Tipo::Password => una_linea(attr("value").unwrap_or(""), tope),
        Tipo::Area => lf(&dom.texto(i), tope),
        Tipo::Casilla | Tipo::Radio => recortar(attr("value").unwrap_or("on"), MAX_VALOR),
        _ => recortar(attr("value").unwrap_or(""), MAX_VALOR),
    };
    let etiqueta = match (dom.etq(i), tipo) {
        ("button", _) => html::compactar(&recortar(&dom.texto(i), 256)),
        (_, Tipo::Enviar | Tipo::Restablecer | Tipo::Boton) if !valor.is_empty() => recortar(&valor, 256),
        (_, Tipo::Imagen) => attr("alt").map(|a| recortar(a.trim(), 256)).filter(|a| !a.is_empty()).unwrap_or_else(|| String::from("Enviar")),
        (_, Tipo::Enviar) => String::from("Enviar"),
        (_, Tipo::Restablecer) => String::from("Restablecer"),
        _ => String::new(),
    };
    let (opciones, sel) = if tipo == Tipo::Select { opciones_de(dom, i, opciones) } else { (Vec::new(), 0) };
    let marcado = matches!(tipo, Tipo::Casilla | Tipo::Radio) && attr("checked").is_some();
    Some(Control {
        nodo: i,
        tipo,
        nombre: recortar(attr("name").unwrap_or(""), 1024),
        valor0: valor.clone(),
        cur: valor.chars().count(),
        valor,
        marcado,
        marcado0: marcado,
        deshab: attr("disabled").is_some(),
        lectura: attr("readonly").is_some(),
        max_largo,
        tam: if tipo == Tipo::Area { num("cols", 20, 500) } else { num("size", 20, 500) },
        filas: num("rows", 2, 100),
        marcador: una_linea(attr("placeholder").unwrap_or(""), 256),
        etiqueta,
        form: None,
        opciones,
        sel,
        sel0: sel,
        ancla: None,
        accion: attr("formaction").map(|a| recortar(a, 8192)),
        metodo: attr("formmethod").map(|a| recortar(a, 16)),
        auto: attr("autofocus").is_some(),
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Metodo {
    Get,
    Post,
}

/// Lo que hay que pedir para enviar un formulario.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Envio {
    pub metodo: Metodo,
    /// Con GET lleva ya la consulta; con POST es la `action`.
    pub url: String,
    /// `application/x-www-form-urlencoded` (solo POST).
    pub cuerpo: Option<Vec<u8>>,
    /// Lleva una contraseña escrita.
    pub tiene_password: bool,
}

/// `application/x-www-form-urlencoded`: los saltos de línea pasan a CRLF, el espacio a `+` y todo lo que no sea
/// `*-._` ni alfanumérico ASCII a %XX en UTF-8.
pub(crate) fn urlencode(s: &str, out: &mut String) {
    let mut it = s.chars().peekable();
    let mut b = [0u8; 4];
    while let Some(c) = it.next() {
        let c = match c {
            '\r' => {
                if it.peek() == Some(&'\n') {
                    it.next();
                }
                '\n'
            }
            c => c,
        };
        let trozo: &str = if c == '\n' { "\r\n" } else { c.encode_utf8(&mut b) };
        for &x in trozo.as_bytes() {
            match x {
                b'*' | b'-' | b'.' | b'_' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' => out.push(x as char),
                b' ' => out.push('+'),
                _ => {
                    out.push('%');
                    out.push(char::from_digit((x >> 4) as u32, 16).unwrap_or('0').to_ascii_uppercase());
                    out.push(char::from_digit((x & 15) as u32, 16).unwrap_or('0').to_ascii_uppercase());
                }
            }
        }
    }
}

impl Forms {
    pub fn construir(dom: &Dom) -> Forms {
        let n = dom.nodos.len();
        let (mut form_de, mut lab_de) = (alloc::vec![SIN_CTL; n], alloc::vec![SIN_CTL; n]);
        for i in 0..n {
            if let Some(p) = dom.padre(i) {
                (form_de[i], lab_de[i]) = (form_de[p], lab_de[p]);
            }
            match dom.etq(i) {
                "form" => form_de[i] = i as u32,
                "label" => lab_de[i] = i as u32,
                _ => {}
            }
        }
        let forms: Vec<usize> = (0..n).filter(|&i| dom.etq(i) == "form").take(FORMS_MAX).collect();
        let (mut ctl, mut ctl_de, mut ops) = (Vec::new(), alloc::vec![SIN_CTL; n], 0);
        for i in 0..n {
            if ctl.len() >= MAX_CONTROLES {
                break;
            }
            if !matches!(dom.etq(i), "input" | "button" | "select" | "textarea") {
                continue;
            }
            let Some(mut c) = nuevo_control(dom, i, &mut ops) else { continue };
            c.form = match dom.attr(i, "form") {
                Some(id) => forms.iter().copied().find(|&f| !id.is_empty() && dom.attr(f, "id") == Some(id)),
                None => (form_de[i] != SIN_CTL).then(|| form_de[i] as usize),
            };
            ctl_de[i] = ctl.len() as u32;
            ctl.push(c);
        }
        let mut f = Forms { ctl, ctl_de, etiqueta_de: alloc::vec![0; n] };
        for k in 0..f.ctl.len() {
            if f.ctl[k].tipo == Tipo::Radio && f.ctl[k].marcado {
                f.marcar_radio(k);
            }
        }
        // `<label>`: su control es el de `for` o, si no, el primero que contiene.
        let mut destino = alloc::vec![0u32; n];
        for (k, c) in f.ctl.iter().enumerate().filter(|(_, c)| c.tipo != Tipo::Oculto) {
            let l = lab_de[c.nodo];
            if l != SIN_CTL && destino[l as usize] == 0 && dom.attr(l as usize, "for").is_none() {
                destino[l as usize] = k as u32 + 1;
            }
        }
        for (l, _) in lab_de.iter().enumerate().filter(|&(i, &x)| x == i as u32 && dom.etq(i) == "label").take(FORMS_MAX) {
            if let Some(id) = dom.attr(l, "for").filter(|v| !v.is_empty()) {
                if let Some(k) = f.ctl.iter().position(|c| c.tipo != Tipo::Oculto && dom.attr(c.nodo, "id") == Some(id)) {
                    destino[l] = k as u32 + 1;
                }
            }
        }
        for i in 0..n {
            if lab_de[i] != SIN_CTL {
                f.etiqueta_de[i] = destino[lab_de[i] as usize];
            }
        }
        f
    }

    /// Marca el radio `k` y desmarca los de su grupo (mismo formulario y mismo nombre).
    pub fn marcar_radio(&mut self, k: usize) {
        let (form, nombre) = (self.ctl[k].form, self.ctl[k].nombre.clone());
        self.ctl[k].marcado = true;
        if nombre.is_empty() {
            return;
        }
        for (j, c) in self.ctl.iter_mut().enumerate() {
            if j != k && c.tipo == Tipo::Radio && c.form == form && c.nombre == nombre {
                c.marcado = false;
            }
        }
    }

    pub fn restablecer(&mut self, form: usize) {
        for c in self.ctl.iter_mut().filter(|c| c.form == Some(form)) {
            c.reiniciar();
        }
    }

    /// Cuántos campos de texto tiene el formulario y cuál es su primer botón de envío.
    fn campos_y_boton(&self, form: usize) -> (usize, Option<usize>) {
        let mut campos = 0;
        let mut boton = None;
        for (k, c) in self.ctl.iter().enumerate().filter(|(_, c)| c.form == Some(form)) {
            if matches!(c.tipo, Tipo::Texto | Tipo::Password) {
                campos += 1;
            }
            if matches!(c.tipo, Tipo::Enviar | Tipo::Imagen) && !c.deshab && boton.is_none() {
                boton = Some(k);
            }
        }
        (campos, boton)
    }

    /// Envío implícito (Enter en un campo): con el primer botón de envío; sin botón, solo si hay un campo.
    pub fn envio_implicito(&self, dom: &Dom, base: &str, doc_url: &str, form: usize) -> Option<Envio> {
        let (campos, boton) = self.campos_y_boton(form);
        match boton {
            Some(b) => self.envio(dom, base, doc_url, form, Some((b, 0, 0))),
            None if campos <= 1 => self.envio(dom, base, doc_url, form, None),
            None => None,
        }
    }

    /// Envío del formulario `form` (nodo). `boton` = (control que lo envía, x, y) con las coordenadas del clic
    /// si es una imagen. `None` si la URL no es válida o el envío es demasiado grande.
    pub fn envio(&self, dom: &Dom, base: &str, doc_url: &str, form: usize, boton: Option<(usize, i32, i32)>) -> Option<Envio> {
        let pulsado = boton.and_then(|b| self.ctl.get(b.0));
        let accion = pulsado.and_then(|c| c.accion.as_deref()).filter(|a| !a.trim().is_empty()).or_else(|| dom.attr(form, "action").filter(|a| !a.trim().is_empty()));
        let url = match accion {
            Some(a) => crate::url::resolver(base, a)?,
            None => crate::url::resolver(doc_url, "")?,
        };
        let m = pulsado.and_then(|c| c.metodo.as_deref()).or_else(|| dom.attr(form, "method")).unwrap_or("");
        let metodo = if m.trim().eq_ignore_ascii_case("post") { Metodo::Post } else { Metodo::Get };
        let mut cuerpo = String::new();
        let mut password = false;
        let mut par = |nombre: &str, valor: &str| -> bool {
            if !cuerpo.is_empty() {
                cuerpo.push('&');
            }
            urlencode(nombre, &mut cuerpo);
            cuerpo.push('=');
            urlencode(valor, &mut cuerpo);
            cuerpo.len() <= MAX_CUERPO
        };
        for (k, c) in self.ctl.iter().enumerate().filter(|(_, c)| c.form == Some(form) && !c.deshab) {
            let es_boton = boton.is_some_and(|b| b.0 == k);
            let ok = match c.tipo {
                Tipo::Enviar if es_boton && !c.nombre.is_empty() => par(&c.nombre, &c.valor),
                Tipo::Imagen if es_boton => {
                    let (x, y) = boton.map_or((0, 0), |b| (b.1, b.2));
                    let pre = if c.nombre.is_empty() { String::new() } else { alloc::format!("{}.", c.nombre) };
                    par(&alloc::format!("{pre}x"), &alloc::format!("{x}")) && par(&alloc::format!("{pre}y"), &alloc::format!("{y}"))
                }
                Tipo::Casilla | Tipo::Radio if c.marcado && !c.nombre.is_empty() => par(&c.nombre, &c.valor),
                Tipo::Select if !c.nombre.is_empty() => c.opciones.get(c.sel).filter(|o| !o.deshab).map_or(true, |o| par(&c.nombre, &o.valor)),
                Tipo::Texto | Tipo::Password | Tipo::Area | Tipo::Oculto if !c.nombre.is_empty() => {
                    password |= c.tipo == Tipo::Password && !c.valor.is_empty();
                    par(&c.nombre, &c.valor)
                }
                _ => true,
            };
            if !ok {
                return None;
            }
        }
        match metodo {
            Metodo::Post => Some(Envio { metodo, url, cuerpo: Some(cuerpo.into_bytes()), tiene_password: password }),
            Metodo::Get => {
                let mut u = String::from(url.split('?').next().unwrap_or(""));
                if !cuerpo.is_empty() {
                    u.push('?');
                    u.push_str(&cuerpo);
                }
                (u.len() <= MAX_URL_GET).then_some(Envio { metodo, url: u, cuerpo: None, tiene_password: password })
            }
        }
    }
}
