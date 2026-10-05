<#
.SYNOPSIS
  The AKS topology as native processes on one machine, without Docker.

  coordinator :8080 · workers :8081-:8083 · API :8090 · Studio :3002
  KaveonDB product authority on local durable files; no PostgreSQL required.

.DESCRIPTION
  Same product-authority contract as docker-compose.kavedb.yml and the
  PostgreSQL-free Helm profile, so what runs here is what runs after retirement
  minus TLS, workload identity and ADLS: the lake and product log are local
  directories. Development-only tokens; never reuse them anywhere.

  .\scripts\local-cluster.ps1 start   [-DataDir D:\Repos\...\tmp\kaveon-events]
  .\scripts\local-cluster.ps1 stop
  .\scripts\local-cluster.ps1 status
#>
param(
  [Parameter(Position = 0)][ValidateSet("start", "stop", "status")][string]$Action = "status",
  [string]$DataDir = "",
  [string]$StateDir = "",
  [int]$Parallelism = 0   # KAVEON_LOCAL_PARALLELISM for every Engine node; 0 leaves the Engine's default
)
$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$defaultDataDir = Join-Path $root "tmp\kaveon-events"
$defaultStateDir = Join-Path $root "tmp\local-cluster"
if ([string]::IsNullOrWhiteSpace($DataDir)) { $DataDir = $defaultDataDir }
if ([string]::IsNullOrWhiteSpace($StateDir)) { $StateDir = $defaultStateDir }
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
  # The same per-query and admission budgets as the AKS worker pods.
  $env:KAVEON_QUERY_MEMORY_LIMIT_BYTES = "3221225472"
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
    $pids = @{}
    $pids["coordinator"] = Start-Node "coordinator-1" (Join-Path $root "infra\local\engine\coordinator.toml") 8080 $true
    if (-not (Wait-Health "http://127.0.0.1:8080/health")) { throw "coordinator did not become healthy; see $StateDir\coordinator-1.log.err" }
    foreach ($i in 1..3) {
      $pids["worker-$i"] = Start-Node "worker-$i" (Join-Path $root "infra\local\engine\worker-$i.toml") (8080 + $i) $false
    }
    foreach ($i in 1..3) { if (-not (Wait-Health "http://127.0.0.1:$(8080 + $i)/health")) { throw "worker-$i did not become healthy" } }

    # API — the PostgreSQL-free product contract, pointed at KaveonDB and the
    # local coordinator.  PostgreSQL is intentionally neither started nor
    # configured in this profile.
    $env:KAVEON_LOCAL_PRODUCT_MODE = "true"
    $env:KAVEON_POSTGRESQL_RETIREMENT_MODE = "true"
    $env:KAVEONDB_AUTHORITY_FAMILIES = "ai_configuration,catalog_sources,data_sources,datasets,dataset_semantics,charts,dashboards,favorites,saved_queries,user_themes,user_recents,query_history,activity,context_cache,dlm_generation,chat_history"
    $env:KAVEONDB_READ_AUTHORITY_FAMILIES = "all"
    $env:KAVEON_PRODUCT_STORAGE_MODE = "local"
    $env:KAVEON_PRODUCT_LOCAL_PATH = Join-Path $StateDir "product-transactions"
    New-Item -ItemType Directory -Force -Path $env:KAVEON_PRODUCT_LOCAL_PATH | Out-Null
    $env:KAVEON_LOCAL_DLM_ARTIFACT_PATH = $env:KAVEON_PRODUCT_LOCAL_PATH
    $env:KAVEON_DLM_LIVE_ARTIFACT_PUBLISH_ENABLED = "true"
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
    Write-Host "coordinator :8080 · workers :8081-:8083 · API :8090 · KaveonDB product log $($env:KAVEON_PRODUCT_LOCAL_PATH) · lake $DataDir"
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
