"""The two spellings of the DLM's assembled SQL: PostgreSQL (the warehouse)
and the Engine (a lake table). One router resolves a question to its slots
— metric, breakdown, filters, time window, ranking — and one assembler
writes the statement; the dialect only decides how identifiers, literals and
time predicates are spelled. Nothing here parses a question.

What differs, and why (checked against `engine/crates/sql/src/logical_plan.rs`
and `engine/crates/exec/src/expr_eval.rs`):

- **Identifiers.** PostgreSQL takes every identifier ANSI-quoted. The Engine's
  SQL front end reads a column identifier's value (quoted or not) but keeps a
  table reference's quotes as part of the name it looks up, so a relation is
  written bare and a column is quoted only when it is not a plain identifier
  or is a reserved word.
- **Dates.** PostgreSQL coerces `'2026-01-01'` against a date column; the
  Engine coerces a text literal against a date column on the readers but
  has no `TIMESTAMP` cast, so a date column takes `DATE '…'` literals (its
  day number, compared as integers), a timestamp column takes `EXTRACT`
  (`YEAR`/`MONTH` for a calendar slot, `EPOCH` for a window), an integer
  year column takes equality, and a text date column keeps ISO string
  comparison, which orders correctly.
- **Relative time.** PostgreSQL gets `CURRENT_DATE` arithmetic; the Engine
  gets the window's resolved bounds, since the DLM already knows the day it
  is asking about and the bounds make the statement reproducible.
- **`LIMIT`/`OFFSET`, `GROUP BY`, `ORDER BY`, string escaping** are the same.
"""
from __future__ import annotations

import re
from datetime import date, datetime, timezone
from typing import Any, Dict, List, Optional, Sequence

_SIMPLE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
# Words the Engine's parser takes as keywords when they stand bare in a
# projection or predicate; quoting keeps them column references.
_RESERVED = frozenset({
    "all", "and", "any", "as", "asc", "between", "by", "case", "cast", "count", "cross", "current_date",
    "current_timestamp", "date", "day", "default", "delete", "desc", "distinct", "else", "end", "except",
    "exists", "extract", "false", "filter", "from", "full", "group", "having", "hour", "in", "inner",
    "insert", "intersect", "interval", "into", "is", "join", "left", "like", "limit", "minute", "month",
    "natural", "not", "null", "offset", "on", "or", "order", "outer", "over", "partition", "right",
    "select", "set", "some", "table", "then", "time", "timestamp", "to", "true", "union", "update",
    "user", "using", "values", "when", "where", "with", "year",
})
_INTEGER_TYPES = ("smallint", "integer", "int", "int2", "int4", "int8", "bigint", "tinyint", "numeric")


def _base_type(dtype: Optional[str]) -> str:
    return (dtype or "").split("(", 1)[0].strip().lower()


def _column_type(column: str, columns: Sequence[dict]) -> str:
    for c in columns or []:
        if (c.get("column_name") or c.get("name") or "") == column:
            return _base_type(c.get("data_type"))
    return ""


def _epoch_seconds(day: str) -> int:
    return int(datetime.strptime(day, "%Y-%m-%d").replace(tzinfo=timezone.utc).timestamp())


class Dialect:
    name = "postgresql"

    # ── identifiers and literals ─────────────────────────────────────────
    def ident(self, name: str) -> str:
        return '"' + str(name).replace('"', '""') + '"'

    def relation(self, schema: Optional[str], table: str) -> str:
        return f"{self.ident(schema)}.{self.ident(table)}" if schema else self.ident(table)

    def literal(self, value: Any) -> str:
        return "'" + str(value).replace("'", "''") + "'"

    def expression(self, expr: str) -> str:
        """A stored metric expression as the source reads it."""
        return expr

    # ── time ─────────────────────────────────────────────────────────────
    def year_predicate(self, column: str, year: int, columns: Sequence[dict]) -> str:
        if self._integer_year(column, columns):
            return f"{self.ident(column)} = {int(year)}"
        return self.window_predicate(column, f"{int(year)}-01-01", f"{int(year) + 1}-01-01", columns)

    def window_predicate(self, column: str, lo: Optional[str], hi: Optional[str],
                         columns: Sequence[dict]) -> str:
        """`[lo, hi)` as ISO days; `hi` None leaves the window open."""
        parts = []
        if lo:
            parts.append(f"{self.ident(column)} >= {self.literal(lo)}")
        if hi:
            parts.append(f"{self.ident(column)} < {self.literal(hi)}")
        return " AND ".join(parts)

    def relative_predicate(self, column: str, relative_time: str, lo: Optional[str], hi: Optional[str],
                           columns: Sequence[dict]) -> str:
        """`relative_time` is the router's PostgreSQL expression (a quoted ISO
        day, `CURRENT_DATE`, or `CURRENT_DATE - INTERVAL …`); `lo`/`hi` are
        the same window resolved to days."""
        return f"{self.ident(column)} >= {relative_time}"

    def _integer_year(self, column: str, columns: Sequence[dict]) -> bool:
        return column.lower() == "year" or _column_type(column, columns) in _INTEGER_TYPES


class EngineDialect(Dialect):
    name = "engine"

    def ident(self, name: str) -> str:
        text = str(name)
        if _SIMPLE.match(text) and text.lower() not in _RESERVED:
            return text
        return '"' + text.replace('"', '""') + '"'

    def relation(self, schema: Optional[str], table: str) -> str:
        # The Engine looks a relation up by the text as written, quotes
        # included; catalog names are plain identifiers.
        return f"{schema}.{table}" if schema else str(table)

    def expression(self, expr: str) -> str:
        return re.sub(r'"([A-Za-z_][A-Za-z0-9_]*)"', r"\1", expr or "")

    def year_predicate(self, column: str, year: int, columns: Sequence[dict]) -> str:
        if self._integer_year(column, columns):
            return f"{self.ident(column)} = {int(year)}"
        if _column_type(column, columns).startswith("timestamp"):
            return f"EXTRACT(YEAR FROM {self.ident(column)}) = {int(year)}"
        return self.window_predicate(column, f"{int(year)}-01-01", f"{int(year) + 1}-01-01", columns)

    def window_predicate(self, column: str, lo: Optional[str], hi: Optional[str],
                         columns: Sequence[dict]) -> str:
        kind = _column_type(column, columns)
        parts = []
        if kind.startswith("timestamp"):
            if lo:
                parts.append(f"EXTRACT(EPOCH FROM {self.ident(column)}) >= {_epoch_seconds(lo)}")
            if hi:
                parts.append(f"EXTRACT(EPOCH FROM {self.ident(column)}) < {_epoch_seconds(hi)}")
        elif kind == "date":
            if lo:
                parts.append(f"{self.ident(column)} >= DATE {self.literal(lo)}")
            if hi:
                parts.append(f"{self.ident(column)} < DATE {self.literal(hi)}")
        else:
            if lo:
                parts.append(f"{self.ident(column)} >= {self.literal(lo)}")
            if hi:
                parts.append(f"{self.ident(column)} < {self.literal(hi)}")
        return " AND ".join(parts)

    def relative_predicate(self, column: str, relative_time: str, lo: Optional[str], hi: Optional[str],
                           columns: Sequence[dict]) -> str:
        if lo is None:
            lo = date.today().isoformat()
        return self.window_predicate(column, lo, hi, columns)


POSTGRESQL = Dialect()
ENGINE = EngineDialect()


def assemble(dialect: Dialect, *, schema: Optional[str], fact: str, metric_expr: str, metric_name: str,
             group_cols: Sequence[str], time_group: Optional[str], filters: Sequence[Dict[str, Any]],
             columns: Sequence[dict], date_column: Optional[str] = None, year: Optional[int] = None,
             month_window: Optional[tuple] = None, relative_time: Optional[str] = None,
             relative_window: Optional[tuple] = None, limit_n: Optional[int] = None,
             sort_asc: bool = False, ranked: bool = True) -> str:
    """The statement for one resolved question. `ranked=False` writes a grouped
    statement without `ORDER BY`/`LIMIT` — the shape the Engine's cube answers —
    and leaves the ordering to the caller."""
    select_parts: List[str] = []
    if time_group:
        select_parts.append(dialect.ident(time_group))
    select_parts.extend(dialect.ident(c) for c in group_cols)
    select_parts.append(f"{dialect.expression(metric_expr)} AS {dialect.ident(metric_name)}")

    where: List[str] = []
    for f in filters:
        where.append(f"{dialect.ident(f['column'])} = {dialect.literal(f['value'])}")
    if year and month_window and date_column:
        where.append(dialect.window_predicate(date_column, month_window[0], month_window[1], columns))
    elif year and date_column:
        where.append(dialect.year_predicate(date_column, year, columns))
    elif relative_time and date_column:
        lo, hi = relative_window or (None, None)
        where.append(dialect.relative_predicate(date_column, relative_time, lo, hi, columns))

    sql = f"SELECT {', '.join(select_parts)} FROM {dialect.relation(schema, fact)}"
    if where:
        sql += " WHERE " + " AND ".join(where)
    if time_group:
        gb = ", ".join([dialect.ident(time_group)] + [dialect.ident(c) for c in group_cols])
        sql += f" GROUP BY {gb} ORDER BY {dialect.ident(time_group)} LIMIT 1000"
    elif group_cols:
        sql += f" GROUP BY {', '.join(dialect.ident(c) for c in group_cols)}"
        if ranked:
            order_dir = "ASC" if sort_asc else "DESC"
            sql += f" ORDER BY {dialect.ident(metric_name)} {order_dir} LIMIT {int(limit_n or 50)}"
    return sql
