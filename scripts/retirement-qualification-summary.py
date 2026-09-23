"""Produce a fail-closed PostgreSQL retirement qualification summary.

This is a read-only evidence merger. It never contacts AKS/PostgreSQL, changes
the write fence, scales workloads, or treats an absent rehearsal as passed.
"""

import argparse
import json
from pathlib import Path

import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services.postgresql_retirement_gate import AUTHORITY_FAMILIES, GLOBAL_GATE_NAMES  # noqa: E402
from services.postgresql_operational_evidence import BASELINE_GATES  # noqa: E402

EXPECTED_AUTHORITY_FAMILY_COUNT = len(AUTHORITY_FAMILIES)


def summarize(audit=None, operational=None):
    audit = audit or {}
    operational = operational or {}
    operational_validated = operational.get("schema_version") == 2
    family_by_name = {
        item.get("family"): item
        for item in audit.get("families", [])
        if isinstance(item, dict)
    }
    gate_by_name = {
        name: value
        for name, value in (audit.get("gates") or {}).items()
        if isinstance(value, dict)
    }
    families = []
    for name in sorted(AUTHORITY_FAMILIES):
        item = family_by_name.get(name)
        families.append({
            "family": name,
            "status": "passed" if item and item.get("status") == "passed" else "pending",
            "source_count": item.get("source_count") if item else None,
            "target_count": item.get("target_count") if item else None,
        })
    gates = []
    for name in GLOBAL_GATE_NAMES:
        item = gate_by_name.get(name)
        gates.append({
            "gate": name,
            "status": "passed" if item and item.get("status") == "passed" else "pending",
            "evidence_id": item.get("evidence_id") if item else None,
        })
    rehearsal = {
        "backup_restore": operational.get("backup_restore", {"status": "pending"}),
        "rollback": operational.get("rollback", {"status": "pending"}),
        "postgresql_unavailable_restart": operational.get(
            "postgresql_unavailable_restart", {"status": "pending"}
        ),
        "durable_checkpoint": operational.get("durable_checkpoint", {"status": "pending"}),
    }
    for name in BASELINE_GATES:
        rehearsal[name] = operational.get(name, {"status": "pending"})
    observed_families = set(family_by_name)
    family_inventory_complete = (
        observed_families == set(AUTHORITY_FAMILIES)
        and audit.get("authority_family_count") == EXPECTED_AUTHORITY_FAMILY_COUNT
    )
    audit_validated = audit.get("passed") is True
    passed = audit_validated and operational_validated and family_inventory_complete
    passed = passed and all(item["status"] == "passed" for item in families + gates)
    passed = passed and all(item.get("status") == "passed" for item in rehearsal.values())
    baseline_bindings = {
        (rehearsal[name].get("baseline_evidence_id"),
         rehearsal[name].get("baseline_sha256"))
        for name in BASELINE_GATES
        if rehearsal[name].get("status") == "passed"
    }
    baseline_bound = (len(baseline_bindings) == 1
                      and all(all(binding) for binding in baseline_bindings)
                      and all(rehearsal[name].get("status") == "passed"
                              for name in BASELINE_GATES))
    passed = passed and baseline_bound
    blockers = []
    if not audit_validated:
        blockers.append("reconciliation audit is missing or not passed")
    if not operational_validated:
        blockers.append("operational evidence schema is missing or unsupported")
    if not family_inventory_complete:
        missing = sorted(set(AUTHORITY_FAMILIES) - observed_families)
        extra = sorted(observed_families - set(AUTHORITY_FAMILIES))
        if missing:
            blockers.append("authority families missing: " + ", ".join(missing))
        if extra:
            blockers.append("unknown authority families present: " + ", ".join(extra))
        if audit.get("authority_family_count") != EXPECTED_AUTHORITY_FAMILY_COUNT:
            blockers.append("authority family count does not match the maintained inventory")
    blockers.extend(
        f"authority family pending: {item['family']}"
        for item in families if item["status"] != "passed"
    )
    blockers.extend(
        f"global gate pending: {item['gate']}"
        for item in gates if item["status"] != "passed"
    )
    blockers.extend(
        f"operational gate pending: {name}"
        for name, item in rehearsal.items() if item.get("status") != "passed"
    )
    if not baseline_bound:
        blockers.append("fresh PostgreSQL baseline gates are not all passed and bound to one identity")
    return {
        "schema_version": 2,
        "qualification": "postgresql-retirement",
        "passed": passed,
        "status": "passed" if passed else "pending",
        "authority_family_count": len(families),
        "expected_authority_family_count": EXPECTED_AUTHORITY_FAMILY_COUNT,
        "family_inventory_complete": family_inventory_complete,
        "authority_families": families,
        "global_gate_count": len(gates),
        "global_gates": gates,
        "rehearsal_gates": rehearsal,
        "fresh_postgresql_baseline_bound": baseline_bound,
        "blocking_reasons": blockers,
        "source": {
            "audit_loaded": bool(audit),
            "audit_validated": audit_validated,
            "operational_loaded": bool(operational),
            "operational_validated": operational_validated,
        },
    }


def read_json(path):
    return json.loads(path.read_text()) if path else {}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--audit", type=Path)
    parser.add_argument("--operational", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = summarize(read_json(args.audit), read_json(args.operational))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"passed": result["passed"], "status": result["status"],
                      "authority_family_count": result["authority_family_count"],
                      "global_gate_count": result["global_gate_count"]}, sort_keys=True))
    return 0 if result["passed"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
