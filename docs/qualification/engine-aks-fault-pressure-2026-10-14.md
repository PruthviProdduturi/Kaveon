# AKS distributed fault and pressure qualification — 2026-10-14

This run exercises the current committed Engine image on the three-worker AKS
deployment. The raw credential-safe report is retained locally as
`tmp/engine-aks-fault-pressure-a4cc215f.json`.

## Artifact and topology

- Source revision: `a4cc215f`
- Image: `kaveonacr.azurecr.io/kaveon-engine@sha256:c997febb6d14040bd9f26d21d1f96c90f33a6c5faf7a728cf2ba350e8181f06f`
- Cluster: `kaveon-aks`, namespace `kaveon`
- Workers: three compatible workers before and after the test
- Catalog snapshot: `sha256:6aa9eb449dbd44613f1922bc43eab3f562d0c055a6d7beb572d0d331ad9a70fd`

## Results

The bounded qualification used one worker-loss round, four concurrent grouped
queries, and one pressure round. Every gate passed:

- worker loss observed during a running distributed scan;
- retry completed after the replacement worker became Ready;
- worker-loss result matched the baseline exactly (`COUNT(*) = 504,000,000`);
- replacement worker retained the required catalog snapshot;
- all concurrent grouped results matched the exact baseline hash;
- cancellation cleanup passed;
- no unexpected worker restart, retained exchange, spill-file growth, or
  memory-limit breach was observed;
- an exact post-recovery probe passed.

This qualifies the current three-worker image for the bounded loss, retry, cancellation, and pressure scenarios above. It does not qualify coordinator failover, dynamic runtime filtering, connector pushdown, or a sustained multi-hour soak.
