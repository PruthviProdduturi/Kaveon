# Distributed runtime

The coordinator builds validated post-order stage DAGs. Exchanges use single, deterministic multi-column hash, broadcast, or round-robin partitioning. Executable fragment v2 carries immutable resolved source format/location so workers do not reinterpret mutable catalog state.

| Shape | Evidence |
|---|---|
| Scan/filter/project | Distributed and Docker-verified over local data |
| Aggregate | Partial/final weighted AVG and exact distinct state transport; representative Docker verification |
| Sort/TopN/limit | Partial/final TopN and root ordering; representative Docker verification |
| Equi-join | Hash repartition of both sides; representative inner join Docker verification |
| Cross join | Round-robin probe and broadcast build contract; broader scale evidence pending |
| Outer joins | Local semantics; distributed equivalence/failure evidence pending |

Arrow IPC v2 exchange envelopes identify query, exchange, stage, attempt, and output partition and include version, bounds, and corruption checks. Authenticated stores enforce byte/count ceilings and idempotency. Retry rotates attempts, stale attempts fail closed, cancellation propagates, and terminal cleanup releases exchange/lifecycle state.

Pending gates are streaming flow control, durable/spooled exchange, skew mitigation, dynamic filtering, cost-based partition choices, admission/resource groups, worker-loss stress, and sustained concurrent performance evidence.

