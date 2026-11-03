# Registered-source execution boundary — 2026-11-02

## Scope

Kaveon now has an explicit connector execution seam for a registered source.
The API resolves the source through KaveonDB, then sends the complete bounded
statement to exactly one source through the existing connection pool. This
preserves source-native SQL execution and keeps credentials, retries, timeout
handling, and result bounds in the pool layer.

## Evidence

```text
cd api
python -m unittest database.test_connectors -v
Ran 1 test ... OK
```

The test verifies that SQL text, row bounds, timeout, actor, and role reach the
pool unchanged and that the pool result is returned unchanged.

## Boundary

This is one-source pushdown only. It does not claim cross-source joins,
federated planning, connector-specific predicate pushdown, JDBC/ODBC wire
compatibility, or Engine execution of external database statements. Those
remain separate qualification gates.
