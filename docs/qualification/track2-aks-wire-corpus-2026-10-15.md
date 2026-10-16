# Track 2 AKS SQL wire corpus — 2026-10-15

The authenticated current AKS Engine image passed an exact-repeat SQL corpus
through the native statement endpoint. Every case was submitted twice, finished
successfully, and returned an identical result on both runs.

| Shape | Rows | Wall time per repeat (s) |
|---|---:|---:|
| Non-recursive CTE with grouped aggregate | 2 | 1.969 / 1.875 |
| Aggregate `FILTER (WHERE ...)` | 1 | 1.828 / 1.937 |
| `GROUPING SETS` | 3 | 2.828 / 2.938 |
| Partitioned window frame | 24 | 2.094 / 2.031 |
| `FETCH FIRST ... WITH TIES` | 2 | 2.015 / 2.063 |
| Scalar subquery | 1 | 0.531 / 0.578 |
| Array, map, and typed JSON expressions | 1 | 0.500 / 0.516 |
| `UNNEST` with ordinality | 3 | 0.641 / 0.578 |
| Correlated `EXISTS` | 24 | 2.984 / 3.141 |
| `UNION ALL` with ordering and limit | 10 | 2.922 / 3.046 |

The raw machine-readable report is
`track2-aks-wire-corpus-2026-10-15.json`. This qualifies these bounded wire
shapes on AKS. Recursive CTEs, broader correlated/lateral Apply forms, complete
complex-type coercion, general DML, and the broader external-client differential
corpus remain open.
