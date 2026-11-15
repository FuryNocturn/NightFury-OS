# Reglas clean room de NightFury

Todo el código y todo el arte de este repositorio se escriben aquí, desde cero.

## Permitido
- El compilador de Rust y sus bibliotecas base `core` y `alloc` (MIT/Apache-2.0).
- Especificaciones y documentación públicas, usadas solo como descripción de interfaces y formatos:
  UEFI Specification, Intel SDM, documentación pública de los chips 8259 (PIC), 8254 (PIT),
  8042 (PS/2) y MC146818 (reloj).
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
| `kernel/src/ps2.rs` | Teclado y ratón | Interfaz del controlador 8042 y protocolo PS/2 |
| `kernel/src/rtc.rs` | Fecha y hora | Registros del MC146818 |
| `nf-core/src/input.rs` | Códigos de tecla y distribución ES | Tabla pública "scan code set 1"; distribución de teclado española |
| `nf-core/src/glyphs.rs` | Tipografía | Diseño propio, trazo a trazo |
| `nf-core/src/icons.rs`, `theme.rs` | Iconos, logo, fondos | Diseño propio, generado por código |
| Resto de `nf-core` | Gráficos, ventanas, apps, montón | Diseño propio |

## Pendiente antes de publicar
- El nombre "NightFury" coincide con un personaje de DreamWorks: cambiarlo.
- Elegir licencia y revisar la skill `cumplimiento`.
