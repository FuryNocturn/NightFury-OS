# Reglas clean room de NightFury

Todo el código y todo el arte de este repositorio se escriben aquí, desde cero.

## Permitido
- El compilador de Rust y sus bibliotecas base `core` y `alloc` (MIT/Apache-2.0).
- Especificaciones y documentación públicas, usadas solo como descripción de interfaces y formatos:
  UEFI Specification, Intel SDM, documentación pública de los chips 8259 (PIC), 8254 (PIT),
  8042 (PS/2) y MC146818 (reloj); especificaciones de USB, xHCI, HID, almacenamiento masivo y sistemas
  de archivos listadas abajo.
- Herramientas externas para compilar y probar (cargo, QEMU). No forman parte del sistema.

## Prohibido
- Crates de terceros, ni en el sistema ni en las pruebas.
- Copiar o adaptar código de otros sistemas operativos, tutoriales o proyectos, sea cual sea su licencia.
- Mirar código de Microsoft, filtrado o descompilado.
- Usar logotipos, iconos, fondos, sonidos, tipografías o nombres de producto de terceros.
  El estilo de escritorio es parecido al de Windows 10; los recursos son propios.

## Registro de fuentes por módulo
| Módulo | Qué hace | Fuente consultada |
|---|---|---|
| `kernel/src/uefi.rs` | Tablas y protocolos de arranque | UEFI Specification (tablas de sistema, GOP, mapa de memoria) |
| `kernel/src/arch.rs` | GDT, IDT, PIC, PIT, puertos | Intel SDM vol. 3; hojas de datos 8259 y 8254 |
| `kernel/src/ps2.rs` | Teclado y ratón | Interfaz del controlador 8042 y protocolo PS/2; ratón con rueda (secuencia de frecuencias 200-100-80, identificador 3, cuarto byte con signo) según la descripción pública del protocolo |
| `kernel/src/rtc.rs` | Fecha y hora | Registros del MC146818 |
| `kernel/src/pci.rs`, `ahci.rs` | Configuración PCI y disco SATA (AHCI) por sondeo | PCI Local Bus Specification (acceso por 0xCF8/0xCFC); Serial ATA AHCI 1.3.1; ATA/ATAPI Command Set (IDENTIFY con modelo y número de serie, READ/WRITE DMA EXT, FLUSH CACHE EXT); todos los controladores y puertos |
| `nf-core/src/input.rs` | Códigos de tecla y distribución ES | Tabla pública "scan code set 1"; distribución de teclado española |
| `nf-core/src/glyphs.rs` | Tipografía | Diseño propio, trazo a trazo |
| `nf-core/src/icons.rs`, `theme.rs` | Iconos, logo, fondos | Diseño propio, generado por código |
| `nf-core/src/crypto.rs` | SHA-256, HMAC-SHA-256, PBKDF2, HKDF, X25519, ChaCha20-Poly1305 | FIPS 180-4; RFC 2104 y RFC 4231 (vectores); RFC 8018; RFC 5869 (HKDF), RFC 7748 (X25519) y RFC 8439 (ChaCha20, Poly1305 y su AEAD), con sus vectores. Código propio, comprobado con los vectores oficiales |
| `nf-core/src/cipher.rs` | AES-256, modos CTR y XTS | FIPS 197 (la caja S se calcula, no se copia); NIST SP 800-38A; IEEE 1619 / NIST SP 800-38E. Código propio, comprobado con los vectores oficiales |
| `nf-core/src/keys.rs`, `accounts.rs` | Llaves envueltas, acumulador de aleatoriedad, cuentas | Diseño propio sobre las primitivas anteriores |
| `kernel/src/main.rs` (`entropy`) | RDRAND, contador de ciclos | Intel SDM vol. 2 (RDRAND, RDTSC, CPUID.01H:ECX bit 30) |
| `kernel/src/usb/xhci.rs`, `dev.rs`, `mod.rs` | Controlador xHCI, hubs, enumeración, teclado y ratón | eXtensible Host Controller Interface 1.2 (Intel); USB 2.0 y 3.2 Specification (descriptores, peticiones estándar, hubs cap. 11); Device Class Definition for HID 1.11 (protocolo de arranque) |
| `kernel/src/usb/masivo.rs`, `kernel/src/unidades.rs` | Pendrives y discos USB | USB Mass Storage Class Bulk-Only Transport 1.0; SCSI Primary/Block Commands (INQUIRY, TEST UNIT READY, READ CAPACITY, READ/WRITE(10/16), REQUEST SENSE) |
| `nf-core/src/usbhid.rs`, `gamepad.rs` | Informes HID, parser de descriptores, mandos | HID 1.11 y HID Usage Tables 1.4; formatos de informe de los mandos (DualShock 4, DualSense, Switch Pro, Xbox 360, GIP de Xbox One) según la documentación pública de la comunidad, sin código de terceros |
| `nf-core/src/vol/part.rs` | MBR y GPT | UEFI Specification (GPT, CRC32); formato público del MBR |
| `nf-core/src/vol/fat.rs` | FAT12/16/32 con nombres largos | Microsoft FAT32 File System Specification (FAT: General Overview of On-Disk Format 1.03) |
| `nf-core/src/vol/exfat.rs` | exFAT | Microsoft exFAT File System Specification (publicada en 2019) |
| `nf-core/src/vol/iso.rs`, `udf.rs` | CD/DVD de solo lectura | ECMA-119 (ISO 9660) y Joliet; ECMA-167 y OSTA UDF 1.02–2.50 |
| `nf-core/src/vol/nfvol.rs`, `vol.rs` | NightFury en pendrives (en claro o cifrado), montaje | Diseño propio sobre NFFS y el cifrado anterior |
| `kernel/src/nvme.rs` | Discos NVMe por sondeo, una pareja de colas | NVM Express Base Specification 2.0 y NVM Command Set Specification (registros, Identify, Create I/O Queue, Read, Write, Flush, listas PRP) |
| `nf-core/src/vol/ntfs.rs` | NTFS en solo lectura (MFT, índices, listas de atributos, LZNT1) | Especificación de sala limpia «ntfs» (ver abajo) |
| `nf-core/src/vol/ext.rs` | ext2, ext3 y ext4 en solo lectura | Especificación de sala limpia «ext4» (ver abajo) |
| `nf-core/src/vol/part.rs` (BitLocker) | Reconocer una unidad cifrada con BitLocker | Solo la firma `-FVE-FS-` del sector de arranque, según la documentación pública de libbde |
| `kernel/src/pci.rs` (ECAM) | Configuración PCI Express por memoria | PCI Express Base Specification (espacio de configuración ampliado); ACPI Specification (tabla MCFG) |
| `kernel/src/red/intel.rs` | Intel 82540EM, 82574L, 82576, I210/I211 e I225/I226 por sondeo | Hojas de datos públicas de Intel (82540EM, 82574L, 82576, I210, I225); I225/I226 también con la especificación de sala limpia «intel-ethernet» (ver abajo) |
| `kernel/src/red/intel/pch.rs` | Intel I217, I218 e I219 (salida de ULP, PHY paginado, ajustes de enlace) | Especificación de sala limpia «intel-ethernet» (ver abajo) |
| `kernel/src/red/realtek.rs` | Realtek RTL8111/8168 (revisiones 8168g/gu/h/m y 8411b), sin firmware | Especificación de sala limpia «realtek» (ver abajo) |
| `kernel/src/red/virtio.rs` | Tarjeta de red de las máquinas virtuales | OASIS Virtual I/O Device (VIRTIO) 1.2 (PCI moderno, colas divididas, dispositivo de red) |
| `nf-core/src/net/` | Ethernet, ARP, IPv4, ICMP, UDP, TCP, DHCP, DNS y cliente HTTP/1.1 | RFC 826 (ARP), 791 (IPv4), 792 (ICMP), 768 (UDP), 9293 (TCP), 6298 (temporizador de retransmisión), 2131 y 2132 (DHCP), 1035 (DNS), 9110 y 9112 (HTTP); IEEE 802.3 (trama Ethernet). Diseño propio. v0.12.1: sockets UDP y TCP a la escucha para programas con filtro de red local, también diseño propio |
| `nf-core/src/crypto.rs`, `cipher.rs` (v0.12.2) | SHA-384/512, HMAC-SHA-384, HKDF-SHA-384, generador ChaCha20, AES-128 y AES-GCM | FIPS 180-4; RFC 2104, 4231 y 5869; FIPS 197; NIST SP 800-38D (GCM), con sus vectores. Código propio |
| `nf-core/src/pk.rs`, `pk/` | Enteros grandes, RSA (PKCS#1 v1.5 y PSS, solo verificar), ECDSA P-256/P-384 y ECDH P-256 | RFC 8017; FIPS 186-4; SEC 1 y SEC 2 (curvas); vectores de NIST CAVP y RFC 6979. Código propio; P-256 con privada en tiempo constante |
| `nf-core/src/x509/` | DER, PEM, certificados X.509 y validación de cadena | ITU-T X.690 (DER); RFC 7468 (PEM); RFC 5280 (certificados, cadena, restricciones de nombre); RFC 6125 (nombre del sitio y comodines). Certificados de prueba hechos con OpenSSL, usado solo como herramienta |
| `nf-core/src/tls/`, `net/tlsconn.rs` | Cliente TLS 1.3 y TLS 1.2 con ECDHE | RFC 8446 (TLS 1.3) con las trazas de RFC 8448; RFC 5246 (TLS 1.2); RFC 8422, 5289 y 7905 (ECDHE y cifrados); RFC 7627 (extended master secret); RFC 6066 (SNI); RFC 7301 (ALPN). Probado contra OpenSSL ejecutándolo, sin leer su código |
| `nf-core/src/raices.rs`, `tools/raices.ps1` | Raíces de confianza (Mozilla + propias) | Formato público de `certdata.txt` de NSS; los datos de las raíces son de Mozilla (ver «Datos de terceros») |
| `nf-core/src/netperm.rs` | Permisos de red de los programas (avisos «usar la red» y «aceptar conexiones») | Diseño propio; la idea de preguntar por programa es la de cualquier cortafuegos, sin código ni textos de otros |
| `programs/mensajes` | Chat cifrado de extremo a extremo en la red local | Protocolo propio (`docs/Mensajes-protocolo.md`) sobre RFC 7748, 8439 y 5869. No se ha mirado el código ni la especificación de otros programas de mensajería; la lista de palabras de verificación es propia |
| Resto de `nf-core` | Gráficos, ventanas, apps, montón | Diseño propio |

## Sala limpia en dos equipos (desde v0.11.1)
Para los formatos que no tienen una especificación pública completa se separa quién lee y quién programa:

1. **Analista.** Lee el código abierto y la documentación pública del formato y escribe una especificación
   en prosa: estructuras, campos, reglas de validación, casos raros y pruebas. La especificación no lleva
   código, ni nombres de funciones o variables del original. Aparte anota sus fuentes (repositorio, commit,
   licencia y qué sección sale de cada una), para poder auditarla.
2. **Programador.** Escribe el código a partir de la especificación y nada más. Nunca ve el código original
   ni la lista de fuentes.
3. **Revisión.** Se comprueba que la especificación no contiene código y que el resultado no se parece al
   original más de lo que obliga el formato.

Especificaciones escritas así:

| Especificación | Fuentes que leyó el analista |
|---|---|
| ntfs | ntfs-3g (commit 7f0f841, GPL-2.0/LGPL-2.1); driver ntfs3 de Linux (`fs/ntfs3`, commit 65e4fd9, GPL-2.0); documentación Linux-NTFS (Russon y Fledel); Microsoft [MS-FSCC] (etiquetas de reparse y atributos) y [MS-XCA] (LZNT1); documentación pública de WOF; documento de formato de libbde (solo la firma de BitLocker) |
| ext4 | Documentación de ext4 del kernel Linux (`Documentation/filesystems/ext4`); código de Linux `fs/ext4`, `fs/jbd2` y `fs/crypto` (commit 65e4fd9, GPL-2.0); e2fsprogs (commit 8fd7952, GPL-2.0/LGPL-2.0) |
| intel-ethernet | Linux `drivers/net/ethernet/intel/e1000e` e `igc` (commit d0f871b, GPL-2.0); FreeBSD `sys/dev/e1000` e `igc` (commit 83c94a8, BSD-3-Clause/BSD-2-Clause) |
| realtek | Linux `drivers/net/ethernet/realtek/r8169*` y `drivers/net/phy/realtek` (commit 4dd10fb, GPL-2.0); FreeBSD `sys/dev/re`, `sys/dev/rl` y `sys/dev/mii/rgephy.c` (commits b0c14ea y 8cbe0dd, BSD). Sin hojas de datos de Realtek |

Sigue prohibido mirar código de Microsoft, filtrado o descompilado. Las pruebas con discos reales usan
`mkfs.ext4` y `mkntfs` solo como herramientas: se ejecutan, no se lee su código.

## Datos de terceros
`nf-core/data/raices/raices.bin` contiene los certificados raíz de la lista de Mozilla (NSS `certdata.txt`), que se
distribuyen bajo la **Mozilla Public License 2.0**. Solo son datos (certificados públicos), no código; van en un
fichero aparte con su licencia y su origen en `nf-core/data/raices/LEEME.md`, y se renuevan con cada versión.

## Firmware de terceros
La versión 0.12.2 no carga ningún firmware. Si una tarjeta lo necesita más adelante (algunas Realtek 8125/8126, el Wi-Fi), irá en una carpeta aparte con su licencia y se anotará aquí.

## Criptografía
Los algoritmos son estándares públicos, pero **la implementación es propia y no está auditada por terceros**. AES usa
tablas (tiempos dependientes de los datos); se acepta mientras el sistema sea un solo programa, y en TLS se prefiere
ChaCha20-Poly1305. HTTPS no comprueba la revocación de certificados (OCSP/CRL). Sirve para uso personal.

## Licencia
GPL-3.0 con un término adicional de crédito (§7(b)); ver `LICENSE` y `NOTICE`.
