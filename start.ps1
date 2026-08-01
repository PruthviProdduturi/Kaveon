# Lens Startup Script
# Starts the Python/FastAPI API and the Next.js web app
#
# Run from anywhere — script locates itself via $PSScriptRoot.

Set-Location $PSScriptRoot

Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  Lens Startup" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""

# ============================================
# Step 1: Check Prerequisites
# ============================================
Write-Host "[1/6] Checking prerequisites..." -ForegroundColor Yellow

# Check Python
$pythonVersion = python --version 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Host "[ERROR] Python is not installed!" -ForegroundColor Red
    Write-Host "Please install Python 3.11+ from: https://www.python.org/"
    Read-Host "Press Enter to exit"
    exit 1
}
Write-Host "  Python: OK ($pythonVersion)" -ForegroundColor Green

# Check pnpm
$pnpmVersion = pnpm -v 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Host "  pnpm not found. Installing via npm..." -ForegroundColor Yellow
    npm install -g pnpm
    $pnpmVersion = pnpm -v 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[ERROR] Failed to install pnpm!" -ForegroundColor Red
        Write-Host "Please run: npm install -g pnpm"
        Read-Host "Press Enter to exit"
        exit 1
    }
}
Write-Host "  pnpm: OK ($pnpmVersion)" -ForegroundColor Green

# ============================================
# Step 2: Check Root .env
# ============================================
Write-Host ""
Write-Host "[2/6] Checking root .env configuration..." -ForegroundColor Yellow

if (-not (Test-Path ".env")) {
    Write-Host "[ERROR] Root .env file not found!" -ForegroundColor Red
    Write-Host ""
    Write-Host "  1. Copy .env.example to .env" -ForegroundColor Cyan
    Write-Host "  2. Edit .env and fill in your values" -ForegroundColor Cyan
    Write-Host ""
    Read-Host "Press Enter to exit"
    exit 1
}

Write-Host "  Root .env: OK" -ForegroundColor Green
Write-Host "  Auth + database configured via UI on first launch." -ForegroundColor DarkGray

# ============================================
# Step 3: Set Up Python API venv
# ============================================
Write-Host ""
Write-Host "[3/6] Setting up Python API..." -ForegroundColor Yellow

Push-Location "apps\lens-api"

if (-not (Test-Path "venv")) {
    Write-Host "  Creating Python virtual environment..." -ForegroundColor Cyan
    python -m venv venv
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[ERROR] Failed to create venv!" -ForegroundColor Red
        Pop-Location
        Read-Host "Press Enter to exit"
        exit 1
    }
    Write-Host "  Virtual environment created." -ForegroundColor Green
}

Write-Host "  Installing Python dependencies..." -ForegroundColor Cyan
& venv\Scripts\python.exe -m pip install -q -r requirements.txt
if ($LASTEXITCODE -ne 0) {
    Write-Host "  [WARNING] Some Python packages may have failed to install." -ForegroundColor Yellow
} else {
    Write-Host "  Python dependencies: OK" -ForegroundColor Green
}

Pop-Location

# ============================================
# Step 4: Install Node Dependencies
# ============================================
Write-Host ""
Write-Host "[4/6] Installing Node dependencies..." -ForegroundColor Yellow

pnpm install -r
if ($LASTEXITCODE -ne 0) {
    Write-Host "[ERROR] Failed to install Node dependencies!" -ForegroundColor Red
    Write-Host "Run 'pnpm install -r' manually to see the full error."
    Read-Host "Press Enter to exit"
    exit 1
}
Write-Host "  Node dependencies: OK" -ForegroundColor Green

# ============================================
# Step 5: Kill Existing Processes on Ports
# ============================================
Write-Host ""
Write-Host "[5/6] Clearing ports..." -ForegroundColor Yellow

foreach ($port in @(8080, 3000)) {
    $procs = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue |
             Select-Object -ExpandProperty OwningProcess -Unique
    foreach ($procId in $procs) {
        Write-Host "  Killing process on port $port (PID: $procId)..." -ForegroundColor Cyan
        Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue
    }
}

Write-Host "  Ports cleared." -ForegroundColor Green
Start-Sleep -Seconds 1

# ============================================
# Step 6: Start Services
# ============================================
Write-Host ""
Write-Host "[6/6] Starting services..." -ForegroundColor Yellow

# Prefer Windows Terminal (wt) for a nicer experience; fall back to cmd.
$useWt = $null -ne (Get-Command wt -ErrorAction SilentlyContinue)

if ($useWt) {
    # Open both services in a single Windows Terminal with two tabs
    $apiCmd  = "cmd /k `"cd /d `"$PSScriptRoot\apps\lens-api`" && venv\Scripts\activate && python main.py`""
    $webCmd  = "cmd /k `"cd /d `"$PSScriptRoot`" && pnpm --filter lens-web dev`""
    Start-Process wt -ArgumentList "new-tab --title `"Lens API`" $apiCmd ; new-tab --title `"Lens Web`" $webCmd"
    Write-Host "  Services starting in Windows Terminal..." -ForegroundColor Green
} else {
    Start-Process cmd -ArgumentList "/k", "cd /d `"$PSScriptRoot\apps\lens-api`" && venv\Scripts\activate && python main.py" -WindowStyle Normal
    Write-Host "  API starting in new window..." -ForegroundColor Green
    Start-Sleep -Seconds 2
    Start-Process cmd -ArgumentList "/k", "cd /d `"$PSScriptRoot`" && pnpm --filter lens-web dev" -WindowStyle Normal
    Write-Host "  Web starting in new window..." -ForegroundColor Green
}

Start-Sleep -Seconds 3

# Quick port check
$apiListening = Get-NetTCPConnection -LocalPort 8080 -State Listen -ErrorAction SilentlyContinue
$webListening = Get-NetTCPConnection -LocalPort 3000 -State Listen -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host "  Lens is Starting!" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
Write-Host ""
Write-Host "  1. Lens API:  http://localhost:8080" -ForegroundColor White
Write-Host "     - Health: http://localhost:8080/api/health" -ForegroundColor Gray
Write-Host "     - Docs:   http://localhost:8080/docs" -ForegroundColor Gray
Write-Host ""
Write-Host "  2. Lens Web:  http://localhost:3000" -ForegroundColor White
Write-Host "     - Next.js 15 + Turbopack" -ForegroundColor Gray
Write-Host ""

if ($apiListening) {
    Write-Host "  API (8080): Listening" -ForegroundColor Green
} else {
    Write-Host "  API (8080): Starting... (check the API window)" -ForegroundColor Yellow
}
if ($webListening) {
    Write-Host "  Web (3000): Listening" -ForegroundColor Green
} else {
    Write-Host "  Web (3000): Starting... (check the Web window)" -ForegroundColor Yellow
}

Write-Host ""
Write-Host "  Once ready, open: http://localhost:3000" -ForegroundColor Cyan
Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host ""
Write-Host "Press any key to close this window..."
$null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
