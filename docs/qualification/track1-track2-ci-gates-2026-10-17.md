# Track 1/2 validation gates — 2026-10-17

The commit-specific workflow gates completed successfully for source commit
`e4ee88d7b7b1378ec01d7278883188a467a8293a`:

| Gate | Run | Result |
| --- | ---: | --- |
| CI | `39032805606` | success |
| Engine | `39032813007` | success |
| Containers | `39032805720` | success |

The Engine workflow ran format, workspace/all-target Clippy, and the complete
workspace test suite. The Containers workflow built and smoke-tested the local
stack. CI completed the web/API checks and secret scan.

These gates validate the pushed source and build artifacts. They do not close
the capability boundaries in the parity tracker; those still require the
workload and distributed-runtime evidence named by each tracker row.

## Latest validated source

The five-way join-reorder corpus extension in
`e1777e504e7e09c57dbbf1856088cd21227bf63c` passed the complete Engine release
workflow `39109416024` on 2026-10-18:

| Gate | Result |
| --- | --- |
| Format, workspace Clippy, full workspace tests | success |
| Linux x86_64 release build and CLI workflow | success |
| Windows x86_64 release build and CLI workflow | success |
| macOS ARM64 release build and CLI workflow | success |
| Dev release publication | success |

This validates the source and release artifacts for the four-way and five-way
join-reorder differential corpus. It does not change the remaining distributed
qualification boundaries in the parity tracker.

The current source also passes the focused Trino compatibility suite:

```text
cargo test -p kaveon-server trino_ --quiet
29 passed; 0 failed; 401 filtered out; finished in 0.28s
```

The complete server differential corpus also passed locally:

```text
cargo test -p kaveon-server --bin kaveon-server differential_tests --quiet
41 passed; 0 failed; 389 filtered out; finished in 223.59s
```
