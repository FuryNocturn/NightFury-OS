# Compila NightFury en release, lo copia a la ESP y lo arranca en QEMU.
# Uso: .\run.ps1 [-Whpx]
param([switch]$Whpx)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
cargo build -p kernel --release --target x86_64-unknown-uefi
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
New-Item -ItemType Directory -Force esp\EFI\BOOT | Out-Null
Copy-Item target\x86_64-unknown-uefi\release\nightfury.efi esp\EFI\BOOT\BOOTX64.EFI -Force
$q = @(
    '-machine', 'q35', '-m', '1024',
    '-drive', 'if=pflash,format=raw,readonly=on,file=C:\Program Files\qemu\share\edk2-x86_64-code.fd',
    '-drive', 'format=raw,file=fat:rw:esp',
    '-rtc', 'base=localtime', '-vga', 'std'
)
if ($Whpx) { $q += @('-accel', 'whpx') }
& "C:\Program Files\qemu\qemu-system-x86_64.exe" @q
