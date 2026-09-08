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
$tempBinary = Join-Path $installDir (".kaveon-" + [Guid]::NewGuid().ToString("N") + ".exe")
Write-Host "  Downloading from $url" -ForegroundColor DarkGray
try {
    Invoke-WebRequest -Uri $url -OutFile $tempBinary -UseBasicParsing
    if ((Get-Item -LiteralPath $tempBinary).Length -eq 0) {
        throw "The downloaded CLI is empty."
    }
    & $tempBinary --version
    if ($LASTEXITCODE -ne 0) {
        throw "The downloaded CLI could not run on this machine."
    }
    if (Test-Path -LiteralPath $dest) {
        # PowerShell can bind $null to an empty string for this .NET overload.
        # Use a real backup path and remove it only after replacement succeeds.
        $backupBinary = Join-Path $installDir (".kaveon-" + [Guid]::NewGuid().ToString("N") + ".bak")
        [IO.File]::Replace($tempBinary, $dest, $backupBinary)
        Remove-Item -LiteralPath $backupBinary -Force
    } else {
        [IO.File]::Move($tempBinary, $dest)
    }
} finally {
    if (Test-Path -LiteralPath $tempBinary) {
        Remove-Item -LiteralPath $tempBinary -Force
    }
}

Write-Host "  Installed: $dest" -ForegroundColor Green

# Add to PATH
$userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($installDir -notin ($userPath -split ';')) {
    [Environment]::SetEnvironmentVariable("PATH", "$userPath;$installDir", "User")
    Write-Host "  Added to PATH: $installDir" -ForegroundColor Green
}
if ($installDir -notin ($env:PATH -split ';')) {
    $env:PATH = "$env:PATH;$installDir"
}

Write-Host ""
Write-Host "  Done! Verify the installation:" -ForegroundColor White
Write-Host "    kaveon --version" -ForegroundColor DarkGray
Write-Host "  Connection steps (including your server and public CA):" -ForegroundColor White
Write-Host "    https://github.com/$repo/blob/dev/docs/engineering/azure-deployment-guide.md" -ForegroundColor DarkGray
Write-Host "  Kaveon reuses your Azure login when the server enables Microsoft authentication." -ForegroundColor DarkGray
Write-Host ""
