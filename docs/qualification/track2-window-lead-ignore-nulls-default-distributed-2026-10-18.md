# Distributed unpartitioned `LEAD IGNORE NULLS` qualification

Date: 2026-10-18

## Scope

This qualification covers an unpartitioned `LEAD` value window with a skip count, `IGNORE NULLS`, and a literal default. A three-row derived relation makes the default path observable while the global order is deterministic.

```sql
SELECT input_id,
       actions,
       LEAD(industry, 2, 'missing') IGNORE NULLS OVER (
         ORDER BY actions, input_id
       ) AS next_industry
FROM (VALUES
  (1, 1, CAST('alpha' AS VARCHAR)),
  (2, 2, CAST(NULL AS VARCHAR)),
  (3, 3, CAST(NULL AS VARCHAR))
) AS input(input_id, actions, industry)
ORDER BY actions, input_id
```

## Result

Local and three-worker distributed execution return the same three exact rows, including the literal `missing` default. The query memory pool returns to zero after both runs. The regression is `distributed_unpartitioned_lead_ignore_nulls_with_default_matches_local` in `engine/crates/server/src/differential_tests.rs`.

## Boundary

This qualifies unpartitioned `LEAD` with offset, NULL skipping, and a literal default through local and in-process distributed execution. It does not claim all value-window modifiers, expression defaults, external-client wire qualification, or cloud qualification.
