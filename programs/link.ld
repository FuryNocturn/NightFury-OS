/* Enlazado de los programas NightFury. Cada sección empieza en su propia página para que el
   .nfx pueda darle sus permisos (código, solo lectura, datos, bss). Dirección de carga fija:
   USER_BASE (0x1000_0000_0000) + 0x40_0000, dentro de USER_BASE..IMAGE_TOP. */
ENTRY(_start)
SECTIONS {
  . = 0x100000400000;
  .text : ALIGN(4096) { *(.text .text.*) }
  .rodata : ALIGN(4096) { *(.rodata .rodata.*) *(.eh_frame*) }
  .data : ALIGN(4096) { *(.data .data.*) *(.got .got.*) }
  .bss : ALIGN(4096) { *(.bss .bss.*) *(COMMON) }
  /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame_hdr) }
}
