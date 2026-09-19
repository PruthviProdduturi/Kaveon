"""Engine-backed datasets — binding a dataset to a table of the Engine's
durable catalog.

A dataset whose `source` is `{"kind": "engine", "table_id": …}` names one
table by the id the Engine keeps for it. Everything the platform's dataset
model needs is read from the Engine's definition (`GET
/v1/catalog/tables/{id}`) rather than typed by hand:

- the catalog, schema and table names (`database_name`, `schema_name`,
  `table_name`), which is what the existing native-catalog path resolves
  a dataset's statements by;
- the column list with the platform's type names;
- and, when the table declares a shape, the DLM's semantics: every declared
  dimension is a breakdown dimension, every measure is a metric under the
  aggregates it is declared with (`sum`, `count`, `min`, `max` additive;
  `count_distinct` non-additive, answered approximately from the cube's
  sketch), and the time column is the dataset's date column.

Without a shape the columns still bind — text and boolean columns are
dimensions, numeric columns that are not identifiers are summed, the first
date or timestamp column is the date column — and the DLM's questions over
the table take the Engine's row path until a shape is declared. A caller
that sends its own `columns` or `metrics` keeps them; the binding fills what
is missing and never overwrites what was given.
"""
from __future__ import annotations

import re
from typing import Any, Dict, List, Optional

from fastapi import HTTPException

from services import engine_bridge

ADDITIVE_AGGREGATES = ("sum", "count", "min", "max")
NON_ADDITIVE_AGGREGATES = ("count_distinct",)
ROW_COUNT_METRIC = "Rows"

# Arrow type names as the Engine's catalog serializes them → the platform's
# column `data_type` (the SQL spelling SQL Lab and the DLM already read).
_ARROW_TO_SQL = {
    "Boolean": "boolean",
    "Int8": "tinyint", "Int16": "smallint", "Int32": "integer", "Int64": "bigint",
    "UInt8": "tinyint", "UInt16": "smallint", "UInt32": "integer", "UInt64": "bigint",
    "Float16": "real", "Float32": "real", "Float64": "double",
    "Utf8": "varchar", "LargeUtf8": "varchar",
    "Binary": "varbinary", "LargeBinary": "varbinary",
    "Date32": "date", "Date64": "date",
}
_NUMERIC = {"tinyint", "smallint", "integer", "bigint", "real", "double", "decimal"}
_TEMPORAL = {"date", "timestamp"}
_IDENTIFIER_COLUMN = re.compile(r"(?:^|_)(?:id|key|uuid|guid)$", re.IGNORECASE)


def sql_type(arrow: Any) -> str:
    """The platform's spelling of an Engine column type. Parameterized Arrow
    types arrive as one-key objects (`{"Timestamp": ["Microsecond", null]}`,
    `{"Decimal128": [18, 2]}`); anything unknown keeps the Arrow name so it is
    visible rather than silently retyped."""
    if isinstance(arrow, str):
        return _ARROW_TO_SQL.get(arrow, arrow.lower())
    if isinstance(arrow, dict) and len(arrow) == 1:
        name, args = next(iter(arrow.items()))
        if name == "Timestamp":
            return "timestamp"
        if name in ("Decimal128", "Decimal256") and isinstance(args, list) and len(args) == 2:
            return f"decimal({args[0]}, {args[1]})"
        if name == "Dictionary" and isinstance(args, list) and len(args) == 2:
            return sql_type(args[1])
        return name.lower()
    return "unknown"


def _base_type(data_type: str) -> str:
    return data_type.split("(", 1)[0].strip().lower()


def resolve_table(table_id: str, actor: str, role: str) -> Dict[str, Any]:
    """The Engine's definition of one table with its catalog and schema names.
    404 for a table the Engine does not know; 409 for one that is not active,
    because the Engine publishes only active definitions into its snapshot and
    a dataset over a draft would fail every statement."""
    table = engine_bridge.table_definition_by_id(table_id, actor, role)
    if not isinstance(table, dict) or not isinstance(table.get("columns"), list):
        raise HTTPException(404, {"code": "table_not_found", "message": "Engine table definition not found."})
    schema = engine_bridge.schema_definition(str(table.get("schema_id") or ""), actor, role)
    if not isinstance(schema, dict) or not isinstance(schema.get("catalog_id"), str):
        raise HTTPException(404, {"code": "schema_not_found", "message": "Engine schema definition not found."})
    catalog = engine_bridge.catalog_definition(schema["catalog_id"], actor, role)
    if not isinstance(catalog, dict) or not isinstance(catalog.get("name"), str):
        raise HTTPException(404, {"code": "catalog_not_found", "message": "Engine catalog definition not found."})
    if table.get("lifecycle") != "Active":
        raise HTTPException(409, {"code": "table_inactive",
                                  "message": f"Engine table {table.get('name')} is {str(table.get('lifecycle')).lower()}; activate it before binding a dataset."})
    columns = [{"name": str(column.get("name") or ""), "data_type": sql_type(column.get("data_type")),
                "nullable": bool(column.get("nullable", True))}
               for column in table["columns"] if isinstance(column, dict) and column.get("name")]
    shape = table.get("shape") if isinstance(table.get("shape"), dict) else None
    return {
        "table_id": str(table.get("id") or table_id),
        "catalog": catalog["name"], "schema": str(schema.get("name") or ""), "table": str(table.get("name") or ""),
        "revision": table.get("revision"), "format": table.get("format"), "location": table.get("location"),
        "columns": columns, "shape": shape,
    }


def metric_name(column: str, aggregate: str) -> str:
    """The metric a measure becomes, named so the column's own word stays a
    token the DLM matches: a sum is the column itself, the rest carry the
    aggregate in front."""
    return {
        "sum": column,
        "count": f"count of {column}",
        "min": f"min {column}",
        "max": f"max {column}",
        "count_distinct": f"distinct {column}",
    }[aggregate]


def _metric(column: str, aggregate: str) -> Dict[str, Any]:
    expression = {
        "sum": f"SUM({column})", "count": f"COUNT({column})",
        "min": f"MIN({column})", "max": f"MAX({column})",
        "count_distinct": f"COUNT(DISTINCT {column})",
    }[aggregate]
    return {"name": metric_name(column, aggregate), "expression": expression,
            "metric_type": aggregate, "format": None}


def semantics(resolved: Dict[str, Any]) -> Dict[str, Any]:
    """The dataset fields a resolved table implies: columns, metrics and the
    date column, from the declared shape when there is one."""
    table = resolved["table"]
    shape = resolved.get("shape") or {}
    declared_dims = [str(d.get("name")) for d in (shape.get("dimensions") or []) if isinstance(d, dict) and d.get("name")]
    declared_measures: List[Dict[str, Any]] = [m for m in (shape.get("measures") or [])
                                               if isinstance(m, dict) and m.get("column")]
    measure_columns = {str(m["column"]) for m in declared_measures}
    time = shape.get("time") if isinstance(shape.get("time"), dict) else None

    dimensions: List[str] = []
    metrics: List[Dict[str, Any]] = [{"name": ROW_COUNT_METRIC, "expression": "COUNT(*)", "metric_type": "count", "format": None}]
    date_column: Optional[str] = str(time["column"]) if time and time.get("column") else None
    if shape:
        dimensions = declared_dims
        for measure in declared_measures:
            for aggregate in measure.get("aggregates") or []:
                if aggregate in ADDITIVE_AGGREGATES or aggregate in NON_ADDITIVE_AGGREGATES:
                    metrics.append(_metric(str(measure["column"]), str(aggregate)))
    else:
        for column in resolved["columns"]:
            base = _base_type(column["data_type"])
            if base in ("varchar", "boolean"):
                dimensions.append(column["name"])
            elif base in _NUMERIC and not _IDENTIFIER_COLUMN.search(column["name"]):
                metrics.append(_metric(column["name"], "sum"))
                measure_columns.add(column["name"])
            elif base in _TEMPORAL and date_column is None:
                date_column = column["name"]
    columns = [{"table_name": table, "column_name": column["name"], "data_type": column["data_type"],
                "is_dimension": column["name"] in dimensions,
                "is_metric": column["name"] in measure_columns,
                "semantic_type": None}
               for column in resolved["columns"]]
    return {"columns": columns, "metrics": metrics, "date_column": date_column, "dimensions": []}


def apply_binding(payload: Dict[str, Any], actor: str, role: str) -> Dict[str, Any]:
    """Fill a dataset create/update payload from its Engine binding. The names
    always come from the Engine (a dataset cannot name one table and bind to
    another); columns, metrics and the date column only when the caller did
    not send them."""
    source = payload.get("source") or {}
    if source.get("kind") != "engine":
        return payload
    resolved = resolve_table(str(source["table_id"]), actor, role)
    derived = semantics(resolved)
    out = dict(payload)
    out["source"] = {"kind": "engine", "table_id": resolved["table_id"]}
    out["database_name"] = resolved["catalog"]
    out["schema_name"] = resolved["schema"]
    out["table_name"] = resolved["table"]
    out.pop("sql_text", None)
    if not out.get("columns"):
        out["columns"] = derived["columns"]
    if not out.get("metrics"):
        out["metrics"] = derived["metrics"]
    if not out.get("date_column") and derived["date_column"]:
        out["date_column"] = derived["date_column"]
    if out.get("dimensions") is None:
        out["dimensions"] = []
    return out
