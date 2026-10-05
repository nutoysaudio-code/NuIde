# Genera res/nuide.ico (256x256, PNG-in-ICO) para el ejecutable.
# Uso:  powershell -ExecutionPolicy Bypass -File packaging/make_icon.ps1
Add-Type -AssemblyName System.Drawing

$size = 256
$bmp = New-Object System.Drawing.Bitmap($size, $size)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::Transparent)

function New-RoundRect([int]$x, [int]$y, [int]$w, [int]$h, [int]$r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $p.AddArc($x, $y, $r, $r, 180, 90)
    $p.AddArc($x + $w - $r, $y, $r, $r, 270, 90)
    $p.AddArc($x + $w - $r, $y + $h - $r, $r, $r, 0, 90)
    $p.AddArc($x, $y + $h - $r, $r, $r, 90, 90)
    $p.CloseFigure()
    return $p
}

$path = New-RoundRect 10 10 236 236 52
$bg = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 21, 29, 31))
$g.FillPath($bg, $path)
$pen = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(255, 131, 217, 203), 10)
$g.DrawPath($pen, $path)

$font = New-Object System.Drawing.Font('Segoe UI', 150, [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
$brush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 131, 217, 203))
$sf = New-Object System.Drawing.StringFormat
$sf.Alignment = [System.Drawing.StringAlignment]::Center
$sf.LineAlignment = [System.Drawing.StringAlignment]::Center
$rectf = New-Object System.Drawing.RectangleF(0, 6, $size, $size)
$g.DrawString('N', $font, $brush, $rectf, $sf)

$g.Dispose()

$ms = New-Object System.IO.MemoryStream
$bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
$png = $ms.ToArray()
$ms.Dispose()
$bmp.Dispose()

$outDir = Join-Path $PSScriptRoot '..\res'
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$out = Join-Path $outDir 'nuide.ico'
$fs = [System.IO.File]::Create($out)
$bw = New-Object System.IO.BinaryWriter($fs)
$bw.Write([UInt16]0)          # reservado
$bw.Write([UInt16]1)          # tipo: icono
$bw.Write([UInt16]1)          # cantidad
$bw.Write([Byte]0)            # ancho 0 => 256
$bw.Write([Byte]0)            # alto 0 => 256
$bw.Write([Byte]0)            # colores
$bw.Write([Byte]0)            # reservado
$bw.Write([UInt16]1)          # planos
$bw.Write([UInt16]32)         # bits por pixel
$bw.Write([UInt32]$png.Length) # tamaño del PNG
$bw.Write([UInt32]22)         # offset (6 + 16)
$bw.Write($png)
$bw.Close()
Write-Output ("Icono generado: " + (Resolve-Path $out))
