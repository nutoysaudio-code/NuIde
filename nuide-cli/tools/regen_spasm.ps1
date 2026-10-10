# Regenera el C de los módulos SpASM (dialecto AOT) de Nuide-CLI.
#
#   powershell -ExecutionPolicy Bypass -File tools/regen_spasm.ps1
#
# Requiere Python + spasmc.py (el mismo toolchain que NuIde). El C generado se
# commitea en spasm/generated/ para que un clone limpio compile sin Python.
#
# Rutas configurables con variables de entorno:
#   NUIDE_PYTHON       -> intérprete de Python
#   NUIDE_SPASMC       -> ruta a spasmc.py

$ErrorActionPreference = "Stop"

function Find-Python {
    if ($env:NUIDE_PYTHON -and (Test-Path $env:NUIDE_PYTHON)) { return $env:NUIDE_PYTHON }
    foreach ($c in @("python", "python3", "py")) {
        $cmd = Get-Command $c -ErrorAction SilentlyContinue
        if ($cmd) { return $cmd.Source }
    }
    throw "No se encontró Python (definí NUIDE_PYTHON)"
}

function Find-Spasmc {
    if ($env:NUIDE_SPASMC -and (Test-Path $env:NUIDE_SPASMC)) { return $env:NUIDE_SPASMC }
    $known = "F:\SpAMS-Oficial\src\shared\spasmc.py"
    if (Test-Path $known) { return $known }
    throw "No se encontró spasmc.py (definí NUIDE_SPASMC)"
}

$root = Split-Path -Parent $PSScriptRoot   # raíz del proyecto (nuide-cli/)
$spasmDir = Join-Path $root "spasm"
$outDir = Join-Path $spasmDir "generated"
New-Item -ItemType Directory -Force $outDir | Out-Null

$python = Find-Python
$spasmc = Find-Spasmc

$files = Get-ChildItem -Path $spasmDir -Filter *.spasm
foreach ($f in $files) {
    $out = Join-Path $outDir ($f.BaseName + ".c")
    Write-Host "spasmc: $($f.Name) -> generated\$($f.BaseName).c"
    & $python $spasmc $f.FullName --target c --out $out
    if ($LASTEXITCODE -ne 0) { throw "spasmc.py falló para $($f.Name)" }
}

Write-Host "Listo. C regenerado en spasm/generated/"
