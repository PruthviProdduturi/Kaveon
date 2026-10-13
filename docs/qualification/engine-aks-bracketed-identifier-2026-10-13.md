# AKS bracketed-identifier qualification — 2026-10-13

The deployed Engine accepted `SELECT DISTINCT [year] FROM energy_annual ORDER BY [year]` and returned the same six exact rows as the unbracketed `year` form. Both requests finished successfully on `kaveon-aks`; the earlier worker-side `ARRAY` error did not reproduce.

Machine-readable evidence: [engine-aks-bracketed-identifier-2026-10-13.json](engine-aks-bracketed-identifier-2026-10-13.json)
