# Cloud qualification precondition — 2026-10-20

Status: **not runnable in the current subscription state**

The authenticated Azure context is:

```text
subscription: 4ed07f02-b111-4eea-98ce-1c177d573a51
tenant:       163a2817-f8e0-463f-a840-d7b6e574a9b2
user:         pruthvi.prodduturi@gmail.com
```

The current resource inventory contains `kaveon-aks` in `kaveon-rg`, but its
power state is **Deallocated**. The `kaveonlake` ADLS Gen2 account is present
and reports `provisioningState: Succeeded` / `statusOfPrimary: available`, but a
data-plane container listing returns `AccountIsDisabled`.

Therefore AKS execution and live ADLS qualification were not attempted. The
local Docker, release-binary, and Azurite gates remain valid; this document is
the explicit prerequisite for resuming cloud tests after the subscription and
storage data plane are reactivated.

The reusable fail-closed check is:

```powershell
powershell -ExecutionPolicy Bypass -File .\\scripts\\check-aks-cloud-precondition.ps1
```

It exits nonzero unless AKS is running and the ADLS data plane accepts an
authenticated container-list request.

