# Dot-source from PowerShell: . ./scripts/dev-env.ps1
$ErrorActionPreference = 'Stop'
$kaveonRoot = Split-Path -Parent $PSScriptRoot
$cargoBin = Join-Path $env:USERPROFILE '.cargo/bin'
if (Test-Path $cargoBin) { $env:PATH = "$cargoBin;$env:PATH" }
if (Get-Command fnm -ErrorAction SilentlyContinue) {
    fnm use (Get-Content (Join-Path $kaveonRoot '.nvmrc') -Raw).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Install the repository Node version with fnm install 22.' }
}
$apiPython = Join-Path $kaveonRoot 'api/venv/Scripts/python.exe'
if (Test-Path $apiPython) { $env:PYO3_PYTHON = $apiPython }
