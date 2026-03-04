-- Migration 031: Task state machine — worker lifecycle (covalence#114)
-- Formalizes task tracking so worker lifecycle is queryable rather than
-- scattered across Discord messages and session metadata.

CREATE TABLE IF NOT EXISTS covalence.tasks (
  id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
  label              TEXT        NOT NULL,
  issue_ref          TEXT,
  status             TEXT        NOT NULL DEFAULT 'pending'
                                 CHECK (status IN ('pending','assigned','running','done','failed')),
  assigned_session_id TEXT,
  started_at         TIMESTAMPTZ,
  completed_at       TIMESTAMPTZ,
  timeout_at         TIMESTAMPTZ,
  failure_class      TEXT,
  result_summary     TEXT,
  metadata           JSONB       NOT NULL DEFAULT '{}',
  created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Trigger to auto-update updated_at on every row change.
CREATE OR REPLACE FUNCTION covalence.tasks_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
  NEW.updated_at = now();
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_tasks_updated_at ON covalence.tasks;
CREATE TRIGGER trg_tasks_updated_at
  BEFORE UPDATE ON covalence.tasks
  FOR EACH ROW EXECUTE FUNCTION covalence.tasks_set_updated_at();

-- Indexes for common query patterns.
CREATE INDEX IF NOT EXISTS idx_tasks_status      ON covalence.tasks (status);
CREATE INDEX IF NOT EXISTS idx_tasks_created_at  ON covalence.tasks (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_tasks_timeout_at  ON covalence.tasks (timeout_at)
    WHERE timeout_at IS NOT NULL AND status = 'running';
