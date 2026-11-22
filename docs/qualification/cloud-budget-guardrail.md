# Cloud qualification budget guardrail

Cloud production qualification is intentionally **not enabled by default**.
Before any AKS/ADLS run:

1. Confirm the active subscription, resource group, and current month spend.
2. Set an operator-approved hard budget of **$100 total monthly spend** for
   Kaveon qualification.
3. Use the smallest temporary worker pool and short-lived storage/compute
   resources; stop or delete the qualification resources immediately after
   each gate.
4. Do not provision AKS, NAT, public IPs, Log Analytics, or a second storage
   account without an explicit cost estimate and approval.
5. Capture start/end resource inventory and billing evidence in the gate
   report. If cost or subscription state is unknown, fail closed and do not
   provision.

Local Docker and emulator qualification remains the default path and has no
incremental cloud spend. This guardrail does not claim any cloud gate has
passed; it prevents an unbounded cloud qualification attempt.

The AKS Bicep defaults are intentionally conservative: `Standard_B2s` nodes
with one production worker minimum and two maximum; the test template uses
one `Standard_B2s` worker by default. Dedicated NAT/public IP resources are
disabled by default (`enableNatGateway=false`) and the templates use the AKS
managed load-balancer egress path. Larger node sizes, extra workers, NAT/public
IP, or additional observability resources require explicit parameter overrides
and an updated cost estimate before deployment.

The infrastructure workflow is manual-only and fail-closed: it requires the
`budget_approved=true` dispatch input after a current-month Cost Management
check, and passes the conservative worker, node-size, and NAT parameters
explicitly. A push to `dev` cannot provision AKS.

The Engine Helm apply workflow has the same explicit approval input and refuses
an apply whose resolved worker count exceeds two. A read-only plan may still be
rendered without approval.

Run the read-only subscription preflight before any cloud gate:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\verify-azure-budget-guardrail.ps1
```

It exits non-zero when current-month Cost Management evidence is unavailable,
when VM and AKS serving paths overlap, when an active worker pool exceeds the
bound, or when a NAT gateway exists. On 2026-11-22 it correctly failed closed:
the subscription had one running VM and one running AKS cluster, and Azure
returned no usable current-month usage rows.

The default root Compose profile was verified on 2026-11-22 to expose only
`engine-coordinator`, two Engine workers, `api`, and `studio`; PostgreSQL and
Azurite are opt-in profiles/services and are not started by the normal local
stack.
