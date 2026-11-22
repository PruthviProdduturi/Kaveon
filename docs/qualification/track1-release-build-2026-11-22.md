# Reproducible release-build qualification — 2026-11-22

## Result

**PASS.** The committed Engine workspace (`e36e6de263fdaa01886eb53c18c2aef313bc802e`) built with the locked dependency graph and pinned Rust toolchain:

```text
cargo build --workspace --release --locked
cargo fmt --all -- --check
```

Both completed successfully. The resulting local artifacts are identified by
SHA-256:

| Artifact | SHA-256 |
|---|---|
| `kaveon-server.exe` | `DD153AF3B0EDF445B19F63D10636D2FBA78F1F764966DEF25B719387387489BC` |
| `kaveon.exe` | `04E7321B442A9E115272C38F588410CC119ACDF5C120DF895EB4EB8B245A5AC0` |

The source tree was clean after the build. This is local packaging evidence;
cloud registry publication and AKS rollout remain separate gates. No cloud
resources were provisioned and incremental cloud spend was **$0**.
