# Arranca NightFury en QEMU sin ventana, manda órdenes de monitor y guarda una captura PNG.
# Uso: tools\shot.ps1 -Out shots\a.png [-Seconds 8] [-Commands 'mouse_move 40 30','sendkey a'] [-Serial shots\a.log] [-NoBuild] [-Disk disco.img] [-NoDisk]
param(
    [string]$Out = 'shots\shot.png',
    [int]$Seconds = 8,
    [string[]]$Commands = @(),
    [string]$Serial = '',
    [int]$Port = 4455,
    [switch]$NoBuild,
    [string]$Disk = 'disco.img',
    [switch]$NoDisk
)
$ErrorActionPreference = 'Stop'
$Commands = @($Commands | ForEach-Object { $_ -split "," })  # con -File, una lista llega como una sola cadena con comas
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
New-Item -ItemType Directory -Force shots | Out-Null
if (-not $NoBuild) {
    # Los programas del sistema van dentro del núcleo (se instalan en C:\Programas al arrancar).
    & "$PSScriptRoot\programas.ps1" | Out-Null
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    cargo build -p kernel --release --target x86_64-unknown-uefi
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
New-Item -ItemType Directory -Force esp\EFI\BOOT | Out-Null
Copy-Item target\x86_64-unknown-uefi\release\nightfury.efi esp\EFI\BOOT\BOOTX64.EFI -Force
# Si tras un reinicio el firmware de QEMU cae en su consola (UEFI Shell), este guion arranca NightFury solo.
Set-Content -Path esp\startup.nsh -Value 'FS0:\EFI\BOOT\BOOTX64.EFI' -Encoding ascii

$ppm = [System.IO.Path]::GetFullPath((Join-Path $root 'shots\_tmp.ppm'))
Remove-Item $ppm -ErrorAction SilentlyContinue
$q = @(
    '-machine', 'q35', '-m', '1024',
    '-drive', '"if=pflash,format=raw,readonly=on,file=C:\Program Files\qemu\share\edk2-x86_64-code.fd"',
    '-drive', 'format=raw,file=fat:rw:esp',
    '-rtc', 'base=localtime', '-vga', 'std',
    '-device', 'qemu-xhci,id=xhci', '-device', 'usb-kbd,bus=xhci.0', '-device', 'usb-mouse,bus=xhci.0',
    '-display', 'none',
    '-monitor', "tcp:127.0.0.1:$Port,server,nowait"
)
if (-not $NoDisk) {
    $img = if ([System.IO.Path]::IsPathRooted($Disk)) { $Disk } else { [System.IO.Path]::GetFullPath((Join-Path $root $Disk)) }
    if (-not (Test-Path $img)) { fsutil file createnew $img 268435456 | Out-Null }
    $q += @('-drive', "id=datos,if=none,format=raw,file=$img", '-device', 'ide-hd,drive=datos,bus=ide.1')
}
if ($Serial) {
    $s = [System.IO.Path]::GetFullPath((Join-Path $root $Serial))
    $q += @('-serial', "file:$s")
}
$proc = Start-Process -FilePath 'C:\Program Files\qemu\qemu-system-x86_64.exe' -ArgumentList $q -PassThru -WindowStyle Hidden -WorkingDirectory $root
try {
    Start-Sleep -Seconds $Seconds
    $cli = $null
    for ($i = 0; $i -lt 20 -and -not $cli; $i++) {
        try { $cli = New-Object System.Net.Sockets.TcpClient('127.0.0.1', $Port) } catch { Start-Sleep -Milliseconds 300 }
    }
    if (-not $cli) { throw 'No se pudo conectar al monitor de QEMU' }
    $w = New-Object System.IO.StreamWriter($cli.GetStream())
    $w.AutoFlush = $true
    foreach ($c in $Commands) {
        if ($c.Trim() -eq 'doble') {
            # Doble clic: las cuatro órdenes seguidas (con 700 ms entre ellas no sería doble).
            foreach ($b in 1, 0, 1, 0) { $w.WriteLine("mouse_button $b"); Start-Sleep -Milliseconds 60 }
        } else { $w.WriteLine($c.Trim()) }
        Start-Sleep -Milliseconds 700
    }
    Start-Sleep -Milliseconds 800
    $w.WriteLine("screendump $ppm")  # el monitor no admite comillas: la ruta no debe tener espacios
    for ($i = 0; $i -lt 30 -and -not (Test-Path $ppm); $i++) { Start-Sleep -Milliseconds 300 }
    Start-Sleep -Milliseconds 500
    $ok = Test-Path $ppm
    if (-not $ok) {
        $buf = New-Object byte[] 4096
        $st = $cli.GetStream()
        while ($st.DataAvailable) { $n = $st.Read($buf, 0, 4096); Write-Host ([System.Text.Encoding]::ASCII.GetString($buf, 0, $n)) }
    }
    $w.WriteLine('quit')
    $cli.Close()
    if (-not $ok) { throw 'QEMU no genero la captura' }
    & "$PSScriptRoot\bmp2png.ps1" $ppm (Join-Path $root $Out)
    Remove-Item $ppm -ErrorAction SilentlyContinue
}
finally {
    if ($proc -and -not $proc.HasExited) {
        if (-not $proc.WaitForExit(3000)) { $proc.Kill() }
    }
}
