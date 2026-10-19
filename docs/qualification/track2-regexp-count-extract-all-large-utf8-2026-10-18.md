# Track 2: `REGEXP_COUNT` and `REGEXP_EXTRACT_ALL` large UTF-8 parity

## Scope

This qualification covers the two scalar regular-expression functions over
both Arrow `Utf8` and `LargeUtf8` values. `REGEXP_COUNT` returns the number of
non-overlapping matches, while `REGEXP_EXTRACT_ALL` returns the first capture
group for each match (or the complete match when the pattern has no capture
group). A NULL input remains NULL.

The evaluator uses the same row-aware text accessor for both UTF-8 widths, so
the pattern and value may independently be either representation. Invalid
patterns continue to fail with the function name in the error.

## Evidence

- `cargo test -p kaveon-exec --lib evaluates_regexp_count_and_extract_all_for_large_utf8 -- --nocapture`
  passed on 2026-10-18. The test checks `LargeUtf8` values, capture-group
  extraction, non-overlapping count semantics, and NULL propagation.
- `KAVEON_DIFF_ONLY=regexp_count_and_extract_all cargo test -p kaveon-server --bin kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture`
  passed on 2026-10-18. The case matched dictionary/plain, multi-file,
  partitioned, clustered, and in-process three-worker fragment execution.

## Boundary

This qualifies the bounded scalar function surface. Full external pinned-Trino
client and cloud deployment differential coverage remains part of the broader
Track 2 release corpus.
