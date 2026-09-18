"""Emit the data behind the product page's benchmark figures from a rounds
campaign (scripts/benchmark-rounds.py): Kaveon only, one number per
statement or per round.

Latency (the default): per statement the median over rounds of each
round's median of its three timed executions, in seconds, in suite order,
with a short description — derived from the SQL's shape for ClickBench, the
specification's query name for TPC-H (`--suite tpch`). A statement that did
not finish in every round carries `seconds: null` and is not counted. The
overall block carries only what the round records and the suite establish:
statements, how many ran in every round, how many finish under one second
and under ten, the slowest, the rows scanned (ClickBench: the result of
q01, `SELECT COUNT(*) FROM hits`) or the scale factor (TPC-H: from the
suite name), the rounds, the date of the campaign directory, the engine
digests, and the record path. The cluster shape is not in the round
records, so it is an argument with the qualification cluster's values as
defaults.

Throughput (`--throughput`): per client count, every round's successful
exact executions per second with its successes, failures, admission
rejections, ties and the measured span it was divided by (the window plus
the time the statements in flight at its end took to return), and the
median over rounds, from kaveon-throughput-<clients>-round<N>.json.

    python scripts/benchmark-chart-data.py docs/qualification/clickbench/runs/rounds-2026-09-17 \
        studio/public/benchmarks/clickbench-2026-09.json
    python scripts/benchmark-chart-data.py docs/qualification/tpch/runs/rounds-<date> \
        studio/public/benchmarks/tpch-2026-09.json --suite tpch
    python scripts/benchmark-chart-data.py docs/qualification/clickbench/runs/rounds-2026-09-17 \
        studio/public/benchmarks/throughput-2026-09.json --throughput
"""
import argparse
import datetime
import json
import pathlib
import re
import statistics

REPO = pathlib.Path(__file__).resolve().parent.parent


def load_rounds(directory: pathlib.Path):
    paths = sorted(directory.glob("kaveon-round*.json"), key=lambda p: int(p.stem.rsplit("round", 1)[1]))
    return [json.loads(p.read_text(encoding="utf-8")) for p in paths]


def round_median(record):
    """A round's median of its timed executions; None when the statement failed."""
    if record.get("error"):
        return None
    seconds = [s for s in record.get("seconds") or [] if s is not None]
    if seconds:
        return statistics.median(seconds)
    return record.get("median_seconds")


def describe(sql: str) -> str:
    """A short description of the statement's shape: the grouping or ordering
    keys with the row cut, else the select list. Derived, not authored."""
    flat = fold_calls(re.sub(r"\s+", " ", sql).strip())
    limit = re.search(r"\bLIMIT (\d+)", flat, re.I)
    offset = re.search(r"\bOFFSET (\d+)", flat, re.I)
    cut = ""
    if limit:
        cut = f"top {limit.group(1)}" if not offset else f"rows {offset.group(1)}–{int(offset.group(1)) + int(limit.group(1))}"
    where = " with a filter" if re.search(r"\bWHERE\b", flat, re.I) else ""
    group = re.search(r"\bGROUP BY (.+?)(?: HAVING| ORDER BY| LIMIT| OFFSET|$)", flat, re.I)
    if group:
        return f"Group by {keys(group.group(1))}{where}" + (f", {cut}" if cut else "")
    order = re.search(r"\bORDER BY (.+?)(?: LIMIT| OFFSET|$)", flat, re.I)
    if order:
        return f"Order by {keys(order.group(1))}{where}" + (f", {cut}" if cut else "")
    select = re.search(r"^SELECT (.+?) FROM\b", flat, re.I)
    return f"{keys(select.group(1)) if select else flat}{where}"


def fold_calls(text: str) -> str:
    """Collapse every function call whose argument is not a bare column to
    `name(…)`, so keys split cleanly on commas and read short."""
    out, i = [], 0
    while i < len(text):
        m = re.match(r"[A-Za-z_]+\(", text[i:])
        if m:
            name = m.group(0)[:-1]
            j = i + len(m.group(0))
            level = 1
            while j < len(text) and level:
                level += {"(": 1, ")": -1}.get(text[j], 0)
                j += 1
            inner = text[i + len(name) + 1:j - 1]
            simple = re.fullmatch(r"(DISTINCT )?[A-Za-z_]+|\*", inner, re.I)
            out.append(f"{name}({inner})" if simple else f"{name.lower()}(…)")
            i = j
            continue
        out.append(text[i])
        i += 1
    return "".join(out)


def keys(text: str) -> str:
    parts = [k.strip() for k in text.split(",") if k.strip()]
    parts = [re.sub(r"^extract\(…\)$", "minute", k) for k in parts]
    if len(parts) > 3:
        return ", ".join(parts[:3]) + f" and {len(parts) - 3} more"
    return ", ".join(parts)


TPCH_NAMES = {
    "q1": "Pricing summary report",
    "q2": "Minimum cost supplier",
    "q3": "Shipping priority",
    "q4": "Order priority checking",
    "q5": "Local supplier volume",
    "q6": "Forecasting revenue change",
    "q7": "Volume shipping",
    "q8": "National market share",
    "q9": "Product type profit measure",
    "q10": "Returned item reporting",
    "q11": "Important stock identification",
    "q12": "Shipping modes and order priority",
    "q13": "Customer distribution",
    "q14": "Promotion effect",
    "q15": "Top supplier",
    "q16": "Parts/supplier relationship",
    "q17": "Small-quantity-order revenue",
    "q18": "Large volume customer",
    "q19": "Discounted revenue",
    "q20": "Potential part promotion",
    "q21": "Suppliers who kept orders waiting",
    "q22": "Global sales opportunity",
}

SUITES = {
    "clickbench": {"suite": "ClickBench", "title": "ClickBench", "suite_file": "clickbench/kaveon-suite.json"},
    "tpch": {"suite": "TPC-H", "title": "TPC-H", "suite_file": "tpch/kaveon-suite.json"},
}


def campaign_meta(directory, rounds):
    date_match = re.search(r"(\d{4}-\d{2}-\d{2})", directory.name)
    digests = sorted({r.get("engine_digest") for r in rounds if r.get("engine_digest")})
    try:
        record_path = directory.resolve().relative_to(REPO).as_posix()
    except ValueError:
        record_path = directory.as_posix()
    return {
        "date": date_match.group(1) if date_match else datetime.date.today().isoformat(),
        "engine_digests": digests,
        "record_path": record_path,
    }


def latency(args, directory):
    kind = SUITES[args.suite]
    suite_path = args.suite_file or (REPO / "docs" / "qualification" / kind["suite_file"])
    suite = json.loads(suite_path.read_text(encoding="utf-8"))
    rounds = load_rounds(directory)
    if not rounds:
        raise SystemExit(f"no kaveon-round*.json under {directory}")

    per_statement = {}
    rows = None
    for record in rounds:
        for r in record["records"]:
            per_statement.setdefault(r["id"], []).append(round_median(r))
            if args.suite == "clickbench" and r["id"] == "q01" and rows is None and r.get("sample"):
                try:
                    rows = int(r["sample"][0][0])
                except (TypeError, ValueError, IndexError):
                    rows = None

    statements = []
    for statement in suite["statements"]:
        sid = statement["id"]
        medians = per_statement.get(sid, [])
        finished = [m for m in medians if m is not None]
        ran_every_round = len(finished) == len(rounds)
        label = TPCH_NAMES.get(sid, sid) if args.suite == "tpch" else describe(statement["sql"])
        statements.append({
            "id": sid,
            "label": label,
            "sql": statement["sql"],
            "seconds": round(statistics.median(finished), 3) if ran_every_round else None,
            "round_medians": [round(m, 3) if m is not None else None for m in medians],
        })

    timed = [s for s in statements if s["seconds"] is not None]
    slowest = max(timed, key=lambda s: s["seconds"]) if timed else None
    repetitions = [r["repetitions"] for r in rounds if isinstance(r.get("repetitions"), int)]
    executions = max(repetitions) if repetitions else max(len(r.get("seconds") or []) for rec in rounds for r in rec["records"])
    scale = re.search(r"\bSF(\d+)\b", suite.get("name", ""))
    title = kind["title"] + (f" SF{scale.group(1)}" if scale else "")

    output = {
        "kind": "latency",
        "suite": kind["suite"],
        "title": title,
        "engine": "Kaveon",
        "rounds": len(rounds),
        "executions_per_round": executions,
        "rows": rows,
        "table": "hits" if args.suite == "clickbench" else None,
        "scale_factor": int(scale.group(1)) if scale else None,
        "cluster": {
            "workers": args.workers,
            "node_sku": args.node_sku,
            "memory_per_query_per_worker": args.memory_per_query,
        },
        **campaign_meta(directory, rounds),
        "summary": {
            "statements": len(statements),
            "ran_every_round": len(timed),
            "under_1s": sum(1 for s in timed if s["seconds"] < 1),
            "under_10s": sum(1 for s in timed if s["seconds"] < 10),
            "slowest": {"id": slowest["id"], "seconds": slowest["seconds"]} if slowest else None,
        },
        "statements": statements,
    }
    s = output["summary"]
    print(f"{title}: {s['ran_every_round']} of {s['statements']} statements ran in every one of {len(rounds)} rounds; "
          f"{s['under_1s']} under 1 s, {s['under_10s']} under 10 s; slowest "
          f"{s['slowest']['id'] if slowest else '—'} at {s['slowest']['seconds'] if slowest else '—'} s; rows {rows}")
    return output


def throughput(args, directory):
    kind = SUITES[args.suite]
    table = {}
    for path in directory.glob("kaveon-throughput-*-round*.json"):
        stem, round_part = path.stem.rsplit("-round", 1)
        clients = int(stem.rsplit("-", 1)[1])
        table.setdefault(clients, []).append((int(round_part), json.loads(path.read_text(encoding="utf-8"))))
    if not table:
        raise SystemExit(f"no kaveon-throughput-<clients>-round<N>.json under {directory}")
    records = [record for rounds in table.values() for _, record in rounds]

    groups = []
    for clients, rounds in sorted(table.items()):
        entries = []
        for number, record in sorted(rounds):
            entries.append({
                "round": record.get("round", number),
                "executions_per_second": record.get("executions_per_second"),
                "elapsed_seconds": record.get("elapsed_seconds"),
                "successful": record.get("successful", 0),
                "failures": record.get("failures", 0),
                "rejections": record.get("rejections", 0),
                "ties": record.get("ties") or 0,
            })
        rates = [e["executions_per_second"] for e in entries if e["executions_per_second"] is not None]
        groups.append({
            "clients": clients,
            "rounds": entries,
            "median_executions_per_second": round(statistics.median(rates), 4) if rates else None,
        })

    durations = {r.get("duration_seconds") for r in records}
    warmups = {r.get("warmup_seconds") for r in records}
    if len(durations) != 1 or len(warmups) != 1:
        raise SystemExit(f"windows differ across records: durations {durations}, warm-ups {warmups}")
    output = {
        "kind": "throughput",
        "suite": kind["suite"],
        "title": f"{kind['title']} concurrency",
        "engine": "Kaveon",
        "duration_seconds": durations.pop(),
        "warmup_seconds": warmups.pop(),
        "cluster": {
            "workers": args.workers,
            "node_sku": args.node_sku,
            "memory_per_query_per_worker": args.memory_per_query,
        },
        **campaign_meta(directory, records),
        "groups": groups,
    }
    for g in groups:
        print(f"{g['clients']} clients: {len(g['rounds'])} rounds, median {g['median_executions_per_second']} executions per second")
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("rounds_dir", type=pathlib.Path)
    parser.add_argument("out", type=pathlib.Path)
    parser.add_argument("--suite", choices=sorted(SUITES), default="clickbench",
                        help="which suite the rounds ran: selects the label source and the suite file")
    parser.add_argument("--suite-file", type=pathlib.Path, default=None,
                        help="statement suite (default: docs/qualification/<suite>/kaveon-suite.json)")
    parser.add_argument("--throughput", action="store_true",
                        help="write the concurrency figure from kaveon-throughput-<clients>-round<N>.json instead")
    parser.add_argument("--workers", type=int, default=3, help="worker nodes the engine ran on")
    parser.add_argument("--node-sku", default="Standard_D4s_v3", help="worker node size")
    parser.add_argument("--memory-per-query", default="3 GiB", help="memory budget per query per worker")
    args = parser.parse_args()

    output = throughput(args, args.rounds_dir) if args.throughput else latency(args, args.rounds_dir)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(output, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main()
