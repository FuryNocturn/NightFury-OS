# Contribuir

Los issues son bienvenidos (usa las plantillas). Las contribuciones de código llegan como pull request a
`master`; se publican bajo **GPL-3.0** y las reviso una a una, sin garantía de que se fusionen.

## Reglas

- **Sala limpia** ([docs/CLEANROOM.md](docs/CLEANROOM.md)): no copies código de otros proyectos; se trabaja
  solo a partir de especificaciones públicas (RFC, estándares, documentación oficial).
- **Sin dependencias de terceros**: el proyecto no usa crates externos.
- **Cabecera** en cada `.rs` nuevo (quien aporta añade además su propia línea de Copyright):

  ```rust
  // SPDX-License-Identifier: GPL-3.0-only
  // Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
  ```

- Los tests deben pasar y la lógica nueva lleva el suyo. Si cambias de versión, actualiza README y CHANGELOG.

## Compilar y probar

Necesitas Rust con el destino `x86_64-unknown-uefi` y QEMU con el firmware edk2 (ver «Probarlo» en el
[README](README.md)):

```powershell
rustup target add x86_64-unknown-uefi x86_64-unknown-none
cargo test -p nf-core -p nf-app -p nf-web -p navegador
powershell -ExecutionPolicy Bypass -File run.ps1
```

## Antes de tu primer commit

```sh
git config core.hooksPath .githooks
git config user.email <id>+<usuario>@users.noreply.github.com
```

El gancho rechaza claves, tokens, emails de buzones personales y commits con un email que no sea el
`noreply` de GitHub. La CI hace las mismas comprobaciones.

---

# Contributing

Issues are welcome (please use the templates). Code contributions come as pull requests to `master`; they are
released under **GPL-3.0** and I review them one by one, with no guarantee of merging.

## Rules

- **Clean room** ([docs/CLEANROOM.md](docs/CLEANROOM.md)): do not copy code from other projects; work only from
  public specifications (RFCs, standards, official documentation).
- **No third-party dependencies**: the project uses no external crates.
- **Header** in every new `.rs` file (contributors also add their own Copyright line):

  ```rust
  // SPDX-License-Identifier: GPL-3.0-only
  // Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
  ```

- Tests must pass and new logic comes with its own. For a release, update README and CHANGELOG.

## Build and test

You need Rust with the `x86_64-unknown-uefi` target and QEMU with the edk2 firmware (see "Probarlo" in the
[README](README.md)):

```powershell
rustup target add x86_64-unknown-uefi x86_64-unknown-none
cargo test -p nf-core -p nf-app -p nf-web -p navegador
powershell -ExecutionPolicy Bypass -File run.ps1
```

## Before your first commit

```sh
git config core.hooksPath .githooks
git config user.email <id>+<username>@users.noreply.github.com
```

The hook rejects keys, tokens, personal-mailbox emails and commits whose email is not GitHub's `noreply`. CI
runs the same checks.
