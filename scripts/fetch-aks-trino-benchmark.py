"""Fetch a completed AKS benchmark report from the runner's bounded logs."""

import argparse
import json
import os
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--subscription", required=True)
    parser.add_argument("--resource-group", required=True)
    parser.add_argument("--cluster", required=True)
    parser.add_argument("--namespace", default="kaveon")
    parser.add_argument("--job", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    azure = "az.cmd" if os.name == "nt" else "az"
    raw = subprocess.check_output([azure, "aks", "command", "invoke", "--subscription", args.subscription,
        "--resource-group", args.resource_group, "--name", args.cluster,
        "--command", f"kubectl -n {args.namespace} logs job/{args.job}", "-o", "json"], text=True)
    envelope = json.loads(raw)
    if envelope.get("exitCode") != 0:
        raise SystemExit("AKS log retrieval failed: " + str(envelope.get("logs")))
    lines = [line[len("REPORT_JSON "):] for line in (envelope.get("logs") or "").splitlines()
             if line.startswith("REPORT_JSON ")]
    if len(lines) != 1:
        raise SystemExit(f"Expected one report marker, found {len(lines)}")
    report = json.loads(lines[0])
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"Fetched report for {args.job}; passed={str(report.get('passed') is True).lower()}")
    return 0 if report.get("passed") is True and not (report.get("restoration") or {}).get("errors") else 2


if __name__ == "__main__":
    raise SystemExit(main())
