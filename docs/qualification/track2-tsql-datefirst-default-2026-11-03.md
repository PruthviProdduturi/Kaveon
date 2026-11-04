# T-SQL `DATEFIRST 7` prefix qualification — 2026-11-03

## Contract

Kaveon accepts the common SQL Server session prefix:

```sql
SET DATEFIRST 7;
SELECT DATEPART(dw, event_date) FROM events;
```

`7` is the Engine's fixed default: Sunday is 1 and Saturday is 7. The
prefix is removed before planning because it does not change the Engine's
existing weekday semantics. Session-specific values remain fail-closed; the
Engine does not yet propagate mutable `DATEFIRST` state through distributed
workers.

## Evidence

- `parser::tests::accepts_only_the_default_tsql_datefirst_prefix`
- `differential_tests::distributed_tsql_default_datefirst_prefix_matches_local`
- The case `set_datefirst_default` is in the versioned 161-case corpus.
- The companion `set_datefirst_is_explicitly_unsupported` case continues to
  reject `SET DATEFIRST 1` with an actionable diagnostic.

## Boundary

This qualifies only the default no-op prefix. It does not claim support for
session-specific weekday numbering, multi-statement session state, or external
Fabric/Azure SQL execution.
