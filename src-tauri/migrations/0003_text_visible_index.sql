-- Ordinary visible-text equality is used for dedup across HTML/RTF/plain.
-- This non-unique index accelerates the equality lookup. Existing user data
-- remains unchanged by the migration; safe duplicate consolidation is performed
-- separately with preservation rules and a transaction.
CREATE INDEX IF NOT EXISTS idx_clipboard_items_text_visible
    ON clipboard_items (search_text)
    WHERE kind = 'text' AND search_text IS NOT NULL;
