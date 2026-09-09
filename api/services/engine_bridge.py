"""Opt-in Engine control/data plane client. No secrets enter catalog definitions."""
import json
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


def _request(method, path, token_name, actor, *, payload=None, revision=None, role=None):
    token = os.getenv(token_name)
    if not token:
        raise HTTPException(503, "Engine service credential is not configured")
    headers = {"Authorization": f"Bearer {token}", "x-kaveon-actor": actor}
    if role:
        headers.update({"x-kaveon-principal": actor, "x-kaveon-role": role})
    if revision is not None:
        headers["If-Match"] = str(revision)
    try:
        response = httpx.request(method, _endpoint() + path, headers=headers, json=payload,
                                 timeout=60, follow_redirects=False, verify=_verify_context())
    except httpx.HTTPError:
        raise HTTPException(502, "Engine is unavailable") from None
    if response.status_code == 404:
        return None
    if response.status_code in {409, 412, 428}:
        raise HTTPException(409, "Engine revision conflict; reload before retrying")
    if not response.is_success:
        raise HTTPException(502, "Engine rejected the request")
    return response.json()


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


def execute(sql, catalog, actor, role, schema=None):
    roles = {"Analyst": "analyst", "Admin": "admin"}
    if role not in roles:
        raise HTTPException(403, "Analyst role required for Engine SQL")
    result = _request("POST", "/v1/statement", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                      payload={"query": sql, "catalog": catalog, "schema": schema,
                               "source": "studio", "client": "kaveon-api"}, role=roles[role])
    if result is None or result.get("error"):
        raise HTTPException(422, "Engine query failed")
    return result


def _read_role(role):
    roles = {"Viewer": "reader", "Analyst": "analyst", "Editor": "analyst", "Admin": "admin"}
    try:
        return roles[role]
    except KeyError:
        raise HTTPException(403, "A recognized Kaveon role is required for Engine catalog access") from None


def catalogs(actor, role):
    return _request("GET", "/v1/catalog", "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_read_role(role))


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
    """Read one table definition through catalog metadata, never through SQL."""
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
    columns = definition.get("columns")
    if not isinstance(columns, list):
        raise HTTPException(502, "Engine table definition is invalid")
    return columns
