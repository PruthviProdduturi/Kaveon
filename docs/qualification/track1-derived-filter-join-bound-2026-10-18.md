# Derived-filter join bound qualification

## Scope

This qualification covers one conservative optimizer rule: a filter above a
bounded or otherwise derived relation cannot increase its cardinality. The
optimizer therefore retains the input relation's existing upper bound when a
predicate is outside the derived relation. A predicate directly over a scan
continues to use the table's statistics for selectivity estimation.

The rule matters for queries shaped like:

```sql
SELECT *
FROM big b
JOIN (
  SELECT id, value FROM small LIMIT 1000
) s ON b.id = s.id
WHERE s.id > 5;
```

Before this change, the outer filter made the derived build relation opaque to
the join cost pass and the plan fell back to a partitioned join. The bounded
`LIMIT` is an explicit upper bound, so preserving it lets the existing safe
broadcast decision apply without guessing filter selectivity.

## Evidence

The focused optimizer regression is:

```text
cargo test -p kaveon-optim statistics::tests::keeps_a_limit_bound_through_a_filter_on_a_derived_subquery --quiet
running 1 test
.
test result: ok. 1 passed; 0 failed
```

The test asserts that the plan retains `JoinDistribution::BroadcastRight` for
the bounded derived input. It does not claim general selectivity estimation,
skew handling, or multi-join cost-based enumeration.

## Boundary

This remains a bounded Track 1 improvement. Filters over derived relations are
costed by the input bound, not by invented statistics. General join ordering,
skew splitting, broader cloud statistics, and larger mixed-workload
qualification remain open in the parity tracker.
