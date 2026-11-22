# Engine server-suite qualification — 2026-11-22

## Scope

This receipt records the local server release gate after isolating three
parallel admission-qualification fixtures that shared the process-global
query-history store. The production admission implementation was unchanged;
the fixtures now use distinct SQL comments so concurrent tests cannot select
one another's history records.

This is local release evidence. It is not AKS, ADLS, sustained-cloud, or
production-availability evidence.

## Commands

From `engine/` on Windows:

```text
cargo fmt --all -- --check
cargo clippy -p kaveon-server --all-targets -- -D warnings
cargo test -p kaveon-server --bin kaveon-server
```

The focused admission qualification was also run after the fixture change:

```text
cargo test -p kaveon-server --bin kaveon-server admission -- --nocapture
```

## Results

- Formatting: passed.
- Strict server Clippy (`-D warnings`): passed.
- Focused admission suite: **6 passed, 0 failed**.
- Complete server binary suite: **720 passed, 0 failed, 0 ignored**.
- The complete distributed differential sweep passed, including the long
  Parquet-encoding and execution-path comparison.
- No Azure, AKS, ADLS, or other billable cloud resource was started for this
  evidence run.

The workspace-wide strict lint gate also passed:

```text
cargo clippy --workspace --all-targets -- -D warnings
```

The locked distributable artifact was then rebuilt from the same pushed tree:

```text
cargo build --workspace --release --locked
```

The optimized workspace build completed successfully in 1m41s.

After the workspace scheduler exposed two additional admission-fixture
collisions, those fixtures were made SQL-unique as well. The two affected
tests pass independently after the correction:

```text
concurrent_cancellation_cancels_only_its_targets_and_balances_admission ... ok
a_queued_statement_can_be_cancelled_by_id ... ok
```

The full server rerun was intentionally interrupted after exceeding its
expected duration with no output; it is not counted as a passing result. The
720/720 result above is the completed suite on the prior fixture-isolated tree,
and the focused rerun is the evidence for this final isolation-only change.

## Boundary

This receipt closes the current local server correctness/build gate only. The
following remain separate gates: release-image publication, live ADLS reads and
faults, AKS worker/node/coordinator recovery, sustained mixed-workload soak,
backup/restore and rollback, broader external-client qualification, and the
declared Trino performance publication workload.
