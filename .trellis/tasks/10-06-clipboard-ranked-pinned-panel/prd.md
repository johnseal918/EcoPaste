# Clipboard Compact Cards, Manual Ranking, and Pinned Panel

## Goal

Improve the main EcoPaste workflow without changing the core left-click-to-paste habit:

1. Remove the per-card source/type title row that currently shows app icon + HTML/Text/Link/Image labels, so copied content gets more vertical space.
2. Add persistent manual ranking for non-pinned history items.
3. Move pinned items out of the main history list into a dedicated companion panel that is always positioned to the right of the main clipboard window.

## Approved interaction

- Left click on any clipboard card remains paste. No drag-to-sort is introduced.
- Main history:
  - ordinary item context menu: **Add to ranking**;
  - ranked item context menu: **Move to first / Move to last / Move to position… / Remove from ranking**;
  - a new ranked item is appended to the end of the ranked section;
  - position numbers are contiguous and automatically normalized;
  - ranked items are visually distinct and remain part of the normal scrolling list;
  - ranked items appear before ordinary history items.
- Pinned panel:
  - pinned items are excluded from the main history list;
  - all pinned items appear in a separate panel to the **right** of the main clipboard window;
  - the panel never switches to the left side;
  - when screen space is tight, move the whole two-window group left as needed instead of changing sides;
  - pinned item context menu: **Move to first / Move to last / Move to position… / Unpin**;
  - new pinned items are appended to the end of pinned order;
  - pinned order is independent from main-history ranking;
  - two-column visual order is left-to-right, then top-to-bottom;
  - pinned panel scrolls independently when needed.

## Persistence

- Ranking survives application restart.
- Pinned ordering survives application restart.
- Existing pinned/favorite/history records remain valid after migration.
- Metadata-only ordering changes must not refresh clipboard item `updated_at`.

## Scope constraints

- Do not add drag sorting.
- Do not change left-click paste semantics.
- Do not redesign unrelated settings, clipboard capture, search, favorites, groups, preview, or backup behavior except where schema compatibility requires it.
- Preserve macOS + Windows support, but Windows right-side geometry is the primary user scenario.

## Acceptance criteria

- [ ] App/type header row is absent from normal and pinned cards; useful content occupies the reclaimed space.
- [ ] Main list never contains pinned items.
- [ ] Ranked non-pinned items appear before ordinary history and retain explicit order across restart.
- [ ] Ranking context-menu operations work without duplicate/gapped positions.
- [ ] Pinned panel opens/hides with the clipboard window and remains on its right side.
- [ ] Pinned panel order is persistent and independently editable through the approved context-menu commands.
- [ ] Left click still pastes on both panels.
- [ ] Existing pin/unpin action moves an item between main list and pinned panel correctly.
- [ ] No drag interaction is added.
- [ ] Rust tests, clippy, formatting, frontend lint/typecheck pass; Windows UI behavior is manually verifiable.
