# Track 1 — paged-result restart cleanup

## Scope

Paged result pages are intentionally process-local: their owner, page files,
and in-memory quota accounting cannot be restored safely after a coordinator
restart. Query history is the durable record and is restored from the query
journal. This gate ensures that a restart does not retain unbounded orphaned
page directories, while a live neighboring process is not disturbed.

## Implementation

`ResultStore` scans the system temporary directory during startup and removes
only `kaveon-result-*` directories whose filesystem modification time is older
than the 15-minute result TTL. Fresh directories are left untouched. A result
that was in flight when its process stopped remains unavailable through the
new process, so clients must resubmit rather than receive partial or
cross-process data.

## Evidence

From `engine/` on 2026-11-22:

```text
cargo fmt --all -- --check                         PASS
cargo test -p kaveon-server --all-targets results::tests -- --test-threads=1
running 7 tests
test result: ok. 7 passed; 0 failed; 0 ignored
cargo clippy -p kaveon-server --all-targets -- -D warnings   PASS
```

The focused suite covers quota refusal, owner isolation, in-progress paging,
abandoned-writer cleanup, explicit removal, expiration cleanup, and startup
preservation of a fresh result directory. This is local evidence; cloud
filesystem and multi-coordinator result continuity remain outside the gate.
