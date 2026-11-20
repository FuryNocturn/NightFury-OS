# Arrancar NightFury

## En máquina virtual (QEMU)
```powershell
powershell -ExecutionPolicy Bypass -File run.ps1
```
Compila, prepara la carpeta `esp\` y abre QEMU. Al hacer clic en la ventana, QEMU captura el ratón;
`Ctrl+Alt+G` lo suelta.

Para una captura sin ventana (pruebas): `tools\shot.ps1 -Out shots\a.png`.

## En un PC real, desde USB
1. Compila: `cargo build -p kernel --release --target x86_64-unknown-uefi`
2. Formatea un USB en **FAT32** (borra su contenido).
3. Crea en el USB la carpeta `EFI\BOOT\` y copia ahí
   `target\x86_64-unknown-uefi\release\nightfury.efi` con el nombre **`BOOTX64.EFI`**.
4. En el PC: desactiva *Secure Boot* en la BIOS/UEFI (el sistema no está firmado), arranca desde el USB
   (menú de arranque: F12, F11, F8 o Esc según el fabricante) en modo UEFI.

No toca los discos del PC: todo vive en memoria.

### Dispositivos USB (v0.11)
Desde la v0.11 NightFury tiene su propio controlador USB (xHCI, USB 3), así que no depende de que la placa
emule PS/2:
- **Teclado y ratón USB**, también detrás de un hub. Los PS/2 siguen funcionando.
- **Pendrives y discos USB** como `D:`, `E:`…: FAT12/16/32 y exFAT en lectura y escritura, ISO 9660 y UDF en
  lectura, y pendrives NightFury (en claro o cifrados). Al conectarlos sale un aviso; se expulsan desde la
  bandeja o el menú contextual, y se formatean o cifran desde el Explorador (**formatear y cifrar borran el
  pendrive**). Lo borrado en una unidad extraíble no va a la Papelera.
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
