#requires -Version 7.0

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Registry,

    [Parameter(Mandatory = $true)]
    [string]$ImageTag,

    [Parameter(Mandatory = $true)]
    [string]$ChefImage,

    [Parameter(Mandatory = $true)]
    [string]$RuntimeImage,

    [string]$Subscription,
    [string]$DependencyRepository = "kaveon-engine-dependencies",
    [string]$ImageRepository = "kaveon-engine",
    [switch]$ForceDependencyRebuild,
    [switch]$PlanOnly
)

$ErrorActionPreference = "Stop"

function Assert-ImmutableImage([string]$Name, [string]$Value) {
    if ($Value -notmatch '^[^@\s]+@sha256:[a-f0-9]{64}$') {
        throw "$Name must be an immutable image reference ending in @sha256:<64 lowercase hex characters>"
    }
}

function Invoke-Az([string[]]$Arguments, [switch]$AllowFailure) {
    $output = & az @Arguments
    if (-not $AllowFailure -and $LASTEXITCODE -ne 0) {
        throw "az $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
    return $output
}

Assert-ImmutableImage "ChefImage" $ChefImage
Assert-ImmutableImage "RuntimeImage" $RuntimeImage
if ($ImageTag -notmatch '^[A-Za-z0-9_][A-Za-z0-9._-]{0,127}$') {
    throw "ImageTag is not a valid OCI tag"
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$engineRoot = Join-Path $repoRoot "engine"
$dependencyDockerfile = Join-Path $engineRoot "Dockerfile.dependencies"
$sourceRevision = (git -C $repoRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $sourceRevision -notmatch '^[a-f0-9]{40}$') {
    throw "Git source revision was not resolved"
}
$engineChanges = git -C $repoRoot status --porcelain -- engine
if ($LASTEXITCODE -ne 0) {
    throw "Git could not inspect the Engine source tree"
}
if ($engineChanges) {
    throw "Engine build context is dirty; commit or remove Engine changes before creating a reproducible image"
}

# This identity covers Cargo's locked dependency inputs and the target layout
# that cargo-chef uses when creating recipe.json. The image build also stores the
# exact recipe hash; Dockerfile.cached compares it before compiling sources.
$identityStream = [System.IO.MemoryStream]::new()
$identityWriter = [System.IO.BinaryWriter]::new(
    $identityStream,
    [System.Text.UTF8Encoding]::new($false),
    $true
)
$identityWriter.Write("chef-image")
$identityWriter.Write($ChefImage)

$contentFiles = @(
    Get-ChildItem -LiteralPath $engineRoot -Recurse -File |
        Where-Object {
            $_.Name -in @("Cargo.toml", "Cargo.lock", "build.rs") -or
            $_.Name -like "rust-toolchain*" -or
            $_.FullName -match '[\\/]\.cargo[\\/]'
        }
)
$contentFiles += Get-Item -LiteralPath $dependencyDockerfile
$contentFiles = $contentFiles | Sort-Object FullName -Unique

foreach ($file in $contentFiles) {
    $relative = [System.IO.Path]::GetRelativePath($engineRoot, $file.FullName).Replace('\', '/')
    $bytes = [System.IO.File]::ReadAllBytes($file.FullName)
    $identityWriter.Write("content")
    $identityWriter.Write($relative)
    $identityWriter.Write($bytes.Length)
    $identityWriter.Write($bytes)
}

Get-ChildItem -LiteralPath (Join-Path $engineRoot "crates") -Recurse -File -Filter "*.rs" |
    Sort-Object FullName |
    ForEach-Object {
        $relative = [System.IO.Path]::GetRelativePath($engineRoot, $_.FullName).Replace('\', '/')
        $identityWriter.Write("target-path")
        $identityWriter.Write($relative)
    }

$identityWriter.Flush()
$sha256 = [System.Security.Cryptography.SHA256]::Create()
try {
    $recipeInputHash = [Convert]::ToHexString($sha256.ComputeHash($identityStream.ToArray())).ToLowerInvariant()
}
finally {
    $sha256.Dispose()
    $identityWriter.Dispose()
    $identityStream.Dispose()
}

$dependencyTag = "recipe-$recipeInputHash"
if ($PlanOnly) {
    [pscustomobject]@{
        dependency_tag = "$DependencyRepository`:$dependencyTag"
        recipe_input_sha256 = $recipeInputHash
        source_revision = $sourceRevision
        image_tag = "$ImageRepository`:$ImageTag"
        chef_image = $ChefImage
        runtime_image = $RuntimeImage
    } | ConvertTo-Json
    return
}

$scope = @()
if ($Subscription) {
    $scope = @("--subscription", $Subscription)
}
$loginServer = (Invoke-Az (@("acr", "show") + $scope + @("--name", $Registry, "--query", "loginServer", "-o", "tsv"))).Trim()
if (-not $loginServer) {
    throw "ACR login server was not resolved"
}

$dependencyDigest = $null
if (-not $ForceDependencyRebuild) {
    $previousErrorPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $candidate = Invoke-Az (@("acr", "repository", "show") + $scope + @(
            "--name", $Registry,
            "--image", "${DependencyRepository}:$dependencyTag",
            "--query", "digest",
            "-o", "tsv"
        )) -AllowFailure 2>$null
        if ($LASTEXITCODE -eq 0) {
            $dependencyDigest = ($candidate | Out-String).Trim()
        }
    }
    finally {
        $ErrorActionPreference = $previousErrorPreference
    }
}

Push-Location $repoRoot
try {
    if ($dependencyDigest -notmatch '^sha256:[a-f0-9]{64}$') {
        Write-Host "Building dependency cache $DependencyRepository`:$dependencyTag"
        Invoke-Az (@("acr", "build") + $scope + @(
            "--registry", $Registry,
            "--platform", "linux/amd64",
            "--image", "${DependencyRepository}:$dependencyTag",
            "--file", "engine/Dockerfile.dependencies",
            "--build-arg", "CHEF_IMAGE=$ChefImage",
            "--build-arg", "RECIPE_INPUT_SHA256=$recipeInputHash",
            "engine"
        )) | Write-Host
        $dependencyDigest = (Invoke-Az (@("acr", "repository", "show") + $scope + @(
            "--name", $Registry,
            "--image", "${DependencyRepository}:$dependencyTag",
            "--query", "digest",
            "-o", "tsv"
        ))).Trim()
    }
    else {
        Write-Host "Reusing dependency cache $DependencyRepository@$dependencyDigest"
    }

    if ($dependencyDigest -notmatch '^sha256:[a-f0-9]{64}$') {
        throw "Dependency image digest was not resolved"
    }

    $dependencyImage = "$loginServer/$DependencyRepository@$dependencyDigest"
    Write-Host "Building source image $ImageRepository`:$ImageTag from $dependencyImage"
    Invoke-Az (@("acr", "build") + $scope + @(
        "--registry", $Registry,
        "--platform", "linux/amd64",
        "--image", "${ImageRepository}:$ImageTag",
        "--file", "engine/Dockerfile.cached",
        "--build-arg", "DEPENDENCY_IMAGE=$dependencyImage",
        "--build-arg", "RUNTIME_IMAGE=$RuntimeImage",
        "--build-arg", "RECIPE_INPUT_SHA256=$recipeInputHash",
        "--build-arg", "SOURCE_REVISION=$sourceRevision",
        "engine"
    )) | Write-Host

    $imageDigest = (Invoke-Az (@("acr", "repository", "show") + $scope + @(
        "--name", $Registry,
        "--image", "${ImageRepository}:$ImageTag",
        "--query", "digest",
        "-o", "tsv"
    ))).Trim()
    if ($imageDigest -notmatch '^sha256:[a-f0-9]{64}$') {
        throw "Runtime image digest was not resolved"
    }

    [pscustomobject]@{
        image = "$loginServer/$ImageRepository@$imageDigest"
        image_tag = "$ImageRepository`:$ImageTag"
        image_digest = $imageDigest
        dependency_image = $dependencyImage
        dependency_tag = "$DependencyRepository`:$dependencyTag"
        recipe_input_sha256 = $recipeInputHash
        source_revision = $sourceRevision
    } | ConvertTo-Json
}
finally {
    Pop-Location
}
