# Track 1 AKS coordinator-loss gate — 2026-10-16

This is a bounded fail-closed recovery gate. It does not claim in-flight query
survival or multi-coordinator failover.

## Environment

- Context: `kaveon-aks`, namespace `kaveon`
- Coordinator and workers: Engine image
  `kaveonacr.azurecr.io/kaveon-engine@sha256:2c43a790ca59cc39155e53a8d4513c5a713b8c4e9566c336fa8357b4ea1c85fc`
- Catalog snapshot: `sha256:652742525737d79eb579cdef4a8386b6fa6c9f3feedc60b197bd3406ab73d1f3`
- Active query: `SELECT COUNT(*) FROM kaveon_events_enriched WHERE actions >= 0`
- Post-restart probe: `SELECT COUNT(*) FROM kaveon_events_dashboard`

## Checks

| Check | Result |
| --- | --- |
| Active query observed running before restart | Pass |
| Interrupted request failed closed without a partial response | Pass |
| Coordinator became Ready after restart | Pass |
| Three workers Ready after restart | Pass |
| Post-restart probe returned the exact result `24` | Pass |
| Retained coordinator exchange files did not grow | Pass |

The raw report is retained in the local qualification workspace as
`tmp/track1-aks-coordinator-loss-current.json`; credentials are not committed.
The harness scope explicitly stops at fail-closed interruption and a fresh
exact probe. Multi-coordinator failover and in-flight query continuity remain
open.
