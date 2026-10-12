-- User-initiated edits may deliberately match existing clipboard text.
-- Keep such rows distinct during legacy duplicate reconciliation after restart.
-- Existing records remain unmarked, preserving previous automatic copy dedup policy.
ALTER TABLE clipboard_items ADD COLUMN is_manually_edited INTEGER NOT NULL DEFAULT 0;
