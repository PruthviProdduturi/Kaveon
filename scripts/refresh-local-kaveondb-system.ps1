<#
Refresh the local read-only KaveonDB.system projection from the durable product
authority and republish the Engine catalog.  The projection is intentionally
materialized outside the API container: the API writes the transaction log,
while this bounded job owns Parquet generation and coordinator publication.
#>

param(
    [string]$Engine = 'http://localhost:8081',
    [string]$Token = 'kaveon-local-admin-token-not-for-production'
)

$ErrorActionPreference = 'Stop'

python scripts/materialize-kaveondb-system.py --engine $Engine --token $Token
if ($LASTEXITCODE -ne 0) { throw "KaveonDB.system materialization failed ($LASTEXITCODE)" }

docker compose up -d --force-recreate engine-coordinator | Out-Host
if ($LASTEXITCODE -ne 0) { throw "Engine coordinator restart failed ($LASTEXITCODE)" }

$deadline = (Get-Date).AddMinutes(2)
do {
    try {
        $health = Invoke-RestMethod -Uri "$Engine/health" -TimeoutSec 5
        if ($health.status -eq 'ok') { break }
    } catch { }
    Start-Sleep -Seconds 2
} while ((Get-Date) -lt $deadline)

if (-not $health -or $health.status -ne 'ok') {
    throw 'Engine coordinator did not become healthy after projection refresh'
}

python scripts/verify-local-product-mirror.py
if ($LASTEXITCODE -ne 0) { throw "Product mirror verification failed ($LASTEXITCODE)" }

Write-Output 'PASS: KaveonDB.system refreshed and coordinator catalog republished.'
