# Kit de programas de NightFury (v0.8)

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
   ```

   `nombre` es también el nombre de la carpeta de salida. `icono` es el icono del sistema que se
   usará (`app` si no sabes cuál).

## Compilar

```powershell
.\tools\programas.ps1
```

Compila todos los de `programs\` y deja, para cada uno, `target\programas\<nombre>\programa.nfx` y su
`app.txt`. Si algo falla, sale con código distinto de 0. Lo hace `nfpack`, que también comprueba que
el `.nfx` es válido y muestra sus secciones.

> **Instalar en el disco** (copiar la carpeta a NightFury con `nfdisk`) llega en la **v0.9**; en la
> v0.8 el núcleo ya sabe cargar `.nfx`, pero todavía no hay forma cómoda de meterlos en el disco.

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

**No existe todavía para programas** (se ignora con una línea en el registro): diálogos
(`cx.dialog`), menús (`cx.menu`), abrir rutas (`cx.open*`) y arrastrar archivos (`cx.drag`).
`cx.sys.files()` es una unidad C: en memoria, **no** el disco real (hasta la v0.10).

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
