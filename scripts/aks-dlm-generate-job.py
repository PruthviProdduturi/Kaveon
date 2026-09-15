"""Rebuild one dataset's DLM context on AKS as a Job, without rolling the API.

The Job copies the live kaveon-api Deployment's pod contract (service account,
environment, secrets, CA mounts, security context) and runs generate_dlm with
force=true in the given image — the image under test — so a new precompute
can be qualified on the cluster's data before it is ever deployed. Prints the
manifest to stdout; apply it with kubectl.

  python scripts/aks-dlm-generate-job.py --dataset 24 --image <repo>@sha256:... | kubectl apply -f -
"""
import argparse
import json
import subprocess

SCRIPT = """import json, time, logging, sys
logging.basicConfig(level=logging.WARNING, format='%(asctime)s %(levelname)s %(message)s')
sys.path.insert(0, '/app')
from dlm import engine
t = time.time()
r = engine.generate_dlm(DATASET, force=True, actor='kaveon-system')
import database.metadata as m
row = m.query_one("SELECT stats_rollup FROM dlm_artifact WHERE dataset_id = @param0", [DATASET]) or {}
g = json.loads(row.get('stats_rollup') or '{}').get('generation', {})
groups = m.query("SELECT group_col, count(*) AS n FROM dlm_answers WHERE dataset_id = @param0 GROUP BY group_col ORDER BY group_col", [DATASET])
keys = [x['group_col'] for x in groups.get('rows', [])]
print('RESULT', json.dumps({k: r.get(k) for k in ('ok', 'status', 'rebuilt')}), json.dumps({
    'answers_precomputed': g.get('answers_precomputed'), 'duration_ms': g.get('duration_ms'),
    'skipped_breakdowns': g.get('skipped_breakdowns'), 'groups': len(keys),
    'pairs': sum(1 for k in keys if '|' in k)}), round(time.time() - t, 1), 's', flush=True)
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dataset", required=True)
    parser.add_argument("--image", required=True, help="immutable image reference to run the build in")
    parser.add_argument("--namespace", default="kaveon")
    args = parser.parse_args()
    live = json.loads(subprocess.check_output(
        ["kubectl", "get", "deploy", "kaveon-api", "-n", args.namespace, "-o", "json"]))
    pod = live["spec"]["template"]["spec"]
    container = pod["containers"][0]
    labels = dict(live["spec"]["template"]["metadata"].get("labels", {}))
    labels.update({"app": "kaveon-api", "kaveon.io/job": "dlm-generate"})
    job = {
        "apiVersion": "batch/v1", "kind": "Job",
        "metadata": {"name": f"kaveon-dlm-generate-{args.dataset}", "namespace": args.namespace,
                     "labels": {"app": "kaveon-dlm-generate"}},
        "spec": {"backoffLimit": 0, "ttlSecondsAfterFinished": 86400, "template": {
            "metadata": {"labels": labels},
            "spec": {
                "restartPolicy": "Never",
                "serviceAccountName": pod.get("serviceAccountName"),
                "securityContext": pod.get("securityContext", {}),
                "containers": [{
                    "name": "generate", "image": args.image, "workingDir": "/app",
                    "command": ["python", "-u", "-c", SCRIPT.replace("DATASET", json.dumps(str(args.dataset)))],
                    "env": container.get("env", []), "envFrom": container.get("envFrom", []),
                    "resources": {"requests": {"cpu": "250m", "memory": "512Mi"},
                                  "limits": {"cpu": "1", "memory": "2Gi"}},
                    # the retirement evidence claim is the Deployment's, single-writer
                    "volumeMounts": [m for m in container.get("volumeMounts", []) if "retirement" not in m["name"]],
                    "securityContext": container.get("securityContext", {}),
                }],
                "volumes": [v for v in pod.get("volumes", []) if "retirement" not in v["name"]],
            }}}}
    print(json.dumps(job, indent=1))


if __name__ == "__main__":
    main()
