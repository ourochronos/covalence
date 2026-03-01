-- Session management table
CREATE TABLE IF NOT EXISTS covalence.sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    label TEXT UNIQUE,
    created_at TIMESTAMPTZ DEFAULT now(),
    last_active_at TIMESTAMPTZ DEFAULT now(),
    metadata JSONB DEFAULT '{}',
    status TEXT DEFAULT 'active' CHECK (status IN ('active', 'expired', 'closed'))
);

CREATE INDEX IF NOT EXISTS sessions_label_idx ON covalence.sessions (label);
CREATE INDEX IF NOT EXISTS sessions_status_idx ON covalence.sessions (status);

-- Link sessions to nodes they've accessed
CREATE TABLE IF NOT EXISTS covalence.session_nodes (
    session_id UUID REFERENCES covalence.sessions(id),
    node_id UUID REFERENCES covalence.nodes(id),
    first_accessed TIMESTAMPTZ DEFAULT now(),
    access_count INTEGER DEFAULT 1,
    PRIMARY KEY (session_id, node_id)
);
