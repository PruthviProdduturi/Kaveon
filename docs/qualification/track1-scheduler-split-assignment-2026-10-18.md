# Track 1 scheduler split-assignment qualification

Date: 2026-10-18  
Scope: coordinator assignment metadata for local Parquet and Delta scans  
Implementation: `85032193` (`feat: attach pinned scan splits to task assignments`)

## Evidence

`CoordinatorOrchestrator::build_assignments` now derives `TaskAssignment.splits`
from the executable scan fragment. A listed scan uses its exact file and
row-group assignment. A local scan without a listing enumerates Parquet files
or Delta active files deterministically and distributes them by task partition.
Delta enumeration uses the fragment's pinned version, so a query does not
receive files added after its snapshot.

The regression creates a two-version Delta log, pins the fragment to version
zero, and verifies that the coordinator assigns `part-a.parquet` while never
assigning `part-b.parquet` from version one. It also verifies that assigned
split metadata identifies the Parquet format.

## Commands and results

Run from `engine/`:

```text
cargo test -p kaveon-server --bin kaveon-server coordinator_assignments_carry_enumerated_local_delta_splits --quiet
1 passed; 0 failed

cargo test -p kaveon-server --bin kaveon-server orchestrator::tests:: --quiet
11 passed; 0 failed

cargo test -p kaveon-server --bin kaveon-server scheduler --quiet
11 passed; 0 failed

cargo fmt --all -- --check
cargo clippy -p kaveon-server --bin kaveon-server --all-targets -- -D warnings
```

Formatting and strict Clippy completed successfully. This is local qualification
evidence; it makes no cloud deployment or endurance claim.

## Open scope

Remote/cloud split enumeration, lease heartbeat and expiry qualification, and
direct worker execution from `TaskAssignment.splits` remain open. Existing
fragment-level scan listings remain authoritative for actual worker reads until
that runtime-consumption work is separately implemented and qualified.
