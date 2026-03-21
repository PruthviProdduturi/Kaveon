-- ============================================
-- LoomX Production Database Schema — MySQL / MariaDB
-- ============================================

CREATE TABLE IF NOT EXISTS datasets (
    id VARCHAR(36) NOT NULL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    description LONGTEXT,
    table_name VARCHAR(255) NOT NULL,
    schema_name VARCHAR(255),
    database_name VARCHAR(255),
    `columns` LONGTEXT,
    dimensions LONGTEXT,
    metrics LONGTEXT,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    created_by VARCHAR(255) NOT NULL DEFAULT 'system',
    INDEX idx_datasets_name (name),
    INDEX idx_datasets_created_at (created_at),
    INDEX idx_datasets_database_name (database_name)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS charts (
    id VARCHAR(36) NOT NULL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    description LONGTEXT,
    dataset_id VARCHAR(36) NOT NULL,
    chart_type VARCHAR(50) NOT NULL,
    config LONGTEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    created_by VARCHAR(255) NOT NULL DEFAULT 'system',
    INDEX idx_charts_dataset_id (dataset_id),
    INDEX idx_charts_name (name),
    INDEX idx_charts_created_at (created_at),
    FOREIGN KEY (dataset_id) REFERENCES datasets(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS dashboards (
    id VARCHAR(36) NOT NULL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    description LONGTEXT,
    layout LONGTEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    created_by VARCHAR(255) NOT NULL DEFAULT 'system',
    INDEX idx_dashboards_name (name),
    INDEX idx_dashboards_created_at (created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS saved_queries (
    id VARCHAR(36) NOT NULL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    description LONGTEXT,
    sql_text LONGTEXT NOT NULL,
    database_name VARCHAR(255),
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    created_by VARCHAR(255) NOT NULL DEFAULT 'system',
    INDEX idx_saved_queries_name (name),
    INDEX idx_saved_queries_created_at (created_at),
    INDEX idx_saved_queries_created_by (created_by)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS query_history (
    id VARCHAR(36) NOT NULL PRIMARY KEY,
    sql_text LONGTEXT NOT NULL,
    database_name VARCHAR(255),
    executed_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    execution_time DOUBLE NOT NULL,
    row_count INT NOT NULL,
    status VARCHAR(20) NOT NULL,
    error_message LONGTEXT,
    user_email VARCHAR(255) NOT NULL DEFAULT 'system',
    trigger_source VARCHAR(50),
    dataset_id VARCHAR(36),
    tables_used LONGTEXT,
    INDEX idx_query_history_executed_at (executed_at),
    INDEX idx_query_history_user_email (user_email),
    INDEX idx_query_history_status (status)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS favorites (
    id VARCHAR(36) NOT NULL PRIMARY KEY,
    user_email VARCHAR(255) NOT NULL,
    object_type VARCHAR(50) NOT NULL,
    object_id VARCHAR(36) NOT NULL,
    object_name VARCHAR(255) NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    INDEX idx_favorites_user_email (user_email),
    INDEX idx_favorites_object_type (object_type),
    INDEX idx_favorites_user_object (user_email, object_id, object_type)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS activity (
    id VARCHAR(36) NOT NULL PRIMARY KEY,
    action VARCHAR(50) NOT NULL,
    object_type VARCHAR(50) NOT NULL,
    object_id VARCHAR(36) NOT NULL,
    object_name VARCHAR(255) NOT NULL,
    `timestamp` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    user_email VARCHAR(255) NOT NULL DEFAULT 'system',
    details LONGTEXT,
    INDEX idx_activity_timestamp (`timestamp`),
    INDEX idx_activity_user_email (user_email),
    INDEX idx_activity_object_type (object_type)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS data_sources (
    id INT NOT NULL AUTO_INCREMENT PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    type VARCHAR(100) NOT NULL,
    connection_string VARCHAR(1000) NOT NULL,
    database_name VARCHAR(255),
    region VARCHAR(10) NOT NULL,
    description LONGTEXT,
    created_by VARCHAR(255),
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    is_active TINYINT(1) DEFAULT 1,
    CONSTRAINT uq_data_sources_name UNIQUE (name),
    CONSTRAINT chk_region CHECK (region IN ('WW', 'EU')),
    INDEX ix_data_sources_region (region),
    INDEX ix_data_sources_is_active (is_active)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS user_themes (
    user_email VARCHAR(255) NOT NULL PRIMARY KEY,
    theme_color VARCHAR(7) NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX idx_user_themes_email (user_email)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS auth_config (
    id INT NOT NULL AUTO_INCREMENT PRIMARY KEY,
    provider VARCHAR(50) NOT NULL DEFAULT 'azure_ad',
    azure_tenant_id VARCHAR(255),
    azure_client_id VARCHAR(255),
    google_client_id VARCHAR(255),
    google_client_secret VARCHAR(1000),
    jwt_secret VARCHAR(1000),
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    updated_by VARCHAR(255)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS local_users (
    id INT NOT NULL AUTO_INCREMENT PRIMARY KEY,
    username VARCHAR(255) NOT NULL,
    email VARCHAR(255) NOT NULL,
    password_hash VARCHAR(255) NOT NULL,
    force_password_change TINYINT(1) NOT NULL DEFAULT 0,
    is_active TINYINT(1) NOT NULL DEFAULT 1,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    CONSTRAINT uq_local_users_username UNIQUE (username),
    CONSTRAINT uq_local_users_email UNIQUE (email),
    INDEX idx_local_users_username (username),
    INDEX idx_local_users_email (email)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
