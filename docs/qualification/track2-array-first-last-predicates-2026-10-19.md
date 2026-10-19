# Track 2 qualification: predicate `ARRAY_FIRST` and `ARRAY_LAST`

Date: 2026-10-19

Kaveon supports Trino's predicate overloads:
`ARRAY_FIRST(array, element -> boolean)` and
`ARRAY_LAST(array, element -> boolean)`. The lambda is evaluated per element
with the outer row scope preserved. The first or last element whose predicate
is true is returned; a NULL array, empty array, or no matching element returns
a typed NULL value. NULL predicate results are treated as non-matches, while a
non-boolean predicate fails closed.

## Evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-exec --lib array_first_last_predicates
```

The distributed differential corpus compares both overloads with the local
evaluator through the three-worker fragment path. The planner retains the
array element type for the scalar result, including empty and NULL rows.

External-client and cloud qualification remain part of the broader Track 2
boundary.
