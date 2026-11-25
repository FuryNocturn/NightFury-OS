// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Pantalla «Arranque desde pendrive» (v0.12.6): Probar, Portátil e Instalar. Sale antes que ninguna
//! otra pantalla de cuentas cuando el sistema arrancó desde un pendrive (`Platform::pen_menu`) y espera
//! sin límite. Es parte de `Login` (se ve como el resto de pantallas de cuentas y comparte su foco, su
//! ratón y su teclado): aquí solo está lo propio de este modo.
use super::*;
use crate::fs::FsError;
use crate::platform::{Donde, PenDisco, PenEstado};
use crate::vol::instalar::Lugar;
use crate::vol::FsKind;

/// Qué pantalla del modo se ve.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum Paso {
    #[default]
    Menu,
    /// Portátil: aviso de que se creará la partición.
    Confirmar,
    /// Portátil: creando la partición y cambiando C: al pendrive.
    Preparando,
    /// Instalar: lista de discos del equipo.
    Instalar,
    /// Instalar: dónde (disco entero, hueco o partición) o quitar NightFury del disco elegido.
    Donde,
    /// Qué se hará (instalar o quitar), con el campo «BORRAR» si se pierde algo.
    Borrar,
    /// Instalando o quitando (sin porcentaje).
    Instalando,
    /// Terminado: quitar el pendrive y reiniciar.
    Instalado,
    /// NightFury quitado del disco.
    Quitado,
}

/// Lo que se hace con el disco elegido.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Accion {
    #[default]
    Entero,
    /// Sitio `k` de `analisis.sitios`.
    Sitio(usize),
    Quitar,
}

/// Una fila de «¿Dónde?»: `bloqueo` la deja en gris y sin poder elegirse; `borra` pide escribir BORRAR.
struct Opcion {
    accion: Accion,
    texto: String,
    nota: String,
    bloqueo: Option<&'static str>,
    borra: bool,
}

#[derive(Default)]
pub(super) struct PenUi {
    pub paso: Paso,
    /// Última respuesta de `Platform::pen_estado` (`None` hasta la primera).
    pub est: Option<PenEstado>,
    /// Aviso rojo bajo las tarjetas (error al preparar el pendrive).
    pub aviso: Option<&'static str>,
    /// Discos internos donde se puede instalar (los que dio `Platform::pen_discos`, con tope).
    pub discos: Vec<PenDisco>,
    /// Disco elegido en `discos`.
    pub sel: usize,
    /// Fila elegida en «¿Dónde?» (la acción está en `accion`).
    pub op: usize,
    accion: Accion,
    /// Instalado: ¿quedó creada la entrada de arranque UEFI?
    pub entrada: bool,
}

const TARJETAS: [(Icon, &str, &str); 3] = [
    (Icon::Equipo, "Probar", "Usa NightFury sin instalar nada. Al apagar se pierde todo."),
    (Icon::Usb, "Portátil", "Guarda tus cuentas y archivos en este pendrive."),
    (Icon::Drive, "Instalar", "Instala NightFury en un disco de este equipo."),
];
/// Alto de cada disco de la lista: nombre y hasta dos líneas de contenido.
const FILA_H: i32 = 74;
const TARJETA_W: i32 = 280;
const TARJETA_H: i32 = 250;
const HUECO: i32 = 24;
const SIN_ESPACIO: &str = "Este pendrive no tiene espacio libre. Graba la imagen en un pendrive de al menos 1 GB.";
const DESACTIVADA: u32 = 0x707070;
/// La palabra que hay que escribir para confirmar que se borra el disco.
const BORRAR: &str = "BORRAR";
/// Discos que caben en la lista (shortcut: el resto no se ofrece; ningún equipo normal tiene tantos).
const MAX_DISCOS: usize = 5;
/// Sitios (huecos y particiones) que se ofrecen por disco (shortcut: los primeros por orden de disco).
const MAX_SITIOS: usize = 5;
/// Alto de cada fila de «¿Dónde?»: texto y nota.
const OP_H: i32 = 46;
/// Menos que esto no cabe en disco entero: lo mismo que rechaza el núcleo.
const MIN_ENTERO: u64 = 1 << 30;
const BITLOCKER: &str = "NightFury no se instala en discos con BitLocker.";
/// Menos que esto no cabe (ESP de 256 MiB + datos): la instalación lo rechaza antes de escribir nada.
const MINIMO: &str = "El disco es demasiado pequeño (mínimo 1 GB).";

/// «1,5 GB» o «300 MB» (potencias de 1024, como el resto del sistema).
fn tamano(b: u64) -> String {
    let mb = b >> 20;
    if mb >= 1024 {
        let d = (mb * 10 / 1024) as u32;
        format!("{},{} GB", d / 10, d % 10)
    } else {
        format!("{mb} MB")
    }
}

/// Qué es una partición, en una palabra para el usuario.
fn etiqueta(k: FsKind) -> &'static str {
    match k {
        FsKind::Ntfs => "Windows (NTFS)",
        FsKind::Ext => "Linux (ext)",
        FsKind::BitLocker => "BitLocker",
        FsKind::NightFury => "NightFury",
        FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32 => "FAT",
        FsKind::ExFat => "exFAT",
        FsKind::Iso9660 | FsKind::Udf => "Disco óptico",
        FsKind::Unknown => "Formato desconocido",
    }
}

/// «Windows (NTFS) 200 GB · BitLocker · Linux (ext) 40 GB», o «Vacío» sin particiones.
fn contenido(d: &PenDisco) -> String {
    if d.partes.is_empty() {
        return String::from("Vacío");
    }
    let v: Vec<String> = d.partes.iter().map(|&(k, b)| format!("{} {}", etiqueta(k), tamano(b))).collect();
    v.join(" · ")
}

/// Estado de un disco para la lista: `(texto, es_error)`.
fn estado(d: &PenDisco) -> Option<(String, bool)> {
    let a = &d.analisis;
    if a.nightfury {
        Some((String::from("NightFury instalado"), false))
    } else {
        a.bloqueo_disco.map(|m| (format!("No compatible: {m}"), true))
    }
}

fn con_bitlocker(d: &PenDisco) -> bool {
    d.partes.iter().any(|p| p.0 == FsKind::BitLocker)
}

/// Lo que se puede hacer con el disco: disco entero, cada sitio y quitar NightFury.
fn opciones(d: &PenDisco) -> Vec<Opcion> {
    let a = &d.analisis;
    let mut v = Vec::new();
    let bloqueo = if con_bitlocker(d) {
        Some(BITLOCKER)
    } else if d.bytes < MIN_ENTERO {
        Some(MINIMO)
    } else {
        None
    };
    v.push(Opcion {
        accion: Accion::Entero,
        texto: String::from("Disco entero (se borra todo)"),
        nota: String::from("NightFury será el único sistema de este disco"),
        bloqueo,
        borra: true,
    });
    if a.bloqueo_disco.is_none() {
        for (k, s) in a.sitios.iter().enumerate().take(MAX_SITIOS) {
            let tam = tamano(s.sectores * d.sector as u64);
            let (texto, nota, borra) = match &s.lugar {
                Lugar::Hueco => (format!("Espacio libre · {tam}"), String::from("No se borra nada de lo que hay"), false),
                Lugar::Particion { numero, fs, etiqueta } => {
                    // El nombre ya suele decir el sistema («Windows»): aquí solo el formato.
                    let fmt = match fs {
                        FsKind::Ntfs => "NTFS",
                        FsKind::Ext => "ext",
                        _ => self::etiqueta(*fs),
                    };
                    let nombre = if etiqueta.is_empty() { String::new() } else { format!("{etiqueta} · ") };
                    (format!("Partición {numero} · {nombre}{fmt} · {tam}"), String::from("Se borra todo lo que tiene"), true)
                }
            };
            let bloqueo = s.bloqueo.or(a.nightfury.then_some("NightFury ya está instalado en este disco"));
            v.push(Opcion { accion: Accion::Sitio(k), texto, nota, bloqueo, borra });
        }
    }
    if a.nightfury {
        v.push(Opcion { accion: Accion::Quitar, texto: String::from("Quitar NightFury de este disco"), nota: String::from("Windows y Linux no se tocan"), bloqueo: None, borra: true });
    }
    v
}

fn error_instalar(e: FsError, a: Accion) -> &'static str {
    match e {
        FsError::NotFound if a == Accion::Quitar => "NightFury ya no está en este disco.",
        FsError::Exists => "NightFury ya está instalado en este disco.",
        FsError::Denied if matches!(a, Accion::Sitio(_)) => "Ese sitio ya no se puede usar.",
        FsError::NoSpace => MINIMO,
        FsError::Denied => "El disco es de solo lectura (puede tener un interruptor o estar protegido).",
        FsError::Disk => "No se ha podido escribir en el disco.",
        FsError::NotFound => "No se encuentra el pendrive de arranque: no lo quites durante la instalación.",
        _ => "No se ha podido instalar NightFury.",
    }
}

impl Login {
    /// Menú de arranque desde pendrive: nada más aparecer, antes del disco, el asistente y el inicio de sesión.
    pub fn start_pen(&mut self) {
        self.enter(Mode::Pen);
        self.pen = PenUi::default();
        self.focus = Item::Card(0);
    }

    fn pen_estado(&self) -> PenEstado {
        self.pen.est.unwrap_or(PenEstado::Buscando)
    }

    /// ¿Se puede elegir la tarjeta `i`? Probar siempre; las otras, con el pendrive encontrado.
    fn pen_activa(&self, i: usize) -> bool {
        i == 0 || matches!(self.pen_estado(), PenEstado::Listo { .. })
    }

    /// Consulta al núcleo dónde está el pendrive (cada tick) y repinta si cambió.
    pub(super) fn pen_sondear(&mut self, sys: &mut dyn Platform) {
        let e = sys.pen_estado();
        if self.pen.est != Some(e) {
            self.pen.est = Some(e);
            self.dmg_all();
        }
    }

    pub(super) fn pen_order(&self) -> Vec<Item> {
        match self.pen.paso {
            Paso::Menu => alloc::vec![Item::Card(0), Item::Card(1), Item::Card(2)],
            Paso::Confirmar => alloc::vec![Item::Ok, Item::Back],
            Paso::Instalar => (0..self.pen.discos.len()).map(Item::Card).chain([Item::Back]).collect(),
            Paso::Donde => {
                let v = self.pen.discos.get(self.pen.sel).map(opciones).unwrap_or_default();
                v.iter().enumerate().filter(|(_, o)| o.bloqueo.is_none()).map(|(i, _)| Item::Card(i)).chain([Item::Back]).collect()
            }
            Paso::Borrar if self.pide_borrar() => alloc::vec![Item::Code, Item::Ok, Item::Back],
            Paso::Borrar => alloc::vec![Item::Ok, Item::Back],
            Paso::Instalado | Paso::Quitado => alloc::vec![Item::Ok],
            Paso::Preparando | Paso::Instalando => Vec::new(),
        }
    }

    /// El disco y la fila elegidos en «¿Dónde?».
    fn elegida(&self) -> Option<(&PenDisco, Opcion)> {
        let d = self.pen.discos.get(self.pen.sel)?;
        Some((d, opciones(d).into_iter().find(|o| o.accion == self.pen.accion)?))
    }

    /// ¿Hay que escribir BORRAR? Solo instalar en un hueco no pierde nada.
    fn pide_borrar(&self) -> bool {
        self.elegida().map_or(true, |(_, o)| o.borra)
    }

    fn pen_texto(&self) -> String {
        match (self.pen.paso, self.pen_estado()) {
            (Paso::Confirmar, PenEstado::Listo { hueco, .. }) => {
                format!("Se creará una partición de {} en el espacio libre del pendrive. Lo que ya hay en el pendrive no se borra.", tamano(hueco))
            }
            (Paso::Preparando, _) => String::from("Un momento. No quites el pendrive."),
            (Paso::Instalar, _) if self.pen.discos.is_empty() => String::from("No hay ningún disco interno."),
            (Paso::Instalar, _) => String::from("Elige el disco donde instalar NightFury."),
            (Paso::Donde, _) => match self.pen.discos.get(self.pen.sel) {
                Some(d) if con_bitlocker(d) => format!("{} · {}\n{BITLOCKER}", d.modelo, tamano(d.bytes)),
                Some(d) => match d.analisis.bloqueo_disco {
                    Some(m) => format!("{} · {}\nNo se puede instalar junto a otros sistemas: {m}.", d.modelo, tamano(d.bytes)),
                    None => format!("{} · {}\nElige dónde instalar NightFury.", d.modelo, tamano(d.bytes)),
                },
                None => String::new(),
            },
            (Paso::Borrar, _) => match self.elegida() {
                Some((d, o)) => {
                    let (modelo, tam) = (&d.modelo, tamano(d.bytes));
                    let sitio = match o.accion {
                        Accion::Sitio(k) => d.analisis.sitios.get(k),
                        _ => None,
                    };
                    match (o.accion, sitio) {
                        (Accion::Quitar, _) => format!("Se borrará NightFury y sus archivos de {modelo} ({tam}). Windows y Linux no se tocan.\nEsto no se puede deshacer."),
                        (_, Some(s)) if s.lugar == Lugar::Hueco => format!("NightFury se instalará en el espacio libre de {modelo} ({}). No se borra nada de lo que hay.", tamano(s.sectores * d.sector as u64)),
                        (_, Some(s)) => format!("Se borrará la partición «{}» ({}) de {modelo}: lo que tiene se pierde.\nEsto no se puede deshacer. Las demás particiones no se tocan.", o.texto, tamano(s.sectores * d.sector as u64)),
                        _ => format!("Se borrará TODO el disco {modelo} ({tam}).\n{}\nEsto no se puede deshacer. NightFury será el único sistema de este disco.", contenido(d)),
                    }
                }
                None => String::new(),
            },
            (Paso::Instalando, _) if self.pen.accion == Accion::Quitar => String::from("Quitando NightFury… No apagues el equipo."),
            (Paso::Instalando, _) => String::from("Instalando NightFury… No apagues el equipo."),
            (Paso::Quitado, _) if self.pen.entrada => String::from("NightFury se ha quitado. Windows y Linux no se han tocado."),
            (Paso::Quitado, _) => String::from(
                "NightFury se ha quitado. Windows y Linux no se han tocado.\nSi el menú de arranque del equipo aún lo muestra, ignóralo: ya no arranca.",
            ),
            (Paso::Instalado, _) if self.pen.entrada => String::from("NightFury está instalado. Quita el pendrive y pulsa Reiniciar."),
            (Paso::Instalado, _) => String::from(
                "NightFury está instalado. Quita el pendrive y pulsa Reiniciar.
Si el equipo no arranca NightFury, elígelo en su menú de arranque (F12, F11, F8 o Esc según el fabricante).",
            ),
            _ => String::new(),
        }
    }

    fn pen_titulo(&self) -> &'static str {
        match self.pen.paso {
            Paso::Menu => "",
            Paso::Confirmar => "Modo Portátil",
            Paso::Preparando => "Preparando el pendrive…",
            Paso::Instalar => "¿En qué disco?",
            Paso::Donde => "¿Dónde?",
            Paso::Borrar if self.pen.accion == Accion::Quitar => "Quitar NightFury",
            Paso::Borrar => "Instalar NightFury",
            Paso::Instalando if self.pen.accion == Accion::Quitar => "Quitando NightFury…",
            Paso::Instalando => "Instalando NightFury…",
            Paso::Quitado => "NightFury quitado",
            Paso::Instalado => "Instalación terminada",
        }
    }

    pub(super) fn layout_pen(&self) -> Layout {
        let mut items: Vec<(Item, Rect)> = Vec::new();
        if self.pen.paso == Paso::Menu {
            let lh = font::line_h(9);
            let cw = ((self.w - 80 - 2 * HUECO) / 3).clamp(160, TARJETA_W);
            let total = 3 * cw + 2 * HUECO;
            let alto = 76 + TARJETA_H + 16 + lh + 4;
            let y0 = ((self.h - alto) / 2).max(16);
            let x0 = (self.w - total) / 2;
            let title = Rect::new(0, y0, self.w, 34);
            for i in 0..3 {
                items.push((Item::Card(i), Rect::new(x0 + i as i32 * (cw + HUECO), y0 + 76, cw, TARJETA_H)));
            }
            items.push((Item::Msg, Rect::new(x0, y0 + 76 + TARJETA_H + 16, total, lh + 4)));
            let bounds = Rect::new(x0, y0, total, alto);
            items.push((Item::Power, self.power_rect()));
            return Layout { items, bounds, card: bounds, title, body_y: y0 + 40 };
        }
        let lh = font::line_h(8);
        let w = CARD_W.min(self.w - 16).max(240);
        let inner = w - 2 * PAD;
        let mut y = PAD;
        let title = Rect::new(PAD, y, inner, 24);
        y += 30;
        let body = y;
        y += wrap(&self.pen_texto(), 8, inner).len() as i32 * lh + 14;
        match self.pen.paso {
            Paso::Confirmar => {
                let bw = (inner - 12) / 2;
                items.push((Item::Ok, Rect::new(PAD, y, bw, BTN_H)));
                items.push((Item::Back, Rect::new(PAD + bw + 12, y, bw, BTN_H)));
                y += BTN_H;
            }
            Paso::Instalar => {
                for i in 0..self.pen.discos.len() {
                    items.push((Item::Card(i), Rect::new(PAD, y, inner, FILA_H)));
                    y += FILA_H + 8;
                }
                if self.pen.aviso.is_some() {
                    items.push((Item::Msg, Rect::new(PAD, y, inner, 2 * lh + 4)));
                    y += 2 * lh + 12;
                }
                items.push((Item::Back, Rect::new(PAD, y, inner, BTN_H)));
                y += BTN_H;
            }
            Paso::Donde => {
                let n = self.pen.discos.get(self.pen.sel).map_or(0, |d| opciones(d).len());
                for i in 0..n {
                    items.push((Item::Card(i), Rect::new(PAD, y, inner, OP_H)));
                    y += OP_H + 8;
                }
                items.push((Item::Back, Rect::new(PAD, y, inner, BTN_H)));
                y += BTN_H;
            }
            Paso::Borrar => {
                if self.pide_borrar() {
                    y += lh + 2;
                    items.push((Item::Code, Rect::new(PAD, y, inner, FIELD_H)));
                    y += FIELD_H + 12;
                }
                let bw = (inner - 12) / 2;
                items.push((Item::Ok, Rect::new(PAD, y, bw, BTN_H)));
                items.push((Item::Back, Rect::new(PAD + bw + 12, y, bw, BTN_H)));
                y += BTN_H;
            }
            Paso::Instalado | Paso::Quitado => {
                items.push((Item::Ok, Rect::new(PAD, y, inner, BTN_H)));
                y += BTN_H;
            }
            _ => {}
        }
        y += PAD;
        let (ox, oy) = ((self.w - w) / 2, ((self.h - y) / 2).max(8));
        for it in items.iter_mut() {
            it.1.x += ox;
            it.1.y += oy;
        }
        let card = Rect::new(ox, oy, w, y);
        let title = Rect::new(title.x + ox, title.y + oy, title.w, title.h);
        items.push((Item::Power, self.power_rect()));
        Layout { items, bounds: card, card, title, body_y: body + oy }
    }

    /// Cambia de pantalla del modo (y repinta todo: cambia el tamaño del contenido).
    fn pen_paso(&mut self, paso: Paso, foco: Item) {
        self.pen.paso = paso;
        self.pen.aviso = None;
        self.focus = foco;
        self.hot = None;
        self.dmg_all();
    }

    /// Empieza a preparar el modo Portátil: la pantalla «Preparando…» sale antes de que el núcleo se ponga a escribir.
    fn pen_preparar(&mut self) -> Out {
        self.pen_paso(Paso::Preparando, Item::Msg);
        self.job = Some((Job::Pen, self.now + JOB_DELAY_MS));
        Out::None
    }

    pub(super) fn pen_crear(&mut self, sys: &mut dyn Platform) -> Out {
        match sys.pen_portatil() {
            Ok(nuevo) => Out::PenPortatil(nuevo),
            Err(e) => {
                self.pen_paso(Paso::Menu, Item::Card(1));
                self.pen.aviso = Some(if e == crate::fs::FsError::NoSpace { SIN_ESPACIO } else { "No se ha podido preparar el pendrive." });
                Out::None
            }
        }
    }

    pub(super) fn pen_activate(&mut self, it: Item, sys: &mut dyn Platform) -> Out {
        match (self.pen.paso, it) {
            (Paso::Menu, Item::Card(0)) => {
                sys.pen_probar();
                Out::PenProbar
            }
            (Paso::Menu, Item::Card(1)) if self.pen_activa(1) => match self.pen_estado() {
                PenEstado::Listo { portatil: false, .. } => {
                    self.pen_paso(Paso::Confirmar, Item::Ok);
                    Out::None
                }
                _ => self.pen_preparar(),
            },
            (Paso::Menu, Item::Card(2)) if self.pen_activa(2) => {
                self.pen.discos = sys.pen_discos().into_iter().take(MAX_DISCOS).collect();
                let foco = if self.pen.discos.is_empty() { Item::Back } else { Item::Card(0) };
                self.pen_paso(Paso::Instalar, foco);
                Out::None
            }
            (Paso::Instalar, Item::Card(i)) if i < self.pen.discos.len() => {
                self.pen.sel = i;
                let foco = opciones(&self.pen.discos[i]).iter().position(|o| o.bloqueo.is_none()).map_or(Item::Back, Item::Card);
                self.pen_paso(Paso::Donde, foco);
                Out::None
            }
            (Paso::Donde, Item::Card(i)) => {
                let Some(o) = self.pen.discos.get(self.pen.sel).and_then(|d| opciones(d).into_iter().nth(i)) else { return Out::None };
                if o.bloqueo.is_some() {
                    return Out::None;
                }
                (self.pen.op, self.pen.accion) = (i, o.accion);
                wipe_str(&mut self.code);
                self.pen_paso(Paso::Borrar, if o.borra { Item::Code } else { Item::Ok });
                Out::None
            }
            (Paso::Donde, Item::Back) => self.pen_volver(),
            // Con BORRAR exacto (mayúsculas), salvo en un hueco: Enter en el campo o el botón.
            (Paso::Borrar, Item::Code | Item::Ok) if !self.pide_borrar() || self.code == BORRAR => {
                wipe_str(&mut self.code);
                self.pen_paso(Paso::Instalando, Item::Msg);
                self.job = Some((Job::Instalar, self.now + JOB_DELAY_MS));
                Out::None
            }
            (Paso::Borrar, Item::Back) => self.pen_volver(),
            (Paso::Instalado, Item::Ok) => Out::Reboot,
            (Paso::Quitado, Item::Ok) => {
                self.pen.discos = sys.pen_discos().into_iter().take(MAX_DISCOS).collect();
                self.pen_paso(Paso::Instalar, if self.pen.discos.is_empty() { Item::Back } else { Item::Card(0) });
                Out::None
            }
            (Paso::Confirmar, Item::Ok) => self.pen_preparar(),
            (Paso::Confirmar, Item::Back) => self.pen_volver(),
            (Paso::Instalar, Item::Back) => self.pen_volver(),
            _ => Out::None,
        }
    }

    /// Esc o «Volver»: de una pantalla del modo al menú.
    pub(super) fn pen_volver(&mut self) -> Out {
        match self.pen.paso {
            Paso::Confirmar => self.pen_paso(Paso::Menu, Item::Card(1)),
            Paso::Instalar => self.pen_paso(Paso::Menu, Item::Card(2)),
            Paso::Donde => self.pen_paso(Paso::Instalar, Item::Card(self.pen.sel)),
            Paso::Borrar => {
                wipe_str(&mut self.code);
                self.pen_paso(Paso::Donde, Item::Card(self.pen.op));
            }
            _ => {}
        }
        Out::None
    }

    /// Trabajo aplazado de «Instalando…»: el núcleo instala (y crea la entrada de arranque) o quita NightFury.
    pub(super) fn pen_instalar_hacer(&mut self, sys: &mut dyn Platform) -> Out {
        let r = match self.pen.accion {
            Accion::Quitar => sys.pen_quitar(self.pen.sel),
            Accion::Entero => sys.pen_instalar(self.pen.sel, Donde::DiscoEntero),
            Accion::Sitio(k) => sys.pen_instalar(self.pen.sel, Donde::Sitio(k)),
        };
        match r {
            Ok(entrada) => {
                self.pen.entrada = entrada;
                self.pen_paso(if self.pen.accion == Accion::Quitar { Paso::Quitado } else { Paso::Instalado }, Item::Ok);
            }
            Err(e) => {
                // El disco pudo quedar a medias: se vuelve a leer lo que tiene.
                self.pen.discos = sys.pen_discos().into_iter().take(MAX_DISCOS).collect();
                self.pen_paso(Paso::Instalar, if self.pen.discos.is_empty() { Item::Back } else { Item::Card(0) });
                self.pen.aviso = Some(error_instalar(e, self.pen.accion));
            }
        }
        Out::None
    }

    pub(super) fn paint_pen(&self, s: &mut Surface, l: &Layout, accent: u32) {
        if self.pen.paso != Paso::Menu {
            return self.paint_pen_tarjeta(s, l, accent);
        }
        let sombra = |s: &mut Surface, r: Rect, t: &str, size: u32, bold: bool, c: u32| {
            text_in(s, Rect::new(r.x + 1, r.y + 1, r.w, r.h), t, size, bold, 0x000000, Align::Center);
            text_in(s, r, t, size, bold, c, Align::Center);
        };
        sombra(s, l.title, &format!("{} {}", crate::NAME, crate::VERSION), 22, true, 0xFFFFFF);
        sombra(s, Rect::new(0, l.body_y, self.w, font::line_h(10)), "Arrancado desde un pendrive", 10, false, 0xE0E0E0);
        for &(it, r) in &l.items {
            if !r.intersects(s.clip()) {
                continue;
            }
            match it {
                Item::Card(i) => self.paint_tarjeta(s, r, i, accent),
                Item::Msg => {
                    if let Some(t) = self.pen.aviso {
                        sombra(s, r, t, 8, false, RED);
                    }
                }
                _ => {}
            }
        }
    }

    fn paint_tarjeta(&self, s: &mut Surface, r: Rect, i: usize, accent: u32) {
        let activa = self.pen_activa(i);
        let luz = activa && (self.focus == Item::Card(i) || self.hot == Some(Item::Card(i)));
        s.fill_rect(r, if luz { 0x2B2B2B } else { CARD_BG });
        s.frame(r, if luz { accent } else { 0x383838 });
        let (icono, nombre, texto) = TARJETAS[i];
        let (fg, gris) = if activa { (0xFFFFFF, GREY) } else { (DESACTIVADA, DESACTIVADA) };
        icons::draw(s, icono, r.x + (r.w - 72) / 2, r.y + 28, 72, fg);
        text_in(s, Rect::new(r.x + 8, r.y + 118, r.w - 16, font::line_h(14) + 4), nombre, 14, true, fg, Align::Center);
        let texto = match (activa, self.pen_estado()) {
            (true, _) => texto,
            (false, PenEstado::Buscando) => "Buscando el pendrive…",
            (false, _) => "No se encuentra el pendrive de arranque",
        };
        let lh = font::line_h(9);
        for (k, linea) in wrap(texto, 9, r.w - 32).iter().take(4).enumerate() {
            text_in(s, Rect::new(r.x + 16, r.y + 154 + k as i32 * lh, r.w - 32, lh), linea, 9, false, gris, Align::Center);
        }
    }

    /// Un disco de la lista de instalación: modelo y tamaño, y debajo lo que tiene.
    fn paint_disco(&self, s: &mut Surface, r: Rect, i: usize, accent: u32) {
        let Some(d) = self.pen.discos.get(i) else { return };
        let luz = self.focus == Item::Card(i) || self.hot == Some(Item::Card(i));
        s.fill_rect(r, if luz { 0x2B2B2B } else { 0x181818 });
        s.frame(r, if luz { accent } else { 0x383838 });
        let (l9, l8) = (font::line_h(9), font::line_h(8));
        text_in(s, Rect::new(r.x + 10, r.y + 6, r.w - 20, l9), &format!("{} · {}", d.modelo, tamano(d.bytes)), 9, true, 0xFFFFFF, Align::Left);
        for (k, linea) in wrap(&contenido(d), 8, r.w - 20).iter().take(2).enumerate() {
            text_in(s, Rect::new(r.x + 10, r.y + 8 + l9 + k as i32 * l8, r.w - 20, l8), linea, 8, false, GREY, Align::Left);
        }
        if let Some((t, malo)) = estado(d) {
            text_in(s, Rect::new(r.x + 10, r.y + 10 + l9 + 2 * l8, r.w - 20, l8), &t, 8, true, if malo { RED } else { accent }, Align::Left);
        }
    }

    /// Una fila de «¿Dónde?»: en gris y sin luz si no se puede elegir.
    fn paint_opcion(&self, s: &mut Surface, r: Rect, i: usize, accent: u32) {
        let Some(o) = self.pen.discos.get(self.pen.sel).and_then(|d| opciones(d).into_iter().nth(i)) else { return };
        let luz = o.bloqueo.is_none() && (self.focus == Item::Card(i) || self.hot == Some(Item::Card(i)));
        s.fill_rect(r, if luz { 0x2B2B2B } else { 0x181818 });
        s.frame(r, if luz { accent } else { 0x383838 });
        let (fg, gris) = if o.bloqueo.is_none() { (0xFFFFFF, GREY) } else { (DESACTIVADA, DESACTIVADA) };
        let (l9, l8) = (font::line_h(9), font::line_h(8));
        text_in(s, Rect::new(r.x + 10, r.y + 6, r.w - 20, l9), &o.texto, 9, true, fg, Align::Left);
        text_in(s, Rect::new(r.x + 10, r.y + 8 + l9, r.w - 20, l8), o.bloqueo.unwrap_or(&o.nota), 8, false, gris, Align::Left);
    }

    /// Confirmar, Preparando e Instalar: una tarjeta con título, texto y botones, como el asistente.
    fn paint_pen_tarjeta(&self, s: &mut Surface, l: &Layout, accent: u32) {
        let c = l.card;
        s.fill_rect(c, CARD_BG);
        s.frame(c, 0x383838);
        text_in(s, l.title, self.pen_titulo(), 14, true, 0xFFFFFF, Align::Left);
        let lh = font::line_h(8);
        for (k, linea) in wrap(&self.pen_texto(), 8, c.w - 2 * PAD).iter().enumerate() {
            text_in(s, Rect::new(c.x + PAD, l.body_y + k as i32 * lh, c.w - 2 * PAD, lh), linea, 8, false, GREY, Align::Left);
        }
        if self.pen.paso == Paso::Borrar && self.pide_borrar() {
            if let Some(&(_, f)) = l.items.iter().find(|i| i.0 == Item::Code) {
                text_in(s, Rect::new(f.x, f.y - lh - 2, f.w, lh), "Escribe BORRAR para continuar", 8, false, GREY, Align::Left);
            }
        }
        for &(it, r) in &l.items {
            match (it, self.pen.paso) {
                (Item::Code, _) => self.field(s, r, it, accent),
                (Item::Card(i), Paso::Donde) => self.paint_opcion(s, r, i, accent),
                (Item::Card(i), _) => self.paint_disco(s, r, i, accent),
                (Item::Msg, _) => {
                    if let Some(t) = self.pen.aviso {
                        for (k, linea) in wrap(t, 8, r.w).iter().take(2).enumerate() {
                            text_in(s, Rect::new(r.x, r.y + k as i32 * lh, r.w, lh), linea, 8, false, RED, Align::Left);
                        }
                    }
                }
                (Item::Ok, Paso::Borrar) if self.pide_borrar() && self.code != BORRAR => {
                    s.fill_rect(r, 0x2A2A2A);
                    text_in(s, r, if self.pen.accion == Accion::Quitar { "Quitar" } else { "Instalar" }, 9, false, DESACTIVADA, Align::Center);
                }
                (Item::Ok, Paso::Borrar) => self.button(s, r, it, if self.pen.accion == Accion::Quitar { "Quitar" } else { "Instalar" }, accent, true),
                (Item::Ok, Paso::Instalado) => self.button(s, r, it, "Reiniciar", accent, true),
                (Item::Ok, Paso::Quitado) => self.button(s, r, it, "Aceptar", accent, true),
                (Item::Ok, _) => self.button(s, r, it, "Crear", accent, true),
                (Item::Back, p) => self.button(s, r, it, "Volver", accent, matches!(p, Paso::Instalar | Paso::Donde)),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::Files;
    use crate::keys::Pool;
    use crate::platform::{DateTime, MemInfo, TestPlatform};
    use crate::vol::instalar::{Analisis, Sitio};

    /// Plataforma de pruebas con cuatro discos internos y una instalación que da `res`.
    struct Falso {
        t: TestPlatform,
        res: Result<bool, FsError>,
        instalado: Option<(usize, Donde)>,
        quitado: Option<usize>,
    }

    const GB: u64 = 1 << 30;

    fn sitio(inicio_lba: u64, gb: u64, lugar: Lugar, bloqueo: Option<&'static str>) -> Sitio {
        Sitio { inicio_lba, sectores: gb * GB / 512, lugar, bloqueo }
    }

    fn disco(modelo: &str, gb: u64, partes: Vec<(FsKind, u64)>, analisis: Analisis) -> PenDisco {
        PenDisco { modelo: String::from(modelo), bytes: gb * GB, partes, sector: 512, analisis }
    }

    fn analisis(bloqueo_disco: Option<&'static str>, sitios: Vec<Sitio>, nightfury: bool) -> Analisis {
        Analisis { bloqueo_disco, esp: None, sitios, nightfury }
    }

    impl Platform for Falso {
        fn now(&mut self) -> DateTime {
            self.t.now()
        }
        fn uptime_ms(&self) -> u64 {
            0
        }
        fn mem(&self) -> MemInfo {
            self.t.mem()
        }
        fn cpu(&self) -> String {
            String::new()
        }
        fn screen(&self) -> (u32, u32) {
            (800, 600)
        }
        fn reboot(&mut self) {}
        fn shutdown(&mut self) {}
        fn files(&mut self) -> &mut Files {
            self.t.files()
        }
        fn entropy(&mut self) -> u64 {
            self.t.entropy()
        }
        fn pool(&mut self) -> &mut Pool {
            self.t.pool()
        }
        fn pen_estado(&mut self) -> PenEstado {
            PenEstado::Listo { portatil: false, hueco: 0 }
        }
        fn pen_discos(&mut self) -> Vec<PenDisco> {
            alloc::vec![
                // 0: vacío
                disco("Disco A", 2, Vec::new(), analisis(None, Vec::new(), false)),
                // 1: un hueco, una partición de datos y otra con Windows (bloqueada)
                disco(
                    "Disco B",
                    500,
                    alloc::vec![(FsKind::Ntfs, 200 * GB), (FsKind::Ntfs, 100 * GB)],
                    analisis(
                        None,
                        alloc::vec![
                            sitio(2048, 120, Lugar::Hueco, None),
                            sitio(1 << 30, 200, Lugar::Particion { numero: 4, fs: FsKind::Ntfs, etiqueta: String::from("Datos") }, None),
                            sitio(2 << 30, 100, Lugar::Particion { numero: 2, fs: FsKind::Ntfs, etiqueta: String::from("Windows") }, Some("Contiene Windows")),
                        ],
                        false,
                    ),
                ),
                // 2: BitLocker
                disco("Disco C", 500, alloc::vec![(FsKind::BitLocker, 400 * GB)], analisis(Some("El disco tiene una partición con BitLocker"), Vec::new(), false)),
                // 3: ya lleva NightFury
                disco("Disco D", 100, alloc::vec![(FsKind::NightFury, 90 * GB)], analisis(None, Vec::new(), true)),
                // 4: MBR (no admite instalar junto, pero sí disco entero)
                disco("Disco E", 100, alloc::vec![(FsKind::Ntfs, 90 * GB)], analisis(Some("El disco usa MBR: no es compatible"), Vec::new(), false)),
            ]
        }
        fn pen_instalar(&mut self, i: usize, donde: Donde) -> Result<bool, FsError> {
            self.instalado = Some((i, donde));
            self.res
        }
        fn pen_quitar(&mut self, i: usize) -> Result<bool, FsError> {
            self.quitado = Some(i);
            self.res
        }
    }

    fn nuevo(res: Result<bool, FsError>) -> (Login, Falso) {
        let mut sys = Falso { t: TestPlatform::default(), res, instalado: None, quitado: None };
        let mut l = Login::new(800, 600);
        l.start_pen();
        l.pen_sondear(&mut sys);
        assert_eq!(l.pen_activate(Item::Card(2), &mut sys), Out::None);
        assert_eq!(l.pen.paso, Paso::Instalar);
        (l, sys)
    }

    /// Elige el disco `d` y llega a «¿Dónde?».
    fn donde(res: Result<bool, FsError>, d: usize) -> (Login, Falso) {
        let (mut l, mut sys) = nuevo(res);
        l.pen_activate(Item::Card(d), &mut sys);
        assert_eq!(l.pen.paso, Paso::Donde);
        (l, sys)
    }

    /// Confirma lo que haya en la pantalla de confirmación y deja pasar el tick del trabajo.
    fn confirmar(l: &mut Login, sys: &mut Falso, code: &str) {
        l.code = String::from(code);
        l.pen_activate(Item::Ok, sys);
        l.tick(l.now + 10_000, sys);
    }

    #[test]
    fn contenido_y_estado_de_los_discos() {
        let (l, _) = nuevo(Ok(true));
        assert_eq!(contenido(&l.pen.discos[0]), "Vacío");
        assert_eq!(contenido(&l.pen.discos[1]), "Windows (NTFS) 200,0 GB · Windows (NTFS) 100,0 GB");
        let e: Vec<_> = l.pen.discos.iter().map(estado).collect();
        assert_eq!(e[0], None);
        assert_eq!(e[1], None);
        assert_eq!(e[2], Some((String::from("No compatible: El disco tiene una partición con BitLocker"), true)));
        assert_eq!(e[3], Some((String::from("NightFury instalado"), false)));
        assert_eq!(e[4].as_ref().unwrap().0, "No compatible: El disco usa MBR: no es compatible");
    }

    #[test]
    fn disco_entero_solo_con_borrar_exacto() {
        let (mut l, mut sys) = donde(Ok(true), 1);
        l.pen_activate(Item::Card(0), &mut sys);
        assert_eq!(l.pen.paso, Paso::Borrar);
        assert!(l.pen_texto().contains("TODO el disco Disco B") && l.pen_texto().contains("Windows (NTFS)"));
        for malo in ["", "BORR", "borrar", "BORRAR ", "BORRARX"] {
            l.code = String::from(malo);
            l.pen_activate(Item::Ok, &mut sys);
            l.pen_activate(Item::Code, &mut sys);
            assert_eq!(l.pen.paso, Paso::Borrar, "{malo:?}");
            assert!(l.job.is_none() && sys.instalado.is_none());
        }
        l.code = String::from("BORRAR");
        l.pen_activate(Item::Ok, &mut sys);
        assert_eq!(l.pen.paso, Paso::Instalando);
        assert!(l.code.is_empty());
        assert_eq!(sys.instalado, None, "instala en el tick siguiente, con la pantalla ya pintada");
        assert_eq!(l.tick(10_000, &mut sys), Out::None);
        assert_eq!((l.pen.paso, sys.instalado), (Paso::Instalado, Some((1, Donde::DiscoEntero))));
        assert!(l.pen_texto().starts_with("NightFury está instalado. Quita el pendrive y pulsa Reiniciar."));
        assert!(!l.pen_texto().contains("F12"));
        assert_eq!(l.pen_activate(Item::Ok, &mut sys), Out::Reboot);
    }

    #[test]
    fn sitios_bloqueados_no_se_eligen() {
        let (mut l, mut sys) = donde(Ok(true), 1);
        let v = opciones(&l.pen.discos[1]);
        assert_eq!(v.len(), 4);
        assert_eq!(v[1].texto, "Espacio libre · 120,0 GB");
        assert_eq!(v[2].texto, "Partición 4 · Datos · NTFS · 200,0 GB");
        assert_eq!(v[3].bloqueo, Some("Contiene Windows"));
        assert!(!l.pen_order().contains(&Item::Card(3)));
        l.pen_activate(Item::Card(3), &mut sys);
        assert_eq!(l.pen.paso, Paso::Donde, "clic en una fila en gris: nada");
        // Disco pequeño: sin disco entero.
        let (mut l, mut sys) = donde(Ok(true), 0);
        l.pen.discos[0].bytes = 500 << 20;
        assert_eq!(opciones(&l.pen.discos[0])[0].bloqueo, Some(MINIMO));
        l.pen_activate(Item::Card(0), &mut sys);
        assert_eq!(l.pen.paso, Paso::Donde);
    }

    #[test]
    fn hueco_sin_borrar_y_particion_con_borrar() {
        let (mut l, mut sys) = donde(Ok(true), 1);
        l.pen_activate(Item::Card(1), &mut sys);
        assert_eq!(l.pen.paso, Paso::Borrar);
        assert!(!l.pide_borrar() && l.pen_order() == [Item::Ok, Item::Back]);
        assert!(l.pen_texto().contains("No se borra nada de lo que hay"));
        l.pen_activate(Item::Ok, &mut sys);
        l.tick(10_000, &mut sys);
        assert_eq!((l.pen.paso, sys.instalado), (Paso::Instalado, Some((1, Donde::Sitio(0)))));

        let (mut l, mut sys) = donde(Ok(true), 1);
        l.pen_activate(Item::Card(2), &mut sys);
        assert!(l.pide_borrar() && l.pen_order()[0] == Item::Code);
        assert!(l.pen_texto().contains("lo que tiene se pierde") && l.pen_texto().contains("Datos"));
        for malo in ["", "borrar", "BORRA"] {
            confirmar(&mut l, &mut sys, malo);
            assert_eq!((l.pen.paso, sys.instalado), (Paso::Borrar, None), "{malo:?}");
        }
        confirmar(&mut l, &mut sys, "BORRAR");
        assert_eq!((l.pen.paso, sys.instalado), (Paso::Instalado, Some((1, Donde::Sitio(1)))));
    }

    #[test]
    fn bitlocker_sin_opciones_y_mbr_solo_entero() {
        let (mut l, mut sys) = donde(Ok(true), 2);
        assert!(opciones(&l.pen.discos[2]).iter().all(|o| o.bloqueo.is_some()));
        assert_eq!(l.pen_order(), [Item::Back]);
        assert!(l.pen_texto().contains("BitLocker"));
        l.pen_activate(Item::Card(0), &mut sys);
        assert_eq!(l.pen.paso, Paso::Donde);
        assert_eq!(l.pen_volver(), Out::None);
        assert_eq!(l.pen.paso, Paso::Instalar);

        let (mut l, mut sys) = donde(Ok(true), 4);
        let v = opciones(&l.pen.discos[4]);
        assert_eq!((v.len(), v[0].bloqueo), (1, None));
        assert!(l.pen_texto().contains("MBR"));
        l.pen_activate(Item::Card(0), &mut sys);
        assert_eq!(l.pen.paso, Paso::Borrar);
    }

    #[test]
    fn quitar_pide_borrar() {
        let (mut l, mut sys) = donde(Ok(true), 3);
        let v = opciones(&l.pen.discos[3]);
        assert_eq!(v.last().map(|o| o.accion), Some(Accion::Quitar));
        l.pen_activate(Item::Card(v.len() - 1), &mut sys);
        assert!(l.pide_borrar() && l.pen_titulo() == "Quitar NightFury");
        assert!(l.pen_texto().contains("Windows y Linux no se tocan"));
        confirmar(&mut l, &mut sys, "borrar");
        assert_eq!((l.pen.paso, sys.quitado), (Paso::Borrar, None));
        confirmar(&mut l, &mut sys, "BORRAR");
        assert_eq!((l.pen.paso, sys.quitado, sys.instalado), (Paso::Quitado, Some(3), None));
        assert_eq!(l.pen_texto(), "NightFury se ha quitado. Windows y Linux no se han tocado.");
        assert_eq!(l.pen_activate(Item::Ok, &mut sys), Out::None);
        assert_eq!(l.pen.paso, Paso::Instalar);
    }

    #[test]
    fn sin_entrada_de_arranque_y_con_errores() {
        let (mut l, mut sys) = donde(Ok(false), 1);
        l.pen_activate(Item::Card(0), &mut sys);
        confirmar(&mut l, &mut sys, "BORRAR");
        assert!(l.pen_texto().contains("F12, F11, F8 o Esc"));

        let (mut l, mut sys) = donde(Err(FsError::NoSpace), 1);
        l.pen_activate(Item::Card(0), &mut sys);
        confirmar(&mut l, &mut sys, "BORRAR");
        assert_eq!((l.pen.paso, l.pen.aviso), (Paso::Instalar, Some(MINIMO)));
        // Esc desde la confirmación vuelve a «¿Dónde?» y borra lo escrito; otro Esc, a la lista.
        l.pen_activate(Item::Card(1), &mut sys);
        l.pen_activate(Item::Card(0), &mut sys);
        l.code = String::from("BOR");
        l.pen_volver();
        assert_eq!(l.pen.paso, Paso::Donde);
        assert!(l.code.is_empty());
        l.pen_volver();
        assert_eq!(l.pen.paso, Paso::Instalar);
    }
}
