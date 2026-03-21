"""Datasets service — port of datasets.service.ts."""

import json
import re
import pyodbc
from datetime import datetime, timezone
from typing import List, Optional
import database.metadata as db

VALID_VISIBILITY = {"private", "internal", "published"}


def _expand_columns_from_dimensions(columns: list, dimensions: list) -> list:
    if not dimensions:
        return columns

    dimension_columns = []

    for index, dim in enumerate(dimensions):
        table_name = (dim.get("dimension_table") or "").lower()
        fact_key_original = dim.get("fact_key") or ""
        fact_key_lower = fact_key_original.lower()

        if not table_name or not fact_key_lower:
            continue

        semantic_type = fact_key_original
        if fact_key_lower.endswith("key"):
            semantic_type = fact_key_original[:-3]
        elif fact_key_lower.endswith("id"):
            semantic_type = fact_key_original[:-2]

        existing_col = next(
            (c for c in columns
             if (c.get("table_name") or "").lower() == table_name and bool(c.get("is_dimension"))),
            None
        )

        if existing_col:
            entry = {**existing_col, "semantic_type": semantic_type, "fact_key": fact_key_original, "dimension_index": index}
            dimension_columns.append(entry)
        else:
            join_condition = dim.get("join_condition") or ""
            match = re.search(r"\[([^\]]+)\]\s*$", join_condition)
            column_name = match.group(1) if match else "Value"
            dimension_columns.append({
                "table_name": dim.get("dimension_table"),
                "column_name": column_name,
                "data_type": "varchar(8000)",
                "is_dimension": True,
                "is_metric": False,
                "semantic_type": semantic_type,
                "fact_key": fact_key_original,
                "dimension_index": index,
            })

    non_dimension_columns = [c for c in columns if not c.get("is_dimension")]
    return dimension_columns + non_dimension_columns


def _adapt(row: dict) -> dict:
    name = row.get("dataset_name")
    sql_text = None
    if row.get("tables_used"):
        try:
            tu = json.loads(row["tables_used"])
            sql_text = tu.get("sql_text")
        except Exception:
            pass
    return {
        "id": str(row["id"]),
        "name": name,
        "dataset_name": name,
        "description": row.get("description"),
        "table_name": row.get("fact_table"),
        "schema_name": row.get("schema_name"),
        "database_name": row.get("database_name"),
        "date_column": row.get("date_column"),
        "sql_text": sql_text,
        "tables_used": row.get("tables_used"),
        "visibility": row.get("visibility") or "internal",
        "created_at": row.get("created_at"),
        "updated_at": row.get("modified_at"),
        "created_by": row.get("created_by"),
        "modified_by": row.get("modified_by"),
        "modified_at": row.get("modified_at"),
        "favorite": row.get("favorite") == 1,
    }


# ── Visibility SQL fragment ────────────────────────────────────────────────────
# Params: @paramR = role string, @paramE = user email
# Replace R and E with actual param indices when building queries.

def _vis_clause(role_idx: int, email_idx: int, alias: str = "d") -> str:
    return (
        f"({alias}.visibility = 'published' "
        f"OR ({alias}.visibility = 'internal' AND @param{role_idx} IN ('Analyst', 'Editor', 'Admin')) "
        f"OR ({alias}.visibility = 'private' AND {alias}.created_by = @param{email_idx}) "
        f"OR @param{role_idx} = 'Admin')"
    )


def list_datasets(user_email: str, role: str = "Viewer") -> List[dict]:
    vis = _vis_clause(1, 0)
    result = db.query(f"""
        SELECT DISTINCT
          d.id, d.dataset_name, d.description, d.fact_table, d.schema_name,
          d.database_name, d.created_at, d.modified_at, d.date_column,
          d.tables_used, d.created_by, d.modified_by, d.visibility,
          CASE WHEN MAX(f.id) IS NOT NULL THEN 1 ELSE 0 END as favorite
        FROM dbo.datasets d
        LEFT JOIN dbo.favorites f
          ON f.object_id = CAST(d.id AS NVARCHAR(255))
          AND f.object_type = 'dataset' AND f.user_email = @param0
        WHERE d.id IS NOT NULL AND {vis}
        GROUP BY d.id, d.dataset_name, d.description, d.fact_table, d.schema_name,
                 d.database_name, d.created_at, d.modified_at, d.date_column,
                 d.tables_used, d.created_by, d.modified_by, d.visibility
        ORDER BY d.modified_at DESC
    """, [user_email, role])
    return [_adapt(r) for r in result["rows"]]


def get_dataset_by_id(
    dataset_id: str,
    user_email: Optional[str] = None,
    role: str = "Admin",
) -> Optional[dict]:
    """
    Fetch a dataset by ID.
    role defaults to 'Admin' for internal service calls (e.g. chart rendering)
    that should not be blocked by visibility.  Pass the actual user role when
    called from a user-facing endpoint.
    """
    did = int(dataset_id)

    if user_email:
        vis = _vis_clause(2, 1)
        row = db.query_one(f"""
            SELECT d.id, d.dataset_name, d.description, d.fact_table, d.schema_name,
                   d.database_name, d.created_at, d.modified_at, d.date_column,
                   d.tables_used, d.created_by, d.modified_by, d.visibility,
                   CASE WHEN f.user_email IS NOT NULL THEN 1 ELSE 0 END as favorite
            FROM dbo.datasets d
            LEFT JOIN dbo.favorites f
              ON CAST(d.id AS NVARCHAR(255)) = f.object_id
              AND f.user_email = @param1 AND f.object_type = 'dataset'
            WHERE d.id = @param0 AND d.id IS NOT NULL AND {vis}
        """, [did, user_email, role])
    else:
        row = db.query_one("""
            SELECT id, dataset_name, description, fact_table, schema_name,
                   database_name, created_at, modified_at, date_column,
                   tables_used, created_by, modified_by, visibility, 0 as favorite
            FROM dbo.datasets WHERE id = @param0 AND id IS NOT NULL
        """, [did])

    if not row:
        return None

    dataset = _adapt(row)

    try:
        dims_result = db.query("""
            SELECT dimension_table, table_name, join_condition,
                   fact_key, join_key, dim_name, display_name
            FROM dbo.dataset_dimensions WHERE dataset_id = @param0
        """, [did])
        dataset["dimensions"] = dims_result["rows"]
    except Exception:
        dataset["dimensions"] = []

    try:
        cols_result = db.query("""
            SELECT table_name, column_name, data_type, is_dimension, is_metric, semantic_type
            FROM dbo.dataset_columns WHERE dataset_id = @param0
        """, [did])
        raw_cols = cols_result["rows"]
    except Exception:
        raw_cols = []

    dataset["columns"] = _expand_columns_from_dimensions(raw_cols, dataset["dimensions"])

    try:
        metrics_result = db.query("""
            SELECT metric_name as name, expression, metric_type, format
            FROM dbo.dataset_metrics WHERE dataset_id = @param0
        """, [did])
        dataset["metrics"] = metrics_result["rows"]
    except Exception:
        dataset["metrics"] = []

    filters = []
    if row.get("tables_used"):
        try:
            parsed = json.loads(row["tables_used"])
            if isinstance(parsed, dict) and isinstance(parsed.get("filters"), list):
                filters = parsed["filters"]
        except Exception:
            pass
    dataset["filters"] = filters

    return dataset


def create_dataset(data: dict, user_id: str) -> dict:
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    tu_payload: dict = {"filters": data.get("filters") or []}
    if data.get("sql_text"):
        tu_payload["sql_text"] = data["sql_text"]
    tables_used = json.dumps(tu_payload)

    visibility = data.get("visibility") or "internal"
    if visibility not in VALID_VISIBILITY:
        visibility = "internal"

    base_name = data["name"]
    name = base_name
    for attempt in range(1, 100):
        try:
            db.execute("""
                INSERT INTO datasets (
                  dataset_name, description, fact_table, schema_name, database_name,
                  date_column, tables_used, visibility, created_at, modified_at, created_by, modified_by
                ) VALUES (
                  @param0, @param1, @param2, @param3, @param4,
                  @param5, @param6, @param7, @param8, @param9, @param10, @param11
                )
            """, [
                name, data.get("description"), data.get("table_name") or "",
                data.get("schema_name", "dbo"), data.get("database_name", "IDEASServingStoreLH"),
                data.get("date_column"), tables_used, visibility,
                now, now, user_id, user_id,
            ])
            break
        except pyodbc.IntegrityError:
            name = f"{base_name} ({attempt})"

    inserted = db.query_one(
        "SELECT TOP 1 id FROM datasets WHERE dataset_name = @param0 AND created_by = @param1 ORDER BY id DESC",
        [name, user_id],
    )
    if not inserted:
        raise RuntimeError("Failed to retrieve created dataset")

    dataset_id = inserted["id"]

    for dim in (data.get("dimensions") or []):
        parts = (dim.get("dimension_table") or "").split(".")
        dim_table = parts[1] if len(parts) > 1 else parts[0] if parts else ""
        join_match = re.search(r"\[([^\]]+)\]\.?\s*=\s*.*\[([^\]]+)\]\s*$", dim.get("join_condition") or "")
        fact_key = join_match.group(1) if join_match else ""
        join_key = join_match.group(2) if join_match else "Key"
        db.execute("""
            INSERT INTO dataset_dimensions (
              dataset_id, dimension_table, table_name, join_condition,
              fact_key, join_key, dim_name, display_name, created_at
            ) VALUES (
              @param0, @param1, @param2, @param3,
              @param4, @param5, @param6, @param7, GETUTCDATE()
            )
        """, [dataset_id, dim.get("dimension_table"), dim_table, dim.get("join_condition"),
              fact_key, join_key, dim_table, dim.get("display_name") or dim_table])

    for col in (data.get("columns") or []):
        db.execute("""
            INSERT INTO dataset_columns (
              dataset_id, table_name, column_name, data_type,
              is_dimension, is_metric, semantic_type
            ) VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6)
        """, [dataset_id, col.get("table_name") or "", col.get("column_name") or "", col.get("data_type") or "",
              1 if col.get("is_dimension") else 0,
              1 if col.get("is_metric") else 0,
              col.get("semantic_type")])

    try:
        for metric in (data.get("metrics") or []):
            db.execute("""
                INSERT INTO dataset_metrics (dataset_id, metric_name, expression, metric_type, format)
                VALUES (@param0, @param1, @param2, @param3, @param4)
            """, [dataset_id, metric.get("name"), metric.get("expression"),
                  metric.get("metric_type", "sum"), metric.get("format")])
    except Exception:
        pass

    created = get_dataset_by_id(str(dataset_id))
    if not created:
        raise RuntimeError("Failed to retrieve created dataset")
    return created


def update_dataset(dataset_id: str, data: dict, user_id: str) -> Optional[dict]:
    if not get_dataset_by_id(dataset_id):
        return None

    now = datetime.now(timezone.utc).replace(tzinfo=None)
    updates, params, i = [], [], 0
    did = int(dataset_id)

    for field_name, col in [("name", "dataset_name"), ("description", "description"),
                             ("table_name", "fact_table"), ("schema_name", "schema_name"),
                             ("database_name", "database_name"), ("date_column", "date_column")]:
        if field_name in data:
            updates.append(f"{col} = @param{i}"); params.append(data[field_name]); i += 1

    if "visibility" in data:
        vis = data["visibility"] if data["visibility"] in VALID_VISIBILITY else "internal"
        updates.append(f"visibility = @param{i}"); params.append(vis); i += 1

    if "filters" in data:
        updates.append(f"tables_used = @param{i}")
        params.append(json.dumps({"filters": data["filters"]})); i += 1

    updates.append(f"modified_at = @param{i}"); params.append(now); i += 1
    updates.append(f"modified_by = @param{i}"); params.append(user_id); i += 1
    params.append(did)

    db.execute(f"UPDATE datasets SET {', '.join(updates)} WHERE id = @param{i}", params)

    if "dimensions" in data and isinstance(data["dimensions"], list):
        db.execute("DELETE FROM dataset_dimensions WHERE dataset_id = @param0", [did])
        for dim in data["dimensions"]:
            parts = (dim.get("dimension_table") or "").split(".")
            dim_table = parts[1] if len(parts) > 1 else parts[0] if parts else ""
            join_match = re.search(r"\[([^\]]+)\]\.?\s*=\s*.*\[([^\]]+)\]\s*$", dim.get("join_condition") or "")
            fact_key = join_match.group(1) if join_match else ""
            join_key = join_match.group(2) if join_match else "Key"
            db.execute("""
                INSERT INTO dataset_dimensions (
                  dataset_id, dimension_table, table_name, join_condition,
                  fact_key, join_key, dim_name, display_name, created_at
                ) VALUES (
                  @param0, @param1, @param2, @param3,
                  @param4, @param5, @param6, @param7, GETUTCDATE()
                )
            """, [did, dim.get("dimension_table"), dim_table, dim.get("join_condition"),
                  fact_key, join_key, dim_table, dim.get("display_name") or dim_table])

    if "columns" in data and isinstance(data["columns"], list):
        db.execute("DELETE FROM dataset_columns WHERE dataset_id = @param0", [did])
        for col in data["columns"]:
            db.execute("""
                INSERT INTO dataset_columns (
                  dataset_id, table_name, column_name, data_type,
                  is_dimension, is_metric, semantic_type
                ) VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6)
            """, [did, col.get("table_name"), col.get("column_name"), col.get("data_type"),
                  1 if col.get("is_dimension") else 0,
                  1 if col.get("is_metric") else 0,
                  col.get("semantic_type")])

    if "metrics" in data and isinstance(data["metrics"], list):
        try:
            db.execute("DELETE FROM dataset_metrics WHERE dataset_id = @param0", [did])
            for metric in data["metrics"]:
                db.execute("""
                    INSERT INTO dataset_metrics (dataset_id, metric_name, expression, metric_type, format)
                    VALUES (@param0, @param1, @param2, @param3, @param4)
                """, [did, metric.get("name"), metric.get("expression"),
                      metric.get("metric_type", "sum"), metric.get("format")])
        except Exception:
            pass

    return get_dataset_by_id(dataset_id)


def delete_dataset(dataset_id: str) -> bool:
    return db.execute("DELETE FROM datasets WHERE id = @param0", [int(dataset_id)]) > 0


def count_datasets() -> int:
    result = db.query_one("SELECT COUNT(*) as count FROM datasets")
    return result.get("count") or 0
