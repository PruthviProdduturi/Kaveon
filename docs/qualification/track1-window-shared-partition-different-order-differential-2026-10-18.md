# Track 1 — Shared partition windows with independent orderings

## Scope

This qualification covers two window expressions that share `PARTITION BY region` but use independent orderings. The query uses `RANK` and a `RANGE` aggregate frame, with a unique final result ordering so peer rows are compared deterministically.

## Evidence

The differential harness runs the case against one-file, dictionary-encoded, partitioned, and clustered Parquet layouts. It compares local execution with the in-process three-worker distributed fragment path and requires exact result equality for every layout.

```text
KAVEON_DIFF_ONLY=partitioned_window_different_orderings cargo test -p kaveon-server --bin kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
result: 1 passed, 0 failed (38.43s)
```

## Boundary

This qualifies the deterministic mixed-order shape on the in-process three-worker harness. Live AKS and larger cloud-topology qualification remain open.
