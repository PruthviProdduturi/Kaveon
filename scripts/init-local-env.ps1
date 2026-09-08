# Creates ignored local-development credentials once. Never rotates existing keys.
$ErrorActionPreference = 'Stop'
$repoPath = Split-Path -Parent $PSScriptRoot
$envPath = Join-Path $repoPath '.env.local'
if (Test-Path -LiteralPath $envPath) {
    Write-Host 'Existing .env.local retained. Use docker compose --env-file .env.local ...'
    return
}
function New-LocalToken([bool]$Padding = $false) {
    $tokenBytes = New-Object byte[] 32
    $generator = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try { $generator.GetBytes($tokenBytes) } finally { $generator.Dispose() }
    $encoded = [Convert]::ToBase64String($tokenBytes).Replace('+', '-').Replace('/', '_')
    if ($Padding) { return $encoded }
    return $encoded.TrimEnd('=')
}
$localValues = [ordered]@{
    KAVEON_EXCHANGE_TOKEN = New-LocalToken
    KAVEON_CATALOG_ADMIN_TOKEN = New-LocalToken
    KAVEON_ENGINE_ADMIN_TOKEN = New-LocalToken
    KAVEON_ENGINE_BRIDGE_TOKEN = New-LocalToken
    KAVEON_CREDENTIAL_ACTIVE_KEY = 'local-v1'
    KAVEON_CREDENTIAL_KEYS = (@{ 'local-v1' = (New-LocalToken $true) } | ConvertTo-Json -Compress)
}
$lines = @('# Local development credentials. Keep this ignored file to decrypt stored connections.')
foreach ($entry in $localValues.GetEnumerator()) { $lines += "$($entry.Key)='$($entry.Value)'" }
$file = [System.IO.File]::Open($envPath, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
try {
    $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes(($lines -join "`n") + "`n")
    $file.Write($bytes, 0, $bytes.Length)
} finally { $file.Dispose() }
Write-Host 'Created ignored .env.local. Use docker compose --env-file .env.local up -d --build'
