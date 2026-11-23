// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Pruebas de extremo a extremo: dos (o más) equipos con su propio sistema de archivos y una red falsa
//! en memoria que los conecta (UDP por difusión, TCP, permisos `Pending`/`Denied`).
use super::*;
use crate::almacen::Estado;
use crate::motor::Visto;
use crate::proto::{anuncio, PUERTO_TCP, PUERTO_UDP};
use nf_core::apps::Ctx;
use nf_core::net::*;
use nf_core::platform::{DateTime, MemInfo, TestPlatform};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq)]
enum P {
    Pendiente,
    Si,
    No,
}

struct Sock {
    dueno: usize,
    par: usize,
    rx: Vec<u8>,
    estado: TcpState,
    vivo: bool,
}
struct Udp {
    dueno: usize,
    puerto: u16,
    cola: VecDeque<Datagram>,
    vivo: bool,
}
struct Escucha {
    dueno: usize,
    puerto: u16,
    cola: VecDeque<usize>,
    vivo: bool,
}

const EXTERNO: usize = usize::MAX;

struct Mundo {
    ips: Vec<Ipv4>,
    /// Permisos de cada equipo: [usar la red, aceptar conexiones].
    perm: Vec<[P; 2]>,
    socks: Vec<Sock>,
    udps: Vec<Udp>,
    escuchas: Vec<Escucha>,
    /// Cambia un byte de todo lo que envía el equipo 0 que no sea un saludo.
    alterar: bool,
    /// Bytes máximos por `tcp_send` (envíos parciales).
    trozo: usize,
}

type M = Rc<RefCell<Mundo>>;

impl Mundo {
    fn nuevo(n: usize) -> M {
        Rc::new(RefCell::new(Mundo { ips: (0..n).map(|i| [192, 168, 1, 10 + i as u8]).collect(), perm: vec![[P::Si, P::Si]; n], socks: vec![], udps: vec![], escuchas: vec![], alterar: false, trozo: usize::MAX }))
    }
    fn permiso(&self, h: usize, k: usize) -> Result<(), NetError> {
        match self.perm[h][k] {
            P::Si => Ok(()),
            P::No => Err(NetError::Denied),
            P::Pendiente => Err(NetError::Pending),
        }
    }
    fn par(&mut self, a: usize, b: usize) -> (usize, usize) {
        let lleno = |dueno, par| Sock { dueno, par, rx: vec![], estado: TcpState::Open, vivo: true };
        self.socks.push(lleno(a, self.socks.len() + 1));
        self.socks.push(lleno(b, self.socks.len() - 1));
        (self.socks.len() - 2, self.socks.len() - 1)
    }
    /// Un cliente que no es ningún equipo (para probar conexiones hostiles): devuelve su socket.
    fn externo_conecta(&mut self, host: usize, puerto: u16) -> usize {
        let l = self.escuchas.iter().position(|l| l.vivo && l.dueno == host && l.puerto == puerto).expect("nadie escucha");
        let (c, s) = self.par(EXTERNO, host);
        self.escuchas[l].cola.push_back(s);
        c
    }
    fn externo_envia(&mut self, c: usize, datos: &[u8]) {
        let p = self.socks[c].par;
        self.socks[p].rx.extend_from_slice(datos);
    }
    fn inyectar_udp(&mut self, host: usize, datos: &[u8]) {
        if let Some(u) = self.udps.iter_mut().find(|u| u.vivo && u.dueno == host && u.puerto == PUERTO_UDP) {
            u.cola.push_back(Datagram { from: [192, 168, 1, 99], port: 5555, data: datos.to_vec() });
        }
    }
}

struct Red_ {
    m: M,
    yo: usize,
}

fn estado_ok(ip: Ipv4) -> NetStatus {
    NetStatus { state: State::Connected, adapter: String::from("falsa"), mac: [0; 6], speed_mbps: 1000, full_duplex: true, ip: Some(ip), mask: Some([255, 255, 255, 0]), gateway: None, dns: vec![], dhcp_server: None, lease_s: 0, lease_since_ms: 0 }
}

impl NetApi for Red_ {
    fn status(&mut self) -> NetStatus {
        estado_ok(self.m.borrow().ips[self.yo])
    }
    fn renew(&mut self) {}
    fn ping(&mut self, _: Ipv4, _: u16, _: u8) -> Result<PingId, NetError> {
        Err(NetError::NoAdapter)
    }
    fn ping_poll(&mut self, _: PingId) -> Option<Result<PingReply, NetError>> {
        None
    }
    fn resolve(&mut self, _: &str) -> Result<QueryId, NetError> {
        Err(NetError::NoAdapter)
    }
    fn resolve_poll(&mut self, _: QueryId) -> Option<Result<Vec<Ipv4>, NetError>> {
        None
    }
    fn tcp_connect(&mut self, dst: Ipv4, port: u16) -> Result<SockId, NetError> {
        let mut m = self.m.borrow_mut();
        m.permiso(self.yo, 0)?;
        let Some(h) = m.ips.iter().position(|i| *i == dst) else { return Err(NetError::Unreachable(dst)) };
        let Some(l) = m.escuchas.iter().position(|l| l.vivo && l.dueno == h && l.puerto == port) else { return Err(NetError::Refused) };
        let (c, s) = m.par(self.yo, h);
        m.escuchas[l].cola.push_back(s);
        Ok(SockId(c as u32))
    }
    fn tcp_state(&mut self, s: SockId) -> TcpState {
        self.m.borrow().socks.get(s.0 as usize).map_or(TcpState::Failed(NetError::Gone), |k| k.estado.clone())
    }
    fn tcp_send(&mut self, s: SockId, data: &[u8]) -> Result<usize, NetError> {
        let mut m = self.m.borrow_mut();
        let k = s.0 as usize;
        if !m.socks.get(k).is_some_and(|x| x.vivo && x.dueno == self.yo) {
            return Err(NetError::Gone);
        }
        let n = data.len().min(m.trozo);
        let mut trozo = data[..n].to_vec();
        if m.alterar && self.yo == 0 && n != 69 {
            let ultimo = trozo.len() - 1;
            trozo[ultimo] ^= 1;
        }
        let p = m.socks[k].par;
        m.socks[p].rx.extend_from_slice(&trozo);
        Ok(n)
    }
    fn tcp_recv(&mut self, s: SockId, buf: &mut [u8]) -> Result<usize, NetError> {
        let mut m = self.m.borrow_mut();
        let k = m.socks.get_mut(s.0 as usize).filter(|x| x.vivo && x.dueno == self.yo).ok_or(NetError::Gone)?;
        let n = buf.len().min(k.rx.len());
        buf[..n].copy_from_slice(&k.rx[..n]);
        k.rx.drain(..n);
        Ok(n)
    }
    fn tcp_close(&mut self, s: SockId) {
        let mut m = self.m.borrow_mut();
        let k = s.0 as usize;
        m.socks[k].vivo = false;
        let p = m.socks[k].par;
        m.socks[p].estado = TcpState::PeerClosed;
    }
    fn tcp_abort(&mut self, s: SockId) {
        let mut m = self.m.borrow_mut();
        let k = s.0 as usize;
        m.socks[k].vivo = false;
        let p = m.socks[k].par;
        m.socks[p].estado = TcpState::Failed(NetError::Reset);
    }
    fn udp_bind(&mut self, port: u16) -> Result<UdpId, NetError> {
        let mut m = self.m.borrow_mut();
        m.permiso(self.yo, if port == 0 { 0 } else { 1 })?;
        if port != 0 && m.udps.iter().any(|u| u.vivo && u.dueno == self.yo && u.puerto == port) {
            return Err(NetError::InUse);
        }
        let puerto = if port == 0 { 49152 + m.udps.len() as u16 } else { port };
        let yo = self.yo;
        m.udps.push(Udp { dueno: yo, puerto, cola: VecDeque::new(), vivo: true });
        Ok(UdpId(m.udps.len() as u32 - 1))
    }
    fn udp_send(&mut self, s: UdpId, dst: Ipv4, port: u16, data: &[u8]) -> Result<(), NetError> {
        let mut m = self.m.borrow_mut();
        m.permiso(self.yo, 0)?;
        let (from, desde) = (m.ips[self.yo], m.udps[s.0 as usize].puerto);
        let yo = self.yo;
        for u in m.udps.iter_mut() {
            // La difusión llega a los demás equipos (no a uno mismo) con el puerto fijo abierto.
            if u.vivo && u.puerto == port && u.dueno != yo && dst == [255; 4] {
                u.cola.push_back(Datagram { from, port: desde, data: data.to_vec() });
            }
        }
        Ok(())
    }
    fn udp_recv(&mut self, s: UdpId) -> Result<Option<Datagram>, NetError> {
        Ok(self.m.borrow_mut().udps.get_mut(s.0 as usize).filter(|u| u.vivo).ok_or(NetError::Gone)?.cola.pop_front())
    }
    fn udp_close(&mut self, s: UdpId) {
        self.m.borrow_mut().udps[s.0 as usize].vivo = false;
    }
    fn tcp_listen(&mut self, port: u16) -> Result<ListenId, NetError> {
        let mut m = self.m.borrow_mut();
        m.permiso(self.yo, 1)?;
        let yo = self.yo;
        m.escuchas.push(Escucha { dueno: yo, puerto: port, cola: VecDeque::new(), vivo: true });
        Ok(ListenId(m.escuchas.len() as u32 - 1))
    }
    fn tcp_accept(&mut self, l: ListenId) -> Result<Option<(SockId, Ipv4, u16)>, NetError> {
        let mut m = self.m.borrow_mut();
        m.permiso(self.yo, 1)?;
        let Some(s) = m.escuchas[l.0 as usize].cola.pop_front() else { return Ok(None) };
        let de = m.socks[m.socks[s].par].dueno;
        Ok(Some((SockId(s as u32), m.ips.get(de).copied().unwrap_or([192, 168, 1, 99]), 50000)))
    }
    fn tcp_unlisten(&mut self, l: ListenId) {
        self.m.borrow_mut().escuchas[l.0 as usize].vivo = false;
    }
    fn connections(&mut self) -> Vec<ConnInfo> {
        vec![]
    }
    fn set_owner(&mut self, _: Proto, _: u32, _: u64) {}
}

/// Un equipo: su sistema de archivos, su semilla de azar y su tarjeta en el mundo falso.
struct Host {
    base: TestPlatform,
    red: Option<Red_>,
}

impl Host {
    fn nuevo(m: &M, yo: usize) -> Host {
        Host { base: TestPlatform { size: (1280, 800), seed: 1000 + yo as u64 * 77, ..Default::default() }, red: Some(Red_ { m: m.clone(), yo }) }
    }
    fn sin_red() -> Host {
        Host { base: TestPlatform::default(), red: None }
    }
}

impl Platform for Host {
    fn now(&mut self) -> DateTime {
        self.base.now()
    }
    fn uptime_ms(&self) -> u64 {
        self.base.uptime_ms()
    }
    fn mem(&self) -> MemInfo {
        self.base.mem()
    }
    fn cpu(&self) -> String {
        self.base.cpu()
    }
    fn screen(&self) -> (u32, u32) {
        self.base.screen()
    }
    fn reboot(&mut self) {}
    fn shutdown(&mut self) {}
    fn files(&mut self) -> &mut nf_core::files::Files {
        self.base.files()
    }
    fn entropy(&mut self) -> u64 {
        self.base.entropy()
    }
    fn pool(&mut self) -> &mut nf_core::keys::Pool {
        self.base.pool()
    }
    fn net(&mut self) -> Option<&mut dyn NetApi> {
        self.red.as_mut().map(|r| r as &mut dyn NetApi)
    }
}

/// Avanza el reloj de `desde` a `hasta` ms de 50 en 50, dando un tick a cada equipo.
fn avanzar(hs: &mut [Host], apps: &mut [Mensajes], desde: u64, hasta: u64) -> u64 {
    let mut t = desde;
    while t < hasta {
        for (h, a) in hs.iter_mut().zip(apps.iter_mut()) {
            h.base.ms = t;
            let mut th = Theme::default();
            let mut cx = Ctx::new(h, &mut th, t);
            a.tick(&mut cx);
        }
        t += 50;
    }
    t
}

fn ev(key: Key, ch: Option<char>) -> KeyEvent {
    KeyEvent { key, ch, down: true, shift: false, ctrl: false, alt: false }
}

fn tecla(a: &mut Mensajes, h: &mut Host, t: u64, key: Key, ch: Option<char>) {
    let mut th = Theme::default();
    let mut cx = Ctx::new(h, &mut th, t);
    a.key(&ev(key, ch), &mut cx);
}

fn clic(a: &mut Mensajes, h: &mut Host, t: u64, x: i32, y: i32) -> Option<DialogRequest> {
    let mut th = Theme::default();
    let mut cx = Ctx::new(h, &mut th, t);
    a.mouse(MouseEv::Down { x, y }, &mut cx);
    cx.dialog.take()
}

fn foto(a: &mut Mensajes, h: &mut Host, w: i32, hh: i32) -> Surface {
    let mut s = Surface::new(w, hh, 0);
    a.draw(&mut s, &Theme::default(), h, true);
    s
}

fn motor(a: &Mensajes) -> &Motor {
    a.motor.as_ref().unwrap()
}

fn escribe(a: &mut Mensajes, h: &mut Host, t: u64, texto: &str) {
    for c in texto.chars() {
        tecla(a, h, t, Key::Char(c), Some(c));
    }
}

/// Dos equipos que ya se ven (con A seleccionado sobre B) y el reloj en ese momento.
fn pareja() -> (M, [Host; 2], [Mensajes; 2], u64) {
    let m = Mundo::nuevo(2);
    let mut hs = [Host::nuevo(&m, 0), Host::nuevo(&m, 1)];
    let mut apps = [Mensajes::new(), Mensajes::new()];
    let t = avanzar(&mut hs, &mut apps, 0, 12_000);
    (m, hs, apps, t)
}

#[test]
fn se_encuentran_y_se_envian_mensajes_cifrados() {
    let m = Mundo::nuevo(2);
    m.borrow_mut().trozo = 100;
    let mut hs = [Host::nuevo(&m, 0), Host::nuevo(&m, 1)];
    let mut apps = [Mensajes::new(), Mensajes::new()];
    // El equipo A empieza con los dos avisos de permiso sin contestar: primero «usar la red», luego «aceptar».
    m.borrow_mut().perm[0] = [P::Pendiente, P::Pendiente];
    let mut t = avanzar(&mut hs, &mut apps, 0, 300);
    assert_eq!(motor(&apps[0]).red, Red::Esperando(Permiso::Usar));
    assert_eq!(apps[0].franja().0, "Esperando permiso para usar la red…");
    m.borrow_mut().perm[0][0] = P::Si;
    t = avanzar(&mut hs, &mut apps, t, t + 1000);
    assert_eq!(motor(&apps[0]).red, Red::Esperando(Permiso::Aceptar));
    m.borrow_mut().perm[0][1] = P::Si;
    t = avanzar(&mut hs, &mut apps, t, t + 12_000);
    assert_eq!(motor(&apps[0]).red, Red::Activo);
    let (a, b) = (motor(&apps[0]).publica, motor(&apps[1]).publica);
    assert_ne!(a, b);
    assert_eq!(motor(&apps[0]).contactos.len(), 1);
    assert_eq!(motor(&apps[0]).contactos[0].pk, b);
    assert!(motor(&apps[0]).contactos[0].en_linea(t) && motor(&apps[1]).contactos[0].en_linea(t));
    assert_eq!(apps[0].franja().0, "1 equipo en la red local");

    // A elige a B con el ratón, escribe y envía con Enter.
    foto(&mut apps[0], &mut hs[0], 640, 420);
    assert!(clic(&mut apps[0], &mut hs[0], t, 50, HEAD_H + 10).is_none());
    assert_eq!(apps[0].sel, Some(b));
    escribe(&mut apps[0], &mut hs[0], t, "  Hola, ¿qué tal?  ");
    tecla(&mut apps[0], &mut hs[0], t, Key::Enter, None);
    assert!(apps[0].linea.vacia());
    assert_eq!(motor(&apps[0]).contactos[0].msgs[0].estado, Estado::Pendiente);
    t = avanzar(&mut hs, &mut apps, t, t + 3000);
    let enviado = &motor(&apps[0]).contactos[0].msgs[0];
    assert_eq!((enviado.estado, enviado.texto.as_str()), (Estado::Entregado, "Hola, ¿qué tal?"));
    let recibido = &motor(&apps[1]).contactos[0];
    assert_eq!((recibido.pk, recibido.msgs.len(), recibido.msgs[0].saliente, recibido.msgs[0].texto.as_str()), (a, 1, false, "Hola, ¿qué tal?"));
    assert!(recibido.sin_leer && recibido.guardado);
    assert_eq!(motor(&apps[0]).conexiones() + motor(&apps[1]).conexiones(), 0, "las conexiones se cierran");

    // Se guardó todo (el de A en disco, con el estado «E»).
    let carpeta = format!("{}\\Mensajes", hs[0].fs().home());
    let hist = String::from_utf8(hs[0].fs().read(&format!("{carpeta}\\{}.txt", huella(&b))).unwrap()).unwrap();
    assert!(hist.starts_with(">\t2026-11-15 21:37:05\t") && hist.trim_end().ends_with("\tE\tHola, ¿qué tal?"), "{hist}");
    assert!(hs[0].fs().exists(&format!("{carpeta}\\identidad")));

    // Mismo id otra vez (la confirmación se «perdió»): B no lo guarda dos veces y A lo da por entregado.
    {
        let c = &mut apps[0].motor.as_mut().unwrap().contactos[0];
        c.msgs[0].estado = Estado::Pendiente;
    }
    t = avanzar(&mut hs, &mut apps, t, t + 3000);
    assert_eq!(motor(&apps[1]).contactos[0].msgs.len(), 1);
    assert_eq!(motor(&apps[0]).contactos[0].msgs[0].estado, Estado::Entregado);

    // Y B contesta: usa su propia conexión.
    foto(&mut apps[1], &mut hs[1], 640, 420);
    clic(&mut apps[1], &mut hs[1], t, 50, HEAD_H + 10);
    assert!(!motor(&apps[1]).contactos[0].sin_leer, "al abrir la conversación se marca leída");
    escribe(&mut apps[1], &mut hs[1], t, "Bien");
    clic(&mut apps[1], &mut hs[1], t, 640 - 50, 420 - STRIP_H - IN_H + 20);
    t = avanzar(&mut hs, &mut apps, t, t + 3000);
    assert_eq!(motor(&apps[1]).contactos[0].msgs[1].estado, Estado::Entregado);
    assert_eq!(motor(&apps[0]).contactos[0].msgs[1].texto, "Bien");

    // Al reabrir el programa en A vuelven la identidad, el contacto y el historial.
    let mut nueva = Mensajes::new();
    let mut th = Theme::default();
    nueva.tick(&mut Ctx::new(&mut hs[0], &mut th, t));
    let n = motor(&nueva);
    assert_eq!((n.publica, n.contactos.len(), n.contactos[0].msgs.len()), (a, 1, 2));
    assert_eq!(n.contactos[0].msgs[0].estado, Estado::Entregado);
    assert!(n.contactos[0].guardado && !n.contactos[0].en_linea(t));
}

#[test]
fn palabras_de_verificacion_y_marca() {
    let (_m, mut hs, mut apps, t) = pareja();
    let b = motor(&apps[1]).publica;
    foto(&mut apps[0], &mut hs[0], 640, 420);
    clic(&mut apps[0], &mut hs[0], t, 50, HEAD_H + 10);
    assert_eq!(apps[0].franja().1, false);
    // El botón de la cabecera abre el diálogo con las seis palabras y la huella.
    let Some(DialogRequest::Message { tag, text, buttons, .. }) = clic(&mut apps[0], &mut hs[0], t, 640 - 50, 20) else { panic!("sin diálogo") };
    assert_eq!((tag, buttons), (TAG_VERIF, Buttons::YesNo));
    let p = palabras(&motor(&apps[0]).publica, &b);
    assert!(text.contains(&p.join(" ")) && text.contains(&huella(&b)) && text.contains("¿Coinciden con las que ve la otra persona?"), "{text}");
    assert_eq!(p, palabras(&b, &motor(&apps[0]).publica));
    let mut th = Theme::default();
    apps[0].dialog_result(DialogResult::Answer { tag: TAG_VERIF, answer: Answer::Yes }, &mut Ctx::new(&mut hs[0], &mut th, t));
    assert!(motor(&apps[0]).contactos[0].verificado);
    avanzar(&mut hs, &mut apps, t, t + 1500);
    let carpeta = format!("{}\\Mensajes", hs[0].fs().home());
    let c = String::from_utf8(hs[0].fs().read(&format!("{carpeta}\\contactos")).unwrap()).unwrap();
    assert!(c.starts_with(&nf_core::crypto::to_hex(&b)) && c.trim_end().ends_with("\t1"), "{c}");
    // Con otro contacto de otra clave y el mismo nombre, se avisa.
    let mut otro = motor::Contacto::nuevo([9; 32], motor(&apps[0]).contactos[0].nombre.clone(), true);
    otro.visto = None;
    let m0 = apps[0].motor.as_mut().unwrap();
    m0.contactos.push(otro);
    assert!(m0.otra_clave(&m0.contactos[1]) && !m0.otra_clave(&m0.contactos[0]));
    // «No» desmarca.
    apps[0].verificando = Some(b);
    apps[0].dialog_result(DialogResult::Answer { tag: TAG_VERIF, answer: Answer::No }, &mut Ctx::new(&mut hs[0], &mut th, t));
    assert!(!motor(&apps[0]).contactos[0].verificado);
}

#[test]
fn permiso_denegado_y_reintentar() {
    let m = Mundo::nuevo(2);
    m.borrow_mut().perm[0] = [P::No, P::Si];
    let mut hs = [Host::nuevo(&m, 0), Host::nuevo(&m, 1)];
    let mut apps = [Mensajes::new(), Mensajes::new()];
    let t = avanzar(&mut hs, &mut apps, 0, 3000);
    assert_eq!(motor(&apps[0]).red, Red::Denegado(Permiso::Usar));
    assert_eq!(apps[0].franja(), (String::from("Sin permiso para usar la red. Puedes cambiarlo en Configuración → Red."), true));
    // Reintentar sin cambiar nada: sigue denegado. Tras cambiarlo en Configuración: funciona.
    foto(&mut apps[0], &mut hs[0], 640, 420);
    clic(&mut apps[0], &mut hs[0], t, 640 - 50, 420 - 13);
    let t = avanzar(&mut hs, &mut apps, t, t + 500);
    assert_eq!(motor(&apps[0]).red, Red::Denegado(Permiso::Usar));
    m.borrow_mut().perm[0] = [P::Si, P::Si];
    clic(&mut apps[0], &mut hs[0], t, 640 - 50, 420 - 13);
    let t = avanzar(&mut hs, &mut apps, t, t + 12_000);
    assert_eq!(motor(&apps[0]).red, Red::Activo);
    assert_eq!(motor(&apps[0]).contactos.len(), 1);
    // Aceptar conexiones denegado da su propio texto.
    let m2 = Mundo::nuevo(1);
    m2.borrow_mut().perm[0] = [P::Si, P::No];
    let mut h = [Host::nuevo(&m2, 0)];
    let mut a = [Mensajes::new()];
    avanzar(&mut h, &mut a, 0, t.min(500));
    assert_eq!(motor(&a[0]).red, Red::Denegado(Permiso::Aceptar));
    assert!(a[0].franja().0.starts_with("Sin permiso para aceptar conexiones."));
}

#[test]
fn sin_tarjeta_de_red() {
    let mut h = [Host::sin_red()];
    let mut a = [Mensajes::new()];
    avanzar(&mut h, &mut a, 0, 300);
    assert_eq!(a[0].franja(), (String::from("Sin conexión de red"), false));
    // También se dibuja sin contacto elegido.
    let s = foto(&mut a[0], &mut h[0], 640, 420);
    assert!(s.px.iter().any(|&p| p != 0xFFFFFF));
}

#[test]
fn un_equipo_con_otra_identidad_corta_y_lo_pendiente_sigue_pendiente() {
    let (_m, mut hs, mut apps, t) = pareja();
    let b_ip = [192, 168, 1, 11];
    // A cree que habla con Z, pero en esa IP está B: B responde con otra identidad.
    {
        let m0 = apps[0].motor.as_mut().unwrap();
        m0.contactos.clear();
        let mut z = motor::Contacto::nuevo([7; 32], String::from("Z"), true);
        z.visto = Some(Visto { ip: b_ip, puerto: PUERTO_TCP, ms: t });
        m0.contactos.push(z);
        let z_pk = m0.contactos[0].pk;
        assert!(m0.enviar(&mut hs[0], &z_pk, "secreto"));
    }
    // Se mantiene a Z «en línea» durante la prueba.
    let mut t = t;
    for _ in 0..40 {
        apps[0].motor.as_mut().unwrap().contactos[0].visto.as_mut().unwrap().ms = t;
        t = avanzar(&mut hs, &mut apps, t, t + 50);
    }
    assert_eq!(motor(&apps[0]).contactos[0].msgs[0].estado, Estado::Pendiente);
    assert!(motor(&apps[1]).contactos.iter().all(|c| c.msgs.is_empty()), "B no recibió nada");
    assert_eq!(motor(&apps[0]).conexiones(), 0);
}

#[test]
fn una_trama_alterada_corta_la_conexion() {
    let (m, mut hs, mut apps, mut t) = pareja();
    let b = motor(&apps[1]).publica;
    assert!({
        let a = apps[0].motor.as_mut().unwrap();
        a.enviar(&mut hs[0], &b, "hola")
    });
    m.borrow_mut().alterar = true;
    t = avanzar(&mut hs, &mut apps, t, t + 4000);
    assert_eq!(motor(&apps[0]).contactos[0].msgs[0].estado, Estado::Pendiente);
    assert!(motor(&apps[1]).contactos[0].msgs.is_empty(), "lo alterado no se acepta");
    assert_eq!(motor(&apps[1]).conexiones(), 0);
    // Sin la alteración, se entrega (tras el respiro de reintento).
    m.borrow_mut().alterar = false;
    avanzar(&mut hs, &mut apps, t, t + 35_000);
    assert_eq!(motor(&apps[0]).contactos[0].msgs[0].estado, Estado::Entregado);
}

#[test]
fn lo_que_llega_de_fuera_es_hostil() {
    let m = Mundo::nuevo(1);
    let mut hs = [Host::nuevo(&m, 0)];
    let mut apps = [Mensajes::new()];
    let mut t = avanzar(&mut hs, &mut apps, 0, 300);
    assert_eq!(motor(&apps[0]).red, Red::Activo);
    let propia = motor(&apps[0]).publica;
    // Anuncios rotos, enormes, con nuestra clave o de nombre malo: no crean nada.
    let mut largo = anuncio(&[5; 32], "x");
    largo.extend_from_slice(&[b'a'; 1400]);
    for d in [vec![], vec![0u8; 1472], largo, anuncio(&propia, "yo"), anuncio(&[5; 32], &"n".repeat(33)), anuncio(&[5; 32], "a\u{7}b"), b"NFM1".to_vec()] {
        m.borrow_mut().inyectar_udp(0, &d);
    }
    t = avanzar(&mut hs, &mut apps, t, t + 300);
    assert!(motor(&apps[0]).contactos.is_empty());
    // 100 equipos distintos: la tabla se queda en 64.
    for n in 1..=100u8 {
        m.borrow_mut().inyectar_udp(0, &anuncio(&[n; 32], "pc"));
    }
    t = avanzar(&mut hs, &mut apps, t, t + 500);
    assert_eq!(motor(&apps[0]).contactos.len(), motor::MAX_VISTOS);
    // Conexiones entrantes: como mucho 8 a la vez, y una basura se corta.
    let ext: Vec<usize> = (0..20).map(|_| m.borrow_mut().externo_conecta(0, PUERTO_TCP)).collect();
    t = avanzar(&mut hs, &mut apps, t, t + 100);
    assert!(motor(&apps[0]).conexiones() <= 8);
    for &e in &ext[..8] {
        m.borrow_mut().externo_envia(e, &[0xAA; 5000]);
    }
    t = avanzar(&mut hs, &mut apps, t, t + 200);
    assert_eq!(m.borrow().socks[ext[0]].estado, TcpState::Failed(NetError::Reset), "basura: se corta");
    // Los que no dicen nada se cortan a los 5 s.
    t = avanzar(&mut hs, &mut apps, t, t + 6000);
    assert_eq!(motor(&apps[0]).conexiones(), 0);
    assert!(ext.iter().all(|&e| m.borrow().socks[e].estado == TcpState::Failed(NetError::Reset)));
    // Un saludo con la clave de orden pequeño (todo ceros) también se corta.
    let c = m.borrow_mut().externo_conecta(0, PUERTO_TCP);
    let hola = proto::hola(proto::HOLA_I, &[3; 32], &[0; 32]);
    m.borrow_mut().externo_envia(c, &hola);
    avanzar(&mut hs, &mut apps, t, t + 200);
    assert_eq!(m.borrow().socks[c].estado, TcpState::Failed(NetError::Reset));
}

#[test]
fn la_identidad_sale_del_azar_y_se_conserva() {
    let m = Mundo::nuevo(2);
    let mut hs = [Host::nuevo(&m, 0), Host::nuevo(&m, 1)];
    let mut apps = [Mensajes::new(), Mensajes::new()];
    avanzar(&mut hs, &mut apps, 0, 100);
    let (a, b) = (motor(&apps[0]).publica, motor(&apps[1]).publica);
    assert_ne!(a, b);
    let carpeta = format!("{}\\Mensajes", hs[0].fs().home());
    let guardada = String::from_utf8(hs[0].fs().read(&format!("{carpeta}\\identidad")).unwrap()).unwrap();
    assert_eq!(guardada.len(), 64);
    // La pública es la de esa privada (y no se guardó nada más en esa carpeta todavía).
    let privada: [u8; 32] = nf_core::crypto::from_hex(&guardada).unwrap().try_into().unwrap();
    assert_eq!(proto::publica(&privada), a);
    // Un archivo de identidad roto se reemplaza por otro nuevo.
    let ruta = format!("{}\\Mensajes\\identidad", hs[1].fs().home());
    hs[1].fs().write(&ruta, b"basura", DateTime::default()).unwrap();
    let mut otra = Mensajes::new();
    otra.tick(&mut Ctx::new(&mut hs[1], &mut Theme::default(), 0));
    assert_ne!(motor(&otra).publica, b);
}

#[test]
fn dibuja_en_cualquier_tamano_y_con_cualquier_estado() {
    let (_m, mut hs, mut apps, t) = pareja();
    foto(&mut apps[0], &mut hs[0], 640, 420);
    clic(&mut apps[0], &mut hs[0], t, 50, HEAD_H + 10);
    escribe(&mut apps[0], &mut hs[0], t, &"palabra ".repeat(80));
    for (w, h) in [(640, 420), (480, 300), (300, 200), (100, 100), (10, 10), (0, 0), (1000, 700)] {
        foto(&mut apps[0], &mut hs[0], w, h);
    }
    // Rueda y teclas de página con y sin contenido.
    let mut th = Theme::default();
    let mut cx = Ctx::new(&mut hs[0], &mut th, t);
    for dz in [-5, 5, 100, -100] {
        apps[0].mouse(MouseEv::Wheel { x: 400, y: 200, dz }, &mut cx);
        apps[0].mouse(MouseEv::Wheel { x: 50, y: 200, dz }, &mut cx);
    }
    apps[0].mouse(MouseEv::Move { x: 3, y: 3 }, &mut cx);
    apps[0].mouse(MouseEv::Leave, &mut cx);
    apps[0].key(&ev(Key::PageUp, None), &mut cx);
    apps[0].key(&ev(Key::PageDown, None), &mut cx);
    drop(cx);
    // Cierra: suelta la red y borra la privada.
    apps[0].release(&mut hs[0]);
    assert_eq!(motor(&apps[0]).conexiones(), 0);
    assert!(apps[0].motor.as_ref().unwrap().privada_borrada());
}

#[test]
fn el_ejemplo_de_las_capturas_se_dibuja() {
    let mut h = Host::sin_red();
    let mut app = crear_demo(&mut h);
    let mut s = Surface::new(640, 420, 0);
    app.draw(&mut s, &Theme::default(), &mut h, true);
    assert!(s.px.iter().any(|&p| p == Theme::default().accent), "burbujas propias con el color de acento");
}
