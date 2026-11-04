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
is 7. Bounded `SET DATEFIRST 1` through `7` is qualified separately in
[Custom `DATEFIRST` read qualification](track2-tsql-datefirst-custom-2026-11-03.md),
where the selected value is carried immutably through local and distributed
planning.

## Evidence

- `parser::tests::accepts_only_default_tsql_read_session_prefixes`
- `differential_tests::distributed_tsql_default_read_session_prefixes_match_local`
- The case `set_default_read_session_prefixes` remains in the authoritative
  versioned corpus.
- `SET NOCOUNT OFF` remains rejected with an actionable diagnostic. The older
  `set_datefirst_is_explicitly_unsupported` wording is superseded by the
  bounded custom-`DATEFIRST` qualification linked above.

## Boundary

This qualifies only the listed default no-op prefixes plus the separately
qualified bounded `DATEFIRST` read prefix. It does not claim support for
arbitrary mutable session state, locale settings, or external Fabric/Azure
SQL execution.
