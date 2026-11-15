# Convierte una imagen (BMP o PPM de QEMU) a PNG para poder revisarla.
# Uso: tools\bmp2png.ps1 entrada.bmp salida.png
param([Parameter(Mandatory)][string]$In, [Parameter(Mandatory)][string]$Out)
Add-Type -AssemblyName System.Drawing
$In = (Resolve-Path $In).Path
$Out = [System.IO.Path]::GetFullPath($Out)
if ($In -like '*.ppm') {
    $bytes = [System.IO.File]::ReadAllBytes($In)
    # Cabecera P6: "P6\n<ancho> <alto>\n255\n"
    $pos = 0; $tokens = @()
    while ($tokens.Count -lt 4) {
        while ($bytes[$pos] -in 9, 10, 13, 32) { $pos++ }
        $start = $pos
        while ($bytes[$pos] -notin 9, 10, 13, 32) { $pos++ }
        $tokens += [System.Text.Encoding]::ASCII.GetString($bytes, $start, $pos - $start)
    }
    $pos++
    $w = [int]$tokens[1]; $h = [int]$tokens[2]
    $bmp = New-Object System.Drawing.Bitmap($w, $h, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
    $data = $bmp.LockBits((New-Object System.Drawing.Rectangle(0, 0, $w, $h)), 'WriteOnly', $bmp.PixelFormat)
    $row = New-Object byte[] ($data.Stride)
    for ($y = 0; $y -lt $h; $y++) {
        $src = $pos + $y * $w * 3
        for ($x = 0; $x -lt $w; $x++) {
            $row[$x * 3] = $bytes[$src + $x * 3 + 2]
            $row[$x * 3 + 1] = $bytes[$src + $x * 3 + 1]
            $row[$x * 3 + 2] = $bytes[$src + $x * 3]
        }
        [System.Runtime.InteropServices.Marshal]::Copy($row, 0, [IntPtr]::Add($data.Scan0, $y * $data.Stride), $data.Stride)
    }
    $bmp.UnlockBits($data)
} else {
    $bmp = [System.Drawing.Image]::FromFile($In)
}
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
"OK $Out"
