# Postinstalación de NuIde (lo corre el instalador Inno Setup).
#
# 1) Asegura Node.js (las CLI de IA son de Node).
# 2) Si se pasa -Clis, instala las CLI de IA con npm.
#
# Los paquetes de npm pueden cambiar: ajustá la lista de abajo si hace falta.
param([switch]$Clis)

$ErrorActionPreference = "Continue"

function Refresh-Path {
    $env:Path = [System.Environment]::GetEnvironmentVariable("Path", "Machine") + ";" +
                [System.Environment]::GetEnvironmentVariable("Path", "User")
}

# --- Node.js ---
if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
    Write-Host "Node.js no está instalado."
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        Write-Host "Instalando Node.js LTS con winget..."
        winget install -e --id OpenJS.NodeJS.LTS --silent `
            --accept-source-agreements --accept-package-agreements
        Refresh-Path
    } else {
        Write-Host "No hay winget. Instalá Node.js LTS desde https://nodejs.org y volvé a correr la instalación de las CLI."
    }
} else {
    Write-Host ("Node.js ya instalado: " + (node --version))
}

# --- CLI de IA ---
if ($Clis) {
    if (-not (Get-Command npm -ErrorAction SilentlyContinue)) {
        Refresh-Path
    }
    if (Get-Command npm -ErrorAction SilentlyContinue) {
        # Lista editable. `opencode-ai` es la CLI de OpenCode.
        $packages = @(
            "opencode-ai"
            # "kilo-code"   # ajustar al nombre real del paquete
            # "mimo"        # ajustar al nombre real del paquete
        )
        foreach ($pkg in $packages) {
            Write-Host ("Instalando " + $pkg + " ...")
            npm install -g $pkg
        }
    } else {
        Write-Host "npm no está disponible; no se instalaron las CLI."
    }
}
