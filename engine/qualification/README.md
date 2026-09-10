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
Expand qualification to typed numeric semantics, randomized/property cases,
cloud snapshots, constrained memory, concurrency, skew, worker loss, retries,
and cancellation before making production-readiness claims.

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
general PostgreSQL or Trino superiority. Kaveon's transactional HTTP/SQL surface
does not yet expose the operations required to produce the transaction input, so
that half of the publication gate remains pending.
