"""Question-class coverage of one dataset — `python -m dlm.coverage`.

Builds a corpus of questions from the dataset's own context spec (its
metrics, dimensions and indexed values), asks each through `POST /dlm/ask`,
and prints per question class how many were answered, clarified or
refused, and of the answered how many came from context, from the cache or
from a live read — so coverage is a number and a table, not an impression.
Every count is taken from the answer's evidence (`evidence.lane`), which
is the Engine's word on an Engine-backed dataset.

    python -m dlm.coverage --dataset 2
    python -m dlm.coverage --dataset 2 --api http://localhost:8082/api/v1 --per-class 8 --markdown

The classes and the phrasing of each are fixed here, so two runs on two
datasets — or the same dataset before and after a shape is declared — are
comparable. A run needs a reachable API and a generated DLM; it writes
nothing.
"""
from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional

CLASSES = [
    ("total", "The metric alone: a grand total"),
    ("breakdown", "The metric by one dimension"),
    ("filter", "The metric for one dimension value"),
    ("filter_breakdown", "The metric by one dimension for a value of another"),
    ("two_filters", "The metric for values of two dimensions"),
    ("top_n", "A ranking over one dimension"),
    ("distinct_total", "A non-additive metric alone"),
    ("distinct_breakdown", "A non-additive metric by one dimension"),
    ("year", "The metric in one year"),
    ("trend", "The metric over time"),
    ("unknown_value", "A value the dataset does not hold"),
    ("out_of_scope", "A question about nothing the platform holds"),
]


@dataclass
class Outcome:
    asked: int = 0
    answered: int = 0
    clarified: int = 0
    refused: int = 0
    context: int = 0
    cache: int = 0
    live: int = 0
    failed: int = 0
    questions: List[Dict[str, Any]] = field(default_factory=list)


class Client:
    def __init__(self, api: str, token: Optional[str], timeout: float):
        self.api = api.rstrip("/")
        self.token = token
        self.timeout = timeout

    def call(self, method: str, path: str, body: Optional[dict] = None) -> Any:
        headers = {"Content-Type": "application/json"}
        if self.token:
            headers["Authorization"] = "Bearer " + self.token
        request = urllib.request.Request(
            self.api + path, method=method, headers=headers,
            data=json.dumps(body).encode("utf-8") if body is not None else None)
        with urllib.request.urlopen(request, timeout=self.timeout) as response:
            return json.loads(response.read())


def _metric_phrase(name: str) -> str:
    return name.lower()


def _dim_phrase(name: str) -> str:
    return name.replace("_", " ").lower()


def corpus(spec: dict, samples: Dict[str, List[str]], date_column: Optional[str],
           per_class: int) -> Dict[str, List[str]]:
    """The questions per class, from the effective spec and a few indexed
    values per dimension. Hidden metrics and dimensions are left out."""
    metrics = {k: v for k, v in (spec.get("metrics") or {}).items() if not v.get("hidden")}
    dims = [k for k, v in (spec.get("dimensions") or {}).items() if not v.get("hidden")]
    additive = [m for m, v in metrics.items() if v.get("additive", True)]
    distinct = [m for m, v in metrics.items() if not v.get("additive", True)]
    primary = spec.get("default_metric") or (additive[0] if additive else next(iter(metrics), None))
    if not primary:
        return {}
    valued = [d for d in dims if samples.get(d)]
    out: Dict[str, List[str]] = {name: [] for name, _ in CLASSES}
    out["total"] = [f"total {_metric_phrase(m)}" for m in additive]
    out["breakdown"] = [f"{_metric_phrase(primary)} by {_dim_phrase(d)}" for d in dims]
    out["filter"] = [f"{_metric_phrase(primary)} in {samples[d][0]}" for d in valued]
    out["filter_breakdown"] = [f"{_metric_phrase(primary)} by {_dim_phrase(d2)} in {samples[d1][0]}"
                               for i, d1 in enumerate(valued) for d2 in valued[i + 1:i + 2]]
    out["two_filters"] = [f"{_metric_phrase(primary)} in {samples[d1][0]} {samples[d2][1 % len(samples[d2])]}"
                          for i, d1 in enumerate(valued) for d2 in valued[i + 1:i + 2]]
    out["top_n"] = [f"top 3 {_dim_phrase(d)} by {_metric_phrase(primary)}" for d in dims]
    out["distinct_total"] = [f"total {_metric_phrase(m)}" for m in distinct]
    out["distinct_breakdown"] = [f"{_metric_phrase(m)} by {_dim_phrase(d)}" for m in distinct for d in dims[:3]]
    if date_column:
        out["year"] = [f"{_metric_phrase(primary)} in {year}" for year in (2024, 2025, 2026)]
        out["trend"] = [f"{_metric_phrase(primary)} over time"] + [
            f"{_metric_phrase(m)} by month" for m in additive[:2]]
    out["unknown_value"] = [f"{_metric_phrase(primary)} in Atlantis", f"{_metric_phrase(primary)} for Zorbo"]
    out["out_of_scope"] = ["what is the weather in Paris", "write a poem about databases"]
    return {name: questions[:per_class] for name, questions in out.items() if questions}


def classify(answer: dict, outcome: Outcome) -> str:
    outcome.asked += 1
    if answer.get("ok"):
        outcome.answered += 1
        lane = ((answer.get("evidence") or {}).get("lane")
                or ("context" if answer.get("from_context") else "live"))
        if lane == "context":
            outcome.context += 1
        elif lane == "cache":
            outcome.cache += 1
        else:
            outcome.live += 1
        return lane
    reason = answer.get("reason") or "refused"
    if reason == "clarify":
        outcome.clarified += 1
        return "clarify"
    if reason == "query_failed":
        outcome.failed += 1
        return "failed"
    outcome.refused += 1
    return reason


def run(client: Client, dataset_id: str, per_class: int, on_question=None) -> Dict[str, Any]:
    spec_response = client.call("GET", f"/datasets/{dataset_id}/dlm/context")
    spec = spec_response.get("effective") or {}
    coverage = client.call("GET", "/dlm/coverage")
    entry = next((d for d in coverage.get("datasets", []) if str(d.get("dataset_id")) == str(dataset_id)), {})
    samples = {d["column"]: [str(v) for v in d.get("values") or []] for d in entry.get("dimensions") or []}
    questions = corpus(spec, samples, entry.get("date_column"), per_class)
    outcomes: Dict[str, Outcome] = {}
    for name, _ in CLASSES:
        for question in questions.get(name, []):
            outcome = outcomes.setdefault(name, Outcome())
            started = time.monotonic()
            try:
                answer = client.call("POST", "/dlm/ask", {"question": question})
            except urllib.error.HTTPError as error:
                answer = {"ok": False, "reason": f"http_{error.code}"}
            except (urllib.error.URLError, TimeoutError) as error:
                answer = {"ok": False, "reason": f"transport: {error}"}
            result = classify(answer, outcome)
            evidence = answer.get("evidence") or {}
            detail = {"question": question, "result": result,
                      "elapsed_ms": round((time.monotonic() - started) * 1000),
                      "detail": ((evidence.get("execution") or {}).get("detail")),
                      "rows": evidence.get("rows"), "note": answer.get("note")}
            outcome.questions.append(detail)
            if on_question:
                on_question(name, detail)
    return {"dataset_id": str(dataset_id), "dataset_name": spec_response.get("dataset_name") or entry.get("name"),
            "source": entry.get("row_count_source"), "row_count": entry.get("row_count"),
            "classes": {name: vars(o) for name, o in outcomes.items()}}


def render(report: Dict[str, Any], markdown: bool) -> str:
    rows = []
    totals = Outcome()
    for name, description in CLASSES:
        o = report["classes"].get(name)
        if not o:
            continue
        rows.append((name, o["asked"], o["answered"], o["clarified"], o["refused"], o["failed"],
                     o["context"], o["cache"], o["live"]))
        for key in ("asked", "answered", "clarified", "refused", "failed", "context", "cache", "live"):
            setattr(totals, key, getattr(totals, key) + o[key])
    header = ("class", "asked", "answered", "clarified", "refused", "failed", "from context", "cache", "live")
    lines = []
    if markdown:
        lines.append("| " + " | ".join(header) + " |")
        lines.append("|" + "|".join("---" for _ in header) + "|")
        for row in rows:
            lines.append("| " + " | ".join(str(c) for c in row) + " |")
        lines.append("| **all** | " + " | ".join(str(c) for c in (
            totals.asked, totals.answered, totals.clarified, totals.refused, totals.failed,
            totals.context, totals.cache, totals.live)) + " |")
    else:
        widths = [max(len(str(r[i])) for r in [header, *rows]) for i in range(len(header))]
        lines.append("  ".join(str(h).ljust(widths[i]) for i, h in enumerate(header)))
        for row in rows:
            lines.append("  ".join(str(c).ljust(widths[i]) for i, c in enumerate(row)))
        lines.append("  ".join(str(c).ljust(widths[i]) for i, c in enumerate((
            "all", totals.asked, totals.answered, totals.clarified, totals.refused, totals.failed,
            totals.context, totals.cache, totals.live))))
    answered_pct = round(100.0 * totals.answered / totals.asked, 1) if totals.asked else 0.0
    context_pct = round(100.0 * totals.context / totals.answered, 1) if totals.answered else 0.0
    lines.append("")
    lines.append(f"Answered {totals.answered} of {totals.asked} ({answered_pct}%); "
                 f"from context {totals.context} of {totals.answered} answered ({context_pct}%); "
                 f"cache {totals.cache}; live {totals.live}; clarified {totals.clarified}; "
                 f"refused {totals.refused}; failed {totals.failed}.")
    return "\n".join(lines)


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(prog="python -m dlm.coverage", description=__doc__.split("\n\n")[0])
    parser.add_argument("--dataset", required=True, help="dataset id")
    parser.add_argument("--api", default="http://localhost:8082/api/v1", help="platform API base URL")
    parser.add_argument("--token", default=None, help="bearer token, when the API needs one")
    parser.add_argument("--per-class", type=int, default=8, help="questions per class at most")
    parser.add_argument("--timeout", type=float, default=300.0, help="seconds to wait for one answer")
    parser.add_argument("--markdown", action="store_true", help="print the table as Markdown")
    parser.add_argument("--questions", action="store_true", help="print every question with its outcome")
    parser.add_argument("--json", dest="as_json", action="store_true", help="print the report as JSON")
    args = parser.parse_args(argv)
    client = Client(args.api, args.token, args.timeout)

    def progress(name: str, detail: Dict[str, Any]) -> None:
        if args.questions and not args.as_json:
            print(f"  [{name}] {detail['question']!r}: {detail['result']}"
                  + (f" ({detail['detail']})" if detail.get("detail") else "")
                  + f" {detail['elapsed_ms']} ms", file=sys.stderr)

    report = run(client, args.dataset, args.per_class, on_question=progress)
    if args.as_json:
        print(json.dumps(report, indent=2))
        return 0
    print(f"Dataset {report['dataset_id']} ({report['dataset_name']}), {report['row_count']} rows")
    print()
    print(render(report, args.markdown))
    if args.questions and args.markdown:
        print()
        print("| class | question | outcome | detail | note | ms |")
        print("|---|---|---|---|---|---|")
        for name, _ in CLASSES:
            for q in report["classes"].get(name, {}).get("questions", []):
                note = (q.get("note") or "").replace("|", "/")
                print(f"| {name} | {q['question']} | {q['result']} | {q.get('detail') or ''} | {note} | {q['elapsed_ms']} |")
    return 0


if __name__ == "__main__":
    sys.exit(main())
