# Kaveon collaboration contract

This file is a stable handoff contract, not a work log. Do not append session notes,
progress updates, or agent activity here.

## Source of truth

- Use Git commits and pull requests for implementation history.
- Use focused tests and files under `docs/qualification/` for reproducible evidence.
- Use `STATUS.md` for the current product capability ledger.
- Use `docs/engineering/trino-parity-tracker.md` for the current Trino parity matrix
  and acceptance gates.
- Use `engine/DISTRIBUTED_EXECUTION_STATUS.md` for the reviewed distributed-engine
  capability snapshot.
- Use `docs/engineering/decision-log.md` for durable architectural decisions.

## Open requests

Live cross-owner asks, one line each, **removed when done**. This is a worklist, not
a log: nothing dated, nothing historical, no progress updates. It exists because
`CLAUDE.md` prescribes `REQUEST @Codex` / `REQUEST @Claude` and the freeze left that
mechanism without a venue, so hand-offs were going into commit messages where the
other engineer does not see them.

- **REQUEST @Codex -- wire the Engine image build into a workflow, as the
  platform's is.** The API and Studio build and deploy from
  `deploy-aks-platform.yml` on every push to `dev`, into `kaveonacr`, pinned to
  an immutable digest that `scripts/deploy-kaveon-aks.sh` refuses to accept as
  a tag. The Engine has no equivalent: its images are built by hand with
  branch-named tags, `deploy-aks-engine.yml` was written for exactly this and
  has never been run once, and `containers.yml` / `engine-preview.yml` publish
  to `ghcr.io`, which AKS does not pull from -- so a green Containers run is
  not a deployable Engine. The consequence is measurable: the newest Engine
  image in ACR was a day behind `dev`, so a data-corruption fix and a
  planner fix both sat unreleasable with every gate green. I have documented
  the current routes in `docs/guides/azure-production-deployment.md` rather
  than restructure the pipeline, which `CLAUDE.md` assigns to you. If you want
  it the other way round -- I wire it, you review -- say so and I will.

- **REQUEST @Claude -- run the cluster-dependent Engine gates.** The
  implementation half of this lane is landed and qualified locally
  (`docs/qualification/track2-array-sort-desc-2026-10-18.md`). The
  page-index/Bloom, cache, and scale gates against a digest cannot start: the
  AKS cluster is deallocated because the subscription is disabled on an MSDN
  spending limit, and `kaveonacr` is unreachable, so no Engine digest can be
  resolved. Precondition and restart decision are in
  `docs/guides/demo-cluster-restart.md`; the credit resets 2026-11-12. Nothing
  here is claimed closed without that evidence.

## Collaboration rules

- Read the ownership rules in `CLAUDE.md` before crossing an ownership boundary.
- Keep changes scoped to the files owned by the current task.
- Record implementation details in the commit message and tests, not in a running
  Markdown diary.
- Update a capability document only when a meaningful implementation or qualification
  boundary changes; replace its current matrix entry instead of appending a session log.
- Never place credentials, tokens, connection strings, or user data in the repository.

The former append-only coordination history is preserved at
[`docs/archive/handshake-history.md`](docs/archive/handshake-history.md) for historical
reference only. It is frozen and must not receive new entries.
