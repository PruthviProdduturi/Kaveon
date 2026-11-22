# Cloud budget live check — 2026-11-22

## Result

**Fail closed: the subscription is not yet proven to be below the $100/month
Kaveon ceiling.** No new resources were provisioned by this check.

The Azure Cost Management usage query returned no usable cost rows for the
selected period, so this report does not invent a dollar total. A spend total
must be obtained from Cost Management before enabling a cloud qualification
run.

## Observed inventory

Read-only inspection of subscription `00000000-0000-4000-8000-000000000000`
(`Visual Studio Enterprise Subscription`) in `westus2` found:

- `kaveon-prod-aks`: **Running**, Free tier, one `Standard_B2s` system node.
- `kaveon-aks`: **Stopped**, Free tier, one `Standard_D4s_v3` system node and
  three `Standard_D4s_v3` worker nodes; its managed resource group and disks
  remain billable where applicable.
- `kaveon-vm`: **Running**, `Standard_B2als_v2`.
- `kaveonlake`: Standard ZRS storage.
- `kaveon-logs`: Log Analytics, `PerGB2018`, 30-day retention.
- Two additional AKS-related storage accounts in the managed resource group.
- No NAT gateway was returned by the subscription inventory.

This combination is not an approved minimal footprint: the VM and the running
AKS cluster overlap, and the stopped cluster still has retained infrastructure.
Do not start another cluster or increase worker count until the owner chooses
one serving path and confirms current-month spend.

## Required operator decision before cloud qualification

1. Export the current-month Cost Management total and resource breakdown.
2. Keep either the VM or the AKS serving path, not both, during qualification.
3. Remove or retain the stopped cluster only after checking its disks, public
   IPs, and rollback requirement.
4. Keep one `Standard_B2s` worker, NAT disabled, and Log Analytics ingestion
   bounded; stop/deallocate resources immediately after each gate.

The local Docker qualification path remains the default and is the only path
that can continue without a verified cloud spend total.
