# Historial de cambios

Todas las versiones de NightFury OS, de la más nueva a la más antigua. Todas probadas en QEMU; en hardware
real todavía no.

## 0.12.7 · 2026-11-25 · Junto a Windows y Linux
- Instalar en el **espacio libre** de un disco que ya tiene Windows o Linux, o en una **partición de datos** que se
  borra (escribiendo BORRAR). Nunca se encogen particiones ni se toca la partición donde está otro sistema.
- Usa la partición EFI que ya tiene el disco: solo añade la carpeta `\EFI\NightFury`.
- **Menú de arranque** propio al encender: NightFury, Windows, Linux y las demás entradas del equipo,
  Configuración del firmware, Reiniciar y Apagar. Cuenta atrás de 5 s y arranca el último sistema elegido.
- **Configuración › Arranque**: segundos de la cuenta atrás, ocultar o renombrar entradas y volver a poner el
  menú el primero si otro sistema se ha puesto delante.
- **Quitar NightFury** desde el pendrive, sin tocar Windows ni Linux.
- No se instala en discos con BitLocker ni en discos MBR.
- `nfdisk disco-prueba` y `nfdisk analizar` para probar el instalador con discos de prueba.

## 0.12.6 · 2026-11-25 · Pendrive con instalador
- Imagen de pendrive con un menú al arrancar: **Probar** (en memoria, sin tocar ningún disco), **Portátil**
  (guarda cuentas y archivos en el propio pendrive) e **Instalar**.
- Instalar en un disco interno SATA o NVMe: hay que escribir BORRAR; NightFury queda como único sistema de ese
  disco, con su entrada en el menú de arranque del equipo y la elección de cifrado en el primer arranque.
- El disco del sistema puede ser NVMe.
- `nfdisk pen-arranque` crea la imagen del pendrive.
- Cada versión se publica en GitHub con la imagen del pendrive, un zip para QEMU y sus SHA-256.
- El repositorio se comprueba solo: compilación y tests en cada PR, búsqueda de secretos, plantillas de issues,
  política de seguridad y guía para aportar.

## 0.12.5 · 2026-11-25 · Navegador, fase 2, y letra TrueType en todo el sistema
- Formularios: `input`, `select`, `textarea`, `button` y `label`; envío GET y POST, con aviso si una contraseña va a viajar por http.
- Cookies: en memoria, en disco solo para favoritos o sitios con «Recordar sesión»; las de terceros, bloqueadas; panel del candado para borrarlas.
- Historial de 90 días con buscador (Ctrl+H) y sugerencias en la barra de direcciones.
- Favoritos: estrella (Ctrl+D), barra (Ctrl+Mayús+B), accesos en la pestaña nueva y gestión con carpetas (Ctrl+Mayús+O).
- Ventana de incógnito (Ctrl+Mayús+N), oscura, sin historial y con cookies solo en memoria.
- Descargas que preguntan antes y van a Descargas, con barra de progreso y lista (Ctrl+J).
- Letra TrueType suavizada en todo el sistema y en los programas del KIT; la de trazos y las fuentes de Windows, en Configuración → Personalización.

## 0.12.4 · 2026-11-24 · El Navegador
- Webs por HTTPS y HTTP (con la marca «No seguro») en una ventana con pestañas, barra de direcciones, atrás, adelante y recargar.
- Texto con letra TrueType (Liberation incluida o las de Windows si hay un Windows en otro disco), CSS básico e imágenes PNG, JPEG y GIF.
- Lo que se escribe en la barra y no es una dirección se busca en DuckDuckGo.

## 0.12.3 · 2026-11-24 · La hora real
- Configuración → Fecha y hora: zona horaria con horario de verano automático y hora por Internet (desactivable).
- HTTPS usa la hora UTC real, comprueba la revocación cuando el servidor adjunta la respuesta OCSP y acepta P-384.
- El selector de archivos cambia de unidad, Ctrl+A selecciona en los campos de texto y desbloquear una unidad cifrada muestra el progreso.

## 0.12.2 · 2026-11-23 · HTTPS
- `curl https://…` comprueba el certificado con la lista de raíces de Mozilla (TLS 1.3 y 1.2) y explica por qué lo rechaza.
- `-k` conecta igual avisando, `-v` muestra la versión, el cifrado y el certificado, y `-L` sigue redirecciones.
- Raíces propias en Configuración → Red → Certificados y «conexión segura» para los programas del KIT.

## 0.12.1 · 2026-11-23 · Red para los programas y Mensajes
- Los programas de terceros usan la red (TCP y UDP) tras un aviso de permiso por programa, como un cortafuegos.
- Configuración y el Administrador de tareas muestran los permisos y las conexiones.
- **Mensajes**: chat cifrado de extremo a extremo entre equipos de la misma red local, sin servidor.

## 0.12.0 · 2026-11-21 · Red por cable
- Tarjetas Intel 82540EM, 82574L, 82576, I210/I211, I217-I219 e I225/I226, Realtek RTL8111/8168 y virtio-net.
- DHCP y DNS; `ping`, `ipconfig`, `nslookup` y `curl` en la Terminal.

## 0.11.1 · 2026-11-20 · Discos de otros sistemas
- Lectura de discos de Windows (NTFS) y de Linux (ext2/3/4); los cifrados con BitLocker se reconocen y quedan bloqueados.
- Discos internos SATA y NVMe en solo lectura.

## 0.11.0 · 2026-11-20 · USB propio
- Controlador xHCI: teclado, ratón, pendrives (FAT, exFAT, ISO y UDF) y mandos.
- Configuración → Dispositivos, con prueba de mandos y lista de unidades.
- Arranque desde un pendrive ([docs/USB.md](docs/USB.md)).

## 0.10.0 · 2026-11-18 · Programas del sistema como procesos
- Bloc de notas, Terminal, Configuración y Explorador pasan a ser programas independientes.
- Los programas piden archivos, diálogos y menús al sistema; abrir archivos según su extensión.

## 0.9.0 · 2026-11-18 · Multitarea e instalación de programas
- Multitarea con turnos, aviso de fallo y «No responde».
- Instalar y desinstalar programas desde Configuración → Aplicaciones; Reloj y Paint como programas del sistema.

## 0.8.0 · 2026-11-17 · Programas en modo usuario
- Tablas de páginas propias, GDT con TSS, llamadas al sistema y procesos en ring 3.
- La Calculadora es el primer programa en modo usuario; nace el [KIT de programas](docs/KIT-programas.md).

## 0.7.0 – 0.7.2 · 2026-11-17 · Arrastrar y soltar
- Recuadro de selección, selección múltiple y arrastrar entre ventanas, también con el botón derecho (Mover aquí / Copiar aquí).
- Elegir el cifrado al estrenar el disco: ninguno, la carpeta del usuario, el disco entero o ambos.

## 0.6.0 · 2026-11-17 · Cifrado
- Cifrado del disco entero y de la carpeta de cada usuario con AES-256-XTS, con código de recuperación.

## 0.5.0 · 2026-11-16 · Cuentas
- Cuentas de usuario con contraseña (PBKDF2), inicio de sesión y carpetas por usuario.

## 0.3.0 – 0.4.0 · 2026-11-16 · Explorador completo
- Menús contextuales, vistas, orden, deshacer, propiedades y rueda del ratón.
- Cinta del Explorador, Mover a / Copiar a, extensiones y barras de desplazamiento.

## 0.2.0 · 2026-11-16 · Archivos
- Sistema de archivos NFFS propio (copia al escribir), Explorador, Bloc de notas con archivos, Terminal y la herramienta `nfdisk`.

## 0.1.0 · 2026-11-16 · Escritorio
- Arranque UEFI, escritorio y gestor de ventanas, tipografía e iconos propios, fondos procedurales, teclado y ratón.
