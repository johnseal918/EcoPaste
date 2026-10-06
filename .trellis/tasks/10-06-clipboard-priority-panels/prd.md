# Clipboard priority panels

## Business result

Make EcoPaste faster to scan and organize without changing the user's established left-click-to-paste habit.

## Requirements

1. Remove the source/type title row (for example app icon + HTML/Text/Link) from clipboard cards so copied content gets more vertical space.
2. Add a persistent manual ordering tier for non-pinned history items.
   - No fixed item-count limit.
   - Normal item menu: **加入排序**.
   - Ordered item menu: **移到最前 / 移到最后 / 移动到指定位置… / 取消排序**.
   - No drag-to-reorder.
   - Ordered items stay in the normal scrollable history and render with a distinct subtle background.
   - Order positions remain contiguous after every operation.
3. Move pinned items out of the main history into a separate panel that is always on the right side of the clipboard window.
   - The panel follows the clipboard window show/hide lifecycle.
   - If the combined width would exceed the monitor work area, shift the whole pair left; never move the pinned panel to the left side.
   - Pinned items do not also appear in the main list.
   - Two-column pinned layout, scrolling independently.
   - New pin is appended to the end.
   - Pinned item menu: **移到最前 / 移到最后 / 移动到指定位置… / 取消置顶**.
   - No drag-to-reorder.
4. Left click behavior remains governed by the existing auto-paste/copy setting; adding ordering must not add a competing drag gesture.

## Acceptance criteria

- Existing data migrates without loss.
- Pinned/manual order survives restart.
- Main query explicitly excludes pinned items.
- Right panel only queries pinned items.
- Single-click paste remains functional in both panels.
- Windows outside-click auto-hide treats the two windows as one visual group.
- Rust and frontend ordering contracts are type-safe and bilingual where user-visible.
