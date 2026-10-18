# Track 1 final-merge shared-pressure candidate — 2026-10-18

Status: **qualified for the local bounded-memory topology; cloud and endurance qualification remain separate**.

## Boundary

Threaded final merges share one query memory budget with their partial aggregators. A refusal can otherwise arrive between a final merge batch and its retry, leaving no headroom for the spill path.

## Change

`HybridFinalMerge` records the query limit and, for threaded merges with a pressure responder and an available spill device, spills its live grouped table before sibling refusals exhaust the query budget. Tight no-responder merges use the same bounded proactive path; refusal-driven paths retain compaction. Existing reservations, spill accounting, and explicit downstream cancellation remain in force.

## Evidence

The current integration source passes the focused bounded-memory gates:

- grouped aggregate under spill matches the local result;
- the threaded TopN tail matches the serial result and leaves zero spill bytes;
- source-thread refusal spills and reports pressure handling;
- typed-null aggregate spill remains exact;
- the complete `kaveon-exec` library suite passes (491 passed, 4 ignored).

This note qualifies the current local test topology only. Distributed object-store, deployed endurance, and fault-injection qualification remain separate gates.
