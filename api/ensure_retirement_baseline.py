"""Create the optional retirement source tables on a fresh PostgreSQL install."""

import database.metadata as db


DDL = (
    """CREATE TABLE IF NOT EXISTS ai_providers (
        id TEXT PRIMARY KEY, provider TEXT NOT NULL, display_name TEXT,
        configuration TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
    )""",
    """CREATE TABLE IF NOT EXISTS user_ai_keys (
        id TEXT PRIMARY KEY, user_email TEXT NOT NULL, provider_id TEXT NOT NULL,
        key_reference TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
    )""",
    """CREATE TABLE IF NOT EXISTS dlm_value_index (
        id TEXT PRIMARY KEY, dataset_id TEXT NOT NULL, element_key TEXT NOT NULL,
        value_text TEXT NOT NULL, value_norm TEXT NOT NULL, key_column TEXT,
        key_value TEXT, freq DOUBLE PRECISION DEFAULT 0, source TEXT
    )""",
    "CREATE INDEX IF NOT EXISTS idx_dlm_value_norm ON dlm_value_index (dataset_id, value_norm)",
    "CREATE INDEX IF NOT EXISTS idx_dlm_value_elem ON dlm_value_index (element_key)",
    "CREATE UNIQUE INDEX IF NOT EXISTS ux_dlm_value_uniq ON dlm_value_index (dataset_id, element_key, value_norm)",
)


def main() -> None:
    for statement in DDL:
        db.execute(statement)
    print("retirement baseline schema ready")


if __name__ == "__main__":
    main()
