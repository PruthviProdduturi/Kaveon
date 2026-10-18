# Global TopN `FETCH ... WITH TIES` differential

Date: 2026-10-18

The server differential test
`distributed_global_topn_fetch_with_ties_matches_local` runs

```sql
SELECT country, actions
FROM events
ORDER BY actions DESC
FETCH FIRST 1 ROW WITH TIES
```

through the node-local planner and the three-worker fragment path. It compares
the complete boundary peer set and repeats the distributed execution to check
that task completion order does not change the result. The comparison is
unordered because the SQL ordering key intentionally leaves peers tied. The
fixture contains multiple rows at the maximum key, so the test would pass
even if a worker incorrectly returned only one peer only when the local and
distributed paths disagreed; the exact multiset comparison catches that case.

Validation:

```text
cargo test -p kaveon-server differential_tests::distributed_global_topn_fetch_with_ties_matches_local -- --nocapture
PASS (1 test)
```

This qualifies correctness of the current coordinator merge for the tested
three-worker shape. It does not remove the remaining coordinator bottleneck or
provide a fully parallel ordered result stream.
