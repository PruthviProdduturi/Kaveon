# Release-mode workspace qualification — 2026-11-22

## Result

**PASS.** After stabilizing the release pressure fixture, the complete
non-server workspace test command passed in optimized mode:

```text
cargo test --workspace --exclude kaveon-server --release -- --test-threads=1
```

Catalog, CLI, core, execution, optimizer, SQL, storage, Python binding, and
their doctest targets completed with no failures. Tests explicitly marked as
benchmarks or emulator-dependent remained ignored by their contract. The
server crate has its own separate release receipt with 720/720 tests passed.

This is local release evidence only; cloud deployment and cloud-scale
performance gates remain separate. No cloud resources were provisioned and
incremental cloud spend was **$0**.
