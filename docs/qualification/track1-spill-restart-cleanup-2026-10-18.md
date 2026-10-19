# Spill restart cleanup qualification

## Result

`SpillManager::new` now performs a bounded startup sweep of its configured spill
root. It removes only child directories with the `kaveon-spill-` private
prefix whose directory mtime is older than the configured stale age. The
default age is 24 hours and can be changed with
`KAVEON_SPILL_STALE_AFTER_SECS`. Unrelated files and recent spill directories
are preserved. This reclaims Arrow IPC runs left behind when a process is
terminated before `SpillInner::drop` runs, without making a fresh manager race
an active long-running manager that shares the root.

## Evidence

- `spill::tests::startup_reclaims_only_stale_private_spill_directories`
  verifies stale private directories are removed while unrelated root entries
  remain.
- `spill::tests::restart_age_fence_preserves_a_recent_spill_directory`
  verifies the age fence preserves a recent private directory.

## Boundary

This is local process-start cleanup with an age fence. It does not prove direct
OS `ENOSPC`, node loss during a live spill, or remote object-store spill
recovery. A shared spill root should use the same configured age policy across
coordinators and workers; operators should choose an age longer than their
longest expected query.