# Global TopN empty-worker result qualification

## Scope

Distributed positive-limit TopN queries may have every worker filtered empty.
The coordinator now preserves the worker schema and returns a zero-row result
instead of reporting that the distributed TopN completed without result rows.

## Evidence

The server regression `distributed_top_n_keeps_the_schema_when_all_workers_are_empty`
passes an empty candidate set through the coordinator merge helper and asserts
that the returned batch retains the declared schema with zero rows. The
production completion path applies the same empty-batch fallback after all
worker tasks finish.

## Boundary

This covers empty positive-limit results only. It does not claim a fully
parallel global sort or change TopN ordering semantics for non-empty inputs.
