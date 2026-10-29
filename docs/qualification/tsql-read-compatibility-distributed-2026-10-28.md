# T-SQL read compatibility: distributed qualification

## Scope

This record qualifies the bounded SQL Server/Fabric read corpus in
[`tsql-read-compatibility-corpus-2026-10-28.json`](tsql-read-compatibility-corpus-2026-10-28.json).
It is Engine execution evidence, not a claim of general Fabric Warehouse
parity or external-client parity.

## Evidence

- Corpus: 63 cases, including exact local/three-worker qualification for
  bounded derived-source `PIVOT`, `UNPIVOT`, `CROSS APPLY`, and uncorrelated
  derived `OUTER APPLY` lowerings and explicit fail-closed cases for unbounded
  `PIVOT`, `UNPIVOT`,
  table-valued/correlated `APPLY`, `TOP WITH TIES` without an order, and
  runtime-column styled date conversion.
- Planner gate: `versioned_tsql_corpus_matches_declared_boundaries` accepts
  every `planner_qualified` case and rejects every `explicit_rejection` case.
- Distributed gate: the in-process three-worker differential sweep completed
  with exact local-vs-distributed results after the `DATENAME` implementation;
  the run completed without worker failures or retained exchange/spill files.
- The corpus-level rerun after adding the `ISNUMERIC` distributed shape
  completed 1/1 with exact results in 729.75 seconds; the full differential
  workload completed without worker or spill failures.
- Focused distributed regression `distributed_tsql_left_and_right_match_local_unicode_semantics`
  passed with three workers and zero retained memory reservations.
- Qualified additions in this corpus revision: `COUNT_BIG`, `LEN`, `IIF`,
  `CHARINDEX`, bounded `PATINDEX`, `TRANSLATE`, bounded width-one
  `DATE_BUCKET`, `EOMONTH`, `DATEFROMPARTS`, `DATETIMEFROMPARTS`, `DATETIME2FROMPARTS` (precision 0–3), `SMALLDATETIMEFROMPARTS`, `DATENAME`, `ISDATE`, `ISNUMERIC`,
  `TIMEFROMPARTS` (precision 0–6), bounded UTC-normalized
  `DATETIMEOFFSETFROMPARTS` (precision 0–6), `DATEPART`/`DATENAME` weekday,
  day-of-year, and ISO-week aliases, `DATENAME` day-of-year aliases,
  `DATEPART` week/quarter/subsecond aliases, day-equivalent `DATEADD`
  weekday/day-of-year aliases, `LEFT`,
  `RIGHT`, `REVERSE`, `TRIM`, `LTRIM`, and `RTRIM`.

The bounded `PIVOT`/`UNPIVOT` evidence is retained in
[`track2-tsql-pivot-unpivot-distributed-2026-10-28.md`](track2-tsql-pivot-unpivot-distributed-2026-10-28.md).

Bounded `CROSS APPLY` over a parenthesized `SELECT` or `VALUES` relation is
lowered to the Engine's existing lateral-join path. Exact local and
three-worker evidence is recorded in
[`track2-tsql-apply-distributed-2026-10-28.md`](track2-tsql-apply-distributed-2026-10-28.md).

`ISDATE` returns a non-null boolean 0/1 result. Text is parsed through a safe
typed date cast; numeric values are rejected as date inputs instead of being
interpreted as epoch offsets.

`ISNUMERIC` returns a non-null boolean 0/1 result. Native numeric values and
text accepted by the safe numeric cast are qualified; locale-specific currency
and SQL Server type-coercion extensions remain outside the contract.

`LEFT` and `RIGHT` return bounded Unicode scalar prefixes/suffixes, preserve
NULLs, and use the Engine's width-aware text path for narrow and large UTF-8
values.
`REVERSE` reverses Unicode scalar values rather than bytes and shares the same
width-aware execution path.
`TRIM`, `LTRIM`, and `RTRIM` preserve NULLs and trim ASCII spaces at the
documented boundaries; broader culture-specific trimming is not claimed.
`TRANSLATE` maps Unicode scalar characters using SQL Server's first-match
mapping rule; extra source characters are removed and extra target characters
are ignored, with NULL propagation.
`PATINDEX` reports one-based Unicode scalar positions for `%` and `_`
wildcards, returns zero for no match, preserves NULLs, and rejects bracket
classes explicitly.
`DATE_BUCKET` is qualified for literal width-one year/quarter/month/day/hour/
minute/second buckets and lowers to typed `DATE_TRUNC`; wider widths and
custom origins are rejected rather than approximated.

## Contract

The qualified surface is deliberately bounded. `DATENAME` returns deterministic
English calendar names (and textual numeric parts); locale/session-language
formatting is not part of the contract. `EOMONTH` and `DATEFROMPARTS` preserve
typed date semantics and fail closed on invalid input. `TOP PERCENT` and
`TOP WITH TIES` remain qualified only where the ordering contract is
deterministic.

External Fabric/Azure SQL sources continue to use the platform passthrough
path. They are not executed by the Engine and are not covered by this record.
