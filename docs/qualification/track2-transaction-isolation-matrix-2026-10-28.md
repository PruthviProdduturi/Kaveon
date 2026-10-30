# KaveonDB transaction and isolation matrix — 2026-10-28

## Scope

This matrix describes the qualified KaveonDB product-record transaction API.
It is deliberately not a claim of PostgreSQL wire compatibility, general
row-store MVCC, or WAL-equivalent recovery.

| Contract | Qualified behavior | Not claimed |
|---|---|---|
| Begin | Reads and pins one durable catalog head as the transaction base snapshot | A database-wide lock or a live read view over arbitrary lake tables |
| Reads | Transaction-local reads include staged writes (read-your-writes) | Repeatable reads across independent transactions |
| Staging | Every statement is validated against the pinned base plus the complete staged write set; failed staging is atomic | Arbitrary SQL table mutations or deferred constraint checking |
| Commit | Publishes one prepared immutable snapshot through a conditional current-head compare-and-swap | Row-level locking, lock waits, or automatic deadlock detection |
| Conflict | A changed head produces a conflict/no-publication outcome; no partial write set is published | Automatic conflict retry or serializable scheduling across arbitrary tables |
| Revision | Product records use owner checks and revision/CAS checks; bounded upserts advance revisions | Global sequences, foreign-key enforcement, or secondary indexes |
| Savepoints | Named savepoints restore the staged write set and retain the savepoint until release | Independent durable commits or savepoints shared across sessions |
| Ownership | Transaction sessions and product records are principal-scoped; cross-principal access fails closed | Database roles/row policies for arbitrary user-defined tables |
| Durability | Local filesystem and ADLS conditional-head implementations are the supported backends; commit outcomes are observable and replay-safe where qualified | S3, multi-coordinator consensus, or cloud restore without a backend-specific gate |
| Recovery | Ambiguous outcomes expose an explicit recovery path; the durable head remains authoritative | PostgreSQL WAL replay or point-in-time recovery semantics |

## Evidence

SQL Server transaction-control spellings are normalized only when the entire
request is a control statement: `BEGIN|COMMIT|ROLLBACK TRAN` and
`... TRANSACTION` are equivalent to their canonical forms. The parser
regression `accepts_sql_server_transaction_control_aliases` covers all six
aliases; multi-statement requests remain rejected.

The contract is covered by:

- `kaveon_catalog::product_transaction::tests::metadata_pins_identity_and_base_snapshot`
- `kaveon_catalog::product_transaction::tests::failed_stage_preserves_the_prior_transaction_view`
- `kaveon_catalog::product_transaction::tests::savepoint_restores_staged_snapshot_without_publishing`
- `kaveon_catalog::product_transaction::tests::concurrent_transactions_conflict_on_the_head`
- `transaction_api::tests::transaction_sessions_do_not_survive_registry_restart`
- `transaction_api::tests::product_updates_and_deletes_are_record_owner_isolated`
- `transaction_api::tests::sql_endpoint_bounded_merge_batch_updates_and_inserts_atomically`

The local catalog and server transaction suites pass with this bounded matrix.
Cloud backend recovery, multi-coordinator failover, general DML, constraints,
indexes, and full MVCC remain separate qualification work.
