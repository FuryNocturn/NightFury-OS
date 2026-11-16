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

### Límites conocidos en PC real (v0.1)
- Teclado y ratón van por PS/2. Funcionan los integrados de muchos portátiles; un teclado o ratón USB
  solo funciona si la placa emula PS/2 para USB. Si no, el escritorio se ve pero no responde.
- Para apagar si no responde: mantener pulsado el botón de encendido.
- No probado todavía en hardware real.
