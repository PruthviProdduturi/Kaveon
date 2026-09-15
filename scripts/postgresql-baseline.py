"""Capture or restore-qualify the seven-table PostgreSQL rollback baseline."""

import argparse
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from database.pool import get_connection_pool  # noqa: E402
from services import postgresql_baseline_operator as operator  # noqa: E402


def connect(dsn):
    import psycopg2
    connection = psycopg2.connect(dsn, connect_timeout=30)
    connection.autocommit = True
    return connection


parser = argparse.ArgumentParser(description=__doc__)
subparsers = parser.add_subparsers(dest="command", required=True)
capture = subparsers.add_parser("capture")
capture.add_argument("--source-id", required=True)
capture.add_argument("--output", required=True, type=Path)
restore = subparsers.add_parser("restore-qualify")
restore.add_argument("--baseline", required=True, type=Path)
restore.add_argument("--receipt", required=True, type=Path)
restore.add_argument("--target-id", required=True)
args = parser.parse_args()

try:
    if args.command == "capture":
        database = os.getenv("METADATA_DATABASE", "")
        if not database: raise RuntimeError("METADATA_DATABASE is required")
        pool = get_connection_pool(database)
        if pool.db_type != "postgresql":
            raise RuntimeError("PostgreSQL baseline source must be PostgreSQL")
        wrapper = pool.get_connection()
        try:
            wrapper.connect()
            payload = operator.capture(wrapper.connection, args.source_id)
        finally:
            pool.return_connection(wrapper)
        operator.write_atomic(args.output, payload)
        result = {"captured": True, "global_sha256": payload["manifest"]["global_sha256"],
                  "row_count": payload["manifest"]["row_count"],
                  "table_count": payload["manifest"]["table_count"]}
    else:
        dsn = os.getenv("KAVEON_POSTGRESQL_BASELINE_TARGET_DATABASE", "")
        if not dsn: raise RuntimeError("KAVEON_POSTGRESQL_BASELINE_TARGET_DATABASE is required")
        payload = operator.read_payload(args.baseline)
        connection = connect(dsn)
        try: result = operator.restore_and_qualify(connection, payload)
        finally: connection.close()
        result = operator.restore_receipt(payload, result, args.target_id)
        operator.write_atomic(args.receipt, result)
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
except Exception as error:
    print(json.dumps({"passed": False, "error": str(error)}, sort_keys=True,
                     separators=(",", ":")))
    raise SystemExit(1)
