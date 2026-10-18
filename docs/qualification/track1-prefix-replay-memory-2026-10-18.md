# Track 1 prefix replay memory qualification — 2026-10-18

The adaptive aggregate prefix may read batches speculatively before falling
back to partitioned processing. Replaying that prefix now releases the
speculative reservations before partition writers reserve their own buffers.

The focused regression is
`adaptive_prefix_caps_retained_batches_and_preserves_the_unread_tail` in
`engine/crates/exec/src/partitioned.rs`. It verifies that a capped prefix
retains the unread tail, releases all prefix memory on replay, and preserves
the exact input rows. Strict Clippy and the focused execution test pass.

This qualifies local accounting and cleanup. It does not claim physical disk
exhaustion, node-loss, or long-duration cloud-spill qualification.
