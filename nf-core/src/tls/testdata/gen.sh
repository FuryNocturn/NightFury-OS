#!/bin/sh
# PKI DE PRUEBA para las pruebas de interoperabilidad de TLS (OpenSSL 3.5).
# Las claves .key de esta carpeta son SOLO de prueba: no protegen nada y no se usan fuera de las pruebas.
# Uso: sh gen.sh   (regenera ca.der, *.pem y *.key; después hay que volver a grabar los vuelos, ver tests.rs)
set -e
export MSYS_NO_PATHCONV=1
cd "$(dirname "$0")"
FROM=20260101000000Z
TO=20500101000000Z

openssl ecparam -genkey -name prime256v1 -noout -out ca.key
openssl req -new -x509 -key ca.key -subj "/CN=NF TLS Raiz de Prueba/O=NightFury Prueba" -sha256 \
  -not_before $FROM -not_after $TO \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign" -out ca.pem
openssl x509 -in ca.pem -outform DER -out ca.der

# hoja <nombre> <nombre DNS> <curva | rsa>
hoja() {
  case $3 in
    rsa) openssl genrsa -out $1.key 2048 ;;
    *) openssl ecparam -genkey -name $3 -noout -out $1.key ;;
  esac
  openssl req -new -key $1.key -subj "/CN=$2/O=NightFury Prueba" -out $1.csr
  printf "basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:$2\n" > $1.ext
  openssl x509 -req -in $1.csr -CA ca.pem -CAkey ca.key -set_serial 0x$(openssl rand -hex 8) \
    -not_before $FROM -not_after $TO -extfile $1.ext -sha256 -out $1.pem
  rm $1.csr $1.ext
}
hoja srv_ec prueba.nf prime256v1
hoja srv_p384 prueba.nf secp384r1
hoja srv_rsa prueba.nf rsa
hoja srv_otro otro.nf prime256v1
