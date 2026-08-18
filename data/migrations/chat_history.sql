-- Chat persistence tables
-- Run against the kaveon metadata database (PostgreSQL)

CREATE TABLE IF NOT EXISTS dbo.chat_sessions (
    id          SERIAL PRIMARY KEY,
    user_email  VARCHAR(255) NOT NULL,
    title       VARCHAR(500) NOT NULL DEFAULT 'New conversation',
    created_at  TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_chat_sessions_user
    ON dbo.chat_sessions (user_email, updated_at DESC);

CREATE TABLE IF NOT EXISTS dbo.chat_messages (
    id          SERIAL PRIMARY KEY,
    session_id  INTEGER NOT NULL REFERENCES dbo.chat_sessions(id) ON DELETE CASCADE,
    role        VARCHAR(10) NOT NULL CHECK (role IN ('user', 'assistant')),
    content     TEXT NOT NULL,
    sql_query   TEXT,
    chart_type  VARCHAR(20),
    data        JSONB,
    route       VARCHAR(20),
    created_at  TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_chat_messages_session
    ON dbo.chat_messages (session_id, created_at);
