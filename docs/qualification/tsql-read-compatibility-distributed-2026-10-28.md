# T-SQL read compatibility: distributed qualification

## Scope

This record qualifies the bounded SQL Server/Fabric read corpus in
[`tsql-read-compatibility-corpus-2026-10-28.json`](tsql-read-compatibility-corpus-2026-10-28.json).
It is Engine execution evidence, not a claim of general Fabric Warehouse
parity or external-client parity.

## Evidence

- Corpus: 103 cases, including exact local/three-worker qualification for
  bounded derived-source `PIVOT`, `UNPIVOT`, `CROSS APPLY`, and uncorrelated
  derived `OUTER APPLY` lowerings and explicit fail-closed cases for unbounded
  `PIVOT`, `UNPIVOT`,
  table-valued/unsupported correlated `APPLY`, `TOP WITH TIES` without an order,
  and runtime-column styled timestamp conversion across local and three-worker
  execution.
- Planner gate: `versioned_tsql_corpus_matches_declared_boundaries` accepts
  every `planner_qualified` case and rejects every `explicit_rejection` case.
- Distributed gate: the in-process three-worker differential sweep completed
  with exact local-vs-distributed results for the 103-case corpus, including
  row-local `STRING_SPLIT` and `ROUND` operation mode, without worker failures
  or retained exchange/spill files.
- The corpus-level rerun after adding the `ISNUMERIC` distributed shape
  completed 1/1 with exact results in 729.75 seconds; the full differential
  workload completed without worker or spill failures.
- Focused distributed regression `distributed_tsql_left_and_right_match_local_unicode_semantics`
  passed with three workers and zero retained memory reservations.
- Qualified additions in this corpus revision: `COUNT_BIG`, `LEN`, `IIF`,
  `CONCAT` with NULL-as-empty arguments, `CONCAT_WS` with NULL-skipping arguments,
  `CHOOSE`, literal `CONVERT(DATE, …, style)` for styles `0`, `101`, and `112`,
  `CHARINDEX`, bounded `PATINDEX`,
  `TRANSLATE`, bounded width-one
  `DATE_BUCKET`, `EOMONTH`, `DATEFROMPARTS`, `DATETIMEFROMPARTS`, `DATETIME2FROMPARTS` (precision 0–6), `SMALLDATETIMEFROMPARTS`, `DATENAME`, `ISDATE`, `ISNUMERIC`,
  `TIMEFROMPARTS` (precision 0–6), bounded UTC-normalized
  `DATETIMEOFFSETFROMPARTS` (precision 0–6), `DATEPART`/`DATENAME` weekday,
  day-of-year, and ISO-week aliases, `DATENAME` day-of-year aliases,
  `DATEPART` week/quarter/subsecond aliases, day-equivalent `DATEADD`
  weekday/day-of-year aliases, `LEFT`,
  `RIGHT`, `REVERSE`, `TRIM`, `LTRIM`, and `RTRIM`.
  The current revision also qualifies bounded SQL Server-compatible scalar
  math (`ABS`, `SIGN`, `SQRT`, and `POWER`) and text (`UPPER`, `LOWER`, and
  `REPLACE`) expressions through the existing vectorized evaluator.

The bounded `PIVOT`/`UNPIVOT` evidence is retained in
[`track2-tsql-pivot-unpivot-distributed-2026-10-28.md`](track2-tsql-pivot-unpivot-distributed-2026-10-28.md).

Bounded `CROSS APPLY` over a parenthesized `SELECT` or `VALUES` relation is
lowered to the Engine's existing lateral-join path. Exact local and
three-worker evidence is recorded in
[`track2-tsql-apply-distributed-2026-10-28.md`](track2-tsql-apply-distributed-2026-10-28.md).
The qualified correlated shapes are limited to scalar `VALUES` expressions
that bind outer columns, for both `CROSS APPLY` and `OUTER APPLY`; table-valued
functions and arbitrary correlated derived `SELECT` forms remain fail-closed.

`ISDATE` returns a non-null boolean 0/1 result. Text is parsed through a safe
typed date cast; numeric values are rejected as date inputs instead of being
interpreted as epoch offsets.

`ISNUMERIC` returns a non-null boolean 0/1 result. Native numeric values and
text accepted by the safe numeric cast are qualified; locale-specific currency
and SQL Server type-coercion extensions remain outside the contract.

`STRING_SPLIT` also accepts scalar row-local string and separator expressions
through `CROSS JOIN LATERAL` or bounded `CROSS APPLY`; literal ordinal mode and the value/ordinal alias
contract remain bounded. The focused regression is
`distributed_tsql_string_split_matches_local`.

`NULLIF` is covered as a row-local scalar expression in the same exact
local-vs-three-worker differential sweep.

`ROUND` is qualified for the SQL Server third `operation` argument when it is
`0` (round) or `1` (truncate toward zero), including row-local integer values;
invalid operation values fail closed.

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
Runtime text-column timestamp conversion has local vectorized evaluator coverage
for ISO styles `20`, `21`, `120`, `121`, and `126`, plus date-only style `101`.
Styles `20`, `21`, `120`, `121`, `126`, and `101` are now included in
`distributed_tsql_runtime_timestamp_styles_match_local`, which compares exact
local and three-worker results.
`DATE_BUCKET` is qualified for literal and bounded row-local BIGINT widths over
year/quarter/month/day/hour/minute/second buckets. Widths are checked at
runtime: zero, negative, and NULL values produce typed NULL rather than a
divide-by-zero or approximation. Literal DATE/TIMESTAMP origins and bounded
row-local DATE/TIMESTAMP origins are qualified; unsupported origin shapes
remain rejected rather than approximated.

## Contract

The qualified surface is deliberately bounded. `DATENAME` returns deterministic
English calendar names (and textual numeric parts); locale/session-language
formatting is not part of the contract. `EOMONTH` and `DATEFROMPARTS` preserve
typed date semantics and fail closed on invalid input. `TOP PERCENT` and
`TOP WITH TIES` remain qualified only where the ordering contract is
deterministic.

External Fabric/Azure SQL sources continue to use the platform passthrough
path. They are not executed by the Engine and are not covered by this record.
