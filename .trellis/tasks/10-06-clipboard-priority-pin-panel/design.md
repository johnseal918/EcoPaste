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
- Manually ordered cards use an alternate light background without restoring a dedicated metadata/title row.
- Pinned panel uses the existing card rendering in a two-column scrolling grid; no drag sorting.

## Burst capture pipeline
The OS clipboard callback performs only: lightweight foreground-app identity lookup, exclusion check, settings snapshot, bounded clipboard read, and capture timestamp. It then returns to the platform watcher.

Item construction (including image persistence), writeback-loop suppression, app icon extraction/materialization, and database persistence execute off the watcher thread. Captured timestamps are copied onto the eventual item so concurrent background preparation cannot reorder history by processing latency.

## Multi side-panel architecture
The existing right companion WebView remains a single host window, but it renders multiple independent panel columns. This avoids multiplying Windows DWM gaps and keeps alignment/movement atomic with the main window.

Rust owns only ephemeral current-session open state. Persistent configuration lives in Settings as sidePanels.order and sidePanels.alwaysShow. On main-window show, Rust restores only alwaysShow panels; on hide it clears the ephemeral open set. Toolbar toggles call a Rust command that changes the current open set and resizes/shows/hides the companion host.

The host width is derived from the number of open panels. Per-panel width is allowed to shrink within a bounded range when monitor space is constrained. React renders open panels in the persisted order and swaps that order when the user moves a visible panel left/right.
