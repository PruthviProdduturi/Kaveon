# PostgreSQL authority inventory

Current as of September 14, 2026. The repository contains PostgreSQL-free
runtime paths for every declared product family. PostgreSQL remains the live
deployment authority until the AKS retirement evidence gate passes.

| Authority family | PostgreSQL source | KaveonDB destination | Implemented cutover behavior |
| --- | --- | --- | --- |
| `catalog_sources` | `catalog_sources` | typed `source` | Direct list, point and revision-CAS mutations |
| `data_sources` | `data_sources` | typed `source` plus Key Vault reference | Secret-free records; versioned secret URI only |
| `datasets` | `datasets` | typed `dataset` | Direct CRUD with owner and revision validation |
| `dataset_semantics` | dimensions, columns, metrics | children in `dataset` document | Parent and semantic children commit atomically |
| `charts` | `charts` | typed `chart` | Dataset reference validation and revision CAS |
| `dashboards` | `dashboards` | typed `dashboard` | Chart references and mutations without fallback |
| `favorites` | `favorites` | typed `favorite` | Owner/type/object deterministic identity |
| `saved_queries` | `saved_queries` | typed `saved_query` | Owner-scoped direct CRUD |
| `user_themes` | `user_themes` | typed `user_theme` | Owner-scoped direct CRUD |
| `user_recents` | `user_recents` | typed `user_recent` | Atomic upsert and bounded retention |
| `query_history` | `query_history` | typed `query_history` | Bounded create, retention and deletion |
| `activity` | `activity` | typed `activity` | Immutable actor-scoped audit record |
| `context_cache` | snapshots and answer cache | rebuilt generated state | Metadata-only retirement proof; result data is not copied |
| `dlm_generation` | five legacy DLM tables | `dlm_definition`, `dlm_run`, immutable ADLS artifact | PostgreSQL-free generation and serving in retirement mode |
| `chat_history` | sessions and messages | typed session/message records | Atomic session touch/message create and bounded cascade delete |
| `ai_configuration` | legacy AI provider/key tables | reviewed deletion or secret boundary | Former service is absent; evidence must prove live-row disposition |

The migration source contains a canonical outbox and transaction boundary.
Replay processes source sequence order, checks payload hashes, resolves
ambiguous responses from exact target content and acknowledges an event only
after the KaveonDB commit is observable. Direct retirement-mode mutations do
not return to PostgreSQL when KaveonDB rejects a revision, reference, owner or
document.

Backfill commands capture bounded repeatable-read snapshots with stable IDs,
owners, references and canonical hashes. Apply checkpoints publish to ADLS with
ETag compare-and-swap and content-addressed versions. Local filesystem apply is
allowed only when `KAVEON_ENVIRONMENT=local`; it is not retirement evidence.

The DLM compiler uses request-local bounded state in retirement mode. It reads
native data through the Engine, seals value indexes, routing terms, precomputed
answers and curation into an immutable ADLS artifact, then commits a
dataset-revision-bound definition and terminal run. Serving verifies the
definition, run and artifact SHA-256 and has no PostgreSQL fallback.

## Machine-checked dependency boundary

Run the source inventory after every database-access change:

```powershell
python scripts/check-postgresql-dependencies.py `
  --output tmp/postgresql-cutover-dependencies.json
```

The scanner maps production SQL call sites to exactly 16 authority families and
rejects unclassified tables or access modes. This is static source coverage. It
cannot discover an older deployed schema or SQL assembled entirely at runtime.

Generate the separate live inventory from the same API image used for migration:

```powershell
python scripts/inventory-postgresql-authority.py `
  --output tmp/postgresql-live-authority.json
```

The command uses one repeatable-read, read-only transaction, rejects an unknown
public table, and records counts, presence, family mapping, source snapshot and
report digest without rows or credentials.

## Operational boundary

Implemented code is insufficient to retire a live database. One immutable AKS
run must still prove:

1. Exact reconciliation for all 16 authority families at one source watermark.
2. Shadow-read parity across representative roles and visibility states.
3. Live create, update and delete rejection after the PostgreSQL write fence.
4. Zero pending outbox events at the fenced watermark.
5. API and Studio restart with PostgreSQL unavailable.
6. KaveonDB backup/restore with identical committed snapshot identity.
7. Bounded rollback to the preserved PostgreSQL snapshot/PVC.
8. Acceptance by the strict evidence runner and qualification summary.

Until those observations are archived and reviewed, PostgreSQL remains the live
authority. The first retirement action is scaling it to zero while retaining
its PVC and snapshot. Permanent deletion is outside the evidence runner and
happens only after the approved rollback window.
