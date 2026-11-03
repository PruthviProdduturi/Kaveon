# T-SQL `EOMONTH` offset qualification — 2026-11-03

Kaveon's bounded SQL Server date surface now has evidence for all three
offset forms supported by the planner:

- the omitted offset (`EOMONTH(date)`), equivalent to zero months;
- a positive literal month offset (`EOMONTH(date, 2)`);
- a row-local integer month offset (`EOMONTH(date, actions - 20)`).

The lowering casts the input to DATE, truncates to the month boundary, adds
one month plus the requested offset, then subtracts one day. This preserves
SQL Server's DATE result type and month-end behavior for DATE and TIMESTAMP
inputs without exposing an unbounded general expression contract.

Evidence:

```text
cargo test -p kaveon-sql --lib lowers_tsql_eomonth_to_typed_date_operators -- --nocapture
cargo test -p kaveon-server --bin kaveon-server distributed_tsql_eomonth_offsets_match_local -- --nocapture
```

The shared differential sweep also carries the same deterministic query across
single-file, multi-file, partitioned, clustered, local, and three-worker
execution paths. The full sweep passed after the deterministic tie-breaker was
added to the case (`643.59s`, one test, zero failures). External Fabric/Azure
SQL passthrough is outside this Engine qualification.
