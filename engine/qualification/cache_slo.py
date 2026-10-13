"""Fail-closed qualification for the coordinator analytical result cache.

This is deliberately a deployment harness, not a unit-test substitute.  It
compares an uncached result with a cold cached result and repeated warm hits,
then checks the cache counters and administrator clear endpoint.  A restart
miss must be supplied by the deployment operator as a separate evidence file;
the harness never pretends that a process it did not restart was restarted.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import time

import requests


def digest(columns, rows):
    payload = json.dumps({"columns": columns, "rows": rows}, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(payload.encode()).hexdigest()


def percentile(values, rank):
    """Nearest-rank percentile, with an empty input returning None."""
    if not values:
        return None
    ordered = sorted(values)
    index = max(0, min(len(ordered) - 1, int((rank / 100) * len(ordered) + 0.999999) - 1))
    return ordered[index]


def cache_stats(session, base, headers):
    response = session.get(f"{base}/v1/cluster", headers=headers, timeout=15)
    response.raise_for_status()
    body = response.json()
    coordinator = body.get("coordinator") or {}
    stats = coordinator.get("result_cache")
    if not isinstance(stats, dict):
        raise RuntimeError("/v1/cluster did not expose coordinator.result_cache")
    return stats


def execute(session, base, headers, sql, settings):
    started = time.perf_counter()
    response = session.post(
        f"{base}/v1/statement",
        headers=headers,
        json={"query": sql, "settings": settings},
        timeout=180,
    )
    body = response.json()
    response.raise_for_status()
    if body.get("error") or body.get("state") != "FINISHED":
        raise RuntimeError(f"statement did not finish: {body}")
    columns = body.get("columns") or []
    rows = list(body.get("data") or [])
    next_uri = body.get("next_uri")
    pages = 0
    while next_uri:
        page = session.get(f"{base}{next_uri}", headers=headers, timeout=60)
        page.raise_for_status()
        page_body = page.json()
        rows.extend(page_body.get("data") or [])
        next_uri = page_body.get("next_uri")
        pages += 1
        if pages > 10000:
            raise RuntimeError("result page loop exceeded safety bound")
    return {
        "query_id": body.get("id"),
        "columns": columns,
        "rows": rows,
        "sha256": digest(columns, rows),
        "elapsed_ms": round((time.perf_counter() - started) * 1000, 3),
        "pages": pages,
    }


def clear_cache(session, base, headers):
    response = session.delete(f"{base}/v1/cache", headers=headers, timeout=15)
    response.raise_for_status()
    return response.json()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", default="http://127.0.0.1:8081")
    parser.add_argument("--token", default=os.environ.get("KAVEON_TOKEN"))
    parser.add_argument("--sql", required=True)
    parser.add_argument("--warm-hits", type=int, default=20)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--restart-evidence",
        type=Path,
        help="JSON evidence produced after restarting the coordinator",
    )
    args = parser.parse_args(argv)
    if not args.token:
        parser.error("--token or KAVEON_TOKEN is required")
    if args.warm_hits < 3:
        parser.error("--warm-hits must be at least 3")

    base = args.server.rstrip("/")
    headers = {"Authorization": f"Bearer {args.token}"}
    report = {
        "server": base,
        "sql": args.sql,
        "warm_hits_requested": args.warm_hits,
        "checks": {},
    }
    session = requests.Session()
    try:
        report["clear_before"] = clear_cache(session, base, headers)
        uncached = execute(session, base, headers, args.sql, {"result_cache": False})
        cold = execute(session, base, headers, args.sql, {"result_cache": True})
        warm = [execute(session, base, headers, args.sql, {"result_cache": True}) for _ in range(args.warm_hits)]
        before_bypass = cache_stats(session, base, headers)
        bypass = execute(session, base, headers, args.sql, {"result_cache": False})
        after_bypass = cache_stats(session, base, headers)
        report.update(
            uncached=uncached,
            cold=cold,
            warm=warm,
            bypass=bypass,
            stats_before_bypass=before_bypass,
            stats_after_bypass=after_bypass,
        )

        before_clear = cache_stats(session, base, headers)
        report["stats_before_clear"] = before_clear
        report["clear_after"] = clear_cache(session, base, headers)
        after_clear = cache_stats(session, base, headers)
        report["stats_after_clear"] = after_clear

        warm_latencies = [entry["elapsed_ms"] for entry in warm]
        same = {uncached["sha256"], cold["sha256"], bypass["sha256"]} | {
            entry["sha256"] for entry in warm
        }
        hit_delta = before_clear.get("hits", 0) - report["clear_before"].get("result_cache", {}).get("hits", 0)
        report["warm_latency_ms"] = {
            "p50": percentile(warm_latencies, 50),
            "p95": percentile(warm_latencies, 95),
            "p99": percentile(warm_latencies, 99),
            "max": max(warm_latencies) if warm_latencies else None,
            "mean": statistics.fmean(warm_latencies) if warm_latencies else None,
        }
        report["checks"].update(
            exact_result_parity=len(same) == 1,
            warm_hit_count=hit_delta >= args.warm_hits,
            bypass_did_not_add_hit=after_bypass.get("hits", 0) == before_bypass.get("hits", 0),
            clear_removed_entries=after_clear.get("entries") == 0 and after_clear.get("bytes") == 0,
            engine_hit_latency_metrics={
                key in before_clear
                for key in ("hit_latency_samples", "hit_latency_ns_total", "hit_latency_ns_max")
            }
            == {True},
        )
        if args.restart_evidence:
            restart = json.loads(args.restart_evidence.read_text(encoding="utf-8"))
            report["restart_evidence"] = restart
            report["checks"]["restart_miss"] = bool(restart.get("restart_miss"))
        else:
            report["checks"]["restart_miss"] = False
            report["restart_evidence"] = {"status": "pending", "reason": "coordinator restart not supplied"}
    except Exception as error:
        report["error"] = str(error)
    report["passed"] = not report.get("error") and all(report["checks"].values())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, sort_keys=True), encoding="utf-8")
    print(json.dumps({"passed": report["passed"], "output": str(args.output)}, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
