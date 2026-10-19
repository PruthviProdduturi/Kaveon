# Track 1: exact source bytes for broadcast admission

## Scope

The server's durable-statistics planning path now retains exact stored bytes
alongside the row count and immutable source identity it already reads. Parquet
footers and Delta add-action sizes supply the bound without reading data pages.
The existing Iceberg manifest path also retains its byte total. Delta sizes are
unknown when any active add action omits a size or the sum overflows; a footer
fallback can still provide exact bytes when it already runs for row metadata.

Broadcast requires a known build byte bound within 256 MiB, at most 1,000,000
estimated rows, and the existing four-to-one probe/build row ratio. A row count
alone no longer admits broadcast. Only a durable statistics document matching
the current source identity can reduce join estimates through selectivity;
stale documents stay available for inspection and refresh. Exact source bytes
take precedence over document bytes, including an incomplete document's zero.

## Evidence

Commands from `engine/` on Windows:

```text
cargo test -p kaveon-optim --lib statistics::tests:: --quiet
19 passed; 0 failed
cargo test -p kaveon-storage --lib source_statistics::tests:: --quiet
6 passed; 0 failed
cargo test -p kaveon-server exact_source_bytes_plan_broadcast_and_stale_stats_keep_partitioned_results -- --nocapture
1 passed; 0 failed
cargo test -p kaveon-server planner::tests:: --quiet
56 passed; 0 failed
cargo test -p kaveon-server stale_statistics_stay_stored_but_never_answer --quiet
1 passed; 0 failed
cargo clippy -p kaveon-optim -p kaveon-storage -p kaveon-server --all-targets -- -D warnings
passed
cargo fmt -p kaveon-optim -p kaveon-storage -p kaveon-server -- --check
passed
git diff --check
passed
```

The optimizer regression checks an oversized current source against a tiny
stale document, missing source bytes even with a current document, and exact
source-byte precedence. Storage regressions check Parquet bytes, Delta footer
fallback, complete add-action sizes, missing sizes on either file, and overflow.

The server regression plans a left join over eight current Parquet rows with
no stored ANALYZE document and a one-row limited build: its exchange broadcasts.
It then installs a stale document whose old bounds exclude the current build
predicate: the unbounded filtered build retains the current source bound and
uses partitioned exchange. Both plans return the same eight exact rows through
local execution and three-worker in-process fragments, including seven NULL
extensions, with zero retained query memory. Automatic refresh is disabled in
the fixture so the stale case stays deterministic.

## Boundary

This qualifies stored-byte admission and stale/missing metadata behavior, not
decoded hash-table memory estimation or a general cost-based optimizer. The
three-worker execution uses in-process fragments and exchanges. Live AKS skew,
pressure, larger volumes, and broader topology qualification remain open.
