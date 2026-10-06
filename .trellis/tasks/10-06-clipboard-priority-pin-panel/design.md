# Design

## Data
Add nullable `priority_order INTEGER` and `pin_order INTEGER` to `clipboard_items` through a new migration.

Invariants:
- `is_pinned = 1` => `priority_order IS NULL`.
- newly pinned item gets `MAX(pin_order)+1`.
- newly manually ordered item gets `MAX(priority_order)+1` among non-pinned rows.
- move/cancel operations run in a transaction and compact affected sequence.

## Queries
- Main history explicitly requests `pinned=false`; SQL orders `priority_order IS NULL`, then `priority_order`, then the configured normal sort.
- Pinned panel requests `pinned=true`; SQL orders `pin_order`, with created time as deterministic fallback.

## Windows
Add `clipboard-pinned` webview route/window. It is shown/hidden with the main clipboard window. Layout calculation keeps the pinned window to the right and shifts the pair left when needed.

## UI
- Card header drops type text; app icon/time metadata should not consume a dedicated title row.
- Manually ordered cards render an order badge and alternate light background.
- Pinned panel uses the existing card rendering in a two-column scrolling grid; no drag sorting.
