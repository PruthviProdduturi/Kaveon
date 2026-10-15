"""Inject one scoped ADLS egress fault and qualify exact task recovery on AKS.

The policy selects only ``kaveon-worker-0``, permits cluster/DNS traffic, and
blocks public object-store traffic. It is always removed in ``finally``. The
report contains query results and recovery metadata but never credentials.
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


_SPEC = importlib.util.spec_from_file_location(
    "qualify_aks_fault_pressure",
    Path(__file__).with_name("qualify-aks-fault-pressure.py"),
)
assert _SPEC and _SPEC.loader
_MODULE = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(_MODULE)


POLICY = "kaveon-storage-fault-injection"
POLICY_YAML = f"""apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: {POLICY}
  namespace: kaveon
spec:
  podSelector:
    matchLabels:
      statefulset.kubernetes.io/pod-name: kaveon-worker-0
  policyTypes: [Egress]
  egress:
  - to:
    - ipBlock: {{cidr: 10.0.0.0/8}}
    - ipBlock: {{cidr: 10.244.0.0/16}}
  - to:
    - namespaceSelector:
        matchLabels: {{kubernetes.io/metadata.name: kube-system}}
      podSelector: {{matchLabels: {{k8s-app: kube-dns}}}}
    ports:
    - {{protocol: UDP, port: 53}}
    - {{protocol: TCP, port: 53}}
"""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--context", default="kaveon-aks")
    parser.add_argument("--namespace", default="kaveon")
    parser.add_argument(
        "--output",
        type=Path,
        default=Path("tmp/aks-storage-fault/report.json"),
    )
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)

    kube = _MODULE.Kube(args.context, args.namespace)
    report: dict[str, object] = {
        "context": args.context,
        "namespace": args.namespace,
        "policy": POLICY,
        "worker": "kaveon-worker-0",
        "query": "SELECT COUNT(*) FROM kaveon_events_enriched WHERE actions >= 0",
        "credential_material_recorded": False,
        "subscription_policy_changed": False,
        "checks": {},
        "passed": False,
    }
    port_forward = None
    log_handle = None
    work_dir: Path | None = None
    policy_applied = False
    try:
        workers = sorted(
            item["metadata"]["name"]
            for item in kube.json("pods")["items"]
            if item["metadata"]["name"].startswith("kaveon-worker-")
        )
        report["retained_files_before"] = {
            "coordinator_exchange": kube.file_count(
                "kaveon-coordinator-0", "/state/exchange"
            ),
            **{
                f"{name}_spill": kube.file_count(name, "/tmp/spill")
                for name in workers
            },
        }
        engine_secret = _MODULE.engine_secret_data(kube)
        tls_secret = _MODULE.secret_data(kube, _MODULE.TLS_SECRET)
        token, principal = _MODULE.choose_principal(
            base64.b64decode(engine_secret["security.json"])
        )
        report["principal"] = principal
        work_dir = Path(tempfile.mkdtemp(prefix="kaveon-storage-fault-"))
        ca_path = work_dir / "ca.crt"
        ca_path.write_bytes(base64.b64decode(tls_secret["ca.crt"]))
        port = _MODULE.free_port()
        log_handle = (work_dir / "port-forward.log").open("w", encoding="utf-8")
        port_forward = subprocess.Popen(
            [
                *kube.prefix,
                "port-forward",
                "service/kaveon-coordinator",
                f"{port}:8080",
                "--address",
                "127.0.0.1",
            ],
            stdout=log_handle,
            stderr=subprocess.STDOUT,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        _MODULE.wait_for_port(port, port_forward)
        engine = _MODULE.Engine(f"https://127.0.0.1:{port}", token, str(ca_path))
        case = {
            "name": "storage_fault_scan",
            "catalog": "OpenSource",
            "schema": "public",
            "sql": report["query"],
        }
        baseline = engine.submit(case, timeout=300)
        report["baseline"] = baseline
        subprocess.run(
            [
                "kubectl",
                "--context",
                args.context,
                "-n",
                args.namespace,
                "apply",
                "-f",
                "-",
            ],
            input=POLICY_YAML,
            text=True,
            check=True,
            capture_output=True,
        )
        policy_applied = True
        report["policy_applied"] = True
        started = time.monotonic()
        try:
            fault = engine.submit(case, timeout=300)
            report["fault_result"] = fault
            report["checks"]["exact_result_after_storage_fault"] = (
                fault.get("data") == baseline.get("data")
            )
            recovery = fault.get("recovery") or {}
            report["checks"]["stage_retry_observed"] = bool(
                recovery.get("stage_retries")
                or any(
                    str(task_id).rsplit(".", 1)[-1].isdigit()
                    and int(str(task_id).rsplit(".", 1)[-1]) > 0
                    for task_id in fault.get("task_ids", [])
                )
            )
        except Exception as error:  # retain the failure evidence in the report
            report["fault_error"] = str(error)
            report["checks"]["exact_result_after_storage_fault"] = False
            report["checks"]["stage_retry_observed"] = False
        report["fault_elapsed_seconds"] = round(time.monotonic() - started, 2)
    except Exception as error:
        report["failure"] = str(error)
    finally:
        if policy_applied:
            subprocess.run(
                [
                    "kubectl",
                    "--context",
                    args.context,
                    "-n",
                    args.namespace,
                    "delete",
                    "networkpolicy",
                    POLICY,
                    "--ignore-not-found=true",
                ],
                check=False,
                capture_output=True,
            )
            report["policy_removed"] = True
        if port_forward is not None and port_forward.poll() is None:
            port_forward.terminate()
            try:
                port_forward.wait(timeout=10)
            except subprocess.TimeoutExpired:
                port_forward.kill()
                port_forward.wait(timeout=10)
        if log_handle is not None:
            log_handle.close()
        if work_dir is not None:
            subprocess.run(
                ["cmd", "/c", "rmdir", "/s", "/q", str(work_dir)],
                check=False,
                capture_output=True,
            )

    workers = sorted(
        item["metadata"]["name"]
        for item in kube.json("pods")["items"]
        if item["metadata"]["name"].startswith("kaveon-worker-")
    )
    report["retained_files_after"] = {
        "coordinator_exchange": kube.file_count(
            "kaveon-coordinator-0", "/state/exchange"
        ),
        **{
            f"{name}_spill": kube.file_count(name, "/tmp/spill")
            for name in workers
        },
    }
    report["checks"]["no_retained_exchange_or_spill_files"] = all(
        value == 0
        for value in report["retained_files_after"].values()
        if value is not None
    )
    report["passed"] = bool(report["checks"]) and all(report["checks"].values()) and bool(
        report.get("policy_removed")
    )
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"{'PASS' if report['passed'] else 'FAIL'}: {args.output}")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
