"""Query Generator — port of queryGenerator.service.ts."""

import re
import warnings
from typing import Dict, List, Optional, Tuple


def normalize_column_name(raw: str) -> str:
    """Replace | with . and trim (FabricExplorer uses | as separator)."""
    if not raw:
        return ""
    return raw.replace("|", ".").strip()


def quote_identifier(identifier: str) -> str:
    """Quote SQL identifier with brackets. Handles dot-separated parts."""
    if not identifier:
        return ""
    clean = identifier.replace("[", "").replace("]", "")
    parts = clean.split(".")
    return ".".join(f"[{p.replace(']', ']]')}]" for p in parts)


def _parse_join_condition(join_condition: str) -> Tuple[Optional[str], Optional[str]]:
    if not join_condition:
        return None, None
    regex = r"\[(?:[^\]]+)\]\.\[(?:[^\]]+)\]\.\[([^\]]+)\]\s*=\s*\[(?:[^\]]+)\]\.\[(?:[^\]]+)\]\.\[([^\]]+)\]"
    m = re.search(regex, join_condition)
    if m:
        return m.group(1), m.group(2)
    simple = re.search(r"(\w+)\s*=\s*(\w+)", join_condition)
    if simple:
        return simple.group(1), simple.group(2)
    return None, None


def _apply_date_format(col_expr: str, date_format: Optional[str], db_type: str = "fabric_sql") -> str:
    if not date_format:
        return col_expr
    fmt = date_format.lower()
    if fmt in ("day", "auto"):
        return col_expr
    pg = db_type in ("postgresql", "mysql")
    if pg:
        # Postgres / MySQL: truncate via date_trunc; label formats via to_char.
        if db_type == "postgresql":
            if fmt == "month":        return f"DATE_TRUNC('month', {col_expr})::date"
            if fmt == "quarter":      return f"DATE_TRUNC('quarter', {col_expr})::date"
            if fmt == "year":         return f"DATE_TRUNC('year', {col_expr})::date"
            if fmt == "week":         return f"DATE_TRUNC('week', {col_expr})::date"
            if fmt == "month-year":   return f"TO_CHAR({col_expr}, 'Mon YYYY')"
            if fmt == "quarter-year": return f"CONCAT('Q', EXTRACT(QUARTER FROM {col_expr})::int, ' ', EXTRACT(YEAR FROM {col_expr})::int)"
            return col_expr
        # mysql
        if fmt == "month":        return f"DATE_FORMAT({col_expr}, '%Y-%m-01')"
        if fmt == "year":         return f"DATE_FORMAT({col_expr}, '%Y-01-01')"
        if fmt == "week":         return f"DATE(DATE_SUB({col_expr}, INTERVAL WEEKDAY({col_expr}) DAY))"
        if fmt == "month-year":   return f"DATE_FORMAT({col_expr}, '%b %Y')"
        return col_expr
    if fmt == "month":
        return f"DATEFROMPARTS(YEAR({col_expr}), MONTH({col_expr}), 1)"
    if fmt == "quarter":
        return f"DATEFROMPARTS(YEAR({col_expr}), ((DATEPART(QUARTER, {col_expr}) - 1) * 3) + 1, 1)"
    if fmt == "year":
        return f"DATEFROMPARTS(YEAR({col_expr}), 1, 1)"
    if fmt == "week":
        return f"DATEADD(DAY, 1 - DATEPART(WEEKDAY, {col_expr}), {col_expr})"
    if fmt == "month-year":
        return f"FORMAT({col_expr}, 'MMM yyyy')"
    if fmt == "quarter-year":
        return f"CONCAT('Q', DATEPART(QUARTER, {col_expr}), ' ', YEAR({col_expr}))"
    return col_expr


def _apply_time_grain(col_expr: str, grain: str, db_type: str = "fabric_sql") -> str:
    """Truncate a datetime expression to the given time grain (dialect-aware)."""
    g = (grain or "").lower()
    if db_type == "postgresql":
        if g in ("", "none", "day"): return f"CAST({col_expr} AS DATE)"
        if g in ("week", "month", "quarter", "year"):
            return f"DATE_TRUNC('{g}', {col_expr})::date"
        return col_expr
    if db_type == "mysql":
        if g in ("", "none", "day"): return f"DATE({col_expr})"
        if g == "week":  return f"DATE(DATE_SUB({col_expr}, INTERVAL WEEKDAY({col_expr}) DAY))"
        if g == "month": return f"DATE_FORMAT({col_expr}, '%Y-%m-01')"
        if g == "year":  return f"DATE_FORMAT({col_expr}, '%Y-01-01')"
        return f"DATE({col_expr})"
    # SQL Server / Fabric (T-SQL)
    if g in ("", "none", "day"):
        return f"CAST({col_expr} AS DATE)"
    if g == "week":
        return f"DATEADD(DAY, 1 - DATEPART(WEEKDAY, {col_expr}), CAST({col_expr} AS DATE))"
    if g == "month":
        return f"DATEFROMPARTS(YEAR({col_expr}), MONTH({col_expr}), 1)"
    if g == "quarter":
        return f"DATEFROMPARTS(YEAR({col_expr}), ((DATEPART(QUARTER, {col_expr}) - 1) * 3) + 1, 1)"
    if g == "year":
        return f"DATEFROMPARTS(YEAR({col_expr}), 1, 1)"
    return col_expr


# ─── Coalesce map ────────────────────────────────────────────────────────────

def _build_coalesce_map(
    dimensions: list,
    alias_map: Dict[str, str],
    columns: list,
) -> Dict[str, list]:
    """Returns {table_lower: [{alias, columnName}, ...]} for multi-dim fact_keys."""
    coalesce_map: Dict[str, list] = {}
    if not dimensions:
        return coalesce_map

    # Group dims by fact_key
    by_fact_key: Dict[str, list] = {}
    for dim in dimensions:
        fk = dim.get("factKey") or ""
        if not fk:
            continue
        fk_norm = normalize_column_name(fk).lower()
        by_fact_key.setdefault(fk_norm, []).append(dim)

    for fact_key, dims in by_fact_key.items():
        if len(dims) <= 1:
            continue

        # Derive semantic label
        semantic = fact_key
        if semantic.endswith("key"):
            semantic = semantic[:-3]
        elif semantic.endswith("id"):
            semantic = semantic[:-2]

        sources = []
        for dim in dims:
            table_name = normalize_column_name(dim.get("table") or "").lower()
            alias = alias_map.get(table_name)
            if not alias:
                continue

            # Find physical column name for this table
            by_semantic = next(
                (c for c in columns
                 if normalize_column_name(c.get("table_name") or "").lower() == table_name
                 and (c.get("semantic_type") or "").lower() == semantic
                 and c.get("is_dimension")),
                None,
            )
            by_table = next(
                (c for c in columns
                 if normalize_column_name(c.get("table_name") or "").lower() == table_name
                 and c.get("is_dimension")),
                None,
            )
            col_obj = by_semantic or by_table
            if not col_obj:
                continue
            sources.append({"alias": alias, "columnName": col_obj["column_name"]})

        if len(sources) < 2:
            continue

        for dim in dims:
            table_name = normalize_column_name(dim.get("table") or "").lower()
            coalesce_map[table_name] = sources
            short = (dim.get("table") or "").split(".")[-1].lower()
            if short and short not in coalesce_map:
                coalesce_map[short] = sources

    return coalesce_map


# ─── Column-to-table map ──────────────────────────────────────────────────────

def _build_column_table_map(columns: list) -> Dict[str, str]:
    m: Dict[str, str] = {}
    for col in (columns or []):
        table = normalize_column_name(col.get("table_name") or "")
        col_name = col.get("column_name")
        if table and col_name:
            m[col_name.lower()] = table
    return m


# ─── Required dimensions ──────────────────────────────────────────────────────

def _get_required_dimensions(
    dimensions: list,
    groupby: List[str],
    filters: list,
    column_table_map: Dict[str, str],
    datasource: str,
) -> list:
    if not dimensions:
        return []

    used_tables: set = set()

    def _add_table_from_col(col: str):
        normalized = normalize_column_name(col)
        parts = [p for p in normalized.split(".") if p]
        if len(parts) == 1:
            tbl = column_table_map.get(normalized.lower())
            if tbl:
                used_tables.add(tbl.lower())
        elif len(parts) >= 2:
            tbl = f"{parts[0]}.{parts[1]}" if len(parts) == 3 else parts[0]
            used_tables.add(tbl.lower())

    for col in groupby:
        _add_table_from_col(col)

    for f in filters:
        raw_col = f.get("column") or f.get("col") or ""
        col = str(raw_col).split("|")[0]
        is_optimized = f.get("keyColumn") and f.get("valueKey") not in (None, "")
        if not is_optimized and col:
            _add_table_from_col(col)

    # First pass: directly referenced
    directly_required = [
        dim for dim in dimensions
        if normalize_column_name(dim.get("table") or "").lower() in used_tables
    ]

    # Expand to include siblings sharing the same fact_key
    required_fact_keys = {
        normalize_column_name(dim.get("factKey") or "").lower()
        for dim in directly_required
        if dim.get("factKey")
    }

    return [
        dim for dim in dimensions
        if normalize_column_name(dim.get("table") or "").lower() in used_tables
        or normalize_column_name(dim.get("factKey") or "").lower() in required_fact_keys
    ]


# ─── Dimension joins ──────────────────────────────────────────────────────────

def _build_dimension_joins(
    dimensions: list,
    fact_alias: str,
    datasource: str,
) -> Tuple[Dict[str, str], List[str]]:
    alias_map: Dict[str, str] = {}
    join_clauses: List[str] = []

    for idx, dim in enumerate(dimensions):
        table_name = normalize_column_name(dim.get("table") or "")
        if not table_name:
            continue

        alias = f"dim{idx + 1}"

        fact_key_parts = [p for p in (dim.get("factKey") or "").split(".") if p]
        dim_key_parts = [p for p in (dim.get("dimKey") or "").split(".") if p]
        fact_key = fact_key_parts[-1] if fact_key_parts else ""
        dim_key = dim_key_parts[-1] if dim_key_parts else ""

        if not fact_key or not dim_key:
            continue

        dim_table = quote_identifier(table_name)
        join_clauses.append(
            f"LEFT JOIN {dim_table} AS {alias} "
            f"ON {fact_alias}.{quote_identifier(fact_key)} = {alias}.{quote_identifier(dim_key)}"
        )

        lower_full = table_name.lower()
        alias_map[lower_full] = alias
        short = table_name.split(".")[-1].lower()
        if short and short not in alias_map:
            alias_map[short] = alias

    return alias_map, join_clauses


# ─── Column resolution ────────────────────────────────────────────────────────

def _resolve_column_alias(
    raw_column: str,
    fact_alias: str,
    alias_map: Dict[str, str],
    column_table_map: Optional[Dict[str, str]] = None,
) -> Tuple[str, str]:
    """Returns (alias, column_name)."""
    clean = normalize_column_name(raw_column)
    if not clean:
        return fact_alias, ""

    parts = [p for p in clean.split(".") if p]

    if len(parts) == 1:
        col_name = parts[0]
        if column_table_map:
            tbl = column_table_map.get(col_name.lower())
            if tbl:
                a = alias_map.get(tbl.lower())
                if a:
                    return a, col_name
        return fact_alias, col_name

    base_col = parts[-1]
    table_prefix = ".".join(parts[:-1]).lower()
    alias = alias_map.get(table_prefix)

    if not alias and len(parts) > 1:
        alias = alias_map.get(parts[0].lower())
    if not alias and len(parts) > 2:
        alias = alias_map.get(parts[-2].lower())

    return alias or fact_alias, base_col


def _resolve_column_expression(
    raw_column: str,
    fact_alias: str,
    alias_map: Dict[str, str],
    column_table_map: Dict[str, str],
    coalesce_map: Dict[str, list],
) -> str:
    alias, col_name = _resolve_column_alias(raw_column, fact_alias, alias_map, column_table_map)

    if not col_name:
        return f"{alias}.*"

    # Find which table name this alias maps to
    table_name = next((t for t, a in alias_map.items() if a == alias), None)

    if table_name and table_name in coalesce_map:
        sources = coalesce_map[table_name]
        coalesce_parts = [f"{s['alias']}.{quote_identifier(s['columnName'])}" for s in sources]
        return f"COALESCE({', '.join(coalesce_parts)})"

    return f"{alias}.{quote_identifier(col_name)}"


# ─── Filter clause ────────────────────────────────────────────────────────────

_SAFE_OPS = {"=", "!=", "<>", "<", ">", "<=", ">=", "LIKE", "NOT LIKE"}


def _build_optimized_filter_clause(
    filter_: dict,
    fact_alias: str,
    alias_map: Dict[str, str],
    column_table_map: Dict[str, str],
) -> Optional[str]:
    raw_col = filter_.get("column") or filter_.get("col") or ""
    col = str(raw_col).split("|")[0]
    val = filter_.get("value")
    key_column = filter_.get("keyColumn")
    value_key = filter_.get("valueKey")

    if not col or val is None:
        return None

    raw_op = (filter_.get("op") or filter_.get("operator") or "=").upper().strip()
    op = raw_op if raw_op in _SAFE_OPS else "="

    # Tier 1/2: use fact table key column
    if key_column and value_key not in (None, ""):
        key_parts = [p for p in normalize_column_name(key_column).split(".") if p]
        key_col_name = key_parts[-1]
        quoted_key_col = f"{fact_alias}.{quote_identifier(key_col_name)}"

        if isinstance(value_key, list):
            values = ", ".join(f"'{str(v).replace(chr(39), chr(39)*2)}'" for v in value_key)
            return f"{quoted_key_col} IN ({values})"
        else:
            safe_val = str(value_key).replace("'", "''")
            return f"{quoted_key_col} {op} '{safe_val}'"

    # Graceful skip: when we have schema info and this single-part column is not
    # part of this dataset, drop the filter instead of emitting fact.<unknown>.
    # This lets a dashboard/cross-filter (e.g. clicking a country on the map) be
    # broadcast to every tile — it applies only to charts whose dataset has the
    # column, and is silently ignored elsewhere rather than erroring.
    _parts = [p for p in normalize_column_name(col).split(".") if p]
    if len(_parts) == 1 and column_table_map and _parts[0].lower() not in column_table_map:
        return None

    # Tier 3: dimension display column
    alias, col_name = _resolve_column_alias(col, fact_alias, alias_map, column_table_map)
    quoted_col = f"{alias}.{quote_identifier(col_name)}"

    if isinstance(val, list):
        values = ", ".join(f"'{str(v).replace(chr(39), chr(39)*2)}'" for v in val)
        return f"{quoted_col} IN ({values})"
    else:
        safe_val = str(val).replace("'", "''")
        return f"{quoted_col} {op} '{safe_val}'"


# ─── Filtering strategy ───────────────────────────────────────────────────────

def _determine_filtering_strategy(
    column: str,
    dimensions: list,
    datasource: str,
    column_table_map: Dict[str, str],
) -> Tuple[Optional[str], int]:
    """Returns (key_column, tier) where tier is 1, 2, or 3."""
    parts = [p for p in column.split(".") if p]

    if len(parts) == 1:
        col_name = parts[0]
        target_table = column_table_map.get(col_name.lower())
    elif len(parts) == 3:
        target_table = f"{parts[0]}.{parts[1]}"
        col_name = parts[2]
    elif len(parts) == 2:
        target_table = parts[0]
        col_name = parts[1]
    else:
        col_name = column
        target_table = None

    if not target_table or target_table.lower() == datasource.lower():
        return None, 3

    matching_dim = next(
        (d for d in dimensions
         if normalize_column_name(d.get("table") or "").lower() == target_table.lower()),
        None,
    )
    if not matching_dim:
        return None, 3

    fact_key_parts = [p for p in normalize_column_name(matching_dim.get("factKey") or "").split(".") if p]
    dim_key_parts = [p for p in normalize_column_name(matching_dim.get("dimKey") or "").split(".") if p]
    fact_key_col = fact_key_parts[-1] if fact_key_parts else None
    dim_key_col = dim_key_parts[-1] if dim_key_parts else None

    if dim_key_col and col_name.lower() == dim_key_col.lower():
        return fact_key_col, 2

    return fact_key_col, 3


# ─── Public API ───────────────────────────────────────────────────────────────

def build_chart_preview_query(params: dict) -> Optional[str]:
    try:
        raw_datasource = params.get("datasource") or ""
        sql_text = (params.get("sql_text") or "").strip().rstrip(";").strip()
        metric = params.get("metric")
        metrics = params.get("metrics")
        groupby = params.get("groupby") or []
        time_column = params.get("time_column")
        time_grain = params.get("time_grain") or "none"
        filters = params.get("filters") or []
        time_range = params.get("time_range")
        date_display_format = params.get("date_display_format")
        dimensions = params.get("dimensions") or []
        columns = params.get("columns") or []
        row_limit = params.get("row_limit") or None
        sort_by = params.get("sort_by")  # { column, direction }
        query_mode = (params.get("query_mode") or "aggregate").lower()
        rolling_calc = (params.get("rolling_calc") or "none").lower()  # "none", "rolling_avg", "cumulative_sum"
        rolling_window = max(2, int(params.get("rolling_window") or 3))
        db_type = (params.get("db_type") or "fabric_sql").lower()  # dialect for date grain/format

        datasource = normalize_column_name(raw_datasource)

        # Virtual SQL dataset — use the saved SQL as a subquery source
        if sql_text and not datasource:
            fact_alias = "fact"
            fact_table = f"(\n{sql_text}\n)"
        elif not datasource:
            return None
        else:
            fact_alias = "fact"
            fact_table = quote_identifier(datasource)

        column_table_map = _build_column_table_map(columns)
        required_dims = _get_required_dimensions(dimensions, groupby, filters, column_table_map, datasource)
        alias_map, join_clauses = _build_dimension_joins(required_dims, fact_alias, datasource)
        coalesce_map = _build_coalesce_map(required_dims, alias_map, columns)

        metric_list = metrics or ([metric] if metric else [])

        # ── RAW / RECORD mode — no aggregation ────────────────────────────────
        if query_mode == "raw":
            select_parts: List[str] = []
            for col in groupby:
                expr = _resolve_column_expression(col, fact_alias, alias_map, column_table_map, coalesce_map)
                if expr not in select_parts:
                    select_parts.append(expr)
            if time_column:
                expr = _resolve_column_expression(time_column, fact_alias, alias_map, column_table_map, coalesce_map)
                if expr not in select_parts:
                    select_parts.append(expr)
            for m in metric_list:
                col = m.get("column") or m.get("field")
                if col:
                    alias, col_name = _resolve_column_alias(col, fact_alias, alias_map, column_table_map)
                    raw_expr = f"{alias}.{quote_identifier(col_name)}"
                    if raw_expr not in select_parts:
                        select_parts.append(raw_expr)
            if not select_parts:
                select_parts.append(f"{fact_alias}.*")

            top_clause = f"TOP {int(row_limit)} " if row_limit and int(row_limit) > 0 else "TOP 1000 "
            select_clause = f"SELECT {top_clause}{', '.join(select_parts)}"
            from_clause = f"FROM {fact_table} AS {fact_alias}"
            join_clause = " ".join(join_clauses)

            where_parts: List[str] = []
            for f in filters:
                fc = _build_optimized_filter_clause(f, fact_alias, alias_map, column_table_map)
                if fc:
                    where_parts.append(fc)
            where_clause = f"WHERE {' AND '.join(where_parts)}" if where_parts else ""

            parts = [p for p in [select_clause, from_clause, join_clause, where_clause] if p]
            return " ".join(parts)

        # ── AGGREGATE mode (default) ───────────────────────────────────────────
        select_parts = []
        group_by_parts: List[str] = []

        for col in groupby:
            expr = _resolve_column_expression(col, fact_alias, alias_map, column_table_map, coalesce_map)
            if expr not in select_parts:
                select_parts.append(expr)

        time_grain_expr = None  # The grain-truncated time expression for GROUP BY / ORDER BY
        if time_column:
            raw_expr = _resolve_column_expression(time_column, fact_alias, alias_map, column_table_map, coalesce_map)
            if time_grain and time_grain != "none":
                grain_expr = _apply_time_grain(raw_expr, time_grain, db_type)
            else:
                grain_expr = _apply_date_format(raw_expr, date_display_format, db_type)
            time_grain_expr = grain_expr
            # Alias the time bucket as "date" so the frontend reliably treats it as
            # the x-axis. Without this, CAST(dt AS DATE) is named "dt" (no date/time
            # token) and the chart mis-classifies every date as a separate series.
            grain_select = f"{grain_expr} AS {quote_identifier('date')}"
            if grain_select not in select_parts:
                select_parts.append(grain_select)

        for m in metric_list:
            agg_func = (m.get("aggregate") or m.get("agg") or "SUM").upper()
            col = m.get("column") or m.get("field")
            if col:
                alias, col_name = _resolve_column_alias(col, fact_alias, alias_map, column_table_map)
                if agg_func == "COUNT_DISTINCT":
                    metric_expr = f"COUNT(DISTINCT {alias}.{quote_identifier(col_name)})"
                else:
                    metric_expr = f"{agg_func}({alias}.{quote_identifier(col_name)})"
                metric_alias = m.get("label") or m.get("name") or col_name or "value"
                select_parts.append(f"{metric_expr} AS {quote_identifier(metric_alias)}")

        if not select_parts:
            select_parts.append("*")

        top_clause = f"TOP {int(row_limit)} " if row_limit and int(row_limit) > 0 else ""
        select_clause = f"SELECT {top_clause}{', '.join(select_parts)}"
        from_clause = f"FROM {fact_table} AS {fact_alias}"
        join_clause = " ".join(join_clauses)

        where_parts = []

        # ── Time range filter ─────────────────────────────────────────────────
        if time_column and time_range and time_range != "all_time":
            raw_time_expr = _resolve_column_expression(time_column, fact_alias, alias_map, column_table_map, coalesce_map)
            today = "CAST(GETUTCDATE() AS DATE)"
            tr = time_range.lower()

            range_sql: Optional[str] = None
            if tr == "latest_day":
                # Filter to rows where the date equals the maximum date in the table
                tc_parts = time_column.split(".")
                tc_quoted = quote_identifier(tc_parts[-1])
                range_sql = (
                    f"CAST({raw_time_expr} AS DATE) = "
                    f"(SELECT MAX(CAST({tc_quoted} AS DATE)) FROM {fact_table})"
                )
            elif tr == "last_day":
                range_sql = f"{raw_time_expr} >= DATEADD(DAY, -1, {today})"
            elif tr == "last_week":
                range_sql = f"{raw_time_expr} >= DATEADD(WEEK, -1, {today})"
            elif tr == "last_month":
                range_sql = f"{raw_time_expr} >= DATEADD(MONTH, -1, {today})"
            elif tr == "last_quarter":
                range_sql = f"{raw_time_expr} >= DATEADD(QUARTER, -1, {today})"
            elif tr == "last_year":
                range_sql = f"{raw_time_expr} >= DATEADD(YEAR, -1, {today})"
            elif tr == "week_to_date":
                range_sql = f"{raw_time_expr} >= DATEADD(DAY, 1 - DATEPART(WEEKDAY, {today}), {today})"
            elif tr == "month_to_date":
                range_sql = f"{raw_time_expr} >= DATEFROMPARTS(YEAR({today}), MONTH({today}), 1)"
            elif tr == "quarter_to_date":
                range_sql = f"{raw_time_expr} >= DATEFROMPARTS(YEAR({today}), ((DATEPART(QUARTER, {today}) - 1) * 3) + 1, 1)"
            elif tr == "year_to_date":
                range_sql = f"{raw_time_expr} >= DATEFROMPARTS(YEAR({today}), 1, 1)"
            elif tr == "previous_week":
                range_sql = (f"{raw_time_expr} >= DATEADD(WEEK, -2, DATEADD(DAY, 1 - DATEPART(WEEKDAY, {today}), {today})) "
                             f"AND {raw_time_expr} < DATEADD(DAY, 1 - DATEPART(WEEKDAY, {today}), {today})")
            elif tr == "previous_month":
                range_sql = (f"{raw_time_expr} >= DATEFROMPARTS(YEAR(DATEADD(MONTH, -1, {today})), MONTH(DATEADD(MONTH, -1, {today})), 1) "
                             f"AND {raw_time_expr} < DATEFROMPARTS(YEAR({today}), MONTH({today}), 1)")
            elif tr == "previous_quarter":
                range_sql = (f"{raw_time_expr} >= DATEADD(QUARTER, -2, DATEFROMPARTS(YEAR({today}), ((DATEPART(QUARTER, {today}) - 1) * 3) + 1, 1)) "
                             f"AND {raw_time_expr} < DATEFROMPARTS(YEAR({today}), ((DATEPART(QUARTER, {today}) - 1) * 3) + 1, 1)")
            elif tr == "previous_year":
                range_sql = (f"{raw_time_expr} >= DATEFROMPARTS(YEAR({today}) - 1, 1, 1) "
                             f"AND {raw_time_expr} < DATEFROMPARTS(YEAR({today}), 1, 1)")
            elif tr == "last_7_days":
                range_sql = f"{raw_time_expr} >= DATEADD(DAY, -7, {today})"
            elif tr == "last_30_days":
                range_sql = f"{raw_time_expr} >= DATEADD(DAY, -30, {today})"
            elif tr == "last_90_days":
                range_sql = f"{raw_time_expr} >= DATEADD(DAY, -90, {today})"
            elif tr == "last_365_days":
                range_sql = f"{raw_time_expr} >= DATEADD(DAY, -365, {today})"
            elif tr == "custom":
                custom_start = params.get("custom_start_date")
                custom_end = params.get("custom_end_date")
                if custom_start and custom_end:
                    range_sql = f"{raw_time_expr} >= '{custom_start}' AND {raw_time_expr} <= '{custom_end}'"
                elif custom_start:
                    range_sql = f"{raw_time_expr} >= '{custom_start}'"
            elif tr == "custom_to_latest":
                custom_start = params.get("custom_start_date")
                if custom_start:
                    range_sql = f"{raw_time_expr} >= '{custom_start}'"

            if range_sql:
                where_parts.append(f"({range_sql})")

        for f in filters:
            fc = _build_optimized_filter_clause(f, fact_alias, alias_map, column_table_map)
            if fc:
                where_parts.append(fc)

        where_clause = f"WHERE {' AND '.join(where_parts)}" if where_parts else ""

        # ── GROUP BY ─────────────────────────────────────────────────────────
        for col in groupby:
            expr = _resolve_column_expression(col, fact_alias, alias_map, column_table_map, coalesce_map)
            if expr not in group_by_parts:
                group_by_parts.append(expr)
        if time_grain_expr and time_grain_expr not in group_by_parts:
            group_by_parts.append(time_grain_expr)

        group_by_clause = f"GROUP BY {', '.join(group_by_parts)}" if group_by_parts else ""

        # ── ORDER BY ─────────────────────────────────────────────────────────
        if sort_by and sort_by.get("column"):
            sb_col = sort_by["column"]
            sb_dir = "DESC" if str(sort_by.get("direction", "asc")).upper() == "DESC" else "ASC"
            # Sorting by a metric under GROUP BY must use the aggregate, not the
            # raw column (which is invalid SQL in both T-SQL and Postgres).
            sb_metric = next(
                (m for m in metric_list
                 if (m.get("column") or m.get("field")) == sb_col
                 or (m.get("label") or m.get("name")) == sb_col),
                None,
            )
            # Is the sort column one of the GROUP BY dimensions? (then sort by it directly)
            _sb_norm = normalize_column_name(sb_col).split(".")[-1].lower()
            _sb_in_groupby = any(
                normalize_column_name(g).split(".")[-1].lower() == _sb_norm for g in groupby
            ) or (time_column and normalize_column_name(time_column).split(".")[-1].lower() == _sb_norm)

            if sb_metric and query_mode != "raw":
                agg_func = (sb_metric.get("aggregate") or sb_metric.get("agg") or "SUM").upper()
                m_alias, m_col = _resolve_column_alias(
                    sb_metric.get("column") or sb_metric.get("field"), fact_alias, alias_map, column_table_map)
                if agg_func == "COUNT_DISTINCT":
                    sb_expr = f"COUNT(DISTINCT {m_alias}.{quote_identifier(m_col)})"
                else:
                    sb_expr = f"{agg_func}({m_alias}.{quote_identifier(m_col)})"
            elif query_mode != "raw" and not _sb_in_groupby:
                # Sort by a non-grouped measure → aggregate it so it's valid under GROUP BY.
                s_alias, s_col = _resolve_column_alias(sb_col, fact_alias, alias_map, column_table_map)
                sb_expr = f"SUM({s_alias}.{quote_identifier(s_col)})"
            else:
                sb_expr = _resolve_column_expression(sb_col, fact_alias, alias_map, column_table_map, coalesce_map)
            order_by_clause = f"ORDER BY {sb_expr} {sb_dir}"
        elif time_grain_expr:
            order_by_clause = f"ORDER BY {time_grain_expr} ASC"
        elif group_by_parts:
            order_by_clause = f"ORDER BY {group_by_parts[0]}"
        else:
            order_by_clause = ""

        # ── Rolling window calculation ─────────────────────────────────────
        if rolling_calc != "none" and time_grain_expr and metric_list:
            # Dimension exprs = group_by_parts minus the time expression.
            # These may be bare COALESCE(...) or table.column exprs with no alias,
            # so we assign explicit positional aliases for the CTE and outer SELECT.
            dim_exprs = [p for p in group_by_parts if p != time_grain_expr]
            dim_aliases = [f"[__dim_{i}__]" for i in range(len(dim_exprs))]

            # Metric aggregation parts = select_parts entries that are NOT in group_by_parts
            # (they contain the SUM/AVG/COUNT aggregations with AS aliases).
            # Exclude the time-grain bucket alias (e.g. "DATE_TRUNC(...) AS [date]"),
            # otherwise the accumulator wraps it as SUM(date) → "function sum(date)
            # does not exist". The outer query already carries the axis as [__time__].
            group_by_set = set(group_by_parts)
            metric_agg_parts = [
                sp for sp in select_parts
                if sp not in group_by_set and not sp.startswith(time_grain_expr)
            ]

            # Extract the quoted alias from "AGG(x) AS [alias]" → "[alias]"
            metric_aliases = []
            for ma in metric_agg_parts:
                upper = ma.upper()
                if " AS " in upper:
                    alias_part = ma[ma.upper().rfind(" AS ") + 4:].strip()
                    metric_aliases.append(alias_part)

            # Build CTE SELECT with explicit aliases for every column
            cte_parts = []
            for expr, alias in zip(dim_exprs, dim_aliases):
                cte_parts.append(f"{expr} AS {alias}")
            cte_parts.append(f"{time_grain_expr} AS [__time__]")
            cte_parts.extend(metric_agg_parts)

            top_cte = f"TOP {int(row_limit)} " if row_limit and int(row_limit) > 0 else ""
            cte_select_clause = f"SELECT {top_cte}{', '.join(cte_parts)}"
            # NOTE: ORDER BY is intentionally omitted from the CTE — SQL Server
            # forbids ORDER BY inside a CTE without TOP/OFFSET.
            cte_body = " ".join([p for p in [cte_select_clause, from_clause, join_clause, where_clause, group_by_clause] if p])

            # PARTITION BY per dimension so window runs per series, not across all data
            partition_by = f"PARTITION BY {', '.join(dim_aliases)} " if dim_aliases else ""

            window_selects = []
            for alias in metric_aliases:
                if rolling_calc == "rolling_avg":
                    window_selects.append(
                        f"AVG({alias}) OVER ({partition_by}ORDER BY [__time__] ROWS BETWEEN {rolling_window - 1} PRECEDING AND CURRENT ROW) AS {alias}"
                    )
                elif rolling_calc == "cumulative_sum":
                    window_selects.append(
                        f"SUM({alias}) OVER ({partition_by}ORDER BY [__time__] ROWS UNBOUNDED PRECEDING) AS {alias}"
                    )

            if window_selects:
                outer_dims = ", ".join(dim_aliases + ["[__time__]"])
                window_query = (
                    f"WITH base AS ({cte_body}) "
                    f"SELECT {outer_dims}, {', '.join(window_selects)} "
                    f"FROM base "
                    f"ORDER BY {', '.join(dim_aliases + ['[__time__]'])} ASC"
                )
                return window_query

        parts = [p for p in [select_clause, from_clause, join_clause, where_clause, group_by_clause, order_by_clause] if p]
        return " ".join(parts)

    except Exception:
        return None


def _dim_direct_subquery(dim_entry: dict, col_name: str, limit: int) -> Optional[str]:
    """
    Build a query that reads directly from a single dimension table — no fact table,
    no join.  Returns None if the dim entry lacks the required metadata.

    For ID-based dimKey columns the real stored ID is returned as [key].
    For Key-based dimKey columns key == value so the frontend can apply mmh3 as usual.
    """
    table_raw = normalize_column_name(dim_entry.get("table") or "")
    if not table_raw:
        return None

    dk_parts = [p for p in normalize_column_name(dim_entry.get("dimKey") or "").split(".") if p]
    dk = dk_parts[-1] if dk_parts else None

    dim_table = quote_identifier(table_raw)
    quoted_display = quote_identifier(col_name)

    if dk and (dk.endswith("ID") or dk.endswith("Id")):
        # Return the actual stored ID from the dim table as [key]
        key_select = f"CAST({quote_identifier(dk)} AS NVARCHAR(MAX))"
    else:
        # Key/hash columns: key == value so the frontend hashes client-side
        key_select = quoted_display

    return (
        f"SELECT DISTINCT TOP {int(limit)} {key_select} AS [key], {quoted_display} AS [value] "
        f"FROM {dim_table} "
        f"WHERE {quoted_display} IS NOT NULL "
        f"ORDER BY [value]"
    )


def build_distinct_filter_values_query(params: dict) -> Optional[dict]:
    """Returns {sql, keyColumn, filteringTier} or None."""
    try:
        raw_datasource = params.get("datasource") or ""
        raw_column = params.get("column") or ""
        dimensions = params.get("dimensions") or []
        columns = params.get("columns") or []
        limit = params.get("limit") or 100

        datasource = normalize_column_name(raw_datasource)
        column = normalize_column_name(raw_column)

        if not datasource or not column:
            return None

        fact_alias = "fact"
        fact_table = quote_identifier(datasource)

        column_table_map = _build_column_table_map(columns)
        fake_filter = {"column": column}
        required_dims = _get_required_dimensions(dimensions, [], [fake_filter], column_table_map, datasource)
        alias_map, join_clauses = _build_dimension_joins(required_dims, fact_alias, datasource)
        key_column, tier = _determine_filtering_strategy(column, dimensions, datasource, column_table_map)
        coalesce_map = _build_coalesce_map(required_dims, alias_map, columns)

        alias, col_name = _resolve_column_alias(column, fact_alias, alias_map, column_table_map)

        # ── Fast path: column lives in a dim table ────────────────────────────
        # Query the dim table directly — no fact table, no join, much cheaper
        # since dim tables are small and typically indexed on their key columns.
        if alias != fact_alias:
            target_table_name = next((t for t, a in alias_map.items() if a == alias), None)
            sibling_group = coalesce_map.get(target_table_name) if target_table_name else None

            # Sibling group: multiple dim tables cover the same logical column (coalesce)
            if sibling_group and len(sibling_group) >= 2:
                subqueries = []
                for source in sibling_group:
                    dim_entry = next(
                        (d for i, d in enumerate(required_dims) if f"dim{i+1}" == source["alias"]),
                        None,
                    )
                    if not dim_entry:
                        continue
                    sub = _dim_direct_subquery(dim_entry, source["columnName"], limit)
                    if sub:
                        subqueries.append(sub)
                if len(subqueries) >= 2:
                    # Strip ORDER BY from each sub before UNIONing, add one at the end
                    stripped = [s.rsplit(" ORDER BY [value]", 1)[0] for s in subqueries]
                    sql = (
                        f"SELECT DISTINCT TOP {int(limit)} [key], [value] "
                        f"FROM ({' UNION '.join(stripped)}) AS [combined_vals] "
                        f"ORDER BY [value]"
                    )
                    return {"sql": sql, "keyColumn": key_column, "filteringTier": tier}

            # Single dim table
            dim_idx = next((i for i, _ in enumerate(required_dims) if f"dim{i+1}" == alias), None)
            if dim_idx is not None:
                sql = _dim_direct_subquery(required_dims[dim_idx], col_name, limit)
                if sql:
                    return {"sql": sql, "keyColumn": key_column, "filteringTier": tier}

            # Fallback: dim entry missing metadata — use fact+join query
            quoted_col = f"{alias}.{quote_identifier(col_name)}"
            join_clause = " ".join(join_clauses)
            sql = " ".join(p for p in [
                f"SELECT DISTINCT TOP {int(limit)} {quoted_col} AS [key], {quoted_col} AS [value]",
                f"FROM {fact_table} AS {fact_alias}",
                join_clause,
                f"WHERE {quoted_col} IS NOT NULL",
                "ORDER BY [value]",
            ] if p)
            return {"sql": sql, "keyColumn": key_column, "filteringTier": tier}

        # ── Column is on the fact table itself — query it directly ────────────
        quoted_col = f"{fact_alias}.{quote_identifier(col_name)}"
        sql = " ".join([
            f"SELECT DISTINCT TOP {int(limit)} {quoted_col} AS [key], {quoted_col} AS [value]",
            f"FROM {fact_table} AS {fact_alias}",
            f"WHERE {quoted_col} IS NOT NULL",
            "ORDER BY [value]",
        ])
        return {"sql": sql, "keyColumn": key_column, "filteringTier": tier}

    except Exception:
        return None
