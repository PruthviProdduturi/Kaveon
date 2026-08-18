-- ============================================================
-- Lens Production Database Schema — PostgreSQL
-- ============================================================
-- Run this script once against your metadata database.
-- All statements are idempotent (safe to re-run).
-- ============================================================

CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- ── Datasets ──────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS datasets (
    id            SERIAL        PRIMARY KEY,
    dataset_name  VARCHAR(500)  NOT NULL,
    description   TEXT          NULL,
    fact_table    VARCHAR(500)  NOT NULL DEFAULT '',
    schema_name   VARCHAR(255)  NOT NULL DEFAULT 'public',
    database_name VARCHAR(255)  NULL,
    date_column   VARCHAR(255)  NULL,
    tables_used   TEXT          NULL,
    visibility    VARCHAR(20)   NOT NULL DEFAULT 'internal'
                  CONSTRAINT ck_datasets_visibility CHECK (visibility IN ('private','internal','published')),
    created_at    TIMESTAMP     NOT NULL DEFAULT NOW(),
    modified_at   TIMESTAMP     NOT NULL DEFAULT NOW(),
    created_by    VARCHAR(255)  NOT NULL DEFAULT 'system',
    modified_by   VARCHAR(255)  NULL
);
CREATE INDEX IF NOT EXISTS idx_datasets_dataset_name  ON datasets(dataset_name);
CREATE INDEX IF NOT EXISTS idx_datasets_database_name ON datasets(database_name);
CREATE INDEX IF NOT EXISTS idx_datasets_visibility    ON datasets(visibility);
CREATE INDEX IF NOT EXISTS idx_datasets_modified_at   ON datasets(modified_at);

-- ── Dataset Dimensions ────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS dataset_dimensions (
    id              SERIAL       PRIMARY KEY,
    dataset_id      INTEGER      NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    dimension_table VARCHAR(500) NULL,
    table_name      VARCHAR(255) NULL,
    join_condition  TEXT         NULL,
    fact_key        VARCHAR(255) NULL,
    join_key        VARCHAR(255) NULL,
    dim_name        VARCHAR(255) NULL,
    display_name    VARCHAR(255) NULL,
    created_at      TIMESTAMP    NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_dataset_dimensions_dataset_id ON dataset_dimensions(dataset_id);

-- ── Dataset Columns ───────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS dataset_columns (
    id            SERIAL       PRIMARY KEY,
    dataset_id    INTEGER      NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    table_name    VARCHAR(255) NOT NULL DEFAULT '',
    column_name   VARCHAR(255) NOT NULL DEFAULT '',
    data_type     VARCHAR(255) NULL,
    is_dimension  BOOLEAN      NOT NULL DEFAULT FALSE,
    is_metric     BOOLEAN      NOT NULL DEFAULT FALSE,
    semantic_type VARCHAR(255) NULL
);
CREATE INDEX IF NOT EXISTS idx_dataset_columns_dataset_id ON dataset_columns(dataset_id);

-- ── Dataset Metrics ───────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS dataset_metrics (
    id          SERIAL       PRIMARY KEY,
    dataset_id  INTEGER      NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    metric_name VARCHAR(255) NOT NULL,
    expression  TEXT         NULL,
    metric_type VARCHAR(50)  NOT NULL DEFAULT 'sum',
    format      VARCHAR(255) NULL
);
CREATE INDEX IF NOT EXISTS idx_dataset_metrics_dataset_id ON dataset_metrics(dataset_id);

-- ── Charts ────────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS charts (
    id          VARCHAR(36)  NOT NULL PRIMARY KEY,
    name        VARCHAR(255) NOT NULL,
    description TEXT         NULL,
    dataset_id  INTEGER      NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    chart_type  VARCHAR(50)  NOT NULL,
    config      TEXT         NOT NULL DEFAULT '{}',
    visibility  VARCHAR(20)  NOT NULL DEFAULT 'internal'
                CONSTRAINT ck_charts_visibility CHECK (visibility IN ('private','internal','published')),
    thumbnail      TEXT      NULL,
    thumbnail_dark TEXT      NULL,
    created_at  TIMESTAMP    NOT NULL DEFAULT NOW(),
    modified_at TIMESTAMP    NOT NULL DEFAULT NOW(),
    created_by  VARCHAR(255) NOT NULL DEFAULT 'system',
    modified_by VARCHAR(255) NULL
);
CREATE INDEX IF NOT EXISTS idx_charts_dataset_id  ON charts(dataset_id);
CREATE INDEX IF NOT EXISTS idx_charts_name        ON charts(name);
CREATE INDEX IF NOT EXISTS idx_charts_visibility  ON charts(visibility);
CREATE INDEX IF NOT EXISTS idx_charts_modified_at ON charts(modified_at);

-- ── Dashboards ────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS dashboards (
    id           VARCHAR(36)  NOT NULL PRIMARY KEY,
    name         VARCHAR(255) NOT NULL,
    slug         VARCHAR(255) NULL,
    description  TEXT         NULL,
    layout       TEXT         NOT NULL DEFAULT '[]',
    charts       TEXT         NOT NULL DEFAULT '[]',
    filters      TEXT         NOT NULL DEFAULT '[]',
    theme        TEXT         NULL,
    tags         TEXT         NULL,
    visibility   VARCHAR(20)  NOT NULL DEFAULT 'internal'
                 CONSTRAINT ck_dashboards_visibility CHECK (visibility IN ('private','internal','published')),
    is_published BOOLEAN      NOT NULL DEFAULT FALSE,
    is_archived  BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at   TIMESTAMP    NOT NULL DEFAULT NOW(),
    modified_at  TIMESTAMP    NOT NULL DEFAULT NOW(),
    created_by   VARCHAR(255) NOT NULL DEFAULT 'system',
    modified_by  VARCHAR(255) NULL
);
CREATE INDEX IF NOT EXISTS idx_dashboards_name        ON dashboards(name);
CREATE INDEX IF NOT EXISTS idx_dashboards_slug        ON dashboards(slug);
CREATE INDEX IF NOT EXISTS idx_dashboards_visibility  ON dashboards(visibility);
CREATE INDEX IF NOT EXISTS idx_dashboards_modified_at ON dashboards(modified_at);

-- ── Saved Queries ─────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS saved_queries (
    id            VARCHAR(36)  NOT NULL PRIMARY KEY,
    name          VARCHAR(255) NOT NULL,
    description   TEXT         NULL,
    sql           TEXT         NOT NULL,
    database_name VARCHAR(255) NULL,
    created_at    TIMESTAMP    NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMP    NOT NULL DEFAULT NOW(),
    created_by    VARCHAR(255) NOT NULL DEFAULT 'system'
);
CREATE INDEX IF NOT EXISTS idx_saved_queries_name       ON saved_queries(name);
CREATE INDEX IF NOT EXISTS idx_saved_queries_created_by ON saved_queries(created_by);

-- ── Query History ─────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS query_history (
    id             VARCHAR(36)  NOT NULL PRIMARY KEY,
    sql_text       TEXT         NOT NULL,
    database_name  VARCHAR(255) NULL,
    executed_at    TIMESTAMP    NOT NULL DEFAULT NOW(),
    execution_time FLOAT        NOT NULL DEFAULT 0,
    row_count      INTEGER      NOT NULL DEFAULT 0,
    status         VARCHAR(20)  NOT NULL DEFAULT 'success',
    error_message  TEXT         NULL,
    user_email     VARCHAR(255) NOT NULL DEFAULT 'system',
    trigger_source VARCHAR(50)  NULL,
    dataset_id     VARCHAR(36)  NULL,
    tables_used    TEXT         NULL
);
CREATE INDEX IF NOT EXISTS idx_query_history_executed_at ON query_history(executed_at);
CREATE INDEX IF NOT EXISTS idx_query_history_user_email  ON query_history(user_email);
CREATE INDEX IF NOT EXISTS idx_query_history_status      ON query_history(status);

-- ── Favorites ─────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS favorites (
    id          VARCHAR(36)  NOT NULL PRIMARY KEY DEFAULT gen_random_uuid()::VARCHAR,
    user_email  VARCHAR(255) NOT NULL,
    object_type VARCHAR(50)  NOT NULL,
    object_id   VARCHAR(36)  NOT NULL,
    object_name VARCHAR(255) NOT NULL,
    created_at  TIMESTAMP    NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_favorites_user_email  ON favorites(user_email);
CREATE INDEX IF NOT EXISTS idx_favorites_object_type ON favorites(object_type);
CREATE INDEX IF NOT EXISTS idx_favorites_user_object ON favorites(user_email, object_id, object_type);

-- ── Activity Log ──────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS activity (
    id          VARCHAR(36)  NOT NULL PRIMARY KEY DEFAULT gen_random_uuid()::VARCHAR,
    action      VARCHAR(50)  NOT NULL,
    object_type VARCHAR(50)  NOT NULL,
    object_id   VARCHAR(36)  NOT NULL,
    object_name VARCHAR(255) NOT NULL,
    timestamp   TIMESTAMP    NOT NULL DEFAULT NOW(),
    user_email  VARCHAR(255) NOT NULL DEFAULT 'system',
    details     TEXT         NULL
);
CREATE INDEX IF NOT EXISTS idx_activity_timestamp   ON activity(timestamp);
CREATE INDEX IF NOT EXISTS idx_activity_user_email  ON activity(user_email);
CREATE INDEX IF NOT EXISTS idx_activity_object_type ON activity(object_type);

-- ── Data Sources ──────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS data_sources (
    id                SERIAL       PRIMARY KEY,
    name              VARCHAR(255) NOT NULL,
    type              VARCHAR(100) NOT NULL,
    connection_string VARCHAR(1000) NOT NULL,
    database_name     VARCHAR(255) NULL,
    region            VARCHAR(10)  NOT NULL DEFAULT 'WW'
                      CONSTRAINT ck_data_sources_region CHECK (region IN ('WW','EU')),
    description       TEXT         NULL,
    created_by        VARCHAR(255) NULL,
    created_at        TIMESTAMP    NOT NULL DEFAULT NOW(),
    updated_at        TIMESTAMP    NOT NULL DEFAULT NOW(),
    is_active         BOOLEAN      NOT NULL DEFAULT TRUE,
    CONSTRAINT uq_data_sources_name UNIQUE (name)
);
CREATE INDEX IF NOT EXISTS ix_data_sources_region    ON data_sources(region);
CREATE INDEX IF NOT EXISTS ix_data_sources_is_active ON data_sources(is_active);

-- ── User Themes ───────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS user_themes (
    user_email  VARCHAR(255) NOT NULL PRIMARY KEY,
    theme_color VARCHAR(7)   NOT NULL DEFAULT '#6366f1',
    created_at  TIMESTAMP    NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMP    NOT NULL DEFAULT NOW()
);

-- ── Auth Config ───────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS auth_config (
    id                   SERIAL       PRIMARY KEY,
    provider             VARCHAR(50)  NOT NULL DEFAULT 'local',
    azure_tenant_id      VARCHAR(255) NULL,
    azure_client_id      VARCHAR(255) NULL,
    google_client_id     VARCHAR(255) NULL,
    google_client_secret VARCHAR(1000) NULL,
    jwt_secret           VARCHAR(1000) NULL,
    updated_at           TIMESTAMP    NOT NULL DEFAULT NOW(),
    updated_by           VARCHAR(255) NULL
);

-- ── Local Users ───────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS local_users (
    id                    SERIAL       PRIMARY KEY,
    username              VARCHAR(255) NOT NULL,
    email                 VARCHAR(255) NOT NULL,
    password_hash         VARCHAR(255) NOT NULL,
    role                  VARCHAR(20)  NOT NULL DEFAULT 'Viewer'
                          CONSTRAINT ck_local_users_role CHECK (role IN ('Viewer','Analyst','Editor','Admin')),
    force_password_change BOOLEAN      NOT NULL DEFAULT FALSE,
    is_active             BOOLEAN      NOT NULL DEFAULT TRUE,
    created_at            TIMESTAMP    NOT NULL DEFAULT NOW(),
    updated_at            TIMESTAMP    NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_local_users_username UNIQUE (username),
    CONSTRAINT uq_local_users_email    UNIQUE (email)
);
CREATE INDEX IF NOT EXISTS idx_local_users_username ON local_users(username);
CREATE INDEX IF NOT EXISTS idx_local_users_email    ON local_users(email);

-- ── User Recents ──────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS user_recents (
    id         SERIAL        PRIMARY KEY,
    user_email VARCHAR(255)  NOT NULL,
    item_id    VARCHAR(255)  NOT NULL,
    label      VARCHAR(500)  NOT NULL,
    href       VARCHAR(1000) NOT NULL,
    type       VARCHAR(50)   NOT NULL,
    created_at TIMESTAMP     NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_user_recents_email_item UNIQUE (user_email, item_id)
);
CREATE INDEX IF NOT EXISTS idx_user_recents_email ON user_recents(user_email);

-- ── Adaptive Context Routing (staleness-scored NL query router) ────────────────
-- Global context representation: one row per profiled table/column element.
-- Populated from pg_stats + pg_stat_user_tables (no LLM, no data scan). The
-- change counters captured here are what let the validity engine detect drift
-- without re-querying the underlying data.
CREATE TABLE IF NOT EXISTS context_snapshots (
    id                TEXT             PRIMARY KEY,
    database_name     TEXT             NOT NULL,
    schema_name       TEXT             NOT NULL,
    table_name        TEXT             NOT NULL,
    column_name       TEXT,                       -- NULL => table-level element
    element_key       TEXT             NOT NULL,   -- "schema.table" or "schema.table.column"
    element_type      TEXT             NOT NULL,   -- 'table' | 'column'
    profile           TEXT,                        -- JSON: distribution / relationship stats
    row_count         BIGINT,                      -- n_live_tup at capture
    mods_at_capture   BIGINT,                      -- n_mod_since_analyze at capture
    last_analyze      TEXT,                        -- source last_analyze at capture (ISO)
    usage_count       BIGINT           DEFAULT 0,  -- query_history hits (usage-weighted decay)
    captured_at       TEXT             NOT NULL,
    refreshed_at      TEXT             NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_context_snapshots_elem
    ON context_snapshots (database_name, element_key);

-- Result cache keyed by question signature + the element dependencies the answer
-- relied on. Unlike a flat TTL cache, an entry is served only while every
-- dependency's validity score stays above the routing threshold.
CREATE TABLE IF NOT EXISTS context_answer_cache (
    id                TEXT             PRIMARY KEY,
    database_name     TEXT             NOT NULL,
    question_sig      TEXT             NOT NULL,
    sql_hash          TEXT,
    result            TEXT,                        -- JSON: cached result set
    element_deps      TEXT,                        -- JSON: [element_key, ...]
    created_at        TEXT             NOT NULL,
    last_served_at    TEXT             NOT NULL,
    serve_count       BIGINT           DEFAULT 0,
    validity_at_serve DOUBLE PRECISION
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_context_answer_sig
    ON context_answer_cache (database_name, question_sig);

-- ── DLM: per-dataset compiled context artifact (Data Language Model) ──────────
-- Compiled ON TOP of context_snapshots: the retrievable manifest + value index
-- that resolves NL terms to columns/filters with no hosted LLM, no data scan.
-- Self-migrates via services/dlm.py; kept here for schema parity. ``codes``
-- columns are reserved for the v2 discrete-code (RQ-VAE) upgrade.
CREATE TABLE IF NOT EXISTS dlm_artifact (
    dataset_id   TEXT             PRIMARY KEY,
    version      INTEGER          NOT NULL DEFAULT 1,
    manifest     TEXT             NOT NULL,   -- JSON: columns, join graph, grain, metrics, synonyms
    stats_rollup TEXT,                        -- JSON: cardinalities / row counts / freshness
    usage_rollup TEXT,                        -- JSON: per-table usage from query_history
    embeds       TEXT,                        -- v1 reserved: packed dense element vectors
    codes        TEXT,                        -- v2 reserved: discrete codes
    source_hash  TEXT             NOT NULL,   -- schema+snapshot fingerprint; skip rebuild if unchanged
    built_at     TEXT             NOT NULL,
    status       TEXT             NOT NULL DEFAULT 'ready'
);

CREATE TABLE IF NOT EXISTS dlm_value_index (
    id          TEXT              PRIMARY KEY,
    dataset_id  TEXT              NOT NULL,
    element_key TEXT              NOT NULL,   -- schema.table.column
    value_text  TEXT              NOT NULL,
    value_norm  TEXT              NOT NULL,   -- lowercased/trimmed/deaccented
    key_column  TEXT,                         -- surrogate key column if role-playing dim
    key_value   TEXT,
    freq        DOUBLE PRECISION  DEFAULT 0,
    source      TEXT
);
CREATE INDEX IF NOT EXISTS idx_dlm_value_norm ON dlm_value_index (dataset_id, value_norm);
CREATE INDEX IF NOT EXISTS idx_dlm_value_elem ON dlm_value_index (element_key);
CREATE UNIQUE INDEX IF NOT EXISTS ux_dlm_value_uniq
    ON dlm_value_index (dataset_id, element_key, value_norm);

CREATE TABLE IF NOT EXISTS dlm_router (
    dataset_id TEXT               PRIMARY KEY,
    summary    TEXT               NOT NULL,
    terms      TEXT,                          -- JSON: normalized keyword bag for lexical routing
    embeds     TEXT,                          -- v1 reserved
    codes      TEXT,                          -- v2 reserved: same code space as artifacts
    updated_at TEXT               NOT NULL
);
