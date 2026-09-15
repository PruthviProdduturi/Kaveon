<#
.SYNOPSIS
  The AKS topology as native processes on one machine, without Docker.

  coordinator :8080 · workers :8081-:8083 · API :8090 · Studio :3002
  PostgreSQL 17 on :5433 (kaveonmeta, kaveon) installed natively.

.DESCRIPTION
  Same environment contract as docker-compose.yml and the Helm charts, so what
  runs here is what runs on AKS minus TLS, workload identity and ADLS: the lake
  is a local directory. Development-only tokens; never reuse them anywhere.

  .\scripts\local-cluster.ps1 start   [-DataDir D:\Repos\...\tmp\kaveon-events]
  .\scripts\local-cluster.ps1 stop
  .\scripts\local-cluster.ps1 status
#>
param(
  [Parameter(Position = 0)][ValidateSet("start", "stop", "status")][string]$Action = "status",
  [string]$DataDir = (Join-Path $PSScriptRoot "..\tmp\kaveon-events"),
  [string]$StateDir = (Join-Path $PSScriptRoot "..\tmp\local-cluster"),
  [int]$Parallelism = 0   # KAVEON_LOCAL_PARALLELISM for every Engine node; 0 leaves the Engine's default
)
$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$engine = Join-Path $root "engine\target\release\kaveon-server.exe"
$DataDir = (Resolve-Path $DataDir).Path
New-Item -ItemType Directory -Force -Path $StateDir | Out-Null
$pidFile = Join-Path $StateDir "pids.json"

# Development-only credentials, identical to docker-compose.yml defaults.
$adminToken = "kaveon-local-admin-token-not-for-production"
$bridgeToken = "kaveon-local-bridge-token-not-for-production"
$catalogToken = "kaveon-local-catalog-admin"
$exchangeToken = "kaveon-local-exchange"
$securityJson = '{"principals":[{"token":"' + $adminToken + '","principal":"local-admin","role":"admin"}],"bridge_token":"' + $bridgeToken + '"}'

function Start-Node([string]$name, [string]$config, [int]$port, [bool]$coordinator) {
  $env:KAVEON_NODE_ID = $name
  $env:KAVEON_ENVIRONMENT = "local"
  $env:KAVEON_HTTP_PORT = "$port"
  $env:KAVEON_BIND_HOST = "127.0.0.1"
  $env:KAVEON_ADVERTISED_URI = "http://127.0.0.1:$port"
  $env:KAVEON_DATA_DIR = $DataDir
  $env:KAVEON_CATALOG_DATABASE_PATH = Join-Path $StateDir "$name-catalog.db"
  $env:KAVEON_EXCHANGE_SPOOL_ROOT = Join-Path $StateDir "$name-exchange"
  $env:KAVEON_INSECURE_DEVELOPMENT = "true"
  $env:KAVEON_SECURITY_JSON = $securityJson
  $env:KAVEON_EXCHANGE_TOKEN = $exchangeToken
  $env:KAVEON_CATALOG_ADMIN_TOKEN = $catalogToken
  $env:KAVEON_QUERY_MEMORY_LIMIT_BYTES = "1073741824"
  $env:KAVEON_MEMORY_ADMISSION_LIMIT_BYTES = "4294967296"
  if ($Parallelism -gt 0) { $env:KAVEON_LOCAL_PARALLELISM = "$Parallelism" } else { Remove-Item Env:KAVEON_LOCAL_PARALLELISM -ErrorAction SilentlyContinue }
  if ($coordinator) { Remove-Item Env:KAVEON_DISCOVERY_URI -ErrorAction SilentlyContinue } else { $env:KAVEON_DISCOVERY_URI = "http://127.0.0.1:8080" }
  New-Item -ItemType Directory -Force -Path $env:KAVEON_EXCHANGE_SPOOL_ROOT | Out-Null
  $log = Join-Path $StateDir "$name.log"
  $p = Start-Process -FilePath $engine -ArgumentList "`"$config`"" -WorkingDirectory $StateDir -RedirectStandardOutput $log -RedirectStandardError "$log.err" -PassThru -WindowStyle Hidden
  return $p.Id
}

function Wait-Health([string]$url, [int]$seconds = 30) {
  for ($i = 0; $i -lt $seconds; $i++) {
    try { if ((Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 2).StatusCode -eq 200) { return $true } } catch {}
    Start-Sleep 1
  }
  return $false
}

switch ($Action) {
  "start" {
    if (-not (Test-Path $engine)) { throw "Build the Engine first: cargo build --release -p kaveon-server (engine/)" }
    & "C:\Program Files\PostgreSQL\17\bin\pg_isready.exe" -p 5433 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "PostgreSQL 17 is not accepting connections on :5433" }
    $pids = @{}
    $pids["coordinator"] = Start-Node "coordinator-1" (Join-Path $root "infra\local\engine\coordinator.toml") 8080 $true
    if (-not (Wait-Health "http://127.0.0.1:8080/health")) { throw "coordinator did not become healthy; see $StateDir\coordinator-1.log.err" }
    foreach ($i in 1..3) {
      $pids["worker-$i"] = Start-Node "worker-$i" (Join-Path $root "infra\local\engine\worker-$i.toml") (8080 + $i) $false
    }
    foreach ($i in 1..3) { if (-not (Wait-Health "http://127.0.0.1:$(8080 + $i)/health")) { throw "worker-$i did not become healthy" } }

    # API — the Compose contract, pointed at the native PostgreSQL and the local coordinator.
    $env:METADATA_DB_TYPE = "postgresql"; $env:METADATA_HOST = "127.0.0.1"; $env:METADATA_PORT = "5433"
    $env:METADATA_DATABASE = "kaveonmeta"; $env:METADATA_USER = "kaveon"; $env:METADATA_PASSWORD = "kaveon-local-only"; $env:METADATA_SSLMODE = "disable"
    $env:KAVEON_PROXY_SECRET = "kaveon-local-proxy"
    $env:KAVEON_ENGINE_URL = "http://127.0.0.1:8080"
    $env:KAVEON_ENGINE_BRIDGE_TOKEN = $bridgeToken
    $env:KAVEON_ENGINE_CATALOG_TOKEN = $catalogToken
    $env:KAVEON_ENGINE_ADMIN_TOKEN = $adminToken
    $env:KAVEON_INSECURE_DEVELOPMENT = "true"
    $env:KAVEON_DEV_USER_EMAIL = "developer@localhost"
    $env:KAVEON_DEV_USER_ROLE = "Admin"
    $env:KAVEON_CREDENTIAL_ACTIVE_KEY = "local"
    $env:KAVEON_CREDENTIAL_KEYS = '{"local":"' + (python -c "from cryptography.fernet import Fernet; print(Fernet.generate_key().decode())") + '"}'
    $apiLog = Join-Path $StateDir "api.log"
    $api = Start-Process -FilePath (Join-Path $root "api\venv\Scripts\python.exe") -ArgumentList "-m uvicorn main:app --host 127.0.0.1 --port 8090" -WorkingDirectory (Join-Path $root "api") -RedirectStandardOutput $apiLog -RedirectStandardError "$apiLog.err" -PassThru -WindowStyle Hidden
    $pids["api"] = $api.Id
    if (-not (Wait-Health "http://127.0.0.1:8090/api/health" 60)) { Write-Warning "API not healthy yet; see $apiLog.err" }

    $pids | ConvertTo-Json | Set-Content $pidFile
    Write-Host "coordinator :8080 · workers :8081-:8083 · API :8090 · PostgreSQL :5433 · lake $DataDir"
    Write-Host "Studio: cd studio; `$env:API_URL='http://127.0.0.1:8090'; `$env:KAVEON_PROXY_SECRET='kaveon-local-proxy'; pnpm dev -- -p 3002"
  }
  "stop" {
    if (Test-Path $pidFile) {
      $pids = Get-Content $pidFile | ConvertFrom-Json
      foreach ($p in $pids.PSObject.Properties) { Stop-Process -Id $p.Value -Force -ErrorAction SilentlyContinue }
      Remove-Item $pidFile
    }
    Write-Host "stopped"
  }
  "status" {
    foreach ($port in 8080, 8081, 8082, 8083) {
      $ok = Wait-Health "http://127.0.0.1:$port/health" 1
      Write-Host ("{0,-12} {1}" -f ":$port", $(if ($ok) { "healthy" } else { "down" }))
    }
    $ok = Wait-Health "http://127.0.0.1:8090/api/health" 1
    Write-Host ("{0,-12} {1}" -f "api :8090", $(if ($ok) { "healthy" } else { "down" }))
  }
}
