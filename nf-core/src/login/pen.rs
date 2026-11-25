// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Pantalla «Arranque desde pendrive» (v0.12.6): Probar, Portátil e Instalar. Sale antes que ninguna
//! otra pantalla de cuentas cuando el sistema arrancó desde un pendrive (`Platform::pen_menu`) y espera
//! sin límite. Es parte de `Login` (se ve como el resto de pantallas de cuentas y comparte su foco, su
//! ratón y su teclado): aquí solo está lo propio de este modo.
use super::*;
use crate::fs::FsError;
use crate::platform::{PenDisco, PenEstado};
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
    /// Instalar: aviso de que se borra el disco elegido y campo «BORRAR».
    Borrar,
    /// Instalando (sin porcentaje).
    Instalando,
    /// Terminado: quitar el pendrive y reiniciar.
    Instalado,
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
    /// Instalado: ¿quedó creada la entrada de arranque UEFI?
    pub entrada: bool,
}

const TARJETAS: [(Icon, &str, &str); 3] = [
    (Icon::Equipo, "Probar", "Usa NightFury sin instalar nada. Al apagar se pierde todo."),
    (Icon::Usb, "Portátil", "Guarda tus cuentas y archivos en este pendrive."),
    (Icon::Drive, "Instalar", "Instala NightFury en un disco de este equipo."),
];
/// Alto de cada disco de la lista: nombre y hasta dos líneas de contenido.
const FILA_H: i32 = 62;
const TARJETA_W: i32 = 280;
const TARJETA_H: i32 = 250;
const HUECO: i32 = 24;
const SIN_ESPACIO: &str = "Este pendrive no tiene espacio libre. Graba la imagen en un pendrive de al menos 1 GB.";
const DESACTIVADA: u32 = 0x707070;
/// La palabra que hay que escribir para confirmar que se borra el disco.
const BORRAR: &str = "BORRAR";
/// Discos que caben en la lista (shortcut: el resto no se ofrece; ningún equipo normal tiene tantos).
const MAX_DISCOS: usize = 6;
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

fn error_instalar(e: FsError) -> &'static str {
    match e {
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
            Paso::Borrar => alloc::vec![Item::Code, Item::Ok, Item::Back],
            Paso::Instalado => alloc::vec![Item::Ok],
            Paso::Preparando | Paso::Instalando => Vec::new(),
        }
    }

    fn pen_texto(&self) -> String {
        match (self.pen.paso, self.pen_estado()) {
            (Paso::Confirmar, PenEstado::Listo { hueco, .. }) => {
                format!("Se creará una partición de {} en el espacio libre del pendrive. Lo que ya hay en el pendrive no se borra.", tamano(hueco))
            }
            (Paso::Preparando, _) => String::from("Un momento. No quites el pendrive."),
            (Paso::Instalar, _) if self.pen.discos.is_empty() => String::from("No hay ningún disco interno."),
            (Paso::Instalar, _) => String::from("Elige el disco donde instalar. Se borrará por completo."),
            (Paso::Borrar, _) => match self.pen.discos.get(self.pen.sel) {
                Some(d) => format!(
                    "Se borrará TODO el disco {} ({}).
{}
Esto no se puede deshacer. NightFury será el único sistema de este disco.",
                    d.modelo,
                    tamano(d.bytes),
                    contenido(d)
                ),
                None => String::new(),
            },
            (Paso::Instalando, _) => String::from("Instalando NightFury… No apagues el equipo."),
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
            Paso::Instalar => "¿Dónde instalar NightFury?",
            Paso::Borrar => "Instalar NightFury",
            Paso::Instalando => "Instalando NightFury…",
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
            Paso::Borrar => {
                y += lh + 2;
                items.push((Item::Code, Rect::new(PAD, y, inner, FIELD_H)));
                y += FIELD_H + 12;
                let bw = (inner - 12) / 2;
                items.push((Item::Ok, Rect::new(PAD, y, bw, BTN_H)));
                items.push((Item::Back, Rect::new(PAD + bw + 12, y, bw, BTN_H)));
                y += BTN_H;
            }
            Paso::Instalado => {
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
                wipe_str(&mut self.code);
                self.pen_paso(Paso::Borrar, Item::Code);
                Out::None
            }
            // Solo con BORRAR exacto (mayúsculas): Enter en el campo o el botón.
            (Paso::Borrar, Item::Code | Item::Ok) if self.code == BORRAR => {
                wipe_str(&mut self.code);
                self.pen_paso(Paso::Instalando, Item::Msg);
                self.job = Some((Job::Instalar, self.now + JOB_DELAY_MS));
                Out::None
            }
            (Paso::Borrar, Item::Back) => self.pen_volver(),
            (Paso::Instalado, Item::Ok) => Out::Reboot,
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
            Paso::Borrar => {
                wipe_str(&mut self.code);
                self.pen_paso(Paso::Instalar, Item::Card(self.pen.sel));
            }
            _ => {}
        }
        Out::None
    }

    /// Trabajo aplazado de «Instalando…»: el núcleo escribe el disco entero y crea la entrada de arranque.
    pub(super) fn pen_instalar_hacer(&mut self, sys: &mut dyn Platform) -> Out {
        match sys.pen_instalar(self.pen.sel) {
            Ok(entrada) => {
                self.pen.entrada = entrada;
                self.pen_paso(Paso::Instalado, Item::Ok);
            }
            Err(e) => {
                // El disco pudo quedar a medias: se vuelve a leer lo que tiene.
                self.pen.discos = sys.pen_discos().into_iter().take(MAX_DISCOS).collect();
                self.pen_paso(Paso::Instalar, if self.pen.discos.is_empty() { Item::Back } else { Item::Card(0) });
                self.pen.aviso = Some(error_instalar(e));
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
        if self.pen.paso == Paso::Borrar {
            if let Some(&(_, f)) = l.items.iter().find(|i| i.0 == Item::Code) {
                text_in(s, Rect::new(f.x, f.y - lh - 2, f.w, lh), "Escribe BORRAR para continuar", 8, false, GREY, Align::Left);
            }
        }
        for &(it, r) in &l.items {
            match (it, self.pen.paso) {
                (Item::Code, _) => self.field(s, r, it, accent),
                (Item::Card(i), _) => self.paint_disco(s, r, i, accent),
                (Item::Msg, _) => {
                    if let Some(t) = self.pen.aviso {
                        for (k, linea) in wrap(t, 8, r.w).iter().take(2).enumerate() {
                            text_in(s, Rect::new(r.x, r.y + k as i32 * lh, r.w, lh), linea, 8, false, RED, Align::Left);
                        }
                    }
                }
                (Item::Ok, Paso::Borrar) if self.code != BORRAR => {
                    s.fill_rect(r, 0x2A2A2A);
                    text_in(s, r, "Instalar", 9, false, DESACTIVADA, Align::Center);
                }
                (Item::Ok, Paso::Borrar) => self.button(s, r, it, "Instalar", accent, true),
                (Item::Ok, Paso::Instalado) => self.button(s, r, it, "Reiniciar", accent, true),
                (Item::Ok, _) => self.button(s, r, it, "Crear", accent, true),
                (Item::Back, p) => self.button(s, r, it, "Volver", accent, p == Paso::Instalar),
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

    /// Plataforma de pruebas con dos discos internos y una instalación que da `res`.
    struct Falso {
        t: TestPlatform,
        res: Result<bool, FsError>,
        instalado: Option<usize>,
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
                PenDisco { modelo: String::from("Disco A"), bytes: 2 << 30, partes: Vec::new() },
                PenDisco { modelo: String::from("Disco B"), bytes: 500 << 30, partes: alloc::vec![(FsKind::Ntfs, 200 << 30), (FsKind::BitLocker, 100 << 30)] },
            ]
        }
        fn pen_instalar(&mut self, i: usize) -> Result<bool, FsError> {
            self.instalado = Some(i);
            self.res
        }
    }

    fn nuevo(res: Result<bool, FsError>) -> (Login, Falso) {
        let mut sys = Falso { t: TestPlatform::default(), res, instalado: None };
        let mut l = Login::new(800, 600);
        l.start_pen();
        l.pen_sondear(&mut sys);
        assert_eq!(l.pen_activate(Item::Card(2), &mut sys), Out::None);
        assert_eq!(l.pen.paso, Paso::Instalar);
        (l, sys)
    }

    #[test]
    fn contenido_de_los_discos() {
        let (l, _) = nuevo(Ok(true));
        assert_eq!(contenido(&l.pen.discos[0]), "Vacío");
        assert_eq!(contenido(&l.pen.discos[1]), "Windows (NTFS) 200,0 GB · BitLocker 100,0 GB");
    }

    #[test]
    fn solo_borrar_exacto_instala() {
        let (mut l, mut sys) = nuevo(Ok(true));
        l.pen_activate(Item::Card(1), &mut sys);
        assert_eq!(l.pen.paso, Paso::Borrar);
        assert!(l.pen_texto().contains("Disco B") && l.pen_texto().contains("BitLocker"));
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
        assert_eq!((l.pen.paso, sys.instalado), (Paso::Instalado, Some(1)));
        assert!(l.pen_texto().starts_with("NightFury está instalado. Quita el pendrive y pulsa Reiniciar."));
        assert!(!l.pen_texto().contains("F12"));
        assert_eq!(l.pen_activate(Item::Ok, &mut sys), Out::Reboot);
    }

    #[test]
    fn sin_entrada_de_arranque_y_con_errores() {
        let (mut l, mut sys) = nuevo(Ok(false));
        l.pen_activate(Item::Card(0), &mut sys);
        l.code = String::from("BORRAR");
        l.pen_activate(Item::Ok, &mut sys);
        l.tick(10_000, &mut sys);
        assert!(l.pen_texto().contains("F12, F11, F8 o Esc"));

        let (mut l, mut sys) = nuevo(Err(FsError::NoSpace));
        l.pen_activate(Item::Card(0), &mut sys);
        l.code = String::from("BORRAR");
        l.pen_activate(Item::Ok, &mut sys);
        l.tick(10_000, &mut sys);
        assert_eq!((l.pen.paso, l.pen.aviso), (Paso::Instalar, Some(MINIMO)));
        // Esc desde la confirmación vuelve a la lista y borra lo escrito.
        l.pen_activate(Item::Card(0), &mut sys);
        l.code = String::from("BOR");
        l.pen_volver();
        assert_eq!(l.pen.paso, Paso::Instalar);
        assert!(l.code.is_empty());
    }
}
