# T-SQL default read-session prefix qualification — 2026-11-03

## Contract

Kaveon accepts common SQL Server read-session prefixes:

```sql
SET DATEFIRST 7;
SELECT DATEPART(dw, event_date) FROM events;
```

The supported default forms are `SET ANSI_NULLS ON`, `SET
QUOTED_IDENTIFIER ON`, `SET ANSI_WARNINGS ON`, `SET NOCOUNT ON`, and
`SET DATEFIRST 7`. They are removed before planning because they do not
change the Engine's fixed read semantics. `7` means Sunday is 1 and Saturday
is 7. Session-specific values remain fail-closed; the Engine does not yet
propagate mutable session state through distributed workers.

## Evidence

- `parser::tests::accepts_only_default_tsql_read_session_prefixes`
- `differential_tests::distributed_tsql_default_read_session_prefixes_match_local`
- The case `set_default_read_session_prefixes` is in the versioned 162-case
  corpus.
- The companion `set_datefirst_is_explicitly_unsupported` case continues to
  reject `SET DATEFIRST 1`, and `SET NOCOUNT OFF` is also rejected, both with
  actionable diagnostics.

## Boundary

This qualifies only the listed default no-op prefixes. It does not claim
support for session-specific settings, arbitrary multi-statement session
state, or external Fabric/Azure SQL execution.
