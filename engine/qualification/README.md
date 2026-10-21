# Engine qualification environment

This suite checks small deterministic SQL fixtures against DuckDB and Trino 483.
Kaveon reads generated Parquet files; DuckDB reads the same Arrow values; Trino
reads equivalent explicitly typed BIGINT VALUES relations. It compares ordered row
values, including NULLs, and reports errors. It is not a throughput benchmark,
a complete type-compatibility suite, or a same-lake-files Trino comparison.

## Setup on Windows

Use Python 3.11 and the native Rust prerequisites in the
[development environment guide](../../docs/engineering/development-environment.md).
From the repository root:

```powershell
. ./scripts/dev-env.ps1
py -3.11 -m venv engine/qualification/venv
& engine/qualification/venv/Scripts/python.exe -m pip install -r engine/qualification/requirements.txt
docker compose -f engine/qualification/compose.yml up -d --wait
cargo build --locked --release --manifest-path engine/Cargo.toml --bin kaveon-server --bin kaveon
```

The Compose project isolates its services and binds only loopback:

| Service | Address | Purpose |
|---|---|---|
| Trino 483 | `http://127.0.0.1:18080` | SQL reference, bundled CLI and TPCH generator |
| PostgreSQL 17 | `127.0.0.1:15432` | Independent relational reference and integration fixtures |

Both container images are pinned by digest. PostgreSQL's checked-in credentials
are disposable local fixture values, not production credentials. Its data lives
in this Compose project's named volume. No cloud resources are provisioned.

## Run the gates

```powershell
# Coordinator-local baseline plus known semantic regressions.
& engine/qualification/venv/Scripts/python.exe engine/qualification/smoke.py --server-bin engine/target/release/kaveon-server.exe --regressions --output tmp/qualification-local-release

# Real coordinator plus two workers; local fallback fails the gate.
& engine/qualification/venv/Scripts/python.exe engine/qualification/smoke.py --server-bin engine/target/release/kaveon-server.exe --workers 2 --output tmp/qualification-distributed-release
```

Each invocation starts its own temporary Engine processes, waits for readiness
and worker discovery, uses a fresh internal exchange token, and stops only the
processes it created. Logs and `report.json` remain in the output directory.
Reports include the Git revision, committed Engine tree ID, dirty-tree paths,
binary SHA-256, fixture checksums, reference versions, SQL, expected/actual rows,
and distributed stage counts. The tree ID identifies committed source; the
binary hash identifies the actual executable and does not prove its provenance.

A nonzero exit status means failure. Known regressions are not marked xfail or
silently omitted. The five initial failures are fixed; the expanded suite currently
passes 37 SQL cases. The suite also checks authenticated paging and replay. See the
[readiness evidence and open gates](../../docs/engineering/engine-readiness-qualification.md).

## Result-cache SLO

The coordinator cache has a separate fail-closed harness. It compares an
uncached result with a cold cached result and repeated warm hits, records p50,
p95, p99 and maximum wall latency, checks exact result hashes, verifies that
`result_cache=false` does not add a hit, checks cache counters and clear
behavior, and requires the Engine's hit-latency metrics. Run it against an
admin-authenticated coordinator:

```powershell
$env:KAVEON_TOKEN = "<admin token>"
python engine/qualification/cache_slo.py `
  --server https://engine.example `
  --sql "SELECT COUNT(*) AS n FROM OpenSource.public.kaveon_events_enriched" `
  --output tmp/cache-slo/report.json
```

The restart gate is intentionally explicit. After restarting the coordinator,
run one identical query with a fresh report and pass a JSON file containing
`{"restart_miss": true}` through `--restart-evidence`; the harness never
claims restart behavior from a process it did not control. A successful run
still qualifies only the declared query and topology, not a shared cache or
arbitrary SQL workload.

## CLI gates

```powershell
# The packaged client's batch contract against a fixture coordinator (also run by the Engine workflow).
python engine/qualification/cli_workflows.py --cli engine/target/release/kaveon.exe

# The interactive shell driven through a Windows ConPTY (pip install pywinpty pyte):
# suggestion, Ctrl-R, prefix history, a multi-line paste, Ctrl-A/K/U, Ctrl-C.
$env:KAVEON_TOKEN = "<admin token>"
python engine/qualification/cli_pty_windows.py --cli engine/target/release/kaveon.exe --server http://localhost:8081
```

`cli_pty_windows.py` needs a reachable coordinator (the local Compose stack)
and checks the text each step leaves on a 120×32 screen; `--show` prints every
screen. It is Windows-only and not part of CI.

## Same-file comparison

The disposable Trino `lake` catalog uses a file metastore for local qualification
only. It reads the exact Parquet files generated under `tmp/qualification-lake`,
mounted read-only in the reference container. Kaveon discovers the corresponding
Delta v1 fixture tables; DuckDB independently reads their Parquet files. Each run
uses a fresh schema and retains fixture checksums for inspection.

```powershell
& engine/qualification/venv/Scripts/python.exe engine/qualification/same_files.py --server-bin engine/target/release/kaveon-server.exe --rows 1000000 --repetitions 5 --workers 5 --output tmp/qualification-same-files
```

This command checks six query results and records timing diagnostics after a
warmup, alternating engine execution order. Native Kaveon and containerized Trino
have unequal resource limits: **these timings cannot support a speedup claim**.
The corrected million-row five-worker comparison passes all six queries; the
original exchange/retry and projection-order failures remain recorded in earlier
reports.

For a resource-matched single-node comparison, build the release image and run:

```powershell
docker build -t kaveon-engine:qualification -f engine/Dockerfile engine
& engine/qualification/venv/Scripts/python.exe engine/qualification/trino_benchmark_preflight.py --docker-image kaveon-engine:qualification --output tmp/qualification-matched/preflight.json
& engine/qualification/venv/Scripts/python.exe engine/qualification/same_files.py --docker-image kaveon-engine:qualification --rows 1000000 --repetitions 5 --local-parallelism 4 --throughput-rounds 20 --concurrency 4 --output tmp/qualification-matched
```

Both engines receive 4 CPUs and 8 GiB. The harness checks Trino's configured
limits, records image IDs, validates every result against DuckDB and alternates
engine order. Throughput uses the fixed equal-weight six-query workload with
the same concurrency and no retry of rejected queries. Kaveon replayable results
are explicitly released after full consumption, with cleanup included in timing;
otherwise repeated runs would exhaust the bounded retained-result quota. The provisional target
is 1.9 times Trino's successful queries per second, with zero errors. This does
not establish superiority on other workloads or overall feature coverage.
Native executables are copied into the report directory before launch so builds
cannot change or lock the binary being qualified.

The same harness supports a resource-matched two-worker Docker topology. It
creates a disposable Docker network and uses container DNS for coordinator
discovery; non-metadata cases must report distributed stages:

```powershell
& engine/qualification/venv/Scripts/python.exe engine/qualification/same_files.py --docker-image kaveon-engine:qualification --suite extended --rows 500000 --customers 50000 --warmups 1 --repetitions 2 --workers 2 --output tmp/qualification-docker-2worker
```

The two-worker run is a distributed correctness gate. Keep it separate from
the single-node performance publication gate because the topologies and
resource scheduling are different.

## Benchmarks and shutdown

The six-query million-row workload is an optimization diagnostic. For the broader
publication gate (count, arithmetic projection, low/medium/high-cardinality groups,
multiple aggregates and grouped join), use the declared scale and repetitions:

```powershell
& engine/qualification/venv/Scripts/python.exe engine/qualification/same_files.py --docker-image kaveon-engine:qualification --suite extended --rows 5000000 --customers 100000 --warmups 5 --repetitions 30 --local-parallelism 4 --throughput-rounds 6 --throughput-repeats 10 --concurrency 4 --output tmp/qualification-extended
```

Smaller or shorter runs remain useful for correctness diagnostics, but cannot set
the performance target flag. Retain per-query and per-round results and satisfy
the remaining hardware, configuration and reporting requirements in the benchmark
protocol before publishing a performance claim.

```powershell
# Check that Criterion workloads execute, without publishing timing claims.
cargo bench --locked --manifest-path engine/Cargo.toml -p kaveon-storage --bench storage -- --test
cargo bench --locked --manifest-path engine/Cargo.toml -p kaveon-exec --bench aggregate -- --test

# Stop reference services; retain their data and installed images.
docker compose -f engine/qualification/compose.yml stop
```

For performance work follow the [benchmark protocol](../benches/README.md).
The proposed 1.90× metric, fail-closed evaluator, cache policy and AKS boundary
were defined in the 1.90× proposal, now the history section of the [benchmark program](../../docs/qualification/benchmark-program.md#history-the-190-proposal-2026-09-03-to-09-17), which supersedes it for every published number.
When local Docker/WSL is unavailable, the three-worker AKS path is defined by
the [resource-matched Trino chart](../../infra/helm/kaveon-trino-benchmark/README.md).
It builds the same extended fixture, records DuckDB result hashes, verifies the
immutable ADLS objects, alternates exclusive Kaveon/Trino leases on the existing
nodes, and evaluates the resulting report with `trino_cloud_claim_gate.py`.
Expand qualification to typed numeric semantics, randomized/property cases,
cloud snapshots, constrained memory, concurrency, skew, worker loss, retries,
and cancellation before making production-readiness claims.

The fault/pressure verifier's `current` profile targets the current
`OpenSource.public` catalog and its documented showcase row counts. Use it for
the current AKS deployment; `nyc` and `medallion` remain compatibility profiles
for older fixtures:

```powershell
python scripts/qualify-aks-fault-pressure.py `
  --context kaveon-aks --namespace kaveon --profile current `
  --output tmp/aks-fault-pressure/report.json
```

## Combined PostgreSQL and Trino publication gate

`comparison_gate.py` combines two independent reports and fails closed. The
analytics input is the `report.json` emitted by `same_files.py`. The transaction
input must be emitted by a resource-matched Kaveon/PostgreSQL runner and contain:

- `resources_matched`, `publication_workload_gate`, and `correctness_passed` set
  to true;
- at least 30 measured, correctness-checked samples for `point_read`, `insert`,
  `update`, `delete`, `conflicting_update`, and `multi_record_commit`;
- a deterministic `state_sha256` for each operation;
- `kaveon_over_postgresql_qps` greater than 1.0 and
  `kaveon_over_postgresql_p95_ratio` below 1.0.

Run the evaluator even when one service is unavailable; omitted inputs are
reported as `pending` and the process returns exit code 2 instead of manufacturing
a result:

```powershell
& engine/qualification/venv/Scripts/python.exe engine/qualification/comparison_gate.py `
  --analytics tmp/qualification-extended/report.json `
  --transactions tmp/qualification-transactions/report.json `
  --output tmp/qualification-combined/report.json
```

The combined report claims only the declared workloads. It cannot establish
general PostgreSQL or Trino superiority. Kaveon's product SQL surface now exposes
the write operations, while its bounded point-read operation remains pending.

### Distributed execution evidence

`aks_distributed_compare.py` is the reproducible three-worker publication
harness. Each successful query retains p50/median, p95 and p99 latency, and
the report includes aggregate throughput plus Kaveon task evidence by stage.
The task evidence records CPU time and compute wall/queue time, peak query and
spill memory, exchange input/output bytes, payload counts, Arrow decode/encode
time, hash/copy time, copy allocations/bytes, and spill bytes/runs/compactions.
Missing task metrics remain visible through `tasks_with_metrics` and
`tasks_with_cpu`; they are not inferred.

The co-tenant lease check records stable controller identities
(`namespace/kind/name`) instead of node-prefixed, restart-specific pod names.
Node placements are retained separately in each observation for audit. This
means an unrelated pod recreation or worker rescheduling cannot invalidate a
long comparison while a real workload appearing or disappearing still fails
closed.

This harness does not itself kill a worker. Before publishing a distributed
result, attach the report from `scripts/qualify-aks-fault-pressure.py` to prove
worker retry, active-query cancellation cleanup, and exact-result recovery. The comparison report records these
gates explicitly as `verification_gates`: hash aggregate/join spill remains
open because those operators fail closed at the memory limit, and coordinator
restart cleanup remains open until retained exchange chunks reach zero.
These gates prevent a latency or throughput report from being interpreted as
Trino-class fault-tolerant execution.

To produce one fail-closed AKS qualification artifact after both runs complete:

```powershell
python scripts/merge-aks-distributed-qualification.py `
  --comparison tmp/qualification-trino-publication/report.json `
  --fault-pressure docs/engineering/engine-aks-fault-pressure-validation-2026-09-10.json `
  --output tmp/qualification-trino-publication/distributed-qualification.json
```

The merged schema has independent gates for exact results, concurrency, worker
retry, worker spill cleanup, coordinator restart cleanup, and pod memory. It
also carries p50/p95/p99 latency and measured CPU, memory, exchange, spill, and
admission counters. Exit code 2 means diagnostics were produced but at least
one release gate remains open. The output also contains a machine-checked
`slo` object and `gates.distributed_execution_slo`; missing cancellation,
image-binding, latency, or reliability evidence is deliberately false rather
than inferred.

### Track 1 distributed execution SLO

The merged artifact is also the SLO evidence record for the declared image and
workload. The release SLO is deliberately bounded rather than a claim about
arbitrary data or hardware:

* every accepted statement must return its expected typed result (100% exact
  result success; admission refusals are counted separately in the `slo`
  object and excluded from the accepted-query denominator);
* at least 99.9% of accepted statements in the five-minute mixed-workload
  window must complete successfully, with no silent mismatches;
* p50, p95, p99, maximum latency, and successful exact-result QPS must be
  present in the report; the 1.90x Trino throughput objective is a separate
  comparison gate;
* worker-loss retry, cancellation cleanup, and bounded memory/spill evidence
  must be present and pass, including zero retained temporary files after the
  cleanup checks.

Missing samples, task metrics, image/configuration binding, or any required
failure/recovery evidence leaves the SLO pending. A passing local unit suite or
an isolated latency sample cannot satisfy this SLO. The current report merger
accepts both the legacy nested fault-pressure schema and the current live
runner schema, including per-request admission-refusal counts. It does not
claim a release SLO when the comparison and fault-pressure reports carry
different Engine image digests; live evidence is cross-bound before the gate
can pass. It also does not claim coordinator-loss survival, sustained network-pressure,
cloud-storage fault injection, longer endurance, or upgrade/rollback evidence;
those remain explicit Track 1 blockers.

Long publication runs also write an atomic `<report>.progress.json` sidecar
after startup and after each engine round. It contains only bounded progress
metadata (current engine/round, completed rounds, and throughput counts); it is
operational visibility, not claim evidence. The complete report remains the
only artifact accepted by the claim gate.

The local pressure runner's grouped case is an exact bounded-completion check;
it does not require a filesystem spill sample because the current final
aggregate emits bounded batches and may finish without spilling. Forced
distributed aggregate spill and cleanup are covered separately by the server
differential test `a_distributed_grouped_aggregate_under_spill_matches_the_local_result`.
The pressure profile still requires deliberate rejection for window memory,
oversized string expansion, and an exhausted disk-spill quota.

The coordinator result-cache SLO is a separate Track 1 gate. It is not implied
by the distributed execution SLO or by the local cache test suite. The AKS
record in `docs/qualification/cache-slo-aks-2026-10-13.json` qualifies the
declared row-result workload: exact warm-hit parity, 20/20 warm hits, latency
percentiles, hit metrics, bypass, clear, authorization scope, and
restart-to-miss all pass on the deployed image. Shared cross-coordinator
invalidation, TTL-expiry evidence, and cache-backed transactional reads remain
outside that qualification and must not be claimed from it.

`transaction_compare.py` now produces the transaction input for the product SQL
facade. It executes point read, insert, update, delete, conflicting update, and a
three-record commit against identical logical records. PostgreSQL stores the
fixture in a fresh disposable table; Kaveon uses a random ID prefix. Every timed
operation is followed by a canonical `(id, revision, document_sha256)` state
comparison. Runs alternate engine order and record individual latency samples.

Both services must run in containers with identical CPU, memory, swap, affinity,
and quota settings. Credentials are read from environment variables and are not
written to the report:

Build the image from the exact commit under test. Start it with
`KAVEON_PRODUCT_TRANSACTIONS_ENABLED=true`, the three
`KAVEON_PRODUCT_ADLS_{ACCOUNT,CONTAINER,PREFIX}` values, its normal authenticated
security configuration, and a read/write ADLS identity. Start PostgreSQL with
the same Docker CPU and memory flags as the Kaveon container. Confirm both names
are running with `docker inspect` before invoking the runner.

```powershell
$env:KAVEON_QUALIFICATION_TOKEN = "<disposable qualification token>"
$env:KAVEON_QUALIFICATION_POSTGRES_DSN = "host=127.0.0.1 port=15432 dbname=qualification user=qualification password=<local fixture password>"
& engine/qualification/venv/Scripts/python.exe engine/qualification/transaction_compare.py `
  --kaveon-url https://127.0.0.1:18443 --ca-cert tmp/kaveon-ca.crt `
  --kaveon-container kaveon-transaction-benchmark `
  --postgres-container kaveon-postgres-benchmark `
  --warmups 5 --repetitions 30 `
  --output tmp/qualification-transactions/report.json
```

Point reads use only authenticated `GET /v1/product/{kind}/{id}`; the timed path
never opens a transaction or filters the complete `BEGIN` snapshot. A missing,
unauthorized, disabled, or malformed endpoint sets `bounded_point_read: false`,
clears its samples, and returns exit code 2. `comparison_gate.py` independently
requires that flag. A complete correctness and publication-scale run returns
zero, but only the combined gate may decide whether the measured PostgreSQL
throughput and p95 targets were met.
## Offline immutable table-publication rehearsal

`table_publication_rehearsal.py` verifies a captured table-publication report
without contacting Azure, AKS, ADLS, or PostgreSQL. It checks object and
manifest digests, replay/conflict/head-preservation evidence, and restart
journal recovery. Reports must explicitly set `mutations_enabled` to `false`.

```powershell
python engine/qualification/table_publication_rehearsal.py `
  --report tmp/table-publication/rehearsal.json `
  --output tmp/table-publication/verification.json
```

The command exits `0` only for a complete passing report and `2` for invalid
or incomplete evidence. It is a verification gate, not a live deployment or
mutation command.
