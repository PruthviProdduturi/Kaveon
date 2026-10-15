"""Run bounded distributed outer-join and skew qualification on AKS.

Credentials are read from Kubernetes Secrets into memory and are never written
to the report. The probe uses the existing fault-pressure Engine client so the
recorded result includes stages, tasks, workers, and the catalog snapshot.
"""

from __future__ import annotations

import argparse
import base64
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import time


ROOT = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("fault_pressure", ROOT / "qualify-aks-fault-pressure.py")
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC and SPEC.loader
SPEC.loader.exec_module(MODULE)


CASES = [
    {
        "name": "left_outer_self_join",
        "schema": "public",
        "sql": (
            "SELECT a.region, COUNT(*) AS rows "
            "FROM kaveon_events_dashboard a "
            "LEFT OUTER JOIN kaveon_events_dashboard b ON a.user_id = b.user_id "
            "GROUP BY a.region ORDER BY a.region"
        ),
    },
    {
        "name": "full_outer_cross_source_join",
        "schema": "public",
        "sql": (
            "SELECT COUNT(*) AS rows "
            "FROM kaveon_events_dashboard a "
            "FULL OUTER JOIN nyc_taxi_borough b ON a.region = b.borough"
        ),
    },
    {
        "name": "skewed_grouped_join",
        "schema": "public",
        "sql": (
            "SELECT a.region, COUNT(*) AS rows "
            "FROM kaveon_events_dashboard a "
            "JOIN kaveon_events_dashboard b ON a.region = b.region "
            "GROUP BY a.region ORDER BY a.region"
        ),
    },
]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--context", default="kaveon-aks")
    parser.add_argument("--namespace", default="kaveon")
    parser.add_argument("--output", type=Path, default=Path("tmp/aks-join-gates.json"))
    args = parser.parse_args()

    kube = MODULE.Kube(args.context, args.namespace)
    report = {
        "context": args.context,
        "namespace": args.namespace,
        "catalog": "OpenSource",
        "credential_material_recorded": False,
        "subscription_policy_changed": False,
        "checks": {},
        "cases": [],
        "passed": False,
    }
    forward = None
    log = None
    try:
        pods = MODULE.pod_inventory(kube)
        workers = sorted(name for name in pods if name.startswith("kaveon-worker-"))
        report["pods"] = {name: pods[name] for name in workers}
        if len(workers) != 3 or not all(pods[name]["ready"] for name in workers):
            raise RuntimeError("expected three Ready worker pods")

        engine_secret = MODULE.engine_secret_data(kube)
        tls_secret = MODULE.secret_data(kube, MODULE.TLS_SECRET)
        token, principal = MODULE.choose_principal(base64.b64decode(engine_secret["security.json"]))
        report["principal"] = principal
        with tempfile.TemporaryDirectory(prefix="kaveon-aks-join-") as private:
            ca_path = Path(private) / "ca.crt"
            ca_path.write_bytes(base64.b64decode(tls_secret["ca.crt"]))
            port = MODULE.free_port()
            log = (args.output.parent / "join-port-forward.log").open("w", encoding="utf-8")
            forward = subprocess.Popen(
                [*kube.prefix, "port-forward", "service/kaveon-coordinator", f"{port}:8080", "--address", "127.0.0.1"],
                stdout=log,
                stderr=subprocess.STDOUT,
                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            )
            MODULE.wait_for_port(port, forward)
            engine = MODULE.Engine(f"https://127.0.0.1:{port}", token, str(ca_path))
            cluster = engine.get("/v1/cluster")
            report["cluster"] = MODULE.cluster_summary(cluster)
            report["checks"]["three_catalog_compatible_workers"] = (
                cluster.get("active_workers") == 3
                and cluster.get("compatible_workers") == 3
            )
            for case in CASES:
                first = engine.submit({**case, "catalog": "OpenSource"})
                second = engine.submit({**case, "catalog": "OpenSource"})
                exact = first.get("data") == second.get("data") and first.get("state") == "FINISHED" and second.get("state") == "FINISHED"
                report["cases"].append({
                    "name": case["name"],
                    "sql": case["sql"],
                    "exact_repeat": exact,
                    "first": first,
                    "second": second,
                })
            report["checks"]["outer_join_exact_repeats"] = all(
                item["exact_repeat"] for item in report["cases"][:2]
            )
            report["checks"]["skewed_join_exact_repeat"] = report["cases"][2]["exact_repeat"]
            report["checks"]["all_cases_finished"] = all(
                item["first"].get("state") == "FINISHED" and item["second"].get("state") == "FINISHED"
                for item in report["cases"]
            )
            report["passed"] = all(report["checks"].values())
    except Exception as error:
        report["error"] = str(error)
        report["passed"] = False
    finally:
        if forward is not None:
            forward.terminate()
            try:
                forward.wait(timeout=5)
            except subprocess.TimeoutExpired:
                forward.kill()
        if log is not None:
            log.close()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps({"passed": report["passed"], "checks": report["checks"]}, sort_keys=True))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
