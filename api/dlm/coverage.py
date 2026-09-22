"""Question-class coverage of a dataset — `python -m dlm.coverage`.

Builds a corpus of questions from the dataset's own context spec (its
metrics, dimensions, time dimension and indexed values), asks each through
`POST /dlm/ask`, and prints per question class how many were answered,
clarified or refused, and of the answered how many came from context, from
the cache or from a live read — so coverage is a number and a table, not an
impression. Every lane count is taken from the answer's evidence
(`evidence.lane`), which is the Engine's word on an Engine-backed dataset.

    python -m dlm.coverage --dataset 25
    python -m dlm.coverage --dataset 25 --dataset 26 --markdown --questions
    python -m dlm.coverage --dataset 25 --check --per-class 12

The class list is `dlm.classes.CLASSES` itself, so the harness and the
product cannot drift apart. The phrasing of each class is fixed here, so two
runs on two datasets — or the same dataset before and after a shape is
declared — are comparable.

`--check` adds the wrong-answer count: a question whose value is also
computed by an **independent** statement (written here, run through `POST
/dlm/reproduce`, which forces `use_statistics = false` and
`result_cache = false`, so neither the cube nor a cached result can answer
it) and compared with what the DLM returned. An approximate answer is
allowed its stated error; everything else must match exactly. A question
that comes back different is a *wrong answer* and is reported as such,
separately from one that was never answered.

A run needs a reachable API and a generated DLM; it writes nothing.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Tuple

from dlm import classes

# The order classes are reported in: the base shapes, then the derived ones,
# then the clarification and refusal paths.
CLASSES: List[Tuple[str, str]] = [(c.name, c.description) for c in classes.CLASSES]
# How far an approximate answer may sit from the exact one before it counts as
# wrong. The Engine states 1.6 % relative standard error for HyperLogLog p=12;
# three standard errors is the bound used here.
APPROXIMATE_TOLERANCE = 0.05


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
    checked: int = 0
    wrong: int = 0
    misclassified: int = 0
    questions: List[Dict[str, Any]] = field(default_factory=list)


class Client:
    def __init__(self, api: str, token: Optional[str], timeout: float,
                 headers: Optional[Dict[str, str]] = None):
        self.api = api.rstrip("/")
        self.token = token
        self.timeout = timeout
        self.extra = dict(headers or {})

    def call(self, method: str, path: str, body: Optional[dict] = None) -> Any:
        headers = {"Content-Type": "application/json", **self.extra}
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


def _identifier(name: str) -> str:
    return name if re.match(r"^[A-Za-z_][A-Za-z0-9_]*$", name) else '"' + name.replace('"', '""') + '"'


# --------------------------------------------------------------------------- #
# The corpus                                                                   #
# --------------------------------------------------------------------------- #

def corpus(spec: dict, samples: Dict[str, List[str]], date_column: Optional[str],
           per_class: int) -> Dict[str, List[str]]:
    """The questions per class, from the effective spec and a few indexed
    values per dimension. Hidden metrics and dimensions are left out, and a
    class whose prerequisite the dataset lacks (no time dimension, one
    measure) produces no questions rather than questions that cannot pass."""
    metrics = {k: v for k, v in (spec.get("metrics") or {}).items() if not v.get("hidden")}
    dims = [k for k, v in (spec.get("dimensions") or {}).items() if not v.get("hidden")]
    additive = [m for m, v in metrics.items() if v.get("additive", True)]
    distinct = [m for m, v in metrics.items() if not v.get("additive", True)]
    primary = spec.get("default_metric") or (additive[0] if additive else next(iter(metrics), None))
    if not primary:
        return {}
    time_spec = spec.get("time") if isinstance(spec.get("time"), dict) else {}
    latest = str(time_spec.get("latest") or "")
    grain = time_spec.get("grain") or "day"
    has_time = bool(date_column or time_spec.get("column"))
    valued = [d for d in dims if samples.get(d)]
    out: Dict[str, List[str]] = {name: [] for name, _ in CLASSES}

    out["total"] = [f"total {_metric_phrase(m)}" for m in additive]
    out["breakdown"] = [f"{_metric_phrase(primary)} by {_dim_phrase(d)}" for d in dims]
    out["filter"] = [f"{_metric_phrase(primary)} in {samples[d][0]}" for d in valued] + \
                    [f"{_metric_phrase(primary)} for {samples[d][1]}" for d in valued
                     if len(samples[d]) > 1]
    out["filter_breakdown"] = [f"{_metric_phrase(primary)} by {_dim_phrase(d2)} in {samples[d1][0]}"
                               for i, d1 in enumerate(valued) for d2 in valued[i + 1:i + 2]]
    out["two_filters"] = [f"{_metric_phrase(primary)} in {samples[d1][0]} {samples[d2][1 % len(samples[d2])]}"
                          for i, d1 in enumerate(valued) for d2 in valued[i + 1:i + 2]]
    out["top_n"] = [f"top 3 {_dim_phrase(d)} by {_metric_phrase(primary)}" for d in dims] + \
                   [f"bottom 3 {_dim_phrase(d)} by {_metric_phrase(primary)}" for d in dims[:4]]
    out["distinct_total"] = [f"total {_metric_phrase(m)}" for m in distinct]
    out["distinct_breakdown"] = [f"{_metric_phrase(m)} by {_dim_phrase(d)}"
                                 for m in distinct for d in dims[:3]]

    if has_time and latest:
        year = latest[:4]
        out["time_slice"] = [f"{_metric_phrase(primary)} in {year}"] + [
            f"{_metric_phrase(m)} in {year}" for m in additive[:3]]
        out["trend"] = [f"{_metric_phrase(primary)} over time"] + [
            f"{_metric_phrase(m)} by month" for m in additive[:3]]
        unit = "month" if grain in ("day", "month") else "year"
        out["comparison_period"] = [
            f"{_metric_phrase(primary)} vs last {unit}",
            f"{_metric_phrase(primary)} compared to the previous {unit}",
        ] + [f"{_metric_phrase(m)} vs last {unit}" for m in additive[1:4]]
        out["year_over_year"] = [
            f"{_metric_phrase(primary)} year over year",
            f"{_metric_phrase(primary)} vs last year",
        ] + [f"{_metric_phrase(m)} year over year" for m in additive[1:3]]

    out["share_of_total"] = [f"what share of {_metric_phrase(primary)} is {samples[d][0]}"
                             for d in valued[:6]] + \
                            [f"percentage of {_metric_phrase(primary)} by {_dim_phrase(d)}"
                             for d in dims[:6]]
    if len(additive) > 1:
        out["ratio"] = [f"{_metric_phrase(a)} per {_metric_phrase(b)}"
                        for a, b in zip(additive[1:7], additive[0:6]) if a != b]
    if len(dims) > 1:
        out["top_n_within"] = [f"top 2 {_dim_phrase(inner)} by {_metric_phrase(primary)} in each {_dim_phrase(outer)}"
                               for outer, inner in zip(dims[:6], dims[1:7])]
    out["existence"] = [f"how many {_dim_phrase(d)} have more than 1000 {_metric_phrase(primary)}"
                        for d in dims[:6]]
    out["vague_default"] = ["what is current usage", "how are we doing",
                            "give me a summary", "where do we stand"]

    out["clarify_value"] = [f"{_metric_phrase(primary)} in {near}"
                            for near in _near_misses(samples, valued)]
    out["clarify_metric"] = []
    out["clarify_dimension"] = [f"{_metric_phrase(primary)} by colour",
                                f"{_metric_phrase(primary)} by sprocket"]
    out["unanswerable"] = [f"{_metric_phrase(primary)} in Atlantis",
                           f"{_metric_phrase(primary)} for Zorbo",
                           f"{_metric_phrase(primary)} in Wakanda"]
    out["out_of_scope"] = ["what is the weather in Paris", "write a poem about databases",
                           "who won the world cup"]
    return {name: questions[:per_class] for name, questions in out.items() if questions}


def _near_misses(samples: Dict[str, List[str]], valued: List[str]) -> List[str]:
    """Values one edit past what the index holds — the clarification path's own
    input. Derived from the dataset's values so the corpus stays portable."""
    out: List[str] = []
    for dimension in valued[:4]:
        value = samples[dimension][0]
        if len(value) >= 6 and " " not in value:
            out.append(value[:-2] + "zz")
        else:
            out.append(value.split()[0][:4] + "zz" if value else "zz")
    return [v for v in out if len(v) >= 4]


# --------------------------------------------------------------------------- #
# The oracle — expected values from an independent statement                   #
# --------------------------------------------------------------------------- #

def expectations(spec: dict, dataset: dict, samples: Dict[str, List[str]],
                 limit: int = 24) -> List[Dict[str, Any]]:
    """Questions whose value is also computed here, by a statement this module
    writes rather than the one the DLM chose. Run through `/dlm/reproduce`,
    which forces the rows to be read, so the comparison is the knowing path
    against the reading path."""
    metrics = {m.get("name"): m for m in dataset.get("metrics") or [] if m.get("name")}
    spec_metrics = {k: v for k, v in (spec.get("metrics") or {}).items() if not v.get("hidden")}
    additive = [m for m, v in spec_metrics.items() if v.get("additive", True) and m in metrics]
    distinct = [m for m, v in spec_metrics.items() if not v.get("additive", True) and m in metrics]
    primary = spec.get("default_metric") or (additive[0] if additive else None)
    dims = [k for k, v in (spec.get("dimensions") or {}).items() if not v.get("hidden")]
    valued = [d for d in dims if samples.get(d)]
    table = f"{_identifier(dataset.get('schema_name') or '')}.{_identifier(dataset.get('table_name') or '')}" \
        if dataset.get("schema_name") else _identifier(dataset.get("table_name") or "")
    out: List[Dict[str, Any]] = []

    def add(question: str, sql: str, approximate: bool = False) -> None:
        out.append({"question": question, "sql": sql, "approximate": approximate})

    for name in additive[:6]:
        expression = metrics[name].get("expression") or "COUNT(*)"
        add(f"total {_metric_phrase(name)}", f"SELECT {expression} AS v FROM {table}")
    for name in distinct[:2]:
        expression = metrics[name].get("expression") or "COUNT(*)"
        add(f"total {_metric_phrase(name)}", f"SELECT {expression} AS v FROM {table}",
            approximate=bool(spec_metrics.get(name, {}).get("approximate")))
    if primary:
        expression = metrics[primary].get("expression") or "COUNT(*)"
        for dimension in valued[:6]:
            value = str(samples[dimension][0]).replace("'", "''")
            add(f"{_metric_phrase(primary)} in {samples[dimension][0]}",
                f"SELECT {expression} AS v FROM {table} "
                f"WHERE {_identifier(dimension)} = '{value}'")
        for dimension in valued[:6]:
            value = str(samples[dimension][0]).replace("'", "''")
            add(f"{_metric_phrase(primary)} by {_dim_phrase(dimension)}",
                f"SELECT {expression} AS v FROM {table} "
                f"WHERE {_identifier(dimension)} = '{value}'")
            out[-1]["cell"] = str(samples[dimension][0])
        # Two-filter cells, phrased exactly as the corpus phrases them.
        for index, first in enumerate(valued[:4]):
            for second in valued[index + 1:index + 2]:
                left = samples[first][0]
                right = samples[second][1 % len(samples[second])]
                add(f"{_metric_phrase(primary)} in {left} {right}",
                    f"SELECT {expression} AS v FROM {table} "
                    f"WHERE {_identifier(first)} = '{str(left)}' "
                    f"AND {_identifier(second)} = '{str(right)}'".replace("''", "''"))
    return out[:limit]


def _headline(answer: dict, cell: Optional[str]) -> Optional[float]:
    rows = answer.get("rows") or []
    if not rows:
        return None
    if cell is not None:
        for row in rows:
            if row and str(row[0]) == cell:
                return _number(row[-1])
        return None
    return _number(rows[0][-1])


def _number(value: Any) -> Optional[float]:
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


# --------------------------------------------------------------------------- #
# The run                                                                      #
# --------------------------------------------------------------------------- #

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


# Which observed classes satisfy a corpus bucket. A clarification bucket is
# satisfied by any clarification or refusal — the point is that the DLM did
# not answer a different question — and a refusal bucket by any refusal.
_ACCEPTS: Dict[str, set] = {
    "clarify_value": {"clarify_value", "clarify_metric", "clarify_dimension", "unanswerable"},
    "clarify_metric": {"clarify_metric", "clarify_value", "unanswerable"},
    "clarify_dimension": {"clarify_dimension", "clarify_value", "unanswerable"},
    "unanswerable": {"unanswerable", "clarify_value", "clarify_metric", "clarify_dimension"},
    "out_of_scope": {"out_of_scope"},
    # A time question over a dataset whose whole span is one period is
    # legitimately answered as the ungrouped shape.
    "time_slice": {"time_slice", "total", "filter"},
    "trend": {"trend", "breakdown", "total"},
}


def run(client: Client, dataset_id: str, per_class: int, check: bool = False,
        on_question=None) -> Dict[str, Any]:
    spec_response = client.call("GET", f"/datasets/{dataset_id}/dlm/context")
    spec = spec_response.get("effective") or {}
    coverage = client.call("GET", "/dlm/coverage")
    entry = next((d for d in coverage.get("datasets", []) if str(d.get("dataset_id")) == str(dataset_id)), {})
    samples = {d["column"]: [str(v) for v in d.get("values") or []] for d in entry.get("dimensions") or []}
    samples = {k: v for k, v in samples.items() if v}
    dataset = client.call("GET", f"/datasets/{dataset_id}")
    questions = corpus(spec, samples, entry.get("date_column"), per_class)
    expected = {e["question"]: e for e in expectations(spec, dataset, samples)} if check else {}
    outcomes: Dict[str, Outcome] = {}
    wrong_answers: List[Dict[str, Any]] = []

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
            observed = answer.get("question_class")
            accepts = _ACCEPTS.get(name, {name})
            if observed and observed not in accepts:
                outcome.misclassified += 1
            evidence = answer.get("evidence") or {}
            detail = {"question": question, "result": result, "class": observed,
                      "elapsed_ms": round((time.monotonic() - started) * 1000),
                      "detail": ((evidence.get("execution") or {}).get("detail")),
                      "rows": evidence.get("rows"), "note": answer.get("note"),
                      "answer": answer.get("answer")}

            oracle = expected.get(question)
            if oracle and answer.get("ok"):
                outcome.checked += 1
                truth = _oracle_value(client, dataset_id, oracle)
                got = _headline(answer, oracle.get("cell"))
                verdict = _compare(got, truth, oracle.get("approximate"))
                detail["expected"] = truth
                detail["got"] = got
                detail["verdict"] = verdict
                if verdict == "wrong":
                    outcome.wrong += 1
                    wrong_answers.append({"class": name, "question": question,
                                          "expected": truth, "got": got,
                                          "sql": oracle["sql"]})
            outcome.questions.append(detail)
            if on_question:
                on_question(name, detail)

    return {"dataset_id": str(dataset_id),
            "dataset_name": spec_response.get("dataset_name") or entry.get("name"),
            "source": entry.get("row_count_source"), "row_count": entry.get("row_count"),
            "shape": bool(((spec.get("derived_from") or {}).get("shape_declared"))),
            "time": spec.get("time"),
            "classes": {name: vars(o) for name, o in outcomes.items()},
            "wrong_answers": wrong_answers}


def _oracle_value(client: Client, dataset_id: str, oracle: dict) -> Optional[float]:
    try:
        result = client.call("POST", "/dlm/reproduce",
                             {"dataset_id": str(dataset_id), "sql": oracle["sql"]})
    except urllib.error.HTTPError as error:
        return None
    except (urllib.error.URLError, TimeoutError):
        return None
    if not result.get("ok"):
        return None
    rows = result.get("rows") or []
    return _number(rows[0][-1]) if rows and rows[0] else None


def _compare(got: Optional[float], truth: Optional[float], approximate: bool) -> str:
    if truth is None or got is None:
        return "unchecked"
    if approximate:
        bound = APPROXIMATE_TOLERANCE * max(abs(truth), 1.0)
        return "right" if abs(got - truth) <= bound else "wrong"
    return "right" if abs(got - truth) < 1e-6 else "wrong"


# --------------------------------------------------------------------------- #
# The report                                                                   #
# --------------------------------------------------------------------------- #

_HEADER = ("class", "asked", "answered", "clarified", "refused", "failed",
           "from context", "cache", "live", "checked", "wrong")
# A class answering less than this is listed as open rather than hidden.
OPEN_THRESHOLD = 0.9


def render(report: Dict[str, Any], markdown: bool) -> str:
    rows = []
    totals = Outcome()
    for name, _description in CLASSES:
        o = report["classes"].get(name)
        if not o:
            continue
        rows.append((name, o["asked"], o["answered"], o["clarified"], o["refused"], o["failed"],
                     o["context"], o["cache"], o["live"], o["checked"], o["wrong"]))
        for key in ("asked", "answered", "clarified", "refused", "failed", "context", "cache",
                    "live", "checked", "wrong", "misclassified"):
            setattr(totals, key, getattr(totals, key) + o[key])
    lines = []
    if markdown:
        lines.append("| " + " | ".join(_HEADER) + " |")
        lines.append("|" + "|".join("---" for _ in _HEADER) + "|")
        for row in rows:
            lines.append("| " + " | ".join(str(c) for c in row) + " |")
        lines.append("| **all** | " + " | ".join(str(c) for c in (
            totals.asked, totals.answered, totals.clarified, totals.refused, totals.failed,
            totals.context, totals.cache, totals.live, totals.checked, totals.wrong)) + " |")
    else:
        widths = [max(len(str(r[i])) for r in [_HEADER, *rows]) for i in range(len(_HEADER))]
        lines.append("  ".join(str(h).ljust(widths[i]) for i, h in enumerate(_HEADER)))
        for row in rows:
            lines.append("  ".join(str(c).ljust(widths[i]) for i, c in enumerate(row)))
        lines.append("  ".join(str(c).ljust(widths[i]) for i, c in enumerate((
            "all", totals.asked, totals.answered, totals.clarified, totals.refused, totals.failed,
            totals.context, totals.cache, totals.live, totals.checked, totals.wrong))))
    answered_pct = round(100.0 * totals.answered / totals.asked, 1) if totals.asked else 0.0
    context_pct = round(100.0 * totals.context / totals.answered, 1) if totals.answered else 0.0
    lines.append("")
    lines.append(f"Answered {totals.answered} of {totals.asked} ({answered_pct}%); "
                 f"from context {totals.context} of {totals.answered} answered ({context_pct}%); "
                 f"cache {totals.cache}; live {totals.live}; clarified {totals.clarified}; "
                 f"refused {totals.refused}; failed {totals.failed}; "
                 f"checked {totals.checked}; wrong {totals.wrong}; "
                 f"misclassified {totals.misclassified}.")
    still_open = open_classes(report)
    if still_open:
        lines.append("")
        lines.append("Open (below 90% answered, or with a wrong answer): "
                     + ", ".join(f"{name} ({why})" for name, why in still_open) + ".")
    return "\n".join(lines)


def open_classes(report: Dict[str, Any]) -> List[Tuple[str, str]]:
    """Classes this run does not cover. A class below 90 % answered is listed,
    and so is one with a wrong answer — a class that answers everything and
    gets one of them wrong is not covered either."""
    out: List[Tuple[str, str]] = []
    refusal = {c.name for c in classes.CLASSES if c.kind == "refusal"}
    for name, o in report["classes"].items():
        if not o["asked"]:
            continue
        if o["wrong"]:
            out.append((name, f"{o['wrong']} wrong of {o['checked']} checked"))
            continue
        if name in refusal:
            # A refusal class is covered when it refused or clarified, not
            # when it answered.
            held = o["clarified"] + o["refused"]
            if held < o["asked"]:
                out.append((name, f"answered {o['asked'] - held} it should have held"))
            continue
        if o["answered"] < OPEN_THRESHOLD * o["asked"]:
            out.append((name, f"{o['answered']} of {o['asked']} answered"))
    return sorted(out)


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(prog="python -m dlm.coverage",
                                     description=__doc__.split("\n\n")[0])
    parser.add_argument("--dataset", required=True, action="append",
                        help="dataset id; repeat for more than one")
    parser.add_argument("--api", default="http://localhost:8082/api/v1", help="platform API base URL")
    parser.add_argument("--token", default=None, help="bearer token, when the API needs one")
    parser.add_argument("--header", action="append", default=[],
                        help="extra request header, 'Name: value'; repeat")
    parser.add_argument("--per-class", type=int, default=12, help="questions per class at most")
    parser.add_argument("--timeout", type=float, default=300.0, help="seconds to wait for one answer")
    parser.add_argument("--check", action="store_true",
                        help="also compute expected values with an independent statement")
    parser.add_argument("--markdown", action="store_true", help="print the table as Markdown")
    parser.add_argument("--questions", action="store_true", help="print every question with its outcome")
    parser.add_argument("--json", dest="as_json", action="store_true", help="print the report as JSON")
    args = parser.parse_args(argv)
    headers = {}
    for raw in args.header:
        name, _, value = raw.partition(":")
        if name and value:
            headers[name.strip()] = value.strip()
    client = Client(args.api, args.token, args.timeout, headers)

    def progress(name: str, detail: Dict[str, Any]) -> None:
        if args.questions and not args.as_json:
            print(f"  [{name}] {detail['question']!r}: {detail['result']}"
                  + (f" as {detail['class']}" if detail.get("class") else "")
                  + (f" ({detail['detail']})" if detail.get("detail") else "")
                  + (f" expected {detail['expected']} got {detail['got']}"
                     if detail.get("verdict") == "wrong" else "")
                  + f" {detail['elapsed_ms']} ms", file=sys.stderr)

    reports = [run(client, dataset_id, args.per_class, check=args.check, on_question=progress)
               for dataset_id in args.dataset]
    if args.as_json:
        print(json.dumps(reports if len(reports) > 1 else reports[0], indent=2))
        return 0
    for report in reports:
        print(f"Dataset {report['dataset_id']} ({report['dataset_name']}), "
              f"{report['row_count']} rows, "
              f"{'declared shape' if report['shape'] else 'no declared shape'}")
        print()
        print(render(report, args.markdown))
        if report["wrong_answers"]:
            print()
            print("Wrong answers:")
            for wrong in report["wrong_answers"]:
                print(f"  [{wrong['class']}] {wrong['question']!r}: "
                      f"expected {wrong['expected']}, got {wrong['got']} ({wrong['sql']})")
        if args.questions and args.markdown:
            print()
            print("| class | question | outcome | answered as | detail | ms |")
            print("|---|---|---|---|---|---|")
            for name, _ in CLASSES:
                for q in report["classes"].get(name, {}).get("questions", []):
                    print(f"| {name} | {q['question']} | {q['result']} | {q.get('class') or ''} "
                          f"| {(q.get('detail') or '')} | {q['elapsed_ms']} |")
        print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
