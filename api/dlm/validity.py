"""
Validity (staleness) scoring — patent claim element 2.

Every context element (a table, or a single column) carries a **validity score**
in [0, 1], where 1 means "trust the cached context, no query needed" and 0 means
"the cached context is worthless, go to the live database". The score is a
composite of three factors, all computed from cheap metadata — never by
re-querying the underlying data:

  (a) TIME       — elapsed time since the element was last refreshed, run through
                   an exponential half-life decay.

  (b) CHANGE     — magnitude of data change since capture, measured as the number
                   of row modifications reported by the database's own change
                   counter (``n_mod_since_analyze`` delta, or a shift in
                   ``last_analyze``) divided by the row count at capture. This is
                   the load-bearing trick: we detect drift from a counter, not
                   from a scan, so measuring staleness costs ~nothing.

  (c) USAGE      — a decay weight tied to how frequently the element has been
                   relied upon for prior answers. Hot elements decay FASTER
                   (shorter effective half-life) so that the answers users lean
                   on most are kept the freshest. This is the disambiguation the
                   claim needs: usage shortens half-life, it does not lengthen it.

Composite: the time and change factors are combined multiplicatively (either one
going to zero should invalidate the element), and usage modulates the time
factor's half-life. The result is per-element, so the router can trust part of
the context and refresh only the stale part (see context_router.hybrid path).
"""

from __future__ import annotations

import math
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional

import database.pool as pool

# ---- tunables -------------------------------------------------------------- #
# Base half-life for the time factor. After this many seconds an *unused,
# unchanged* element's time-freshness has fallen to 0.5.
BASE_HALF_LIFE_SECONDS = 6 * 3600.0          # 6 hours
# Change sensitivity: fraction of rows modified at which change-freshness = 0.5.
CHANGE_HALF_FRACTION = 0.05                   # 5% of rows churned => half stale
# How hard usage shortens the half-life. effective = base / (1 + USAGE_GAIN*log1p(usage))
USAGE_GAIN = 0.35
# Minimum effective half-life so a hyper-used element can't demand a refresh
# on literally every request.
MIN_HALF_LIFE_SECONDS = 10 * 60.0            # 10 minutes
# Default routing threshold: answer from context only when validity >= this.
DEFAULT_THRESHOLD = 0.7


def _now() -> datetime:
    return datetime.now(timezone.utc).replace(tzinfo=None)


def _parse_dt(v: Any) -> Optional[datetime]:
    if v is None:
        return None
    if isinstance(v, datetime):
        return v.replace(tzinfo=None)
    try:
        s = str(v).replace("Z", "").replace("T", " ")
        # tolerate fractional seconds / date-only
        for fmt in ("%Y-%m-%d %H:%M:%S.%f", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d"):
            try:
                return datetime.strptime(s[: len(fmt) + 6], fmt)
            except Exception:
                continue
    except Exception:
        return None
    return None


# --------------------------------------------------------------------------- #
# the three factors                                                           #
# --------------------------------------------------------------------------- #


def _effective_half_life(usage_count: int) -> float:
    """Factor (c): hotter elements get a shorter half-life so heavily-relied-upon
    context is kept fresher."""
    hl = BASE_HALF_LIFE_SECONDS / (1.0 + USAGE_GAIN * math.log1p(max(0, usage_count)))
    return max(MIN_HALF_LIFE_SECONDS, hl)


def _time_factor(age_seconds: float, half_life: float) -> float:
    """Factor (a): exponential decay by age, half_life-scaled."""
    if age_seconds <= 0:
        return 1.0
    return math.exp(-math.log(2.0) * age_seconds / max(1.0, half_life))


def _change_factor(rows_at_capture: int, mods_now: int, mods_at_capture: int,
                   last_analyze_changed: bool) -> float:
    """Factor (b): decay by fraction of rows modified since capture.
    ``mods_now``/``mods_at_capture`` come from the DB change counter; a bump in
    ``last_analyze`` (autovacuum re-analyzed) also implies the distribution moved."""
    delta = max(0, mods_now - mods_at_capture)
    if last_analyze_changed and delta == 0:
        # analyze ran but counter reset — treat as at least the half fraction
        delta = max(1, int(CHANGE_HALF_FRACTION * max(1, rows_at_capture)))
    frac = delta / float(max(1, rows_at_capture))
    return math.exp(-math.log(2.0) * frac / CHANGE_HALF_FRACTION)


# --------------------------------------------------------------------------- #
# scoring one element                                                         #
# --------------------------------------------------------------------------- #


def score_element(snapshot: Dict[str, Any], live: Optional[Dict[str, Any]]) -> Dict[str, Any]:
    """Score a single element from its stored *snapshot* and the *live* change
    stats read now (row_count / mods_since_analyze / last_analyze). Returns the
    composite score plus a per-factor breakdown for observability & the whitepaper."""
    now = _now()
    refreshed = _parse_dt(snapshot.get("refreshed_at")) or _parse_dt(snapshot.get("captured_at")) or now
    age = max(0.0, (now - refreshed).total_seconds())
    usage = _int(snapshot.get("usage_count"))

    half_life = _effective_half_life(usage)
    t = _time_factor(age, half_life)

    rows_cap = _int(snapshot.get("row_count"))
    mods_cap = _int(snapshot.get("mods_at_capture"))
    if live is not None:
        mods_now = _int(live.get("mods_since_analyze"))
        la_changed = _iso(live.get("last_analyze")) != (snapshot.get("last_analyze") or None)
        rows_ref = _int(live.get("row_count")) or rows_cap
        c = _change_factor(rows_ref, mods_now, mods_cap, la_changed)
        row_delta = abs(_int(live.get("row_count")) - rows_cap)
    else:
        c = 1.0            # no live signal available -> lean on time only
        row_delta = 0

    score = round(t * c, 4)
    return {
        "element_key": snapshot.get("element_key"),
        "score": score,
        "factors": {
            "time": round(t, 4),
            "change": round(c, 4),
            "age_seconds": round(age, 1),
            "effective_half_life_seconds": round(half_life, 1),
            "usage_count": usage,
            "row_delta": row_delta,
        },
    }


def score_elements(database: str, snapshots: Dict[str, Dict[str, Any]],
                   schema: str = "public") -> Dict[str, Dict[str, Any]]:
    """Score many elements at once. Reads live change stats for the relevant
    schemas in a single cheap catalog query, then scores each element."""
    live_by_schema: Dict[str, Dict[str, Dict[str, Any]]] = {}
    schemas = {k.split(".")[0] for k in snapshots.keys() if "." in k} or {schema}
    for sch in schemas:
        live_by_schema[sch] = _live_change(database, sch)

    out: Dict[str, Dict[str, Any]] = {}
    for key, snap in snapshots.items():
        parts = key.split(".")
        sch, tbl = (parts[0], parts[1]) if len(parts) >= 2 else (schema, parts[0])
        live = live_by_schema.get(sch, {}).get(tbl)
        out[key] = score_element(snap, live)
    return out


def _live_change(database: str, schema: str) -> Dict[str, Dict[str, Any]]:
    """Cheap live read of the change counters used by factor (b)."""
    try:
        sql = f"""
            SELECT relname AS table_name, n_live_tup, n_mod_since_analyze,
                   GREATEST(COALESCE(last_analyze,'epoch'),
                            COALESCE(last_autoanalyze,'epoch')) AS last_analyze
            FROM pg_stat_user_tables WHERE schemaname = '{schema.replace("'", "''")}'
        """
        res = pool.execute_query(sql, database)
    except Exception:
        return {}
    out: Dict[str, Dict[str, Any]] = {}
    for r in res.get("rows_objects", []):
        out[r["table_name"]] = {
            "row_count": _int(r.get("n_live_tup")),
            "mods_since_analyze": _int(r.get("n_mod_since_analyze")),
            "last_analyze": _iso(r.get("last_analyze")),
        }
    return out


# --------------------------------------------------------------------------- #
# routing decision from a set of relevant elements                            #
# --------------------------------------------------------------------------- #


def route_decision(scores: Dict[str, Dict[str, Any]],
                   threshold: float = DEFAULT_THRESHOLD) -> Dict[str, Any]:
    """Given validity scores for the elements a question depends on, decide the
    route (claim element 4). Per-element, not dataset-wide:

      - all relevant elements >= threshold  -> answer from context
      - some below threshold                -> hybrid: query only the stale ones
      - (no valid elements)                 -> full live query
    """
    if not scores:
        return {"route": "query", "stale_elements": [], "min_score": 0.0,
                "reason": "no context captured for this question"}

    stale = [k for k, s in scores.items() if s["score"] < threshold]
    fresh = [k for k, s in scores.items() if s["score"] >= threshold]
    min_score = min(s["score"] for s in scores.values())

    if not stale:
        route = "context"
        reason = f"all {len(fresh)} relevant elements valid (min {min_score:.2f} >= {threshold:.2f})"
    elif fresh:
        route = "hybrid"
        reason = f"{len(stale)} of {len(scores)} elements stale; query only those"
    else:
        route = "query"
        reason = f"all {len(scores)} relevant elements stale (min {min_score:.2f} < {threshold:.2f})"

    return {
        "route": route,
        "threshold": threshold,
        "min_score": round(min_score, 4),
        "stale_elements": stale,
        "fresh_elements": fresh,
        "reason": reason,
    }


def _int(v: Any) -> int:
    try:
        return int(v)
    except Exception:
        return 0


def _iso(v: Any) -> Optional[str]:
    if v is None:
        return None
    if isinstance(v, datetime):
        return v.isoformat()
    return str(v)
