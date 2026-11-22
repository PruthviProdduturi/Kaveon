[CmdletBinding()]
param(
    [string]$SubscriptionId = "00000000-0000-4000-8000-000000000000",
    [string]$ResourceGroup = "kaveon-rg",
    [decimal]$MonthlyCeilingUsd = 100
)

$ErrorActionPreference = "Stop"

function Invoke-AzJson([string[]]$Arguments) {
    $raw = & az @Arguments -o json 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw (($raw | ForEach-Object { $_.ToString().Trim() } | Where-Object { $_ }) -join " ")
    }
    if (-not $raw) { return @() }
    return ($raw -join "`n" | ConvertFrom-Json)
}

az account set --subscription $SubscriptionId
$account = Invoke-AzJson @("account", "show")
$aks = @(Invoke-AzJson @("aks", "list") | ForEach-Object {
    $pool = @($_.agentPoolProfiles)
    [pscustomobject]@{
        name = $_.name
        resource_group = $_.resourceGroup
        power = $_.powerState.code
        tier = $_.sku.tier
        pools = @($pool | ForEach-Object {
            [pscustomobject]@{
                name = $_.name; mode = $_.mode; count = $_.count
                vm_size = $_.vmSize; autoscaling = $_.enableAutoScaling
                min = $_.minCount; max = $_.maxCount
            }
        })
    }
})
$vms = @(Invoke-AzJson @("vm", "list", "-d") | ForEach-Object {
    [pscustomobject]@{
        name = $_.name; resource_group = $_.resourceGroup
        power = $_.powerState; vm_size = $_.hardwareProfile.vmSize
    }
})
$nats = @(Invoke-AzJson @("network", "nat", "gateway", "list") | ForEach-Object {
    [pscustomobject]@{ name = $_.name; resource_group = $_.resourceGroup; sku = $_.sku.name }
})

$monthStart = (Get-Date -Day 1).ToUniversalTime().ToString("yyyy-MM-dd")
$tomorrow = (Get-Date).ToUniversalTime().Date.AddDays(1).ToString("yyyy-MM-dd")
$costRows = @()
$costError = $null
try {
    $costRows = @(Invoke-AzJson @(
        "consumption", "usage", "list", "--start-date", $monthStart,
        "--end-date", $tomorrow
    ))
} catch {
    $costError = $_.Exception.Message
}
$costTotal = $null
if ($costRows.Count -gt 0) {
    $costTotal = [decimal](($costRows | ForEach-Object { [decimal]$_.pretaxCost } |
            Measure-Object -Sum).Sum)
}

$violations = [System.Collections.Generic.List[string]]::new()
$runningAks = @($aks | Where-Object { $_.power -eq "Running" })
$runningVms = @($vms | Where-Object { $_.power -eq "VM running" })
if (($runningAks.Count + $runningVms.Count) -gt 1) {
    $violations.Add("overlapping serving compute: $($runningAks.Count) running AKS cluster(s), $($runningVms.Count) running VM(s)")
}
foreach ($cluster in $runningAks) {
    if ($cluster.tier -ne "Free") { $violations.Add("AKS '$($cluster.name)' is not Free tier") }
    foreach ($pool in $cluster.pools | Where-Object { $_.mode -eq "User" }) {
        if ($pool.vm_size -ne "Standard_B2s") { $violations.Add("AKS '$($cluster.name)' user pool '$($pool.name)' uses $($pool.vm_size), expected Standard_B2s") }
        if ([int]$pool.count -gt 2 -or ($pool.max -and [int]$pool.max -gt 2)) { $violations.Add("AKS '$($cluster.name)' worker pool '$($pool.name)' exceeds two workers") }
    }
}
if ($nats.Count -gt 0) { $violations.Add("NAT gateway(s) exist; dedicated NAT is disabled by the Kaveon budget policy") }
if ($null -eq $costTotal) {
    $violations.Add("current-month Cost Management usage is unavailable; cannot prove the $MonthlyCeilingUsd USD ceiling")
} elseif ($costTotal -gt $MonthlyCeilingUsd) {
    $violations.Add("current-month pretax usage $costTotal USD exceeds the $MonthlyCeilingUsd USD ceiling")
}

$result = [ordered]@{
    subscription = $account.id
    subscription_name = $account.name
    month_start = $monthStart
    cost_total_usd = $costTotal
    cost_rows = $costRows.Count
    cost_error = $costError
    running_aks = $runningAks | ForEach-Object name
    running_vms = $runningVms | ForEach-Object name
    aks = $aks
    vms = $vms
    nat_gateways = $nats
    violations = @($violations)
    passed = ($violations.Count -eq 0)
}
$result | ConvertTo-Json -Depth 8
if (-not $result.passed) { exit 2 }
