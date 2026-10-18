# Track 2 qualification: residual correlated LATERAL `USING`

Date: 2026-10-18

## Contract

A correlated derived relation joined with `JOIN LATERAL ... USING (...)` may
contain a residual predicate in addition to its equality correlation. Inner
and left lateral joins preserve the residual predicate and match local and
distributed execution. The equality-only restriction applies only to the
separate positive correlated scalar `OFFSET n LIMIT 1` rewrite, which needs
per-key `ROW_NUMBER()` partition keys.

```sql
SELECT u.user_id, o.actions
FROM users u
JOIN LATERAL (
  SELECT user_id, actions
  FROM events
  WHERE events.user_id = u.user_id
    AND actions > u.user_id
) o USING (user_id)
ORDER BY u.user_id, o.actions
LIMIT 10
```

The left-lateral form also preserves the null-extended outer row when the
residual predicate removes every right-side match.

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-server --bin kaveon-server correlated_residual -- --nocapture
cargo test -p kaveon-optim correlated_scalar_offset -- --nocapture
```

The two server differential tests pass with identical local and three-worker
results. The optimizer tests continue to assert that residual correlations
are rejected for positive correlated scalar `OFFSET n LIMIT 1`.

## Scope

This covers bounded inner and left residual correlated lateral derived
relations with `USING`. General Apply/lateral paging, nested lateral shapes,
and arbitrary multi-column or unbounded correlated scalar forms remain open.
