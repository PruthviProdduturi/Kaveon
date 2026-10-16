# Track 1 AKS: current-image three-client soak

## Result

The current Engine image completed a 900-second mixed-workload soak with three
concurrent clients.

- 1,386 of 1,386 requests returned exact expected results;
- zero failures and zero unexpected restart deltas were recorded;
- all three workers remained Ready;
- exchange and spill files were zero before and after the run;
- the report recorded no credentials and no subscription-level policy change.

The raw harness report is retained in
`track1-aks-current-soak-900-c3-2026-10-16.json`. The run used Engine image
`kaveonacr.azurecr.io/kaveon-engine@sha256:6a4fa35e1d511bdd533d6d3ee22c331137775a0848544bfd0fd98c2e6dea5b18`.

## Boundary

This closes the declared 900-second, three-client soak sample for the current
image. It does not qualify node loss during the soak, longer endurance,
physical disk exhaustion, or broader topologies.
