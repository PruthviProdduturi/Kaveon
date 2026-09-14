"""Bounded, read-only PostgreSQL watermark and migration-outbox observations."""

import hashlib
import json

import database.metadata as db


def collect() -> dict:
    with db.transaction() as transaction:
        transaction.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        row = transaction.query_one("""
            SELECT txid_current()::bigint AS query_id,
                   txid_current_snapshot() AS source_snapshot,
                   COALESCE(MAX(source_sequence), 0)::bigint AS watermark,
                   COUNT(*) FILTER (WHERE applied_at IS NULL)::bigint AS pending_events
            FROM product_migration_outbox
        """) or {}
    query_id = row.get("query_id")
    watermark = row.get("watermark")
    pending = row.get("pending_events")
    snapshot = row.get("source_snapshot")
    if (type(query_id) is not int or query_id <= 0 or type(watermark) is not int or
            watermark < 0 or type(pending) is not int or pending < 0 or
            not isinstance(snapshot, str) or not snapshot):
        raise RuntimeError("PostgreSQL source-state probe returned invalid values")
    identity = hashlib.sha256(json.dumps(
        {"source_snapshot": snapshot, "watermark": watermark},
        sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    return {"query_id": query_id, "source_snapshot": identity,
            "watermark": watermark, "pending_events": pending}
