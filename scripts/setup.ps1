# Kaveon — local development setup (Windows)
# Usage: .\scripts\setup.ps1
#
# Sets up everything needed to run Kaveon locally:
#   1. Checks/installs prerequisites (Node.js, Python, Rust)
#   2. Installs dependencies for each pillar
#   3. Builds the kaveon CLI
#   4. Prints next steps

$ErrorActionPreference = "Stop"

function Write-Step($msg) { Write-Host "`n==> $msg" -ForegroundColor Cyan }
function Write-Ok($msg)   { Write-Host "    $msg" -ForegroundColor Green }
function Write-Warn($msg) { Write-Host "    $msg" -ForegroundColor Yellow }
function Write-Err($msg)  { Write-Host "    $msg" -ForegroundColor Red }

$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not (Test-Path "$root\engine\Cargo.toml")) {
    $root = Split-Path -Parent $PSScriptRoot
}
Set-Location $root

Write-Host ""
Write-Host "  Kaveon — Local Development Setup" -ForegroundColor White
Write-Host "  Talk to your data." -ForegroundColor DarkGray
Write-Host ""

# --- Prerequisites ---

Write-Step "Checking prerequisites"

# Node.js
$node = Get-Command node -ErrorAction SilentlyContinue
if ($node) {
    $nodeVer = (node --version)
    Write-Ok "Node.js $nodeVer"
} else {
    Write-Warn "Node.js not found — installing via winget"
    winget install OpenJS.NodeJS.LTS --accept-source-agreements --accept-package-agreements
}

# pnpm
$pnpm = Get-Command pnpm -ErrorAction SilentlyContinue
if ($pnpm) {
    Write-Ok "pnpm $(pnpm --version)"
} else {
    Write-Warn "pnpm not found — installing"
    npm install -g pnpm
}

# Python
$python = Get-Command python -ErrorAction SilentlyContinue
if ($python) {
    $pyVer = (python --version 2>&1)
    Write-Ok "$pyVer"
} else {
    Write-Warn "Python not found — installing via winget"
    winget install Python.Python.3.11 --accept-source-agreements --accept-package-agreements
}

# Rust
$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if ($cargo) {
    $rustVer = (rustc --version)
    Write-Ok "$rustVer"
} else {
    Write-Warn "Rust not found — installing via rustup"
    Write-Host "    Run this in a new terminal if it fails:" -ForegroundColor DarkGray
    Write-Host '    winget install Rustlang.Rust.MSVC' -ForegroundColor DarkGray
    Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile "$env:TEMP\rustup-init.exe"
    & "$env:TEMP\rustup-init.exe" -y --default-toolchain stable
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
}

# --- Studio ---

Write-Step "Setting up Studio (Next.js)"
if (Test-Path "studio\package.json") {
    Set-Location studio
    pnpm install --no-frozen-lockfile
    Write-Ok "Studio dependencies installed"
    Set-Location $root
} else {
    Write-Warn "studio/package.json not found — skipping"
}

# --- API ---

Write-Step "Setting up API (Python)"
if (Test-Path "api\requirements.txt") {
    Set-Location api
    if (-not (Test-Path ".venv")) {
        python -m venv .venv
    }
    & .venv\Scripts\Activate.ps1
    pip install -q -r requirements.txt
    Write-Ok "API dependencies installed"
    Set-Location $root
} else {
    Write-Warn "api/requirements.txt not found — skipping"
}

# --- Engine ---

Write-Step "Building Kaveon Engine CLI"
$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if ($cargo) {
    Set-Location engine
    cargo build --release --package kaveon-cli
    $bin = "target\release\kaveon.exe"
    if (Test-Path $bin) {
        Write-Ok "Built: engine\$bin"

        # Add to user PATH if not already there
        $engineBin = (Resolve-Path "target\release").Path
        $userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
        if ($userPath -notlike "*$engineBin*") {
            [Environment]::SetEnvironmentVariable("PATH", "$userPath;$engineBin", "User")
            $env:PATH = "$env:PATH;$engineBin"
            Write-Ok "Added to PATH: $engineBin"
        }
    }
    Set-Location $root
} else {
    Write-Err "cargo not found — restart terminal after Rust install, then re-run this script"
}

# --- Done ---

Write-Host ""
Write-Host "  Setup complete!" -ForegroundColor Green
Write-Host ""
Write-Host "  Quick start:" -ForegroundColor White
Write-Host "    kaveon --data-dir <path>     # SQL shell" -ForegroundColor DarkGray
Write-Host "    cd studio && pnpm dev        # Studio on localhost:3000" -ForegroundColor DarkGray
Write-Host "    cd api && python main.py     # API on localhost:8000" -ForegroundColor DarkGray
Write-Host ""
