"""Run an exclusive, resource-matched three-worker Kaveon/Trino AKS comparison."""

import argparse
import base64
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import ssl
import statistics
import time
from urllib.error import HTTPError
from urllib.parse import quote, urlencode, urljoin
from urllib.request import Request, urlopen


def utcnow():
    return datetime.now(timezone.utc).isoformat()


def canonical_hash(rows):
    return hashlib.sha256(json.dumps(rows, separators=(",", ":")).encode()).hexdigest()


def http(method, url, headers=None, body=None, context=None, timeout=180):
    payload = None if body is None else (body if isinstance(body, bytes) else json.dumps(body).encode())
    request = Request(url, data=payload, headers=headers or {}, method=method)
    try:
        with urlopen(request, context=context, timeout=timeout) as response:
            raw = response.read()
            return response.status, dict(response.headers.items()), json.loads(raw) if raw else None
    except HTTPError as error:
        raw = error.read()
        detail = raw.decode(errors="replace")[:4000]
        raise RuntimeError(f"{method} {url} returned HTTP {error.code}: {detail}") from error


class Kubernetes:
    def __init__(self, namespace):
        self.namespace = namespace
        self.base = "https://kubernetes.default.svc"
        self.token = Path("/var/run/secrets/kubernetes.io/serviceaccount/token").read_text().strip()
        self.context = ssl.create_default_context(cafile="/var/run/secrets/kubernetes.io/serviceaccount/ca.crt")

    def request(self, method, path, body=None):
        headers = {"Authorization": "Bearer " + self.token, "Accept": "application/json"}
        if body is not None:
            headers["Content-Type"] = "application/merge-patch+json"
        return http(method, self.base + path, headers, body, self.context)[2]

    def statefulset(self, name):
        return self.request("GET", f"/apis/apps/v1/namespaces/{self.namespace}/statefulsets/{name}")

    def scale(self, name, replicas):
        self.request("PATCH", f"/apis/apps/v1/namespaces/{self.namespace}/statefulsets/{name}/scale",
                     {"spec": {"replicas": replicas}})

    def wait_ready(self, name, replicas, timeout=600):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            item = self.statefulset(name)
            status = item.get("status") or {}
            if replicas == 0:
                if status.get("replicas", 0) == 0:
                    return item
            elif status.get("readyReplicas", 0) == replicas and status.get("currentReplicas", 0) == replicas:
                return item
            time.sleep(3)
        raise TimeoutError(f"StatefulSet {name} did not reach {replicas} ready replicas")

    def wait_pods_gone(self, statefulset_name, timeout=300):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            found = [pod for pod in self.pods() if pod["metadata"].get("namespace") == self.namespace
                     and any(owner.get("kind") == "StatefulSet" and owner.get("name") == statefulset_name
                             for owner in pod["metadata"].get("ownerReferences", []))]
            if not found:
                return
            time.sleep(3)
        raise TimeoutError(f"StatefulSet {statefulset_name} pods did not terminate")

    def pods(self):
        return self.request("GET", "/api/v1/pods").get("items", [])

    def nodes(self):
        return self.request("GET", "/api/v1/nodes").get("items", [])


def pod_resources(sts):
    container = sts["spec"]["template"]["spec"]["containers"][0]
    return container.get("resources") or {}


def image(sts):
    return sts["spec"]["template"]["spec"]["containers"][0]["image"]


def desired(sts):
    return sts.get("spec", {}).get("replicas", 0)


def validate_topology(kube, names, worker_count, expected_kaveon_digest):
    objects = {key: kube.statefulset(name) for key, name in names.items()}
    checks = {
        "initial_kaveon_topology": desired(objects["kaveon_coordinator"]) == 1 and desired(objects["kaveon_worker"]) == worker_count,
        "initial_trino_scaled_to_zero": desired(objects["trino_coordinator"]) == 0 and desired(objects["trino_worker"]) == 0,
        "coordinator_resources_matched": pod_resources(objects["kaveon_coordinator"]) == pod_resources(objects["trino_coordinator"]),
        "worker_resources_matched": pod_resources(objects["kaveon_worker"]) == pod_resources(objects["trino_worker"]),
        "kaveon_image_pinned_and_expected": image(objects["kaveon_coordinator"]).endswith("@" + expected_kaveon_digest)
            and image(objects["kaveon_worker"]).endswith("@" + expected_kaveon_digest),
        "trino_image_pinned": "@sha256:" in image(objects["trino_coordinator"])
            and image(objects["trino_coordinator"]) == image(objects["trino_worker"]),
    }
    nodes = kube.nodes()
    system = [node for node in nodes if node["metadata"].get("labels", {}).get("kubernetes.azure.com/agentpool") == "system"]
    workers = [node for node in nodes if node["metadata"].get("labels", {}).get("workload") == "kaveon-worker"]
    skus = {node["metadata"].get("labels", {}).get("node.kubernetes.io/instance-type") for node in system + workers}
    checks["one_system_three_worker_nodes"] = len(system) == 1 and len(workers) == worker_count
    checks["one_node_sku"] = len(skus) == 1 and None not in skus
    failed = [name for name, passed in checks.items() if not passed]
    if failed:
        raise RuntimeError("topology preflight failed: " + ", ".join(failed))
    node_details = [{"name": node["metadata"]["name"], "instance_type": node["metadata"].get("labels", {}).get("node.kubernetes.io/instance-type"),
                     "os_image": node.get("status", {}).get("nodeInfo", {}).get("osImage"),
                     "kernel": node.get("status", {}).get("nodeInfo", {}).get("kernelVersion"),
                     "container_runtime": node.get("status", {}).get("nodeInfo", {}).get("containerRuntimeVersion"),
                     "kubelet": node.get("status", {}).get("nodeInfo", {}).get("kubeletVersion")} for node in system + workers]
    return {"checks": checks, "node_skus": sorted(skus), "nodes": node_details,
            "resources": {key: pod_resources(value) for key, value in objects.items()},
            "images": {key: image(value) for key, value in objects.items()}}


def active_worker_nodes(kube, statefulset_name, worker_count):
    pods = [pod for pod in kube.pods() if pod["metadata"].get("namespace") == kube.namespace
            and any(owner.get("kind") == "StatefulSet" and owner.get("name") == statefulset_name
                    for owner in pod["metadata"].get("ownerReferences", []))]
    ready = [pod for pod in pods if pod.get("status", {}).get("phase") == "Running"
             and any(condition.get("type") == "Ready" and condition.get("status") == "True"
                     for condition in pod.get("status", {}).get("conditions", []))]
    nodes = [pod.get("spec", {}).get("nodeName") for pod in ready]
    if len(ready) != worker_count or len(set(nodes)) != worker_count or None in nodes:
        raise RuntimeError(f"{statefulset_name} workers are not Ready on {worker_count} distinct nodes")
    allowed = {pod["metadata"]["uid"] for pod in ready}
    contenders = []
    for pod in kube.pods():
        if pod.get("spec", {}).get("nodeName") not in nodes or pod["metadata"].get("uid") in allowed:
            continue
        owners = pod["metadata"].get("ownerReferences", [])
        phase = pod.get("status", {}).get("phase")
        if phase in {"Succeeded", "Failed"} or any(owner.get("kind") == "DaemonSet" for owner in owners):
            continue
        contenders.append(
            f"{pod.get('spec', {}).get('nodeName')}/{pod['metadata'].get('namespace')}/{pod['metadata'].get('name')}"
        )
    image_ids = sorted({status.get("imageID") for pod in ready for status in pod.get("status", {}).get("containerStatuses", [])
                        if status.get("imageID")})
    if not image_ids or not all("sha256:" in value for value in image_ids):
        raise RuntimeError(f"{statefulset_name} runtime image digest was not reported")
    return {"nodes": sorted(nodes), "worker_image_ids": image_ids, "co_tenants": sorted(contenders)}


def workload_identity_token():
    tenant = os.environ["AZURE_TENANT_ID"]
    client = os.environ["AZURE_CLIENT_ID"]
    assertion = Path(os.environ["AZURE_FEDERATED_TOKEN_FILE"]).read_text().strip()
    form = urlencode({"client_id": client, "scope": "https://storage.azure.com/.default",
                      "client_assertion": assertion, "client_assertion_type": "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
                      "grant_type": "client_credentials"}).encode()
    return http("POST", f"https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token",
                {"Content-Type": "application/x-www-form-urlencoded"}, form)[2]["access_token"]


def verify_blobs(manifest):
    dataset = manifest["dataset"]
    token = workload_identity_token()
    verified = []
    for item in dataset["objects"]:
        path = quote(item["path"], safe="/")
        url = f"https://{dataset['account']}.blob.core.windows.net/{dataset['container']}/{path}"
        status, headers, _ = http("HEAD", url, {"Authorization": "Bearer " + token, "x-ms-version": "2023-11-03"})
        normalized = {key.lower(): value for key, value in headers.items()}
        if (status != 200 or normalized.get("x-ms-meta-sha256") != item["sha256"]
                or normalized.get("content-md5") != item["content_md5"] or int(normalized["content-length"]) != item["bytes"]):
            raise RuntimeError(f"blob identity mismatch for {item['path']}")
        verified.append({"path": item["path"], "sha256": item["sha256"], "content_md5": item["content_md5"], "bytes": item["bytes"],
                         "etag": normalized.get("etag"), "last_modified": normalized.get("last-modified")})
    return verified


class Engines:
    def __init__(self, manifest):
        self.manifest = manifest
        self.kaveon_url = os.environ["KAVEON_URL"].rstrip("/")
        self.trino_url = os.environ["TRINO_URL"].rstrip("/")
        security = json.loads(Path("/auth/security.json").read_text())
        self.kaveon_token = security["principals"][0]["token"]
        self.catalog_token = Path("/auth/catalog-token").read_text().strip()
        self.kaveon_ssl = ssl.create_default_context(cafile="/tls/ca.crt")
        self.trino_ssl = ssl.create_default_context(cafile="/trino-tls/ca.crt")
        trino_password = Path("/trino-auth/client-password").read_text().strip()
        self.trino_authorization = "Basic " + base64.b64encode(("qualification:" + trino_password).encode()).decode()
        suffix = manifest["manifest_payload_sha256"][:12]
        self.catalog_id, self.schema_id = "bench-" + suffix, "bench-schema-" + suffix
        self.catalog, self.schema = "bench_" + suffix, "data"

    def krequest(self, method, path, body=None, catalog=False):
        token = self.catalog_token if catalog else self.kaveon_token
        headers = {"Authorization": "Bearer " + token, "Content-Type": "application/json"}
        if catalog:
            headers["x-kaveon-actor"] = "aks-benchmark-runner"
        url = path if path.startswith("http://") or path.startswith("https://") else self.kaveon_url + path
        return http(method, url, headers, body, self.kaveon_ssl)[2]

    def kaveon_query(self, sql):
        result = self.krequest("POST", "/v1/statement", {"query": sql, "catalog": self.catalog,
                               "schema": self.schema, "result_delivery": "paged"})
        if result.get("error") or result.get("state") != "FINISHED":
            raise RuntimeError(str(result))
        rows = result.get("data") or []
        next_uri = result.get("next_uri")
        while next_uri:
            page = self.krequest("GET", next_uri)
            rows.extend(page.get("data") or [])
            next_uri = page.get("next_uri")
        self.krequest("DELETE", "/v1/query/" + result["id"])
        return rows

    def trino_query(self, sql):
        headers = {"X-Trino-User": "qualification", "X-Trino-Catalog": "lake", "X-Trino-Schema": self.catalog,
                   "Content-Type": "text/plain; charset=utf-8", "Authorization": self.trino_authorization}
        _, _, page = http("POST", self.trino_url + "/v1/statement", headers, sql.encode(), self.trino_ssl)
        rows = []
        while True:
            if page.get("error"):
                raise RuntimeError(str(page["error"]))
            rows.extend(page.get("data") or [])
            if not page.get("nextUri"):
                return rows
            _, _, page = http("GET", page["nextUri"], headers, context=self.trino_ssl)

    def bootstrap_kaveon(self):
        dataset = self.manifest["dataset"]
        catalog = {"id": self.catalog_id, "name": self.catalog, "revision": 1, "adapter": "Native",
                   "storage": {"AdlsGen2": {"account": dataset["account"], "container": dataset["container"],
                                              "root_path": dataset["prefix"]}},
                   "credential": {"kind": "WorkloadIdentity", "reference": os.environ["KAVEON_WORKLOAD_IDENTITY_REFERENCE"]},
                   "lifecycle": "Draft"}
        schema = {"id": self.schema_id, "catalog_id": self.catalog_id, "name": self.schema, "revision": 1, "lifecycle": "Draft"}
        self._ensure_kaveon("/v1/catalog/definitions", f"/v1/catalog/definitions/{self.catalog_id}", catalog)
        self._ensure_kaveon(f"/v1/catalog/definitions/{self.catalog_id}/schemas", f"/v1/catalog/schemas/{self.schema_id}", schema)
        columns = {
            "events": ["event_id", "customer_id", "category", "amount"],
            "customers": ["customer_id"],
        }
        for table, names in columns.items():
            table_id = f"{self.catalog_id}-{table}"
            value = {"id": table_id, "schema_id": self.schema_id, "name": table, "revision": 1,
                     "location": f"{table}/data.parquet", "access": "Shortcut", "format": "Parquet",
                     "columns": [{"name": name, "data_type": "Int64", "nullable": True} for name in names], "lifecycle": "Draft"}
            self._ensure_kaveon(f"/v1/catalog/schemas/{self.schema_id}/tables", f"/v1/catalog/tables/{table_id}", value)

    def _ensure_kaveon(self, collection, item, value):
        try:
            self.krequest("POST", collection, value, catalog=True)
            active = dict(value, revision=2, lifecycle="Active")
            headers = {"Authorization": "Bearer " + self.catalog_token, "Content-Type": "application/json",
                       "x-kaveon-actor": "aks-benchmark-runner", "If-Match": "1"}
            http("PUT", self.kaveon_url + item, headers, active, self.kaveon_ssl)
        except RuntimeError as error:
            if "HTTP 409" not in str(error):
                raise
            existing = self.krequest("GET", item)
            if existing.get("name") != value.get("name") or existing.get("lifecycle") != "Active":
                raise RuntimeError(f"existing Kaveon catalog object disagrees at {item}") from error

    def bootstrap_trino(self):
        dataset = self.manifest["dataset"]
        self.trino_query(f"CREATE SCHEMA IF NOT EXISTS lake.{self.catalog}")
        root = f"abfss://{dataset['container']}@{dataset['account']}.dfs.core.windows.net/{dataset['prefix']}"
        for table in ("events", "customers"):
            self.trino_query(
                "CALL lake.system.register_table("
                f"schema_name => '{self.catalog}', table_name => '{table}', "
                f"table_location => '{root}/{table}')"
            )

    def wait_workers(self, engine, count, timeout=300):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                if engine == "kaveon":
                    if self.krequest("GET", "/v1/cluster").get("active_workers") == count:
                        return
                elif self.trino_query("SELECT COUNT(*) FROM system.runtime.nodes WHERE state = 'active'") == [[count + 1]]:
                    return
            except Exception:
                pass
            time.sleep(3)
        raise TimeoutError(f"{engine} did not report {count} active workers")

    def query(self, engine, sql):
        return self.kaveon_query(sql) if engine == "kaveon" else self.trino_query(sql)

    def unauthenticated_rejected(self, engine):
        try:
            if engine == "kaveon":
                http("POST", self.kaveon_url + "/v1/statement", {"Content-Type": "application/json"},
                     {"query": "SELECT 1"}, self.kaveon_ssl)
            else:
                headers = {"X-Trino-User": "qualification", "Content-Type": "text/plain; charset=utf-8"}
                http("POST", self.trino_url + "/v1/statement", headers, b"SELECT 1", self.trino_ssl)
        except RuntimeError as error:
            return "HTTP 401" in str(error)
        return False


def percentile(samples, fraction):
    ordered = sorted(samples)
    return ordered[math.ceil(len(ordered) * fraction) - 1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text())
    policy = {"worker_count": int(os.environ["WORKER_COUNT"]), "warmups": int(os.environ["WARMUPS"]),
              "repetitions_per_round": int(os.environ["REPETITIONS_PER_ROUND"]),
              "rounds": int(os.environ["THROUGHPUT_ROUNDS"]), "throughput_repeats": int(os.environ["THROUGHPUT_REPEATS"]),
              "concurrency": int(os.environ["CONCURRENCY"]), "target_ratio": float(os.environ["TARGET_RATIO"])}
    if policy != {"worker_count": 3, "warmups": 5, "repetitions_per_round": 5, "rounds": 6,
                  "throughput_repeats": 10, "concurrency": 4, "target_ratio": 1.9}:
        raise RuntimeError("publication runner policy must remain 3 workers, 5 warmups, 30 samples, 6 rounds, 10 repeats, concurrency 4, target 1.90")
    queries = manifest["query_corpus"]["queries"]
    if len(queries) != 12 or manifest["dataset"]["rows"] < 5_000_000 or manifest["dataset"]["customers"] < 100_000:
        raise RuntimeError("manifest does not meet the frozen distributed publication workload")
    namespace = os.environ["BENCHMARK_NAMESPACE"]
    kube = Kubernetes(namespace)
    names = {"kaveon_coordinator": os.environ["KAVEON_COORDINATOR_STATEFULSET"],
             "kaveon_worker": os.environ["KAVEON_WORKER_STATEFULSET"],
             "trino_coordinator": os.environ["TRINO_COORDINATOR_STATEFULSET"],
             "trino_worker": os.environ["TRINO_WORKER_STATEFULSET"]}
    original = {key: desired(kube.statefulset(name)) for key, name in names.items()}
    report = {"schema_version": 1, "started_at": utcnow(), "suite": "extended", "environment": "AKS distributed matched co-tenant lease",
              "workers": 3, "warmups_per_activation": 5, "policy": policy, "manifest": manifest,
              "cases": {name: {"name": name, "sql": case["sql"], "result_sha256": case["result_sha256"],
                               "kaveon_ms": [], "trino_ms": [], "passed": True} for name, case in queries.items()},
              "throughput": {"kaveon": [], "trino": [], "passed": True},
              "co_tenant_baseline": None, "co_tenant_observations": [],
              "security_boundaries": {"kaveon": False, "trino": False}, "claim_evidence": False}
    try:
        report["preflight"] = validate_topology(kube, names, policy["worker_count"], os.environ["KAVEON_EXPECTED_IMAGE_DIGEST"])
        report["verified_blobs"] = verify_blobs(manifest)
        engines = Engines(manifest)

        def deactivate(engine):
            kube.scale(names[f"{engine}_worker"], 0)
            kube.scale(names[f"{engine}_coordinator"], 0)
            kube.wait_ready(names[f"{engine}_worker"], 0)
            kube.wait_ready(names[f"{engine}_coordinator"], 0)
            kube.wait_pods_gone(names[f"{engine}_worker"])
            kube.wait_pods_gone(names[f"{engine}_coordinator"])

        def activate(engine):
            other = "trino" if engine == "kaveon" else "kaveon"
            deactivate(other)
            kube.scale(names[f"{engine}_coordinator"], 1)
            kube.wait_ready(names[f"{engine}_coordinator"], 1)
            kube.scale(names[f"{engine}_worker"], policy["worker_count"])
            kube.wait_ready(names[f"{engine}_worker"], policy["worker_count"])
            engines.bootstrap_kaveon() if engine == "kaveon" else engines.bootstrap_trino()
            engines.wait_workers(engine, policy["worker_count"])
            if not engines.unauthenticated_rejected(engine):
                raise RuntimeError(f"{engine} accepted an unauthenticated statement")
            report["security_boundaries"][engine] = True
            runtime = active_worker_nodes(kube, names[f"{engine}_worker"], policy["worker_count"])
            if report["co_tenant_baseline"] is None:
                report["co_tenant_baseline"] = runtime["co_tenants"]
            elif runtime["co_tenants"] != report["co_tenant_baseline"]:
                raise RuntimeError(f"worker-node co-tenant topology changed before {engine} phase")
            report["co_tenant_observations"].append(
                {"engine": engine, "worker_nodes": runtime["nodes"], "co_tenants": runtime["co_tenants"]}
            )
            return runtime

        items = list(queries.items())
        for round_index in range(policy["rounds"]):
            order = ["trino", "kaveon"] if round_index % 2 == 0 else ["kaveon", "trino"]
            for engine in order:
                runtime = activate(engine)
                for _ in range(policy["warmups"]):
                    for name, case in items:
                        actual = engines.query(engine, case["sql"])
                        if len(actual) != case["result_rows"] or canonical_hash(actual) != case["result_sha256"]:
                            raise RuntimeError(f"{engine} warmup result mismatch for {name}")
                for name, case in items:
                    for _ in range(policy["repetitions_per_round"]):
                        started = time.perf_counter()
                        actual = engines.query(engine, case["sql"])
                        elapsed = (time.perf_counter() - started) * 1000
                        if len(actual) != case["result_rows"] or canonical_hash(actual) != case["result_sha256"]:
                            report["cases"][name]["passed"] = False
                            raise RuntimeError(f"{engine} exact result mismatch for {name}")
                        report["cases"][name][engine + "_ms"].append(elapsed)
                rotated = items[round_index % len(items):] + items[:round_index % len(items)]
                workload = rotated * policy["throughput_repeats"]

                def checked(item):
                    name, case = item
                    started = time.perf_counter()
                    try:
                        actual = engines.query(engine, case["sql"])
                        passed = len(actual) == case["result_rows"] and canonical_hash(actual) == case["result_sha256"]
                        return {"name": name, "passed": passed, "ms": (time.perf_counter() - started) * 1000}
                    except Exception as error:
                        return {"name": name, "passed": False, "ms": (time.perf_counter() - started) * 1000, "error": str(error)}

                started = time.perf_counter()
                with ThreadPoolExecutor(max_workers=policy["concurrency"]) as executor:
                    results = list(executor.map(checked, workload))
                seconds = time.perf_counter() - started
                passed = all(item["passed"] for item in results)
                report["throughput"]["passed"] &= passed
                report["throughput"][engine].append({"round": round_index + 1, "order": order, "seconds": seconds,
                                                       "successful_qps": sum(item["passed"] for item in results) / seconds,
                                                       "worker_nodes": runtime["nodes"], "worker_image_ids": runtime["worker_image_ids"],
                                                       "results": results})
                if not passed:
                    raise RuntimeError(f"{engine} throughput result mismatch in round {round_index + 1}")
                deactivate(engine)
        report["cases"] = list(report["cases"].values())
        for case in report["cases"]:
            for engine in ("kaveon", "trino"):
                samples = case[engine + "_ms"]
                case.setdefault("statistics", {})[engine] = {"min_ms": min(samples), "median_ms": statistics.median(samples),
                                                              "p95_ms": percentile(samples, 0.95), "max_ms": max(samples)}
        throughput = report["throughput"]
        throughput["aggregate_qps"] = {
            engine: sum(sum(result["passed"] for result in sample["results"]) for sample in throughput[engine])
            / sum(item["seconds"] for item in throughput[engine])
            for engine in ("kaveon", "trino")
        }
        throughput["kaveon_over_trino"] = throughput["aggregate_qps"]["kaveon"] / throughput["aggregate_qps"]["trino"]
        throughput["paired_round_ratios"] = [
            candidate["successful_qps"] / reference["successful_qps"] if reference["successful_qps"] else None
            for candidate, reference in zip(throughput["kaveon"], throughput["trino"], strict=True)
        ]
        throughput["target_met"] = throughput["passed"] and throughput["kaveon_over_trino"] >= policy["target_ratio"]
        report["passed"] = all(case["passed"] for case in report["cases"]) and throughput["passed"]
    except Exception as error:
        report["passed"] = False
        report["error"] = f"{type(error).__name__}: {error}"
    finally:
        restoration_errors = []
        if isinstance(report.get("cases"), dict):
            report["cases"] = list(report["cases"].values())
        for key in ("trino_worker", "trino_coordinator"):
            try:
                kube.scale(names[key], original[key])
                kube.wait_ready(names[key], original[key])
            except Exception as error:
                restoration_errors.append(f"{key}: {type(error).__name__}: {error}")
        for key in ("kaveon_coordinator", "kaveon_worker"):
            try:
                kube.scale(names[key], original[key])
                kube.wait_ready(names[key], original[key])
            except Exception as error:
                restoration_errors.append(f"{key}: {type(error).__name__}: {error}")
        report["restoration"] = {"requested_replicas": original, "errors": restoration_errors}
        report["finished_at"] = utcnow()
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
    print("REPORT_JSON " + json.dumps(report, separators=(",", ":")))
    print(f"passed={str(report['passed']).lower()}; output={args.output}; restored={str(not report['restoration']['errors']).lower()}")
    return 0 if report["passed"] and not report["restoration"]["errors"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
