# Cloud qualification precondition — 2026-10-20

Status: **not runnable in the current subscription state**

The authenticated Azure context is:

```text
subscription: 00000000-0000-4000-8000-000000000000
tenant:       11111111-1111-4111-8111-111111111111
user:         demo-owner@example.org
```

The current resource inventory contains `kaveon-aks` in `kaveon-rg`, but its
power state is **Deallocated**. The `kaveonlake` ADLS Gen2 account is present
and reports `provisioningState: Succeeded` / `statusOfPrimary: available`, but a
data-plane container listing returns `AccountIsDisabled`.

Therefore AKS execution and live ADLS qualification were not attempted. The
local Docker, release-binary, and Azurite gates remain valid; this document is
the explicit prerequisite for resuming cloud tests after the subscription and
storage data plane are reactivated.

