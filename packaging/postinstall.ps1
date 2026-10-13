# Postinstalación de NuIde (lo corre el instalador Inno Setup).
#
# -AddPath: agrega la carpeta de NuIde (con `nuide.exe` y, si se instalaron,
# `nucoder.exe` y `nucook.exe`) a la variable PATH del usuario, idempotente.
#
# NuCoder y NuCook son binarios propios (no de npm): no necesitan Node.js.
param([switch]$AddPath)

$ErrorActionPreference = "Continue"

if ($AddPath) {
    $dir = $PSScriptRoot
    if ($dir) {
        $userPath = [System.Environment]::GetEnvironmentVariable("Path", "User")
        if (-not $userPath) { $userPath = "" }
        $entries = $userPath -split ';' | Where-Object { $_ -ne "" }
        if ($entries -notcontains $dir) {
            [System.Environment]::SetEnvironmentVariable("Path", (($entries + $dir) -join ';'), "User")
            Write-Host ("Agregado al PATH del usuario: " + $dir)
        } else {
            Write-Host "La carpeta ya estaba en el PATH del usuario."
        }
    }
}
