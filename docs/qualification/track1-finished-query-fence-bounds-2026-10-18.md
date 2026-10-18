# Track 1 finished-query fence bounds qualification

The in-memory exchange store keeps terminal query markers so uploads already in
flight when cancellation completes cannot recreate exchange state. When the
bounded marker map is full, finalization now evicts the oldest expiry and keeps
the marker for the query finalized most recently. Previously the new marker was
silently dropped at the cap, reopening that cancellation race for the newest
query.

Local qualification:

- `cargo fmt --all -- --check` — passed
- `cargo test -p kaveon-server --bin kaveon-server exchange::tests -- --nocapture` — 29 passed, 0 failed
- `cargo clippy -p kaveon-server --bin kaveon-server --all-targets -- -D warnings` — passed

The focused regression fills the 10,000-marker bound, verifies the newest
query rejects a late upload, and verifies the oldest marker is the one evicted.
This is local exchange lifecycle evidence; it does not claim a new AKS
topology, workload, or distributed execution SLO.
