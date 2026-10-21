[CmdletBinding()]
param(
    [string]$SubscriptionId = "4ed07f02-b111-4eea-98ce-1c177d573a51",
    [string]$ResourceGroup = "kaveon-rg",
    [string]$AksName = "kaveon-aks",
    [string]$StorageAccount = "kaveonlake",
    [string]$Container = "kaveon-product"
)

$ErrorActionPreference = "Stop"

az account set --subscription $SubscriptionId
$account = az account show --query "{subscription:id,tenant:tenantId,user:user.name}" -o json | ConvertFrom-Json
$aks = az aks show --resource-group $ResourceGroup --name $AksName `
    --query "{name:name,powerState:powerState.code,provisioningState:provisioningState}" -o json | ConvertFrom-Json
$storage = az storage account show --resource-group $ResourceGroup --name $StorageAccount `
    --query "{name:name,provisioningState:provisioningState,statusOfPrimary:statusOfPrimary,endpoint:primaryEndpoints.dfs}" -o json | ConvertFrom-Json

$dataPlane = "not_checked"
$dataPlaneError = $null
try {
    $null = az storage container list --account-name $StorageAccount --auth-mode login `
        --query "[].name" -o tsv 2>&1
    if ($LASTEXITCODE -eq 0) {
        $dataPlane = "ready"
    } else {
        $dataPlane = "failed"
        $dataPlaneError = "az storage container list exited with code $LASTEXITCODE"
    }
} catch {
    $dataPlane = "failed"
    $dataPlaneError = $_.Exception.Message
}

$result = [ordered]@{
    account = $account
    aks = $aks
    storage = $storage
    storage_data_plane = $dataPlane
    storage_data_plane_error = $dataPlaneError
    ready = ($aks.powerState -eq "Running" -and
        $aks.provisioningState -eq "Succeeded" -and
        $storage.provisioningState -eq "Succeeded" -and
        $storage.statusOfPrimary -eq "available" -and
        $dataPlane -eq "ready")
}
$result | ConvertTo-Json -Depth 6
if (-not $result.ready) { exit 2 }

