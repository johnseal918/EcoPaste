ALTER TABLE clipboard_items ADD COLUMN manual_order INTEGER;
ALTER TABLE clipboard_items ADD COLUMN pin_order INTEGER;

WITH ranked AS (
    SELECT id, ROW_NUMBER() OVER (ORDER BY updated_at DESC, created_at DESC, id ASC) AS position
    FROM clipboard_items
    WHERE is_pinned = 1
)
UPDATE clipboard_items
SET pin_order = (
    SELECT position
    FROM ranked
    WHERE ranked.id = clipboard_items.id
)
WHERE is_pinned = 1;

CREATE INDEX idx_clipboard_items_manual_order
ON clipboard_items (manual_order)
WHERE manual_order IS NOT NULL;

CREATE INDEX idx_clipboard_items_pin_order
ON clipboard_items (pin_order)
WHERE pin_order IS NOT NULL;
