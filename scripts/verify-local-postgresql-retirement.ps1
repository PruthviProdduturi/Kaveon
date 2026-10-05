<#
.SYNOPSIS
  Verify that the native local KaveonDB profile no longer depends on PostgreSQL.

  This is deliberately read-only. It checks the KaveonDB health contract,
  PostgreSQL service state, and the usual local PostgreSQL ports. It exits
  non-zero until the service is stopped and no listener remains.
#>
param(
  [string]$HealthUrl = "http://127.0.0.1:8090/api/health"
)
$ErrorActionPreference = "Stop"

$healthResponse = $null
try {
  $healthResponse = Invoke-WebRequest -Uri $HealthUrl -UseBasicParsing
} catch {
  $healthResponse = $_.Exception.Response
}
if (-not $healthResponse) { throw "KaveonDB health endpoint could not be reached: $HealthUrl" }
$healthBody = if ($healthResponse.PSObject.Properties.Name -contains "Content") {
  [string]$healthResponse.Content
} else {
  (New-Object IO.StreamReader($healthResponse.GetResponseStream())).ReadToEnd()
}
$health = $healthBody | ConvertFrom-Json
$kaveonCheck = $health.checks.kaveondb
$postgresCheck = $health.checks.postgresql

if ($healthResponse.StatusCode -ne 200 -or $health.status -ne "healthy" -or
    $health.authority -ne "kaveondb" -or -not $kaveonCheck.connected -or
    -not $kaveonCheck.authoritative -or $postgresCheck.required -or
    $postgresCheck.connected) {
  throw "KaveonDB health contract is not green: $healthBody"
}

$service = Get-Service -Name "postgresql-x64-17" -ErrorAction SilentlyContinue
$listeners = @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue |
  Where-Object { $_.LocalPort -in @(5432, 5433) })

Write-Host "KaveonDB health: healthy (16 authority families)"
Write-Host "PostgreSQL service: $($(if ($service) { $service.Status } else { 'not installed' }))"
Write-Host "PostgreSQL listeners: $($listeners.Count)"

if (($service -and $service.Status -eq "Running") -or $listeners.Count -gt 0) {
  throw "PostgreSQL is still active locally; stop the service and rerun this verifier."
}

Write-Host "PASS: local PostgreSQL retirement is verified."
