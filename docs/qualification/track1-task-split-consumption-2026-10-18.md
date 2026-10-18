# Track 1 task split consumption — 2026-10-18

## Scope

This qualification covers the coordinator-to-worker handoff for local Parquet
and pinned Delta scan splits. The coordinator adds the scan node identity to
each `TaskAssignment.splits` entry and copies the task's entries into the
optional executable-fragment `ScanSpec.assigned_splits` field. A worker uses
those concrete file and row-group entries without listing or re-enumerating the
table. Older fragments that omit the field retain the existing local fallback.

## Evidence

The focused server tests pass:

- `worker_consumes_coordinator_splits_without_relisting_post_plan_files`
  reads only the coordinator-selected file and row group after a second file
  is added. It observes one opened file and one selected row group.
- `coordinator_assignments_carry_enumerated_local_delta_splits` verifies that
  the dispatch fragment carries the same pinned split entries as the task
  assignment and excludes a Delta file outside the pinned version.

Validation on the current worktree:

```text
cargo fmt --all -- --check                  PASS
cargo clippy -p kaveon-server --all-targets -- -D warnings  PASS
focused tests                               2 passed, 0 failed
```

## Boundary

This is local implementation evidence. Directory scans that already carry a
`ScanListing` continue to use their existing exact file assignment. Remote
object-store assignment and cloud snapshot/failure qualification remain open,
as do broader topology and endurance gates in Track 1.
