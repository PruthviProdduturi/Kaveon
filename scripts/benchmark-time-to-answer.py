"""Time to answer: the product corpus on Kaveon versus the same SQL on Trino.

Runs inside the cluster during a leased benchmark window, after the Trino
StatefulSets are up and the read-only `opensource` Hive catalog is enabled in
`infra/helm/kaveon-trino-benchmark`. Input is a corpus report written by
`scripts/qualify-dlm-questions.py` (every question's route, seconds and SQL as
served through the portal). For each question that produced SQL, the same
statement is executed on Trino against external tables declared from
`infra/aks/opensource-catalog-manifest.json`; the DLM's own seconds are what
the portal measured. Output is one JSON record per question with both sides,
never any row data beyond the count.

Environment: TRINO_URL, /trino-tls/ca.crt, /trino-auth/client-password,
TTA_REPORT (corpus report path), TTA_MANIFEST (catalog manifest path),
TTA_ACCOUNT (storage account), TTA_OUTPUT (where to write the result).
"""
import base64
import json
import os
import ssl
import statistics
import time
import urllib.request
import urllib.parse
from pathlib import Path

TYPES = {"Utf8": "VARCHAR", "Int64": "BIGINT", "Float64": "DOUBLE", "Boolean": "BOOLEAN"}
REPETITIONS = 3


def validated_next_uri(base_url, candidate):
    """Keep Trino credentials on the configured origin and statement path."""
    base, target = urllib.parse.urlsplit(base_url), urllib.parse.urlsplit(str(candidate or ""))
    if (target.scheme, target.hostname, target.port) != (base.scheme, base.hostname, base.port) \
            or target.username or target.password or target.fragment \
            or not target.path.startswith("/v1/statement/"):
        raise RuntimeError("Trino returned an unsafe next URI")
    return target.geturl()


class Trino:
    def __init__(self):
        self.url = os.environ["TRINO_URL"].rstrip("/")
        self.ssl = ssl.create_default_context(cafile="/trino-tls/ca.crt")
        password = Path("/trino-auth/client-password").read_text().strip()
        self.auth = "Basic " + base64.b64encode(("qualification:" + password).encode()).decode()

    def _fetch(self, method, url, body=None):
        headers = {"X-Trino-User": "qualification", "X-Trino-Catalog": "opensource",
                   "Content-Type": "text/plain; charset=utf-8", "Authorization": self.auth}
        request = urllib.request.Request(url, data=body, method=method, headers=headers)
        with urllib.request.urlopen(request, context=self.ssl, timeout=600) as response:
            return json.loads(response.read().decode())

    def query(self, sql):
        page = self._fetch("POST", self.url + "/v1/statement", sql.encode())
        rows = []
        while True:
            if page.get("error"):
                raise RuntimeError(page["error"].get("message", "Trino error"))
            rows.extend(page.get("data") or [])
            if not page.get("nextUri"):
                return rows
            page = self._fetch("GET", validated_next_uri(self.url, page["nextUri"]))


def declare_tables(trino, manifest, account):
    root = f"abfs://opensource@{account}.dfs.core.windows.net/{manifest['root_path']}"
    declared = 0
    for schema in sorted({t["schema"] for t in manifest["tables"]}):
        trino.query(f"CREATE SCHEMA IF NOT EXISTS opensource.{schema} WITH (location = '{root}/{schema}/')")
    for table in manifest["tables"]:
        columns = ", ".join(f"{c['name']} {TYPES[c['data_type']]}" for c in table["columns"])
        location = table["location"]
        directory = location if location.endswith("/") else location.rsplit("/", 1)[0] + "/"
        trino.query(f"CREATE TABLE IF NOT EXISTS opensource.{table['schema']}.{table['name']} ({columns}) "
                    f"WITH (external_location = '{root}/{directory}', format = 'PARQUET')")
        declared += 1
    return declared


def main():
    trino = Trino()
    report = json.loads(Path(os.environ["TTA_REPORT"]).read_text(encoding="utf-8"))
    manifest = json.loads(Path(os.environ["TTA_MANIFEST"]).read_text(encoding="utf-8"))
    declared = declare_tables(trino, manifest, os.environ["TTA_ACCOUNT"])
    records = []
    for result in report["results"]:
        sql = result.get("sql")
        if not result.get("ok") or not sql:
            continue
        record = {"id": result["id"], "question": result["question"], "dlm_route": result["route"],
                  "dlm_seconds": result["seconds"], "kaveon_live_seconds": result.get("live_seconds"),
                  "trino_seconds": [], "trino_rows": None, "trino_error": None}
        try:
            trino.query(sql)                      # warm-up; never counted
            for _ in range(REPETITIONS):
                t0 = time.time()
                rows = trino.query(sql)
                record["trino_seconds"].append(round(time.time() - t0, 3))
            record["trino_rows"] = len(rows)
            record["trino_median_seconds"] = round(statistics.median(record["trino_seconds"]), 3)
        except Exception as exc:
            record["trino_error"] = str(exc)[:200]
        records.append(record)
        print(json.dumps({k: v for k, v in record.items() if k != "question"}), flush=True)
    output = {"declared_tables": declared, "repetitions": REPETITIONS, "records": records}
    Path(os.environ["TTA_OUTPUT"]).write_text(json.dumps(output, indent=2) + "\n", encoding="utf-8")
    print(f"TIME_TO_ANSWER={len(records)} questions; output {os.environ['TTA_OUTPUT']}")


if __name__ == "__main__":
    main()
