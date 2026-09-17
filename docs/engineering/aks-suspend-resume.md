# Suspend and resume the test AKS cluster

## Pause and resume (the weekend procedure since 2026-09-17)

The westus2 cluster (`test-prproddu-test-westus2` / `kaveon-test-aks`) is
paused over the weekend, not deleted: `az aks stop` deallocates the control
plane and both node pools while the managed disks stay, so the coordinator's
catalog PVC, the PostgreSQL PVC, the storage account with the benchmark
objects (ClickBench `hits.parquet`, TPC-H SF100 as Delta under
`opensource/benchmarks/tpch/delta/sf100/`) and the ACR images all come back
as they were. Worker `emptyDir` state (exchange spools, spills) is lost,
which is by design.

```powershell
az account set --subscription eaa4a83d-8511-497c-b0bc-40aa5f0deae1
# Friday, after the last run's record is committed:
kubectl -n kaveon get jobs            # nothing Running
az aks stop  --resource-group test-prproddu-test-westus2 --name kaveon-test-aks
# Monday:
az aks start --resource-group test-prproddu-test-westus2 --name kaveon-test-aks
az aks get-credentials --resource-group test-prproddu-test-westus2 --name kaveon-test-aks --overwrite-existing
kubectl config use-context kaveon-test-aks
kubectl -n kaveon rollout status sts/kaveon-coordinator && kubectl -n kaveon rollout status sts/kaveon-worker
kubectl -n kaveon rollout status deploy/kaveon-api
```

After a start, verify before benchmarking: the Engine answers
`SELECT COUNT(*) FROM clickbench.hits` on catalog `Benchmarks` (99,997,497)
and `SELECT COUNT(*) FROM tpch_sf100.lineitem` (600,037,902) — both through
`scripts/scale-suite.py` or the API pod; the Trino benchmark StatefulSets are
at 0 replicas (a Trino window scales them up and registers the TPC-H Delta
tables again by their logs, `scripts/benchmark-trino-suite.py`). The
Engine's admission and exchange settings are in the chart
(`infra/helm/kaveon-test/values.yaml`); a StatefulSet that lost a
`kubectl set image` roll keeps the digest recorded in the last run record
under `docs/qualification/clickbench/runs/`.

## Delete and recreate (the earlier procedure)

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
