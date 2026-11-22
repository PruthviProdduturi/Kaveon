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

The default root Compose profile was verified on 2026-11-22 to expose only
`engine-coordinator`, two Engine workers, `api`, and `studio`; PostgreSQL and
Azurite are opt-in profiles/services and are not started by the normal local
stack.
