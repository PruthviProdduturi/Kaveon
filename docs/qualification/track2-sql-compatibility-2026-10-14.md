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

## Current qualified local surface

The current tests cover the implemented Trino-shaped surface including core
SELECT semantics, joins and semi/anti joins, CTEs and derived relations, set
operations, grouping sets, windows and QUALIFY, arrays/maps/rows, JSON paths,
prepared statements and typed parameters, metadata statements, global ordering
and limits, and local/distributed exact-result execution.

## Explicit remaining boundaries

The full Track 2 claim remains open. The compatibility reference still records
recursive CTEs, broader correlated/lateral subqueries, unsupported complex-type
combinations, broad lake-table DML, full external-client prepare/describe/bind
interoperability, and the remaining distributed-window shapes as open. Those
boundaries are rejected or documented explicitly; they are not counted as
implemented parity.

This report is local evidence. AKS workload and external Trino-client
qualification are separate gates in `docs/engineering/trino-parity-tracker.md`.

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
Object-directory source injection and cloud snapshot qualification remain
open.
