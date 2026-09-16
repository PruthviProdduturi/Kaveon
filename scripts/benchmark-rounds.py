"""Alternating benchmark rounds on the AKS qualification cluster: Kaveon
then Trino, each alone on the worker nodes, N times, so the published
figures are medians over rounds rather than one pass. With --cold, every
Kaveon round starts from freshly restarted Engine pods (an empty decoded
batch cache; the object store is remote either way) and every Trino round
from freshly started Trino pods.

    python scripts/benchmark-rounds.py --rounds 5 --cold \\
        --suite docs/qualification/clickbench/kaveon-suite.json \\
        --tables docs/qualification/clickbench/trino-tables.json \\
        --out docs/qualification/clickbench/runs/rounds-2026-09-17

Writes <out>/kaveon-round<N>.json and <out>/trino-round<N>.json (the
records each suite printed), then <out>/rounds.json with the round-level
medians per statement. Summarise with scripts/benchmark-rounds-report.py.
Requires kubectl against the cluster and the two suite job manifests this
repository already carries. Never publish a single round as a claim.
"""
import argparse
import json
import os
import pathlib
import subprocess
import sys
import time

NAMESPACE = "kaveon"
RUNNER_IMAGE = os.environ.get(
    "KAVEON_RUNNER_IMAGE",
    "kvtesticmwwliihpppo.azurecr.io/kaveon-api@sha256:a946aa322715db61d14568ab2e987ad23ef4ea982e0dba29645733ab4ea1ba14",
)
KAVEON_STS = ["kaveon-coordinator", "kaveon-worker"]
TRINO_STS = ["kaveon-benchmark-trino-coordinator", "kaveon-benchmark-trino-worker"]


def sh(*args, check=True, capture=False):
    result = subprocess.run(args, check=check, text=True, capture_output=capture, encoding="utf-8", errors="replace")
    return result.stdout if capture else None


def kubectl(*args, **kw):
    return sh("kubectl", "-n", NAMESPACE, *args, **kw)


def scale(name, replicas):
    kubectl("scale", f"sts/{name}", f"--replicas={replicas}")


def wait_rollout(name, timeout="900s"):
    kubectl("rollout", "status", f"sts/{name}", f"--timeout={timeout}")


def wait_job(name, timeout_seconds):
    started = time.time()
    while True:
        status = json.loads(kubectl("get", "job", name, "-o", "json", capture=True))["status"]
        if status.get("succeeded"):
            return True
        if status.get("failed"):
            return False
        if time.time() - started > timeout_seconds:
            raise TimeoutError(f"job {name} did not finish in {timeout_seconds}s")
        time.sleep(30)


def job_records(name, summary_prefix):
    records, summary = [], None
    for line in kubectl("logs", f"job/{name}", capture=True).splitlines():
        line = line.strip()
        if line.startswith(summary_prefix):
            summary = json.loads(line[len(summary_prefix):])
        elif line.startswith("{"):
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                continue
            if "id" in record and ("median_seconds" in record or "error" in record):
                records.append(record)
    if summary is None:
        raise RuntimeError(f"job {name} printed no {summary_prefix} summary")
    summary["records"] = summary.get("records") or records
    return summary


def kaveon_window(cold):
    for name in TRINO_STS:
        scale(name, 0)
    scale("kaveon-coordinator", 1)
    scale("kaveon-worker", 3)
    if cold:
        for name in KAVEON_STS:
            kubectl("rollout", "restart", f"sts/{name}")
    for name in KAVEON_STS:
        wait_rollout(name)


def trino_window():
    for name in KAVEON_STS:
        scale(name, 0)
    scale("kaveon-benchmark-trino-coordinator", 1)
    scale("kaveon-benchmark-trino-worker", 3)
    for name in TRINO_STS:
        wait_rollout(name)


def run_kaveon(suite, out_path, round_index):
    manifest = kubectl("create", "configmap", "kaveon-clickbench-input",
                       "--from-file=scale-suite.py=scripts/scale-suite.py",
                       f"--from-file=suite.json={suite}",
                       "--dry-run=client", "-o", "yaml", capture=True)
    subprocess.run(["kubectl", "-n", NAMESPACE, "apply", "-f", "-"], input=manifest, text=True, check=True)
    kubectl("delete", "job", "kaveon-clickbench", "--ignore-not-found")
    env = dict(os.environ, MSYS_NO_PATHCONV="1")
    job = subprocess.run(
        [sys.executable, "scripts/aks-scale-suite-job.py", "--image", RUNNER_IMAGE, "--name", "kaveon-clickbench",
         "--input", "kaveon-clickbench-input", "--suite", "/input/suite.json"],
        check=True, capture_output=True, text=True, env=env,
    ).stdout
    subprocess.run(["kubectl", "-n", NAMESPACE, "apply", "-f", "-"], input=job, text=True, check=True)
    ok = wait_job("kaveon-clickbench", 6 * 3600)
    record = job_records("kaveon-clickbench", "SCALE_SUITE=")
    record["round"] = round_index
    record["job_succeeded"] = ok
    out_path.write_text(json.dumps(record, indent=1), encoding="utf-8")
    return record


def run_trino(suite, tables, out_path, round_index):
    manifest = kubectl("create", "configmap", "kaveon-trino-suite-input",
                       "--from-file=benchmark-trino-suite.py=scripts/benchmark-trino-suite.py",
                       f"--from-file=suite.json={suite}",
                       f"--from-file=tables.json={tables}",
                       "--dry-run=client", "-o", "yaml", capture=True)
    subprocess.run(["kubectl", "-n", NAMESPACE, "apply", "-f", "-"], input=manifest, text=True, check=True)
    kubectl("delete", "job", "kaveon-trino-suite", "--ignore-not-found")
    kubectl("apply", "-f", "infra/aks/kaveon-trino-suite-job.yaml")
    ok = wait_job("kaveon-trino-suite", 6 * 3600)
    record = job_records("kaveon-trino-suite", "TRINO_SUITE=")
    record["round"] = round_index
    record["job_succeeded"] = ok
    out_path.write_text(json.dumps(record, indent=1), encoding="utf-8")
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--cold", action="store_true", help="restart the Engine pods before every Kaveon round")
    parser.add_argument("--suite", required=True)
    parser.add_argument("--tables", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--skip-trino", action="store_true")
    parser.add_argument("--start-round", type=int, default=1, help="resume a campaign at this round")
    args = parser.parse_args()
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    for round_index in range(args.start_round, args.rounds + 1):
        print(f"round {round_index}: Kaveon window", flush=True)
        kaveon_window(args.cold)
        run_kaveon(args.suite, out / f"kaveon-round{round_index}.json", round_index)
        if not args.skip_trino:
            print(f"round {round_index}: Trino window", flush=True)
            trino_window()
            run_trino(args.suite, args.tables, out / f"trino-round{round_index}.json", round_index)
    # Leave the cluster as the platform expects it: Kaveon up, Trino down.
    kaveon_window(cold=False)
    subprocess.run([sys.executable, "scripts/benchmark-rounds-report.py", str(out)], check=True)


if __name__ == "__main__":
    main()
