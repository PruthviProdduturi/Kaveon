# Track 2: quoted JSON path surrogate pairs

Quoted dot and bracket JSON member paths now decode adjacent UTF-16 high/low
surrogate escapes into one supplementary Unicode character. This closes the
previous gap where a valid pair such as `\uD83D\uDE00` could not select a key.
Both uppercase and lowercase hexadecimal digits work. Lone, reversed, truncated,
non-hex, and incorrectly paired surrogates remain invalid paths: the default
JSON_VALUE evaluator returns NULL, and JSON_VALUE_STRICT returns an error.

The focused evaluator regression covers Utf8 and LargeUtf8 documents, the lowest
and highest supplementary code points, nested member continuation, and eight
malformed sequences under both error behaviors. The parser regression verifies
that path escapes survive SQL parsing and typed RETURNING normalization.

The local planner and in-process three-worker fragment executor return the exact
same row for:

```sql
SELECT
  JSON_VALUE(JSON_PARSE('{"\ud83d\ude00":{"value":17}}'),
    '$."\uD83D\uDE00".value' RETURNING BIGINT) AS dot_value,
  JSON_QUERY(JSON_PARSE('{"\ud83d\ude00":{"value":17}}'),
    '$["\ud83d\ude00"]') AS bracket_value,
  JSON_VALUE(JSON_PARSE('{"\ud83d\ude00":17}'),
    '$."\uD83D\u0041"') IS NULL AS malformed_pair
FROM events
LIMIT 1
```

Expected and observed row: `17 | {"value":17} | true`. The distributed regression
also checks that the query memory pool returns to zero bytes.

Validation passed: 25 evaluator JSON tests, 26 SQL JSON tests, and 14 distributed
JSON tests, including the new regressions. Clippy for all targets in the three
affected crates, workspace formatting, and the Git whitespace check also passed.

Reproduce from `engine/`:

```powershell
cargo test -p kaveon-exec json_path_surrogate_pairs -- --nocapture
cargo test -p kaveon-sql json_path_surrogate_pairs -- --nocapture
cargo test -p kaveon-server distributed_json_path_surrogate_pairs -- --nocapture
cargo clippy -p kaveon-exec -p kaveon-sql -p kaveon-server --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

This is local bounded qualification; broader JSON path semantics and a pinned
external Trino comparison remain open.
