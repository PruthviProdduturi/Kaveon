"""CLI for canonical PostgreSQL baseline capture and isolated qualification."""

import argparse
import json
import os
from datetime import datetime, timezone
from pathlib import Path

from database.pool import get_connection_pool
from services import postgresql_baseline_operator as operator
from services import postgresql_baseline_identity as identity
from services import postgresql_operational_evidence as operational


def _checked_at(value):
    return value or datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _target_connection():
    import psycopg2
    required = {key: os.getenv(key, "") for key in (
        "KAVEON_POSTGRESQL_BASELINE_TARGET_HOST",
        "KAVEON_POSTGRESQL_BASELINE_TARGET_DATABASE",
        "KAVEON_POSTGRESQL_BASELINE_TARGET_USER",
        "KAVEON_POSTGRESQL_BASELINE_TARGET_PASSWORD")}
    if any(not value for value in required.values()):
        raise RuntimeError("complete isolated PostgreSQL target configuration is required")
    connection = psycopg2.connect(
        host=required["KAVEON_POSTGRESQL_BASELINE_TARGET_HOST"],
        port=int(os.getenv("KAVEON_POSTGRESQL_BASELINE_TARGET_PORT", "5432")),
        dbname=required["KAVEON_POSTGRESQL_BASELINE_TARGET_DATABASE"],
        user=required["KAVEON_POSTGRESQL_BASELINE_TARGET_USER"],
        password=required["KAVEON_POSTGRESQL_BASELINE_TARGET_PASSWORD"],
        sslmode=os.getenv("KAVEON_POSTGRESQL_BASELINE_TARGET_SSLMODE", "require"),
        connect_timeout=30)
    connection.autocommit = True
    return connection


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    capture = commands.add_parser("capture")
    capture.add_argument("--source-id", required=True)
    capture.add_argument("--output", required=True, type=Path)
    capture.add_argument("--receipt", required=True, type=Path)
    capture.add_argument("--evidence-id", required=True)
    capture.add_argument("--checked-at")
    restore = commands.add_parser("restore-qualify")
    restore.add_argument("--baseline", required=True, type=Path)
    restore.add_argument("--receipt", required=True, type=Path)
    restore.add_argument("--target-id", required=True)
    restore.add_argument("--evidence-id", required=True)
    restore.add_argument("--checked-at")
    post = commands.add_parser("qualify-post-rollback")
    post.add_argument("--baseline", required=True, type=Path)
    post.add_argument("--receipt", required=True, type=Path)
    post.add_argument("--evidence-id", required=True)
    post.add_argument("--checked-at")
    install = commands.add_parser("install-live")
    install.add_argument("--baseline", required=True, type=Path)
    install.add_argument("--expected-empty-baseline", required=True, type=Path)
    install.add_argument("--isolated-restore-receipt", required=True, type=Path)
    install.add_argument("--write-fence-receipt", required=True, type=Path)
    install.add_argument("--receipt", required=True, type=Path)
    empty = commands.add_parser("derive-empty")
    empty.add_argument("--baseline", required=True, type=Path)
    empty.add_argument("--source-id", required=True)
    empty.add_argument("--output", required=True, type=Path)
    args = parser.parse_args(argv)
    try:
        if args.command == "capture":
            database = os.getenv("METADATA_DATABASE", "")
            if not database:
                raise RuntimeError("METADATA_DATABASE is required")
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
            observation = identity.baseline_observation(payload)
            receipt = operational.receipt_from_observation(
                "postgresql_baseline_identity", observation,
                checked_at=_checked_at(args.checked_at),
                evidence_id=args.evidence_id)
            operator.write_atomic(args.receipt, receipt)
            result = {"captured": True,
                      "global_sha256": payload["manifest"]["global_sha256"],
                      "row_count": payload["manifest"]["row_count"],
                      "table_count": payload["manifest"]["table_count"]}
        elif args.command == "restore-qualify":
            payload = operator.read_payload(args.baseline)
            connection = _target_connection()
            try:
                qualified = operator.restore_and_qualify(connection, payload)
            finally:
                connection.close()
            observation = identity.restore_qualification_observation(
                payload, qualified, args.target_id)
            result = operational.receipt_from_observation(
                "baseline_restore_qualification", observation,
                checked_at=_checked_at(args.checked_at),
                evidence_id=args.evidence_id)
            operator.write_atomic(args.receipt, result)
        elif args.command == "qualify-post-rollback":
            payload = operator.read_payload(args.baseline)
            connection = _target_connection()
            try:
                qualified = operator.qualify_existing(connection, payload)
            finally:
                connection.close()
            observation = identity.post_rollback_observation(payload, qualified)
            result = operational.receipt_from_observation(
                "exact_post_rollback_identity", observation,
                checked_at=_checked_at(args.checked_at),
                evidence_id=args.evidence_id)
            operator.write_atomic(args.receipt, result)
        elif args.command == "derive-empty":
            if args.output.exists():
                raise RuntimeError("refusing to overwrite documented empty baseline")
            result = operator.documented_empty(operator.read_payload(args.baseline), args.source_id)
            operator.write_atomic(args.output, result)
            result = {"derived": True, "baseline_sha256": identity.baseline_sha256(result),
                      "table_count": result["manifest"]["table_count"], "row_count": 0}
        else:
            if args.receipt.exists():
                raise RuntimeError("refusing to overwrite live baseline installation receipt")
            now = datetime.now(timezone.utc)
            payload = operator.read_payload(args.baseline)
            empty = operator.read_payload(args.expected_empty_baseline)
            isolated = operational.load_receipt(args.isolated_restore_receipt,
                "baseline_restore_qualification", now=now, max_age_hours=24,
                max_rollback_seconds=900)
            fence = operational.load_receipt(args.write_fence_receipt, "write_fence",
                now=now, max_age_hours=24, max_rollback_seconds=900)
            database = os.getenv("METADATA_DATABASE", "")
            pool = get_connection_pool(database)
            if not database or pool.db_type != "postgresql":
                raise RuntimeError("live baseline target must be configured PostgreSQL")
            wrapper = pool.get_connection()
            try:
                wrapper.connect()
                result = operator.install_live_baseline(wrapper.connection, payload, empty,
                    isolated, fence["observation"])
            finally:
                pool.return_connection(wrapper)
            operator.write_atomic(args.receipt, result)
        print(json.dumps(result, sort_keys=True, separators=(",", ":")))
        return 0
    except Exception as error:
        print(json.dumps({"passed": False, "error": str(error)}, sort_keys=True,
                         separators=(",", ":")))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
