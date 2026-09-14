"""Contract for safely upgrading the deployed PostgreSQL metadata schema."""
import re
from pathlib import Path

from services import product_outbox


SCHEMA = (Path(__file__).parents[1] / "schema_postgresql.sql").read_text("utf-8")


def _outbox_upgrade() -> str:
    start = SCHEMA.index("ALTER TABLE product_migration_outbox ADD COLUMN IF NOT EXISTS owner_principal")
    end = SCHEMA.index("-- ── Adaptive Context Routing", start)
    return SCHEMA[start:end]


def _simulate_additive_upgrade(columns: set[str], rows: list[dict]) -> None:
    """Apply the additive column/backfill semantics to a legacy catalog image."""
    block = _outbox_upgrade()
    additions = re.findall(
        r"ALTER TABLE product_migration_outbox ADD COLUMN IF NOT EXISTS ([a-z_]+)", block
    )
    for column in additions:
        if column in columns:
            continue
        columns.add(column)
        default = 0 if column == "apply_attempts" else None
        for row in rows:
            row[column] = default
    for row in rows:
        if row.get("owner_principal") is None:
            row["owner_principal"] = row["actor_principal"]


def test_legacy_outbox_upgrade_is_idempotent_and_preserves_events():
    columns = {
        "source_sequence", "event_id", "family", "operation", "record_id",
        "payload_json", "payload_sha256", "actor_principal", "created_at", "applied_at",
    }
    rows = [
        {"source_sequence": 7, "event_id": "event-7", "family": "query_history",
         "record_id": "query-7", "payload_sha256": "a" * 64,
         "actor_principal": "owner@example.com", "applied_at": None},
        {"source_sequence": 8, "event_id": "event-8", "family": "dlm_runs",
         "record_id": "24-v3", "payload_sha256": "b" * 64,
         "actor_principal": "owner@example.com", "applied_at": "2026-09-14"},
    ]
    immutable = [{key: row[key] for key in (
        "source_sequence", "event_id", "family", "record_id", "payload_sha256", "applied_at"
    )} for row in rows]
    _simulate_additive_upgrade(columns, rows)
    once = [dict(row) for row in rows]
    _simulate_additive_upgrade(columns, rows)
    assert rows == once
    assert [{key: row[key] for key in immutable[0]} for row in rows] == immutable
    assert all(row["owner_principal"] == row["actor_principal"] for row in rows)
    assert all(row["apply_attempts"] == 0 for row in rows)
    assert {"owner_principal", "target_generation", "apply_attempts", "last_error_code"} <= columns


def test_outbox_family_constraint_and_indices_match_runtime_contract():
    block = _outbox_upgrade()
    constraints = re.findall(r"ck_product_outbox_family CHECK\s*\(family IN \((.*?)\)\)", block, re.S)
    assert constraints
    schema_families = {item.strip(" '\n\r") for item in constraints[-1].split(",")}
    assert schema_families == product_outbox.SUPPORTED_FAMILIES
    assert "idx_product_outbox_unapplied" in block
    assert "idx_product_outbox_family_unapplied" in block
    assert not re.search(r"\b(?:DELETE|TRUNCATE|DROP TABLE)\b", block, re.I)


def test_query_history_and_dlm_artifact_upgrades_are_additive():
    for statement in (
        "ALTER TABLE query_history ADD COLUMN IF NOT EXISTS engine_query_id",
        "ALTER TABLE query_history ADD COLUMN IF NOT EXISTS engine_details",
        "ALTER TABLE dlm_artifact ADD COLUMN IF NOT EXISTS version",
        "ALTER TABLE dlm_artifact ADD COLUMN IF NOT EXISTS curation",
    ):
        assert statement in SCHEMA
    assert "'dlm_runs','query_history'" in SCHEMA
