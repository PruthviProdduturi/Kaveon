"""Versioned, lossless identity for bounded PostgreSQL rollback baselines."""

from __future__ import annotations

import base64
import hashlib
import json
import math
import unicodedata
from datetime import date, datetime, timezone
from decimal import Decimal, InvalidOperation

SCHEMA_VERSION = 1
ENCODING = "kaveon-postgresql-canonical-v1"
MAX_TABLES = 32
MAX_COLUMNS = 256
MAX_ROWS = 10_000
MAX_PAYLOAD_BYTES = 512 * 1024 * 1024
JSON_TYPES = {"json", "jsonb"}
BYTE_TYPES = {"bytea"}
TIMESTAMP_TYPES = {"timestamp with time zone", "timestamptz", "timestamp without time zone", "timestamp"}
NUMERIC_TYPES = {"numeric", "decimal", "real", "double precision"}
INTEGER_TYPES = {"smallint", "integer", "bigint"}


def _json(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False,
                      allow_nan=False).encode("utf-8")


def _sha(value):
    return hashlib.sha256(_json(value)).hexdigest()


def _text(value):
    if not isinstance(value, str):
        raise RuntimeError("PostgreSQL text value has the wrong type")
    value = unicodedata.normalize("NFC", value)
    try:
        value.encode("utf-8")
    except UnicodeEncodeError as error:
        raise RuntimeError("PostgreSQL text value is not valid UTF-8") from error
    return value


def _number(value):
    if isinstance(value, bool):
        raise RuntimeError("PostgreSQL numeric value has the wrong type")
    try:
        number = value if isinstance(value, Decimal) else Decimal(str(value))
    except (InvalidOperation, ValueError) as error:
        raise RuntimeError("PostgreSQL numeric value is invalid") from error
    if not number.is_finite() or (isinstance(value, float) and not math.isfinite(value)):
        raise RuntimeError("PostgreSQL numeric value is not finite")
    if number == 0:
        return "0"
    rendered = format(number, "f")
    if "." in rendered:
        rendered = rendered.rstrip("0").rstrip(".")
    return rendered or "0"


def _normalize_json(value):
    if isinstance(value, dict):
        if any(not isinstance(key, str) for key in value):
            raise RuntimeError("PostgreSQL JSON object key is invalid")
        return {_text(key): _normalize_json(child) for key, child in value.items()}
    if isinstance(value, list):
        return [_normalize_json(child) for child in value]
    if isinstance(value, str):
        return _text(value)
    if value is None or type(value) in {bool, int}:
        return value
    if isinstance(value, (float, Decimal)):
        return ["numeric", _number(value)]
    raise RuntimeError("PostgreSQL JSON value is unsupported")


def encode_value(value, pg_type):
    """Encode one typed cell without conflating null, text, JSON or numerics."""
    pg_type = _text(pg_type).lower()
    if value is None:
        return ["null"]
    if pg_type in JSON_TYPES:
        parsed = json.loads(value) if isinstance(value, str) else value
        return ["json", _normalize_json(parsed)]
    if pg_type in BYTE_TYPES:
        if not isinstance(value, (bytes, bytearray, memoryview)):
            raise RuntimeError("PostgreSQL bytea value has the wrong type")
        return ["bytea", base64.b64encode(bytes(value)).decode("ascii")]
    if pg_type in TIMESTAMP_TYPES:
        instant = value
        if isinstance(instant, str):
            instant = datetime.fromisoformat(instant.replace("Z", "+00:00"))
        if not isinstance(instant, datetime):
            raise RuntimeError("PostgreSQL timestamp value has the wrong type")
        if "with time zone" in pg_type or pg_type == "timestamptz":
            if instant.tzinfo is None or instant.utcoffset() is None:
                raise RuntimeError("PostgreSQL timestamptz value is missing a timezone")
            instant = instant.astimezone(timezone.utc).replace(tzinfo=None)
            suffix = "Z"
        else:
            if instant.tzinfo is not None and instant.utcoffset() is not None:
                raise RuntimeError("PostgreSQL timestamp without timezone is timezone-aware")
            suffix = ""
        return ["timestamp", instant.isoformat(timespec="microseconds") + suffix]
    if pg_type == "date":
        if isinstance(value, str):
            value = date.fromisoformat(value)
        if not isinstance(value, date) or isinstance(value, datetime):
            raise RuntimeError("PostgreSQL date value has the wrong type")
        return ["date", value.isoformat()]
    if pg_type in INTEGER_TYPES:
        if type(value) is not int:
            raise RuntimeError("PostgreSQL integer value has the wrong type")
        return ["integer", str(value)]
    if pg_type in NUMERIC_TYPES:
        return ["numeric", _number(value)]
    if pg_type == "boolean":
        if type(value) is not bool:
            raise RuntimeError("PostgreSQL boolean value has the wrong type")
        return ["boolean", value]
    return ["text", _text(value)]


def table_identity(name, columns, primary_key, rows):
    if (not isinstance(name, str) or not name.isidentifier() or not isinstance(columns, list)
            or not 1 <= len(columns) <= MAX_COLUMNS or not isinstance(primary_key, list)
            or not primary_key or len(primary_key) != len(set(primary_key))):
        raise RuntimeError("PostgreSQL baseline table definition is invalid")
    normalized_columns = []
    for index, column in enumerate(columns, 1):
        if (not isinstance(column, dict) or set(column) != {"name", "type", "nullable", "ordinal"}
                or not isinstance(column["name"], str) or not column["name"].isidentifier()
                or not isinstance(column["type"], str) or not column["type"]
                or type(column["nullable"]) is not bool or column["ordinal"] != index):
            raise RuntimeError("PostgreSQL baseline column definition is invalid")
        normalized_columns.append({**column, "type": column["type"].lower()})
    names = [column["name"] for column in normalized_columns]
    if len(names) != len(set(names)) or any(key not in names for key in primary_key):
        raise RuntimeError("PostgreSQL baseline key definition is invalid")
    if not isinstance(rows, list) or len(rows) > MAX_ROWS:
        raise RuntimeError("PostgreSQL baseline row set is invalid or oversized")
    encoded = []
    for row in rows:
        if not isinstance(row, dict) or set(row) != set(names):
            raise RuntimeError("PostgreSQL baseline row shape is invalid")
        if any(row[column["name"]] is None and not column["nullable"]
               for column in normalized_columns):
            raise RuntimeError("PostgreSQL baseline contains null in a non-nullable column")
        values = [encode_value(row[column["name"]], column["type"])
                  for column in normalized_columns]
        encoded.append(values)
    key_indexes = [names.index(key) for key in primary_key]
    encoded.sort(key=lambda row: _json([row[index] for index in key_indexes]))
    keys = [[row[index] for index in key_indexes] for row in encoded]
    if len({_json(key) for key in keys}) != len(keys):
        raise RuntimeError("PostgreSQL baseline contains duplicate primary keys")
    return {"name": name, "columns": normalized_columns, "primary_key": primary_key,
            "row_count": len(encoded), "rows": encoded,
            "schema_sha256": _sha(normalized_columns), "key_sha256": _sha(keys),
            "content_sha256": _sha(encoded)}


def build(source_id, tables):
    if not isinstance(source_id, str) or not source_id or not isinstance(tables, list):
        raise RuntimeError("PostgreSQL baseline source is invalid")
    if not 1 <= len(tables) <= MAX_TABLES:
        raise RuntimeError("PostgreSQL baseline table set is invalid")
    ordered = sorted(tables, key=lambda table: table["name"])
    if len({table["name"] for table in ordered}) != len(ordered):
        raise RuntimeError("PostgreSQL baseline contains duplicate tables")
    if sum(table["row_count"] for table in ordered) > MAX_ROWS:
        raise RuntimeError("PostgreSQL baseline exceeds its row bound")
    inventory = [{key: table[key] for key in (
        "name", "row_count", "schema_sha256", "key_sha256", "content_sha256")}
        for table in ordered]
    manifest = {"schema_version": SCHEMA_VERSION, "encoding": ENCODING,
                "source_id": source_id, "table_count": len(ordered),
                "row_count": sum(table["row_count"] for table in ordered),
                "tables": inventory, "global_sha256": _sha(inventory)}
    payload = {"manifest": manifest, "tables": ordered}
    if len(_json(payload)) > MAX_PAYLOAD_BYTES:
        raise RuntimeError("PostgreSQL baseline payload exceeds its byte bound")
    validate(payload)
    return payload


def validate(payload):
    if not isinstance(payload, dict) or set(payload) != {"manifest", "tables"}:
        raise RuntimeError("PostgreSQL baseline payload schema is invalid")
    manifest, tables = payload["manifest"], payload["tables"]
    if (not isinstance(manifest, dict) or set(manifest) != {"schema_version", "encoding",
            "source_id", "table_count", "row_count", "tables", "global_sha256"}
            or manifest["schema_version"] != SCHEMA_VERSION or manifest["encoding"] != ENCODING
            or not isinstance(manifest["source_id"], str) or not manifest["source_id"]
            or not isinstance(tables, list) or not 1 <= len(tables) <= MAX_TABLES
            or len(_json(payload)) > MAX_PAYLOAD_BYTES
            or tables != sorted(tables, key=lambda table: table["name"])):
        raise RuntimeError("PostgreSQL baseline manifest is invalid")
    inventory = []
    for table in tables:
        if (not isinstance(table, dict) or set(table) != {"name", "columns", "primary_key",
                "row_count", "rows", "schema_sha256", "key_sha256", "content_sha256"}
                or not isinstance(table["name"], str) or not table["name"].isidentifier()
                or not isinstance(table["columns"], list)
                or not 1 <= len(table["columns"]) <= MAX_COLUMNS
                or not isinstance(table["primary_key"], list) or not table["primary_key"]
                or not isinstance(table["rows"], list) or len(table["rows"]) > MAX_ROWS
                or table["row_count"] != len(table["rows"])):
            raise RuntimeError("PostgreSQL baseline table schema is invalid")
        names = [column.get("name") for column in table["columns"]
                 if isinstance(column, dict)]
        if (len(names) != len(table["columns"]) or len(names) != len(set(names))
                or any(key not in names for key in table["primary_key"])
                or any(not isinstance(row, list) or len(row) != len(names)
                       for row in table["rows"])):
            raise RuntimeError("PostgreSQL baseline table shape is invalid")
        expected = {key: table[key] for key in (
            "name", "row_count", "schema_sha256", "key_sha256", "content_sha256")}
        if (table["schema_sha256"] != _sha(table["columns"])
                or table["content_sha256"] != _sha(table["rows"])):
            raise RuntimeError("PostgreSQL baseline table identity mismatch")
        indexes = [next(index for index, column in enumerate(table["columns"])
                        if column["name"] == key) for key in table["primary_key"]]
        keys = [[row[index] for index in indexes] for row in table["rows"]]
        if table["key_sha256"] != _sha(keys) or keys != sorted(keys, key=_json):
            raise RuntimeError("PostgreSQL baseline key identity mismatch")
        inventory.append(expected)
    if (len({table["name"] for table in tables}) != len(tables)
            or sum(table["row_count"] for table in tables) > MAX_ROWS
            or manifest["tables"] != inventory or manifest["table_count"] != len(tables)
            or manifest["row_count"] != sum(table["row_count"] for table in tables)
            or manifest["global_sha256"] != _sha(inventory)):
        raise RuntimeError("PostgreSQL baseline global identity mismatch")
    return manifest


def require_dataset17_sentinel(payload):
    validate(payload)
    table = next((item for item in payload["tables"] if item["name"] == "dlm_artifact"), None)
    if table is None:
        raise RuntimeError("PostgreSQL baseline omits DLM artifacts")
    names = [column["name"] for column in table["columns"]]
    try: dataset_index, manifest_index = names.index("dataset_id"), names.index("manifest")
    except ValueError as error: raise RuntimeError("PostgreSQL baseline DLM shape is invalid") from error
    row = next((row for row in table["rows"] if row[dataset_index] == ["text", "17"]), None)
    if row is None or row[manifest_index][0] not in {"text", "json"}:
        raise RuntimeError("PostgreSQL baseline omits dataset 17")
    document = (json.loads(row[manifest_index][1]) if row[manifest_index][0] == "text"
                else row[manifest_index][1])
    if document.get("name") != "Climate × Energy":
        raise RuntimeError("PostgreSQL baseline dataset 17 UTF-8 sentinel failed")
    return True


def qualify_restore(source, restored):
    before, after = validate(source), validate(restored)
    require_dataset17_sentinel(source); require_dataset17_sentinel(restored)
    if before != after:
        raise RuntimeError("PostgreSQL baseline restore identity mismatch")
    return {"source_inventory_sha256": before["global_sha256"],
            "restored_inventory_sha256": after["global_sha256"],
            "restored_table_count": after["table_count"], "restore_verified": True}


def backup_identity_observation(source, restored, *, backup_id, restore_job_id,
                                immutable_prefix, manifest_sha256):
    """Return the strict operational backup observation after exact qualification."""
    qualified = qualify_restore(source, restored)
    if (not isinstance(backup_id, str) or not backup_id or not isinstance(restore_job_id, str)
            or not restore_job_id or not isinstance(immutable_prefix, str)
            or f"/backups/{backup_id}/" not in immutable_prefix.rstrip("/") + "/"):
        raise RuntimeError("PostgreSQL baseline backup identity is invalid")
    if (not isinstance(manifest_sha256, str) or len(manifest_sha256) != 64
            or any(character not in "0123456789abcdef" for character in manifest_sha256)):
        raise RuntimeError("PostgreSQL baseline backup manifest digest is invalid")
    backup_sha256 = hashlib.sha256(_json(source)).hexdigest()
    return {"backup_id": backup_id, "backup_sha256": backup_sha256,
            "restore_job_id": restore_job_id,
            "source_inventory_sha256": qualified["source_inventory_sha256"],
            "restored_inventory_sha256": qualified["restored_inventory_sha256"],
            "restored_table_count": qualified["restored_table_count"],
            "immutable_prefix": immutable_prefix, "manifest_sha256": manifest_sha256,
            "restore_executed": True}
