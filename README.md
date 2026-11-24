# NightFury-OS

**ES** · Sistema operativo x86_64 con arranque UEFI, escrito desde cero en Rust (*clean room*): núcleo, escritorio, ventanas, tipografía, iconos, sistema de archivos, cuentas, cifrado de disco, USB y red propios. Sin crates de terceros.

**EN** · An x86_64 UEFI operating system written from scratch in Rust (*clean room*): its own kernel, desktop, windowing, font, icons, file system, user accounts, disk encryption, USB and network stacks. No third-party crates.

![Escritorio / Desktop](docs/img/escritorio.png)

| Explorador / File explorer | Terminal |
|---|---|
| ![Explorador](docs/img/explorador.png) | ![Terminal](docs/img/terminal.png) |

## Estado / Status
Versión 0.12.4, en desarrollo: **el Navegador**. Abre webs por HTTPS y HTTP (con la marca «No seguro») en una ventana con pestañas, barra de direcciones, atrás/adelante y recargar; muestra el texto con letra TrueType (las Liberation incluidas o las de Windows si hay un Windows en otro disco), CSS básico e imágenes PNG, JPEG y GIF; lo que se escribe en la barra que no es una dirección se busca en DuckDuckGo. Aún sin JavaScript, formularios ni cookies, así que muchas webs modernas se ven incompletas. La 0.12.3 trajo **la hora real**: Configuración → Fecha y hora elige la zona (horario de verano automático) y sincroniza la hora por Internet (desactivable); HTTPS usa la hora UTC real, comprueba la revocación cuando el servidor adjunta la respuesta OCSP y acepta P-384; el selector de archivos cambia de unidad, Ctrl+A selecciona en los campos de texto y desbloquear una unidad cifrada muestra «Desbloqueando…». La 0.12.2 trajo **HTTPS**: `curl https://…` en la Terminal comprueba el certificado del sitio con la lista de raíces de Mozilla (TLS 1.3 y 1.2); si no es válido se niega y explica por qué, `-k` conecta igual avisando, `-v` muestra la versión, el cifrado y el certificado, y `-L` sigue redirecciones. Un administrador puede añadir raíces propias en Configuración → Red → Certificados, y los programas tienen una «conexión segura» en el KIT que nunca se salta la comprobación. La 0.12.1 abrió la red a los programas de terceros con un aviso de permiso por programa, como un cortafuegos, y trajo **Mensajes**, un chat cifrado de extremo a extremo en la red local. La 0.12.0 trajo la red por cable (Intel 82540EM, 82574L, 82576, I210/I211, I217-I219 e I225/I226; Realtek RTL8111/8168; virtio-net), con DHCP, DNS, `ping`, `ipconfig`, `nslookup` y `curl`. Probado en QEMU; en hardware real todavía no.
Version 0.12.4, work in progress: **the web browser** (Navegador). It opens websites over HTTPS and HTTP (marked "Not secure") in a tabbed window with an address bar, back/forward and reload; text uses TrueType fonts (the bundled Liberation fonts, or Windows fonts if a Windows install is on another disk), basic CSS and PNG, JPEG and GIF images; anything typed in the bar that is not an address is searched on DuckDuckGo. No JavaScript, forms or cookies yet, so many modern sites look incomplete. 0.12.3 brought **real time**: Settings → Date and time picks the time zone (automatic daylight saving) and syncs the clock over the Internet (can be turned off); HTTPS uses the real UTC time, checks revocation when the server staples an OCSP response, and accepts P-384; the file picker can switch drives, Ctrl+A selects text fields and unlocking an encrypted drive shows a progress indicator. 0.12.2 brought **HTTPS**: `curl https://…` in the Terminal checks the site's certificate against Mozilla's root list (TLS 1.3 and 1.2); if it is invalid it refuses and explains why, `-k` connects anyway with a warning, `-v` shows the version, cipher and certificate, and `-L` follows redirects. An administrator can add custom roots in Settings → Network → Certificates, and programs get a "secure connection" in the KIT that never skips the check. 0.12.1 opened the network to third-party programs behind a per-program permission prompt, like a firewall, and shipped **Mensajes**, an end-to-end encrypted chat on the local network. 0.12.0 brought wired networking (Intel 82540EM, 82574L, 82576, I210/I211, I217-I219 and I225/I226; Realtek RTL8111/8168; virtio-net) with DHCP, DNS, `ping`, `ipconfig`, `nslookup` and `curl`. Tested in QEMU; not yet on real hardware.

> **Aviso / Warning:** la criptografía (SHA-256/384/512, PBKDF2, AES-256-XTS, AES-GCM, X25519, HKDF, ChaCha20-Poly1305, RSA, ECDSA y el TLS de HTTPS) es una implementación propia **no auditada**, y HTTPS solo comprueba si un certificado está revocado cuando el servidor lo adjunta. No protejas con ella nada importante.
> The cryptography, including the TLS used for HTTPS, is a custom, **unaudited** implementation, and HTTPS only checks certificate revocation when the server staples it. Do not rely on it for anything important.

## Compilar y arrancar / Build and run
Requisitos / Requirements: Rust (con el target `x86_64-unknown-uefi`) y QEMU con firmware edk2.

```powershell
rustup target add x86_64-unknown-uefi
powershell -ExecutionPolicy Bypass -File run.ps1
```

Pruebas / Tests: `cargo test -p nf-core -p nf-web -p navegador`.
Fuentes incluidas / Bundled fonts: Liberation 2.1.5, SIL Open Font License 1.1 ([nf-core/data/fuentes/LICENSE](nf-core/data/fuentes/LICENSE)).
Arranque desde USB y herramienta `nfdisk` / USB boot and `nfdisk`: [docs/USB.md](docs/USB.md).
Hacer programas para el sistema / Writing programs: [docs/KIT-programas.md](docs/KIT-programas.md).
Reglas *clean room* y fuentes consultadas / Clean-room rules and sources: [docs/CLEANROOM.md](docs/CLEANROOM.md).

## Licencia / License
Copyright (C) 2026 Tare Fernandez Salas (**FuryNocturn**).

**GPL-3.0** ([LICENSE](LICENSE)) con un término adicional del §7(b) ([NOTICE](NOTICE)): toda copia u obra derivada debe conservar el fichero NOTICE y mostrar en su pantalla «Acerca de» el crédito:

GPL-3.0 with an additional term under section 7(b): every copy or derivative work must keep the NOTICE file and show this attribution in its "About" screen:

> **Basado en NightFury-OS de FuryNocturn** — https://github.com/FuryNocturn/NightFury-OS
