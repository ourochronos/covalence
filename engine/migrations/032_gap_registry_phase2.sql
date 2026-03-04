-- Migration 032: Gap Registry Phase 2 — structural and horizon scores (covalence#120)

ALTER TABLE covalence.gap_registry
  ADD COLUMN IF NOT EXISTS structural_score FLOAT NOT NULL DEFAULT 0.0,
  ADD COLUMN IF NOT EXISTS horizon_score FLOAT NOT NULL DEFAULT 0.0;
