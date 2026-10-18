# Track 1 global TopN: zero-limit fast path

## Scope

The distributed ordered-stream path is used for `ORDER BY … LIMIT` over
partitionable scans. A zero limit has an empty result and must not schedule
workers or enter the coordinator merge, which has no candidate batch to
return.

## Result

The TopN contract now rejects `LIMIT 0`, allowing the ordinary empty-result
execution path to produce the correct schema and result without dispatching a
global merge. This removes unnecessary worker requests and avoids the prior
"distributed TopN completed without result rows" failure mode.

Focused qualification:

```text
cargo test -p kaveon-server does_not_dispatch_a_zero_limit_to_the_global_top_n_merge
1 passed; 0 failed
```

## Boundary

This is a zero-limit admission guard only. It does not change global ordering,
tie handling, offset semantics, or the coordinator's bounded TopN merge for
positive limits. Full ordered-stream scaling remains a separate qualification
area.
