# LoomX Startup Script
# Starts the Python/FastAPI API and the Next.js web app

Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  LoomX Startup" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""

# ============================================
# Step 1: Check Prerequisites
# ============================================
Write-Host "[1/6] Checking prerequisites..." -ForegroundColor Yellow

# Check Python
try {
    $pythonVersion = python --version 2>$null
    if ($LASTEXITCODE -eq 0) {
        Write-Host "  Python: OK ($pythonVersion)" -ForegroundColor Green
    } else {
        throw "Python not found"
    }
} catch {
    Write-Host "[ERROR] Python is not installed!" -ForegroundColor Red
    Write-Host "Please install Python 3.11+ from: https://www.python.org/"
    Read-Host "Press Enter to exit"
    exit 1
}

# Check pnpm
try {
    $pnpmVersion = pnpm -v 2>$null
    if ($LASTEXITCODE -eq 0) {
        Write-Host "  pnpm: OK ($pnpmVersion)" -ForegroundColor Green
    } else {
        throw "pnpm not found"
    }
} catch {
    Write-Host "  pnpm not found. Installing via corepack..." -ForegroundColor Yellow
    corepack enable
    corepack prepare pnpm@latest --activate
    $pnpmVersion = pnpm -v 2>$null
    if ($LASTEXITCODE -eq 0) {
        Write-Host "  pnpm: OK ($pnpmVersion)" -ForegroundColor Green
    } else {
        Write-Host "[ERROR] Failed to install pnpm!" -ForegroundColor Red
        Write-Host "Please run: npm install -g pnpm"
        Read-Host "Press Enter to exit"
        exit 1
    }
}

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

$envContent = Get-Content ".env" -Raw

$missingVars = @()
if ($envContent -notmatch "AZURE_CLIENT_ID=") { $missingVars += "AZURE_CLIENT_ID" }
if ($envContent -notmatch "AZURE_TENANT_ID=") { $missingVars += "AZURE_TENANT_ID" }

if ($missingVars.Count -gt 0) {
    Write-Host "  [WARNING] Missing variables in .env:" -ForegroundColor Yellow
    foreach ($var in $missingVars) { Write-Host "    - $var" -ForegroundColor Red }
} else {
    Write-Host "  All required variables present" -ForegroundColor Green
}

# ============================================
# Step 3: Set Up Python API venv
# ============================================
Write-Host ""
Write-Host "[3/6] Setting up Python API..." -ForegroundColor Yellow

Push-Location "apps\loomx-api"

if (-not (Test-Path "venv")) {
    Write-Host "  Creating Python virtual environment..." -ForegroundColor Cyan
    python -m venv venv
    if ($?) {
        Write-Host "  Virtual environment created." -ForegroundColor Green
    } else {
        Write-Host "[ERROR] Failed to create venv!" -ForegroundColor Red
        Pop-Location
        Read-Host "Press Enter to exit"
        exit 1
    }
}

Write-Host "  Installing Python dependencies (this may take a minute)..." -ForegroundColor Cyan
& venv\Scripts\python.exe -m pip install -r requirements.txt
if ($?) {
    Write-Host "  Python dependencies: OK" -ForegroundColor Green
} else {
    Write-Host "  [WARNING] Some packages may have failed to install." -ForegroundColor Yellow
}

Pop-Location

# ============================================
# Step 4: Install Node Dependencies
# ============================================
Write-Host ""
Write-Host "[4/6] Installing Node dependencies..." -ForegroundColor Yellow

pnpm install 2>&1
if ($?) {
    Write-Host "  Node dependencies: OK" -ForegroundColor Green
} else {
    Write-Host "[ERROR] Failed to install Node dependencies!" -ForegroundColor Red
    Write-Host "Run 'pnpm install' manually to see the error"
    Read-Host "Press Enter to exit"
    exit 1
}

# ============================================
# Step 5: Kill Existing Processes on Ports
# ============================================
Write-Host ""
Write-Host "[5/6] Clearing ports..." -ForegroundColor Yellow

foreach ($port in @(8080, 3000)) {
    $proc = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue |
            Select-Object -ExpandProperty OwningProcess -Unique
    if ($proc) {
        Write-Host "  Killing process on port $port (PID: $proc)..." -ForegroundColor Cyan
        Stop-Process -Id $proc -Force -ErrorAction SilentlyContinue
    }
}

Write-Host "  Ports cleared." -ForegroundColor Green
Start-Sleep -Seconds 1

# ============================================
# Step 6: Start Services
# ============================================
Write-Host ""
Write-Host "[6/6] Starting services..." -ForegroundColor Yellow

# Start Python API
Start-Process cmd -ArgumentList "/k", "cd /d `"$PWD\apps\loomx-api`" && venv\Scripts\activate && python main.py" -WindowStyle Normal
Write-Host "  API starting in new window..." -ForegroundColor Green

Start-Sleep -Seconds 3

# Start Next.js web
Start-Process cmd -ArgumentList "/k", "cd /d `"$PWD`" && pnpm --filter loomx-web dev" -WindowStyle Normal
Write-Host "  Web starting in new window..." -ForegroundColor Green

Start-Sleep -Seconds 3

# Quick port check
$apiListening = Get-NetTCPConnection -LocalPort 8080 -State Listen -ErrorAction SilentlyContinue
$webListening = Get-NetTCPConnection -LocalPort 3000 -State Listen -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host "  LoomX is Starting!" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
Write-Host ""
Write-Host "  Two terminal windows have opened:" -ForegroundColor Cyan
Write-Host ""
Write-Host "  1. LoomX API:  http://localhost:8080" -ForegroundColor White
Write-Host "     - FastAPI + pyodbc + Azure AD token auth" -ForegroundColor Gray
Write-Host "     - Health: http://localhost:8080/api/health" -ForegroundColor Gray
Write-Host "     - Docs:   http://localhost:8080/docs" -ForegroundColor Gray
Write-Host ""
Write-Host "  2. LoomX Web:  http://localhost:3000" -ForegroundColor White
Write-Host "     - Next.js 15 frontend" -ForegroundColor Gray
Write-Host ""

if ($apiListening) {
    Write-Host "  API (8080): Listening" -ForegroundColor Green
} else {
    Write-Host "  API (8080): Starting... (watch the API window)" -ForegroundColor Yellow
}

if ($webListening) {
    Write-Host "  Web (3000): Listening" -ForegroundColor Green
} else {
    Write-Host "  Web (3000): Starting... (watch the Web window)" -ForegroundColor Yellow
}

Write-Host ""
Write-Host "  Once ready, open: http://localhost:3000" -ForegroundColor Cyan
Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host ""
Write-Host "Press any key to close this window..."
$null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
