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
| `kernel/src/pci.rs`, `ahci.rs` | Configuración PCI y disco SATA (AHCI) por sondeo | PCI Local Bus Specification (acceso por 0xCF8/0xCFC); Serial ATA AHCI 1.3.1; ATA/ATAPI Command Set (IDENTIFY, READ/WRITE DMA EXT, FLUSH CACHE EXT) |
| `nf-core/src/input.rs` | Códigos de tecla y distribución ES | Tabla pública "scan code set 1"; distribución de teclado española |
| `nf-core/src/glyphs.rs` | Tipografía | Diseño propio, trazo a trazo |
| `nf-core/src/icons.rs`, `theme.rs` | Iconos, logo, fondos | Diseño propio, generado por código |
| `nf-core/src/crypto.rs` | SHA-256, HMAC-SHA-256, PBKDF2 | FIPS 180-4; RFC 2104 y RFC 4231 (vectores); RFC 8018. Código propio, comprobado con los vectores oficiales |
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
| Resto de `nf-core` | Gráficos, ventanas, apps, montón | Diseño propio |

## Criptografía
Los algoritmos son estándares públicos, pero **la implementación es propia y no está auditada por terceros**. AES usa
tablas (tiempos dependientes de los datos); se acepta mientras el sistema sea un solo programa. Sirve para uso personal.

## Licencia
GPL-3.0 con un término adicional de crédito (§7(b)); ver `LICENSE` y `NOTICE`.
