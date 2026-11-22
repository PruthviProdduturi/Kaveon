#!/usr/bin/env python3
"""Fail-closed validator for the local Engine release evidence bundle."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


def read_report(path: Path) -> dict:
    if not path.is_file():
        raise ValueError(f"missing report: {path}")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ValueError(f"invalid JSON report: {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise ValueError(f"report must be an object: {path}")
    return value


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--docker-report", type=Path, required=True)
    parser.add_argument("--exchange-report", type=Path, required=True)
    parser.add_argument("--pressure-report", type=Path, required=True)
    parser.add_argument("--soak-report", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    reports = {
        "docker_distributed": read_report(args.docker_report),
        "exchange_loss": read_report(args.exchange_report),
        "pressure": read_report(args.pressure_report),
        "soak": read_report(args.soak_report),
    }
    if not args.binary.is_file():
        raise SystemExit(f"missing Engine binary: {args.binary}")
    digest = hashlib.sha256(args.binary.read_bytes()).hexdigest()
    checks = {
        "docker_exact": reports["docker_distributed"].get("same_file_correctness") is True
        and reports["docker_distributed"].get("workers", 0) >= 2
        and bool(reports["docker_distributed"].get("cases"))
        and all(case.get("passed") is True for case in reports["docker_distributed"]["cases"]),
        "exchange_recovery": reports["exchange_loss"].get("passed") is True
        and reports["exchange_loss"].get("status") == 200
        and reports["exchange_loss"].get("data") == reports["exchange_loss"].get("expected")
        and reports["exchange_loss"].get("exchange_files_after_completion") == 0,
        "pressure_cleanup": reports["pressure"].get("passed") is True
        and all(case.get("passed") is True for case in reports["pressure"].get("cases", []))
        and any(case.get("name") == "sort_spill" and case.get("spill_observed") is True
                for case in reports["pressure"].get("cases", [])),
        "soak_recovery": reports["soak"].get("passed") is True
        and all(reports["soak"].get("checks", {}).values()),
    }
    result = {
        "schema_version": 1,
        "scope": "local-docker",
        "binary_sha256": digest,
        "checks": checks,
        "cloud_gates": "pending",
        "passed": all(checks.values()),
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result["passed"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
