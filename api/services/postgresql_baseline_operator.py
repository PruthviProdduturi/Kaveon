"""Bounded capture and isolated restore operator for PostgreSQL rollback baselines."""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path

from services import postgresql_baseline_identity as identity
from services import postgresql_special_family_retirement as retirement

TABLES = (*retirement.CONTEXT_TABLES, *retirement.DLM_TABLES)
MAX_FILE_BYTES = identity.MAX_PAYLOAD_BYTES


def _identifier(value: str) -> str:
    if not isinstance(value, str) or not value.isidentifier():
        raise RuntimeError("PostgreSQL baseline contains an unsafe identifier")
    return f'"{value}"'


def discover(cursor, table: str):
    """Discover ordered columns and the ordered primary key for one table."""
    _identifier(table)
    cursor.execute(
        "SELECT column_name,data_type,is_nullable,ordinal_position "
        "FROM information_schema.columns WHERE table_schema=current_schema() "
        "AND table_name=%s ORDER BY ordinal_position", (table,))
    columns = [{"name": row[0], "type": row[1], "nullable": row[2] == "YES",
                "ordinal": int(row[3])} for row in cursor.fetchall()]
    cursor.execute(
        "SELECT a.attname FROM pg_index i "
        "JOIN pg_class c ON c.oid=i.indrelid "
        "JOIN pg_namespace n ON n.oid=c.relnamespace "
        "JOIN unnest(i.indkey) WITH ORDINALITY k(attnum,position) ON true "
        "JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum=k.attnum "
        "WHERE n.nspname=current_schema() AND c.relname=%s AND i.indisprimary "
        "ORDER BY k.position", (table,))
    primary_key = [row[0] for row in cursor.fetchall()]
    if not columns or not primary_key:
        raise RuntimeError(f"PostgreSQL baseline table or primary key is missing: {table}")
    return columns, primary_key


def capture_cursor(cursor, source_id: str):
    tables = []
    remaining = identity.MAX_ROWS
    for name in TABLES:
        columns, primary_key = discover(cursor, name)
        cursor.execute("SELECT " + ",".join(_identifier(item["name"]) for item in columns)
                       + f" FROM {_identifier(name)} LIMIT %s", (remaining + 1,))
        rows = cursor.fetchall()
        if len(rows) > remaining:
            raise RuntimeError("PostgreSQL baseline exceeds its row bound")
        remaining -= len(rows)
        tables.append(identity.table_identity(name, columns, primary_key,
            [dict(zip((item["name"] for item in columns), row)) for row in rows]))
    return identity.build(source_id, tables)


def capture(connection, source_id: str):
    """Capture all seven tables in one read-only repeatable-read snapshot."""
    if connection.autocommit is not True:
        raise RuntimeError("PostgreSQL baseline connection already has a transaction")
    connection.autocommit = False
    cursor = connection.cursor()
    try:
        cursor.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        payload = capture_cursor(cursor, source_id)
        identity.require_dataset17_sentinel(payload)
        connection.commit()
        return payload
    except Exception:
        connection.rollback()
        raise
    finally:
        cursor.close()
        connection.autocommit = True


def decode_value(cell, pg_type):
    if not isinstance(cell, list) or not cell:
        raise RuntimeError("PostgreSQL baseline cell is invalid")
    tag = cell[0]
    if tag == "null" and cell == ["null"]:
        return None
    if len(cell) != 2:
        raise RuntimeError("PostgreSQL baseline cell is invalid")
    value = cell[1]
    if tag == "json":
        return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    if tag == "bytea":
        import base64
        return base64.b64decode(value, validate=True)
    if tag == "integer":
        return int(value)
    if tag == "numeric":
        from decimal import Decimal
        return Decimal(value)
    if tag == "boolean" and type(value) is bool:
        return value
    if tag == "date":
        from datetime import date
        return date.fromisoformat(value)
    if tag == "timestamp":
        from datetime import datetime
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    if tag == "text" and isinstance(value, str):
        return value
    raise RuntimeError(f"PostgreSQL baseline cell does not match {pg_type}")


def restore_and_qualify(connection, payload):
    """Restore only into empty, schema-identical tables and qualify before commit."""
    identity.validate(payload)
    if connection.autocommit is not True:
        raise RuntimeError("PostgreSQL baseline target already has a transaction")
    by_name = {item["name"]: item for item in payload["tables"]}
    if set(by_name) != set(TABLES):
        raise RuntimeError("PostgreSQL baseline does not contain the exact seven tables")
    connection.autocommit = False
    cursor = connection.cursor()
    try:
        cursor.execute("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        cursor.execute("LOCK TABLE " + ",".join(_identifier(name) for name in TABLES)
                       + " IN ACCESS EXCLUSIVE MODE")
        for name in TABLES:
            columns, primary_key = discover(cursor, name)
            expected = by_name[name]
            if columns != expected["columns"] or primary_key != expected["primary_key"]:
                raise RuntimeError(f"PostgreSQL baseline target schema differs: {name}")
            cursor.execute(f"SELECT COUNT(*) FROM {_identifier(name)}")
            if cursor.fetchone()[0] != 0:
                raise RuntimeError(f"PostgreSQL baseline target is not empty: {name}")
        for name in TABLES:
            table = by_name[name]
            columns = table["columns"]
            statement = (f"INSERT INTO {_identifier(name)} (" +
                ",".join(_identifier(item["name"]) for item in columns) + ") VALUES (" +
                ",".join(["%s"] * len(columns)) + ")")
            for row in table["rows"]:
                cursor.execute(statement, tuple(decode_value(cell, column["type"])
                    for cell, column in zip(row, columns)))
        restored = capture_cursor(cursor, payload["manifest"]["source_id"])
        result = identity.qualify_restore(payload, restored)
        connection.commit()
        return result
    except Exception:
        connection.rollback()
        raise
    finally:
        cursor.close()
        connection.autocommit = True


def qualify_existing(connection, payload):
    """Read an existing rollback target and compare its exact canonical identity."""
    restored = capture(connection, payload["manifest"]["source_id"])
    return identity.qualify_restore(payload, restored)


def read_payload(path: Path):
    if not path.is_file() or path.stat().st_size > MAX_FILE_BYTES:
        raise RuntimeError("PostgreSQL baseline input is missing or oversized")
    return json.loads(path.read_text(encoding="utf-8"))


def write_atomic(path: Path, value):
    data = json.dumps(value, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=False).encode("utf-8")
    if len(data) > MAX_FILE_BYTES:
        raise RuntimeError("PostgreSQL baseline output is oversized")
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(dir=path.parent, prefix=path.name + ".",
                                               suffix=".tmp")
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(data); stream.flush(); os.fsync(stream.fileno())
        os.replace(temporary, path)
    except Exception:
        try: os.unlink(temporary)
        except FileNotFoundError: pass
        raise
