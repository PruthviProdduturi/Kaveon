param(
    [switch]$Down,
    [switch]$Logs,
    [switch]$Status,
    [switch]$Build,
    [string]$DataPath = ""
)

$ErrorActionPreference = "Stop"
$compose = Join-Path $PSScriptRoot "..\docker-compose.kavedb.yml"
if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
    throw "Docker Desktop with Compose is required."
}

$args = @("compose", "-f", $compose)
if ($DataPath) { $env:KAVEON_DATA_PATH = (Resolve-Path $DataPath).Path }
if ($Down) { & docker @args down; exit $LASTEXITCODE }
if ($Logs) { & docker @args logs -f kaveon-db; exit $LASTEXITCODE }
if ($Status) { & docker @args ps; exit $LASTEXITCODE }

$up = @("up", "-d")
if ($Build) { $up += "--build" }
$command = $args + $up
& docker @command
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Host "KaveonDB is starting at http://localhost:$($env:KAVEON_DB_PORT ?? '8080')"
Write-Host "Health:  http://localhost:$($env:KAVEON_DB_PORT ?? '8080')/health"
Write-Host "CLI:     kaveon --server http://localhost:$($env:KAVEON_DB_PORT ?? '8080') --auth none"
