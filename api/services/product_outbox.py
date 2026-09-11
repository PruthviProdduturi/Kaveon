"""Durable PostgreSQL source outbox for family-by-family KaveonDB migration."""

import hashlib
import json
import uuid
from dataclasses import dataclass
from typing import Literal, Mapping

from database.metadata import MetadataTransaction


Operation = Literal["create", "update", "delete"]
SUPPORTED_FAMILIES = {"datasets", "charts", "dashboards", "saved_queries", "user_themes"}
MAX_PAYLOAD_BYTES = 16 * 1024 * 1024


@dataclass(frozen=True)
class OutboxEvent:
    event_id: str
    source_sequence: int
    family: str
    operation: Operation
    record_id: str
    payload_sha256: str


def enqueue(
    transaction: MetadataTransaction,
    *,
    family: str,
    operation: Operation,
    record_id: str,
    payload: Mapping,
    actor: str,
    event_id: str | None = None,
) -> OutboxEvent:
    """Append exactly one canonical event inside the caller's source transaction."""
    if family not in SUPPORTED_FAMILIES:
        raise ValueError("unsupported product outbox family")
    if operation not in {"create", "update", "delete"}:
        raise ValueError("unsupported product outbox operation")
    if not record_id or not actor:
        raise ValueError("product outbox record ID and actor are required")
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
          (event_id, family, operation, record_id, payload_json, payload_sha256, actor_principal)
        VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6)
        ON CONFLICT (event_id) DO UPDATE SET event_id = EXCLUDED.event_id
        RETURNING source_sequence, family, operation, record_id, payload_sha256
        """,
        [event_id, family, operation, record_id, canonical, digest, actor],
    )
    if not row:
        raise RuntimeError("PostgreSQL did not return the durable outbox event")
    if (
        row["family"] != family
        or row["operation"] != operation
        or row["record_id"] != record_id
        or row["payload_sha256"] != digest
    ):
        raise ValueError("product outbox event ID was reused with a different request")
    return OutboxEvent(
        event_id=event_id,
        source_sequence=int(row["source_sequence"]),
        family=row["family"],
        operation=row["operation"],
        record_id=row["record_id"],
        payload_sha256=row["payload_sha256"],
    )
