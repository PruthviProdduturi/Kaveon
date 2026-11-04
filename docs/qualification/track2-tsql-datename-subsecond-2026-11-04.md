# T-SQL `DATENAME` subsecond parts qualification

Date: 2026-11-04  
Scope: bounded SQL Server/Fabric `DATENAME` millisecond, microsecond, and
nanosecond parts over typed timestamp expressions.

## Contract

Kaveon lowers `DATENAME(millisecond|microsecond|nanosecond, value)` to the
existing typed temporal evaluator and returns the subsecond component as
invariant decimal text. The timestamp execution contract is microsecond
precision, so nanosecond output is the represented microsecond value scaled by
1,000; it does not claim SQL Server's 100-nanosecond `datetime2` storage
precision. NULL inputs remain NULL. Locale-specific names and unsupported
datepart expressions remain outside this qualification.

## Evidence

Focused distributed differential:

```text
$env:KAVEON_DIFF_ONLY='tsql_datename_subsecond_parts'
cargo test -p kaveon-server --bin kaveon-server differential_sweep --no-fail-fast --quiet
running 1 test
.
test result: ok. 1 passed; 0 failed; 0 ignored
```

The case runs the same statement through local execution and the in-process
three-worker fragment path over the standard dictionary, plain, fragmented,
partitioned, and clustered Parquet layouts. Exact canonical rows match.

## Boundary

This qualifies the bounded scalar read shape only. It does not expand
nanosecond timestamp storage, culture-aware formatting, dynamic datepart
expressions, or external-client compatibility.
