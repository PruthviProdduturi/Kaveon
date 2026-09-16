"""Summarise a rounds campaign (scripts/benchmark-rounds.py): per statement
and engine, the median over rounds of each round's median, the fastest and
slowest round, and the Trino ÷ Kaveon ratio of the round medians. A
statement that failed in any round is reported with that round's error and
counted as not run. Prints Markdown; writes <dir>/rounds.json beside it.

    python scripts/benchmark-rounds-report.py docs/qualification/clickbench/runs/rounds-2026-09-17
"""
import json
import pathlib
import statistics
import sys


def load_rounds(directory, engine):
    rounds = []
    for path in sorted(directory.glob(f"{engine}-round*.json"), key=lambda p: int(p.stem.rsplit("round", 1)[1])):
        rounds.append(json.loads(path.read_text(encoding="utf-8")))
    return rounds


def per_statement(rounds):
    """id -> {"medians": [round medians], "errors": [(round, error)], "digests": set}"""
    table = {}
    for record in rounds:
        for r in record["records"]:
            entry = table.setdefault(r["id"], {"medians": [], "errors": [], "digests": set()})
            if r.get("median_seconds") is not None:
                entry["medians"].append(r["median_seconds"])
            else:
                entry["errors"].append((record.get("round"), (r.get("error") or "")[:80]))
            if r.get("result_sha256"):
                entry["digests"].add(r["result_sha256"])
    return table


def cell(entry, rounds):
    if len(entry["medians"]) < rounds:
        return None, f"— ran {len(entry['medians'])} of {rounds} rounds ({entry['errors'][0][1] if entry['errors'] else 'missing'})"
    medians = entry["medians"]
    return statistics.median(medians), f"{statistics.median(medians):.2f} ({min(medians):.2f}–{max(medians):.2f})"


def main():
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    directory = pathlib.Path(sys.argv[1])
    kaveon = load_rounds(directory, "kaveon")
    trino = load_rounds(directory, "trino")
    rounds = max(len(kaveon), len(trino))
    if rounds == 0:
        raise SystemExit("no round records found")
    k_table = per_statement(kaveon)
    t_table = per_statement(trino)
    ids = list(dict.fromkeys(list(k_table) + list(t_table)))
    print(f"Rounds: Kaveon {len(kaveon)}, Trino {len(trino)}. Cells are the median over rounds of each round's median of three timed executions, with the fastest and slowest round.")
    print()
    print("| Query | Kaveon s | Trino s | Trino ÷ Kaveon | Result |")
    print("|---|---:|---:|---:|---|")
    wins = losses = both = 0
    ratios = []
    summary = {}
    for statement in ids:
        k = k_table.get(statement, {"medians": [], "errors": [], "digests": set()})
        t = t_table.get(statement, {"medians": [], "errors": [], "digests": set()})
        k_median, k_cell = cell(k, len(kaveon)) if kaveon else (None, "—")
        t_median, t_cell = cell(t, len(trino)) if trino else (None, "—")
        ratio_cell = verdict = ""
        if k_median is not None and t_median is not None:
            both += 1
            ratio = t_median / k_median if k_median else float("inf")
            ratios.append(ratio)
            wins += ratio > 1
            losses += ratio <= 1
            ratio_cell = f"{ratio:.2f}×"
            same = k["digests"] and t["digests"] and k["digests"] == t["digests"]
            verdict = "same" if same else ("stable per engine" if len(k["digests"]) <= 1 and len(t["digests"]) <= 1 else "varies")
        summary[statement] = {
            "kaveon_round_medians": k["medians"], "trino_round_medians": t["medians"],
            "kaveon_median": k_median, "trino_median": t_median,
            "kaveon_errors": k["errors"], "trino_errors": t["errors"],
            "digests_agree": bool(k["digests"] and t["digests"] and k["digests"] == t["digests"]),
        }
        print(f"| `{statement}` | {k_cell} | {t_cell} | {ratio_cell} | {verdict} |")
    print()
    if ratios:
        print(f"Both ran in every round: {both} of {len(ids)}; Kaveon faster on {wins}, Trino faster on {losses}; "
              f"geometric mean of Trino ÷ Kaveon over those: {statistics.geometric_mean(ratios):.2f}×")
    (directory / "rounds.json").write_text(json.dumps({"rounds": rounds, "statements": summary}, indent=1), encoding="utf-8")


if __name__ == "__main__":
    main()
