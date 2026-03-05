-- Migration 040: cap edge confidence to [0, 1] — retroactive fix (covalence#187)
--
-- Tier 1 and Tier 2 confidence branches in `infer_article_edges` were missing
-- a `.min(0.95)` cap, allowing `combined_score` (which combines Jaccard,
-- domain-path overlap, and cosine similarity) to push confidence above 1.0
-- (observed max: 1.630).  The Rust fix is in the same commit; this migration
-- retroactively corrects the 43 already-inserted RELATES_TO edges whose
-- `confidence` column exceeds 1.0.
--
-- LEAST(confidence, 1.0) is intentionally used here rather than 0.95 because:
--   a) clamping to 1.0 preserves semantic correctness (probability bound);
--   b) the 0.95 business cap is enforced going-forward by the code fix;
--   c) retroactively lowering already-correct edges from, say, 0.94 → 0.94
--      would be a no-op anyway, so LEAST(confidence, 1.0) is the safest,
--      narrowest correction.
--
-- Safe to re-run: the WHERE clause makes the UPDATE a no-op on clean data.
-- No lock escalation beyond a row-level UPDATE on the 43 affected rows.

BEGIN;

UPDATE covalence.edges
   SET confidence = LEAST(confidence, 1.0)
 WHERE confidence > 1.0;

COMMIT;
