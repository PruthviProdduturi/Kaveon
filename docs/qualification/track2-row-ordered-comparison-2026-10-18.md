# Track 2 ROW ordered comparison qualification — 2026-10-18

Kaveon evaluates ordered comparisons between compatible `ROW` values lexicographically, matching Trino's three-valued comparison contract: fields are compared from left to right, a later field is consulted only when preceding fields compare equal, and a NULL field or NULL parent ROW produces SQL NULL rather than an arbitrary ordering. MAP values remain explicitly rejected for ordered comparison because Trino does not define an order for MAP.

The evaluator regression covers a NULL parent ROW alongside a valid row and verifies `[NULL, TRUE]` for `<`; the existing lexicographic regression covers first-field short-circuiting and NULL field propagation. The differential corpus adds a constant ROW ordering case for every local Parquet layout and the in-process three-worker fragment path; the complete corpus gate remains a separate longer-running qualification.

Validation:

```text
cargo test -p kaveon-exec row_ordering --quiet
2 passed, 0 failed

the differential corpus case was added; the full sweep was not rerun in this focused change
```

This change qualifies the evaluator's primitive-compatible ROW ordering locally and adds the local/distributed differential case. The full sweep, external Trino-client and cloud qualification, broader nested complex-type ordering, and MAP ordering remain out of scope; MAP ordering is intentionally unsupported.
