// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! `arranque.efi`: menú de arranque de NightFury en modo texto. Lo arranca el firmware; busca los sistemas
//! instalados, deja elegir (flechas + Enter, cuenta atrás) y los arranca. La lógica está en `nf_core::arranque`.
#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use alloc::{format, vec};
use core::alloc::{GlobalAlloc, Layout};
use core::ffi::c_void;
use core::ptr::{null, null_mut};
use efi::*;
use nf_core::arranque::{self as a, Accion, Ajustes, Detectado, Entrada, Menu, Tecla, Tipo};

/// Tabla del sistema, fijada al entrar. Un solo hilo, sin interrupciones propias.
static mut ST: *mut SystemTable = null_mut();

fn st() -> &'static SystemTable {
    unsafe { &*ST }
}
fn bs() -> &'static BootServices {
    unsafe { &*st().boot_services }
}
fn rt() -> &'static RuntimeServices {
    unsafe { &*st().runtime_services }
}

// ---------- Memoria (AllocatePool; se guarda el puntero original delante del alineado) ----------

struct Asignador;
unsafe impl GlobalAlloc for Asignador {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let mut p = null_mut();
        if (bs().allocate_pool)(POOL_LOADER_DATA, l.size() + l.align() + 8, &mut p) != 0 {
            return null_mut();
        }
        let r = ((p as usize + 8 + l.align() - 1) & !(l.align() - 1)) as *mut u8;
        (r as *mut *mut u8).sub(1).write_unaligned(p);
        r
    }
    unsafe fn dealloc(&self, p: *mut u8, _: Layout) {
        (bs().free_pool)((p as *mut *mut u8).sub(1).read_unaligned());
    }
}

#[global_allocator]
static ASIGNADOR: Asignador = Asignador;

#[panic_handler]
fn panico(_: &core::panic::PanicInfo) -> ! {
    let o = st().con_out;
    (unsafe { &*o }.output_string)(o, u16s("\r\nError interno del menu de arranque. Reinicia el equipo.").as_ptr());
    loop {}
}

// ---------- Utilidades ----------

fn u16s(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn guid_nf() -> Guid {
    let g = a::GUID_NF;
    Guid { d1: g.0, d2: g.1, d3: g.2, d4: g.3 }
}

/// Texto en la consola; si no tiene algún carácter, se sustituye por ASCII (y `?` lo que quede).
fn escribir(s: &str) {
    let o = st().con_out;
    let mut v = u16s(s);
    if (unsafe { &*o }.test_string)(o, v.as_ptr()) != 0 {
        v = u16s(&a::a_ascii(s).chars().map(|c| if c.is_ascii() { c } else { '?' }).collect::<String>());
    }
    (unsafe { &*o }.output_string)(o, v.as_ptr());
}

fn dibujar(m: &Menu) {
    let o = st().con_out;
    (unsafe { &*o }.clear_screen)(o);
    for l in m.pintar() {
        let sel = l.starts_with("> ");
        if sel {
            (unsafe { &*o }.set_attribute)(o, 0x70); // negro sobre gris
        }
        escribir(&l);
        (unsafe { &*o }.set_attribute)(o, 0x07);
        escribir("\r\n");
    }
}

fn leer_tecla() -> Option<Tecla> {
    let i = st().con_in;
    let mut k = InputKey { scan_code: 0, unicode_char: 0 };
    if (unsafe { &*i }.read_key_stroke)(i, &mut k) != 0 {
        return None;
    }
    Some(match (k.scan_code, k.unicode_char) {
        (1, _) => Tecla::Arriba,
        (2, _) => Tecla::Abajo,
        (_, 0x0D) => Tecla::Enter,
        _ => Tecla::Otra,
    })
}

// ---------- Variables ----------

fn leer_var(nombre: &str, guid: &Guid) -> Option<Vec<u8>> {
    let n = u16s(nombre);
    let mut len = 0usize;
    let s = (rt().get_variable)(n.as_ptr(), guid, null_mut(), &mut len, null_mut());
    if s != EFI_BUFFER_TOO_SMALL || len > 64 * 1024 {
        return None;
    }
    let mut v = vec![0u8; len];
    let s = (rt().get_variable)(n.as_ptr(), guid, null_mut(), &mut len, v.as_mut_ptr().cast());
    v.truncate(len);
    (s == 0).then_some(v)
}

fn escribir_var(nombre: &str, guid: &Guid, datos: &[u8]) -> Status {
    (rt().set_variable)(u16s(nombre).as_ptr(), guid, VAR_NV_BS_RT, datos.len(), datos.as_ptr().cast())
}

fn reiniciar(tipo: u32) -> ! {
    (rt().reset_system)(tipo, 0, 0, null());
    loop {}
}

// ---------- Volúmenes ----------

fn volumenes() -> Vec<Handle> {
    let (mut n, mut b) = (0usize, null_mut());
    if (bs().locate_handle_buffer)(BY_PROTOCOL, &SIMPLE_FILE_SYSTEM_GUID, null_mut(), &mut n, &mut b) != 0 || b.is_null() {
        return Vec::new();
    }
    let v = unsafe { core::slice::from_raw_parts(b, n) }.to_vec();
    (bs().free_pool)(b.cast());
    v
}

fn abrir(dir: *mut File, ruta: &str) -> Option<*mut File> {
    let mut f = null_mut();
    ((unsafe { &*dir }.open)(dir, &mut f, u16s(ruta).as_ptr(), FILE_MODE_READ, 0) == 0).then_some(f)
}

fn cerrar(f: *mut File) {
    (unsafe { &*f }.close)(f);
}

/// Carpetas dentro de `ruta` (entradas de EFI_FILE_INFO: atributos en el byte 72, nombre desde el 80).
fn subcarpetas(raiz: *mut File, ruta: &str) -> Vec<String> {
    let mut v = Vec::new();
    let Some(d) = abrir(raiz, ruta) else { return v };
    let mut buf = [0u64; 128];
    for _ in 0..256 {
        let mut n = core::mem::size_of_val(&buf);
        if (unsafe { &*d }.read)(d, &mut n, buf.as_mut_ptr().cast()) != 0 || n < 82 {
            break;
        }
        let b = unsafe { core::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), n) };
        let nombre: Vec<u16> = b[80..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
        let nombre = String::from_utf16_lossy(&nombre);
        if buf[9] & FILE_DIRECTORY != 0 && nombre != "." && nombre != ".." {
            v.push(nombre);
        }
    }
    cerrar(d);
    v
}

/// Rutas de arranque conocidas que existen en el volumen `h`.
fn explorar(h: Handle) -> Vec<String> {
    let mut fs: *mut c_void = null_mut();
    let mut raiz = null_mut();
    if (bs().handle_protocol)(h, &SIMPLE_FILE_SYSTEM_GUID, &mut fs) != 0 {
        return Vec::new();
    }
    let fs = fs.cast::<SimpleFileSystem>();
    if (unsafe { &*fs }.open_volume)(fs, &mut raiz) != 0 {
        return Vec::new();
    }
    let mut v = Vec::new();
    let mut existe = |p: &str| {
        let ok = abrir(raiz, p).map(cerrar).is_some();
        if ok {
            v.push(p.into());
        }
    };
    for p in [a::WINDOWS, a::NIGHTFURY, a::SYSTEMD] {
        existe(p);
    }
    for d in subcarpetas(raiz, "\\EFI") {
        for f in ["shimx64.efi", "grubx64.efi"] {
            existe(&format!("\\EFI\\{d}\\{f}"));
        }
    }
    cerrar(raiz);
    v
}

// ---------- Arranque ----------

/// Ruta de dispositivo del volumen + nodo File Path + nodo fin.
fn ruta_completa(h: Handle, ruta: &str) -> Option<Vec<u8>> {
    let mut p: *mut c_void = null_mut();
    if (bs().handle_protocol)(h, &DEVICE_PATH_GUID, &mut p) != 0 || p.is_null() {
        return None;
    }
    let (mut v, mut q) = (Vec::new(), p as *const u8);
    loop {
        let cab = unsafe { core::slice::from_raw_parts(q, 4) };
        let l = u16::from_le_bytes([cab[2], cab[3]]) as usize;
        if cab[0] == 0x7F && cab[1] == 0xFF {
            break;
        }
        if l < 4 || v.len() > 4096 {
            return None;
        }
        v.extend_from_slice(unsafe { core::slice::from_raw_parts(q, l) });
        q = unsafe { q.add(l) };
    }
    let r: Vec<u8> = ruta.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
    v.extend([4, 4]);
    v.extend(((4 + r.len()) as u16).to_le_bytes());
    v.extend(r);
    v.extend([0x7F, 0xFF, 4, 0]);
    Some(v)
}

fn guardar_ultimo(aj: &mut Ajustes, e: &Entrada) {
    aj.ultimo = Some(e.clave.clone());
    escribir_var(a::VARIABLE, &guid_nf(), &aj.a_bytes());
}

/// Arranca la entrada. Solo vuelve si algo falla o el sistema arrancado termina: devuelve el aviso.
fn arrancar(imagen: Handle, e: &Entrada, det: &[Detectado], vols: &[Handle], aj: &mut Ajustes) -> String {
    match e.tipo {
        Tipo::Reiniciar => reiniciar(RESET_COLD),
        Tipo::Apagar => reiniciar(RESET_SHUTDOWN),
        Tipo::Firmware => {
            let actual = leer_var("OsIndications", &GLOBAL_VARIABLE_GUID).and_then(|b| b.get(..8)?.try_into().ok()).map_or(0, u64::from_le_bytes);
            if escribir_var("OsIndications", &GLOBAL_VARIABLE_GUID, &(actual | 1).to_le_bytes()) != 0 {
                return "El firmware no deja abrir su configuración.".into();
            }
            reiniciar(RESET_COLD)
        }
        Tipo::Uefi(n) => {
            guardar_ultimo(aj, e);
            if escribir_var("BootNext", &GLOBAL_VARIABLE_GUID, &n.to_le_bytes()) != 0 {
                return format!("El firmware no deja arrancar {}.", e.nombre);
            }
            reiniciar(RESET_COLD)
        }
        _ => {
            let Some(d) = det.iter().find(|d| d.entrada.clave == e.clave) else {
                return format!("No se encuentra {} en ningún disco.", e.nombre);
            };
            let Some(ruta) = ruta_completa(vols[d.esp], &d.ruta) else {
                return format!("No se puede localizar el disco de {}.", e.nombre);
            };
            guardar_ultimo(aj, e);
            let mut img = null_mut();
            let s = (bs().load_image)(0, imagen, ruta.as_ptr(), null(), 0, &mut img);
            if s != 0 {
                return format!("No se pudo cargar {} (error {:#x}).", e.nombre, s);
            }
            let s = (bs().start_image)(img, &mut 0, &mut null_mut());
            if s == 0 {
                format!("{} terminó y ha vuelto al menú.", e.nombre)
            } else {
                format!("{} terminó con error {:#x}.", e.nombre, s)
            }
        }
    }
}

#[no_mangle]
pub extern "efiapi" fn efi_main(imagen: Handle, tabla: *mut SystemTable) -> Status {
    unsafe { ST = tabla };
    (bs().set_watchdog_timer)(0, 0, 0, null());
    let o = st().con_out;
    (unsafe { &*o }.enable_cursor)(o, 0);

    let vols = volumenes();
    let rutas: Vec<Vec<String>> = vols.iter().map(|&h| explorar(h)).collect();
    let det = a::detectar(&rutas);
    let orden: Vec<u16> = leer_var("BootOrder", &GLOBAL_VARIABLE_GUID).unwrap_or_default().chunks_exact(2).take(256).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let opciones: Vec<(u16, Vec<u8>)> = orden.iter().filter_map(|&n| Some((n, leer_var(&format!("Boot{n:04X}"), &GLOBAL_VARIABLE_GUID)?))).collect();
    let mut aj = leer_var(a::VARIABLE, &guid_nf()).map_or_else(Ajustes::default, |b| Ajustes::de_bytes(&b));
    let firmware_ok = leer_var("OsIndicationsSupported", &GLOBAL_VARIABLE_GUID).is_some_and(|b| b.first().is_some_and(|x| x & 1 != 0));
    let otras = a::otras_uefi(&orden, &opciones, &det);
    // Configuración lee esta lista (sin ocultas ni nombres); solo se escribe si cambió, para no gastar la NVRAM.
    let completa = a::lista_a_bytes(&a::para_lista(&a::lista(&det, &otras, &Ajustes::default(), firmware_ok)));
    if leer_var(a::VARIABLE_LISTA, &guid_nf()).as_deref() != Some(&completa[..]) {
        escribir_var(a::VARIABLE_LISTA, &guid_nf(), &completa);
    }
    let lista = a::lista(&det, &otras, &aj, firmware_ok);
    let defecto = a::por_defecto(&lista, &aj);
    let mut menu = Menu::nuevo(lista, defecto, aj.segundos);

    let mut temporizador = null_mut();
    (bs().create_event)(EVT_TIMER, TPL_APPLICATION, 0, null_mut(), &mut temporizador);
    (bs().set_timer)(temporizador, TIMER_PERIODIC, 10_000_000);
    // Con 0 segundos no se dibuja nada: solo se muestra el menú si hay una tecla pulsada en este instante.
    let mut accion = None;
    if aj.segundos == 0 {
        accion = match leer_tecla() {
            Some(t) => menu.tecla(t),
            None => menu.tic_segundo(),
        };
    }
    loop {
        if let Some(Accion::Arrancar(i)) = accion.take() {
            let e = menu.lista[i].clone();
            menu.aviso = Some(arrancar(imagen, &e, &det, &vols, &mut aj));
            menu.cuenta = None;
            (bs().set_watchdog_timer)(0, 0, 0, null());
        }
        dibujar(&menu);
        let mut eventos = [unsafe { &*st().con_in }.wait_for_key, temporizador];
        let mut i = 0;
        // Sin cuenta atrás (tecla pulsada) solo se espera la tecla.
        let n = if menu.cuenta.is_some() { 2 } else { 1 };
        if (bs().wait_for_event)(n, eventos.as_mut_ptr(), &mut i) != 0 {
            continue;
        }
        accion = if i == 0 { leer_tecla().and_then(|t| menu.tecla(t)) } else { menu.tic_segundo() };
    }
}
