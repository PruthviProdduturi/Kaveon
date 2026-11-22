# Cloud budget live check — 2026-11-22

## Result

**Pass: the subscription is currently below the $100/month Kaveon ceiling.**
The month-to-date Cost Management REST query reports **$51.194352 USD** for
the selected subscription. No new resources were provisioned by this check.

The Azure CLI preview usage endpoint still returns 230 unpriced records, but
the guardrail now falls back to the authenticated Cost Management REST query
(`api-version=2026-06-01`) and records its priced total. The script therefore
fails closed only when both sources are unavailable. This is month-to-date
actual cost, not a forecast of the remainder of the billing period.

## Observed inventory

Read-only inspection of subscription `00000000-0000-4000-8000-000000000000`
(`Visual Studio Enterprise Subscription`) in `westus2` found:

- `kaveon-prod-aks`: Free tier, one `Standard_B2s` system node; it was
  **stopped during this check** after confirming it had no Kaveon workloads.
- `kaveon-aks`: **Stopped**, Free tier, one `Standard_D4s_v3` system node and
  three `Standard_D4s_v3` worker nodes; its managed resource group and disks
  remain billable where applicable.
- `kaveon-vm`: `Standard_B2als_v2`; it was **deallocated during this check**
  and its disk was retained for reversible rollback.
- `kaveonlake`: Standard ZRS storage.
- `kaveon-logs`: Log Analytics, `PerGB2018`, 30-day retention.
- `kaveon-logs` ingestion is now capped at **0.1 GB/day** (`dailyQuotaGb=0.1`)
  to prevent unbounded telemetry spend; retention remains 30 days.
- Both stopped AKS managed VM scale sets have **zero instances**; no AKS worker
  compute is currently running.
- `kaveonacr` is Basic tier; image publication uses bounded CI/ACR builds and
  does not require an AKS runtime.
- Final read-only check found no active ACR build (the bounded publication runs
  `cc17y`, `cc17x`, and `cc17w` are all `Succeeded`).
- The push-triggered AKS deployment preflight correctly refused to proceed while
  `kaveon-aks` was stopped; AKS deployment is now manual-dispatch only so normal
  Studio or documentation pushes cannot initiate billable cloud qualification.
- Two additional AKS-related storage accounts in the managed resource group.
- No NAT gateway was returned by the subscription inventory.

Before the check, the VM and the running AKS cluster overlapped. Both serving
compute paths are now stopped/deallocated, and the stopped legacy cluster still
has retained infrastructure. Do not start a cluster or increase worker count
until the owner confirms current-month spend.

## Required operator checks before cloud qualification

1. Re-run the guardrail and retain the current-month Cost Management total and
   resource breakdown; the current check is $51.194352 USD month-to-date.
2. Keep either the VM or the AKS serving path, not both, during qualification.
3. Remove or retain the stopped cluster only after checking its disks, public
   IPs, and rollback requirement.
4. Keep one `Standard_B2s` worker only for an approved qualification window,
   NAT disabled, and Log Analytics ingestion bounded; stop/deallocate resources
   immediately after each gate.

The local Docker qualification path remains the default. The budget guardrail
now passes for this check, while the post-stop preflight still prevents a cloud
qualification run unless the cluster is explicitly enabled and the spend check
remains below the ceiling.
