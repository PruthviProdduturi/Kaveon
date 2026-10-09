#!/usr/bin/env python3
"""Validate and summarize a ClickBench rounds campaign.

The rounds runner records three samples per statement and five alternating
rounds.  This tool turns those records into an evidence artifact without
re-running an engine: p50/p95 are calculated from every recorded sample,
result hashes are checked across all rounds, and rows/failures are retained.

Kaveon detail JSON can be supplied with ``--detail q33=path.json`` (repeat the
option).  Detail records are intentionally optional because the rounds runner
does not retain stage telemetry for every statement.  When present, the tool
totals scan, CPU, exchange, memory, and spill counters, and marks the source
record in the output.  Missing metrics are represented as ``null`` rather
than inferred.

Example::

  python scripts/clickbench-evidence.py \
    docs/qualification/clickbench/runs/rounds-2026-09-17 \
    --detail q33=tmp-q33-current-detail.json \
    --detail q35=tmp-q35-final-detail.json \
    --json docs/qualification/clickbench/runs/rounds-2026-09-17/evidence.json \
    --markdown docs/qualification/clickbench/runs/rounds-2026-09-17/evidence.md

The report is descriptive evidence, not a parity claim.  A ClickHouse result
must be supplied by a separate control and is never silently mixed into the
matched Kaveon/Trino campaign.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import pathlib
import statistics
import sys
from collections import defaultdict
from typing import Any


def percentile(values: list[float], p: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    if len(ordered) == 1:
        return ordered[0]
    position = (len(ordered) - 1) * p
    lower = math.floor(position)
    upper = math.ceil(position)
    if lower == upper:
        return ordered[lower]
    return ordered[lower] + (ordered[upper] - ordered[lower]) * (position - lower)


def load_rounds(directory: pathlib.Path, engine: str) -> list[dict[str, Any]]:
    records = []
    for path in sorted(directory.glob(f"{engine}-round*.json")):
        try:
            round_no = int(path.stem.rsplit("round", 1)[1])
        except (ValueError, IndexError):
            continue
        item = json.loads(path.read_text(encoding="utf-8"))
        item["_path"] = str(path)
        item["_round"] = round_no
        records.append(item)
    return sorted(records, key=lambda item: item["_round"])


def statement_rows(rounds: list[dict[str, Any]]) -> dict[str, list[dict[str, Any]]]:
    result: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for campaign in rounds:
        for row in campaign.get("records", []):
            result[str(row.get("id"))].append({"round": campaign["_round"], **row})
    return dict(result)


def digest_summary(rows: list[dict[str, Any]]) -> dict[str, Any]:
    digests = sorted({str(row["result_sha256"]) for row in rows if row.get("result_sha256")})
    return {"values": digests, "stable": len(digests) <= 1, "missing": not bool(digests)}


def summarize_statement(rows: list[dict[str, Any]]) -> dict[str, Any]:
    samples: list[float] = []
    round_medians: list[float] = []
    errors: list[dict[str, Any]] = []
    row_counts: list[int] = []
    for row in rows:
        samples.extend(float(value) for value in row.get("seconds", []) if value is not None)
        if row.get("median_seconds") is not None:
            round_medians.append(float(row["median_seconds"]))
        else:
            errors.append({"round": row["round"], "error": row.get("error")})
        if row.get("rows") is not None:
            row_counts.append(int(row["rows"]))
    digest = digest_summary(rows)
    return {
        "rounds": len(rows),
        "successful_rounds": len(round_medians),
        "samples": len(samples),
        "sample_seconds_p50": percentile(samples, 0.50),
        "sample_seconds_p95": percentile(samples, 0.95),
        "round_median_seconds_p50": percentile(round_medians, 0.50),
        "round_median_seconds_p95": percentile(round_medians, 0.95),
        "round_median_min": min(round_medians) if round_medians else None,
        "round_median_max": max(round_medians) if round_medians else None,
        "rows": sorted(set(row_counts)),
        "errors": errors,
        "result_sha256": digest,
    }


METRIC_KEYS = (
    "compute_cpu_us",
    "compute_wall_us",
    "exchange_input_bytes",
    "exchange_output_bytes",
    "exchange_decode_bytes",
    "exchange_decode_us",
    "exchange_encode_us",
    "exchange_hash_us",
    "exchange_copy_us",
    "exchange_copied_bytes",
    "spill_bytes_written",
    "spill_peak_bytes",
    "spill_runs_written",
    "spill_compactions",
    "spill_write_us",
    "spill_read_us",
    "memory_peak_bytes",
    "aggregate_input_rows",
    "aggregate_groups_created",
)


def add_metric(target: dict[str, int], source: dict[str, Any], key: str) -> None:
    value = source.get(key)
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        target[key] = target.get(key, 0) + int(value)


def detail_metrics(path: pathlib.Path) -> dict[str, Any]:
    detail = json.loads(path.read_text(encoding="utf-8"))
    tasks = []
    for stage in detail.get("stages", []) or []:
        tasks.extend(stage.get("tasks", []) or [])
    totals: dict[str, int] = {}
    for task in tasks:
        execution = task.get("execution") or {}
        for key in METRIC_KEYS:
            add_metric(totals, execution, key)
    scans: dict[str, int] = {}
    for scan in detail.get("scans", []) or []:
        for key in (
            "files_considered", "files_opened", "files_skipped", "row_groups_considered",
            "row_groups_read", "row_groups_pruned", "rows_selected", "rows_emitted",
            "batches_emitted", "compressed_bytes_selected", "compressed_bytes_read",
            "read_ns", "footer_ns",
        ):
            add_metric(scans, scan, key)
    timings = detail.get("timings") or {}
    return {
        "source": str(path),
        "query_id": detail.get("id"),
        "state": detail.get("state"),
        "row_count": detail.get("row_count"),
        "elapsed_ms": detail.get("elapsed_ms"),
        "timings_us": {key: value for key, value in timings.items() if value is not None},
        "stage_count": len(detail.get("stages", []) or []),
        "task_count": len(tasks),
        "execution_totals": totals,
        "scan_totals": scans,
    }


def parse_details(values: list[str]) -> dict[str, Any]:
    result = {}
    for value in values:
        if "=" not in value:
            raise ValueError(f"--detail must be statement-id=path: {value}")
        statement, raw_path = value.split("=", 1)
        path = pathlib.Path(raw_path)
        if not path.is_file():
            raise FileNotFoundError(path)
        result[statement] = detail_metrics(path)
    return result


def campaign_summary(directory: pathlib.Path, details: dict[str, Any]) -> dict[str, Any]:
    kaveon_rounds = load_rounds(directory, "kaveon")
    trino_rounds = load_rounds(directory, "trino")
    if not kaveon_rounds or not trino_rounds:
        raise ValueError("campaign must contain both kaveon-round*.json and trino-round*.json")
    kaveon = statement_rows(kaveon_rounds)
    trino = statement_rows(trino_rounds)
    statement_ids = sorted(set(kaveon) | set(trino), key=lambda value: (len(value), value))
    statements = {}
    wins = losses = equal = 0
    for statement in statement_ids:
        k = summarize_statement(kaveon.get(statement, []))
        t = summarize_statement(trino.get(statement, []))
        k_median = k["round_median_seconds_p50"]
        t_median = t["round_median_seconds_p50"]
        ratio = (t_median / k_median) if k_median and t_median else None
        verdict = "missing"
        if ratio is not None:
            if ratio > 1:
                wins += 1
                verdict = "kaveon-faster"
            elif ratio < 1:
                losses += 1
                verdict = "trino-faster"
            else:
                equal += 1
                verdict = "equal"
        statements[statement] = {
            "kaveon": k,
            "trino": t,
            "trino_to_kaveon_round_median_ratio": ratio,
            "verdict": verdict,
            "detail": details.get(statement),
        }
    shared_hashes = 0
    stable_hashes = 0
    for item in statements.values():
        kd = item["kaveon"]["result_sha256"]["values"]
        td = item["trino"]["result_sha256"]["values"]
        if kd and td:
            shared_hashes += 1
            if kd == td:
                stable_hashes += 1
    return {
        "schema_version": 1,
        "campaign": str(directory),
        "kaveon_rounds": len(kaveon_rounds),
        "trino_rounds": len(trino_rounds),
        "statement_count": len(statement_ids),
        "kaveon_faster": wins,
        "trino_faster": losses,
        "equal_latency": equal,
        "result_hashes": {"shared_statements": shared_hashes, "equal_across_engines": stable_hashes},
        "statements": statements,
        "limitations": [
            "p50/p95 use the three recorded samples from each round; they are not a replacement for a new five-round campaign.",
            "Rows and hashes are from the runner records. CPU, exchange, memory, and spill counters are present only for supplied detail JSON.",
            "ClickHouse controls are intentionally not mixed into this matched Kaveon/Trino campaign.",
        ],
    }


def markdown(report: dict[str, Any]) -> str:
    lines = [
        "# ClickBench evidence validation",
        "",
        f"Campaign: `{report['campaign']}`  ",
        f"Rounds: Kaveon {report['kaveon_rounds']}, Trino {report['trino_rounds']}  ",
        f"Statements: {report['statement_count']}  ",
        f"Latency wins: Kaveon {report['kaveon_faster']}, Trino {report['trino_faster']}, equal {report['equal_latency']}  ",
        f"Exact result hashes equal: {report['result_hashes']['equal_across_engines']} / {report['result_hashes']['shared_statements']} shared statements",
        "",
        "P50/P95 below are calculated over every recorded per-round sample (normally 15 samples per engine).",
        "",
        "| Query | Kaveon p50/p95 s | Trino p50/p95 s | Trino ÷ Kaveon | Rows K/T | Exact hash | Result |",
        "|---|---:|---:|---:|---:|---|---|",
    ]
    for statement, item in report["statements"].items():
        k, t = item["kaveon"], item["trino"]
        def pair(side: dict[str, Any]) -> str:
            p50, p95 = side["sample_seconds_p50"], side["sample_seconds_p95"]
            return "—" if p50 is None else f"{p50:.3f} / {p95:.3f}"
        rows_k = ",".join(str(v) for v in k["rows"]) or "—"
        rows_t = ",".join(str(v) for v in t["rows"]) or "—"
        ratio = item["trino_to_kaveon_round_median_ratio"]
        ratio_text = "—" if ratio is None else f"{ratio:.2f}×"
        exact = "yes" if k["result_sha256"]["values"] == t["result_sha256"]["values"] and k["result_sha256"]["values"] else "no/missing"
        lines.append(f"| `{statement}` | {pair(k)} | {pair(t)} | {ratio_text} | {rows_k} / {rows_t} | {exact} | {item['verdict']} |")
    details = [(key, value["detail"]) for key, value in report["statements"].items() if value.get("detail")]
    if details:
        lines += ["", "## Kaveon stage evidence", "", "| Query | Elapsed | CPU µs | Exchange bytes | Spill bytes/runs | Peak memory | Row groups read/pruned |", "|---|---:|---:|---:|---:|---:|---:|"]
        for statement, detail in details:
            execution = detail["execution_totals"]
            scans = detail["scan_totals"]
            elapsed = detail.get("elapsed_ms")
            cpu = execution.get("compute_cpu_us")
            exchange = execution.get("exchange_input_bytes", 0) + execution.get("exchange_output_bytes", 0)
            spill = f"{execution.get('spill_bytes_written', 0)} / {execution.get('spill_runs_written', 0)}"
            memory = execution.get("memory_peak_bytes")
            rg = f"{scans.get('row_groups_read', 0)} / {scans.get('row_groups_pruned', 0)}"
            lines.append(f"| `{statement}` | {elapsed if elapsed is not None else '—'} ms | {cpu if cpu is not None else '—'} | {exchange} | {spill} | {memory if memory is not None else '—'} | {rg} |")
    lines += ["", "## Limits", ""]
    lines.extend(f"- {value}" for value in report["limitations"])
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("campaign", type=pathlib.Path)
    parser.add_argument("--detail", action="append", default=[], metavar="QUERY=JSON")
    parser.add_argument("--json", type=pathlib.Path)
    parser.add_argument("--markdown", type=pathlib.Path)
    args = parser.parse_args()
    try:
        report = campaign_summary(args.campaign, parse_details(args.detail))
    except (OSError, ValueError, json.JSONDecodeError) as error:
        parser.error(str(error))
    payload = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.json:
        args.json.write_text(payload, encoding="utf-8")
    if args.markdown:
        args.markdown.write_text(markdown(report), encoding="utf-8")
    if not args.json and not args.markdown:
        print(markdown(report), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
