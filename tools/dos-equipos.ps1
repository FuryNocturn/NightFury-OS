# Arranca dos NightFury en QEMU unidos por una red local virtual, con Mensajes instalado en los dos.
# Uso: .\tools\dos-equipos.ps1 [-Whpx] [-Nuevos]
# A lleva la red de usuario de QEMU (DHCP: A sale como 10.0.2.15 y B como 10.0.2.16) y un concentrador
# que B alcanza por un socket local, así que se ven entre sí como en la misma red.
# Discos: disco-a.img y disco-b.img (la primera vez, copias de disco.img si existe; -Nuevos los rehace).
param([switch]$Whpx, [switch]$Nuevos)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
& "$PSScriptRoot\programas.ps1" | Out-Null
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo build -p kernel --release --target x86_64-unknown-uefi
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
# Una ESP para cada uno: QEMU no lleva bien dos máquinas escribiendo en la misma carpeta fat:rw.
foreach ($esp in 'esp', 'target\esp-b') {
    New-Item -ItemType Directory -Force "$esp\EFI\BOOT" | Out-Null
    Copy-Item target\x86_64-unknown-uefi\release\nightfury.efi "$esp\EFI\BOOT\BOOTX64.EFI" -Force
    Set-Content -Path "$esp\startup.nsh" -Value 'FS0:\EFI\BOOT\BOOTX64.EFI' -Encoding ascii
}
foreach ($d in 'disco-a.img', 'disco-b.img') {
    if ($Nuevos -or -not (Test-Path $d)) {
        if (Test-Path disco.img) { Copy-Item disco.img $d -Force }
        else { cargo run -q -p nfdisk -- crear $d 256; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE } }
    }
    cargo run -q -p nfdisk -- instalar $d target\programas\Mensajes --reemplazar
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
function Get-Args([string]$esp, [string]$disk, [string]$mac, [string[]]$net) {
    $q = @(
        '-machine', 'q35', '-m', '1024',
        '-drive', '"if=pflash,format=raw,readonly=on,file=C:\Program Files\qemu\share\edk2-x86_64-code.fd"',
        '-drive', "format=raw,file=fat:rw:$esp",
        '-drive', "id=datos,if=none,format=raw,file=$disk", '-device', 'ide-hd,drive=datos,bus=ide.1',
        '-rtc', 'base=localtime', '-vga', 'std',
        '-device', 'qemu-xhci,id=xhci', '-device', 'usb-kbd,bus=xhci.0', '-device', 'usb-mouse,bus=xhci.0'
    ) + $net + @('-netdev', 'hubport,id=nic,hubid=0', '-device', "e1000e,netdev=nic,mac=$mac")
    if ($Whpx) { $q += @('-accel', 'whpx') }
    $q
}
$qemu = 'C:\Program Files\qemu\qemu-system-x86_64.exe'
$a = Get-Args 'esp' 'disco-a.img' '52:54:00:4e:46:0a' @(
    '-name', 'NightFury-A',
    '-netdev', 'user,id=u0', '-netdev', 'hubport,id=pu,hubid=0,netdev=u0',
    '-netdev', 'socket,id=s0,listen=127.0.0.1:47500', '-netdev', 'hubport,id=ps,hubid=0,netdev=s0')
Start-Process -FilePath $qemu -ArgumentList $a -WorkingDirectory $root | Out-Null
Start-Sleep -Seconds 2  # B se conecta al socket que abre A
$b = Get-Args 'target\esp-b' 'disco-b.img' '52:54:00:4e:46:0b' @(
    '-name', 'NightFury-B',
    '-netdev', 'socket,id=s0,connect=127.0.0.1:47500', '-netdev', 'hubport,id=ps,hubid=0,netdev=s0')
Start-Process -FilePath $qemu -ArgumentList $b -WorkingDirectory $root | Out-Null
