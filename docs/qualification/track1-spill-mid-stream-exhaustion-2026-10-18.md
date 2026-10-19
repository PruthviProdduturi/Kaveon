# Mid-stream spill exhaustion qualification

## Result

`mid_stream_write_failure_releases_bytes_and_removes_partial_artifact`
simulates exhaustion of the shared spill device budget after an Arrow stream
has already written data. The bounded writer rejects the next write, drops the
unfinished writer, removes its `.partial` artifact, and returns the shared
spill reservation to zero.

This covers the failure point where a spill run is partially materialized and
the next write cannot be admitted. It also verifies that no directory artifact
remains after the failed run.

## Boundary

The test uses Kaveon's enforced spill quota to deterministically model a full
device. Direct operating-system `ENOSPC`, process restart cleanup, node loss,
and remote object-store failure remain separate qualification work.
