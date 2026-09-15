"""Transactional post-fence retirement for PostgreSQL generated-state tables."""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
from datetime import datetime, timezone
from pathlib import Path

from database.pool import get_connection_pool
from services import (context_cache_retirement, dlm_generation_retirement,
                      dlm_migration_evidence, postgresql_retirement_gate,
                      postgresql_special_family_migration as lossless)
from services.postgresql_write_fence import enabled as fence_enabled

CONTEXT_TABLES = ("context_answer_cache", "context_snapshots")
DLM_TABLES = tuple(dlm_generation_retirement.TABLES)
DELETE_ORDER = ("dlm_answers", "dlm_value_index", "dlm_router", "dlm_sketch",
                "dlm_artifact", *CONTEXT_TABLES)
OUTBOX_WATERMARK_SQL = (
    "SELECT COALESCE(MAX(source_sequence),0) FROM product_migration_outbox"
)
OUTBOX_PENDING_SQL = "SELECT COUNT(*) FROM product_migration_outbox WHERE applied_at IS NULL"


def _canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=False).encode("utf-8")


def _schema_digest(cursor, table: str) -> str:
    cursor.execute(
        "SELECT column_name,data_type,is_nullable,ordinal_position "
        "FROM information_schema.columns WHERE table_schema=current_schema() "
        "AND table_name=%s ORDER BY ordinal_position", (table,))
    rows = [list(row) for row in cursor.fetchall()]
    if not rows:
        raise RuntimeError(f"retirement table is missing: {table}")
    return hashlib.sha256(_canonical(rows)).hexdigest()


def _counts(cursor, tables) -> dict[str, int]:
    result = {}
    for table in tables:
        cursor.execute(f'SELECT COUNT(*) FROM "{table}"')
        result[table] = int(cursor.fetchone()[0])
    return result


def _validate_fence_observation(value: dict) -> None:
    expected = set(postgresql_retirement_gate.AUTHORITY_FAMILIES)
    probes = value.get("family_probes") if isinstance(value, dict) else None
    found = {item.get("family") for item in probes or [] if isinstance(item, dict)}
    if (set(value or {}) != {"deployment_revision", "readonly_probe_passed", "family_probes"}
            or not value.get("deployment_revision") or value.get("readonly_probe_passed") is not True
            or found != expected or len(probes) != len(expected)
            or any(item.get("passed") is not True or set(item) != {"family", "passed"}
                   for item in probes)):
        raise RuntimeError("complete live write-fence observation is required")


def _validate_rebuild(value: dict) -> None:
    keys = {"target_snapshot_id", "dataset_revision_sha256", "active_datasets",
            "covered_datasets", "failed_datasets", "first_probe_sha256",
            "repeat_probe_sha256"}
    hashes = ("dataset_revision_sha256", "first_probe_sha256", "repeat_probe_sha256")
    if (not isinstance(value, dict) or set(value) != keys
            or not isinstance(value["target_snapshot_id"], str) or not value["target_snapshot_id"]
            or any(not isinstance(value[key], str) or len(value[key]) != 64
                   or set(value[key]) - set("0123456789abcdef") for key in hashes)
            or any(type(value[key]) is not int or value[key] < 0
                   for key in ("active_datasets", "covered_datasets", "failed_datasets"))
            or value["active_datasets"] != value["covered_datasets"]
            or value["failed_datasets"] != 0
            or value["first_probe_sha256"] != value["repeat_probe_sha256"]):
        raise RuntimeError("complete deterministic context-cache rebuild observation is required")


def _content_identities(cursor, baseline: dict) -> dict:
    result = {}
    tables = {table["name"]: table for table in baseline["tables"]}
    for table in (*CONTEXT_TABLES, *DLM_TABLES):
        item = tables[table]
        columns = [column["name"] for column in item["columns"]]
        cursor.execute("SELECT " + ",".join(f'\"{column}\"' for column in columns)
                       + f' FROM "{table}"')
        result[table] = lossless.raw_table_identity(item, cursor.fetchall())
    return result


def _delete_transactionally(expected_counts: dict[str, int], baseline: dict) -> dict:
    database = os.getenv("METADATA_DATABASE", "")
    pool = get_connection_pool(database)
    if pool.db_type != "postgresql":
        raise RuntimeError("special-family retirement requires PostgreSQL")
    connection = pool.get_connection()
    discard = False
    try:
        connection.connect()
        raw = connection.connection
        if not raw.autocommit:
            raise RuntimeError("retirement connection already has an open transaction")
        raw.autocommit = False
        cursor = raw.cursor()
        try:
            cursor.execute("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
            cursor.execute("LOCK TABLE " + ",".join(f'\"{t}\"' for t in DELETE_ORDER)
                           + " IN ACCESS EXCLUSIVE MODE")
            cursor.execute("SELECT txid_current_snapshot()")
            snapshot_id = str(cursor.fetchone()[0])
            cursor.execute(OUTBOX_WATERMARK_SQL)
            watermark = int(cursor.fetchone()[0])
            cursor.execute(OUTBOX_PENDING_SQL)
            outbox_pending = int(cursor.fetchone()[0])
            if outbox_pending != 0:
                raise RuntimeError("special-family deletion requires a drained outbox")
            schemas = {table: _schema_digest(cursor, table)
                       for table in (*CONTEXT_TABLES, *DLM_TABLES)}
            before = _counts(cursor, (*CONTEXT_TABLES, *DLM_TABLES))
            if before != expected_counts:
                raise RuntimeError("special-family source counts changed before deletion")
            identities = _content_identities(cursor, baseline)
            expected_identities = lossless.verify_baseline(baseline)["table_identities"]
            if identities != expected_identities:
                raise RuntimeError("special-family source content changed before deletion")
            deleted = {}
            for table in DELETE_ORDER:
                cursor.execute(f'DELETE FROM "{table}"')
                deleted[table] = int(cursor.rowcount)
            remaining = _counts(cursor, (*CONTEXT_TABLES, *DLM_TABLES))
            if deleted != {table: before[table] for table in DELETE_ORDER} or any(remaining.values()):
                raise RuntimeError("special-family transactional deletion did not reconcile")
            raw.commit()
        except Exception:
            raw.rollback()
            raise
        finally:
            cursor.close()
            raw.autocommit = True
        # A new transaction proves committed visibility rather than only the
        # deleting transaction's private view.
        verify = raw.cursor()
        try:
            committed_remaining = _counts(verify, (*CONTEXT_TABLES, *DLM_TABLES))
        finally:
            verify.close()
        if any(committed_remaining.values()):
            raise RuntimeError("special-family rows remain after commit")
        return {"snapshot_id": snapshot_id, "watermark": watermark,
                "outbox_pending": outbox_pending,
                "schemas": schemas, "identities": identities,
                "before": before, "deleted": deleted,
                "remaining": committed_remaining}
    finally:
        pool.return_connection(connection)


def _write_new(path: Path, value: dict) -> None:
    path = path.resolve()
    if path.exists():
        raise RuntimeError(f"refusing to overwrite retirement evidence: {path.name}")
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile("wb", dir=path.parent,
                                         prefix=path.name + ".", delete=False) as handle:
            temporary = Path(handle.name)
            handle.write(_canonical(value) + b"\n")
            handle.flush(); os.fsync(handle.fileno())
        os.replace(temporary, path); temporary = None
    finally:
        if temporary and temporary.exists(): temporary.unlink()


def run(*, bundle: dict, rebuild: dict, fence_observation: dict, output_directory: Path,
        expected_counts: dict[str, int], baseline: dict, migration_evidence: dict,
        now: datetime | None = None,
        delete_runner=None, max_age_hours: int = 1) -> dict:
    if os.getenv("KAVEON_SPECIAL_FAMILY_RETIREMENT_ENABLED") != "true":
        raise RuntimeError("special-family retirement requires explicit enablement")
    if not fence_enabled():
        raise RuntimeError("PostgreSQL write fence is not enabled")
    _validate_fence_observation(fence_observation)
    _validate_rebuild(rebuild)
    expected_tables = set((*CONTEXT_TABLES, *DLM_TABLES))
    if (not isinstance(expected_counts, dict) or set(expected_counts) != expected_tables
            or any(type(value) is not int or value < 0 for value in expected_counts.values())):
        raise RuntimeError("exact special-family source counts are required")
    output_directory = output_directory.resolve()
    if output_directory.exists():
        raise RuntimeError("refusing to overwrite special-family retirement output")
    instant = (now or datetime.now(timezone.utc)).astimezone(timezone.utc)
    verified_bundle = dlm_migration_evidence.verify(bundle, now=instant,
                                                     max_age_hours=max_age_hours)
    baseline_identity = lossless.verify_evidence(migration_evidence, baseline)
    if rebuild["target_snapshot_id"] != verified_bundle["target_snapshot_id"]:
        raise RuntimeError("context-cache and DLM target snapshots do not match")
    captured = (delete_runner or _delete_transactionally)(expected_counts, baseline)
    if captured["before"] != expected_counts:
        raise RuntimeError("special-family source counts changed before deletion")
    if captured.get("identities") != baseline_identity["table_identities"]:
        raise RuntimeError("special-family deletion precheck does not match baseline")
    if captured.get("outbox_pending") != 0:
        raise RuntimeError("special-family deletion precheck requires a drained outbox")
    verified_at = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
    observed_at = instant.isoformat().replace("+00:00", "Z")
    context_observation = {
        "schema_version": 1, "observed_at": observed_at,
        "source": {"snapshot_id": captured["snapshot_id"], "watermark": captured["watermark"],
                   "snapshot_rows": captured["before"]["context_snapshots"],
                   "cache_rows": captured["before"]["context_answer_cache"],
                   "snapshot_schema_sha256": captured["schemas"]["context_snapshots"],
                   "cache_schema_sha256": captured["schemas"]["context_answer_cache"]},
        "rebuild": rebuild,
        "deletion": {"writes_fenced": True,
                     "deleted_snapshot_rows": captured["deleted"]["context_snapshots"],
                     "deleted_cache_rows": captured["deleted"]["context_answer_cache"],
                     "remaining_snapshot_rows": captured["remaining"]["context_snapshots"],
                     "remaining_cache_rows": captured["remaining"]["context_answer_cache"],
                     "verified_at": verified_at},
    }
    dlm_observation = {
        "schema_version": 1, "observed_at": observed_at,
        "source": {"snapshot_id": captured["snapshot_id"], "watermark": captured["watermark"],
                   "rows": {t: captured["before"][t] for t in DLM_TABLES},
                   "schema_sha256": {t: captured["schemas"][t] for t in DLM_TABLES}},
        "deletion": {"writes_fenced": True,
                     "deleted_rows": {t: captured["deleted"][t] for t in DLM_TABLES},
                     "remaining_rows": {t: captured["remaining"][t] for t in DLM_TABLES},
                     "verified_at": verified_at,
                     "target_snapshot_id": verified_bundle["target_snapshot_id"]},
    }
    keys = ("KAVEON_CONTEXT_CACHE_RETIREMENT_ENABLED",
            "KAVEON_DLM_GENERATION_RETIREMENT_ENABLED")
    previous = {key: os.environ.get(key) for key in keys}
    try:
        for key in keys: os.environ[key] = "true"
        context_report = context_cache_retirement.build_report(
            context_observation, now=datetime.now(timezone.utc), max_age_hours=max_age_hours)
        dlm_report = dlm_generation_retirement.build_report(
            bundle, dlm_observation, now=datetime.now(timezone.utc), max_age_hours=max_age_hours)
    finally:
        for key, value in previous.items():
            if value is None: os.environ.pop(key, None)
            else: os.environ[key] = value
    for name, value in (("context-cache-live.json", context_observation),
                        ("dlm-generation-live.json", dlm_observation),
                        ("context_cache.json", context_report),
                        ("dlm_generation.json", dlm_report),
                        ("special-family-lossless.json", {
                            "baseline_evidence_id": baseline_identity["baseline_evidence_id"],
                            "baseline_sha256": baseline_identity["baseline_evidence_id"],
                            "observed_sha256": baseline_identity["baseline_evidence_id"],
                            "table_count": len(lossless.TABLES),
                            "writes_fenced": True,
                            "outbox_pending": captured["outbox_pending"],
                        })):
        _write_new(output_directory / name, value)
    return {"passed": True, "source_snapshot": captured["snapshot_id"],
            "watermark": captured["watermark"], "output_directory": str(output_directory),
            "reports": ["context_cache", "dlm_generation"]}
