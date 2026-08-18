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
import re
import unicodedata
import uuid
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional

import database.metadata as meta
import services.context_profiler as profiler
import services.datasets as datasets_svc

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
    "revenue": ["sales", "turnover", "top line", "income"],
    "cost": ["spend", "expense", "expenditure"],
    "profit": ["margin", "earnings", "net income", "bottom line"],
    "count": ["number", "total", "volume", "qty", "quantity"],
    "customer": ["client", "account", "user", "buyer"],
    "provider": ["vendor", "supplier", "publisher", "maker"],
    "model": ["variant", "version", "sku"],
    "region": ["geography", "geo", "area", "territory"],
    "date": ["day", "time", "period", "when"],
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
    source_hash    TEXT NOT NULL,          -- schema+snapshot fingerprint for change-detection
    built_at       TEXT NOT NULL,
    status         TEXT NOT NULL DEFAULT 'ready'   -- building | ready | stale | error | unsupported
);

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

    return {
        "ok": True,
        "dataset_id": dataset_id,
        "status": "ready" if stats_supported else "unsupported",
        "rebuilt": True,
        "stats_supported": stats_supported,
        "columns": len(columns),
        "values_indexed": len(value_rows),
        "built_at": _now_iso(),
        "method": "manifest + pg_stats value inventory + query_history usage (no LLM, no data scan)",
    }


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


def route(question: str, limit: int = 3) -> List[Dict[str, Any]]:
    """CLM-over-CLMs (weighted lexical): rank datasets by how strongly the
    question matches each dataset's metrics (x3), indexed values (x2), name (x2)
    and columns (x1). A floor prevents a single generic word from routing (which
    sent 'USA consumption' to the AI leaderboard). Discrete-code routing at v2."""
    ensure_tables()
    q_tokens = set(_tokenize(question))
    if not q_tokens:
        return []

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

        m_hit = q_tokens & metric_toks
        n_hit = q_tokens & name_toks
        c_hit = q_tokens & col_toks
        v_hit = value_hits.get(did, 0)
        score = 3 * len(m_hit) + 2 * v_hit + 2 * len(n_hit) + 1 * len(c_hit)
        if score >= _ROUTE_FLOOR:
            ranked.append({
                "dataset_id": did,
                "score": score,
                "matched": sorted(m_hit | n_hit | c_hit),
                "value_matches": v_hit,
                "summary": manifest.get("name"),
            })
    ranked.sort(key=lambda x: x["score"], reverse=True)
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
    """Deterministic NL -> SQL via the DLM — no LLM. Routes to a dataset, resolves
    entity filters from the value index, matches a metric (by name/expression/
    synonyms), detects a group-by and a year filter, and assembles SQL against
    the dataset's fact table. This is the homepage's fallback when the in-browser
    template parser can't match a question."""
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

    qset = set(_tokenize(question))

    # 1) entity filters from the value index (e.g. "india" -> country='India')
    filters = _resolve_entity_filters(dataset_id, question)

    # 2) metric — match by name/expression/synonym tokens, else first metric
    metric = _match_metric(qset, metrics)

    # 3) group-by dimension ("... by country")
    group_col = _match_group_by(question, dims)

    # 3b) top-N ("top 10 countries by consumption") — sets the row limit and, if
    #     no explicit "by <dim>", groups by the dimension named in the question.
    top_n = None
    mtop = re.search(r"\btop\s+(\d+)\b", question, re.I)
    if mtop:
        top_n = int(mtop.group(1))
        if not group_col:
            group_col = _match_any_dim(question, dims)
    limit_n = top_n or limit

    # 4) year filter
    year = _extract_year(question)

    # 4b) if the requested year is beyond where this metric actually has data
    #     (for these entity filters), answer with the latest available year and
    #     say so, instead of returning an empty result for a future/missing year.
    note = None
    metric_name = (metric or {}).get("name") or "Count"
    if year and metric:
        latest = _metric_latest_year(database, schema, fact, date_column, metric, columns, filters)
        if latest is not None and year > latest:
            note = f"No data for {year} yet — showing the latest available ({latest}) for {metric_name}."
            year = latest

    # ── assemble ──────────────────────────────────────────────────────────────
    metric_expr = (metric or {}).get("expression") or "COUNT(*)"

    select_parts: List[str] = []
    if group_col:
        select_parts.append(_qid(group_col))
    select_parts.append(f"{metric_expr} AS {_qid(metric_name)}")

    where: List[str] = []
    for f in filters:
        where.append(f"{_qid(f['column'])} = '{str(f['value']).replace(chr(39), chr(39) * 2)}'")
    if year and date_column:
        where.append(_year_clause(date_column, year, columns))

    sql = f"SELECT {', '.join(select_parts)} FROM {_qid(schema)}.{_qid(fact)}"
    if where:
        sql += " WHERE " + " AND ".join(where)
    if group_col:
        sql += f" GROUP BY {_qid(group_col)} ORDER BY {_qid(metric_name)} DESC LIMIT {int(limit_n)}"

    title = metric_name
    if group_col:
        title += f" by {group_col}"
    ctx_bits = [f["value"] for f in filters] + ([str(year)] if year else [])
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
        "chartType": "bar" if group_col else "kpi",
        "xAxis": group_col,
        "yAxis": metric_name,
        "title": title,
        "columns": ([group_col] if group_col else []) + [metric_name],
        "filters": filters,
        "year": year,
        "note": note,
        "confidence": round(routed[0].get("score", 0.0), 3),
    }


def _metric_latest_year(database: str, schema: str, table: str, date_column: Optional[str],
                        metric: dict, columns: List[dict], filters: List[Dict[str, Any]]) -> Optional[int]:
    """Latest year for which *metric* actually has data under the given entity
    filters — e.g. India's Total Energy ends 2024 even though the table's 'year'
    column reaches 2025. Returns None when it can't be determined (COUNT(*),
    no date column, query error)."""
    if not date_column:
        return None
    mcols = _metric_columns([metric], columns, date_column)
    if not mcols:
        return None
    conds = [f"{_qid(c)} IS NOT NULL" for c in mcols]
    for f in filters:
        conds.append(f"{_qid(f['column'])} = '{str(f['value']).replace(chr(39), chr(39) * 2)}'")
    import database.pool as pool
    tbl = f"{_qid(schema)}.{_qid(table)}" if schema else _qid(table)
    try:
        res = pool.execute_query(
            f"SELECT MAX({_qid(date_column)}) AS mx FROM {tbl} WHERE {' AND '.join(conds)}", database)
    except Exception:
        return None
    rows = res.get("rows") or res.get("rows_objects") or []
    if not rows:
        return None
    mx = rows[0].get("mx") if isinstance(rows[0], dict) else rows[0][0]
    if mx is None:
        return None
    try:
        return int(mx) if isinstance(mx, int) else int(str(mx)[:4])
    except Exception:
        return None


def _resolve_entity_filters(dataset_id: str, question: str) -> List[Dict[str, Any]]:
    """Resolve value phrases in the question to (column, value) filters via the
    value index. Longest n-grams first so "United States" wins over "United".
    One filter per column."""
    words = re.findall(r"[A-Za-z0-9][A-Za-z0-9&.\-]*", question)
    out: List[Dict[str, Any]] = []
    used_cols: set = set()
    used_spans: set = set()
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
            hits = resolve_value(dataset_id, phrase, limit=1, exact_only=True)
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


def _match_metric(qset: set, metrics: List[dict]) -> Optional[dict]:
    """Pick the metric whose name/expression/synonym tokens best overlap the
    question. Falls back to the first metric when nothing matches."""
    if not metrics:
        return None
    best, best_score = None, 0
    for m in metrics:
        name = m.get("name") or m.get("metric_name") or ""
        expr = m.get("expression") or ""
        toks = set(_tokenize(name)) | set(_tokenize(expr)) | set(_synonyms_for(name))
        toks.discard("sum"); toks.discard("avg"); toks.discard("count")
        score = len(qset & toks)
        if score > best_score:
            best, best_score = m, score
    return best or metrics[0]


def _match_group_by(question: str, dims: List[dict]) -> Optional[str]:
    """Detect a 'by <dimension>' / 'per <dimension>' grouping and map it to a
    dimension column."""
    m = re.search(r"\b(?:by|per|across|for each)\s+([A-Za-z][A-Za-z ]*)", question, re.I)
    if not m:
        return None
    target = _tokenize(m.group(1))
    if not target:
        return None
    for d in dims:
        col = d.get("column_name") or d.get("name") or ""
        ctoks = set(_tokenize(col)) | set(_synonyms_for(col))
        if set(target) & ctoks:
            return col
    return None


def _match_any_dim(question: str, dims: List[dict]) -> Optional[str]:
    """Find a dimension named anywhere in the question (handles plurals/typos via
    a 4-char prefix match, e.g. 'countries'/'counties' -> country)."""
    qt = _tokenize(question)
    for d in dims:
        col = (d.get("column_name") or d.get("name") or "").strip()
        if not col:
            continue
        c4 = _normalize(col)[:4]
        if c4 and len(c4) >= 4 and any(_normalize(t)[:4] == c4 for t in qt):
            return col
        if set(_synonyms_for(col)) & set(qt):
            return col
    return None


def _extract_year(question: str) -> Optional[int]:
    m = re.search(r"\b(19|20)\d{2}\b", question)
    return int(m.group(0)) if m else None


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
        manifest = _loads(r.get("manifest")) or {}
        stats = _loads(r.get("stats_rollup")) or {}
        row_counts = stats.get("row_counts") or {}
        max_rows = max(row_counts.values()) if row_counts else None
        out.append({
            "dataset_id": r.get("dataset_id"),
            "name": manifest.get("name"),
            "date_column": manifest.get("date_column"),
            "date_range": stats.get("date_range"),
            "row_count": max_rows,
            "values_indexed": _value_count(str(r.get("dataset_id"))),
            "metrics": [m.get("name") for m in (manifest.get("metrics") or []) if m.get("name")],
            "status": r.get("status"),
            "built_at": r.get("built_at"),
        })
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
    for head, syns in _SEED_SYNONYMS.items():
        if head in n or n in syns:
            return sorted(set([head, *syns]) - {n})
    return []


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
