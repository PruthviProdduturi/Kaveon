# Track 2 SQL compatibility qualification — 2026-10-14

**Scope:** local SQL/parser/optimizer/executor regression on the current
working tree. This is semantic qualification, not a claim of complete Trino
wire or feature parity.

## Validation

From `engine/`:

```powershell
cargo test --workspace --no-fail-fast --quiet
```

The complete workspace run passed with zero failures. The run included the
server differential sweep (389 tests, 4 ignored benchmark cases), SQL parser
and binder coverage, optimizer tests, execution and storage tests, transaction
and CLI tests, and the remaining workspace targets.

From the repository root:

```powershell
node scripts/validate-docs.mjs
```

Documentation validation passed: 159 Markdown files, 39 routes, and 9 SVGs.

## Clean workspace gate after exchange backpressure fix — 2026-10-15

Commit `5b6bda22` passed from a fresh clean checkout. The gate recorded 74
core tests, 211 SQL tests (one ignored), 85 planner/runtime tests, 368
execution tests (four ignored), 57 CLI tests, 394 server tests, 159 additional
SQL tests, and 121 storage tests (one ignored), with zero failures. The server
backpressure regression is deterministic when the bounded exchange sender
closes at the same time as the producer completion signal. This validates the
current pushed commit; it does not close the remaining cloud and broader
Trino-compatibility boundaries below.

## Single-row scalar subquery qualification — 2026-10-15

The planner now admits scalar subqueries whose input is a statically single-row
constant relation, including Trino's `SELECT (SELECT 42)`. Implicit scalar
SELECTs and constant `UNNEST(ARRAY[...])` inputs are accepted only when every
array has at most one element; scans and correlated subqueries remain
fail-closed. Filters over constant scalar inputs also remain fail-closed
because a zero-row scalar requires Apply/null-fill semantics. The SQL planner
suite passes 158 tests with zero failures.

## Workspace gate rerun — 2026-10-15

After the JSON path and scalar-array compatibility changes, the complete
workspace gate passed with zero failures. The current run includes 394 server
tests (with the long differential sweep), 158 SQL tests, 356 execution tests
with four ignored benchmark cases, and 121 storage tests with one ignored
case. The repository documentation validator also passed with 159 Markdown
files, 39 routes, and 9 SVGs.

## Workspace gate rerun after composite pushdown — 2026-10-15

The complete workspace gate passed again after composite runtime-predicate
wiring: 394 server tests, 158 SQL tests, 356 execution tests with four ignored
benchmark cases, and 121 storage tests with one ignored case. The long server
differential sweep passed without failures.

## Current qualified local surface

The current tests cover the implemented Trino-shaped surface including core
SELECT semantics, joins and semi/anti joins, CTEs and derived relations, set
operations, grouping sets and metadata functions, `COUNT_IF`, windows and QUALIFY, arrays/maps/rows, JSON paths,
prepared statements and typed parameters, metadata statements, global ordering
and limits (including `FETCH … WITH TIES`), grouping metadata functions, and
local/distributed exact-result execution.

## Correlated subquery boundary — 2026-10-15

The server differential sweep exercises the bounded correlated shapes that the
binder lowers to executable joins: keyed `EXISTS`, `NOT EXISTS`, `IN`, and
`NOT IN` with residual predicates, plus correlated scalar `MIN`/`MAX` and
`COUNT` filters. Each case is compared across node-local and three-fragment
execution over the same Parquet inputs. This is qualified coverage for those
shapes, not a general Apply implementation. Positive top-level `LIMIT` on
correlated `EXISTS`/`NOT EXISTS` is now removed before decorrelation because it
cannot change existence; correlated `OFFSET`, `LIMIT 0`, limits in correlated
`IN`/scalar forms, `UNION`, multi-level outer references, and lateral/derived
relation forms remain explicitly outside the contract and continue to fail
closed.

## Fresh clean workspace gate — commit `33ad59e1` — 2026-10-15

The complete workspace gate was rerun from a detached clean checkout of the
pushed commit. It passed with zero failures: 74 core tests, 211 SQL tests (one
ignored), 85 planner/runtime tests, 368 execution tests (four ignored), 57 CLI
tests, 394 server tests, 159 additional SQL tests, and 121 storage tests (one
ignored). The long server differential sweep completed successfully in that
same run.

The focused clean-checkout regressions also passed for grouping metadata and
tie-aware fetch: `executes_grouping_sets_grand_total`,
`executes_grouping_id_with_branch_specific_flags`, and
`fetch_percent_rejects_and_with_ties_preserves_rank_semantics` each passed
with zero failures.

The SQL planner's `COUNT_IF` coverage also passed in both supported forms:
grouped `COUNT_IF` lowering and aggregate `FILTER` validation (`2 passed,
0 failed`).

## JSON_FORMAT qualification — 2026-10-15

`JSON_FORMAT(json)` now canonicalizes the Engine's validated UTF-8 JSON
representation with `serde_json` serialization, preserves SQL NULL, and
fails with the row index for invalid input. The executor regression covers
canonical object/array output, null preservation, and the invalid-row error
contract. The focused expression suite passes 144 tests with one ignored
benchmark case; broader JSON coercion and path semantics remain open in the
matrix.

## JSON_ARRAY_GET qualification — 2026-10-15

`JSON_ARRAY_GET(json_array, index)` now supports zero-based and negative
indexes over the Engine's validated UTF-8 JSON representation. Missing
positions, non-array documents, and SQL NULL inputs produce NULL; returned
objects, arrays, and JSON null values retain canonical JSON text. The focused
expression regression covers object, scalar, negative-index, out-of-range, and
NULL behavior. Full JSON coercion and path semantics remain open.

## JSON_QUERY and JSON_VALUE qualification — 2026-10-15

`JSON_QUERY(document, path)` and `JSON_VALUE(document, path)` now use the
qualified JSON path implementation for object/array and scalar extraction,
respectively. Missing paths and JSON nulls preserve the existing NULL
contract. The focused expression suite covers both aliases; richer SQL
standard returning/error clauses remain outside the current surface.

## JSON_EXISTS qualification — 2026-10-15

`JSON_EXISTS(document, path)` now returns true for a present path, including
an explicit JSON null, false for a missing path, and SQL NULL for NULL inputs.
The focused executor regression covers all four cases. Invalid or unsupported
paths remain conservative NULL results under the existing JSON path contract.

## ARRAY_JOIN qualification — 2026-10-15

`ARRAY_JOIN(array, delimiter[, null_replacement])` now handles regular and
large lists whose scalar elements can be cast to VARCHAR, skips null elements
by default, applies the optional replacement text when provided, and preserves
NULL array/delimiter rows. The focused executor regression covers text and
integer values, null replacement, and a NULL array row. Nested values remain
explicitly unsupported until their coercion contract is qualified.

## Quoted JSON path keys qualification — 2026-10-15

The JSON path reader now accepts quoted bracket keys, including escaped
characters, alongside dotted object keys and numeric array subscripts. This
covers keys containing punctuation such as `customer-name` for
`JSON_QUERY`, `JSON_VALUE`, and `JSON_EXISTS`; broader SQL/JSON modes and
returning/error clauses remain open.

## Large UTF-8 JSON input qualification — 2026-10-15

`JSON_FORMAT` and `JSON_ARRAY_GET` now accept both Arrow `Utf8` and
`LargeUtf8` document inputs through the same row-local JSON contract. The
focused regression covers canonical output and NULL preservation for a large
string array; path and returning-mode boundaries remain unchanged.

## TRY JSON compatibility qualification — 2026-10-15

`TRY(JSON_FORMAT(...))` and `TRY(JSON_ARRAY_GET(...))` now lower through
explicit row-local safe evaluators, matching the existing `TRY(JSON_PARSE(...))`
contract. Malformed documents return NULL instead of aborting the statement;
valid values and ordinary out-of-range array accesses retain their normal
results. The focused SQL and executor suites cover planner lowering and
execution behavior.

## Large UTF-8 JSON parsing qualification — 2026-10-15

`JSON_PARSE` and `TRY_JSON_PARSE` now accept `LargeUtf8` document columns in
addition to ordinary `Utf8`. Valid documents retain their text representation,
malformed values follow the existing strict/TRY behavior, and SQL NULL remains
NULL. The focused executor regression covers all three row states.

## Large UTF-8 JSON path qualification — 2026-10-15

JSON extraction, `JSON_EXISTS`, `JSON_SIZE`, `JSON_ARRAY_LENGTH`, and
`JSON_ARRAY_CONTAINS` now accept both `Utf8` and `LargeUtf8` documents; path
arguments for extraction, existence, and size accept both widths as well. A
large-document and large-path `JSON_VALUE` regression covers quoted keys and
scalar output. The JSON coercion and returning-mode boundaries remain open.

## Typed JSON array needles qualification — 2026-10-15

`JSON_ARRAY_CONTAINS` now accepts signed and unsigned integer widths plus
`Float32`/`Float64` needles, preserving exact JSON numeric comparison and the
existing NaN/infinity rejection. The focused executor regression adds an
`Int32` probe alongside the existing 64-bit and text cases.

Decimal128 needles are accepted as well. Their Arrow scale is rendered as an
exact JSON number before comparison, so values such as `12.34` and `-0.50` do
not pass through a binary floating-point conversion. Decimal256 and other
unsupported needle types remain rejected explicitly.

## Workspace regression gate — 2026-10-15

The post-change workspace gate at commit `3d2085cb` passed with 74 core tests,
211 SQL tests (one ignored), 84 planner/runtime tests, 356 execution tests
(four ignored), 57 CLI tests, 394 server tests, 158 additional SQL tests, and
121 storage tests (one ignored). The focused Decimal128 expression regression
and the 19-test server differential suite also passed. This is repository
evidence; multi-worker cloud qualification remains a separate Track 1 gate.

## Composite runtime-filter null safety — 2026-10-15

The composite storage-predicate path was exercised with nullable build keys.
The regression confirms that independent per-column `IN` projections contain
only non-null build values; exact composite probing remains responsible for
rejecting the false-positive cross-products. The join-focused suite passes all
46 tests with this case included.

## Composite runtime-filter qualification — 2026-10-15

Composite inner and semi joins now expose independent build-key `IN`
predicates to eligible storage readers. The projections are safe prefilters:
they can retain rows that the later composite hash probe rejects, but cannot
remove a valid composite match. Unsupported key values fall back to the exact
batch filter. The execution regression verifies the two-column predicate shape;
cloud snapshot and null/type qualification remain open.

## Composite predicate pushdown wiring qualification — 2026-10-15

The runtime operator contract now accepts a safe conjunction of single-column
`IN` predicates and forwards each child to the underlying scan. This closes
the integration gap between composite-key filter construction and storage
readers; unsupported shapes still decline and retain the exact batch filter.
The core contract regression verifies that every conjunction child is applied.

## Correlated EXISTS positive-limit qualification — 2026-10-15

The binder now removes a positive top-level `LIMIT` from a correlated
`EXISTS`/`NOT EXISTS` subquery before decorrelation. Existence only depends on
whether at least one row survives, so `LIMIT 1` and other positive limits do
not change the predicate's truth value. `LIMIT 0` is lowered to an empty
relation, while limits in correlated `IN` or scalar subqueries are not
rewritten because their cardinality affects semantics.

The optimizer suite passes `23/23`, including
`correlated_exists_positive_limit_is_decorrelated` and
`correlated_exists_zero_limit_becomes_empty_relation`, plus no-op `OFFSET 0`
coverage for correlated EXISTS and scalar aggregate subqueries. This is a local binder
qualification. The shared local/distributed differential corpus also includes
the grouped `correlated_exists_positive_limit` and `correlated_exists_zero_offset`
queries and compares both paths against the same exact results.

The binder also regression-tests positive correlated `OFFSET` rejection. It
fails closed with an explicit unsupported-correlated-subquery diagnostic rather
than evaluating an incorrect semi-join.

## Explicit remaining boundaries

The full Track 2 claim remains open. The compatibility reference still records
recursive CTEs, broader correlated/lateral subqueries, unsupported complex-type
combinations, broad lake-table DML, full external-client prepare/describe/bind
interoperability, and the remaining distributed-window shapes as open. Those
boundaries are rejected or documented explicitly; they are not counted as
implemented parity.

This report is local evidence. AKS workload and external Trino-client
qualification are separate gates in `docs/engineering/trino-parity-tracker.md`.


## Clean workspace gate rerun — 2026-10-15

The clean detached parity workspace was rerun after the distributed runtime
qualification updates. `cargo test --workspace --no-fail-fast --quiet` returned
exit code `0`: core `74/74`, SQL `211 passed / 1 ignored`, planner/runtime
`85/85`, execution `368 passed / 4 ignored`, CLI `57/57`, server `394/394`,
additional SQL `159/159`, and storage `121 passed / 1 ignored`. The long server
differential sweep completed successfully. The differential corpus includes
grouped `MIN(DISTINCT actions)` and `MAX(DISTINCT actions)` on local and
distributed execution paths; both match the exact reference extrema.

The clean current-source Trino wire qualification is retained in
`docs/qualification/track2-trino-wire-2026-10-15.md`. It passes the prepared
statement lifecycle and paged result retrieval locally. The deployed AKS image
has not passed that gate because its digest still serves the older native
statement behavior for control/prepared requests; an image rebuild and rollout
are required before cloud client compatibility is claimed.

## Scalar-query follow-up

Commit `b6a80ec5` adds the missing Trino-compatible implicit single-row
relation for scalar `SELECT` statements without `FROM`. The focused SQL suite
passes 151 tests, the server suite passes 389 tests, and a rebuilt Docker
coordinator returned `SELECT CAST(1 AS BIGINT) AS value` as one typed `Int64`
row and `SELECT 1 UNION ALL SELECT 2 ORDER BY n DESC` as `2, 1`. The full
workspace gate was rerun after the change and all targets passed. This closes
that specific compatibility gap; recursive CTEs, broader lateral forms, and
the other boundaries in the matrix remain open.

The same pass found a result-boundary defect for Arrow's `Null` type. Commit
`3d34d2e1` maps it to JSON `null` instead of the Arrow debug string
`NullArray(1)`. The focused regression and all 389 server tests pass, and the
rebuilt Docker coordinator now returns `[[null]]` for `SELECT NULL`. The full
workspace gate was rerun after this change: all targets passed, including 389
server tests, 148 SQL tests, and the differential sweep.

The next probe found that direct typed date literals were exposed as internal
day numbers. The projection path now preserves the `Date32` type while keeping
the existing day-number lowering for date predicates and storage pruning. The
SQL suite passes 151 tests, and rebuilt Docker returned `Date32` plus the ISO
value `2026-01-01` for `SELECT DATE '2026-01-01' AS d`.

The decimal probe then found that exact decimal literals failed only after
distributed fragment serialization because JSON cannot represent Rust `i128`
values directly. Commit `c227f91b` serializes the unscaled decimal value as an
exact string on the internal wire and restores it losslessly. Core and SQL
regressions pass, and Docker now completes `SELECT DECIMAL '1.20' AS n`,
`SELECT 1.20 AS n`, and `SELECT DECIMAL '-0.005' AS n` with Decimal128 results
and exact formatted values.

The full workspace gate was rerun after the decimal wire change: all targets
passed, including 389 server tests, 151 SQL tests, and the differential sweep.

Typed `TIME` literals are now lowered to a runtime time expression and
evaluated on workers as Arrow `Time64(Microsecond)` values. The rebuilt Docker
endpoint qualifies both whole-second and fractional values:

```sql
SELECT TIME '12:34:56' AS t;       -- 12:34:56.000000
SELECT TIME '12:34:56.789' AS t;   -- 12:34:56.789000
```

Invalid hours, minutes, seconds, and malformed fractional values fail during
expression evaluation rather than being silently coerced.

Top-level `VALUES` and derived `VALUES` relations now lower through the same
columnar `UNNEST` path used by constant row-producing relations. Docker
qualification returned the declared aliases and exact rows for:

```sql
SELECT t.id, t.label
FROM (VALUES (1, 'a'), (2, 'b')) AS t(id, label);
```

The ordered top-level form also qualifies:

```sql
VALUES (1, 2), (3, 4) ORDER BY column1 DESC;
```

The focused SQL suite now passes 157 tests. The server backpressure test had
one transient race during the first full run; its isolated rerun and five
consecutive repetitions passed before this change was accepted.

The post-change server regression suite was then rerun cleanly: all 389 tests
passed in 121.94 seconds, including the differential sweep. The full
workspace gate remains the release qualification command and must be rerun
after the next cross-crate change.

After the typed-time change, the complete workspace gate was rerun on the
current tree and passed with zero failures: server `389/389`, SQL `151/151`,
execution `351/351` with four pre-existing ignored benchmark tests, and all
remaining workspace targets green. The server differential sweep completed in
111.72 seconds.

## Current workspace gate (follow-up)

The focused parity evidence remains green: `cargo test -p kaveon-sql --quiet`
passes `157/157`, and the VALUES and typed-TIME runtime checks above remain
valid. A later workspace run on the shared working tree is **not** a release
gate: it is concurrently carrying unrelated catalog and cube changes. That
run reported a catalog timing benchmark failure and the server
`analyze_reads_the_columns_on_the_workers_and_cancels_with_them` regression
reported a changed cube-grouping payload. These failures are outside the
VALUES/TIME changes and are retained as open workspace gates rather than being
counted as parity evidence. The full workspace command must be rerun from a
clean, integrated tree before Track 2 is called qualified.

The distributed cube regression was then isolated to artifact ordering: worker
waves persisted single-axis groupings before the rolled-up total, while the
local builder emits the declared shape order. The coordinator now canonicalizes
grouping order before persistence. The targeted regression passed, followed by
the complete server suite: `393/393` tests passed, including the differential
sweep. This validates deterministic cube artifacts without changing the
remaining Track 1 cloud-pressure and operational gates.

The clean workspace qualification was rerun after the deterministic ordering
fix with output captured and an authoritative exit status. `cargo test
--workspace --no-fail-fast --quiet` returned exit code `0`: server `393/393`,
SQL `157/157`, execution `351 passed / 4 ignored`, and the remaining workspace
targets all passed. The server differential sweep completed in 105.78 seconds.

## UNNEST offset qualification

The row-producing UNNEST surface now accepts the BigQuery-compatible
`WITH OFFSET AS name` form in addition to Trino's `WITH ORDINALITY`. The
leaf operator carries an explicit start value: offsets emit `0..n-1` and
ordinality emits `1..n`. Local SQL and executor tests pass, and the
distributed differential gate returns the exact rows `10,0` and `20,1` for:

```sql
SELECT * FROM UNNEST(ARRAY[10, 20]) WITH OFFSET AS off ORDER BY off;
```

Correlated/lateral UNNEST and arbitrary table-function expressions remain
outside this qualification.

## Trino wire compatibility qualification

The focused Trino compatibility gate passed on the same workspace commit:
`cargo test -p kaveon-server trino_ --quiet` completed with **25 passed, 0
failed**. This covers Trino session headers, response paging and structured
errors, SHOW SESSION/SHOW FUNCTIONS, PREPARE/EXECUTE/DEALLOCATE ownership,
DESCRIBE INPUT/OUTPUT, typed scalar/array/map bindings, and the session
context headers. It is a server-side protocol qualification; a sweep from
external Trino JDBC/CLI clients remains an explicit open gate.

## Runtime-filter qualification boundary

The bounded build-side runtime-filter path was rerun with
`cargo test -p kaveon-exec runtime_filter --quiet`: **2 passed, 0 failed**.
The filter is applied before hash probing for eligible inner and semi joins,
with typed integer/date/text key coverage. It intentionally does not claim
scan-level Parquet/Delta pruning: storage predicates are fixed when a source
is constructed, so a future source-level filter interface must preserve
snapshot consistency and exact outer/anti-join semantics before that gate can
close.

## Delta source-level runtime-filter qualification

The first source-level runtime-filter path is now implemented for local Delta
scans. `DeltaBatchIterator` defers opening its first Parquet file until the
first batch, accepts an exact single-key build-side `IN` predicate before that
open, and combines it with the statement predicate. Existing Delta snapshot
and row-group pruning remain in force. The focused storage regression passed
with the injected keys (`predicate_prunes_row_groups_and_filters_rows_across_all_files`), and the full server suite passed **394/394** afterward.

This narrows, but does not close, the Track 1 gate. Local Parquet files and
directory tables now use the same deferred source contract: the footer and
pinned listing are available for planning, while the first decoder is held
until the exact build-side keys arrive. A focused `parquet_reader` regression
selects only the expected labels from the six-row fixture, and the full
storage Parquet test group passes. Single-object readers now publish their
footer schema, wait for the first batch request, and accept exact build-side
keys before constructing the remote decoder; an in-memory object-store
regression returns exactly those keys with decoder row filtering enabled.
Object-directory scans now wait after their pinned listing and schema plan.
They fold exact keys over each file's partition values first, prune files whose
path cannot match, and add only the remaining physical-column residual to each
remote decoder. A focused directory regression selects only the first and last
keys from the fixture. Cloud snapshot qualification remains open.

## Workspace qualification follow-up (2026-10-15)

The current integrated workspace gate completed with exit code `0`:

- server: `394 passed, 0 failed` (including the differential sweep);
- SQL: `158 passed, 0 failed`;
- execution: `352 passed, 0 failed, 4 ignored`;
- storage: `121 passed, 0 failed, 1 ignored`;
- all remaining workspace targets passed.

The focused TopN executor suite also passed `15/15`. This verifies exact
partition-local candidate merging, multi-column and NULL ordering, schema
validation, and bounded spill behavior. It does not close the Track 1 global
ORDER BY gate: final ordered results still merge at one coordinator, so a
fully parallel ordered stream and its deployment qualification remain open.

## Large UTF-8 map-key qualification

`ELEMENT_AT(map, key)` now accepts maps whose key array uses Arrow
`LargeUtf8`, matching the existing regular-UTF8 lookup behavior. The focused
expression suite passed `142/143` tests with one pre-existing ignored case,
and the server regression suite passed `394/394`.

The same path now covers `Int32` map keys, with an exact typed lookup
regression. The focused expression suite passed `143/144` tests with one
pre-existing ignored case.

Unsigned 64-bit map keys are covered as well, including values above the
signed 64-bit range. The focused expression suite passed `144/145` tests with
one pre-existing ignored case.

## Large-list element access qualification

`ELEMENT_AT(array, index)` now accepts both regular and `LargeListArray`
representations. Positive indexes are one-based, negative indexes address from
the end, and null or out-of-range indexes produce null through the normal
Arrow take path. The focused expression suite passed `142/143` tests with one
pre-existing ignored case, and the server regression suite passed `394/394`.

The merge path now detects worker batches that are already ordered on a single
integer key and performs a bounded k-way head merge, while retaining the full
sort fallback for unsorted or non-integer inputs. The new executor regression
(`merges_sorted_integer_batches_with_k_way_heads`) passed alongside the server
suite (`394/394`). This reduces coordinator-side candidate sorting work; it
does not change the stated single-coordinator ordered-stream boundary.

## Composite partitioned-join qualification — 2026-10-15

The partitioned executor now has an exact-result regression for a two-column
inner join where independent runtime `IN` projections would retain a false
cross-product. The final composite probe returns only the two matching pairs,
and the partitioned join suite passes 45 tests with one pre-existing ignored
case. A clean committed checkout also requires the `ScanOperator` predicate
forwarding import, which is included in the same fix.

## Clean-checkout compatibility gate — 2026-10-15

A clean worktree at the release branch was compiled and tested after repairing
the staged-source serialization defects. The workspace passed 74 core, 211
SQL, 85 planner/runtime, 367 execution, 57 CLI, 394 server, 159 SQL, and 121
storage tests, with only the recorded ignored cases. The gate includes
`JSON_ARRAY_GET` (including negative indexes and `TRY`), JSON query/value
scalar-vs-container behavior, Decimal128 JSON needles, and the composite
partitioned-join regression.

The focused JSON regression also covers a `LargeUtf8` document with an
`Int32` index, confirming that scalar index coercion does not depend on a
64-bit input column.

## Current differential rerun — 2026-10-15

The server differential corpus was rerun from the current source with:

```powershell
cargo test -p kaveon-server --bin kaveon-server differential_tests --quiet
```

All **19 differential cases passed** with zero failures. This includes the
aggregate `FILTER (WHERE ...)` cases, windowed filtered aggregates, joins,
subqueries, set operations, ordering, arrays, maps, JSON, and typed literal
paths across the local and distributed execution variants. This is current
repository evidence; the remaining cloud and broader external-client gates
stay open in the parity matrix.

## Current-image AKS wire-shape qualification (2026-10-15)

The current AKS image passed two exact-repeat executions for four bounded Trino-compatible shapes: multi-array `UNNEST … WITH ORDINALITY` with null padding, BigQuery-compatible `WITH OFFSET`, `JSON_PARSE` with `JSON_VALUE` and array length, and typed `ROW` plus `MAP_KEYS` materialization. Every run returned HTTP 200 and `FINISHED` with identical result digests. The distributed UNNEST cases completed through the fragment path; constant-expression JSON and map/row cases correctly stayed single-task.

This strengthens the current-image wire evidence for arrays, JSON, maps, rows, and row-producing UNNEST. Correlated/lateral table functions, full complex-type coercion, and broader external-client differential coverage remain open.

Raw evidence: [`track2-aks-wire-shapes-2026-10-15.json`](./track2-aks-wire-shapes-2026-10-15.json).
