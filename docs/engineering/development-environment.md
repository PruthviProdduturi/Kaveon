# Development environment and qualification baseline

Verified on Windows on September 4, 2026. Environment readiness does not imply
Engine production readiness. The purpose of this setup is to make the correctness,
memory, distributed execution, and security work measurable.

## Installed and verified tools

| Component | Verified version or configuration |
|---|---|
| Rust / Cargo | 1.98.1 stable, MSVC x64; rustfmt and Clippy installed |
| Native compiler | Visual Studio Build Tools 2022, C++ x64/x86 workload and Windows SDK |
| Node / pnpm | Node 22.23.2; pnpm 10.34.5, matching package.json and CI |
| Python API | Python 3.11.9 in `api/venv`, repository requirements installed |
| ODBC | Microsoft ODBC Driver 18 for SQL Server, x64 |
| Qualification Python | Separate `engine/qualification/venv`; frozen requirements alongside it |
| SQL/file tooling | DuckDB 1.5.5, PyArrow 25.0.1, Trino client 0.339.0, psycopg2 |
| Test tooling | pytest, Hypothesis, Rust workspace tests and Criterion |
| Containers | Docker Engine 29.7.2, Compose 5.5.0, Linux containers |
| References | Trino 483 and PostgreSQL 17, pinned image digests |
| Operations | Git, GitHub CLI, Azure CLI, kubectl 1.36.1, Helm 4.2.4, Gitleaks 8.30.1 |

This machine has 24 logical CPUs and approximately 191 GiB RAM. Docker reports
approximately 94 GiB available memory. Reference services have explicit resource
limits. These facts are capacity inventory, not measured scale or performance.

## Session entry and repeatable checks

From the repository root:

```powershell
. ./scripts/dev-env.ps1
./scripts/check-environment.ps1
```

The session helper adds the Rust bin directory to the current PATH, selects Node
from `.nvmrc` through fnm, and points PyO3 at the API's Python 3.11 environment.
`.nvmrc` now matches the project's Node 22 requirement. It does not change another
repository's Node default or the system's default Python 3.13 installation.

Run the full build gates with:

```powershell
./scripts/check-environment.ps1 -RunChecks -NpmRegistry https://packagefeedproxy.microsoft.io/npm/
```

This machine's configured package feed works from Windows and Docker. Direct
npmjs access from Docker failed with a TLS handshake error. On an unrestricted
network, omit `-NpmRegistry` to use the Dockerfile's public npm registry.
Certificate validation remains enabled.

The full check uses the Studio Docker image for production packaging. Native
Windows `next build` compiled and generated pages but failed when creating
standalone symbolic links (`EPERM`). The Linux container build passed and its
`/docs` route returned HTTP 200. No Windows security policy was changed to bypass
the symbolic-link restriction. Existing React hook and Edge-runtime warnings
remain visible.

Use the [qualification guide](../../engine/qualification/README.md) for the
reference services, fixture generation, local/distributed checks, and shutdown.

## Provisioning another Windows machine

Install [Rust through rustup](https://rust-lang.org/tools/install/) with the
documented MSVC prerequisites. The following winget packages were used here:

```powershell
winget install --id Rustlang.Rustup --exact
winget install --id Microsoft.VisualStudio.2022.BuildTools --exact --override '--wait --quiet --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
winget install --id Python.Python.3.11 --exact
winget install --id Microsoft.msodbcsql.18 --exact
```

With fnm installed, select Node and install the pinned package manager:

```powershell
fnm install 22
fnm use 22
npm install --global pnpm@10.34.5
pnpm install --frozen-lockfile
py -3.11 -m venv api/venv
& api/venv/Scripts/python.exe -m pip install -r api/requirements.txt
rustup component add rustfmt clippy
```

Install Docker Desktop with a working Linux engine. GitHub/Azure/Kubernetes tools
need their own account access before remote work; a successful version check
does not establish cloud permissions. Trino uses its official
[container distribution](https://trino.io/docs/current/installation/containers.html),
so Java is provided inside the image and need not be installed on Windows.

## Verified baseline

- 228 Rust workspace tests passed; formatting and strict workspace/all-target Clippy passed.
- Native debug and release CLI/server builds passed.
- Both Criterion suites compiled in release mode and their test-mode workloads passed at the default one-million-row input.
- Shared/Studio type checks, Studio lint, documentation validation, API syntax and dependency consistency checks passed. There are no tracked API pytest cases to claim as passing.
- Studio production container built and served `/docs` with HTTP 200.
- PostgreSQL was healthy and completed a query through psycopg2; Trino served the semantic reference queries.
- All five basic query cases matched DuckDB and Trino locally.
- All five basic cases matched with two Engine workers and recorded 2–4 distributed stages per query; the harness rejects local fallback.

The targeted regression gate is intentionally red:

| Case | Reference result | Kaveon observation |
|---|---|---|
| `NOT IN` with right-side NULL | No rows | Incorrectly retains `2` and NULL |
| Uncorrelated `EXISTS` | All three outer rows | Incorrectly retains only matching value `1` |
| `RANGE` window with peers | Peer-aware counts | HTTP 500: window COUNT evaluated inline |
| `GROUPS` window with peers | Peer-group counts | Same execution error |
| Empty following-row frame | Final count is zero | Same execution error |

The window queries currently fail in planning/execution before proving frame
semantics. Static review also found that frame bounds ignore ROWS/RANGE/GROUPS
units; retain these cases while repairing both layers. The harness compares
ordered values and does not yet prove complete SQL type equivalence.

## Gates before a production-class rating

1. Make the semantic regression suite pass; broaden NULL, decimal, date/time, join and window coverage with independent reference results.
2. Complete query-wide memory accounting, aggregate/join spill, streaming exchange and root-result delivery.
3. Add Engine end-user authentication, authorization, TLS, resource groups and quotas; complete the Studio/catalog bridge.
4. Qualify actual ADLS identity and range reads, cloud Delta/checkpoints and required table formats.
5. Run worker-loss, retry/cancellation, skew, memory-pressure and concurrency tests on representative topology and data.
6. Publish reproducible equal-resource, equal-data, correctness-gated performance measurements.

No cloud resources were created or modified during environment setup. Existing
GitHub and Azure CLI sessions were readable; the active Azure subscription was
not Kaveon's personal subscription. Scope future Azure commands explicitly and
verify resource/data-plane access before running ADLS or AKS tests. No live
cloud or five-worker qualification is claimed here.
