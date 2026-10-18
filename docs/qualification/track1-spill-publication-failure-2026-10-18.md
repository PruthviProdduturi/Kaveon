# Spill publication failure qualification

## Result

The spill writer fails closed when the final publication rename cannot be
completed. `failed_run_publication_releases_bytes_and_removes_partial_artifact`
creates a destination conflict after writing the Arrow stream, then verifies
that the unpublished `.partial` file is removed and the shared spill byte
reservation returns to zero.

The pre-existing destination conflict is preserved. This keeps cleanup scoped
to the writer's own artifact and avoids deleting an unrelated path in the
private spill directory.

## Boundary

This is a local publication and accounting guard. It does not qualify physical
disk exhaustion, process restart cleanup, node loss, or remote object-store
spill behavior. Those remain separate operational gates.
