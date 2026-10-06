# Design

## Data model

Add two nullable ordering columns through a new migration:

- `manual_order INTEGER NULL`: order among non-pinned manually ranked history items.
- `pin_order INTEGER NULL`: order among pinned items.

Rules:

- `is_pinned = 1` => item is returned only by pinned-panel queries; `manual_order` is cleared.
- adding an item to main ranking requires `is_pinned = 0`; assign `MAX(manual_order)+1`.
- pinning assigns `MAX(pin_order)+1`; unpinning clears `pin_order`.
- reordering runs in a DB transaction and compacts affected order values to contiguous 1..N.
- ordering metadata updates do not modify `updated_at`.

## Query contracts

Extend item query with an explicit surface/filter so callers can request:

- main history: exclude pinned; order by ranked-first then configured normal sort.
- pinned panel: pinned-only; order by `pin_order` then stable fallback.

Search/group/favorite filters remain applicable where meaningful.

## Commands / menu actions

Add Rust commands for:

- add item to ranking
- remove item from ranking
- move ranked item to first
- move ranked item to last
- move ranked item to explicit position
- move pinned item to first
- move pinned item to last
- move pinned item to explicit position

Windows/macOS context menus expose the actions based on `is_pinned` and ordering state. “Move to position…” uses the existing context submenu/window mechanism or a minimal numeric prompt compatible with both platforms; no drag behavior is introduced.

## Pinned companion window

Create a dedicated Tauri webview route/window label for the pinned panel.

Behavior:

- show clipboard => show/position pinned panel to the main window’s right;
- hide clipboard => hide pinned panel;
- keep the pinned panel right-adjacent;
- if right-side placement would exceed monitor work area, shift the main+panel group left while retaining right-side relation;
- pinned panel uses the same clipboard card/paste logic but pinned-only data source, two-column layout, and independent scrolling;
- the panel is skipped from the taskbar and follows clipboard auto-hide/pin behavior as one visual group.

## Frontend

- Remove the current card metadata title row (source-app icon + kind/sub-kind label).
- Keep timestamps and useful quick actions.
- Show a subtle rank marker/background for manually ranked history items.
- Show a small pinned-order index in the pinned panel.
- Reuse existing card rendering/content components where practical instead of duplicating paste/content logic.

## Compatibility

A new SQL migration is mandatory because the app has released data. Update every SELECT/INSERT/query_as field list and test fixture to include the new nullable columns. Backup/import schema paths must be checked for compatibility.

## Validation

- DB migration + repository tests for normalization/reorder/pin transitions.
- command/menu tests where available.
- frontend type/lint checks.
- manual Windows check for two-window right-side geometry, paste, context menus, and scroll behavior.
