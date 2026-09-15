"""Run the scale suite on AKS as a Job built from the API Deployment's pod
contract, in the given API image, against the Engine digest that is deployed.

  python scripts/aks-scale-suite-job.py --image <repo>@sha256:... > tmp/scale-suite-job.json
  kubectl -n kaveon create configmap kaveon-scale-suite-input \\
      --from-file=scale-suite.py=scripts/scale-suite.py \\
      --from-file=scale-suite.json=docs/qualification/scale-suite.json --dry-run=client -o yaml | kubectl apply -f -
  kubectl apply -f tmp/scale-suite-job.json
"""
import argparse
import json
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--image", required=True)
    parser.add_argument("--namespace", default="kaveon")
    parser.add_argument("--name", default="kaveon-scale-suite")
    args = parser.parse_args()
    live = json.loads(subprocess.check_output(["kubectl", "get", "deploy", "kaveon-api", "-n", args.namespace, "-o", "json"]))
    engine = json.loads(subprocess.check_output(["kubectl", "get", "sts", "kaveon-worker", "-n", args.namespace, "-o", "json"]))
    engine_digest = engine["spec"]["template"]["spec"]["containers"][0]["image"].split("@")[-1]
    pod = live["spec"]["template"]["spec"]
    container = pod["containers"][0]
    labels = dict(live["spec"]["template"]["metadata"].get("labels", {}))
    labels.update({"app": "kaveon-api", "kaveon.io/job": "scale-suite"})
    job = {
        "apiVersion": "batch/v1", "kind": "Job",
        "metadata": {"name": args.name, "namespace": args.namespace, "labels": {"app": "kaveon-scale-suite"}},
        "spec": {"backoffLimit": 0, "ttlSecondsAfterFinished": 86400, "template": {
            "metadata": {"labels": labels},
            "spec": {
                "restartPolicy": "Never",
                "serviceAccountName": pod.get("serviceAccountName"),
                "securityContext": pod.get("securityContext", {}),
                "containers": [{
                    "name": "suite", "image": args.image, "workingDir": "/app",
                    "command": ["python", "-u", "/input/scale-suite.py"],
                    "env": container.get("env", []) + [{"name": "ENGINE_DIGEST", "value": engine_digest}],
                    "envFrom": container.get("envFrom", []),
                    "resources": {"requests": {"cpu": "100m", "memory": "256Mi"}, "limits": {"cpu": "500m", "memory": "1Gi"}},
                    "volumeMounts": [m for m in container.get("volumeMounts", []) if "retirement" not in m["name"]]
                                    + [{"name": "input", "mountPath": "/input", "readOnly": True}],
                    "securityContext": container.get("securityContext", {}),
                }],
                "volumes": [v for v in pod.get("volumes", []) if "retirement" not in v["name"]]
                           + [{"name": "input", "configMap": {"name": "kaveon-scale-suite-input"}}],
            }}}}
    print(json.dumps(job, indent=1))


if __name__ == "__main__":
    main()
