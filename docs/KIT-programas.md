# Kit de programas de NightFury (v0.12.3)

Un programa NightFury es un ejecutable en modo usuario escrito en Rust, sin dependencias externas:
usa el mismo trait `App` que las apps del sistema (`nf-core`) y el runtime `nf-app`.

## Requisitos

```powershell
rustup target add x86_64-unknown-none
```

Nada más: ni Visual Studio ni librerías extra.

## Hacer un programa

1. Copia la carpeta `programs\plantilla` a `programs\mi_programa`.
2. En `programs\mi_programa\Cargo.toml` cambia `name = "plantilla"` por `name = "mi_programa"`
   (el nombre del paquete **tiene que ser el de la carpeta**) y añade la carpeta a `members` del
   `Cargo.toml` de la raíz.
3. Edita `src\main.rs`: la plantilla ya tiene una ventana que dibuja un texto y cuenta clics.
4. Rellena `app.txt` (una clave por línea):

   ```
   nombre=Mi programa
   version=1.0
   autor=Yo
   icono=app
   ancho=480
   alto=320
   abre=txt,md
   ```

   `nombre` es también el nombre de la carpeta de salida. `icono` es el icono del sistema que se
   usará (`app` si no sabes cuál) cuando el programa no trae uno propio. `abre` (opcional) son las
   extensiones que el programa sabe abrir, sin punto y separadas por comas (hasta 32, de 16 letras como
   mucho): al abrir un archivo así, el escritorio lo abre con él y se lo pasa en `App::open_path`. Si un
   programa del sistema declara la misma extensión, gana el del sistema.
5. (Opcional) Icono propio: un `icono.bmp` en la carpeta del programa (ver "Icono propio").

## Compilar

```powershell
.\tools\programas.ps1
```

Compila todos los de `programs\` y deja, para cada uno, `target\programas\<nombre>\programa.nfx`, su
`app.txt` y su `icono.bmp` si lo tiene. Si algo falla, sale con código distinto de 0. Lo hace `nfpack`, que también comprueba que
el `.nfx` es válido y muestra sus secciones.

Para instalarlo, copia esa carpeta al disco de NightFury y elígela en Configuración → Aplicaciones →
«Instalar programa…».

## Icono propio

Si en la carpeta del programa hay un `icono.bmp`, el sistema lo dibuja en el escritorio, el menú Inicio, la
barra de título y la barra de tareas en vez del icono de `icono=`.

- BMP sin comprimir de **24 o 32 bits**, cuadrado, hasta 256×256 (lo normal: 32×32).
- En **24 bits** el magenta puro `FF00FF` es transparente. En **32 bits** manda el canal alfa (si es todo cero,
  la imagen se toma opaca).
- `tools\programas.ps1` lo copia junto al `.nfx`. Los iconos de Reloj y Paint los dibuja con `nf-core`
  `cargo run -p nfprog -- iconos` (ver `tools\nfprog\src\main.rs`, sirve de ejemplo para hacer el tuyo).
- Con `cargo run -p nfprog -- capturas shots` ves cómo quedan Reloj, Paint y el Administrador de tareas sin
  arrancar el sistema: los programas con su lógica en `src\lib.rs` (y un `main.rs` de tres líneas) se pueden
  probar y fotografiar en el PC; mira `programs\reloj` y `programs\paint`.

## Multitarea

Cada programa corre por su cuenta: puedes tardar en atender un evento sin congelar el escritorio ni los
demás programas, así que ya no hace falta volver rápido de cada `key`, `mouse` o `tick`. Pero el sistema
vigila: si pasan **5 segundos** sin que el programa recoja los eventos que se le envían, su ventana pone
"(No responde)" en el título y el usuario puede finalizarlo (también desde el Administrador de tareas,
Ctrl+Mayús+Esc). Para trabajos largos, reparte el trabajo en trozos pequeños entre eventos.

## Cómo es un programa

```rust
#![no_std]
#![no_main]
extern crate alloc;
// ... impl App for MiPrograma ...
nf_app::programa!(Box::new(MiPrograma::new()), None);   // None = sin tick; Some(16) = cada 16 ms
```

- `App::draw` pinta **todo** el contenido; se llama al cambiar tamaño, foco o tema y cuando pones
  `cx.redraw = true`.
- `key`, `mouse` y `tick` reciben un `Ctx`: `cx.redraw`, `cx.close` (cerrar la ventana), `cx.now_ms`,
  `cx.sys.now()` (fecha y hora), `cx.theme`.
- `title()` y `min_size()` se leen tras cada evento; si cambian, la ventana se actualiza sola.
- `request_close` decide qué hacer cuando el usuario pulsa la X (devuelve `true` = cerrar).
- Tienes `alloc` (`Vec`, `String`, `Box`…): el montón crece solo hasta 256 MiB en total.
- Si el programa entra en pánico, el mensaje va al registro (puerto serie) y termina con código 101.

### Disco, diálogos, menús y arrastre (v0.10)

Un programa tiene lo mismo que una app del sistema:

- **Disco real**: `cx.sys.fs()` (el trait `FilesApi`), con los permisos del usuario de la sesión. Cada
  llamada viaja al sistema (`SERVICE`) y el programa espera la respuesta. `cx.sys.files()` sigue siendo una
  unidad en memoria: no la uses para el disco.
- **Diálogos** (`cx.dialog`), **menús** (`cx.menu`), **abrir rutas** (`cx.open`, `cx.open_new`,
  `cx.open_terminal`), **arrastrar archivos** (`cx.drag`) y **cambiar el tema** (`cx.theme` +
  `cx.theme_changed`): el runtime los manda al escritorio (`REQUEST`) y las respuestas llegan a
  `dialog_result`, `menu_result`, `drop_files`, `open_path`, `drag_over` y `drag_source`. Los diálogos de
  Abrir y Guardar devuelven la ruta elegida.

Lo que un programa **de terceros** no puede (se le contesta `FsError::Denied` o se ignora):

- escribir, borrar, mover o leer en `C:\Sistema`;
- escribir, borrar o mover en `C:\Programas` (instalar y desinstalar es cosa de Configuración);
- arrastrar rutas de esas carpetas;
- cuentas, contraseñas, cifrado, reiniciar o apagar.

Eso solo lo hacen los programas del sistema que vienen con NightFury (Bloc de notas, Terminal,
Configuración y Explorador), que el sistema reconoce por su binario exacto.

### Unidades extraíbles y mandos (v0.11)

- **Pendrives y discos USB** aparecen como `D:`, `E:`… y se usan con el mismo `cx.sys.fs()` que `C:`
  (`D:\carpeta\archivo.txt`), con los permisos del usuario. `fs().drives()` da la lista (`DriveInfo`: letra,
  etiqueta, formato, tamaño, libre, `read_only`, `removable`, `locked`). Las ISO y UDF son de solo lectura
  (al escribir, `FsError::Denied`) y una unidad cifrada sin desbloquear no se puede abrir. Si el usuario quita el
  pendrive, las llamadas sobre esa letra fallan: trátalo como cualquier error de disco.
- **Discos internos y formatos de solo lectura (v0.11.1).** Las particiones de los discos SATA y NVMe del PC
  también salen como letras (`removable` a `false`) y son de solo lectura, igual que NTFS y ext2/3/4 en
  cualquier unidad: escribir da `FsError::Denied`. Una unidad con BitLocker sale con `locked` y no se abre.
  `list()` devuelve también los archivos ocultos y de sistema; el Explorador los esconde por su cuenta.
- Formatear, expulsar, cifrar y desbloquear unidades es solo para los programas del sistema.
- **Mandos**: `App::gamepads(&[PadInfo], cx)` llega al abrir la ventana y cada vez que se conecta o quita
  uno (`id`, `kind`: genérico, DualShock 4, DualSense, Switch Pro, Xbox 360, Xbox One, y `name`).
  `App::gamepad(id, &PadState, cx)` llega **solo a la ventana activa**, cuando cambia algo y al ganar el foco;
  al perderlo, da todos los botones por sueltos.
  - Botones por posición (`nf_core::gamepad::BTN_*`): `BTN_SOUTH` es el de abajo (A en Xbox, ✕ en
    PlayStation, B en Nintendo), y además `EAST/WEST/NORTH`, `L1 R1 L2 R2`, `SELECT START HOME`, `L3 R3`,
    la cruceta `UP DOWN LEFT RIGHT` y `EXTRA` (panel táctil o captura).
  - Ejes `state.axes[AXIS_LX..=AXIS_RT]`: sticks de −32768 a 32767 (Y negativa = arriba) y gatillos de 0 a
    32767. En el ABI son los eventos `EV_GAMEPAD` (20) y `EV_GAMEPADS` (21); ver `nf-core\src\abi.rs`.

### Red (v0.12.1)

Desde la v0.12.1 cualquier programa puede usar la red con `cx.sys.net()` (trait `nf_core::net::NetApi`):
DNS, ping, TCP saliente, UDP y hacer de servidor TCP. Todo es **sin bloqueo**: pides algo y lo consultas en
los siguientes `tick` (pon un tick, por ejemplo `Some(50)`, en `programa!`).

**Permisos.** La primera vez que el programa usa la red, el sistema pregunta al usuario, como el cortafuegos
de Windows. Hay dos avisos, y cada uno se recuerda por separado:

| Aviso | Lo piden |
|---|---|
| «¿Permitir que X use la red?» | `resolve`, `ping`, `tcp_connect`, `tcp_connect_tls`, `udp_send`, `udp_bind(0)` |
| «¿Permitir que X acepte conexiones de otros equipos?» | `udp_bind(puerto)` con puerto fijo, `tcp_listen`, `tcp_accept` |

- Mientras el aviso está en pantalla la llamada devuelve `Err(NetError::Pending)`: vuelve a intentarlo en otro
  tick. Si el usuario dice que no, `Err(NetError::Denied)`; dilo en la ventana (el usuario puede cambiarlo en
  Configuración → Red) y no te cuelgues.
- El permiso va ligado al contenido exacto del `.nfx`: si publicas una versión nueva, vuelve a preguntar.
- Los puertos 1–1023 son del sistema (`Err(NetError::InUse)`). `renew` no hace nada y `status()` da la MAC
  a ceros.
- **Entrantes solo de la red local**: un puerto TCP a la escucha solo acepta equipos de la misma subred; un
  socket UDP recibe respuestas de quien le enviaste y, si tiene puerto fijo, también de la red local y por
  difusión.

**Llamadas.**

| Llamada | Qué hace |
|---|---|
| `status()` | estado, IP, máscara, puerta de enlace, DNS |
| `resolve(nombre)` / `resolve_poll(id)` | DNS: `None` mientras espera, luego `Some(Ok(ips))` |
| `ping(ip, bytes, ttl)` / `ping_poll(id)` | eco ICMP |
| `tcp_connect(ip, puerto)` → `SockId` | conecta; mira `tcp_state(s)` hasta `Open` o `Failed(e)` |
| `tcp_send(s, datos)` | envía lo que quepa y devuelve cuántos bytes ha aceptado: reenvía el resto después |
| `tcp_recv(s, buf)` | copia lo recibido; `Ok(0)` si de momento no hay nada |
| `tcp_connect_tls(ip, puerto, nombre)` → `SockId` | como `tcp_connect`, con TLS (HTTPS): ver «Conexión segura» |
| `tcp_tls_info(s)` | `Some(TlsInfo)` con versión, cifrado, titular del certificado y caducidad |
| `tcp_close(s)` / `tcp_abort(s)` | cierre limpio / corte inmediato |
| `tcp_listen(puerto)` → `ListenId` | escucha (cola de 8) |
| `tcp_accept(l)` | `Some((s, ip, puerto))` si hay una conexión nueva |
| `tcp_unlisten(l)` | deja de escuchar |
| `udp_bind(puerto)` → `UdpId` | `0` = puerto libre al azar (49152–65535) |
| `udp_send(u, ip, puerto, datos)` | un datagrama de 1472 bytes como mucho; `255.255.255.255` = difusión |
| `udp_recv(u)` | `Some(Datagram { from, port, data })` o `None` |
| `udp_close(u)` | libera el socket |
| `connections()` | las conexiones de tu programa (lo que ve el Administrador de tareas) |

Lo que el programa abre se cierra solo al terminar. Como mucho 32 sockets UDP y 16 puertos a la escucha en
todo el equipo.

```rust
fn tick(&mut self, cx: &mut Ctx) {
    let Some(net) = cx.sys.net() else { return };
    if self.sock.is_none() {
        match net.tcp_connect([192, 168, 1, 20], 8080) {
            Ok(s) => self.sock = Some(s),
            Err(NetError::Pending) => {}                       // el aviso está abierto: luego
            Err(NetError::Denied) => self.texto = "Sin permiso para usar la red".into(),
            Err(e) => self.texto = format!("{e:?}"),
        }
        return;
    }
    let s = self.sock.unwrap();
    if net.tcp_state(s) == TcpState::Open && !self.enviado {
        self.enviado = net.tcp_send(s, b"hola") == Ok(4);
    }
    let mut buf = [0u8; 512];
    if let Ok(n) = net.tcp_recv(s, &mut buf) {
        self.recibido.extend_from_slice(&buf[..n]);
        cx.redraw |= n > 0;
    }
}
```

**Conexión segura (v0.12.2).** `tcp_connect_tls(ip, puerto, nombre)` abre una conexión TLS (HTTPS)
al servidor de esa `ip`; `nombre` es el que debe llevar su certificado (resuélvelo antes con `resolve`).
Mismo `SockId`, y `tcp_state`, `tcp_send`, `tcp_recv`, `tcp_close` y `tcp_abort` funcionan igual, pero con
**texto claro**: el sistema guarda las claves y cifra y descifra por ti. Pide el **mismo permiso** que
`tcp_connect` (`Pending`/`Denied` igual). `tcp_state` sigue en `Connecting` mientras dura el saludo seguro.

- El certificado **siempre** se comprueba (raíces de Configuración → Certificados, nombre y fechas) y un
  programa de terceros **no puede saltárselo**. Si falla, `tcp_state` da `Failed(NetError::Tls(motivo))` con
  un texto en español listo para mostrar («El certificado ha caducado…», «…no corresponde a este nombre…»);
  lo mismo si el servidor corta durante el saludo o solo habla un TLS antiguo.
- El nombre solo puede tener letras, cifras, `-`, `.` y `_` (253 como mucho); si no, `Err(NetError::BadUrl)`.
- `tcp_send` y `tcp_recv` mueven como mucho 64 KiB por llamada, igual que en TCP.
- La fecha de los certificados sale del reloj del equipo: con el reloj mal, un certificado bueno parecerá
  caducado.

```rust
// Pide «GET /» por HTTPS a una IP ya resuelta (tick a tick, como el resto de la red).
fn tick(&mut self, cx: &mut Ctx) {
    let Some(net) = cx.sys.net() else { return };
    let s = match self.sock {
        Some(s) => s,
        None => match net.tcp_connect_tls(self.ip, 443, "ejemplo.com") {
            Ok(s) => { self.sock = Some(s); return }
            Err(NetError::Pending) => return,
            Err(e) => { self.texto = format!("{e:?}"); return }
        },
    };
    match net.tcp_state(s) {
        TcpState::Failed(NetError::Tls(motivo)) => self.texto = motivo, // «El certificado ha caducado…»
        TcpState::Open if !self.enviado => {
            self.enviado = net.tcp_send(s, b"GET / HTTP/1.1
Host: ejemplo.com
Connection: close

").is_ok();
        }
        _ => {}
    }
    let mut buf = [0u8; 512];
    if let Ok(n) = net.tcp_recv(s, &mut buf) {
        self.recibido.extend_from_slice(&buf[..n]); // texto claro
    }
}
```

El ejemplo completo es **Mensajes** (`programs\mensajes`): descubre equipos por difusión UDP, conecta por
TCP y cifra de extremo a extremo con `nf_core::crypto` (X25519, ChaCha20-Poly1305, HKDF). Su protocolo está
en `docs\Mensajes-protocolo.md`. Para probarlo instálalo desde Configuración → Aplicaciones en dos equipos de
la misma red.

**Cifrado.** `nf_core::crypto` trae `sha256`, `hmac`, `hkdf_extract`/`hkdf_expand`, `x25519` (y
`X25519_BASE`), `chacha20_xor`, `poly1305` y `aead_seal`/`aead_open` (ChaCha20-Poly1305). Están probados con
los vectores de los RFC 7748, 8439 y 5869, pero son implementaciones propias **no auditadas**.

## Guardar un archivo (`SAVE_AS`)

```rust
// Dentro de un `key`/`mouse`/`tick`:
if !nf_app::guardar_como("dibujo.bmp", &bytes) {
    // no se pudo pedir: nombre no válido (sin \ / :), más de 32 MiB o ya hay un guardado en curso
}
```

El sistema abre el diálogo "Guardar como" (el usuario elige carpeta y nombre; empieza en Imágenes para `.bmp`
y en Documentos para lo demás) y escribe los datos con los permisos del usuario. **El programa nunca ve la
ruta.** Solo puede haber un guardado a la vez. La respuesta (`EV_SAVED` en el ABI) llega después a
`App::dialog_result` con la etiqueta `nf_app::TAG_GUARDADO`:

| Resultado | Significa |
|---|---|
| `DialogResult::Path { tag: TAG_GUARDADO, path: Some(nombre) }` | guardado (`nombre` es el propuesto, no la ruta real) |
| `DialogResult::Path { tag: TAG_GUARDADO, path: None }` | el usuario canceló |
| `DialogResult::Answer { tag: TAG_GUARDADO, answer: Answer::No }` | error (sin permiso, disco lleno…) |

```rust
fn dialog_result(&mut self, r: DialogResult, cx: &mut Ctx) {
    match r {
        DialogResult::Path { tag: nf_app::TAG_GUARDADO, path: Some(_) } => self.guardado = true,
        _ => {}
    }
}
```

Para hacer un BMP de 24 bits desde una `Surface` usa `nf_core::bmp::encode`. Paint es el ejemplo completo
(`programs\paint`).

## Llamadas al núcleo (por si no usas `nf-app`)

`syscall` con el número en `rax` y los argumentos en `rdi, rsi, rdx, r10, r8`; devuelve `rax` (< 0 =
error). Se pierden `rcx` y `r11`. Contrato completo en `nf-core\src\abi.rs`.

| Nº | Nombre | Qué hace |
|---|---|---|
| 0 | `EXIT(código)` | termina el programa |
| 1 | `LOG(ptr, len)` | texto al registro de depuración |
| 2 | `WAIT_EVENT(ptr, timeout_ms)` | espera un evento (`WireEvent`, 32 bytes) |
| 3 | `PRESENT(ptr, w, h, stride)` | entrega la imagen de la ventana (u32 `0x00RRGGBB`) |
| 4 | `SET_TITLE(ptr, len)` | título de la ventana |
| 5 | `MEM_GROW(bytes)` | amplía el montón |
| 6 | `UPTIME()` | milisegundos desde el arranque |
| 7 | `DATETIME(ptr)` | fecha y hora local (8 bytes) |
| 8 | `SET_TIMER(ms)` | pide un evento de temporizador |
| 9 | `SET_MIN_SIZE(w, h)` | tamaño mínimo del contenido |
| 10 | `SAVE_AS(ptr, len, name_ptr, name_len)` | diálogo Guardar como; la respuesta llega como el evento `EV_SAVED` (`a` = 1 guardado, 0 cancelado, -1 error) |
| 11 | `SERVICE(ptr, len)` | petición de archivos codificada con `nf_core::wire` (`service::Call`); el programa espera y devuelve el tamaño de la respuesta |
| 12 | `TAKE_REPLY(ptr, cap)` | copia esa respuesta |
| 13 | `REQUEST(ptr, len)` | petición al escritorio (`request::Request`), sin esperar |
| 14 | `EVENT_DATA(ptr, cap)` | datos del último evento (diálogo, menú, arrastre, abrir); su tamaño viene en `d` |

## Límites

- El código va enlazado en una dirección fija (`programs\link.ld`); no hay reubicación al cargar.
- El programa se compila **sin SSE** (así es el destino `x86_64-unknown-none`): los `f32`/`f64` van por
  software, algo más lentos pero correctos.
- Un programa solo ve su propia memoria; si falla o se cuelga, el sistema lo termina sin caerse.

## Licencia

Los programas se enlazan con `nf-app` y `nf-core`, que son GPL-3.0. Si repartes a otros un programa hecho
con este kit, también tiene que publicarse con la GPL-3.0 y con su código fuente, conservando el fichero
`NOTICE` y el crédito "Basado en NightFury-OS de FuryNocturn" (ver `LICENSE` y `NOTICE`). Para uso
propio, sin repartirlo, no hace falta nada.
