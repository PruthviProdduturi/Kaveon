"""
Chat API — /api/v1/chat

Natural-language questions → SQL → results. The deterministic NL→SQL
engine runs server-side so clients don't need the parser logic.

POST /api/v1/chat
  { "question": "Top 10 countries by carbon intensity" }
  → { "answer": "...", "sql": "...", "data": {...}, "route": "...", "duration_ms": ... }
"""

from typing import Optional, List
from fastapi import APIRouter, Depends
from pydantic import BaseModel
import time
import re
import os

from middleware.auth import require_user_context, UserContext
import database.metadata as db
import database.pool as pool

router = APIRouter()


# ── Types ──────────────────────────────────────────────────────────────────────

class ChatRequest(BaseModel):
    question: str
    database: Optional[str] = None
    dataset_id: Optional[int] = None


class ChatResponse(BaseModel):
    question: str
    answer: str
    sql: Optional[str] = None
    data: Optional[dict] = None
    chart_type: Optional[str] = None
    route: str = "direct"
    duration_ms: int = 0
    dataset_name: Optional[str] = None
    error: Optional[str] = None


# ── Column aliases (mirrors frontend nlToSql.ts) ──────────────────────────────

COLUMN_ALIASES = {
    "energy": ["primary_energy_consumption", "energy_per_capita", "electricity_generation"],
    "consumption": ["primary_energy_consumption", "fossil_fuel_consumption", "renewables_consumption"],
    "renewables": ["renewables_share_energy", "renewables_consumption", "renewables_electricity"],
    "carbon": ["carbon_intensity_elec", "greenhouse_gas_emissions"],
    "emissions": ["greenhouse_gas_emissions", "carbon_intensity_elec"],
    "temperature": ["temp_change_c", "avg_tc"],
    "warming": ["temp_change_c", "avg_tc"],
    "elo": ["arena_elo"],
    "score": ["arena_elo", "mmlu", "humaneval", "gsm8k"],
    "benchmark": ["mmlu", "humaneval", "gsm8k", "gpqa"],
    "performance": ["arena_elo", "mmlu", "humaneval"],
    "cost": ["input_cost", "output_cost"],
    "model": ["model_name", "model_a", "model_b"],
    "country": ["country"],
    "provider": ["provider"],
}

TYPO_MAP = {
    "cosumption": "consumption", "consumtion": "consumption", "enery": "energy",
    "contry": "country", "temperture": "temperature", "renewble": "renewable",
    "intesity": "intensity", "emisions": "emissions", "cabon": "carbon",
    "modle": "model", "benchmak": "benchmark",
}

NOT_ENTITIES = {
    "energy", "carbon", "total", "average", "show", "get", "what", "top", "consumption",
    "emissions", "temperature", "renewable", "renewables", "intensity", "fossil", "solar",
    "wind", "nuclear", "hydro", "electricity", "global", "trend", "distribution",
    "arena", "benchmark", "model", "models", "score", "pricing", "compare",
    "summary", "summarize", "list", "count", "how", "which", "where", "the",
    "much", "many", "does", "did", "about", "more", "than",
}

AGG_MAP = {
    "total": "SUM", "sum": "SUM", "count": "COUNT", "average": "AVG",
    "avg": "AVG", "mean": "AVG", "min": "MIN", "max": "MAX",
    "minimum": "MIN", "maximum": "MAX",
}


# ── Helpers ────────────────────────────────────────────────────────────────────

def _fix_typos(text: str) -> str:
    return " ".join(TYPO_MAP.get(w.lower(), w) for w in text.split())


def _find_dataset(question: str, dataset_id: Optional[int] = None):
    """Find the best matching dataset for the question."""
    if dataset_id:
        row = db.query_one("SELECT id, dataset_name, fact_table, schema_name, database_name FROM dbo.datasets WHERE id = @param0", [dataset_id])
        if row:
            cols = db.query("SELECT column_name, data_type, is_dimension, is_metric FROM dbo.dataset_columns WHERE dataset_id = @param0", [dataset_id])
            metrics = db.query("SELECT metric_name, expression, metric_type FROM dbo.dataset_metrics WHERE dataset_id = @param0", [dataset_id])
            return row, cols.get("rows", []), metrics.get("rows", [])
        return None, [], []

    # Score all datasets against the question
    q_lower = question.lower()
    q_words = [w for w in q_lower.split() if len(w) >= 3]

    result = db.query("SELECT id, dataset_name, fact_table, schema_name, database_name FROM dbo.datasets")
    best = None
    best_score = 0

    for ds in result.get("rows", []):
        score = 0
        name = (ds.get("dataset_name") or "").lower()
        name_words = [w for w in re.split(r"[\s_\-·:]+", name) if len(w) >= 3]

        for nw in name_words:
            for qw in q_words:
                if nw in qw or qw in nw:
                    score += 0.3

        # Check column names
        cols = db.query("SELECT column_name, data_type, is_dimension, is_metric FROM dbo.dataset_columns WHERE dataset_id = @param0", [ds["id"]])
        col_rows = cols.get("rows", [])
        for col in col_rows:
            cn = (col.get("column_name") or "").lower().replace("_", " ")
            for cw in cn.split():
                if len(cw) < 3:
                    continue
                for qw in q_words:
                    if cw in qw or qw in cw:
                        score += 0.15

        # Check aliases
        for qw in q_words:
            if qw in COLUMN_ALIASES:
                for alias in COLUMN_ALIASES[qw]:
                    for col in col_rows:
                        if alias == col.get("column_name"):
                            score += 0.2

        metrics = db.query("SELECT metric_name, expression, metric_type FROM dbo.dataset_metrics WHERE dataset_id = @param0", [ds["id"]])
        met_rows = metrics.get("rows", [])
        for m in met_rows:
            mn = (m.get("metric_name") or m.get("name") or "").lower()
            for mw in mn.split():
                if len(mw) < 3:
                    continue
                for qw in q_words:
                    if mw in qw or qw in mw:
                        score += 0.2

        if score > best_score:
            best_score = score
            best = (ds, col_rows, met_rows)

    if best and best_score >= 0.2:
        return best
    return None, [], []


def _extract_entity(question: str, columns: list) -> tuple:
    """Extract entity filter (country name, model name, etc.)."""
    str_cols = [c for c in columns if c.get("data_type") in ("varchar", "character varying", "text")]
    if not str_cols:
        return None, None, question

    patterns = [
        r"\b(?:in|for|of)\s+([A-Z][a-z]+(?:\s+[A-Z][a-z]+)*)",
        r"\b(?:does|did|is|has|about)\s+([A-Z][a-z]+(?:\s+[A-Z][a-z]+)*)",
        r"^([A-Z][a-z]+(?:\s+[A-Z][a-z]+)*)\s+",
    ]

    for pat in patterns:
        m = re.search(pat, question)
        if m and m.group(1).lower() not in NOT_ENTITIES:
            entity = m.group(1)
            col = next((c for c in str_cols if re.search(r"country|name|region|provider|model", c["column_name"], re.I)), str_cols[0])
            clean = question.replace(entity, "").replace("  ", " ").strip()
            return col["column_name"], entity, clean

    return None, None, question


def _extract_year(question: str, columns: list) -> tuple:
    """Extract year filter."""
    m = re.search(r"\b(?:in|for|during|year)?\s*(20[1-9]\d)\b", question)
    year_col = next((c for c in columns if re.search(r"year|dt|date", c["column_name"], re.I)), None)
    if m and year_col:
        year = m.group(1)
        clean = re.sub(r"\b(?:in|for|during|year)?\s*20[1-9]\d\b", "", question).strip()
        is_numeric = year_col.get("data_type") in ("smallint", "integer", "bigint", "numeric")
        return year_col["column_name"], year, is_numeric, clean
    return None, None, None, question


def _build_sql(question: str, ds: dict, columns: list, metrics: list) -> tuple:
    """Build SQL from the question. Returns (sql, chart_type, title)."""
    q = question.lower().strip()
    schema = ds.get("schema_name") or "public"
    table = ds.get("fact_table") or ds.get("table_name")
    full_table = f"{schema}.{table}" if schema not in ("public", "dbo") else table

    # Find columns by type
    str_cols = [c["column_name"] for c in columns if c.get("is_dimension")]
    num_cols = [c["column_name"] for c in columns if c.get("is_metric")]
    group_col = next((c for c in str_cols if re.search(r"country|name|provider|model|borough", c, re.I)), str_cols[0] if str_cols else None)

    # Find matching metric
    metric_expr = None
    metric_name = None
    for m in metrics:
        mn = (m.get("metric_name") or m.get("name") or "").lower()
        for qw in q.split():
            if len(qw) >= 3 and (qw in mn or mn in qw):
                metric_expr = m.get("expression")
                metric_name = m.get("metric_name") or m.get("name")
                break
        if metric_expr:
            break

    # Alias matching
    if not metric_expr:
        for qw in q.split():
            if qw in COLUMN_ALIASES:
                for alias in COLUMN_ALIASES[qw]:
                    if alias in num_cols:
                        metric_expr = f"SUM({alias})"
                        metric_name = alias
                        break
            if metric_expr:
                break

    # Fallback to first metric
    if not metric_expr and metrics:
        metric_expr = metrics[0].get("expression")
        metric_name = metrics[0].get("metric_name") or metrics[0].get("name")
    if not metric_expr and num_cols:
        metric_expr = f"SUM({num_cols[0]})"
        metric_name = num_cols[0]

    # Extract filters
    entity_col, entity_val, clean_q = _extract_entity(question, columns)
    year_col, year_val, year_numeric, clean_q2 = _extract_year(clean_q or question, columns)

    where_parts = []
    if entity_col and entity_val:
        where_parts.append(f"{entity_col} = '{entity_val}'")
    if year_col and year_val:
        if year_numeric:
            where_parts.append(f"{year_col} = {year_val}")
        else:
            where_parts.append(f"EXTRACT(YEAR FROM {year_col}) = {year_val}")

    where = f" WHERE {' AND '.join(where_parts)}" if where_parts else ""

    # Detect pattern
    top_match = re.search(r"top\s+(\d+)", q)

    if top_match and group_col and metric_expr:
        n = int(top_match.group(1))
        sql = f"SELECT {group_col}, {metric_expr} FROM {full_table}{where} GROUP BY {group_col} HAVING {metric_expr} IS NOT NULL ORDER BY {metric_expr} DESC LIMIT {n}"
        return sql, "bar", f"Top {n} {group_col} by {metric_name}"

    if re.search(r"\bby\b", q) and group_col and metric_expr:
        sql = f"SELECT {group_col}, {metric_expr} FROM {full_table}{where} GROUP BY {group_col} HAVING {metric_expr} IS NOT NULL ORDER BY {metric_expr} DESC LIMIT 1000"
        return sql, "bar", f"{metric_name} by {group_col}"

    if entity_val and metric_expr:
        sql = f"SELECT {metric_expr} FROM {full_table}{where} LIMIT 1"
        return sql, "kpi", f"{metric_name} for {entity_val}"

    if metric_expr:
        sql = f"SELECT {metric_expr} FROM {full_table}{where} LIMIT 1"
        return sql, "kpi", metric_name or "Result"

    return None, None, None


def _format_number(v) -> str:
    try:
        n = float(v)
        if abs(n) >= 1e9:
            return f"{n/1e9:.1f}B"
        if abs(n) >= 1e6:
            return f"{n/1e6:.1f}M"
        if abs(n) >= 1e3:
            return f"{n/1e3:.1f}K"
        return f"{n:,.2f}" if n != int(n) else f"{int(n):,}"
    except:
        return str(v)


def _generate_answer(rows, columns, chart_type, title, question) -> str:
    if not rows:
        return "No results found."

    if chart_type == "kpi" and len(rows) == 1:
        val = rows[0][0] if rows[0] else 0
        return f"**{title}:** {_format_number(val)}"

    if len(rows) <= 20:
        lines = [f"**{title}** ({len(rows)} results)\n"]
        for i, row in enumerate(rows):
            name = row[0] if row else "?"
            val = _format_number(row[1]) if len(row) > 1 else ""
            lines.append(f"{i+1}. **{name}** — {val}")
        return "\n".join(lines)

    top5 = rows[:5]
    lines = [f"Found **{len(rows)}** results for {title}.\n"]
    for i, row in enumerate(top5):
        name = row[0] if row else "?"
        val = _format_number(row[1]) if len(row) > 1 else ""
        lines.append(f"{i+1}. **{name}** — {val}")
    lines.append(f"\n*...and {len(rows) - 5} more.*")
    return "\n".join(lines)


# ── Endpoint ───────────────────────────────────────────────────────────────────

@router.post("/chat")
def chat(req: ChatRequest, ctx: UserContext = Depends(require_user_context)):
    t0 = time.time()
    question = _fix_typos(req.question.strip())

    if not question:
        return ChatResponse(question=req.question, answer="Please ask a question.", route="error")

    # Find matching dataset
    ds_info, columns, metrics = _find_dataset(question, req.dataset_id)
    if not ds_info:
        # List available datasets
        all_ds = db.query("SELECT dataset_name FROM dbo.datasets")
        names = [r.get("dataset_name") for r in all_ds.get("rows", [])]
        return ChatResponse(
            question=req.question,
            answer=f"I couldn't match that to your data. Available datasets: **{', '.join(names)}**",
            route="no_match",
        )

    database = req.database or ds_info.get("database_name") or "kaveon"

    # Build SQL
    sql, chart_type, title = _build_sql(question, ds_info, columns, metrics)
    if not sql:
        return ChatResponse(
            question=req.question,
            answer="I couldn't generate a query for that question. Try: 'Show [metric] by [column]' or 'Top 10 [column] by [metric]'.",
            route="parse_error",
            dataset_name=ds_info.get("dataset_name"),
        )

    # Execute
    try:
        result = pool.execute_query(sql, database)
        rows = result.get("rows", [])
        cols = result.get("columns", [])
        duration_ms = int((time.time() - t0) * 1000)

        answer = _generate_answer(rows, cols, chart_type, title or "", question)

        return ChatResponse(
            question=req.question,
            answer=answer,
            sql=sql,
            data={"columns": cols, "rows": rows, "row_count": len(rows)},
            chart_type=chart_type,
            route="direct",
            duration_ms=duration_ms,
            dataset_name=ds_info.get("dataset_name"),
        )
    except Exception as e:
        duration_ms = int((time.time() - t0) * 1000)
        return ChatResponse(
            question=req.question,
            answer=f"Query failed: {str(e)[:200]}",
            sql=sql,
            route="error",
            duration_ms=duration_ms,
            error=str(e)[:500],
            dataset_name=ds_info.get("dataset_name"),
        )
