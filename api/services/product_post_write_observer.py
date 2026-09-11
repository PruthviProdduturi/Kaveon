"""Default-off, read-only dataset outbox/target verification telemetry."""

import os

from services import product_outbox, product_shadow_read, product_store


def observe_dataset(event: product_outbox.OutboxEvent) -> dict:
    if os.getenv("KAVEON_DATASET_POST_WRITE_VERIFY_ENABLED") != "true":
        return {"family": "datasets", "enabled": False, "status": "disabled"}
    if event.family != "datasets":
        raise RuntimeError("dataset post-write observer received another family")
    row = product_outbox.status(event.event_id)
    base = {
        "family": "datasets", "enabled": True, "event_id": event.event_id,
        "source_sequence": event.source_sequence, "operation": event.operation,
        "record_id": event.record_id, "payload_sha256": event.payload_sha256,
    }
    if row is None:
        return {**base, "status": "outbox_missing"}
    identities = ("event_id", "source_sequence", "family", "operation", "record_id", "payload_sha256", "owner_principal")
    if any(str(row.get(name)) != str(getattr(event, name)) for name in identities):
        return {**base, "status": "outbox_divergence"}
    if row.get("applied_at") is None:
        return {
            **base, "status": "pending_replay",
            "apply_attempts": int(row.get("apply_attempts") or 0),
            "last_error_code": row.get("last_error_code"),
        }
    target = product_store.read("dataset", event.record_id, event.owner_principal, "Admin")
    if event.operation == "delete":
        return {
            **base, "status": "verified" if target is None else "target_divergence",
            "target_generation": None if target is None else int(target.get("generation") or 0),
        }
    if target is None or not isinstance(target.get("document"), dict):
        return {**base, "status": "target_divergence", "target_sha256": None}
    target_sha, target_bytes = product_shadow_read._identity(target["document"])
    return {
        **base,
        "status": "verified" if target_sha == event.payload_sha256 else "target_divergence",
        "target_sha256": target_sha, "target_bytes": target_bytes,
        "target_generation": int(target.get("generation") or 0),
    }
