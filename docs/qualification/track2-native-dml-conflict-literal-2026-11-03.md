# Track 2 native DML — literal conflict document update — 2026-11-03

## Boundary

Product-record `INSERT ... ON CONFLICT ... DO UPDATE` now accepts one bounded
literal document assignment:

```sql
ON CONFLICT (id)
DO UPDATE SET document_json = '{"name":"replacement"}'
```

The JSON-typed spelling `CAST('{...}' AS JSON)` is equivalent. The literal is
used only on the conflict/update path; a newly inserted key still publishes
the document from `VALUES`. The existing identity-target, owner-isolation,
revision/CAS, `RETURNING`, and 1,000-row expansion limits remain unchanged.
`excluded.document_json` remains the supported incoming-row assignment.

Arbitrary expressions, predicates other than the existing identity no-op,
non-primary-key targets, source-backed `INSERT ... SELECT` literal overrides,
and general typed/lake-table DML remain rejected.

## Evidence

Parser adaptation and the transaction API exercise the literal override and
verify that the returned document is the conflict literal:

```text
cargo test -p kaveon-sql product_insert_supports_bounded_on_conflict_do_update -- --nocapture
cargo test -p kaveon-server --bin kaveon-server sql_endpoint_on_conflict_do_update_revises_owned_product -- --nocapture
```

Result on 2026-11-03: both focused tests passed.
