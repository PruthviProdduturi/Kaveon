# Cloud budget live check — 2026-11-22

## Result

**Fail closed: the subscription is not yet proven to be below the $100/month
Kaveon ceiling.** No new resources were provisioned by this check.

The Azure Cost Management usage query returned no usable cost rows for the
selected period, so this report does not invent a dollar total. A spend total
must be obtained from Cost Management before enabling a cloud qualification
run. The read-only `az consumption usage list` fallback returned 230 usage
records, but every record had `pretaxCost=None` (zero priced rows). That proves
usage metadata exists, not the billed dollar total, so it is not accepted as
proof of the `$100` ceiling.

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
- Two additional AKS-related storage accounts in the managed resource group.
- No NAT gateway was returned by the subscription inventory.

Before the check, the VM and the running AKS cluster overlapped. Both serving
compute paths are now stopped/deallocated, and the stopped legacy cluster still
has retained infrastructure. Do not start a cluster or increase worker count
until the owner confirms current-month spend.

## Required operator decision before cloud qualification

1. Export the current-month Cost Management total and resource breakdown.
2. Keep either the VM or the AKS serving path, not both, during qualification.
3. Remove or retain the stopped cluster only after checking its disks, public
   IPs, and rollback requirement.
4. Keep one `Standard_B2s` worker only for an approved qualification window,
   NAT disabled, and Log Analytics ingestion bounded; stop/deallocate resources
   immediately after each gate.

The local Docker qualification path remains the default and is the only path
that can continue without a verified cloud spend total. The post-stop preflight
still fails closed solely because Cost Management returned no usable
current-month usage rows.
