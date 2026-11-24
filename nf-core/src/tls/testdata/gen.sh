#!/bin/sh
# PKI DE PRUEBA para las pruebas de interoperabilidad de TLS (OpenSSL 3.5).
# Las claves .key de esta carpeta son SOLO de prueba: no protegen nada y no se usan fuera de las pruebas.
# Uso: sh gen.sh        (regenera ca.der, *.pem y *.key y las respuestas OCSP; después hay que volver a grabar los vuelos, ver tests.rs)
#      sh gen.sh ocsp   (rehace solo las respuestas OCSP con la PKI que hay; hacen falta ca.key y srv_*.pem)
# Las respuestas OCSP (ocsp_*.der) son las que el servidor engrapa con `openssl s_server -status_file`:
#   bueno / revocado / caducado (nextUpdate a 1 día) / de otro certificado / con respondedor delegado /
#   delegado sin id-kp-OCSPSigning. thisUpdate es el momento de generarlas: la hora fija de las pruebas
#   (`ahora()` en tests.rs) tiene que ser posterior. La de «firma mala» sale de alterar un byte en la prueba.
set -e
export MSYS_NO_PATHCONV=1
cd "$(dirname "$0")"
FROM=20260101000000Z
TO=20500101000000Z

pki() {
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
}

# resp <hoja> <V|R> <días> <cert firmante> <clave firmante> <salida> [opciones de openssl ocsp]
resp() {
  serie=$(openssl x509 -in $1.pem -noout -serial | cut -d= -f2)
  asunto=$(openssl x509 -in $1.pem -noout -subject -nameopt compat | sed 's/^subject=//')
  if [ $2 = R ]; then rev=261101000000Z; else rev=; fi
  printf "$2	500101000000Z	$rev	$serie	unknown	$asunto
" > ocsp.idx
  openssl ocsp -issuer ca.pem -cert $1.pem -reqout ocsp.req
  openssl ocsp -index ocsp.idx -CA ca.pem -rsigner $4 -rkey $5 -reqin ocsp.req -respout $6 -ndays $3 $7
  rm ocsp.idx ocsp.req
}

# respondedor delegado: <nombre> <ext. de uso>
respondedor() {
  openssl ecparam -genkey -name prime256v1 -noout -out $1.key
  openssl req -new -key $1.key -subj "/CN=Respondedor OCSP $1/O=NightFury Prueba" -out $1.csr
  printf "basicConstraints=CA:FALSE
$2" > $1.ext
  openssl x509 -req -in $1.csr -CA ca.pem -CAkey ca.key -set_serial 0x$(openssl rand -hex 8)     -not_before $FROM -not_after $TO -extfile $1.ext -sha256 -out $1.pem
  rm $1.csr $1.ext
}

ocsp() {
  resp srv_ec V 36500 ca.pem ca.key ocsp_bueno.der -resp_no_certs
  resp srv_ec R 36500 ca.pem ca.key ocsp_revocado.der -resp_no_certs
  resp srv_ec V 1 ca.pem ca.key ocsp_caducado.der -resp_no_certs
  resp srv_otro V 36500 ca.pem ca.key ocsp_otro.der -resp_no_certs
  respondedor ocsp_resp "extendedKeyUsage=OCSPSigning
"
  respondedor ocsp_malo "keyUsage=digitalSignature
"
  resp srv_ec V 36500 ocsp_resp.pem ocsp_resp.key ocsp_delegado.der
  resp srv_ec V 36500 ocsp_malo.pem ocsp_malo.key ocsp_delegado_sin_eku.der
  rm ocsp_resp.pem ocsp_resp.key ocsp_malo.pem ocsp_malo.key
}

[ "$1" = ocsp ] || pki
ocsp
