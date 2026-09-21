"""Read-only reconciliation of the seven PostgreSQL special-family tables.

This path exists for a live database that still contains the qualified rows.
It never fences or mutates PostgreSQL and accepts only the reviewed Sep14
baseline plus byte-identical readback of its immutable ADLS publication.
"""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
from datetime import datetime, timezone
from pathlib import Path

from database.pool import get_connection_pool
from services import postgresql_evidence_collector as collector
from services import postgresql_retirement_gate as gate
from services import postgresql_special_family_migration as migration


QUALIFIED_BASELINE_SHA256 = "0681380c8c4ca338eda021491cb5795f99809416d14481e01cef7572feb8231e"
CONTEXT_TABLES = tuple(gate.AUTHORITY_FAMILIES["context_cache"])
DLM_TABLES = tuple(gate.AUTHORITY_FAMILIES["dlm_generation"])
TABLES = migration.TABLES


def _canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=False, allow_nan=False).encode("utf-8")


def _capture(baseline: dict) -> dict:
    pool = get_connection_pool(os.getenv("METADATA_DATABASE", ""))
    if pool.db_type != "postgresql":
        raise RuntimeError("read-only special-family verification requires PostgreSQL")
    connection = pool.get_connection()
    try:
        connection.connect()
        raw = connection.connection
        if not raw.autocommit:
            raise RuntimeError("verification connection already has an open transaction")
        raw.autocommit = False
        cursor = raw.cursor()
        try:
            cursor.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            cursor.execute("SELECT txid_current_snapshot()")
            snapshot_id = str(cursor.fetchone()[0])
            cursor.execute("SELECT COALESCE(MAX(source_sequence),0) "
                           "FROM product_migration_outbox")
            watermark = int(cursor.fetchone()[0])
            identities = {}
            by_name = {table["name"]: table for table in baseline["tables"]}
            for name in TABLES:
                table = by_name[name]
                columns = [column["name"] for column in table["columns"]]
                cursor.execute("SELECT " + ",".join(f'\"{column}\"' for column in columns)
                               + f' FROM \"{name}\"')
                identities[name] = migration.raw_table_identity(table, cursor.fetchall())
            raw.commit()
            return {"snapshot_id": snapshot_id, "watermark": watermark,
                    "table_identities": identities}
        except Exception:
            raw.rollback()
            raise
        finally:
            cursor.close()
            raw.autocommit = True
    finally:
        pool.return_connection(connection)


def _readback(client, prefix: str, baseline: dict, evidence: dict) -> dict:
    prefix = prefix.strip("/")
    if (not prefix or any(part in {"", ".", ".."} for part in prefix.split("/"))):
        raise RuntimeError("special-family ADLS prefix is invalid")
    identity = migration.verify_evidence(evidence, baseline)
    table_by_name = {table["name"]: table for table in baseline["tables"]}
    for name, receipt in zip(TABLES, evidence["objects"]):
        body = migration._canonical(migration._table_object(identity, table_by_name[name]))
        observed = client.read(f"{prefix}/{receipt['path']}", len(body))
        if observed != body or hashlib.sha256(observed or b"").hexdigest() != receipt["sha256"]:
            raise RuntimeError(f"special-family ADLS table readback mismatch: {name}")
    manifest_value = {
        "schema_version": migration.SCHEMA_VERSION, "encoding": identity["encoding"],
        "baseline_evidence_id": identity["baseline_evidence_id"],
        "source_snapshot_id": identity["source_snapshot_id"],
        "global_content_sha256": identity["global_content_sha256"],
        "tables": [{"table": name, "path": evidence["objects"][index]["path"],
                    "sha256": evidence["objects"][index]["sha256"],
                    **identity["table_identities"][name]}
                   for index, name in enumerate(TABLES)],
    }
    manifest_body = migration._canonical(manifest_value)
    digest = hashlib.sha256(manifest_body).hexdigest()
    if digest != evidence["manifest"]["sha256"]:
        raise RuntimeError("special-family manifest digest mismatch")
    observed_manifest = client.read(f"{prefix}/manifests/{digest}.json", len(manifest_body))
    if observed_manifest != manifest_body:
        raise RuntimeError("special-family ADLS manifest readback mismatch")
    head_body = _canonical({"manifest_path": f"manifests/{digest}.json", "sha256": digest})
    observed_head = client.read_with_etag(f"{prefix}/head.json", len(head_body))
    if observed_head is None or observed_head[0] != head_body:
        raise RuntimeError("special-family ADLS head readback mismatch")
    return {"manifest_sha256": digest, "head_etag": observed_head[1]}


def _report(family: str, captured: dict, identity: dict, publication: dict,
            reconciled_at: str) -> dict:
    tables = tuple(gate.AUTHORITY_FAMILIES[family])
    count = sum(captured["table_identities"][table]["row_count"] for table in tables)
    report = {
        "schema_version": collector.REPORT_SCHEMA_VERSION, "family": family,
        "tables": list(tables), "status": "passed", "reconciled_at": reconciled_at,
        "source_watermark": captured["watermark"], "source_count": count,
        "target_count": count, "checks": {name: True for name in gate.REQUIRED_CHECKS},
        "provenance": {
            "producer": "special-family-readonly-verifier-v1",
            "source_snapshot": (f"postgresql:{captured['snapshot_id']}:"
                                f"baseline:{identity['baseline_evidence_id']}"),
            "target_snapshot": f"adls-manifest:{publication['manifest_sha256']}",
        },
    }
    report["report_sha256"] = hashlib.sha256(collector._canonical(report)).hexdigest()
    return report


def _write_new(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = _canonical(value) + b"\n"
    with path.open("xb") as handle:
        handle.write(encoded); handle.flush(); os.fsync(handle.fileno())


def run(*, baseline: dict, migration_evidence: dict, prefix: str, output_directory: Path,
        client, now: datetime | None = None, capture=None) -> dict:
    if os.getenv("KAVEON_SPECIAL_FAMILY_READONLY_ENABLED") != "true":
        raise RuntimeError("read-only special-family verification requires explicit enablement")
    identity = migration.verify_baseline(baseline)
    if identity["baseline_evidence_id"] != QUALIFIED_BASELINE_SHA256:
        raise RuntimeError("special-family baseline is not the qualified Sep14 baseline")
    migration.verify_evidence(migration_evidence, baseline)
    captured = (capture or _capture)(baseline)
    if captured.get("table_identities") != identity["table_identities"]:
        raise RuntimeError("live PostgreSQL special-family identity differs from qualified baseline")
    if type(captured.get("watermark")) is not int or captured["watermark"] < 0 \
            or not isinstance(captured.get("snapshot_id"), str) or not captured["snapshot_id"]:
        raise RuntimeError("live PostgreSQL special-family observation is invalid")
    publication = _readback(client, prefix, baseline, migration_evidence)
    instant = (now or datetime.now(timezone.utc)).astimezone(timezone.utc)
    reconciled_at = instant.isoformat().replace("+00:00", "Z")
    reports = {family: _report(family, captured, identity, publication, reconciled_at)
               for family in ("context_cache", "dlm_generation")}
    receipt = {
        "schema_version": 1, "status": "passed", "verified_at": reconciled_at,
        "mode": "read-only", "baseline_evidence_id": identity["baseline_evidence_id"],
        "source_snapshot_id": captured["snapshot_id"],
        "source_watermark": captured["watermark"],
        "table_identities": captured["table_identities"],
        "target_manifest_sha256": publication["manifest_sha256"],
        "target_head_etag": publication["head_etag"],
        "writes_fenced": False, "rows_deleted": 0,
    }
    receipt["receipt_sha256"] = hashlib.sha256(_canonical(receipt)).hexdigest()
    destination = output_directory.resolve()
    if destination.exists():
        raise RuntimeError("refusing to overwrite read-only special-family output")
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = Path(tempfile.mkdtemp(prefix=destination.name + ".", dir=destination.parent))
    try:
        _write_new(temporary / "context_cache.json", reports["context_cache"])
        _write_new(temporary / "dlm_generation.json", reports["dlm_generation"])
        _write_new(temporary / "read-only-special-verification.json", receipt)
        os.replace(temporary, destination)
    except Exception:
        import shutil
        shutil.rmtree(temporary, ignore_errors=True)
        raise
    return {"passed": True, "mode": "read-only", "output_directory": str(destination),
            "baseline_evidence_id": identity["baseline_evidence_id"],
            "source_count": sum(v["row_count"] for v in identity["table_identities"].values())}
