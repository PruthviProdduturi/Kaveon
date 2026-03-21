-- ============================================
-- LoomX Production Database Schema
-- ============================================
-- Run this script in your metadata database to create all required tables
-- This is a complete schema including all migrations

-- ============================================
-- Datasets Table
-- ============================================
IF OBJECT_ID('datasets', 'U') IS NULL
BEGIN
    CREATE TABLE datasets (
        id NVARCHAR(36) PRIMARY KEY,
        name NVARCHAR(255) NOT NULL,
        description NVARCHAR(MAX),
        table_name NVARCHAR(255) NOT NULL,
        schema_name NVARCHAR(255),
        database_name NVARCHAR(255), -- Added to support multiple data sources
        columns NVARCHAR(MAX), -- JSON array of columns
        dimensions NVARCHAR(MAX), -- JSON array of dimensions
        metrics NVARCHAR(MAX), -- JSON array of metrics
        created_at DATETIME2 NOT NULL DEFAULT GETDATE(),
        updated_at DATETIME2 NOT NULL DEFAULT GETDATE(),
        created_by NVARCHAR(255) NOT NULL DEFAULT 'system'
    );

    CREATE INDEX idx_datasets_name ON datasets(name);
    CREATE INDEX idx_datasets_created_at ON datasets(created_at);
    CREATE INDEX idx_datasets_database_name ON datasets(database_name);

    PRINT 'Table datasets created successfully';
END
GO

-- ============================================
-- Charts Table
-- ============================================
IF OBJECT_ID('charts', 'U') IS NULL
BEGIN
    CREATE TABLE charts (
        id NVARCHAR(36) PRIMARY KEY,
        name NVARCHAR(255) NOT NULL,
        description NVARCHAR(MAX),
        dataset_id NVARCHAR(36) NOT NULL,
        chart_type NVARCHAR(50) NOT NULL, -- bar, line, pie, table, scatter
        config NVARCHAR(MAX) NOT NULL, -- JSON configuration
        created_at DATETIME2 NOT NULL DEFAULT GETDATE(),
        updated_at DATETIME2 NOT NULL DEFAULT GETDATE(),
        created_by NVARCHAR(255) NOT NULL DEFAULT 'system',
        FOREIGN KEY (dataset_id) REFERENCES datasets(id) ON DELETE CASCADE
    );

    CREATE INDEX idx_charts_dataset_id ON charts(dataset_id);
    CREATE INDEX idx_charts_name ON charts(name);
    CREATE INDEX idx_charts_created_at ON charts(created_at);

    PRINT 'Table charts created successfully';
END
GO

-- ============================================
-- Dashboards Table
-- ============================================
IF OBJECT_ID('dashboards', 'U') IS NULL
BEGIN
    CREATE TABLE dashboards (
        id NVARCHAR(36) PRIMARY KEY,
        name NVARCHAR(255) NOT NULL,
        description NVARCHAR(MAX),
        layout NVARCHAR(MAX) NOT NULL, -- JSON layout configuration
        created_at DATETIME2 NOT NULL DEFAULT GETDATE(),
        updated_at DATETIME2 NOT NULL DEFAULT GETDATE(),
        created_by NVARCHAR(255) NOT NULL DEFAULT 'system'
    );

    CREATE INDEX idx_dashboards_name ON dashboards(name);
    CREATE INDEX idx_dashboards_created_at ON dashboards(created_at);

    PRINT 'Table dashboards created successfully';
END
GO

-- ============================================
-- Saved Queries Table
-- ============================================
IF OBJECT_ID('saved_queries', 'U') IS NULL
BEGIN
    CREATE TABLE saved_queries (
        id NVARCHAR(36) PRIMARY KEY,
        name NVARCHAR(255) NOT NULL,
        description NVARCHAR(MAX),
        sql NVARCHAR(MAX) NOT NULL,
        database_name NVARCHAR(255), -- Which database this query runs against
        created_at DATETIME2 NOT NULL DEFAULT GETDATE(),
        updated_at DATETIME2 NOT NULL DEFAULT GETDATE(),
        created_by NVARCHAR(255) NOT NULL DEFAULT 'system'
    );

    CREATE INDEX idx_saved_queries_name ON saved_queries(name);
    CREATE INDEX idx_saved_queries_created_at ON saved_queries(created_at);
    CREATE INDEX idx_saved_queries_created_by ON saved_queries(created_by);

    PRINT 'Table saved_queries created successfully';
END
GO

-- ============================================
-- Query History Table
-- ============================================
IF OBJECT_ID('query_history', 'U') IS NULL
BEGIN
    CREATE TABLE query_history (
        id NVARCHAR(36) PRIMARY KEY,
        sql_text NVARCHAR(MAX) NOT NULL,
        database_name NVARCHAR(255), -- Which database the query ran against
        executed_at DATETIME2 NOT NULL DEFAULT GETDATE(),
        execution_time FLOAT NOT NULL, -- Execution time in seconds
        row_count INT NOT NULL,
        status NVARCHAR(20) NOT NULL, -- success, error
        error_message NVARCHAR(MAX),
        user_email NVARCHAR(255) NOT NULL DEFAULT 'system',
        trigger_source NVARCHAR(50), -- e.g., 'lab', 'dataset-preview', 'chart-builder'
        dataset_id NVARCHAR(36), -- If query was from a dataset
        tables_used NVARCHAR(MAX) -- JSON array of table names
    );

    CREATE INDEX idx_query_history_executed_at ON query_history(executed_at);
    CREATE INDEX idx_query_history_user_email ON query_history(user_email);
    CREATE INDEX idx_query_history_status ON query_history(status);

    PRINT 'Table query_history created successfully';
END
GO

-- ============================================
-- Favorites Table
-- ============================================
IF OBJECT_ID('favorites', 'U') IS NULL
BEGIN
    CREATE TABLE favorites (
        id NVARCHAR(36) PRIMARY KEY DEFAULT NEWID(),
        user_email NVARCHAR(255) NOT NULL,
        object_type NVARCHAR(50) NOT NULL, -- dataset, chart, dashboard, query, data_source
        object_id NVARCHAR(36) NOT NULL,
        object_name NVARCHAR(255) NOT NULL,
        created_at DATETIME2 NOT NULL DEFAULT GETDATE()
    );

    CREATE INDEX idx_favorites_user_email ON favorites(user_email);
    CREATE INDEX idx_favorites_object_type ON favorites(object_type);
    CREATE INDEX idx_favorites_user_object ON favorites(user_email, object_id, object_type);

    PRINT 'Table favorites created successfully';
END
GO

-- ============================================
-- Activity Log Table
-- ============================================
IF OBJECT_ID('activity', 'U') IS NULL
BEGIN
    CREATE TABLE activity (
        id NVARCHAR(36) PRIMARY KEY DEFAULT NEWID(),
        action NVARCHAR(50) NOT NULL, -- created, updated, deleted, viewed, executed
        object_type NVARCHAR(50) NOT NULL, -- dataset, chart, dashboard, query
        object_id NVARCHAR(36) NOT NULL,
        object_name NVARCHAR(255) NOT NULL,
        timestamp DATETIME2 NOT NULL DEFAULT GETDATE(),
        user_email NVARCHAR(255) NOT NULL DEFAULT 'system',
        details NVARCHAR(MAX) -- JSON details
    );

    CREATE INDEX idx_activity_timestamp ON activity(timestamp);
    CREATE INDEX idx_activity_user_email ON activity(user_email);
    CREATE INDEX idx_activity_object_type ON activity(object_type);

    PRINT 'Table activity created successfully';
END
GO

-- ============================================
-- Data Sources Table
-- ============================================
IF OBJECT_ID('data_sources', 'U') IS NULL
BEGIN
    CREATE TABLE data_sources (
        id INT IDENTITY(1,1) PRIMARY KEY,
        name NVARCHAR(255) NOT NULL,
        type NVARCHAR(100) NOT NULL, -- e.g., 'Fabric SQL AEP', 'Fabric SQL DW', 'Azure SQL'
        connection_string NVARCHAR(1000) NOT NULL, -- SQL endpoint
        database_name NVARCHAR(255) NULL, -- Database name (required for Fabric SQL)
        region NVARCHAR(10) NOT NULL CHECK (region IN ('WW', 'EU')), -- Worldwide or Europe
        description NVARCHAR(MAX) NULL,
        created_by NVARCHAR(255) NULL,
        created_at DATETIME2 DEFAULT GETDATE(),
        updated_at DATETIME2 DEFAULT GETDATE(),
        is_active BIT DEFAULT 1, -- Whether this data source is currently active
        CONSTRAINT UQ_data_sources_name UNIQUE (name)
    );

    CREATE INDEX IX_data_sources_region ON data_sources(region);
    CREATE INDEX IX_data_sources_is_active ON data_sources(is_active);

    PRINT 'Table data_sources created successfully';
END
GO

-- ============================================
-- User Themes Table
-- ============================================
IF OBJECT_ID('user_themes', 'U') IS NULL
BEGIN
    CREATE TABLE user_themes (
        user_email NVARCHAR(255) NOT NULL PRIMARY KEY,
        theme_color NVARCHAR(7) NOT NULL, -- Hex color format: #RRGGBB
        created_at DATETIME2 NOT NULL DEFAULT GETDATE(),
        updated_at DATETIME2 NOT NULL DEFAULT GETDATE()
    );

    CREATE INDEX idx_user_themes_email ON user_themes(user_email);

    PRINT 'Table user_themes created successfully';
END
GO

-- ============================================
-- Auth Config Table
-- ============================================
IF OBJECT_ID('auth_config', 'U') IS NULL
BEGIN
    CREATE TABLE auth_config (
        id INT IDENTITY(1,1) PRIMARY KEY,
        provider NVARCHAR(50) NOT NULL DEFAULT 'azure_ad', -- azure_ad | local | google
        azure_tenant_id NVARCHAR(255) NULL,
        azure_client_id NVARCHAR(255) NULL,
        google_client_id NVARCHAR(255) NULL,
        google_client_secret NVARCHAR(1000) NULL, -- encrypted
        jwt_secret NVARCHAR(1000) NULL,           -- encrypted; used for local HS256 tokens
        updated_at DATETIME2 NOT NULL DEFAULT GETDATE(),
        updated_by NVARCHAR(255) NULL
    );

    PRINT 'Table auth_config created successfully';
END
GO

-- ============================================
-- Local Users Table
-- ============================================
IF OBJECT_ID('local_users', 'U') IS NULL
BEGIN
    CREATE TABLE local_users (
        id INT IDENTITY(1,1) PRIMARY KEY,
        username NVARCHAR(255) NOT NULL,
        email NVARCHAR(255) NOT NULL,
        password_hash NVARCHAR(255) NOT NULL,
        force_password_change BIT NOT NULL DEFAULT 0,
        is_active BIT NOT NULL DEFAULT 1,
        created_at DATETIME2 NOT NULL DEFAULT GETDATE(),
        updated_at DATETIME2 NOT NULL DEFAULT GETDATE(),
        CONSTRAINT UQ_local_users_username UNIQUE (username),
        CONSTRAINT UQ_local_users_email UNIQUE (email)
    );

    CREATE INDEX idx_local_users_username ON local_users(username);
    CREATE INDEX idx_local_users_email ON local_users(email);

    PRINT 'Table local_users created successfully';
END
GO

-- ============================================
-- Schema Creation Complete
-- ============================================
PRINT '';
PRINT '============================================';
PRINT 'LoomX Production Schema Created Successfully';
PRINT '============================================';
PRINT 'Tables created:';
PRINT '  - datasets (with database_name support)';
PRINT '  - charts';
PRINT '  - dashboards';
PRINT '  - saved_queries';
PRINT '  - query_history';
PRINT '  - favorites';
PRINT '  - activity';
PRINT '  - data_sources';
PRINT '  - user_themes';
PRINT '  - auth_config';
PRINT '  - local_users';
PRINT '';
PRINT 'Next steps:';
PRINT '  1. Add your data sources via the /data-sources UI';
PRINT '  2. Configure METADATA_ENDPOINT and METADATA_DATABASE in .env';
PRINT '  3. Start creating datasets, charts, and dashboards!';
PRINT '============================================';
GO
