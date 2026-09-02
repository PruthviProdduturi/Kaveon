# Install Kaveon Engine CLI (Windows)
# Usage: irm https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.ps1 | iex
#    or: .\scripts\install.ps1

$ErrorActionPreference = "Stop"

$repo = "PruthviProdduturi/Kaveon"
$asset = "kaveon-windows-x64.exe"
$installDir = "$env:LOCALAPPDATA\kaveon\bin"

Write-Host ""
Write-Host "  Installing Kaveon Engine CLI" -ForegroundColor Cyan
Write-Host ""

# Find latest release
$tag = "engine-dev"
$url = "https://github.com/$repo/releases/download/$tag/$asset"

# Create install directory
New-Item -ItemType Directory -Force -Path $installDir | Out-Null

# Download
$dest = "$installDir\kaveon.exe"
Write-Host "  Downloading from $url" -ForegroundColor DarkGray
Invoke-WebRequest -Uri $url -OutFile $dest -UseBasicParsing

if (-not (Test-Path $dest)) {
    Write-Host "  Download failed." -ForegroundColor Red
    exit 1
}

Write-Host "  Installed: $dest" -ForegroundColor Green

# Add to PATH
$userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($userPath -notlike "*$installDir*") {
    [Environment]::SetEnvironmentVariable("PATH", "$userPath;$installDir", "User")
    $env:PATH = "$env:PATH;$installDir"
    Write-Host "  Added to PATH: $installDir" -ForegroundColor Green
}

Write-Host ""
Write-Host "  Done! Restart your terminal, then:" -ForegroundColor White
Write-Host "    kaveon --version" -ForegroundColor DarkGray
Write-Host "    kaveon --data-dir C:\path\to\parquet\files" -ForegroundColor DarkGray
Write-Host ""
