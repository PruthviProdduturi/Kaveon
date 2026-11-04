# T-SQL read compatibility: distributed qualification

## Scope

This record qualifies the bounded SQL Server/Fabric read corpus in
[`tsql-read-compatibility-corpus-2026-10-28.json`](tsql-read-compatibility-corpus-2026-10-28.json).
It is Engine execution evidence, not a claim of general Fabric Warehouse
parity or external-client parity.

## Evidence

The sweep-count bullets below are retained as dated evidence for the corpus
revision that existed when each run was executed. They are historical run
sizes, not the current corpus total; the authoritative current total is the
187-case count in the scope and tracker, enforced by
`scripts/validate-tsql-compatibility-tracker.py`.

- Historical corpus at this qualification: 119 cases. The prior full sweep qualified bounded
  derived-source `PIVOT`, `UNPIVOT`, `CROSS APPLY`, and uncorrelated derived
  `OUTER APPLY` lowerings and explicit fail-closed cases for unbounded `PIVOT`,
  `UNPIVOT`, table-valued/unsupported correlated `APPLY`, `TOP WITH TIES`
  without an order, and runtime-column styled timestamp conversion across local
  and three-worker execution. The newly added `format_numeric` and
  `date_bucket_subsecond_timestamp` cases have their own exact local/three-worker
  differential qualifications below.
- Planner gate: `versioned_tsql_corpus_matches_declared_boundaries` accepts
  every `planner_qualified` case and rejects every `explicit_rejection` case.
- Distributed gate: the in-process three-worker differential sweep completed
  with exact local-vs-distributed results for the full corpus, including
  row-local `STRING_SPLIT` and `ROUND` operation mode, without worker failures
  or retained exchange/spill files. The revision also includes bounded
  `PIVOT(COUNT(*))`, `PIVOT(COUNT_BIG(*))`, and expression-valued
  `PIVOT(COUNT_BIG(value))` over the explicit derived-source contract and
  `COUNT_BIG(DISTINCT ...)` through the exact distinct-count path. The
  unordered `STRING_AGG(...) FILTER (WHERE ...)` shape lowers to the
  NULL-preserving input path and passes the same exact local/three-worker gate.
  Expression
  forms preserve SQL NULL-counting semantics.
- The SQL Server pagination case `ORDER BY ... OFFSET 1 ROWS FETCH NEXT 2
  ROWS ONLY` is included in the differential sweep and matches local results
  across the supported layouts and three-worker execution. It requires an
  explicit deterministic ordering key; conflicting pagination bounds remain
  rejected before execution.
- The corpus-level rerun after adding the `ISNUMERIC` distributed shape
  completed 1/1 with exact results in 729.75 seconds; the full differential
  workload completed without worker or spill failures.
- A complete 113-case differential sweep was rerun on 2026-10-30 from the
  current Engine source and completed 1/1 with exact local/layout/three-worker
  results in 642.30 seconds. The run included the qualified filtered
  `STRING_AGG`, deterministic `TOP` row limiting, and bracket-quoted
  identifier cases; it reported no worker failures or retained exchange/spill
  files.
- The complete 113-case differential sweep was rerun on 2026-11-02 after the
  ordered derived-source PIVOT/UNPIVOT qualification. It completed 1/1 with
  exact local/layout/three-worker results in 680.25 seconds, with no worker
  failures or retained exchange/spill files. Ordered cases use total ordering
  keys; tied rows remain governed by normal SQL ordering semantics.
- The complete 114-case differential sweep was rerun after adding
  `CHARINDEX(..., start_location)` and completed 1/1 with exact
  local/layout/three-worker results in 670.03 seconds, with no worker
  failures or retained exchange/spill files.
- The complete 115-case differential sweep was rerun after adding bounded
  Unicode-safe `STUFF` and completed 1/1 with exact local/layout/three-worker
  results in 640.23 seconds, with no worker failures or retained exchange/spill
  files.
- The complete 117-case differential sweep was rerun after adding bounded
  SQL Server `SPACE` with row-local counts and negative-count NULL semantics;
  it completed 1/1 with exact local/layout/three-worker results in 634.13
  seconds, with no worker failures or retained exchange/spill files.
- A dated 152-case differential sweep was rerun on 2026-11-03 from
  Engine commit `07bc8c47` and completed 1/1 with exact local/layout/
  three-worker results in 661.92 seconds. No worker failures or retained
  exchange/spill files were reported. This rerun contains no new SQL
  semantics relative to the corpus implementation pin; it requalifies the
  current tree after diagnostics changes.
- Focused invariant temporal `TRY_PARSE` qualification covers DATE inputs and
  mixed valid/invalid row-local values with exact local/three-worker results;
  culture-aware `USING` forms remain explicitly rejected.
- Focused `ISJSON` validity and `VALUE`/`ARRAY`/`OBJECT`/`SCALAR` constraint
  execution passed through the same differential layouts and three-worker
  path (`KAVEON_DIFF_ONLY=tsql_isjson_constraints`).
- Focused distributed regression `distributed_tsql_left_and_right_match_local_unicode_semantics`
  passed with three workers and zero retained memory reservations.
- Qualified additions in this corpus revision: `COUNT_BIG`, `LEN`, `IIF`,
  `CONCAT` with NULL-as-empty arguments, `CONCAT_WS` with NULL-skipping arguments,
  `CHOOSE`, literal `CONVERT(DATE, …, style)` for styles `0`, `101`, and `112`,
  `CHARINDEX` including its optional one-based start location, `STUFF` with
  Unicode scalar positions and NULL propagation, `SPACE` with bounded
  row-local counts and negative-count NULL semantics, bounded `PATINDEX`,
  `TRANSLATE`, bounded width-one
  `DATE_BUCKET`, `EOMONTH`, `DATEFROMPARTS`, `DATETIMEFROMPARTS`, `DATETIME2FROMPARTS` (precision 0–6), `SMALLDATETIMEFROMPARTS`, `DATENAME`, `ISDATE`, `ISNUMERIC`,
  `TIMEFROMPARTS` (precision 0–6), bounded UTC-normalized
  `DATETIMEOFFSETFROMPARTS` (precision 0–6), `DATEPART`/`DATENAME` weekday,
  day-of-year, and ISO-week aliases, `DATENAME` day-of-year aliases,
  `DATEPART` week/quarter/subsecond aliases, day-equivalent `DATEADD`
  weekday/day-of-year aliases, `LEFT`,
  `RIGHT`, `REVERSE`, `TRIM`, `LTRIM`, and `RTRIM`.
  The current revision also qualifies bounded SQL Server-compatible scalar
  math (`ABS`, `CEILING`, `FLOOR`, `SIGN`, `SQRT`, and `POWER`) and text (`UPPER`, `LOWER`, and
  `REPLACE`) expressions through the existing vectorized evaluator.
  `SUBSTRING` now lowers from the SQL parser's dedicated AST form to the same
  Unicode-safe vectorized substring evaluator.
  The new bounded numeric `FORMAT` case (`N2`/`F1`) also matches exactly
  between local and three-worker execution; unsupported culture and arbitrary
  .NET patterns remain fail-closed. See
  [`track2-tsql-format-numeric-2026-11-02.md`](track2-tsql-format-numeric-2026-11-02.md).
Millisecond and microsecond `DATE_BUCKET` timestamp inputs also match exactly
between local and three-worker execution; nanosecond input remains an explicit
rejection. See
[`track2-tsql-date-bucket-subsecond-2026-11-02.md`](track2-tsql-date-bucket-subsecond-2026-11-02.md).

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
`EOMONTH` is qualified for DATE/TIMESTAMP inputs with zero, literal, and
row-local month offsets; the result remains a typed DATE. `DATE_BUCKET` is
qualified for literal and bounded row-local BIGINT widths over
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
