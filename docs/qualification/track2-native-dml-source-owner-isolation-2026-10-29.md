# Bounded source-copy owner isolation qualification

Date: 2026-10-29  
Scope: KaveonDB product-record transaction API  
Status: qualified (bounded)

The bounded product-table `INSERT … SELECT` path enforces the authenticated
principal's ownership boundary before it reads source documents for
`RETURNING document_json`.

Qualified contract:

- one same-kind `kaveon.product` source table;
- literal `id IN (...)` source filter, capped at 1,000 IDs;
- source rows and documents are resolved from the transaction's pinned
  snapshot;
- a source record owned by another principal fails with HTTP 403;
- no target write is staged when the ownership check fails.

Evidence:

`transaction_api::tests::bounded_source_copy_returning_document_is_owner_isolated`
seeds a product record as `bob`, attempts a bounded source copy as `alice`,
asserts the ownership-forbidden response, and therefore verifies that the
document is not exposed or staged across principals.

Run from `engine/`:

```powershell
cargo test -p kaveon-server --bin kaveon-server transaction_api::tests::bounded_source_copy_returning_document_is_owner_isolated -- --nocapture
```

This does not expand the contract to cross-kind copies, unbounded source
scans, or lake-table DML.
