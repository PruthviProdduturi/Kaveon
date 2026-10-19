# Track 1 cloud qualification precondition — 2026-10-19

The remaining AKS/object-store gates were attempted against the configured
subscription before starting any new resources.

```text
az aks start --resource-group kaveon-rg --name kaveon
ERROR: ReadOnlyDisabledSubscription
The subscription ... is disabled and therefore marked as read only.
You cannot perform any write actions on this subscription until it is re-enabled.
```

Read-only inspection still finds the existing `kaveon-aks` cluster in
`kaveon-rg`, deallocated with failed provisioning, and the `kaveonacr` registry.
Because Azure rejects the start operation before a control-plane transition,
no AKS digest, object-store soak, scale, or failure-recovery result can be
produced from this environment. Local Track 1/2 gates remain independent and
are recorded in `track1-track2-workspace-2026-10-19.md`.

This is an environment precondition, not a passing qualification result. The
cloud request must be retried after the subscription is writable; no resource
recreation or scaling was performed by this qualification.\r\nA recheck on 2026-10-19 confirmed the same control-plane state: `az account show` reports `Enabled`, but `az aks start --resource-group kaveon-rg --name kaveon-aks` still returns `ReadOnlyDisabledSubscription`. Read-only inspection reports `kaveon-aks` as `Deallocated` with provisioning state `Failed`; `kaveonacr.azurecr.io` remains present. No cluster write or qualification run was possible.\r\n