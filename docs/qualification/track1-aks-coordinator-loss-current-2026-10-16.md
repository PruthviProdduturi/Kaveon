# Track 1 AKS: current-image coordinator-loss qualification

## Result

The current AKS deployment passed the bounded fail-closed coordinator-loss
qualification. The harness observed an active distributed query, restarted the
coordinator, and verified that the interrupted query was rejected/cleaned up
rather than reported as a surviving query.

All checks passed:

- the active query was observed running;
- coordinator interruption was observed;
- the coordinator became Ready after restart;
- all three workers became Ready after restart;
- a post-restart exact probe passed;
- retained exchange/spill state did not grow.

The report records the current API, Studio, and Engine image digests and is
retained in `track1-aks-coordinator-loss-current-2026-10-16.json`.

## Boundary

This is bounded single-coordinator fail-closed evidence. It does not claim
multi-coordinator continuity or transparent query survival across a coordinator
failure; those remain outside the current gate.
