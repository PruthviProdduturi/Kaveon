"""Archive the Engine's own catalog registry to object storage.

Two different things are called "the catalog", and only one of them was
durable.

The *product* catalog — datasets, charts, dashboards, sources, chat, history —
is committed to ADLS on every transaction: immutable documents written
create-only, then a conditional head update. `kaveondb_backup` copies that
prefix, so the part that was already durable also had backups.

The Engine's *registry* — which catalogs, schemas and tables exist, where each
table lives, and the statistics and cubes observed for it — is a SQLite
database on the coordinator's volume. Nothing archived it. Its only copy in
object storage was a file named `vm-catalog-20261009T233432Z.db`: the hand-made
transplant from the VM this installation replaced. Every table registered and
every statistic observed after that moment existed in exactly one place, on one
disk, in one zone. All of the data those registrations point at is in ADLS and
would have survived; nothing would have known it was there.

This archives the registry logically rather than by copying the file, for three
reasons. The coordinator holds that volume ReadWriteOnce, so no second pod can
read it while the Engine runs. A `.db` file is only restorable into a
compatible build, while a document of definitions, schemas and tables is
restorable by replaying the same API that produced it. And a file copy of a
live SQLite database is not a backup at all unless it goes through `.backup`,
because the write-ahead log is a separate file.

**Nothing is written unless something changed.** The document is addressed by
the digest of its own canonical bytes, so an unchanged registry resolves to an
object that already exists and the run writes nothing at all. A re-observation
of unchanged statistics is not a change: the observation timestamp is excluded
from the identity, while the source version it observed is not, so a genuinely
new table version does produce a new archive.

The archive deliberately lands in a **different container** from the live
catalog. Blob soft delete does not protect against container deletion, and an
archive sharing a container with its original is lost to the same accident.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import time
from urllib.parse import quote

from services import engine_bridge

ACTOR = "kaveon-catalog-archive"
PREFIX = "engine-catalog"
# The registry is a few megabytes of JSON at any realistic table count. A bound
# well above that still refuses to publish a response that is not a registry.
MAX_DOCUMENT_BYTES = 64 * 1024 * 1024
SCHEMA_VERSION = 1


def container() -> str:
    """Where archives go. A separate container from the live catalog, because
    container deletion is the one accident blob soft delete does not cover."""
    return os.getenv("KAVEON_CATALOG_ARCHIVE_CONTAINER", "backups").strip() or "backups"


def _canonical(value) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=False).encode("utf-8")


def _without_observation_times(value):
    """The document with observation timestamps removed.

    `observed_at_ms` moves every time statistics are re-observed, including when
    the observation found nothing new. Including it in the identity would make
    every run produce a fresh archive of an unchanged registry, which is both
    the opposite of the rule this module follows and a slow leak of objects
    into a container with no lifecycle policy. The source version that the
    observation *reports* stays in the identity, so a table that genuinely
    advanced is archived.
    """
    if isinstance(value, dict):
        return {key: _without_observation_times(item) for key, item in value.items()
                if key != "observed_at_ms"}
    if isinstance(value, list):
        return [_without_observation_times(item) for item in value]
    return value


def identity(document) -> str:
    """The digest that decides whether this registry has been archived before."""
    return hashlib.sha256(_canonical(_without_observation_times(document))).hexdigest()


def _require_list(value, what: str) -> list:
    if not isinstance(value, list):
        raise RuntimeError(f"Engine returned no usable {what}")
    return value


def export(actor: str = ACTOR) -> dict:
    """Read the whole registry through the Engine's catalog API.

    Depth-first and complete: a partial export is worse than none, because it
    restores a registry that silently lacks tables. Anything the Engine cannot
    answer raises rather than being recorded as absent.
    """
    definitions = _require_list(
        engine_bridge.catalog_read("/v1/catalog/definitions", actor), "catalog definitions")
    if not definitions:
        raise RuntimeError("Engine reports no catalog definitions; refusing to archive an empty registry")

    catalogs = []
    for definition in sorted(definitions, key=lambda item: str(item.get("id"))):
        catalog_id = definition.get("id")
        if not isinstance(catalog_id, str) or not catalog_id:
            raise RuntimeError("Engine returned a catalog definition with no id")
        quoted = quote(catalog_id, safe="")
        schemas = _require_list(
            engine_bridge.catalog_read(f"/v1/catalog/definitions/{quoted}/schemas", actor),
            f"schemas for {catalog_id}")
        exported_schemas = []
        for schema in sorted(schemas, key=lambda item: str(item.get("id"))):
            schema_id = schema.get("id")
            if not isinstance(schema_id, str) or not schema_id:
                raise RuntimeError(f"Engine returned a schema with no id in {catalog_id}")
            tables = _require_list(
                engine_bridge.catalog_read(
                    f"/v1/catalog/schemas/{quote(schema_id, safe='')}/tables", actor),
                f"tables for {schema_id}")
            exported_tables = []
            for table in sorted(tables, key=lambda item: str(item.get("id"))):
                table_id = table.get("id")
                if not isinstance(table_id, str) or not table_id:
                    raise RuntimeError(f"Engine returned a table with no id in {schema_id}")
                # Statistics are derivable — a restore can recompute them — so a
                # table whose statistics have never been observed is archived
                # without them rather than failing the run.
                statistics = engine_bridge.catalog_read(
                    f"/v1/catalog/tables/{quote(table_id, safe='')}/statistics", actor)
                exported_tables.append({"table": table, "statistics": statistics})
            exported_schemas.append({"schema": schema, "tables": exported_tables})
        catalogs.append({"definition": definition, "schemas": exported_schemas})

    document = {"schema_version": SCHEMA_VERSION, "catalogs": catalogs,
                "counts": _counts(catalogs)}
    size = len(_canonical(document))
    if size > MAX_DOCUMENT_BYTES:
        raise RuntimeError(f"Engine registry export is {size} bytes, beyond its bound")
    return document


def _counts(catalogs: list) -> dict:
    schemas = sum(len(entry["schemas"]) for entry in catalogs)
    tables = sum(len(schema["tables"]) for entry in catalogs for schema in entry["schemas"])
    observed = sum(1 for entry in catalogs for schema in entry["schemas"]
                   for table in schema["tables"] if table["statistics"])
    return {"catalogs": len(catalogs), "schemas": schemas, "tables": tables,
            "tables_with_statistics": observed}


def _run_id(now_ms: int) -> str:
    return time.strftime("%Y%m%dT%H%M%SZ", time.gmtime(now_ms / 1000))


def archive(client, *, actor: str = ACTOR, now_ms: int | None = None) -> dict:
    """Archive the registry if it has changed since the last archive.

    Returns what happened, so a caller can report an unchanged registry as a
    success rather than a silent no-op. `written` is false exactly when the
    registry already had an archive with this identity.
    """
    now_ms = int(time.time() * 1000) if now_ms is None else int(now_ms)
    document = export(actor)

    # The stored bytes are exactly the bytes the identity is computed over.
    #
    # They were not. The path was the identity — which excludes the statistics
    # observation timestamps, so that re-observing an unchanged registry is not
    # a change — while the payload was the document with those timestamps still
    # in it. Two runs of an unchanged registry therefore agreed on the path and
    # disagreed on the bytes, and the check below, which exists to catch an
    # object whose content does not match the digest naming it, fired on the
    # ordinary case and refused to archive anything at all.
    #
    # Storing the canonical form the identity is taken over makes the path a
    # true digest of its own content, which is what made the check worth having.
    # Nothing is lost: an observation time is not registry state, and the run
    # record carries when the archive was taken.
    payload = _canonical(_without_observation_times(document))
    document_identity = hashlib.sha256(payload).hexdigest()
    if re.fullmatch(r"[0-9a-f]{64}", document_identity) is None:
        raise RuntimeError("registry identity is not a SHA-256 digest")
    path = f"{PREFIX}/catalog/{document_identity}.json"

    existing = client.read(path, len(payload) + 1)
    if existing is not None:
        # Content-addressed, so an object already at this path is this registry.
        # Verified rather than assumed: a path that holds different bytes than
        # its own digest is corruption, and reporting it as "unchanged" would
        # hide it.
        if hashlib.sha256(existing).hexdigest() != document_identity:
            raise RuntimeError(f"{path} holds bytes that are not the registry it is named for")
        return {"written": False, "identity_sha256": document_identity, "path": path,
                "counts": document["counts"], "run_id": None}

    etag = client.create_if_absent_with_etag(path, payload)
    read_back = client.read(path, len(payload) + 1)
    if read_back != payload:
        raise RuntimeError(f"{path} read-back does not match the bytes published")

    # A run record only when a new registry was published, so an unchanged
    # registry leaves the container untouched. That the run happened at all is
    # the job's own business — its logs and its exit status say so — and is not
    # worth an object per schedule tick in a container with no expiry policy.
    run_id = _run_id(now_ms)
    run_path = f"{PREFIX}/runs/{run_id}.json"
    run = _canonical({"schema_version": SCHEMA_VERSION, "run_id": run_id,
                      "archived_at_ms": now_ms, "identity_sha256": document_identity,
                      "catalog_path": path, "etag": etag, "counts": document["counts"]})
    client.create_if_absent_with_etag(run_path, run)
    return {"written": True, "identity_sha256": document_identity, "path": path,
            "counts": document["counts"], "run_id": run_id}


def main() -> int:
    """Entry point for the scheduled archive.

    Prints one line of what happened and nothing else. A run that found the
    registry unchanged is a success and says so, because a job that only ever
    reported failure would make "nothing to do" indistinguishable from "never
    ran".
    """
    from services.adls_artifact_client import AzureArtifactClient

    account = os.getenv("KAVEON_ADLS_ACCOUNT", "").strip()
    if not account:
        print("KAVEON_ADLS_ACCOUNT is not set; refusing to archive nowhere")
        return 2
    client = AzureArtifactClient(account, container())
    result = archive(client)
    counts = result["counts"]
    state = "published" if result["written"] else "unchanged"
    print(f"registry {state}: {counts['catalogs']} catalogs, {counts['schemas']} schemas, "
          f"{counts['tables']} tables ({counts['tables_with_statistics']} with statistics), "
          f"identity {result['identity_sha256'][:12]}, path {result['path']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
