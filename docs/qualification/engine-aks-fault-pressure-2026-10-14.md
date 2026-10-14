# AKS distributed fault and pressure qualification — 2026-10-14

This run exercises the current committed Engine image on the three-worker AKS
deployment. The raw credential-safe report is retained locally as
`tmp/engine-aks-fault-pressure-current.json`.

## Artifact and topology

- Source revision: `f68df663ca042a4ea6848033b152fcbe250c0013`
- Image: `kaveonacr.azurecr.io/kaveon-engine@sha256:f069e56542ae89f0f52e7eeefa1e8e435baccad37e3aa0ec98159be70119b1f3`
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

This qualifies the current three-worker image for the bounded loss, retry,
cancellation, and pressure scenarios above. It does not qualify coordinator
failover, node-drain recovery, dynamic runtime filtering, connector pushdown,
or a sustained multi-hour soak.
