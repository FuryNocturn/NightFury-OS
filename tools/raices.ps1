# SPDX-License-Identifier: GPL-3.0-only
# Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
#
# Convierte el certdata.txt de Mozilla NSS en nf-core/data/raices/raices.bin: una secuencia de
# [u16 longitud big endian][certificado DER] (formato de x509::RootStore::load_bundle).
# No descarga nada: se le pasa la ruta del certdata.txt.
#
# Entran solo las raíces cuyo objeto de confianza tiene CKA_TRUST_SERVER_AUTH =
# CKT_NSS_TRUSTED_DELEGATOR y NO tiene CKA_NSS_SERVER_DISTRUST_AFTER con fecha (conservador).
# El certificado y su confianza se emparejan por emisor + número de serie.
#
# Uso: powershell -File tools\raices.ps1 <certdata.txt> [-Salida <raices.bin>]
param(
    [Parameter(Mandatory = $true)][string]$CertData,
    [string]$Salida = ''
)
# En PowerShell 5.1 $PSScriptRoot puede estar vacío en los valores por defecto de param.
if (-not $Salida) { $Salida = Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path) '..\nf-core\data\raices\raices.bin' }
$ErrorActionPreference = 'Stop'

# Separa el fichero en objetos (cada uno empieza en su CKA_CLASS) con sus atributos.
# Los MULTILINE_OCTAL se guardan como byte[] y el resto como el texto tras el tipo.
$objetos = New-Object System.Collections.Generic.List[hashtable]
$actual = $null
$lineas = [System.IO.File]::ReadAllLines((Resolve-Path $CertData), [System.Text.Encoding]::UTF8)
$i = 0
while ($i -lt $lineas.Length) {
    $l = $lineas[$i++]
    if ($l -match '^CKA_CLASS\s') {
        $actual = @{}
        $objetos.Add($actual)
    }
    if ($null -eq $actual -or $l -match '^\s*(#|$)') { continue }
    if ($l -match '^(CKA_\w+)\s+MULTILINE_OCTAL\s*$') {
        $nombre = $Matches[1]
        $bytes = New-Object System.Collections.Generic.List[byte]
        while ($i -lt $lineas.Length -and $lineas[$i].Trim() -ne 'END') {
            foreach ($m in [regex]::Matches($lineas[$i], '\\([0-7]{3})')) {
                $bytes.Add([Convert]::ToByte($m.Groups[1].Value, 8))
            }
            $i++
        }
        $i++ # END
        $actual[$nombre] = $bytes.ToArray()
    }
    elseif ($l -match '^(CKA_\w+)\s+(\S+)\s*(.*)$') {
        $actual[$Matches[1]] = $Matches[3].Trim()
    }
}

function Clave([hashtable]$o) {
    [BitConverter]::ToString($o['CKA_ISSUER']) + '/' + [BitConverter]::ToString($o['CKA_SERIAL_NUMBER'])
}

# Confianza por emisor+serie: solo las de servidor, delegador de confianza y sin fecha de desconfianza.
$confiables = @{}
foreach ($o in $objetos) {
    if ($o['CKA_CLASS'] -notmatch 'CKO_NSS_TRUST') { continue }
    if (-not $o.ContainsKey('CKA_ISSUER') -or -not $o.ContainsKey('CKA_SERIAL_NUMBER')) { continue }
    $distrust = $o['CKA_NSS_SERVER_DISTRUST_AFTER']
    if ($o['CKA_TRUST_SERVER_AUTH'] -eq 'CKT_NSS_TRUSTED_DELEGATOR' -and
        -not ($distrust -is [byte[]] -and $distrust.Length -gt 0)) {
        $confiables[(Clave $o)] = $true
    }
}

$ms = New-Object System.IO.MemoryStream
$n = 0
foreach ($o in $objetos) {
    if ($o['CKA_CLASS'] -notmatch 'CKO_CERTIFICATE' -or -not $o.ContainsKey('CKA_VALUE')) { continue }
    if (-not $confiables.ContainsKey((Clave $o))) { continue }
    # En el certdata real la fecha de desconfianza va en el objeto del certificado, no en el de confianza.
    $distrust = $o['CKA_NSS_SERVER_DISTRUST_AFTER']
    if ($distrust -is [byte[]] -and $distrust.Length -gt 0) { continue }
    $der = [byte[]]$o['CKA_VALUE']
    if ($der.Length -eq 0 -or $der.Length -gt 65535) { Write-Warning "se salta un certificado de $($der.Length) bytes"; continue }
    $ms.WriteByte([byte]($der.Length -shr 8))
    $ms.WriteByte([byte]($der.Length -band 0xff))
    $ms.Write($der, 0, $der.Length)
    $n++
}
if ($n -eq 0) { throw 'No ha salido ninguna raíz: ¿es un certdata.txt de Mozilla?' }
$dir = Split-Path -Parent $Salida
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
[System.IO.File]::WriteAllBytes($Salida, $ms.ToArray())
Write-Host "$n raíces -> $Salida ($($ms.Length) bytes)"
