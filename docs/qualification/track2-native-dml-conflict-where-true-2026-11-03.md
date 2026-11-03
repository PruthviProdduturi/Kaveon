# Bounded `ON CONFLICT ... WHERE TRUE` qualification

## Contract

KaveonDB accepts the PostgreSQL-shaped product upsert form

```sql
INSERT INTO product.datasets (id, document_json)
VALUES ('ds-1', '{"name":"new"}')
ON CONFLICT (id)
DO UPDATE SET document_json = excluded.document_json
WHERE TRUE
```

`WHERE TRUE` is a semantic no-op. The contract also accepts the equivalent
identity predicate `WHERE id = excluded.id` (in either operand order): the
`(id)` conflict target already guarantees that identity, so no row-dependent
expression needs to be evaluated. Both forms follow the existing revision/CAS
`ProductDmlCommand::Upsert` path. Arbitrary conflict predicates, named
constraints, non-`id` conflict targets, and arbitrary assignments remain
rejected; this qualification does not claim general PostgreSQL
conflict-predicate support.

## Evidence

- `engine/crates/sql/src/parser.rs`
  `product_insert_supports_bounded_on_conflict_do_update` proves the literal
  predicate parses, retains the bounded upsert flags, and lowers to the same
  `ProductDmlCommand::Upsert` as the predicate-free form.
- The transaction API upsert test exercises the identity predicate through the
  revisioned product commit path and verifies the resulting revision.
- The parser test covers both operand orders and proves that the identity
  predicate retains the bounded upsert flags.
- The negative cases in the same parser test continue to reject a non-`id`
  conflict target, a nontrivial predicate, and an arbitrary assignment.

## Boundary

This is syntax compatibility for deterministic no-op predicates only. The
identity predicate is accepted because its truth follows from the declared
`(id)` conflict target; it is not evaluated as a general expression. General
conflict predicates would require carrying a conflict-row snapshot through the
transactional planner and preserving revision/CAS semantics for every
supported expression shape.
