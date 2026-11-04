# SQL compatibility gap plan

This is the implementation tracker for closing the gap between Kaveon's
current analytical SQL surface and the read, transactional, and operational
capabilities users commonly expect from Fabric Warehouse, Azure SQL,
PostgreSQL, and Trino.

It is deliberately evidence-based: a capability moves to **qualified** only
after parser, planner, local execution, distributed execution (where
applicable), and result-correctness tests pass. Documentation or parser
acceptance alone is not a parity claim.

## Current boundary

Kaveon Engine is a distributed, columnar lake query engine with a bounded
T-SQL compatibility layer. Fabric/Azure SQL registered sources are API
passthrough sources; they are not executed by Kaveon Engine and cannot be
joined with Engine catalogs in one query today. KaveonDB transactions are a
bounded product-record contract, not a PostgreSQL wire-compatible row store.
The native local profile exercises KaveonDB without PostgreSQL, but the current
API source audit still finds active PostgreSQL-backed metadata call sites in
the cloud migration path. Therefore “KaveonDB-native” is a qualified profile,
not evidence that every deployed API authority has already cut over.

The authoritative capability details remain in:

- [Engine SQL compatibility](../reference/engine-sql-compatibility.md)
- [Kaveon–Trino parity tracker](trino-parity-tracker.md)
- [Connector capabilities](../reference/connector-capabilities.md)
- [Fabric comparison](../research/kaveon-engine-vs-fabric-sql-analytics-endpoint.md)

The current PostgreSQL/Fabric authority-language audit is recorded in
[`track0-current-authority-language-audit-2026-11-03.md`](../qualification/track0-current-authority-language-audit-2026-11-03.md).

## Closure order

### Phase 1 — T-SQL/Fabric read compatibility

- [x] Create a versioned T-SQL corpus covering `TOP`, bracket identifiers,
  `DATEADD`, `DATEDIFF`, `DATEPART`, `CONVERT`, `TRY_CONVERT`, `ISNULL`,
  `GETDATE`, `SYSDATETIME`, bounded `DATE_BUCKET`, `PIVOT`, `UNPIVOT`, and `CROSS APPLY`,
  explicit unsupported APPLY forms, and SQL Server pagination.
- [x] Classify every case as accepted, translated, or fail-closed with an
  actionable diagnostic.
- [x] Add exact local and distributed result tests for accepted translations.
  The current 199-case corpus has planner coverage and focused exact
  three-worker differential evidence; the versioned corpus and distributed
  evidence are retained in
  `docs/qualification/tsql-read-compatibility-distributed-2026-10-28.md`,
  with bounded fixed-offset `AT TIME ZONE` evidence in
  `docs/qualification/track2-tsql-at-time-zone-2026-11-03.md`.
- [x] Keep unbounded `PIVOT`, `UNPIVOT`, and unsupported `APPLY` forms rejected
  until their semantics are implemented and qualified. Bounded derived-source
  `PIVOT`, `UNPIVOT`, and `CROSS APPLY` lower to conditional aggregation,
  `UNION ALL`, or the existing lateral path and are qualified across local
  layouts and three-worker fragments; see the corresponding qualification
  records under `docs/qualification/`. Unbounded `PIVOT`/`UNPIVOT`, arbitrary
  correlated APPLY, and table-valued APPLY remain explicitly rejected.
  bounded derived-source `OUTER APPLY` is also qualified through the existing
  left-lateral path, including NULL extension. Unbounded table-valued APPLY,
  arbitrary correlated APPLY, and unbounded PIVOT/UNPIVOT remain explicitly
  rejected. `TOP PERCENT` and `TOP WITH
  TIES` are qualified only when their `ORDER BY` keys define a deterministic
  order.
- [x] Add Fabric/Azure SQL passthrough routing tests without implying Engine
  execution. Native Engine catalog resolution and Fabric/Azure SQL passthrough
  separation are qualified in
  `docs/qualification/track1-fabric-passthrough-routing-2026-10-29.md`.
- [x] Preserve Arrow `Utf8View` text through aggregate grouping, statistics
  metadata, chart/dashboard cube materialization, client-facing type labels,
  and JSON result serialization so chart dimensions and text-key consumers
  receive logical strings. Evidence:
  `docs/qualification/track5-utf8view-result-serialization-2026-11-02.md`,
  `docs/qualification/track1-utf8view-partition-catalog-2026-11-03.md`,
  `docs/qualification/track3-utf8view-statistics-2026-11-03.md`,
  `docs/qualification/track3-utf8view-cubes-2026-11-03.md`, and
  `docs/qualification/track4-utf8view-text-routing-2026-11-03.md`, and
  `docs/qualification/track3-utf8view-predicate-pushdown-2026-11-03.md`, and
  `docs/qualification/track2-utf8view-expression-text-2026-11-03.md`, and
  `docs/qualification/track2-utf8view-regex-functions-2026-11-03.md`, and
  `docs/qualification/track2-utf8view-array-functions-2026-11-03.md`, and
  `docs/qualification/track1-utf8view-distinct-2026-11-03.md`.
- [x] Enforce the versioned corpus as a release gate. The Engine workflow
  validates the corpus count, case IDs, status fields, repository commit pin,
  and tracker synchronization before Rust compilation; changes to the tracker
  or corpus also trigger that workflow.

Progress: the Engine now lowers bounded `TOP n PERCENT`, `TOP n WITH TIES`, `ISNULL`, `CONCAT`, `CONCAT_WS`, `CHOOSE`, `STUFF`, `SPACE`, character-set `TRIM` in BOTH/LEADING/TRAILING modes, `DATEADD`, `DATEDIFF`, `DATEPART`, and common SQL Server clock functions,
unstyled `CONVERT`, and unstyled `TRY_CONVERT`, including common SQL Server
datepart abbreviations. Runtime DATE and timestamp styles have qualified local
and three-worker coverage for the declared style set. Literal DATE styles `0`,
`1`, `23`, `101`, `102`, `103`, `104`, `105`, `106`, `107`, `110`, `111`,
`112`, `20`, `21`, `120`, `121`, and `126` now normalize to ISO values.
`ISNULL`, `CONCAT`, `CONCAT_WS`, `CHOOSE`, `DATEADD`, `DATEDIFF`, and `DATEPART` now
pass exact local-vs-three-worker differential checks across dictionary, plain,
partitioned, clustered, and fragmented Parquet layouts. `TOP PERCENT` and
The T-SQL `DATEPART(dw|weekday|w, ...)` slice uses the explicit default
`DATEFIRST 7` numbering (Sunday=1 through Saturday=7) in local and
three-worker execution. The common default read-session prefixes `SET
ANSI_NULLS ON`, `SET QUOTED_IDENTIFIER ON`, `SET ANSI_WARNINGS ON`, `SET
NOCOUNT ON`, and `SET DATEFIRST 1..7;` are accepted as semantics-preserving
read prefixes, with the selected first weekday lowered into the immutable
distributed plan; evidence is in
`docs/qualification/track2-tsql-datefirst-default-2026-11-03.md` and
`docs/qualification/track2-tsql-datepart-weekday-2026-11-03.md`.
`TOP WITH TIES` also pass that sweep when their order keys are deterministic;
without a complete tie-breaker, SQL permits different tied rows and the
qualification is intentionally withheld. Numeric `CONVERT`, `TRY_CONVERT`,
and `TRY_CAST` casts are also qualified across those layouts, including
bounded `DECIMAL(p,s)` safe casts that return typed NULL on malformed or
overflowing input; strict decimal conversion preserves statement errors.
Evidence is recorded in
`docs/qualification/track2-tsql-try-convert-decimal-2026-11-04.md` and
`docs/qualification/track2-tsql-try-cast-decimal-2026-11-04.md`. Unstyled
fixed-offset `DATETIMEOFFSET` inputs; strict `CONVERT(DATETIMEOFFSET, ...)`
uses the same parser and fails closed on malformed non-NULL values. SQL Server `COUNT_BIG`
now canonicalizes to the Engine's mergeable 64-bit `COUNT` state; it shares
the existing distributed count semantics and does not claim a separate
SQL Server numeric type. `LEN` is qualified with SQL Server's trailing-space
rule, `IIF` uses the existing vectorized conditional path, and `CHARINDEX`
uses a bounded Unicode scalar search with SQL Server's argument order.
`EOMONTH` now lowers to typed month truncation and calendar arithmetic and is
qualified for date/timestamp inputs with an optional row-local month offset.
`DATETRUNC` now canonicalizes to the same typed `DATE_TRUNC` path for the
qualified SQL Server datepart/input boundary; evidence is retained in
`docs/qualification/track2-tsql-datetrunc-2026-10-30.md`.
SQL Server `STRING_AGG(value, literal_separator)` and bounded SQL-standard/Trino
`LISTAGG(value[, literal_separator])` now carry a mergeable
distributed string state with NULL omission. The bounded
`WITHIN GROUP (ORDER BY value [ASC|DESC])` form is also qualified; independent
or multiple order keys, explicit NULL placement, dynamic separators, and
unbounded ordered state remain rejected. Evidence is retained in
`docs/qualification/track2-tsql-string-agg-within-group-2026-11-02.md`.
`DATEFROMPARTS` now builds validated `DATE` values from integer parts with
NULL propagation and distributed differential coverage.
`DATENAME` now returns deterministic English calendar names or textual numeric
parts and is covered by the distributed differential sweep. Millisecond,
microsecond, and nanosecond parts over typed timestamps are separately
qualified in `docs/qualification/track2-tsql-datename-subsecond-2026-11-04.md`;
the nanosecond value is the microsecond execution value scaled by 1,000, not a
claim of 100-nanosecond storage precision. Locale-specific formatting remains
outside the contract. The complete bounded corpus and its distributed evidence are recorded in
`docs/qualification/tsql-read-compatibility-distributed-2026-10-28.md`.
SQL Server `CONVERT`/`TRY_CONVERT` to `TIME` also qualify styles `8`/`108`
(`HH:MM:SS`) and `114` (`HH:MM:SS:mmm`) with strict-error and safe-NULL
behavior; other time styles remain explicitly rejected. Evidence is in
`docs/qualification/track2-tsql-convert-time-styles-2026-11-03.md`.
Fixed-duration SQL Server `DATEADD` over microsecond `TIME` values now wraps
at midnight for hour through microsecond units, with local and three-worker
differential evidence in
`docs/qualification/track2-tsql-time-dateadd-2026-11-03.md`.
`ISDATE` now returns a safe boolean validity result for text and temporal
inputs, explicitly refusing numeric epoch coercion.
`ISNUMERIC` now returns a safe boolean result for native numeric and
parseable-text inputs without claiming locale-specific currency parsing.
`PATINDEX` now supports bounded `%` and `_` wildcard matching with Unicode
scalar positions, NULL propagation, and explicit rejection of bracket classes;
the local and three-worker evidence is retained in
`docs/qualification/track2-tsql-patindex-2026-10-28.md`.
Bracket-quoted identifiers remain identifiers in `DISTINCT` projections and
ordered pagination shapes; the local/three-worker evidence across Parquet
encodings is retained in
`docs/qualification/track2-tsql-bracket-identifiers-2026-10-29.md`.
`QUOTENAME` now supports the bounded SQL Server identifier quoting surface
(square brackets and double quotes, 128-Unicode-character limit) with local
and three-worker evidence in
`docs/qualification/track2-tsql-quotename-2026-10-29.md`.
`REPLICATE` now canonicalizes to the bounded Unicode `REPEAT` path for
non-negative literals and bounded row-local BIGINT expressions, returning NULL
for negative counts; it is included in the distributed corpus.
`LEFT` and `RIGHT` preserve SQL Server's Unicode scalar character semantics
for bounded text lengths and are covered by the distributed differential
regression.
`REVERSE` preserves Unicode scalar order for text values and is covered by the
same distributed differential path.
`TRIM`, `LTRIM`, and `RTRIM` preserve NULLs and remove only the documented
ASCII-space boundaries on the width-aware text path. Character-set `TRIM`
now qualifies BOTH, LEADING, and TRAILING forms with local and three-worker
differential evidence in `docs/qualification/track2-tsql-trim-characters-2026-10-29.md`
and `docs/qualification/track2-tsql-trim-directional-characters-2026-10-29.md`.
`TRANSLATE` now has a bounded SQL Server-compatible Unicode scalar mapping
path, including NULL propagation and first-match/extra-character semantics,
and is covered by the versioned planner corpus and three-worker differential
gate. `DATE_BUCKET` buckets for year, quarter, month, week, day, hour, minute,
second, millisecond, and microsecond lower to typed calendar arithmetic with
widths normalized to the arithmetic unit (for example, two weeks is fourteen
days and two years is twenty-four months). Hour and minute buckets use the
corresponding seconds offset, and typed custom origins are advanced by the
whole bucket offset. Calendar buckets also normalize typed TIMESTAMP origins
to the DATE arithmetic domain, while time-of-day buckets retain timestamp
precision. Exact local/distributed coverage is recorded in
`docs/qualification/track2-tsql-date-bucket-week-2026-10-30.md` and
`docs/qualification/track2-tsql-date-bucket-width-origin-2026-11-03.md`;
row-local and typed-literal custom origins are qualified, while unsupported
origin shapes and dateparts remain explicitly rejected. Row-local BIGINT
widths are guarded at runtime: zero, negative, or NULL widths return typed
NULL.
SQL Server `FORMAT` now lowers to the bounded temporal formatter for typed
DATE/TIMESTAMP values. Invariant patterns cover the common `yyyy`, `MM`, `dd`,
`HH`, `mm`, `ss`, and fractional-second tokens. It also accepts bounded
invariant numeric patterns (`N0`–`N6`, `F0`–`F6`, and fixed decimal patterns);
culture-specific overloads remain rejected. The same evaluator now honors the
supplied pattern for `DATE_FORMAT`/`TO_CHAR`, with exact local and three-worker
evidence in `docs/qualification/track2-tsql-format-temporal-2026-10-29.md` and
`docs/qualification/track2-tsql-format-numeric-2026-11-02.md`.
Subsecond timestamp buckets have exact local/three-worker evidence in
`docs/qualification/track2-tsql-date-bucket-subsecond-2026-11-02.md`;
nanosecond bucket inputs remain rejected because the execution timestamp
contract is microsecond precision.
Runtime `CONVERT`/`TRY_CONVERT` timestamp inputs qualify the implemented
date-only styles `0`, `1`, `23`, `101`, `102`, `103`, `104`, `105`, `106`,
`107`, `110`, `111`, and `112`, producing midnight timestamps, together with
SQL Server style `113` (`dd mon yyyy hh:mi:ss:mmm`) using invariant English
month abbreviations and preserving milliseconds, and the ISO timestamp styles
`20`, `21`, `120`, `121`, and `126` (fractional seconds bounded to the
Engine's microsecond timestamp contract). Malformed
and NULL inputs preserve `TRY_CONVERT` NULL-on-parse-failure behavior; the
same parser rejects unsupported style codes rather than guessing. Evidence:
`docs/qualification/track2-tsql-runtime-timestamp-date-styles-2026-11-03.md`.

`DATALENGTH` is qualified for text inputs as a nullable UTF-8 byte count,
including trailing spaces, across dictionary, plain, fragmented,
partitioned, clustered, and three-worker execution. This is explicitly the
Engine UTF-8 contract, not SQL Server UTF-16 `nvarchar` storage emulation;
evidence is retained in `docs/qualification/track2-tsql-datalength-2026-11-02.md`.
`ISNULL` is also preserved as a distinct two-argument SQL Server function with
first-argument type semantics and exact local/three-worker evidence in
`docs/qualification/track2-tsql-isnull-contract-2026-11-02.md`.

The common SQL Server `YEAR(value)`, `MONTH(value)`, and `DAY(value)` aliases
now lower to the existing typed `DATE_PART` path and pass the full local and
three-worker differential sweep. Evidence:
`docs/qualification/track2-tsql-date-part-aliases-2026-11-03.md`.

`ASCII`, `UNICODE`, and bounded `CHAR` are also qualified: ASCII returns the
leading ASCII scalar and NULL for a non-ASCII leading scalar, UNICODE returns
the leading Unicode scalar, and CHAR accepts only integer byte values 0..255,
returning NULL outside that range. This is an explicit invariant UTF-8/byte
contract, not locale/code-page emulation; local and three-worker evidence is
recorded in `docs/qualification/track2-tsql-character-code-2026-10-29.md`.
`NCHAR` now accepts valid Unicode scalar values with NULL propagation and
rejects NULL, surrogate, negative, and out-of-range code points; it has the
same local and three-worker qualification boundary.
`NULLIF` is qualified for row-local scalar expressions through the existing
vectorized null-preserving conditional path, with exact local and three-worker
evidence in `docs/qualification/track2-tsql-nullif-2026-11-03.md`.
SQL Server `ROUND` now accepts the bounded third `operation` argument (`0` for
rounding and `1` for truncation toward zero), including row-local integer
expressions; other operation values fail closed.
`TRY_PARSE(value AS type)` now lowers to the existing invariant `TRY_CAST`
path for numeric and bounded DATE, TIME, DATETIME, SMALLDATETIME, DATETIME2, and fixed-offset
DATETIMEOFFSET targets, with exact local/three-worker evidence in
`docs/qualification/track2-tsql-try-parse-2026-10-29.md` and
`docs/qualification/track2-tsql-try-parse-time-2026-11-03.md`; culture-aware
`USING` forms remain explicitly rejected.
Strict `PARSE(value AS type)` now uses the same invariant target boundary with
statement-failing cast semantics; its local and three-worker qualification is
recorded in `docs/qualification/track2-tsql-parse-invariant-2026-11-04.md`.
Direct SQL Server `TRY_CAST` DATE, DATETIME2, and fixed-offset DATETIMEOFFSET
targets now have exact local/three-worker evidence, including
NULL-on-invalid-text behavior; the
qualified read corpus records this as a separate temporal target boundary.
`SQUARE(value)` lowers to the existing vectorized `POWER(value, 2)` path with
exact local/three-worker evidence in
`docs/qualification/track2-tsql-square-2026-10-29.md`.
The bounded SQL Server scalar math aliases `ABS`, `CEILING`, `FLOOR`, `SIGN`,
and `SQRT`, together with `POWER`, and the Unicode text functions `UPPER`,
`LOWER`, and `REPLACE`
are now included in the exact local/three-worker corpus; unsupported parser
shapes remain explicitly rejected until their AST lowering is implemented.
`SUBSTRING` is now lowered from the dedicated SQL AST form and is qualified for
Unicode-safe bounded scalar inputs in the same corpus.
Bounded SQL Server `STRING_SPLIT` now lowers to the existing typed `SPLIT` and
`UNNEST` path for scalar inputs; optional literal ordinal mode `0`/`1`, alias
columns, row-local lateral inputs, the bounded `CROSS APPLY` spelling, and the lateral boundary are qualified in
`docs/qualification/track2-tsql-string-split-2026-10-29.md`.
Native SQL Server/Fabric `JSON_VALUE` and `JSON_QUERY` text forms are also
qualified, including scalar-versus-container NULL behavior and canonical
container rendering (`docs/qualification/track2-tsql-json-text-functions-2026-11-02.md`).
The bounded `PERCENTILE_CONT` and `PERCENTILE_DISC` window forms now qualify
one literal fraction, one numeric order key, partition-wide interpolation or
discrete selection, and NULL omission; dynamic or ambiguous ordered-set forms remain rejected
(`docs/qualification/track2-tsql-percentile-cont-2026-11-02.md`).

`GREATEST` and `LEAST` now have a bounded SQL Server/Fabric numeric read
qualification for non-null arguments, with exact local and three-worker
results. NULL-bearing arguments remain outside this cross-dialect claim because
SQL Server and Trino differ in their function-level NULL semantics; evidence is
recorded in `docs/qualification/track2-tsql-greatest-least-2026-11-03.md`.

SQL Server/Fabric `LOG`, two-argument `LOG`, `LOG10`, and `EXP` now qualify for
finite numeric scalar inputs with SQL NULL propagation and exact local/
three-worker differential evidence. Domain errors and non-finite results remain
fail-closed; external-source execution is not implied. Evidence:
`docs/qualification/track2-tsql-log-exp-2026-11-03.md`.

The same bounded numeric contract now qualifies SQL Server/Fabric `SIN`, `COS`,
`TAN`, `ASIN`, `ACOS`, `ATAN`, `DEGREES`, `RADIANS`, and `PI`, including
NULL-preserving local and three-worker results. Evidence:
`docs/qualification/track2-tsql-trigonometric-2026-11-03.md`.

SQL Server's `ATN2(y, x)` now maps to the existing typed two-argument
arctangent path and is qualified across local and three-worker execution;
`ATAN2` remains the generic Trino-compatible spelling. Evidence:
`docs/qualification/track2-tsql-atn2-2026-11-03.md`.

The bounded SQL Server `FORMATMESSAGE` literal-template form is qualified for
`%s`, `%d`, `%i`, `%u`, and `%%` substitutions with positional text or numeric
arguments. Signed `%i` and unsigned integer `%u` inputs are covered explicitly.
Its local and three-worker differential evidence is recorded in
`docs/qualification/track2-tsql-formatmessage-2026-11-03.md`; dynamic templates,
unsupported tokens, and unbounded output remain explicitly rejected.

### Phase 2 — Transactional SQL surface

- [x] Qualify bounded typed-row DML for KaveonDB-owned tables. The
  `kaveon.product.typed_rows` facade supports revisioned create/update/delete
  with owner isolation and replay-safe `ON CONFLICT DO NOTHING`, plus the
  bounded `ON CONFLICT (id) DO UPDATE SET document_json = excluded.document_json`
  form with table-identity preservation; the assignment may also use the
  direct typed spelling `CAST(excluded.document_json AS JSON)`. The same upsert is exercised through
  `/v1/transaction/sql` with `RETURNING id, revision` and a committed
  revision/CAS follow-up. Bounded `ON CONFLICT DO NOTHING RETURNING id,
  revision, document_json` is also qualified: duplicates return no rows and
  stage no write. Arbitrary typed-table schemas and conflict updates
  remain outside this contract. Evidence:
  `docs/qualification/track2-native-dml-typed-row-conflict-ignore-2026-10-28.md`.
- [x] Accept an unambiguous target alias on the bounded product `UPDATE` and
  `DELETE` contracts. The alias is syntactic only; revision/CAS predicates,
  owner isolation, assignments, and `RETURNING` semantics are unchanged.
  Joins and arbitrary lake-table mutation remain rejected. A separate bounded
  same-kind source-backed `UPDATE ... FROM` form is qualified below. Evidence is recorded in
  `docs/qualification/track2-native-dml-target-alias-2026-11-02.md`.
- [x] Accept the bounded SQL Server `OUTPUT inserted/deleted.column` and
  statement-wildcard spellings
  for product `INSERT`, `UPDATE`, and `DELETE`, lowering it to the existing
  revision/CAS result projection. Expressions, `OUTPUT INTO`, `MERGE OUTPUT`,
  mixed pseudo-tables, and mixed wildcard/column projections remain rejected.
  Wildcards expand only to `id`, `revision`, and `document_json`. Evidence:
  `docs/qualification/track2-tsql-output-2026-11-02.md`.
- [x] Qualify bounded product `UPDATE` and `DELETE` statements with an explicit
  `id IN (...)` list and one positive revision CAS predicate. The adapter
  expands at most 1,000 unique IDs into owner-isolated commands under one
  transaction; joins, `NOT IN`, and per-row revisions remain rejected. The
  separate bounded source-backed `UPDATE ... FROM` contract is documented
  below. Evidence:
  `docs/qualification/track2-native-dml-id-list-cas-2026-11-02.md`.
- [x] Add bounded source-backed product `INSERT … SELECT`. The qualified form
  copies `id` and `document_json` from one same-kind `kaveon.product` table
  under the pinned transaction snapshot, optionally narrowed by a literal
  `id IN (...)` list, and is capped at 1,000 source records,
  enforces owner isolation, and reuses the existing conflict/CAS path.
  `RETURNING id, revision, document_json` is qualified from the staged snapshot
  with or without the literal ID filter; the unfiltered form keeps the same
  1,000-row expansion cap. General lake-table source DML remains open.
  The document projection may also use the direct typed form
  `CAST(document_json AS JSON)`; arbitrary expressions remain rejected.
- [x] Add bounded source-backed product `UPDATE ... FROM`. One same-kind,
  aliased product source table can replace one or more target `document_json`
  values under a pinned snapshot when the statement supplies a literal
  revision and either a literal source id with a target-id list (capped at
  1,000), or an identity join paired with a target-id list. Sources are
  owner-checked, staged documents are reused, and targets use the existing
  revision/CAS and `RETURNING` path. The direct source assignment may also be
  written as `CAST(source.document_json AS JSON)`; this is syntax-only.
  Arbitrary joins, cross-kind sources, arbitrary assignments, and general
  lake-table mutation remain rejected.
  Evidence:
  `docs/qualification/track2-native-dml-update-from-2026-11-03.md`.
- [x] Add bounded same-kind product `DELETE ... USING`. The source is
  owner-checked under the pinned snapshot before up to 1,000 target IDs are
  expanded into revision/CAS deletes; arbitrary `USING` joins, cross-kind
  sources, and unbounded scans remain rejected. Evidence:
  `docs/qualification/track2-native-dml-update-from-2026-11-03.md`.
- [x] Add bounded `MERGE` with explicit revision/CAS semantics. `MERGE`
  accepts one `VALUES` batch with an optional simple target alias, up to 1,000 rows containing `id` and
  `document_json`, an equality `target.id = source.id` predicate, and exactly
  `WHEN MATCHED THEN UPDATE SET document_json = source.document_json` followed
  by `WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id,
  source.document_json)` or a bounded literal `CAST('<json>' AS JSON)`
  document. Each source row lowers to the existing
  owner-isolated `Upsert` command, so commit/revision behavior is shared with
  the qualified conflict upsert path. Explicit `WHEN NOT MATCHED BY TARGET`
  is accepted as an alias, and duplicate source ids fail closed before staging
  to keep revisions deterministic. Evidence:
`docs/qualification/track2-native-dml-merge-2026-10-28.md`. The bounded
  product-record MERGE surface now also qualifies matched-delete-only batches
  and bounded not-matched-by-source updates with one literal replacement
  document:
matched rows use revision-CAS and ownership checks, while unmatched source
rows are no-ops. Insert-only `WHEN NOT MATCHED` clauses may carry a bounded
`source.id` equality or `IN` filter; the source batch is filtered before the
same idempotent create-if-absent commands are staged. General MERGE clauses and typed lake-table DML remain outside
  the contract. The target alias is optional for the bounded form; an
  unaliased target derives its qualifier from the base table name, with the
  same fail-closed clause and source-shape rules. Matched document assignments
  may use either the unqualified `document_json` name or the declared target
  alias (`target.document_json`), and may wrap the direct source value as
  `CAST(source.document_json AS JSON)` or use a bounded literal wrapped as
  `CAST('<json>' AS JSON)`; mismatched qualifiers, arbitrary expressions, and
  other casts remain rejected. Evidence:
  `docs/qualification/track2-native-dml-merge-json-typed-source-2026-11-03.md`.
  A matched-only clause may also carry the bounded source filter
  `source.id = 'literal'` or `source.id IN ('literal', ...)`; excluded source
  rows remain no-ops and selected rows use the same revision/CAS path. When a
  target alias is declared, the equivalent `target.id = 'literal'` or
  `target.id IN ('literal', ...)` filter is also accepted; the `ON` equality
  makes it a deterministic filter over the same source batch. Other target
  columns, arbitrary expressions, `NOT IN`, and compound predicates remain
  rejected. The first matched clause may additionally carry the redundant
  identity predicate `target.id = source.id` (or the reversed operand order),
  including bounded update-or-insert and not-matched-by-source forms; the
  `ON` equality makes it a deterministic no-op. Evidence: `docs/qualification/track2-native-dml-merge-predicates-2026-11-03.md`.
  A two-clause update-or-insert MERGE may also use the bounded matched
  source/target ID filter: existing rows outside the filter are no-ops while
  absent source rows still insert. The transaction layer resolves this form
  against the pinned snapshot before staging ordinary revision/CAS commands.
  Filtered matched DELETE paired with an insert clause is also qualified:
  selected existing rows delete, excluded existing rows no-op, and absent
  source rows insert through the same pinned snapshot and CAS path. Evidence:
  `docs/qualification/track2-tsql-merge-filtered-update-insert-2026-11-04.md`
  and `docs/qualification/track2-tsql-merge-filtered-delete-insert-2026-11-04.md`.
  The not-matched-by-source action also accepts a bounded target-id equality or
  `IN` filter during snapshot expansion; rows outside that literal set remain
  untouched. Evidence is recorded in
  `docs/qualification/track2-native-dml-merge-2026-10-28.md`.
- [ ] Expand `MERGE` and broader conflict handling beyond the qualified bounded
  product surface only with explicit revision/CAS semantics. The current
  contract covers update-or-insert, delete-or-insert, matched-update-only,
  matched-delete-only, unmatched-insert-only, and matched-update plus
  not-matched-by-source-delete batches (up to 1,000 expanded affected rows),
  plus product
  `ON CONFLICT DO NOTHING` and the bounded `ON CONFLICT (id) DO UPDATE SET
  document_json = excluded.document_json` upsert (or its direct typed
  `CAST(excluded.document_json AS JSON)` spelling), with literal `WHERE TRUE`
  or the equivalent `WHERE id = excluded.id` identity predicate accepted as a
  semantic no-op. The conventional `<product_table>_pkey` named target is
  accepted as the same `id` identity target. Other named constraints, other
  conflict predicates, arbitrary assignments, and arbitrary conflict targets remain
  rejected. Product `RETURNING id, revision` and the qualified `RETURNING *`
  expansion are implemented from the staged revision/CAS write set.
  A bounded literal `document_json` assignment (including `CAST(literal AS
  JSON)`) is also accepted on the conflict/update path; new rows still use the
  `VALUES` document, and source-backed `INSERT ... SELECT` literal overrides
  remain rejected. Evidence:
  `docs/qualification/track2-native-dml-conflict-literal-2026-11-03.md`.
  Savepoints are implemented for the product
  transaction session and covered by parser, catalog, and API tests. The
  update-only form is qualified in
  `docs/qualification/track2-native-dml-merge-2026-10-28.md`; matched updates
  also accept one bounded `document_json` string literal and reuse the same
  revision/CAS path. `MERGE OUTPUT` is qualified for update-or-insert,
  delete-or-insert, matched-update-only, and unmatched-insert-only forms when the projection is `inserted.id`,
  `inserted.revision`, `inserted.document_json`, or `inserted.*`; matched-
  delete-only forms also qualify `deleted.id`, `deleted.revision`, and the
  pre-delete `deleted.document_json` snapshot. Unmatched
  source rows produce no result row. Delete-or-insert output is action-aware:
  `inserted` returns inserted/update actions and `deleted` returns deleted/update
  actions from the ordered staged write set. Bounded `$action` output returns
  the resolved `INSERT`/`UPDATE`/`DELETE` label for each staged action.
  Bounded not-matched-by-source
  delete forms also qualify `deleted.id` and `deleted.revision`, and mixed
  action batches qualify the corresponding inserted identity/revision/document
  output; deleted documents are captured from the pinned pre-delete snapshot.
  Mixed pseudo-table projections remain rejected.
  Evidence:
  `docs/qualification/track2-tsql-merge-output-2026-11-03.md`.
- [x] Qualify bounded snapshot isolation and transaction-session restart
  behavior for KaveonDB-owned product records; see
  `docs/qualification/track2-transaction-isolation-matrix-2026-10-28.md`.
- [x] Accept a typed JSON literal in bounded product `UPDATE` assignments.
  `CAST('<json>' AS JSON)` is normalized to the existing document publication
  path; arbitrary expressions remain rejected. Evidence:
  `docs/qualification/track2-native-dml-json-typed-update-2026-11-03.md`.
- [x] Qualify bounded sized SQL Server/Fabric text casts. `VARCHAR(n)`,
  `NVARCHAR(n)`, and `CHAR(n)` use the Engine's explicit UTF-8 scalar contract
  (truncate, or truncate and U+0020-pad for `CHAR`), preserve NULLs, and cap
  lengths at 1,000,000 scalars. Binary/code-page semantics and styled sized
  text `CONVERT` remain rejected. Evidence:
  `docs/qualification/track2-tsql-sized-text-casts-2026-11-03.md`.
- [ ] Add a durable journal/recovery qualification for cloud/object-store
  failures and multi-coordinator restart. This remains distinct from the
  bounded snapshot contract above. The local coordinator journal now fsyncs
  the replacement directory entry on Unix during compaction (Windows keeps
  file-level sync); this narrows the crash window but does not close the
  cloud/object-store or cross-host recovery boundary. Evidence:
  `docs/qualification/track1-coordinator-journal-compaction-durability-2026-10-18.md`.
- [x] Qualify primary/unique indexes, typed-schema validation, and bounded
  product-reference enforcement for KaveonDB-owned typed rows. Evidence:
  `docs/qualification/track2-typed-schema-integrity-2026-10-28.md`.
- [ ] Add general SQL constraint/index DDL and catalog-backed enforcement for
  arbitrary user tables and lake-table mutation. This is intentionally not a
  parser-only task: the implementation must persist constraint/index metadata
  in the catalog revision, validate it on every supported writer (including
  transactional and maintenance rewrites), expose it through `SHOW CREATE
  TABLE`/metadata, and define behavior for Parquet, Delta, and object-store
  snapshot replacement. `clustered_by` and Bloom filters are physical layout
  hints, not uniqueness or referential constraints, and must not be reported
  as indexes. Acceptance requires exact duplicate/null/foreign-key behavior,
  atomic rollback on violation, restart/replay recovery, and local plus
  distributed qualification; until those gates exist, unsupported general
  `CREATE INDEX`, `ALTER TABLE ... ADD CONSTRAINT`, and arbitrary lake-table
  mutation remain fail-closed. The current rejection boundary is qualified in
  `docs/qualification/track2-lake-ddl-boundary-2026-11-03.md`.
- [x] Publish a supported transaction/isolation matrix rather than claiming
  PostgreSQL equivalence. The bounded product-record contract is recorded in
  `docs/qualification/track2-transaction-isolation-matrix-2026-10-28.md`.

### Phase 3 — Optimizer and statistics

- [x] Persist source-version-bound row counts, bytes, distinct sketches, null
  fractions, quantiles, and bounds for supported Parquet/Delta/Iceberg paths.
  Evidence: `docs/qualification/track3-statistics-planning-boundary-2026-10-28.md`.
- [x] Add a bounded persisted numeric histogram model derived from the
  source-version-bound KLL profile. Histogram boundaries/counts are used for
  range selectivity when present; text and metadata-only columns remain
  bounds/quantile-only. Evidence:
  `docs/qualification/track3-statistics-histogram-2026-10-29.md`.
- [x] Use qualified statistics for bounded local join ordering, broadcast
  choice, selectivity, and reader pruning. Cloud pruning and universal
  cost-based planning remain separate gates; see
  `docs/qualification/track3-statistics-planning-boundary-2026-10-28.md`.
- [x] Answer an unfiltered single-table `COUNT(DISTINCT column)` from an exact,
  source-version-bound distinct count produced by `ANALYZE ... WITH
  (distinct = true)`, while keeping missing/stale metadata and all filtered,
  grouped, joined, or derived shapes on the scan path. Evidence:
  `docs/qualification/track3-exact-distinct-context-2026-11-03.md`.
- [x] Add bounded plan-vs-runtime diagnostics. The query-level estimate-vs-actual
  endpoint now compares source-row estimates with finished scan telemetry and
  exposes ordinal per-scan attribution when scan cardinalities match exactly;
  broader per-operator attribution remains open. Runtime-only comparison now exposes a
  conservative doubled-P95 regression threshold after five comparable runs;
  evidence: `docs/qualification/track3-runtime-regression-threshold-2026-10-29.md`;
  optimized and physical scan plans now expose versioned source estimates;
  evidence: `docs/qualification/track3-plan-estimate-visibility-2026-10-29.md`;
  estimate-vs-actual evidence: `docs/qualification/track3-estimate-vs-actual-query-diagnostic-2026-10-29.md`.
  Distributed fragment roots with an exact operator/estimate now also expose
  bounded `stage_boundaries` output-row diagnostics; the stage-level evidence
  is recorded in `docs/qualification/track3-operator-estimate-diagnostics-2026-11-02.md`.
  Coordinator aggregate and TopN shortcuts expose the same boundary when the
  physical root estimate is available, with final merged rows explicitly
  scoped as `query_result`.
- [ ] Extend estimate-vs-actual diagnostics to every physical operator and
  selectivity boundary; this remains open. Physical plan display now carries
  conservative child-input lineage for unary operators, joins, semi/anti
  joins, and set operations, plus conservative bounded output caps for
  `TopN`, `LIMIT`, and `OFFSET`. Row-preserving `Filter`, `Project`, `Sort`,
  and `Window` nodes propagate the same upper bound; joins and set operations
  now expose mathematically safe product/sum/minimum/left-input caps without
  inventing selectivity estimates. Aggregate, hash-aggregate, final-aggregate,
  and distinct nodes also expose a safe child-row upper bound, without
  claiming group cardinality. The estimate-diagnostics endpoint now reports
  operator rows only where the runtime attached a matching root or scan
  counter; missing intermediate counters remain unavailable rather than
  inferred. Evidence:
  `docs/qualification/track3-plan-estimate-lineage-2026-10-29.md` and
  `docs/qualification/track3-operator-estimate-diagnostics-2026-11-02.md`.
  Finished query rows now also propagate through a directly connected
  `Window`/`Project`/`Sort` chain and attach to the first directly reached
  cardinality boundary (`Filter`, `Limit`, `Offset`, `TopN`, aggregate
  variants, `Distinct`, joins, `Unnest`, or `Union`/`Intersect`/`Except`) as
  the output boundary, with an explicit `derived_row_preserving` scope. The
  physical scan boundary now also retains the reader's exact
  `actual_rows_emitted` counter alongside `actual_rows_selected`, so
  row-group selection and post-filter output are not conflated. It also
  exposes exact decoder `actual_filter_rows_examined` and
  `actual_filter_rows_admitted` counts when a row predicate ran. Evidence:
  `docs/qualification/track3-scan-emitted-row-diagnostics-2026-11-03.md`.
  boundary's input is never labeled with post-operation rows, and propagation
  stops at unrelated operators. Evidence:
  `docs/qualification/track3-row-preserving-actuals-2026-11-03.md`.
  The diagnostics collector preserves the attached `derived_row_preserving`,
  `query_result`, or `scan_selected_rows` scope instead of relabeling every
  output counter as a final query result; evidence:
  `docs/qualification/track3-operator-estimate-scope-2026-11-03.md`.
- [x] Preserve stage operator and planner-estimate metadata during live
  distributed task publication. A later completion fills metadata on an
  already-running stage without inferring any row counts. Evidence:
  `docs/qualification/track3-live-stage-estimate-metadata-2026-11-03.md`.
- [x] Qualify result-cache behavior separately from data-cache behavior. The
  result cache is keyed by normalized SQL, parameters, catalog snapshot,
  pinned source versions, and time zone; it reports cache execution mode,
  excludes paged results, honors per-statement bypass, applies bounded
  TTL/byte/LRU eviction, and is cleared on catalog publication and product
  transaction commit/replay. Evidence:
  `docs/qualification/track3-result-cache-boundary-2026-10-28.md`.

### Phase 4 — Distributed completeness

- [ ] Finish adaptive joins and aggregate exchange for high-cardinality data.
  The final-merge repartition path now streams encoded binary state directly
  into partition builders instead of materializing per-partition `take` batches;
  selected broadcast workers also divide prepaid input admission across the
  shared query budget, allowing tight workloads to reach spill instead of
  failing during per-worker prepayment. Its recursive fanout now adapts to
  encoded spill bytes and the active memory limit within a bounded 16–256-way
  policy. Exact skew/replay, release-pressure, and fanout-policy evidence is in
  `docs/qualification/track1-final-merge-repartition-streaming-2026-11-03.md`
  and `docs/qualification/track1-adaptive-final-merge-fanout-2026-11-03.md`.
  Join secondary hot-bucket splits now also choose a bounded 16–256-way
  fanout from measured encoded bytes while preserving hash alignment; evidence
  is in `docs/qualification/track1-adaptive-join-fanout-2026-11-03.md`.
  Larger-scale adaptive exchange and deployed performance qualification remain
  open.
- [ ] Distribute remaining window and global-sort shapes.
- [x] Complete correlated/lateral execution or maintain explicit rejection.
  Bounded correlated and lateral forms are qualified, including residual
  predicates in semi/anti-join `EXISTS`/`NOT EXISTS` shapes. Scalar residual
  `OFFSET`/`LIMIT`, table-valued, and other unsupported shapes remain explicit
  parser/planner rejections with actionable diagnostics in the compatibility
  corpus.
- [ ] Qualify object-store retries, spill exhaustion, cancellation, and mixed
  workload endurance on every release image.

### Phase 5 — Federation and client compatibility

- [x] Add a connector execution abstraction with one-source pushdown first.
  `api/database/connectors.py` now provides the explicit registered-source
  execution seam: KaveonDB resolves the source and the complete bounded
  statement is pushed to exactly one source through the pool. This does not
  claim cross-source planning or joins. Evidence:
  `api/database/test_connectors.py` and
  `docs/qualification/track5-registered-source-single-source-boundary-2026-11-03.md`.
- [ ] Add cross-source joins only after snapshot, type, auth, and failure
  semantics are defined.
- [ ] Qualify JDBC/ODBC and external Trino clients against the documented wire
  surface.
- [x] Add system metadata views with role-filtered visibility. `KaveonDB`
  exposes read-only `product`, `catalog`, and `system` projections to Admins;
  ordinary roles cannot browse or query the reserved authority. Runtime and
  catalog-access API tests cover the projection and denial boundary. Evidence:
  `docs/qualification/track5-kaveondb-system-catalog-access-2026-11-02.md`.

## Release gate

No phase is complete until its corpus has exact-result evidence, bounded-memory
and cancellation tests, a clean workspace gate, and updated compatibility
documentation. Claims must identify whether they are Engine execution,
registered-source passthrough, or KaveonDB transaction behavior.

The current local workspace gate is recorded in
`docs/qualification/track0-workspace-gate-2026-11-03.md`: the SQL crate has
388 passing tests, the execution crate has 638, the optimizer has 86, storage
has 158, and the server binary has 716 passing tests, including the complete
distributed differential sweep. This evidence does not substitute for the
open cloud, external-client, or broader semantic qualification boundaries
listed above.

