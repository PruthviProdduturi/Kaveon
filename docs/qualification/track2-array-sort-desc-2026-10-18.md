# Track 2 qualification: `ARRAY_SORT_DESC`

Date: 2026-10-18

Commit: the change implementing `ARRAY_SORT_DESC` on `dev`.

## Contract

`ARRAY_SORT_DESC(array)` orders present elements descending and places NULL
elements **last**, matching Trino.

The ordering is deliberately not `ARRAY_SORT` reversed. NULL is not one of the
values being ordered, and Trino sorts it after every value in both directions,
so reversing the ascending result would lead with the nulls instead of
trailing them. The direction is therefore applied only when both compared
elements are present, and the null cases keep the ordering the shared
comparator already defines.

Accepted element types are exactly those `ARRAY_SORT` accepts through its
generic path: signed and unsigned integers, `Float32`/`Float64`, `Date32`/
`Date64`, `Time32`/`Time64`, `Timestamp` at every supported unit,
`Decimal128`, `Utf8`/`LargeUtf8`, `Boolean`, and orderable nested
`List`/`LargeList`/`Struct` elements. Both regular and large Arrow lists are
accepted. An unsupported element type fails closed and the message names
`ARRAY_SORT_DESC` rather than `ARRAY_SORT`.

`ARRAY_SORT_DESC` is also allowed as an `UNNEST` input, as `ARRAY_SORT`
already was, because it is a typed LIST producer. It is listed in
`SHOW FUNCTIONS`.

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-exec array_sort_desc
```

Observed result:

```text
running 2 tests
test expr_eval::tests::array_sort_desc_orders_values_descending_and_keeps_nulls_last ... ok
test expr_eval::tests::array_sort_desc_orders_strings_descending ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 436 filtered out
```

Data shape: a four-element `Int64` list containing `3`, NULL, `1`, `2`, and a
three-element `Utf8` list containing `apple`, `pear`, `fig`.

Pass criteria, both met:

- The `Int64` case returns `[3, 2, 1, NULL]`. The reversal it is distinguished
  from would have returned `[NULL, 3, 2, 1]`.
- The `Utf8` case returns `[pear, fig, apple]`, exercising the comparator on a
  non-numeric type.

## Distributed and Trino-wire evidence

The server differential corpus now includes
`array_sort_desc_nulls_last`:

```sql
SELECT ARRAY_SORT_DESC(ARRAY[3, NULL, 1, 2]) AS sorted_values
FROM {T} LIMIT 1
```

The case is compared across dictionary, plain, multi-file, partitioned, and
clustered Parquet layouts, then against the in-process three-worker fragment
path. The expected canonical value is `[3, 2, 1, NULL]`; all layouts and the
distributed path must match the same result. The case can be run in isolation
with:

```powershell
$env:KAVEON_DIFF_ONLY = "array_sort_desc_nulls_last"
cargo test -p kaveon-server --bin kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths --quiet
```

The Trino-compatible HTTP route is also covered by
`trino_wire_array_sort_desc_preserves_null_last_result`. It submits the query
through the `X-Trino-Prepared-Statement` wire header, executes it through the
Trino statement route, fetches the result page, and asserts the JSON array
shape and NULL-last ordering:

```powershell
cargo test -p kaveon-server --bin kaveon-server trino_wire_array_sort_desc_preserves_null_last_result --quiet
```

This is Kaveon's distributed and wire serialization evidence. It is not a
claim of a live external Trino differential run.

## Workspace gates

Run from `engine/`, all on the same tree as the change:

| Gate | Command | Result |
|---|---|---|
| Format | `cargo fmt --all -- --check` | exit 0 |
| Clippy | `cargo clippy --workspace --all-targets -- -D warnings` | exit 0, no warnings |
| Tests | `cargo test --workspace` | 15 suites, 1,640 passed, 0 failed, 6 ignored |
| Release | `cargo build --release` | exit 0 |

The full-workspace run matters more than the two focused tests here: the
ascending `ARRAY_SORT` path was refactored to share one direction-aware
comparator, so every existing `ARRAY_SORT` regression is also a regression
test for this change. All of them still pass.

## Not qualified

**No cluster evidence.** The distributed page-index/Bloom, cache, and scale
gates were not run, and no claim is made about them. The AKS cluster is
deallocated because the subscription is disabled on a spending limit, and
`kaveonacr` is unreachable, so the current Engine digest could not even be
resolved — see [`../guides/demo-cluster-restart.md`](../guides/demo-cluster-restart.md).

What that leaves open for this function specifically:

- No external Trino-wire comparison against the pinned Trino version, so the
  NULL placement and the collation used for text ordering are matched to
  documented Trino behaviour rather than to an observed Trino result.
- The `LargeListArray` branch delegates to the same qualified function as the
  regular-list branch but has no test of its own.

These are stated rather than closed. The boundary this report claims is local
execution correctness on the listed element types, nothing wider.
