# Regenera spasm/generated/*.c desde spasm/*.spasm (spasmc.py --target c).
#
# Requiere Python y el compilador del ecosistema (spasmc.py + sus generadores).
# Se puede apuntar a otra copia con NUIDE_SPASMC / NUIDE_PYTHON.
# El test `spasm_logic::tests::el_c_generado_esta_al_dia` avisa si el C
# commiteado quedó viejo.
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

$spasmc = $env:NUIDE_SPASMC
if (-not $spasmc) { $spasmc = "F:\SpAMS-Oficial\src\shared\spasmc.py" }
if (-not (Test-Path $spasmc)) {
    throw "No se encontró spasmc.py (definí NUIDE_SPASMC con la ruta completa)"
}
$python = $env:NUIDE_PYTHON
if (-not $python) { $python = "python" }

$generado = Join-Path $root "spasm\generated"
New-Item -ItemType Directory -Force -Path $generado | Out-Null

Get-ChildItem (Join-Path $root "spasm\*.spasm") | ForEach-Object {
    $out = Join-Path $generado ($_.BaseName + ".c")
    & $python $spasmc $_.FullName --target c --out $out
    if ($LASTEXITCODE -ne 0) { throw "falló la transpilación de $($_.Name)" }
}
Write-Output "ok: C generado en $generado"
