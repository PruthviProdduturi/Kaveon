"""
DLM — Data Language Model (per-dataset compiled context artifact).

A DLM is the *retrievable* compilation of everything Kaveon knows about one
dataset — structure, value inventory, statistics, usage — built so a natural-
language question can be resolved to the right columns/filters **without a
hosted LLM and without a data scan**.

It sits one layer above the context engine:

    context_profiler  ->  context_snapshots   (raw per-element stats, from pg_stats)
    dlm (this module) ->  dlm_artifact         (compiled manifest + rollups per dataset)
                          dlm_value_index       (value -> column/key resolution)
                          dlm_router            (cross-dataset "which dataset?" summaries)

"Generate DLM" is an *encode* step, not a train step: one global compressor
(future v2, RQ-VAE discrete codes) is applied per dataset; v1 uses the
zero-scan statistics the profiler already captured. The v2 code columns
(``codes``) are reserved now so the discrete-code upgrade needs no migration.

Storage: metadata DB (``database.metadata``). Value/stat inputs come from the
context engine, which is Postgres-only — so the value-index/stats portion
degrades to a no-op on other metadata dialects while the manifest still builds.
"""

from __future__ import annotations

import hashlib
import json
import logging
import re
import threading
import time as _time_mod
import unicodedata
import uuid
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional

import database.metadata as meta
import database.pool as pool
import dlm.profiler as profiler
import services.datasets as datasets_svc
import dlm.hll as hll

logger = logging.getLogger(__name__)

# Low-cardinality dimensions get a value index; high-card columns (ids, free
# text) are matched structurally, never by value.
_MAX_CARDINALITY_FOR_VALUES = 1000

# Common aliases so "USA"/"America" resolve to "United States", etc. Keyed and
# valued by normalized (lowercased) text.
_VALUE_ALIASES = {
    "usa": "united states", "us": "united states", "u s": "united states",
    "america": "united states", "united states of america": "united states",
    "uk": "united kingdom", "u k": "united kingdom", "britain": "united kingdom",
    "great britain": "united kingdom", "uae": "united arab emirates",
    "south korea": "korea", "s korea": "korea", "russia": "russian federation",
}

# Words that never denote a filter value on their own — prevents "in" matching
# "India", "for" matching a value, etc.
_STOPWORDS = frozenset({
    "in", "by", "for", "of", "the", "to", "a", "an", "and", "or", "per", "top",
    "show", "get", "what", "how", "is", "are", "was", "over", "time", "trend",
    "total", "all", "each", "across", "vs", "versus", "with", "on", "at", "from",
    "me", "give", "list", "average", "avg", "sum", "count", "number", "share",
})

# Seed synonym lexicon — column/metric aliases that let "revenue"/"sales"/"top
# line" hit the same element with no model. Admin-editable overrides land in the
# manifest later; this is the cold-start floor.
_SEED_SYNONYMS: Dict[str, List[str]] = {
    # finance / business
    "revenue":      ["sales", "turnover", "top line", "income", "sell", "sold", "selling", "earned"],
    "cost":         ["spend", "expense", "expenditure", "pricing", "price", "cheap", "expensive", "paid"],
    "profit":       ["margin", "earnings", "net income", "bottom line", "gain", "surplus"],
    "loss":         ["deficit", "shortfall", "negative"],
    "budget":       ["allocation", "funding", "appropriation"],
    "transaction":  ["order", "purchase", "deal", "payment", "checkout"],
    "export":       ["exports", "exported", "outbound", "shipment"],
    "import":       ["imports", "imported", "inbound"],
    "gdp":          ["gross domestic product", "output", "economic output"],
    # counts / quantities
    "count":        ["number", "total", "volume", "qty", "quantity", "how many", "tally"],
    "rate":         ["ratio", "percentage", "pct", "percent", "fraction", "share", "proportion"],
    "average":      ["avg", "mean", "typical", "per capita"],
    "maximum":      ["max", "highest", "peak", "top", "most", "largest", "biggest", "greatest"],
    "minimum":      ["min", "lowest", "bottom", "least", "smallest", "fewest"],
    # people / users
    "customer":     ["client", "account", "user", "buyer", "subscriber", "member", "person"],
    "employee":     ["staff", "worker", "headcount", "hc", "fte", "personnel", "team member"],
    "population":   ["people", "inhabitants", "residents", "citizens"],
    # health / epidemiology
    "deaths":       ["mortality", "fatality", "fatalities", "died", "killed", "deceased"],
    "cases":        ["infections", "infected", "positive", "confirmed", "diagnosed"],
    "vaccination":  ["vaccine", "vaccinated", "immunization", "jab", "dose", "inoculation"],
    "hospitalization": ["admitted", "hospital", "icu", "inpatient"],
    "recovery":     ["recovered", "healed", "cured", "discharged"],
    # energy / climate
    "emission":     ["emissions", "co2", "carbon", "ghg", "greenhouse", "pollution"],
    "generation":   ["produced", "output", "supply", "capacity"],
    "consumption":  ["usage", "demand", "consumed", "utilized", "utilization"],
    "renewable":    ["solar", "wind", "hydro", "clean", "green"],
    "fossil":       ["coal", "oil", "gas", "petroleum", "nonrenewable"],
    "temperature":  ["temp", "warming", "heat", "degrees"],
    # product / tech
    "provider":     ["vendor", "supplier", "publisher", "maker", "manufacturer"],
    "model":        ["variant", "version", "sku", "product"],
    "plan":         ["tier", "segment", "segments", "subscription", "package", "license"],
    "score":        ["rating", "grade", "benchmark", "performance", "rank"],
    "accuracy":     ["precision", "quality", "correctness", "fidelity"],
    "latency":      ["response time", "delay", "lag", "speed"],
    "event":        ["action", "activity", "occurrence", "log", "record", "entry"],
    "session":      ["visit", "pageview", "interaction", "engagement"],
    "churn":        ["attrition", "turnover", "cancellation", "lost"],
    "retention":    ["kept", "renewed", "sticky", "loyalty"],
    "acquisition":  ["signup", "registration", "onboarding", "new user", "conversion"],
    "active":       ["engaged", "dau", "mau", "wau", "live"],
    # geography / dimensions
    "region":       ["geography", "geo", "area", "territory", "zone", "continent"],
    "country":      ["nation", "state", "sovereign"],
    "city":         ["town", "municipality", "metro", "urban"],
    "industry":     ["sector", "vertical", "field", "domain"],
    "category":     ["type", "class", "group", "kind", "classification"],
    "channel":      ["source", "medium", "origin", "referral", "touchpoint"],
    # time
    "date":         ["day", "time", "period", "when", "timestamp"],
    "year":         ["annual", "yearly", "yr"],
    "month":        ["monthly", "mo"],
    "quarter":      ["quarterly", "qtr", "q1", "q2", "q3", "q4"],
    "week":         ["weekly", "wk"],
    # transport
    "trip":         ["ride", "journey", "fare", "travel", "commute"],
    "distance":     ["miles", "km", "kilometers", "length", "mileage"],
    "duration":     ["time", "minutes", "hours", "elapsed"],
    "passenger":    ["rider", "traveler", "occupant"],
    "fare":         ["charge", "fee", "toll", "cost"],
}


def _now_iso() -> str:
    return datetime.now(timezone.utc).replace(tzinfo=None).isoformat()


# --------------------------------------------------------------------------- #
# self-migrating storage                                                       #
# --------------------------------------------------------------------------- #

_DDL = """
CREATE TABLE IF NOT EXISTS dlm_artifact (
    dataset_id     TEXT PRIMARY KEY,
    version        INTEGER NOT NULL DEFAULT 1,
    manifest       TEXT NOT NULL,          -- JSON: columns, join graph, grain, metrics, synonyms
    stats_rollup   TEXT,                   -- JSON: row_count, cardinalities, freshness
    usage_rollup   TEXT,                   -- JSON: per-table usage from query_history
    embeds         TEXT,                   -- v1 (reserved): packed dense element vectors
    codes          TEXT,                   -- v2 (reserved): RQ-VAE discrete codes
    curation       TEXT,                   -- JSON: per-dataset human overrides (aliases, breakdowns, additivity...)
    source_hash    TEXT NOT NULL,          -- schema+snapshot fingerprint for change-detection
    built_at       TEXT NOT NULL,
    status         TEXT NOT NULL DEFAULT 'ready'   -- building | ready | stale | error | unsupported
);
ALTER TABLE dlm_artifact ADD COLUMN IF NOT EXISTS curation TEXT;

CREATE TABLE IF NOT EXISTS dlm_value_index (
    id            TEXT PRIMARY KEY,
    dataset_id    TEXT NOT NULL,
    element_key   TEXT NOT NULL,           -- schema.table.column
    value_text    TEXT NOT NULL,           -- the distinct value as stored
    value_norm    TEXT NOT NULL,           -- lowercased/trimmed/deaccented for match
    key_column    TEXT,                    -- surrogate key column if role-playing dim
    key_value     TEXT,                    -- the key to filter on
    freq          DOUBLE PRECISION DEFAULT 0,   -- approx row support
    source        TEXT                     -- 'pg_stats.most_common_vals' etc.
);
CREATE INDEX IF NOT EXISTS idx_dlm_value_norm ON dlm_value_index (dataset_id, value_norm);
CREATE INDEX IF NOT EXISTS idx_dlm_value_elem ON dlm_value_index (element_key);
CREATE UNIQUE INDEX IF NOT EXISTS ux_dlm_value_uniq
    ON dlm_value_index (dataset_id, element_key, value_norm);

CREATE TABLE IF NOT EXISTS dlm_router (
    dataset_id   TEXT PRIMARY KEY,
    summary      TEXT NOT NULL,            -- what this dataset is about
    terms        TEXT,                     -- JSON: normalized keyword bag for lexical routing
    embeds       TEXT,                     -- v1 (reserved)
    codes        TEXT,                     -- v2 (reserved): same code space as artifacts
    updated_at   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS dlm_answers (
    id           TEXT PRIMARY KEY,
    dataset_id   TEXT NOT NULL,
    metric_name  TEXT NOT NULL,
    group_col    TEXT NOT NULL DEFAULT '',  -- '' = grand total (no group-by)
    columns      TEXT NOT NULL,             -- JSON: [column names]
    rows         TEXT NOT NULL,             -- JSON: precomputed result rows
    computed_at  TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_dlm_answers
    ON dlm_answers (dataset_id, metric_name, group_col);

CREATE TABLE IF NOT EXISTS dlm_sketch (
    id           TEXT PRIMARY KEY,
    dataset_id   TEXT NOT NULL,
    metric_name  TEXT NOT NULL,             -- the non-additive COUNT(DISTINCT) metric
    dims         TEXT NOT NULL,              -- JSON: cuboid dim columns (canonical order)
    cell_key     TEXT NOT NULL,             -- JSON array of values per dim (empty string = header row)
    registers    TEXT NOT NULL,             -- sparse JSON registers (header row carries p + dims)
    computed_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_dlm_sketch ON dlm_sketch (dataset_id, metric_name);
"""

_tables_ready = False


def ensure_tables() -> None:
    global _tables_ready
    if _tables_ready:
        return
    for stmt in filter(str.strip, _DDL.split(";")):
        meta.execute(stmt)
    _tables_ready = True


# --------------------------------------------------------------------------- #
# public: generate / read a DLM                                                #
# --------------------------------------------------------------------------- #


def generate_dlm(dataset_id: str, force: bool = False) -> Dict[str, Any]:
    """Compile (or refresh) the DLM artifact for one dataset. Idempotent: a
    matching ``source_hash`` short-circuits unless *force*. Returns a summary."""
    import time as _time
    _gen_t0 = _time.time()
    ensure_tables()

    ds = datasets_svc.get_dataset_by_id(str(dataset_id))
    if not ds:
        return {"ok": False, "reason": "dataset_not_found", "dataset_id": dataset_id}

    database = ds.get("database_name") or _metadata_database()
    schema = ds.get("schema_name") or "public"
    columns = ds.get("columns") or []
    dimensions = ds.get("dimensions") or []
    metrics = ds.get("metrics") or []

    # 1) fingerprint the structural definition — cheap change-detection
    source_hash = _fingerprint(ds, columns, dimensions, metrics)
    if not force:
        existing = meta.query_one(
            "SELECT source_hash, status FROM dlm_artifact WHERE dataset_id = @param0",
            [str(dataset_id)],
        )
        if existing and existing.get("source_hash") == source_hash and existing.get("status") == "ready":
            return {"ok": True, "dataset_id": dataset_id, "status": "ready", "rebuilt": False}

    # 2) refresh the zero-scan statistics substrate (no-op off Postgres).
    #    ANALYZE first so pg_stats is complete for freshly-loaded tables.
    stats_supported = False
    try:
        _analyze_tables(database, schema, _dataset_tables(columns, dimensions,
                        ds.get("table_name") or ds.get("fact_table")))
        prof = profiler.build_context(database, schema)
        stats_supported = bool(prof.get("supported"))
    except Exception:
        stats_supported = False

    if not stats_supported:
        existing_art = meta.query_one(
            "SELECT status FROM dlm_artifact WHERE dataset_id = @param0",
            [str(dataset_id)],
        )
        if existing_art and existing_art.get("status") == "ready":
            logger.info("Profiler unavailable for dataset %s — preserving existing ready artifact", dataset_id)
            return {"ok": True, "dataset_id": dataset_id, "status": "ready",
                    "rebuilt": False, "reason": "profiler_unavailable_preserved_existing"}

    snapshots = {}
    if stats_supported:
        try:
            snapshots = profiler.load_snapshots(database)
        except Exception:
            snapshots = {}

    # 3) value inventory — prefer pg_stats most_common_vals (zero scan); fall
    #    back to a bounded generate-time scan for views / unanalyzed tables that
    #    have no catalog stats. Only low-cardinality dimensions get indexed.
    value_rows: List[Dict[str, Any]] = []
    if stats_supported:
        value_rows = _build_value_index(str(dataset_id), database, schema,
                                        columns, dimensions, snapshots)

    # 4) usage rollup — how often each table has actually been asked about
    usage_rollup = _usage_rollup(schema, columns, dimensions, snapshots)

    # 5) stats rollup — cardinalities / row counts / freshness / date-range digest
    stats_rollup = _stats_rollup(database, schema, columns, dimensions, snapshots, metrics,
                                 ds.get("date_column"), ds.get("table_name") or ds.get("fact_table"))

    # 6) manifest — the deterministic assembler's map of the dataset
    manifest = _manifest(ds, columns, dimensions, metrics)

    # 7) persist artifact + value index + router summary atomically-ish
    _persist_value_index(str(dataset_id), value_rows)
    _upsert_artifact(str(dataset_id), manifest, stats_rollup, usage_rollup,
                     source_hash, "ready" if stats_supported else "unsupported")
    _upsert_router(str(dataset_id), ds, columns, metrics, value_rows)

    # refresh the cached effective spec so precompute (and serving) see the freshly
    # written suggestions overlaid with any persisted human curation.
    _SPEC_CACHE.pop(str(dataset_id), None)
    spec = _effective_spec(str(dataset_id))

    # 8) precompute the common answers (each metric's total + per-dimension
    #    breakdown the spec marks for precompute) so those questions serve from
    #    context with no live query. A handful of scans now; a lookup forever after.
    answers = _precompute_answers(str(dataset_id), database, schema,
                                  ds.get("table_name") or ds.get("fact_table"),
                                  columns, dimensions, metrics, spec)
    _ANSWER_CACHE.pop(str(dataset_id), None)  # invalidate in-memory caches after regen
    _SKETCH_CACHE.pop(str(dataset_id), None)
    _RANGE_CACHE.pop(str(dataset_id), None)

    # record generation timing (+ what drove it) into the stored stats rollup so
    # the dataset page can be transparent about how long it took and why.
    duration_ms = int((_time.time() - _gen_t0) * 1000)
    stats_rollup["generation"] = {
        "duration_ms": duration_ms,
        "built_at": _now_iso(),
        "answers_precomputed": answers,
        "values_indexed": len(value_rows),
        "rows_scanned": max((stats_rollup.get("row_counts") or {}).values(), default=None),
        "scans": 1 + len([c for c in columns if c.get("is_dimension")]),  # totals + per-dim
    }
    # 9) dashboard-level curation — precompute N-dim combos for any dashboards
    #    that reference this dataset, so multi-filter interactions serve instantly.
    dash_answers = 0
    try:
        dash_answers = _curate_linked_dashboards(str(dataset_id))
        if dash_answers:
            answers += dash_answers
            _ANSWER_CACHE.pop(str(dataset_id), None)
    except Exception:
        pass

    # 10) watermark — track row count + max date for incremental refresh
    row_count = max((stats_rollup.get("row_counts") or {}).values(), default=0)
    max_date = None
    date_col = ds.get("date_column")
    fact_tbl = ds.get("table_name") or ds.get("fact_table")
    if date_col and database and fact_tbl:
        try:
            tbl = f"{_qid(schema)}.{_qid(fact_tbl)}" if schema else _qid(fact_tbl)
            dr = pool.execute_query(
                f"SELECT MAX({_qid(date_col)}) AS mx FROM {tbl}", database)
            drow = (dr.get("rows") or dr.get("rows_objects") or [{}])[0]
            max_date = str(drow.get("mx") if isinstance(drow, dict) else drow[0])
        except Exception:
            pass
    stats_rollup["watermark"] = {
        "row_count": int(row_count) if row_count else 0,
        "max_date": max_date,
        "built_at": _now_iso(),
        "method": "full_rebuild",
    }

    try:
        meta.execute("UPDATE dlm_artifact SET stats_rollup = @param0 WHERE dataset_id = @param1",
                     [json.dumps(stats_rollup, default=str), str(dataset_id)])
    except Exception:
        pass

    return {
        "ok": True,
        "dataset_id": dataset_id,
        "status": "ready" if stats_supported else "unsupported",
        "rebuilt": True,
        "stats_supported": stats_supported,
        "columns": len(columns),
        "values_indexed": len(value_rows),
        "answers_precomputed": answers,
        "dashboard_answers": dash_answers,
        "built_at": _now_iso(),
        "method": "manifest + pg_stats value inventory + query_history usage (no LLM, no data scan)",
    }


def _precompute_answers(dataset_id: str, database: str, schema: str, fact: Optional[str],
                        columns: List[dict], dimensions: List[dict], metrics: List[dict],
                        spec: Optional[dict] = None) -> int:
    """Compute + store the common answers for this dataset: every metric's grand
    total (one scan for all metrics) and every metric grouped by each dimension the
    curated spec marks for precompute, to its configured depth. Stored in dlm_answers
    for instant, no-DB-trip serving. Returns how many answers were stored."""
    import database.pool as pool
    meta.execute("DELETE FROM dlm_answers WHERE dataset_id = @param0", [dataset_id])
    if not fact or not metrics:
        return 0

    tbl = f"{_qid(schema)}.{_qid(fact)}" if schema else _qid(fact)
    # metric_name -> (alias, expression); alias is a safe positional handle
    mdefs = []
    for i, m in enumerate(metrics):
        name = m.get("name") or m.get("metric_name")
        expr = m.get("expression")
        if name and expr:
            mdefs.append((f"m{i}", name, expr))
    if not mdefs:
        return 0
    now = _now_iso()
    stored = 0

    # 1) grand totals — one scan computes all metrics at once
    sel = ", ".join(f"{expr} AS {alias}" for alias, _n, expr in mdefs)
    try:
        res = pool.execute_query(f"SELECT {sel} FROM {tbl}", database)
        row = (res.get("rows") or res.get("rows_objects") or [None])[0]
        vals = list(row.values()) if isinstance(row, dict) else (list(row) if row else [])
        for j, (_a, name, _e) in enumerate(mdefs):
            v = vals[j] if j < len(vals) else None
            _store_answer(dataset_id, name, "", [name], [[_json_scalar(v)]], now)
            stored += 1
    except Exception:
        pass

    # 2) per dimension — one scan per dim computes all metrics grouped. Honor the
    #    curated spec: skip dims marked precompute=false/hidden; use the curated depth.
    dspec = (spec or {}).get("dimensions") or {}
    dim_cols = [(_c.get("column_name") or _c.get("name") or "").strip()
                for _c in columns if _c.get("is_dimension")]
    card: Dict[str, int] = {}   # per-dim breakdown row count — drives low-card pair selection
    for dim in dim_cols:
        if not dim:
            continue
        cfg = dspec.get(dim) or {}
        if cfg.get("precompute") is False or cfg.get("hidden"):
            continue
        depth = int(cfg.get("top_n") or 500)
        gsel = ", ".join(f"{expr} AS {alias}" for alias, _n, expr in mdefs)
        # order by the first metric desc so top-N questions can slice the head
        order = mdefs[0][0]
        try:
            res = pool.execute_query(
                f"SELECT {_qid(dim)} AS grp, {gsel} FROM {tbl} "
                f"WHERE {_qid(dim)} IS NOT NULL GROUP BY {_qid(dim)} "
                f"ORDER BY {order} DESC LIMIT {int(depth)}", database)
            rows = res.get("rows") or res.get("rows_objects") or []
            if not rows:
                continue
            norm = [(_row_vals(r)) for r in rows]   # [grp, m0, m1, ...]
            card[dim] = len(norm)
            for j, (_a, name, _e) in enumerate(mdefs):
                out = [[_json_scalar(rv[0]), _json_scalar(rv[j + 1] if j + 1 < len(rv) else None)] for rv in norm]
                _store_answer(dataset_id, name, dim, [dim, name], out, now)
                stored += 1
        except Exception:
            continue

    # 3) common 2-dim combos — so a two-filter question ("... in Asia Enterprise")
    #    serves from context instead of a live scan. Non-additive metrics stay exact:
    #    COUNT(DISTINCT) is computed independently per (d1,d2) cell. Bounded to low-card
    #    pairs (both dims fully enumerated, cell product under a cap) and a max pair
    #    count, so generation time + storage stay sane.
    CELL_CAP, MAX_PAIRS, HIGH_CARD = 5000, 12, 500
    lowcard = [d for d in card if 0 < card[d] < HIGH_CARD]
    pairs: List[tuple] = []
    for i in range(len(lowcard)):
        for k in range(i + 1, len(lowcard)):
            d1, d2 = sorted((lowcard[i], lowcard[k]))
            prod = card[d1] * card[d2]
            if prod <= CELL_CAP:
                pairs.append((prod, d1, d2))
    pairs.sort()   # cheapest / smallest cell-count pairs first
    for _prod, d1, d2 in pairs[:MAX_PAIRS]:
        gsel = ", ".join(f"{expr} AS {alias}" for alias, _n, expr in mdefs)
        order = mdefs[0][0]
        try:
            res = pool.execute_query(
                f"SELECT {_qid(d1)} AS g1, {_qid(d2)} AS g2, {gsel} FROM {tbl} "
                f"WHERE {_qid(d1)} IS NOT NULL AND {_qid(d2)} IS NOT NULL "
                f"GROUP BY {_qid(d1)}, {_qid(d2)} ORDER BY {order} DESC LIMIT {int(CELL_CAP)}", database)
            rows = res.get("rows") or res.get("rows_objects") or []
            if not rows:
                continue
            norm = [_row_vals(r) for r in rows]   # [g1, g2, m0, m1, ...]
            key = f"{d1}|{d2}"   # canonical (lexicographic) pair key
            for j, (_a, name, _e) in enumerate(mdefs):
                out = [[_json_scalar(rv[0]), _json_scalar(rv[1]),
                        _json_scalar(rv[j + 2] if j + 2 < len(rv) else None)] for rv in norm]
                _store_answer(dataset_id, name, key, [d1, d2, name], out, now)
                stored += 1
        except Exception:
            continue

    # 4) sketch cuboid — for non-additive COUNT(DISTINCT) metrics, build ONE base
    #    HyperLogLog cuboid over the low-card dims. Any sub-combo of those dims
    #    (all 2^k filter subsets — including the 3+ filter cases the exact combos
    #    above don't materialize) is then answered by unioning register vectors in
    #    Python: ~1-2% error, no live scan. This is the "big-dataset grain" story.
    try:
        stored += _build_sketch_cuboids(dataset_id, database, schema, fact,
                                        columns, metrics, card, spec)
    except Exception:
        pass
    return stored


def _store_answer(dataset_id: str, metric_name: str, group_col: str,
                  cols: List[str], rows: List[list], now: str) -> None:
    meta.execute(
        "INSERT INTO dlm_answers (id, dataset_id, metric_name, group_col, columns, rows, computed_at) "
        "VALUES (@param0,@param1,@param2,@param3,@param4,@param5,@param6) "
        "ON CONFLICT (dataset_id, metric_name, group_col) DO UPDATE SET "
        "columns = EXCLUDED.columns, rows = EXCLUDED.rows, computed_at = EXCLUDED.computed_at",
        [str(uuid.uuid4()), dataset_id, metric_name, group_col,
         json.dumps(cols), json.dumps(rows, default=str), now])


def _row_vals(r) -> list:
    return list(r.values()) if isinstance(r, dict) else list(r)


def _json_scalar(v: Any) -> Any:
    import decimal
    import datetime as _dt
    if isinstance(v, decimal.Decimal):
        return float(v)
    if isinstance(v, (_dt.date, _dt.datetime)):
        return v.isoformat()
    return v


# --------------------------------------------------------------------------- #
# sketch cuboids — mergeable HLL for non-additive COUNT(DISTINCT)              #
# --------------------------------------------------------------------------- #

def _distinct_col(expr: str) -> Optional[str]:
    """The single column inside a ``COUNT(DISTINCT <col>)`` metric, or None if the
    metric isn't a plain distinct-count (only a bare column can be hashed per-row
    into a sketch; distinct-of-an-expression is left to the live path)."""
    m = re.search(r"count\s*\(\s*distinct\s+(.+?)\s*\)", expr or "", re.I)
    if not m:
        return None
    col = m.group(1).strip().strip('"').strip('`').strip("[]")
    if "." in col:                      # strip a table/alias qualifier: t.user_id
        col = col.split(".")[-1].strip('"').strip('`').strip("[]")
    return col if re.match(r"^[A-Za-z_][A-Za-z0-9_ ]*$", col) else None


def _pg_register_sql(tbl: str, dims: List[str], dcol: str) -> str:
    """Postgres SQL that extracts per-cell HLL registers in ONE scan: hash each
    row's distinct-column value, take the top P bits as the register index and the
    run of leading zeros in the rest as rho, then MAX(rho) per (cell, register).
    Pure SQL — no `hll` extension (Azure Flexible Server doesn't ship it)."""
    dim_sel = ", ".join(_qid(d) for d in dims)
    dc = _qid(dcol)
    p, rb = hll.P, hll.RBITS
    return (
        f"WITH h AS (SELECT {dim_sel}, "
        f"hashtextextended(CAST({dc} AS text), 0)::bit(64) AS b "
        f"FROM {tbl} WHERE {dc} IS NOT NULL), "
        f"e AS (SELECT {dim_sel}, "
        f"substring(b from 1 for {p})::text AS reg, "
        f"CASE WHEN position('1' in substring(b from {p + 1} for {rb})::text) = 0 "
        f"THEN {rb + 1} ELSE position('1' in substring(b from {p + 1} for {rb})::text) END AS rho "
        f"FROM h) "
        f"SELECT {dim_sel}, reg, MAX(rho) AS rho FROM e GROUP BY {dim_sel}, reg"
    )


def _store_sketch(dataset_id: str, metric_name: str, dims: List[str],
                  cell_key: str, registers: str, now: str) -> None:
    meta.execute(
        "INSERT INTO dlm_sketch (id, dataset_id, metric_name, dims, cell_key, registers, computed_at) "
        "VALUES (@param0,@param1,@param2,@param3,@param4,@param5,@param6)",
        [str(uuid.uuid4()), dataset_id, metric_name, json.dumps(dims), cell_key, registers, now])


def _build_sketch_cuboids(dataset_id: str, database: str, schema: str, fact: Optional[str],
                          columns: List[dict], metrics: List[dict],
                          card: Dict[str, int], spec: Optional[dict]) -> int:
    """Build + store a base HLL sketch cuboid per non-additive COUNT(DISTINCT)
    metric, at the grain of the low-card precomputed dims. Returns the number of
    cuboids built. Postgres-only (register SQL is dialect-specific); other engines
    skip cleanly — the live path is unchanged for them."""
    import database.pool as pool
    if not fact:
        return 0
    try:
        engine = pool.get_connection_pool(database).db_type
    except Exception:
        engine = None
    if engine != "postgresql":
        return 0

    dmetrics = []
    for m in metrics:
        name = m.get("name") or m.get("metric_name")
        dcol = _distinct_col(m.get("expression") or "")
        if name and dcol:
            dmetrics.append((name, dcol))
    if not dmetrics:
        return 0

    # Cuboid grain = low-card precomputed dims, greedily smallest-first until the
    # cell product hits the cap. High-card dims (ids, city, user) are never axes —
    # they're what you count-distinct or filter live. Needs >=2 dims: single-dim
    # distincts are already served exactly from the per-dim breakdowns.
    # Only genuinely low-card dims are axes. card[d] is the (LIMIT-capped) breakdown
    # row count; requiring < HIGH_CARD both keeps the cuboid small AND guarantees the
    # breakdown wasn't truncated, so a high-card dim can't slip in with a capped count.
    SKETCH_MAX_CELLS, SKETCH_MAX_DIMS, HIGH_CARD = 8000, 6, 500
    dspec = (spec or {}).get("dimensions") or {}
    lowcard = sorted((d for d in card if 0 < card[d] < HIGH_CARD), key=lambda d: card[d])
    dims: List[str] = []
    prod = 1
    for d in lowcard:
        if len(dims) >= SKETCH_MAX_DIMS or (dspec.get(d) or {}).get("hidden"):
            continue
        if prod * card[d] > SKETCH_MAX_CELLS:
            continue
        dims.append(d)
        prod *= card[d]
    if len(dims) < 2:
        return 0

    tbl = f"{_qid(schema)}.{_qid(fact)}" if schema else _qid(fact)
    now = _now_iso()
    built = 0
    ndim = len(dims)
    for name, dcol in dmetrics:
        try:
            res = pool.execute_query(_pg_register_sql(tbl, dims, dcol), database)
        except Exception:
            continue
        rows = res.get("rows") or res.get("rows_objects") or []
        if not rows:
            continue
        # fold (cell, register) -> max rho into a full register vector per cell
        cells: Dict[str, bytearray] = {}
        for r in rows:
            rv = _row_vals(r)
            if len(rv) < ndim + 2:
                continue
            ck = json.dumps([_json_scalar(v) for v in rv[:ndim]], default=str)
            try:
                reg = int(str(rv[ndim]), 2)      # register-index bits -> int
                rho = int(rv[ndim + 1])
            except (ValueError, TypeError):
                continue
            buf = cells.get(ck)
            if buf is None:
                buf = hll.empty()
                cells[ck] = buf
            if 0 <= reg < hll.M and rho > buf[reg]:
                buf[reg] = rho
        if not cells:
            continue
        meta.execute("DELETE FROM dlm_sketch WHERE dataset_id = @param0 AND metric_name = @param1",
                     [dataset_id, name])
        _store_sketch(dataset_id, name, dims, "", json.dumps({"p": hll.P, "dims": dims}), now)
        for ck, buf in cells.items():
            _store_sketch(dataset_id, name, dims, ck, hll.to_sparse(buf), now)
        built += 1
    return built


def get_dlm(dataset_id: str) -> Optional[Dict[str, Any]]:
    """Return the compiled artifact row (manifest/rollups parsed) or None."""
    ensure_tables()
    row = meta.query_one(
        "SELECT dataset_id, version, manifest, stats_rollup, usage_rollup, "
        "source_hash, built_at, status FROM dlm_artifact WHERE dataset_id = @param0",
        [str(dataset_id)],
    )
    if not row:
        return None
    for k in ("manifest", "stats_rollup", "usage_rollup"):
        row[k] = _loads(row.get(k))
    row["values_indexed"] = _value_count(str(dataset_id))
    return row


def resolve_value(dataset_id: str, term: str, limit: int = 5,
                  exact_only: bool = False) -> List[Dict[str, Any]]:
    """The no-LLM retrieval workhorse: map a term to the column + filter key it
    denotes. e.g. "anthropic" -> {column: provider, key_value: 'Anthropic'}.
    Exact-normalized match first; a prefix match is used as a fallback unless
    *exact_only* (the entity-filter path sets this, so "in" can't match "India")."""
    ensure_tables()
    norm = _normalize(term)
    if not norm:
        return []
    norm = _VALUE_ALIASES.get(norm, norm)   # USA -> united states, etc.
    exact = meta.query(
        "SELECT element_key, value_text, key_column, key_value, freq "
        "FROM dlm_value_index WHERE dataset_id = @param0 AND value_norm = @param1 "
        "ORDER BY freq DESC",
        [str(dataset_id), norm],
    )
    rows = exact.get("rows_objects", exact.get("rows", []))
    if not rows and not exact_only and len(norm) >= 4:
        pref = meta.query(
            "SELECT element_key, value_text, key_column, key_value, freq "
            "FROM dlm_value_index WHERE dataset_id = @param0 AND value_norm LIKE @param1 "
            "ORDER BY freq DESC",
            [str(dataset_id), norm + "%"],
        )
        rows = pref.get("rows_objects", pref.get("rows", []))
    # typo tolerance: fuzzy-match against this dataset's values (japn -> japan)
    if not rows and len(norm) >= 4:
        rows = _fuzzy_value(str(dataset_id), norm)
    return [_value_hit(r) for r in rows[:limit]]


def _fuzzy_value(dataset_id: str, norm: str) -> List[dict]:
    """Close-match a (possibly misspelled) term against the dataset's indexed
    values using edit-distance ratio. 'paskistan' -> 'Pakistan'."""
    import difflib
    res = meta.query(
        "SELECT element_key, value_text, value_norm, key_column, key_value, freq "
        "FROM dlm_value_index WHERE dataset_id = @param0", [dataset_id])
    cand = res.get("rows_objects", res.get("rows", []))
    if not cand:
        return []
    by_norm = {}
    for c in cand:
        if isinstance(c, dict):
            by_norm.setdefault(c.get("value_norm"), c)
    close = difflib.get_close_matches(norm, [k for k in by_norm if k], n=1, cutoff=0.84)
    return [by_norm[close[0]]] if close else []


_ROUTE_NOISE = frozenset({"sum", "avg", "count", "total", "max", "min", "the", "of", "by"})
_ROUTE_FLOOR = 2  # reject routing on a single stray generic-word match


def _stem_set(tokens: set) -> set:
    return {_stem(t) for t in tokens}


def route(question: str, limit: int = 3) -> List[Dict[str, Any]]:
    """CLM-over-CLMs (weighted lexical): rank datasets by how strongly the
    question matches each dataset's metrics (x3), indexed values (x2), name (x4)
    and columns (min(hits,3)). A floor prevents a single generic word from routing
    (which sent 'USA consumption' to the AI leaderboard). Discrete-code routing
    at v2."""
    ensure_tables()
    q_tokens = set(_tokenize(question))
    if not q_tokens:
        return []
    q_stems = _stem_set(q_tokens)

    value_hits = _value_dataset_hits(q_tokens)  # dataset_id -> count of value matches
    arts = meta.query("SELECT dataset_id, manifest FROM dlm_artifact", [])
    ranked: List[Dict[str, Any]] = []
    for a in arts.get("rows_objects", arts.get("rows", [])):
        did = str(a.get("dataset_id"))
        manifest = _loads(a.get("manifest")) or {}
        metric_toks: set = set()
        for m in manifest.get("metrics", []) or []:
            metric_toks |= set(_tokenize(m.get("name"))) | set(_tokenize(m.get("expression"))) | set(m.get("synonyms") or [])
        col_toks: set = set()
        for c in manifest.get("columns", []) or []:
            col_toks |= set(_tokenize(c.get("name"))) | set(c.get("synonyms") or [])
        name_toks = set(_tokenize(manifest.get("name")))
        metric_toks -= _ROUTE_NOISE
        col_toks -= _ROUTE_NOISE
        name_toks -= _ROUTE_NOISE

        # Match both exact and stemmed tokens for better recall
        m_hit = q_tokens & metric_toks | (q_stems & _stem_set(metric_toks))
        n_hit = q_tokens & name_toks | (q_stems & _stem_set(name_toks))
        c_hit = q_tokens & col_toks | (q_stems & _stem_set(col_toks))
        v_hit = value_hits.get(did, 0)
        # Name hits are the strongest signal — a question containing "taxi"
        # should route to the taxi dataset even if other datasets have matching
        # entity values (like "india" appearing in climate data).
        # Cap column hits at 3 so broad datasets (14 columns) don't dominate
        # narrow ones (3 columns) on generic tokens.
        score = 3 * len(m_hit) + 2 * v_hit + 4 * len(n_hit) + min(len(c_hit), 3)
        if score >= _ROUTE_FLOOR:
            # Tiebreaker: prefer narrower datasets (fewer columns → more focused)
            col_count = len(manifest.get("columns") or [])
            ranked.append({
                "dataset_id": did,
                "score": score,
                "specificity": 1000 - col_count,  # higher = narrower
                "matched": sorted(m_hit | n_hit | c_hit),
                "value_matches": v_hit,
                "summary": manifest.get("name"),
            })
    ranked.sort(key=lambda x: (x["score"], x.get("specificity", 0)), reverse=True)
    return ranked[:limit]


def _value_dataset_hits(q_tokens: set) -> Dict[str, int]:
    """Which datasets have an indexed value equal to one of the question tokens
    (e.g. 'japan', 'anthropic') — a strong routing signal."""
    toks = [t for t in q_tokens if len(t) >= 3]
    if not toks:
        return {}
    placeholders = ",".join(f"@param{i}" for i in range(len(toks)))
    try:
        res = meta.query(
            f"SELECT dataset_id, COUNT(*) AS n FROM dlm_value_index "
            f"WHERE value_norm IN ({placeholders}) GROUP BY dataset_id", list(toks))
    except Exception:
        return {}
    out: Dict[str, int] = {}
    for r in res.get("rows_objects", res.get("rows", [])):
        if isinstance(r, dict):
            try:
                out[str(r.get("dataset_id"))] = int(r.get("n") or 0)
            except Exception:
                continue
    return out


# --------------------------------------------------------------------------- #
# builders                                                                     #
# --------------------------------------------------------------------------- #


def _build_value_index(dataset_id: str, database: str, schema: str, columns: List[dict],
                       dimensions: List[dict], snapshots: Dict[str, dict]) -> List[Dict[str, Any]]:
    """Enumerate the values of each low-cardinality dimension. A bounded GROUP BY
    scan is the primary source — it returns the COMPLETE set exactly, which
    matters for uniformly-distributed dims (e.g. covid 'country' in a daily
    panel) where pg_stats most_common_vals only samples a few. pg_stats is used
    just to pre-skip clearly high-cardinality columns (ids, free text) so we
    never scan those. Falls back to most_common_vals if the scan can't run."""
    key_by_dim = _dim_key_columns(dimensions)
    out: List[Dict[str, Any]] = []
    for col in columns:
        if not col.get("is_dimension"):
            continue
        table = (col.get("table_name") or "").strip()
        cname = (col.get("column_name") or col.get("name") or "").strip()
        if not (cname and table):
            continue
        ek = profiler.column_key(schema, table, cname)
        snap = snapshots.get(ek)

        # pre-skip high-cardinality columns using the cheap pg_stats estimate
        if snap:
            prof = _loads(snap.get("profile")) or {}
            est = _estimate_distinct(prof.get("n_distinct"), snap.get("row_count") or 0)
            if est is not None and est > _MAX_CARDINALITY_FOR_VALUES:
                continue

        # complete, exact values via bounded scan (returns None if > cap or fails)
        pairs = _scan_distinct(database, schema, table, cname, _MAX_CARDINALITY_FOR_VALUES)
        source = "scan.group_by"
        if pairs is None:               # scan unavailable — fall back to pg_stats sample
            pairs = []
            if snap:
                prof = _loads(snap.get("profile")) or {}
                rc = snap.get("row_count") or 0
                vals = _parse_pg_array(prof.get("most_common_vals"))
                freqs = _parse_pg_floats(prof.get("most_common_freqs"))
                pairs = [(v, (freqs[i] if i < len(freqs) else 0.0) * float(rc or 0))
                         for i, v in enumerate(vals)]
                source = "pg_stats.most_common_vals"

        if not pairs:
            continue
        key_col = key_by_dim.get(table)
        for v, freq in pairs:
            if v is None or str(v).strip() == "":
                continue
            out.append({
                "id": str(uuid.uuid4()),
                "dataset_id": dataset_id,
                "element_key": ek,
                "value_text": str(v),
                "value_norm": _normalize(v),
                "key_column": key_col,
                "key_value": str(v),  # dimension label == filter value unless a key map exists
                "freq": float(freq or 0),
                "source": source,
            })
    return out


def _manifest(ds: dict, columns: List[dict], dimensions: List[dict],
              metrics: List[dict]) -> Dict[str, Any]:
    cols = [{
        "table": c.get("table_name"),
        "name": c.get("column_name") or c.get("name"),
        "type": c.get("data_type"),
        "is_dimension": bool(c.get("is_dimension")),
        "is_metric": bool(c.get("is_metric")),
        "semantic_type": c.get("semantic_type"),
        "synonyms": _synonyms_for(c.get("column_name") or c.get("name") or ""),
    } for c in columns]
    joins = [{
        "dim_table": d.get("dimension_table") or d.get("table_name"),
        "fact_key": d.get("fact_key"),
        "join_key": d.get("join_key"),
        "join_condition": d.get("join_condition"),
        "display_name": d.get("display_name") or d.get("dim_name"),
    } for d in dimensions]
    mets = [{
        "name": m.get("name") or m.get("metric_name"),
        "expression": m.get("expression"),
        "type": m.get("metric_type"),
        "format": m.get("format"),
        "synonyms": _synonyms_for(m.get("name") or m.get("metric_name") or ""),
    } for m in metrics]
    return {
        "dataset_id": ds.get("id"),
        "name": ds.get("dataset_name") or ds.get("name"),
        "fact_table": ds.get("table_name") or ds.get("fact_table"),
        "schema": ds.get("schema_name"),
        "date_column": ds.get("date_column"),
        "grain": ds.get("date_column"),
        "columns": cols,
        "joins": joins,
        "metrics": mets,
        "context_spec": _suggest_spec(columns, metrics),
    }


def _qid(x: str) -> str:
    return '"' + str(x).replace('"', '""') + '"'


def _scan_distinct(database: str, schema: str, table: str, column: str,
                   cap: int) -> Optional[List[tuple]]:
    """Bounded generate-time distinct scan for a column whose table has no
    catalog stats (a view, or never analyzed). Returns [(value, count), ...] or
    None if the column is high-cardinality (> cap) or the scan fails. Works on
    views. One-time cost, at generate — never on the query path."""
    import database.pool as pool
    tbl = f"{_qid(schema)}.{_qid(table)}" if schema else _qid(table)
    qcol = _qid(column)
    sql = (f"SELECT {qcol} AS v, COUNT(*) AS c FROM {tbl} "
           f"WHERE {qcol} IS NOT NULL GROUP BY {qcol} ORDER BY c DESC LIMIT {int(cap) + 1}")
    try:
        res = pool.execute_query(sql, database)
    except Exception:
        return None
    rows = res.get("rows_objects", res.get("rows", []))
    if len(rows) > cap:      # high-cardinality — don't index values
        return None
    out: List[tuple] = []
    for r in rows:
        if isinstance(r, dict):
            out.append((r.get("v"), _num(r.get("c"))))
    return out


def _scan_min_max(database: str, schema: str, table: str, column: str,
                  require_nonnull: Optional[List[str]] = None) -> Optional[tuple]:
    """MIN/MAX of a column via one aggregate. With *require_nonnull*, restricts to
    rows where ALL those columns are non-null — used to bound the date range to
    where the defined metrics actually have data (so a future placeholder row
    with null metrics doesn't stretch the range)."""
    import database.pool as pool
    tbl = f"{_qid(schema)}.{_qid(table)}" if schema else _qid(table)
    qcol = _qid(column)
    where = ""
    if require_nonnull:
        where = " WHERE " + " AND ".join(f"{_qid(c)} IS NOT NULL" for c in require_nonnull)
    try:
        res = pool.execute_query(f"SELECT MIN({qcol}) AS mn, MAX({qcol}) AS mx FROM {tbl}{where}", database)
    except Exception:
        return None
    rows = res.get("rows_objects", res.get("rows", []))
    if not rows or not isinstance(rows[0], dict):
        return None
    mn, mx = rows[0].get("mn"), rows[0].get("mx")
    if mn is None and mx is None:
        return None
    return (str(mn) if mn is not None else None, str(mx) if mx is not None else None)


def _num(v: Any) -> float:
    try:
        return float(v)
    except Exception:
        return 0.0


def _stats_rollup(database: str, schema: str, columns: List[dict], dimensions: List[dict],
                  snapshots: Dict[str, dict], metrics: Optional[List[dict]] = None,
                  date_column: Optional[str] = None,
                  fact_table: Optional[str] = None) -> Dict[str, Any]:
    cardinalities: Dict[str, Any] = {}
    freshest = None
    row_counts: Dict[str, Any] = {}
    for col in columns:
        table = (col.get("table_name") or "").strip()
        cname = (col.get("column_name") or col.get("name") or "").strip()
        if not (table and cname):
            continue
        snap = snapshots.get(profiler.column_key(schema, table, cname))
        if not snap:
            continue
        prof = _loads(snap.get("profile")) or {}
        cardinalities[f"{table}.{cname}"] = prof.get("n_distinct")
        if snap.get("row_count") is not None:
            row_counts[table] = snap.get("row_count")
        ra = snap.get("refreshed_at")
        if ra and (freshest is None or ra > freshest):
            freshest = ra
    metric_cols = _metric_columns(metrics or [], columns, date_column)
    return {
        "cardinalities": cardinalities,
        "row_counts": row_counts,
        "profiled_at": freshest,
        "date_range": _date_range(database, schema, columns, snapshots, date_column, fact_table, metric_cols),
    }


def _metric_columns(metrics: List[dict], columns: List[dict], date_column: Optional[str]) -> List[str]:
    """Underlying fact columns referenced by the dataset's defined metric
    expressions (e.g. SUM(primary_energy_consumption) -> primary_energy_consumption).
    Used to bound the date range to where those metrics actually have data."""
    colnames = {(c.get("column_name") or c.get("name") or "").strip() for c in columns}
    colnames.discard("")
    dc = (date_column or "").strip()
    out: List[str] = []
    seen: set = set()
    for m in metrics:
        for ident in re.findall(r"[A-Za-z_][A-Za-z0-9_]*", m.get("expression") or ""):
            if ident in colnames and ident != dc and ident not in seen:
                seen.add(ident)
                out.append(ident)
    return out


def _date_range(database: str, schema: str, columns: List[dict], snapshots: Dict[str, dict],
                date_column: Optional[str], fact_table: Optional[str],
                metric_cols: Optional[List[str]] = None) -> Optional[Dict[str, Any]]:
    """Min/max of the dataset's date column, bounded to where the defined metrics
    have data. ``basis='metric_coverage'`` means the range only spans rows where
    every defined-metric column is non-null (so a future placeholder row with
    empty metrics won't extend it — e.g. energy 'year' reaches 2025 but the
    consumption metric ends 2024). Falls back to the full column span
    (``basis='all_rows'``) when no metrics resolve."""
    if not date_column:
        return None
    dc = date_column.strip()
    # locate the table owning the date column; default to the fact table
    table = fact_table
    for col in columns:
        if (col.get("column_name") or col.get("name") or "").strip() == dc:
            table = (col.get("table_name") or table or "").strip()
            break
    if not table:
        return None

    # Metric-coverage range: exact MIN/MAX over rows where every defined metric
    # is present. One aggregate query at generate time; works on views.
    if metric_cols:
        mm = _scan_min_max(database, schema, table, dc, require_nonnull=metric_cols)
        if mm and (mm[0] is not None or mm[1] is not None):
            return {"column": dc, "min": mm[0], "max": mm[1], "approx": False, "basis": "metric_coverage"}

    # Fallback: full column span — zero-scan histogram / most_common_vals first.
    snap = snapshots.get(profiler.column_key(schema, table, dc))
    if snap:
        prof = _loads(snap.get("profile")) or {}
        bounds = _parse_pg_array(prof.get("histogram_bounds"))
        if bounds:
            return {"column": dc, "min": bounds[0], "max": bounds[-1], "approx": True, "basis": "all_rows"}
        vals = sorted(_parse_pg_array(prof.get("most_common_vals")))
        if vals:
            return {"column": dc, "min": vals[0], "max": vals[-1], "approx": True, "basis": "all_rows"}

    mm = _scan_min_max(database, schema, table, dc)
    if mm:
        return {"column": dc, "min": mm[0], "max": mm[1], "approx": False, "basis": "all_rows"}
    return None


def ask(question: str, limit: int = 50) -> Dict[str, Any]:
    """Deterministic NL -> SQL via the DLM — no LLM."""
    t0 = _time_mod.monotonic()
    ensure_tables()
    routed = route(question, limit=1)
    if not routed:
        return {"ok": False, "reason": "no_dataset"}
    dataset_id = str(routed[0]["dataset_id"])
    ds = datasets_svc.get_dataset_by_id(dataset_id)
    if not ds:
        return {"ok": False, "reason": "dataset_not_found"}

    database = ds.get("database_name") or _metadata_database()
    schema = ds.get("schema_name") or "public"
    fact = ds.get("table_name") or ds.get("fact_table")
    if not fact:
        return {"ok": False, "reason": "no_fact_table"}
    date_column = ds.get("date_column")
    columns = ds.get("columns") or []
    metrics = ds.get("metrics") or []
    dims = [c for c in columns if c.get("is_dimension")]

    # per-dataset curated context: aliases, default metric, hidden items
    spec = _effective_spec(dataset_id)
    m_alias = _alias_index(spec, "metrics")
    d_alias = _alias_index(spec, "dimensions")
    _mspec, _dspec = spec.get("metrics") or {}, spec.get("dimensions") or {}
    metrics = [m for m in metrics
               if not _mspec.get(m.get("name") or m.get("metric_name"), {}).get("hidden")]
    dims = [d for d in dims
            if not _dspec.get(d.get("column_name") or d.get("name"), {}).get("hidden")]

    qset = set(_tokenize(question))

    # 1) entity filters from the value index (e.g. "india" -> country='India')
    filters = _resolve_entity_filters(dataset_id, question)

    # 1b) detect unresolved entity phrases — if the question says "in <X>" or
    #     "for <X>" but <X> didn't match any value, warn the user instead of
    #     silently ignoring the filter and returning unfiltered results.
    unresolved_entity = None
    if not filters:
        ent_m = re.search(r"\b(?:in|for)\s+([A-Z][a-z]+(?:\s+[A-Z][a-z]+)*)\b", question)
        if ent_m:
            candidate = ent_m.group(1)
            if candidate.lower() not in _STOPWORDS and len(candidate) >= 3:
                unresolved_entity = candidate

    # 2) metric — curated aliases + default metric; generic quantifier words ignored
    metric = _match_metric(qset, metrics, m_alias, spec.get("default_metric"))

    # 3) group-by dimension ("... by country")
    group_col = _match_group_by(question, dims, d_alias)

    # 3b) top-N ("top 10 countries by consumption") — sets the row limit and, if
    #     no explicit "by <dim>", groups by the dimension named in the question.
    top_n = None
    mtop = re.search(r"\btop\s+(\d+)\b", question, re.I)
    if mtop:
        top_n = int(mtop.group(1))
    elif re.search(r"\btop\b", question, re.I):
        top_n = 10  # "top models" without a number → default 10
    # Superlative → top 1 ("which country has the most", "highest scoring model")
    if not top_n and re.search(r"\b(which|what)\b.*\b(most|highest|largest|biggest|greatest|lowest|least|fewest|smallest)\b", question, re.I):
        top_n = 1
    if top_n:
        if not group_col:
            group_col = _match_any_dim(question, dims, d_alias)
    # If "by <metric>" was parsed but didn't match a dimension, also try
    # matching a dimension anywhere in the question (e.g. "top models by ELO")
    wanted_groupby = bool(re.search(r"\b(?:by|per|across|for each)\b", question, re.I))
    if not group_col and wanted_groupby:
        group_col = _match_any_dim(question, dims, d_alias)
    limit_n = top_n or limit
    sort_asc = bool(re.search(r"\b(lowest|least|fewest|smallest|bottom)\b", question, re.I))

    note = None
    if unresolved_entity:
        dim_names = [d.get("column_name") or d.get("name") for d in dims if d.get("column_name") or d.get("name")]
        note = f'"{unresolved_entity}" not found in this dataset\'s values.' + (
            f" Available dimensions: {', '.join(dim_names)}." if dim_names else "")
    # If the user asked for a breakdown but no dimension matched, note it
    if wanted_groupby and not group_col:
        dim_names = [d.get("column_name") or d.get("name") for d in dims if d.get("column_name") or d.get("name")]
        if dim_names:
            note = f"No matching breakdown found. Available dimensions: {', '.join(dim_names)}."

    # 4) year filter
    year = _extract_year(question)

    # 4-rel) relative time filter ("last 7 days", "this month", "yesterday")
    relative_time = _extract_relative_time(question) if not year else None
    if relative_time and not date_column:
        relative_time = None  # dataset has no date column — can't filter by time

    # 4a) if the requested year spans the dataset's ENTIRE date range, the filter
    #     is a no-op — e.g. a single-year dataset (2026-only) asked "... in 2026".
    #     Drop it (zero DB trip via the cached range) so the answer serves from
    #     precomputed context instead of a full live scan.
    if year:
        _dlo, _dhi = _dataset_year_bounds(dataset_id)
        if _dlo is not None and _dhi is not None and _dlo == _dhi == year:
            year = None

    # 4b) if the requested year is beyond where this metric actually has data
    #     (for these entity filters), answer with the latest available year and
    #     say so, instead of returning an empty result for a future/missing year.
    metric_name = (metric or {}).get("name") or "Count"
    if year and metric:
        lo, hi = _metric_year_bounds(database, schema, fact, date_column, metric, columns, filters)
        if hi is not None and year > hi:
            note = f"No data for {year} yet — showing the latest available ({hi}) for {metric_name}."
            year = hi
        elif lo is not None and year < lo:
            note = f"No data for {year} — {metric_name} data starts in {lo}; showing {lo}."
            year = lo

    # 5) trend detection — "trend over time" / "by year" → time-series line
    is_trend = bool(re.search(r"\b(trend|over time|by year|by month|yearly|monthly|over the years)\b", question, re.I))
    time_group = None
    if is_trend and date_column and not group_col:
        time_group = date_column
        year = None  # don't filter by year when showing trend

    # ── answer from context (precomputed) — NO database trip ─────────────────
    # Totals, single-dimension breakdowns, and single-dimension equality filters
    # are all already in dlm_answers. Only trends/year-slices/combos/relative-time
    # fall through to a live query below (which we then cache).
    if not time_group and not year and not relative_time:
        served = _serve_from_context(dataset_id, ds, metric_name, group_col, top_n,
                                     filters, routed[0], sort_asc=sort_asc)
        if served is not None:
            if note:
                served["note"] = note
            served["duration_ms"] = round((_time_mod.monotonic() - t0) * 1000, 1)
            return served

        # exact combo not materialized → for a non-additive COUNT(DISTINCT) metric,
        # answer approximately from the HLL sketch cuboid (register union, no scan)
        # before falling to a live query.
        if metric and _distinct_col((metric or {}).get("expression") or ""):
            sketched = _serve_sketch(dataset_id, ds, metric_name, group_col, top_n,
                                     filters, routed[0])
            if sketched is not None:
                sketched["duration_ms"] = round((_time_mod.monotonic() - t0) * 1000, 1)
                return sketched

    # ── assemble (live query path) ───────────────────────────────────────────
    metric_expr = (metric or {}).get("expression") or "COUNT(*)"

    select_parts: List[str] = []
    if time_group:
        select_parts.append(_qid(time_group))
    if group_col:
        select_parts.append(_qid(group_col))
    select_parts.append(f"{metric_expr} AS {_qid(metric_name)}")

    where: List[str] = []
    for f in filters:
        where.append(f"{_qid(f['column'])} = '{str(f['value']).replace(chr(39), chr(39) * 2)}'")
    if year and date_column:
        where.append(_year_clause(date_column, year, columns))
    elif relative_time and date_column:
        where.append(f"{_qid(date_column)} >= {relative_time}")

    sql = f"SELECT {', '.join(select_parts)} FROM {_qid(schema)}.{_qid(fact)}"
    if where:
        sql += " WHERE " + " AND ".join(where)
    if time_group:
        gb = _qid(time_group)
        if group_col:
            gb += f", {_qid(group_col)}"
        sql += f" GROUP BY {gb} ORDER BY {_qid(time_group)} LIMIT 1000"
    elif group_col:
        order_dir = "ASC" if sort_asc else "DESC"
        sql += f" GROUP BY {_qid(group_col)} ORDER BY {_qid(metric_name)} {order_dir} LIMIT {int(limit_n)}"

    chart_type = "line" if time_group else ("bar" if group_col else "kpi")
    x_axis = time_group or group_col

    title = metric_name
    if time_group:
        title += " over time"
    elif group_col:
        title += f" by {group_col}"
    ctx_bits = [f["value"] for f in filters] + ([str(year)] if year else [])
    if relative_time:
        rt_m = _RELATIVE_TIME_RE.search(question) or _RELATIVE_NAMED_RE.search(question)
        rt_label = rt_m.group(0).strip() if rt_m else ("today" if _TODAY_RE.search(question) else "yesterday")
        ctx_bits.append(rt_label)
    if ctx_bits:
        title += " — " + ", ".join(str(b) for b in ctx_bits)
    if note:
        title += " (latest available)"

    return {
        "ok": True,
        "dataset_id": dataset_id,
        "dataset_name": ds.get("dataset_name") or ds.get("name"),
        "database": database,
        "schema_name": schema,
        "sql": sql,
        "from_context": False,
        "route": "live",
        "chartType": chart_type,
        "xAxis": x_axis,
        "yAxis": metric_name,
        "title": title,
        "columns": ([group_col] if group_col else []) + [metric_name],
        "filters": filters,
        "year": year,
        "note": note,
        # what we already know from context — shown instantly while the live
        # query fetches the exact (multi-filter / combo) figure.
        "context_hints": _context_hints(dataset_id, metric_name, filters),
        "confidence": round(routed[0].get("score", 0.0), 3),
        "duration_ms": round((_time_mod.monotonic() - t0) * 1000, 1),
    }


def _context_hints(dataset_id: str, metric_name: str,
                   filters: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Relevant precomputed slices to surface while a live query runs: the metric's
    grand total and its value for each single-dimension filter (pulled from the
    by-that-dimension breakdown). All in-memory dict hits — no database trip."""
    hints: List[Dict[str, Any]] = []
    total = _context_answer(dataset_id, metric_name, "")
    if total and total.get("rows"):
        try:
            hints.append({"label": f"{metric_name}, overall", "value": total["rows"][0][0]})
        except Exception:
            pass
    for f in filters or []:
        ctx = _context_answer(dataset_id, metric_name, f.get("column"))
        if not ctx:
            continue
        want = _normalize(f.get("value"))
        for r in ctx.get("rows", []):
            if r and _normalize(r[0]) == want:
                hints.append({"label": f"{metric_name} in {f.get('value')}", "value": r[1]})
                break
    return hints


# In-memory answer cache: {dataset_id: {(metric_name, group_col): {columns, rows}}}.
# Warmed lazily from dlm_answers (one query per dataset), invalidated on regen.
# After warmup every context answer is a dict hit — microseconds, zero DB trip.
_ANSWER_CACHE: Dict[str, Dict[tuple, Dict[str, Any]]] = {}


def _load_answers(dataset_id: str) -> Dict[tuple, Dict[str, Any]]:
    cached = _ANSWER_CACHE.get(dataset_id)
    if cached is not None:
        return cached
    ensure_tables()
    res = meta.query(
        "SELECT metric_name, group_col, columns, rows FROM dlm_answers WHERE dataset_id = @param0",
        [dataset_id])
    out: Dict[tuple, Dict[str, Any]] = {}
    for r in res.get("rows_objects", res.get("rows", [])):
        if not isinstance(r, dict):
            continue
        out[(r.get("metric_name"), r.get("group_col") or "")] = {
            "columns": _loads(r.get("columns")) or [],
            "rows": _loads(r.get("rows")) or [],
        }
    _ANSWER_CACHE[dataset_id] = out
    return out


def _context_answer(dataset_id: str, metric_name: str, group_col: str) -> Optional[Dict[str, Any]]:
    return _load_answers(dataset_id).get((metric_name, group_col or ""))


# Per-dataset HLL sketch cuboids: {metric_name: {"dims":[...], "cells": {raw-value
# tuple: sparse-registers}}}. Warmed once per dataset, invalidated on regen/save.
_SKETCH_CACHE: Dict[str, Dict[str, dict]] = {}


def _load_sketches(dataset_id: str) -> Dict[str, dict]:
    cached = _SKETCH_CACHE.get(dataset_id)
    if cached is not None:
        return cached
    ensure_tables()
    res = meta.query(
        "SELECT metric_name, cell_key, registers FROM dlm_sketch WHERE dataset_id = @param0",
        [dataset_id])
    out: Dict[str, dict] = {}
    for r in res.get("rows_objects", res.get("rows", [])):
        if not isinstance(r, dict):
            continue
        mn = r.get("metric_name")
        ck = r.get("cell_key")
        reg = r.get("registers")
        m = out.setdefault(mn, {"dims": [], "cells": {}})
        if not ck:                          # header row carries the dim list
            try:
                m["dims"] = (_loads(reg) or {}).get("dims") or []
            except Exception:
                m["dims"] = []
        else:
            try:
                m["cells"][tuple(_loads(ck) or [])] = reg
            except Exception:
                continue
    _SKETCH_CACHE[dataset_id] = out
    return out


# Per-dataset (min_year, max_year) from the precomputed stats_rollup.date_range —
# warmed once, invalidated on regen. Lets ask() tell whether a requested year
# spans the whole dataset (a no-op filter) with ZERO database trip.
_RANGE_CACHE: Dict[str, tuple] = {}


def _dataset_year_bounds(dataset_id: str) -> tuple:
    """(min_year, max_year) for the dataset's date column, read from the compiled
    stats_rollup.date_range — no live query. (None, None) when unknown."""
    if dataset_id in _RANGE_CACHE:
        return _RANGE_CACHE[dataset_id]
    ensure_tables()
    res = meta.query("SELECT stats_rollup FROM dlm_artifact WHERE dataset_id = @param0", [dataset_id])
    lo = hi = None
    rows = res.get("rows_objects", res.get("rows", []))
    if rows:
        r = rows[0]
        stats = _loads(r.get("stats_rollup") if isinstance(r, dict) else r[0]) or {}
        dr = stats.get("date_range") or {}

        def _yr(v):
            if v is None:
                return None
            try:
                return int(v) if isinstance(v, int) else int(str(v)[:4])
            except Exception:
                return None
        lo, hi = _yr(dr.get("min")), _yr(dr.get("max"))
    _RANGE_CACHE[dataset_id] = (lo, hi)
    return (lo, hi)


# --------------------------------------------------------------------------- #
# freshness scoring + background auto-rebuild                                  #
# --------------------------------------------------------------------------- #

# Tracks in-flight rebuilds: dataset_id -> timestamp when the rebuild started.
# Prevents concurrent rebuilds of the same dataset and acts as a cooldown so a
# stale dataset doesn't spam rebuilds on every request.
_REBUILD_IN_PROGRESS: Dict[str, float] = {}
_REBUILD_LOCK = threading.Lock()

# Minimum seconds between rebuild attempts for the same dataset. Prevents
# hammering: once a rebuild starts (or recently finished), no new one kicks off.
_REBUILD_COOLDOWN_SECONDS = 300.0  # 5 minutes

# Freshness score below which auto-rebuild triggers.
_STALE_THRESHOLD = 0.5


def invalidate_caches(dataset_id: Optional[str] = None) -> int:
    """Clear in-memory caches. Returns count of datasets cleared."""
    if dataset_id:
        ds_id = str(dataset_id)
        cleared = int(ds_id in _ANSWER_CACHE)
        _ANSWER_CACHE.pop(ds_id, None)
        _SKETCH_CACHE.pop(ds_id, None)
        _RANGE_CACHE.pop(ds_id, None)
        _SPEC_CACHE.pop(ds_id, None)
        return cleared
    n = len(_ANSWER_CACHE)
    _ANSWER_CACHE.clear()
    _SKETCH_CACHE.clear()
    _RANGE_CACHE.clear()
    _SPEC_CACHE.clear()
    return n


def check_freshness(dataset_id: str) -> Dict[str, Any]:
    """Compute how fresh a dataset's DLM context is, combining time decay since
    the artifact was built with the data-change signal from pg_stat_user_tables.
    Returns a recommendation: use_context / rebuild / no_context."""
    ensure_tables()

    # 1) load artifact metadata
    art = meta.query_one(
        "SELECT built_at, status, stats_rollup, manifest FROM dlm_artifact "
        "WHERE dataset_id = @param0", [str(dataset_id)])
    if not art:
        return {
            "fresh": False, "score": 0.0, "computed_at": None,
            "data_modified": False, "recommendation": "no_context",
        }

    built_at = art.get("built_at")
    manifest = _loads(art.get("manifest")) or {}
    stats = _loads(art.get("stats_rollup")) or {}
    fact_table = manifest.get("fact_table")
    schema = manifest.get("schema") or "public"

    # 2) time factor — exponential decay from built_at
    from dlm.validity import _parse_dt, _time_factor, BASE_HALF_LIFE_SECONDS
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    refreshed = _parse_dt(built_at) or now
    age_seconds = max(0.0, (now - refreshed).total_seconds())
    t_factor = _time_factor(age_seconds, BASE_HALF_LIFE_SECONDS)

    # 3) change factor — row modifications since the artifact was built
    c_factor = 1.0
    data_modified = False
    if fact_table:
        ds = datasets_svc.get_dataset_by_id(str(dataset_id))
        database = (ds.get("database_name") or _metadata_database()) if ds else _metadata_database()
        try:
            from dlm.validity import _live_change, _change_factor, CHANGE_HALF_FRACTION
            live = _live_change(database, schema)
            live_stats = live.get(fact_table)
            if live_stats:
                mods_now = int(live_stats.get("mods_since_analyze") or 0)
                # row count at build time from stats_rollup
                row_counts = stats.get("row_counts") or {}
                rows_at_build = int(row_counts.get(fact_table) or 0) or 1
                # treat any non-zero mod count as potential data change
                data_modified = mods_now > 0
                c_factor = _change_factor(rows_at_build, mods_now, 0, False)
        except Exception:
            pass

    score = round(t_factor * c_factor, 4)
    fresh = score >= _STALE_THRESHOLD

    if score >= 0.7:
        recommendation = "use_context"
    elif art.get("status") == "ready":
        recommendation = "rebuild"
    else:
        recommendation = "no_context"

    return {
        "fresh": fresh,
        "score": score,
        "computed_at": built_at,
        "data_modified": data_modified,
        "recommendation": recommendation,
    }


def _trigger_background_rebuild(dataset_id: str) -> bool:
    """Kick off a background thread to rebuild the DLM for a dataset if one isn't
    already in progress (or recently completed). Returns True if a rebuild was
    started, False if skipped (cooldown / already running)."""
    now = _time_mod.time()
    with _REBUILD_LOCK:
        last = _REBUILD_IN_PROGRESS.get(dataset_id)
        if last is not None and (now - last) < _REBUILD_COOLDOWN_SECONDS:
            return False
        _REBUILD_IN_PROGRESS[dataset_id] = now

    def _do_rebuild():
        try:
            logger.info("Auto-rebuild started for dataset %s", dataset_id)
            generate_dlm(dataset_id, force=True)
            logger.info("Auto-rebuild completed for dataset %s", dataset_id)
        except Exception:
            logger.exception("Auto-rebuild failed for dataset %s", dataset_id)
        finally:
            # Update timestamp so cooldown runs from completion, not start.
            with _REBUILD_LOCK:
                _REBUILD_IN_PROGRESS[dataset_id] = _time_mod.time()

    threading.Thread(target=_do_rebuild, daemon=True, name=f"dlm-rebuild-{dataset_id}").start()
    return True


def maybe_auto_rebuild(dataset_id: str) -> Optional[bool]:
    """Check freshness and trigger a background rebuild when stale. Called from
    the ask/serve path so context stays current without manual intervention.
    Returns True if rebuild was triggered, False if skipped, None if context is
    fresh (no action needed)."""
    freshness = check_freshness(dataset_id)
    if freshness["fresh"]:
        return None
    if freshness["recommendation"] != "rebuild":
        return None
    return _trigger_background_rebuild(dataset_id)


_SWEEP_INTERVAL_SECONDS = 1800.0  # 30 minutes


def freshness_sweep() -> Dict[str, Any]:
    """Check all datasets and trigger rebuilds for any that are stale. Returns
    a summary of what was found and triggered — useful for cron/health checks."""
    ensure_tables()
    arts = meta.query("SELECT dataset_id FROM dlm_artifact WHERE status = 'ready'", [])
    results: Dict[str, Any] = {"checked": 0, "stale": 0, "triggered": 0, "datasets": []}
    for a in arts.get("rows_objects", arts.get("rows", [])):
        did = str(a.get("dataset_id"))
        results["checked"] += 1
        freshness = check_freshness(did)
        if not freshness["fresh"] and freshness["recommendation"] == "rebuild":
            results["stale"] += 1
            triggered = _trigger_background_rebuild(did)
            if triggered:
                results["triggered"] += 1
            results["datasets"].append({"dataset_id": did, "score": freshness["score"], "triggered": triggered})
    return results


def _start_sweep_loop():
    """Background loop that runs freshness_sweep periodically. Started once at
    import time so DLM context stays fresh without requiring user traffic."""
    def _loop():
        import time as _t
        _t.sleep(30)  # let the app finish startup
        while True:
            try:
                summary = freshness_sweep()
                if summary["triggered"]:
                    logger.info("DLM sweep: checked=%d stale=%d triggered=%d",
                                summary["checked"], summary["stale"], summary["triggered"])
            except Exception:
                logger.exception("DLM sweep failed")
            _t.sleep(_SWEEP_INTERVAL_SECONDS)

    threading.Thread(target=_loop, daemon=True, name="dlm-sweep").start()


_start_sweep_loop()


def _serve_from_context(dataset_id: str, ds: dict, metric_name: str, group_col: Optional[str],
                        top_n: Optional[int], filters: List[Dict[str, Any]],
                        routed: dict, sort_asc: bool = False) -> Optional[Dict[str, Any]]:
    """Serve totals / single-dim breakdowns / single-dim equality filters straight
    from the precomputed answers — no live query."""
    dataset_name = ds.get("dataset_name") or ds.get("name")
    conf = round(routed.get("score", 0.0), 3)

    if not filters:
        ctx = _context_answer(dataset_id, metric_name, group_col or "")
        if ctx is None:
            return None
        rows = ctx["rows"]
        if group_col and sort_asc:
            rows = list(reversed(rows))
        if group_col and top_n:
            rows = rows[:int(top_n)]
        return _ctx_response(dataset_id, dataset_name, metric_name, group_col, ctx["columns"], rows, conf)

    # exactly one single-dimension equality filter, no group-by → pick the row
    if len(filters) == 1 and not group_col:
        f = filters[0]
        ctx = _context_answer(dataset_id, metric_name, f["column"])
        if ctx is None:
            return None
        want = _normalize(f.get("value"))
        for r in ctx["rows"]:
            if r and _normalize(r[0]) == want:
                return _ctx_response(dataset_id, dataset_name, metric_name, None,
                                     [metric_name], [[r[1]]], conf, subtitle=str(f.get("value")))

    # one filter WITH group_by → 2-dim combo lookup, filter one dim, return the other
    if len(filters) == 1 and group_col:
        f = filters[0]
        pair = sorted([f["column"], group_col])
        ctx = _context_answer(dataset_id, metric_name, f"{pair[0]}|{pair[1]}")
        if ctx is not None:
            want = _normalize(f.get("value"))
            fi = 0 if _normalize(pair[0]) == _normalize(f["column"]) else 1
            gi = 1 - fi
            matched = [[r[gi], r[2]] for r in ctx["rows"]
                       if r and len(r) >= 3 and _normalize(r[fi]) == want]
            if matched:
                label = str(f.get("value"))
                if top_n:
                    matched = matched[:int(top_n)]
                return _ctx_response(dataset_id, dataset_name, metric_name, group_col,
                                     [group_col, metric_name], matched, conf, subtitle=label)

    # two single-dimension equality filters, no group-by → pick the cell from the
    # precomputed 2-dim combo (if that pair was precomputed). Exact, no DB trip.
    if len(filters) == 2 and not group_col:
        by_col = {f["column"]: f for f in filters}
        cols = sorted(by_col.keys())               # canonical pair order, matches storage
        if len(cols) == 2:
            ctx = _context_answer(dataset_id, metric_name, f"{cols[0]}|{cols[1]}")
            if ctx is None:
                return None
            w0 = _normalize(by_col[cols[0]].get("value"))
            w1 = _normalize(by_col[cols[1]].get("value"))
            for r in ctx["rows"]:
                if r and len(r) >= 3 and _normalize(r[0]) == w0 and _normalize(r[1]) == w1:
                    label = ", ".join(str(f.get("value")) for f in filters)
                    return _ctx_response(dataset_id, dataset_name, metric_name, None,
                                         [metric_name], [[r[2]]], conf, subtitle=label)

    # ── generalized N-dim handler: N filters ± groupby ────────────────────
    # Covers 2+ filters + groupby, 3+ filters no groupby — any combo that was
    # precomputed by dashboard-level curation.
    filter_cols = sorted(f["column"] for f in filters)
    if group_col:
        all_dims = sorted(set(filter_cols + [group_col]))
        key = "|".join(all_dims)
        ctx = _context_answer(dataset_id, metric_name, key)
        if ctx is not None and ctx.get("columns") and ctx.get("rows"):
            cols_list = ctx["columns"]
            dim_count = len(all_dims)
            by_col = {f["column"]: _normalize(f.get("value")) for f in filters}
            col_idx = {c: i for i, c in enumerate(cols_list) if i < dim_count}
            gb_idx = col_idx.get(group_col)
            if gb_idx is not None:
                matched = []
                for r in ctx["rows"]:
                    if not r or len(r) < dim_count + 1:
                        continue
                    if all(_normalize(r[col_idx[fc]]) == by_col[fc]
                           for fc in by_col if fc in col_idx):
                        matched.append([r[gb_idx], r[dim_count]])
                if matched:
                    label = ", ".join(str(f.get("value")) for f in filters)
                    if top_n:
                        matched = matched[:int(top_n)]
                    return _ctx_response(dataset_id, dataset_name, metric_name, group_col,
                                         [group_col, metric_name], matched, conf, subtitle=label)
    elif len(filters) >= 3:
        key = "|".join(filter_cols)
        ctx = _context_answer(dataset_id, metric_name, key)
        if ctx is not None and ctx.get("columns") and ctx.get("rows"):
            cols_list = ctx["columns"]
            n = len(filter_cols)
            by_col = {f["column"]: _normalize(f.get("value")) for f in filters}
            col_idx = {c: i for i, c in enumerate(cols_list) if i < n}
            for r in ctx["rows"]:
                if not r or len(r) < n + 1:
                    continue
                if all(_normalize(r[col_idx[fc]]) == by_col[fc]
                       for fc in by_col if fc in col_idx):
                    label = ", ".join(str(f.get("value")) for f in filters)
                    return _ctx_response(dataset_id, dataset_name, metric_name, None,
                                         [metric_name], [[r[n]]], conf, subtitle=label)

    # ── proportional multi-filter fallback ──────────────────────────────
    # When exact N-dim combos aren't precomputed, approximate by scaling
    # single-dim breakdowns with each filter's proportional weight.
    if len(filters) >= 2:
        grand = _context_answer(dataset_id, metric_name, "")
        if grand is not None and grand["rows"]:
            grand_total = grand["rows"][0][0] if grand["rows"][0] else None
            if grand_total is not None:
                _mm = next((m for m in (ds.get("metrics") or [])
                            if (m.get("name") or m.get("metric_name")) == metric_name), None)
                is_intensive = (_mm.get("metric_type") or "") in ("avg", "ratio") if _mm else False
                weights = []
                for f in filters:
                    if f.get("column") == group_col:
                        continue
                    dim_ctx = _context_answer(dataset_id, metric_name, f["column"])
                    if dim_ctx is None:
                        break
                    want = _normalize(f.get("value"))
                    dim_total = sum(r[1] for r in dim_ctx["rows"] if r and len(r) >= 2)
                    for r in dim_ctx["rows"]:
                        if r and _normalize(r[0]) == want and dim_total:
                            weights.append(r[1] / dim_total)
                            break
                    else:
                        break
                else:
                    label = ", ".join(str(f.get("value")) for f in filters)
                    if not group_col:
                        scaled = grand_total
                        if not is_intensive:
                            for w in weights:
                                scaled *= w
                        return _ctx_response(dataset_id, dataset_name, metric_name, None,
                                             [metric_name], [[round(scaled, 1)]], conf,
                                             subtitle=label, approx=True)
                    else:
                        gb_ctx = _context_answer(dataset_id, metric_name, group_col)
                        if gb_ctx is not None:
                            scaled_rows = []
                            for r in gb_ctx["rows"]:
                                if r and len(r) >= 2:
                                    val = r[1]
                                    if not is_intensive:
                                        for w in weights:
                                            val *= w
                                    scaled_rows.append([r[0], round(val, 1)])
                            scaled_rows.sort(key=lambda x: -(x[1] or 0))
                            if top_n:
                                scaled_rows = scaled_rows[:int(top_n)]
                            return _ctx_response(dataset_id, dataset_name, metric_name,
                                                 group_col, [group_col, metric_name],
                                                 scaled_rows, conf, subtitle=label,
                                                 approx=True)

    return None


def _ctx_response(dataset_id, dataset_name, metric_name, group_col, columns, rows, conf, subtitle=None, approx=False):
    title = metric_name + (f" by {group_col}" if group_col else "")
    if subtitle:
        title += f" — {subtitle}"
    return {
        "ok": True, "dataset_id": dataset_id, "dataset_name": dataset_name,
        "from_context": True, "route": "context",
        "columns": columns, "rows": rows,
        "chartType": "bar" if group_col else "kpi",
        "xAxis": group_col, "yAxis": metric_name, "title": title,
        "note": None, "confidence": conf, "approx": approx,
    }


def _serve_sketch(dataset_id: str, ds: dict, metric_name: str, group_col: Optional[str],
                  top_n: Optional[int], filters: List[Dict[str, Any]],
                  routed: dict) -> Optional[Dict[str, Any]]:
    """Answer a non-additive COUNT(DISTINCT) question from the HLL sketch cuboid by
    unioning the matching cells' registers in memory — no live scan. Covers ANY
    filter subset (and single group-by) over the cuboid dims, including the 3+ filter
    combos the exact precomputed answers don't materialize. Returns None (→ live)
    when the metric has no cuboid or a filter/group-by lands off the cuboid dims."""
    sk = _load_sketches(dataset_id).get(metric_name)
    if not sk or not sk.get("cells"):
        return None
    dims = sk.get("dims") or []
    pos = {d: i for i, d in enumerate(dims)}
    if any(f["column"] not in pos for f in filters):
        return None
    if group_col and group_col not in pos:
        return None
    cons = {pos[f["column"]]: _normalize(f.get("value")) for f in filters}
    cells = sk["cells"]

    def _match(key: tuple) -> bool:
        return all(len(key) > p and _normalize(key[p]) == v for p, v in cons.items())

    dataset_name = ds.get("dataset_name") or ds.get("name")
    conf = round(routed.get("score", 0.0), 3)

    if not group_col:
        sel = [reg for key, reg in cells.items() if _match(key)]
        if not sel:
            return None
        est = hll.union_estimate(sel)
        label = ", ".join(str(f.get("value")) for f in filters) or None
        return _sketch_response(dataset_id, dataset_name, metric_name, None,
                                [metric_name], [[est]], conf, subtitle=label)

    gp = pos[group_col]
    groups: Dict[Any, List[str]] = {}
    for key, reg in cells.items():
        if _match(key) and len(key) > gp:
            groups.setdefault(key[gp], []).append(reg)
    if not groups:
        return None
    rows = [[g, hll.union_estimate(regs)] for g, regs in groups.items()]
    rows.sort(key=lambda r: (r[1] is None, -(r[1] or 0)))
    if top_n:
        rows = rows[:int(top_n)]
    return _sketch_response(dataset_id, dataset_name, metric_name, group_col,
                            [group_col, metric_name], rows, conf)


def _sketch_response(dataset_id, dataset_name, metric_name, group_col, columns, rows, conf, subtitle=None):
    title = metric_name + (f" by {group_col}" if group_col else "")
    if subtitle:
        title += f" — {subtitle}"
    return {
        "ok": True, "dataset_id": dataset_id, "dataset_name": dataset_name,
        "from_context": True, "route": "context", "approx": True,
        "columns": columns, "rows": rows,
        "chartType": "bar" if group_col else "kpi",
        "xAxis": group_col, "yAxis": metric_name, "title": title,
        "note": "≈ estimated from a HyperLogLog sketch (~1-2% error) · no DB scan",
        "confidence": conf,
    }


def _strip_table_prefix(col: str) -> str:
    """'kaveon_usage_daily.queries_run' → 'queries_run'."""
    return col.rsplit(".", 1)[-1] if "." in col else col


def _serve_in_filter(dataset_id: str, ds: dict, metric_name: str, metric_obj: Optional[dict],
                     group_col: Optional[str], filters: List[Dict[str, Any]],
                     routed: dict) -> Optional[Dict[str, Any]]:
    """Handle IN-operator filters by looking up the precomputed 1-dim answer and
    filtering/aggregating across the matching values. For additive metrics (SUM,
    COUNT), sums across IN values. Falls back to None for non-additive."""
    in_filters = [f for f in filters if f.get("_in")]
    eq_filters = [f for f in filters if not f.get("_in")]

    if len(in_filters) != 1:
        return None

    inf = in_filters[0]
    in_col = inf["column"]
    in_vals = set(_normalize(v) for v in inf["value"])
    dataset_name = ds.get("dataset_name") or ds.get("name")
    conf = round(routed.get("score", 0.0), 3)

    if not eq_filters and not group_col:
        ctx = _context_answer(dataset_id, metric_name, in_col)
        if ctx is None:
            return None
        total = 0
        matched = 0
        for r in ctx["rows"]:
            if r and _normalize(r[0]) in in_vals:
                try:
                    total += float(r[1] or 0)
                    matched += 1
                except (ValueError, TypeError):
                    pass
        if matched:
            label = ", ".join(str(v) for v in inf["value"])
            return _ctx_response(dataset_id, dataset_name, metric_name, None,
                                 [metric_name], [[_json_scalar(total)]], conf, subtitle=label)

    if not eq_filters and group_col:
        pair = sorted([in_col, group_col])
        ctx = _context_answer(dataset_id, metric_name, f"{pair[0]}|{pair[1]}")
        if ctx is not None:
            fi = 0 if _normalize(pair[0]) == _normalize(in_col) else 1
            gi = 1 - fi
            grouped: Dict[str, float] = {}
            for r in ctx["rows"]:
                if r and len(r) >= 3 and _normalize(r[fi]) in in_vals:
                    gk = r[gi]
                    try:
                        grouped[gk] = grouped.get(gk, 0) + float(r[2] or 0)
                    except (ValueError, TypeError):
                        pass
            if grouped:
                rows = [[k, _json_scalar(v)] for k, v in sorted(grouped.items(),
                        key=lambda x: -(x[1] or 0))]
                label = ", ".join(str(v) for v in inf["value"])
                return _ctx_response(dataset_id, dataset_name, metric_name, group_col,
                                     [group_col, metric_name], rows, conf, subtitle=label)

    if len(eq_filters) >= 1:
        filter_cols = sorted(f["column"] for f in eq_filters)
        all_dims = sorted(set(filter_cols + [in_col] + ([group_col] if group_col else [])))
        key = "|".join(all_dims)
        ctx = _context_answer(dataset_id, metric_name, key)
        if ctx is not None and ctx.get("columns") and ctx.get("rows"):
            cols_list = ctx["columns"]
            dim_count = len(all_dims)
            col_idx = {c: i for i, c in enumerate(cols_list) if i < dim_count}
            in_idx = col_idx.get(in_col)
            if in_idx is not None:
                eq_match = {f["column"]: _normalize(f.get("value")) for f in eq_filters}
                if group_col and group_col in col_idx:
                    gb_idx = col_idx[group_col]
                    grouped_r: Dict[str, float] = {}
                    for r in ctx["rows"]:
                        if not r or len(r) < dim_count + 1:
                            continue
                        if _normalize(r[in_idx]) not in in_vals:
                            continue
                        if all(_normalize(r[col_idx[fc]]) == eq_match[fc]
                               for fc in eq_match if fc in col_idx):
                            gk = r[gb_idx]
                            try:
                                grouped_r[gk] = grouped_r.get(gk, 0) + float(r[dim_count] or 0)
                            except (ValueError, TypeError):
                                pass
                    if grouped_r:
                        rows = [[k, _json_scalar(v)] for k, v in sorted(grouped_r.items(),
                                key=lambda x: -(x[1] or 0))]
                        label = ", ".join(str(f.get("value")) for f in eq_filters) + ", " + ", ".join(str(v) for v in inf["value"])
                        return _ctx_response(dataset_id, dataset_name, metric_name, group_col,
                                             [group_col, metric_name], rows, conf, subtitle=label)
                else:
                    total = 0
                    matched_count = 0
                    for r in ctx["rows"]:
                        if not r or len(r) < dim_count + 1:
                            continue
                        if _normalize(r[in_idx]) not in in_vals:
                            continue
                        if all(_normalize(r[col_idx[fc]]) == eq_match[fc]
                               for fc in eq_match if fc in col_idx):
                            try:
                                total += float(r[dim_count] or 0)
                                matched_count += 1
                            except (ValueError, TypeError):
                                pass
                    if matched_count:
                        label = ", ".join(str(f.get("value")) for f in eq_filters) + ", " + ", ".join(str(v) for v in inf["value"])
                        return _ctx_response(dataset_id, dataset_name, metric_name, None,
                                             [metric_name], [[_json_scalar(total)]], conf, subtitle=label)
    return None


def serve_chart(dataset_id: str, metric_column: str, aggregation: str,
                group_by: Optional[str] = None,
                filters: Optional[List[Dict[str, Any]]] = None) -> Dict[str, Any]:
    """Serve a dashboard chart query from precomputed context (dlm_answers / HLL
    sketches) without touching the live database.  Returns ``served=True`` with
    columns/rows on a hit, or ``served=False`` when context can't answer."""
    dataset_id = str(dataset_id)
    metric_column = _strip_table_prefix(metric_column)
    if group_by:
        group_by = _strip_table_prefix(group_by)
    ds = datasets_svc.get_dataset_by_id(dataset_id)
    if not ds:
        return {"served": False, "reason": "dataset_not_found"}

    metrics = ds.get("metrics") or []
    filters = filters or []

    # ── map metric_column + aggregation to a metric name ──────────────────
    metric_name, metric_obj = _resolve_metric(metric_column, aggregation, metrics)
    if not metric_name:
        return {"served": False, "reason": "metric_not_found"}

    # ── validate + normalize filters ────────────────────────────────────
    clean_filters: List[Dict[str, Any]] = []
    has_in = False
    for f in filters:
        op = (f.get("operator") or "=").upper()
        col = _strip_table_prefix(f["column"])
        if op == "IN":
            has_in = True
            vals = [v.strip() for v in str(f["value"]).split(",") if v.strip()]
            if len(vals) == 1:
                clean_filters.append({"column": col, "value": vals[0]})
            else:
                clean_filters.append({"column": col, "value": vals, "_in": True})
        elif op == "=":
            clean_filters.append({"column": col, "value": f["value"]})
        else:
            return {"served": False, "reason": "non_equality_filter"}

    # ── validate group_by against known dimensions ────────────────────────
    group_col: Optional[str] = None
    if group_by:
        spec = _effective_spec(dataset_id)
        dim_spec = spec.get("dimensions") or {}
        if group_by in dim_spec:
            group_col = group_by
        else:
            # try case-insensitive match
            gb_norm = _normalize(group_by)
            for dk in dim_spec:
                if _normalize(dk) == gb_norm:
                    group_col = dk
                    break
        if not group_col:
            return {"served": False, "reason": "dimension_not_found"}

    # ── IN-filter expansion: serve per-value and merge ──────────────────
    dummy_routed = {"score": 1.0}
    if has_in:
        served = _serve_in_filter(dataset_id, ds, metric_name, metric_obj,
                                   group_col, clean_filters, dummy_routed)
    else:
        served = _serve_from_context(dataset_id, ds, metric_name, group_col, None,
                                      clean_filters, dummy_routed)
        if served is None and metric_obj:
            expr = (metric_obj.get("expression") or "")
            if _distinct_col(expr):
                served = _serve_sketch(dataset_id, ds, metric_name, group_col, None,
                                        clean_filters, dummy_routed)

    if served is None:
        return {"served": False}

    # ── freshness score ───────────────────────────────────────────────────
    fresh = check_freshness(dataset_id)

    return {
        "served": True,
        "columns": served.get("columns", []),
        "rows": served.get("rows", []),
        "from_context": True,
        "freshness": {
            "score": fresh.get("score", 0.0),
            "recommendation": fresh.get("recommendation"),
        },
        "chartType": served.get("chartType"),
        "xAxis": served.get("xAxis"),
        "yAxis": served.get("yAxis"),
        "title": served.get("title"),
        "approx": served.get("approx", False),
        "note": served.get("note"),
    }


def _resolve_metric(metric_column: str, aggregation: str,
                    ds_metrics: List[dict]) -> tuple:
    """Map (column, aggregation) to (metric_name, metric_obj) or (None, None)."""
    agg_upper = aggregation.strip().upper()
    if agg_upper == "COUNT_DISTINCT":
        target_expr = _normalize(f"COUNT(DISTINCT {metric_column})")
    else:
        target_expr = _normalize(f"{agg_upper}({metric_column})")
    for m in ds_metrics:
        name = m.get("name") or m.get("metric_name") or ""
        expr = m.get("expression") or ""
        if _normalize(expr) == target_expr:
            return (name, m)
    mc_norm = _normalize(metric_column)
    agg_base = agg_upper.split("_")[0]
    for m in ds_metrics:
        name = m.get("name") or m.get("metric_name") or ""
        expr = m.get("expression") or ""
        for ident in re.findall(r"[A-Za-z_][A-Za-z0-9_]*", expr):
            if _normalize(ident) == mc_norm and agg_base in expr.upper():
                return (name, m)
    return (None, None)


def serve_chart_multi(dataset_id: str,
                      metric_specs: List[Dict[str, str]],
                      group_by: Optional[str] = None,
                      filters: Optional[List[Dict[str, Any]]] = None) -> Dict[str, Any]:
    """Serve a multi-metric dashboard chart (stacked bar, combo) by merging
    precomputed per-metric answers. Returns served=True only when ALL metrics
    are answered from context."""
    dataset_id = str(dataset_id)
    if group_by:
        group_by = _strip_table_prefix(group_by)
    metric_specs = [
        {**s, "column": _strip_table_prefix(s.get("column", ""))}
        for s in metric_specs
    ]
    ds = datasets_svc.get_dataset_by_id(dataset_id)
    if not ds:
        return {"served": False, "reason": "dataset_not_found"}

    ds_metrics = ds.get("metrics") or []
    filters = filters or []

    clean_filters: List[Dict[str, Any]] = []
    for f in filters:
        if f.get("operator", "=") != "=":
            return {"served": False, "reason": "non_equality_filter"}
        clean_filters.append({"column": _strip_table_prefix(f["column"]), "value": f["value"]})

    group_col: Optional[str] = None
    if group_by:
        spec = _effective_spec(dataset_id)
        dim_spec = spec.get("dimensions") or {}
        if group_by in dim_spec:
            group_col = group_by
        else:
            gb_norm = _normalize(group_by)
            for dk in dim_spec:
                if _normalize(dk) == gb_norm:
                    group_col = dk
                    break
        if not group_col:
            return {"served": False, "reason": "dimension_not_found"}

    resolved = []
    for ms in metric_specs:
        mname, mobj = _resolve_metric(ms["column"], ms["aggregation"], ds_metrics)
        if not mname:
            return {"served": False, "reason": "metric_not_found",
                    "detail": f"{ms['aggregation']}({ms['column']})"}
        resolved.append((mname, mobj, ms))

    dummy_routed = {"score": 1.0}
    per_metric = []
    for mname, mobj, ms in resolved:
        served = _serve_from_context(dataset_id, ds, mname, group_col, None,
                                      clean_filters, dummy_routed)
        if served is None and mobj:
            expr = (mobj.get("expression") or "")
            if _distinct_col(expr):
                served = _serve_sketch(dataset_id, ds, mname, group_col, None,
                                        clean_filters, dummy_routed)
        if served is None:
            return {"served": False, "reason": "no_precomputed_answer",
                    "detail": mname}
        per_metric.append((mname, served))

    if not group_col:
        columns = [mn for mn, _ in per_metric]
        row = [s["rows"][0][0] if s.get("rows") and s["rows"][0] else None
               for _, s in per_metric]
        rows = [row]
    else:
        metric_names = [mn for mn, _ in per_metric]
        columns = [group_col] + metric_names
        merged: Dict[str, list] = {}
        for idx, (mn, s) in enumerate(per_metric):
            for r in s.get("rows") or []:
                if not r:
                    continue
                key = _normalize(str(r[0]))
                if key not in merged:
                    merged[key] = [r[0]] + [None] * len(metric_names)
                merged[key][idx + 1] = r[1] if len(r) > 1 else None
        rows = list(merged.values())

    fresh = check_freshness(dataset_id)
    return {
        "served": True,
        "columns": columns,
        "rows": rows,
        "from_context": True,
        "freshness": {
            "score": fresh.get("score", 0.0),
            "recommendation": fresh.get("recommendation"),
        },
        "chartType": per_metric[0][1].get("chartType") if per_metric else None,
        "xAxis": group_col,
        "yAxis": [mn for mn, _ in per_metric],
        "title": None,
        "approx": any(s.get("approx") for _, s in per_metric),
        "note": None,
    }


def filter_values(dataset_id: str, column: str, limit: int = 200) -> Dict[str, Any]:
    """Return distinct values for a dimension column from precomputed DLM answers.
    No live SQL — instant on any hardware."""
    dataset_id = str(dataset_id)
    column = _strip_table_prefix(column)
    ds = datasets_svc.get_dataset_by_id(dataset_id)
    if not ds:
        return {"ok": False, "reason": "dataset_not_found", "values": []}

    spec = _effective_spec(dataset_id)
    dim_spec = spec.get("dimensions") or {}
    col_norm = _normalize(column)
    matched_col = None
    for dk in dim_spec:
        if _normalize(dk) == col_norm:
            matched_col = dk
            break
    if not matched_col:
        return {"ok": False, "reason": "dimension_not_found", "values": []}

    answers = _load_answers(dataset_id)
    for (metric_name, group_col), entry in answers.items():
        if _normalize(group_col) == col_norm and entry.get("rows"):
            vals = sorted(set(
                str(r[0]) for r in entry["rows"] if r and r[0] is not None
            ))[:limit]
            return {
                "ok": True,
                "column": matched_col,
                "values": [{"key": v, "value": v} for v in vals],
                "from_context": True,
            }

    return {"ok": False, "reason": "no_precomputed_values", "values": []}


# --------------------------------------------------------------------------- #
# dashboard-level curation — N-dim combos from filters × chart groupbys        #
# --------------------------------------------------------------------------- #

MAX_CURATION_DIM = 4
CURATION_CELL_CAP = 5000


def _precompute_n_dim(dataset_id: str, database: str, schema: str, fact: str,
                      combo: tuple, mdefs: list, now: str) -> int:
    """Precompute a single N-dim GROUP BY combo for all metrics. Returns stored count."""
    n = len(combo)
    key = "|".join(combo)
    dim_sel = ", ".join(f"{_qid(d)} AS g{j}" for j, d in enumerate(combo))
    met_sel = ", ".join(f"{expr} AS {alias}" for alias, _, expr in mdefs)
    where = " AND ".join(f"{_qid(d)} IS NOT NULL" for d in combo)
    tbl = f"{_qid(schema)}.{_qid(fact)}" if schema else _qid(fact)
    order = mdefs[0][0]
    try:
        res = pool.execute_query(
            f"SELECT {dim_sel}, {met_sel} FROM {tbl} "
            f"WHERE {where} GROUP BY {', '.join(_qid(d) for d in combo)} "
            f"ORDER BY {order} DESC LIMIT {CURATION_CELL_CAP}", database)
        rows = res.get("rows") or res.get("rows_objects") or []
        if not rows:
            return 0
        norm = [_row_vals(r) for r in rows]
        stored = 0
        for j, (_, name, _) in enumerate(mdefs):
            out = []
            for rv in norm:
                row = [_json_scalar(rv[k]) for k in range(n)]
                row.append(_json_scalar(rv[n + j] if n + j < len(rv) else None))
                out.append(row)
            _store_answer(dataset_id, name, key, list(combo) + [name], out, now)
            stored += 1
        return stored
    except Exception:
        return 0


def _dashboard_combos(dashboard_id: str) -> Dict[str, set]:
    """Derive per-dataset N-dim combos from a dashboard's filters × chart groupbys.
    Returns {dataset_id: set of dim tuples (sorted, 3+ dims)}."""
    from itertools import combinations as _combs
    import services.charts as chart_svc
    import services.dashboards as dash_svc

    dash = dash_svc.get_dashboard_by_id(dashboard_id)
    if not dash:
        return {}

    filters_raw = _loads(dash.get("filters")) or []
    chart_ids = _loads(dash.get("charts")) or []

    ds_filters: Dict[str, set] = {}
    for f in filters_raw:
        col = _strip_table_prefix(f.get("column") or "")
        ds_id = str(f.get("datasetId") or "")
        if col and ds_id:
            ds_filters.setdefault(ds_id, set()).add(col)

    ds_groupbys: Dict[str, set] = {}
    for cid in chart_ids:
        chart = chart_svc.get_chart_by_id(str(cid))
        if not chart:
            continue
        qc = _loads(chart.get("query_config")) or {}
        ds_id = str(qc.get("dataset_id") or "")
        if not ds_id:
            continue
        for gb in qc.get("groupby") or []:
            ds_groupbys.setdefault(ds_id, set()).add(_strip_table_prefix(gb))

    result: Dict[str, set] = {}
    for ds_id in set(ds_filters) | set(ds_groupbys):
        f_dims = sorted(ds_filters.get(ds_id, set()))
        g_dims = sorted(ds_groupbys.get(ds_id, set()))
        combos: set = set()

        for nf in range(1, min(len(f_dims) + 1, MAX_CURATION_DIM + 1)):
            for f_sub in _combs(f_dims, nf):
                if len(f_sub) >= 3:
                    combos.add(tuple(sorted(f_sub)))
                for gb in g_dims:
                    if gb not in f_sub:
                        c = tuple(sorted(list(f_sub) + [gb]))
                        if len(c) <= MAX_CURATION_DIM:
                            combos.add(c)

        if combos:
            result[ds_id] = combos
    return result


def curate_dashboard(dashboard_id: str) -> Dict[str, Any]:
    """Precompute N-dim answer combos driven by a dashboard's filter×chart definitions.
    Stores in the same dlm_answers table — serve_chart picks them up transparently."""
    ensure_tables()
    combo_map = _dashboard_combos(dashboard_id)
    if not combo_map:
        return {"ok": False, "reason": "no_combos_needed"}

    total_stored = 0
    total_combos = 0
    now = _now_iso()

    for ds_id, combos in combo_map.items():
        ds = datasets_svc.get_dataset_by_id(ds_id)
        if not ds:
            continue
        database = ds.get("database_name") or ds.get("database")
        schema_name = ds.get("schema_name") or ds.get("schema")
        fact = ds.get("table_name") or ds.get("fact_table")
        metrics = ds.get("metrics") or []
        if not fact or not metrics:
            continue

        mdefs = []
        for i, m in enumerate(metrics):
            name = m.get("name") or m.get("metric_name")
            expr = m.get("expression")
            if name and expr:
                mdefs.append((f"m{i}", name, expr))
        if not mdefs:
            continue

        for combo in sorted(combos):
            n = _precompute_n_dim(ds_id, database, schema_name, fact,
                                  combo, mdefs, now)
            total_stored += n
            if n:
                total_combos += 1

        _ANSWER_CACHE.pop(ds_id, None)

    return {
        "ok": True,
        "dashboard_id": dashboard_id,
        "combos_computed": total_combos,
        "answers_stored": total_stored,
    }


def _curate_linked_dashboards(dataset_id: str) -> int:
    """Find dashboards that reference this dataset and curate their N-dim combos.
    Called at the end of generate_dlm to keep dashboard curation in sync."""
    try:
        rows = meta.query(
            "SELECT id, charts, filters FROM dashboards", [])
        all_dashes = rows.get("rows_objects", rows.get("rows", []))
    except Exception:
        return 0

    curated = 0
    for dash in all_dashes:
        if not isinstance(dash, dict):
            continue
        dash_id = dash.get("id")
        filters_raw = _loads(dash.get("filters")) or []
        charts_raw = _loads(dash.get("charts")) or []

        ds_ids = set()
        for f in filters_raw:
            ds_id = str(f.get("datasetId") or "")
            if ds_id:
                ds_ids.add(ds_id)

        if str(dataset_id) not in ds_ids:
            for cid in charts_raw:
                try:
                    import services.charts as chart_svc
                    chart = chart_svc.get_chart_by_id(str(cid))
                    if chart:
                        qc = _loads(chart.get("query_config")) or {}
                        cds = str(qc.get("dataset_id") or "")
                        if cds == str(dataset_id):
                            ds_ids.add(cds)
                            break
                except Exception:
                    continue

        if str(dataset_id) in ds_ids and dash_id:
            result = curate_dashboard(str(dash_id))
            if result.get("ok"):
                curated += result.get("answers_stored", 0)
    return curated


# --------------------------------------------------------------------------- #
# incremental refresh — delta processing for new data                          #
# --------------------------------------------------------------------------- #

def _metric_agg_type(expr: str) -> str:
    """Classify metric additivity from its expression."""
    e = (expr or "").strip().upper()
    if e.startswith("SUM("):
        return "additive"
    if e.startswith("COUNT(") and "DISTINCT" not in e:
        return "additive"
    if e.startswith("MIN(") or e.startswith("MAX("):
        return "semi_additive"
    return "non_additive"


def incremental_refresh(dataset_id: str) -> Dict[str, Any]:
    """Refresh DLM answers using delta processing when possible.
    Additive metrics (SUM, COUNT) merge deltas from new rows.
    Non-additive metrics trigger a full recompute of that answer."""
    dataset_id = str(dataset_id)
    ensure_tables()

    ds = datasets_svc.get_dataset_by_id(dataset_id)
    if not ds:
        return {"ok": False, "reason": "dataset_not_found"}

    art = get_dlm(dataset_id)
    if not art:
        return generate_dlm(dataset_id)

    stats = _loads(art.get("stats_rollup")) or {}
    watermark = stats.get("watermark") or {}
    prev_count = watermark.get("row_count", 0)

    database = ds.get("database_name") or ds.get("database")
    schema_name = ds.get("schema_name") or ds.get("schema")
    fact = ds.get("table_name") or ds.get("fact_table")
    date_col = ds.get("date_column")
    if not fact:
        return {"ok": False, "reason": "no_fact_table"}

    tbl = f"{_qid(schema_name)}.{_qid(fact)}" if schema_name else _qid(fact)
    try:
        cr = pool.execute_query(f"SELECT COUNT(*) AS cnt FROM {tbl}", database)
        row = (cr.get("rows") or cr.get("rows_objects") or [{}])[0]
        cur_count = int(row.get("cnt") if isinstance(row, dict) else row[0])
    except Exception:
        cur_count = 0

    if cur_count <= prev_count and prev_count > 0:
        return {"ok": True, "no_new_data": True, "row_count": cur_count}

    new_rows = cur_count - prev_count
    prev_date = watermark.get("max_date")

    metrics = ds.get("metrics") or []
    additive_metrics = [m for m in metrics
                        if _metric_agg_type(m.get("expression", "")) == "additive"]

    if date_col and prev_date and additive_metrics and new_rows < cur_count * 0.5:
        merged = _delta_merge(dataset_id, database, schema_name, fact, tbl,
                              date_col, prev_date, metrics)
    else:
        result = generate_dlm(dataset_id, force=True)
        merged = result.get("answers_precomputed", 0)

    max_date = None
    if date_col:
        try:
            dr = pool.execute_query(
                f"SELECT MAX({_qid(date_col)}) AS mx FROM {tbl}", database)
            drow = (dr.get("rows") or dr.get("rows_objects") or [{}])[0]
            max_date = str(drow.get("mx") if isinstance(drow, dict) else drow[0])
        except Exception:
            pass

    stats["watermark"] = {
        "row_count": cur_count,
        "max_date": max_date,
        "built_at": _now_iso(),
        "method": "incremental" if new_rows < cur_count * 0.5 else "full_rebuild",
        "delta_rows": new_rows,
    }
    try:
        meta.execute("UPDATE dlm_artifact SET stats_rollup = @param0 WHERE dataset_id = @param1",
                     [json.dumps(stats, default=str), str(dataset_id)])
    except Exception:
        pass

    _ANSWER_CACHE.pop(dataset_id, None)
    _SKETCH_CACHE.pop(dataset_id, None)
    _RANGE_CACHE.pop(dataset_id, None)

    return {"ok": True, "method": stats["watermark"]["method"],
            "delta_rows": new_rows, "answers_refreshed": merged}


def _delta_merge(dataset_id: str, database: str, schema: str, fact: str,
                 tbl: str, date_col: str, prev_date: str,
                 metrics: List[dict]) -> int:
    """Compute deltas for rows added since prev_date and merge into existing answers.
    Returns count of answers updated."""
    answers = _load_answers(dataset_id)
    if not answers:
        return 0

    where_new = f"{_qid(date_col)} > '{str(prev_date).replace(chr(39), chr(39)*2)}'"
    now = _now_iso()
    updated = 0

    for (metric_name, group_col), entry in list(answers.items()):
        m = next((m for m in metrics
                  if (m.get("name") or m.get("metric_name")) == metric_name), None)
        if not m:
            continue
        expr = m.get("expression", "")
        agg = _metric_agg_type(expr)

        if not group_col:
            if agg != "additive":
                continue
            try:
                dr = pool.execute_query(
                    f"SELECT {expr} AS v FROM {tbl} WHERE {where_new}", database)
                drow = (dr.get("rows") or dr.get("rows_objects") or [{}])[0]
                delta = float(drow.get("v") if isinstance(drow, dict) else drow[0] or 0)
            except Exception:
                continue
            old_val = float(entry["rows"][0][0]) if entry["rows"] and entry["rows"][0] else 0
            _store_answer(dataset_id, metric_name, "", [metric_name],
                          [[_json_scalar(old_val + delta)]], now)
            updated += 1
            continue

        dims = group_col.split("|")
        if agg != "additive":
            continue

        dim_sel = ", ".join(_qid(d) for d in dims)
        try:
            dr = pool.execute_query(
                f"SELECT {dim_sel}, {expr} AS v FROM {tbl} "
                f"WHERE {where_new} AND {' AND '.join(_qid(d) + ' IS NOT NULL' for d in dims)} "
                f"GROUP BY {dim_sel}", database)
            delta_rows = dr.get("rows") or dr.get("rows_objects") or []
        except Exception:
            continue
        if not delta_rows:
            continue

        existing = {tuple(_normalize(r[k]) for k in range(len(dims))): r
                    for r in entry["rows"] if r and len(r) > len(dims)}
        for drow in delta_rows:
            rv = _row_vals(drow)
            dim_key = tuple(_normalize(rv[k]) for k in range(len(dims)))
            delta_val = float(rv[len(dims)] or 0)
            if dim_key in existing:
                old = existing[dim_key]
                old[len(dims)] = _json_scalar(float(old[len(dims)] or 0) + delta_val)
            else:
                new_row = [_json_scalar(rv[k]) for k in range(len(dims))]
                new_row.append(_json_scalar(delta_val))
                entry["rows"].append(new_row)
                existing[dim_key] = new_row

        _store_answer(dataset_id, metric_name, group_col,
                      entry["columns"], entry["rows"], now)
        updated += 1

    return updated


def _metric_year_bounds(database: str, schema: str, table: str, date_column: Optional[str],
                        metric: dict, columns: List[dict], filters: List[Dict[str, Any]]) -> tuple:
    """(earliest, latest) year for which *metric* actually has data under the
    given entity filters — e.g. India's Total Energy spans 2020..2024 even though
    the table's 'year' column reaches 2025. Returns (None, None) when it can't be
    determined (COUNT(*), no date column, query error)."""
    if not date_column:
        return (None, None)
    mcols = _metric_columns([metric], columns, date_column)
    if not mcols:
        return (None, None)
    conds = [f"{_qid(c)} IS NOT NULL" for c in mcols]
    for f in filters:
        conds.append(f"{_qid(f['column'])} = '{str(f['value']).replace(chr(39), chr(39) * 2)}'")
    import database.pool as pool
    tbl = f"{_qid(schema)}.{_qid(table)}" if schema else _qid(table)
    dc = _qid(date_column)
    try:
        res = pool.execute_query(
            f"SELECT MIN({dc}) AS mn, MAX({dc}) AS mx FROM {tbl} WHERE {' AND '.join(conds)}", database)
    except Exception:
        return (None, None)
    rows = res.get("rows") or res.get("rows_objects") or []
    if not rows:
        return (None, None)
    row = rows[0]
    mn = row.get("mn") if isinstance(row, dict) else row[0]
    mx = row.get("mx") if isinstance(row, dict) else row[1]

    def _yr(v):
        if v is None:
            return None
        try:
            return int(v) if isinstance(v, int) else int(str(v)[:4])
        except Exception:
            return None
    return (_yr(mn), _yr(mx))


def _resolve_entity_filters(dataset_id: str, question: str) -> List[Dict[str, Any]]:
    """Resolve value phrases in the question to (column, value) filters via the
    value index. Longest n-grams first so "United States" wins over "United".
    One filter per column."""
    words = re.findall(r"[A-Za-z0-9][A-Za-z0-9&.\-]*", question)
    out: List[Dict[str, Any]] = []
    used_cols: set = set()
    used_spans: set = set()
    va = _effective_spec(dataset_id).get("value_aliases") or {}   # curated e.g. "smb" -> "Team"
    n = len(words)
    for size in (3, 2, 1):
        for i in range(0, n - size + 1):
            if any((i + k) in used_spans for k in range(size)):
                continue
            phrase = " ".join(words[i:i + size])
            if phrase.isdigit():          # years/counts are not dimension values
                continue
            # single stop/short words never denote a value ("in" != India)
            if size == 1 and (phrase.lower() in _STOPWORDS or len(phrase) < 3):
                continue
            hits = resolve_value(dataset_id, va.get(phrase.lower(), phrase), limit=1, exact_only=True)
            if not hits:
                continue
            h = hits[0]
            col = h.get("key_column") or h.get("column")
            if not col or col in used_cols:
                continue
            used_cols.add(col)
            for k in range(size):
                used_spans.add(i + k)
            out.append({"column": col, "value": h.get("key_value") or h.get("value"),
                        "element_key": h.get("element_key")})
    return out


# Generic quantifier words that must not sway metric matching. They appear in
# metric names ("Total Queries") and questions ("total number of users") alike,
# so counting them makes "users" tie with "total" and the wrong metric win.
_GENERIC_METRIC_TOKENS = {"sum", "avg", "average", "mean", "count", "total",
                         "number", "num", "amount", "overall", "all", "of", "the"}


def _match_metric(qset: set, metrics: List[dict], extra: Optional[Dict[str, List[str]]] = None,
                  default_metric: Optional[str] = None) -> Optional[dict]:
    """Pick the metric whose name/expression/alias tokens best overlap the question,
    ignoring generic quantifier words so a distinctive term like "users" wins over a
    generic one like "total". Uses stem expansion so "selling" matches "sales",
    "exported" matches "exports", etc. Falls back to the curated default metric,
    else the simplest additive metric (COUNT(*)), else the first."""
    if not metrics:
        return None
    q = _expand_tokens(qset) - _GENERIC_METRIC_TOKENS
    best, best_score = None, 0
    for m in metrics:
        name = m.get("name") or m.get("metric_name") or ""
        expr = m.get("expression") or ""
        toks = _expand_tokens(set(_tokenize(name)) | set(_tokenize(expr)) | set(_syn(name, extra))) - _GENERIC_METRIC_TOKENS
        score = len(q & toks)
        if score > best_score:
            best, best_score = m, score
    if best:
        return best
    if default_metric:
        for m in metrics:
            if (m.get("name") or m.get("metric_name")) == default_metric:
                return m
    # Prefer the simplest additive metric (COUNT(*) / SUM) over non-additive ones
    # (COUNT(DISTINCT ...)) when the question didn't name a specific metric — a
    # generic "how many" / "total number" question wants the row count, not an
    # arbitrary first metric like "Countries Tracked".
    for m in metrics:
        expr = (m.get("expression") or "").upper().strip()
        if expr in ("COUNT(*)", "COUNT(1)") or (expr.startswith("SUM(") and "DISTINCT" not in expr):
            return m
    # Among remaining, prefer additive (no DISTINCT / AVG) over non-additive
    for m in metrics:
        expr = (m.get("expression") or "").upper()
        if "DISTINCT" not in expr and "AVG" not in expr:
            return m
    return metrics[0]


def _match_group_by(question: str, dims: List[dict],
                    extra: Optional[Dict[str, List[str]]] = None) -> Optional[str]:
    """Detect a 'by <dimension>' / 'per <dimension>' grouping and map it to a
    dimension column, using per-dataset curated aliases."""
    m = re.search(r"\b(?:by|per|across|for each)\s+([A-Za-z][A-Za-z ]*)", question, re.I)
    if not m:
        return None
    target = _expand_tokens(set(_tokenize(m.group(1))))
    if not target:
        return None
    for d in dims:
        col = d.get("column_name") or d.get("name") or ""
        ctoks = _expand_tokens(set(_tokenize(col)) | set(_syn(col, extra)))
        if target & ctoks:
            return col
    return None


def _match_any_dim(question: str, dims: List[dict],
                   extra: Optional[Dict[str, List[str]]] = None) -> Optional[str]:
    """Find a dimension named anywhere in the question. Handles plurals ('orgs'
    -> org, 'countries' -> country) and typos via singular-strip + 4-char prefix."""
    qt = _expand_tokens(set(_normalize(t) for t in _tokenize(question)))
    for d in dims:
        col = (d.get("column_name") or d.get("name") or "").strip()
        cn = _normalize(col)
        if not cn:
            continue
        col_toks = _expand_tokens({cn} | set(_syn(col, extra)))
        if qt & col_toks:
            return col
        if len(cn) >= 4:
            for tn in qt:
                if len(tn) >= 4 and tn[:4] == cn[:4]:
                    return col
    return None


def _extract_year(question: str) -> Optional[int]:
    m = re.search(r"\b(19|20)\d{2}\b", question)
    return int(m.group(0)) if m else None


_RELATIVE_TIME_RE = re.compile(
    r"\b(?:(?:in|from|over)\s+)?(?:the\s+)?last\s+(\d+)\s+(day|week|month|year)s?\b", re.I)
_RELATIVE_NAMED_RE = re.compile(
    r"\b(?:(?:in|from|over)\s+)?(?:the\s+)?(this|last|past)\s+(week|month|quarter|year)\b", re.I)
_TODAY_RE = re.compile(r"\btoday\b", re.I)
_YESTERDAY_RE = re.compile(r"\byesterday\b", re.I)


def _extract_relative_time(question: str) -> Optional[str]:
    """Parse relative time expressions into a SQL-embeddable interval clause.
    Returns an expression like ``CURRENT_DATE - INTERVAL '7 days'`` or None."""
    m = _RELATIVE_TIME_RE.search(question)
    if m:
        n, unit = int(m.group(1)), m.group(2).lower()
        return f"CURRENT_DATE - INTERVAL '{n} {unit}s'"
    m = _RELATIVE_NAMED_RE.search(question)
    if m:
        _qual, unit = m.group(1).lower(), m.group(2).lower()
        mapping = {"week": "7 days", "month": "1 month", "quarter": "3 months", "year": "1 year"}
        return f"CURRENT_DATE - INTERVAL '{mapping[unit]}'"
    if _TODAY_RE.search(question):
        return "CURRENT_DATE"
    if _YESTERDAY_RE.search(question):
        return "CURRENT_DATE - INTERVAL '1 day'"
    return None


def _year_clause(date_column: str, year: int, columns: List[dict]) -> str:
    """Year filter. Integer 'year'-style columns use equality; true dates use a
    half-open range so an index can be used."""
    dtype = ""
    for c in columns:
        if (c.get("column_name") or c.get("name") or "") == date_column:
            dtype = (c.get("data_type") or "").lower()
            break
    is_int_year = date_column.lower() == "year" or dtype in (
        "smallint", "integer", "int", "int2", "int4", "int8", "bigint", "numeric")
    if is_int_year:
        return f"{_qid(date_column)} = {int(year)}"
    return (f"{_qid(date_column)} >= '{int(year)}-01-01' "
            f"AND {_qid(date_column)} < '{int(year) + 1}-01-01'")


def coverage() -> List[Dict[str, Any]]:
    """What context is available to test against — one row per compiled DLM, with
    the date range, row count, and value coverage. Drives the homepage banner."""
    ensure_tables()
    res = meta.query(
        "SELECT dataset_id, manifest, stats_rollup, status, built_at FROM dlm_artifact "
        "ORDER BY built_at DESC", []
    )
    out: List[Dict[str, Any]] = []
    for r in res.get("rows_objects", res.get("rows", [])):
        did = str(r.get("dataset_id"))
        manifest = _loads(r.get("manifest")) or {}
        stats = _loads(r.get("stats_rollup")) or {}
        row_counts = stats.get("row_counts") or {}
        max_rows = max(row_counts.values()) if row_counts else None
        if max_rows is None:
            wm = stats.get("watermark") or {}
            max_rows = wm.get("row_count") or None
        cols = manifest.get("columns") or []
        samples = _sample_values(did)
        out.append({
            "dataset_id": did,
            "name": manifest.get("name"),
            "date_column": manifest.get("date_column"),
            "date_range": stats.get("date_range"),
            "row_count": max_rows,
            "values_indexed": _value_count(did),
            "columns_count": len(cols),
            "dimensions": [
                {"column": c.get("name"), "values": samples.get(c.get("name"), [])}
                for c in cols if c.get("is_dimension")
            ],
            "metrics": [m.get("name") for m in (manifest.get("metrics") or []) if m.get("name")],
            "status": r.get("status"),
            "built_at": r.get("built_at"),
        })
    return out


def _sample_values(dataset_id: str, per_col: int = 6) -> Dict[str, List[str]]:
    """A few example indexed values per dimension column — for the banner hover."""
    res = meta.query(
        "SELECT element_key, value_text, freq FROM dlm_value_index "
        "WHERE dataset_id = @param0 ORDER BY freq DESC", [dataset_id])
    out: Dict[str, List[str]] = {}
    for r in res.get("rows_objects", res.get("rows", [])):
        if not isinstance(r, dict):
            continue
        col = (r.get("element_key") or "").split(".")[-1]
        v = r.get("value_text")
        if not col or v is None:
            continue
        lst = out.setdefault(col, [])
        if len(lst) < per_col and v not in lst:
            lst.append(v)
    return out


def _usage_rollup(schema: str, columns: List[dict], dimensions: List[dict],
                  snapshots: Dict[str, dict]) -> Dict[str, Any]:
    per_table: Dict[str, int] = {}
    tables = {(c.get("table_name") or "").strip() for c in columns if c.get("table_name")}
    for tbl in tables:
        snap = snapshots.get(profiler.table_key(schema, tbl))
        if snap and snap.get("usage_count") is not None:
            per_table[tbl] = int(snap.get("usage_count") or 0)
    return {"per_table": per_table}


# --------------------------------------------------------------------------- #
# persistence                                                                  #
# --------------------------------------------------------------------------- #


def _persist_value_index(dataset_id: str, rows: List[Dict[str, Any]]) -> None:
    meta.execute("DELETE FROM dlm_value_index WHERE dataset_id = @param0", [dataset_id])
    for r in rows:
        try:
            meta.execute(
                "INSERT INTO dlm_value_index (id, dataset_id, element_key, value_text, "
                "value_norm, key_column, key_value, freq, source) VALUES "
                "(@param0,@param1,@param2,@param3,@param4,@param5,@param6,@param7,@param8)",
                [r["id"], r["dataset_id"], r["element_key"], r["value_text"],
                 r["value_norm"], r.get("key_column"), r.get("key_value"),
                 r.get("freq", 0.0), r.get("source")],
            )
        except Exception:
            # duplicate normalized value within a column, etc. — skip, don't fail the build
            continue


def _upsert_artifact(dataset_id: str, manifest: dict, stats_rollup: dict,
                     usage_rollup: dict, source_hash: str, status: str) -> None:
    now = _now_iso()
    existing = meta.query_one(
        "SELECT version FROM dlm_artifact WHERE dataset_id = @param0", [dataset_id]
    )
    m, s, u = json.dumps(manifest, default=str), json.dumps(stats_rollup, default=str), json.dumps(usage_rollup, default=str)
    if existing:
        meta.execute(
            "UPDATE dlm_artifact SET version = version + 1, manifest = @param0, "
            "stats_rollup = @param1, usage_rollup = @param2, source_hash = @param3, "
            "built_at = @param4, status = @param5 WHERE dataset_id = @param6",
            [m, s, u, source_hash, now, status, dataset_id],
        )
    else:
        meta.execute(
            "INSERT INTO dlm_artifact (dataset_id, version, manifest, stats_rollup, "
            "usage_rollup, source_hash, built_at, status) VALUES "
            "(@param0,1,@param1,@param2,@param3,@param4,@param5,@param6)",
            [dataset_id, m, s, u, source_hash, now, status],
        )


def _upsert_router(dataset_id: str, ds: dict, columns: List[dict],
                   metrics: List[dict], value_rows: List[Dict[str, Any]]) -> None:
    name = ds.get("dataset_name") or ds.get("name") or ""
    desc = ds.get("description") or ""
    col_names = [c.get("column_name") or c.get("name") or "" for c in columns]
    met_names = [m.get("name") or m.get("metric_name") or "" for m in metrics]
    top_values = [r["value_text"] for r in sorted(value_rows, key=lambda x: x.get("freq", 0), reverse=True)[:40]]
    summary = f"{name}. {desc}".strip()
    terms = sorted({
        t for src in [name, desc, *col_names, *met_names, *top_values]
        for t in _tokenize(src)
    })
    now = _now_iso()
    existing = meta.query_one("SELECT dataset_id FROM dlm_router WHERE dataset_id = @param0", [dataset_id])
    terms_json = json.dumps(terms)
    if existing:
        meta.execute(
            "UPDATE dlm_router SET summary = @param0, terms = @param1, updated_at = @param2 "
            "WHERE dataset_id = @param3",
            [summary, terms_json, now, dataset_id],
        )
    else:
        meta.execute(
            "INSERT INTO dlm_router (dataset_id, summary, terms, updated_at) VALUES "
            "(@param0,@param1,@param2,@param3)",
            [dataset_id, summary, terms_json, now],
        )


# --------------------------------------------------------------------------- #
# small helpers                                                                #
# --------------------------------------------------------------------------- #


def _metadata_database() -> str:
    try:
        from config import settings
        return getattr(settings, "METADATA_DATABASE", "") or ""
    except Exception:
        import os
        return os.environ.get("METADATA_DATABASE", "")


def _estimate_distinct(n_distinct: Any, row_count: Any) -> Optional[float]:
    """Convert a pg_stats n_distinct into an absolute distinct-count estimate.
    Postgres encodes it two ways: a positive value is the estimated count; a
    negative value is the count as a *fraction of rows* (e.g. -0.2 => 20% of
    rows, -1 => every row unique). Returns None when unknown."""
    if not isinstance(n_distinct, (int, float)):
        return None
    if n_distinct >= 0:
        return float(n_distinct)
    rows = 0
    try:
        rows = float(row_count or 0)
    except Exception:
        rows = 0
    return abs(n_distinct) * rows


def _analyze_tables(database: str, schema: str, tables: List[str]) -> None:
    """Best-effort ANALYZE so pg_stats is complete before we read it — freshly
    loaded tables may not be auto-analyzed yet. Cheap (sampled), Postgres-only,
    and never fatal to the build."""
    import database.pool as pool
    for tbl in tables:
        t = (tbl or "").strip()
        if not t:
            continue
        sfx = f'"{schema}"."{t}"' if schema else f'"{t}"'
        try:
            pool.execute_query(f"ANALYZE {sfx}", database)
        except Exception:
            continue


def _dataset_tables(columns: List[dict], dimensions: List[dict], fact_table: Optional[str]) -> List[str]:
    tables = set()
    if fact_table:
        tables.add(fact_table.strip())
    for c in columns:
        t = (c.get("table_name") or "").strip()
        if t:
            tables.add(t)
    for d in dimensions:
        t = (d.get("dimension_table") or d.get("table_name") or "").strip()
        if t:
            tables.add(t)
    return sorted(tables)


def _dim_key_columns(dimensions: List[dict]) -> Dict[str, Optional[str]]:
    """Map a dimension's table -> its join/surrogate key column, so the value
    index can filter on the key rather than the label when they differ."""
    out: Dict[str, Optional[str]] = {}
    for d in dimensions:
        tbl = (d.get("dimension_table") or d.get("table_name") or "").strip()
        if tbl:
            out[tbl] = d.get("join_key") or d.get("fact_key")
    return out


def _synonyms_for(name: str) -> List[str]:
    n = (name or "").lower().strip()
    if not n:
        return []
    tokens = set(n.split())
    for head, syns in _SEED_SYNONYMS.items():
        if head in tokens or n in syns:
            return sorted(set([head, *syns]) - {n})
    return []


# --------------------------------------------------------------------------- #
# per-dataset context spec — the curatable "model context"                     #
#   suggested  (auto, in manifest)  ⊕  curation (human overrides, own column)   #
#   = effective spec used by the matchers, precompute, and the editor UI.       #
# --------------------------------------------------------------------------- #

_SPEC_CACHE: Dict[str, dict] = {}


def _suggest_spec(columns: List[dict], metrics: List[dict]) -> dict:
    """Auto-derive a starter context spec at generate time: aliases from the seed
    lexicon, additivity inferred from the aggregate, every dimension precomputed
    by default. The user edits this on the context page; edits persist separately."""
    ms: Dict[str, dict] = {}
    for i, m in enumerate(metrics):
        name = m.get("name") or m.get("metric_name")
        if not name:
            continue
        expr = (m.get("expression") or "")
        # non-additive: distinct counts and averages can't be summed across a breakdown
        additive = not (re.search(r"\bDISTINCT\b", expr, re.I) or re.search(r"\bAVG\s*\(", expr, re.I))
        ms[name] = {
            "display_name": name,
            "aliases": _synonyms_for(name),
            "additive": additive,
            "default": (i == 0),
        }
    ds: Dict[str, dict] = {}
    for c in columns:
        if not c.get("is_dimension"):
            continue
        col = (c.get("column_name") or c.get("name") or "").strip()
        if not col:
            continue
        ds[col] = {
            "display_name": c.get("display_name") or col,
            "aliases": _synonyms_for(col),
            "precompute": True,
            "top_n": 500,
        }
    return {"metrics": ms, "dimensions": ds, "value_aliases": {}}


def _merge_spec(suggested: dict, curation: dict) -> dict:
    """Overlay human curation on the auto-suggested spec. Aliases union (users add,
    never silently lose a suggestion); scalar flags override; value_aliases merge."""
    out: Dict[str, Any] = {"metrics": {}, "dimensions": {}, "value_aliases": {}}
    for kind in ("metrics", "dimensions"):
        base = suggested.get(kind) or {}
        over = curation.get(kind) or {}
        for key, val in base.items():
            entry = dict(val)
            o = over.get(key) or {}
            if "aliases" in o:
                # curated list is authoritative (WYSIWYG editor pre-fills from
                # effective, so this supports removing a suggested alias too)
                entry["aliases"] = sorted(set(o.get("aliases") or []))
            for f in ("display_name", "additive", "default", "precompute", "top_n", "hidden"):
                if f in o:
                    entry[f] = o[f]
            out[kind][key] = entry
    va = dict(suggested.get("value_aliases") or {})
    va.update(curation.get("value_aliases") or {})
    out["value_aliases"] = va
    # default metric: explicit curation wins, else the one flagged default, else first
    dflt = curation.get("default_metric")
    if not dflt:
        for k, v in out["metrics"].items():
            if v.get("default"):
                dflt = k
                break
    out["default_metric"] = dflt or (next(iter(out["metrics"]), None))
    return out


def _effective_spec(dataset_id: str) -> dict:
    """Suggested ⊕ curation for a dataset, cached in-memory (invalidated on regen
    and on save). Empty spec when no artifact exists."""
    if dataset_id in _SPEC_CACHE:
        return _SPEC_CACHE[dataset_id]
    ensure_tables()
    row = meta.query_one(
        "SELECT manifest, curation FROM dlm_artifact WHERE dataset_id = @param0", [dataset_id]) or {}
    manifest = _loads(row.get("manifest")) or {}
    suggested = manifest.get("context_spec") or {}
    curation = _loads(row.get("curation")) or {}
    eff = _merge_spec(suggested, curation)
    _SPEC_CACHE[dataset_id] = eff
    return eff


def _alias_index(spec: dict, kind: str) -> Dict[str, List[str]]:
    """normalized name -> curated aliases, for the ask-time matchers."""
    idx: Dict[str, List[str]] = {}
    for name, v in (spec.get(kind) or {}).items():
        al = v.get("aliases") or []
        if al:
            idx[_normalize(name)] = al
    return idx


def _syn(name: str, extra: Optional[Dict[str, List[str]]] = None) -> List[str]:
    """Global seed synonyms for a column/metric name, plus any per-dataset curated
    aliases for it. This is what makes the lexicon dataset-specific."""
    base = set(_synonyms_for(name))
    if extra:
        base |= set(extra.get(_normalize(name), []))
    return list(base)


def get_context_spec(dataset_id: str) -> Dict[str, Any]:
    """Effective context spec (suggested ⊕ curation) for the curation editor, with
    the raw pieces so the UI can show default vs edited and offer a reset."""
    ensure_tables()
    row = meta.query_one(
        "SELECT manifest, curation, status, built_at FROM dlm_artifact WHERE dataset_id = @param0",
        [str(dataset_id)])
    if not row:
        return {"ok": False, "reason": "no_artifact", "dataset_id": str(dataset_id)}
    manifest = _loads(row.get("manifest")) or {}
    suggested = manifest.get("context_spec") or {}
    curation = _loads(row.get("curation")) or {}
    return {
        "ok": True,
        "dataset_id": str(dataset_id),
        "dataset_name": manifest.get("name"),
        "status": row.get("status"),
        "built_at": row.get("built_at"),
        "suggested": suggested,
        "curation": curation,
        "effective": _merge_spec(suggested, curation),
    }


def save_curation(dataset_id: str, curation: Any) -> Dict[str, Any]:
    """Persist human curation overrides. Alias / display / default / value-alias edits
    take effect immediately (caches invalidated). Breakdown / depth edits change what
    is precomputed, so those need a regenerate — flagged as ``needs_regenerate``."""
    ensure_tables()
    row = meta.query_one(
        "SELECT curation FROM dlm_artifact WHERE dataset_id = @param0", [str(dataset_id)])
    if not row:
        return {"ok": False, "reason": "no_artifact"}
    clean = _sanitize_curation(curation)
    prev = _loads(row.get("curation")) or {}
    meta.execute("UPDATE dlm_artifact SET curation = @param0 WHERE dataset_id = @param1",
                 [json.dumps(clean, default=str), str(dataset_id)])
    _SPEC_CACHE.pop(str(dataset_id), None)
    _ANSWER_CACHE.pop(str(dataset_id), None)
    _SKETCH_CACHE.pop(str(dataset_id), None)
    return {"ok": True, "dataset_id": str(dataset_id),
            "needs_regenerate": _curation_affects_precompute(prev, clean),
            "effective": _effective_spec(str(dataset_id))}


def _sanitize_curation(c: Any) -> Dict[str, Any]:
    """Whitelist + coerce a curation payload so only known shapes are persisted."""
    c = c if isinstance(c, dict) else {}
    out: Dict[str, Any] = {}
    for kind in ("metrics", "dimensions"):
        src = c.get(kind)
        if not isinstance(src, dict):
            continue
        d: Dict[str, Any] = {}
        for key, v in src.items():
            if not isinstance(v, dict):
                continue
            e: Dict[str, Any] = {}
            if isinstance(v.get("aliases"), list):
                e["aliases"] = [str(a).strip().lower() for a in v["aliases"] if str(a).strip()][:50]
            if v.get("display_name") is not None:
                e["display_name"] = str(v["display_name"])[:120]
            for f in ("additive", "default", "precompute", "hidden"):
                if f in v:
                    e[f] = bool(v[f])
            if "top_n" in v:
                try:
                    e["top_n"] = max(1, min(5000, int(v["top_n"])))
                except Exception:
                    pass
            if e:
                d[str(key)] = e
        if d:
            out[kind] = d
    va = c.get("value_aliases")
    if isinstance(va, dict):
        cleaned = {str(k).strip().lower(): str(val) for k, val in va.items()
                   if str(k).strip() and val is not None}
        if cleaned:
            out["value_aliases"] = cleaned
    if c.get("default_metric"):
        out["default_metric"] = str(c["default_metric"])
    return out


def _curation_affects_precompute(prev: dict, new: dict) -> bool:
    """True when a dimension's precompute/depth/hidden changed — the only edits that
    require re-running the (scanning) precompute step."""
    def _shape(spec):
        return {k: (v.get("precompute"), v.get("top_n"), v.get("hidden"))
                for k, v in ((spec or {}).get("dimensions") or {}).items()}
    return _shape(prev) != _shape(new)


def _fingerprint(ds: dict, columns: List[dict], dimensions: List[dict], metrics: List[dict]) -> str:
    payload = {
        "name": ds.get("dataset_name") or ds.get("name"),
        "fact": ds.get("table_name") or ds.get("fact_table"),
        "schema": ds.get("schema_name"),
        "date_column": ds.get("date_column"),
        "columns": sorted([(c.get("table_name"), c.get("column_name") or c.get("name"),
                            c.get("data_type"), bool(c.get("is_dimension")), bool(c.get("is_metric")))
                           for c in columns]),
        "dimensions": sorted([(d.get("dimension_table") or d.get("table_name"),
                               d.get("join_key"), d.get("fact_key")) for d in dimensions]),
        "metrics": sorted([(m.get("name") or m.get("metric_name"), m.get("expression"),
                            m.get("metric_type")) for m in metrics]),
    }
    blob = json.dumps(payload, sort_keys=True, default=str)
    return hashlib.sha256(blob.encode("utf-8")).hexdigest()


def _value_hit(r: dict) -> Dict[str, Any]:
    ek = r.get("element_key") or ""
    parts = ek.split(".")
    column = parts[-1] if parts else None
    return {
        "element_key": ek,
        "column": column,
        "value": r.get("value_text"),
        "key_column": r.get("key_column") or column,
        "key_value": r.get("key_value"),
        "freq": r.get("freq"),
    }


def _value_count(dataset_id: str) -> int:
    row = meta.query_one(
        "SELECT COUNT(*) AS n FROM dlm_value_index WHERE dataset_id = @param0", [dataset_id]
    )
    try:
        return int((row or {}).get("n") or 0)
    except Exception:
        return 0


def _normalize(s: Any) -> str:
    if s is None:
        return ""
    t = unicodedata.normalize("NFKD", str(s))
    t = "".join(ch for ch in t if not unicodedata.combining(ch))
    return t.lower().strip()


_TOKEN_RE = re.compile(r"[a-z0-9]+")


def _tokenize(s: Any) -> List[str]:
    return [t for t in _TOKEN_RE.findall(_normalize(s)) if len(t) > 1]


def _stem(word: str) -> str:
    """Lightweight suffix stripper — no dependencies. Covers the common English
    inflections that cause misses in token matching (exports→export, selling→sell)."""
    w = word.lower()
    if len(w) <= 3:
        return w
    if w.endswith("ies") and len(w) > 4:
        return w[:-3] + "y"
    if w.endswith("tion") or w.endswith("sion"):
        return w[:-3] + "e" if len(w) > 6 else w[:-4]
    for suffix in ("ment", "ness", "able", "ible", "ful", "less", "ous", "ive",
                    "ity", "ing", "ation"):
        if w.endswith(suffix) and len(w) - len(suffix) >= 3:
            root = w[:-len(suffix)]
            if suffix == "ing" and root.endswith(root[-1]) and len(root) > 2:
                root = root[:-1]  # selling -> sell
            return root
    if w.endswith("ed") and len(w) > 4:
        return w[:-2] if not w[-3] == w[-4] else w[:-3]
    if w.endswith("er") and len(w) > 4:
        return w[:-2]
    if w.endswith("es") and len(w) > 4:
        return w[:-2]
    if w.endswith("s") and not w.endswith("ss") and len(w) > 3:
        return w[:-1]
    return w


def _expand_tokens(tokens: set) -> set:
    """Expand a token set with stems and seed synonyms for broader matching."""
    expanded = set(tokens)
    for t in tokens:
        st = _stem(t)
        expanded.add(st)
        syns = _synonyms_for(t)
        if not syns:
            syns = _synonyms_for(st)
        for s in syns:
            expanded.update(_tokenize(s))
    return expanded


def _loads(v: Any) -> Any:
    if v is None:
        return None
    if isinstance(v, (dict, list)):
        return v
    try:
        return json.loads(v)
    except Exception:
        return None


def _parse_pg_array(raw: Any) -> List[str]:
    """Parse a Postgres text-array literal like ``{Anthropic,OpenAI,"Big, Co"}``
    into a Python list. Handles double-quoted elements containing commas."""
    if raw is None:
        return []
    s = str(raw).strip()
    if not s or s in ("{}", "[]"):
        return []
    if s.startswith("{") and s.endswith("}"):
        s = s[1:-1]
    out: List[str] = []
    for m in re.finditer(r'"((?:[^"\\]|\\.)*)"|([^,]+)', s):
        if m.group(1) is not None:
            out.append(m.group(1).replace('\\"', '"').replace("\\\\", "\\"))
        else:
            val = m.group(2).strip()
            if val:
                out.append(val)
    return out


def _parse_pg_floats(raw: Any) -> List[float]:
    if raw is None:
        return []
    s = str(raw).strip().strip("{}[]")
    if not s:
        return []
    out: List[float] = []
    for part in s.split(","):
        try:
            out.append(float(part.strip()))
        except Exception:
            out.append(0.0)
    return out
