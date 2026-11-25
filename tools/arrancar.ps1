# Arranca NightFury OS en QEMU (Windows). Es una versión de prueba.
# Uso: powershell -ExecutionPolicy Bypass -File arrancar.ps1 [-Disk otro.img]
# Debe estar en la misma carpeta que BOOTX64.EFI. Necesita QEMU para Windows: https://www.qemu.org/download/#windows
# Si el disco (disco.img, 256 MiB) no existe, se crea en blanco.
param([string]$Disk = 'disco.img')
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
if (-not (Test-Path BOOTX64.EFI)) { Write-Host 'Falta BOOTX64.EFI junto a este guion.'; exit 1 }
$cmd = Get-Command qemu-system-x86_64.exe -ErrorAction SilentlyContinue
$qemu = if ($cmd) { $cmd.Source } else { 'C:\Program Files\qemu\qemu-system-x86_64.exe' }
$fw = Join-Path (Split-Path $qemu) 'share\edk2-x86_64-code.fd'
if (-not ((Test-Path $qemu) -and (Test-Path $fw))) {
    Write-Host 'No encuentro QEMU (qemu-system-x86_64.exe y share\edk2-x86_64-code.fd).'
    Write-Host 'Instalalo desde la web oficial: https://www.qemu.org/download/#windows'
    exit 1
}
New-Item -ItemType Directory -Force esp\EFI\BOOT | Out-Null
Copy-Item BOOTX64.EFI esp\EFI\BOOT\BOOTX64.EFI -Force
# Si el firmware cae en su consola (UEFI Shell), arranca NightFury solo.
Set-Content -Path esp\startup.nsh -Value 'FS0:\EFI\BOOT\BOOTX64.EFI' -Encoding ascii
if (-not (Test-Path $Disk)) { fsutil file createnew $Disk 268435456 | Out-Null }
& $qemu -machine q35 -m 1024 `
    -drive "if=pflash,format=raw,readonly=on,file=$fw" `
    -drive 'format=raw,file=fat:rw:esp' `
    -drive "id=datos,if=none,format=raw,file=$Disk" -device ide-hd,drive=datos,bus=ide.1 `
    -rtc base=localtime -vga std `
    -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0 -device usb-mouse,bus=xhci.0
