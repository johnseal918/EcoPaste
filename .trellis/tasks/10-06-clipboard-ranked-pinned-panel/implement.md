# Implementation Plan

## Checklist

- [ ] Add migration and Rust model fields for main ranking and pinned ordering.
- [ ] Update all clipboard item SELECT/INSERT/backup paths for schema compatibility.
- [ ] Implement transactional ordering helpers and tests.
- [ ] Make main-history queries exclude pinned and rank manual items first.
- [ ] Add pinned-only query path ordered by pinned order.
- [ ] Add thin commands and centralized frontend command wrappers/constants.
- [ ] Extend native/web context menus with approved ranking actions and numeric-position flow.
- [ ] Remove app/type header row from clipboard cards and reclaim content space.
- [ ] Add visual distinction + small order indicator for ranked history items.
- [ ] Add pinned-panel route/components with two-column pinned cards.
- [ ] Add pinned companion Tauri window and right-side group positioning.
- [ ] Couple pinned panel show/hide lifecycle to clipboard window without changing left-click paste.
- [ ] Verify pin/unpin moves records between surfaces and preserves normalized orders.
- [ ] Run formatting, Rust tests/clippy, frontend lint/typecheck.
- [ ] Perform final full-scope diff review; no unrelated feature expansion.

## Validation commands

```bash
cd src-tauri
cargo fmt --check
cargo test
cargo clippy -- -D warnings
cd ..
pnpm lint
pnpm tsc
```

## Manual Windows acceptance

- Main window left; pinned panel always immediately right.
- Near right screen edge, group shifts left rather than placing pinned panel on left.
- Main list has no pinned cards.
- Left-click paste works identically on both surfaces.
- No drag-to-sort behavior.
- Ranking/pinned context menu operations and explicit position entry behave correctly.
- Restart preserves both order systems.
