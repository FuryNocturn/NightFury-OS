# Arrancar NightFury

## En máquina virtual (QEMU)
```powershell
powershell -ExecutionPolicy Bypass -File run.ps1
```
Compila, prepara la carpeta `esp\` y abre QEMU. Al hacer clic en la ventana, QEMU captura el ratón;
`Ctrl+Alt+G` lo suelta.

Para una captura sin ventana (pruebas): `tools\shot.ps1 -Out shots\a.png`.

## En un PC real, desde un pendrive (v0.12.7)
1. Descarga `nightfury-<versión>-usb.img` de las
   [releases](https://github.com/FuryNocturn/NightFury-OS/releases) (comprueba su SHA-256 con
   `SHA256SUMS.txt`) o créala tú:
   ```powershell
   cargo build -p arranque --release --target x86_64-unknown-uefi
   target\release\nfdisk.exe pen-arranque pen.img target\x86_64-unknown-uefi\release\nightfury.efi 0.12.7 target\x86_64-unknown-uefi\release\arranque.efi
   ```
2. Grábala en un pendrive con Rufus o balenaEtcher en modo imagen (DD). **Borra el pendrive.** Para el modo
   Portátil usa uno de al menos 1 GB.
3. En el PC: desactiva *Secure Boot* (el sistema no está firmado) y arranca desde el pendrive en modo UEFI
   (menú de arranque: F12, F11, F8 o Esc según el fabricante).

Sale un menú que espera a que elijas:
- **Probar**: todo vive en memoria y se pierde al apagar. No escribe en los discos del equipo ni en el pendrive.
- **Portátil**: la primera vez crea una partición en el espacio libre del pendrive (sin borrar lo que hay) y
  guarda ahí cuentas y archivos, con cifrado opcional. Llévate NightFury a cualquier PC.
- **Instalar**: elige un disco interno (SATA o NVMe) y dónde:
  - **Espacio libre** (al menos 2 GB): no se borra nada de lo que hay.
  - **Una partición de datos**: se borra entera (hay que escribir `BORRAR`). La partición donde está instalado
    Windows o Linux no se puede elegir.
  - **Disco entero**: **se borra todo** (`BORRAR`) y NightFury queda como único sistema.

  NightFury usa la partición EFI que ya tenga el disco (solo añade `\EFI\NightFury`) y pone su entrada
  «NightFury» la primera en el menú de arranque del firmware. Quita el pendrive y reinicia.
  No se instala en discos con BitLocker ni en discos MBR. El espacio libre lo tienes que dejar tú antes (por
  ejemplo, encogiendo una partición desde Windows): NightFury nunca encoge particiones.
- **Quitar NightFury** (en Instalar, sobre un disco donde ya está): borra su partición, su carpeta en la
  partición EFI y su entrada de arranque, y deja el espacio libre. Windows y Linux no se tocan.

### Menú de arranque (v0.12.7)
Al encender sale el menú de NightFury: los sistemas que encuentra (NightFury, Windows, Linux), las demás
entradas del equipo (red, USB…), Configuración del firmware, Reiniciar y Apagar. Flechas y Enter; si no tocas
nada, a los 5 s arranca el último sistema elegido.
- En **Configuración › Arranque** se cambian los segundos (0 = arranca directo; mantén una tecla para ver el
  menú) y se ocultan o renombran entradas.
- Si una actualización de Windows se pone delante y el menú deja de salir, el botón «Poner el menú de
  NightFury el primero» lo arregla. El menú de arranque de la placa (F12…) siempre puede arrancar Windows
  directamente.
- Algunos Linux con *shim* avisan de que Secure Boot está desactivado: es normal.

### Probar el instalador en QEMU
```powershell
nfdisk disco-prueba win.img windows 2400 target\x86_64-unknown-uefi\release\prueba-so.efi   # también windows-datos, linux, bitlocker, mbr
nfdisk analizar win.img                     # dónde se podría instalar y por qué no en el resto
```
El «Windows» de prueba es un EFI mínimo que solo escribe un mensaje.

### Pendrive en QEMU
```powershell
& "C:\Program Files\qemu\qemu-system-x86_64.exe" -machine q35 -m 1024 `
  -drive "if=pflash,format=raw,readonly=on,file=C:\Program Files\qemu\share\edk2-x86_64-code.fd" `
  -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0 -device usb-mouse,bus=xhci.0 `
  -drive if=none,id=pen,format=raw,file=pen.img -device usb-storage,bus=xhci.0,drive=pen,bootindex=0 -vga std
```
Para probar Instalar, añade un disco con `bootindex=1` (sin él, el firmware de QEMU borra la entrada «NightFury» al
arrancar desde el pendrive) y una copia de las variables UEFI (`edk2-i386-vars.fd`) como segundo `pflash`, para que
la entrada de arranque se conserve: `-drive file=nvme.img,if=none,id=nv,format=raw -device nvme,serial=nf1,drive=nv,bootindex=1`.

### Máquina virtual descargada
El `nightfury-<versión>-qemu.zip` de cada release trae `BOOTX64.EFI` y `arrancar.ps1`: descomprímelo y ejecuta
`powershell -ExecutionPolicy Bypass -File arrancar.ps1` (necesita QEMU para Windows).

### Dispositivos USB (v0.11)
Desde la v0.11 NightFury tiene su propio controlador USB (xHCI, USB 3), así que no depende de que la placa
emule PS/2:
- **Teclado y ratón USB**, también detrás de un hub. Los PS/2 siguen funcionando.
- **Pendrives y discos USB** como `D:`, `E:`…: FAT12/16/32 y exFAT en lectura y escritura, ISO 9660 y UDF en
  lectura, y pendrives NightFury (en claro o cifrados). Al conectarlos sale un aviso; se expulsan desde la
  bandeja o el menú contextual, y se formatean o cifran desde el Explorador (**formatear y cifrar borran el
  pendrive**). Lo borrado en una unidad extraíble no va a la Papelera.
- **NTFS (Windows) y ext2/3/4 (Linux)** en lectura, tanto en USB como en los discos internos (v0.11.1).
  Una unidad con BitLocker aparece bloqueada.

### Discos internos (v0.11.1)
Los demás discos SATA y los NVMe del PC se ven como unidades **de solo lectura**, sea cual sea su formato.
No salen las particiones que no son de datos (sistema EFI, MSR, recuperación, swap de Linux). Se pueden
formatear o cifrar desde el Explorador, con doble confirmación, y entonces sí pasan a ser escribibles:
**se borra todo lo que tenga la partición, también si es de Windows o de Linux.** Solo se ven los discos que
hay al arrancar.
- **Mandos**: PlayStation (DualShock 4, DualSense), Nintendo Switch Pro, Xbox 360/One y mandos HID genéricos.
  Se prueban en Configuración › Dispositivos › Probar mando.

### Límites conocidos en PC real
- No probado todavía en hardware real; el soporte USB solo se ha probado en QEMU.
- Controladores USB antiguos (UHCI/OHCI/EHCI sin xHCI) no están soportados.
- Para apagar si no responde: mantener pulsado el botón de encendido.

## Pendrives de prueba en QEMU
```powershell
nfdisk usb pen.img fat32 64                  # también exfat o nightfury
nfdisk usb pen.img nightfury-cifrado 64 clave   # imprime el código de recuperación
powershell -ExecutionPolicy Bypass -File run.ps1 -Usb pen.img   # varias: -Usb a.img,b.img
nfdisk usb-ver pen.img                       # lo que quedó dentro tras usarla (NFDISK_CLAVE si va cifrada)
```
`nfdisk usb` solo crea archivos nuevos; nunca escribe en dispositivos ni unidades reales.

## Intercambiar archivos con el disco virtual (nfdisk)
`nfdisk` es una herramienta de consola para Windows que lee y escribe la imagen NFFS (`disco.img`)
sin arrancar NightFury. **Úsala con QEMU cerrado**: si QEMU tiene la imagen abierta, ambos podrían
escribir a la vez y dañarla.

Compilar: `cargo build -p nfdisk --release` → `target\release\nfdisk.exe`.

```powershell
nfdisk crear disco.img 64                       # imagen nueva de 64 MiB, formateada y con la estructura base
nfdisk info disco.img                           # tamaño, libre y usado
nfdisk listar disco.img C:\Usuarios -r          # carpetas de usuario (incluidas las ocultas .config y .papelera)
nfdisk meter disco.img foto.bmp C:\Usuarios\Ana\Imágenes\foto.bmp
nfdisk carpeta disco.img C:\Usuarios\Ana\Documentos\Trabajo
nfdisk ver disco.img C:\Usuarios\Ana\Documentos\nota.txt
nfdisk sacar disco.img C:\Usuarios\Ana\Documentos\nota.txt nota.txt
nfdisk borrar disco.img C:\Usuarios\Ana\Documentos\nota.txt
nfdisk comprobar disco.img                      # revisa la coherencia del volumen
```
- Estructura: `Usuarios\<nombre>\{Escritorio, Documentos, Descargas, Imágenes, Música, Vídeos, .config, .papelera}` y
  `Sistema\{config, temp, lib, drivers, logs}`. Una imagen nueva no tiene cuentas: NightFury las crea en el primer
  arranque (asistente), y entonces crea la carpeta de cada una.
- `nfdisk` trabaja directamente sobre el volumen NFFS, sin sesión: es como un administrador sin restricciones de usuario
  (puede entrar en la carpeta de cualquier cuenta y en `Sistema`). No crea cuentas ni migra discos con la estructura
  antigua (eso lo hace NightFury al arrancar con ese disco). Sí se niega a borrar `Usuarios`, `Sistema`, sus subcarpetas,
  la carpeta de cada cuenta ni sus carpetas estándar, `.config` y `.papelera`.
- Las rutas de la imagen se aceptan como `C:\Usuarios\Ana\a.txt`, `\Usuarios\Ana\a.txt` o `Usuarios/Ana/a.txt`.
- `meter` y `sacar` no sustituyen un archivo existente salvo con `--reemplazar`.
- `formatear disco.img` solo funciona con una imagen en blanco; con datos exige `--forzar` (borra todo).
- Solo trabaja con archivos normales: rechaza `\\.\PhysicalDrive0`, `\\?\...`, `D:\` y similares.
- Una imagen creada con `nfdisk crear` arranca en NightFury sin preguntar por formato.
- Las imágenes nuevas (`crear`, `formatear`) dejan **sin usar los dos últimos bloques** del archivo: son el sitio de la cabecera
  del cifrado total. Las imágenes antiguas siguen funcionando con todos sus bloques.

### Discos con cifrado total
Si el disco se cifró desde NightFury (Configuración › Disco), `nfdisk` lo detecta por su cabecera:
- **Sin clave**, `info` y `comprobar` solo dicen «Disco cifrado de NightFury»; no muestran tamaños, nombres ni nada del contenido.
  Las demás órdenes piden la clave y se niegan sin ella.
- **La clave del disco nunca va por la línea de órdenes** (quedaría en el historial). Se da con la variable de entorno
  `NFDISK_CLAVE` (para scripts y pruebas) o, si no existe, `nfdisk` la pregunta por teclado (sin mostrarla en Windows; en otros
  sistemas avisa de que se verá).
  ```powershell
  $env:NFDISK_CLAVE = "mi clave del disco"
  nfdisk listar disco.img C:\Usuarios -r
  Remove-Item Env:NFDISK_CLAVE
  ```
- `--recuperacion`: la clave que se da es el **código de recuperación** del disco (8 grupos de 5 caracteres) en vez de la clave.
- `--clave`: hace que `info` y `comprobar` pregunten la clave por teclado (por defecto solo usan `NFDISK_CLAVE`, para no
  interrumpir scripts).
- Con la clave, todas las órdenes funcionan como en un disco normal, **también sobre un disco a medio convertir** (cifrando o
  descifrando): lo que se escribe va cifrado o en claro según el punto en que esté la conversión. Si la conversión se cortó con
  un bloque a medias, `nfdisk` lo rehace al abrir (por eso entonces abre la imagen también para escribir).
- `formatear --forzar` en un disco cifrado exige además la clave y deja un NFFS normal sin cifrar.
- `nfdisk` **no activa ni quita** el cifrado: eso lo hace NightFury, con barra de progreso y reanudable.
- Cierra QEMU antes de usarlo, igual que siempre, y haz copia de seguridad de la imagen antes de convertirla desde NightFury.

### Carpetas de usuario cifradas
Si una cuenta cifró su carpeta desde NightFury (Configuración › Cuentas), `nfdisk` la reconoce por su archivo de llaves
(`Sistema\config\llaves\<cuenta>`):
- **Sin clave de usuario**, `listar` muestra la carpeta tal como está en el disco: los nombres de archivos y carpetas dentro de
  `Usuarios\<cuenta>` salen cifrados (empiezan por `$e`) y se indica «(carpeta cifrada)». Se ve cuántos elementos hay, su tamaño
  aproximado y sus fechas, pero no nombres ni contenido. `ver`, `sacar`, `meter`, `carpeta` y `borrar` se niegan a tocar dentro
  de ella (lo que se escribiera iría en claro).
- **Con `--usuario <nombre>`** y la clave de esa cuenta en la variable de entorno `NFDISK_CLAVE_USUARIO` (o preguntada por teclado;
  nunca por argumento), `listar`, `ver`, `sacar`, `meter`, `carpeta` y `borrar` trabajan en claro dentro de su carpeta, con las
  rutas de siempre y con los permisos de esa cuenta. Una cuenta sin clave entra sin pedirla.
  ```powershell
  $env:NFDISK_CLAVE_USUARIO = "clave de Ana"
  nfdisk ver disco.img C:\Usuarios\Ana\Documentos\nota.txt --usuario Ana
  Remove-Item Env:NFDISK_CLAVE_USUARIO
  ```
- Se puede combinar con el cifrado total (`NFDISK_CLAVE` para el disco y `NFDISK_CLAVE_USUARIO` para la cuenta).
