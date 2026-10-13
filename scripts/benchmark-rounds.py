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

With --throughput 4,8 each engine's window also runs the throughput tier
(scripts/benchmark-throughput.py) after its latency suite, once per client
count, with the same duration and warm-up on both engines:
<out>/kaveon-throughput-<clients>-round<N>.json and trino-… beside it.
"""
import argparse
import json
import os
import pathlib
import subprocess
import sys
import time

NAMESPACE = "kaveon"


def runner_image():
    """The image the Kaveon-side Jobs run: the API image the cluster is
    serving, so the mounted scripts and the Engine bridge they import are
    the same build; KAVEON_RUNNER_IMAGE overrides it."""
    if image := os.environ.get("KAVEON_RUNNER_IMAGE"):
        return image
    image = kubectl("get", "deploy/kaveon-api", "-o",
                    "jsonpath={.spec.template.spec.containers[?(@.name==\"api\")].image}", capture=True).strip()
    if not image:
        raise SystemExit("kaveon-api Deployment has no api container image; set KAVEON_RUNNER_IMAGE")
    return image


KAVEON_STS = ["kaveon-coordinator", "kaveon-worker"]
# Workers each system gets while it is the one being measured. The pool is four
# nodes and the chart places one worker per node, so three leaves a node free
# for the other system's survivor — which is what lets the platform stay up
# through a round without either measurement acquiring a co-tenant.
BENCH_WORKERS = 3
# What Kaveon keeps while Trino is the one being measured: enough to serve the
# platform this cluster also hosts, on the node the benchmark is not using.
# Zero here is what took the demo down for a whole campaign.
DEMO_WORKERS = 1
# The resting state once a campaign is over: every node working.
REST_WORKERS = 4
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


def job_summary(name, summary_prefix):
    for line in kubectl("logs", f"job/{name}", capture=True).splitlines():
        line = line.strip()
        if line.startswith(summary_prefix):
            return json.loads(line[len(summary_prefix):])
    raise RuntimeError(f"job {name} printed no {summary_prefix} summary")


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
    scale("kaveon-worker", BENCH_WORKERS)
    if cold:
        for name in KAVEON_STS:
            kubectl("rollout", "restart", f"sts/{name}")
    for name in KAVEON_STS:
        wait_rollout(name)


def trino_window():
    """Give Trino the nodes it is measured on, and keep the platform alive.

    Kaveon used to go to zero here, which is what took the demo down for the
    length of a campaign. It now keeps its coordinator and one worker, on the
    fourth node — the one Trino is not using — so Studio stays served and
    Trino still measures on three nodes with nothing else on them.

    That is the whole reason the pool is four and not three. On three nodes the
    two are mutually exclusive: either Trino acquires a co-tenant and the
    measurement is not node-equal, or Kaveon goes to zero and the platform
    dies.
    """
    scale("kaveon-coordinator", 1)
    scale("kaveon-worker", DEMO_WORKERS)
    scale("kaveon-benchmark-trino-coordinator", 1)
    scale("kaveon-benchmark-trino-worker", BENCH_WORKERS)
    for name in TRINO_STS:
        wait_rollout(name)


def restore_platform():
    """Put the cluster back the way the platform needs it: Kaveon up, Trino down.

    Called from `finally`, so it runs after a failed round and after a Ctrl-C
    as well as after a clean campaign. It must therefore not raise: a restore
    that throws while unwinding an earlier failure would replace the real error
    with its own and still leave the Engine down. Each statefulset is scaled
    independently for the same reason — one that will not come back must not
    stop the others from trying.

    Scaling is what matters and is reported as it happens; waiting for the
    rollout is best-effort, because a worker that cannot schedule is a problem
    to see rather than a reason to hold the campaign's exit.
    """
    print("restoring the platform: Kaveon up, Trino down", flush=True)
    failures = []
    for name in TRINO_STS:
        try:
            scale(name, 0)
        except Exception as error:  # noqa: BLE001 - reported, never raised from finally
            failures.append(f"{name} -> 0: {error}")
    # One coordinator and every worker the pool can hold. Never zero: the
    # Engine is also this cluster's system of record, and a platform left with
    # no workers is indistinguishable to a reader from an installation that has
    # no data in it.
    for name, replicas in (("kaveon-coordinator", 1), ("kaveon-worker", REST_WORKERS)):
        try:
            scale(name, replicas)
        except Exception as error:  # noqa: BLE001
            failures.append(f"{name} -> {replicas}: {error}")
    for name in KAVEON_STS:
        try:
            wait_rollout(name, timeout="600s")
        except Exception as error:  # noqa: BLE001
            failures.append(f"{name} rollout: {error}")
    if failures:
        # Loud, and on stderr, because the platform is the thing left wrong.
        print("PLATFORM NOT FULLY RESTORED — scale it by hand:", file=sys.stderr, flush=True)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr, flush=True)
        print("  kubectl -n kaveon scale sts/kaveon-coordinator --replicas=1",
              file=sys.stderr, flush=True)
        print("  kubectl -n kaveon scale sts/kaveon-worker --replicas=3",
              file=sys.stderr, flush=True)


def apply_configmap(name, *files):
    manifest = kubectl("create", "configmap", name, *[f"--from-file={f}" for f in files],
                       "--dry-run=client", "-o", "yaml", capture=True)
    subprocess.run(["kubectl", "-n", NAMESPACE, "apply", "-f", "-"], input=manifest, text=True, check=True)


def apply_kaveon_input(suite):
    apply_configmap("kaveon-clickbench-input", "scale-suite.py=scripts/scale-suite.py",
                    "benchmark-throughput.py=scripts/benchmark-throughput.py", f"suite.json={suite}")


def apply_trino_input(suite, tables):
    apply_configmap("kaveon-trino-suite-input", "benchmark-trino-suite.py=scripts/benchmark-trino-suite.py",
                    "benchmark-throughput.py=scripts/benchmark-throughput.py",
                    f"suite.json={suite}", f"tables.json={tables}")


def launch_kaveon_job(name, script, extra_env=(), node_pool=None):
    """A Job from the live API pod contract (scripts/aks-scale-suite-job.py)
    running one mounted script from kaveon-clickbench-input."""
    kubectl("delete", "job", name, "--ignore-not-found")
    env = dict(os.environ, MSYS_NO_PATHCONV="1")
    command = [sys.executable, "scripts/aks-scale-suite-job.py", "--image", runner_image(), "--name", name,
               "--input", "kaveon-clickbench-input", "--suite", "/input/suite.json", "--script", script]
    for item in extra_env:
        command += ["--env", item]
    if node_pool:
        command += ["--node-pool", node_pool]
    job = subprocess.run(command, check=True, capture_output=True, text=True, env=env).stdout
    subprocess.run(["kubectl", "-n", NAMESPACE, "apply", "-f", "-"], input=job, text=True, check=True)


def finish(summary, ok, out_path, round_index):
    summary["round"] = round_index
    summary["job_succeeded"] = ok
    out_path.write_text(json.dumps(summary, indent=1), encoding="utf-8")
    return summary


def run_kaveon(suite, out_path, round_index):
    apply_kaveon_input(suite)
    launch_kaveon_job("kaveon-clickbench", "/input/scale-suite.py")
    ok = wait_job("kaveon-clickbench", 6 * 3600)
    return finish(job_records("kaveon-clickbench", "SCALE_SUITE="), ok, out_path, round_index)


def run_trino(suite, tables, out_path, round_index):
    apply_trino_input(suite, tables)
    kubectl("delete", "job", "kaveon-trino-suite", "--ignore-not-found")
    kubectl("apply", "-f", "infra/aks/kaveon-trino-suite-job.yaml")
    ok = wait_job("kaveon-trino-suite", 6 * 3600)
    return finish(job_records("kaveon-trino-suite", "TRINO_SUITE="), ok, out_path, round_index)


def throughput_env(clients, duration, warmup):
    return {"CLIENTS": str(clients), "DURATION_SECONDS": str(duration), "WARMUP_SECONDS": str(warmup)}


def throughput_timeout(duration, warmup):
    # The window, the warm-up, the statements in flight at the deadline and
    # Trino's table declarations; a Job past this is a failed round.
    return duration + warmup + 3600


def run_kaveon_throughput(suite, clients, duration, warmup, out_path, round_index):
    apply_kaveon_input(suite)
    env = throughput_env(clients, duration, warmup)
    # The clients stay on the system node: never a co-tenant of the workers.
    launch_kaveon_job("kaveon-throughput", "/input/benchmark-throughput.py",
                      ["ENGINE=kaveon", *[f"{k}={v}" for k, v in env.items()]], node_pool="system")
    ok = wait_job("kaveon-throughput", throughput_timeout(duration, warmup))
    return finish(job_summary("kaveon-throughput", "THROUGHPUT="), ok, out_path, round_index)


def with_env(job, values):
    """The manifest's container env with `values` substituted or appended."""
    container = job["spec"]["template"]["spec"]["containers"][0]
    env = [item for item in container.get("env", []) if item["name"] not in values]
    container["env"] = env + [{"name": name, "value": value} for name, value in values.items()]
    return job


def run_trino_throughput(suite, tables, clients, duration, warmup, out_path, round_index):
    apply_trino_input(suite, tables)
    kubectl("delete", "job", "kaveon-trino-throughput", "--ignore-not-found")
    job = json.loads(kubectl("create", "-f", "infra/aks/kaveon-trino-throughput-job.yaml",
                             "--dry-run=client", "-o", "json", capture=True))
    subprocess.run(["kubectl", "-n", NAMESPACE, "apply", "-f", "-"],
                   input=json.dumps(with_env(job, throughput_env(clients, duration, warmup))), text=True, check=True)
    ok = wait_job("kaveon-trino-throughput", throughput_timeout(duration, warmup))
    return finish(job_summary("kaveon-trino-throughput", "THROUGHPUT="), ok, out_path, round_index)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--cold", action="store_true", help="restart the Engine pods before every Kaveon round")
    parser.add_argument("--suite", required=True)
    parser.add_argument("--tables", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--skip-trino", action="store_true")
    parser.add_argument("--start-round", type=int, default=1, help="resume a campaign at this round")
    parser.add_argument("--throughput", default="", metavar="CLIENTS[,CLIENTS...]",
                        help="also run the throughput tier at these client counts in every window")
    parser.add_argument("--throughput-duration", type=int, default=300, help="measured seconds per throughput run")
    parser.add_argument("--throughput-warmup", type=int, default=30, help="warm-up seconds before each throughput run")
    args = parser.parse_args()
    client_counts = [int(value) for value in args.throughput.split(",") if value.strip()]
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    # The Kaveon window is restored in `finally`, not after the loop.
    #
    # This campaign gives each system the same three nodes by scaling the other
    # to zero, which is what makes the comparison node-equal. The restore used
    # to sit after the loop, so any round that raised — a failed job, a rollout
    # timeout, a Ctrl-C — left the cluster in whichever window it was in. When
    # that was Trino's, Kaveon stayed at zero replicas: the platform this
    # cluster also serves went down and stayed down, reporting "KaveonDB
    # unavailable" and, because Studio could not read its own configuration,
    # claiming it had no system of record at all.
    #
    # A benchmark may take the Engine down. It may not leave it down.
    try:
        for round_index in range(args.start_round, args.rounds + 1):
            print(f"round {round_index}: Kaveon window", flush=True)
            kaveon_window(args.cold)
            run_kaveon(args.suite, out / f"kaveon-round{round_index}.json", round_index)
            for clients in client_counts:
                print(f"round {round_index}: Kaveon throughput, {clients} clients", flush=True)
                run_kaveon_throughput(args.suite, clients, args.throughput_duration, args.throughput_warmup,
                                      out / f"kaveon-throughput-{clients}-round{round_index}.json", round_index)
            if not args.skip_trino:
                print(f"round {round_index}: Trino window", flush=True)
                trino_window()
                run_trino(args.suite, args.tables, out / f"trino-round{round_index}.json", round_index)
                for clients in client_counts:
                    print(f"round {round_index}: Trino throughput, {clients} clients", flush=True)
                    run_trino_throughput(args.suite, args.tables, clients, args.throughput_duration,
                                         args.throughput_warmup,
                                         out / f"trino-throughput-{clients}-round{round_index}.json", round_index)
    finally:
        restore_platform()
    subprocess.run([sys.executable, "scripts/benchmark-rounds-report.py", str(out)], check=True)


if __name__ == "__main__":
    main()
