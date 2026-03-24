-- ============================================================
-- LoomX Production Database Schema — MySQL / MariaDB
-- ============================================================
-- Run this script once against your metadata database.
-- All statements are idempotent (safe to re-run).
-- ============================================================

-- ── Datasets ──────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS datasets (
    id            INT           NOT NULL AUTO_INCREMENT PRIMARY KEY,
    dataset_name  VARCHAR(500)  NOT NULL,
    description   LONGTEXT      NULL,
    fact_table    VARCHAR(500)  NOT NULL DEFAULT '',
    schema_name   VARCHAR(255)  NOT NULL DEFAULT 'public',
    database_name VARCHAR(255)  NULL,
    date_column   VARCHAR(255)  NULL,
    tables_used   LONGTEXT      NULL,
    visibility    VARCHAR(20)   NOT NULL DEFAULT 'internal',
    created_at    DATETIME      NOT NULL DEFAULT CURRENT_TIMESTAMP,
    modified_at   DATETIME      NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    created_by    VARCHAR(255)  NOT NULL DEFAULT 'system',
    modified_by   VARCHAR(255)  NULL,
    CONSTRAINT ck_datasets_visibility CHECK (visibility IN ('private','internal','published')),
    INDEX idx_datasets_dataset_name  (dataset_name),
    INDEX idx_datasets_database_name (database_name),
    INDEX idx_datasets_visibility    (visibility),
    INDEX idx_datasets_modified_at   (modified_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- ── Dataset Dimensions ────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS dataset_dimensions (
    id              INT          NOT NULL AUTO_INCREMENT PRIMARY KEY,
    dataset_id      INT          NOT NULL,
    dimension_table VARCHAR(500) NULL,
    table_name      VARCHAR(255) NULL,
    join_condition  LONGTEXT     NULL,
    fact_key        VARCHAR(255) NULL,
    join_key        VARCHAR(255) NULL,
    dim_name        VARCHAR(255) NULL,
    display_name    VARCHAR(255) NULL,
    created_at      DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT fk_dataset_dimensions_dataset FOREIGN KEY (dataset_id) REFERENCES datasets(id) ON DELETE CASCADE,
    INDEX idx_dataset_dimensions_dataset_id (dataset_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- ── Dataset Columns ───────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS dataset_columns (
    id            INT          NOT NULL AUTO_INCREMENT PRIMARY KEY,
    dataset_id    INT          NOT NULL,
    table_name    VARCHAR(255) NOT NULL DEFAULT '',
    column_name   VARCHAR(255) NOT NULL DEFAULT '',
    data_type     VARCHAR(255) NULL,
    is_dimension  TINYINT(1)   NOT NULL DEFAULT 0,
    is_metric     TINYINT(1)   NOT NULL DEFAULT 0,
    semantic_type VARCHAR(255) NULL,
    CONSTRAINT fk_dataset_columns_dataset FOREIGN KEY (dataset_id) REFERENCES datasets(id) ON DELETE CASCADE,
    INDEX idx_dataset_columns_dataset_id (dataset_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- ── Dataset Metrics ───────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS dataset_metrics (
    id          INT          NOT NULL AUTO_INCREMENT PRIMARY KEY,
    dataset_id  INT          NOT NULL,
    metric_name VARCHAR(255) NOT NULL,
    expression  LONGTEXT     NULL,
    metric_type VARCHAR(50)  NOT NULL DEFAULT 'sum',
    format      VARCHAR(255) NULL,
    CONSTRAINT fk_dataset_metrics_dataset FOREIGN KEY (dataset_id) REFERENCES datasets(id) ON DELETE CASCADE,
    INDEX idx_dataset_metrics_dataset_id (dataset_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- ── Charts ────────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS charts (
    id          VARCHAR(36)  NOT NULL PRIMARY KEY,
    name        VARCHAR(255) NOT NULL,
    description LONGTEXT     NULL,
    dataset_id  INT          NOT NULL,
    chart_type  VARCHAR(50)  NOT NULL,
    config      LONGTEXT     NOT NULL,
    visibility  VARCHAR(20)  NOT NULL DEFAULT 'internal',
    created_at  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    modified_at DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    created_by  VARCHAR(255) NOT NULL DEFAULT 'system',
    modified_by VARCHAR(255) NULL,
    CONSTRAINT ck_charts_visibility CHECK (visibility IN ('private','internal','published')),
    CONSTRAINT fk_charts_dataset FOREIGN KEY (dataset_id) REFERENCES datasets(id) ON DELETE CASCADE,
    INDEX idx_charts_dataset_id  (dataset_id),
    INDEX idx_charts_name        (name),
    INDEX idx_charts_visibility  (visibility),
    INDEX idx_charts_modified_at (modified_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- ── Dashboards ────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS dashboards (
    id           VARCHAR(36)  NOT NULL PRIMARY KEY,
    name         VARCHAR(255) NOT NULL,
    slug         VARCHAR(255) NULL,
    description  LONGTEXT     NULL,
    layout       LONGTEXT     NOT NULL,
    charts       LONGTEXT     NOT NULL,
    filters      LONGTEXT     NOT NULL,
    theme        LONGTEXT     NULL,
    tags         LONGTEXT     NULL,
    visibility   VARCHAR(20)  NOT NULL DEFAULT 'internal',
    is_published TINYINT(1)   NOT NULL DEFAULT 0,
    is_archived  TINYINT(1)   NOT NULL DEFAULT 0,
    created_at   DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    modified_at  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    created_by   VARCHAR(255) NOT NULL DEFAULT 'system',
    modified_by  VARCHAR(255) NULL,
    CONSTRAINT ck_dashboards_visibility CHECK (visibility IN ('private','internal','published')),
    INDEX idx_dashboards_name        (name),
    INDEX idx_dashboards_slug        (slug),
    INDEX idx_dashboards_visibility  (visibility),
    INDEX idx_dashboards_modified_at (modified_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- ── Saved Queries ─────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS saved_queries (
    id            VARCHAR(36)  NOT NULL PRIMARY KEY,
    name          VARCHAR(255) NOT NULL,
    description   LONGTEXT     NULL,
    sql_text      LONGTEXT     NOT NULL,
    database_name VARCHAR(255) NULL,
    created_at    DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    created_by    VARCHAR(255) NOT NULL DEFAULT 'system',
    INDEX idx_saved_queries_name       (name),
    INDEX idx_saved_queries_created_by (created_by)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- ── Query History ─────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS query_history (
    id             VARCHAR(36)  NOT NULL PRIMARY KEY,
    sql_text       LONGTEXT     NOT NULL,
    database_name  VARCHAR(255) NULL,
    executed_at    DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    execution_time DOUBLE       NOT NULL DEFAULT 0,
    row_count      INT          NOT NULL DEFAULT 0,
    status         VARCHAR(20)  NOT NULL DEFAULT 'success',
    error_message  LONGTEXT     NULL,
    user_email     VARCHAR(255) NOT NULL DEFAULT 'system',
    trigger_source VARCHAR(50)  NULL,
    dataset_id     VARCHAR(36)  NULL,
    tables_used    LONGTEXT     NULL,
    INDEX idx_query_history_executed_at (executed_at),
    INDEX idx_query_history_user_email  (user_email),
    INDEX idx_query_history_status      (status)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- ── Favorites ─────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS favorites (
    id          VARCHAR(36)  NOT NULL PRIMARY KEY,
    user_email  VARCHAR(255) NOT NULL,
    object_type VARCHAR(50)  NOT NULL,
    object_id   VARCHAR(36)  NOT NULL,
    object_name VARCHAR(255) NOT NULL,
    created_at  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    INDEX idx_favorites_user_email  (user_email),
    INDEX idx_favorites_object_type (object_type),
    INDEX idx_favorites_user_object (user_email, object_id, object_type)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- ── Activity Log ──────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS activity (
    id          VARCHAR(36)  NOT NULL PRIMARY KEY,
    action      VARCHAR(50)  NOT NULL,
    object_type VARCHAR(50)  NOT NULL,
    object_id   VARCHAR(36)  NOT NULL,
    object_name VARCHAR(255) NOT NULL,
    `timestamp` DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    user_email  VARCHAR(255) NOT NULL DEFAULT 'system',
    details     LONGTEXT     NULL,
    INDEX idx_activity_timestamp   (`timestamp`),
    INDEX idx_activity_user_email  (user_email),
    INDEX idx_activity_object_type (object_type)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- ── Data Sources ──────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS data_sources (
    id                INT          NOT NULL AUTO_INCREMENT PRIMARY KEY,
    name              VARCHAR(255) NOT NULL,
    type              VARCHAR(100) NOT NULL,
    connection_string VARCHAR(1000) NOT NULL,
    database_name     VARCHAR(255) NULL,
    region            VARCHAR(10)  NOT NULL DEFAULT 'WW',
    description       LONGTEXT     NULL,
    created_by        VARCHAR(255) NULL,
    created_at        DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at        DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    is_active         TINYINT(1)   NOT NULL DEFAULT 1,
    CONSTRAINT uq_data_sources_name UNIQUE (name),
    CONSTRAINT ck_data_sources_region CHECK (region IN ('WW','EU')),
    INDEX ix_data_sources_region    (region),
    INDEX ix_data_sources_is_active (is_active)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- ── User Themes ───────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS user_themes (
    user_email  VARCHAR(255) NOT NULL PRIMARY KEY,
    theme_color VARCHAR(7)   NOT NULL DEFAULT '#6366f1',
    created_at  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX idx_user_themes_email (user_email)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- ── Auth Config ───────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS auth_config (
    id                   INT          NOT NULL AUTO_INCREMENT PRIMARY KEY,
    provider             VARCHAR(50)  NOT NULL DEFAULT 'local',
    azure_tenant_id      VARCHAR(255) NULL,
    azure_client_id      VARCHAR(255) NULL,
    google_client_id     VARCHAR(255) NULL,
    google_client_secret VARCHAR(1000) NULL,
    jwt_secret           VARCHAR(1000) NULL,
    updated_at           DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    updated_by           VARCHAR(255) NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- ── Local Users ───────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS local_users (
    id                    INT          NOT NULL AUTO_INCREMENT PRIMARY KEY,
    username              VARCHAR(255) NOT NULL,
    email                 VARCHAR(255) NOT NULL,
    password_hash         VARCHAR(255) NOT NULL,
    role                  VARCHAR(20)  NOT NULL DEFAULT 'Viewer',
    force_password_change TINYINT(1)   NOT NULL DEFAULT 0,
    is_active             TINYINT(1)   NOT NULL DEFAULT 1,
    created_at            DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    CONSTRAINT ck_local_users_role CHECK (role IN ('Viewer','Analyst','Editor','Admin')),
    CONSTRAINT uq_local_users_username UNIQUE (username),
    CONSTRAINT uq_local_users_email    UNIQUE (email),
    INDEX idx_local_users_username (username),
    INDEX idx_local_users_email    (email)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
