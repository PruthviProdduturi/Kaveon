param([switch]$RunChecks, [string]$NpmRegistry = 'https://registry.npmjs.org')
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/dev-env.ps1"
$failures = [System.Collections.Generic.List[string]]::new()
function Check([string]$Name, [scriptblock]$Action) {
    Write-Host "Checking $Name"
    try {
        $global:LASTEXITCODE = 0
        & $Action
        if ($LASTEXITCODE -ne 0) { throw "exit code $LASTEXITCODE" }
    } catch {
        $failures.Add("${Name}: $_")
        Write-Warning "${Name}: $_"
    }
}
Push-Location $kaveonRoot
try {
    Check 'Node 22' { if ((node --version) -notmatch '^v22\.') { throw 'Node 22 required' }; node --version }
    Check 'pinned pnpm' {
        $expected = (Get-Content package.json -Raw | ConvertFrom-Json).packageManager.Split('@')[-1]
        $actual = pnpm --version
        if ($actual -ne $expected) { throw "Expected pnpm $expected; got $actual" }
        $actual
    }
    Check 'Rust' { rustc --version; cargo --version; rustup component list --installed }
    Check 'MSVC C++ compiler' {
        $vswhere = "${env:ProgramFiles(x86)}/Microsoft Visual Studio/Installer/vswhere.exe"
        if (-not (Test-Path $vswhere)) { throw 'Visual Studio Build Tools missing' }
        $installation = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if (-not $installation) { throw 'MSVC x64/x86 component missing' }
        $installation
    }
    Check 'API Python 3.11 and dependencies' {
        & $apiPython -c 'import sys, fastapi, pyodbc, psycopg2, pytest; assert sys.version_info[:2] == (3, 11); print(sys.version); assert "ODBC Driver 18 for SQL Server" in pyodbc.drivers(), pyodbc.drivers()'
        if ($LASTEXITCODE -ne 0) { throw 'Python imports/version/ODBC failed' }
        & $apiPython -m pip check
    }
    Check 'qualification dependencies' {
        & engine/qualification/venv/Scripts/python.exe -c 'import duckdb, pyarrow, trino, hypothesis, pytest, psycopg2; print("DuckDB", duckdb.__version__, "PyArrow", pyarrow.__version__, "Trino client", trino.__version__)'
    }
    Check 'Docker engine' { docker info --format '{{.ServerVersion}} / {{.OSType}} / {{.NCPU}} CPUs / {{.MemTotal}} bytes' }
    Check 'Docker Compose' { docker compose version }
    Check 'GitHub CLI' { gh --version }
    Check 'Azure CLI' { az version -o json }
    Check 'Kubernetes client' { kubectl version --client -o json }
    Check 'Helm' { helm version --short }
    Check 'Gitleaks' { gitleaks version }
    if ($RunChecks) {
        Check 'Rust formatting' { cargo fmt --all --manifest-path engine/Cargo.toml -- --check }
        Check 'Rust tests' { cargo test --locked --workspace --manifest-path engine/Cargo.toml --no-fail-fast }
        Check 'strict Clippy' { cargo clippy --locked --workspace --all-targets --manifest-path engine/Cargo.toml -- -D warnings }
        Check 'benchmark compilation' { cargo bench --locked --manifest-path engine/Cargo.toml -p kaveon-storage -p kaveon-exec --no-run }
        Check 'shared types' { pnpm --filter @kaveon/types type-check }
        Check 'Studio types' { pnpm --filter kaveon-studio type-check }
        Check 'Studio lint' { pnpm --filter kaveon-studio lint }
        Check 'Studio production image' { docker build --file studio/Dockerfile --tag kaveon-studio:qualification --build-arg "NPM_REGISTRY=$NpmRegistry" . }
        Check 'documentation' { node scripts/validate-docs.mjs }
        Check 'API syntax' { & $apiPython -m compileall -q -x '[/\\]venv[/\\]' api }
        Check 'patch whitespace' { git diff --check }
    }
} finally { Pop-Location }
if ($failures.Count) {
    $failures | ForEach-Object { Write-Host "FAIL $_" -ForegroundColor Red }
    exit 1
}
Write-Host 'All requested environment checks passed.' -ForegroundColor Green
