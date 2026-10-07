# Engine operator microbenchmarks — 2026-10-06

These are release-build operator measurements on the local development host.
They are diagnostic evidence for the next AKS Kaveon-versus-Trino campaign;
they are not a cross-engine performance claim because Trino was not run in the
same process, on the same host, or under the same resource budget.

Commands:

```text
cargo test -p kaveon-exec --release partial_stage_rate -- --ignored --nocapture
cargo test -p kaveon-exec --release merge_rate_of_near_unique_partial_rows -- --ignored --nocapture
```

Results:

| Operator shape | Configuration | Result |
|---|---|---:|
| Four million near-unique keys | adaptive off | 150 ns/row |
| Four million near-unique keys | adaptive on | 112 ns/row; 1,843,200 rows passed through |
| Four million low-cardinality keys | adaptive off | 37 ns/row |
| Four million low-cardinality keys | adaptive on | 39 ns/row |
| Four million encoded near-unique partial rows merged into 3,750,000 groups | best of three | 8,304,068 rows/s (120 ns/row) |

The adaptive path reduces the near-unique partial-stage cost in this fixture
by about 25% and leaves low-cardinality aggregation essentially unchanged.
The merge benchmark reached a 316 MiB peak reservation with 390 reservation
calls. The result supports the existing diagnosis that high-cardinality
aggregation and exchange/merge work are the main remaining Trino-loss shape,
while also showing that local operator improvements cannot substitute for the
resource-matched distributed campaign.

The next valid comparison remains the declared six-round AKS run with the same
object, worker budgets, warm-up policy, exact-result checks, and no result
cache. Until that run is complete, the measured 1.45x throughput result and
the 1.90x objective remain unchanged.
