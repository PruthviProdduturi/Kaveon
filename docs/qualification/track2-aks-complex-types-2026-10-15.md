# Track 2: complex-type wire probe on AKS

**Date:** 2026-10-15  
**Environment:** AKS `kaveon-aks`, namespace `kaveon`  
**Engine image:** `kaveonacr.azurecr.io/kaveon-engine@sha256:215184a5bc538369bf8bc16fe66b3beb1c891326e9263e23c6864e6ccaef88f4`

The authenticated `/v1/trino/statement` endpoint was probed with scalar
complex-type statements. Every response page was drained.

| Statement shape | Result |
|---|---|
| `SELECT ARRAY[1, 2, 3]` | Passed; exact nested row returned |
| `SELECT ARRAY_JOIN(ARRAY['a', 'b'], ',')` | Passed; returned `a,b` |
| `SELECT ELEMENT_AT(ARRAY[10, 20], 2)` | Passed; returned `20` |
| `SELECT ELEMENT_AT(MAP(ARRAY['a'], ARRAY[42]), 'a')` | Passed; returned `42` |
| `SELECT JSON_VALUE(JSON '{"a": 7}', '$.a')` | Failed closed: unsupported `JSON` typed-string expression |
| `SELECT JSON_FORMAT(JSON '{"a": 7}')` | Failed closed: unsupported `JSON` typed-string expression |

The array and map results establish a passing bounded complex-type wire path.
The JSON failures are a concrete Trino syntax gap: equivalent
`JSON_PARSE(...)` function forms are implemented, but the typed JSON literal
form is not yet accepted by the deployed image. This evidence keeps the JSON
surface partial until the parser change is built, deployed, and requalified.
