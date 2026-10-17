# Track 2 qualification: nested `ARRAYS_OVERLAP` NULL semantics

Date: 2026-10-16  
Commit: follow-up to `8619d418`

## Contract

When nested ROW or array elements compare as SQL `UNKNOWN` because a nested
field is NULL, `ARRAYS_OVERLAP` must preserve that uncertainty. It returns
`TRUE` if any pair is definitely equal; otherwise it returns `NULL` when an
unknown pair exists, and `FALSE` only when every pair is definitively unequal.

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-exec arrays_overlap_preserves_unknown_nested_row_equality --quiet
```

Observed result:

```text
running 1 test
.
test result: ok. 1 passed; 0 failed
```

The regression compares `ROW(1, CAST(NULL AS BIGINT))` with
`ROW(1, 2)` and asserts a NULL result rather than a false negative.

The broader `ARRAYS_OVERLAP` and distributed differential suites remain the
authoritative checks for ordinary nested, temporal, string, and numeric paths.
