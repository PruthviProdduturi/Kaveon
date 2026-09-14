"""Live, content-free shadow-parity and PostgreSQL write-fence probes."""

import hashlib
import json
import os
from datetime import datetime,timezone
from pathlib import Path

import database.metadata as db
from services import postgresql_evidence_collector as collector
from services import postgresql_retirement_gate as gate
from services import postgresql_write_fence as fence


def _canonical(value):
    return json.dumps(value,sort_keys=True,separators=(",",":")).encode()


def shadow_parity(report_directory: Path, *, now=None, max_age_hours=24) -> dict:
    if not report_directory.is_dir():
        raise RuntimeError("shadow parity report directory is missing")
    expected={f"{family}.json" for family in gate.AUTHORITY_FAMILIES}
    actual={path.name for path in report_directory.iterdir() if path.is_file()}
    if actual != expected:
        raise RuntimeError("shadow parity reports do not cover every authority family exactly once")
    current=(now or datetime.now(timezone.utc)).astimezone(timezone.utc)
    if max_age_hours <= 0: raise RuntimeError("shadow parity freshness bound is invalid")
    probes=[];source=[];target=[];mismatches=0
    for family in sorted(gate.AUTHORITY_FAMILIES):
        report=collector._load_report(report_directory/f"{family}.json",family)
        observed=gate._parse_utc(report["reconciled_at"])
        age=(current-observed.astimezone(timezone.utc)).total_seconds()
        if age < 0 or age > max_age_hours*3600:
            raise RuntimeError(f"shadow parity report is not fresh for {family}")
        passed=(report["status"]=="passed" and report["source_count"]==report["target_count"]
                and all(report["checks"].get(name) is True for name in gate.REQUIRED_CHECKS))
        probes.append({"family":family,"passed":passed})
        source.append({"family":family,"count":report["source_count"],"identity":report["provenance"]["source_snapshot"]})
        target.append({"family":family,"count":report["target_count"],"identity":report["provenance"]["target_snapshot"]})
        mismatches += 0 if passed else 1
    if mismatches:
        raise RuntimeError("live shadow parity contains mismatches")
    return {"source_snapshot":hashlib.sha256(_canonical(source)).hexdigest(),
            "target_snapshot":hashlib.sha256(_canonical(target)).hexdigest(),
            "family_probes":probes,"mismatch_count":0}


def write_fence(deployment_revision: str) -> dict:
    if not isinstance(deployment_revision,str) or not deployment_revision or len(deployment_revision)>256:
        raise RuntimeError("deployment revision is invalid")
    if (os.getenv("METADATA_DB_TYPE","").lower()!="postgresql" or not fence.enabled()):
        raise RuntimeError("PostgreSQL write fence is not enabled")
    readonly=db.query("SELECT 1 AS retirement_fence_read_probe")
    if not isinstance(readonly,dict) or not readonly.get("rows"):
        raise RuntimeError("PostgreSQL read probe failed while fenced")
    probes=[]
    for family,tables in sorted(gate.AUTHORITY_FAMILIES.items()):
        statement=f"DELETE FROM {tables[0]} WHERE 1 = 0"
        try:
            db.execute(statement)
        except fence.PostgreSQLWriteFencedError:
            probes.append({"family":family,"passed":True})
        else:
            raise RuntimeError(f"PostgreSQL write fence allowed {family}")
    return {"deployment_revision":deployment_revision,"readonly_probe_passed":True,"family_probes":probes}
