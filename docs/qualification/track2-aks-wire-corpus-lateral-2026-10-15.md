# Track 2 AKS wire corpus: constant lateral `UNNEST`

## Deployment

- Engine image: `kaveonacr.azurecr.io/kaveon-engine@sha256:6a4fa35e1d511bdd533d6d3ee22c331137775a0848544bfd0fd98c2e6dea5b18`
- Catalog snapshot: `sha256:652742525737d79eb579cdef4a8386b6fa6c9f3feedc60b197bd3406ab73d1f3`
- Topology: one coordinator and three workers, all participating.

## Corpus

The authenticated HTTPS wire corpus ran eleven SQL shapes twice each:

- CTE with aggregate
- aggregate `FILTER`
- `GROUPING SETS`
- partitioned window frame
- `FETCH FIRST ... WITH TIES`
- scalar subquery
- array/map/JSON expressions
- `UNNEST ... WITH ORDINALITY`
- constant `CROSS JOIN LATERAL UNNEST(...)`
- `EXISTS`
- `UNION ALL`

## Result

All 22 requests finished successfully and every repeated result matched exactly.
The lateral case returned two rows (`1,10` and `1,20`) through the deployed
fragment path. The raw request/response report is retained in
`track2-aks-wire-corpus-lateral-2026-10-15.json`.

## Boundary

This qualifies constant lateral `UNNEST` only. Correlated lateral expressions,
other table functions, and the remaining broader Trino compatibility corpus
remain open.
