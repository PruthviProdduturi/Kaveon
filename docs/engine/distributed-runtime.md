# Distributed runtime

The coordinator builds validated post-order stage DAGs. Exchanges use
deterministic multi-column hash, broadcast, or round-robin partitioning.
Executable fragments carry immutable resolved source format, location and
pinned Delta version so workers do not reinterpret mutable catalog state.
`engine/DISTRIBUTED_EXECUTION_STATUS.md` is the commit-by-commit record.

## Shapes

| Shape | Evidence |
|---|---|
| Scan/filter/project | Cluster-verified (ADLS decoder lanes, typed comparisons on the lanes, dictionary columns end to end) |
| Grouped and global aggregates | Cluster-verified: columnar partials on several threads that flush their groups to the exchange on memory pressure, a hybrid final merge that spills sub-partitions instead of replaying (the hybrid merge is on `dev`, measured locally, not yet on the cluster) |
| Exact `COUNT(DISTINCT)` and `DISTINCT` | Cluster-verified: hash-partitioned distinct rows across the workers, several threads per stage |
| Sort/TopN/limit/offset | Cluster-verified: partial/final TopN, `OFFSET n LIMIT m` as a top-N keeping n + m rows per partition, a TopN over the final aggregate inside each merge thread |
| Equi-join | Cluster-verified: hash repartition of both sides or broadcast of the small side chosen from exact statistics; forced worker loss retried the join on another worker with the exact result (2026-09-10) |
| Semi/anti join | Distributed: the subquery side broadcasts into the probe stage |
| Outer joins | Distributed; local semantics tested, distributed equivalence covered by the differential sweep on the shapes it has |
| UNION / INTERSECT / EXCEPT | Distributed; INTERSECT/EXCEPT deduplicate each side on the workers |
| Window functions | Distributed |

## Exchange

Arrow IPC exchange envelopes identify query, exchange, stage, attempt and
output partition and carry version, bounds and checksums. A task's output
streams while it runs: one lane per (partition, destination), 4 MiB chunks
in a bounded channel, four uploads in flight, up to 8 GiB per partition
(2048 chunks); chunks carry an open count and the last one the final count.
Where the consumer's node spools (`KAVEON_WORKER_EXCHANGE_SPOOL`, the AKS
configuration) producers upload straight to it and the coordinator relays
nothing; otherwise the coordinator's disk store is the hub
(`KAVEON_COORDINATOR_EXCHANGE_SPOOL`, the default) or workers hold payloads
in memory. Stores enforce byte and count ceilings and idempotency; retry
rotates attempts, stale attempts fail closed, cancellation propagates,
finishing a query cancels its orphaned tasks, and terminal cleanup releases
exchange and lifecycle state. Consumers download a whole payload to disk
before decoding it, and decode each producer's payload on a thread of its
own; streaming decode is the next exchange item.

## Memory

Every node runs a counting allocator under its cgroup memory limit with a
headroom of max(256 MiB, 15 %); reservations fail closed against live
bytes before the query budget. Statements and tasks are admitted through a
FIFO queue (64 deep, 60 s wait on the coordinator) rather than refused on
arrival. Hash aggregate, join, Sort and TopN spill through partitioned
paths when `KAVEON_HASH_SPILL_ROOT` is set. See the [memory
reference](../reference/engine-memory-management.md) and the [settings
reference](settings.md).

## Open gates

Streaming decode on the exchange consumer; aggregate and join spill
qualified under skew; the final-merge rate on near-unique keys measured on
the cluster; dynamic filtering; cost-based partition choices beyond exact
statistics; a sustained mixed-workload soak; backup/restore and upgrade
rehearsal; five-worker evidence.
