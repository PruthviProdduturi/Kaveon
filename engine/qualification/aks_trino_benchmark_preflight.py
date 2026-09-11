"""Read-only Azure/AKS preflight for the three-worker cloud comparison."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

from same_files import EXTENDED_QUERIES


def run(command, timeout=120):
    try:
        completed = subprocess.run(command, capture_output=True, text=True, timeout=timeout, check=False)
        return {"returncode": completed.returncode, "stdout": completed.stdout, "stderr": completed.stderr}
    except (OSError, subprocess.SubprocessError) as error:
        return {"returncode": None, "stdout": "", "stderr": f"{type(error).__name__}: {error}"}


def parse_json(text):
    start, end = text.find("{"), text.rfind("}")
    if start < 0 or end < start:
        raise ValueError("command output did not contain a JSON object")
    return json.loads(text[start:end + 1])


def pod_resources(sts):
    return sts["spec"]["template"]["spec"]["containers"][0].get("resources") or {}


def evaluate(snapshot, manifest, expected_subscription, expected_kaveon_digest, release):
    cluster = snapshot.get("cluster") or {}
    pools = cluster.get("agentPoolProfiles") or []
    system = [pool for pool in pools if pool.get("mode") == "System"]
    workers = [pool for pool in pools if pool.get("name") == "workers"]
    objects = {item["metadata"]["name"]: item for item in (snapshot.get("statefulsets") or {}).get("items", [])}
    names = {"kc": "kaveon-coordinator", "kw": "kaveon-worker",
             "tc": f"{release}-trino-coordinator", "tw": f"{release}-trino-worker"}
    found = all(name in objects for name in names.values())
    corpus_hash = hashlib.sha256(json.dumps(EXTENDED_QUERIES, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    data = (manifest or {}).get("dataset") or {}
    corpus = (manifest or {}).get("query_corpus") or {}
    checks = {
        "azure_cli": snapshot.get("azure_cli") is True,
        "exact_subscription": (snapshot.get("account") or {}).get("id") == expected_subscription,
        "target_cluster_running": (cluster.get("powerState") or {}).get("code") == "Running",
        "one_system_pool": len(system) == 1 and system[0].get("count") == 1,
        "three_worker_pool": len(workers) == 1 and workers[0].get("count") == 3
            and (workers[0].get("nodeLabels") or {}).get("workload") == "kaveon-worker",
        "same_node_sku": bool(system and workers) and system[0].get("vmSize") == workers[0].get("vmSize"),
        "all_statefulsets_present": found,
        "secrets_manifest_and_identity_present": snapshot.get("dependencies_present") is True,
        "kaveon_live_trino_parked": found and objects[names["kc"]]["spec"].get("replicas") == 1
            and objects[names["kw"]]["spec"].get("replicas") == 3
            and objects[names["tc"]]["spec"].get("replicas") == 0 and objects[names["tw"]]["spec"].get("replicas") == 0,
        "role_resources_matched": found and pod_resources(objects[names["kc"]]) == pod_resources(objects[names["tc"]])
            and pod_resources(objects[names["kw"]]) == pod_resources(objects[names["tw"]]),
        "immutable_expected_kaveon_image": found and all(
            objects[names[key]]["spec"]["template"]["spec"]["containers"][0]["image"].endswith("@" + expected_kaveon_digest)
            for key in ("kc", "kw")),
        "immutable_trino_image": found and "@sha256:" in objects[names["tc"]]["spec"]["template"]["spec"]["containers"][0]["image"]
            and objects[names["tc"]]["spec"]["template"]["spec"]["containers"][0]["image"]
            == objects[names["tw"]]["spec"]["template"]["spec"]["containers"][0]["image"],
        "fixture_publication_size": data.get("rows", 0) >= 5_000_000 and data.get("customers", 0) >= 100_000,
        "fixture_exact_corpus": corpus.get("sha256") == corpus_hash and set(corpus.get("queries") or {}) == set(EXTENDED_QUERIES),
        "fixture_two_parquet_objects": len([item for item in data.get("objects") or [] if item.get("parquet_data")]) == 2
            and all(item.get("sha256") and item.get("content_md5") and item.get("bytes", 0) > 0
                    for item in data.get("objects") or []),
    }
    return {"checks": checks, "ready": all(checks.values()),
            "blocked_by": [name for name, passed in checks.items() if not passed]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--subscription", required=True)
    parser.add_argument("--resource-group", required=True)
    parser.add_argument("--cluster", required=True)
    parser.add_argument("--namespace", default="kaveon")
    parser.add_argument("--release", default="kaveon-benchmark")
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--kaveon-image-digest", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    azure = "az.cmd" if os.name == "nt" else "az"
    snapshot = {"azure_cli": shutil.which(azure) is not None}
    diagnostics = {}
    if snapshot["azure_cli"]:
        account = run([azure, "account", "show", "-o", "json"])
        cluster = run([azure, "aks", "show", "--subscription", args.subscription, "--resource-group", args.resource_group,
                       "--name", args.cluster, "-o", "json"])
        command = run([azure, "aks", "command", "invoke", "--subscription", args.subscription,
                       "--resource-group", args.resource_group, "--name", args.cluster,
                       "--command", f"kubectl -n {args.namespace} get statefulsets -o json", "-o", "json"], timeout=180)
        dependencies = run([azure, "aks", "command", "invoke", "--subscription", args.subscription,
            "--resource-group", args.resource_group, "--name", args.cluster,
            "--command", (f"kubectl -n {args.namespace} get secret/kaveon-engine-auth secret/kaveon-engine-tls "
                "secret/kaveon-trino-benchmark-auth secret/kaveon-trino-benchmark-tls "
                "configmap/kaveon-trino-benchmark-manifest serviceaccount/kaveon-engine -o name"), "-o", "json"], timeout=180)
        diagnostics = {"account": account, "cluster": cluster, "statefulsets": command, "dependencies": dependencies}
        try:
            snapshot["account"] = json.loads(account["stdout"])
            snapshot["cluster"] = json.loads(cluster["stdout"])
            envelope = json.loads(command["stdout"])
            if envelope.get("exitCode") != 0:
                raise ValueError(envelope.get("logs") or "AKS command failed")
            snapshot["statefulsets"] = parse_json(envelope.get("logs") or "")
            dependency_envelope = json.loads(dependencies["stdout"])
            snapshot["dependencies_present"] = dependency_envelope.get("exitCode") == 0
        except (json.JSONDecodeError, ValueError) as error:
            diagnostics["parse_error"] = f"{type(error).__name__}: {error}"
    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    result = evaluate(snapshot, manifest, args.subscription, args.kaveon_image_digest, args.release)
    bounded_diagnostics = {}
    for name, diagnostic in diagnostics.items():
        if isinstance(diagnostic, dict):
            bounded_diagnostics[name] = {**diagnostic, "stdout": diagnostic.get("stdout", "")[-8000:],
                                         "stderr": diagnostic.get("stderr", "")[-8000:]}
        else:
            bounded_diagnostics[name] = diagnostic
    report = {"schema_version": 1, "checked_at": datetime.now(timezone.utc).isoformat(),
              "benchmark": "resource-matched distributed Kaveon versus Trino on AKS",
              **result, "diagnostics": bounded_diagnostics, "claim_evidence": False,
              "limitation": "Readiness only. This report is never performance or correctness evidence."}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"ready={str(report['ready']).lower()}; blocked_by={','.join(report['blocked_by']) or 'none'}")
    return 0 if report["ready"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
