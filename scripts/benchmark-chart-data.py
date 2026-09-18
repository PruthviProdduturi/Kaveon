"""Emit the data behind the product page's ClickBench figure from a rounds
campaign (scripts/benchmark-rounds.py): Kaveon only, one number per
statement.

Per statement: the median over rounds of each round's median of its three
timed executions, in seconds, in upstream order, with a short description
derived from the statement's shape and the upstream SQL itself. A statement
that did not finish in every round carries `seconds: null` and is not
counted. The overall block carries only what the round records and the
suite establish: statements, how many ran in every round, how many finish
under one second and under ten, the slowest, the rows scanned (the result of
q01, `SELECT COUNT(*) FROM hits`), the rounds, the date of the campaign
directory, the engine digests, and the record path. The cluster shape is not
in the round records, so it is an argument with the qualification cluster's
values as defaults.

    python scripts/benchmark-chart-data.py docs/qualification/clickbench/runs/rounds-2026-09-17 \
        studio/public/benchmarks/clickbench-2026-09.json
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


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("rounds_dir", type=pathlib.Path)
    parser.add_argument("out", type=pathlib.Path)
    parser.add_argument("--suite", type=pathlib.Path, default=None,
                        help="statement suite (default: <rounds dir>/../../kaveon-suite.json)")
    parser.add_argument("--workers", type=int, default=3, help="worker nodes the engine ran on")
    parser.add_argument("--node-sku", default="Standard_D4s_v3", help="worker node size")
    parser.add_argument("--memory-per-query", default="3 GiB", help="memory budget per query per worker")
    args = parser.parse_args()

    directory = args.rounds_dir
    suite_path = args.suite or (directory.parent.parent / "kaveon-suite.json")
    suite = json.loads(suite_path.read_text(encoding="utf-8"))
    rounds = load_rounds(directory)
    if not rounds:
        raise SystemExit(f"no kaveon-round*.json under {directory}")

    per_statement = {}
    rows = None
    for record in rounds:
        for r in record["records"]:
            per_statement.setdefault(r["id"], []).append(round_median(r))
            if r["id"] == "q01" and rows is None and r.get("sample"):
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
        statements.append({
            "id": sid,
            "label": describe(statement["sql"]),
            "sql": statement["sql"],
            "seconds": round(statistics.median(finished), 3) if ran_every_round else None,
            "round_medians": [round(m, 3) if m is not None else None for m in medians],
        })

    timed = [s for s in statements if s["seconds"] is not None]
    slowest = max(timed, key=lambda s: s["seconds"]) if timed else None
    date_match = re.search(r"(\d{4}-\d{2}-\d{2})", directory.name)
    date = date_match.group(1) if date_match else datetime.date.today().isoformat()
    digests = sorted({r.get("engine_digest") for r in rounds if r.get("engine_digest")})
    try:
        record_path = directory.resolve().relative_to(REPO).as_posix()
    except ValueError:
        record_path = directory.as_posix()
    repetitions = [r["repetitions"] for r in rounds if isinstance(r.get("repetitions"), int)]

    output = {
        "suite": "ClickBench",
        "engine": "Kaveon",
        "date": date,
        "rounds": len(rounds),
        "executions_per_round": max(repetitions) if repetitions else max(len(r.get("seconds") or []) for rec in rounds for r in rec["records"]),
        "rows": rows,
        "table": "hits",
        "cluster": {
            "workers": args.workers,
            "node_sku": args.node_sku,
            "memory_per_query_per_worker": args.memory_per_query,
        },
        "engine_digests": digests,
        "record_path": record_path,
        "summary": {
            "statements": len(statements),
            "ran_every_round": len(timed),
            "under_1s": sum(1 for s in timed if s["seconds"] < 1),
            "under_10s": sum(1 for s in timed if s["seconds"] < 10),
            "slowest": {"id": slowest["id"], "seconds": slowest["seconds"]} if slowest else None,
        },
        "statements": statements,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(output, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    s = output["summary"]
    print(f"{s['ran_every_round']} of {s['statements']} statements ran in every one of {len(rounds)} rounds; "
          f"{s['under_1s']} under 1 s, {s['under_10s']} under 10 s; slowest "
          f"{s['slowest']['id'] if slowest else '—'} at {s['slowest']['seconds'] if slowest else '—'} s; "
          f"rows {rows}; wrote {args.out}")


if __name__ == "__main__":
    main()
