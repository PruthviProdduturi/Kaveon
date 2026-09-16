"""The scale suite: docs/qualification/scale-suite.json on KaveonDB, alone on
the worker nodes, through the API bridge. One warm-up then three timed
executions per statement, median reported beside the Trino column and the
target. Runs inside the cluster (a Job built from the API pod contract, see
scripts/aks-dlm-generate-job.py for the pattern) so the measurement sits on
the same network as the workers; prints one JSON record per statement and a
final SCALE_SUITE line with the verdict.

Environment: the API's Engine bridge settings; SUITE (path to the suite
JSON, default /input/scale-suite.json); ENGINE_DIGEST (recorded verbatim).
"""
import json
import os
import statistics
import sys
import time

sys.path.insert(0, "/app")
import services.engine_bridge as eb  # noqa: E402


def main():
    suite = json.load(open(os.environ.get("SUITE", "/input/scale-suite.json"), encoding="utf-8"))
    # A suite names its catalog and schema; the telemetry suite predates that.
    catalog = suite.get("catalog", "OpenSource")
    schema = suite.get("schema", "kaveon_product")
    records = []
    for statement in suite["statements"]:
        record = {"id": statement["id"], "seconds": [], "rows": None, "rows_selected": None, "error": None,
                  "trino_seconds": statement.get("trino_seconds"), "target_seconds": statement.get("target_seconds")}
        try:
            eb.execute(statement["sql"], catalog, "scale-suite", "Admin", schema, timeout=900)
            for _ in range(3):
                t0 = time.time()
                result = eb.execute(statement["sql"], catalog, "scale-suite", "Admin", schema, timeout=900)
                record["seconds"].append(round(time.time() - t0, 3))
            record["rows"] = len(result.get("data") or result.get("rows") or [])
            stages = (result.get("query_details") or {}).get("stages") or []
            record["rows_selected"] = sum((t.get("scan") or {}).get("rows_selected", 0) for st in stages for t in st.get("tasks", []))
            record["median_seconds"] = round(statistics.median(record["seconds"]), 3)
            record["meets_target"] = record["median_seconds"] <= float(statement["target_seconds"]) if statement.get("target_seconds") else None
            record["beats_trino"] = (record["median_seconds"] < float(statement["trino_seconds"])) if statement.get("trino_seconds") else None
        except Exception as exc:
            record["error"] = str(getattr(exc, "detail", exc))[:200]
        records.append(record)
        print(json.dumps(record), flush=True)
    measured = [r for r in records if r.get("median_seconds") is not None]
    verdict = {
        "engine_digest": os.environ.get("ENGINE_DIGEST"),
        "statements": len(records), "measured": len(measured),
        "meet_target": sum(1 for r in measured if r.get("meets_target")),
        "beat_trino": sum(1 for r in measured if r.get("beats_trino")),
        "trino_compared": sum(1 for r in measured if r.get("trino_seconds")),
    }
    print("SCALE_SUITE=" + json.dumps({**verdict, "records": records}), flush=True)


if __name__ == "__main__":
    main()
