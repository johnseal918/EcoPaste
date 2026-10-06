# Design

## Persistence

Add nullable columns to `clipboard_items`:
- `manual_order INTEGER`
- `pin_order INTEGER`

No new clipboard item payload fields are required. Main-list ordered highlighting is derived from an `orderedCount` value returned with the page; menu state is resolved server-side when the context menu is opened.

## Ordering

- Main list uses `pinned=false`. Rows with `manual_order` sort first by ascending manual order, then ordinary rows use the selected existing sort.
- Pinned panel uses `pinned=true` and sorts by ascending `pin_order`.
- Reordering renumbers the relevant ordered set to contiguous 1..N values in a transaction.
- Pinning clears `manual_order` and appends `pin_order`; unpinning clears `pin_order`.

## UI

- ClipboardCard removes the source/type title block. Timestamp/quick actions become a compact overlay so body content starts higher.
- Main list uses orderedCount to color the leading manually ordered rows.
- A new `/pinned-panel` route renders pinned cards in two columns.
- Context-menu actions are extended for manual and pinned ordering. “Move to position” opens a small numeric modal in the owning window.

## Windowing

Add a non-focusable always-on-top `pinned-panel` Tauri window. Showing the clipboard window positions the pair together; pinned panel stays right of the clipboard window. Hiding the clipboard window hides the pinned panel. Windows global outside-click detection uses the union of both window rectangles.
