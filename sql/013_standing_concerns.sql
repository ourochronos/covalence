-- Migration 013: Standing concerns table
-- Operational health signals written by the heartbeat (jane-ops)
-- and displayed on the Covalence dashboard.

CREATE TABLE IF NOT EXISTS covalence.standing_concerns (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT        NOT NULL UNIQUE,
    status      TEXT        NOT NULL CHECK (status IN ('green', 'yellow', 'red')),
    notes       TEXT,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
