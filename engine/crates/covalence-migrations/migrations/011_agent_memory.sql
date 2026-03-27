-- Agent memory context table.
-- Supplements sources: content lives in sources, agent context here.
CREATE TABLE IF NOT EXISTS agent_memories (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id     UUID NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    agent_id      TEXT,
    topic         TEXT,
    task_id       TEXT,
    confidence    FLOAT8 NOT NULL DEFAULT 0.5,
    access_count  INT NOT NULL DEFAULT 0,
    last_accessed TIMESTAMPTZ,
    expires_at    TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_agent_memories_agent ON agent_memories(agent_id);
CREATE INDEX IF NOT EXISTS idx_agent_memories_topic ON agent_memories(topic);
CREATE INDEX IF NOT EXISTS idx_agent_memories_source ON agent_memories(source_id);
CREATE INDEX IF NOT EXISTS idx_agent_memories_expires ON agent_memories(expires_at) WHERE expires_at IS NOT NULL;
