"""
Adaptive context-based query router — the method's control loop (claim
elements 3-6).

Given a natural-language question and a target database:

  3. Receive the question and map it, deterministically (no LLM), to the
     *specific* context elements it depends on — the tables/columns whose names,
     aliases, or descriptions the question mentions.

  4. Score only those elements for validity (context_validity) and decide the
     route:
        context : answer from the stored context representation, no query
        hybrid  : some elements stale — refresh/query only those
        query   : elements too stale (or uncaptured) — go live

  5. When a live query runs, execute it, cache the result against the question
     signature with its element dependencies, and refresh the validity of the
     affected elements to a fresh state.

  6. When answering from context, return the answer with zero query execution —
     either from a previously-cached result whose dependencies are still valid,
     or synthesised directly from the statistical profile (row counts, distinct
     counts, null rates) with no query having ever run.

The router is the drop-in upgrade for the flat TTL result cache: instead of
"expire after N seconds", it expires per-element by measured data change.
"""

from __future__ import annotations

import hashlib
import json
import re
import time
import uuid
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional, Tuple

import database.metadata as meta
import database.pool as pool
import dlm.profiler as profiler
import dlm.validity as validity

_STOPWORDS = {
    "the", "a", "an", "of", "for", "to", "by", "in", "on", "and", "or", "me",
    "show", "get", "what", "whats", "is", "are", "how", "many", "much", "give",
    "list", "per", "each", "with", "count", "total", "sum", "average", "avg",
    "over", "time", "top", "vs", "versus", "all", "from", "group", "where",
}


def _now_iso() -> str:
    return datetime.now(timezone.utc).replace(tzinfo=None).isoformat()


# --------------------------------------------------------------------------- #
# 3. question -> relevant context elements (deterministic mapping)            #
# --------------------------------------------------------------------------- #


def _tokens(question: str) -> List[str]:
    words = re.split(r"[^a-z0-9]+", question.lower())
    return [w for w in words if len(w) >= 3 and w not in _STOPWORDS]


def _norm(s: str) -> str:
    return re.sub(r"[^a-z0-9]+", " ", str(s).lower()).strip()


def map_question_to_elements(question: str,
                             snapshots: Dict[str, Dict[str, Any]]) -> List[str]:
    """Return the element_keys the question depends on. A table element is
    included when its name is mentioned OR any of its columns match; a column
    element is included when its name matches a question token."""
    toks = set(_tokens(question))
    if not toks:
        return list({k for k in snapshots if snapshots[k].get("element_type") == "table"})

    relevant: set = set()
    tables_hit: set = set()

    for key, snap in snapshots.items():
        etype = snap.get("element_type")
        name = _norm(snap.get("column_name") or snap.get("table_name") or "")
        name_words = set(name.split())
        # token match: overlap between question tokens and the element's name words,
        # plus substring containment for compound names (e.g. "cases" ~ "new_cases")
        matched = bool(toks & name_words) or any(
            t in name.replace(" ", "") or name.replace(" ", "") in t for t in toks
        )
        if matched:
            relevant.add(key)
            if etype == "column":
                tables_hit.add(f"{snap.get('schema_name')}.{snap.get('table_name')}")

    # pull in the table element for any table whose column matched
    for tk in tables_hit:
        if tk in snapshots:
            relevant.add(tk)

    return list(relevant)


# --------------------------------------------------------------------------- #
# 6. answer directly from the statistical profile (no query, ever)            #
# --------------------------------------------------------------------------- #


def _profile_answer(question: str, snapshots: Dict[str, Dict[str, Any]],
                    relevant: List[str]) -> Optional[Dict[str, Any]]:
    """Some questions are answerable from the profile alone — row counts,
    distinct counts, null rates. If one matches, return the answer with no query."""
    q = question.lower()
    cols = [snapshots[k] for k in relevant if snapshots[k].get("element_type") == "column"]
    tbls = [snapshots[k] for k in relevant if snapshots[k].get("element_type") == "table"]

    def _profile(snap) -> Dict[str, Any]:
        try:
            return json.loads(snap.get("profile") or "{}")
        except Exception:
            return {}

    # row count: "how many rows / records / total rows"
    if re.search(r"\b(how many|number of|count of)\b.*\b(rows?|records?|entries)\b", q) and tbls:
        t = tbls[0]
        return {"answer": {"metric": "row_count", "table": t.get("table_name"),
                           "value": _int(t.get("row_count"))},
                "explanation": "row count read from context profile (pg_stat_user_tables)",
                "source_elements": [t.get("element_key")]}

    # distinct count: "how many distinct/unique <col>"
    m = re.search(r"\b(how many|number of|count of)\s+(?:distinct|unique)\s+([a-z0-9_ ]+)", q)
    if m and cols:
        target = _norm(m.group(2))
        for c in cols:
            if _norm(c.get("column_name")) in target or target in _norm(c.get("column_name")):
                nd = _profile(c).get("n_distinct")
                val = _ndistinct_to_count(nd, _int(c.get("row_count")))
                if val is not None:
                    return {"answer": {"metric": "approx_distinct",
                                       "column": c.get("column_name"), "value": val,
                                       "approximate": True},
                            "explanation": "approximate distinct count from pg_stats.n_distinct "
                                           "(estimate, no scan)",
                            "source_elements": [c.get("element_key")]}

    # null rate: "null rate / missing / % null of <col>"
    if re.search(r"\b(null rate|missing|nulls?|% null|percent null|completeness)\b", q) and cols:
        for c in cols:
            if _norm(c.get("column_name")) in q.replace(" ", "") or any(
                t in _norm(c.get("column_name")) for t in _tokens(question)
            ):
                nf = _profile(c).get("null_frac")
                if nf is not None:
                    return {"answer": {"metric": "null_fraction",
                                       "column": c.get("column_name"),
                                       "value": round(float(nf), 4),
                                       "approximate": True},
                            "explanation": "null fraction from pg_stats.null_frac (approximate)",
                            "source_elements": [c.get("element_key")]}

    # most common value: "most common/popular/frequent <col>"
    mc = re.search(r"\b(most (?:common|popular|frequent))\s+([a-z0-9_ ]+)", q)
    if mc and cols:
        target = _norm(mc.group(2))
        for c in cols:
            if _norm(c.get("column_name")) in target or target in _norm(c.get("column_name")):
                mcv = _profile(c).get("most_common_vals")
                if mcv:
                    vals = mcv if isinstance(mcv, list) else [v.strip() for v in mcv.strip("{}").split(",")]
                    if vals:
                        return {"answer": {"metric": "most_common_value",
                                           "column": c.get("column_name"),
                                           "value": vals[0],
                                           "approximate": True},
                                "explanation": f"most common value from pg_stats.most_common_vals (approximate)",
                                "source_elements": [c.get("element_key")]}

    return None


# --------------------------------------------------------------------------- #
# result cache keyed by question signature + element dependencies             #
# --------------------------------------------------------------------------- #


def _question_sig(question: str) -> str:
    toks = sorted(set(_tokens(question)))
    return " ".join(toks) if toks else _norm(question)


def _cache_get(database: str, sig: str) -> Optional[Dict[str, Any]]:
    row = meta.query_one(
        "SELECT * FROM context_answer_cache WHERE database_name = @param0 AND question_sig = @param1",
        [database, sig],
    )
    return row or None


def _cache_put(database: str, sig: str, sql: str, result: Dict[str, Any],
               deps: List[str], validity_score: float) -> None:
    now = _now_iso()
    sql_hash = hashlib.sha256((sql or "").encode()).hexdigest()[:16]
    result_json = json.dumps(result, default=str)
    deps_json = json.dumps(deps)
    existing = meta.query_one(
        "SELECT id FROM context_answer_cache WHERE database_name = @param0 AND question_sig = @param1",
        [database, sig],
    )
    if existing:
        meta.execute(
            "UPDATE context_answer_cache SET sql_hash=@param0, result=@param1, element_deps=@param2, "
            "last_served_at=@param3, serve_count=serve_count+1, validity_at_serve=@param4 WHERE id=@param5",
            [sql_hash, result_json, deps_json, now, validity_score, existing["id"]],
        )
    else:
        meta.execute(
            "INSERT INTO context_answer_cache (id, database_name, question_sig, sql_hash, result, "
            "element_deps, created_at, last_served_at, serve_count, validity_at_serve) VALUES "
            "(@param0,@param1,@param2,@param3,@param4,@param5,@param6,@param7,@param8,@param9)",
            [str(uuid.uuid4()), database, sig, sql_hash, result_json, deps_json, now, now, 1, validity_score],
        )


# --------------------------------------------------------------------------- #
# the routing entrypoint                                                      #
# --------------------------------------------------------------------------- #


def ask(question: str, database: str, sql: Optional[str] = None,
        schema: str = "public", threshold: float = validity.DEFAULT_THRESHOLD) -> Dict[str, Any]:
    """Route a natural-language question. See module docstring for the contract."""
    t0 = time.monotonic()

    def _with_duration(resp: Dict[str, Any]) -> Dict[str, Any]:
        resp["duration_ms"] = round((time.monotonic() - t0) * 1000, 1)
        return resp

    if not profiler.is_supported():
        return _with_duration({"route": "query", "source": "live", "supported": False,
                "reason": "context engine requires a Postgres metadata store",
                "sql": sql, "result": _maybe_exec(sql, database) if sql else None})

    profiler.ensure_tables()
    snapshots = profiler.load_snapshots(database)
    if not snapshots:
        return _with_duration({"route": "query", "source": "live", "context_built": False,
                "reason": "no context representation built yet — call /context/build",
                "sql": sql, "result": _maybe_exec(sql, database) if sql else None})

    # 3. map question -> relevant elements
    relevant = map_question_to_elements(question, snapshots)
    rel_snaps = {k: snapshots[k] for k in relevant if k in snapshots}

    # 4. score those elements + decide route
    scores = validity.score_elements(database, rel_snaps, schema=schema)
    decision = validity.route_decision(scores, threshold=threshold)
    base = {
        "question": question,
        "database": database,
        "relevant_elements": relevant,
        "validity": scores,
        "decision": decision,
    }

    # 6a. answer purely from the profile when possible AND the profile is valid
    if decision["route"] == "context":
        pa = _profile_answer(question, snapshots, relevant)
        if pa is not None:
            return _with_duration({**base, "route": "context", "source": "profile",
                    "answer": pa["answer"], "explanation": pa["explanation"],
                    "executed_query": False})

        # 6b. serve a cached result whose dependencies are still valid
        sig = _question_sig(question)
        cached = _cache_get(database, sig)
        if cached and _deps_valid(cached, scores, threshold):
            _cache_put(database, sig, "", _load(cached.get("result")), _load(cached.get("element_deps")),
                       decision["min_score"])
            return _with_duration({**base, "route": "context", "source": "cache",
                    "result": _load(cached.get("result")), "executed_query": False,
                    "served_from": "context_answer_cache"})

        # context is valid but we have nothing cached and can't synthesise from
        # profile — fall through to a live query and populate the cache.

    # 5. live path (route == query/hybrid, or context-but-cold-cache)
    if not sql:
        return _with_duration({**base, "route": decision["route"], "source": "needs_sql",
                "executed_query": False,
                "reason": "route requires a live query; supply generated SQL as `sql`"})

    result = _maybe_exec(sql, database)
    is_hybrid = decision["route"] == "hybrid"
    to_refresh = decision.get("stale_elements", []) if is_hybrid else relevant
    refreshed = profiler.refresh_elements(database, to_refresh or relevant)
    _cache_put(database, _question_sig(question), sql, result, relevant, decision["min_score"])

    return _with_duration({**base, "route": decision["route"],
            "source": "live", "sql": sql, "result": result,
            "executed_query": True, "elements_refreshed": refreshed,
            "elements_from_context": decision.get("fresh_elements", []) if is_hybrid else [],
            "elements_queried": to_refresh if is_hybrid else relevant})


# --------------------------------------------------------------------------- #
# observability: full validity report for a database (no routing)             #
# --------------------------------------------------------------------------- #


def validity_report(database: str, schema: str = "public") -> Dict[str, Any]:
    if not profiler.is_supported():
        return {"supported": False}
    profiler.ensure_tables()
    snaps = profiler.load_snapshots(database)
    scores = validity.score_elements(database, snaps, schema=schema)
    tables = {k: v for k, v in scores.items() if snaps.get(k, {}).get("element_type") == "table"}
    avg = round(sum(s["score"] for s in scores.values()) / len(scores), 4) if scores else 0.0
    return {
        "supported": True,
        "database": database,
        "elements": len(scores),
        "tables": len(tables),
        "avg_validity": avg,
        "stale_below_threshold": [k for k, s in scores.items() if s["score"] < validity.DEFAULT_THRESHOLD],
        "scores": scores,
    }


# --------------------------------------------------------------------------- #
# helpers                                                                     #
# --------------------------------------------------------------------------- #


def _maybe_exec(sql: Optional[str], database: str) -> Optional[Dict[str, Any]]:
    if not sql:
        return None
    try:
        return pool.execute_query(sql, database)
    except Exception as e:  # surface, don't crash the router
        return {"error": str(e)}


def _deps_valid(cached: Dict[str, Any], scores: Dict[str, Dict[str, Any]], threshold: float) -> bool:
    deps = _load(cached.get("element_deps")) or []
    if not deps:
        return False
    for d in deps:
        s = scores.get(d)
        if s is None or s["score"] < threshold:
            return False
    return True


def _load(v: Any) -> Any:
    if v is None:
        return None
    if isinstance(v, (dict, list)):
        return v
    try:
        return json.loads(v)
    except Exception:
        return v


def _ndistinct_to_count(n_distinct: Any, row_count: int) -> Optional[int]:
    """pg_stats.n_distinct: >=0 is an absolute estimate; <0 is a negative ratio
    of the row count (e.g. -0.5 => distinct is 50% of rows)."""
    try:
        nd = float(n_distinct)
    except Exception:
        return None
    if nd >= 0:
        return int(round(nd))
    return int(round(abs(nd) * max(0, row_count)))


def _int(v: Any) -> int:
    try:
        return int(v)
    except Exception:
        return 0
