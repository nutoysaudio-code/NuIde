# Genera el instalador de NuIde (Windows).
#
#   powershell -ExecutionPolicy Bypass -File packaging\make_installer.ps1
#   powershell -ExecutionPolicy Bypass -File packaging\make_installer.ps1 -SkipBuild
#
# 1) `cargo build --release` (el exe de release es de ventanas: sin consola).
# 2) Compila `packaging\NuIde.iss` con Inno Setup 6 (ISCC.exe).
# 3) Deja releases\NuIde-<version>-setup.exe
#
# ISCC.exe no suele estar en el PATH (la instalación por usuario lo pone en
# %LOCALAPPDATA%\Programs\Inno Setup 6), así que se busca en las rutas típicas.

[CmdletBinding()]
param([switch]$SkipBuild)

$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

# --- 1) Binarios release ---
if (-not $SkipBuild) {
    Write-Host "Compilando en modo release..."
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build fallo ($LASTEXITCODE)." }

    # Nuide-CLI es su propio workspace: se compila aparte.
    Write-Host "Compilando Nuide-CLI (release)..."
    cargo build --release --manifest-path (Join-Path $repo "nuide-cli\Cargo.toml")
    if ($LASTEXITCODE -ne 0) { throw "cargo build de nuide-cli fallo ($LASTEXITCODE)." }
}

$exe = Join-Path $repo "target\release\nuide.exe"
if (-not (Test-Path $exe)) { throw "No existe $exe. Corré el script sin -SkipBuild." }
$cli = Join-Path $repo "nuide-cli\target\release\nuide-cli.exe"
if (-not (Test-Path $cli)) { throw "No existe $cli. Corré el script sin -SkipBuild." }

# El exe de release NO debe ser de consola (si no, aparece una terminal vacía
# antes de la ventana). Se lee el campo Subsystem del encabezado PE.
$bytes = [System.IO.File]::ReadAllBytes($exe)
$peOff = [System.BitConverter]::ToInt32($bytes, 0x3C)
$subsystem = [System.BitConverter]::ToUInt16($bytes, $peOff + 24 + 68)
if ($subsystem -ne 2) {
    Write-Host ("AVISO: nuide.exe es subsistema {0} (2 = ventanas). " -f $subsystem) +
        "Revisá el #![cfg_attr(not(debug_assertions), windows_subsystem = ""windows"")] de src\main.rs."
}

# --- 2) ISCC.exe ---
$iscc = $null
$cmd = Get-Command iscc -ErrorAction SilentlyContinue
if ($cmd) { $iscc = $cmd.Source }
if (-not $iscc) {
    $candidates = @(
        (Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe"),
        "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
        "C:\Program Files\Inno Setup 6\ISCC.exe"
    )
    foreach ($c in $candidates) {
        if (Test-Path $c) { $iscc = $c; break }
    }
}
if (-not $iscc) {
    throw @'
No se encontro ISCC.exe (Inno Setup 6). Instalalo con:
    winget install -e --id JRSoftware.InnoSetup
y volve a correr este script.
'@
}
Write-Host ("Usando Inno Setup: " + $iscc)

# --- 3) Compilar el instalador ---
& $iscc (Join-Path $repo "packaging\NuIde.iss")
if ($LASTEXITCODE -ne 0) { throw "ISCC fallo ($LASTEXITCODE)." }

# El nombre del archivo lleva la version del Cargo.toml.
$cargo = Get-Content (Join-Path $repo "Cargo.toml") -TotalCount 40 |
    Select-String -Pattern '^version\s*=\s*"([^"]+)"' |
    ForEach-Object { $_.Matches[0].Groups[1].Value }
$out = Join-Path $repo "releases\NuIde-$cargo-setup.exe"
if (Test-Path $out) {
    $info = Get-Item $out
    Write-Host ""
    Write-Host ("Instalador: " + $info.FullName)
    Write-Host ("Tamanio   : {0:N1} MB   (actualizado: {1})" -f ($info.Length / 1MB), $info.LastWriteTime)
} else {
    Write-Host "ISCC termino, pero no se encontro $out (revisar OutputBaseFilename en NuIde.iss)."
}
