#!/bin/sh
# Busca claves privadas, tokens y emails de buzones personales.
# Uso: tools/secretos.sh --staged   (lo añadido y preparado en el índice)
#      tools/secretos.sh            (todo el árbol versionado)
# Sale con 1 si encuentra algo. Los patrones son por forma: no lleva ningún dato personal.
cd "$(git rev-parse --show-toplevel)" || exit 2

CLAVE='BEGIN (RSA |EC |OPENSSH |DSA |ENCRYPTED |)PRIVATE KEY'
TOKEN='ghp_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{22,}|sk-[A-Za-z0-9_-]{32,}|AKIA[0-9A-Z]{16}'
EMAIL='[A-Za-z0-9._%+-]+@(gmail|hotmail|outlook|yahoo|live|icloud|protonmail)\.|[A-Za-z0-9._%+-]+@proton\.me'

# Rutas con claves privadas de PRUEBA que el repo trae a propósito (certificados de test de TLS/X.509).
# Solo se relaja el patrón de clave privada en ellas; tokens y emails se siguen buscando.
PRUEBA='^nf-core/src/(tls|x509)/testdata/'

fallos=0
if [ "$1" = "--staged" ]; then
    # Solo las líneas añadidas, con el nombre del fichero delante.
    lineas=$(git diff --cached -U0 --diff-filter=AM --no-color -- . ':!tools/secretos.sh' |
        awk '/^\+\+\+ b\//{f=substr($0,7)} /^\+[^+]|^\+$/{print f": "substr($0,2)}')
    buscar() { printf '%s\n' "$lineas" | grep -E -e "$1"; }
else
    buscar() { git grep -InE -e "$1" -- . ':!tools/secretos.sh'; }
fi

for par in "clave privada|$CLAVE" "token|$TOKEN" "email personal|$EMAIL"; do
    nombre=${par%%|*}; patron=${par#*|}
    hits=$(buscar "$patron")
    [ "$nombre" = "clave privada" ] && hits=$(printf '%s\n' "$hits" | grep -Ev "$PRUEBA")
    if [ -n "$hits" ]; then
        echo "secretos: posible $nombre:" >&2
        printf '%s\n' "$hits" | cut -c1-120 | sed 's/^/  /' >&2
        fallos=1
    fi
done
[ "$fallos" = 0 ] && echo "secretos: nada sospechoso"
exit $fallos
