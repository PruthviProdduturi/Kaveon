# Suspend and resume the test AKS cluster

This runbook removes only the `kaveon-test-aks` resource. It does not delete
the resource group, PostgreSQL, ADLS Gen2, ACR, managed identities, VNet, or
network security groups.

## Before suspension

```powershell
$sub = "eaa4a83d-8511-497c-b0bc-40aa5f0deae1"
$rg = "test-prproddu-test"
$aks = "kaveon-test-aks"
az account set --subscription $sub
az aks show --resource-group $rg --name $aks --query "{name:name,state:provisioningState,location:location,pools:agentPoolProfiles[].{name:name,count:count,vmSize:vmSize}}" -o json
az resource list --resource-group $rg --query "[].{name:name,type:type,id:id}" -o json > tmp/test-prproddu-test-resources-before-aks-delete.json
```

Create and verify a PostgreSQL disk snapshot before deleting AKS. The PVC uses
the AKS managed node resource group and its reclaim policy is `Delete`, so this
snapshot is the recovery boundary for the metadata database:

```powershell
$nodeRg = az aks show --resource-group $rg --name $aks --query nodeResourceGroup -o tsv
$postgresDisk = az disk show --resource-group $nodeRg --name pvc-c050911c-b695-4b6e-924d-0d241ca17f5c --query id -o tsv
az snapshot create --resource-group $rg --name kaveon-postgres-pre-aks-delete-20260912 --source $postgresDisk --sku Standard_LRS
az snapshot show --resource-group $rg --name kaveon-postgres-pre-aks-delete-20260912 --query "{name:name,state:provisioningState}" -o json
az storage account show --resource-group $rg --name kvtestegmf6oweugsno --query "{name:name,defaultAction:networkRuleSet.defaultAction,sku:sku.name}" -o json
```

## Suspend at the end of the test window

```powershell
az aks delete --resource-group $rg --name $aks --yes --no-wait
az aks show --resource-group $rg --name $aks --query provisioningState -o tsv
```

Wait for the resource to disappear, then verify that the storage account,
PostgreSQL pod PVC resources, ACR, identities, and VNet still exist. Do not use
`az group delete`.

## Resume on Monday

The AKS control plane is not a stop/start resource. Recreate it from the
checked-in Bicep/Helm deployment using the same subscription, resource group,
network, identity, storage, and image digests. Reapply the saved values and
verify the PostgreSQL PVC and ADLS catalog before starting Engine workers.

ADLS migration to a personal subscription is a separate operation. It needs a
destination subscription, resource group, storage account, container, and
write authorization before any copy can be planned or executed.
