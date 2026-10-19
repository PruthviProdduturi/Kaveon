# Track 2: DECIMAL MAP cast members in UNNEST

The SQL parser now accepts bounded `DECIMAL(precision, scale)` and
`NUMERIC(precision, scale)` members in `MAP(key_type, value_type)` casts.
DuckDB's custom-type modifier grammar cannot represent the nested decimal
parentheses; a failed parse can now use the generic parser's native MAP type
AST when the statement contains a decimal MAP cast with scalar members.
The logical planner retains its existing Decimal128 precision (1 through 38)
and scale (0 through precision) checks.

The local and in-process three-worker fragment regression executes both:

```sql
SELECT key, value
FROM UNNEST(CAST(NULL AS MAP(DECIMAL(10, 2), DECIMAL(8, 3))))
  AS t(key, value);

SELECT key, value, id, ord
FROM UNNEST(
  CAST(NULL AS MAP(DECIMAL(10, 2), DECIMAL(8, 3))), ARRAY[1, 2]
) WITH ORDINALITY AS t(key, value, id, ord)
ORDER BY ord;
```

The first query has Decimal128(10, 2) and Decimal128(8, 3) output columns and
zero rows. The second has those same decimal columns, BIGINT `id` and `ord`,
and exactly `(NULL, NULL, 1, 1)` and `(NULL, NULL, 2, 2)`. The test asserts the
local output schema and each returned distributed batch schema, exact local
rows, exact local/distributed equality, and zero retained query memory after
each execution. The populated sibling array ensures distributed precision
and scale assertions run even when the standalone empty relation emits no
batches.

Validation on 2026-10-18 from `engine/`:

```text
cargo test -p kaveon-sql -- --nocapture
230 passed, 0 failed

cargo test -p kaveon-server distributed_unnest_typed_null_decimal_map_preserves_schema_and_padding -- --nocapture
1 passed, 0 failed

cargo clippy -p kaveon-sql -p kaveon-server --all-targets -- -D warnings
passed

git diff --check
passed
```

SQL regressions also assert the precise MAP cast target for decimal keys and
NUMERIC values, reject invalid precision/scale in either member, and retain
the rejection of nested MAP/ARRAY members through this fallback. This is
bounded typed-NULL/schema and UNNEST padding evidence. Non-null decimal MAP
casts, arbitrary nested complex members, all mixed-dialect syntax combinations,
external-client wire behavior, live pinned-Trino comparison, and cloud/AKS
qualification remain outside this evidence.
