# AKS nested-result serialization qualification — 13 October 2026

The Engine image built from commit `4d5b32c7` was published to
`kaveonacr.azurecr.io/kaveon-engine` at digest
`sha256:7b3b3763fae225b0b80570afc32b06a1f81d816f1302dca38308bcc84a7434a9`.
Both `kaveon-coordinator` and `kaveon-worker` StatefulSets rolled successfully
to that digest on `kaveon-aks`.

Authenticated statements through the coordinator returned JSON-native values:

- `SELECT ARRAY[1, 2] ...` returned `[[1,2]]` with column type `List(Int64)`.
- `SELECT MAP(ARRAY['a'], ARRAY[3]) ...` returned `{"a":3}`.

The same conversion is used by the embedded CLI. Focused server coverage and
the full server/CLI suites passed before deployment. This qualifies supported
list, struct, and UTF-8-keyed map result materialization; nested temporal and
binary values and non-UTF-8 map keys remain outside the claim.
