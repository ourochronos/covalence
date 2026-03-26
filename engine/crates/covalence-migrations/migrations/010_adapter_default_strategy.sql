-- Add default_search_strategy to source_adapters.
-- When set, /ask queries scoped to this adapter use this strategy
-- instead of "auto".
ALTER TABLE source_adapters
    ADD COLUMN IF NOT EXISTS default_search_strategy TEXT;
