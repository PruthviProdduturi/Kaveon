# SQL/JSON array-range qualification — 2026-10-18

Kaveon now evaluates the bounded SQL/JSON array-range selector form
`[start to end]`. Bounds are inclusive and can be followed by the existing
object, index, wildcard, and quoted-member selectors. A range over multiple
values materializes a JSON array for `JSON_QUERY`; `JSON_VALUE` continues to
return NULL for a non-scalar result. Reversed ranges fail closed.

The evaluator clamps the upper bound to the source array before iterating. This
keeps a syntactically valid but very large bound from causing an unbounded
loop. Local coverage exercises nested object extraction, scalar rejection,
upper-bound clamping, and reversed-range rejection. The server differential
corpus includes the same range query for local and distributed comparison.

This is a bounded path extension. Full SQL/JSON strict/lax mode, filters,
methods, recursive descent, and complete coercion semantics remain outside this
qualification.
