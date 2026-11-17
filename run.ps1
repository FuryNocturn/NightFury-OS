# Compila NightFury en release, lo copia a la ESP y lo arranca en QEMU.
# Uso: .\run.ps1 [-Whpx] [-Disk otro.img]   (si el disco no existe, se crea en blanco de 256 MiB)
param([switch]$Whpx, [string]$Disk = 'disco.img')
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
cargo build -p kernel --release --target x86_64-unknown-uefi
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
New-Item -ItemType Directory -Force esp\EFI\BOOT | Out-Null
Copy-Item target\x86_64-unknown-uefi\release\nightfury.efi esp\EFI\BOOT\BOOTX64.EFI -Force
if (-not (Test-Path $Disk)) { fsutil file createnew $Disk 268435456 | Out-Null }
$q = @(
    '-machine', 'q35', '-m', '1024',
    '-drive', 'if=pflash,format=raw,readonly=on,file=C:\Program Files\qemu\share\edk2-x86_64-code.fd',
    '-drive', 'format=raw,file=fat:rw:esp',
    '-drive', "id=datos,if=none,format=raw,file=$Disk", '-device', 'ide-hd,drive=datos,bus=ide.1',
    '-rtc', 'base=localtime', '-vga', 'std'
)
if ($Whpx) { $q += @('-accel', 'whpx') }
& "C:\Program Files\qemu\qemu-system-x86_64.exe" @q
