"""Durable PostgreSQL source outbox for family-by-family KaveonDB migration."""

import hashlib
import json
import uuid
from dataclasses import dataclass
from typing import Literal, Mapping

from database.metadata import MetadataTransaction
import database.metadata as db


Operation = Literal["create", "update", "delete"]
SUPPORTED_FAMILIES = {
    "datasets", "charts", "dashboards", "saved_queries", "user_themes", "user_recents", "favorites", "catalog_sources", "data_sources", "activity", "dlm_definitions",
}
MAX_PAYLOAD_BYTES = 16 * 1024 * 1024


@dataclass(frozen=True)
class OutboxEvent:
    event_id: str
    source_sequence: int
    family: str
    operation: Operation
    record_id: str
    payload_sha256: str
    actor_principal: str
    owner_principal: str


def pending(limit: int = 50) -> list[dict]:
    """Read the oldest unapplied events; source order is the replay contract."""
    if not 1 <= limit <= 100:
        raise ValueError("product outbox read limit must be between 1 and 100")
    return db.query("""
        SELECT source_sequence, event_id, family, operation, record_id,
               payload_json, payload_sha256, actor_principal, owner_principal,
               created_at, apply_attempts
        FROM product_migration_outbox
        WHERE applied_at IS NULL
        ORDER BY source_sequence
        LIMIT @param0
    """, [limit])["rows"]


def status(event_id: str) -> dict | None:
    """Read bounded replay state for one exact source event."""
    try:
        event_id = str(uuid.UUID(event_id))
    except (AttributeError, TypeError, ValueError):
        raise ValueError("product outbox event ID must be a UUID") from None
    return db.query_one("""
        SELECT event_id, source_sequence, family, operation, record_id,
               payload_sha256, owner_principal, applied_at, target_generation,
               apply_attempts, last_error_code
        FROM product_migration_outbox
        WHERE event_id = @param0
    """, [event_id])


def mark_applied(event_id: str, payload_sha256: str, target_generation: int | None) -> bool:
    """Acknowledge a target commit only if the locked source event is unchanged."""
    with db.transaction() as transaction:
        row = transaction.query_one("""
            SELECT payload_sha256, applied_at
            FROM product_migration_outbox
            WHERE event_id = @param0
            FOR UPDATE
        """, [event_id])
        if not row:
            raise RuntimeError("Product outbox event disappeared before acknowledgment")
        if row["payload_sha256"] != payload_sha256:
            raise RuntimeError("Product outbox event changed before acknowledgment")
        if row.get("applied_at") is not None:
            return False
        changed = transaction.execute("""
            UPDATE product_migration_outbox
            SET applied_at = NOW(), target_generation = @param1,
                apply_attempts = apply_attempts + 1, last_error_code = NULL
            WHERE event_id = @param0 AND applied_at IS NULL
        """, [event_id, target_generation])
        if changed != 1:
            raise RuntimeError("Product outbox acknowledgment lost its row lock")
        return True


def record_failure(event_id: str, payload_sha256: str, error_code: str) -> None:
    """Persist a bounded failure classification without advancing source order."""
    if not error_code or len(error_code) > 100 or not error_code.replace("_", "").isalnum():
        raise ValueError("product outbox error code is invalid")
    with db.transaction() as transaction:
        row = transaction.query_one("""
            SELECT payload_sha256, applied_at
            FROM product_migration_outbox
            WHERE event_id = @param0
            FOR UPDATE
        """, [event_id])
        if not row or row["payload_sha256"] != payload_sha256:
            raise RuntimeError("Product outbox event changed before failure recording")
        if row.get("applied_at") is not None:
            return
        transaction.execute("""
            UPDATE product_migration_outbox
            SET apply_attempts = apply_attempts + 1, last_error_code = @param1
            WHERE event_id = @param0 AND applied_at IS NULL
        """, [event_id, error_code])


def enqueue(
    transaction: MetadataTransaction,
    *,
    family: str,
    operation: Operation,
    record_id: str,
    payload: Mapping,
    actor: str,
    owner: str | None = None,
    event_id: str | None = None,
) -> OutboxEvent:
    """Append exactly one canonical event inside the caller's source transaction."""
    if family not in SUPPORTED_FAMILIES:
        raise ValueError("unsupported product outbox family")
    if operation not in {"create", "update", "delete"}:
        raise ValueError("unsupported product outbox operation")
    owner = owner or actor
    if not record_id or not actor or not owner:
        raise ValueError("product outbox record ID, actor, and owner are required")
    event_id = event_id or str(uuid.uuid4())
    try:
        event_id = str(uuid.UUID(event_id))
    except (AttributeError, TypeError, ValueError):
        raise ValueError("product outbox event ID must be a UUID") from None
    canonical = json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    if len(canonical.encode("utf-8")) > MAX_PAYLOAD_BYTES:
        raise ValueError("product outbox payload exceeds the transaction limit")
    digest = hashlib.sha256(canonical.encode("utf-8")).hexdigest()
    row = transaction.query_one(
        """
        INSERT INTO product_migration_outbox
          (event_id, family, operation, record_id, payload_json, payload_sha256,
           actor_principal, owner_principal)
        VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6, @param7)
        ON CONFLICT (event_id) DO UPDATE SET event_id = EXCLUDED.event_id
        RETURNING source_sequence, family, operation, record_id, payload_sha256,
                  actor_principal, owner_principal
        """,
        [event_id, family, operation, record_id, canonical, digest, actor, owner],
    )
    if not row:
        raise RuntimeError("PostgreSQL did not return the durable outbox event")
    if (
        row["family"] != family
        or row["operation"] != operation
        or row["record_id"] != record_id
        or row["payload_sha256"] != digest
        or row["actor_principal"] != actor
        or row["owner_principal"] != owner
    ):
        raise ValueError("product outbox event ID was reused with a different request")
    return OutboxEvent(
        event_id=event_id,
        source_sequence=int(row["source_sequence"]),
        family=row["family"],
        operation=row["operation"],
        record_id=row["record_id"],
        payload_sha256=row["payload_sha256"],
        actor_principal=row["actor_principal"],
        owner_principal=row["owner_principal"],
    )
