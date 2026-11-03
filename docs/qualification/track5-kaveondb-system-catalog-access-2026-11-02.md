# KaveonDB system-catalog access qualification — 2026-11-02

## Scope

This qualification covers the reserved transactional catalog exposed as
`KaveonDB`. It verifies that administrators can browse the read-only
`product`, `catalog`, and `system` projections, while ordinary principals
cannot browse or query that reserved authority. The check does not claim that
the projections are a general PostgreSQL `information_schema` replacement or
that arbitrary product records are writable through catalog-access grants.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-server catalog_access --all-targets --no-default-features -- --nocapture
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 648 filtered out
```

The covered tests include the catalog-access API's reserved-catalog grant
rejection and administrator visibility assertions, plus the catalog-access
store's reserved-catalog and role-boundary checks. `KaveonDB` is intentionally
not grantable; its read-only projections are visible only to the administrator
scope.

## Boundary

This is an Engine/KaveonDB authorization qualification. It does not qualify
JDBC/ODBC metadata compatibility, cross-source federation, or arbitrary
user-defined catalog DDL. Those remain separate compatibility work items.
