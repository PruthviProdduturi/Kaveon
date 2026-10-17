# Track 1 AKS join and skew gate — 2026-10-16

This is a bounded deployed qualification of distributed join correctness. It
does not close the larger disk-exhaustion, high-cardinality, or sustained
mixed-workload gates in the parity tracker.

## Environment

- Context: `kaveon-aks`, namespace `kaveon`
- Topology: three Ready workers and one coordinator
- Worker image digest: `kaveonacr.azurecr.io/kaveon-engine@sha256:2c43a790ca59cc39155e53a8d4513c5a713b8c4e9566c336fa8357b4ea1c85fc`
- Catalog snapshot: `sha256:652742525737d79eb579cdef4a8386b6fa6c9f3feedc60b197bd3406ab73d1f3`
- Worker restarts during the gate: zero

## Gate

The repository harness `scripts/qualify-aks-join-gates.py` submitted each case
twice through the TLS coordinator endpoint and required both executions to
finish with byte-identical result data.

| Case | Run 1 | Run 2 | Exact repeat |
| --- | ---: | ---: | --- |
| Left outer self-join grouped by region | 2,484 ms | 2,141 ms | Pass |
| Full outer cross-source join | 2,167 ms | 2,364 ms | Pass |
| Skewed grouped self-join | 2,223 ms | 2,330 ms | Pass |

All six requests finished, all three workers were catalog-compatible, and the
gate passed. The raw report was written to the local qualification workspace
(`tmp/track1-aks-join-gates-current.json`) and is intentionally not committed.

This evidence qualifies exact-result execution for the declared bounded cases;
it does not claim general skew splitting, disk-exhaustion recovery, or a
universal latency SLO.
