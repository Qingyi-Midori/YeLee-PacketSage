-- Per-tool-call evidence facts (ADR-019 cold recovery + ADR-023 the ledger is
-- the single source of truth for V2-V4).
--
-- Before this migration the ledger's `numbers` and `ref_ids` lived only in the
-- memory of the process that produced them. A report rendered by a *later*
-- process (Agent CLI §6: `run` then `report`) therefore had no way to prove the
-- numbers a stored finding cites, and the anti-hallucination lint rejected
-- legitimate findings. Persisting them keeps the lint strict *and* makes
-- cross-process reports possible.
--
-- Both dialects support adding a NOT NULL column with a default; sqlx records
-- applied migrations, so each statement runs exactly once.

ALTER TABLE tool_calls ADD COLUMN numbers_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE tool_calls ADD COLUMN ref_ids_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE tool_calls ADD COLUMN tokens_json TEXT NOT NULL DEFAULT '[]';
