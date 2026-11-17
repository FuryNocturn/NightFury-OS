# Compila todos los programas de programs\* para NightFury y los empaqueta como .nfx.
# Resultado: target\programas\<nombre>\programa.nfx + app.txt  (<nombre> = el valor de nombre= en app.txt).
# Uso: .\tools\programas.ps1    (sale con código distinto de 0 si algo falla)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

function Falla($m) { Write-Host "FALLO: $m" -ForegroundColor Red; exit 1 }

cargo build -p nfpack --release
if ($LASTEXITCODE -ne 0) { Falla 'no compila nfpack' }
$nfpack = 'target\release\nfpack.exe'

$progs = Get-ChildItem programs -Directory | Where-Object { Test-Path "$($_.FullName)\Cargo.toml" }
if (-not $progs) { Falla 'no hay programas en programs\' }
foreach ($p in $progs) {
    $app = "$($p.FullName)\app.txt"
    if (-not (Test-Path $app)) { Falla "$($p.Name): falta app.txt" }
    $nombre = (Get-Content $app -Encoding UTF8 | Where-Object { $_ -match '^nombre=(.+)$' } | ForEach-Object { $Matches[1].Trim() } | Select-Object -First 1)
    if (-not $nombre) { Falla "$($p.Name): app.txt no tiene nombre=" }
    # El nombre del paquete es el de la carpeta (así lo piden los Cargo.toml de programs\).
    cargo build -p $p.Name --target x86_64-unknown-none --release
    if ($LASTEXITCODE -ne 0) { Falla "$($p.Name): no compila" }
    $dest = "target\programas\$nombre"
    New-Item -ItemType Directory -Force $dest | Out-Null
    & $nfpack "target\x86_64-unknown-none\release\$($p.Name)" "$dest\programa.nfx"
    if ($LASTEXITCODE -ne 0) { Falla "$($p.Name): nfpack ha fallado" }
    Copy-Item $app "$dest\app.txt" -Force
}
Write-Host "OK: programas en target\programas\" -ForegroundColor Green
