# Track 2 native product DML column order — 2026-10-18

Status: **qualified for the declared product-record transaction facade**.

## Shape qualified

`INSERT INTO product.datasets (document_json, id) VALUES (...)` now resolves
the two supported fields by name. The parser accepts either the canonical
`(id, document_json)` order or the equivalent reordered form and maps each
value to the correct product-record field before staging. Duplicate, missing,
unknown, or extra columns still fail closed.

## Evidence

- `kaveon-sql` parser regression accepts reordered columns and rejects a
  duplicate `id` column.
- `kaveon-server` transaction regression commits a reordered insert and reads
  back the expected product document under the requested ID.
- The transaction boundary, owner checks, revision CAS, and JSON document
  validation are unchanged.

## Boundary

This is a compatibility improvement to Kaveon's product-record DML facade. It
does not make arbitrary lake tables writable. `INSERT ... SELECT`, generated
columns, conflict clauses, `RETURNING`, arbitrary typed assignments, general
constraints, and multi-statement SQL transactions remain outside the qualified
surface.
