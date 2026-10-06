# Implementation plan
1. Add DB migration, model fields, query ordering, and transactional ordering helpers.
2. Add Tauri commands and command constants/wrappers for priority and pin ordering.
3. Extend context-menu actions and localization; support target-position prompt action from the clipboard UI.
4. Remove card type labels and add manual-order visual treatment.
5. Make main list explicitly exclude pinned items.
6. Add pinned-panel route/component/window and couple its show/hide/position to the main clipboard window.
7. Extend Windows outside-click handling so the main + pinned panel behave as one clipboard surface.
8. Add focused Rust/frontend tests where existing test structure supports them; run lint/cargo tests/build in CI or local checkout.
