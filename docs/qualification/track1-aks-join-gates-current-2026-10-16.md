# Track 1 AKS: current-image join gates

## Result

The current AKS Engine image passed the bounded distributed join qualification:

- three catalog-compatible workers were active;
- left outer self-join results matched on two exact repeats;
- full outer cross-source join results matched on two exact repeats;
- the skewed grouped join matched on two exact repeats;
- every case finished successfully with no admission refusal.

The run used Engine image `kaveonacr.azurecr.io/kaveon-engine@sha256:6a4fa35e1d511bdd533d6d3ee22c331137775a0848544bfd0fd98c2e6dea5b18` and catalog snapshot `sha256:652742525737d79eb579cdef4a8386b6fa6c9f3feedc60b197bd3406ab73d1f3`.

The raw harness report is retained in
`track1-aks-join-gates-current-2026-10-16.json`.

## Boundary

This strengthens the bounded join evidence for Track 1 item 3. Physical disk exhaustion, larger input volumes, sustained mixed-workload join pressure, and repeated node-loss endurance remain open gates.
