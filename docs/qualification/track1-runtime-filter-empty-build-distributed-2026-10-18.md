# Empty-build runtime filter distributed qualification

An empty build relation must publish an empty runtime filter to every probe
worker. The probe operator should finish without opening or decoding probe
batches, while preserving the exact empty inner-join result and releasing all
query memory.

Qualification query:

```sql
SELECT e.user_id, b.user_id
FROM events e
JOIN empty b ON e.user_id = b.user_id
ORDER BY e.user_id;
```

The server differential test
`distributed_empty_build_runtime_filter_matches_local` creates a non-empty
Parquet probe and an empty Parquet build in an isolated catalog. It compares
the local operator with the in-process three-worker fragment path. Both return
zero rows, and the shared query memory pool returns to zero after each path.

Boundary: this qualifies empty-build inner-join short-circuiting. It does not
claim cloud file-pruning measurements, injected worker loss, or outer/anti-join
NULL semantics; those remain separate runtime-filter qualifications.
