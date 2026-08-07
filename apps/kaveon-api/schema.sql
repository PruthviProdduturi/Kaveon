-- ============================================================
-- Lens Production Database Schema — SQL Server / Fabric SQL / Azure SQL
-- ============================================================
-- Run this script once against your metadata database.
-- All statements are idempotent (safe to re-run).
-- ============================================================

-- ── Datasets ──────────────────────────────────────────────────────────────────
IF OBJECT_ID('datasets', 'U') IS NULL
BEGIN
    CREATE TABLE datasets (
        id              INT           IDENTITY(1,1) PRIMARY KEY,
        dataset_name    NVARCHAR(500) NOT NULL,
        description     NVARCHAR(MAX) NULL,
        fact_table      NVARCHAR(500) NOT NULL DEFAULT '',
        schema_name     NVARCHAR(255) NOT NULL DEFAULT 'dbo',
        database_name   NVARCHAR(255) NULL,
        date_column     NVARCHAR(255) NULL,
        tables_used     NVARCHAR(MAX) NULL,   -- JSON: { filters, sql_text }
        visibility      NVARCHAR(20)  NOT NULL DEFAULT 'internal'
                        CONSTRAINT CK_datasets_visibility CHECK (visibility IN ('private','internal','published')),
        created_at      DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        modified_at     DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        created_by      NVARCHAR(255) NOT NULL DEFAULT 'system',
        modified_by     NVARCHAR(255) NULL
    );
    CREATE INDEX idx_datasets_dataset_name  ON datasets(dataset_name);
    CREATE INDEX idx_datasets_database_name ON datasets(database_name);
    CREATE INDEX idx_datasets_visibility    ON datasets(visibility);
    CREATE INDEX idx_datasets_modified_at   ON datasets(modified_at);
    PRINT 'Table datasets created';
END
GO

-- ── Dataset Dimensions ────────────────────────────────────────────────────────
IF OBJECT_ID('dataset_dimensions', 'U') IS NULL
BEGIN
    CREATE TABLE dataset_dimensions (
        id               INT           IDENTITY(1,1) PRIMARY KEY,
        dataset_id       INT           NOT NULL,
        dimension_table  NVARCHAR(500) NULL,
        table_name       NVARCHAR(255) NULL,
        join_condition   NVARCHAR(MAX) NULL,
        fact_key         NVARCHAR(255) NULL,
        join_key         NVARCHAR(255) NULL,
        dim_name         NVARCHAR(255) NULL,
        display_name     NVARCHAR(255) NULL,
        created_at       DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        CONSTRAINT FK_dataset_dimensions_dataset FOREIGN KEY (dataset_id) REFERENCES datasets(id) ON DELETE CASCADE
    );
    CREATE INDEX idx_dataset_dimensions_dataset_id ON dataset_dimensions(dataset_id);
    PRINT 'Table dataset_dimensions created';
END
GO

-- ── Dataset Columns ───────────────────────────────────────────────────────────
IF OBJECT_ID('dataset_columns', 'U') IS NULL
BEGIN
    CREATE TABLE dataset_columns (
        id            INT           IDENTITY(1,1) PRIMARY KEY,
        dataset_id    INT           NOT NULL,
        table_name    NVARCHAR(255) NOT NULL DEFAULT '',
        column_name   NVARCHAR(255) NOT NULL DEFAULT '',
        data_type     NVARCHAR(255) NULL,
        is_dimension  BIT           NOT NULL DEFAULT 0,
        is_metric     BIT           NOT NULL DEFAULT 0,
        semantic_type NVARCHAR(255) NULL,
        CONSTRAINT FK_dataset_columns_dataset FOREIGN KEY (dataset_id) REFERENCES datasets(id) ON DELETE CASCADE
    );
    CREATE INDEX idx_dataset_columns_dataset_id ON dataset_columns(dataset_id);
    PRINT 'Table dataset_columns created';
END
GO

-- ── Dataset Metrics ───────────────────────────────────────────────────────────
IF OBJECT_ID('dataset_metrics', 'U') IS NULL
BEGIN
    CREATE TABLE dataset_metrics (
        id          INT           IDENTITY(1,1) PRIMARY KEY,
        dataset_id  INT           NOT NULL,
        metric_name NVARCHAR(255) NOT NULL,
        expression  NVARCHAR(MAX) NULL,
        metric_type NVARCHAR(50)  NOT NULL DEFAULT 'sum',
        format      NVARCHAR(255) NULL,
        CONSTRAINT FK_dataset_metrics_dataset FOREIGN KEY (dataset_id) REFERENCES datasets(id) ON DELETE CASCADE
    );
    CREATE INDEX idx_dataset_metrics_dataset_id ON dataset_metrics(dataset_id);
    PRINT 'Table dataset_metrics created';
END
GO

-- ── Charts ────────────────────────────────────────────────────────────────────
IF OBJECT_ID('charts', 'U') IS NULL
BEGIN
    CREATE TABLE charts (
        id          NVARCHAR(36)  NOT NULL PRIMARY KEY,
        name        NVARCHAR(255) NOT NULL,
        description NVARCHAR(MAX) NULL,
        dataset_id  INT           NOT NULL,
        chart_type  NVARCHAR(50)  NOT NULL,
        config      NVARCHAR(MAX) NOT NULL DEFAULT '{}',
        visibility  NVARCHAR(20)  NOT NULL DEFAULT 'internal'
                    CONSTRAINT CK_charts_visibility CHECK (visibility IN ('private','internal','published')),
        created_at  DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        modified_at DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        created_by  NVARCHAR(255) NOT NULL DEFAULT 'system',
        modified_by NVARCHAR(255) NULL,
        CONSTRAINT FK_charts_dataset FOREIGN KEY (dataset_id) REFERENCES datasets(id) ON DELETE CASCADE
    );
    CREATE INDEX idx_charts_dataset_id  ON charts(dataset_id);
    CREATE INDEX idx_charts_name        ON charts(name);
    CREATE INDEX idx_charts_visibility  ON charts(visibility);
    CREATE INDEX idx_charts_modified_at ON charts(modified_at);
    PRINT 'Table charts created';
END
GO

-- ── Dashboards ────────────────────────────────────────────────────────────────
IF OBJECT_ID('dashboards', 'U') IS NULL
BEGIN
    CREATE TABLE dashboards (
        id           NVARCHAR(36)  NOT NULL PRIMARY KEY,
        name         NVARCHAR(255) NOT NULL,
        slug         NVARCHAR(255) NULL,
        description  NVARCHAR(MAX) NULL,
        layout       NVARCHAR(MAX) NOT NULL DEFAULT '[]',
        charts       NVARCHAR(MAX) NOT NULL DEFAULT '[]',
        filters      NVARCHAR(MAX) NOT NULL DEFAULT '[]',
        theme        NVARCHAR(MAX) NULL,
        tags         NVARCHAR(MAX) NULL,
        visibility   NVARCHAR(20)  NOT NULL DEFAULT 'internal'
                     CONSTRAINT CK_dashboards_visibility CHECK (visibility IN ('private','internal','published')),
        is_published BIT           NOT NULL DEFAULT 0,
        is_archived  BIT           NOT NULL DEFAULT 0,
        created_at   DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        modified_at  DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        created_by   NVARCHAR(255) NOT NULL DEFAULT 'system',
        modified_by  NVARCHAR(255) NULL
    );
    CREATE INDEX idx_dashboards_name        ON dashboards(name);
    CREATE INDEX idx_dashboards_slug        ON dashboards(slug);
    CREATE INDEX idx_dashboards_visibility  ON dashboards(visibility);
    CREATE INDEX idx_dashboards_modified_at ON dashboards(modified_at);
    PRINT 'Table dashboards created';
END
GO

-- ── Saved Queries ─────────────────────────────────────────────────────────────
IF OBJECT_ID('saved_queries', 'U') IS NULL
BEGIN
    CREATE TABLE saved_queries (
        id            NVARCHAR(36)  NOT NULL PRIMARY KEY,
        name          NVARCHAR(255) NOT NULL,
        description   NVARCHAR(MAX) NULL,
        sql           NVARCHAR(MAX) NOT NULL,
        database_name NVARCHAR(255) NULL,
        created_at    DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        updated_at    DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        created_by    NVARCHAR(255) NOT NULL DEFAULT 'system'
    );
    CREATE INDEX idx_saved_queries_name       ON saved_queries(name);
    CREATE INDEX idx_saved_queries_created_by ON saved_queries(created_by);
    PRINT 'Table saved_queries created';
END
GO

-- ── Query History ─────────────────────────────────────────────────────────────
IF OBJECT_ID('query_history', 'U') IS NULL
BEGIN
    CREATE TABLE query_history (
        id             NVARCHAR(36)  NOT NULL PRIMARY KEY,
        sql_text       NVARCHAR(MAX) NOT NULL,
        database_name  NVARCHAR(255) NULL,
        executed_at    DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        execution_time FLOAT         NOT NULL DEFAULT 0,
        row_count      INT           NOT NULL DEFAULT 0,
        status         NVARCHAR(20)  NOT NULL DEFAULT 'success',
        error_message  NVARCHAR(MAX) NULL,
        user_email     NVARCHAR(255) NOT NULL DEFAULT 'system',
        trigger_source NVARCHAR(50)  NULL,
        dataset_id     NVARCHAR(36)  NULL,
        tables_used    NVARCHAR(MAX) NULL
    );
    CREATE INDEX idx_query_history_executed_at ON query_history(executed_at);
    CREATE INDEX idx_query_history_user_email  ON query_history(user_email);
    CREATE INDEX idx_query_history_status      ON query_history(status);
    PRINT 'Table query_history created';
END
GO

-- ── Favorites ─────────────────────────────────────────────────────────────────
IF OBJECT_ID('favorites', 'U') IS NULL
BEGIN
    CREATE TABLE favorites (
        id          NVARCHAR(36)  NOT NULL PRIMARY KEY DEFAULT NEWID(),
        user_email  NVARCHAR(255) NOT NULL,
        object_type NVARCHAR(50)  NOT NULL,
        object_id   NVARCHAR(36)  NOT NULL,
        object_name NVARCHAR(255) NOT NULL,
        created_at  DATETIME2     NOT NULL DEFAULT GETUTCDATE()
    );
    CREATE INDEX idx_favorites_user_email   ON favorites(user_email);
    CREATE INDEX idx_favorites_object_type  ON favorites(object_type);
    CREATE INDEX idx_favorites_user_object  ON favorites(user_email, object_id, object_type);
    PRINT 'Table favorites created';
END
GO

-- ── Activity Log ──────────────────────────────────────────────────────────────
IF OBJECT_ID('activity', 'U') IS NULL
BEGIN
    CREATE TABLE activity (
        id          NVARCHAR(36)  NOT NULL PRIMARY KEY DEFAULT NEWID(),
        action      NVARCHAR(50)  NOT NULL,
        object_type NVARCHAR(50)  NOT NULL,
        object_id   NVARCHAR(36)  NOT NULL,
        object_name NVARCHAR(255) NOT NULL,
        timestamp   DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        user_email  NVARCHAR(255) NOT NULL DEFAULT 'system',
        details     NVARCHAR(MAX) NULL
    );
    CREATE INDEX idx_activity_timestamp   ON activity(timestamp);
    CREATE INDEX idx_activity_user_email  ON activity(user_email);
    CREATE INDEX idx_activity_object_type ON activity(object_type);
    PRINT 'Table activity created';
END
GO

-- ── Data Sources ──────────────────────────────────────────────────────────────
IF OBJECT_ID('data_sources', 'U') IS NULL
BEGIN
    CREATE TABLE data_sources (
        id                INT           IDENTITY(1,1) PRIMARY KEY,
        name              NVARCHAR(255) NOT NULL,
        type              NVARCHAR(100) NOT NULL,
        connection_string NVARCHAR(1000) NOT NULL,
        database_name     NVARCHAR(255) NULL,
        region            NVARCHAR(10)  NOT NULL DEFAULT 'WW'
                          CONSTRAINT CK_data_sources_region CHECK (region IN ('WW','EU')),
        description       NVARCHAR(MAX) NULL,
        created_by        NVARCHAR(255) NULL,
        created_at        DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        updated_at        DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        is_active         BIT           NOT NULL DEFAULT 1,
        CONSTRAINT UQ_data_sources_name UNIQUE (name)
    );
    CREATE INDEX ix_data_sources_region    ON data_sources(region);
    CREATE INDEX ix_data_sources_is_active ON data_sources(is_active);
    PRINT 'Table data_sources created';
END
GO

-- ── User Themes ───────────────────────────────────────────────────────────────
IF OBJECT_ID('user_themes', 'U') IS NULL
BEGIN
    CREATE TABLE user_themes (
        user_email  NVARCHAR(255) NOT NULL PRIMARY KEY,
        theme_color NVARCHAR(7)   NOT NULL DEFAULT '#6366f1',
        created_at  DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        updated_at  DATETIME2     NOT NULL DEFAULT GETUTCDATE()
    );
    PRINT 'Table user_themes created';
END
GO

-- ── Auth Config ───────────────────────────────────────────────────────────────
IF OBJECT_ID('auth_config', 'U') IS NULL
BEGIN
    CREATE TABLE auth_config (
        id                   INT           IDENTITY(1,1) PRIMARY KEY,
        provider             NVARCHAR(50)  NOT NULL DEFAULT 'local',
        azure_tenant_id      NVARCHAR(255) NULL,
        azure_client_id      NVARCHAR(255) NULL,
        google_client_id     NVARCHAR(255) NULL,
        google_client_secret NVARCHAR(1000) NULL,   -- Fernet-encrypted
        jwt_secret           NVARCHAR(1000) NULL,   -- Fernet-encrypted
        updated_at           DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        updated_by           NVARCHAR(255) NULL
    );
    PRINT 'Table auth_config created';
END
GO

-- ── Local Users ───────────────────────────────────────────────────────────────
IF OBJECT_ID('local_users', 'U') IS NULL
BEGIN
    CREATE TABLE local_users (
        id                    INT           IDENTITY(1,1) PRIMARY KEY,
        username              NVARCHAR(255) NOT NULL,
        email                 NVARCHAR(255) NOT NULL,
        password_hash         NVARCHAR(255) NOT NULL,
        role                  NVARCHAR(20)  NOT NULL DEFAULT 'Viewer'
                              CONSTRAINT CK_local_users_role CHECK (role IN ('Viewer','Analyst','Editor','Admin')),
        force_password_change BIT           NOT NULL DEFAULT 0,
        is_active             BIT           NOT NULL DEFAULT 1,
        created_at            DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        updated_at            DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        CONSTRAINT UQ_local_users_username UNIQUE (username),
        CONSTRAINT UQ_local_users_email    UNIQUE (email)
    );
    CREATE INDEX idx_local_users_username ON local_users(username);
    CREATE INDEX idx_local_users_email    ON local_users(email);
    PRINT 'Table local_users created';
END
GO

PRINT '';
PRINT '============================================================';
PRINT 'Lens schema initialised successfully.';
PRINT 'Tables: datasets, dataset_dimensions, dataset_columns,';
PRINT '        dataset_metrics, charts, dashboards, saved_queries,';
PRINT '        query_history, favorites, activity, data_sources,';
PRINT '        user_themes, auth_config, local_users';
PRINT '============================================================';
GO

-- ── User Recents ──────────────────────────────────────────────────────────────
IF OBJECT_ID('user_recents', 'U') IS NULL
BEGIN
    CREATE TABLE user_recents (
        id         INT           IDENTITY(1,1) PRIMARY KEY,
        user_email NVARCHAR(255) NOT NULL,
        item_id    NVARCHAR(255) NOT NULL,
        label      NVARCHAR(500) NOT NULL,
        href       NVARCHAR(1000) NOT NULL,
        type       NVARCHAR(50)  NOT NULL,
        created_at DATETIME2     NOT NULL DEFAULT GETUTCDATE(),
        CONSTRAINT uq_user_recents_email_item UNIQUE (user_email, item_id)
    );
    CREATE INDEX idx_user_recents_email ON user_recents(user_email);
    PRINT 'Table user_recents created';
END
GO
