# EcoPaste Specification Index

These specs capture how EcoPaste is built today. Read this index together with
`AGENTS.md`; when they overlap, `AGENTS.md` is the project-level authority and
these files add source-backed detail.

## Project Shape

EcoPaste is a Tauri v2 desktop app for macOS and Windows. Product behavior is
Rust-first: clipboard capture and writeback, database access, settings
persistence, window positioning, shortcuts, tray, autostart, backup, and storage
live in `src-tauri/src/`. The React app in `src/` renders UI, mirrors settings,
handles ordinary interactions, and calls Rust through typed command wrappers.

## Spec Directories

| Directory | Scope |
| --- | --- |
| [backend](./backend/index.md) | Rust/Tauri modules, SQLite, clipboard pipeline, settings, storage, platform behavior |
| [frontend](./frontend/index.md) | React, Ant Design, UnoCSS, Valtio mirrors, command wrappers, i18n, UI quality |
| [guides](./guides/index.md) | Cross-layer and code-reuse checklists for changes that span modules |

## Before Editing

- Identify which layer owns the behavior. If it touches system clipboard,
  persistence, content detection, OS integration, or stored settings, start in
  Rust.
- Search for existing constants and contracts before adding strings. Command
  names are mirrored by `src-tauri/src/lib.rs` and `src/constants/commands.ts`;
  event names are mirrored by Rust constants and `src/constants/events.ts`.
- Keep frontend business data as command results, not as a local database copy.
  `src/stores/clipboardView.ts` and `src/stores/settings.ts` are examples of UI
  state and settings mirrors only.
- Use macOS and Windows `#[cfg]` branches only. There is no Linux support target
  in this rewrite.

## Current software result

An additional run is necessary only when it directly confirms a user-agreed current business outcome, uses the real application's normal business flow with the relevant real input or deliverable, and resolves a specific gap in the existing applicable evidence. If existing evidence already answers that outcome, perform zero additional runs; an actual delivery operation that supplies the evidence already completes this work without a separate test phase. Running a script, changing code or SHA, or starting a new chat is not a reason to rerun. Unit, mock, synthetic-fixture, static/style, coverage, historical-suite, and governance checks are excluded. If a real business flow fails, fix only that blocked flow within the assigned scope; do not require all-green results, a fixed matrix, or unrelated repairs. Necessary build/packaging remains available without added tests. Preserve actual permissions, data protections, accepted behavior, and the user's delivery standard.
