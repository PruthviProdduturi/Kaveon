"""Merge a Kaveon suite record (scripts/scale-suite.py) and a Trino suite
record (scripts/benchmark-trino-suite.py) for the same suite into one
Markdown table: median seconds per statement on both engines, the ratio,
whether the result digests agree, and the coverage summary. Prints the
table; write it into the tier record by hand beside the two JSON files.

    python scripts/benchmark-suite-report.py kaveon.json trino.json
"""
import json
import statistics
import sys


def load(path):
    text = open(path, encoding="utf-8").read()
    if text.startswith("SCALE_SUITE="):
        text = text[len("SCALE_SUITE="):]
    return json.loads(text)


def main():
    kaveon = load(sys.argv[1])
    trino = load(sys.argv[2])
    trino_by = {r["id"]: r for r in trino["records"]}
    rows = []
    wins = losses = both = 0
    ratios = []
    print("| Query | Kaveon s | Trino s | Trino ÷ Kaveon | Result |")
    print("|---|---:|---:|---:|---|")
    for k in kaveon["records"]:
        t = trino_by.get(k["id"], {})
        ks, ts = k.get("median_seconds"), t.get("median_seconds")
        if ks is not None and ts is not None:
            both += 1
            ratio = ts / ks if ks else float("inf")
            ratios.append(ratio)
            wins += ratio > 1
            losses += ratio <= 1
            verdict = "same" if k.get("result_sha256") == t.get("result_sha256") else (
                "rows " + str(k.get("rows")) + "/" + str(t.get("rows")))
            note = " (adapted)" if k.get("adapted") or t.get("adapted") else ""
            print(f"| `{k['id']}` | {ks:.2f} | {ts:.2f} | {ratio:.2f}× | {verdict}{note} |")
        else:
            kaveon_cell = f"{ks:.2f}" if ks is not None else "— " + (k.get("error") or "")[:60]
            trino_cell = f"{ts:.2f}" if ts is not None else "— " + (t.get("error") or "")[:60]
            print(f"| `{k['id']}` | {kaveon_cell} | {trino_cell} | | |")
    print()
    print(f"Both ran: {both} of {len(kaveon['records'])}; Kaveon faster on {wins}, Trino faster on {losses}; "
          f"geometric mean of Trino ÷ Kaveon over statements both ran: "
          f"{statistics.geometric_mean(ratios):.2f}×" if ratios else "no statements ran on both engines")
    print(f"Kaveon ran {sum(1 for r in kaveon['records'] if r.get('median_seconds') is not None)} of {len(kaveon['records'])}; "
          f"Trino ran {sum(1 for r in trino['records'] if r.get('median_seconds') is not None)} of {len(trino['records'])}.")


if __name__ == "__main__":
    main()
