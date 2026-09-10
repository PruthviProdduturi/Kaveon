# Proposed Kaveon–Trino 1.90× benchmark

The proposed primary metric is **successful exact-result queries per second**
for an equal-weight, concurrent analytical workload. A 1.90 ratio means Kaveon
completes 90% more correct queries per second than Trino in the declared test.
This metric is proposed pending explicit user acceptance. It is different from
90% lower latency, 1.90× lower cost, or general product superiority.

Throughput is the strongest primary candidate because both systems are
distributed analytical engines and the product goal concerns concurrent
dashboard and analytical work. Per-query median (p50), p95, minimum, and maximum
wall latency remain required diagnostics, so throughput cannot hide a severe
tail-latency regression. Cost is excluded until repeatable infrastructure cost
and utilization measurements exist.

## Publication contract

The first executable comparison is the resource-matched single-node Docker
suite in `engine/qualification/same_files.py`:

- Kaveon and Trino read the identical two Parquet files; SHA-256 hashes are
  recorded and DuckDB supplies the independent exact-result reference.
- The fixed extended corpus has twelve named queries: count, selective filter,
  arithmetic projection, low/medium/high-cardinality aggregation,
  multi-aggregate, exact distinct, TopN, equi-join and grouped join shapes.
- Both containers receive 4 CPU and 8 GiB. The harness compares Docker CPU,
  memory, swap, affinity and quota fields exactly.
- Concurrency is four. Each query receives at least five warmups and thirty
  measured latency executions. The mixed corpus runs for at least six
  alternating engine-order rounds with ten executions of every query per round.
- The primary policy is warm cache. Cold-cache results must use a separate
  matched experiment because a portable user-space command cannot prove equal
  eviction across the OS filesystem cache, Trino JVM caches and Kaveon caches.
  Cold and warm samples must never be combined.
- Every measured result is fully consumed and checked before it counts as a
  success. Any missing query, wrong row, wrong result hash, failed request,
  unmatched resource, short run, or ratio below 1.90 fails the gate.

Build the release image and start the pinned qualification Trino service, then
run:

```powershell
docker compose -f engine/qualification/compose.yml up -d --wait trino
docker build -t kaveon-engine:qualification -f engine/Dockerfile engine
& engine/qualification/venv/Scripts/python.exe engine/qualification/same_files.py `
  --docker-image kaveon-engine:qualification --suite extended `
  --rows 5000000 --customers 100000 --warmups 5 --repetitions 30 `
  --local-parallelism 4 --throughput-rounds 6 --throughput-repeats 10 `
  --concurrency 4 --output tmp/qualification-trino-publication
& engine/qualification/venv/Scripts/python.exe engine/qualification/trino_claim_gate.py `
  tmp/qualification-trino-publication/report.json `
  --output tmp/qualification-trino-publication/claim-gate.json
```

The evaluator may report `technical_gate_passed=true`; it always records
`claim_eligible=false` while the metric remains proposed. Even after metric
acceptance, the result applies only to the recorded workload and environment.

## AKS boundary

The current Kaveon AKS chart requests one coordinator with limits of 2 CPU/4 GiB
and three workers with limits of 3 CPU/6 GiB each. Requests are lower: 500m/1 GiB
for the coordinator and 1 CPU/2 GiB per worker. There is no matched Trino AKS
deployment in this repository. The existing AKS cluster can establish Kaveon
correctness, worker use, recovery and pressure behavior, but it cannot establish
a Kaveon/Trino speed ratio.

A distributed publication run requires an isolated Trino coordinator and three
workers with the same per-role requests and limits, the same node SKU and count,
the same scheduling constraints, identical ADLS objects and region, equivalent
TLS/authentication boundaries, and recorded pod/node image digests. Run warm and
cold policies separately and retain correctness hashes, per-query p50/p95,
throughput rounds, CPU, memory, network, storage I/O and cost. No subscription
policy changes are needed or permitted.
