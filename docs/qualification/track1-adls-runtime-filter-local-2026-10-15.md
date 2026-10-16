# Track 1 ADLS runtime-filter regression — 2026-10-15

This is local implementation evidence for the Track 1 runtime dynamic-filtering
boundary. It is not a cloud-performance qualification.

## Scope

The ADLS single-object reader now publishes its projected schema before opening
the decoder, waits for the first consumer batch request, and copies the shared
runtime predicate immediately before lane construction. A build-side filter can
therefore arrive after the reader is opened but before row groups or lanes are
decoded. Filters are still applied as an exact residual at execution time.

The regression uses the in-memory ADLS object-store fixture with 100 rows and a
late `id IN (7)` filter. It verifies that the reader returns exactly one row and
records one accepted filter carrying one value.

## Reproduction

Run from `engine/`:

```powershell
cargo test -p kaveon-storage adls_reader --quiet
```

The focused suite passed `18 passed, 0 failed`. The regression is
`late_runtime_filter_is_applied_before_adls_lanes_start` in
`crates/storage/src/adls_reader.rs`.

The full workspace gate also passed on commit `89cbf573`:

```powershell
cargo test --workspace --no-fail-fast --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

## Boundary

This proves predicate ordering and exact results for the reader implementation.
It does not prove measurable file or row-group reduction on a live ADLS account,
cross-worker pruning, or cloud snapshot/retry behavior. Those remain open in the
parity tracker and require a representative cloud dataset and retained runtime
metrics.
