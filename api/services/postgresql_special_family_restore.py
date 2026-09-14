"""Fail-closed restoration of generated PostgreSQL state during rollback."""

import hashlib
import json
import os
import time

from database.pool import get_connection_pool
from services import postgresql_special_family_retirement as retirement
from services.postgresql_write_fence import enabled as fence_enabled

SCHEMA_VERSION = 1
MAX_ROWS = 10_000
TABLES = (*retirement.CONTEXT_TABLES, *retirement.DLM_TABLES)


def _canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=False).encode("utf-8")


def _digest(value):
    if (not isinstance(value, str) or len(value) != 64
            or any(character not in "0123456789abcdef" for character in value)):
        raise RuntimeError("special-family restore digest is invalid")
    return value


def _expected(deletion):
    try:
        context = deletion["context"]
        dlm = deletion["dlm"]
        counts = {
            "context_answer_cache": context["source"]["cache_rows"],
            "context_snapshots": context["source"]["snapshot_rows"],
            **dlm["source"]["rows"],
        }
        schemas = {
            "context_answer_cache": context["source"]["cache_schema_sha256"],
            "context_snapshots": context["source"]["snapshot_schema_sha256"],
            **dlm["source"]["schema_sha256"],
        }
        snapshot = context["source"]["snapshot_id"]
    except (KeyError, TypeError) as error:
        raise RuntimeError("special-family deletion evidence is incomplete") from error
    if (set(counts) != set(TABLES) or set(schemas) != set(TABLES)
            or any(type(count) is not int or count < 0 for count in counts.values())
            or any(_digest(value) != value for value in schemas.values())
            or snapshot != dlm["source"].get("snapshot_id")):
        raise RuntimeError("special-family deletion evidence does not identify one source")
    return counts, schemas, snapshot


def validate_bundle(bundle, deletion, expected_sha256):
    _digest(expected_sha256)
    if not isinstance(bundle, dict) or set(bundle) != {
            "schema_version", "source_id", "tables", "bundle_sha256"}:
        raise RuntimeError("special-family restore bundle schema is invalid")
    unsigned = {key: value for key, value in bundle.items() if key != "bundle_sha256"}
    actual = hashlib.sha256(_canonical(unsigned)).hexdigest()
    if bundle["schema_version"] != SCHEMA_VERSION or bundle["bundle_sha256"] != actual:
        raise RuntimeError("special-family restore bundle failed integrity validation")
    if actual != expected_sha256:
        raise RuntimeError("special-family restore bundle does not match reviewed evidence")
    counts, schemas, snapshot = _expected(deletion)
    if bundle["source_id"] != snapshot or not isinstance(bundle["tables"], list):
        raise RuntimeError("special-family restore source identity does not match deletion")
    found = {}
    for item in bundle["tables"]:
        if (not isinstance(item, dict) or set(item) != {
                "table", "columns", "rows", "row_count", "rows_sha256", "schema_sha256"}
                or item["table"] in found or item["table"] not in TABLES
                or not isinstance(item["columns"], list) or not item["columns"]
                or len(item["columns"]) != len(set(item["columns"]))
                or any(not isinstance(column, str) or not column.isidentifier()
                       for column in item["columns"])
                or not isinstance(item["rows"], list)
                or any(not isinstance(row, list) or len(row) != len(item["columns"])
                       for row in item["rows"])):
            raise RuntimeError("special-family restore table is invalid")
        if (item["row_count"] != len(item["rows"])
                or item["row_count"] != counts[item["table"]]
                or _digest(item["schema_sha256"]) != schemas[item["table"]]
                or _digest(item["rows_sha256"]) != hashlib.sha256(
                    _canonical(item["rows"])).hexdigest()):
            raise RuntimeError("special-family restore table does not match deletion evidence")
        found[item["table"]] = item
    if set(found) != set(TABLES) or sum(counts.values()) > MAX_ROWS:
        raise RuntimeError("special-family restore bundle is incomplete or oversized")
    return found, actual


def _restore_transactionally(tables, expected_schemas):
    pool = get_connection_pool(os.getenv("METADATA_DATABASE", ""))
    if pool.db_type != "postgresql":
        raise RuntimeError("special-family rollback requires PostgreSQL")
    connection = pool.get_connection()
    try:
        connection.connect(); raw = connection.connection
        if not raw.autocommit:
            raise RuntimeError("special-family restore connection already has a transaction")
        raw.autocommit = False; cursor = raw.cursor()
        try:
            cursor.execute("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
            cursor.execute("LOCK TABLE " + ",".join(f'\"{table}\"' for table in TABLES)
                           + " IN ACCESS EXCLUSIVE MODE")
            current = retirement._counts(cursor, TABLES)
            if any(current.values()):
                raise RuntimeError("refusing to overwrite nonempty special-family source tables")
            for table in TABLES:
                if retirement._schema_digest(cursor, table) != expected_schemas[table]:
                    raise RuntimeError("special-family target schema does not match deletion evidence")
                item = tables[table]
                columns = ",".join(f'\"{column}\"' for column in item["columns"])
                placeholders = ",".join(["%s"] * len(item["columns"]))
                statement = f'INSERT INTO "{table}" ({columns}) VALUES ({placeholders})'
                for row in item["rows"]:
                    cursor.execute(statement, tuple(row))
            after = retirement._counts(cursor, TABLES)
            if after != {table: tables[table]["row_count"] for table in TABLES}:
                raise RuntimeError("special-family restored counts did not reconcile")
            raw.commit()
        except Exception:
            raw.rollback(); raise
        finally:
            cursor.close(); raw.autocommit = True
        return after
    finally:
        pool.return_connection(connection)


def restore(bundle, deletion, *, expected_bundle_sha256, cutover_revision,
            max_operations=10_000, max_duration_seconds=900, runner=None, clock=time.monotonic):
    if os.getenv("KAVEON_SPECIAL_FAMILY_RESTORE_ENABLED") != "true" or not fence_enabled():
        raise RuntimeError("special-family restore requires explicit enablement and write fence")
    if not isinstance(cutover_revision, str) or not cutover_revision:
        raise RuntimeError("special-family restore requires the exact cutover revision")
    tables, _ = validate_bundle(bundle, deletion, expected_bundle_sha256)
    operations = sum(item["row_count"] for item in tables.values())
    if (type(max_operations) is not int or not 1 <= max_operations <= MAX_ROWS
            or operations > max_operations or type(max_duration_seconds) is not int
            or not 1 <= max_duration_seconds <= 900):
        raise RuntimeError("special-family restore exceeds its rollback bound")
    _, schemas, _ = _expected(deletion)
    started = clock()
    restored = (runner or _restore_transactionally)(tables, schemas)
    duration = clock() - started
    if duration < 0 or duration > max_duration_seconds:
        raise RuntimeError("special-family restore exceeded its duration bound")
    if restored != {table: tables[table]["row_count"] for table in TABLES}:
        raise RuntimeError("special-family restore verification failed")
    return {"cutover_revision": cutover_revision,
            "rollback_operation_count": operations}
