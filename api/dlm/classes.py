"""Question classes — what the DLM can answer, named.

A *class* is one shape of question with one SQL shape and one answer
sentence. Naming them is the point: a class is testable on its own, the
coverage harness reports per class, and the documentation can say what the
DLM answers without hand-waving. Nothing here parses free text into SQL —
`dlm.engine.ask` resolves a question to slots (metric, breakdown, filters,
time window, ranking) and this module says which class those slots are, what
extra statements the class needs, and how to say the answer.

Two kinds of class:

**Base** — one statement over the resolved slots. `total`, `breakdown`,
`filter`, `filter_breakdown`, `two_filters`, `top_n`, `distinct_total`,
`distinct_breakdown`, `time_slice`, `trend`.

**Derived** — composed in Python from two or three base results, so a
comparison is two windows of the same statement rather than a second SQL
dialect to maintain. `comparison_period`, `year_over_year`,
`share_of_total`, `ratio`, `top_n_within`, `existence`, `vague_default`.

Every derived class keeps the evidence of each base result it composed, so a
percentage can be traced to the two numbers and the two statements behind it.
"""
from __future__ import annotations

import calendar
import re
from dataclasses import dataclass, field
from datetime import date
from typing import Any, Dict, List, Optional, Sequence, Tuple


@dataclass(frozen=True)
class QuestionClass:
    name: str
    kind: str                     # "base" | "derived" | "refusal"
    description: str
    sql_shape: str
    sentence: str


CLASSES: Tuple[QuestionClass, ...] = (
    QuestionClass("total", "base", "The metric alone: a grand total",
                  "SELECT <agg> FROM t",
                  "{metric} is {value}."),
    QuestionClass("breakdown", "base", "The metric by one dimension",
                  "SELECT d, <agg> FROM t GROUP BY d",
                  "{metric} by {dimension}: {lead}."),
    QuestionClass("filter", "base", "The metric for one dimension value",
                  "SELECT <agg> FROM t WHERE d = v",
                  "{metric} in {filters} is {value}."),
    QuestionClass("filter_breakdown", "base",
                  "The metric by one dimension for a value of another",
                  "SELECT d2, <agg> FROM t WHERE d1 = v GROUP BY d2",
                  "{metric} by {dimension} in {filters}: {lead}."),
    QuestionClass("two_filters", "base", "The metric for values of two dimensions",
                  "SELECT <agg> FROM t WHERE d1 = v1 AND d2 = v2",
                  "{metric} in {filters} is {value}."),
    QuestionClass("top_n", "base", "A ranking over one dimension",
                  "SELECT d, <agg> FROM t GROUP BY d ORDER BY 2 DESC LIMIT n",
                  "Top {n} {dimension} by {metric}: {lead}."),
    QuestionClass("distinct_total", "base",
                  "A non-additive count of distinct values, answered approximately",
                  "SELECT APPROX_COUNT_DISTINCT(c) FROM t",
                  "{metric} is about {value} (approximate, {error})."),
    QuestionClass("distinct_breakdown", "base",
                  "A non-additive distinct count by one dimension, approximate",
                  "SELECT d, APPROX_COUNT_DISTINCT(c) FROM t GROUP BY d",
                  "{metric} by {dimension} (approximate, {error}): {lead}."),
    QuestionClass("time_slice", "base", "The metric in one named period",
                  "SELECT <agg> FROM t WHERE date >= lo AND date < hi",
                  "{metric} in {period} is {value}."),
    QuestionClass("trend", "base", "The metric over the dataset's time dimension",
                  "SELECT date, <agg> FROM t GROUP BY date ORDER BY date",
                  "{metric} over {periods} {grain}s, from {first} to {last}."),

    QuestionClass("comparison_period", "derived",
                  "The metric in one period against the period before it",
                  "two time_slice statements, one per window",
                  "{metric} in {period} is {value}, {direction} {delta} ({percent}) "
                  "from {previous_period}'s {previous_value}."),
    QuestionClass("year_over_year", "derived",
                  "The metric in one year against the same span a year earlier",
                  "two time_slice statements, one year apart",
                  "{metric} in {period} is {value}, {direction} {percent} year over year "
                  "from {previous_value} in {previous_period}."),
    QuestionClass("share_of_total", "derived",
                  "One slice of the metric as a share of the whole",
                  "a filter or breakdown statement and the grand total",
                  "{subject} is {percent} of {metric} ({value} of {total})."),
    QuestionClass("ratio", "derived", "One measure divided by another",
                  "two statements, one per measure, over the same slots",
                  "{numerator} per {denominator} is {value} ({numerator_value} / {denominator_value})."),
    QuestionClass("top_n_within", "derived",
                  "A ranking over one dimension inside each value of another",
                  "one statement grouped by both dimensions, ranked per outer group",
                  "Top {n} {inner} by {metric} within each {outer}: {lead}."),
    QuestionClass("existence", "derived",
                  "How many values of a dimension clear a threshold",
                  "a breakdown statement, counted against the threshold",
                  "{count} of {total} {dimension} have {metric} {comparison} {threshold}."),
    QuestionClass("vague_default", "derived",
                  "A question with no explicit measure or period, answered against "
                  "the spec's defaults and stated as such",
                  "the default metric's statement at the dataset's latest period",
                  "{metric} at {period}, the latest the data holds, is {value}."),

    QuestionClass("clarify_value", "refusal",
                  "A word that resolves to no indexed value, or to more than one column",
                  "no statement is run",
                  "{prompt}"),
    QuestionClass("clarify_metric", "refusal", "Two measures read the question equally well",
                  "no statement is run", "{prompt}"),
    QuestionClass("clarify_dimension", "refusal", "Two dimensions read the breakdown equally well",
                  "no statement is run", "{prompt}"),
    QuestionClass("unanswerable", "refusal",
                  "A question this dataset's spec genuinely cannot answer",
                  "no statement is run",
                  "This dataset cannot answer that. The closest it can answer is: {closest}."),
    QuestionClass("out_of_scope", "refusal", "A question about nothing the platform holds",
                  "no dataset is consulted",
                  "That is outside the data Kaveon holds."),
)

BY_NAME: Dict[str, QuestionClass] = {c.name: c for c in CLASSES}
NAMES: Tuple[str, ...] = tuple(c.name for c in CLASSES)
DERIVED: Tuple[str, ...] = tuple(c.name for c in CLASSES if c.kind == "derived")
BASE: Tuple[str, ...] = tuple(c.name for c in CLASSES if c.kind == "base")


# --------------------------------------------------------------------------- #
# Periods                                                                      #
# --------------------------------------------------------------------------- #

_MONTHS = ("january", "february", "march", "april", "may", "june", "july",
           "august", "september", "october", "november", "december")


def period_bounds(period: str, grain: str) -> Tuple[str, str]:
    """`[lo, hi)` as ISO days for a period written at its own grain: `2026`,
    `2026-08` or `2026-08-18`."""
    text = str(period)
    if grain == "year" or len(text) == 4:
        year = int(text[:4])
        return f"{year}-01-01", f"{year + 1}-01-01"
    if grain == "month" or len(text) == 7:
        year, month = int(text[:4]), int(text[5:7])
        last = calendar.monthrange(year, month)[1]
        end = date(year, month, last).toordinal() + 1
        return f"{year:04d}-{month:02d}-01", date.fromordinal(end).isoformat()
    day = date.fromisoformat(text[:10])
    return day.isoformat(), date.fromordinal(day.toordinal() + 1).isoformat()


def previous_period(period: str, grain: str, years_back: int = 0) -> str:
    """The period before this one at the same grain, or the same period
    `years_back` years earlier."""
    text = str(period)
    if years_back:
        return f"{int(text[:4]) - years_back}{text[4:]}"
    if grain == "year" or len(text) == 4:
        return str(int(text[:4]) - 1)
    if grain == "month" or len(text) == 7:
        year, month = int(text[:4]), int(text[5:7])
        return f"{year - 1:04d}-12" if month == 1 else f"{year:04d}-{month - 1:02d}"
    day = date.fromisoformat(text[:10])
    return date.fromordinal(day.toordinal() - 1).isoformat()


def period_label(period: str, grain: str) -> str:
    text = str(period)
    if grain == "year" or len(text) == 4:
        return text[:4]
    if grain == "month" or len(text) == 7:
        return f"{_MONTHS[int(text[5:7]) - 1].title()} {text[:4]}"
    return text[:10]


def clamp_period(period: str, grain: str, earliest: Optional[str],
                 latest: Optional[str]) -> Optional[str]:
    """None when the period lies outside what the dataset holds — the caller
    says so rather than answering zero."""
    lo, _hi = period_bounds(period, grain)
    if earliest and lo < str(earliest)[:10][:len(str(earliest)[:10])] and \
            period_bounds(period, grain)[1] <= str(earliest)[:10]:
        return None
    if latest and lo > str(latest)[:10]:
        return None
    return period


# --------------------------------------------------------------------------- #
# Detection — which class the question is, given the slots already resolved    #
# --------------------------------------------------------------------------- #

_VS_PREVIOUS = re.compile(
    r"\b(?:vs\.?|versus|compared\s+to|against|change\s+from)\s+"
    r"(?:the\s+)?(?:last|previous|prior)\s+(year|month|quarter|week|day)\b|"
    r"\b(?:month[\s-]over[\s-]month|mom)\b|"
    r"\b(?:week[\s-]over[\s-]week|wow)\b|"
    r"\bcompared\s+(?:to|with)\s+(?:the\s+)?(?:last|previous|prior)\s+(year|month|quarter|week|day)\b",
    re.I)
_YOY = re.compile(r"\b(?:year[\s-]over[\s-]year|yoy|year[\s-]on[\s-]year)\b|"
                  r"\b(?:vs\.?|versus|compared\s+to|against)\s+(?:the\s+)?(?:last|previous|prior)\s+year\b", re.I)
_EXPLICIT_VS = re.compile(
    r"\b(?P<a>[A-Z][a-z]+\s+\d{4}|\d{4}-\d{2}|\d{4})\s+(?:vs\.?|versus|compared\s+to)\s+"
    r"(?P<b>[A-Z][a-z]+\s+\d{4}|[A-Z][a-z]+|\d{4}-\d{2}|\d{4})\b", re.I)
_SHARE = re.compile(r"\b(?:share\s+of|percentage\s+of|percent\s+of|what\s+(?:share|percentage|percent)|"
                    r"as\s+a\s+(?:share|percentage|percent)|proportion\s+of|%\s+of)\b", re.I)
_RATIO = re.compile(r"\b(?P<num>[\w ]+?)\s+per\s+(?P<den>[\w ]+?)\b|"
                    r"\bratio\s+of\s+(?P<num2>[\w ]+?)\s+to\s+(?P<den2>[\w ]+?)\b", re.I)
_WITHIN = re.compile(r"\b(?:in|for|within|per)\s+each\s+(?P<outer>[\w ]+?)\s*$|"
                     r"\bby\s+(?P<outer2>[\w ]+?)\s*$", re.I)
_EXISTENCE = re.compile(
    r"\bhow\s+many\s+(?P<dim>[\w ]+?)\s+(?:have|has|had|with)\b.*?"
    r"(?P<cmp>more\s+than|greater\s+than|at\s+least|over|above|fewer\s+than|less\s+than|under|below|at\s+most|exactly)\s+"
    r"(?P<value>[\d.,]+)", re.I)
_VAGUE = re.compile(r"\b(?:current|currently|latest|right\s+now|today'?s?|now|"
                    r"how\s+(?:are|is)\s+(?:we|it|things)|how'?s\s+it\s+going|"
                    r"what'?s\s+(?:the\s+)?(?:state|status)|where\s+do\s+we\s+stand|"
                    r"give\s+me\s+(?:an?\s+)?(?:overview|summary|snapshot))\b", re.I)
_TREND = re.compile(r"\b(trend|over\s+time|by\s+year|by\s+month|by\s+day|yearly|monthly|daily|"
                    r"over\s+the\s+years|by\s+week|weekly)\b", re.I)


# The words the derived grammar above accounts for. They are grammar, not data,
# so an unresolved-term check must not treat "than" in "more than 150" or "per"
# in "errors per query" as a filter value the dataset failed to recognise.
GRAMMAR_WORDS = frozenset({
    "vs", "versus", "compared", "comparing", "against", "change", "changed",
    "last", "previous", "prior", "year", "month", "quarter", "week", "day",
    "yoy", "mom", "wow", "over", "on",
    "share", "percentage", "percent", "proportion", "ratio", "per", "each",
    "within", "how", "many", "much", "have", "has", "had", "with",
    "more", "than", "greater", "least", "above", "fewer", "less", "under",
    "below", "most", "exactly", "at",
    "current", "currently", "latest", "right", "now", "today", "yesterday",
    "going", "state", "status", "stand", "overview", "summary", "snapshot",
    "trend", "time", "usage", "us", "we", "it", "things",
})


@dataclass
class Intent:
    """A derived class the question asks for, with what the composer needs."""
    name: str
    params: Dict[str, Any] = field(default_factory=dict)


def detect_derived(question: str, *, has_time: bool, dimensions: Sequence[str],
                   measures: Sequence[str], has_group: bool,
                   explicit_metric: bool, explicit_period: bool) -> Optional[Intent]:
    """The derived class this question asks for, or None for a base one.
    Order matters: the most specific reading wins, and a class whose
    prerequisite the dataset does not have (no time dimension, one measure)
    is never claimed."""
    if has_time and _YOY.search(question):
        return Intent("year_over_year")
    if has_time and (_VS_PREVIOUS.search(question) or _EXPLICIT_VS.search(question)):
        return Intent("comparison_period", {"explicit": _EXPLICIT_VS.search(question) is not None})
    if _SHARE.search(question):
        return Intent("share_of_total")
    existence = _EXISTENCE.search(question)
    if existence:
        return Intent("existence", {
            "dimension_phrase": existence.group("dim").strip(),
            "comparison": _COMPARISONS[existence.group("cmp").lower().replace("  ", " ")],
            "threshold": float(existence.group("value").replace(",", "")),
        })
    ratio = _RATIO.search(question)
    if ratio and len(measures) > 1:
        numerator = (ratio.group("num") or ratio.group("num2") or "").strip()
        denominator = (ratio.group("den") or ratio.group("den2") or "").strip()
        if numerator and denominator:
            return Intent("ratio", {"numerator_phrase": numerator, "denominator_phrase": denominator})
    within = re.search(r"\b(?:in|for|within)\s+each\s+(?P<outer>[\w ]+?)\s*$", question, re.I)
    if within and has_group:
        return Intent("top_n_within", {"outer_phrase": within.group("outer").strip()})
    if _VAGUE.search(question) and not explicit_metric and not has_group and not explicit_period:
        return Intent("vague_default")
    return None


_COMPARISONS = {
    "more than": ">", "greater than": ">", "over": ">", "above": ">",
    "at least": ">=", "fewer than": "<", "less than": "<", "under": "<",
    "below": "<", "at most": "<=", "exactly": "==",
}
_COMPARISON_WORDS = {">": "above", ">=": "at least", "<": "below", "<=": "at most",
                     "==": "exactly"}


def base_class(*, group_cols: Sequence[str], filters: Sequence[dict], time_group: Optional[str],
               period: Optional[str], top_n: Optional[int], distinct: bool) -> str:
    """The base class one resolved slot set is. Exactly one name, decided in a
    fixed order so two runs over the same question agree."""
    if time_group:
        return "trend"
    if distinct:
        return "distinct_breakdown" if group_cols else "distinct_total"
    if top_n and group_cols:
        return "top_n"
    if period:
        return "time_slice"
    if group_cols and filters:
        return "filter_breakdown"
    if group_cols:
        return "breakdown"
    if len(filters) >= 2:
        return "two_filters"
    if filters:
        return "filter"
    return "total"


# --------------------------------------------------------------------------- #
# Composition — the derived answers, built from base results                   #
# --------------------------------------------------------------------------- #

def _number(value: Any) -> Optional[float]:
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def _scalar(result: Optional[dict]) -> Optional[float]:
    rows = (result or {}).get("rows") or []
    if not rows or not rows[0]:
        return None
    return _number(rows[0][-1])


def fmt(value: Any) -> str:
    number = _number(value)
    if number is None:
        return str(value)
    if abs(number - round(number)) < 1e-9:
        return f"{int(round(number)):,}"
    return f"{number:,.2f}"


def percent(part: Optional[float], whole: Optional[float]) -> Optional[float]:
    if part is None or not whole:
        return None
    return 100.0 * part / whole


def fmt_percent(value: Optional[float]) -> str:
    return "—" if value is None else f"{value:.1f}%"


def compose_comparison(current: dict, previous: dict, *, metric: str, period: str,
                       previous_label: str, year_over_year: bool = False) -> Dict[str, Any]:
    """Two windows of one statement into one answer: the level, the change and
    the percent change, with both numbers kept."""
    now, before = _scalar(current), _scalar(previous)
    delta = None if now is None or before is None else now - before
    change = percent(delta, before) if before else None
    direction = "unchanged" if not delta else ("up" if delta > 0 else "down")
    name = "year_over_year" if year_over_year else "comparison_period"
    if before is None:
        sentence = (f"{metric} in {period} is {fmt(now)}. The data holds no {previous_label} "
                    f"to compare it with.")
    else:
        sentence = BY_NAME[name].sentence.format(
            metric=metric, period=period, value=fmt(now), direction=direction,
            delta=fmt(abs(delta)) if delta is not None else "—",
            percent=fmt_percent(abs(change) if change is not None else None),
            previous_period=previous_label, previous_value=fmt(before))
    return {
        "question_class": name,
        "columns": ["period", metric],
        "rows": [[previous_label, before], [period, now]],
        "answer": sentence,
        "chartType": "bar", "xAxis": "period", "yAxis": metric,
        "title": f"{metric} — {period} vs {previous_label}",
        "comparison": {"current": now, "previous": before, "delta": delta,
                       "percent_change": change, "period": period,
                       "previous_period": previous_label},
    }


def compose_share(part: dict, total: dict, *, metric: str, subject: str,
                  group_col: Optional[str]) -> Dict[str, Any]:
    """A slice, or every slice of a breakdown, as a share of the grand total."""
    whole = _scalar(total)
    rows = part.get("rows") or []
    if group_col and rows and len(rows[0]) >= 2:
        out_rows = [[row[0], row[-1], percent(_number(row[-1]), whole)] for row in rows]
        out_rows.sort(key=lambda r: (r[2] is None, -(r[2] or 0)))
        lead = ", ".join(f"{r[0]} {fmt_percent(r[2])}" for r in out_rows[:3])
        sentence = (f"{metric} by {group_col} as a share of {fmt(whole)}: {lead}."
                    if whole else f"{metric} by {group_col}: {lead}.")
        return {
            "question_class": "share_of_total",
            "columns": [group_col, metric, "share %"],
            "rows": out_rows, "answer": sentence,
            "chartType": "bar", "xAxis": group_col, "yAxis": "share %",
            "title": f"{metric} by {group_col} — share of total",
            "share": {"total": whole},
        }
    slice_value = _scalar(part)
    share = percent(slice_value, whole)
    sentence = BY_NAME["share_of_total"].sentence.format(
        subject=subject, percent=fmt_percent(share), metric=metric,
        value=fmt(slice_value), total=fmt(whole))
    return {
        "question_class": "share_of_total",
        "columns": [metric, "total", "share %"],
        "rows": [[slice_value, whole, share]],
        "answer": sentence, "chartType": "kpi", "xAxis": None, "yAxis": "share %",
        "title": f"{subject} — share of {metric}",
        "share": {"part": slice_value, "total": whole, "percent": share},
    }


def compose_ratio(numerator: dict, denominator: dict, *, numerator_name: str,
                  denominator_name: str, group_col: Optional[str]) -> Dict[str, Any]:
    """One measure over another, cell by cell when both are broken down."""
    if group_col:
        left = {str(row[0]): _number(row[-1]) for row in (numerator.get("rows") or [])}
        right = {str(row[0]): _number(row[-1]) for row in (denominator.get("rows") or [])}
        label = f"{numerator_name} per {denominator_name}"
        rows = [[key, left[key], right.get(key),
                 (left[key] / right[key]) if left.get(key) is not None and right.get(key) else None]
                for key in left]
        rows.sort(key=lambda r: (r[3] is None, -(r[3] or 0)))
        lead = ", ".join(f"{r[0]} {fmt(r[3])}" for r in rows[:3] if r[3] is not None)
        return {
            "question_class": "ratio",
            "columns": [group_col, numerator_name, denominator_name, label],
            "rows": rows, "answer": f"{label} by {group_col}: {lead}.",
            "chartType": "bar", "xAxis": group_col, "yAxis": label,
            "title": f"{label} by {group_col}",
        }
    top, bottom = _scalar(numerator), _scalar(denominator)
    value = (top / bottom) if top is not None and bottom else None
    sentence = BY_NAME["ratio"].sentence.format(
        numerator=numerator_name, denominator=denominator_name,
        value=fmt(value) if value is not None else "—",
        numerator_value=fmt(top), denominator_value=fmt(bottom))
    return {
        "question_class": "ratio",
        "columns": [numerator_name, denominator_name, f"{numerator_name} per {denominator_name}"],
        "rows": [[top, bottom, value]], "answer": sentence,
        "chartType": "kpi", "xAxis": None, "yAxis": f"{numerator_name} per {denominator_name}",
        "title": f"{numerator_name} per {denominator_name}",
        "ratio": {"numerator": top, "denominator": bottom, "value": value},
    }


def compose_top_n_within(result: dict, *, metric: str, outer: str, inner: str,
                         n: int) -> Dict[str, Any]:
    """A two-dimension breakdown ranked inside each value of the outer
    dimension — the window function the DLM does not need to write."""
    columns = result.get("columns") or []
    rows = result.get("rows") or []
    try:
        outer_index, inner_index = columns.index(outer), columns.index(inner)
    except ValueError:
        outer_index, inner_index = 0, 1
    groups: Dict[str, List[list]] = {}
    for row in rows:
        groups.setdefault(str(row[outer_index]), []).append(row)
    out: List[list] = []
    for key in sorted(groups):
        ranked = sorted(groups[key], key=lambda r: (r[-1] is None, -(_number(r[-1]) or 0)))
        for rank, row in enumerate(ranked[:n], start=1):
            out.append([key, row[inner_index], row[-1], rank])
    lead = "; ".join(f"{row[0]}: {row[1]}" for row in out if row[3] == 1)
    sentence = BY_NAME["top_n_within"].sentence.format(
        n=n, inner=inner, outer=outer, metric=metric, lead=lead)
    return {
        "question_class": "top_n_within",
        "columns": [outer, inner, metric, "rank"],
        "rows": out, "answer": sentence,
        "chartType": "bar", "xAxis": inner, "yAxis": metric,
        "title": f"Top {n} {inner} by {metric} within each {outer}",
    }


def compose_existence(result: dict, *, metric: str, dimension: str, comparison: str,
                      threshold: float) -> Dict[str, Any]:
    """How many values of a dimension clear a threshold — counted from the
    breakdown, with the ones that do listed."""
    rows = result.get("rows") or []
    tests = {">": lambda v: v > threshold, ">=": lambda v: v >= threshold,
             "<": lambda v: v < threshold, "<=": lambda v: v <= threshold,
             "==": lambda v: v == threshold}
    test = tests.get(comparison, tests[">"])
    matching = [row for row in rows if _number(row[-1]) is not None and test(_number(row[-1]))]
    matching.sort(key=lambda r: -(_number(r[-1]) or 0))
    sentence = BY_NAME["existence"].sentence.format(
        count=len(matching), total=len(rows), dimension=dimension, metric=metric,
        comparison=_COMPARISON_WORDS.get(comparison, comparison), threshold=fmt(threshold))
    if matching:
        sentence += " " + ", ".join(f"{row[0]} ({fmt(row[-1])})" for row in matching[:5]) + "."
    return {
        "question_class": "existence",
        "columns": [dimension, metric], "rows": matching,
        "answer": sentence, "chartType": "bar", "xAxis": dimension, "yAxis": metric,
        "title": f"{dimension} with {metric} {_COMPARISON_WORDS.get(comparison, comparison)} {fmt(threshold)}",
        "existence": {"matched": len(matching), "considered": len(rows),
                      "comparison": comparison, "threshold": threshold},
    }


def vague_sentence(metric: str, period: Optional[str], value: Any) -> str:
    if period:
        return BY_NAME["vague_default"].sentence.format(
            metric=metric, period=period, value=fmt(value))
    return f"{metric}, the dataset's headline measure, is {fmt(value)} across everything it holds."


def base_sentence(name: str, *, metric: str, value: Any = None, dimension: Optional[str] = None,
                  filters: Optional[Sequence[dict]] = None, rows: Optional[Sequence[Sequence]] = None,
                  n: Optional[int] = None, period: Optional[str] = None,
                  error: Optional[str] = None, grain: str = "period") -> str:
    """The template answer sentence for a base class. Kept here so the class
    list and the words a person reads cannot drift apart."""
    filter_text = ", ".join(f"{f.get('value')}" for f in filters or []) or "everything"
    lead = ", ".join(f"{row[0]} {fmt(row[-1])}" for row in (rows or [])[:3])
    if name == "total":
        return f"{metric} is {fmt(value)}."
    if name == "filter":
        return f"{metric} in {filter_text} is {fmt(value)}."
    if name == "two_filters":
        return f"{metric} in {filter_text} is {fmt(value)}."
    if name == "breakdown":
        return f"{metric} by {dimension}: {lead}."
    if name == "filter_breakdown":
        return f"{metric} by {dimension} in {filter_text}: {lead}."
    if name == "top_n":
        return f"Top {n} {dimension} by {metric}: {lead}."
    if name == "distinct_total":
        return f"{metric} is about {fmt(value)} (approximate{', ' + error if error else ''})."
    if name == "distinct_breakdown":
        return f"{metric} by {dimension} (approximate{', ' + error if error else ''}): {lead}."
    if name == "time_slice":
        return f"{metric} in {period} is {fmt(value)}."
    if name == "trend":
        first = (rows or [[None]])[0][0] if rows else None
        last = (rows or [[None]])[-1][0] if rows else None
        return f"{metric} over {len(rows or [])} {grain}s, from {first} to {last}."
    return f"{metric}: {fmt(value)}."


def documented() -> List[Dict[str, str]]:
    """The class list as the documentation and the coverage report print it."""
    return [{"name": c.name, "kind": c.kind, "description": c.description,
             "sql_shape": c.sql_shape, "sentence": c.sentence} for c in CLASSES]
