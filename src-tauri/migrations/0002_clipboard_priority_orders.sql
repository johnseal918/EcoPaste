ALTER TABLE clipboard_items ADD COLUMN manual_order INTEGER;
ALTER TABLE clipboard_items ADD COLUMN pin_order INTEGER;

CREATE INDEX IF NOT EXISTS idx_clipboard_items_manual_order
ON clipboard_items(manual_order)
WHERE manual_order IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_clipboard_items_pin_order
ON clipboard_items(pin_order)
WHERE pin_order IS NOT NULL;

-- Preserve existing pinned records in a deterministic initial order.
UPDATE clipboard_items
SET pin_order = (
  SELECT COUNT(*)
  FROM clipboard_items AS earlier
  WHERE earlier.is_pinned = 1
    AND (
      earlier.updated_at > clipboard_items.updated_at
      OR (
        earlier.updated_at = clipboard_items.updated_at
        AND earlier.created_at > clipboard_items.created_at
      )
      OR (
        earlier.updated_at = clipboard_items.updated_at
        AND earlier.created_at = clipboard_items.created_at
        AND earlier.id <= clipboard_items.id
      )
    )
)
WHERE is_pinned = 1;

CREATE TRIGGER IF NOT EXISTS trg_clipboard_items_delete_manual_order
AFTER DELETE ON clipboard_items
WHEN OLD.manual_order IS NOT NULL
BEGIN
  UPDATE clipboard_items
  SET manual_order = manual_order - 1
  WHERE manual_order > OLD.manual_order;
END;

CREATE TRIGGER IF NOT EXISTS trg_clipboard_items_delete_pin_order
AFTER DELETE ON clipboard_items
WHEN OLD.pin_order IS NOT NULL
BEGIN
  UPDATE clipboard_items
  SET pin_order = pin_order - 1
  WHERE pin_order > OLD.pin_order;
END;
