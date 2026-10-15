# Engine chart examples

## `resource-groups.maintenance.json`

Keeps maintenance work out of the interactive share — defect D-43.

A cube build issues one statement per grouping, concurrently. With a single
group it takes all four of that group's slots, and a chart is then **refused
outright** rather than merely slowed:

```
during build — admitted: 4294967296 of 8589934592
  group default running 4 queued 0
chart statement: 429 after the full 60 s admission wait
```

Half the memory pool was free and the chart was still refused, because the
binding constraint is concurrency rather than memory. Bounding the build's
memory cannot help: at 2 GiB it was worse, because the build's children filled
the whole 8 GiB pool between them.

So this document gives maintenance a quarter of the pool and two slots of its
own, and leaves the interactive `default` group its four. Maintenance waits far
longer before giving up (900 s against 60 s) because nobody is watching it, and
its statements default to a small per-query budget. `default` takes a priority
above maintenance so interactive work wins the pool under contention.

The first selector matches the tag the API stamps on every `ANALYZE` —
`engine_bridge.MAINTENANCE_CLIENT_TAG`, `kaveon:maintenance`. Selectors are
ordered, every matcher given must hold, and the catch-all must be last.

### Applying it

Through the product: **Settings → Governance**. Or, as an administrator:

```
PUT /api/v1/engine/admin/resource-groups
```

with this file as the body. The coordinator stores it durably at
`/state/catalog/resource-groups.json`.

### Reverting

The same call with the builtin document — one group and no selectors:

```json
{
  "demo": { "enabled": false },
  "groups": [
    { "name": "default", "max_concurrent": 4, "max_queued": 64,
      "max_queue_wait_seconds": 60, "priority": 1 }
  ],
  "selectors": []
}
```

### One interaction to know about

If the chart's `resourceGroups.enabled` is ever turned on, the rendered
ConfigMap takes precedence over this runtime copy and a runtime `PUT` starts
failing, because the file is then Helm's. Move the same groups into
`values.yaml` at that point.

`.github/workflows/deploy-aks-engine.yml` will not let that happen by accident:
it refuses any render that names `KAVEON_RESOURCE_GROUPS` unless the dispatch
is given `allow_resource_groups_takeover`. So committing the flag does not
quietly take the document away from the coordinator — the deploy fails and says
so.

The document is validated strictly: the top level accepts only `groups`,
`selectors` and `demo`, and unknown fields are rejected rather than ignored —
so it carries no comments of its own, which is why this note is a separate
file.
