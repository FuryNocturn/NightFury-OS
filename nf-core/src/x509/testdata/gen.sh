#!/bin/sh
# Genera la PKI de prueba (certificados en DER, sin claves) con OpenSSL 3.5 (el de Git for Windows).
# Uso: sh gen.sh   (escribe aquí los .der; las claves viven en un directorio temporal)
# Variables que usa: NB/NA (fechas), H (hash, por defecto -sha256), OPTS (opciones extra de x509).
set -e
export MSYS_NO_PATHCONV=1
OUT=$(cd "$(dirname "$0")" && { pwd -W 2>/dev/null || pwd; })
T=$(mktemp -d)
cd "$T"
FROM=20260101000000Z
TO=20300101000000Z

# firmar <nombre> <sujeto> <emisor> <extensiones>   (la clave <nombre>.key ya existe)
firmar() {
  n=$1; subj=$2; iss=$3
  openssl req -new -key $n.key -subj "$subj" -out $n.csr
  printf "%s\n" "$4" > $n.ext
  openssl x509 -req -in $n.csr -CA $iss.pem -CAkey $iss.key -set_serial 0x$(openssl rand -hex 8) \
    -not_before ${NB:-$FROM} -not_after ${NA:-$TO} -extfile $n.ext ${H:--sha256} $OPTS -out $n.pem
  openssl x509 -in $n.pem -outform DER -out "$OUT/$n.der"
}
ec() { openssl ecparam -genkey -name ${2:-prime256v1} -noout -out $1.key; }

# hoja <nombre> <emisor> <san> [extensiones extra]: «prueba.nf», serverAuth
hoja() {
  ec $1
  firmar $1 "/CN=prueba.nf/O=NightFury Prueba" $2 "basicConstraints=CA:FALSE
extendedKeyUsage=serverAuth
subjectAltName=$3
$4"
}

# Raíz RSA-2048 (caduca en 2050 → GeneralizedTime) e intermedio EC P-256 (pathlen 0)
openssl genrsa -out raiz.key 2048
openssl req -new -x509 -key raiz.key -subj "/CN=NF Raiz/O=NightFury Prueba" -sha256 \
  -not_before 20200101000000Z -not_after 20500101000000Z \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" -out raiz.pem
openssl x509 -in raiz.pem -outform DER -out "$OUT/raiz.der"

ec inter
NB=20200101000000Z NA=20400101000000Z firmar inter "/CN=NF Intermedio/O=NightFury Prueba" raiz "basicConstraints=critical,CA:TRUE,pathlen:0
keyUsage=critical,keyCertSign"

hoja hoja inter "DNS:prueba.nf"
hoja comodin inter "DNS:*.prueba.nf"
hoja ip inter "IP:192.168.1.10,IP:2001:db8::1"
NB=20200101000000Z NA=20210101000000Z hoja caducada inter "DNS:prueba.nf"
NB=20400101000000Z NA=20410101000000Z hoja futura inter "DNS:prueba.nf"
hoja otrodominio inter "DNS:otro.nf"
hoja critica inter "DNS:prueba.nf" "1.2.3.4=critical,DER:0500"
# sin serverAuth: EKU solo clientAuth
ec sinserver
firmar sinserver "/CN=prueba.nf" inter "basicConstraints=CA:FALSE
extendedKeyUsage=clientAuth
subjectAltName=DNS:prueba.nf"
# hoja firmada por la raíz RSA con RSASSA-PSS (SHA-256, MGF1-SHA-256, sal 32)
OPTS="-sigopt rsa_padding_mode:pss -sigopt rsa_pss_saltlen:32 -sigopt rsa_mgf1_md:sha256" hoja pss raiz "DNS:prueba.nf"
# hoja autofirmada
ec auto
openssl req -new -x509 -key auto.key -subj "/CN=prueba.nf" -sha256 -not_before $FROM -not_after $TO \
  -addext "subjectAltName=DNS:prueba.nf" -out auto.pem
openssl x509 -in auto.pem -outform DER -out "$OUT/autofirmada.der"

# intermedio que no es CA, y su hoja
ec nocaint
NB=20200101000000Z NA=20400101000000Z firmar nocaint "/CN=NF Intermedio NoCA/O=NightFury Prueba" raiz "basicConstraints=CA:FALSE
keyUsage=digitalSignature"
hoja hoja_noca nocaint "DNS:prueba.nf"

# intermedios con nameConstraints: excluye prueba.nf / solo permite otra.nf / subárbol de IP
for v in excl:"excluded;DNS:prueba.nf" perm:"permitted;DNS:otra.nf" ip:"permitted;IP:10.0.0.0/255.0.0.0"; do
  k=${v%%:*}; nc=${v#*:}
  ec nc$k
  NB=20200101000000Z NA=20400101000000Z firmar nc$k "/CN=NF Intermedio NC/O=NightFury Prueba" raiz "basicConstraints=critical,CA:TRUE
keyUsage=keyCertSign
nameConstraints=critical,$nc"
  hoja hoja_nc$k nc$k "DNS:prueba.nf"
done

# pathLen: un CA bajo el intermedio (pathlen 0) y su hoja
ec inter2
NB=20200101000000Z NA=20400101000000Z firmar inter2 "/CN=NF Intermedio 2" inter "basicConstraints=critical,CA:TRUE
keyUsage=keyCertSign"
hoja hoja_pl inter2 "DNS:prueba.nf"

# cadena demasiado larga: raíz → c1 → … → c5 → hoja (7 eslabones)
p=raiz
for i in 1 2 3 4 5; do
  ec c$i
  NB=20200101000000Z NA=20400101000000Z firmar c$i "/CN=NF Cadena $i" $p "basicConstraints=critical,CA:TRUE
keyUsage=keyCertSign"
  p=c$i
done
hoja hoja_larga c5 "DNS:prueba.nf"

# raíz EC P-384 con una hoja directa (ecdsa-with-SHA384)
ec raiz384 secp384r1
openssl req -new -x509 -key raiz384.key -subj "/CN=NF Raiz 384/O=NightFury Prueba" -sha384 \
  -not_before 20200101000000Z -not_after 20400101000000Z \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign" -out raiz384.pem
openssl x509 -in raiz384.pem -outform DER -out "$OUT/raiz384.der"
H=-sha384 hoja hoja384 raiz384 "DNS:prueba.nf"
rm -rf "$T"
echo hecho
