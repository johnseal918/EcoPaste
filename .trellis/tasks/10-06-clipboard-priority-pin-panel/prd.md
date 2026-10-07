# PRD — Clipboard priority ordering and pinned panel

## Goal
Improve daily clipboard use without changing the primary left-click-to-paste habit.

## Requirements
1. Remove the card header type labels such as HTML / 文本 / 链接 / 图片 so real content gets more vertical space.
2. Add persistent manual ordering for non-pinned history items.
   - No fixed count limit.
   - Normal item context menu: 加入排序.
   - Ordered item context menu: 移到最前 / 移到最后 / 移动到指定位置… / 取消排序.
   - Ordered items stay in the normal scrolling history and render with a visually distinct light background; no extra title row is reserved for ordering metadata.
   - No drag-to-reorder.
3. Pinned items no longer appear in the main history list.
4. Pinned items appear in a dedicated panel that is always on the right side of the main clipboard window.
   - When the right edge would overflow, shift the main+panel pair left; never move the pinned panel to the left side.
   - Pinned panel scrolls independently and uses two columns.
   - Pinned items have their own persistent order.
   - Context menu: 移到最前 / 移到最后 / 移动到指定位置… / 取消置顶.
5. Left click continues to mean paste; no drag reordering is added.
6. Main manual order and pinned order are independent. Pinning an ordered item removes it from the main order. Unpinning returns it to ordinary history (not manually ordered).

## Acceptance
- Existing databases migrate without data loss.
- Main list excludes pinned items and prioritizes manually ordered items.
- Pinned panel contains only pinned items, ordered by pin order.
- Reordering compacts positions to 1..N with no gaps/duplicates.
- Existing copy/paste/favorite/delete/note actions remain functional.

## Burst-safe clipboard capture
- Consecutive distinct clipboard changes must each be captured when Windows delivers the events and the clipboard can still be read.
- The watcher callback must freeze the current payload before slow image/app-icon/database work.
- Repeated identical content continues to use the existing content-hash deduplication semantics.
- Capture must remain automatic; no separate batch-mode toggle or drag workflow is introduced.

## Multi side-panel host
- The built-in Pinned, Favorites, Text, Image and Files buttons act as independent right-side panel toggles.
- Any number of these panels may be open at the same time; there is no one-panel-only restriction.
- Manual opening is session-only by default. When the main clipboard window hides, non-persistent open state is discarded.
- Each panel has its own “Always show” switch. Only panels enabled there reopen automatically with the main clipboard window next time.
- Users can reorder the currently visible panels. The order persists and is reused when panels are reopened.
- The Add/custom-group controls remain main-list organization controls rather than side panels.
