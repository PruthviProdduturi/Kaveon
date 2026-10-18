# Track 1 four-way join-reorder differential — 2026-10-18

The distributed differential corpus now includes a four-way inner equi-join
with repeated dimension inputs. The case exercises statistics-aware join
reordering, preserves the projected `country` output, and compares exact
results across the local and partitioned execution paths.

Case: `join_four_way_reorder` in
`engine/crates/server/src/differential_tests.rs`.

```sql
SELECT t.country, COUNT(*) AS n
FROM {T} t
JOIN {U} u ON u.user_id = t.user_id
JOIN {U} v ON v.locale = u.locale
JOIN {U} w ON w.user_id = v.user_id
WHERE t.event_date = '2026-07-04'
GROUP BY t.country
ORDER BY n DESC, t.country
LIMIT 5
```

Validation on 2026-10-18:

```text
cargo test -p kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths --quiet
1 passed; 0 failed; 426 filtered out; finished in 205.10s
```

The same command was rerun against the current `dev` source after the release
workflow completed:

```text
1 passed; 0 failed; 429 filtered out; finished in 212.59s
```

The corpus was then extended with a five-way repeated-dimension join, adding
one more statistics-aware reorder boundary:

```sql
SELECT t.country, COUNT(*) AS n
FROM {T} t
JOIN {U} u ON u.user_id = t.user_id
JOIN {U} v ON v.locale = u.locale
JOIN {U} w ON w.user_id = v.user_id
JOIN {U} x ON x.locale = w.locale
WHERE t.event_date = '2026-07-04'
GROUP BY t.country
ORDER BY n DESC, t.country
LIMIT 5
```

The focused differential sweep passed this five-way case with exact local and
partitioned results:

```text
1 passed; 0 failed; 429 filtered out; finished in 366.59s
```

This expands the bounded join-reorder corpus beyond the existing three-way
case. It does not qualify skew splitting, a general cost-based join enumerator,
or broad cloud-topology behavior; those remain open in the parity tracker.
