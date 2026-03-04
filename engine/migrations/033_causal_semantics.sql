-- Migration 033: Causal semantics — edge causal weights and dual confidence (covalence#75)

ALTER TABLE covalence.edges
  ADD COLUMN IF NOT EXISTS causal_weight FLOAT NOT NULL DEFAULT 0.5;

ALTER TABLE covalence.nodes
  ADD COLUMN IF NOT EXISTS provenance_confidence FLOAT;
