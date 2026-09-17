"""Summarise a rounds campaign (scripts/benchmark-rounds.py): per statement
and engine, the median over rounds of each round's median, the fastest and
slowest round, and the Trino ÷ Kaveon ratio of the round medians. A
statement that failed in any round is reported with that round's error and
counted as not run. Prints Markdown; writes <dir>/rounds.json beside it.

When the campaign ran the throughput tier (--throughput), a second table
gives, per client count and engine, successful exact executions per second
as the median over rounds with the fastest and slowest round, the failures
and admission rejections summed over the rounds, and the statements that
failed in every attempt.

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


def load_throughput(directory, engine):
    """clients -> [round records], from <engine>-throughput-<clients>-round<N>.json."""
    table = {}
    for path in directory.glob(f"{engine}-throughput-*-round*.json"):
        stem, round_part = path.stem.rsplit("-round", 1)
        clients = int(stem.rsplit("-", 1)[1])
        table.setdefault(clients, []).append((int(round_part), json.loads(path.read_text(encoding="utf-8"))))
    return {clients: [record for _, record in sorted(rounds)] for clients, rounds in sorted(table.items())}


def throughput_cell(records):
    """(median executions per second, rendered cell, summed failures, summed
    rejections, summed ties, statements that failed everywhere in any round)."""
    rates = [r["executions_per_second"] for r in records if r.get("executions_per_second") is not None]
    if not rates:
        return None, "—", 0, 0, 0, []
    failed = sorted({sid for r in records for sid in r.get("failed_everywhere", [])})
    return (round(statistics.median(rates), 4), f"{statistics.median(rates):.3f} ({min(rates):.3f}–{max(rates):.3f})",
            sum(r.get("failures", 0) for r in records), sum(r.get("rejections", 0) for r in records),
            sum(r.get("ties", 0) for r in records), failed)


def digests_agree(kaveon_records, trino_records):
    """How many statements both engines digested identically in every round."""
    def by_statement(records):
        table = {}
        for record in records:
            for statement in record.get("statements", []):
                if statement.get("result_sha256"):
                    table.setdefault(statement["id"], set()).add(statement["result_sha256"])
        return table
    k, t = by_statement(kaveon_records), by_statement(trino_records)
    shared = [sid for sid in k if sid in t]
    return sum(1 for sid in shared if k[sid] == t[sid] and len(k[sid]) == 1), len(shared)


def report_throughput(directory):
    kaveon, trino = load_throughput(directory, "kaveon"), load_throughput(directory, "trino")
    counts = sorted(set(kaveon) | set(trino))
    if not counts:
        return None
    print()
    print("Throughput: successful exact executions per second, median over rounds with the fastest and slowest round; "
          "failures, admission rejections and ties (an ORDER BY … LIMIT statement returning a different row set of the same size) "
          "are summed over the rounds. Same clients, duration and warm-up on both engines.")
    print()
    print("| Clients | Rounds | Kaveon exec/s | Trino exec/s | Kaveon ÷ Trino | Kaveon fail / rej / tie | Trino fail / rej / tie | Digests agree | Failed every attempt |")
    print("|---:|---|---:|---:|---:|---:|---:|---:|---|")
    summary = {}
    for clients in counts:
        k_records, t_records = kaveon.get(clients, []), trino.get(clients, [])
        k_rate, k_cell, k_fail, k_rej, k_tie, k_failed = throughput_cell(k_records)
        t_rate, t_cell, t_fail, t_rej, t_tie, t_failed = throughput_cell(t_records)
        ratio = f"{k_rate / t_rate:.2f}×" if k_rate is not None and t_rate else ""
        agree, shared = digests_agree(k_records, t_records)
        coverage = "; ".join(part for part in (
            f"Kaveon: {', '.join(k_failed)}" if k_failed else "", f"Trino: {', '.join(t_failed)}" if t_failed else "") if part) or "none"
        print(f"| {clients} | K {len(k_records)}, T {len(t_records)} | {k_cell} | {t_cell} | {ratio} | {k_fail} / {k_rej} / {k_tie} | "
              f"{t_fail} / {t_rej} / {t_tie} | {agree} of {shared} | {coverage} |")
        summary[str(clients)] = {
            "kaveon": {"rounds": len(k_records), "executions_per_second": k_rate,
                       "round_rates": [r.get("executions_per_second") for r in k_records],
                       "failures": k_fail, "rejections": k_rej, "ties": k_tie, "failed_everywhere": k_failed},
            "trino": {"rounds": len(t_records), "executions_per_second": t_rate,
                      "round_rates": [r.get("executions_per_second") for r in t_records],
                      "failures": t_fail, "rejections": t_rej, "ties": t_tie, "failed_everywhere": t_failed},
            "digests_agree": agree, "digests_compared": shared,
        }
    return summary


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
        # A throughput-only directory (hand-launched windows) still reports.
        throughput = report_throughput(directory)
        if not throughput:
            raise SystemExit("no round records found")
        (directory / "rounds.json").write_text(json.dumps({"rounds": 0, "throughput": throughput}, indent=1), encoding="utf-8")
        return
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
    throughput = report_throughput(directory)
    output = {"rounds": rounds, "statements": summary}
    if throughput:
        output["throughput"] = throughput
    (directory / "rounds.json").write_text(json.dumps(output, indent=1), encoding="utf-8")


if __name__ == "__main__":
    main()
