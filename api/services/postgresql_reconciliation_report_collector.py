"""Build strict retirement-family reports from durable migration evidence.

This collector is deliberately read-only.  It verifies completed backfill
checkpoints, compares their records with one pinned KaveonDB snapshot, and
publishes reports only after every authority family has evidence.  Rebuild and
retire families keep their own evidence producers; this module validates and
copies those reports rather than inferring success from unrelated checkpoints.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import quote

from services import (
    activity_backfill_operation,
    chart_backfill_operation,
    chat_history_backfill_operation,
    dashboard_backfill_operation,
    dlm_definition_backfill_operation,
    dlm_run_backfill_operation,
    favorite_backfill_operation,
    postgresql_evidence_collector as evidence_collector,
    postgresql_retirement_gate as gate,
    product_backfill_operation,
    query_history_backfill_operation,
    saved_query_backfill_operation,
    source_backfill_operation,
    user_recent_backfill_operation,
    user_theme_backfill_operation,
)
from services import engine_bridge


SCHEMA_VERSION = 1
MAX_MANIFEST_BYTES = 64 * 1024
MAX_TARGET_RECORDS = 300_000
ADMIN_ACTOR = "kaveon-retirement-reconciler"
HEX = frozenset("0123456789abcdef")

CHECKPOINTS = {
    "datasets": (product_backfill_operation.load_checkpoint, "datasets"),
    "charts": (chart_backfill_operation.load, "charts"),
    "dashboards": (dashboard_backfill_operation.load, "dashboards"),
    "favorites": (favorite_backfill_operation.load, "favorites"),
    "saved_queries": (saved_query_backfill_operation.load, "saved_queries"),
    "user_themes": (user_theme_backfill_operation.load, "user_themes"),
    "user_recents": (user_recent_backfill_operation.load, "user_recents"),
    "query_history": (query_history_backfill_operation.load, "query_history"),
    "activity": (activity_backfill_operation.load, "activity"),
    "sources": (source_backfill_operation.load, "sources"),
    "chat_history": (chat_history_backfill_operation.load, "chat_history"),
    "dlm_definitions": (dlm_definition_backfill_operation.load, "dlm_definitions"),
    "dlm_runs": (dlm_run_backfill_operation.load, "dlm_runs"),
}

DIRECT_KINDS = {
    "datasets": ("dataset",), "charts": ("chart",), "dashboards": ("dashboard",),
    "favorites": ("favorite",), "saved_queries": ("saved_query",),
    "user_themes": ("user_theme",), "user_recents": ("user_recent",),
    "query_history": ("query_history",), "activity": ("activity",),
    "chat_history": ("chat_session", "chat_message"),
}


def _canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=False).encode("utf-8")


def _load_json(path: Path, label: str, max_bytes: int = MAX_MANIFEST_BYTES) -> dict:
    if not path.is_file() or path.stat().st_size > max_bytes:
        raise RuntimeError(f"missing or oversized {label}")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise RuntimeError(f"invalid {label}") from error
    if not isinstance(value, dict):
        raise RuntimeError(f"invalid {label}")
    return value


def _completed_checkpoint(name: str, path: Path):
    loader, _ = CHECKPOINTS[name]
    snapshot, position, complete = loader(path)
    records = tuple(snapshot.records)
    if complete is not True or position != len(records):
        raise RuntimeError(f"{name} checkpoint is incomplete")
    return snapshot


def _target_records(kind: str) -> tuple[list[dict], str]:
    records, cursor, snapshot_id = [], None, None
    while True:
        query = "?limit=100" + (("&cursor=" + quote(cursor, safe="")) if cursor else "")
        page = engine_bridge._request("GET", f"/v1/products/{kind}{query}",
                                      "KAVEON_ENGINE_BRIDGE_TOKEN", ADMIN_ACTOR, role="admin")
        if not isinstance(page, dict) or not isinstance(page.get("records"), list):
            raise RuntimeError(f"KaveonDB returned an invalid {kind} product page")
        current = page.get("snapshot_id")
        if not isinstance(current, str) or not current or (snapshot_id and current != snapshot_id):
            raise RuntimeError(f"KaveonDB {kind} snapshot changed during reconciliation")
        snapshot_id = current
        for record in page["records"]:
            if (not isinstance(record, dict) or not isinstance(record.get("id"), str)
                    or not isinstance(record.get("document"), dict)):
                raise RuntimeError(f"KaveonDB returned an invalid {kind} record")
            records.append(record)
            if len(records) > MAX_TARGET_RECORDS:
                raise RuntimeError(f"KaveonDB {kind} records exceed the reconciliation bound")
        cursor = page.get("next_cursor")
        if cursor is None:
            return records, snapshot_id
        if not isinstance(cursor, str) or not cursor:
            raise RuntimeError(f"KaveonDB returned an invalid {kind} cursor")


def _exact_records(expected, kinds: tuple[str, ...], *, predicate=None) -> tuple[int, str]:
    wanted = {(getattr(record, "kind", kinds[0]), record.record_id): record for record in expected}
    if len(wanted) != len(expected):
        raise RuntimeError("checkpoint contains duplicate product identities")
    found, snapshots = {}, set()
    for kind in kinds:
        records, snapshot = _target_records(kind)
        snapshots.add(snapshot)
        for record in records:
            if predicate is not None and not predicate(record):
                continue
            identity = (kind, record["id"])
            if identity in found:
                raise RuntimeError(f"KaveonDB contains duplicate {kind} identities")
            found[identity] = record
    if set(found) != set(wanted):
        missing = len(set(wanted) - set(found)); extra = len(set(found) - set(wanted))
        raise RuntimeError(f"KaveonDB target identity mismatch: {missing} missing, {extra} extra")
    for identity, source in wanted.items():
        target = found[identity]
        if target["document"] != source.document:
            raise RuntimeError(f"KaveonDB target content mismatch: {identity[1]}")
    if len(snapshots) != 1:
        raise RuntimeError("KaveonDB target families were not read from one snapshot")
    return len(found), snapshots.pop()


def _report(family: str, *, watermark: int, source_count: int, target_count: int,
            source_snapshot: str, target_snapshot: str, reconciled_at: str,
            producer: str = "checkpoint-live-reconciler-v1") -> dict:
    report = {
        "schema_version": evidence_collector.REPORT_SCHEMA_VERSION,
        "family": family, "tables": list(gate.AUTHORITY_FAMILIES[family]),
        "status": "passed", "reconciled_at": reconciled_at,
        "source_watermark": watermark, "source_count": source_count,
        "target_count": target_count,
        "checks": {name: True for name in gate.REQUIRED_CHECKS},
        "provenance": {"producer": producer, "source_snapshot": source_snapshot,
                       "target_snapshot": target_snapshot},
    }
    report["report_sha256"] = hashlib.sha256(evidence_collector._canonical(report)).hexdigest()
    return report


def _verify_inventory(path: Path) -> dict:
    inventory = _load_json(path, "PostgreSQL live inventory", 1024 * 1024)
    claimed = inventory.get("report_sha256")
    unsigned = {key: value for key, value in inventory.items() if key != "report_sha256"}
    if (not isinstance(claimed, str) or len(claimed) != 64 or set(claimed) - HEX
            or hashlib.sha256(_canonical(unsigned)).hexdigest() != claimed):
        raise RuntimeError("PostgreSQL live inventory digest mismatch")
    required = {"schema_version", "captured_at", "source_snapshot", "discovered_tables",
                "authority_tables", "infrastructure_tables", "unclassified_tables"}
    if set(unsigned) != required or inventory.get("schema_version") != 1:
        raise RuntimeError("PostgreSQL live inventory schema is invalid")
    rows = {entry.get("table"): entry for entry in inventory["authority_tables"]
            if isinstance(entry, dict)}
    for table in gate.AUTHORITY_FAMILIES["ai_configuration"]:
        entry = rows.get(table)
        if (not isinstance(entry, dict) or entry.get("family") != "ai_configuration"
                or type(entry.get("present")) is not bool or type(entry.get("row_count")) is not int):
            raise RuntimeError("AI configuration inventory coverage is incomplete")
        if entry["row_count"] != 0:
            raise RuntimeError("AI configuration still contains PostgreSQL authority rows")
    return inventory


def _special_report(path: Path, family: str) -> dict:
    # _load_report validates exact schema, family identity and digest.
    normalized = evidence_collector._load_report(path, family)
    return {"schema_version": evidence_collector.REPORT_SCHEMA_VERSION, **normalized}


def collect(manifest_path: Path, output_directory: Path, *, now: datetime | None = None) -> dict:
    if os.getenv("KAVEON_RECONCILIATION_REPORT_COLLECTION_ENABLED") != "true":
        raise RuntimeError("report collection requires explicit enablement")
    manifest = _load_json(manifest_path, "reconciliation manifest")
    if set(manifest) != {"schema_version", "checkpoints", "live_inventory", "special_reports"} \
            or manifest.get("schema_version") != SCHEMA_VERSION:
        raise RuntimeError("reconciliation manifest schema is invalid")
    checkpoints = manifest["checkpoints"]
    if not isinstance(checkpoints, dict) or set(checkpoints) != set(CHECKPOINTS):
        raise RuntimeError("reconciliation checkpoint coverage is incomplete")
    base = manifest_path.resolve().parent
    snapshots = {name: _completed_checkpoint(name, (base / value).resolve())
                 for name, value in checkpoints.items() if isinstance(value, str) and value}
    if set(snapshots) != set(CHECKPOINTS):
        raise RuntimeError("reconciliation checkpoint paths are invalid")

    instant = (now or datetime.now(timezone.utc)).astimezone(timezone.utc)
    reconciled_at = instant.isoformat().replace("+00:00", "Z")
    reports = {}
    for family, kinds in DIRECT_KINDS.items():
        snapshot = snapshots[family]
        count, target_snapshot = _exact_records(snapshot.records, kinds)
        reports[family] = _report(
            family, watermark=snapshot.source_watermark, source_count=len(snapshot.records),
            target_count=count, source_snapshot=f"checkpoint:{snapshot.snapshot_sha256}",
            target_snapshot=f"kaveondb:{target_snapshot}", reconciled_at=reconciled_at)

    source_snapshot = snapshots["sources"]
    source_groups = {
        "catalog_sources": tuple(r for r in source_snapshot.records if r.document.get("source_kind") == "catalog"),
        "data_sources": tuple(r for r in source_snapshot.records if r.document.get("source_kind") == "data"),
    }
    if sum(map(len, source_groups.values())) != len(source_snapshot.records):
        raise RuntimeError("source checkpoint contains an unknown source kind")
    for family, records in source_groups.items():
        count, target_snapshot = _exact_records(
            records, ("source",), predicate=lambda target, f=family:
            target["document"].get("source_kind") == ("catalog" if f == "catalog_sources" else "data"))
        reports[family] = _report(
            family, watermark=source_snapshot.source_watermark, source_count=len(records),
            target_count=count, source_snapshot=f"checkpoint:{source_snapshot.snapshot_sha256}:{family}",
            target_snapshot=f"kaveondb:{target_snapshot}", reconciled_at=reconciled_at)

    datasets = snapshots["datasets"]
    semantics = sum(len(record.document.get(key, [])) for record in datasets.records
                    for key in ("dimensions", "columns", "metrics"))
    reports["dataset_semantics"] = _report(
        "dataset_semantics", watermark=datasets.source_watermark, source_count=semantics,
        target_count=semantics, source_snapshot=f"embedded:{datasets.snapshot_sha256}",
        target_snapshot=reports["datasets"]["provenance"]["target_snapshot"],
        reconciled_at=reconciled_at, producer="embedded-dataset-semantics-reconciler-v1")

    if not isinstance(manifest["live_inventory"], str) or not manifest["live_inventory"]:
        raise RuntimeError("PostgreSQL live inventory path is invalid")
    inventory = _verify_inventory((base / manifest["live_inventory"]).resolve())
    reports["ai_configuration"] = _report(
        "ai_configuration", watermark=0, source_count=0, target_count=0,
        source_snapshot=f"postgresql:{inventory['source_snapshot']}:{inventory['report_sha256']}",
        target_snapshot="kaveondb:not-applicable:no-ai-authority",
        reconciled_at=reconciled_at, producer="live-inventory-zero-authority-v1")

    special = manifest["special_reports"]
    if (not isinstance(special, dict) or set(special) != {"context_cache", "dlm_generation"}
            or any(not isinstance(value, str) or not value for value in special.values())):
        raise RuntimeError("special reconciliation report coverage is incomplete")
    reports["context_cache"] = _special_report((base / special["context_cache"]).resolve(), "context_cache")

    # DLM retirement evidence is accepted only after both definition and run
    # checkpoints independently reconcile against live KaveonDB state.
    for name, kind in (("dlm_definitions", "dlm_definition"), ("dlm_runs", "dlm_run")):
        _exact_records(snapshots[name].records, (kind,))
    reports["dlm_generation"] = _special_report(
        (base / special["dlm_generation"]).resolve(), "dlm_generation")

    if set(reports) != set(gate.AUTHORITY_FAMILIES):
        raise RuntimeError("collector did not produce the exact retirement family set")
    destination = output_directory.resolve()
    if destination.exists():
        raise RuntimeError("output directory already exists")
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = Path(tempfile.mkdtemp(prefix=destination.name + ".", dir=destination.parent))
    try:
        for family, report in reports.items():
            (temporary / f"{family}.json").write_bytes(_canonical(report))
        os.replace(temporary, destination)
    except Exception:
        shutil.rmtree(temporary, ignore_errors=True)
        raise
    return {"status": "passed", "family_count": len(reports),
            "output_directory": str(destination)}
