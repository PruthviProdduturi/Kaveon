# Kaveon IQ agent API contract

Kaveon IQ is the product contract for a governed data agent. It connects the
Studio experience, the deterministic Data Language Model (DLM), and Kaveon
Engine without turning the agent into an unbounded chatbot or an untrusted SQL
proxy.

This document separates the APIs that exist today from the small unified agent
surface that is a target. It is intentionally usable with Docker and local
Engine qualification; it does not require Azure, an external model, or a
metadata database outside Kaveon.

## Contract goals

An IQ request must be:

1. **Grounded** in a registered dataset and its compiled context.
2. **Authorized** for the server-asserted principal before routing or value
   discovery.
3. **Deterministic** for the same question, context version, permissions, and
   routing inputs.
4. **Bounded** by result, time, and admission limits.
5. **Auditable** through an answer ID, source/version, lane, SQL or artifact,
   and execution details.
6. **Cancellable** when a user leaves a page, closes a chat, or explicitly
   stops a request.

IQ does not claim arbitrary enterprise reasoning, unrestricted cross-source
joins, or a hosted LLM call. If the governed system cannot answer safely, it
returns a structured refusal or clarification.

## Current API building blocks

These routes are the current implementation. They remain the compatibility
surface while the unified `/iq` contract is introduced.

| Route | Purpose | Current behavior |
|---|---|---|
| `POST /api/v1/dlm/ask` | Resolve a natural-language question | Deterministic routing, context answer or bounded live-query fallback; returns assembled SQL, chart hints, and evidence where applicable |
| `GET /api/v1/dlm/route` | Preview dataset routing | Ranks visible datasets only; does not execute SQL |
| `GET /api/v1/datasets/{id}/dlm/context` | Read compiled context | Returns the curatable context spec for an authorized dataset |
| `GET /api/v1/datasets/{id}/freshness` | Inspect context validity | Returns freshness and rebuild recommendation |
| `POST /api/v1/dlm/reproduce` | Replay answer SQL | Runs the answer's read-only statement through the authorized source path and returns run evidence |
| `POST /api/v1/dlm/serve-chart` | Serve a chart query | Uses precomputed context when possible; the client falls back to bounded SQL when it cannot |
| `POST /api/v1/lab/query` | Stream an Engine statement | Returns a query ID, then exposes status and paged results |
| `GET /api/v1/lab/query/{id}` | Poll an Engine statement | Owner-scoped status, stages, workers, timing, and error details |
| `GET /api/v1/lab/query/{id}/results/{page}` | Read one result page | `202` while pending, `410` after failure/cancellation |
| `DELETE /api/v1/lab/query/{id}` | Cancel a streamed statement | Owner-scoped, idempotent cancellation |
| `DELETE /api/v1/sql/engine/cancel/{token}` | Cancel a tagged statement | Best-effort cancellation using a caller-minted token |

The route prefixes above are shown as mounted under `/api/v1`; deployment
configuration may expose the same router under a local prefix.

## Unified request envelope (target)

The following is the proposed versioned agent envelope. It is a contract
proposal, not a claim that `POST /api/v1/iq/ask` already exists.

```http
POST /api/v1/iq/ask
Authorization: Bearer <server-issued-token>
Idempotency-Key: 8f3b4e6c-...
Content-Type: application/json
```

```json
{
  "question": "Which region had the most active users last month?",
  "dataset_id": "ds_kaveon_events",
  "catalog": "OpenSource",
  "schema": "public",
  "mode": "deterministic",
  "limit": 100,
  "deadline_ms": 10000,
  "cancel_token": "client-generated-token",
  "conversation": {
    "session_id": "chat_01J...",
    "turn_id": "turn_01J..."
  }
}
```

The client may provide question, optional dataset scope, conversation
correlation, a result limit, and a deadline. The client must not provide the
authoritative email, role, tenant, catalog permissions, or source credentials
in the body. Those are derived from the authenticated request and server-side
policy.

`mode` is currently `deterministic`. An eventual assisted mode must be
explicitly enabled by policy and must preserve the same permission, evidence,
SQL guard, and refusal rules. It must never silently change a deterministic
answer into a model-generated answer.

## Unified response envelope (target)

```json
{
  "request_id": "iq_01J...",
  "status": "SUCCEEDED",
  "answer": {
    "kind": "table",
    "columns": [
      {"name": "region", "type": "Utf8"},
      {"name": "active_users", "type": "Int64"}
    ],
    "rows": [["North America", 1842], ["Europe", 1631]]
  },
  "sql": {
    "text": "SELECT region, COUNT(DISTINCT user_id) AS active_users ...",
    "statement_id": "q_01J...",
    "dialect": "kaveon"
  },
  "routing": {
    "dataset_id": "ds_kaveon_events",
    "lane": "context",
    "question_class": "metric_by_dimension"
  },
  "evidence": {
    "context_version": 42,
    "source": {
      "catalog": "OpenSource",
      "schema": "public",
      "table": "kaveon_events_enriched",
      "snapshot": "delta-v18"
    },
    "freshness": {"state": "fresh", "score": 0.94},
    "rows": 2,
    "elapsed_ms": 132,
    "reproduce": {
      "method": "POST /api/v1/dlm/reproduce",
      "dataset_id": "ds_kaveon_events"
    }
  },
  "principal": {"subject": "user@example.com", "entry_point": "studio"}
}
```

The response must distinguish these states:

* `SUCCEEDED`: answer rows and evidence are available.
* `RUNNING`: a request ID and statement ID are available for polling.
* `CLARIFICATION_REQUIRED`: multiple governed interpretations remain.
* `REFUSED`: the system will not execute the request; see below.
* `FAILED`: execution or an upstream source failed.
* `CANCELLED`: the caller or server stopped the request.

The exact field names may be versioned as the route is implemented. Existing
DLM responses remain authoritative for current clients.

## Routing and SQL handoff

The proposed agent sequence is:

```text
authenticate principal
  -> filter catalogs, schemas, and datasets by permission
  -> route to a supported question class
  -> evaluate context freshness
  -> answer from context when covered and fresh
  -> otherwise assemble one bounded read-only SQL statement
  -> execute through Engine or the selected registered source
  -> return rows plus evidence and a reproducible statement
```

The handoff rules are strict:

* SQL is generated from registered dataset semantics; a question cannot invent
  a table or column.
* SQL is read-only, platform tables are protected, and interactive output is
  bounded. A bare `SELECT *` receives the SQL Lab preview cap.
* The selected execution path is recorded. Kaveon does not imply that a
  relationship is executable across sources merely because metadata declares a
  relationship.
* Context answers carry `context_version` and source snapshot evidence. Live
  answers carry the statement/query ID, SQL, source, timing, and result digest
  where available.
* Reproduction is an explicit action. It must re-check current authorization;
  possession of old SQL or an old answer ID does not grant access.

## Identity and permission model

Identity is server asserted. A browser session, trusted proxy, or bearer token
is validated by the API and becomes a `UserContext` containing the principal,
role, and any tenant/account boundary. Request JSON is never used to elevate
that context.

Before routing, the API evaluates:

1. catalog and schema visibility;
2. dataset query permission and ownership rules;
3. source-specific credentials and row/column policy inputs; and
4. the operation permission (`query`, `reproduce`, `chart`, `dashboard`, or
   administrative context editing).

An unauthorized dataset must not appear in ranking, autocomplete, suggested
clarifications, error text, timing details, or evidence. Context caches must be
keyed by principal/tenant policy where those boundaries exist. Local Docker
development may use `auth none` only as an explicitly insecure development
mode; it must not be treated as production identity.

## Cancellation and lifecycle

Every long-running request receives a request ID before work starts. If the
client leaves a page, closes a chat, hits Cancel, or exceeds its deadline, the
API should cancel the associated Engine statement and release result pages,
exchange buffers, and spill files.

The current implementation already supports owner-scoped cancellation for
streamed SQL (`DELETE /lab/query/{id}`) and tagged Engine statements
(`DELETE /sql/engine/cancel/{cancel_token}`). The target IQ route should map
its request ID to one of these mechanisms rather than inventing a second
cancellation protocol.

Cancellation is idempotent. A completed statement is not an error merely
because a later cancel arrives. The final state must be observable as
`CANCELLED` when the server successfully stops it; a race with completion may
legitimately return `SUCCEEDED`.

## Refusal and clarification behavior

Refusal is a product feature: it is safer than fabricating a metric or running
an unauthorized scan. A refusal response should include a stable machine code,
a short safe message, and a remediation that does not reveal hidden metadata.

| Code | Meaning | Example remediation |
|---|---|---|
| `NO_VISIBLE_DATASET` | No permitted dataset matches | Select a dataset or ask an administrator for access |
| `UNSUPPORTED_QUESTION` | The DLM has no governed template for the request | Ask for a supported metric, dimension, filter, or time range |
| `AMBIGUOUS_INTENT` | More than one permitted interpretation matches | Name the dataset, metric, or dimension |
| `STALE_CONTEXT` | Context is not valid for a safe context answer and live fallback is unavailable | Rebuild the DLM or retry with a permitted live path |
| `SOURCE_NOT_QUALIFIED` | The selected source/execution path cannot perform the operation | Choose a qualified catalog or supported query shape |
| `PERMISSION_DENIED` | The operation is outside the principal's policy | Request the required role/access |
| `RESOURCE_LIMIT` | Deadline, row, memory, or admission limit would be exceeded | Narrow the filter or increase an approved budget |
| `UNSAFE_SQL` | The generated or supplied statement fails read-only/platform guards | Use a bounded read-only query |

Do not expose whether a hidden dataset exists. For ambiguity that can be
resolved without revealing hidden options, return a clarification request with
only the permitted choices.

## Audit and observability

The minimum trace fields are `request_id`, `statement_id`/`query_id` when
present, principal, entry point, dataset/catalog/schema, routing lane,
context version, status, elapsed time, row count, bytes, and cancellation or
refusal code. SQL and question text are subject to the deployment's redaction
policy; secrets and source credentials never enter evidence.

The API should emit one terminal audit event for every request, including
refusals and cancellations. Query history and DLM evidence may be retained in
KaveonDB according to the configured retention policy.

## Local Docker qualification

Before a unified IQ route is enabled, qualify the existing building blocks in
the local Docker profile:

1. Compile a fixture dataset and record its context version.
2. Ask the same supported question twice; verify deterministic routing and SQL.
3. Verify a fresh context returns `lane=context` and no source scan.
4. Change the fixture/snapshot, verify stale context falls back or refuses.
5. Verify an unauthorized principal cannot route, resolve values, reproduce,
   or infer the hidden dataset.
6. Verify bounded SQL rejects writes, platform tables, and unsafe/unbounded
   requests.
7. Start a streamed query, cancel it, and verify the terminal state plus no
   retained result pages/spill files after cleanup.
8. Verify evidence contains the source, version, lane, statement/query ID, and
   enough information for an authorized reproduction.

These checks are local contract tests; they do not require AKS, Azure, or a
hosted model. A Docker pass is necessary but not sufficient for a distributed
release qualification.

## Acceptance bar for shipping IQ

The unified agent surface may be called shipping when:

* current DLM and SQL routes preserve backward compatibility;
* every successful answer is permission-filtered, bounded, deterministic, and
  evidence-bearing;
* refusal/clarification states are machine-readable and do not leak metadata;
* cancellation reaches the underlying statement and cleans up resources;
* local Docker qualification passes repeatedly, including restart and failure
  cases; and
* the public documentation labels target features separately from implemented
  behavior.

The contract does not require pretending that Kaveon IQ is a general-purpose
LLM agent. Its advantage is governed, reproducible data reasoning with a
clear handoff to Kaveon Engine.
