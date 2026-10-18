# Track 1 global TopN: empty partition result

## Scope

Positive-limit distributed `ORDER BY … LIMIT` queries can have every worker
return zero rows after filtering. The coordinator must preserve the worker
schema and return an empty result rather than treating the missing candidate as
an execution failure.

## Result

The coordinator TopN merge now materializes an empty typed `RecordBatch` when
all inputs are empty. Subsequent worker results can still replace it with a
non-empty candidate, while an all-empty query finishes with the correct schema
and zero rows.

Focused qualification:

```text
cargo test -p kaveon-server distributed_top_n_keeps_the_schema_when_all_workers_are_empty
1 passed; 0 failed
```

## Boundary

This fixes the empty-result boundary for positive limits. It does not change
the bounded coordinator merge for non-empty results or remove the remaining
coordinator bottleneck for fully ordered, unbounded result streams.
