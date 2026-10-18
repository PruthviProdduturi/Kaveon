# Track 1 join planning: limit byte-bound qualification

## Scope

This gate covers the byte estimate propagated through a `LIMIT` or `OFFSET`
wrapper around a relation used by the join planner. The estimate is an upper
bound for build-side costing; it must never round a non-empty relation down to
zero bytes.

## Result

The optimizer now uses the same ceiling-based scaling helper for row-derived
byte estimates that it uses for filtered estimates. A three-row source with a
two-byte source hint and `LIMIT 1` therefore retains a one-byte upper bound,
rather than the previous zero-byte floor. The bound remains conservative for
broadcast admission and build-side comparison.

Focused qualification on the local optimizer crate:

```text
cargo test -p kaveon-optim statistics::tests::
18 passed; 0 failed
```

The regression is `keeps_nonzero_byte_upper_bound_through_a_small_limit`.
Existing limit, offset, byte-cap, build-reorder, and broadcast tests also pass
in the same run.

## Boundary

This is planner costing evidence only. It does not claim that join skew is
resolved or that multi-node high-cardinality joins have been qualified. AKS
skew qualification and a broader multi-join cost model remain open.
