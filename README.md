# NightFury-OS

**ES** · Sistema operativo x86_64 con arranque UEFI, escrito desde cero en Rust (*clean room*): núcleo, escritorio, ventanas, tipografía, iconos, sistema de archivos, cuentas, cifrado de disco, USB y red propios. Sin crates de terceros.

**EN** · An x86_64 UEFI operating system written from scratch in Rust (*clean room*): its own kernel, desktop, windowing, font, icons, file system, user accounts, disk encryption, USB and network stacks. No third-party crates.

![Escritorio / Desktop](docs/img/escritorio.png)

| Explorador / File explorer | Terminal |
|---|---|
| ![Explorador](docs/img/explorador.png) | ![Terminal](docs/img/terminal.png) |

## Estado / Status
Versión 0.12.0, en desarrollo: red por cable (Intel 82540EM, 82574L, 82576, I210/I211, I217-I219 e I225/I226; Realtek RTL8111/8168; virtio-net), con DHCP, DNS y, en la Terminal, `ping`, `ipconfig`, `nslookup` y `curl` (HTTP; HTTPS en una versión próxima). Ya estaban el USB propio y la lectura de discos NTFS y ext2/3/4. Probado en QEMU; en hardware real todavía no.
Version 0.12.0, work in progress: wired networking (Intel 82540EM, 82574L, 82576, I210/I211, I217-I219 and I225/I226; Realtek RTL8111/8168; virtio-net) with DHCP, DNS and, in the Terminal, `ping`, `ipconfig`, `nslookup` and `curl` (HTTP; HTTPS coming soon). Native USB and read-only NTFS and ext2/3/4 disks were already there. Tested in QEMU; not yet on real hardware.

> **Aviso / Warning:** la criptografía (SHA-256, PBKDF2, AES-256-XTS) es una implementación propia **no auditada**. No protejas con ella nada importante.
> The cryptography is a custom, **unaudited** implementation. Do not rely on it for anything important.

## Compilar y arrancar / Build and run
Requisitos / Requirements: Rust (con el target `x86_64-unknown-uefi`) y QEMU con firmware edk2.

```powershell
rustup target add x86_64-unknown-uefi
powershell -ExecutionPolicy Bypass -File run.ps1
```

Pruebas / Tests: `cargo test -p nf-core`.
Arranque desde USB y herramienta `nfdisk` / USB boot and `nfdisk`: [docs/USB.md](docs/USB.md).
Hacer programas para el sistema / Writing programs: [docs/KIT-programas.md](docs/KIT-programas.md).
Reglas *clean room* y fuentes consultadas / Clean-room rules and sources: [docs/CLEANROOM.md](docs/CLEANROOM.md).

## Licencia / License
Copyright (C) 2026 Tare Fernandez Salas (**FuryNocturn**).

**GPL-3.0** ([LICENSE](LICENSE)) con un término adicional del §7(b) ([NOTICE](NOTICE)): toda copia u obra derivada debe conservar el fichero NOTICE y mostrar en su pantalla «Acerca de» el crédito:

GPL-3.0 with an additional term under section 7(b): every copy or derivative work must keep the NOTICE file and show this attribution in its "About" screen:

> **Basado en NightFury-OS de FuryNocturn** — https://github.com/FuryNocturn/NightFury-OS
