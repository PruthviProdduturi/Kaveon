# AKS coordinator exchange cleanup — 2026-10-13

The current coordinator StatefulSet was restarted on `kaveon-aks`. The coordinator had zero retained exchange files before the restart and zero afterward. It became Ready, and the authenticated cluster view reported three active and three catalog-compatible workers on snapshot `sha256:e3025334a4d5476c701c10c70c2d51b170118af13bfd91b3e4c4d0f3a842c7cb`.

Machine-readable evidence: [engine-aks-coordinator-cleanup-2026-10-13.json](engine-aks-coordinator-cleanup-2026-10-13.json)

This closes the zero-retained-file cleanup check for an idle coordinator restart and verifies post-restart worker compatibility. It does not claim that an active query survives coordinator loss; that remains a separate recovery gate.
