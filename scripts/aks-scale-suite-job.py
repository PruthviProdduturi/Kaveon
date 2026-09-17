"""Run the scale suite on AKS as a Job built from the API Deployment's pod
contract, in the given API image, against the Engine digest that is deployed.

  python scripts/aks-scale-suite-job.py --image <repo>@sha256:... > tmp/scale-suite-job.json
  kubectl -n kaveon create configmap kaveon-scale-suite-input \\
      --from-file=scale-suite.py=scripts/scale-suite.py \\
      --from-file=scale-suite.json=docs/qualification/scale-suite.json --dry-run=client -o yaml | kubectl apply -f -
  kubectl apply -f tmp/scale-suite-job.json

The same contract launches the throughput tier (scripts/benchmark-throughput.py)
against the Engine, with the script and its parameters chosen here:

  python scripts/aks-scale-suite-job.py --image <repo>@sha256:... --name kaveon-throughput \\
      --input kaveon-clickbench-input --suite /input/suite.json --script /input/benchmark-throughput.py \\
      --env ENGINE=kaveon --env CLIENTS=4 --env DURATION_SECONDS=300 --env WARMUP_SECONDS=30 \\
      --node-pool system > tmp/throughput-job.json
"""
import argparse
import json
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--image", required=True)
    parser.add_argument("--namespace", default="kaveon")
    parser.add_argument("--name", default="kaveon-scale-suite")
    parser.add_argument("--input", default="kaveon-scale-suite-input",
                        help="configmap holding scale-suite.py and the suite JSON")
    parser.add_argument("--suite", default="/input/scale-suite.json",
                        help="suite JSON path inside the input mount")
    parser.add_argument("--script", default="/input/scale-suite.py",
                        help="the mounted script the Job runs")
    parser.add_argument("--env", action="append", default=[], metavar="NAME=VALUE",
                        help="extra environment for the script (repeatable)")
    parser.add_argument("--node-pool", default=None,
                        help="pin the Job to this agent pool (system keeps the client off the worker nodes)")
    args = parser.parse_args()
    extra = []
    for item in args.env:
        name, separator, value = item.partition("=")
        if not separator or not name:
            parser.error(f"--env expects NAME=VALUE, got {item!r}")
        extra.append({"name": name, "value": value})
    job_kind = args.script.rsplit("/", 1)[-1].removesuffix(".py")
    live = json.loads(subprocess.check_output(["kubectl", "get", "deploy", "kaveon-api", "-n", args.namespace, "-o", "json"]))
    engine = json.loads(subprocess.check_output(["kubectl", "get", "sts", "kaveon-worker", "-n", args.namespace, "-o", "json"]))
    engine_digest = engine["spec"]["template"]["spec"]["containers"][0]["image"].split("@")[-1]
    pod = live["spec"]["template"]["spec"]
    container = pod["containers"][0]
    labels = dict(live["spec"]["template"]["metadata"].get("labels", {}))
    labels.update({"app": "kaveon-api", "kaveon.io/job": job_kind})
    job = {
        "apiVersion": "batch/v1", "kind": "Job",
        "metadata": {"name": args.name, "namespace": args.namespace, "labels": {"app": "kaveon-" + job_kind}},
        "spec": {"backoffLimit": 0, "ttlSecondsAfterFinished": 86400, "template": {
            "metadata": {"labels": labels},
            "spec": {
                "restartPolicy": "Never",
                **({"nodeSelector": {"kubernetes.azure.com/agentpool": args.node_pool}} if args.node_pool else {}),
                "serviceAccountName": pod.get("serviceAccountName"),
                "securityContext": pod.get("securityContext", {}),
                "containers": [{
                    "name": "suite", "image": args.image, "workingDir": "/app",
                    "command": ["python", "-u", args.script],
                    "env": container.get("env", []) + [{"name": "ENGINE_DIGEST", "value": engine_digest},
                                                       {"name": "SUITE", "value": args.suite}] + extra,
                    "envFrom": container.get("envFrom", []),
                    "resources": {"requests": {"cpu": "100m", "memory": "256Mi"}, "limits": {"cpu": "500m", "memory": "1Gi"}},
                    "volumeMounts": [m for m in container.get("volumeMounts", []) if "retirement" not in m["name"]]
                                    + [{"name": "input", "mountPath": "/input", "readOnly": True}],
                    "securityContext": container.get("securityContext", {}),
                }],
                "volumes": [v for v in pod.get("volumes", []) if "retirement" not in v["name"]]
                           + [{"name": "input", "configMap": {"name": args.input}}],
            }}}}
    print(json.dumps(job, indent=1))


if __name__ == "__main__":
    main()
