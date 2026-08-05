# Kaveon Rebrand — Directory Renames
# Run this AFTER closing VS Code, terminals, and any running dev servers.
# Usage: powershell -ExecutionPolicy Bypass -File rename-to-kaveon.ps1

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path

Write-Host "`n=== Kaveon Rebrand: Directory Renames ===" -ForegroundColor Cyan

# 1. Rename app directories
Write-Host "`n[1/4] Renaming apps/lens-web -> apps/kaveon-web..." -ForegroundColor Yellow
Set-Location "$root"
git mv apps/lens-web apps/kaveon-web
Write-Host "  Done." -ForegroundColor Green

Write-Host "[2/4] Renaming apps/lens-api -> apps/kaveon-api..." -ForegroundColor Yellow
git mv apps/lens-api apps/kaveon-api
Write-Host "  Done." -ForegroundColor Green

# 3. Rename the API proxy route directory
Write-Host "[3/4] Renaming apps/kaveon-web/app/api/lens -> apps/kaveon-web/app/api/kaveon..." -ForegroundColor Yellow
git mv apps/kaveon-web/app/api/lens apps/kaveon-web/app/api/kaveon
Write-Host "  Done." -ForegroundColor Green

# 4. Commit
Write-Host "[4/4] Committing..." -ForegroundColor Yellow
git add -A
git commit -m "chore: rename directories lens-web -> kaveon-web, lens-api -> kaveon-api"
Write-Host "  Done." -ForegroundColor Green

Write-Host "`n=== Rebrand complete! ===" -ForegroundColor Cyan
Write-Host "Next: rename this repo folder from 'Lens' to 'Kaveon':"
Write-Host "  cd .. && ren Lens Kaveon" -ForegroundColor Yellow
Write-Host "Then rename the GitHub repo:"
Write-Host "  gh repo rename Kaveon --repo PruthviProdduturi/Lens" -ForegroundColor Yellow
Write-Host ""
