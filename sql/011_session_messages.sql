-- Migration 011: session ingestion engine — message buffering
-- Adds platform/channel/parent fields to sessions and creates session_messages table.

ALTER TABLE covalence.sessions
    ADD COLUMN IF NOT EXISTS platform TEXT,
    ADD COLUMN IF NOT EXISTS channel TEXT,
    ADD COLUMN IF NOT EXISTS parent_session_id UUID REFERENCES covalence.sessions(id);

CREATE TABLE IF NOT EXISTS covalence.session_messages (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id UUID NOT NULL REFERENCES covalence.sessions(id) ON DELETE CASCADE,
    speaker TEXT,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    chunk_index INTEGER,
    created_at TIMESTAMPTZ DEFAULT now(),
    flushed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS session_messages_session_id_created_at_idx
    ON covalence.session_messages (session_id, created_at);
CREATE INDEX IF NOT EXISTS session_messages_unflushed_idx
    ON covalence.session_messages (session_id)
    WHERE flushed_at IS NULL;
