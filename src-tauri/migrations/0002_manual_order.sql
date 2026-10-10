ALTER TABLE clipboard_items ADD COLUMN priority_order INTEGER;
ALTER TABLE clipboard_items ADD COLUMN pin_order INTEGER;

CREATE INDEX idx_clipboard_items_priority_order ON clipboard_items (priority_order);
CREATE INDEX idx_clipboard_items_pin_order ON clipboard_items (pin_order);

UPDATE clipboard_items
SET pin_order = (
    SELECT COUNT(*)
    FROM clipboard_items AS earlier
    WHERE earlier.is_pinned = 1
      AND (
        earlier.created_at > clipboard_items.created_at
        OR (earlier.created_at = clipboard_items.created_at AND earlier.id <= clipboard_items.id)
      )
)
WHERE is_pinned = 1;
