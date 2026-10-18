# Track 1 final-merge shared-pressure candidate — 2026-10-18

Status: **candidate; qualification remains open**.

## Boundary

Threaded final merges share one query memory budget with their partial aggregators. A refusal can otherwise arrive between a final merge batch and its retry, leaving no headroom for the spill path.

## Change

The integration branch is evaluating a `HybridFinalMerge` policy that records the query limit and, for threaded merges with a pressure responder and an available spill device, can spill its live grouped table before sibling refusals exhaust the query budget. Serial merges retain refusal-driven spilling. Existing reservations, spill accounting, and cleanup remain in force.

## Evidence

The earlier isolated candidate at `8042fdf8` passed the three focused merge cases, but that commit is not an ancestor of the current integration branch. The current branch has additional threshold, prepayment, and downstream-tail handling changes, so those results cannot be attributed to the current source until the full focused suite is rerun.

- The aggregate-under-spill case passes on the current branch.
- The TopN-tail case is currently red under the current branch's downstream cleanup/pressure configuration and must pass before this behavior is qualified.
- Typed-null spill, the complete `fragment_exec::tests` subset, and the `kaveon-exec final_merge` suite must be rerun against the final source.

This note records an active qualification boundary for the current local test topology. It does not qualify the behavior. Distributed object-store and deployed endurance qualification remain separate gates.
