# Track 2: complex-type wire probe on AKS

**Date:** 2026-10-15  
**Environment:** AKS `kaveon-aks`, namespace `kaveon`  
**Engine image:** `kaveonacr.azurecr.io/kaveon-engine@sha256:0ea4244c392267f5d821935cb63f4c2a251c30be2f44a33554b340e1eb63a1f6`

The authenticated `/v1/trino/statement` endpoint was probed with scalar
complex-type statements. Every response page was drained.

| Statement shape | Result |
|---|---|
| `SELECT ARRAY[1, 2, 3]` | Passed; exact nested row returned |
| `SELECT ARRAY_JOIN(ARRAY['a', 'b'], ',')` | Passed; returned `a,b` |
| `SELECT ELEMENT_AT(ARRAY[10, 20], 2)` | Passed; returned `20` |
| `SELECT ELEMENT_AT(MAP(ARRAY['a'], ARRAY[42]), 'a')` | Passed; returned `42` |
| `SELECT JSON_VALUE(JSON '{"a": 7}', '$.a')` | Passed; returned `7` |
| `SELECT JSON_FORMAT(JSON '{"a": 7}')` | Passed; returned `{"a":7}` |

The array, map, and typed-JSON results establish a passing bounded
complex-type wire path on the current image. The parser change lowers typed
JSON literals to the existing JSON parse path and is now qualified on AKS.
The JSON surface remains partial for broader coercion and complete Trino path
semantics; those cases still require differential coverage.
