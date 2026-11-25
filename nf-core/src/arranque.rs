// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Lógica pura del menú de arranque (`arranque.efi`): ajustes guardados, detección de sistemas, lista de
//! entradas y estado del menú. Sin hardware: se prueba con `cargo test -p nf-core arranque`.
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// Variable UEFI (con `GUID_NF`) donde se guardan los `Ajustes`.
pub const VARIABLE: &str = "NfArranque";
/// GUID de fabricante propio (d1, d2, d3, d4) de NightFury.
pub const GUID_NF: (u32, u16, u16, [u8; 8]) = (0x67e6dee3, 0xaa3f, 0x4995, [0xbe, 0x6b, 0x46, 0x43, 0x5b, 0x1f, 0x52, 0x9e]);
/// Tamaño máximo de `Ajustes::a_bytes`.
pub const MAX_AJUSTES: usize = 1024;
const MAX_SEGUNDOS: u8 = 30;
const FIJAS: [&str; 3] = ["nightfury", "reiniciar", "apagar"];

#[derive(Clone, Debug, PartialEq)]
pub struct Ajustes {
    pub segundos: u8,
    /// Clave de la última entrada elegida.
    pub ultimo: Option<String>,
    pub ocultas: Vec<String>,
    /// (clave, nombre) para renombrar entradas.
    pub nombres: Vec<(String, String)>,
}

impl Default for Ajustes {
    fn default() -> Self {
        Ajustes { segundos: 5, ultimo: None, ocultas: Vec::new(), nombres: Vec::new() }
    }
}

/// Cadena con prefijo de longitud (u8) en `b` desde `i`.
fn cadena(b: &[u8], i: &mut usize) -> Option<String> {
    let n = *b.get(*i)? as usize;
    let s = core::str::from_utf8(b.get(*i + 1..*i + 1 + n)?).ok()?.to_string();
    *i += 1 + n;
    Some(s)
}

impl Ajustes {
    /// Formato: "NFA", versión 1, segundos, último (longitud u8 + texto, 0 = ninguno), nº de ocultas + cada
    /// una, nº de nombres + pares. Lo que no cabe en `MAX_AJUSTES` (o pasa de 255 bytes) se descarta.
    pub fn a_bytes(&self) -> Vec<u8> {
        let mut o = vec![b'N', b'F', b'A', 1, self.segundos.min(MAX_SEGUNDOS)];
        let u = self.ultimo.as_deref().filter(|s| s.len() < 256).unwrap_or("");
        o.push(u.len() as u8);
        o.extend(u.bytes());
        o.push(0);
        let pos = o.len() - 1;
        // Se deja sitio para el byte de cuenta de los nombres.
        for c in &self.ocultas {
            if c.len() > 255 || c.len() + 2 + o.len() > MAX_AJUSTES {
                continue;
            }
            o.push(c.len() as u8);
            o.extend(c.bytes());
            o[pos] += 1;
        }
        o.push(0);
        let pos = o.len() - 1;
        for (c, n) in &self.nombres {
            if c.len() > 255 || n.len() > 255 || c.len() + n.len() + 2 + o.len() > MAX_AJUSTES {
                continue;
            }
            o.push(c.len() as u8);
            o.extend(c.bytes());
            o.push(n.len() as u8);
            o.extend(n.bytes());
            o[pos] += 1;
        }
        o
    }

    /// Datos ausentes, cortados, de otra versión o corruptos → los de por defecto.
    pub fn de_bytes(b: &[u8]) -> Ajustes {
        Self::leer(b).unwrap_or_default()
    }

    /// `None` si los datos no valen (a diferencia de `de_bytes`, que da los de por defecto).
    pub fn leer(b: &[u8]) -> Option<Ajustes> {
        if b.len() > MAX_AJUSTES || b.get(..4)? != b"NFA\x01" {
            return None;
        }
        let segundos = *b.get(4)?;
        if segundos > MAX_SEGUNDOS {
            return None;
        }
        let mut i = 5;
        let ultimo = Some(cadena(b, &mut i)?).filter(|s| !s.is_empty());
        let n = *b.get(i)?;
        i += 1;
        let ocultas = (0..n).map(|_| cadena(b, &mut i)).collect::<Option<Vec<_>>>()?;
        let n = *b.get(i)?;
        i += 1;
        let nombres = (0..n).map(|_| Some((cadena(b, &mut i)?, cadena(b, &mut i)?))).collect::<Option<Vec<_>>>()?;
        Some(Ajustes { segundos, ultimo, ocultas, nombres })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tipo {
    NightFury,
    Windows,
    Linux,
    /// Entrada `Boot####` del firmware.
    Uefi(u16),
    Firmware,
    Reiniciar,
    Apagar,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entrada {
    pub clave: String,
    pub nombre: String,
    pub tipo: Tipo,
}

impl Entrada {
    fn nueva(clave: &str, nombre: &str, tipo: Tipo) -> Entrada {
        Entrada { clave: clave.to_string(), nombre: nombre.to_string(), tipo }
    }
}

/// Sistema encontrado en una ESP: dónde (índice de ESP y ruta) para poder arrancarlo.
#[derive(Clone, Debug, PartialEq)]
pub struct Detectado {
    pub entrada: Entrada,
    pub esp: usize,
    pub ruta: String,
}

pub const WINDOWS: &str = "\\EFI\\Microsoft\\Boot\\bootmgfw.efi";
pub const NIGHTFURY: &str = "\\EFI\\NightFury\\nightfury.efi";
pub const SYSTEMD: &str = "\\EFI\\systemd\\systemd-bootx64.efi";

/// Compara rutas como FAT: sin distinguir mayúsculas.
fn igual(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn capitalizar(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or(String::new(), |p| p.to_uppercase().chain(c).collect())
}

/// Carpeta `<d>` de `\EFI\<d>\<fichero>` si la ruta es de ese tipo.
fn carpeta_de<'a>(ruta: &'a str, fichero: &str) -> Option<&'a str> {
    let r = ruta.strip_prefix("\\EFI\\").or_else(|| ruta.strip_prefix("\\efi\\"))?;
    let (d, f) = r.split_once('\\')?;
    (igual(f, fichero) && !d.is_empty()).then_some(d)
}

/// Sistemas que hay en las ESP (`rutas_por_esp[i]` = rutas halladas en la ESP `i`). Uno por sistema, el de
/// la primera ESP; en Linux gana `shimx64.efi` sobre `grubx64.efi`. Orden: NightFury, Windows, Linux por clave.
pub fn detectar(rutas_por_esp: &[Vec<String>]) -> Vec<Detectado> {
    let mut v: Vec<Detectado> = Vec::new();
    let mut poner = |clave: String, nombre: String, tipo: Tipo, esp: usize, ruta: &String| {
        if !v.iter().any(|d| d.entrada.clave == clave) {
            v.push(Detectado { entrada: Entrada { clave, nombre, tipo }, esp, ruta: ruta.clone() });
        }
    };
    for (esp, rutas) in rutas_por_esp.iter().enumerate() {
        for r in rutas.iter().filter(|r| igual(r, NIGHTFURY)) {
            poner("nightfury".into(), "NightFury".into(), Tipo::NightFury, esp, r);
        }
    }
    for (esp, rutas) in rutas_por_esp.iter().enumerate() {
        for r in rutas.iter().filter(|r| igual(r, WINDOWS)) {
            poner("windows".into(), "Windows".into(), Tipo::Windows, esp, r);
        }
    }
    for fichero in ["shimx64.efi", "grubx64.efi"] {
        for (esp, rutas) in rutas_por_esp.iter().enumerate() {
            for r in rutas {
                if let Some(d) = carpeta_de(r, fichero).filter(|d| !["microsoft", "boot", "nightfury", "systemd"].iter().any(|x| igual(d, x))) {
                    poner(format!("linux:{}", d.to_ascii_lowercase()), capitalizar(d), Tipo::Linux, esp, r);
                }
            }
        }
    }
    for (esp, rutas) in rutas_por_esp.iter().enumerate() {
        for r in rutas.iter().filter(|r| igual(r, SYSTEMD)) {
            poner("linux:systemd".into(), "systemd-boot".into(), Tipo::Linux, esp, r);
        }
    }
    v
}

/// Una `EFI_LOAD_OPTION` (UEFI 2.x 3.1.3): atributos, descripción y la ruta del último nodo File Path.
struct Opcion {
    activa: bool,
    descripcion: String,
    ruta: Option<String>,
}

/// `None` si los datos no encajan (cortos, sin terminador, longitudes fuera de rango).
fn leer_opcion(v: &[u8]) -> Option<Opcion> {
    let attr = u32::from_le_bytes(v.get(..4)?.try_into().ok()?);
    let lista_len = u16::from_le_bytes(v.get(4..6)?.try_into().ok()?) as usize;
    let mut i = 6;
    let mut desc = Vec::new();
    loop {
        let c = u16::from_le_bytes(v.get(i..i + 2)?.try_into().ok()?);
        i += 2;
        if c == 0 {
            break;
        }
        desc.push(c);
    }
    let lista = v.get(i..i.checked_add(lista_len)?)?;
    let (mut p, mut ruta) = (0, None);
    while let Some(h) = lista.get(p..p + 4) {
        let n = u16::from_le_bytes([h[2], h[3]]) as usize;
        if n < 4 || p + n > lista.len() || h[0] == 0x7F {
            break;
        }
        if h[0] == 4 && h[1] == 4 {
            let u: Vec<u16> = lista[p + 4..p + n].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
            ruta = Some(String::from_utf16_lossy(&u));
        }
        p += n;
    }
    Some(Opcion { activa: attr & 1 != 0 && attr & 8 == 0, descripcion: String::from_utf16_lossy(&desc), ruta })
}

/// Entradas `Boot####` activas del `BootOrder` (en ese orden) que no son la propia «NightFury» ni repiten un
/// sistema ya detectado (por ruta, por nombre o «Windows Boot Manager»).
pub fn otras_uefi(boot_order: &[u16], opciones: &[(u16, Vec<u8>)], detectados: &[Detectado]) -> Vec<Entrada> {
    let mut v = Vec::new();
    for n in boot_order {
        let Some(o) = opciones.iter().find(|(m, _)| m == n).and_then(|(_, d)| leer_opcion(d)) else { continue };
        let d = o.descripcion.to_lowercase();
        let repetida = detectados.iter().any(|x| {
            o.ruta.as_deref().is_some_and(|r| igual(r, &x.ruta))
                || d.contains(&x.entrada.nombre.to_lowercase())
                || (x.entrada.tipo == Tipo::Windows && d.contains("windows"))
        });
        if o.activa && !o.descripcion.is_empty() && !d.contains("nightfury") && !repetida {
            v.push(Entrada { clave: format!("boot:{n:04X}"), nombre: o.descripcion, tipo: Tipo::Uefi(*n) });
        }
    }
    v
}

/// Lista final del menú. NightFury, Reiniciar y Apagar no se pueden ocultar.
pub fn lista(detectados: &[Detectado], otras: &[Entrada], ajustes: &Ajustes, firmware_ok: bool) -> Vec<Entrada> {
    let nf = Entrada::nueva("nightfury", "NightFury", Tipo::NightFury);
    let mut v = vec![nf];
    v.extend(detectados.iter().map(|d| d.entrada.clone()).filter(|e| e.tipo != Tipo::NightFury));
    v.extend(otras.iter().cloned());
    if firmware_ok {
        v.push(Entrada::nueva("firmware", "Configuración del firmware", Tipo::Firmware));
    }
    v.push(Entrada::nueva("reiniciar", "Reiniciar", Tipo::Reiniciar));
    v.push(Entrada::nueva("apagar", "Apagar", Tipo::Apagar));
    v.retain(|e| FIJAS.contains(&e.clave.as_str()) || !ajustes.ocultas.contains(&e.clave));
    for e in &mut v {
        if let Some((_, n)) = ajustes.nombres.iter().find(|(c, n)| *c == e.clave && !n.is_empty()) {
            e.nombre = n.clone();
        }
    }
    v
}

/// Índice del último sistema elegido si sigue en la lista; si no, NightFury (o 0).
pub fn por_defecto(lista: &[Entrada], ajustes: &Ajustes) -> usize {
    let buscar = |k: &str| lista.iter().position(|e| e.clave == k);
    ajustes.ultimo.as_deref().and_then(buscar).or_else(|| buscar("nightfury")).unwrap_or(0)
}

/// Variable UEFI (con `GUID_NF`) con las entradas detectadas en el último arranque del menú, antes de aplicar
/// ocultas y nombres. La escribe `arranque.efi` y la lee Configuración.
pub const VARIABLE_LISTA: &str = "NfArranqueLista";
/// Tamaño máximo de `lista_a_bytes`.
pub const MAX_LISTA: usize = 2048;
/// Letras como máximo de un nombre puesto por el usuario.
pub const MAX_NOMBRE: usize = 32;

/// Entrada para Configuración: (clave, nombre original, se puede ocultar).
pub type Item = (String, String, bool);

/// Lo que Configuración enseña de un `lista` sin ocultas ni nombres: sin Reiniciar ni Apagar.
pub fn para_lista(lista: &[Entrada]) -> Vec<Item> {
    lista.iter().filter(|e| !matches!(e.tipo, Tipo::Reiniciar | Tipo::Apagar)).map(|e| (e.clave.clone(), e.nombre.clone(), !FIJAS.contains(&e.clave.as_str()))).collect()
}

/// Formato: "NFL", versión 1, nº de entradas y por cada una: se puede ocultar (0/1), clave y nombre (longitud u8
/// + texto). Lo que no cabe en `MAX_LISTA` (o pasa de 255 bytes) se descarta.
pub fn lista_a_bytes(v: &[Item]) -> Vec<u8> {
    let mut o = vec![b'N', b'F', b'L', 1, 0];
    for (c, n, oc) in v {
        if c.len() > 255 || n.len() > 255 || c.len() + n.len() + 3 + o.len() > MAX_LISTA || o[4] == 255 {
            continue;
        }
        o.push(*oc as u8);
        o.push(c.len() as u8);
        o.extend(c.bytes());
        o.push(n.len() as u8);
        o.extend(n.bytes());
        o[4] += 1;
    }
    o
}

/// `None` si falta algo, sobra algo o es de otra versión.
pub fn lista_de_bytes(b: &[u8]) -> Option<Vec<Item>> {
    if b.len() > MAX_LISTA || b.get(..4)? != b"NFL\x01" {
        return None;
    }
    let mut i = 5;
    let v = (0..*b.get(4)?)
        .map(|_| {
            let oc = *b.get(i)?;
            i += 1;
            Some((cadena(b, &mut i)?, cadena(b, &mut i)?, match oc { 0 => false, 1 => true, _ => return None }))
        })
        .collect::<Option<Vec<_>>>()?;
    (i == b.len()).then_some(v)
}

/// Lo que Configuración lee del menú de arranque.
#[derive(Clone, Debug, PartialEq)]
pub struct InfoArranque {
    pub ajustes: Ajustes,
    pub entradas: Vec<Item>,
    /// La entrada «NightFury» del firmware es la primera de `BootOrder`.
    pub primero: bool,
}

impl InfoArranque {
    /// Para viajar por el servicio: `primero`, longitud (u16) de los ajustes, ajustes y lista.
    pub fn a_bytes(&self) -> Vec<u8> {
        let aj = self.ajustes.a_bytes();
        let mut o = vec![self.primero as u8];
        o.extend((aj.len() as u16).to_le_bytes());
        o.extend(aj);
        o.extend(lista_a_bytes(&self.entradas));
        o
    }

    pub fn de_bytes(b: &[u8]) -> Option<InfoArranque> {
        let n = u16::from_le_bytes(b.get(1..3)?.try_into().ok()?) as usize;
        let ajustes = Ajustes::leer(b.get(3..3 + n)?)?;
        let entradas = lista_de_bytes(b.get(3 + n..)?)?;
        let primero = match b[0] { 0 => false, 1 => true, _ => return None };
        Some(InfoArranque { ajustes, entradas, primero })
    }
}

/// Motivos de fallo de `Platform::arranque_*`. Van por el servicio como índice: el texto tiene que estar aquí.
pub const ERRORES: [&str; 6] = [
    "No disponible en este equipo.",
    "Hace falta una cuenta de administrador.",
    "Los valores no son válidos.",
    "El firmware no deja guardar el cambio.",
    "No se encuentra la entrada de NightFury en el firmware.",
    "El firmware no deja cambiar el orden de arranque.",
];

/// Comprueba lo que se va a guardar: cuenta atrás hasta 30 s, nombres de hasta 32 letras y nada oculto de lo fijo.
pub fn validar(a: &Ajustes) -> Result<(), &'static str> {
    let mal = a.segundos > MAX_SEGUNDOS || a.nombres.iter().any(|(_, n)| n.chars().count() > MAX_NOMBRE) || a.ocultas.iter().any(|c| FIJAS.contains(&c.as_str()));
    if mal { Err(ERRORES[2]) } else { Ok(()) }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tecla {
    Arriba,
    Abajo,
    Enter,
    Otra,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Accion {
    Arrancar(usize),
}

pub struct Menu {
    pub lista: Vec<Entrada>,
    pub sel: usize,
    /// Segundos que faltan; `None` si una tecla paró la cuenta.
    pub cuenta: Option<u8>,
    pub aviso: Option<String>,
}

impl Menu {
    /// Con `segundos` = 0, el primer `tic_segundo` arranca el elegido: quien lo use mira antes si hay una
    /// tecla pulsada (y la pasa a `tecla`); si no la hay, arranca sin dibujar el menú.
    pub fn nuevo(lista: Vec<Entrada>, defecto: usize, segundos: u8) -> Menu {
        let sel = defecto.min(lista.len().saturating_sub(1));
        Menu { lista, sel, cuenta: Some(segundos.min(MAX_SEGUNDOS)), aviso: None }
    }

    /// Cualquier tecla para la cuenta atrás y borra el aviso. Flechas dan la vuelta; Enter arranca.
    pub fn tecla(&mut self, t: Tecla) -> Option<Accion> {
        self.cuenta = None;
        self.aviso = None;
        let n = self.lista.len();
        match t {
            Tecla::Arriba if n > 0 => self.sel = (self.sel + n - 1) % n,
            Tecla::Abajo if n > 0 => self.sel = (self.sel + 1) % n,
            Tecla::Enter if n > 0 => return Some(Accion::Arrancar(self.sel)),
            _ => {}
        }
        None
    }

    /// Pasa un segundo; al llegar a 0 arranca el elegido. Sin cuenta (tecla pulsada) no hace nada.
    pub fn tic_segundo(&mut self) -> Option<Accion> {
        let n = self.cuenta?;
        self.cuenta = Some(n.saturating_sub(1));
        (n <= 1 && !self.lista.is_empty()).then_some(Accion::Arrancar(self.sel))
    }

    /// Líneas de pantalla. Usa «·», «↑» y «↓»: la consola UEFI puede no tenerlos (ver `a_ascii`).
    pub fn pintar(&self) -> Vec<String> {
        let mut v = vec!["NightFury · Elige sistema".to_string(), String::new()];
        for (i, e) in self.lista.iter().enumerate() {
            v.push(format!("{} {}", if i == self.sel { ">" } else { " " }, e.nombre));
        }
        v.push(String::new());
        let mut pie = "↑↓ elegir · Enter arrancar".to_string();
        if let (Some(n), Some(e)) = (self.cuenta, self.lista.get(self.sel)) {
            pie += &format!(" · Arranca {} en {} s", e.nombre, n);
        }
        v.push(pie);
        if let Some(a) = &self.aviso {
            v.push(String::new());
            v.push(a.clone());
        }
        v
    }
}

/// Sustituye lo que la consola UEFI suele no tener por ASCII.
pub fn a_ascii(s: &str) -> String {
    s.chars().map(|c| match c {
        '·' => '-',
        '↑' => '^',
        '↓' => 'v',
        c => c,
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rutas(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn opcion(desc: &str, ruta: &str) -> Vec<u8> {
        let ruta: Vec<u8> = ruta.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        let mut lista = vec![4, 4];
        lista.extend(((4 + ruta.len()) as u16).to_le_bytes());
        lista.extend(ruta);
        lista.extend([0x7F, 0xFF, 4, 0]);
        let mut o = 1u32.to_le_bytes().to_vec();
        o.extend((lista.len() as u16).to_le_bytes());
        o.extend(desc.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
        o.extend(lista);
        o
    }

    #[test]
    fn ajustes_ida_y_vuelta() {
        let a = Ajustes {
            segundos: 12,
            ultimo: Some("windows".into()),
            ocultas: vec!["firmware".into()],
            nombres: vec![("windows".into(), "Juegos".into())],
        };
        assert_eq!(Ajustes::de_bytes(&a.a_bytes()), a);
        assert_eq!(Ajustes::de_bytes(&Ajustes::default().a_bytes()), Ajustes::default());
    }

    #[test]
    fn ajustes_invalidos_dan_los_de_por_defecto() {
        let bueno = Ajustes { ultimo: Some("linux:ubuntu".into()), ocultas: vec!["a".into()], ..Ajustes::default() }.a_bytes();
        assert_eq!(Ajustes::de_bytes(&[]), Ajustes::default());
        assert_eq!(Ajustes::de_bytes(b"XXXXXXXX"), Ajustes::default());
        for n in 0..bueno.len() {
            assert_eq!(Ajustes::de_bytes(&bueno[..n]), Ajustes::default(), "cortado a {n}");
        }
        let mut mal = bueno.clone();
        mal[4] = 99; // segundos fuera de rango
        assert_eq!(Ajustes::de_bytes(&mal), Ajustes::default());
        let mut mal = bueno;
        mal[6] = 0xFF; // UTF-8 inválido
        assert_eq!(Ajustes::de_bytes(&mal), Ajustes::default());
        assert_eq!(Ajustes::de_bytes(&[0u8; 2000]), Ajustes::default());
    }

    #[test]
    fn ajustes_no_pasan_de_1024() {
        let a = Ajustes {
            ocultas: (0..100).map(|i| format!("oculta-{i:0>50}")).collect(),
            nombres: (0..100).map(|i| (format!("c{i}"), "n".repeat(100))).collect(),
            ..Ajustes::default()
        };
        let b = a.a_bytes();
        assert!(b.len() <= MAX_AJUSTES);
        let r = Ajustes::de_bytes(&b);
        assert!(!r.ocultas.is_empty() && r.ocultas.len() < 100 && r.ocultas[0] == a.ocultas[0]);
    }

    #[test]
    fn detecta_sistemas() {
        let d = detectar(&[
            rutas(&["\\EFI\\ubuntu\\grubx64.efi", "\\EFI\\fedora\\shimx64.efi", "\\EFI\\fedora\\grubx64.efi", "\\EFI\\BOOT\\grubx64.efi"]),
            rutas(&["\\EFI\\Microsoft\\Boot\\bootmgfw.efi", "\\EFI\\NightFury\\nightfury.efi", "\\EFI\\ubuntu\\shimx64.efi", "\\EFI\\systemd\\systemd-bootx64.efi"]),
        ]);
        let claves: Vec<_> = d.iter().map(|x| x.entrada.clave.as_str()).collect();
        assert_eq!(claves, ["nightfury", "windows", "linux:fedora", "linux:ubuntu", "linux:systemd"]);
        assert_eq!(d[2].ruta, "\\EFI\\fedora\\shimx64.efi");
        assert_eq!(d[3].entrada.nombre, "Ubuntu");
        // Ubuntu: shim de la ESP 1 gana a grub de la ESP 0 (se prefiere shim).
        assert_eq!((d[3].esp, d[3].ruta.as_str()), (1, "\\EFI\\ubuntu\\shimx64.efi"));
        assert_eq!(d[4].entrada.nombre, "systemd-boot");
        assert!(detectar(&[rutas(&["\\EFI\\BOOT\\BOOTX64.EFI"])]).is_empty());
        assert_eq!(detectar(&[rutas(&["\\efi\\MICROSOFT\\boot\\BOOTMGFW.EFI"])][..])[0].entrada.tipo, Tipo::Windows);
    }

    #[test]
    fn otras_entradas_uefi() {
        let det = detectar(&[rutas(&["\\EFI\\Microsoft\\Boot\\bootmgfw.efi", "\\EFI\\ubuntu\\shimx64.efi"])]);
        let ops = vec![
            (1, opcion("Windows Boot Manager", "\\EFI\\Microsoft\\Boot\\bootmgfw.efi")),
            (2, opcion("NightFury", "\\EFI\\NightFury\\nightfury.efi")),
            (3, opcion("ubuntu", "\\EFI\\ubuntu\\shimx64.efi")),
            (4, opcion("Disco USB", "\\x.efi")),
            (5, { let mut o = opcion("Inactiva", "\\y.efi"); o[0] = 0; o }),
            (6, vec![1, 0, 0]),
            (0x1A, opcion("Red PXE", "")),
        ];
        let v = otras_uefi(&[4, 1, 2, 3, 5, 6, 7, 0x1A], &ops, &det);
        let claves: Vec<_> = v.iter().map(|e| (e.clave.as_str(), e.nombre.as_str())).collect();
        assert_eq!(claves, [("boot:0004", "Disco USB"), ("boot:001A", "Red PXE")]);
        assert_eq!(v[0].tipo, Tipo::Uefi(4));
        // Basura: ninguna llega a entrar ni a hacer pánico.
        assert!(otras_uefi(&[1], &[(1, vec![1, 0, 0, 0, 255, 255, 65])], &[]).is_empty());
        assert!(otras_uefi(&[1], &[(1, vec![1, 0, 0, 0, 3, 0, 65, 0, 0, 0])], &[]).is_empty());
    }

    #[test]
    fn lista_y_defecto() {
        let det = detectar(&[rutas(&["\\EFI\\Microsoft\\Boot\\bootmgfw.efi", "\\EFI\\ubuntu\\shimx64.efi"])]);
        let otras = vec![Entrada::nueva("boot:0004", "Disco USB", Tipo::Uefi(4))];
        let mut a = Ajustes::default();
        let l = lista(&det, &otras, &a, true);
        let claves: Vec<_> = l.iter().map(|e| e.clave.as_str()).collect();
        assert_eq!(claves, ["nightfury", "windows", "linux:ubuntu", "boot:0004", "firmware", "reiniciar", "apagar"]);
        assert_eq!(lista(&det, &otras, &a, false).len(), 6);
        a.ocultas = ["windows", "nightfury", "reiniciar", "apagar", "firmware"].map(String::from).to_vec();
        a.nombres = vec![("linux:ubuntu".into(), "Mi Linux".into())];
        a.ultimo = Some("linux:ubuntu".into());
        let l = lista(&det, &otras, &a, true);
        let claves: Vec<_> = l.iter().map(|e| e.clave.as_str()).collect();
        assert_eq!(claves, ["nightfury", "linux:ubuntu", "boot:0004", "reiniciar", "apagar"]);
        assert_eq!(l[1].nombre, "Mi Linux");
        assert_eq!(por_defecto(&l, &a), 1);
        a.ultimo = Some("windows".into());
        assert_eq!(por_defecto(&l, &a), 0);
        a.ultimo = None;
        assert_eq!(por_defecto(&l, &a), 0);
    }

    #[test]
    fn lista_ida_y_vuelta_y_basura() {
        let det = detectar(&[rutas(&["\\EFI\\Microsoft\\Boot\\bootmgfw.efi"])]);
        let l = lista(&det, &[Entrada::nueva("boot:0004", "Disco ñ", Tipo::Uefi(4))], &Ajustes::default(), true);
        let v = para_lista(&l);
        let claves: Vec<_> = v.iter().map(|(c, _, o)| (c.as_str(), *o)).collect();
        assert_eq!(claves, [("nightfury", false), ("windows", true), ("boot:0004", true), ("firmware", true)]);
        let b = lista_a_bytes(&v);
        assert_eq!(lista_de_bytes(&b), Some(v));
        assert_eq!(lista_de_bytes(&lista_a_bytes(&[])), Some(vec![]));
        for n in 0..b.len() {
            assert_eq!(lista_de_bytes(&b[..n]), None, "cortada a {n}");
        }
        let mut largo = b.clone();
        largo.push(0);
        assert_eq!(lista_de_bytes(&largo), None);
        let mut mal = b.clone();
        mal[5] = 7; // indicador distinto de 0 y 1
        assert_eq!(lista_de_bytes(&mal), None);
        mal = b.clone();
        mal[8] = 0xFF; // UTF-8 inválido
        assert_eq!(lista_de_bytes(&mal), None);
        assert_eq!(lista_de_bytes(&[0u8; 3000]), None);
        assert_eq!(lista_de_bytes(b"NFL\x01\xFF"), None);
        let mucho: Vec<Item> = (0..300).map(|i| (format!("c{i}"), "n".repeat(200), true)).collect();
        let b = lista_a_bytes(&mucho);
        assert!(b.len() <= MAX_LISTA);
        assert!(lista_de_bytes(&b).is_some_and(|r| !r.is_empty() && r.len() < 300));
    }

    #[test]
    fn info_arranque_y_validar() {
        let i = InfoArranque { ajustes: Ajustes { segundos: 9, ultimo: Some("windows".into()), ..Ajustes::default() }, entradas: vec![("windows".into(), "Windows".into(), true)], primero: true };
        let b = i.a_bytes();
        assert_eq!(InfoArranque::de_bytes(&b), Some(i));
        for n in 0..b.len() {
            assert_eq!(InfoArranque::de_bytes(&b[..n]), None, "cortada a {n}");
        }
        assert_eq!(InfoArranque::de_bytes(&[2, 0, 0]), None);
        assert_eq!(InfoArranque::de_bytes(&[0xFF; 50]), None);
        let mut a = Ajustes::default();
        assert!(validar(&a).is_ok());
        a.segundos = 31;
        assert!(validar(&a).is_err());
        a.segundos = 30;
        a.nombres = vec![("windows".into(), "ñ".repeat(32))];
        assert!(validar(&a).is_ok());
        a.nombres = vec![("windows".into(), "ñ".repeat(33))];
        assert!(validar(&a).is_err());
        a.nombres.clear();
        a.ocultas = vec!["nightfury".into()];
        assert!(validar(&a).is_err());
        a.ocultas = vec!["firmware".into(), "windows".into()];
        assert!(validar(&a).is_ok());
    }

    fn menu(segundos: u8) -> Menu {
        let l = lista(&[], &[], &Ajustes::default(), false);
        Menu::nuevo(l, 0, segundos)
    }

    #[test]
    fn cuenta_atras_y_teclas() {
        let mut m = menu(2);
        assert_eq!(m.tic_segundo(), None);
        assert_eq!(m.cuenta, Some(1));
        assert_eq!(m.tic_segundo(), Some(Accion::Arrancar(0)));
        let mut m = menu(3);
        m.tecla(Tecla::Otra);
        assert_eq!(m.cuenta, None);
        assert_eq!(m.tic_segundo(), None);
        assert_eq!(m.tecla(Tecla::Arriba), None);
        assert_eq!(m.sel, 2); // da la vuelta: NightFury, Reiniciar, Apagar
        m.tecla(Tecla::Abajo);
        assert_eq!(m.sel, 0);
        m.tecla(Tecla::Abajo);
        assert_eq!(m.tecla(Tecla::Enter), Some(Accion::Arrancar(1)));
        assert_eq!(menu(0).tic_segundo(), Some(Accion::Arrancar(0)));
    }

    #[test]
    fn pantalla() {
        let mut m = menu(5);
        let p = m.pintar();
        assert_eq!(p[0], "NightFury · Elige sistema");
        assert_eq!(p[2], "> NightFury");
        assert_eq!(p[3], "  Reiniciar");
        assert_eq!(p.last().unwrap(), "↑↓ elegir · Enter arrancar · Arranca NightFury en 5 s");
        m.tecla(Tecla::Otra);
        m.aviso = Some("No arrancó".into());
        let p = m.pintar();
        assert_eq!(p[p.len() - 3], "↑↓ elegir · Enter arrancar");
        assert_eq!(a_ascii("↑↓ a · b"), "^v a - b");
    }
}
