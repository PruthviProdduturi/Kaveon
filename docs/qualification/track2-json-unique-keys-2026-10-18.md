# Track 2 JSON `WITH UNIQUE KEYS` qualification

## Scope

This qualification covers the bounded Trino SQL/JSON constructor clause
`JSON_OBJECT(... WITH UNIQUE KEYS)`. Kaveon's JSON object evaluator already
rejects duplicate keys instead of silently selecting a winner. The SQL parser
now removes the explicit clause and routes the constructor through that
existing fail-closed behavior.

`WITHOUT UNIQUE KEYS` remains explicitly rejected. Implementing it would
require defining and testing duplicate-key winner and serialization-order
semantics; accepting it and relying on a map implementation would not be a
safe compatibility claim.

## Evidence

The SQL parser tests cover:

- SQL-standard `:` and `VALUE` key/value forms with `WITH UNIQUE KEYS`;
- nested `JSON_OBJECT` calls and quoted text containing the clause words;
- rejection of `WITHOUT UNIQUE KEYS` with an actionable error.

The server differential corpus includes a `JSON_OBJECT ... WITH UNIQUE KEYS`
query alongside the existing JSON constructor cases. Duplicate keys continue
to use the evaluator regression that asserts a deterministic execution error.

## Boundary

This is a bounded constructor-clause qualification. Full JSON coercion,
`FORMAT JSON`, arbitrary key/value expressions, `WITHOUT UNIQUE KEYS`, and
broader external-client coverage remain open in the parity tracker.
