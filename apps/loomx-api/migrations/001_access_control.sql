-- ============================================================
-- Migration 001: Access Control
-- Adds user_roles table + visibility column to content tables
-- Run in your LoomX metadata database
-- ============================================================

-- 1. User roles table (LoomX-managed role assignments)
IF OBJECT_ID('user_roles', 'U') IS NULL
BEGIN
    CREATE TABLE user_roles (
        id         NVARCHAR(36)  NOT NULL PRIMARY KEY DEFAULT NEWID(),
        user_email NVARCHAR(255) NOT NULL,
        role       NVARCHAR(20)  NOT NULL CHECK (role IN ('Viewer', 'Analyst', 'Editor', 'Admin')),
        granted_by NVARCHAR(255) NOT NULL,
        granted_at DATETIME2    NOT NULL DEFAULT GETUTCDATE(),
        updated_at DATETIME2    NOT NULL DEFAULT GETUTCDATE(),
        CONSTRAINT UQ_user_roles_email UNIQUE (user_email)
    );
    CREATE INDEX idx_user_roles_email ON user_roles(user_email);
    PRINT 'Table user_roles created';
END
GO

-- 2. Visibility on datasets
IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID('dbo.datasets') AND name = 'visibility'
)
BEGIN
    ALTER TABLE dbo.datasets
    ADD visibility NVARCHAR(20) NOT NULL DEFAULT 'internal'
        CONSTRAINT CK_datasets_visibility CHECK (visibility IN ('private', 'internal', 'published'));
    CREATE INDEX idx_datasets_visibility ON dbo.datasets(visibility);
    PRINT 'Column datasets.visibility added';
END
GO

-- 3. Visibility on charts
IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID('dbo.charts') AND name = 'visibility'
)
BEGIN
    ALTER TABLE dbo.charts
    ADD visibility NVARCHAR(20) NOT NULL DEFAULT 'internal'
        CONSTRAINT CK_charts_visibility CHECK (visibility IN ('private', 'internal', 'published'));
    CREATE INDEX idx_charts_visibility ON dbo.charts(visibility);
    PRINT 'Column charts.visibility added';
END
GO

-- 4. Visibility on dashboards
IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID('dbo.dashboards') AND name = 'visibility'
)
BEGIN
    ALTER TABLE dbo.dashboards
    ADD visibility NVARCHAR(20) NOT NULL DEFAULT 'internal'
        CONSTRAINT CK_dashboards_visibility CHECK (visibility IN ('private', 'internal', 'published'));
    CREATE INDEX idx_dashboards_visibility ON dbo.dashboards(visibility);
    PRINT 'Column dashboards.visibility added';
END
GO

PRINT '001_access_control migration complete';
GO
