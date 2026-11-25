// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Configuración: personalización (fondo y color de énfasis), cuentas y datos del sistema.
//! CONTRATO: `pub fn new() -> Box<dyn App>`.
//!
//! Cuentas: las operaciones con varios pasos (poner o cambiar la clave, añadir, renombrar y
//! eliminar una cuenta) encadenan diálogos; lo que se va recogiendo vive en `Job`. Las claves
//! solo pasan por `Secret`, que las borra de la memoria al soltarlas, y nunca van a un título,
//! al registro ni a un archivo.
//!
//! Cifrado (v0.6.0): "Mi cuenta" cifra la carpeta del usuario y la página Disco el disco entero.
//! Ambos flujos son largos (aviso, clave, repetir, pantalla del ratón, código de recuperación);
//! las claves viven en `Job` solo entre un diálogo y el siguiente y el código de recuperación
//! solo existe mientras se construye el diálogo que lo muestra.
use super::{App, AppKind, Ctx, MouseEv};
use crate::accounts::{salt_from, AccountError, MIN_PASSWORD};
use crate::arranque::{self, InfoArranque};
use crate::crypto::{ct_eq, wipe};
use crate::dialogs::{Answer, Buttons, DialogRequest, DialogResult};
use crate::font::{self, Align};
use crate::fsapi::FilesApi;
use crate::fs::{same, FsError};
use crate::gamepad::*;
use crate::geom::Rect;
use crate::icons::{self, Icon};
use crate::input::KeyEvent;
use crate::net::{ip_text, mac_text, Ipv4, NetStatus, State};
use crate::keys::{self, Pool};
use crate::platform::{DateTime, Platform};
use crate::programs::{self, Program};
use crate::scrollbar::{Metrics, ScrollBar, SB_W};
use crate::surface::Surface;
use crate::letra;
use crate::theme::{render_wallpaper, Theme, ACCENTS, WALLPAPERS};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const PANEL_W: i32 = 220;
const THUMB: (i32, i32) = (128, 72);
const TEXT: u32 = 0x1A1A1A;
const GRAY: u32 = 0x6A6A6A;
const SECTIONS: [(&str, Icon); 9] = [
    ("Personalización", Icon::Palette),
    ("Cuentas", Icon::Cuentas),
    ("Aplicaciones", Icon::VistaIconosGrandes),
    ("Disco", Icon::DiscoSeguro),
    ("Dispositivos", Icon::Mando),
    ("Red", Icon::RedConectada),
    ("Fecha y hora", Icon::Reloj),
    ("Arranque", Icon::Power),
    ("Acerca de", Icon::Info),
];
const PAGE_ACCOUNTS: usize = 1;
const PAGE_APPS: usize = 2;
const PAGE_DISK: usize = 3;
const PAGE_DEVICES: usize = 4;
const PAGE_NET: usize = 5;
const PAGE_TIME: usize = 6;
const PAGE_ARRANQUE: usize = 7;
const PAGE_ABOUT: usize = 8;
const BTN_H: i32 = 26;
/// Paso de la rueda y de las flechas de la barra, en píxeles.
const STEP: i32 = 24;

const TITLE: &str = "Cuentas";
const T_MSG: u32 = 1;
const T_PW_OLD: u32 = 10;
const T_PW_NEW: u32 = 11;
const T_PW_REP: u32 = 12;
const T_ADD_NAME: u32 = 20;
const T_ADD_PW: u32 = 21;
const T_ADD_REP: u32 = 22;
const T_ADD_ADMIN: u32 = 23;
const T_REN: u32 = 30;
const T_DEL_FILES: u32 = 40;
const T_DEL_NAME: u32 = 41;
const T_CONF: u32 = 50;
const T_SEC: u32 = 51;
const T_NEWK: u32 = 52;
const T_REPK: u32 = 53;
const T_WEAK: u32 = 54;
const T_ENT: u32 = 55;
const T_CODE: u32 = 56;
const T_ASK_HOME: u32 = 57;
/// Aplicaciones (v0.9): carpeta a instalar, reemplazar una ya instalada, confirmar desinstalar.
const T_APP_DIR: u32 = 60;
const T_APP_REPLACE: u32 = 61;
const T_APP_DEL: u32 = 62;
/// Certificados (v0.12.2): archivo a añadir, confirmar añadir, confirmar quitar.
const T_CERT_FILE: u32 = 63;
const T_CERT_ADD: u32 = 64;
const T_CERT_DEL: u32 = 65;
/// Fecha y hora (v0.12.3): cambiar la fecha o la hora a mano.
const T_DATE: u32 = 66;
const T_TIME: u32 = 67;
/// Arranque (v0.12.7): nombre nuevo de una entrada del menú.
const T_ARR_NOM: u32 = 68;
const NO_MATCH: &str = "Las claves no coinciden.";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Act {
    SetPwd,
    ChangePwd,
    RemovePwd,
    Add,
    /// Índice en `Settings::others`.
    Rename(usize),
    Admin(usize),
    Delete(usize),
    HomeEnc,
    HomeDec,
    HomeCode,
    DiskEnc,
    DiskDec,
    DiskChange,
    DiskCode,
    AppInstall,
    /// Índice en `Settings::apps`.
    AppUninstall(usize),
    /// Probar el mando `i` de `Settings::pads` y volver a la lista.
    PadTest(usize),
    PadBack,
    /// Red: pedir la dirección otra vez.
    NetRenew,
    /// Índice en `Settings::net_perms`: cambiar «usar la red», «aceptar conexiones» u olvidarlo.
    NetOut(usize),
    NetIn(usize),
    NetForget(usize),
    /// Certificados: elegir un archivo para añadir una raíz; quitar la raíz propia `i` de `Settings::certs`.
    CertAdd,
    CertDel(usize),
    /// Fecha y hora: interruptor «automático», «Sincronizar ahora», zona `i` de `hora::ZONAS`, cambiar fecha u hora a mano.
    TimeAuto,
    TimeSync,
    TimeZone(usize),
    TimeDate,
    TimeClock,
    /// Arranque: segundos de la cuenta atrás, ocultar o mostrar y renombrar la entrada `i` de `Settings::arr`, y poner NightFury el primero.
    ArrMenos,
    ArrMas,
    ArrOcultar(usize),
    ArrNombre(usize),
    ArrPrimero,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hit {
    Nav(usize),
    Wall(u8),
    Accent(usize),
    /// Letra del sistema que se elige (valor de `Theme::letra`).
    Letra(u8),
    Act(Act),
}

/// Texto secreto (claves) que se borra de la memoria al soltarlo.
#[derive(Default)]
struct Secret(String);

impl Drop for Secret {
    fn drop(&mut self) {
        // SAFETY: se escriben ceros, que son UTF-8 válido.
        wipe(unsafe { self.0.as_bytes_mut() });
    }
}

impl Secret {
    fn same(&self, other: &Secret) -> bool {
        ct_eq(self.0.as_bytes(), other.0.as_bytes())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    SetPwd,
    ChangePwd,
    RemovePwd,
    Add,
    Rename,
    Delete,
    HomeEnc,
    HomeDec,
    HomeCode,
    DiskEnc,
    DiskDec,
    DiskChange,
    DiskCode,
}

/// Operación de cuentas en marcha entre un diálogo y el siguiente.
struct Job {
    op: Op,
    /// Cuenta sobre la que se actúa (o la que se crea).
    name: String,
    old: Secret,
    new: Secret,
    /// Mensaje que se muestra al cerrar el diálogo del código de recuperación.
    after: &'static str,
}

struct Settings {
    page: usize,
    /// Fuentes de Windows ofrecidas (se listan al abrir Personalización) y la TrueType elegida por última vez.
    fuentes: Option<Vec<String>>,
    tt: u8,
    thumbs: Vec<Surface>,
    /// Color de énfasis con el que se generó la miniatura de "Color sólido".
    thumb_accent: u32,
    hover: Option<usize>,
    /// Zonas pulsables de lo dibujado por última vez.
    hits: Vec<(Rect, Hit)>,
    /// Segundo (de `now_ms`) de la última vez que se redibujó "Acerca de".
    last_sec: u64,
    /// Página de cuentas: desplazamiento, alto del contenido y tamaño dibujado, barra, y las
    /// demás cuentas (nombre, administrador) tal como se vieron al dibujar.
    scroll: i32,
    content_h: i32,
    size: (i32, i32),
    sb: ScrollBar,
    others: Vec<(String, bool)>,
    job: Option<Job>,
    /// Botón de la página Cuentas bajo el puntero.
    hover_hit: Option<Act>,
    /// Programas instalados tal como se vieron al dibujar la página Aplicaciones.
    apps: Vec<Program>,
    /// Programa a instalar (reemplazo) o a desinstalar mientras se pregunta.
    pending_app: Option<Program>,
    /// Mandos conectados (`App::gamepads`), el que se está probando y su último estado.
    pads: Vec<PadInfo>,
    testing: Option<u32>,
    pad_state: PadState,
    /// Estado de la red tal como se vio al dibujar la página Red (para redibujar cuando cambia).
    net_seen: Option<NetStatus>,
    /// Permisos de red de los programas tal como se vieron al dibujar la página Red.
    net_perms: Vec<crate::netperm::Perm>,
    /// Raíces propias tal como se vieron al dibujar la página Red.
    certs: Vec<crate::x509::CertInfo>,
    /// Raíz elegida (DER) esperando la confirmación, y huella de la que se va a quitar.
    cert_der: Option<Vec<u8>>,
    cert_del: Option<[u8; 32]>,
    /// Menú de arranque tal como se vio al dibujar la página Arranque, y la entrada que se está renombrando.
    arr: Option<InfoArranque>,
    arr_nom: Option<String>,
}

pub fn new() -> Box<dyn App> {
    Box::new(init())
}

fn init() -> Settings {
    Settings {
        page: 0,
        fuentes: None,
        tt: letra::LIBERATION,
        thumbs: Vec::new(),
        thumb_accent: 0,
        hover: None,
        hits: Vec::new(),
        last_sec: 0,
        scroll: 0,
        content_h: 0,
        size: (0, 0),
        sb: ScrollBar::new(false),
        others: Vec::new(),
        job: None,
        hover_hit: None,
        apps: Vec::new(),
        pending_app: None,
        pads: Vec::new(),
        testing: None,
        pad_state: PadState::default(),
        net_seen: None,
        net_perms: Vec::new(),
        certs: Vec::new(),
        cert_der: None,
        cert_del: None,
        arr: None,
        arr_nom: None,
    }
}

fn nav_rect(i: usize) -> Rect {
    Rect::new(0, 56 + 40 * i as i32, PANEL_W, 40)
}

impl Settings {
    fn panel(&mut self, s: &mut Surface, theme: &Theme) {
        s.fill_rect(Rect::new(0, 0, PANEL_W, s.h), 0xF2F2F2);
        font::draw(s, 16, 18, "Configuración", 12, true, TEXT);
        for (i, (name, icon)) in SECTIONS.iter().enumerate() {
            let r = nav_rect(i);
            self.hits.push((r, Hit::Nav(i)));
            if self.page == i {
                s.fill_rect(r, 0xE5E5E5);
                s.fill_rect(Rect::new(0, r.y + 8, 3, r.h - 16), theme.accent);
            } else if self.hover == Some(i) {
                s.fill_rect(r, 0xEAEAEA);
            }
            icons::draw(s, *icon, 16, r.y + 10, 20, 0x444444);
            font::draw_in(s, Rect::new(48, r.y, PANEL_W - 48, r.h), name, 9, false, TEXT, Align::Left);
        }
    }

    fn personalization(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform) {
        let compact = s.h < 440;
        let x0 = PANEL_W + 24;
        let avail = (s.w - x0 - if compact { 12 } else { 24 }).max(1);
        let mut y = if compact { 14 } else { 24 } - self.scroll;
        font::draw(s, x0, y, "Personalización", 18, false, TEXT);
        y += font::line_h(18) + if compact { 4 } else { 10 };
        font::draw(s, x0, y, "Fondo", 11, true, TEXT);
        y += font::line_h(11) + 4;
        // Miniaturas de los fondos.
        let (tw, th) = THUMB;
        let per = ((avail + 8) / (tw + 8)).max(1);
        let row_h = th + 2 + font::line_h(9) + if compact { 4 } else { 8 };
        for (i, name) in WALLPAPERS.iter().enumerate() {
            let (col, row) = (i as i32 % per, i as i32 / per);
            let r = Rect::new(x0 + col * (tw + 8), y + row * row_h, tw, th);
            s.blit(&self.thumbs[i], r.x, r.y);
            let sel = theme.wallpaper as usize == i;
            if sel {
                s.frame(r.inflate(2), theme.accent);
                s.frame(r.inflate(1), theme.accent);
            }
            font::draw(s, r.x, r.bottom() + 2, name, 9, sel, TEXT);
            self.hits.push((r, Hit::Wall(i as u8)));
        }
        y += ((WALLPAPERS.len() as i32 + per - 1) / per) * row_h + if compact { 0 } else { 4 };
        font::draw(s, x0, y, "Color de énfasis", 11, true, TEXT);
        y += font::line_h(11) + 6;
        // Muestras de color.
        let (sz, gap) = if compact { (30, 6) } else { (40, 6) };
        let per = ((avail + gap) / (sz + gap)).max(1);
        for (i, &c) in ACCENTS.iter().enumerate() {
            let (col, row) = (i as i32 % per, i as i32 / per);
            let r = Rect::new(x0 + col * (sz + gap), y + row * (sz + gap), sz, sz);
            s.fill_rect(r, c);
            if c == theme.accent {
                s.frame(r.inflate(2), 0x333333);
                s.frame(r.inflate(1), 0x333333);
                font::draw_in(s, r, "✓", 14, true, 0xFFFFFF, Align::Center);
            }
            self.hits.push((r, Hit::Accent(i)));
        }
        y += ((ACCENTS.len() as i32 + per - 1) / per) * (sz + gap) + 10;
        self.letra_section(s, theme, sys, x0, avail, y);
    }

    /// «Letra del sistema»: TrueType o trazos y, con TrueType, qué fuente (Liberation u otra de Windows).
    fn letra_section(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, x0: i32, avail: i32, mut y: i32) {
        let fuentes = self.fuentes.get_or_insert_with(|| letra::lista(sys.fs())).clone();
        if theme.letra != letra::TRAZOS {
            self.tt = theme.letra;
        }
        // Una fuente que ya no está en la lista (se quitó el disco) se muestra como Liberation.
        let cur = if self.tt >= 2 && self.tt as usize - 1 > fuentes.len() { letra::LIBERATION } else { self.tt };
        font::draw(s, x0, y, "Letra del sistema", 11, true, TEXT);
        y += font::line_h(11) + 6;
        for (i, (nombre, valor)) in [("TrueType", cur), ("Trazos", letra::TRAZOS)].into_iter().enumerate() {
            let r = Rect::new(x0 + i as i32 * 118, y, 110, 28);
            chip(s, r, nombre, (theme.letra == letra::TRAZOS) == (valor == letra::TRAZOS), theme);
            self.hits.push((r, Hit::Letra(valor)));
        }
        y += 28 + 8;
        if theme.letra != letra::TRAZOS {
            // Posición en «Liberation + fuentes de Windows»: 0 es Liberation, p >= 1 es `fuentes[p - 1]`.
            let (n, p) = (fuentes.len() as i32 + 1, if cur == letra::LIBERATION { 0 } else { cur as i32 - 1 });
            let valor = |p: i32| if p == 0 { letra::LIBERATION } else { p as u8 + 1 };
            let nombre = if p == 0 { "Liberation (incluida)" } else { fuentes[p as usize - 1].trim_end_matches(".ttf") };
            let nw = (avail - 72).clamp(100, 260);
            let ra = Rect::new(x0, y, 28, 28);
            let rn = Rect::new(x0 + 34, y, nw, 28);
            let rb = Rect::new(rn.right() + 6, y, 28, 28);
            chip(s, ra, "<", false, theme);
            chip(s, rb, ">", false, theme);
            s.fill_rect(rn, OFF);
            s.frame(rn, OFF_LINE);
            font::draw_in(s, Rect::new(rn.x + 8, rn.y, rn.w - 16, rn.h), nombre, 9, false, TEXT, Align::Left);
            self.hits.push((ra, Hit::Letra(valor((p + n - 1) % n))));
            self.hits.push((rb, Hit::Letra(valor((p + 1) % n))));
            y += 28 + 8;
        }
        self.content_h = y + self.scroll + 16;
    }

    fn net_page(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform) {
        let compact = s.h < 440;
        let x0 = PANEL_W + 24;
        let mut y = if compact { 14 } else { 24 } - self.scroll;
        font::draw(s, x0, y, "Red", 18, false, TEXT);
        y += font::line_h(18) + if compact { 8 } else { 16 };
        let st = sys.net().map(|n| n.status());
        self.net_seen = st.clone();
        let some = |f: &dyn Fn(&NetStatus) -> Option<String>| st.as_ref().and_then(f).unwrap_or_else(|| String::from("—"));
        let ipt = |v: Option<Ipv4>| v.map(ip_text);
        let state = match st.as_ref().map(|s| s.state) {
            Some(State::Connected) => "Conectado",
            Some(State::NoCable) => "Sin cable",
            Some(State::Configuring) => "Obteniendo dirección…",
            Some(State::NoInternet) => "Sin Internet",
            Some(State::NoAdapter) | None => "Sin tarjeta",
        };
        let rows = [
            ("Tarjeta", st.as_ref().map_or(String::from("No hay ninguna tarjeta de red"), |s| s.adapter.clone())),
            ("Estado", String::from(state)),
            ("Velocidad", some(&|s| (s.speed_mbps > 0).then(|| super::netcmd::speed_text(s.speed_mbps, s.full_duplex)))),
            ("Dirección física (MAC)", some(&|s| (!s.adapter.is_empty()).then(|| mac_text(s.mac)))),
            ("Dirección IPv4", some(&|s| ipt(s.ip))),
            ("Máscara de subred", some(&|s| ipt(s.mask))),
            ("Puerta de enlace", some(&|s| ipt(s.gateway))),
            ("Servidores DNS", some(&|s| (!s.dns.is_empty()).then(|| s.dns.iter().map(|d| ip_text(*d)).collect::<Vec<_>>().join(", ")))),
            ("Servidor DHCP", some(&|s| ipt(s.dhcp_server))),
        ];
        let row_h = if compact { 22 } else { 28 };
        for (label, value) in rows.iter() {
            font::draw_in(s, Rect::new(x0, y, 170, row_h), label, 9, false, GRAY, Align::Left);
            font::draw_in(s, Rect::new(x0 + 170, y, (s.w - x0 - 170).max(0), row_h), value, 9, false, TEXT, Align::Left);
            y += row_h;
        }
        y += if compact { 6 } else { 14 };
        let on = matches!(st.map(|s| s.state), Some(State::Configuring | State::NoInternet | State::Connected));
        let right = (s.w - SB_W - 8).max(x0 + 1);
        self.button_if(s, &mut (x0, y), x0, right, "Renovar dirección", Act::NetRenew, on, theme);
        y += BTN_H + if compact { 14 } else { 22 };
        y = self.net_perms_list(s, theme, sys, x0, y, right);
        y = self.certs_list(s, theme, sys, x0, y, right);
        self.content_h = y + self.scroll + 16;
    }

    /// v0.12.1: los programas de terceros que han pedido usar la red y lo que se les contestó.
    #[allow(clippy::too_many_arguments)]
    fn net_perms_list(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, x0: i32, mut y: i32, right: i32) -> i32 {
        font::draw(s, x0, y, "Programas que pueden usar la red", 11, true, TEXT);
        y += font::line_h(11) + 6;
        let admin = sys.fs().is_admin();
        y = note(s, x0, y, right - x0, if admin {
            "Cada programa pregunta la primera vez. Si el programa cambia, vuelve a preguntar."
        } else {
            "Solo un administrador puede cambiar estos permisos."
        });
        y += 10;
        self.net_perms = sys.fs().net_perms();
        if self.net_perms.is_empty() {
            font::draw(s, x0, y, "Ningún programa ha pedido usar la red.", 9, false, GRAY);
            return y + 24;
        }
        let text = |v: Option<bool>| match v {
            Some(true) => "Permitido",
            Some(false) => "No permitido",
            None => "Sin preguntar",
        };
        for i in 0..self.net_perms.len() {
            let p = self.net_perms[i].clone();
            font::draw(s, x0, y, &p.name, 9, true, TEXT);
            y += font::line_h(9) + 2;
            for (label, v) in [("Usar la red", p.out), ("Aceptar conexiones de la red local", p.inn)] {
                let end = font::draw(s, x0, y, label, 9, false, GRAY);
                font::draw(s, end + 8, y, text(v), 9, false, TEXT);
                y += font::line_h(9);
            }
            if admin {
                let mut at = (x0, y + 4);
                let flip = |v: Option<bool>, yes: &'static str, no: &'static str| if v == Some(true) { no } else { yes };
                self.button(s, &mut at, x0, right, flip(p.out, "Permitir la red", "Bloquear la red"), Act::NetOut(i), theme);
                self.button(s, &mut at, x0, right, flip(p.inn, "Permitir conexiones", "Bloquear conexiones"), Act::NetIn(i), theme);
                self.button(s, &mut at, x0, right, "Olvidar", Act::NetForget(i), theme);
                y = at.1 + BTN_H;
            }
            y += 12;
        }
        y
    }

    /// v0.12.2: raíces de confianza para HTTPS: las de Mozilla (fijas) y las que añade un administrador.
    #[allow(clippy::too_many_arguments)]
    fn certs_list(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, x0: i32, mut y: i32, right: i32) -> i32 {
        font::draw(s, x0, y, "Certificados", 11, true, TEXT);
        y += font::line_h(11) + 6;
        let admin = sys.fs().is_admin();
        y = note(s, x0, y, right - x0, if admin {
            "Son las autoridades en las que confía el sistema para abrir sitios seguros (https). Las de Mozilla vienen con el sistema y no se pueden cambiar."
        } else {
            "Son las autoridades en las que confía el sistema para abrir sitios seguros (https). Solo un administrador puede añadir o quitar certificados."
        });
        y += 6;
        font::draw(s, x0, y, &format!("{} certificados de Mozilla", crate::raices::mozilla_count()), 9, false, TEXT);
        y += font::line_h(9) + 8;
        self.certs = sys.fs().custom_roots().iter().filter_map(|d| crate::x509::Cert::parse(d).ok()).map(|c| c.info()).collect();
        if self.certs.is_empty() {
            font::draw(s, x0, y, "No has añadido ningún certificado.", 9, false, GRAY);
            y += 24;
        }
        for i in 0..self.certs.len() {
            let c = self.certs[i].clone();
            font::draw(s, x0, y, &cert_who(&c.cn, &c.org), 9, true, TEXT);
            y += font::line_h(9) + 2;
            let line = format!("Emitido por {} · Caduca el {} · Huella {}", cert_who(&c.issuer.cn, &c.issuer.org), cert_date(&c.not_after), cert_fp(&c.sha256, 0, 4));
            font::draw_in(s, Rect::new(x0, y, (right - x0).max(0), font::line_h(9)), &line, 9, false, GRAY, Align::Left);
            y += font::line_h(9);
            if admin {
                let mut at = (x0, y + 4);
                self.button(s, &mut at, x0, right, "Quitar", Act::CertDel(i), theme);
                y = at.1 + BTN_H;
            }
            y += 10;
        }
        if admin {
            let mut at = (x0, y);
            self.button(s, &mut at, x0, right, "Añadir certificado…", Act::CertAdd, theme);
            y = at.1 + BTN_H;
        }
        y
    }

    /// v0.12.3: hora actual, zona, hora automática por internet y cambio a mano.
    fn time_page(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform) {
        let compact = s.h < 440;
        let x0 = PANEL_W + 24;
        let right = (s.w - SB_W - 8).max(x0 + 1);
        let mut y = if compact { 14 } else { 24 } - self.scroll;
        font::draw(s, x0, y, "Fecha y hora", 18, false, TEXT);
        y += font::line_h(18) + if compact { 8 } else { 16 };
        let admin = sys.fs().is_admin();
        let cfg = sys.fs().time_cfg();
        let d = sys.now();
        font::draw(s, x0, y, &format!("{:02}:{:02}:{:02}", d.hour, d.minute, d.second), 22, true, TEXT);
        y += font::line_h(22) + 2;
        font::draw(s, x0, y, &format!("{:02}/{:02}/{}", d.day, d.month, d.year), 11, false, GRAY);
        y += font::line_h(11) + if compact { 10 } else { 18 };
        if !admin {
            y = note(s, x0, y, right - x0, "Solo un administrador puede cambiar la fecha, la hora y la zona horaria.") + 10;
        }
        // Interruptor «automático».
        let sw = Rect::new(x0, y, 38, 18);
        let (on_c, knob) = (if cfg.auto { theme.accent } else { 0xBDBDBD }, if cfg.auto { sw.x + 29 } else { sw.x + 9 });
        s.fill_circle((sw.x + 9) as f32, (sw.y + 9) as f32, 9.0, on_c, 255);
        s.fill_circle((sw.x + 29) as f32, (sw.y + 9) as f32, 9.0, on_c, 255);
        s.fill_rect(Rect::new(sw.x + 9, sw.y, 20, 18), on_c);
        s.fill_circle(knob as f32, (sw.y + 9) as f32, 7.0, 0xFFFFFF, 255);
        if admin {
            self.hits.push((sw, Hit::Act(Act::TimeAuto)));
        }
        font::draw_in(s, Rect::new(x0 + 48, y, (right - x0 - 48).max(0), 18), "Ajustar la hora automáticamente", 10, true, TEXT, Align::Left);
        y += 18 + 4;
        y = note(s, x0 + 48, y, right - x0 - 48, "Pide la hora a pool.ntp.org; ese servidor ve la dirección IP del equipo.") + 10;
        let info = sys.fs().time_sync_info();
        let mut at = (x0, y);
        self.button_if(s, &mut at, x0, right, "Sincronizar ahora", Act::TimeSync, admin && info.estado != 1, theme);
        if !cfg.auto && admin {
            self.button(s, &mut at, x0, right, "Cambiar fecha…", Act::TimeDate, theme);
            self.button(s, &mut at, x0, right, "Cambiar hora…", Act::TimeClock, theme);
        }
        y = at.1 + BTN_H + 6;
        let f = info.fin;
        let hora = format!("{:02}/{:02}/{} {:02}:{:02}", f.day, f.month, f.year, f.hour, f.minute);
        let estado = match info.estado {
            1 => String::from("Sincronizando…"),
            2 => format!("Hora ajustada con internet el {hora}."),
            3 => format!("La hora ya era correcta ({hora})."),
            4 => format!("No se pudo obtener la hora de pool.ntp.org ({hora})."),
            _ => String::from("Todavía no se ha sincronizado."),
        };
        font::draw(s, x0, y, &estado, 9, false, GRAY);
        y += font::line_h(9) + if compact { 10 } else { 18 };
        font::draw(s, x0, y, "Zona horaria", 11, true, TEXT);
        y += font::line_h(11) + 6;
        for (i, z) in crate::hora::ZONAS.iter().enumerate() {
            let r = Rect::new(x0, y, (right - x0).max(1), 24);
            let sel = cfg.zona as usize == i;
            if sel {
                s.fill_rect(r, theme.accent);
            } else if admin && self.hover_hit == Some(Act::TimeZone(i)) {
                s.fill_rect(r, 0xEAEAEA);
            }
            font::draw_in(s, Rect::new(r.x + 8, r.y, r.w - 8, r.h), z.nombre, 9, sel, if sel { 0xFFFFFF } else { TEXT }, Align::Left);
            if admin {
                self.hits.push((r, Hit::Act(Act::TimeZone(i))));
            }
            y += 24;
        }
        self.content_h = y + self.scroll + 16;
    }

    /// v0.12.7: ajustes del menú de arranque (`arranque.efi`): cuenta atrás, entradas y orden en el firmware.
    fn arranque_page(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform) {
        let compact = s.h < 440;
        let x0 = PANEL_W + 24;
        let right = (s.w - SB_W - 8).max(x0 + 1);
        let mut y = if compact { 14 } else { 24 } - self.scroll;
        font::draw(s, x0, y, "Arranque", 18, false, TEXT);
        y += font::line_h(18) + if compact { 6 } else { 12 };
        self.arr = sys.arranque_leer();
        let Some(info) = self.arr.clone() else {
            let y = note(s, x0, y, right - x0, "El menú de arranque de NightFury no está instalado en este equipo. Se instala al instalar NightFury desde el pendrive.");
            self.content_h = y + self.scroll + 16;
            return;
        };
        let admin = sys.fs().is_admin();
        if !admin {
            y = note(s, x0, y, right - x0, "Hace falta una cuenta de administrador.") + 10;
        }
        font::draw(s, x0, y, "Cuenta atrás", 11, true, TEXT);
        y += font::line_h(11) + 6;
        let seg = info.ajustes.segundos;
        let txt = format!("{seg} s");
        let mut at = (x0, y);
        if admin {
            self.button_if(s, &mut at, x0, right, "−", Act::ArrMenos, seg > 0, theme);
        }
        let w = font::width(&txt, 10, true) + 16;
        font::draw_in(s, Rect::new(at.0, at.1, w, BTN_H), &txt, 10, true, TEXT, Align::Center);
        at.0 += w + 8;
        if admin {
            self.button_if(s, &mut at, x0, right, "+", Act::ArrMas, seg < 30, theme);
        }
        y = at.1 + BTN_H + 4;
        if seg == 0 {
            y = note(s, x0, y, right - x0, "Arranca directo (mantén una tecla para ver el menú).");
        }
        y += if compact { 8 } else { 14 };
        font::draw(s, x0, y, "Sistemas del menú", 11, true, TEXT);
        y += font::line_h(11) + 6;
        for (i, (clave, orig, ocultable)) in info.entradas.iter().enumerate() {
            let oculta = info.ajustes.ocultas.contains(clave);
            let nombre = info.ajustes.nombres.iter().find(|(c, n)| c == clave && !n.is_empty()).map_or(orig.as_str(), |(_, n)| n.as_str());
            let end = font::draw(s, x0, y, nombre, 9, true, if oculta { GRAY } else { TEXT });
            if oculta {
                font::draw(s, end + 10, y, "Oculto", 9, false, GRAY);
            }
            y += font::line_h(9) + 4;
            if admin {
                let mut at = (x0, y);
                if *ocultable {
                    self.button(s, &mut at, x0, right, if oculta { "Mostrar" } else { "Ocultar" }, Act::ArrOcultar(i), theme);
                }
                self.button(s, &mut at, x0, right, "Renombrar", Act::ArrNombre(i), theme);
                y = at.1 + BTN_H;
            }
            y += 10;
        }
        y += if compact { 4 } else { 10 };
        font::draw(s, x0, y, if info.primero { "El menú de NightFury es lo primero al encender." } else { "Otro sistema se ha puesto delante." }, 9, false, if info.primero { TEXT } else { GRAY });
        y += font::line_h(9) + 8;
        if admin && !info.primero {
            let mut at = (x0, y);
            self.button(s, &mut at, x0, right, "Poner el menú de NightFury el primero", Act::ArrPrimero, theme);
            y = at.1 + BTN_H;
        }
        self.content_h = y + self.scroll + 16;
    }

    fn about(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform) {
        let compact = s.h < 440;
        let x0 = PANEL_W + 24;
        let mut y = if compact { 14 } else { 24 };
        font::draw(s, x0, y, "Acerca de", 18, false, TEXT);
        y += font::line_h(18) + if compact { 8 } else { 16 };
        icons::draw(s, Icon::Logo, x0, y, 48, theme.accent);
        let name = format!("{} {}", crate::NAME, crate::VERSION);
        font::draw_in(s, Rect::new(x0 + 60, y, (s.w - x0 - 60).max(0), 48), &name, 20, true, TEXT, Align::Left);
        y += 48 + if compact { 12 } else { 20 };
        let mem = sys.mem();
        let mb = |b: u64| b / (1 << 20);
        let up = sys.uptime_ms() / 60_000;
        let (sw, sh) = sys.screen();
        let (total, free) = sys.fs().space();
        let disk = format!(
            "{} libres de {}{}",
            super::terminal::units(free),
            super::terminal::units(total),
            if sys.fs().persistent() { "" } else { " (en memoria)" }
        );
        let rows = [
            ("Procesador", sys.cpu()),
            ("Memoria instalada", format!("{} MB", mb(mem.total))),
            ("Memoria del sistema", format!("{} / {} MB", mb(mem.heap_used), mb(mem.heap_total))),
            ("Pantalla", format!("{} x {}", sw, sh)),
            ("Disco C:", disk),
            ("Tipo de sistema", String::from("64 bits, arranque UEFI")),
            ("Tiempo encendido", format!("{} h {} min", up / 60, up % 60)),
        ];
        let row_h = if compact { 22 } else { 28 };
        for (label, value) in rows.iter() {
            font::draw_in(s, Rect::new(x0, y, 170, row_h), label, 9, false, GRAY, Align::Left);
            font::draw_in(s, Rect::new(x0 + 170, y, (s.w - x0 - 170).max(0), row_h), value, 9, false, TEXT, Align::Left);
            y += row_h;
        }
        y += if compact { 6 } else { 14 };
        font::draw(s, x0, y, "Sistema operativo escrito desde cero en Rust.", 9, false, GRAY);
        y += font::line_h(9) + 6;
        for line in [crate::CREDIT, crate::CREDIT_URL, crate::LICENSE_LINE] {
            font::draw(s, x0, y, line, 9, false, GRAY);
            y += font::line_h(9) + 2;
        }
    }
}

/// «Nombre — Organización» (o lo que haya) de un certificado.
fn cert_who(cn: &str, org: &str) -> String {
    match (cn.is_empty(), org.is_empty()) {
        (false, false) if cn != org => format!("{cn} — {org}"),
        (false, _) => String::from(cn),
        (true, false) => String::from(org),
        _ => String::from("(sin nombre)"),
    }
}

fn cert_date(t: &crate::x509::Time) -> String {
    format!("{:02}/{:02}/{}", t.day, t.month, t.year)
}

/// `n` grupos de 4 cifras (mayúsculas) de la huella SHA-256, desde el grupo `from` (hay 16 en total).
fn cert_fp(sha: &[u8; 32], from: usize, n: usize) -> String {
    let hex = crate::crypto::to_hex(sha).to_uppercase();
    let parts: Vec<&str> = (from..(from + n).min(16)).map(|i| &hex[i * 4..i * 4 + 4]).collect();
    parts.join(" ")
}

fn message(cx: &mut Ctx, text: &str) {
    cx.dialog = Some(DialogRequest::Message { tag: T_MSG, title: String::from(TITLE), text: String::from(text), buttons: Buttons::Ok });
}

fn password(cx: &mut Ctx, tag: u32, label: &str) {
    cx.dialog = Some(DialogRequest::Password { tag, title: String::from(TITLE), label: String::from(label) });
}

fn input(cx: &mut Ctx, tag: u32, label: &str, value: &str) {
    cx.dialog = Some(DialogRequest::Input { tag, title: String::from(TITLE), label: String::from(label), value: String::from(value) });
}

/// Texto gris que salta de línea para caber en `width` píxeles. Devuelve la `y` siguiente.
fn note(s: &mut Surface, x: i32, mut y: i32, width: i32, text: &str) -> i32 {
    let mut line = String::new();
    for word in text.split(' ') {
        let try_line = if line.is_empty() { String::from(word) } else { format!("{line} {word}") };
        if !line.is_empty() && font::width(&try_line, 9, false) > width {
            font::draw(s, x, y, &line, 9, false, GRAY);
            y += font::line_h(9);
            line = String::from(word);
        } else {
            line = try_line;
        }
    }
    font::draw(s, x, y, &line, 9, false, GRAY);
    y + font::line_h(9)
}

fn confirm(cx: &mut Ctx, text: &str) {
    cx.dialog = Some(DialogRequest::Message { tag: T_CONF, title: String::from(TITLE), text: String::from(text), buttons: Buttons::YesNo });
}

/// Llama a `f` con el sistema de archivos y el acumulador de aleatoriedad del sistema. El
/// acumulador se saca de `sys` (son dos préstamos del mismo objeto) y SIEMPRE se devuelve.
fn with_pool<R>(cx: &mut Ctx, f: impl FnOnce(&mut dyn FilesApi, &mut Pool) -> R) -> R {
    let mut pool = core::mem::take(cx.sys.pool());
    let r = f(cx.sys.fs(), &mut pool);
    *cx.sys.pool() = pool;
    r
}

/// Muestra el código de recuperación (una sola vez) y borra la copia que se tenía aquí.
fn show_code(cx: &mut Ctx, mut code: String) {
    let text = format!("{code}\n\nApúntalo en papel y guárdalo en un lugar seguro. Es la única forma de recuperar tus datos si olvidas la clave. No se volverá a mostrar.");
    // SAFETY: se escriben ceros, que son UTF-8 válido.
    wipe(unsafe { code.as_bytes_mut() });
    cx.dialog = Some(DialogRequest::Message { tag: T_CODE, title: String::from("Código de recuperación"), text, buttons: Buttons::Ok });
}

fn salt(cx: &mut Ctx) -> [u8; 16] {
    let (a, b) = (cx.sys.entropy(), cx.sys.entropy());
    salt_from(a, b)
}

// ---- Página Cuentas: dibujo ----
impl Settings {
    /// Botón en la posición del cursor `at`; salta de línea si no cabe. Devuelve su ancho.
    fn button(&mut self, s: &mut Surface, at: &mut (i32, i32), x0: i32, right: i32, label: &str, act: Act, theme: &Theme) {
        self.button_if(s, at, x0, right, label, act, true, theme);
    }

    /// Como `button`; con `on == false` se dibuja apagado y no se puede pulsar.
    #[allow(clippy::too_many_arguments)]
    fn button_if(&mut self, s: &mut Surface, at: &mut (i32, i32), x0: i32, right: i32, label: &str, act: Act, on: bool, theme: &Theme) {
        let w = font::width(label, 9, false) + 24;
        if at.0 > x0 && at.0 + w > right {
            *at = (x0, at.1 + BTN_H + 6);
        }
        let r = Rect::new(at.0, at.1, w, BTN_H);
        if !on {
            s.fill_rect(r, 0xF0F0F0);
            s.frame(r, 0xCFCFCF);
            font::draw_in(s, r, label, 9, false, 0xA8A8A8, Align::Center);
            at.0 += w + 8;
            return;
        }
        let hot = self.hover_hit == Some(act);
        s.fill_rect(r, if hot { 0xD8D8D8 } else { 0xE6E6E6 });
        s.frame(r, if hot { theme.accent } else { 0xA0A0A0 });
        font::draw_in(s, r, label, 9, false, TEXT, Align::Center);
        self.hits.push((r, Hit::Act(act)));
        at.0 += w + 8;
    }

    fn accounts_page(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform) {
        let compact = s.h < 440;
        let x0 = PANEL_W + 24;
        let right = (s.w - SB_W - 8).max(x0 + 1);
        let mut y = if compact { 14 } else { 24 } - self.scroll;
        font::draw(s, x0, y, TITLE, 18, false, TEXT);
        y += font::line_h(18) + if compact { 6 } else { 12 };
        self.others.clear();
        if !sys.fs().persistent() {
            font::draw(s, x0, y, "Sin disco no hay cuentas que guardar.", 9, false, GRAY);
            self.content_h = y + self.scroll + font::line_h(9) + 16;
            return;
        }
        let me = sys.fs().user().unwrap_or_default();
        let admin = sys.fs().is_admin();
        let has_pwd = sys.fs().has_password();
        let list = sys.fs().accounts();
        font::draw(s, x0, y, "Mi cuenta", 11, true, TEXT);
        y += font::line_h(11) + 6;
        let busy = sys.fs().home_conversion();
        let crypt = match busy {
            Some((_, _, true)) => "Cifrando…",
            Some((_, _, false)) => "Descifrando…",
            None if sys.fs().home_encrypted() => "Sí",
            None => "No",
        };
        for (label, value) in [
            ("Nombre", me.clone()),
            ("Tipo", String::from(if admin { "Administrador" } else { "Usuario" })),
            ("Clave", String::from(if has_pwd { "Puesta" } else { "Sin clave" })),
            ("Carpeta cifrada", String::from(crypt)),
        ] {
            font::draw_in(s, Rect::new(x0, y, 120, 22), label, 9, false, GRAY, Align::Left);
            font::draw_in(s, Rect::new(x0 + 120, y, (right - x0 - 120).max(0), 22), &value, 9, false, TEXT, Align::Left);
            y += 22;
        }
        y += 6;
        let mut at = (x0, y);
        if has_pwd {
            self.button(s, &mut at, x0, right, "Cambiar clave", Act::ChangePwd, theme);
            self.button(s, &mut at, x0, right, "Quitar clave", Act::RemovePwd, theme);
        } else {
            self.button(s, &mut at, x0, right, "Poner clave", Act::SetPwd, theme);
        }
        if has_pwd {
            let (enc, on) = (sys.fs().home_encrypted(), busy.is_none());
            if !enc {
                self.button_if(s, &mut at, x0, right, "Cifrar mi carpeta", Act::HomeEnc, on, theme);
            } else {
                self.button_if(s, &mut at, x0, right, "Quitar el cifrado", Act::HomeDec, on, theme);
                self.button_if(s, &mut at, x0, right, "Nuevo código de recuperación", Act::HomeCode, on, theme);
            }
        } else {
            at = (x0, note(s, x0, at.1 + BTN_H + 6, right - x0, "Para cifrar tu carpeta, primero pon una clave.") - BTN_H);
        }
        y = at.1 + BTN_H + if compact { 12 } else { 20 };
        if admin {
            font::draw(s, x0, y, "Otras cuentas", 11, true, TEXT);
            y += font::line_h(11) + 8;
            self.others = list.iter().filter(|a| !same(&a.name, &me)).map(|a| (a.name.clone(), a.admin)).collect();
            if self.others.is_empty() {
                font::draw(s, x0, y, "No hay más cuentas.", 9, false, GRAY);
                y += 24;
            }
            for i in 0..self.others.len() {
                let (name, is_admin) = self.others[i].clone();
                let end = font::draw(s, x0, y, &name, 9, true, TEXT);
                font::draw(s, end + 10, y, if is_admin { "Administrador" } else { "Usuario" }, 9, false, GRAY);
                y += font::line_h(9) + 4;
                let mut at = (x0, y);
                self.button(s, &mut at, x0, right, "Cambiar nombre", Act::Rename(i), theme);
                self.button(s, &mut at, x0, right, if is_admin { "Hacer usuario normal" } else { "Hacer administrador" }, Act::Admin(i), theme);
                self.button(s, &mut at, x0, right, "Eliminar", Act::Delete(i), theme);
                y = at.1 + BTN_H + 12;
            }
            let mut at = (x0, y + 4);
            self.button(s, &mut at, x0, right, "Añadir cuenta", Act::Add, theme);
            y = at.1 + BTN_H;
        }
        self.content_h = y + self.scroll + 16;
    }

    fn disk_page(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform) {
        let compact = s.h < 440;
        let x0 = PANEL_W + 24;
        let right = (s.w - SB_W - 8).max(x0 + 1);
        let mut y = if compact { 14 } else { 24 } - self.scroll;
        font::draw(s, x0, y, "Disco", 18, false, TEXT);
        y += font::line_h(18) + if compact { 6 } else { 12 };
        if !sys.fs().persistent() {
            font::draw(s, x0, y, "Sin disco no hay nada que cifrar.", 9, false, GRAY);
            self.content_h = y + self.scroll + font::line_h(9) + 16;
            return;
        }
        let units = super::terminal::units;
        let (total, free) = sys.fs().space();
        let admin = sys.fs().is_admin();
        let enc = sys.fs().disk_encrypted();
        let busy = sys.fs().disk_conversion();
        let state = match busy {
            Some((done, all, encrypting)) => format!("{}… {} %", if encrypting { "Cifrando" } else { "Descifrando" }, if all == 0 { 0 } else { (done.min(all) * 100 / all) as u32 }),
            None => String::from(if enc { "Sí" } else { "No" }),
        };
        for (label, value) in [
            ("Unidad", String::from("C:")),
            ("Tamaño", units(total)),
            ("Espacio libre", units(free)),
            ("Persistente", String::from("Sí")),
            ("Cifrado total", state),
        ] {
            font::draw_in(s, Rect::new(x0, y, 120, 22), label, 9, false, GRAY, Align::Left);
            font::draw_in(s, Rect::new(x0 + 120, y, (right - x0 - 120).max(0), 22), &value, 9, false, TEXT, Align::Left);
            y += 22;
        }
        y += 6;
        if admin {
            let mut at = (x0, y);
            let on = busy.is_none();
            if enc {
                self.button_if(s, &mut at, x0, right, "Quitar cifrado total", Act::DiskDec, on, theme);
                self.button_if(s, &mut at, x0, right, "Cambiar la clave del disco", Act::DiskChange, on, theme);
                self.button_if(s, &mut at, x0, right, "Nuevo código de recuperación", Act::DiskCode, on, theme);
            } else {
                self.button_if(s, &mut at, x0, right, "Activar cifrado total", Act::DiskEnc, on, theme);
            }
            y = at.1 + BTN_H;
        } else {
            y = note(s, x0, y, right - x0, "Solo un administrador puede cambiar el cifrado del disco.");
        }
        self.content_h = y + self.scroll + 16;
    }

    fn apps_page(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform) {
        let x0 = PANEL_W + 24;
        let right = (s.w - SB_W - 8).max(x0 + 1);
        let mut y = if s.h < 440 { 14 } else { 24 } - self.scroll;
        font::draw(s, x0, y, "Aplicaciones", 18, false, TEXT);
        y += font::line_h(18) + 12;
        let admin = sys.fs().is_admin();
        if admin {
            let mut at = (x0, y);
            self.button(s, &mut at, x0, right, "Instalar programa…", Act::AppInstall, theme);
            y = at.1 + BTN_H + 6;
            y = note(s, x0, y, right - x0, "Elige una carpeta que tenga app.txt y programa.nfx (por ejemplo, de Descargas).");
        } else {
            y = note(s, x0, y, right - x0, "Solo un administrador puede instalar o desinstalar programas.");
        }
        y += 10;
        self.apps = programs::installed(sys);
        if self.apps.is_empty() {
            font::draw(s, x0, y, "No hay programas instalados.", 9, false, GRAY);
            y += 24;
        }
        for i in 0..self.apps.len() {
            let p = self.apps[i].clone();
            match &p.picture {
                Some(pic) => pic.draw(s, x0, y + 2, 32),
                None => icons::draw(s, p.icon, x0, y + 2, 32, TEXT),
            }
            let tx = x0 + 44;
            font::draw(s, tx, y, &p.name, 10, true, TEXT);
            let mut info = String::new();
            if !p.version.is_empty() {
                info = format!("Versión {}", p.version);
            }
            if !p.author.is_empty() {
                info = if info.is_empty() { p.author.clone() } else { format!("{info} · {}", p.author) };
            }
            if p.system {
                info = if info.is_empty() { String::from("Del sistema") } else { format!("{info} · Del sistema") };
            }
            font::draw(s, tx, y + font::line_h(10) + 2, &info, 9, false, GRAY);
            let mut at = (tx, y + font::line_h(10) + font::line_h(9) + 8);
            if admin && !p.system {
                self.button(s, &mut at, tx, right, "Desinstalar", Act::AppUninstall(i), theme);
                y = at.1 + BTN_H + 14;
            } else {
                y = at.1 + 8;
            }
        }
        self.content_h = y + self.scroll + 16;
    }

    fn scrolls(&self) -> bool {
        self.page == 0 || self.page == PAGE_ACCOUNTS || self.page == PAGE_DISK || self.page == PAGE_APPS || self.page == PAGE_NET || self.page == PAGE_TIME || self.page == PAGE_ARRANQUE || (self.page == PAGE_DEVICES && self.testing.is_none())
    }

    fn metrics(&self) -> Metrics {
        Metrics { rect: Rect::new(self.size.0 - SB_W, 0, SB_W, self.size.1), total: self.content_h, page: self.size.1, pos: self.scroll, line: STEP }
    }

    fn max_scroll(&self) -> i32 {
        (self.content_h - self.size.1).max(0)
    }
}

// ---- Página Dispositivos: mandos y unidades ----

/// Marca del mando, para los nombres y las etiquetas de sus botones.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Family {
    Xbox,
    PlayStation,
    Nintendo,
    Generic,
}

fn family(k: PadKind) -> Family {
    match k {
        PadKind::Xbox360 | PadKind::XboxOne => Family::Xbox,
        PadKind::DualShock4 | PadKind::DualSense => Family::PlayStation,
        PadKind::SwitchPro => Family::Nintendo,
        PadKind::Generic => Family::Generic,
    }
}

fn kind_text(k: PadKind) -> &'static str {
    match k {
        PadKind::Generic => "Mando USB genérico",
        PadKind::DualShock4 => "PlayStation (DualShock 4)",
        PadKind::DualSense => "PlayStation (DualSense)",
        PadKind::SwitchPro => "Nintendo (Switch Pro)",
        PadKind::Xbox360 => "Xbox 360",
        PadKind::XboxOne => "Xbox One / Series",
    }
}

/// Etiquetas de (hombro izquierdo, hombro derecho, gatillo izquierdo, gatillo derecho).
fn shoulder_labels(f: Family) -> [&'static str; 4] {
    match f {
        Family::Xbox => ["LB", "RB", "LT", "RT"],
        Family::PlayStation => ["L1", "R1", "L2", "R2"],
        Family::Nintendo => ["L", "R", "ZL", "ZR"],
        Family::Generic => ["L1", "R1", "L2", "R2"],
    }
}

/// Etiquetas de los botones de los botones del rombo en el orden (abajo, derecha, izquierda, arriba).
/// `None` en PlayStation: ahí se dibujan los símbolos.
fn face_labels(f: Family) -> Option<[&'static str; 4]> {
    match f {
        Family::Xbox => Some(["A", "B", "X", "Y"]),
        Family::Nintendo => Some(["B", "A", "Y", "X"]),
        Family::Generic => Some(["1", "2", "3", "4"]),
        Family::PlayStation => None,
    }
}

/// Botones del centro y de los sticks, con su nombre en cada marca.
fn center_buttons(f: Family) -> [(u32, &'static str); 6] {
    let (sel, start, home, extra) = match f {
        Family::Xbox => ("Ver", "Menú", "Xbox", "Compartir"),
        Family::PlayStation => ("Share", "Options", "PS", "Panel táctil"),
        Family::Nintendo => ("-", "+", "Home", "Captura"),
        Family::Generic => ("Select", "Start", "Home", "Extra"),
    };
    [(BTN_SELECT, sel), (BTN_HOME, home), (BTN_START, start), (BTN_L3, "L3"), (BTN_R3, "R3"), (BTN_EXTRA, extra)]
}

/// Posición de un stick dentro del círculo unidad (Y hacia abajo), sin salirse de él.
fn stick_dot(x: i16, y: i16) -> (f32, f32) {
    let (fx, fy) = (x as f32 / 32767.0, y as f32 / 32767.0);
    let len = crate::math::sqrt(fx * fx + fy * fy);
    if len > 1.0 { (fx / len, fy / len) } else { (fx, fy) }
}

const OFF: u32 = 0xE6E6E6;
const OFF_LINE: u32 = 0xA0A0A0;

/// Rectángulo con etiqueta que se ilumina (hombros, botones del centro).
fn chip(s: &mut Surface, r: Rect, label: &str, lit: bool, theme: &Theme) {
    s.fill_rect(r, if lit { theme.accent } else { OFF });
    s.frame(r, if lit { theme.accent } else { OFF_LINE });
    font::draw_in(s, r, label, 9, false, if lit { 0xFFFFFF } else { TEXT }, Align::Center);
}

fn stick(s: &mut Surface, c: (f32, f32), r: f32, pos: (i16, i16), pressed: bool, theme: &Theme) {
    s.fill_circle(c.0, c.1, r, OFF, 255);
    s.arc(c.0, c.1, r - 0.5, 0.0, 360.0, if pressed { 3.0 } else { 1.5 }, if pressed { theme.accent } else { OFF_LINE });
    let (dx, dy) = stick_dot(pos.0, pos.1);
    let reach = r - 7.0;
    s.fill_circle(c.0 + dx * reach, c.1 + dy * reach, 6.0, theme.accent, 255);
}

fn dpad(s: &mut Surface, c: (i32, i32), d: i32, buttons: u32, theme: &Theme) {
    let (l, t) = (d / 2, (d / 3).max(4));
    let col = |m: u32| if buttons & m != 0 { theme.accent } else { 0xCFCFCF };
    let arm = l - t / 2;
    s.fill_rect(Rect::new(c.0 - t / 2, c.1 - l, t, arm), col(BTN_UP));
    s.fill_rect(Rect::new(c.0 - t / 2, c.1 + t / 2, t, arm), col(BTN_DOWN));
    s.fill_rect(Rect::new(c.0 - l, c.1 - t / 2, arm, t), col(BTN_LEFT));
    s.fill_rect(Rect::new(c.0 + t / 2, c.1 - t / 2, arm, t), col(BTN_RIGHT));
    s.fill_rect(Rect::new(c.0 - t / 2, c.1 - t / 2, t, t), 0xCFCFCF);
}

/// Los cuatro botones del rombo: abajo, derecha, izquierda, arriba (en ese orden de `BTN_*`).
fn face_buttons(s: &mut Surface, c: (i32, i32), d: i32, buttons: u32, fam: Family, theme: &Theme) {
    let rb = (d / 5).max(7) as f32;
    let o = d as f32 / 2.0 - rb;
    let (cx, cy) = (c.0 as f32, c.1 as f32);
    let spots = [(BTN_SOUTH, cx, cy + o), (BTN_EAST, cx + o, cy), (BTN_WEST, cx - o, cy), (BTN_NORTH, cx, cy - o)];
    for (i, (m, x, y)) in spots.into_iter().enumerate() {
        let lit = buttons & m != 0;
        s.fill_circle(x, y, rb, if lit { theme.accent } else { OFF }, 255);
        s.arc(x, y, rb - 0.5, 0.0, 360.0, 1.2, if lit { theme.accent } else { OFF_LINE });
        let ink = if lit { 0xFFFFFF } else { TEXT };
        match face_labels(fam) {
            Some(names) => {
                let r = Rect::new(x as i32 - 8, y as i32 - 8, 16, 16);
                font::draw_in(s, r, names[i], 9, true, ink, Align::Center);
            }
            // PlayStation: cruz (abajo), círculo (derecha), cuadrado (izquierda) y triángulo (arriba).
            None => {
                let g = rb * 0.5;
                match i {
                    0 => {
                        s.stroke(&[(x - g, y - g), (x + g, y + g)], 1.6, ink);
                        s.stroke(&[(x - g, y + g), (x + g, y - g)], 1.6, ink);
                    }
                    1 => s.arc(x, y, g, 0.0, 360.0, 1.6, ink),
                    2 => s.frame(Rect::new((x - g) as i32, (y - g) as i32, (2.0 * g) as i32, (2.0 * g) as i32), ink),
                    _ => s.stroke(&[(x, y - g), (x + g, y + g * 0.8), (x - g, y + g * 0.8), (x, y - g)], 1.6, ink),
                }
            }
        }
    }
}

impl Settings {
    fn devices_page(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, focused: bool) {
        if self.testing.is_some() {
            // Sin el foco no llegan estados: se da todo por suelto.
            if !focused {
                self.pad_state = PadState::default();
            }
            return self.pad_test(s, theme);
        }
        let x0 = PANEL_W + 24;
        let right = (s.w - SB_W - 8).max(x0 + 1);
        let mut y = if s.h < 440 { 14 } else { 24 } - self.scroll;
        font::draw(s, x0, y, "Dispositivos", 18, false, TEXT);
        y += font::line_h(18) + 12;
        font::draw(s, x0, y, "Mandos", 11, true, TEXT);
        y += font::line_h(11) + 6;
        if self.pads.is_empty() {
            font::draw(s, x0, y, "No hay ningún mando conectado.", 9, false, GRAY);
            y += font::line_h(9) + 6;
        }
        for i in 0..self.pads.len() {
            let p = self.pads[i].clone();
            icons::draw(s, Icon::Mando, x0, y + 2, 32, TEXT);
            let tx = x0 + 44;
            font::draw(s, tx, y, &p.name, 10, true, TEXT);
            font::draw(s, tx, y + font::line_h(10) + 2, kind_text(p.kind), 9, false, GRAY);
            let mut at = (tx, y + font::line_h(10) + font::line_h(9) + 8);
            self.button(s, &mut at, tx, right, "Probar mando", Act::PadTest(i), theme);
            y = at.1 + BTN_H + 14;
        }
        y += 8;
        font::draw(s, x0, y, "Unidades", 11, true, TEXT);
        y += font::line_h(11) + 6;
        for d in sys.fs().drives() {
            icons::draw(s, if d.locked && d.removable { Icon::UsbSeguro } else if d.removable { Icon::Usb } else { Icon::Drive }, x0, y + 2, 32, TEXT);
            let tx = x0 + 44;
            font::draw(s, tx, y, &d.name(), 10, true, TEXT);
            let info = if d.locked {
                String::from("Cifrada: hay que desbloquearla para verla")
            } else {
                let units = super::terminal::units;
                format!("{} · {} libres de {}", crate::drives::kind_name(d.kind), units(d.free), units(d.total))
            };
            font::draw(s, tx, y + font::line_h(10) + 2, &info, 9, false, GRAY);
            y += font::line_h(10) + font::line_h(9) + 16;
        }
        self.content_h = y + self.scroll + 16;
    }

    /// Vista de «Probar mando»: sticks, gatillos, cruceta y botones, que se iluminan al pulsar.
    fn pad_test(&mut self, s: &mut Surface, theme: &Theme) {
        let Some(pad) = self.testing.and_then(|t| self.pads.iter().find(|p| p.id == t)).cloned() else { return };
        let st = self.pad_state;
        let fam = family(pad.kind);
        let compact = s.h < 440;
        let x0 = PANEL_W + 24;
        let right = (s.w - 12).max(x0 + 1);
        let avail = right - x0;
        let on = |m: u32| st.buttons & m != 0;
        let mut y = if compact { 14 } else { 24 };
        font::draw(s, x0, y, "Probar mando", 18, false, TEXT);
        y += font::line_h(18) + 2;
        font::draw(s, x0, y, &pad.name, 9, false, GRAY);
        y += font::line_h(9) + if compact { 6 } else { 10 };
        // Hombros y gatillos (con su recorrido).
        let [lb, rb, lt, rt] = shoulder_labels(fam);
        let half = ((avail - 8) / 2).max(1);
        let cols = [x0, x0 + half + 8];
        chip(s, Rect::new(cols[0], y, half, 16), lb, on(BTN_L1), theme);
        chip(s, Rect::new(cols[1], y, half, 16), rb, on(BTN_R1), theme);
        y += 20;
        for (i, (name, v)) in [(lt, st.axes[AXIS_LT]), (rt, st.axes[AXIS_RT])].into_iter().enumerate() {
            let bar = Rect::new(cols[i] + 26, y, (half - 26).max(1), 14);
            font::draw_in(s, Rect::new(cols[i], y, 24, 14), name, 9, false, TEXT, Align::Left);
            s.fill_rect(bar, OFF);
            s.fill_rect(Rect::new(bar.x, bar.y, (bar.w as i64 * v.max(0) as i64 / 32767) as i32, bar.h), theme.accent);
            s.frame(bar, OFF_LINE);
        }
        y += 14 + if compact { 8 } else { 14 };
        // Sticks, cruceta y rombo: izquierda arriba / derecha abajo, como en un mando de Xbox.
        let d = if compact { 56 } else { 72 };
        let row = d + if compact { 8 } else { 12 };
        let (c1, c2) = (x0 + d / 2 + 4, x0 + half + 8 + d / 2 + 4);
        let mid = y + d / 2;
        stick(s, (c1 as f32, mid as f32), d as f32 / 2.0, (st.axes[AXIS_LX], st.axes[AXIS_LY]), on(BTN_L3), theme);
        face_buttons(s, (c2, mid), d, st.buttons, fam, theme);
        let mid = mid + row;
        dpad(s, (c1, mid), d, st.buttons, theme);
        stick(s, (c2 as f32, mid as f32), d as f32 / 2.0, (st.axes[AXIS_RX], st.axes[AXIS_RY]), on(BTN_R3), theme);
        y += 2 * row + 2;
        // Botones del centro.
        let mut at = (x0, y);
        for (m, name) in center_buttons(fam) {
            let w = font::width(name, 9, false) + 16;
            if at.0 > x0 && at.0 + w > right {
                at = (x0, at.1 + 22);
            }
            chip(s, Rect::new(at.0, at.1, w, 18), name, on(m), theme);
            at.0 += w + 6;
        }
        at = (x0, at.1 + 18 + if compact { 8 } else { 14 });
        self.button(s, &mut at, x0, right, "Volver", Act::PadBack, theme);
    }
}

// ---- Página Cuentas: operaciones ----
impl Settings {
    fn start(&mut self, act: Act, cx: &mut Ctx) {
        match act {
            Act::AppInstall => {
                let dir = cx.sys.fs().std_dir(crate::files::Std::Downloads);
                cx.dialog = Some(DialogRequest::Folder { tag: T_APP_DIR, title: String::from("Instalar programa"), dir });
                return;
            }
            Act::CertAdd => {
                let dir = cx.sys.fs().std_dir(crate::files::Std::Downloads);
                cx.dialog = Some(DialogRequest::Open { tag: T_CERT_FILE, title: String::from("Añadir certificado"), dir });
                return;
            }
            Act::CertDel(i) => {
                let Some(c) = self.certs.get(i) else { return };
                let text = format!("¿Quitar el certificado de {}?\nLos sitios que dependan de él dejarán de ser de confianza.", cert_who(&c.cn, &c.org));
                cx.dialog = Some(DialogRequest::Message { tag: T_CERT_DEL, title: String::from("Certificados"), text, buttons: Buttons::YesNo });
                self.cert_del = Some(c.sha256);
                return;
            }
            Act::AppUninstall(i) => {
                let Some(p) = self.apps.get(i).cloned() else { return };
                let text = format!("¿Desinstalar «{}»?\nSu carpeta irá a la Papelera.", p.name);
                cx.dialog = Some(DialogRequest::Message { tag: T_APP_DEL, title: String::from("Aplicaciones"), text, buttons: Buttons::YesNo });
                self.pending_app = Some(p);
                return;
            }
            Act::PadTest(i) => {
                let Some(p) = self.pads.get(i) else { return };
                self.testing = Some(p.id);
                self.pad_state = PadState::default();
                self.scroll = 0;
                cx.redraw = true;
                return;
            }
            Act::PadBack => {
                self.testing = None;
                cx.redraw = true;
                return;
            }
            Act::TimeAuto | Act::TimeZone(_) => {
                let c = cx.sys.fs().time_cfg();
                let nuevo = match act {
                    Act::TimeZone(i) => crate::hora::Ajuste { zona: i as u8, ..c },
                    _ => crate::hora::Ajuste { auto: !c.auto, ..c },
                };
                let now = cx.sys.now();
                if let Err(e) = cx.sys.fs().set_time_cfg(nuevo, now) {
                    hora_message(cx, e.message());
                }
                cx.redraw = true;
                return;
            }
            Act::TimeSync => {
                if let Err(e) = cx.sys.fs().time_sync() {
                    hora_message(cx, e.message());
                }
                cx.redraw = true;
                return;
            }
            Act::TimeDate | Act::TimeClock => {
                let d = cx.sys.now();
                let (tag, label, value) = if act == Act::TimeDate {
                    (T_DATE, "Fecha (DD/MM/AAAA)", format!("{:02}/{:02}/{}", d.day, d.month, d.year))
                } else {
                    (T_TIME, "Hora (HH:MM o HH:MM:SS)", format!("{:02}:{:02}:{:02}", d.hour, d.minute, d.second))
                };
                cx.dialog = Some(DialogRequest::Input { tag, title: String::from("Fecha y hora"), label: String::from(label), value });
                return;
            }
            Act::ArrMenos | Act::ArrMas | Act::ArrOcultar(_) | Act::ArrNombre(_) | Act::ArrPrimero => return self.arranque_act(act, cx),
            Act::NetRenew => {
                if let Some(n) = cx.sys.net() {
                    n.renew();
                }
                cx.redraw = true;
                return;
            }
            Act::NetOut(i) | Act::NetIn(i) | Act::NetForget(i) => {
                let Some(p) = self.net_perms.get(i) else { return };
                // «Permitir» si no lo estaba; «Bloquear» si lo estaba. Olvidar: vuelve a preguntar.
                let flip = |v: Option<bool>| Some(v != Some(true));
                let (out, inn) = match act {
                    Act::NetOut(_) => (flip(p.out), p.inn),
                    Act::NetIn(_) => (p.out, flip(p.inn)),
                    _ => (None, None),
                };
                let (hash, now) = (p.hash, cx.sys.now());
                if let Err(e) = cx.sys.fs().set_net_perm(hash, out, inn, now) {
                    message(cx, e.message());
                }
                cx.redraw = true;
                return;
            }
            _ => {}
        }
        let crypto = match act {
            Act::HomeEnc => Some(Op::HomeEnc),
            Act::HomeDec => Some(Op::HomeDec),
            Act::HomeCode => Some(Op::HomeCode),
            Act::DiskEnc => Some(Op::DiskEnc),
            Act::DiskDec => Some(Op::DiskDec),
            Act::DiskChange => Some(Op::DiskChange),
            Act::DiskCode => Some(Op::DiskCode),
            _ => None,
        };
        if let Some(op) = crypto {
            return self.start_crypto(op, cx);
        }
        if act == Act::RemovePwd && cx.sys.fs().home_encrypted() {
            return message(cx, AccountError::Encrypted.text());
        }
        let other = |i: usize| self.others.get(i).cloned();
        let (op, name) = match act {
            Act::SetPwd => (Op::SetPwd, String::new()),
            Act::ChangePwd => (Op::ChangePwd, String::new()),
            Act::RemovePwd => (Op::RemovePwd, String::new()),
            Act::Add => (Op::Add, String::new()),
            Act::Rename(i) => match other(i) {
                Some((n, _)) => (Op::Rename, n),
                None => return,
            },
            Act::Delete(i) => match other(i) {
                Some((n, _)) => (Op::Delete, n),
                None => return,
            },
            Act::HomeEnc | Act::HomeDec | Act::HomeCode | Act::DiskEnc | Act::DiskDec | Act::DiskChange | Act::DiskCode | Act::AppInstall | Act::AppUninstall(_) | Act::PadTest(_) | Act::PadBack | Act::NetRenew | Act::NetOut(_) | Act::NetIn(_) | Act::NetForget(_) | Act::CertAdd | Act::CertDel(_) | Act::TimeAuto | Act::TimeSync | Act::TimeZone(_) | Act::TimeDate | Act::TimeClock | Act::ArrMenos | Act::ArrMas | Act::ArrOcultar(_) | Act::ArrNombre(_) | Act::ArrPrimero => return,
            Act::Admin(i) => {
                let Some((n, is_admin)) = other(i) else { return };
                let now = cx.sys.now();
                if let Err(e) = cx.sys.fs().set_admin(&n, !is_admin, now) {
                    message(cx, e.text());
                }
                cx.redraw = true;
                return;
            }
        };
        match act {
            Act::SetPwd => password(cx, T_PW_NEW, "Clave nueva (mínimo 6 caracteres):"),
            Act::ChangePwd | Act::RemovePwd => password(cx, T_PW_OLD, "Clave actual:"),
            Act::Add => input(cx, T_ADD_NAME, "Nombre de la cuenta nueva:", ""),
            Act::Rename(_) => input(cx, T_REN, "Nombre nuevo:", &name),
            _ => {
                cx.dialog = Some(DialogRequest::Message {
                    tag: T_DEL_FILES,
                    title: String::from(TITLE),
                    text: format!("¿Eliminar también sus archivos?\nSí: se borran para siempre.\nNo: se conserva la carpeta de «{name}»."),
                    buttons: Buttons::YesNoCancel,
                })
            }
        }
        self.job = Some(Job { op, name, old: Secret::default(), new: Secret::default(), after: "" });
    }

    /// Cambios de la página Arranque: cada uno se guarda al momento y, si el firmware no deja, se avisa.
    fn arranque_act(&mut self, act: Act, cx: &mut Ctx) {
        cx.redraw = true;
        let Some(info) = self.arr.clone() else { return };
        let mut a = info.ajustes;
        match act {
            Act::ArrMenos => a.segundos = a.segundos.saturating_sub(1),
            Act::ArrMas => a.segundos = (a.segundos + 1).min(30),
            Act::ArrOcultar(i) => {
                let Some((c, _, true)) = info.entradas.get(i) else { return };
                match a.ocultas.iter().position(|x| x == c) {
                    Some(p) => drop(a.ocultas.remove(p)),
                    None => a.ocultas.push(c.clone()),
                }
            }
            Act::ArrNombre(i) => {
                let Some((c, orig, _)) = info.entradas.get(i) else { return };
                let actual = a.nombres.iter().find(|(k, n)| k == c && !n.is_empty()).map_or(orig.as_str(), |(_, n)| n.as_str());
                input(cx, T_ARR_NOM, "Nombre en el menú (vacío = el original):", actual);
                self.arr_nom = Some(c.clone());
                return;
            }
            _ => {
                if let Err(e) = cx.sys.arranque_primero() {
                    arranque_message(cx, e);
                }
                return;
            }
        }
        arranque_guardar(cx, &a);
    }

    /// Primer paso de las operaciones de cifrado: el aviso o la primera clave.
    fn start_crypto(&mut self, op: Op, cx: &mut Ctx) {
        match op {
            Op::HomeEnc => confirm(cx, "Tus archivos quedarán cifrados con una llave que solo abre tu clave. Si olvidas la clave y pierdes el código de recuperación, no se podrán recuperar. ¿Continuar?"),
            Op::HomeDec => confirm(cx, "Tu carpeta se descifrará y sus archivos quedarán sin protección. ¿Continuar?"),
            Op::HomeCode => password(cx, T_SEC, "Tu clave actual:"),
            Op::DiskEnc => confirm(cx, "Se cifrará todo el disco con una clave del disco, distinta de las claves de usuario. Se pedirá cada vez que arranques. Haz antes una copia de seguridad: si olvidas la clave y pierdes el código de recuperación, se pierde todo. ¿Continuar?"),
            Op::DiskDec => confirm(cx, "Se descifrará todo el disco y quedará sin protección. ¿Continuar?"),
            Op::DiskChange => password(cx, T_SEC, "Clave actual del disco:"),
            _ => password(cx, T_SEC, "Clave del disco:"),
        }
        self.job = Some(Job { op, name: String::new(), old: Secret::default(), new: Secret::default(), after: "" });
    }

    /// Pantalla del ratón; al terminar se ejecuta la operación.
    fn entropy(&mut self, job: Job, cx: &mut Ctx) {
        cx.dialog = Some(DialogRequest::Entropy { tag: T_ENT, title: String::from("Generar la llave") });
        self.job = Some(job);
    }

    /// Ejecuta la operación de cifrado con las claves recogidas y las borra al terminar.
    fn run_crypto(&mut self, mut job: Job, cx: &mut Ctx) {
        let now: DateTime = cx.sys.now();
        let (old, new) = (&job.old.0, &job.new.0);
        let r = with_pool(cx, |f, pool| match job.op {
            Op::HomeEnc => f.encrypt_home(old, pool, now).map(Some),
            Op::HomeDec => f.decrypt_home(old, now).map(|()| None),
            Op::HomeCode => f.new_home_recovery(old, pool, now).map(Some),
            Op::DiskEnc => f.encrypt_disk(new, pool, now).map(Some),
            Op::DiskDec => f.decrypt_disk(old, now).map(|()| None),
            Op::DiskChange => f.change_disk_password(old, new, pool, now).map(|()| None),
            Op::DiskCode => f.new_disk_recovery(old, pool, now).map(Some),
            _ => Ok(None),
        });
        job.old = Secret::default();
        job.new = Secret::default();
        match r {
            Err(e) => message(cx, e.text()),
            Ok(None) => message(
                cx,
                match job.op {
                    Op::HomeDec => "Se está descifrando tu carpeta.",
                    Op::DiskDec => "Se está descifrando el disco. Puedes seguir usando el equipo.",
                    _ => "Se ha cambiado la clave del disco.",
                },
            ),
            Ok(Some(code)) => {
                job.after = match job.op {
                    Op::HomeEnc => "Se está cifrando tu carpeta. Puedes seguir usando el equipo.",
                    Op::DiskEnc => "El disco se está cifrando. Puedes seguir usando el equipo.",
                    _ => "",
                };
                show_code(cx, code);
                self.job = Some(job);
            }
        }
    }

    /// Termina la operación de clave con la clave nueva ya repetida (`None` = quitarla).
    fn finish_password(&mut self, mut job: Job, new: Option<&str>, cx: &mut Ctx) {
        let (now, salt) = (cx.sys.now(), salt(cx));
        let r = cx.sys.fs().set_password(&job.old.0, new, salt, now);
        if r.is_ok() && job.op == Op::SetPwd {
            // Recién puesta la clave: se ofrece cifrar la carpeta con ella, sin volver a pedirla.
            job.new = Secret(String::from(new.unwrap_or_default()));
            cx.dialog = Some(DialogRequest::Message {
                tag: T_ASK_HOME,
                title: String::from(TITLE),
                text: String::from("Se ha puesto la clave.\n¿Cifrar también tu carpeta? (Recomendado)"),
                buttons: Buttons::YesNo,
            });
            self.job = Some(job);
            return;
        }
        match r {
            Ok(()) => message(
                cx,
                match job.op {
                    Op::SetPwd => "Se ha puesto la clave.",
                    Op::ChangePwd => "Se ha cambiado la clave.",
                    _ => "Se ha quitado la clave.",
                },
            ),
            Err(e) => message(cx, e.text()),
        }
    }

    fn step_text(&mut self, tag: u32, value: Option<Secret>, cx: &mut Ctx) {
        let Some(mut job) = self.job.take() else { return };
        let Some(v) = value else { return };
        match tag {
            T_PW_OLD => {
                if !cx.sys.fs().verify_password(&v.0) {
                    return message(cx, AccountError::WrongPassword.text());
                }
                if job.op == Op::RemovePwd {
                    job.old = v;
                    return self.finish_password(job, None, cx);
                }
                job.old = v;
                password(cx, T_PW_NEW, "Clave nueva (mínimo 6 caracteres):");
            }
            T_PW_NEW | T_ADD_PW => {
                // Al crear una cuenta, la clave vacía significa "sin clave".
                if tag == T_ADD_PW && v.0.is_empty() {
                    job.new = v;
                    return self.ask_admin(job, cx);
                }
                if v.0.chars().count() < MIN_PASSWORD {
                    return message(cx, AccountError::WeakPassword.text());
                }
                job.new = v;
                password(cx, if tag == T_PW_NEW { T_PW_REP } else { T_ADD_REP }, "Repite la clave:");
            }
            T_PW_REP | T_ADD_REP => {
                if !job.new.same(&v) {
                    return message(cx, NO_MATCH);
                }
                if tag == T_ADD_REP {
                    return self.ask_admin(job, cx);
                }
                let new = Secret(core::mem::take(&mut job.new.0));
                return self.finish_password(job, Some(&new.0), cx);
            }
            T_SEC => {
                job.old = v;
                match job.op {
                    Op::HomeEnc | Op::HomeDec | Op::HomeCode => {
                        if !cx.sys.fs().verify_password(&job.old.0) {
                            return message(cx, AccountError::WrongPassword.text());
                        }
                        return if job.op == Op::HomeDec { self.run_crypto(job, cx) } else { self.entropy(job, cx) };
                    }
                    Op::DiskDec => return self.run_crypto(job, cx),
                    Op::DiskCode => return self.entropy(job, cx),
                    Op::DiskChange => password(cx, T_NEWK, "Clave nueva del disco (mínimo 6 caracteres):"),
                    _ => return,
                }
            }
            T_NEWK => {
                if v.0.chars().count() < MIN_PASSWORD {
                    return message(cx, keys::KeyError::Weak.text());
                }
                job.new = v;
                password(cx, T_REPK, "Repite la clave del disco:");
            }
            T_REPK => {
                if !job.new.same(&v) {
                    return message(cx, NO_MATCH);
                }
                if job.op == Op::DiskChange {
                    return self.run_crypto(job, cx);
                }
                if keys::strength(&job.new.0) < 2 {
                    cx.dialog = Some(DialogRequest::Message {
                        tag: T_WEAK,
                        title: String::from(TITLE),
                        text: String::from("La clave es débil. ¿Usarla de todos modos?"),
                        buttons: Buttons::YesNo,
                    });
                    self.job = Some(job);
                    return;
                }
                return self.entropy(job, cx);
            }
            T_ADD_NAME => {
                if !crate::accounts::valid_user_name(&v.0) {
                    return message(cx, AccountError::BadName.text());
                }
                if cx.sys.fs().accounts().iter().any(|a| same(&a.name, &v.0)) {
                    return message(cx, AccountError::Exists.text());
                }
                job.name = v.0.clone();
                password(cx, T_ADD_PW, "Clave (vacía = sin clave):");
            }
            T_REN => {
                let now = cx.sys.now();
                if let Err(e) = cx.sys.fs().rename_account(&job.name, &v.0, now) {
                    message(cx, e.text());
                }
                return;
            }
            T_DEL_NAME => {
                if v.0 != job.name {
                    return message(cx, "El nombre no coincide. No se ha eliminado nada.");
                }
                return self.delete(&job.name, false, cx);
            }
            _ => return,
        }
        self.job = Some(job);
    }

    fn ask_admin(&mut self, job: Job, cx: &mut Ctx) {
        cx.dialog = Some(DialogRequest::Message {
            tag: T_ADD_ADMIN,
            title: String::from(TITLE),
            text: format!("¿Será «{}» administrador?\nSí: administrador. No: usuario normal.", job.name),
            buttons: Buttons::YesNoCancel,
        });
        self.job = Some(job);
    }

    fn delete(&mut self, name: &str, keep: bool, cx: &mut Ctx) {
        let now = cx.sys.now();
        match cx.sys.fs().delete_account(name, keep, now) {
            Ok(()) => message(cx, if keep { "Cuenta eliminada. Su carpeta se ha conservado." } else { "Cuenta eliminada junto con sus archivos." }),
            Err(e) => message(cx, e.text()),
        }
    }

    fn step_answer(&mut self, tag: u32, answer: Answer, cx: &mut Ctx) {
        let Some(mut job) = self.job.take() else { return };
        if tag == T_CODE {
            if !job.after.is_empty() {
                message(cx, job.after);
            }
            return;
        }
        if answer == Answer::Cancel || (answer == Answer::No && matches!(tag, T_CONF | T_WEAK | T_ASK_HOME)) {
            return;
        }
        match tag {
            T_CONF => {
                match job.op {
                    Op::HomeEnc | Op::HomeDec => password(cx, T_SEC, "Tu clave actual:"),
                    Op::DiskEnc => password(cx, T_NEWK, "Clave nueva del disco (mínimo 6 caracteres):"),
                    Op::DiskDec => password(cx, T_SEC, "Clave del disco:"),
                    _ => return,
                }
                self.job = Some(job);
            }
            T_WEAK => self.entropy(job, cx),
            T_ASK_HOME => {
                job.old = Secret(core::mem::take(&mut job.new.0));
                job.op = Op::HomeEnc;
                self.entropy(job, cx);
            }
            T_ENT => self.run_crypto(job, cx),
            T_ADD_ADMIN => {
                let (now, salt) = (cx.sys.now(), salt(cx));
                let pw = Some(job.new.0.as_str()).filter(|p| !p.is_empty());
                if let Err(e) = cx.sys.fs().create_account(&job.name, pw, answer == Answer::Yes, salt, now) {
                    message(cx, e.text());
                }
            }
            T_DEL_FILES if answer == Answer::No => self.delete(&job.name, true, cx),
            T_DEL_FILES => {
                input(cx, T_DEL_NAME, &format!("Para confirmar, escribe el nombre ({}):", job.name), "");
                self.job = Some(job);
            }
            _ => {}
        }
    }
}

impl App for Settings {
    fn kind(&self) -> AppKind {
        AppKind::Settings
    }
    fn title(&self) -> String {
        String::from("Configuración")
    }
    fn min_size(&self) -> (i32, i32) {
        (520, 360)
    }

    fn draw(&mut self, s: &mut Surface, theme: &Theme, sys: &mut dyn Platform, focused: bool) {
        // Las miniaturas se generan una vez; la de "Color sólido" se rehace si cambia el énfasis.
        if self.thumbs.is_empty() {
            self.thumbs = (0..WALLPAPERS.len() as u8).map(|i| render_wallpaper(i, THUMB.0, THUMB.1, theme.accent)).collect();
            self.thumb_accent = theme.accent;
        } else if self.thumb_accent != theme.accent {
            self.thumbs[3] = render_wallpaper(3, THUMB.0, THUMB.1, theme.accent);
            self.thumb_accent = theme.accent;
        }
        let b = s.bounds();
        self.size = (s.w, s.h);
        // Si el contenido de Cuentas ha encogido, se redibuja ya con el desplazamiento corregido.
        for _ in 0..2 {
            s.fill_rect(b, 0xFFFFFF);
            self.hits.clear();
            self.panel(s, theme);
            // Lo de la derecha no se sale de su zona.
            s.set_clip(Rect::new(PANEL_W, 0, (s.w - PANEL_W).max(0), s.h));
            match self.page {
                0 => self.personalization(s, theme, sys),
                PAGE_ACCOUNTS => self.accounts_page(s, theme, sys),
                PAGE_DISK => self.disk_page(s, theme, sys),
                PAGE_APPS => self.apps_page(s, theme, sys),
                PAGE_DEVICES => self.devices_page(s, theme, sys, focused),
                PAGE_NET => self.net_page(s, theme, sys),
                PAGE_TIME => self.time_page(s, theme, sys),
                PAGE_ARRANQUE => self.arranque_page(s, theme, sys),
                _ => self.about(s, theme, sys),
            }
            s.reset_clip();
            if !self.scrolls() || self.scroll <= self.max_scroll() {
                break;
            }
            self.scroll = self.max_scroll();
        }
        if self.scrolls() && self.metrics().needed() {
            s.set_clip(Rect::new(PANEL_W, 0, (s.w - PANEL_W).max(0), s.h));
            self.sb.draw(s, &self.metrics(), theme);
            s.reset_clip();
        }
    }

    fn key(&mut self, _ev: &KeyEvent, _cx: &mut Ctx) {}

    /// `NET_PAGE` (el icono de red de la bandeja) abre la página Red.
    fn open_path(&mut self, path: &str, cx: &mut Ctx) {
        if path == super::NET_PAGE {
            self.page = PAGE_NET;
            self.scroll = 0;
            self.hits.clear();
            cx.redraw = true;
        }
    }

    fn mouse(&mut self, ev: MouseEv, cx: &mut Ctx) {
        let at = |s: &Self, x: i32, y: i32| s.hits.iter().find(|(r, _)| r.contains(x, y)).map(|h| h.1);
        if self.scrolls() {
            if let MouseEv::Wheel { dz, .. } = ev {
                let to = (self.scroll + dz * STEP * 2).clamp(0, self.max_scroll());
                if to != self.scroll {
                    self.scroll = to;
                    cx.redraw = true;
                }
                return;
            }
            let m = self.metrics();
            if m.needed() {
                let e = self.sb.mouse(ev, &m, cx.now_ms);
                if let Some(p) = e.pos {
                    self.scroll = p;
                }
                cx.redraw |= e.redraw;
                if e.captured {
                    return;
                }
            }
        }
        match ev {
            MouseEv::RightDown { .. } | MouseEv::Wheel { .. } => {}
            MouseEv::Move { x, y } => {
                let (h, b) = match at(self, x, y) {
                    Some(Hit::Nav(i)) => (Some(i), None),
                    Some(Hit::Act(a)) => (None, Some(a)),
                    _ => (None, None),
                };
                if h != self.hover || b != self.hover_hit {
                    self.hover = h;
                    self.hover_hit = b;
                    cx.redraw = true;
                }
            }
            MouseEv::Leave => {
                if self.hover.take().is_some() | self.hover_hit.take().is_some() {
                    cx.redraw = true;
                }
            }
            MouseEv::Up { .. } => {}
            MouseEv::Down { x, y } => match at(self, x, y) {
                Some(Hit::Nav(i)) if i != self.page => {
                    self.page = i;
                    self.hits.clear();
                    self.scroll = 0;
                    cx.redraw = true;
                }
                Some(Hit::Act(a)) => self.start(a, cx),
                Some(Hit::Wall(i)) if cx.theme.wallpaper != i => {
                    cx.theme.wallpaper = i;
                    cx.theme_changed = true;
                    cx.redraw = true;
                }
                Some(Hit::Letra(v)) if cx.theme.letra != v => {
                    cx.theme.letra = v;
                    cx.theme_changed = true;
                    cx.redraw = true;
                }
                Some(Hit::Accent(i)) if cx.theme.accent != ACCENTS[i] => {
                    cx.theme.accent = ACCENTS[i];
                    cx.theme_changed = true;
                    cx.redraw = true;
                }
                _ => {}
            },
        }
    }

    fn gamepad(&mut self, id: u32, state: &PadState, cx: &mut Ctx) {
        if self.testing == Some(id) && self.pad_state != *state {
            self.pad_state = *state;
            cx.redraw = true;
        }
    }

    fn gamepads(&mut self, list: &[PadInfo], cx: &mut Ctx) {
        self.pads = list.to_vec();
        // Si el mando que se probaba se ha desconectado, vuelve a la lista.
        if self.testing.is_some_and(|t| !self.pads.iter().any(|p| p.id == t)) {
            self.testing = None;
        }
        cx.redraw |= self.page == PAGE_DEVICES;
    }

    fn dialog_result(&mut self, result: DialogResult, cx: &mut Ctx) {
        cx.redraw = true;
        match result {
            DialogResult::Path { tag: T_APP_DIR, path: Some(src) } => self.app_source(&src, cx),
            DialogResult::Answer { tag: T_APP_REPLACE | T_APP_DEL, answer } => {
                let Some(p) = self.pending_app.take() else { return };
                if answer != Answer::Yes {
                    return;
                }
                let now = cx.sys.now();
                let r = if p.dir.to_lowercase().starts_with(&crate::files::PROGRAMS_DIR.to_lowercase()) {
                    programs::uninstall(cx.sys.fs(), &p, now).map(|_| format!("«{}» se ha desinstalado.", p.name))
                } else {
                    programs::install(cx.sys.fs(), &p, now).map(|_| format!("«{}» se ha instalado. Está en el menú Inicio.", p.name))
                };
                app_message(cx, &r.unwrap_or_else(|e| e));
            }
            DialogResult::Path { tag: T_CERT_FILE, path: Some(src) } => self.cert_file(&src, cx),
            DialogResult::Answer { tag: T_CERT_ADD, answer } => {
                let Some(der) = self.cert_der.take() else { return };
                if answer == Answer::Yes {
                    let now = cx.sys.now();
                    let text = match cx.sys.fs().add_root(der, now) {
                        Ok(()) => "El certificado se ha añadido.",
                        Err(FsError::Exists) => "Ese certificado ya está instalado.",
                        Err(FsError::Denied) => "Solo un administrador puede añadir certificados.",
                        Err(_) => "No se pudo guardar el certificado.",
                    };
                    cert_message(cx, text);
                }
            }
            DialogResult::Answer { tag: T_CERT_DEL, answer } => {
                let Some(sha) = self.cert_del.take() else { return };
                if answer == Answer::Yes {
                    let now = cx.sys.now();
                    if cx.sys.fs().remove_root(sha, now).is_err() {
                        cert_message(cx, "No se pudo quitar el certificado.");
                    }
                }
            }
            DialogResult::Text { tag: T_DATE | T_TIME, value: Some(v) } => {
                let mut d = cx.sys.now();
                if let Some((y, m, dd)) = parse_fecha(&v) {
                    (d.year, d.month, d.day) = (y, m, dd);
                } else if let Some((h, mi, sec)) = parse_hora(&v) {
                    (d.hour, d.minute, d.second) = (h, mi, sec);
                } else {
                    return hora_message(cx, "No se entiende. Escribe la fecha como 24/11/2026 o la hora como 21:30.");
                }
                if let Err(e) = cx.sys.fs().set_clock(d) {
                    hora_message(cx, if e == FsError::Invalid { "Esa fecha u hora no existe." } else { e.message() });
                }
            }
            DialogResult::Text { tag: T_DATE | T_TIME, value: None } => {}
            DialogResult::Text { tag: T_ARR_NOM, value } => {
                let (Some(v), Some(c), Some(info)) = (value, self.arr_nom.take(), self.arr.clone()) else { return };
                let v = v.trim();
                if v.chars().count() > arranque::MAX_NOMBRE {
                    return arranque_message(cx, "El nombre puede tener hasta 32 letras.");
                }
                let mut a = info.ajustes;
                a.nombres.retain(|(k, _)| *k != c);
                if !v.is_empty() {
                    a.nombres.push((c, String::from(v)));
                }
                arranque_guardar(cx, &a);
            }
            DialogResult::Text { tag, value } => self.step_text(tag, value.map(Secret), cx),
            DialogResult::Answer { tag, answer } => self.step_answer(tag, answer, cx),
            DialogResult::Path { .. } => {}
        }
    }

    fn tick(&mut self, cx: &mut Ctx) {
        if self.scrolls() {
            let m = self.metrics();
            if let Some(p) = self.sb.tick(&m, cx.now_ms) {
                self.scroll = p;
                cx.redraw = true;
            }
        }
        // "Tiempo encendido" y memoria cambian solos: redibuja una vez por segundo.
        let sec = cx.now_ms / 1000;
        // El avance de las conversiones de cifrado también cambia solo.
        let moving = match self.page {
            PAGE_ABOUT | PAGE_DISK | PAGE_DEVICES | PAGE_TIME => true,
            PAGE_ACCOUNTS => cx.sys.fs().home_conversion().is_some(),
            _ => false,
        };
        // La página Red se pone al día sola cuando cambia el estado de la red.
        if self.page == PAGE_NET && cx.sys.net().map(|n| n.status()) != self.net_seen {
            cx.redraw = true;
        }
        if moving && sec != self.last_sec {
            self.last_sec = sec;
            cx.redraw = true;
        }
    }
}

impl Settings {
    /// Carpeta elegida para instalar: se comprueba y, si ya hay uno con ese nombre, se pregunta.
    fn app_source(&mut self, src: &str, cx: &mut Ctx) {
        let p = match programs::check_source(cx.sys.fs(), src) {
            Ok(p) => p,
            Err(e) => return app_message(cx, &format!("No se puede instalar.\n{e}")),
        };
        match programs::installed_as(cx.sys.fs(), &p.name) {
            Some(true) => app_message(cx, &format!("«{}» es un programa del sistema y no se puede reemplazar.", p.name)),
            Some(false) => {
                let text = format!("Ya hay un programa «{}» instalado.\n¿Reemplazarlo por esta versión?", p.name);
                cx.dialog = Some(DialogRequest::Message { tag: T_APP_REPLACE, title: String::from("Aplicaciones"), text, buttons: Buttons::YesNo });
                self.pending_app = Some(p);
            }
            None => {
                let now = cx.sys.now();
                let r = programs::install(cx.sys.fs(), &p, now).map(|_| format!("«{}» se ha instalado. Está en el menú Inicio.", p.name));
                app_message(cx, &r.unwrap_or_else(|e| e));
            }
        }
    }
}

fn arranque_message(cx: &mut Ctx, text: &str) {
    cx.dialog = Some(DialogRequest::Message { tag: T_MSG, title: String::from("Arranque"), text: String::from(text), buttons: Buttons::Ok });
}

fn arranque_guardar(cx: &mut Ctx, a: &arranque::Ajustes) {
    if let Err(e) = cx.sys.arranque_guardar(a) {
        arranque_message(cx, e);
    }
}

fn hora_message(cx: &mut Ctx, text: &str) {
    cx.dialog = Some(DialogRequest::Message { tag: T_MSG, title: String::from("Fecha y hora"), text: String::from(text), buttons: Buttons::Ok });
}

/// «24/11/2026» → (año, mes, día).
fn parse_fecha(t: &str) -> Option<(u16, u8, u8)> {
    let mut p = t.trim().split('/');
    let (d, m, y) = (p.next()?.trim().parse().ok()?, p.next()?.trim().parse().ok()?, p.next()?.trim().parse().ok()?);
    p.next().is_none().then_some((y, m, d))
}

/// «21:30» o «21:30:15» → (hora, minuto, segundo).
fn parse_hora(t: &str) -> Option<(u8, u8, u8)> {
    let mut p = t.trim().split(':');
    let (h, m) = (p.next()?.trim().parse().ok()?, p.next()?.trim().parse().ok()?);
    let s = match p.next() {
        Some(s) => s.trim().parse().ok()?,
        None => 0,
    };
    p.next().is_none().then_some((h, m, s))
}

fn cert_message(cx: &mut Ctx, text: &str) {
    cx.dialog = Some(DialogRequest::Message { tag: T_MSG, title: String::from("Certificados"), text: String::from(text), buttons: Buttons::Ok });
}

impl Settings {
    /// Archivo elegido para añadir: se comprueba y se enseña a quién pertenece antes de confiar en él.
    fn cert_file(&mut self, src: &str, cx: &mut Ctx) {
        let Ok(bytes) = cx.sys.fs().read(src) else { return cert_message(cx, "No se pudo leer el archivo.") };
        let (der, c) = match crate::raices::check(&bytes) {
            Ok(r) => r,
            Err(e) => return cert_message(cx, e.mensaje()),
        };
        if cx.sys.fs().custom_roots().iter().any(|d| crate::crypto::sha256(d) == c.sha256) {
            return cert_message(cx, "Ese certificado ya está instalado.");
        }
        let text = format!(
            "¿Confiar en este certificado?\n\nPertenece a: {}\nEmitido por: {}\nCaduca el: {}\nHuella SHA-256:\n{}\n{}\n\nConfiar en una autoridad permite a quien la controla hacerse pasar por cualquier sitio.",
            cert_who(&c.cn, &c.org),
            cert_who(&c.issuer.cn, &c.issuer.org),
            cert_date(&c.not_after),
            cert_fp(&c.sha256, 0, 8),
            cert_fp(&c.sha256, 8, 8),
        );
        cx.dialog = Some(DialogRequest::Message { tag: T_CERT_ADD, title: String::from("Certificados"), text, buttons: Buttons::YesNo });
        self.cert_der = Some(der);
    }
}

fn app_message(cx: &mut Ctx, text: &str) {
    cx.dialog = Some(DialogRequest::Message { tag: T_MSG, title: String::from("Aplicaciones"), text: String::from(text), buttons: Buttons::Ok });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::TestPlatform;

    struct Banco {
        app: Settings,
        sys: TestPlatform,
        theme: Theme,
        redraw: bool,
        changed: bool,
        /// Último diálogo pedido por la app (se queda hasta que se coge con `take`).
        dialog: Option<DialogRequest>,
    }

    #[test]
    fn aplicaciones_instalar_reemplazar_y_desinstalar() {
        use crate::nfx::{encode, Kind, Section};
        let mut b = Banco::new();
        b.app.page = PAGE_APPS;
        let n = b.sys.now();
        let base = crate::abi::USER_BASE + 0x40_0000;
        let nfx = encode(base, &[Section { kind: Kind::Code, vaddr: base, memsz: 4, data: &[0x90; 4] }]);
        let src = String::from("C:\\Usuarios\\Usuario\\Descargas\\Juego");
        b.sys.files.mkdir(&src, n).unwrap();
        b.sys.files.write(&format!("{src}\\app.txt"), b"nombre=Juego\nversion=1.0\nautor=Ana\n", n).unwrap();
        b.sys.files.write(&format!("{src}\\programa.nfx"), &nfx, n).unwrap();
        // Instalar: elegir carpeta y listo.
        b.pulsa(Act::AppInstall);
        assert!(matches!(b.dialog.take(), Some(DialogRequest::Folder { tag: T_APP_DIR, .. })));
        b.responde(DialogResult::Path { tag: T_APP_DIR, path: Some(src.clone()) });
        assert!(matches!(&b.dialog, Some(DialogRequest::Message { text, .. }) if text.contains("se ha instalado")));
        b.dibuja(780, 520);
        assert_eq!(b.app.apps.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Juego"]);
        // Otra vez: pregunta si se reemplaza.
        b.responde(DialogResult::Path { tag: T_APP_DIR, path: Some(src.clone()) });
        assert!(matches!(&b.dialog, Some(DialogRequest::Message { tag: T_APP_REPLACE, .. })));
        b.respuesta(T_APP_REPLACE, Answer::No);
        assert!(b.dialog.is_none());
        // Una carpeta que no es un programa.
        b.responde(DialogResult::Path { tag: T_APP_DIR, path: Some(String::from("C:\\Usuarios\\Usuario\\Documentos")) });
        assert!(matches!(&b.dialog, Some(DialogRequest::Message { text, .. }) if text.contains("app.txt")));
        // Desinstalar con confirmación.
        b.pulsa(Act::AppUninstall(0));
        assert!(matches!(&b.dialog, Some(DialogRequest::Message { tag: T_APP_DEL, .. })));
        b.respuesta(T_APP_DEL, Answer::Yes);
        assert!(matches!(&b.dialog, Some(DialogRequest::Message { text, .. }) if text.contains("desinstalado")));
        b.dibuja(780, 520);
        assert!(b.app.apps.is_empty());
    }

    impl Banco {
        fn new() -> Self {
            Banco { app: init(), sys: TestPlatform { size: (1280, 800), ..Default::default() }, theme: Theme::default(), redraw: false, changed: false, dialog: None }
        }
        fn dibuja(&mut self, w: i32, h: i32) -> Surface {
            let mut s = Surface::new(w, h, 0);
            self.app.draw(&mut s, &self.theme, &mut self.sys, true);
            s
        }
        fn ev(&mut self, ev: MouseEv) {
            let mut cx = Ctx::new(&mut self.sys, &mut self.theme, 0);
            self.app.mouse(ev, &mut cx);
            self.redraw |= cx.redraw;
            self.changed |= cx.theme_changed;
            if cx.dialog.is_some() {
                self.dialog = cx.dialog.take();
            }
        }
        /// Contesta al diálogo pendiente.
        fn responde(&mut self, r: DialogResult) {
            self.dialog = None;
            let mut cx = Ctx::new(&mut self.sys, &mut self.theme, 0);
            self.app.dialog_result(r, &mut cx);
            self.dialog = cx.dialog.take();
        }
        fn texto(&mut self, tag: u32, v: &str) {
            self.responde(DialogResult::Text { tag, value: Some(String::from(v)) });
        }
        fn respuesta(&mut self, tag: u32, answer: Answer) {
            self.responde(DialogResult::Answer { tag, answer });
        }
        /// Pulsa el botón `act` de la página de cuentas (tras dibujarla).
        fn pulsa(&mut self, act: Act) {
            self.dibuja(780, 520);
            let r = self.app.hits.iter().find(|h| h.1 == Hit::Act(act)).unwrap_or_else(|| panic!("no hay botón {act:?}")).0;
            self.clic(r.x + r.w / 2, r.y + r.h / 2);
        }
        /// Como `pulsa` pero sin volver a dibujar: la pantalla se ha quedado con lo que se vio antes.
        fn pulsa_viejo(&mut self, act: Act) {
            let r = self.app.hits.iter().find(|h| h.1 == Hit::Act(act)).unwrap_or_else(|| panic!("no hay botón {act:?}")).0;
            self.clic(r.x + r.w / 2, r.y + r.h / 2);
        }
        fn hay(&mut self, act: Act) -> bool {
            self.dibuja(780, 520);
            self.app.hits.iter().any(|h| h.1 == Hit::Act(act))
        }
        fn pagina(&mut self, i: usize) {
            self.dibuja(780, 520);
            self.clic(50, 56 + 40 * i as i32 + 5);
        }
        fn mensaje(&mut self) -> String {
            match self.dialog.take() {
                Some(DialogRequest::Message { text, tag: T_MSG, .. }) => text,
                otro => panic!("se esperaba un mensaje y hay {otro:?}"),
            }
        }
        /// Posición de la cuenta `name` en la lista de "Otras cuentas".
        fn fila(&mut self, name: &str) -> usize {
            self.dibuja(780, 520);
            self.app.others.iter().position(|o| o.0 == name).unwrap_or_else(|| panic!("no está {name}"))
        }
        fn files(&mut self) -> &mut crate::files::Files {
            self.sys.files()
        }
        /// Disco persistente con la administradora Ana y el usuario normal Luis; la sesión es de `quien`,
        /// con la página de cuentas abierta.
        fn con_disco(quien: &str) -> Self {
            let n = crate::platform::DateTime::default();
            let mut f = crate::files::Files::probe(Some(Box::new(crate::block::RamDisk::new(2048))), n);
            f.format_pending(n).unwrap();
            f.create_account("Ana", None, true, [1; 16], n).unwrap();
            f.login("Ana", "").unwrap();
            f.create_account("Luis", None, false, [2; 16], n).unwrap();
            f.login(quien, "").unwrap();
            let mut b = Banco::new();
            b.sys.files = f;
            b.pagina(PAGE_ACCOUNTS);
            b.dibuja(780, 520);
            b
        }
        fn clic(&mut self, x: i32, y: i32) {
            self.redraw = false;
            self.changed = false;
            self.ev(MouseEv::Down { x, y });
            self.ev(MouseEv::Up { x, y });
        }
    }

    #[test]
    fn muestra_cambia_acento() {
        let mut b = Banco::new();
        b.dibuja(780, 520);
        // Posición de la muestra 5: buscada por el dibujo (color único en la zona).
        let s = b.dibuja(780, 520);
        let (x, y) = (0..s.h).flat_map(|y| (PANEL_W..s.w).map(move |x| (x, y))).find(|&(x, y)| s.px[(y * s.w + x) as usize] == ACCENTS[5]).unwrap();
        b.clic(x + 2, y + 2);
        assert_eq!(b.theme.accent, ACCENTS[5]);
        assert!(b.changed && b.redraw);
    }

    #[test]
    fn letra_trazos_se_elige_y_se_vuelve() {
        let mut b = Banco::new();
        b.dibuja(780, 520);
        let r = b.app.hits.iter().find(|h| h.1 == Hit::Letra(letra::TRAZOS)).expect("falta el botón Trazos").0;
        b.clic(r.x + 2, r.y + 2);
        assert_eq!(b.theme.letra, letra::TRAZOS);
        assert!(b.changed && b.redraw);
        // Con trazos ya no se ofrece elegir fuente; el botón TrueType devuelve a Liberation.
        b.dibuja(780, 520);
        assert!(b.app.hits.iter().filter(|h| matches!(h.1, Hit::Letra(_))).count() == 2);
        let r = b.app.hits.iter().find(|h| h.1 == Hit::Letra(letra::LIBERATION)).expect("falta el botón TrueType").0;
        b.clic(r.x + 2, r.y + 2);
        assert_eq!(b.theme.letra, letra::LIBERATION);
    }

    #[test]
    fn miniatura_cambia_fondo() {
        let mut b = Banco::new();
        b.dibuja(780, 520);
        // Miniatura del fondo 1: en la primera fila, segunda columna.
        b.clic(PANEL_W + 24 + 136 + 10, 24 + font::line_h(18) + 10 + font::line_h(11) + 4 + 10);
        assert_eq!(b.theme.wallpaper, 1);
        assert!(b.changed);
    }

    #[test]
    fn cambio_de_seccion() {
        let mut b = Banco::new();
        b.dibuja(780, 520);
        b.ev(MouseEv::Move { x: 50, y: 56 + 120 + 5 });
        assert!(b.redraw);
        b.clic(50, 56 + 120 + 5);
        assert!(b.redraw && !b.changed);
        let s = b.dibuja(780, 520);
        // En "Acerca de" ya no hay muestras de color en la zona de contenido.
        assert!(!s.px.iter().any(|&p| p == ACCENTS[6]));
        b.clic(50, 56 + 5);
        let s = b.dibuja(780, 520);
        assert!(s.px.iter().any(|&p| p == ACCENTS[6]));
    }

    #[test]
    fn tick_solo_en_acerca_de() {
        let mut b = Banco::new();
        b.dibuja(780, 520);
        let mut cx = Ctx::new(&mut b.sys, &mut b.theme, 5000);
        b.app.tick(&mut cx);
        assert!(!cx.redraw);
        drop(cx);
        b.clic(50, 56 + 120 + 5);
        b.dibuja(780, 520);
        let mut cx = Ctx::new(&mut b.sys, &mut b.theme, 6000);
        b.app.tick(&mut cx);
        assert!(cx.redraw);
        cx.redraw = false;
        b.app.tick(&mut cx);
        assert!(!cx.redraw);
    }

    fn mando(id: u32, kind: PadKind) -> PadInfo {
        PadInfo { id, kind, name: format!("Mando {id}") }
    }

    impl Banco {
        fn mandos(&mut self, lista: &[PadInfo]) {
            let mut cx = Ctx::new(&mut self.sys, &mut self.theme, 0);
            self.app.gamepads(lista, &mut cx);
            self.redraw |= cx.redraw;
        }
        fn estado(&mut self, id: u32, st: PadState) {
            self.redraw = false;
            let mut cx = Ctx::new(&mut self.sys, &mut self.theme, 0);
            self.app.gamepad(id, &st, &mut cx);
            self.redraw |= cx.redraw;
        }
    }

    #[test]
    fn dispositivos_lista_y_prueba_del_mando() {
        let mut b = Banco::new();
        b.pagina(PAGE_DEVICES);
        // Sin mandos: no hay botón de probar y se dibuja igual.
        b.dibuja(780, 520);
        assert!(!b.app.hits.iter().any(|h| matches!(h.1, Hit::Act(Act::PadTest(_)))));
        b.mandos(&[mando(1, PadKind::Xbox360), mando(2, PadKind::DualSense)]);
        assert!(b.redraw);
        assert!(b.hay(Act::PadTest(0)) && b.hay(Act::PadTest(1)));
        // Probar el segundo: la vista de prueba tiene «Volver» y ya no la lista.
        b.pulsa(Act::PadTest(1));
        assert_eq!(b.app.testing, Some(2));
        assert!(b.hay(Act::PadBack) && !b.hay(Act::PadTest(1)));
        // Un botón pulsado ilumina con el color de acento; el estado de otro mando no cuenta.
        let acento = b.theme.accent;
        let cuenta = |s: &Surface| s.px.iter().filter(|&&p| p == acento).count();
        let suelto = cuenta(&b.dibuja(780, 520));
        b.estado(1, PadState { buttons: BTN_SOUTH, axes: [0; 6] });
        assert!(!b.redraw && b.app.pad_state == PadState::default());
        let pulsado = PadState { buttons: BTN_SOUTH | BTN_UP | BTN_L1, axes: [-32768, 0, 0, 32767, 0, 32767] };
        b.estado(2, pulsado);
        assert!(b.redraw && b.app.pad_state == pulsado);
        assert!(cuenta(&b.dibuja(780, 520)) > suelto);
        // El mismo estado no pide redibujar.
        b.estado(2, pulsado);
        assert!(!b.redraw);
        // Si se desconecta mientras se prueba, vuelve a la lista.
        b.mandos(&[mando(1, PadKind::Xbox360)]);
        assert_eq!(b.app.testing, None);
        assert!(b.hay(Act::PadTest(0)));
        // Volver desde la prueba.
        b.pulsa(Act::PadTest(0));
        b.pulsa(Act::PadBack);
        assert_eq!(b.app.testing, None);
    }

    #[test]
    fn dispositivos_dibuja_cada_familia_y_tamano() {
        let mut b = Banco::new();
        b.pagina(PAGE_DEVICES);
        let todos = [PadKind::Generic, PadKind::DualShock4, PadKind::DualSense, PadKind::SwitchPro, PadKind::Xbox360, PadKind::XboxOne];
        let lista: Vec<PadInfo> = todos.iter().enumerate().map(|(i, k)| mando(i as u32, *k)).collect();
        b.mandos(&lista);
        for i in 0..lista.len() {
            b.pulsa(Act::PadTest(i));
            b.estado(i as u32, PadState { buttons: u32::MAX, axes: [i16::MIN, i16::MAX, i16::MAX, i16::MIN, i16::MAX, i16::MAX] });
            for (w, h) in [(520, 360), (780, 520), (1, 1), (0, 0), (221, 400), (600, 1)] {
                b.dibuja(w, h);
            }
            b.pulsa(Act::PadBack);
        }
    }

    /// Para mirar la vista «Probar mando» sin mando: `NF_CAPTURA=<carpeta> cargo test -p nf-core captura_probar_mando -- --ignored`.
    #[test]
    #[ignore]
    fn captura_probar_mando() {
        let dir = std::env::var("NF_CAPTURA").unwrap_or_else(|_| String::from("."));
        let mut b = Banco::new();
        b.pagina(PAGE_DEVICES);
        let todos = [PadKind::Xbox360, PadKind::DualSense, PadKind::SwitchPro, PadKind::Generic];
        let lista: Vec<PadInfo> = todos.iter().enumerate().map(|(i, k)| mando(i as u32, *k)).collect();
        b.mandos(&lista);
        std::fs::write(format!("{dir}/mandos-lista.bmp"), crate::bmp::encode(&b.dibuja(780, 520))).unwrap();
        for i in 0..lista.len() {
            b.pulsa(Act::PadTest(i));
            b.estado(i as u32, PadState { buttons: BTN_SOUTH | BTN_UP | BTN_L1 | BTN_START, axes: [-20000, 0, 12000, 25000, 0, 30000] });
            std::fs::write(format!("{dir}/mando-{i}.bmp"), crate::bmp::encode(&b.dibuja(780, 520))).unwrap();
            b.pulsa(Act::PadBack);
        }
    }

    #[test]
    fn sticks_y_etiquetas_por_familia() {
        assert_eq!(stick_dot(0, 0), (0.0, 0.0));
        assert_eq!(stick_dot(32767, 0), (1.0, 0.0));
        // En la esquina no se sale del círculo.
        let (x, y) = stick_dot(i16::MAX, i16::MAX);
        assert!(x * x + y * y <= 1.001);
        assert_eq!(face_labels(family(PadKind::SwitchPro)), Some(["B", "A", "Y", "X"]));
        assert_eq!(face_labels(family(PadKind::XboxOne)), Some(["A", "B", "X", "Y"]));
        assert_eq!(face_labels(family(PadKind::Generic)), Some(["1", "2", "3", "4"]));
        assert_eq!(face_labels(family(PadKind::DualShock4)), None);
        assert_eq!(shoulder_labels(Family::Nintendo)[2], "ZL");
    }

    #[test]
    fn tamanos_extremos() {
        let mut b = Banco::new();
        for pagina in 0..6 {
            b.clic(50, 56 + 40 * pagina + 5);
            for (w, h) in [(520, 360), (780, 520), (1, 1), (0, 0), (221, 400), (600, 1)] {
                b.dibuja(w, h);
            }
            b.ev(MouseEv::Leave);
        }
        // Color sólido se regenera con el nuevo énfasis sin pánico.
        b.theme.accent = ACCENTS[3];
        b.dibuja(520, 360);
    }

    fn pw_dialog(b: &Banco, tag: u32) {
        assert!(matches!(&b.dialog, Some(DialogRequest::Password { tag: t, .. }) if *t == tag), "{:?}", b.dialog);
    }

    #[test]
    fn poner_cambiar_y_quitar_la_clave() {
        let mut b = Banco::con_disco("Ana");
        assert!(b.hay(Act::SetPwd) && !b.hay(Act::ChangePwd) && !b.hay(Act::RemovePwd));
        // Poner.
        b.pulsa(Act::SetPwd);
        pw_dialog(&b, T_PW_NEW);
        b.texto(T_PW_NEW, "secreta1");
        pw_dialog(&b, T_PW_REP);
        b.texto(T_PW_REP, "secreta1");
        assert!(b.files().has_password() && b.files().verify_password("secreta1"));
        assert!(matches!(&b.dialog, Some(DialogRequest::Message { tag: T_ASK_HOME, buttons: Buttons::YesNo, text, .. }) if text.contains("Se ha puesto la clave")));
        b.respuesta(T_ASK_HOME, Answer::No);
        assert!(b.dialog.is_none() && b.app.job.is_none(), "las claves se sueltan al terminar");
        // Cambiar: la clave actual mala corta el proceso.
        assert!(b.hay(Act::ChangePwd) && b.hay(Act::RemovePwd) && !b.hay(Act::SetPwd));
        b.pulsa(Act::ChangePwd);
        pw_dialog(&b, T_PW_OLD);
        b.texto(T_PW_OLD, "mala-clave");
        assert_eq!(b.mensaje(), AccountError::WrongPassword.text());
        assert!(b.app.job.is_none() && b.files().verify_password("secreta1"));
        b.pulsa(Act::ChangePwd);
        b.texto(T_PW_OLD, "secreta1");
        pw_dialog(&b, T_PW_NEW);
        b.texto(T_PW_NEW, "otra-clave");
        pw_dialog(&b, T_PW_REP);
        b.texto(T_PW_REP, "otra-clave");
        assert_eq!(b.mensaje(), "Se ha cambiado la clave.");
        assert!(b.files().verify_password("otra-clave") && !b.files().verify_password("secreta1"));
        // Quitar.
        b.pulsa(Act::RemovePwd);
        b.texto(T_PW_OLD, "otra-clave");
        assert_eq!(b.mensaje(), "Se ha quitado la clave.");
        assert!(!b.files().has_password() && b.files().verify_password(""));
        assert!(b.hay(Act::SetPwd));
    }

    #[test]
    fn claves_que_no_coinciden_cortas_o_canceladas_no_cambian_nada() {
        let mut b = Banco::con_disco("Ana");
        b.pulsa(Act::SetPwd);
        b.texto(T_PW_NEW, "secreta1");
        b.texto(T_PW_REP, "secreta2");
        assert_eq!(b.mensaje(), "Las claves no coinciden.");
        assert!(!b.files().has_password() && b.app.job.is_none());
        b.pulsa(Act::SetPwd);
        b.texto(T_PW_NEW, "corta");
        assert_eq!(b.mensaje(), AccountError::WeakPassword.text());
        assert!(!b.files().has_password() && b.app.job.is_none());
        // Cancelar en cualquier paso.
        b.pulsa(Act::SetPwd);
        b.responde(DialogResult::Text { tag: T_PW_NEW, value: None });
        assert!(b.dialog.is_none() && b.app.job.is_none());
        b.pulsa(Act::SetPwd);
        b.texto(T_PW_NEW, "secreta1");
        b.responde(DialogResult::Text { tag: T_PW_REP, value: None });
        assert!(b.dialog.is_none() && b.app.job.is_none() && !b.files().has_password());
        // Con clave puesta, cancelar la actual tampoco cambia nada.
        let n = crate::platform::DateTime::default();
        b.files().set_password("", Some("secreta1"), [3; 16], n).unwrap();
        b.pulsa(Act::RemovePwd);
        b.responde(DialogResult::Text { tag: T_PW_OLD, value: None });
        assert!(b.dialog.is_none() && b.files().has_password());
    }

    #[test]
    fn anadir_cuenta_con_y_sin_clave_y_con_errores() {
        let mut b = Banco::con_disco("Ana");
        // Sin clave y administradora.
        b.pulsa(Act::Add);
        assert!(matches!(&b.dialog, Some(DialogRequest::Input { tag: T_ADD_NAME, .. })));
        b.texto(T_ADD_NAME, "Marta");
        pw_dialog(&b, T_ADD_PW);
        b.texto(T_ADD_PW, "");
        assert!(matches!(&b.dialog, Some(DialogRequest::Message { tag: T_ADD_ADMIN, buttons: Buttons::YesNoCancel, .. })));
        b.respuesta(T_ADD_ADMIN, Answer::Yes);
        assert!(b.dialog.is_none());
        let marta = b.files().accounts().into_iter().find(|a| a.name == "Marta").unwrap();
        assert!(marta.admin && !marta.has_password());
        assert!(b.files().exists("C:\\Usuarios\\Marta\\Documentos"));
        // Con clave y usuaria normal.
        b.pulsa(Act::Add);
        b.texto(T_ADD_NAME, "Pepe");
        b.texto(T_ADD_PW, "clavepepe");
        pw_dialog(&b, T_ADD_REP);
        b.texto(T_ADD_REP, "clavepepe");
        b.respuesta(T_ADD_ADMIN, Answer::No);
        let pepe = b.files().accounts().into_iter().find(|a| a.name == "Pepe").unwrap();
        assert!(!pepe.admin && pepe.has_password() && pepe.verify("clavepepe") && !pepe.verify(""));
        // Errores: nombre mal, repetido (sin distinguir mayúsculas), claves distintas, corta y cancelar al final.
        b.pulsa(Act::Add);
        b.texto(T_ADD_NAME, "a|b");
        assert_eq!(b.mensaje(), AccountError::BadName.text());
        b.pulsa(Act::Add);
        b.texto(T_ADD_NAME, "luis");
        assert_eq!(b.mensaje(), AccountError::Exists.text());
        b.pulsa(Act::Add);
        b.texto(T_ADD_NAME, "Nora");
        b.texto(T_ADD_PW, "clavenora");
        b.texto(T_ADD_REP, "otraclave");
        assert_eq!(b.mensaje(), "Las claves no coinciden.");
        b.pulsa(Act::Add);
        b.texto(T_ADD_NAME, "Nora");
        b.texto(T_ADD_PW, "corta");
        assert_eq!(b.mensaje(), AccountError::WeakPassword.text());
        b.pulsa(Act::Add);
        b.texto(T_ADD_NAME, "Nora");
        b.texto(T_ADD_PW, "");
        b.respuesta(T_ADD_ADMIN, Answer::Cancel);
        assert!(b.dialog.is_none() && !b.files().accounts().iter().any(|a| a.name == "Nora"));
        assert_eq!(b.files().accounts().len(), 4);
        // La lista dibujada incluye las nuevas.
        b.dibuja(780, 520);
        assert_eq!(b.app.others.iter().map(|o| o.0.as_str()).collect::<Vec<_>>(), ["Luis", "Marta", "Pepe"]);
    }

    #[test]
    fn renombrar_y_cambiar_el_tipo_de_otra_cuenta() {
        let mut b = Banco::con_disco("Ana");
        let luis = b.fila("Luis");
        b.files().write("C:\\Usuarios\\Luis\\Documentos\\a.txt", b"x", crate::platform::DateTime::default()).unwrap();
        b.pulsa(Act::Rename(luis));
        assert!(matches!(&b.dialog, Some(DialogRequest::Input { tag: T_REN, value, .. }) if value == "Luis"));
        b.texto(T_REN, "Luisa");
        assert!(b.dialog.is_none());
        assert!(b.files().accounts().iter().any(|a| a.name == "Luisa") && b.files().exists("C:\\Usuarios\\Luisa\\Documentos\\a.txt"));
        assert!(!b.files().exists("C:\\Usuarios\\Luis"));
        // Errores: nombre repetido y nombre malo.
        let luisa = b.fila("Luisa");
        b.pulsa(Act::Rename(luisa));
        b.texto(T_REN, "ana");
        assert_eq!(b.mensaje(), AccountError::Exists.text());
        b.pulsa(Act::Rename(luisa));
        b.texto(T_REN, "x|y");
        assert_eq!(b.mensaje(), AccountError::BadName.text());
        // Hacer administradora y volver a usuaria normal.
        b.pulsa(Act::Admin(luisa));
        assert!(b.dialog.is_none() && b.files().accounts().iter().find(|a| a.name == "Luisa").unwrap().admin);
        b.dibuja(780, 520);
        assert!(b.app.others[luisa].1);
        b.pulsa(Act::Admin(luisa));
        assert!(!b.files().accounts().iter().find(|a| a.name == "Luisa").unwrap().admin);
    }

    #[test]
    fn eliminar_conservando_o_con_archivos_y_confirmando_el_nombre() {
        let mut b = Banco::con_disco("Ana");
        let n = crate::platform::DateTime::default();
        for c in ["Marta", "Pepe"] {
            b.files().create_account(c, None, false, [5; 16], n).unwrap();
            b.files().write(&format!("C:\\Usuarios\\{c}\\Documentos\\a.txt"), b"x", n).unwrap();
        }
        // Conservar la carpeta: No.
        let luis = b.fila("Luis");
        b.files().write("C:\\Usuarios\\Luis\\Documentos\\a.txt", b"x", n).unwrap();
        b.pulsa(Act::Delete(luis));
        assert!(matches!(&b.dialog, Some(DialogRequest::Message { tag: T_DEL_FILES, buttons: Buttons::YesNoCancel, .. })));
        b.respuesta(T_DEL_FILES, Answer::No);
        assert!(b.mensaje().contains("conservado"));
        assert!(!b.files().accounts().iter().any(|a| a.name == "Luis") && b.files().exists("C:\\Usuarios\\Luis\\Documentos\\a.txt"));
        // Cancelar: no se toca nada.
        let marta = b.fila("Marta");
        b.pulsa(Act::Delete(marta));
        b.respuesta(T_DEL_FILES, Answer::Cancel);
        assert!(b.dialog.is_none() && b.app.job.is_none() && b.files().accounts().iter().any(|a| a.name == "Marta"));
        // Con archivos: pide el nombre; mal escrito (otra capitalización o vacío) no elimina.
        b.pulsa(Act::Delete(marta));
        b.respuesta(T_DEL_FILES, Answer::Yes);
        assert!(matches!(&b.dialog, Some(DialogRequest::Input { tag: T_DEL_NAME, .. })));
        b.texto(T_DEL_NAME, "marta");
        assert!(b.mensaje().contains("No se ha eliminado nada"));
        assert!(b.files().accounts().iter().any(|a| a.name == "Marta") && b.files().exists("C:\\Usuarios\\Marta\\Documentos\\a.txt"));
        b.pulsa(Act::Delete(marta));
        b.respuesta(T_DEL_FILES, Answer::Yes);
        b.texto(T_DEL_NAME, "");
        assert!(b.mensaje().contains("No se ha eliminado nada") && b.files().accounts().iter().any(|a| a.name == "Marta"));
        // Cancelar el nombre tampoco.
        b.pulsa(Act::Delete(marta));
        b.respuesta(T_DEL_FILES, Answer::Yes);
        b.responde(DialogResult::Text { tag: T_DEL_NAME, value: None });
        assert!(b.dialog.is_none() && b.files().accounts().iter().any(|a| a.name == "Marta"));
        // Bien escrito: se elimina con sus archivos.
        b.pulsa(Act::Delete(marta));
        b.respuesta(T_DEL_FILES, Answer::Yes);
        b.texto(T_DEL_NAME, "Marta");
        assert!(b.mensaje().contains("sus archivos"));
        assert!(!b.files().accounts().iter().any(|a| a.name == "Marta") && !b.files().exists("C:\\Usuarios\\Marta"));
        assert!(b.files().exists("C:\\Usuarios\\Pepe\\Documentos\\a.txt") && b.app.job.is_none());
    }

    #[test]
    fn errores_de_cuentas_se_muestran() {
        // Tres administradoras y una usuaria; la lista se dibuja con una sesión y luego cambia
        // la situación (la pantalla puede ir por detrás del disco).
        let mut b = Banco::con_disco("Ana");
        let n = crate::platform::DateTime::default();
        b.files().set_admin("Luis", true, n).unwrap();
        let luis = b.fila("Luis");
        // Sesión de Luis, único administrador: no puede quitarse el tipo ni eliminarse a sí mismo.
        b.files().login("Luis", "").unwrap();
        b.files().set_admin("Ana", false, n).unwrap();
        b.pulsa_viejo(Act::Admin(luis));
        assert_eq!(b.mensaje(), AccountError::LastAdmin.text());
        b.pulsa_viejo(Act::Delete(luis));
        b.respuesta(T_DEL_FILES, Answer::No);
        assert_eq!(b.mensaje(), AccountError::InUse.text());
        // Un usuario normal que aún tiene la pantalla de administrador.
        let mut b = Banco::con_disco("Ana");
        let luis = b.fila("Luis");
        b.files().login("Luis", "").unwrap();
        b.pulsa_viejo(Act::Admin(luis));
        assert_eq!(b.mensaje(), AccountError::NotAdmin.text());
        b.pulsa_viejo(Act::Rename(luis));
        b.texto(T_REN, "Otro");
        assert_eq!(b.mensaje(), AccountError::NotAdmin.text());
        b.pulsa_viejo(Act::Delete(luis));
        b.respuesta(T_DEL_FILES, Answer::No);
        assert_eq!(b.mensaje(), AccountError::NotAdmin.text());
    }

    #[test]
    fn un_usuario_normal_solo_ve_su_cuenta() {
        let mut b = Banco::con_disco("Luis");
        assert!(b.hay(Act::SetPwd) && !b.hay(Act::Add));
        assert!(b.app.others.is_empty() && !b.app.hits.iter().any(|h| matches!(h.1, Hit::Act(Act::Rename(_) | Act::Admin(_) | Act::Delete(_)))));
        // Su clave sí la puede cambiar.
        b.pulsa(Act::SetPwd);
        b.texto(T_PW_NEW, "clave-luis");
        b.texto(T_PW_REP, "clave-luis");
        b.respuesta(T_ASK_HOME, Answer::No);
        assert!(b.files().verify_password("clave-luis"));
        // Sin sesión de administrador, las cuentas de los demás siguen intactas.
        assert_eq!(b.files().accounts().len(), 2);
    }

    #[test]
    fn sin_disco_se_avisa_y_no_hay_acciones() {
        let mut b = Banco::new();
        b.pagina(PAGE_ACCOUNTS);
        let s = b.dibuja(780, 520);
        assert!(!b.app.hits.iter().any(|h| matches!(h.1, Hit::Act(_))));
        assert!(s.px.iter().any(|&p| p != 0xFFFFFF));
        assert!(!b.sys.fs().persistent());
        // Un clic donde estaría un botón no hace nada.
        b.clic(PANEL_W + 40, 200);
        assert!(b.dialog.is_none());
    }

    #[test]
    fn la_lista_larga_se_desplaza_y_se_dibuja_en_cualquier_tamano() {
        let mut b = Banco::con_disco("Ana");
        let n = crate::platform::DateTime::default();
        for i in 0..14 {
            b.files().create_account(&format!("Cuenta{i:02}"), None, false, [6; 16], n).unwrap();
        }
        b.dibuja(780, 520);
        assert!(b.app.metrics().needed());
        let max = b.app.max_scroll();
        b.ev(MouseEv::Wheel { x: 400, y: 300, dz: 1 });
        assert!(b.app.scroll > 0 && b.redraw);
        b.ev(MouseEv::Wheel { x: 400, y: 300, dz: 1000 });
        assert_eq!(b.app.scroll, max);
        b.dibuja(780, 520);
        // El último botón (Añadir cuenta) está a la vista y se puede pulsar.
        b.pulsa(Act::Add);
        assert!(matches!(&b.dialog, Some(DialogRequest::Input { tag: T_ADD_NAME, .. })));
        // Si la ventana crece, el desplazamiento se corrige solo.
        b.dialog = None;
        b.dibuja(780, 1400);
        assert_eq!(b.app.scroll, 0);
        for (w, h) in [(520, 360), (1, 1), (0, 0), (221, 400), (600, 1), (780, 520)] {
            b.dibuja(w, h);
        }
        // La barra arrastra el contenido.
        b.dibuja(780, 520);
        let m = b.app.metrics();
        let (ts, _) = b.app.sb.thumb(&m);
        b.ev(MouseEv::Down { x: m.rect.x + 5, y: m.rect.y + ts + 2 });
        b.ev(MouseEv::Move { x: m.rect.x + 5, y: 9000 });
        b.ev(MouseEv::Up { x: m.rect.x + 5, y: 9000 });
        assert_eq!(b.app.scroll, b.app.max_scroll());
    }

    // ---- Cifrado (v0.6.0) ----

    /// Disco en memoria que se puede reabrir tras "reiniciar".
    #[derive(Clone)]
    struct Compartido(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);
    impl crate::block::BlockDev for Compartido {
        fn blocks(&self) -> u64 {
            (self.0.borrow().len() / crate::block::BLOCK) as u64
        }
        fn read(&mut self, n: u64, buf: &mut [u8]) -> Result<(), crate::block::DiskError> {
            let o = n as usize * crate::block::BLOCK;
            buf.copy_from_slice(self.0.borrow().get(o..o + crate::block::BLOCK).ok_or(crate::block::DiskError::Range)?);
            Ok(())
        }
        fn write(&mut self, n: u64, buf: &[u8]) -> Result<(), crate::block::DiskError> {
            let o = n as usize * crate::block::BLOCK;
            self.0.borrow_mut().get_mut(o..o + crate::block::BLOCK).ok_or(crate::block::DiskError::Range)?.copy_from_slice(buf);
            Ok(())
        }
    }

    const CLAVE_DISCO: &str = "Clave-del-disco-9!";
    const NUEVA: &str = "Otra-clave-nueva-7?";
    const AHORA: crate::platform::DateTime = crate::platform::DateTime { year: 2026, month: 11, day: 15, hour: 21, minute: 37, second: 5 };

    impl Banco {
        /// Como `con_disco` pero sobre un disco compartido de 300 bloques, con la página `pagina` abierta.
        fn con_disco_compartido(quien: &str, pagina: usize) -> (Banco, Compartido) {
            let disco = Compartido(std::rc::Rc::new(std::cell::RefCell::new(vec![0; 300 * crate::block::BLOCK])));
            let mut f = crate::files::Files::probe(Some(Box::new(disco.clone())), AHORA);
            f.format_pending(AHORA).unwrap();
            f.create_account("Ana", None, true, [1; 16], AHORA).unwrap();
            f.login("Ana", "").unwrap();
            f.create_account("Luis", None, false, [2; 16], AHORA).unwrap();
            f.login(quien, "").unwrap();
            let mut b = Banco::new();
            b.sys.files = f;
            b.pagina(pagina);
            b.dibuja(780, 520);
            (b, disco)
        }
        fn pool_listo(&mut self) {
            for i in 0..300 {
                self.sys.pool.add_event(i, i * 5, i as u64, i as u64 * 11);
            }
        }
        /// Texto del diálogo `Message` pendiente con esa etiqueta (sin quitarlo).
        fn texto_de(&self, tag: u32) -> String {
            match &self.dialog {
                Some(DialogRequest::Message { tag: t, text, .. }) if *t == tag => text.clone(),
                otro => panic!("se esperaba el mensaje {tag} y hay {otro:?}"),
            }
        }
        fn entropia(&self) {
            assert!(matches!(&self.dialog, Some(DialogRequest::Entropy { tag: T_ENT, .. })), "{:?}", self.dialog);
        }
        /// Recoge el diálogo del código de recuperación y devuelve el código.
        fn codigo(&mut self) -> String {
            match self.dialog.take() {
                Some(DialogRequest::Message { tag: T_CODE, title, text, buttons: Buttons::Ok }) => {
                    assert_eq!(title, "Código de recuperación");
                    assert!(text.ends_with("No se volverá a mostrar."));
                    let (code, resto) = text.split_once("\n\n").unwrap();
                    assert!(!code.contains(' ') && code.len() >= 40, "el código va solo en su línea");
                    assert!(resto.contains("Apúntalo en papel"));
                    String::from(code)
                }
                otro => panic!("se esperaba el código y hay {otro:?}"),
            }
        }
        /// Activa el cifrado total de principio a fin; devuelve el código de recuperación.
        fn activa_disco(&mut self, clave: &str) -> String {
            self.pool_listo();
            self.pulsa(Act::DiskEnc);
            assert!(self.texto_de(T_CONF).contains("copia de seguridad"));
            self.respuesta(T_CONF, Answer::Yes);
            pw_dialog(self, T_NEWK);
            self.texto(T_NEWK, clave);
            pw_dialog(self, T_REPK);
            self.texto(T_REPK, clave);
            self.entropia();
            self.respuesta(T_ENT, Answer::Ok);
            let code = self.codigo();
            assert!(self.app.job.as_ref().is_some_and(|j| j.old.0.is_empty() && j.new.0.is_empty()), "las claves ya se han soltado");
            self.respuesta(T_CODE, Answer::Ok);
            assert_eq!(self.mensaje(), "El disco se está cifrando. Puedes seguir usando el equipo.");
            assert!(self.app.job.is_none());
            code
        }
        fn termina_conversion(&mut self) {
            let mut n = 0;
            while !self.files().convert_step(40, AHORA).unwrap() {
                n += 1;
                assert!(n < 1000);
            }
        }
        /// Disco cifrado y con la conversión terminada.
        fn disco_cifrado(quien: &str) -> (Banco, Compartido, String) {
            let (mut b, d) = Banco::con_disco_compartido(quien, PAGE_DISK);
            let code = b.activa_disco(CLAVE_DISCO);
            b.termina_conversion();
            (b, d, code)
        }
        fn eventos(&self) -> u32 {
            self.sys.pool.events()
        }
        /// Lanza una acción sin pasar por el dibujo (botones que la pantalla aún no ofrece).
        fn lanza(&mut self, act: Act) {
            self.dialog = None;
            let mut cx = Ctx::new(&mut self.sys, &mut self.theme, 0);
            self.app.start(act, &mut cx);
            self.dialog = cx.dialog.take();
        }
    }

    /// Reinicio: vuelve a abrir el disco compartido.
    fn reabre(d: &Compartido) -> crate::files::Files {
        crate::files::Files::probe(Some(Box::new(d.clone())), AHORA)
    }

    #[test]
    fn la_pagina_disco_segun_quien_mira() {
        // Administradora: ve el botón de activar y nada más.
        let (mut b, _) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        assert!(b.hay(Act::DiskEnc) && !b.hay(Act::DiskDec) && !b.hay(Act::DiskChange) && !b.hay(Act::DiskCode));
        // Usuario normal: ve el estado pero ningún botón.
        let (mut b, _) = Banco::con_disco_compartido("Luis", PAGE_DISK);
        b.dibuja(780, 520);
        assert!(!b.app.hits.iter().any(|h| matches!(h.1, Hit::Act(_))));
        assert!(b.app.content_h > 0);
        // Sin disco: solo el aviso.
        let mut b = Banco::new();
        b.pagina(PAGE_DISK);
        b.dibuja(780, 520);
        assert!(!b.app.hits.iter().any(|h| matches!(h.1, Hit::Act(_))));
        b.clic(PANEL_W + 40, 100);
        assert!(b.dialog.is_none());
        // Tamaños extremos con el disco cifrándose.
        let (mut b, _) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        b.activa_disco(CLAVE_DISCO);
        for (w, h) in [(520, 360), (1, 1), (0, 0), (221, 400), (780, 520)] {
            b.dibuja(w, h);
        }
    }

    #[test]
    fn el_estado_del_disco_muestra_el_porcentaje_y_apaga_los_botones() {
        let (mut b, _) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        b.activa_disco(CLAVE_DISCO);
        for a in [Act::DiskEnc, Act::DiskDec, Act::DiskChange, Act::DiskCode] {
            assert!(!b.hay(a), "{a:?} debe estar desactivado mientras convierte");
        }
        let antes = b.dibuja(780, 520);
        b.files().convert_step(100, AHORA).unwrap();
        let (hechos, total, cifrando) = b.files().disk_conversion().unwrap();
        assert!(cifrando && hechos > 0 && hechos < total);
        // La pantalla cambia con el avance y el tick pide redibujar.
        assert!(b.dibuja(780, 520).px != antes.px);
        let mut cx = Ctx::new(&mut b.sys, &mut b.theme, 5000);
        b.app.tick(&mut cx);
        assert!(cx.redraw);
        drop(cx);
        b.termina_conversion();
        assert!(b.files().disk_conversion().is_none() && b.files().disk_encrypted());
        assert!(b.hay(Act::DiskDec) && b.hay(Act::DiskChange) && b.hay(Act::DiskCode) && !b.hay(Act::DiskEnc));
        // Descifrando también apaga los botones.
        b.pulsa(Act::DiskDec);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_SEC, CLAVE_DISCO);
        assert_eq!(b.mensaje(), "Se está descifrando el disco. Puedes seguir usando el equipo.");
        assert!(matches!(b.files().disk_conversion(), Some((_, _, false))));
        assert!(!b.hay(Act::DiskDec) && !b.hay(Act::DiskChange));
    }

    #[test]
    fn activar_el_cifrado_total_y_abrir_con_la_clave_o_con_el_codigo() {
        let (mut b, disco) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        b.files().write("C:\\Usuarios\\Ana\\Documentos\\nota.txt", b"hola", AHORA).unwrap();
        let code = b.activa_disco(CLAVE_DISCO);
        assert!(b.files().disk_encrypted());
        assert_eq!(b.eventos(), 300, "el acumulador se devuelve al sistema");
        b.termina_conversion();
        // Con el código de recuperación.
        let mut g = reabre(&disco);
        assert!(g.disk_locked());
        assert_eq!(g.unlock_disk("no-es-la-clave", false, AHORA), Err(keys::KeyError::WrongSecret));
        g.unlock_disk(&code, true, AHORA).unwrap();
        g.login("Ana", "").unwrap();
        assert_eq!(g.read("C:\\Usuarios\\Ana\\Documentos\\nota.txt").unwrap(), b"hola");
        // Y con la clave del disco.
        reabre(&disco).unlock_disk(CLAVE_DISCO, false, AHORA).unwrap();
        // Ni el código ni la clave están en claro en el disco.
        let raw = disco.0.borrow().clone();
        assert!(!raw.windows(code.len()).any(|w| w == code.as_bytes()));
        assert!(!raw.windows(CLAVE_DISCO.len()).any(|w| w == CLAVE_DISCO.as_bytes()));
        assert!(b.hay(Act::DiskDec) && !b.hay(Act::DiskEnc));
    }

    #[test]
    fn claves_del_disco_distintas_cortas_y_debiles() {
        let (mut b, _) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        b.pool_listo();
        // No coinciden.
        b.pulsa(Act::DiskEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_NEWK, CLAVE_DISCO);
        b.texto(T_REPK, NUEVA);
        assert_eq!(b.mensaje(), NO_MATCH);
        assert!(b.app.job.is_none() && !b.files().disk_encrypted());
        // Corta.
        b.pulsa(Act::DiskEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_NEWK, "abc");
        assert_eq!(b.mensaje(), keys::KeyError::Weak.text());
        assert!(b.app.job.is_none() && !b.files().disk_encrypted());
        // Débil y se dice que no: no cambia nada.
        b.pulsa(Act::DiskEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_NEWK, "abcdef");
        b.texto(T_REPK, "abcdef");
        assert_eq!(b.texto_de(T_WEAK), "La clave es débil. ¿Usarla de todos modos?");
        b.respuesta(T_WEAK, Answer::No);
        assert!(b.dialog.is_none() && b.app.job.is_none() && !b.files().disk_encrypted());
        // Débil y se dice que sí: sigue con la pantalla del ratón y la acepta.
        b.pulsa(Act::DiskEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_NEWK, "abcdef");
        b.texto(T_REPK, "abcdef");
        b.respuesta(T_WEAK, Answer::Yes);
        b.entropia();
        b.respuesta(T_ENT, Answer::Ok);
        b.codigo();
        assert!(b.files().disk_encrypted());
    }

    #[test]
    fn cancelar_en_cualquier_paso_no_cambia_nada_ni_deja_claves() {
        let (mut b, _) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        b.pool_listo();
        // Se cancela en el paso `n` de la activación (0 aviso, 1 clave, 2 repetir, 3 ratón).
        for n in 0..4 {
            b.pulsa(Act::DiskEnc);
            for paso in 0..=n {
                let cancela = paso == n;
                match paso {
                    0 if cancela => b.respuesta(T_CONF, Answer::Cancel),
                    0 => b.respuesta(T_CONF, Answer::Yes),
                    1 if cancela => b.responde(DialogResult::Text { tag: T_NEWK, value: None }),
                    1 => b.texto(T_NEWK, CLAVE_DISCO),
                    2 if cancela => b.responde(DialogResult::Text { tag: T_REPK, value: None }),
                    2 => b.texto(T_REPK, CLAVE_DISCO),
                    _ => b.respuesta(T_ENT, Answer::Cancel),
                }
            }
            assert!(b.dialog.is_none() && b.app.job.is_none(), "paso {n}");
            assert!(!b.files().disk_encrypted() && b.eventos() == 300);
        }
        // "No" en el aviso también lo deja todo igual.
        b.pulsa(Act::DiskEnc);
        b.respuesta(T_CONF, Answer::No);
        assert!(b.dialog.is_none() && b.app.job.is_none());
        // Cerrar la ventana a mitad: el trabajo (con su clave) se suelta con la app.
        b.pulsa(Act::DiskEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_NEWK, CLAVE_DISCO);
        assert!(b.app.job.as_ref().unwrap().new.same(&Secret(String::from(CLAVE_DISCO))));
        b.app.job = None;
        assert!(!b.files().disk_encrypted());
        // Con el disco ya cifrado: cancelar al quitar, cambiar y pedir código.
        let (mut b, _, _) = Banco::disco_cifrado("Ana");
        b.pulsa(Act::DiskDec);
        b.respuesta(T_CONF, Answer::Cancel);
        b.pulsa(Act::DiskDec);
        b.respuesta(T_CONF, Answer::Yes);
        b.responde(DialogResult::Text { tag: T_SEC, value: None });
        b.pulsa(Act::DiskChange);
        b.texto(T_SEC, CLAVE_DISCO);
        b.responde(DialogResult::Text { tag: T_NEWK, value: None });
        b.pulsa(Act::DiskCode);
        b.texto(T_SEC, CLAVE_DISCO);
        b.respuesta(T_ENT, Answer::Cancel);
        assert!(b.dialog.is_none() && b.app.job.is_none() && b.files().disk_encrypted() && b.files().disk_conversion().is_none());
    }

    #[test]
    fn la_app_no_guarda_la_clave_ni_el_codigo() {
        let (mut b, _) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        b.activa_disco(CLAVE_DISCO);
        assert!(b.app.job.is_none());
        // Un error tampoco deja claves: clave del disco mala al quitar el cifrado.
        b.termina_conversion();
        b.pulsa(Act::DiskDec);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_SEC, "clave-equivocada");
        assert_eq!(b.mensaje(), keys::KeyError::WrongSecret.text());
        assert!(b.app.job.is_none() && b.files().disk_conversion().is_none());
    }

    #[test]
    fn quitar_cambiar_clave_y_nuevo_codigo_del_disco() {
        let (mut b, disco, viejo) = Banco::disco_cifrado("Ana");
        // Cambiar la clave: la actual mala falla, la corta y la que no coincide también.
        b.pulsa(Act::DiskChange);
        b.texto(T_SEC, "clave-equivocada");
        b.texto(T_NEWK, NUEVA);
        b.texto(T_REPK, NUEVA);
        assert_eq!(b.mensaje(), keys::KeyError::WrongSecret.text());
        b.pulsa(Act::DiskChange);
        b.texto(T_SEC, CLAVE_DISCO);
        b.texto(T_NEWK, "corta");
        assert_eq!(b.mensaje(), keys::KeyError::Weak.text());
        b.pulsa(Act::DiskChange);
        b.texto(T_SEC, CLAVE_DISCO);
        b.texto(T_NEWK, NUEVA);
        b.texto(T_REPK, "otra-clave-distinta");
        assert_eq!(b.mensaje(), NO_MATCH);
        // Buena: la vieja deja de abrir.
        b.pulsa(Act::DiskChange);
        b.texto(T_SEC, CLAVE_DISCO);
        b.texto(T_NEWK, NUEVA);
        b.texto(T_REPK, NUEVA);
        assert_eq!(b.mensaje(), "Se ha cambiado la clave del disco.");
        let mut g = reabre(&disco);
        assert_eq!(g.unlock_disk(CLAVE_DISCO, false, AHORA), Err(keys::KeyError::WrongSecret));
        g.unlock_disk(NUEVA, false, AHORA).unwrap();
        // Nuevo código: el anterior deja de valer y el nuevo abre.
        b.pulsa(Act::DiskCode);
        b.texto(T_SEC, NUEVA);
        b.entropia();
        b.respuesta(T_ENT, Answer::Ok);
        let nuevo = b.codigo();
        b.respuesta(T_CODE, Answer::Ok);
        assert!(b.dialog.is_none(), "un código de reemplazo no lleva mensaje de cierre");
        assert_ne!(nuevo, viejo);
        let mut h = reabre(&disco);
        assert_eq!(h.unlock_disk(&viejo, true, AHORA), Err(keys::KeyError::WrongSecret));
        h.unlock_disk(&nuevo, true, AHORA).unwrap();
        // Código con la clave mala.
        b.pulsa(Act::DiskCode);
        b.texto(T_SEC, "clave-equivocada");
        b.respuesta(T_ENT, Answer::Ok);
        assert_eq!(b.mensaje(), keys::KeyError::WrongSecret.text());
        // Quitar el cifrado.
        b.pulsa(Act::DiskDec);
        assert!(b.texto_de(T_CONF).contains("descifrará todo el disco"));
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_SEC, NUEVA);
        assert_eq!(b.mensaje(), "Se está descifrando el disco. Puedes seguir usando el equipo.");
        b.termina_conversion();
        assert!(!b.files().disk_encrypted() && !reabre(&disco).disk_locked());
        assert!(b.hay(Act::DiskEnc));
    }

    #[test]
    fn errores_del_disco_no_admin_y_acumulador_sin_llenar() {
        // Un usuario normal con la pantalla de administrador todavía abierta.
        let (mut b, _) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        b.dibuja(780, 520);
        b.files().login("Luis", "").unwrap();
        b.pool_listo();
        b.pulsa_viejo(Act::DiskEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_NEWK, CLAVE_DISCO);
        b.texto(T_REPK, CLAVE_DISCO);
        b.respuesta(T_ENT, Answer::Ok);
        assert_eq!(b.mensaje(), keys::KeyError::NotAdmin.text());
        assert!(b.app.job.is_none() && b.eventos() == 300);
        // Aleatoriedad insuficiente: se avisa y no se cifra; el acumulador vuelve.
        let (mut b, _) = Banco::con_disco_compartido("Ana", PAGE_DISK);
        b.pulsa(Act::DiskEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_NEWK, CLAVE_DISCO);
        b.texto(T_REPK, CLAVE_DISCO);
        b.respuesta(T_ENT, Answer::Ok);
        assert_eq!(b.mensaje(), keys::KeyError::NotReady.text());
        assert!(!b.files().disk_encrypted() && b.app.job.is_none());
        // Cambiar la clave con la pantalla vieja de un usuario normal.
        let (mut b, _, _) = Banco::disco_cifrado("Ana");
        b.dibuja(780, 520);
        b.files().login("Luis", "").unwrap();
        b.pulsa_viejo(Act::DiskChange);
        b.texto(T_SEC, CLAVE_DISCO);
        b.texto(T_NEWK, NUEVA);
        b.texto(T_REPK, NUEVA);
        assert_eq!(b.mensaje(), keys::KeyError::NotAdmin.text());
    }

    #[test]
    fn mi_cuenta_botones_de_cifrado_segun_la_clave() {
        let mut b = Banco::con_disco("Luis");
        // Sin clave no se ofrece cifrar.
        assert!(!b.hay(Act::HomeEnc) && !b.hay(Act::HomeDec) && !b.hay(Act::HomeCode));
        b.files().set_password("", Some("clave-luis"), [3; 16], AHORA).unwrap();
        assert!(b.hay(Act::HomeEnc) && !b.hay(Act::HomeDec) && !b.hay(Act::HomeCode));
        for (w, h) in [(520, 360), (1, 1), (221, 400)] {
            b.dibuja(w, h);
        }
    }

    /// Resultado de pedir cifrar/renovar con la clave buena: el código o el error de la llamada.
    fn resultado_de_la_llamada(b: &mut Banco) {
        match b.dialog.take() {
            Some(DialogRequest::Message { tag: T_CODE, .. }) => {}
            Some(DialogRequest::Message { tag: T_MSG, text, .. }) => assert!(
                [keys::KeyError::Unavailable, keys::KeyError::NotEncrypted].iter().any(|e| e.text() == text),
                "{text}"
            ),
            otro => panic!("{otro:?}"),
        }
    }

    #[test]
    fn mi_cuenta_cifrar_pide_aviso_clave_y_raton_y_muestra_el_resultado() {
        let mut b = Banco::con_disco("Luis");
        b.files().set_password("", Some("clave-luis"), [3; 16], AHORA).unwrap();
        b.pool_listo();
        b.pulsa(Act::HomeEnc);
        assert!(b.texto_de(T_CONF).contains("Si olvidas la clave y pierdes el código de recuperación, no se podrán recuperar. ¿Continuar?"));
        b.respuesta(T_CONF, Answer::Yes);
        pw_dialog(&b, T_SEC);
        // Clave mala: se corta antes de la pantalla del ratón.
        b.texto(T_SEC, "mala-clave");
        assert_eq!(b.mensaje(), AccountError::WrongPassword.text());
        assert!(b.app.job.is_none());
        // Cancelar en cada paso no cambia nada.
        b.pulsa(Act::HomeEnc);
        b.respuesta(T_CONF, Answer::Cancel);
        b.pulsa(Act::HomeEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.responde(DialogResult::Text { tag: T_SEC, value: None });
        b.pulsa(Act::HomeEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_SEC, "clave-luis");
        b.respuesta(T_ENT, Answer::Cancel);
        assert!(b.dialog.is_none() && b.app.job.is_none());
        // Clave buena: pantalla del ratón y la llamada; la pantalla muestra lo que ella devuelva.
        b.pulsa(Act::HomeEnc);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_SEC, "clave-luis");
        b.entropia();
        b.respuesta(T_ENT, Answer::Ok);
        resultado_de_la_llamada(&mut b);
        assert!(b.app.job.as_ref().map_or(true, |j| j.old.0.is_empty() && j.new.0.is_empty()));
        assert_eq!(b.eventos(), 300, "el acumulador vuelve al sistema también si la llamada falla");
    }

    #[test]
    fn mi_cuenta_quitar_cifrado_y_nuevo_codigo_muestran_el_resultado_de_la_llamada() {
        let mut b = Banco::con_disco("Luis");
        b.files().set_password("", Some("clave-luis"), [3; 16], AHORA).unwrap();
        b.pool_listo();
        // Quitar el cifrado: con clave mala se corta; con la buena sale lo que devuelva la llamada.
        b.lanza(Act::HomeDec);
        assert!(b.texto_de(T_CONF).contains("se descifrará"));
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_SEC, "mala-clave");
        assert_eq!(b.mensaje(), AccountError::WrongPassword.text());
        b.lanza(Act::HomeDec);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_SEC, "clave-luis");
        if !b.files().home_encrypted() {
            assert_eq!(b.mensaje(), keys::KeyError::NotEncrypted.text());
        }
        // Nuevo código.
        b.app.job = None;
        b.lanza(Act::HomeCode);
        b.texto(T_SEC, "clave-luis");
        b.entropia();
        b.respuesta(T_ENT, Answer::Ok);
        resultado_de_la_llamada(&mut b);
        assert_eq!(b.eventos(), 300);
    }

    #[test]
    fn mi_cuenta_tras_poner_la_clave_ofrece_cifrar_la_carpeta() {
        let mut b = Banco::con_disco("Ana");
        b.pool_listo();
        // Sí: sigue con la pantalla del ratón sin volver a pedir la clave.
        b.pulsa(Act::SetPwd);
        b.texto(T_PW_NEW, "secreta1");
        b.texto(T_PW_REP, "secreta1");
        assert_eq!(b.texto_de(T_ASK_HOME), "Se ha puesto la clave.\n¿Cifrar también tu carpeta? (Recomendado)");
        assert!(matches!(&b.dialog, Some(DialogRequest::Message { buttons: Buttons::YesNo, .. })));
        b.respuesta(T_ASK_HOME, Answer::Yes);
        b.entropia();
        b.respuesta(T_ENT, Answer::Ok);
        assert!(b.files().verify_password("secreta1"));
        resultado_de_la_llamada(&mut b);
        assert!(b.app.job.as_ref().map_or(true, |j| j.old.0.is_empty() && j.new.0.is_empty()));
        if b.files().home_encrypted() {
            return; // con el cifrado real la clave ya no se puede quitar: lo cubre la prueba de integración
        }
        // Quitar la clave y volver a ponerla; esta vez se cancela la pantalla del ratón: la clave queda puesta.
        b.app.job = None;
        b.pulsa(Act::RemovePwd);
        b.texto(T_PW_OLD, "secreta1");
        assert_eq!(b.mensaje(), "Se ha quitado la clave.");
        b.pulsa(Act::SetPwd);
        b.texto(T_PW_NEW, "secreta2");
        b.texto(T_PW_REP, "secreta2");
        b.respuesta(T_ASK_HOME, Answer::Yes);
        b.respuesta(T_ENT, Answer::Cancel);
        assert!(b.dialog.is_none() && b.app.job.is_none() && b.files().verify_password("secreta2"));
    }

    #[test]
    fn integracion_real_cifrar_la_carpeta_desde_mi_cuenta() {
        let mut b = Banco::con_disco("Ana");
        b.pool_listo();
        b.files().write("C:\\Usuarios\\Ana\\Documentos\\nota.txt", b"hola", AHORA).unwrap();
        // Poner la clave y aceptar cifrar la carpeta.
        b.pulsa(Act::SetPwd);
        b.texto(T_PW_NEW, "secreta1");
        b.texto(T_PW_REP, "secreta1");
        b.respuesta(T_ASK_HOME, Answer::Yes);
        b.respuesta(T_ENT, Answer::Ok);
        let code = b.codigo();
        b.respuesta(T_CODE, Answer::Ok);
        assert_eq!(b.mensaje(), "Se está cifrando tu carpeta. Puedes seguir usando el equipo.");
        assert!(b.files().home_encrypted());
        // Mientras convierte, los botones están desactivados.
        if b.files().home_conversion().is_some() {
            assert!(!b.hay(Act::HomeDec) && !b.hay(Act::HomeCode));
        }
        while !b.files().home_convert_step(10, AHORA).unwrap() {}
        assert_eq!(b.files().read("C:\\Usuarios\\Ana\\Documentos\\nota.txt").unwrap(), b"hola");
        assert!(b.hay(Act::HomeDec) && b.hay(Act::HomeCode) && !b.hay(Act::HomeEnc));
        // Con la carpeta cifrada no se puede quitar la clave.
        b.pulsa(Act::RemovePwd);
        assert_eq!(b.mensaje(), AccountError::Encrypted.text());
        // Nuevo código de recuperación: cambia.
        b.pulsa(Act::HomeCode);
        b.texto(T_SEC, "secreta1");
        b.respuesta(T_ENT, Answer::Ok);
        let nuevo = b.codigo();
        assert_ne!(code, nuevo);
        b.respuesta(T_CODE, Answer::Ok);
        // Quitar el cifrado.
        b.pulsa(Act::HomeDec);
        b.respuesta(T_CONF, Answer::Yes);
        b.texto(T_SEC, "secreta1");
        assert_eq!(b.mensaje(), "Se está descifrando tu carpeta.");
        while !b.files().home_convert_step(10, AHORA).unwrap() {}
        assert!(!b.files().home_encrypted() && b.hay(Act::HomeEnc));
    }

    // ---- Red ----

    fn tick_banco(b: &mut Banco) -> bool {
        let mut cx = Ctx::new(&mut b.sys, &mut b.theme, 0);
        b.app.tick(&mut cx);
        cx.redraw
    }

    /// Deja correr la red simulada de `lan` con la pila que tiene el banco.
    fn corre_red(b: &mut Banco, lan: &mut crate::net::tests::Lan, ms: u64) {
        core::mem::swap(&mut lan.net, b.sys.net.as_mut().unwrap());
        lan.run(ms);
        core::mem::swap(&mut lan.net, b.sys.net.as_mut().unwrap());
    }

    #[test]
    fn red_lista_los_permisos_de_los_programas() {
        use crate::netperm::Perm;
        let n = crate::platform::DateTime::default();
        let perms = |b: &mut Banco| b.files().net_perms();
        for quien in ["Ana", "Luis"] {
            let mut b = Banco::con_disco(quien);
            b.pagina(PAGE_NET);
            assert!(!b.hay(Act::NetOut(0)), "sin programas no hay botones");
            let p = |h: u8, name: &str, out, inn| Perm { hash: [h; 32], name: String::from(name), out, inn };
            b.files().set_net_perms(&[p(1, "Mensajes", Some(true), None), p(2, "Juego", Some(false), Some(false))], n).unwrap();
            if quien == "Luis" {
                // Un usuario normal ve la lista pero no puede cambiarla.
                assert!(!b.hay(Act::NetOut(0)) && !b.hay(Act::NetForget(1)));
                assert_eq!(b.app.net_perms.len(), 2);
                continue;
            }
            // Bloquear la red a Mensajes; permitir que Juego acepte conexiones.
            b.pulsa(Act::NetOut(0));
            assert_eq!(perms(&mut b)[0].out, Some(false));
            b.pulsa(Act::NetIn(1));
            assert_eq!((perms(&mut b)[1].out, perms(&mut b)[1].inn), (Some(false), Some(true)));
            // «Sin preguntar» pasa a permitido.
            b.pulsa(Act::NetIn(0));
            assert_eq!(perms(&mut b)[0].inn, Some(true));
            // Olvidar quita el programa: la próxima vez vuelve a preguntar.
            b.pulsa(Act::NetForget(0));
            let quedan = perms(&mut b);
            assert_eq!((quedan.len(), quedan[0].name.as_str()), (1, "Juego"));
            assert!(b.hay(Act::NetForget(0)) && !b.hay(Act::NetForget(1)));
            assert!(b.dialog.is_none());
            // Una lista larga se desplaza.
            let muchos: Vec<Perm> = (0..30).map(|i| p(i, "Programa", Some(true), Some(true))).collect();
            b.files().set_net_perms(&muchos, n).unwrap();
            b.dibuja(780, 520);
            assert!(b.app.content_h > 520 && b.app.scrolls());
        }
    }

    #[test]
    fn red_certificados_anadir_y_quitar() {
        let raiz = include_bytes!("../x509/testdata/raiz.der");
        let hoja = include_bytes!("../x509/testdata/hoja.der");
        let n = crate::platform::DateTime::default();
        let ruta = |b: &mut Banco, nombre: &str, d: &[u8]| {
            let p = format!("C:\\Usuarios\\{}\\{nombre}",b.files().user().unwrap());
            b.files().write(&p, d, n).unwrap();
            p
        };
        let mut b = Banco::con_disco("Luis");
        b.pagina(PAGE_NET);
        assert!(!b.hay(Act::CertAdd), "un usuario normal no añade");
        assert_eq!(b.files().add_root(raiz.to_vec(), n), Err(FsError::Denied));
        let mut b = Banco::con_disco("Ana");
        b.pagina(PAGE_NET);
        // No es una autoridad: el error sale antes de confirmar.
        let p = ruta(&mut b, "hoja.der", hoja);
        b.pulsa(Act::CertAdd);
        assert!(matches!(b.dialog.take(), Some(DialogRequest::Open { tag: T_CERT_FILE, .. })));
        b.responde(DialogResult::Path { tag: T_CERT_FILE, path: Some(p) });
        assert!(b.mensaje().contains("no es de una autoridad"));
        // Una raíz: se enseña y se confirma.
        let p = ruta(&mut b, "raiz.der", raiz);
        b.responde(DialogResult::Path { tag: T_CERT_FILE, path: Some(p) });
        assert!(matches!(&b.dialog, Some(DialogRequest::Message { tag: T_CERT_ADD, text, .. }) if text.contains("Huella SHA-256") && text.contains("hacerse pasar")));
        b.respuesta(T_CERT_ADD, Answer::Cancel);
        assert!(b.files().custom_roots().is_empty());
        let p = ruta(&mut b, "raiz.der", raiz);
        b.responde(DialogResult::Path { tag: T_CERT_FILE, path: Some(p.clone()) });
        b.respuesta(T_CERT_ADD, Answer::Yes);
        assert!(b.mensaje().contains("se ha añadido"));
        assert_eq!(b.files().custom_roots().len(), 1);
        // Otra vez el mismo: ya está.
        b.responde(DialogResult::Path { tag: T_CERT_FILE, path: Some(p) });
        assert!(b.mensaje().contains("ya está instalado"));
        // Quitar, con confirmación.
        b.pulsa(Act::CertDel(0));
        b.respuesta(T_CERT_DEL, Answer::No);
        assert_eq!(b.files().custom_roots().len(), 1);
        b.pulsa(Act::CertDel(0));
        b.respuesta(T_CERT_DEL, Answer::Yes);
        assert!(b.files().custom_roots().is_empty() && b.dialog.is_none());
    }

    #[test]
    fn red_muestra_el_estado_y_renueva_la_direccion() {
        use crate::net::tests::{Lan, LINK};
        use crate::net::Net;
        let mut b = Banco::new();
        // Se llega a la página desde el icono de la bandeja y desde la lista; sin tarjeta no se puede renovar.
        let mut cx = Ctx::new(&mut b.sys, &mut b.theme, 0);
        b.app.open_path("C:\\x", &mut cx);
        assert_eq!(b.app.page, 0, "una ruta cualquiera no cambia de página");
        b.app.open_path(super::super::NET_PAGE, &mut cx);
        assert_eq!(b.app.page, PAGE_NET);
        assert!(cx.redraw);
        assert!(!b.hay(Act::NetRenew));
        assert_eq!(b.app.net_seen, None);
        assert!(!tick_banco(&mut b), "sin tarjeta y sin cambios no redibuja");
        // Conectada: aparece el botón y el estado queda visto.
        let mut lan = Lan::up();
        b.sys.net = Some(core::mem::replace(&mut lan.net, Net::new([0; 32])));
        assert!(tick_banco(&mut b), "ha cambiado el estado");
        assert!(b.hay(Act::NetRenew));
        let st = b.app.net_seen.clone().unwrap();
        assert_eq!((st.state, st.ip), (State::Connected, Some([192, 168, 1, 50])));
        assert!(!tick_banco(&mut b));
        // Renovar pide la dirección otra vez; el tick lo ve y la página se rehace sola.
        b.pulsa(Act::NetRenew);
        assert_eq!(b.sys.net.as_ref().unwrap().status().state, State::Configuring);
        assert!(tick_banco(&mut b));
        b.dibuja(780, 520);
        corre_red(&mut b, &mut lan, 600);
        assert!(tick_banco(&mut b));
        assert_eq!(b.sys.net.as_ref().unwrap().status().state, State::Connected);
        // Sin cable no se puede renovar.
        lan.wire.borrow_mut().link = None;
        corre_red(&mut b, &mut lan, 100);
        assert!(tick_banco(&mut b));
        assert!(!b.hay(Act::NetRenew));
        assert_eq!(b.app.net_seen.as_ref().unwrap().state, State::NoCable);
        lan.wire.borrow_mut().link = Some(LINK);
        // Fuera de la página Red no se redibuja por la red.
        b.pagina(0);
        corre_red(&mut b, &mut lan, 600);
        assert!(!tick_banco(&mut b));
    }

    #[test]
    fn fecha_y_hora_texto_de_fecha_y_de_hora() {
        assert_eq!(parse_fecha(" 24/11/2026 "), Some((2026, 11, 24)));
        assert_eq!(parse_fecha("24/11"), None);
        assert_eq!(parse_fecha("24/11/2026/1"), None);
        assert_eq!(parse_fecha("ab/11/2026"), None);
        assert_eq!(parse_hora("21:30"), Some((21, 30, 0)));
        assert_eq!(parse_hora("21:30:15"), Some((21, 30, 15)));
        assert_eq!(parse_hora("21"), None);
        assert_eq!(parse_hora("21:30:15:1"), None);
    }

    #[test]
    fn fecha_y_hora_solo_administrador_y_a_mano_sin_automatico() {
        let tokio = crate::hora::ZONAS.iter().position(|z| z.id == "Tokio").unwrap();
        let mut b = Banco::con_disco("Luis");
        b.pagina(PAGE_TIME);
        assert!(!b.hay(Act::TimeAuto) && !b.hay(Act::TimeZone(tokio)), "un usuario normal ve pero no cambia");
        assert_eq!(b.files().set_clock(DateTime { year: 2027, month: 3, day: 1, hour: 8, minute: 0, second: 0 }), Err(FsError::Denied));
        assert_eq!(b.files().time_sync(), Err(FsError::Denied));
        assert_eq!(b.files().set_time_cfg(crate::hora::Ajuste::default(), DateTime::default()), Err(FsError::Denied));

        let mut b = Banco::con_disco("Ana");
        b.pagina(PAGE_TIME);
        assert!(b.hay(Act::TimeAuto) && b.hay(Act::TimeSync));
        assert!(!b.hay(Act::TimeDate), "con el automático no se cambia a mano");
        b.pulsa(Act::TimeZone(tokio));
        assert_eq!(b.files().time_cfg().zona().id, "Tokio");
        b.pulsa(Act::TimeAuto);
        assert!(!b.files().time_cfg().auto);
        // A mano: la fecha y la hora válidas quedan pendientes para el escritorio; las que no existen, no.
        b.pulsa(Act::TimeDate);
        assert!(matches!(b.dialog.take(), Some(DialogRequest::Input { tag: T_DATE, .. })));
        b.texto(T_DATE, "01/03/2027");
        assert_eq!(b.files().hora_pendiente().0, Some(DateTime { year: 2027, month: 3, day: 1, hour: 21, minute: 37, second: 5 }));
        b.texto(T_DATE, "31/02/2027");
        assert!(b.mensaje().contains("no existe"));
        b.texto(T_TIME, "25:00");
        assert!(b.mensaje().contains("no existe"));
        b.texto(T_TIME, "hola");
        assert!(b.mensaje().contains("No se entiende"));
        assert_eq!(b.files().hora_pendiente(), (None, false));
        b.pulsa(Act::TimeSync);
        assert_eq!(b.files().hora_pendiente(), (None, true));
    }

    fn menu_arranque() -> InfoArranque {
        let e = |c: &str, n: &str, o: bool| (String::from(c), String::from(n), o);
        InfoArranque { ajustes: arranque::Ajustes::default(), entradas: vec![e("nightfury", "NightFury", false), e("windows", "Windows", true), e("firmware", "Configuración del firmware", true)], primero: false }
    }

    #[test]
    fn arranque_sin_el_menu_instalado_solo_avisa() {
        let mut b = Banco::new();
        b.pagina(PAGE_ARRANQUE);
        let s = b.dibuja(780, 520);
        assert!(!b.app.hits.iter().any(|h| matches!(h.1, Hit::Act(_))));
        assert!(s.px.iter().any(|&p| p != 0xFFFFFF) && b.app.arr.is_none());
    }

    #[test]
    fn arranque_cuenta_atras_ocultar_renombrar_y_primero() {
        let mut b = Banco::new();
        b.sys.arranque = Some(menu_arranque());
        b.pagina(PAGE_ARRANQUE);
        let aj = |b: &Banco| b.sys.arranque.clone().unwrap().ajustes;
        // Cuenta atrás: de 5 a 6, y bajando hasta 0 ya no se puede bajar más; arriba, tope en 30.
        b.pulsa(Act::ArrMas);
        assert_eq!(aj(&b).segundos, 6);
        for _ in 0..6 {
            b.pulsa(Act::ArrMenos);
        }
        assert_eq!(aj(&b).segundos, 0);
        assert!(!b.hay(Act::ArrMenos) && b.hay(Act::ArrMas));
        b.sys.arranque.as_mut().unwrap().ajustes.segundos = 30;
        assert!(!b.hay(Act::ArrMas) && b.hay(Act::ArrMenos));
        // Ocultar y volver a mostrar; NightFury no se puede ocultar (pero sí renombrar).
        assert!(!b.hay(Act::ArrOcultar(0)) && b.hay(Act::ArrNombre(0)));
        b.pulsa(Act::ArrOcultar(1));
        assert_eq!(aj(&b).ocultas, ["windows"]);
        b.pulsa(Act::ArrOcultar(1));
        assert!(aj(&b).ocultas.is_empty());
        // Renombrar: el diálogo trae el nombre actual; vacío = el original.
        b.pulsa(Act::ArrNombre(1));
        assert!(matches!(&b.dialog, Some(DialogRequest::Input { tag: T_ARR_NOM, value, .. }) if value == "Windows"));
        b.texto(T_ARR_NOM, "  Juegos ");
        assert_eq!(aj(&b).nombres, [(String::from("windows"), String::from("Juegos"))]);
        b.pulsa(Act::ArrNombre(1));
        assert!(matches!(&b.dialog, Some(DialogRequest::Input { value, .. }) if value == "Juegos"));
        b.texto(T_ARR_NOM, "");
        assert!(aj(&b).nombres.is_empty());
        b.pulsa(Act::ArrNombre(1));
        b.texto(T_ARR_NOM, &"x".repeat(33));
        assert!(b.mensaje().contains("32 letras") && aj(&b).nombres.is_empty());
        // Otro sistema delante: el botón lo pone primero y desaparece.
        assert!(b.hay(Act::ArrPrimero));
        b.pulsa(Act::ArrPrimero);
        assert!(b.sys.arranque.clone().unwrap().primero && !b.hay(Act::ArrPrimero));
        // Si el firmware no deja guardar, se avisa.
        b.sys.arranque = None;
        b.pulsa_viejo(Act::ArrMenos);
        assert_eq!(b.mensaje(), arranque::ERRORES[0]);
    }

    #[test]
    fn arranque_sin_administrador_es_de_solo_lectura() {
        let mut b = Banco::con_disco("Luis");
        b.sys.arranque = Some(menu_arranque());
        b.pagina(PAGE_ARRANQUE);
        b.dibuja(780, 520);
        assert!(b.app.arr.is_some() && !b.app.hits.iter().any(|h| matches!(h.1, Hit::Act(_))));
    }
}
