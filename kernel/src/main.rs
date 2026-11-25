// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! NightFury: núcleo. Solo aporta hardware (vídeo, interrupciones, PS/2, reloj, memoria);
//! todo lo demás vive en `nf-core`.
#![no_std]
#![no_main]

extern crate alloc;

mod ahci;
mod arch;
#[cfg(feature = "demo")]
mod demo;
mod boot;
mod entrada;
mod fb;
mod nvme;
mod paging;
mod pci;
mod pendrive;
mod ps2;
mod red;
mod rtc;
mod serial;
mod uefi;
mod unidades;
mod usb;
mod user;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering::*};
use fb::Fb;
use nf_core::desktop::Desktop;
use nf_core::block::BlockDev;
use nf_core::files::Files;
use nf_core::fs::Nffs;
use nf_core::geom::Rect;
use nf_core::heap::Heap;
use nf_core::icons::{self, Icon};
use nf_core::input::{Event, Keyboard, Ps2Mouse};
use nf_core::platform::{DateTime, MemInfo, PenEstado, Platform};
use nf_core::proc::{ProcInfo, Process};
use nf_core::surface::Surface;
use nf_core::vol::nfvol::SectorBlocks;
use nf_core::{font, NAME};
use serial::traza;

/// Trazas por COM1 al arrancar.
const TRAZA: bool = true;
/// Traza además cada byte crudo de teclado y ratón.
const TRAZA_BYTES: bool = false;
// ---------- Montón ----------

struct Global(UnsafeCell<Heap>);
// Un solo núcleo y las interrupciones no reservan memoria.
unsafe impl Sync for Global {}

static HEAP: Global = Global(UnsafeCell::new(Heap::empty()));
static HEAP_LISTO: AtomicBool = AtomicBool::new(false);

struct Asignador;

unsafe impl GlobalAlloc for Asignador {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        (*HEAP.0.get()).alloc(l.size(), l.align())
    }
    unsafe fn dealloc(&self, p: *mut u8, _l: Layout) {
        (*HEAP.0.get()).free(p)
    }
}

/// Programas que trae el sistema (los compila `tools\programas.ps1` antes que el núcleo).
const PROGRAMAS_SISTEMA: &[(&str, &[(&str, &[u8])])] = &[
    (
        "Calculadora",
        &[
            ("app.txt", include_bytes!("../../programs/calculadora/app.txt")),
            ("programa.nfx", include_bytes!("../../target/programas/Calculadora/programa.nfx")),
        ],
    ),
    (
        "Reloj",
        &[
            ("app.txt", include_bytes!("../../programs/reloj/app.txt")),
            ("programa.nfx", include_bytes!("../../target/programas/Reloj/programa.nfx")),
            ("icono.bmp", include_bytes!("../../programs/reloj/icono.bmp")),
        ],
    ),
    (
        "Paint",
        &[
            ("app.txt", include_bytes!("../../programs/paint/app.txt")),
            ("programa.nfx", include_bytes!("../../target/programas/Paint/programa.nfx")),
            ("icono.bmp", include_bytes!("../../programs/paint/icono.bmp")),
        ],
    ),
    // v0.10: las apps que antes iban dentro del núcleo.
    (
        "Bloc de notas",
        &[
            ("app.txt", include_bytes!("../../programs/bloc-notas/app.txt")),
            ("programa.nfx", include_bytes!("../../target/programas/Bloc de notas/programa.nfx")),
        ],
    ),
    (
        "Terminal",
        &[
            ("app.txt", include_bytes!("../../programs/terminal/app.txt")),
            ("programa.nfx", include_bytes!("../../target/programas/Terminal/programa.nfx")),
        ],
    ),
    (
        "Configuración",
        &[
            ("app.txt", include_bytes!("../../programs/configuracion/app.txt")),
            ("programa.nfx", include_bytes!("../../target/programas/Configuración/programa.nfx")),
        ],
    ),
    (
        "Explorador de archivos",
        &[
            ("app.txt", include_bytes!("../../programs/explorador/app.txt")),
            ("programa.nfx", include_bytes!("../../target/programas/Explorador de archivos/programa.nfx")),
        ],
    ),
    // v0.12.4: el navegador (programa del sistema: no pide permiso de red).
    (
        "Navegador",
        &[
            ("app.txt", include_bytes!("../../programs/navegador/app.txt")),
            ("programa.nfx", include_bytes!("../../target/programas/Navegador/programa.nfx")),
            ("icono.bmp", include_bytes!("../../programs/navegador/icono.bmp")),
        ],
    ),
];

/// Fuentes que trae el sistema (v0.12.4; Liberation, licencia OFL 1.1 en LICENSE), en `C:\Sistema\fuentes`.
const FUENTES_SISTEMA: &[(&str, &[u8])] = &[
    ("LiberationSans-Regular.ttf", include_bytes!("../../nf-core/data/fuentes/LiberationSans-Regular.ttf")),
    ("LiberationSans-Bold.ttf", include_bytes!("../../nf-core/data/fuentes/LiberationSans-Bold.ttf")),
    ("LiberationSans-Italic.ttf", include_bytes!("../../nf-core/data/fuentes/LiberationSans-Italic.ttf")),
    ("LiberationSans-BoldItalic.ttf", include_bytes!("../../nf-core/data/fuentes/LiberationSans-BoldItalic.ttf")),
    ("LiberationSerif-Regular.ttf", include_bytes!("../../nf-core/data/fuentes/LiberationSerif-Regular.ttf")),
    ("LiberationSerif-Bold.ttf", include_bytes!("../../nf-core/data/fuentes/LiberationSerif-Bold.ttf")),
    ("LiberationSerif-Italic.ttf", include_bytes!("../../nf-core/data/fuentes/LiberationSerif-Italic.ttf")),
    ("LiberationSerif-BoldItalic.ttf", include_bytes!("../../nf-core/data/fuentes/LiberationSerif-BoldItalic.ttf")),
    ("LiberationMono-Regular.ttf", include_bytes!("../../nf-core/data/fuentes/LiberationMono-Regular.ttf")),
    ("LiberationMono-Bold.ttf", include_bytes!("../../nf-core/data/fuentes/LiberationMono-Bold.ttf")),
    ("LiberationMono-Italic.ttf", include_bytes!("../../nf-core/data/fuentes/LiberationMono-Italic.ttf")),
    ("LiberationMono-BoldItalic.ttf", include_bytes!("../../nf-core/data/fuentes/LiberationMono-BoldItalic.ttf")),
    ("LICENSE", include_bytes!("../../nf-core/data/fuentes/LICENSE")),
    ("AUTHORS", include_bytes!("../../nf-core/data/fuentes/AUTHORS")),
];

#[global_allocator]
static ASIGNADOR: Asignador = Asignador;

// ---------- Plataforma ----------

struct Plat {
    info: boot::BootInfo,
    files: Files,
    pool: nf_core::keys::Pool,
    net: nf_core::net::Net,
    pen: pendrive::Pen,
    /// Discos internos (SATA y NVMe) que aún no se han montado: se montan al elegir modo en el menú del pendrive.
    internos: Option<Vec<unidades::Interno>>,
}

fn cpu_name() -> String {
    use core::arch::x86_64::__cpuid;
    let mut bytes = [0u8; 48];
    let max = __cpuid(0x8000_0000).eax;
    let n = if max >= 0x8000_0004 {
        for i in 0..3u32 {
            let r = __cpuid(0x8000_0002 + i);
            for (j, v) in [r.eax, r.ebx, r.ecx, r.edx].iter().enumerate() {
                bytes[i as usize * 16 + j * 4..][..4].copy_from_slice(&v.to_le_bytes());
            }
        }
        48
    } else {
        let r = __cpuid(0);
        for (j, v) in [r.ebx, r.edx, r.ecx].iter().enumerate() {
            bytes[j * 4..][..4].copy_from_slice(&v.to_le_bytes());
        }
        12
    };
    let end = bytes[..n].iter().position(|&b| b == 0).unwrap_or(n);
    let mut out = String::new();
    for w in core::str::from_utf8(&bytes[..end]).unwrap_or("").split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

impl Plat {
    fn reset(&self, kind: u32) {
        if !self.info.runtime.is_null() {
            unsafe { ((*self.info.runtime).reset_system)(kind, 0, 0, core::ptr::null()) };
        }
    }
}

impl Platform for Plat {
    fn now(&mut self) -> DateTime {
        rtc::now()
    }
    fn set_now(&mut self, t: DateTime) {
        rtc::poner(t);
    }
    fn uptime_ms(&self) -> u64 {
        arch::TICKS.load(Relaxed) * 10
    }
    fn mem(&self) -> MemInfo {
        let h = unsafe { &*HEAP.0.get() };
        MemInfo { total: self.info.ram_total, heap_total: h.total() as u64, heap_used: h.used() as u64 }
    }
    fn cpu(&self) -> String {
        cpu_name()
    }
    fn screen(&self) -> (u32, u32) {
        (self.info.width, self.info.height)
    }
    fn reboot(&mut self) {
        self.reset(uefi::RESET_COLD);
        unsafe { arch::outb(0x64, 0xFE) };
    }
    fn shutdown(&mut self) {
        self.reset(uefi::RESET_SHUTDOWN);
    }
    fn arranque_leer(&mut self) -> Option<nf_core::arranque::InfoArranque> {
        entrada::arranque_leer(self.info.runtime)
    }
    fn arranque_guardar(&mut self, a: &nf_core::arranque::Ajustes) -> Result<(), &'static str> {
        entrada::arranque_guardar(self.info.runtime, a)
    }
    fn arranque_primero(&mut self) -> Result<(), &'static str> {
        entrada::arranque_primero(self.info.runtime)
    }
    fn files(&mut self) -> &mut Files {
        &mut self.files
    }
    fn pool(&mut self) -> &mut nf_core::keys::Pool {
        &mut self.pool
    }
    fn spawn(&mut self, nfx: &[u8], name: &str) -> Result<Box<dyn Process>, String> {
        user::spawn(nfx, name)
    }
    fn processes(&mut self) -> Vec<ProcInfo> {
        user::lista()
    }
    fn kill_process(&mut self, id: u64) {
        user::finalizar(id)
    }
    fn run_programs(&mut self) {
        // `correr` exige las interrupciones cerradas, como en `arch::idle`.
        unsafe {
            core::arch::asm!("cli", options(nomem, nostack));
            user::correr();
            core::arch::asm!("sti", options(nomem, nostack));
        }
    }
    fn net(&mut self) -> Option<&mut dyn nf_core::net::NetApi> {
        Some(&mut self.net)
    }
    fn is_system_binary(&self, nfx: &[u8]) -> bool {
        PROGRAMAS_SISTEMA.iter().flat_map(|(_, items)| items.iter()).any(|(n, b)| *n == "programa.nfx" && *b == nfx)
    }
    fn entropy(&mut self) -> u64 {
        entropia()
    }
    fn pen_menu(&self) -> bool {
        self.pen.menu
    }
    fn pen_estado(&mut self) -> PenEstado {
        let t = self.uptime_ms();
        self.pen.estado(t)
    }
    fn pen_probar(&mut self) {
        self.files.set_probar();
        self.pen.elegir(pendrive::Eleccion::Probar);
    }
    fn pen_discos(&mut self) -> Vec<nf_core::platform::PenDisco> {
        if !self.pen.menu {
            return Vec::new();
        }
        let lista = self.internos.as_deref().unwrap_or(&[]);
        lista.iter().map(|d| nf_core::platform::PenDisco { modelo: String::from(d.modelo()), bytes: d.bytes(), partes: d.contenido() }).collect()
    }
    fn pen_instalar(&mut self, i: usize) -> Result<bool, nf_core::fs::FsError> {
        use nf_core::fs::FsError;
        // Solo desde el menú del pendrive y sobre un disco interno de la lista: nada más se escribe nunca.
        if !self.pen.menu {
            return Err(FsError::Invalid);
        }
        let disco = self.internos.as_deref().and_then(|l| l.get(i)).ok_or(FsError::NotFound)?;
        let efi = self.pen.efi()?;
        let arranque = self.pen.arranque();
        let esp = disco.instalar(&efi, &arranque, nf_core::VERSION)?;
        let ruta = if arranque.is_empty() { nf_core::vol::instalar::NF_NUCLEO } else { nf_core::vol::instalar::NF_ARRANQUE };
        match entrada::crear(self.info.runtime, &esp, ruta) {
            Ok(()) => Ok(true),
            Err(e) => {
                traza!("NF instalar: sin entrada de arranque UEFI: {}", e);
                Ok(false)
            }
        }
    }
    fn pen_portatil(&mut self) -> Result<bool, nf_core::fs::FsError> {
        let now = rtc::now();
        let (nuevo, files) = self.pen.portatil(now)?;
        self.files = files;
        self.pen.elegir(pendrive::Eleccion::Portatil);
        Ok(nuevo)
    }
}

/// 64 bits impredecibles: ver `Platform::entropy`.
fn entropia() -> u64 {
    use core::arch::x86_64::{__cpuid, _rdtsc};
    // Contador de ciclos del procesador y reloj del sistema: siempre disponibles.
    #[allow(unused_unsafe)]
    let mut v = unsafe { _rdtsc() } ^ arch::TICKS.load(Relaxed).rotate_left(40);
    // RDRAND (Intel SDM vol. 2, CPUID.01H:ECX bit 30) si el procesador lo tiene.
    #[allow(unused_unsafe)]
    let has = unsafe { __cpuid(1) }.ecx & (1 << 30) != 0;
    if has {
        // hasta 10 intentos, como recomienda Intel: RDRAND puede fallar un momento si se agota
        for _ in 0..10 {
            let r: u64;
            let ok: u8;
            unsafe { core::arch::asm!("rdrand {r}", "setc {ok}", r = out(reg) r, ok = out(reg_byte) ok, options(nomem, nostack)) };
            if ok != 0 {
                v ^= r;
                break;
            }
        }
    }
    v
}

// ---------- Disco de datos ----------

/// Elige el disco del sistema (C:) entre los SATA y NVMe y devuelve aparte el resto, que son internos.
/// 1.º un disco con NightFury instalado (GPT con una partición NFFS: C: es esa partición); 2.º, como siempre,
/// el primer SATA que sea de NightFury entero o esté en blanco. Con `todo_interno` (arranque desde un pendrive)
/// no se ofrece ninguno: C: no es un disco del equipo.
fn elegir_disco(todo_interno: bool, sata: Vec<Box<ahci::Disco>>, nvme: Vec<nvme::Espacio>) -> (Option<Box<dyn BlockDev>>, Vec<unidades::Interno>) {
    let mut lista = unidades::preparar(sata, nvme);
    if todo_interno {
        return (None, lista);
    }
    if let Some(k) = lista.iter().position(|d| d.instalada().is_some()) {
        let d = lista.remove(k);
        let (inicio, sectores) = d.instalada().unwrap_or_default();
        traza!("NF disco: «{}» lleva NightFury instalado (partición desde {} con {} sectores): es C:", d.modelo(), inicio, sectores);
        let dev: Box<dyn BlockDev> = Box::new(SectorBlocks { dev: Box::new(d.manejador(false)), start: inicio, sectors: sectores });
        return (Some(dev), lista);
    }
    for k in 0..lista.len() {
        let motivo = match &mut *lista[k].fis.borrow_mut() {
            unidades::Fisico::Sata(d) => motivo_sata(&mut **d),
            unidades::Fisico::Nvme(_) => "",
        };
        if TRAZA {
            traza!("NF disco: «{}» {} MiB ({}) {}", lista[k].modelo(), lista[k].bytes() >> 20, motivo, if motivo.is_empty() { "es interno" } else { "se ofrece" });
        }
        if !motivo.is_empty() {
            if let Some(d) = lista.remove(k).en_sata() {
                return (Some(d), lista);
            }
            break;
        }
    }
    if TRAZA {
        traza!("NF disco: ninguno ofrecido");
    }
    (None, lista)
}

/// Qué tiene un disco SATA entero que NightFury pueda usar como C: ("" si nada).
fn motivo_sata(d: &mut ahci::Disco) -> &'static str {
    if Nffs::is_formatted(d) {
        "NightFury"
    } else if nf_core::cryptdev::detect(d).is_some() {
        // Cifrado total: el NFFS no se ve, pero la cabecera del último bloque es la nuestra.
        "NightFury cifrado"
    } else if Nffs::is_blank(d) {
        "en blanco"
    } else {
        ""
    }
}

// ---------- Núcleo ----------

#[no_mangle]
extern "C" fn kmain() -> ! {
    let info = boot::info();
    let fb = Fb::new(&info);
    if TRAZA {
        serial::init();
    }
    unsafe { (*HEAP.0.get()).init(info.heap_base as *mut u8, info.heap_size as usize) };
    HEAP_LISTO.store(true, Release);
    paging::init(info.ram_end, info.fb_base + info.stride as u64 * info.height as u64 * 4);
    arch::init_gdt();
    arch::init_idt();
    arch::init_pic();
    arch::init_pit();
    user::init_syscall();

    pantalla_de_arranque(&fb);
    let con_rueda = ps2::init();
    let n_usb = usb::init();
    if TRAZA {
        traza!("NF usb: {} controlador(es) xHCI iniciado(s)", n_usb);
    }

    let abars = pci::buscar_ahci();
    if abars.is_empty() && TRAZA {
        traza!("NF disco: sin controlador AHCI");
    }
    let (disco, internos) = elegir_disco(info.usb.is_some(), abars.into_iter().flat_map(ahci::sondear).collect(), nvme::sondear());
    let files = Files::probe(disco, rtc::now());
    if TRAZA {
        traza!("NF archivos: persistente={} formato_pendiente={}", files.persistent(), files.pending_format());
    }
    // Secreto de la red: el acumulador todavía no tiene movimientos del ratón, así que se mezclan
    // 32 lecturas del generador del procesador y del reloj fino.
    let mut pool = nf_core::keys::Pool::new();
    for _ in 0..32 {
        pool.add(&entropia().to_le_bytes());
    }
    let mut secreto = [0u8; 32];
    pool.take(&mut secreto);
    let mut net = nf_core::net::Net::new(secreto);
    if let Some(dev) = red::sondear() {
        traza!("NF red: {} {}", dev.name(), nf_core::net::mac_text(dev.mac()));
        net.attach(dev);
    }
    let mut plat = Plat { info, files, pool, net, pen: pendrive::Pen::new(info.usb), internos: Some(internos) };
    let (w, h) = (info.width as i32, info.height as i32);
    // La letra TrueType del sistema sale de los bytes incluidos: no depende del disco (bloqueado al iniciar sesión).
    let fuente = |n: &str| FUENTES_SISTEMA.iter().find(|f| f.0 == n).map_or(&[][..], |f| f.1);
    nf_core::letra::incluidas(fuente("LiberationSans-Regular.ttf"), fuente("LiberationSans-Bold.ttf"), fuente("LiberationMono-Regular.ttf"));
    let mut desk = Desktop::new(w, h, &mut plat);

    if TRAZA {
        let t = plat.now();
        traza!(
            "NF arranque: {}x{} formato={} pitch={} ram={} MiB heap={} MiB cpu='{}' fecha={:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            w,
            h,
            if info.bgr { "BGR" } else { "RGB" },
            info.stride,
            info.ram_total >> 20,
            info.heap_size >> 20,
            plat.cpu(),
            t.year,
            t.month,
            t.day,
            t.hour,
            t.minute,
            t.second
        );
    }
    #[cfg(feature = "demo")]
    demo::lanzar(&mut desk, &mut plat);
    // Los discos internos se enganchan antes que ningún USB. Arrancando de un pendrive, esperan a que se elija
    // modo (Probar los monta de solo lectura; Instalar no los monta): hasta entonces nada del equipo se toca.
    let mut unidades = unidades::Unidades::default();
    if !plat.pen.menu {
        let lista = plat.internos.take().unwrap_or_default();
        unidades.internos(&mut plat.files, lista, false);
    }
    unsafe { core::arch::asm!("sti") };

    let mut teclado = Keyboard::new();
    let mut raton = Ps2Mouse::new();
    raton.set_wheel(con_rueda);
    let mut usb_teclado: Vec<u8> = Vec::new();
    let mut usb_raton: Vec<nf_core::input::MousePacket> = Vec::new();
    let mut usb_mandos: Vec<nf_core::gamepad::PadEvent> = Vec::new();
    let mut ultimo = u64::MAX;
    let mut programas_listos = false;
    let mut estado_red = None;
    loop {
        // Programas del sistema: se instalan (o actualizan) en cuanto el disco está listo.
        if let Some(e) = plat.pen.eleccion.take() {
            if let Some(lista) = plat.internos.take() {
                unidades.internos(&mut plat.files, lista, e == pendrive::Eleccion::Probar);
            }
        }
        if !programas_listos && !plat.pen.menu && !plat.files.disk_locked() && !plat.files.pending_format() {
            programas_listos = true;
            let now = plat.now();
            for (dir, items) in PROGRAMAS_SISTEMA {
                if let Err(e) = plat.files.install_system_program(dir, items, now) {
                    traza!("NF programas: no se pudo instalar {}: {:?}", dir, e);
                }
            }
            if let Err(e) = plat.files.install_system_files("fuentes", FUENTES_SISTEMA, now) {
                traza!("NF fuentes: no se pudieron instalar: {:?}", e);
            }
        }
        while let Some(b) = ps2::TECLADO.pop() {
            if TRAZA_BYTES {
                traza!("tecla byte={:#04x}", b);
            }
            if let Some(k) = teclado.feed(b) {
                desk.event(Event::Key(k), &mut plat);
            }
        }
        while let Some(b) = ps2::RATON.pop() {
            if TRAZA_BYTES {
                traza!("raton byte={:#04x}", b);
            }
            if let Some(p) = raton.feed(b) {
                desk.event(Event::Mouse(p), &mut plat);
            }
        }
        // USB: sondeo del xHCI; el teclado entra por el mismo decodificador que el PS/2.
        usb::poll(arch::TICKS.load(Relaxed) * 10, &mut usb_teclado, &mut usb_raton, &mut usb_mandos);
        unidades.sondear(&mut plat.files, &mut plat.pool, &mut plat.pen);
        plat.net.poll(arch::TICKS.load(Relaxed) * 10);
        if TRAZA {
            let st = plat.net.status();
            if Some(st.state) != estado_red {
                estado_red = Some(st.state);
                let ip = |x: Option<nf_core::net::Ipv4>| x.map_or(String::from("-"), nf_core::net::ip_text);
                traza!("NF red: {:?} ip={} puerta={} dns={:?}", st.state, ip(st.ip), ip(st.gateway), st.dns);
            }
        }
        for b in usb_teclado.drain(..) {
            if let Some(k) = teclado.feed(b) {
                desk.event(Event::Key(k), &mut plat);
            }
        }
        for p in usb_raton.drain(..) {
            desk.event(Event::Mouse(p), &mut plat);
        }
        for e in usb_mandos.drain(..) {
            desk.gamepad(e, &mut plat);
        }
        let t = arch::TICKS.load(Relaxed);
        if t != ultimo {
            ultimo = t;
            desk.event(Event::Tick(t * 10), &mut plat);
        }
        for r in desk.render(&mut plat) {
            fb.present(desk.back(), r);
        }
        arch::idle(|| !ps2::TECLADO.vacia() || !ps2::RATON.vacia() || arch::TICKS.load(Relaxed) != ultimo);
    }
}

fn pantalla_de_arranque(fb: &Fb) {
    const FONDO: u32 = 0x0B1030;
    fb.clear(FONDO);
    let (sw, sh) = fb.size();
    let (w, h) = (400.min(sw), 260.min(sh));
    let mut s = Surface::new(w, h, FONDO);
    icons::draw(&mut s, Icon::Logo, (w - 128) / 2, 30, 128, 0xFFFFFF);
    font::draw_in(&mut s, Rect::new(0, 180, w, 40), NAME, 20, true, 0xFFFFFF, font::Align::Center);
    fb.present_at(&s, s.bounds(), (sw - w) / 2, (sh - h) / 2);
}

// ---------- Fallos ----------

/// Búfer fijo en pila para formatear sin usar el montón.
struct Buf {
    b: [u8; 512],
    n: usize,
}

impl Write for Buf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &c in s.as_bytes() {
            if self.n < self.b.len() {
                self.b[self.n] = if c == b'\n' || c == b'\r' { b' ' } else { c };
                self.n += 1;
            }
        }
        Ok(())
    }
}

impl Buf {
    fn text(&self) -> &str {
        match core::str::from_utf8(&self.b[..self.n]) {
            Ok(s) => s,
            Err(e) => core::str::from_utf8(&self.b[..e.valid_up_to()]).unwrap_or(""),
        }
    }
}

static EN_FALLO: AtomicBool = AtomicBool::new(false);

fn pantalla_de_error(detalle: &str) -> ! {
    unsafe { core::arch::asm!("cli") };
    const FONDO: u32 = 0x2A1040;
    let fb = Fb::new(&boot::info());
    fb.clear(FONDO);
    // Un segundo fallo mientras se dibuja (p. ej. sin memoria): se queda el fondo.
    if HEAP_LISTO.load(Acquire) && !EN_FALLO.swap(true, AcqRel) {
        let (sw, sh) = fb.size();
        let (w, h) = (1000.min(sw), 320.min(sh));
        let mut s = Surface::new(w, h, FONDO);
        let (x, mut y) = (24, 24);
        font::draw(&mut s, x, y, "NightFury ha tenido un problema y se ha detenido.", 14, true, 0xFFFFFF);
        y += font::line_h(14) * 2;
        let por_linea = ((w - 2 * x) / font::adv('M', 9, false).max(1)).max(1) as usize;
        let mut resto = detalle;
        while !resto.is_empty() && y < h - 80 {
            let corte = resto.char_indices().nth(por_linea).map_or(resto.len(), |(i, _)| i);
            font::draw(&mut s, x, y, &resto[..corte], 9, false, 0xFFFFFF);
            resto = &resto[corte..];
            y += font::line_h(9);
        }
        font::draw(&mut s, x, h - 48, "Mantén pulsado el botón de encendido para apagar.", 9, false, 0xFFFFFF);
        fb.present_at(&s, s.bounds(), (sw - w) / 2, (sh - h) / 2);
    }
    loop {
        unsafe { core::arch::asm!("cli", "hlt") };
    }
}

#[no_mangle]
extern "C" fn fault_handler(vector: u64, frame: *const u64) -> ! {
    // Con código de error la pila empieza por él y el RIP está una posición más allá.
    let con_codigo = matches!(vector, 8 | 10 | 11 | 12 | 13 | 14 | 17 | 21 | 29 | 30);
    let rip = unsafe { *frame.add(con_codigo as usize) };
    // CS del marco: si era un programa (ring 3) solo se mata al programa; el sistema sigue.
    if unsafe { *frame.add(con_codigo as usize + 1) } & 3 == 3 {
        user::fallo(vector, rip);
    }
    let mut b = Buf { b: [0; 512], n: 0 };
    let _ = write!(b, "Excepcion {} en la direccion {:#018x}", vector, rip);
    if TRAZA {
        traza!("NF fallo: {}", b.text());
    }
    pantalla_de_error(b.text())
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut b = Buf { b: [0; 512], n: 0 };
    let _ = write!(b, "{}", info);
    if TRAZA {
        traza!("NF panico: {}", b.text());
    }
    pantalla_de_error(b.text())
}
