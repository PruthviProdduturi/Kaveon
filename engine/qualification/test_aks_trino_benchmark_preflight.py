import hashlib
import json
import unittest

from aks_trino_benchmark_preflight import evaluate
from same_files import EXTENDED_QUERIES


def sts(name, replicas, image, resources):
    return {"metadata": {"name": name}, "spec": {"replicas": replicas, "template": {"spec": {"containers": [
        {"name": "engine", "image": image, "resources": resources}]}}}}


class PreflightTests(unittest.TestCase):
    def test_matched_parked_deployment_is_ready(self):
        digest = "sha256:" + "a" * 64
        kimage, timage = "acr/engine@" + digest, "trinodb/trino@sha256:" + "b" * 64
        coord = {"requests": {"cpu": "500m", "memory": "1Gi"}, "limits": {"cpu": "2", "memory": "4Gi"}}
        worker = {"requests": {"cpu": "1", "memory": "2Gi"}, "limits": {"cpu": "3", "memory": "6Gi"}}
        snapshot = {"azure_cli": True, "dependencies_present": True, "account": {"id": "sub"}, "cluster": {"powerState": {"code": "Running"},
            "agentPoolProfiles": [{"mode": "System", "name": "system", "count": 1, "vmSize": "D4"},
                {"mode": "User", "name": "workers", "count": 3, "vmSize": "D4", "nodeLabels": {"workload": "kaveon-worker"}}]},
            "statefulsets": {"items": [sts("kaveon-coordinator", 1, kimage, coord), sts("kaveon-worker", 3, kimage, worker),
                sts("kaveon-benchmark-trino-coordinator", 0, timage, coord), sts("kaveon-benchmark-trino-worker", 0, timage, worker)]}}
        objects = [{"path": name, "sha256": "c" * 64, "content_md5": "bWQ1", "bytes": 1, "parquet_data": name.endswith("parquet")}
                   for name in ("events/data.parquet", "events/log", "customers/data.parquet", "customers/log")]
        manifest = {"dataset": {"rows": 5_000_000, "customers": 100_000, "objects": objects},
            "query_corpus": {"sha256": hashlib.sha256(json.dumps(EXTENDED_QUERIES, sort_keys=True, separators=(",", ":")).encode()).hexdigest(),
                             "queries": {name: {} for name in EXTENDED_QUERIES}}}
        self.assertTrue(evaluate(snapshot, manifest, "sub", digest, "kaveon-benchmark")["ready"])


if __name__ == "__main__":
    unittest.main()
