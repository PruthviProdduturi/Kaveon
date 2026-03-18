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


def _apply_date_format(col_expr: str, date_format: Optional[str]) -> str:
    if not date_format:
        return col_expr
    fmt = date_format.lower()
    if fmt in ("day", "auto"):
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
        metric = params.get("metric")
        metrics = params.get("metrics")
        groupby = params.get("groupby") or []
        time_column = params.get("time_column")
        filters = params.get("filters") or []
        time_range = params.get("time_range")
        date_display_format = params.get("date_display_format")
        dimensions = params.get("dimensions") or []
        columns = params.get("columns") or []
        row_limit = params.get("row_limit") or 1000

        datasource = normalize_column_name(raw_datasource)
        if not datasource:
            return None

        fact_alias = "fact"
        fact_table = quote_identifier(datasource)

        column_table_map = _build_column_table_map(columns)
        required_dims = _get_required_dimensions(dimensions, groupby, filters, column_table_map, datasource)
        alias_map, join_clauses = _build_dimension_joins(required_dims, fact_alias, datasource)
        coalesce_map = _build_coalesce_map(required_dims, alias_map, columns)

        select_parts: List[str] = []

        for col in groupby:
            expr = _resolve_column_expression(col, fact_alias, alias_map, column_table_map, coalesce_map)
            if expr not in select_parts:
                select_parts.append(expr)

        if time_column:
            expr = _resolve_column_expression(time_column, fact_alias, alias_map, column_table_map, coalesce_map)
            expr = _apply_date_format(expr, date_display_format)
            if expr not in select_parts:
                select_parts.append(expr)

        metric_list = metrics or ([metric] if metric else [])
        for m in metric_list:
            agg_func = m.get("aggregate") or m.get("agg") or "SUM"
            col = m.get("column") or m.get("field")
            if col:
                alias, col_name = _resolve_column_alias(col, fact_alias, alias_map, column_table_map)
                metric_expr = f"{agg_func}({alias}.{quote_identifier(col_name)})"
                metric_alias = m.get("label") or m.get("name") or "value"
                select_parts.append(f"{metric_expr} AS {quote_identifier(metric_alias)}")

        if not select_parts:
            select_parts.append("*")

        select_clause = f"SELECT TOP {int(row_limit)} {', '.join(select_parts)}"
        from_clause = f"FROM {fact_table} AS {fact_alias}"
        join_clause = " ".join(join_clauses)

        where_parts: List[str] = []
        for f in filters:
            fc = _build_optimized_filter_clause(f, fact_alias, alias_map, column_table_map)
            if fc:
                where_parts.append(fc)

        if time_range and time_column:
            alias, col_name = _resolve_column_alias(time_column, fact_alias, alias_map, column_table_map)
            time_col = f"{alias}.{quote_identifier(col_name)}"
            if time_range == "last_7_days":
                where_parts.append(f"{time_col} >= DATEADD(day, -7, GETDATE())")
            elif time_range == "last_30_days":
                where_parts.append(f"{time_col} >= DATEADD(day, -30, GETDATE())")
            elif time_range == "last_90_days":
                where_parts.append(f"{time_col} >= DATEADD(day, -90, GETDATE())")

        where_clause = f"WHERE {' AND '.join(where_parts)}" if where_parts else ""

        group_by_parts: List[str] = []
        for col in groupby:
            expr = _resolve_column_expression(col, fact_alias, alias_map, column_table_map, coalesce_map)
            if expr not in group_by_parts:
                group_by_parts.append(expr)
        if time_column:
            expr = _resolve_column_expression(time_column, fact_alias, alias_map, column_table_map, coalesce_map)
            expr = _apply_date_format(expr, date_display_format)
            if expr not in group_by_parts:
                group_by_parts.append(expr)

        group_by_clause = f"GROUP BY {', '.join(group_by_parts)}" if group_by_parts else ""

        if time_column:
            time_expr = _resolve_column_expression(time_column, fact_alias, alias_map, column_table_map, coalesce_map)
            time_expr = _apply_date_format(time_expr, date_display_format)
            order_by_clause = f"ORDER BY {time_expr} ASC"
        elif group_by_parts:
            order_by_clause = f"ORDER BY {group_by_parts[0]}"
        else:
            order_by_clause = ""

        parts = [p for p in [select_clause, from_clause, join_clause, where_clause, group_by_clause, order_by_clause] if p]
        return " ".join(parts)

    except Exception:
        return None


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

        # Find target table for this alias
        target_table_name = next((t for t, a in alias_map.items() if a == alias), None)
        sibling_group = coalesce_map.get(target_table_name) if target_table_name else None

        if sibling_group and len(sibling_group) >= 2:
            subqueries = []
            for source in sibling_group:
                dim_entry = next(
                    (d for i, d in enumerate(required_dims) if f"dim{i+1}" == source["alias"]),
                    None,
                )
                if not dim_entry:
                    continue
                fk_parts = [p for p in (dim_entry.get("factKey") or "").split(".") if p]
                dk_parts = [p for p in (dim_entry.get("dimKey") or "").split(".") if p]
                fk = fk_parts[-1] if fk_parts else ""
                dk = dk_parts[-1] if dk_parts else ""
                if not fk or not dk:
                    continue
                dim_table = quote_identifier(normalize_column_name(dim_entry.get("table") or ""))
                col_expr = f"{source['alias']}.{quote_identifier(source['columnName'])}"
                subqueries.append(
                    f"SELECT {col_expr} AS [key], {col_expr} AS [value] "
                    f"FROM {fact_table} AS {fact_alias} "
                    f"LEFT JOIN {dim_table} AS {source['alias']} "
                    f"ON {fact_alias}.{quote_identifier(fk)} = {source['alias']}.{quote_identifier(dk)} "
                    f"WHERE {col_expr} IS NOT NULL"
                )
            if len(subqueries) >= 2:
                sql = (
                    f"SELECT DISTINCT TOP {int(limit)} [key], [value] "
                    f"FROM ({' UNION '.join(subqueries)}) AS [combined_vals] "
                    f"ORDER BY [value]"
                )
                return {"sql": sql, "keyColumn": key_column, "filteringTier": tier}

        # Single-table query
        quoted_col = f"{alias}.{quote_identifier(col_name)}"
        join_clause = " ".join(join_clauses)
        parts = [
            f"SELECT DISTINCT TOP {int(limit)} {quoted_col} AS [key], {quoted_col} AS [value]",
            f"FROM {fact_table} AS {fact_alias}",
            join_clause,
            f"WHERE {quoted_col} IS NOT NULL",
            "ORDER BY [value]",
        ]
        sql = " ".join(p for p in parts if p)
        return {"sql": sql, "keyColumn": key_column, "filteringTier": tier}

    except Exception:
        return None
