# Azurite conditional-commit qualification — 2026-10-20

The Azure Blob-compatible conditional commit path was exercised against a real Azurite HTTP endpoint rather than the in-memory or filesystem test doubles.

## Setup

```powershell
docker compose up -d azurite
az storage container create --name kaveon-product `
  --connection-string UseDevelopmentStorage=true
cargo test -p kaveon-storage azurite -- --ignored --nocapture
```

## Result

Both ignored provider tests passed:

- `azurite_conditional_commit_matches_the_contract`: immutable create, stale ETag rejection, and current ETag replacement all behaved correctly against Azure-shaped HTTP errors (`409 BlobAlreadyExists` and `412 ConditionNotMet`).
- `azurite_concurrent_writers_never_lose_an_update`: six concurrent writers observed seven conflicts and every writer eventually committed; the final counter was exactly six.

The test result was **2 passed, 0 failed**. This qualifies provider error classification and compare-and-swap safety against the emulator. It does not replace an ADLS Gen2 or AKS identity/region fault run.

