# Track 2 qualification: NULL-safe nested `ARRAY_DISTINCT`

Date: 2026-10-17

## Contract

Nested ROW and array elements in `ARRAY_DISTINCT` use NULL-safe equality for
deduplication. Two rows with the same non-NULL fields and NULL in the same
fields are one distinct value, while genuinely different rows remain separate.

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-exec array_distinct_deduplicates_nested_array_elements_in_order --quiet
```

Observed result:

```text
running 1 test
.
test result: ok; 1 passed; 0 failed
```

The regression covers `[ROW(1, NULL), ROW(1, NULL), ROW(2, NULL)]` and asserts
two output rows while preserving the typed NULL field.

This closes nested NULL equality for the distinct operation only; full complex
array-function parity remains Partial in the tracker.
