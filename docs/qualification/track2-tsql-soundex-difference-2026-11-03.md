# T-SQL `SOUNDEX` / `DIFFERENCE` qualification — 2026-11-03

Kaveon now evaluates SQL Server's bounded `SOUNDEX(text)` and
`DIFFERENCE(left, right)` scalar forms on the vectorized text path.

The contract is deliberately explicit: input is ASCII alphabetic text after
the first character (punctuation is ignored), `SOUNDEX` returns the invariant
four-character code, and `DIFFERENCE` counts equal code positions. NULL input
propagates to NULL. Non-ASCII input also returns NULL rather than silently
transliterating under a locale or worker-specific code page.

## Evidence

The local evaluator regression is:

```text
cargo test -p kaveon-exec soundex_and_difference_preserve_nulls_and_reject_unicode -- --nocapture
test result: ok. 1 passed; 0 failed
```

The same two expressions are included in the server differential corpus under
`tsql_soundex_difference`, so the qualification gate compares local and
three-worker execution across the existing Parquet encodings and exact result
serialization path.

This does not claim locale-aware SQL Server phonetic behavior, arbitrary code
pages, or external Fabric/Azure SQL execution.
