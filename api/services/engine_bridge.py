"""Opt-in Engine control/data plane client. No secrets enter catalog definitions."""
import json
import re
import threading
import time
import uuid
import os
import ssl
from urllib.parse import urlsplit, quote

import httpx
from fastapi import HTTPException


def _endpoint():
    value = os.getenv("KAVEON_ENGINE_URL", "").rstrip("/")
    parsed = urlsplit(value)
    if not value:
        raise HTTPException(503, "Engine integration is not configured")
    if parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise HTTPException(503, "Invalid Engine endpoint configuration")
    if parsed.scheme != "https" and not (
        parsed.scheme == "http" and (
            parsed.hostname in {"localhost", "127.0.0.1", "::1"}
            or os.getenv("KAVEON_ENGINE_PRIVATE_HTTP") == "true"
        )
    ):
        raise HTTPException(503, "Engine requires HTTPS or an explicitly isolated private HTTP boundary")
    return value


def _verify_context():
    """Use system trust by default, or a configured private CA. Never disable TLS verification."""
    ca_path = os.getenv("KAVEON_ENGINE_CA_CERT")
    if not ca_path:
        return True
    try:
        return ssl.create_default_context(cafile=ca_path)
    except (OSError, ssl.SSLError):
        raise HTTPException(503, "Engine CA certificate is unavailable or invalid") from None


def _send(method, path, token_name, actor, *, payload=None, revision=None, role=None, timeout=60):
    """One Engine round trip. Returns the raw response; only transport failures raise."""
    token = os.getenv(token_name)
    if not token:
        raise HTTPException(503, "Engine service credential is not configured")
    headers = {"Authorization": f"Bearer {token}", "x-kaveon-actor": actor}
    if role:
        headers.update({"x-kaveon-principal": actor, "x-kaveon-role": role})
    if revision is not None:
        headers["If-Match"] = str(revision)
    try:
        return httpx.request(method, _endpoint() + path, headers=headers, json=payload,
                             timeout=timeout, follow_redirects=False, verify=_verify_context())
    except httpx.TimeoutException:
        # The statement may still be running on the Engine; the caller chose how
        # long it was prepared to wait, so say that rather than "unavailable".
        raise HTTPException(504, f"Engine statement exceeded the client bound ({timeout}s)") from None
    except httpx.HTTPError:
        raise HTTPException(502, "Engine is unavailable") from None


# ── Engine refusals ───────────────────────────────────────────────────────────
# The coordinator answers 429 for two different things. Admission exhaustion
# (`MEMORY_ADMISSION_REJECTED`, `RESOURCE_GROUP_REJECTED`) is momentary and
# the Studio retries it, so it stays the short message it always was. The
# per-principal live-read quota of a demo coordinator (`RATE_LIMITED`) is not
# retryable until the time the Engine names, so its body — code, message,
# `retry_after_seconds`, `next_allowed_at` — is passed through unchanged for
# the Studio to show as a notice.
QUOTA_CODE = "RATE_LIMITED"


def _too_many_requests(body):
    """The HTTPException for an Engine 429 whose JSON body is `body`."""
    if isinstance(body, dict) and body.get("code") == QUOTA_CODE:
        retry_after = body.get("retry_after_seconds")
        retry_after = int(retry_after) if isinstance(retry_after, (int, float)) and retry_after >= 0 else 1
        detail = {"code": QUOTA_CODE, "message": str(body.get("message") or body.get("error") or "Query quota exhausted"),
                  "retry_after_seconds": retry_after}
        for key in ("next_allowed_at", "limit", "resource_group"):
            if key in body:
                detail[key] = body[key]
        return HTTPException(429, detail, headers={"Retry-After": str(retry_after)})
    return HTTPException(429, "Engine query capacity is temporarily exhausted", headers={"Retry-After": "1"})


def _response_json(response):
    try:
        return response.json()
    except ValueError:
        return None


def _request(method, path, token_name, actor, *, payload=None, revision=None, role=None, timeout=60):
    response = _send(method, path, token_name, actor, payload=payload, revision=revision, role=role, timeout=timeout)
    if response.status_code == 404:
        return None
    # Admission exhaustion is temporary. Preserve it for the Studio's bounded
    # retry path instead of turning a healthy Engine into an opaque 502.
    if response.status_code == 429:
        raise _too_many_requests(_response_json(response))
    if response.status_code in {409, 412, 428}:
        raise HTTPException(409, "Engine revision conflict; reload before retrying")
    if not response.is_success:
        raise HTTPException(502, "Engine rejected the request")
    try:
        return response.json()
    except ValueError:
        return None   # 204 and other bodiless successes (a cancelled query)


def _object(value):
    return json.loads(value) if isinstance(value, str) else value or {}


def definition(source):
    storage = {"local": "Local", "adls_gen2": "AdlsGen2", "s3": "S3"}
    credentials = {"managed_identity": "ManagedIdentity", "workload_identity": "WorkloadIdentity",
                   "environment": "Environment", "secret_store": "SecretStore"}
    # External adapters are configuration records, not executable Engine adapters.
    if source["adapter_type"] != "native" or _object(source.get("adapter_config")):
        raise HTTPException(422, "Only native catalog adapters can currently be synchronized")
    config = _object(source["storage_config"])
    allowed = {"local": {"base_path"}, "adls_gen2": {"account", "container", "root_path"},
               "s3": {"bucket", "region", "prefix"}}[source["storage_type"]]
    if set(config) != allowed:
        raise HTTPException(422, "Storage configuration must contain only the required location fields")
    kind, reference = source.get("credential_kind"), source.get("credential_ref")
    if bool(kind) != bool(reference):
        raise HTTPException(422, "Credential kind and indirect reference must be supplied together")
    return {"id": "platform-" + str(source["id"]), "name": source["engine_catalog"],
            "revision": 1, "adapter": "Native", "storage": {storage[source["storage_type"]]: config},
            "credential": {"kind": credentials[kind], "reference": reference} if kind else None,
            "lifecycle": source["lifecycle"].capitalize()}


def sync_catalog(source, actor, expected_revision=None):
    desired = definition(source)
    path = "/v1/catalog/definitions/" + quote(desired["id"], safe="")
    token = "KAVEON_ENGINE_CATALOG_TOKEN"
    current = _request("GET", path, token, actor)
    created = current is None
    if current is None:
        if expected_revision is not None:
            raise HTTPException(409, "Mapped Engine catalog no longer exists")
        # Engine catalogs must first be created as draft revision 1.
        if desired["lifecycle"] not in {"Draft", "Active"}:
            raise HTTPException(409, "Synchronize a source while draft or active before later lifecycle transitions")
        draft = {**desired, "lifecycle": "Draft"}
        current = _request("POST", "/v1/catalog/definitions", token, actor, payload=draft)
        expected_revision = current["revision"]
    comparable = {**desired, "revision": current["revision"]}
    if comparable == current:
        return {"catalog": current, "changed": created}
    if expected_revision is None or expected_revision != current["revision"]:
        raise HTTPException(409, "Supply the current Engine revision to synchronize changes")
    desired["revision"] = current["revision"] + 1
    result = _request("PUT", path, token, actor, payload=desired, revision=current["revision"])
    return {"catalog": result, "changed": True}


def cancel_tagged(tag, actor, role):
    """Cancel every statement carrying *tag* that the Engine still reports as
    running. Best effort: a statement that finished between the client's
    timeout and this call is simply not there any more."""
    try:
        queries = _request("GET", "/v1/query", "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=role) or []
    except HTTPException:
        return 0
    cancelled = 0
    for record in queries if isinstance(queries, list) else []:
        tags = record.get("client_tags") or (record.get("context") or {}).get("client_tags") or []
        if tag in tags and str(record.get("state", "")).upper() in {"QUEUED", "PLANNING", "RUNNING", "STARTING"}:
            try:
                _request("DELETE", "/v1/query/" + quote(str(record["id"]), safe=""),
                         "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=role)
                cancelled += 1
            except HTTPException:
                continue
    return cancelled


def execute(sql, catalog, actor, role, schema=None, timeout=60, settings=None):
    """Run one statement. `timeout` is how long this caller waits for the
    response: 60 s suits an interactive request; a DLM build passes its own
    bound because a full-table aggregate legitimately runs for minutes. When
    the bound passes, the statement is cancelled on the Engine as well: a
    client that has given up must not leave a full-table scan running for
    everyone else. `settings` is the Engine's per-request settings object
    (`query_memory_limit_bytes`, `local_parallelism`, `result_cache`); it is
    sent only when given, so callers that do not pass it are unchanged."""
    engine_role = _sql_role(role)
    tag = "kaveon-api:" + uuid.uuid4().hex
    payload = _statement_payload(sql, catalog, schema, tag, settings)
    try:
        result = _request("POST", "/v1/statement", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                          payload=payload, role=engine_role, timeout=timeout)
    except HTTPException as error:
        if error.status_code == 504:
            cancel_tagged(tag, actor, engine_role)
        raise
    if result is None:
        raise HTTPException(422, "Engine query failed")
    query_id = result.get("id")
    details = None
    if query_id:
        try:
            details = _request(
                "GET", "/v1/query/" + quote(str(query_id), safe=""),
                "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=engine_role,
            )
        except HTTPException:
            # Telemetry enrichment is best effort after a successful statement;
            # never turn a completed query into an execution failure.
            details = None
        if isinstance(details, dict):
            result["query_details"] = details
    if result.get("error"):
        # Preserve the opaque Engine UUID and bounded query details for the
        # server-side history record. Never return statement diagnostics or
        # credentials directly to the browser.
        raise HTTPException(422, {
            "message": "Engine query failed",
            "query_id": query_id,
            **({"engine_details": details} if isinstance(details, dict) else {}),
        })
    return result


def _sql_role(role):
    roles = {"Analyst": "analyst", "Editor": "analyst", "Admin": "admin"}
    if role not in roles:
        raise HTTPException(403, "A recognized Kaveon role is required for Engine SQL")
    return roles[role]


def _statement_payload(sql, catalog, schema, tag, settings):
    payload = {"query": sql, "catalog": catalog, "schema": schema,
               "source": "studio", "client": "kaveon-api", "client_tags": [tag]}
    if settings is not None:
        payload["settings"] = dict(settings)
    return payload


# ── Streamed statements ───────────────────────────────────────────────────────
# A statement submitted with `result_delivery: "paged"` writes its rows to
# pages on the coordinator while it runs (1,000 rows or 4 MiB each), and its
# record carries `next_uri` for page 0 from the moment it is RUNNING. The
# Studio reads the record and the pages through the routes below while the
# POST that holds the statement waits on a background thread here; when that
# POST returns the thread reads the final record and the row total and hands
# them to `on_finish`, which is where the platform's query history is written.
# Every read is scoped by the coordinator to the actor that submitted, so the
# same actor must stamp the submit and the reads.

TERMINAL_STATES = {"FINISHED", "FAILED", "CANCELED"}
# How long a submit waits for the coordinator to register the statement's
# record before giving up; the statement is cancelled by its tag when it does.
SUBMIT_WAIT_SECONDS = 30.0
# How long the background thread holds the statement's POST. The coordinator
# applies its own statement limits; this only bounds a lost connection.
STREAM_HOLD_SECONDS = 4 * 3600


def find_tagged(tag, actor, role):
    """The query record carrying *tag* among those this principal can see, or None."""
    queries = _request("GET", "/v1/query", "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=role)
    for record in queries if isinstance(queries, list) else []:
        tags = record.get("client_tags") or (record.get("context") or {}).get("client_tags") or []
        if tag in tags:
            return record
    return None


class StreamedStatement:
    """A paged statement in flight: its tag, its record id once known, and
    the outcome of the POST that holds it."""

    def __init__(self, tag):
        self.tag = tag
        self.query_id = None
        self.record = None
        self.done = threading.Event()
        self.status = None
        self.body = None
        self.error = None
        self.wall_ms = None


def _submit_error(statement):
    """The refusal to return when the POST failed before the record existed."""
    if statement.error is not None:
        return statement.error
    status, body = statement.status, statement.body if isinstance(statement.body, dict) else {}
    message = body.get("error") if isinstance(body.get("error"), str) and body.get("error").strip() else None
    if status == 429:
        return _too_many_requests(body)
    if status == 400:
        return HTTPException(400, message or "Engine refused the statement")
    return HTTPException(502, message or "Engine rejected the request")


def _hold(statement, payload, actor, role, timeout, on_finish):
    """Body of the background thread: post the statement, wait for it, then
    resolve its record and row total and report through `on_finish`."""
    engine_role = _sql_role(role)
    started = time.monotonic()
    try:
        try:
            response = _send("POST", "/v1/statement", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                             payload=payload, role=engine_role, timeout=timeout)
        except HTTPException as error:
            if error.status_code == 504:
                cancel_tagged(statement.tag, actor, engine_role)
            statement.error = error
        else:
            statement.status = response.status_code
            try:
                statement.body = response.json()
            except ValueError:
                statement.body = {}
        statement.wall_ms = int((time.monotonic() - started) * 1000)
        if statement.query_id is None:
            query_id = statement.body.get("id") if isinstance(statement.body, dict) else None
            if not query_id:
                try:
                    record = find_tagged(statement.tag, actor, engine_role)
                except HTTPException:
                    record = None
                query_id = record.get("id") if isinstance(record, dict) else None
            statement.query_id = str(query_id) if query_id else None
    finally:
        statement.done.set()
    if on_finish is not None and statement.query_id:
        on_finish(finish_summary(statement, actor, role))


def finish_summary(statement, actor, role):
    """The final record and row total of a finished streamed statement, for
    the history row. Every read here is best effort: a coordinator that has
    already expired the result still leaves a usable summary."""
    record = None
    try:
        record = query(statement.query_id, actor, role)
    except HTTPException:
        record = None
    record = record if isinstance(record, dict) else {}
    row_count = None
    if str(record.get("state", "")).upper() == "FINISHED":
        try:
            status, page = result_page(statement.query_id, 0, actor, role)
        except HTTPException:
            status, page = None, None
        if status == 200 and isinstance(page, dict) and isinstance(page.get("row_count"), int):
            row_count = page["row_count"]
    elapsed = record.get("elapsed_ms")
    state = str(record.get("state") or ("FINISHED" if statement.status == 200 else "FAILED")).upper()
    error = record.get("error")
    if not error and statement.error is not None:
        error = statement.error.detail if isinstance(statement.error.detail, str) else "Engine statement failed"
    if not error and isinstance(statement.body, dict) and isinstance(statement.body.get("error"), str):
        error = statement.body["error"]
    return {
        "query_id": statement.query_id, "state": state,
        "error": error if isinstance(error, str) else None,
        "elapsed_ms": int(elapsed) if isinstance(elapsed, (int, float)) and elapsed > 0 else statement.wall_ms,
        "row_count": row_count, "record": record or None,
    }


def submit_streamed(sql, catalog, actor, role, schema=None, settings=None, on_finish=None,
                    wait=SUBMIT_WAIT_SECONDS, hold=STREAM_HOLD_SECONDS):
    """Submit one statement with paged delivery and return once its record
    exists: `{"query_id", "tag", "record"}`. The POST that holds the
    statement runs on a daemon thread; `on_finish(summary)` is called from
    that thread when it returns (see `finish_summary`). A statement the
    coordinator refuses before it has a record — a parse error, exhausted
    capacity — raises the refusal here instead."""
    engine_role = _sql_role(role)
    tag = "kaveon-api:stream:" + uuid.uuid4().hex
    payload = {**_statement_payload(sql, catalog, schema, tag, settings), "result_delivery": "paged"}
    statement = StreamedStatement(tag)
    thread = threading.Thread(target=_hold, args=(statement, payload, actor, role, hold, on_finish),
                              name="kaveon-stream-" + tag[-12:], daemon=True)
    thread.start()
    deadline = time.monotonic() + wait
    while True:
        try:
            record = find_tagged(tag, actor, engine_role)
        except HTTPException:
            record = None
        if isinstance(record, dict) and record.get("id"):
            if statement.query_id is None:
                statement.query_id = str(record["id"])
            statement.record = record
            return {"query_id": statement.query_id, "tag": tag, "record": record}
        if statement.done.is_set():
            if statement.query_id:
                record = query(statement.query_id, actor, role)
                if isinstance(record, dict):
                    return {"query_id": statement.query_id, "tag": tag, "record": record}
            raise _submit_error(statement)
        if time.monotonic() >= deadline:
            cancel_tagged(tag, actor, engine_role)
            raise HTTPException(504, "Engine did not register the statement within the submit bound")
        statement.done.wait(0.1)


def result_page(query_id, page, actor, role):
    """One page of a paged result: `(status, body)` with the coordinator's
    status passed through — 200 with the rows, 202 while the page is not yet
    written, 404 past the end or unknown, 410 once the statement failed or
    was cancelled. Only transport failures raise."""
    response = _send("GET", "/v1/query/" + quote(str(query_id), safe="") + "/results/" + str(int(page)),
                     "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role))
    if response.status_code == 429:
        raise _too_many_requests(_response_json(response))
    if response.status_code in {200, 202, 404, 410}:
        try:
            body = response.json()
        except ValueError:
            body = None
        return response.status_code, body
    if response.status_code in {401, 403}:
        raise HTTPException(502, "Engine rejected the bridge credential")
    raise HTTPException(502, "Engine rejected the request")


def cancel(query_id, actor, role):
    """Cancel one statement by id. True when the coordinator accepted the
    cancellation, False when it no longer knows the statement."""
    result = _send("DELETE", "/v1/query/" + quote(str(query_id), safe=""),
                   "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role))
    if result.status_code == 404:
        return False
    if result.is_success:
        return True
    raise HTTPException(502, "Engine rejected the request")


def native_analyze_supported():
    """Return true only when the connected Engine explicitly advertises ANALYZE."""
    try:
        result = _request(
            "GET", "/v1/capabilities", "KAVEON_ENGINE_BRIDGE_TOKEN",
            "kaveon-system", role="reader",
        )
    except HTTPException:
        return False
    return isinstance(result, dict) and result.get("native_analyze") is True


def statistics(actor, role):
    """Bounded, credential-free durable statistics diagnostics for administrators."""
    if role != "Admin":
        raise HTTPException(403, "Administrator access is required for Engine statistics")
    return _request("GET", "/v1/statistics", "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role="admin")


def _read_role(role):
    roles = {"Viewer": "reader", "Analyst": "analyst", "Editor": "analyst", "Admin": "admin"}
    try:
        return roles[role]
    except KeyError:
        raise HTTPException(403, "A recognized Kaveon role is required for Engine catalog access") from None


def catalogs(actor, role):
    return _request("GET", "/v1/catalog", "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role))


def cluster(actor, role):
    """Coordinator, worker, uptime, and memory telemetry for the operations console."""
    return _request("GET", "/v1/cluster", "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role))


def queries(actor, role):
    """Query history visible to this principal. The Engine applies ownership scoping."""
    return _request("GET", "/v1/query", "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role))


def query(query_id, actor, role):
    """One query record, or None when it does not exist or is not visible to this principal."""
    return _request(
        "GET", "/v1/query/" + quote(query_id, safe=""),
        "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role),
    )


def schemas(catalog, actor, role):
    return _request(
        "GET", "/v1/catalog/" + quote(catalog, safe="") + "/schema",
        "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role),
    )


def tables(catalog, schema, actor, role):
    return _request(
        "GET", "/v1/catalog/" + quote(catalog, safe="") + "/schema/" + quote(schema, safe="") + "/table",
        "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role),
    )


def table_columns(catalog, schema, table, actor, role):
    """Columns of one table definition, read through catalog metadata, never through SQL."""
    return table_definition(catalog, schema, table, actor, role)["columns"]


def table_definition(catalog, schema, table, actor, role):
    """One complete table definition: location, access pattern, format, revision, columns."""
    scoped_role = _read_role(role)
    catalogs = _request("GET", "/v1/catalog/definitions", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                        role=scoped_role) or []
    catalog_definition = next((item for item in catalogs if item.get("name") == catalog), None)
    if not catalog_definition or not catalog_definition.get("id"):
        raise HTTPException(404, "Engine catalog definition not found")
    schemas = _request(
        "GET", "/v1/catalog/definitions/" + quote(str(catalog_definition["id"]), safe="") + "/schemas",
        "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=scoped_role,
    ) or []
    schema_definition = next((item for item in schemas if item.get("name") == schema), None)
    if not schema_definition or not schema_definition.get("id"):
        raise HTTPException(404, "Engine schema definition not found")
    definitions = _request(
        "GET", "/v1/catalog/schemas/" + quote(str(schema_definition["id"]), safe="") + "/tables",
        "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=scoped_role,
    ) or []
    definition = next((item for item in definitions if item.get("name") == table), None)
    if not definition:
        raise HTTPException(404, "Engine table definition not found")
    if not isinstance(definition.get("columns"), list):
        raise HTTPException(502, "Engine table definition is invalid")
    return definition


# ── Catalog management ───────────────────────────────────────────────────────
# Trino users add catalogs, schemas and tables by name; the Engine keeps stable
# ids, optimistic revisions and a Draft → Active lifecycle underneath. These
# helpers speak the Engine's catalog API with the catalog-admin credential and
# keep the Engine's own refusal message, because the person registering a
# table needs to read why a definition or a location was refused. Locations
# are storage paths and credentials are references; nothing here is secret.

# ── Catalog access: the grants family and the caller's standing ───────────────

def _access_response(response, fallback):
    """The Engine's answer for a catalog access call, its message and code
    kept: validation refusals are 422, revision conflicts 409 (reload and
    retry), an unknown catalog 404, the admin gate 403, a store that is not
    configured or reachable 503."""
    status = response.status_code
    body = _response_json(response)
    code = body.get("code") if isinstance(body, dict) else None
    message = _engine_message(response, fallback)
    if status == 400:
        raise HTTPException(422, {"code": code or "invalid_request", "message": message})
    if status == 403:
        raise HTTPException(403, {"code": "forbidden", "message": _engine_message(response, "Engine refused the administrator credential")})
    if status == 404:
        raise HTTPException(404, {"code": code or "not_found", "message": message})
    if status == 409:
        raise HTTPException(409, {"code": code or "revision_conflict", "message": message})
    if status in {401, 503}:
        raise HTTPException(503, {"code": code or "engine_unavailable", "message": _engine_message(response, "Engine catalog access is unavailable")})
    if not response.is_success:
        raise HTTPException(502, "Engine rejected the request")
    if not isinstance(body, dict):
        raise HTTPException(502, "Engine returned an invalid catalog access document")
    return body


def catalog_access(actor, role):
    """Every grant, the grantable catalogs, the reserved authority and the role ceilings."""
    response = _send("GET", "/v1/admin/catalog-access", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                     role=_admin_role(role, "catalog access"))
    return _access_response(response, "Engine could not read the catalog grants")


def grant_catalog_access(document, actor, role):
    """Create a grant, or change one at the revision the document names."""
    response = _send("PUT", "/v1/admin/catalog-access/grants", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                     payload=document, role=_admin_role(role, "catalog access"))
    return _access_response(response, "Engine refused the grant")


def revoke_catalog_access(document, actor, role):
    """Remove a grant at the revision the document names."""
    response = _send("DELETE", "/v1/admin/catalog-access/grants", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                     payload=document, role=_admin_role(role, "catalog access"))
    return _access_response(response, "Engine refused the revoke")


def effective_catalog_access(principal, actor, role):
    """What each Engine role reaches on one principal's grants."""
    response = _send("GET", "/v1/admin/catalog-access/effective/" + quote(principal, safe=""),
                     "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_admin_role(role, "catalog access"))
    return _access_response(response, "Engine could not read the principal's access")


def import_catalog_access(document, actor, role):
    """The open-policy reconciliation: a proposal from the audit ledger, recorded only with `apply`."""
    response = _send("POST", "/v1/admin/catalog-access/import", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                     payload=document, role=_admin_role(role, "catalog access"), timeout=120)
    return _access_response(response, "Engine refused the import")


def my_catalog_access(actor, role):
    """The caller's own catalogs and levels, as the Engine evaluates them."""
    response = _send("GET", "/v1/catalog-access/me", "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role))
    return _access_response(response, "Engine could not read the caller's access")


CATALOG_TOKEN = "KAVEON_ENGINE_CATALOG_TOKEN"


def _engine_message(response, fallback):
    try:
        body = response.json()
    except ValueError:
        return fallback
    message = body.get("error") if isinstance(body, dict) else None
    return message.strip() if isinstance(message, str) and message.strip() else fallback


def catalog_request(method, path, actor, *, payload=None, revision=None):
    """A catalog definition read or mutation. Refusals carry the Engine's message."""
    response = _send(method, path, CATALOG_TOKEN, actor, payload=payload, revision=revision)
    status = response.status_code
    if status == 404:
        return None
    if status in {409, 412, 428}:
        raise HTTPException(409, _engine_message(response, "Engine revision conflict; reload before retrying"))
    if status == 400:
        raise HTTPException(422, _engine_message(response, "Engine refused the definition"))
    if status == 401:
        raise HTTPException(502, "Engine rejected the catalog credential")
    if status in {403, 503}:
        raise HTTPException(503, _engine_message(response, "Engine catalog mutations are unavailable"))
    if not response.is_success:
        raise HTTPException(502, "Engine rejected the request")
    try:
        return response.json()
    except ValueError:
        return None   # 204 from a delete


def catalog_definitions(actor, role):
    """Every durable catalog definition the Engine holds, in every lifecycle state."""
    result = _request("GET", "/v1/catalog/definitions", "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role))
    return result if isinstance(result, list) else []


def catalog_definition(catalog_id, actor, role):
    return _request("GET", "/v1/catalog/definitions/" + quote(catalog_id, safe=""),
                    "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role))


def schema_definitions(catalog_id, actor, role):
    result = _request("GET", "/v1/catalog/definitions/" + quote(catalog_id, safe="") + "/schemas",
                      "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role))
    return result if isinstance(result, list) else []


def schema_definition(schema_id, actor, role):
    return _request("GET", "/v1/catalog/schemas/" + quote(schema_id, safe=""),
                    "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role))


def table_definitions(schema_id, actor, role):
    result = _request("GET", "/v1/catalog/schemas/" + quote(schema_id, safe="") + "/tables",
                      "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role))
    return result if isinstance(result, list) else []


def table_definition_by_id(table_id, actor, role):
    return _request("GET", "/v1/catalog/tables/" + quote(table_id, safe=""),
                    "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role))


def table_version(table_id, actor, role):
    """The table's current source version — `{table_id, table, source_version,
    observed_at_ms}` from the least metadata that establishes it (a Delta log
    tail, an Iceberg pointer, a listing, a file's identity), never a data
    page. The platform's freshness signal for Engine-backed datasets; None
    for an unknown table."""
    return _request("GET", "/v1/catalog/tables/" + quote(table_id, safe="") + "/version",
                    "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role))


def table_statistics(table_id, actor, role):
    """The table's statistics record and the version observed now — `{table_id,
    table, source_version, current_source_version, observed_at_ms, stale,
    statistics}`. What DLM auto-curation derives a dataset's context spec from:
    per-column distinct counts, bounds, null counts and the sketches. None for
    a table that has never been analyzed (the Engine answers 404) or one the
    Engine will not publish."""
    return _request("GET", "/v1/catalog/tables/" + quote(table_id, safe="") + "/statistics",
                    "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role))


def _activate(path, definition, actor):
    """Draft revision 1 → Active revision 2. The Engine publishes only Active
    definitions into its query snapshot, so nothing is queryable before this."""
    active = {**definition, "revision": definition["revision"] + 1, "lifecycle": "Active"}
    return catalog_request("PUT", path, actor, payload=active, revision=definition["revision"])


def create_schema(catalog_id, schema_id, name, actor):
    draft = {"id": schema_id, "catalog_id": catalog_id, "name": name, "revision": 1, "lifecycle": "Draft"}
    created = catalog_request("POST", "/v1/catalog/definitions/" + quote(catalog_id, safe="") + "/schemas",
                              actor, payload=draft)
    if not isinstance(created, dict):
        raise HTTPException(502, "Engine returned an invalid schema definition")
    return _activate("/v1/catalog/schemas/" + quote(schema_id, safe=""), created, actor)


def delete_schema(schema_id, revision, actor):
    catalog_request("DELETE", "/v1/catalog/schemas/" + quote(schema_id, safe=""), actor, revision=revision)


def create_table(definition, actor):
    """Register a table as Draft and activate it. Returns the Active definition."""
    draft = {**definition, "revision": 1, "lifecycle": "Draft"}
    created = catalog_request("POST", "/v1/catalog/schemas/" + quote(definition["schema_id"], safe="") + "/tables",
                              actor, payload=draft)
    if not isinstance(created, dict):
        raise HTTPException(502, "Engine returned an invalid table definition")
    return _activate("/v1/catalog/tables/" + quote(definition["id"], safe=""), created, actor)


def replace_table(definition, revision, actor):
    """Revision-replace one table; `revision` is the caller's If-Match."""
    payload = {**definition, "revision": revision + 1}
    return catalog_request("PUT", "/v1/catalog/tables/" + quote(definition["id"], safe=""), actor,
                           payload=payload, revision=revision)


def delete_table(table_id, revision, actor):
    catalog_request("DELETE", "/v1/catalog/tables/" + quote(table_id, safe=""), actor, revision=revision)


def create_table_inferred(catalog, schema, table, location, fmt, actor, role, timeout=120):
    """Register a table through the Engine's own statement path — `CREATE
    TABLE … WITH (location, format)` with no column list — so the columns come
    from the table's metadata (the Delta log, the Iceberg metadata, the first
    Parquet footer) and the Engine's Draft → metadata probe → Active sequence
    runs in one statement: an unreadable location registers nothing. Returns
    {"ok": True, "result"} or {"ok": False, "message", "code"} with the
    Engine's error verbatim."""
    roles = {"Editor": "analyst", "Admin": "admin"}
    if role not in roles:
        raise HTTPException(403, "The Editor role is required to register a table")
    if not all(re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", part) for part in (schema, table)):
        raise HTTPException(422, "Schema and table names must be plain SQL identifiers")
    ddl_format = str(fmt).lower()
    if ddl_format not in ("parquet", "delta", "iceberg"):
        raise HTTPException(422, "format must be parquet, delta or iceberg")
    quoted_location = location.replace("'", "''")
    sql = (f"CREATE TABLE {schema}.{table} "
           f"WITH (location = '{quoted_location}', format = '{ddl_format}')")
    tag = "kaveon-api:catalog-create:" + uuid.uuid4().hex
    payload = {"query": sql, "catalog": catalog, "schema": schema, "source": "studio",
               "client": "kaveon-api", "client_tags": [tag]}
    try:
        response = _send("POST", "/v1/statement", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                         payload=payload, role=roles[role], timeout=timeout)
    except HTTPException as error:
        if error.status_code == 504:
            cancel_tagged(tag, actor, roles[role])
        raise
    try:
        body = response.json()
    except ValueError:
        body = {}
    if not response.is_success or (isinstance(body, dict) and body.get("error")):
        message = _engine_message(response, "Engine could not register the table")
        code = body.get("code") if isinstance(body, dict) and isinstance(body.get("code"), str) else None
        return {"ok": False, "message": message, "code": code}
    return {"ok": True, "result": body}


def probe_table(catalog, schema, table, actor, role, timeout=120):
    """Read the table once through the Engine — `SELECT COUNT(*)`, result cache
    bypassed — so registration proves the location is readable. Parquet and
    Delta answer from footers and the pinned snapshot without scanning rows.
    Returns {"ok": True, "row_count", "elapsed_ms", "query_id"} or
    {"ok": False, "message", "code"} with the Engine's storage or analysis
    error verbatim: the registrant needs the path or schema mismatch it names."""
    roles = {"Editor": "analyst", "Admin": "admin"}
    if role not in roles:
        raise HTTPException(403, "The Editor role is required to verify a table")
    if not all(re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", part) for part in (schema, table)):
        raise HTTPException(422, "Schema and table names must be plain SQL identifiers to be verified")
    tag = "kaveon-api:catalog-probe:" + uuid.uuid4().hex
    payload = {"query": f"SELECT COUNT(*) FROM {schema}.{table}", "catalog": catalog, "schema": schema,
               "source": "studio", "client": "kaveon-api", "client_tags": [tag],
               "settings": {"result_cache": False}}
    try:
        response = _send("POST", "/v1/statement", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                         payload=payload, role=roles[role], timeout=timeout)
    except HTTPException as error:
        if error.status_code == 504:
            cancel_tagged(tag, actor, roles[role])
        raise
    try:
        body = response.json()
    except ValueError:
        body = {}
    if response.status_code == 429:
        raise _too_many_requests(body)
    if not response.is_success or (isinstance(body, dict) and body.get("error")):
        message = _engine_message(response, "Engine could not read the table")
        code = body.get("code") if isinstance(body, dict) and isinstance(body.get("code"), str) else None
        return {"ok": False, "message": message, "code": code}
    rows = body.get("data", body.get("rows")) if isinstance(body, dict) else None
    try:
        row_count = int(rows[0][0])
    except (TypeError, IndexError, ValueError, KeyError):
        return {"ok": False, "message": "Engine returned no row count for the table", "code": None}
    elapsed = body.get("elapsed_ms")
    return {"ok": True, "row_count": row_count,
            "elapsed_ms": int(elapsed) if isinstance(elapsed, (int, float)) else None,
            "query_id": body.get("id")}


# ── Governance: resource groups and the audit ledger ──────────────────────────

def _admin_role(role, what):
    if role != "Admin":
        raise HTTPException(403, f"Administrator access is required for {what}")
    return "admin"


def _governance_response(response, fallback):
    """The Engine's answer for an admin governance call, its message kept:
    validation refusals are 422, its admin gate 403, the rest 502."""
    status = response.status_code
    if status == 400:
        raise HTTPException(422, _engine_message(response, fallback))
    if status == 403:
        raise HTTPException(403, _engine_message(response, "Engine refused the administrator credential"))
    if status == 404:
        raise HTTPException(404, _engine_message(response, "Not available on this Engine"))
    if status in {401, 503}:
        raise HTTPException(503, _engine_message(response, "Engine governance is unavailable"))
    if not response.is_success:
        raise HTTPException(502, "Engine rejected the request")
    try:
        return response.json()
    except ValueError:
        raise HTTPException(502, "Engine returned an invalid governance document") from None


def resource_groups(actor, role):
    """The resource groups in force, their source and each group's counters."""
    response = _send("GET", "/v1/admin/resource-groups", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                     role=_admin_role(role, "resource groups"))
    return _governance_response(response, "Engine could not read the resource groups")


def replace_resource_groups(document, actor, role):
    """Replace every group and selector at once; the Engine validates and
    applies it for the next admission, durably."""
    response = _send("PUT", "/v1/admin/resource-groups", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                     payload=document, role=_admin_role(role, "resource groups"))
    return _governance_response(response, "Engine refused the resource groups")


def quota(actor, role):
    """The caller's live-read quota on the coordinator (`GET /v1/quota`):
    `{"demo": {"enabled"}, "quota": null | {...}}`. A coordinator that does
    not know the route (before the demo posture) answers 404, which is
    reported as the posture being off."""
    response = _send("GET", "/v1/quota", "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role), timeout=10)
    if response.status_code == 404:
        return {"demo": {"enabled": False}, "quota": None}
    if response.status_code in {401, 403}:
        raise HTTPException(502, "Engine rejected the bridge credential")
    if not response.is_success:
        raise HTTPException(502, "Engine rejected the request")
    body = _response_json(response)
    if not isinstance(body, dict) or not isinstance(body.get("demo"), dict):
        raise HTTPException(502, "Engine returned an invalid quota document")
    return body


def _audit_path(params):
    allowed = ("since", "until", "principal", "kind", "query_id", "limit", "cursor", "format")
    query = "&".join(f"{key}={quote(str(params[key]), safe='')}" for key in allowed
                     if params.get(key) not in (None, ""))
    return "/v1/audit" + ("?" + query if query else "")


def audit(params, actor, role):
    """One page of the audit ledger, the Engine's filters passed through."""
    response = _send("GET", _audit_path({**params, "format": "json"}), "KAVEON_ENGINE_BRIDGE_TOKEN",
                     actor, role=_admin_role(role, "the audit ledger"))
    return _governance_response(response, "Engine refused the audit query")


def audit_export(params, actor, role):
    """Every matching ledger record as JSON lines, streamed from the Engine a
    chunk at a time so the export is never held whole."""
    token = os.getenv("KAVEON_ENGINE_BRIDGE_TOKEN")
    if not token:
        raise HTTPException(503, "Engine service credential is not configured")
    engine_role = _admin_role(role, "the audit ledger")
    headers = {"Authorization": f"Bearer {token}", "x-kaveon-actor": actor,
               "x-kaveon-principal": actor, "x-kaveon-role": engine_role}
    url = _endpoint() + _audit_path({**params, "format": "jsonl"})
    try:
        client = httpx.Client(timeout=httpx.Timeout(600, connect=10), verify=_verify_context())
        stream = client.stream("GET", url, headers=headers, follow_redirects=False)
        response = stream.__enter__()
    except httpx.HTTPError:
        raise HTTPException(502, "Engine is unavailable") from None
    if not response.is_success:
        body = response.read()
        stream.__exit__(None, None, None)
        client.close()
        message = "Engine refused the audit export"
        try:
            parsed = json.loads(body)
            if isinstance(parsed, dict) and isinstance(parsed.get("error"), str):
                message = parsed["error"]
        except ValueError:
            pass
        raise HTTPException(422 if response.status_code == 400 else 502, message)

    def lines():
        try:
            yield from response.iter_bytes()
        finally:
            stream.__exit__(None, None, None)
            client.close()

    return lines()
