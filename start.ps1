# LOOMX Complete Startup Script
# PowerShell version - handles EVERYTHING

Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  LOOMX Complete Setup and Startup" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "This script handles everything needed to run LOOMX."
Write-Host ""

# ============================================
# Step 1: Check Prerequisites
# ============================================
Write-Host "[1/12] Checking prerequisites..." -ForegroundColor Yellow

# Check Node.js
try {
    $nodeVersion = node -v 2>$null
    if ($LASTEXITCODE -eq 0) {
        Write-Host "  Node.js: OK ($nodeVersion)" -ForegroundColor Green
    } else {
        throw "Node.js not found"
    }
} catch {
    Write-Host "[ERROR] Node.js is not installed!" -ForegroundColor Red
    Write-Host "Please install Node.js 20+ from: https://nodejs.org/"
    Read-Host "Press Enter to exit"
    exit 1
}

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
    Write-Host "Please install Python 3.12+ from: https://www.python.org/"
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
# Step 2: Setup Root Environment Configuration
# ============================================
Write-Host ""
Write-Host "[2/12] Checking root .env configuration..." -ForegroundColor Yellow

$rootEnvPath = ".env"

if (-not (Test-Path $rootEnvPath)) {
    Write-Host "[ERROR] Root .env file not found!" -ForegroundColor Red
    Write-Host ""
    Write-Host "Please create .env file from template:" -ForegroundColor Yellow
    Write-Host "  1. Copy .env.example to .env" -ForegroundColor Cyan
    Write-Host "  2. Edit .env and fill in your configuration values" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "Example:" -ForegroundColor Gray
    Write-Host "  cp .env.example .env" -ForegroundColor Gray
    Write-Host ""
    Read-Host "Press Enter to exit"
    exit 1
}

Write-Host "  Root .env: OK" -ForegroundColor Green
Write-Host "  All services will read from this single .env file" -ForegroundColor Gray

# ============================================
# Step 3: Verify Environment Configuration (Optional)
# ============================================
Write-Host ""
Write-Host "[3/12] Verifying configuration..." -ForegroundColor Yellow

$envContent = Get-Content $rootEnvPath -Raw

# Check for required variables
$requiredVars = @(
    "AZURE_TENANT_ID",
    "AZURE_CLIENT_ID",
    "FABRIC_METADATA_ENDPOINT",
    "FABRIC_METADATA_DATABASE"
)

Write-Host ""
Write-Host "  NOTE: Data warehouse endpoints are retrieved from the data_sources" -ForegroundColor Gray
Write-Host "        table in the metadata database, not from .env" -ForegroundColor Gray

$missingVars = @()
foreach ($var in $requiredVars) {
    if ($envContent -notmatch "$var=") {
        $missingVars += $var
    }
}

if ($missingVars.Count -gt 0) {
    Write-Host "  [WARNING] Missing required variables in .env:" -ForegroundColor Yellow
    foreach ($var in $missingVars) {
        Write-Host "    - $var" -ForegroundColor Red
    }
    Write-Host "  Please edit .env and add these variables" -ForegroundColor Yellow
    Write-Host ""
} else {
    Write-Host "  All required variables present" -ForegroundColor Green
}

# ============================================
# Step 4: Install Python Packages for Proxy
# ============================================
Write-Host ""
Write-Host "[4/12] Checking Python packages for proxy..." -ForegroundColor Yellow

Push-Location "apps\loomx-python-proxy"

# Check if venv exists
if (-not (Test-Path "venv")) {
    Write-Host "  Creating Python virtual environment..." -ForegroundColor Cyan
    python -m venv venv
    if ($?) {
        Write-Host "  Virtual environment created." -ForegroundColor Green
    } else {
        Write-Host "  [WARNING] Failed to create venv, using global Python." -ForegroundColor Yellow
    }
}

# Activate venv and install packages
if (Test-Path "venv\Scripts\Activate.ps1") {
    Write-Host "  Installing/verifying Python packages in venv..." -ForegroundColor Cyan
    & venv\Scripts\Activate.ps1
    pip install --quiet pyodbc flask flask-cors azure-identity python-dotenv > $null 2>&1
    if ($?) {
        Write-Host "  Python packages: OK" -ForegroundColor Green
    } else {
        Write-Host "  [WARNING] Some packages may have failed to install." -ForegroundColor Yellow
    }
    deactivate
} else {
    Write-Host "  Installing Python packages globally..." -ForegroundColor Cyan
    pip install --quiet pyodbc flask flask-cors azure-identity python-dotenv > $null 2>&1
    if ($?) {
        Write-Host "  Python packages: OK" -ForegroundColor Green
    } else {
        Write-Host "  [WARNING] Some packages may have failed to install." -ForegroundColor Yellow
    }
}

Pop-Location

# ============================================
# Step 5: Install Node Dependencies
# ============================================
Write-Host ""
Write-Host "[5/12] Installing Node dependencies..." -ForegroundColor Yellow
Write-Host "  This may take 2-3 minutes on first run..." -ForegroundColor Cyan

Write-Host "  Installing root workspace..." -ForegroundColor Cyan
pnpm install --silent 2>&1 | Out-Null
if ($?) {
    Write-Host "  Root dependencies: OK" -ForegroundColor Green
} else {
    Write-Host "[ERROR] Failed to install root dependencies!" -ForegroundColor Red
    Write-Host "Run 'pnpm install' manually to see the error" -ForegroundColor Yellow
    Read-Host "Press Enter to exit"
    exit 1
}

Write-Host "  Installing API dependencies..." -ForegroundColor Cyan
pnpm --filter loomx-api install --silent 2>&1 | Out-Null
if ($?) {
    Write-Host "  API dependencies: OK" -ForegroundColor Green
} else {
    Write-Host "[ERROR] Failed to install API dependencies!" -ForegroundColor Red
    Write-Host "Run 'pnpm --filter loomx-api install' manually to see the error" -ForegroundColor Yellow
    Read-Host "Press Enter to exit"
    exit 1
}

Write-Host "  Installing Web dependencies..." -ForegroundColor Cyan
pnpm --filter loomx-web install --silent 2>&1 | Out-Null
if ($?) {
    Write-Host "  Web dependencies: OK" -ForegroundColor Green
} else {
    Write-Host "[ERROR] Failed to install Web dependencies!" -ForegroundColor Red
    Write-Host "Run 'pnpm --filter loomx-web install' manually to see the error" -ForegroundColor Yellow
    Read-Host "Press Enter to exit"
    exit 1
}

# ============================================
# Step 6: Verify Azure Packages
# ============================================
Write-Host ""
Write-Host "[6/12] Verifying Azure packages..." -ForegroundColor Yellow

# API Azure packages
Push-Location "apps\loomx-api"
if (-not (Test-Path "node_modules\@azure\identity")) {
    Write-Host "  Installing API Azure packages..." -ForegroundColor Cyan
    pnpm install @azure/identity @azure/msal-node --silent 2>&1 | Out-Null
}
if (-not (Test-Path "node_modules\@azure\msal-node")) {
    Write-Host "  Installing @azure/msal-node..." -ForegroundColor Cyan
    pnpm install @azure/msal-node --silent 2>&1 | Out-Null
}
Pop-Location
Write-Host "  API Azure packages: OK" -ForegroundColor Green

# Web Azure packages
Push-Location "apps\loomx-web"
if (-not (Test-Path "node_modules\@azure\msal-browser")) {
    Write-Host "  Installing Web Azure packages..." -ForegroundColor Cyan
    pnpm install @azure/msal-browser --silent 2>&1 | Out-Null
}
Pop-Location
Write-Host "  Web Azure packages: OK" -ForegroundColor Green

# ============================================
# Step 7: Verify TypeScript Configuration
# ============================================
Write-Host ""
Write-Host "[7/12] Verifying TypeScript configuration..." -ForegroundColor Yellow

if (Test-Path "apps\loomx-api\tsconfig.json") {
    Write-Host "  API TypeScript config: OK" -ForegroundColor Green
} else {
    Write-Host "  [WARNING] API tsconfig.json not found!" -ForegroundColor Yellow
}

if (Test-Path "apps\loomx-web\tsconfig.json") {
    Write-Host "  Web TypeScript config: OK" -ForegroundColor Green
} else {
    Write-Host "  [WARNING] Web tsconfig.json not found!" -ForegroundColor Yellow
}

# ============================================
# Step 8: Kill Existing Processes
# ============================================
Write-Host ""
Write-Host "[8/12] Checking for existing processes..." -ForegroundColor Yellow

# Kill Python proxy on port 5001
$proxyProcess = Get-NetTCPConnection -LocalPort 5001 -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -Unique
if ($proxyProcess) {
    Write-Host "  Killing Python proxy on port 5001 (PID: $proxyProcess)..." -ForegroundColor Cyan
    Stop-Process -Id $proxyProcess -Force -ErrorAction SilentlyContinue
}

# Kill process on port 8080 (API)
$apiProcess = Get-NetTCPConnection -LocalPort 8080 -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -Unique
if ($apiProcess) {
    Write-Host "  Killing process on port 8080 (PID: $apiProcess)..." -ForegroundColor Cyan
    Stop-Process -Id $apiProcess -Force -ErrorAction SilentlyContinue
}

# Kill process on port 3000 (Web)
$webProcess = Get-NetTCPConnection -LocalPort 3000 -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -Unique
if ($webProcess) {
    Write-Host "  Killing process on port 3000 (PID: $webProcess)..." -ForegroundColor Cyan
    Stop-Process -Id $webProcess -Force -ErrorAction SilentlyContinue
}

Write-Host "  Ports cleared." -ForegroundColor Green
Start-Sleep -Seconds 2

# ============================================
# Step 9: Start Python Proxy
# ============================================
Write-Host ""
Write-Host "[9/12] Starting Python Proxy..." -ForegroundColor Yellow

# Start proxy in new window (activating venv first)
Start-Process cmd -ArgumentList "/k", "cd /d `"$PWD\apps\loomx-python-proxy`" && venv\Scripts\activate && python proxy.py" -WindowStyle Normal
Write-Host "  Python Proxy starting in new window..." -ForegroundColor Green
Write-Host "  Watch for: 'Metadata Endpoint' and 'Datawarehouse Endpoint' in proxy window" -ForegroundColor Cyan

# Wait for proxy to initialize
Write-Host "  Waiting for proxy to initialize..." -ForegroundColor Gray
Start-Sleep -Seconds 5

# Test proxy connection
try {
    $proxyHealth = Invoke-RestMethod -Uri "http://localhost:5001/health" -Method Get -TimeoutSec 3 -ErrorAction Stop
    if ($proxyHealth.status -eq "ok") {
        Write-Host "  Python Proxy: READY" -ForegroundColor Green
    } else {
        Write-Host "  [WARNING] Python Proxy health check returned unexpected status" -ForegroundColor Yellow
    }
} catch {
    Write-Host "  [WARNING] Could not verify Python Proxy health (may still be starting)" -ForegroundColor Yellow
}

# ============================================
# Step 10: Start API Server
# ============================================
Write-Host ""
Write-Host "[10/12] Starting LOOMX API..." -ForegroundColor Yellow
Start-Process cmd -ArgumentList "/k", "cd /d `"$PWD\apps\loomx-api`" && pnpm dev" -WindowStyle Normal
Write-Host "  API starting in new window..." -ForegroundColor Green
Write-Host "  Watch for: 'Server listening on port 8080'" -ForegroundColor Cyan

# Wait for API to initialize
Write-Host "  Waiting for API to initialize..." -ForegroundColor Gray
Start-Sleep -Seconds 5

# ============================================
# Step 11: Start Web Server
# ============================================
Write-Host ""
Write-Host "[11/12] Starting LOOMX Web..." -ForegroundColor Yellow
Start-Process cmd -ArgumentList "/k", "cd /d `"$PWD\apps\loomx-web`" && pnpm dev" -WindowStyle Normal
Write-Host "  Web starting in new window..." -ForegroundColor Green
Write-Host "  Watch for: 'Ready on http://localhost:3000'" -ForegroundColor Cyan

# Wait a moment
Start-Sleep -Seconds 3

# ============================================
# Step 12: Final Verification
# ============================================
Write-Host ""
Write-Host "[12/12] Performing final checks..." -ForegroundColor Yellow

# Check if processes are listening
Start-Sleep -Seconds 2

$proxyListening = Get-NetTCPConnection -LocalPort 5001 -State Listen -ErrorAction SilentlyContinue
$apiListening = Get-NetTCPConnection -LocalPort 8080 -State Listen -ErrorAction SilentlyContinue
$webListening = Get-NetTCPConnection -LocalPort 3000 -State Listen -ErrorAction SilentlyContinue

if ($proxyListening) {
    Write-Host "  Python Proxy (5001): Listening" -ForegroundColor Green
} else {
    Write-Host "  Python Proxy (5001): Not ready yet (check proxy window)" -ForegroundColor Yellow
}

if ($apiListening) {
    Write-Host "  API Server (8080): Listening" -ForegroundColor Green
} else {
    Write-Host "  API Server (8080): Not ready yet (check API window)" -ForegroundColor Yellow
}

if ($webListening) {
    Write-Host "  Web Server (3000): Listening" -ForegroundColor Green
} else {
    Write-Host "  Web Server (3000): Not ready yet (check Web window)" -ForegroundColor Yellow
}

# ============================================
# Success Message
# ============================================
Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host "  LOOMX is Starting!" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
Write-Host ""
Write-Host "  Three terminal windows have opened:" -ForegroundColor Cyan
Write-Host ""
Write-Host "  1. Python Proxy: http://localhost:5001" -ForegroundColor White
Write-Host "     - Handles Fabric SQL connections" -ForegroundColor Gray
Write-Host "     - Routes to metadata + datawarehouse endpoints" -ForegroundColor Gray
Write-Host ""
Write-Host "  2. LOOMX API: http://localhost:8080" -ForegroundColor White
Write-Host "     - Backend API server" -ForegroundColor Gray
Write-Host ""
Write-Host "  3. LOOMX Web: http://localhost:3000" -ForegroundColor White
Write-Host "     - Frontend application" -ForegroundColor Gray
Write-Host ""
Write-Host "  First startup takes 1-2 minutes for" -ForegroundColor Yellow
Write-Host "  TypeScript compilation to complete."
Write-Host ""
Write-Host "  Watch the terminal windows for:" -ForegroundColor Cyan
Write-Host "  - Proxy: 'Metadata Endpoint' + 'Datawarehouse Endpoint'" -ForegroundColor Gray
Write-Host "  - API: 'Server listening on port 8080'" -ForegroundColor Gray
Write-Host "  - Web: 'Ready on http://localhost:3000'" -ForegroundColor Gray
Write-Host ""
Write-Host "  Once all services are ready, open:" -ForegroundColor Cyan
Write-Host ""
Write-Host "  http://localhost:3000" -ForegroundColor White -BackgroundColor DarkBlue
Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host "  Setup Complete!" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
Write-Host ""
Write-Host "  Troubleshooting:" -ForegroundColor Yellow
Write-Host "  - If metadata not loading: Check Python Proxy window" -ForegroundColor Gray
Write-Host "  - Should show 'Metadata Endpoint: ...msit-database...'" -ForegroundColor Gray
Write-Host "  - If errors: Close all windows and run start.ps1 again" -ForegroundColor Gray
Write-Host ""
Write-Host "Press any key to close this window..."
$null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
