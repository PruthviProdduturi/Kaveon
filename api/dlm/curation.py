"""Auto-curation — a dataset's context spec derived from what the Engine knows.

The DLM used to guess a dataset's semantics from column names: a text column
was a dimension, a numeric column was a measure unless its name ended in
`_id`, the first date column was the time column. That is a heuristic over
spelling, and it is wrong exactly where it matters — a numeric code is not a
measure, a free-text column is not a breakdown, and "current" is not today.

This module derives the same spec from the Engine's own record instead:

| Spec element | Derived from |
|---|---|
| dimension vs measure vs identifier | the column's distinct count (HyperLogLog p=12, or `distinct_exact` when one is on record) against the table's row count |
| a dimension's `top_n` (how deep the value index goes) | the same distinct count |
| a measure's range and outlier-safe default | the KLL quantile sketch (p01/p50/p99) and the record's exact bounds |
| a column's `optional` flag | the record's `null_count` |
| the time dimension, its grain, and what "current"/"latest" means | the temporal column's bounds — **the maximum date in the data, never today's date** — and the distinct count of its values against the span |
| which dimensions the value index enumerates | the low-cardinality dimensions the statistics name |

Where the table declares a shape (`ALTER TABLE … SET SHAPE (…)`), the shape
wins: a declared dimension is a dimension whatever its cardinality, a
declared measure is a measure under the aggregates it is declared with, and
the declared time column is the time dimension. The statistics still fill in
the cardinalities, ranges, null counts and bounds around it.

Every derived element carries the evidence it came from — which statistic,
at which source version, observed when — so the curation editor can show a
curator *why* a column is a dimension rather than asking them to trust it.

Nothing here reads a row. The distinct counts and quantiles come from one
`SELECT APPROX_COUNT_DISTINCT(…), APPROX_PERCENTILE(…) FROM t` answered by
the Engine from its statistics (`execution.mode = "context"`), and the probe
is only sent when the record says every column it names carries the sketch
it needs — so a table without a full `ANALYZE` degrades to the record's
metadata-depth facts rather than paying for a scan.
"""
from __future__ import annotations

import logging
import re
from datetime import date
from typing import Any, Dict, List, Optional, Sequence, Tuple

logger = logging.getLogger(__name__)

# A column whose distinct count is at least this share of the table's rows is
# an identifier, not a dimension: it names a row rather than grouping rows.
IDENTIFIER_DISTINCT_RATIO = 0.9
# A dimension the value index will enumerate. Above this a column may still be
# a dimension (a declared one always is) but its values are not indexed.
MAX_INDEXED_DISTINCT = 1_000
# Above this a column that is not declared is free text, not a breakdown.
MAX_DERIVED_DIMENSION_DISTINCT = 5_000
# The deepest the value index goes for one dimension.
MAX_TOP_N = 500
# Quantiles asked of the KLL sketch: an outlier-safe floor, the middle, and an
# outlier-safe ceiling. A generated range uses p01..p99, not min..max, because
# one bad row must not set a dataset's default scale.
PROBE_QUANTILES = (0.01, 0.5, 0.99)

_NUMERIC_TYPES = ("tinyint", "smallint", "integer", "int", "bigint", "real", "double", "float",
                  "decimal", "numeric")
_TEMPORAL_TYPES = ("date", "timestamp", "datetime")
_ISO_DAY = re.compile(r"^\d{4}-\d{2}-\d{2}")
_ISO_MONTH = re.compile(r"^\d{4}-\d{2}$")
_YEAR_ONLY = re.compile(r"^\d{4}$")


# --------------------------------------------------------------------------- #
# The facts the Engine hands over                                              #
# --------------------------------------------------------------------------- #

def _base_type(data_type: Any) -> str:
    return str(data_type or "").split("(", 1)[0].strip().lower()


def _scalar(value: Any) -> Any:
    """The Engine serializes a bound as a one-key tagged object
    (`{"Int": 44000}`, `{"Text": "Web"}`, `{"Date": 20300}`); anything else is
    already a scalar."""
    if isinstance(value, dict) and len(value) == 1:
        return next(iter(value.values()))
    return value


class TableFacts:
    """What the Engine's record and one context-answered probe say about a
    table. Built by :func:`facts_from_engine`; also constructible directly,
    which is how the tests state a table without a stack."""

    def __init__(self, rows: Optional[int] = None, columns: Optional[Dict[str, dict]] = None,
                 source_version: Optional[dict] = None, computed_at_ms: Optional[int] = None,
                 depth: Optional[str] = None, stale: bool = False,
                 shape: Optional[dict] = None, table: Optional[str] = None):
        self.rows = rows
        self.columns: Dict[str, dict] = columns or {}
        self.source_version = source_version
        self.computed_at_ms = computed_at_ms
        self.depth = depth
        self.stale = bool(stale)
        self.shape = shape if isinstance(shape, dict) else None
        self.table = table

    # ── per-column accessors, each None when the record does not say ──
    def column(self, name: str) -> dict:
        return self.columns.get(name) or {}

    def distinct(self, name: str) -> Optional[int]:
        value = self.column(name).get("distinct")
        return int(value) if isinstance(value, (int, float)) else None

    def distinct_basis(self, name: str) -> Optional[str]:
        return self.column(name).get("distinct_basis")

    def null_count(self, name: str) -> Optional[int]:
        value = self.column(name).get("null_count")
        return int(value) if isinstance(value, (int, float)) else None

    def bounds(self, name: str) -> Tuple[Any, Any, bool]:
        column = self.column(name)
        return _scalar(column.get("min")), _scalar(column.get("max")), bool(column.get("bounds_exact"))

    def quantiles(self, name: str) -> Optional[List[float]]:
        value = self.column(name).get("quantiles")
        return [float(v) for v in value] if isinstance(value, list) and value else None

    def data_type(self, name: str) -> str:
        return _base_type(self.column(name).get("data_type"))

    def known(self) -> bool:
        return bool(self.columns) or self.rows is not None

    def evidence(self, statistic: str, basis: Optional[str] = None) -> Dict[str, Any]:
        """The provenance stamp every derived element carries."""
        out: Dict[str, Any] = {"statistic": statistic, "source": "engine_statistics"}
        if basis:
            out["basis"] = basis
        if self.source_version:
            out["source_version"] = self.source_version
        if self.computed_at_ms is not None:
            out["computed_at_ms"] = self.computed_at_ms
        if self.depth:
            out["depth"] = self.depth
        return out


def _declared_evidence(facts: TableFacts, what: str) -> Dict[str, Any]:
    out: Dict[str, Any] = {"statistic": "declared shape", "source": "shape", "basis": what}
    if facts.source_version:
        out["source_version"] = facts.source_version
    return out


# --------------------------------------------------------------------------- #
# Reading the record, and the one probe that costs nothing                     #
# --------------------------------------------------------------------------- #

def _probe_targets(statistics: dict, arrow_types: Dict[str, str]) -> Tuple[List[str], List[str]]:
    """The columns whose distinct count and quantiles the probe may ask for:
    only those the record already carries a sketch for, so the statement is
    answerable from the statistics and never becomes a scan."""
    distinct_cols: List[str] = []
    quantile_cols: List[str] = []
    for column in statistics.get("columns") or []:
        if not isinstance(column, dict):
            continue
        name = str(column.get("name") or "")
        if not name:
            continue
        if column.get("distinct") or column.get("distinct_exact") is not None:
            distinct_cols.append(name)
        if column.get("quantiles") and _base_type(arrow_types.get(name)) in _NUMERIC_TYPES:
            quantile_cols.append(name)
    return distinct_cols, quantile_cols


def probe_statement(table: str, distinct_cols: Sequence[str], quantile_cols: Sequence[str]) -> str:
    """The single statement that reads the sketches. Column names are written
    as the Engine's own identifiers; an alias per output keeps the mapping
    positional-free."""
    def ident(name: str) -> str:
        return name if re.match(r"^[A-Za-z_][A-Za-z0-9_]*$", name) else '"' + name.replace('"', '""') + '"'

    parts = [f"APPROX_COUNT_DISTINCT({ident(c)}) AS d_{i}" for i, c in enumerate(distinct_cols)]
    quantile_list = ", ".join(str(q) for q in PROBE_QUANTILES)
    parts += [f"APPROX_PERCENTILE({ident(c)}, ARRAY[{quantile_list}]) AS q_{i}"
              for i, c in enumerate(quantile_cols)]
    return f"SELECT {', '.join(parts)} FROM {table}"


def facts_from_engine(statistics: Optional[dict], shape: Optional[dict],
                      arrow_types: Dict[str, str], run_probe=None,
                      table: Optional[str] = None) -> TableFacts:
    """Fold the `/statistics` response and (when it is free) one probe into
    :class:`TableFacts`. `run_probe(sql)` must return `(row, execution)` and
    is only called when every column it would name carries its sketch; a probe
    whose record does not say `mode: context` is discarded, because a derived
    spec is never worth a scan."""
    if not isinstance(statistics, dict):
        return TableFacts(shape=shape, table=table)
    record = statistics.get("statistics") if isinstance(statistics.get("statistics"), dict) else {}
    columns: Dict[str, dict] = {}
    for column in record.get("columns") or []:
        if isinstance(column, dict) and column.get("name"):
            name = str(column["name"])
            entry = {"data_type": arrow_types.get(name) or column.get("data_type"),
                     "null_count": column.get("null_count"),
                     "min": column.get("min"), "max": column.get("max"),
                     "bounds_exact": column.get("bounds_exact")}
            if column.get("distinct_exact") is not None:
                entry["distinct"] = column["distinct_exact"]
                entry["distinct_basis"] = "exact count"
            columns[name] = entry

    facts = TableFacts(
        rows=record.get("rows"), columns=columns,
        source_version=record.get("source_version") or statistics.get("source_version"),
        computed_at_ms=record.get("computed_at_ms"), depth=record.get("depth"),
        stale=bool(statistics.get("stale")), shape=shape,
        table=table or statistics.get("table"))

    distinct_cols, quantile_cols = _probe_targets(record, arrow_types)
    if not run_probe or facts.stale or not (distinct_cols or quantile_cols) or not facts.table:
        return facts
    try:
        row, execution = run_probe(probe_statement(facts.table, distinct_cols, quantile_cols))
    except Exception as error:                      # a probe is an optimization, never a failure
        logger.info("Context probe unavailable for %s: %s", facts.table, type(error).__name__)
        return facts
    if not row or (execution or {}).get("mode") != "context":
        logger.info("Context probe for %s was not answered from statistics; discarded", facts.table)
        return facts
    index = 0
    for name in distinct_cols:
        value = row[index] if index < len(row) else None
        index += 1
        if isinstance(value, (int, float)) and columns.get(name) is not None:
            if columns[name].get("distinct_basis") != "exact count":
                columns[name]["distinct"] = int(value)
                columns[name]["distinct_basis"] = "hyperloglog p=12"
    for name in quantile_cols:
        value = row[index] if index < len(row) else None
        index += 1
        if isinstance(value, list) and columns.get(name) is not None:
            columns[name]["quantiles"] = value
    return facts


# --------------------------------------------------------------------------- #
# Roles                                                                        #
# --------------------------------------------------------------------------- #

def _looks_temporal(name: str, data_type: str, lo: Any, hi: Any) -> bool:
    if data_type in _TEMPORAL_TYPES:
        return True
    for bound in (lo, hi):
        if isinstance(bound, str) and (_ISO_DAY.match(bound) or _ISO_MONTH.match(bound)):
            return True
    return False


def _grain(lo: Any, hi: Any, distinct: Optional[int]) -> str:
    """`day`, `month` or `year` — the finest grain the values actually take.
    Decided by the shape of the bounds, then confirmed against how many
    distinct values cover the span."""
    text_lo, text_hi = str(lo or ""), str(hi or "")
    if _YEAR_ONLY.match(text_lo) and _YEAR_ONLY.match(text_hi):
        return "year"
    if _ISO_MONTH.match(text_lo) and _ISO_MONTH.match(text_hi):
        return "month"
    if _ISO_DAY.match(text_lo) and _ISO_DAY.match(text_hi):
        try:
            span = (date.fromisoformat(text_hi[:10]) - date.fromisoformat(text_lo[:10])).days + 1
        except ValueError:
            return "day"
        if distinct and span > 0 and distinct <= max(2, span // 20):
            return "month"
        return "day"
    return "day"


def _latest_period(hi: Any, grain: str) -> Optional[str]:
    """What "current" / "latest" means for this dataset: the newest period the
    data actually holds, at the dataset's own grain. Never `date.today()` —
    a dataset that stops in August is current as of August."""
    text = str(hi or "")
    if not text:
        return None
    if grain == "year":
        return text[:4]
    if grain == "month":
        return text[:7]
    return text[:10] if _ISO_DAY.match(text) else text


def derive_time(columns: Sequence[dict], facts: TableFacts,
                declared_time: Optional[str] = None) -> Optional[Dict[str, Any]]:
    """The dataset's time dimension: which column, over which bounds, at which
    grain, and what its latest period is. A declared shape's time column wins;
    otherwise the temporal column with the widest recorded bounds."""
    candidates: List[str] = []
    if declared_time:
        candidates = [declared_time]
    else:
        for column in columns:
            name = str(column.get("column_name") or column.get("name") or "")
            if not name:
                continue
            lo, hi, _exact = facts.bounds(name)
            data_type = facts.data_type(name) or _base_type(column.get("data_type"))
            if _looks_temporal(name, data_type, lo, hi):
                candidates.append(name)
    for name in candidates:
        lo, hi, exact = facts.bounds(name)
        if lo is None and hi is None:
            continue
        distinct = facts.distinct(name)
        grain = _grain(lo, hi, distinct)
        basis = "declared shape" if declared_time else "column bounds"
        return {
            "column": name,
            "min": lo, "max": hi, "bounds_exact": exact,
            "grain": grain,
            "periods": distinct,
            "latest": _latest_period(hi, grain),
            "evidence": (_declared_evidence(facts, "time column")
                         if declared_time else facts.evidence("columns[].min/max", basis)),
        }
    if declared_time:
        # The shape names it even when the record has no bounds for it yet.
        return {"column": declared_time, "min": None, "max": None, "bounds_exact": False,
                "grain": "day", "periods": None, "latest": None,
                "evidence": _declared_evidence(facts, "time column")}
    return None


def classify_column(name: str, data_type: str, facts: TableFacts,
                    declared_dimension: bool = False, declared_measure: bool = False,
                    time_column: Optional[str] = None) -> Tuple[str, Dict[str, Any]]:
    """`("dimension" | "measure" | "identifier" | "time" | "text", evidence)`.

    A declared shape wins outright. Otherwise the distinct count decides:
    a column that is nearly unique names a row (identifier), a numeric column
    that is not an identifier is a measure, a column with few enough values to
    group by is a dimension, and anything else is free text the DLM will not
    break down by."""
    if time_column and name == time_column:
        return "time", _declared_evidence(facts, "time column") if declared_dimension else \
            facts.evidence("columns[].min/max", "temporal bounds")
    if declared_dimension:
        return "dimension", _declared_evidence(facts, "dimension")
    if declared_measure:
        return "measure", _declared_evidence(facts, "measure")

    distinct = facts.distinct(name)
    basis = facts.distinct_basis(name) or "hyperloglog p=12"
    rows = facts.rows
    numeric = data_type in _NUMERIC_TYPES
    if distinct is None:
        # No record for this column: fall back to the type alone, and say so.
        evidence = facts.evidence("columns[].data_type", "type only; no distinct count on record")
        return ("measure" if numeric else "dimension"), evidence
    evidence = facts.evidence("columns[].distinct", basis)
    if rows and rows > 1 and distinct >= IDENTIFIER_DISTINCT_RATIO * rows:
        return "identifier", evidence
    if numeric:
        return "measure", evidence
    if distinct <= MAX_DERIVED_DIMENSION_DISTINCT:
        return "dimension", evidence
    return "text", evidence


# --------------------------------------------------------------------------- #
# The derived spec                                                             #
# --------------------------------------------------------------------------- #

def _measure_range(name: str, facts: TableFacts) -> Optional[Dict[str, Any]]:
    lo, hi, exact = facts.bounds(name)
    quantiles = facts.quantiles(name)
    if lo is None and hi is None and not quantiles:
        return None
    out: Dict[str, Any] = {"min": lo, "max": hi, "bounds_exact": exact}
    if quantiles and len(quantiles) >= 3:
        out["p01"], out["p50"], out["p99"] = quantiles[0], quantiles[1], quantiles[2]
        # The scale a generated answer defaults to: outliers are on record but
        # do not set the range a person is shown.
        out["outlier_safe_min"], out["outlier_safe_max"] = quantiles[0], quantiles[2]
        out["evidence"] = facts.evidence("columns[].quantiles", "kll k=200")
    else:
        out["evidence"] = facts.evidence("columns[].min/max",
                                         "exact bounds" if exact else "metadata bounds")
    return out


def derive(columns: Sequence[dict], metrics: Sequence[dict], facts: TableFacts,
           engine: bool = False, synonyms=None, distinct_expr=None) -> Dict[str, Any]:
    """The auto-derived context spec. `synonyms(name)` supplies the seed alias
    lexicon and `distinct_expr(expression)` says whether a metric expression is
    a `COUNT(DISTINCT …)`; both are injected so this module stays free of the
    DLM's lexicon and of its SQL parsing."""
    synonyms = synonyms or (lambda name: [])
    distinct_expr = distinct_expr or (lambda expression: bool(re.search(r"\bDISTINCT\b", expression or "", re.I)))

    shape = facts.shape or {}
    declared_dimensions = {str(d.get("name")) for d in (shape.get("dimensions") or [])
                           if isinstance(d, dict) and d.get("name")}
    declared_measures = {str(m.get("column")) for m in (shape.get("measures") or [])
                         if isinstance(m, dict) and m.get("column")}
    declared_time = None
    if isinstance(shape.get("time"), dict) and shape["time"].get("column"):
        declared_time = str(shape["time"]["column"])

    time = derive_time(columns, facts, declared_time)
    time_column = (time or {}).get("column")

    roles: Dict[str, str] = {}
    column_evidence: Dict[str, Dict[str, Any]] = {}
    for column in columns:
        name = str(column.get("column_name") or column.get("name") or "")
        if not name:
            continue
        data_type = facts.data_type(name) or _base_type(column.get("data_type"))
        role, evidence = classify_column(
            name, data_type, facts,
            declared_dimension=name in declared_dimensions,
            declared_measure=name in declared_measures,
            time_column=time_column)
        roles[name] = role
        column_evidence[name] = evidence

    dimensions: Dict[str, Any] = {}
    identifiers: Dict[str, Any] = {}
    ranges: Dict[str, Any] = {}
    for name, role in roles.items():
        null_count = facts.null_count(name)
        distinct = facts.distinct(name)
        if role == "dimension":
            indexed = distinct is None or distinct <= MAX_INDEXED_DISTINCT
            dimensions[name] = {
                "display_name": name,
                "aliases": sorted(set(synonyms(name))),
                "precompute": True,
                "top_n": min(MAX_TOP_N, int(distinct)) if distinct else MAX_TOP_N,
                "cardinality": distinct,
                "index_values": indexed,
                "optional": bool(null_count),
                "null_count": null_count,
                "evidence": column_evidence[name],
            }
        elif role in ("identifier", "text"):
            identifiers[name] = {"reason": role, "cardinality": distinct,
                                 "evidence": column_evidence[name]}
        elif role == "measure":
            column_range = _measure_range(name, facts)
            if column_range:
                column_range["optional"] = bool(null_count)
                column_range["null_count"] = null_count
                ranges[name] = column_range

    measures: Dict[str, Any] = {}
    for index, metric in enumerate(metrics):
        name = metric.get("name") or metric.get("metric_name")
        if not name:
            continue
        expression = metric.get("expression") or ""
        distinct = distinct_expr(expression)
        additive = not (distinct or re.search(r"\bAVG\s*\(", expression, re.I))
        entry: Dict[str, Any] = {
            "display_name": name,
            "aliases": sorted(set(synonyms(name))),
            "additive": additive,
            "default": (index == 0),
        }
        if engine and distinct:
            entry["approximate"] = True
        column = _metric_column(expression)
        if column and column in ranges:
            entry["range"] = ranges[column]
            entry["evidence"] = ranges[column]["evidence"]
        elif column and column in column_evidence:
            entry["evidence"] = column_evidence[column]
        else:
            entry["evidence"] = facts.evidence("rows", "row count") if facts.rows is not None else \
                {"statistic": "metric expression", "source": "dataset"}
        measures[name] = entry

    spec: Dict[str, Any] = {
        "metrics": measures,
        "dimensions": dimensions,
        "identifiers": identifiers,
        "value_aliases": {},
    }
    if time:
        spec["time"] = time
    if facts.rows is not None:
        spec["rows"] = facts.rows
    if engine:
        spec["freshness_policy"] = "cached"
    spec["derived_from"] = {
        "source": "engine_statistics" if facts.known() else "dataset_definition",
        "shape_declared": bool(declared_dimensions or declared_measures or declared_time),
        "depth": facts.depth,
        "source_version": facts.source_version,
        "computed_at_ms": facts.computed_at_ms,
        "stale": facts.stale,
    }
    return spec


_METRIC_COLUMN = re.compile(r"\(\s*(?:DISTINCT\s+)?\"?([A-Za-z_][A-Za-z0-9_]*)\"?\s*\)", re.I)


def _metric_column(expression: str) -> Optional[str]:
    match = _METRIC_COLUMN.search(expression or "")
    column = match.group(1) if match else None
    return None if column == "*" else column


# Fields a curator may override on a spec element. Everything else on an
# element is derived and refreshes on every regenerate.
CURATABLE_FIELDS = ("display_name", "additive", "default", "precompute", "top_n", "hidden",
                    "approximate", "index_values", "optional")
# Fields that are always the record's word, never a curator's — a regenerate
# replaces them and curation on them is ignored rather than silently kept.
DERIVED_FIELDS = ("cardinality", "null_count", "evidence", "range")


def indexable_dimensions(spec: Dict[str, Any]) -> List[str]:
    """The dimensions the statistics say are low-cardinality enough to
    enumerate into the value index."""
    return [name for name, entry in (spec.get("dimensions") or {}).items()
            if entry.get("index_values", True) and not entry.get("hidden")]
