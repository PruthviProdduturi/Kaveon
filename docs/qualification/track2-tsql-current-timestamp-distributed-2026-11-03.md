# T-SQL current-timestamp distributed qualification — 2026-11-03

## Contract

`GETDATE()` and `SYSDATETIME()` are statement-stable in Kaveon's distributed
execution path. During logical planning, the coordinator captures one UTC
microsecond instant and lowers both functions to the Engine's typed timestamp
cast. Every worker therefore evaluates the same literal rather than sampling
its own wall clock. This preserves the existing timestamp type and keeps the
behavior deterministic across retries and fragment boundaries.

The qualification is limited to the no-argument forms. It does not claim local
time-zone presentation, clock-skew diagnostics, or arbitrary nondeterministic
functions.

## Evidence

- `sql::logical_plan::tests::lowers_common_tsql_scalar_aliases` — both aliases
  lower to typed timestamp literals.
- `differential_tests::distributed_tsql_current_timestamps_are_statement_stable`
  — exact local versus three-worker distributed result comparison over the
  shared fixture.
- `cargo test -p kaveon-sql --lib lowers_common_tsql_scalar_aliases` — passed.
- `cargo test -p kaveon-server --bin kaveon-server distributed_tsql_current_timestamps_are_statement_stable` — passed.

The authoritative corpus marks `getdate` and `sysdatetime` as
`qualified_for_statement_stable_timestamp` for the distributed path.
