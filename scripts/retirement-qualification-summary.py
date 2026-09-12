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

EXPECTED_AUTHORITY_FAMILY_COUNT = len(AUTHORITY_FAMILIES)


def summarize(audit=None, operational=None):
    audit = audit or {}
    operational = operational or {}
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
    family_inventory_complete = len(families) == EXPECTED_AUTHORITY_FAMILY_COUNT
    passed = family_inventory_complete and all(item["status"] == "passed" for item in families + gates)
    passed = passed and all(item.get("status") == "passed" for item in rehearsal.values())
    return {
        "schema_version": 1,
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
        "source": {
            "audit_loaded": bool(audit),
            "operational_loaded": bool(operational),
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
