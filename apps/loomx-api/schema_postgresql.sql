-- ============================================
-- LoomX Production Database Schema — PostgreSQL
-- ============================================

CREATE EXTENSION IF NOT EXISTS "pgcrypto";

CREATE TABLE IF NOT EXISTS datasets (
    id VARCHAR(36) PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    description TEXT,
    table_name VARCHAR(255) NOT NULL,
    schema_name VARCHAR(255),
    database_name VARCHAR(255),
    columns TEXT,
    dimensions TEXT,
    metrics TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP NOT NULL DEFAULT NOW(),
    created_by VARCHAR(255) NOT NULL DEFAULT 'system'
);
CREATE INDEX IF NOT EXISTS idx_datasets_name ON datasets(name);
CREATE INDEX IF NOT EXISTS idx_datasets_created_at ON datasets(created_at);
CREATE INDEX IF NOT EXISTS idx_datasets_database_name ON datasets(database_name);

CREATE TABLE IF NOT EXISTS charts (
    id VARCHAR(36) PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    description TEXT,
    dataset_id VARCHAR(36) NOT NULL,
    chart_type VARCHAR(50) NOT NULL,
    config TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP NOT NULL DEFAULT NOW(),
    created_by VARCHAR(255) NOT NULL DEFAULT 'system',
    FOREIGN KEY (dataset_id) REFERENCES datasets(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_charts_dataset_id ON charts(dataset_id);
CREATE INDEX IF NOT EXISTS idx_charts_name ON charts(name);
CREATE INDEX IF NOT EXISTS idx_charts_created_at ON charts(created_at);

CREATE TABLE IF NOT EXISTS dashboards (
    id VARCHAR(36) PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    description TEXT,
    layout TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP NOT NULL DEFAULT NOW(),
    created_by VARCHAR(255) NOT NULL DEFAULT 'system'
);
CREATE INDEX IF NOT EXISTS idx_dashboards_name ON dashboards(name);
CREATE INDEX IF NOT EXISTS idx_dashboards_created_at ON dashboards(created_at);

CREATE TABLE IF NOT EXISTS saved_queries (
    id VARCHAR(36) PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    description TEXT,
    sql TEXT NOT NULL,
    database_name VARCHAR(255),
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP NOT NULL DEFAULT NOW(),
    created_by VARCHAR(255) NOT NULL DEFAULT 'system'
);
CREATE INDEX IF NOT EXISTS idx_saved_queries_name ON saved_queries(name);
CREATE INDEX IF NOT EXISTS idx_saved_queries_created_at ON saved_queries(created_at);
CREATE INDEX IF NOT EXISTS idx_saved_queries_created_by ON saved_queries(created_by);

CREATE TABLE IF NOT EXISTS query_history (
    id VARCHAR(36) PRIMARY KEY,
    sql_text TEXT NOT NULL,
    database_name VARCHAR(255),
    executed_at TIMESTAMP NOT NULL DEFAULT NOW(),
    execution_time FLOAT NOT NULL,
    row_count INT NOT NULL,
    status VARCHAR(20) NOT NULL,
    error_message TEXT,
    user_email VARCHAR(255) NOT NULL DEFAULT 'system',
    trigger_source VARCHAR(50),
    dataset_id VARCHAR(36),
    tables_used TEXT
);
CREATE INDEX IF NOT EXISTS idx_query_history_executed_at ON query_history(executed_at);
CREATE INDEX IF NOT EXISTS idx_query_history_user_email ON query_history(user_email);
CREATE INDEX IF NOT EXISTS idx_query_history_status ON query_history(status);

CREATE TABLE IF NOT EXISTS favorites (
    id VARCHAR(36) PRIMARY KEY DEFAULT gen_random_uuid()::VARCHAR,
    user_email VARCHAR(255) NOT NULL,
    object_type VARCHAR(50) NOT NULL,
    object_id VARCHAR(36) NOT NULL,
    object_name VARCHAR(255) NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_favorites_user_email ON favorites(user_email);
CREATE INDEX IF NOT EXISTS idx_favorites_object_type ON favorites(object_type);
CREATE INDEX IF NOT EXISTS idx_favorites_user_object ON favorites(user_email, object_id, object_type);

CREATE TABLE IF NOT EXISTS activity (
    id VARCHAR(36) PRIMARY KEY DEFAULT gen_random_uuid()::VARCHAR,
    action VARCHAR(50) NOT NULL,
    object_type VARCHAR(50) NOT NULL,
    object_id VARCHAR(36) NOT NULL,
    object_name VARCHAR(255) NOT NULL,
    timestamp TIMESTAMP NOT NULL DEFAULT NOW(),
    user_email VARCHAR(255) NOT NULL DEFAULT 'system',
    details TEXT
);
CREATE INDEX IF NOT EXISTS idx_activity_timestamp ON activity(timestamp);
CREATE INDEX IF NOT EXISTS idx_activity_user_email ON activity(user_email);
CREATE INDEX IF NOT EXISTS idx_activity_object_type ON activity(object_type);

CREATE TABLE IF NOT EXISTS data_sources (
    id SERIAL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    type VARCHAR(100) NOT NULL,
    connection_string VARCHAR(1000) NOT NULL,
    database_name VARCHAR(255),
    region VARCHAR(10) NOT NULL CHECK (region IN ('WW', 'EU')),
    description TEXT,
    created_by VARCHAR(255),
    created_at TIMESTAMP DEFAULT NOW(),
    updated_at TIMESTAMP DEFAULT NOW(),
    is_active BOOLEAN DEFAULT TRUE,
    CONSTRAINT uq_data_sources_name UNIQUE (name)
);
CREATE INDEX IF NOT EXISTS ix_data_sources_region ON data_sources(region);
CREATE INDEX IF NOT EXISTS ix_data_sources_is_active ON data_sources(is_active);

CREATE TABLE IF NOT EXISTS user_themes (
    user_email VARCHAR(255) NOT NULL PRIMARY KEY,
    theme_color VARCHAR(7) NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_user_themes_email ON user_themes(user_email);

CREATE TABLE IF NOT EXISTS auth_config (
    id SERIAL PRIMARY KEY,
    provider VARCHAR(50) NOT NULL DEFAULT 'azure_ad',
    azure_tenant_id VARCHAR(255),
    azure_client_id VARCHAR(255),
    google_client_id VARCHAR(255),
    google_client_secret VARCHAR(1000),
    jwt_secret VARCHAR(1000),
    updated_at TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_by VARCHAR(255)
);

CREATE TABLE IF NOT EXISTS local_users (
    id SERIAL PRIMARY KEY,
    username VARCHAR(255) NOT NULL,
    email VARCHAR(255) NOT NULL,
    password_hash VARCHAR(255) NOT NULL,
    force_password_change BOOLEAN NOT NULL DEFAULT FALSE,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_local_users_username UNIQUE (username),
    CONSTRAINT uq_local_users_email UNIQUE (email)
);
CREATE INDEX IF NOT EXISTS idx_local_users_username ON local_users(username);
CREATE INDEX IF NOT EXISTS idx_local_users_email ON local_users(email);
