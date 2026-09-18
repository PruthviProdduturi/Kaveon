"""Opt-in Engine control/data plane client. No secrets enter catalog definitions."""
import json
import re
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


def _request(method, path, token_name, actor, *, payload=None, revision=None, role=None, timeout=60):
    response = _send(method, path, token_name, actor, payload=payload, revision=revision, role=role, timeout=timeout)
    if response.status_code == 404:
        return None
    # Admission exhaustion is temporary. Preserve it for the Studio's bounded
    # retry path instead of turning a healthy Engine into an opaque 502.
    if response.status_code == 429:
        raise HTTPException(429, "Engine query capacity is temporarily exhausted", headers={"Retry-After": "1"})
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
    roles = {"Analyst": "analyst", "Editor": "analyst", "Admin": "admin"}
    if role not in roles:
        raise HTTPException(403, "A recognized Kaveon role is required for Engine SQL")
    tag = "kaveon-api:" + uuid.uuid4().hex
    payload = {"query": sql, "catalog": catalog, "schema": schema,
               "source": "studio", "client": "kaveon-api", "client_tags": [tag]}
    if settings is not None:
        payload["settings"] = dict(settings)
    try:
        result = _request("POST", "/v1/statement", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                          payload=payload, role=roles[role], timeout=timeout)
    except HTTPException as error:
        if error.status_code == 504:
            cancel_tagged(tag, actor, roles[role])
        raise
    if result is None:
        raise HTTPException(422, "Engine query failed")
    query_id = result.get("id")
    details = None
    if query_id:
        try:
            details = _request(
                "GET", "/v1/query/" + quote(str(query_id), safe=""),
                "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=roles[role],
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
        raise HTTPException(429, "Engine query capacity is temporarily exhausted", headers={"Retry-After": "1"})
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
