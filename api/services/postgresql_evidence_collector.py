"""Collect verified local reconciliation reports into retirement-gate input."""

import hashlib
import json
import os
from pathlib import Path

from services import postgresql_retirement_gate as gate


REPORT_SCHEMA_VERSION = 1
MAX_REPORT_BYTES = 1024 * 1024
REPORT_KEYS = gate.FAMILY_KEYS | {"schema_version"}


def _canonical(value: dict) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def _load_report(path: Path, family: str) -> dict:
    if not path.is_file() or path.stat().st_size > MAX_REPORT_BYTES:
        raise RuntimeError(f"missing or oversized reconciliation report for {family}")
    try:
        report = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise RuntimeError(f"invalid reconciliation report for {family}") from error
    if not isinstance(report, dict) or set(report) != REPORT_KEYS:
        raise RuntimeError(f"unexpected reconciliation report schema for {family}")
    if report.get("schema_version") != REPORT_SCHEMA_VERSION or report.get("family") != family:
        raise RuntimeError(f"reconciliation report identity mismatch for {family}")
    claimed = report.get("report_sha256")
    unsigned = {key: value for key, value in report.items() if key != "report_sha256"}
    actual = hashlib.sha256(_canonical(unsigned)).hexdigest()
    if claimed != actual:
        raise RuntimeError(f"reconciliation report digest mismatch for {family}")
    return {key: value for key, value in report.items() if key != "schema_version"}


def _load_gates(path: Path) -> dict:
    if not path.is_file() or path.stat().st_size > MAX_REPORT_BYTES:
        raise RuntimeError("missing or oversized retirement-gates.json")
    try:
        gates = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise RuntimeError("invalid retirement-gates.json") from error
    if not isinstance(gates, dict):
        raise RuntimeError("retirement-gates.json must contain an object")
    return gates


def collect(report_directory: Path) -> dict:
    """Read one integrity-bound report per maintained authority family."""
    if os.getenv("KAVEON_RETIREMENT_EVIDENCE_COLLECTION_ENABLED") != "true":
        raise RuntimeError("collection requires KAVEON_RETIREMENT_EVIDENCE_COLLECTION_ENABLED=true")
    reports = [
        _load_report(report_directory / f"{family}.json", family)
        for family in sorted(gate.AUTHORITY_FAMILIES)
    ]
    evidence = {
        "schema_version": gate.SCHEMA_VERSION,
        "families": reports,
        "gates": _load_gates(report_directory / "retirement-gates.json"),
    }
    if len(_canonical(evidence)) > gate.MAX_EVIDENCE_BYTES:
        raise RuntimeError("collected retirement evidence exceeds its byte bound")
    return evidence
