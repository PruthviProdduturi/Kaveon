# Delegated identity token validation — 2026-11-22

The local Entra/JWT validation suite passed **3/3**:

- cached signing keys validate without a network call and unknown key IDs are
  throttled;
- signed delegated tokens require the complete security claim set;
- signed display claims are preferred and invalid values fail closed.

```powershell
cargo test -p kaveon-server entra -- --test-threads=1
```

This is local token-validation evidence only. It does not qualify live Entra
key rotation/revocation, tenant policy, or cloud identity wiring. No cloud
resources were provisioned or accessed; incremental spend was **$0**.
