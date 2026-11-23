# Mensajes: protocolo (v1)

Mensajes es un programa de terceros del KIT (v0.12.1) para chatear entre equipos NightFury de la misma red
local, sin servidor y cifrado de extremo a extremo. Es un diseño propio hecho desde los RFC 7748 (X25519),
8439 (ChaCha20-Poly1305) y 5869 (HKDF); usa `nf_core::crypto`. **No está auditado.**

## Identidad

- Cada cuenta de cada equipo tiene una clave X25519 fija (la *identidad*). La privada son 32 bytes sacados de
  `sha256` de varias llamadas a `entropy()` y la hora; se guarda en `<carpeta del usuario>\Mensajes\identidad`
  (64 cifras hex). Si se borra, el programa crea otra y los demás te verán como un contacto nuevo.
- Un contacto **es** su clave pública. El nombre que anuncia es solo una etiqueta.
- Huella de una clave: las 16 primeras cifras hex de `sha256(clave pública)`.
- **Palabras de verificación** de una conversación: `h = sha256("NFM1 verificar" ‖ menor ‖ mayor)`, donde
  menor/mayor son las dos claves públicas ordenadas byte a byte; las palabras son `LISTA[h[0]] … LISTA[h[5]]`
  (6 palabras de una lista propia de 256 palabras en español, en minúsculas, sin tildes, todas distintas).
  Las dos personas ven las mismas seis; si coinciden (dichas en voz alta o en persona), se marca el contacto
  como verificado.

## Descubrimiento (UDP 47320)

Cada 5 s, y nada más empezar, se envía por difusión a `255.255.255.255:47320`:

| Bytes | Contenido |
|---|---|
| 4 | `NFM1` |
| 1 | `0x10` (anuncio) |
| 32 | clave pública de identidad |
| 2 | puerto TCP de mensajes (big endian) |
| 1 + n | longitud y nombre en UTF-8 (n ≤ 32, sin caracteres de control) |

- Se ignora lo que no cumpla el formato exacto, lo que traiga nuestra propia clave y lo que exceda los topes.
- Al ver un equipo nuevo se manda un anuncio extra (como mucho uno por segundo).
- Tabla de equipos vistos: 64 como mucho; un equipo pasa a «desconectado» a los 20 s sin anuncios.
- El anuncio no se fía de nada: si alguien anuncia una IP falsa, la conexión fallará en el saludo.

## Conexión (TCP 47321)

Quien tiene mensajes pendientes para un contacto conectado (*I*, iniciador) abre una conexión TCP a la IP del
anuncio y el otro (*R*) la acepta. Cada conexión tiene claves nuevas (secreto hacia delante por conexión).

### Saludo

```
I → R:  hola_I = "NFM1" ‖ 0x01 ‖ s_I ‖ e_I      (69 bytes)
R → I:  hola_R = "NFM1" ‖ 0x02 ‖ s_R ‖ e_R      (69 bytes)
```

`s_*` es la clave pública de identidad y `e_*` una efímera nueva para esta conexión. Los dos calculan:

```
ee = X25519(e_I, e_R)        es = X25519(e_I, s_R)        se = X25519(s_I, e_R)
th = sha256("NightFury Mensajes 1" ‖ hola_I ‖ hola_R)
prk = HKDF-Extract(sal = th, ikm = ee ‖ es ‖ se)
k_IR = HKDF-Expand(prk, "NFM1 i->r", 32)     k_RI = HKDF-Expand(prk, "NFM1 r->i", 32)
```

- Si cualquiera de los tres resultados es todo ceros, se corta.
- *I* corta si `s_R` no es la clave del contacto al que quería escribir.
- Solo quien tiene la privada de su identidad llega a las mismas claves: si alguien se hace pasar por otro,
  la primera trama no se descifra y se corta.
- Las privadas efímeras y las claves de la conexión se borran (ceros) al cerrar.
- *R* corta si el saludo no llega completo en 5 s. Como mucho 8 conexiones entrantes a la vez.

### Tramas

```
longitud (u16 big endian, ≤ 4112) ‖ sellado
sellado = ChaCha20-Poly1305(clave, nonce = 0u32 ‖ contador u64 LE, aad = los 2 bytes de longitud, texto)
```

- Un contador por sentido, desde 0, que sube en cada trama; nunca se repite con la misma clave.
- Si una trama no se descifra o la longitud se pasa del tope, se corta la conexión.

Contenido (primer byte = tipo):

| Tipo | Campos |
|---|---|
| `1` texto | id u64 (al azar) ‖ fecha (año u16, mes, día, hora, min, seg: 7 bytes) ‖ texto (u16 + UTF-8, ≤ 4000 bytes, ≤ 2000 caracteres) |
| `2` recibido | id u64 |
| `3` adiós | — |

*I* envía todos los pendientes de ese contacto, espera un «recibido» por cada uno y manda «adiós»; los dos
cierran. Si en 10 s no ha terminado, se corta y lo no confirmado sigue pendiente. *R* descarta textos con un
id ya visto de ese contacto (recuerda los 256 últimos) pero confirma igual.

## Estados de un mensaje enviado

«Enviando…» → «Entregado» al llegar el «recibido». Si el contacto está desconectado: «Pendiente» (se envía
solo cuando vuelve a anunciarse). Se reintenta cada 30 s mientras esté conectado.

## Lo que se guarda (`<carpeta del usuario>\Mensajes\`)

- `identidad`: la clave privada (ver arriba).
- `contactos`: una línea por contacto, `clave pública hex \t nombre \t verificado (0/1)`.
- `<huella>.txt`: la conversación, una línea por mensaje:
  `> o < \t AAAA-MM-DD HH:MM:SS \t id hex \t estado (E entregado, P pendiente, R recibido) \t texto`.
  En el texto, `\` se escribe `\\`, el salto de línea `\n` y el tabulador `\t`. Se guardan los 1000 últimos.
- Si la carpeta del usuario está cifrada (Configuración → Cuentas), todo esto también lo está.

## Permisos de red

Mensajes pide los dos: «usar la red» (enviar anuncios y conectar) y «aceptar conexiones» (recibir anuncios en
el 47320 y escuchar en el 47321). Mientras el aviso está abierto las llamadas devuelven `Pending` y el
programa lo reintenta; con `Denied` lo dice y ofrece reintentar tras cambiarlo en Configuración → Red.
