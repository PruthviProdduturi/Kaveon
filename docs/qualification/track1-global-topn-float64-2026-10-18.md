# Track 1 — finite Float64 TopN qualification

## Scope

Global `ORDER BY ... LIMIT` candidate selection now uses the typed selection
comparator for non-null finite `Float64` keys. This avoids materializing the
general Arrow lexsort candidate path for the common finite case. Values that
are NULL or NaN intentionally retain Arrow ordering so Trino-compatible null
and non-finite placement remains authoritative.

## Evidence

From `engine/`:

```text
cargo fmt --all -- --check
cargo test -p kaveon-exec topn::tests::typed_float64_topn_matches_arrow_for_finite_values -- --nocapture
running 1 test
test topn::tests::typed_float64_topn_matches_arrow_for_finite_values ... ok

test result: ok. 1 passed; 0 failed
```

The regression compares ascending and descending finite Float64 results for
limits 0, 1, 3, and above the input size against Arrow
`lexsort_to_indices`. It also verifies NaN declines the typed path.

## Boundary

This is local/in-process TopN evidence. Cloud qualification and a fully
parallel ordered result stream remain open.
