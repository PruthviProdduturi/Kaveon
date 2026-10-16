# AKS bounded mixed-workload qualification — 2026-10-15

The current three-worker Engine ran three concurrent rounds of four different
workloads against the OpenSource catalog:

- full-table grouped aggregate over the 504M-row events table;
- bounded high-cardinality self-join over the same source;
- partitioned `ROW_NUMBER()` with `QUALIFY`; and
- global multi-column `ORDER BY ... LIMIT` over the 504M-row table.

Every request finished and matched its sequential baseline exactly in all three
rounds. The four queries used all three workers; the round wall times were
63.968s, 56.047s, and 55.563s. No worker spill files remained after the run and
no request failed. The complete raw report is retained in
`track1-aks-mixed-workload-2026-10-15.json`.

This qualifies bounded concurrent shape mixing on the declared topology. A
five-minute or longer endurance run, disk-exhaustion behavior, skew stress, and
broader topologies remain open.
