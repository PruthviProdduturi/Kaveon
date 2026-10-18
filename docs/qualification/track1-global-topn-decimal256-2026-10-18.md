# Track 1 — Decimal256 global TopN qualification

The global TopN merge now has a typed single-key Decimal256 path. Worker
candidates are ranked directly on Arrow `i256` values, preserving exact
ordering for values wider than `i128`; null placement and ascending or
descending direction follow the same `SortOptions` contract as the general
lexsort path.

## Evidence

The focused regression compares typed TopN indices with Arrow lexsort across
null placement, both directions, duplicate values, and limits below, equal to,
and above the input size:

```text
cargo test -p kaveon-exec typed_decimal256_topn_matches_arrow_with_nulls_and_ordering --quiet
running 1 test
.
test result: ok. 1 passed; 0 failed
```

This is local executor evidence. The distributed global-order contract remains
bounded by the coordinator merge and still needs deployed multi-worker
qualification for a fully parallel ordered result stream.
