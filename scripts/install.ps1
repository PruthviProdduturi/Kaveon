# Install the Kaveon CLI (Windows)
#
# Preview build (the moving engine-dev prerelease, default):
#   irm https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.ps1 | iex
# Tagged release, verified against its SHA256SUMS:
#   $env:KAVEON_VERSION = "0.3.0"; irm https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.ps1 | iex
#   .\scripts\install.ps1 -Version 0.3.0
#
# Environment: KAVEON_VERSION (release to install; empty means the preview),
# KAVEON_INSTALL_DIR (default %LOCALAPPDATA%\kaveon\bin), KAVEON_DOWNLOAD_BASE
# (a mirror of https://github.com/PruthviProdduturi/Kaveon/releases/download).
param(
    [string]$Version = $env:KAVEON_VERSION,
    [string]$InstallDir = $env:KAVEON_INSTALL_DIR,
    [string]$DownloadBase = $env:KAVEON_DOWNLOAD_BASE
)

$ErrorActionPreference = "Stop"

$repo = "PruthviProdduturi/Kaveon"
$previewTag = "engine-dev"
$previewAsset = "kaveon-windows-x64.exe"
$target = "x86_64-pc-windows-msvc"
if (-not $InstallDir) { $InstallDir = "$env:LOCALAPPDATA\kaveon\bin" }
if (-not $DownloadBase) { $DownloadBase = "https://github.com/$repo/releases/download" }

# Accept 0.3.0, v0.3.0 or cli-v0.3.0.
$Version = $Version -replace '^(cli-v|v)', ''
if ($Version -and $Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') {
    throw "'$Version' is not a release version (expected X.Y.Z)."
}

Write-Host ""
Write-Host "  Installing Kaveon CLI" -ForegroundColor Cyan
Write-Host ""

function Get-ReleaseFile([string]$Tag, [string]$Asset, [string]$Destination) {
    $url = "$DownloadBase/$Tag/$Asset"
    Write-Host "  Downloading $url" -ForegroundColor DarkGray
    Invoke-WebRequest -Uri $url -OutFile $Destination -UseBasicParsing
    if ((Get-Item -LiteralPath $Destination).Length -eq 0) {
        throw "The download of $Asset is empty."
    }
}

function Test-CliVersion([string]$Binary, [string]$Expected) {
    $reported = (& $Binary --version | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "The downloaded CLI could not run on this machine."
    }
    Write-Host "  $reported"
    if ($Expected -and $reported -ne "kaveon $Expected") {
        throw "The binary reports '$reported', expected 'kaveon $Expected'."
    }
}

# Copies $Source over $InstallDir\kaveon.exe through a staged file so a running
# kaveon.exe is replaced atomically.
function Install-Binary([string]$Source) {
    $dest = Join-Path $InstallDir "kaveon.exe"
    $staged = Join-Path $InstallDir (".kaveon-" + [Guid]::NewGuid().ToString("N") + ".exe")
    try {
        Copy-Item -LiteralPath $Source -Destination $staged
        if (Test-Path -LiteralPath $dest) {
            # PowerShell can bind $null to an empty string for this .NET overload.
            # Use a real backup path and remove it only after replacement succeeds.
            $backup = Join-Path $InstallDir (".kaveon-" + [Guid]::NewGuid().ToString("N") + ".bak")
            [IO.File]::Replace($staged, $dest, $backup)
            Remove-Item -LiteralPath $backup -Force
        } else {
            [IO.File]::Move($staged, $dest)
        }
    } finally {
        if (Test-Path -LiteralPath $staged) {
            Remove-Item -LiteralPath $staged -Force
        }
    }
    return $dest
}

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$work = Join-Path ([IO.Path]::GetTempPath()) ("kaveon-install-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $work | Out-Null

try {
    if (-not $Version) {
        $binary = Join-Path $work "kaveon.exe"
        Get-ReleaseFile -Tag $previewTag -Asset $previewAsset -Destination $binary
        Test-CliVersion -Binary $binary
    } else {
        $tag = "cli-v$Version"
        $asset = "kaveon-$Version-$target.zip"
        $archive = Join-Path $work $asset
        $sums = Join-Path $work "SHA256SUMS"
        Get-ReleaseFile -Tag $tag -Asset $asset -Destination $archive
        Get-ReleaseFile -Tag $tag -Asset "SHA256SUMS" -Destination $sums

        $expected = $null
        foreach ($line in Get-Content -LiteralPath $sums) {
            if ($line -match '^([0-9A-Fa-f]{64})\s+\*?(.+?)\s*$' -and $Matches[2] -eq $asset) {
                $expected = $Matches[1].ToUpperInvariant()
            }
        }
        if (-not $expected) {
            throw "SHA256SUMS in $tag has no entry for $asset."
        }
        $actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToUpperInvariant()
        if ($actual -ne $expected) {
            throw "SHA-256 mismatch for ${asset}: expected $expected, actual $actual. The download was not installed."
        }
        Write-Host "  Verified SHA-256 $actual"

        $extracted = Join-Path $work "extract"
        Expand-Archive -LiteralPath $archive -DestinationPath $extracted
        $binary = Join-Path $extracted "kaveon.exe"
        if (-not (Test-Path -LiteralPath $binary)) {
            throw "$asset does not contain kaveon.exe."
        }
        Test-CliVersion -Binary $binary -Expected $Version
    }

    $installed = Install-Binary -Source $binary
} finally {
    if (Test-Path -LiteralPath $work) {
        Remove-Item -LiteralPath $work -Recurse -Force
    }
}

Write-Host "  Installed: $installed" -ForegroundColor Green

# Add to PATH
$userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($InstallDir -notin ($userPath -split ';')) {
    [Environment]::SetEnvironmentVariable("PATH", "$userPath;$InstallDir", "User")
    Write-Host "  Added to PATH: $InstallDir" -ForegroundColor Green
}
if ($InstallDir -notin ($env:PATH -split ';')) {
    $env:PATH = "$env:PATH;$InstallDir"
}

Write-Host ""
Write-Host "  Done. Verify the installation:" -ForegroundColor White
Write-Host "    kaveon --version" -ForegroundColor DarkGray
Write-Host "  Connection steps (including your server and public CA):" -ForegroundColor White
Write-Host "    https://github.com/$repo/blob/dev/docs/engineering/azure-deployment-guide.md" -ForegroundColor DarkGray
Write-Host "  Kaveon reuses your Azure login when the server enables Microsoft authentication." -ForegroundColor DarkGray
Write-Host ""
