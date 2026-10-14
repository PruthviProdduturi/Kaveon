# AKS Engine image qualification — 2026-10-14

This smoke qualification binds the running Engine to the committed source
revision and checks the Studio-generated bracketed identifier path that had
previously returned `unknown scalar function: ARRAY`.

## Artifact

- Source revision: `f68df663ca042a4ea6848033b152fcbe250c0013`
- Image: `kaveonacr.azurecr.io/kaveon-engine@sha256:f069e56542ae89f0f52e7eeefa1e8e435baccad37e3aa0ec98159be70119b1f3`
- Deployment: `kaveon-aks`, namespace `kaveon`
- Topology: one coordinator and three workers, all Ready after the rollout

## Compatibility smoke

The coordinator accepted this request through the Trino-compatible wire route:

```sql
SELECT DISTINCT [year]
FROM OpenSource.climate_energy.energy_annual;
```

The result was HTTP `200`, state `FINISHED`, and a typed `Int64` `year` column.
The response contained zero rows because that catalog table currently has no
active rows; this is a data/catalog condition, not a parser or executor error.
The request completed in 1,428 ms and returned the normal `nextUri`, query
inspection, and cancellation links.

The source-level differential corpus also covers the same bracketed identifier
form with populated fixture data (`distinct_bracketed_identifier`) and passes
on both local and distributed execution paths.

## Scope

This is an image-bound compatibility smoke test. It does not qualify worker
loss, spill pressure, cancellation stress, coordinator failover, connector
pushdown, or sustained soak; those remain open in the parity tracker.
