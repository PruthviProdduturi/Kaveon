# AKS Kaveon/Trino qualification

This chart adds a pinned Trino 483 coordinator and three workers to the existing
`kaveon-test-aks` cluster. Both Trino StatefulSets stay at zero replicas until a
runner Job holds the benchmark lease. The runner alternates Kaveon and Trino by
round, runs only one engine at a time on the three existing worker nodes, warms
the newly activated engine, and restores the original Kaveon replica counts in
all exit paths. It never creates or resizes an AKS node pool.
Kaveon activation waits for all workers to advertise the coordinator's current
catalog snapshot, in addition to ordinary worker readiness.

The role budgets exactly mirror `infra/helm/kaveon-test`: coordinator requests
500m CPU/1 GiB and limits 2 CPU/4 GiB; each worker requests 1 CPU/2 GiB and
limits 3 CPU/6 GiB. The preflight also requires one system node, three worker
nodes, one node SKU, immutable images, three-way worker spreading, and no
an identical recorded set of non-DaemonSet co-tenants on worker nodes in every
engine phase. Any pod-name or placement change fails the run. The coordinator
shares ordinary system-node background load in both engine phases.

Trino uses the same `kaveon-engine` service account and Azure workload identity
as Kaveon. Client traffic is TLS protected and password authenticated; Trino
node traffic uses its separate shared secret and required internal TLS. Its
Delta catalog reads the fixture from ADLS with the Trino native
Azure filesystem. The generated fixture stores a Parquet file plus a minimal
Delta log for each table. Kaveon reads those Parquet objects directly; Trino's
external Delta tables resolve to those exact objects. The runner verifies every
blob's byte length and create-time SHA-256 metadata immediately before the run.
Trino's native local filesystem is enabled only for its ephemeral file-metastore
directory; measured table bytes remain in ADLS.
The runner registers the pre-existing Delta tables with Trino's opt-in
`system.register_table` procedure, so registration never rewrites fixture data.

## One-time tools and variables

Use the recorded test coordinates. Do not change Azure policy, firewall rules,
node counts, or SKUs for this run.

```powershell
$subscription = "eaa4a83d-8511-497c-b0bc-40aa5f0deae1"
$resourceGroup = "test-prproddu-test"
$cluster = "kaveon-test-aks"
$registry = "kvtestegmf6oweugsno"
$account = "kvtestegmf6oweugsno"
$namespace = "kaveon"
$release = "kaveon-benchmark"
$runId = "run-20260910-a1" # lowercase DNS label; choose a new value every run
$prefix = "benchmarks/$runId"
$kaveonDigest = "sha256:1b41e38c56cb4fff74f17aa3c67ea599d6c3a6adfa4df55dede984ebc1d8d50a"
$apiDigest = "sha256:4dc35a7b61f905f7debbf93f45da68c5560a2a17d6ca33e485d1956f54e526f6"
```

The qualification virtual environment at `engine/qualification/venv`, Azure CLI, Helm 3,
and the existing Azure login are required. The fixture directory is private and
gitignored. It contains a short-lived caller token; delete it after upload.

## Build and upload the immutable fixture

```powershell
& engine/qualification/venv/Scripts/python.exe engine/qualification/aks_cloud_fixture.py `
  --account $account --container silver --prefix $prefix `
  --rows 5000000 --customers 100000 --output "tmp/$runId"

az aks command invoke --subscription $subscription --resource-group $resourceGroup `
  --name $cluster --file "tmp/$runId/upload-bundle.tar.gz" `
  --command "mkdir -p /tmp/$runId && tar -xzf upload-bundle.tar.gz -C /tmp/$runId && sh /tmp/$runId/kaveon-benchmark-upload/run.sh" -o json `
  | Set-Content "tmp/$runId/upload-command.json"
```

The upload uses `If-None-Match: *`; an existing object makes it fail rather than
replace prior benchmark data. Check `exitCode == 0` and retain the command JSON.
If the AKS command attachment limit rejects the generated archive, use an
approved runner inside the allowed subnet to upload the same files with the
same create-only headers. Do not open the storage firewall.

Apply the small manifest ConfigMap and the private Trino authentication and TLS
secrets through the same authenticated AKS command path:

```powershell
az aks command invoke --subscription $subscription --resource-group $resourceGroup `
  --name $cluster --file "tmp/$runId/manifest-configmap.json" `
  --command "kubectl apply -f manifest-configmap.json" -o json
python scripts/aks-trino-benchmark-secret.py --release $release `
  --output "tmp/$runId/trino-secret.json"
az aks command invoke --subscription $subscription --resource-group $resourceGroup `
  --name $cluster --file "tmp/$runId/trino-secret.json" `
  --command "kubectl apply -f trino-secret.json" -o json
```

## Build the runner and park Trino

The runner has only Python standard-library dependencies. Build it in the
existing ACR from the already-pinned API image so local Docker is unnecessary:

```powershell
az acr build --registry $registry --image "kaveon-benchmark-runner:$runId" `
  --file engine/qualification/cloud_runner.Dockerfile `
  --build-arg "BASE_IMAGE=$registry.azurecr.io/kaveon-api@$apiDigest" `
  engine/qualification
$runnerDigest = az acr repository show --name $registry `
  --image "kaveon-benchmark-runner:$runId" --query digest -o tsv
if ($runnerDigest -notmatch '^sha256:[a-f0-9]{64}$') { throw "Runner digest was not resolved" }
```

Import the pinned upstream Trino manifest into the existing ACR when the cluster
enforces an allowed-registry policy. Import preserves the digest and does not
change subscription policy:

```powershell
$trinoDigest = "sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534"
az acr import --subscription $subscription --name $registry `
  --source "docker.io/trinodb/trino@$trinoDigest" `
  --image "trino:483"
```

Install the chart with Trino parked at zero replicas. The Azure CLI accepts one
file attachment, so package the chart as an uncompressed tar; the AKS command
environment does not guarantee that `gzip` is installed:

```powershell
tar -cf "tmp/$runId/kaveon-trino-benchmark-chart.tar" `
  -C infra/helm kaveon-trino-benchmark
az aks command invoke --subscription $subscription --resource-group $resourceGroup `
  --name $cluster --file "tmp/$runId/kaveon-trino-benchmark-chart.tar" `
  --command "tar -xf kaveon-trino-benchmark-chart.tar && helm upgrade --install $release ./kaveon-trino-benchmark --namespace $namespace --set-string trino.image.repository=$registry.azurecr.io/trino --set-string trino.image.digest=$trinoDigest --set-string trino.storage.account=$account --set-string runner.kaveon.expectedImageDigest=$kaveonDigest --wait --timeout 10m" -o json
```

The chart uses Trino's manual internal-TLS mode with a dedicated headless
Service and pod FQDNs. The generated certificate covers the client Service DNS
names and only the benchmark StatefulSet names beneath the private `kb`
headless Service. Its short name keeps the coordinator FQDN within Linux's
64-character hostname limit.
The same private CA validates both client and node traffic.

Run the read-only preflight. It addresses Azure by subscription, resource group,
and cluster name, so the unrelated local `kubectl` context cannot redirect it:

```powershell
python engine/qualification/aks_trino_benchmark_preflight.py `
  --subscription $subscription --resource-group $resourceGroup --cluster $cluster `
  --namespace $namespace --release $release --manifest "tmp/$runId/manifest.json" `
  --kaveon-image-digest $kaveonDigest --output "tmp/$runId/preflight.json"
```

Do not create the Job unless this exits zero. The Job temporarily interrupts the
test portal's Engine while it leases the existing Kaveon StatefulSets.

## Run and gate

```powershell
az aks command invoke --subscription $subscription --resource-group $resourceGroup `
  --name $cluster --file "tmp/$runId/kaveon-trino-benchmark-chart.tar" `
  --command "tar -xf kaveon-trino-benchmark-chart.tar && helm upgrade --install $release ./kaveon-trino-benchmark --namespace $namespace --set-string trino.image.repository=$registry.azurecr.io/trino --set-string trino.image.digest=$trinoDigest --set-string trino.storage.account=$account --set runner.enabled=true --set-string runner.runId=$runId --set-string runner.image.repository=$registry.azurecr.io/kaveon-benchmark-runner --set-string runner.image.digest=$runnerDigest --set-string runner.kaveon.expectedImageDigest=$kaveonDigest --wait=false" -o json
```

Poll without holding an ARM command open:

```powershell
az aks command invoke --subscription $subscription --resource-group $resourceGroup `
  --name $cluster --command "kubectl -n $namespace get job $release-$runId -o json" -o json
```

After the Job is complete, fetch and evaluate the report:

```powershell
python scripts/fetch-aks-trino-benchmark.py --subscription $subscription `
  --resource-group $resourceGroup --cluster $cluster --namespace $namespace `
  --job "$release-$runId" --output "tmp/$runId/report.json"
python engine/qualification/trino_cloud_claim_gate.py "tmp/$runId/report.json" `
  --output "tmp/$runId/claim-gate.json"
```

The gate requires twelve exact DuckDB reference hashes, 30 measured latency
samples per query and engine, six alternating throughput rounds, ten executions
of every query per round, concurrency four, matching role resources, pinned
images, verified blobs, distinct worker nodes, successful restoration, and a
ratio of at least 1.90. It always emits `claim_eligible=false` while the primary
metric remains proposed. A passing result applies only to this fixture, cache
policy, cluster, and recorded images.

The first live execution must also prove that Trino 483's Azure default
credential consumes the projected AKS workload-identity token and that its
file metastore can register the read-only external Delta locations. Those are
runtime integration checks, not facts established by a rendered manifest.
The runner records resource specifications, placement and image identity; it
does not collect Azure Monitor CPU/network/storage time series or Azure cost
exports. Retain those separately before any publication that discusses
utilization or cost.

Delete the private local bundle after preserving the non-secret manifest and
reports in the intended evidence store. Scaling or deleting the cluster,
changing policy, and deleting the existing Kaveon release or PVC are outside
this procedure.
