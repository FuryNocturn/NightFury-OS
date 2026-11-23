# Certificados raíz (raices.bin)

`raices.bin` son los certificados raíz de confianza para sitios web de la lista de Mozilla NSS, sacados
de `certdata.txt` con `tools/raices.ps1` (formato: secuencia de `[u16 big endian longitud][DER]`).

- Origen: https://hg.mozilla.org/projects/nss/raw-file/tip/lib/ckfw/builtins/certdata.txt
  (descargado el 2026-11-23).
- Solo entran las raíces con `CKA_TRUST_SERVER_AUTH = CKT_NSS_TRUSTED_DELEGATOR` y sin
  `CKA_NSS_SERVER_DISTRUST_AFTER` (109 raíces).
- Se renueva con cada versión de NightFury: se descarga otra vez `certdata.txt` y se vuelve a ejecutar
  `tools/raices.ps1 <certdata.txt>`.

## Licencia

Estos datos proceden de Mozilla y NO están bajo la GPL de NightFury:

> This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the
> MPL was not distributed with this file, You can obtain one at http://mozilla.org/MPL/2.0/.

La forma original (`certdata.txt`) está disponible en la dirección de arriba.
