# Storage release suite — 2026-11-22

The optimized storage crate passed its release-profile suite with the pinned
toolchain and lockfile:

```powershell
cargo test -p kaveon-storage --release --locked -- --test-threads=1
```

Result: **158 passed, 0 failed, 3 ignored**. The suite covers Parquet and
Delta snapshot pinning, row-group and partition pruning, object-store range
reads, conditional commits, immutable writes, rewrite recovery, statistics,
runtime filters, UTF-8 view profiling, Iceberg snapshots, and spill/storage
metadata. The three ignored tests are Azurite-only concurrent conditional-commit
checks and remain explicitly cloud/local-emulator gates; they are not counted as
passes.
