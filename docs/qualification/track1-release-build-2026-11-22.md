# Reproducible release-build qualification — 2026-11-22

## Result

**PASS.** The committed Engine workspace (`e5991668`) built with the locked dependency graph and pinned Rust toolchain:

```text
cargo build --workspace --release --locked
cargo fmt --all -- --check
```

Both completed successfully. The resulting local artifacts are identified by
SHA-256:

| Artifact | SHA-256 |
|---|---|
| `kaveon-server.exe` | `5627B9F265B3D40246D77E607C68174D294FB19018B38D2DCC79C98EECC3D338` |
| `kaveon.exe` | `04E7321B442A9E115272C38F588410CC119ACDF5C120DF895EB4EB8B245A5AC0` |

The source tree was clean after the build. This is local packaging evidence;
cloud registry publication and AKS rollout remain separate gates. No cloud
resources were provisioned and incremental cloud spend was **$0**.
