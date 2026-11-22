# Kaveon Intelligence context contract

Kaveon Intelligence is the governed intelligence layer that connects three product
pillars: Kaveon Studio, the deterministic Data Language Model (DLM), and
Kaveon Engine. This document defines the context that may be used to answer a
question and the evidence that must travel with the answer.

The contract is deliberately narrower than a general-purpose knowledge graph.
It describes data products and their governed semantics; it does not claim
that Kaveon can infer arbitrary business meaning from unregistered sources.

## Design principles

1. **Governed inputs only.** A context item is usable only when its dataset,
   source, owner, and visibility policy are known.
2. **Deterministic resolution.** A question is resolved by matching supported
   intent, entities, metrics, dimensions, filters, and time bounds against the
   compiled context. An LLM is not required on this path.
3. **Evidence travels with the answer.** Every answer identifies the context
   version, source, execution lane, and statement or artifact that produced it.
4. **Freshness is explicit.** Context can answer directly only when its
   freshness policy says it is valid. Otherwise the request falls through to a
   bounded live query or a rebuild.
5. **Least privilege.** Permission checks happen before a context item is
   eligible for routing; hidden datasets must not be revealed through ranking,
   suggestions, or error messages.

## Context model

The compiled artifact is composed of the following parts. The status column
distinguishes what is implemented in the current DLM path from approved target
work.

| Part | Contract | Status |
|---|---|---|
| Dataset | Stable dataset identity, display name, source table, schema, owner, visibility, and compilation version. | Implemented |
| Semantic entity | A named business object or value domain resolved to one or more source columns, with aliases and allowed values. | Implemented for registered dataset semantics |
| Metric | A governed aggregate or scalar expression with type, null behavior, and supported aggregation forms. | Implemented for supported DLM question classes |
| Dimension | A groupable or filterable column with display label, type, aliases, and optional hierarchy. | Implemented for registered datasets; richer hierarchies are target work |
| Relationship | A declared join or semantic relationship between datasets, including cardinality and join keys. | Implemented in dataset metadata; cross-source planning remains target work |
| Lineage | Source object, snapshot/version, compilation input, and answer artifact references. | Partial: source/version/evidence are current; end-to-end graph lineage is target work |
| Freshness | Validity score, source-change signal, compilation time, and rebuild policy. | Implemented for DLM context routing |
| Permissions | Principal, role, dataset/catalog visibility, and row/column policy inputs. | Implemented at product boundaries; fine-grained context policies are target work |
| Evidence | Query or context artifact ID, statement, source, version, lane, timing, and result digest. | Implemented for DLM answers and Engine-backed results |
| Confidence | Deterministic resolution score plus freshness and evidence state; never a claim of probabilistic model certainty. | Partial: resolver/freshness scores exist; unified IQ confidence policy is target work |

### Dataset identity

Dataset identity is stable across recompilations. A compilation creates a new
context version; it does not silently mutate the meaning of an older answer.
The minimum identity is:

```text
dataset_id + source_catalog + source_schema + source_table + context_version
```

The source location is descriptive metadata. The Engine remains responsible for
executing a selected source path; the DLM does not pretend to federate sources
that the planner cannot join.

### Semantic entities, metrics, and dimensions

The DLM resolves supported questions by matching words and values to governed
dataset metadata. Synonyms and value indexes improve matching, but they do not
authorize an unregistered column or invent a metric.

A valid metric must state its aggregation and result type. A valid dimension
must state its source column and type. Ambiguous matches are rejected or routed
to clarification rather than silently selecting a different dataset.

### Relationships

Relationships describe how two datasets may be combined. The contract records
the key columns, declared cardinality, and permitted join direction. A
relationship is not proof that a cross-source join is executable: the planner
must still qualify the selected execution path and its snapshot semantics.

## Freshness and routing

Each context element carries a validity state derived from its compilation age
and source-change observations. A request follows this deterministic order:

1. Authenticate the principal and resolve its catalog/dataset visibility.
2. Match the question to eligible datasets and supported question classes.
3. Evaluate the freshness of the required context elements.
4. If all required elements are fresh and the question is covered, answer from
   compiled context.
5. Otherwise issue one bounded live query through the selected source path,
   then record the evidence and schedule or perform a targeted rebuild.
6. Return the answer with its lane (`context` or `live_query`) and evidence.

The live-query fallback is not a license to execute an unbounded scan. Result
limits, source permissions, query budgets, and Engine admission controls remain
in force.

## Permission boundary

Eligibility is evaluated before ranking. The resolver must not use a dataset
that the principal cannot query, and it must not reveal the dataset's name or
values through a failed match. Catalog-level access, schema-level visibility,
dataset ownership, and any row/column policies are inputs to this decision.

For a future multi-tenant deployment, the tenant and principal identifiers
must be part of the context cache key. Context compiled for one tenant must
never be reused for another tenant without an explicit, governed sharing
policy.

## Evidence and confidence

An answer is explainable when a reviewer can identify what was used and replay
the live path. Evidence should include:

- `answer_id` or query ID;
- dataset and `context_version`;
- source/catalog/schema/table;
- lane (`context` or `live_query`);
- generated SQL when a live query ran;
- freshness state at routing time;
- execution timing and result digest;
- principal and entry point, subject to privacy policy.

`confidence` is a routing signal, not a probability that the business answer is
true. It should be decomposable into match quality, freshness, and evidence
completeness. Until the unified score is implemented, clients should display
the lane and evidence state rather than inventing a single confidence number.

## Example context artifact

This is a compact wire shape for the contract. It is illustrative; field names
may evolve with the API version.

```json
{
  "dataset_id": "ds_kaveon_events",
  "context_version": 42,
  "source": {
    "catalog": "OpenSource",
    "schema": "public",
    "table": "kaveon_events_enriched",
    "snapshot": "delta-v18"
  },
  "owner": "kaveon-system",
  "visibility": {"catalog": "OpenSource", "roles": ["analyst", "admin"]},
  "entities": [
    {"name": "region", "column": "region", "type": "string", "aliases": ["country region"]}
  ],
  "metrics": [
    {"name": "actions", "expression": "SUM(actions)", "type": "integer"}
  ],
  "dimensions": [
    {"name": "platform", "column": "platform", "type": "string"}
  ],
  "freshness": {
    "compiled_at": "2026-11-21T20:00:00Z",
    "validity_score": 0.94,
    "source_change_signal": "stable",
    "rebuild_policy": "targeted"
  },
  "routing": {
    "supported": ["metric_by_dimension", "top_n"],
    "fallback": "bounded_live_query"
  }
}
```

## Acceptance criteria

Kaveon may claim this contract as shipping only when the following are true:

- The same supported question resolves to the same dataset, metric, and SQL
  under the same context version and permissions.
- A stale context never returns a context answer without an explicit stale
  state; it falls through to a bounded live query or reports that it cannot
  answer.
- A user without dataset permission cannot discover the dataset through
  ranking, autocomplete, clarification, or error text.
- Every answer exposes its lane, context version or query ID, source, and
  freshness/evidence state.
- Context artifacts can be rebuilt idempotently and older versions remain
  auditable.
- Docker qualification covers deterministic routing, permission filtering,
  stale-context fallback, evidence shape, and exact result parity.

The following remain roadmap items rather than current claims: a universal
enterprise knowledge graph, arbitrary cross-source joins, tenant-isolated
context sharing, and a single calibrated confidence score across every answer
type.
