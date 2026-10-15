# Track 2 AKS compatibility probe — 2026-10-14

**Cluster:** `kaveon-aks` / namespace `kaveon`  
**Engine image:** `kaveonacr.azurecr.io/kaveon-engine@sha256:c997febb6d14040bd9f26d21d1f96c90f33a6c5faf7a728cf2ba350e8181f06f`

The current deployed image was queried through the coordinator Service using
the authenticated native statement endpoint. This probe reproduces the
bracketed identifier form that previously returned `unknown scalar function:
ARRAY` in Studio:

```sql
SELECT DISTINCT [year]
FROM OpenSource.climate_energy.energy_annual
```

The statement finished successfully with no error, returned a typed `Int64`
`year` column, and produced six rows: `2021, 2022, 2023, 2024, 2025, 2020`.
The recorded Engine elapsed time was 1,524 ms. This is current-image AKS
evidence for that compatibility shape; it does not close the broader Track 2
external-client or unsupported-feature boundaries.
