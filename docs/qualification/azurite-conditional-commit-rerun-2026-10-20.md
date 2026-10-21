# Azurite conditional-commit rerun — 2026-10-20

Status: **passed**

With the repository's Azurite service running, the ignored provider tests were
executed from the Engine workspace:

```powershell
cargo test -p kaveon-storage azurite -- --ignored --nocapture
```

Both tests passed: the conditional-commit contract classified Azure-shaped
`409`/`412` responses correctly, and six concurrent writers completed without a
lost update (15 expected conflicts). Azurite was stopped immediately after the
run. This validates the local Azure-compatible provider path; it is not a live
ADLS account qualification.

