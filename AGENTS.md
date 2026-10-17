# OmaLook — agent working notes

Native Linux mail/calendar/contacts/tasks/SMS client: **Rust backend daemon + Qt/QML UI** (runs under Quickshell),
Omarchy-themed, keyboard-first. Providers: Microsoft Graph (Exchange/Outlook), Gmail, others via `src/providers/`.
Renamed from "omarchylook" on 2026-10-12. **README.md is stale** (describes the old Python version); trust this file,
`docs/TRACKER.md` and `COMPONENT_REFERENCE.md` (master dependency/version guide) instead.

## Layout
- `src/` Rust: `main.rs` (launcher + HTTP backend), `email_daemon.rs` (sync), `providers/`, `db.rs` + `db/` (SQLite+FTS5),
  `compose.rs`, `outbox.rs`, `search.rs`, `invites.rs`, `calendar_daemon.rs`, `sms_daemon.rs`, `qt_bridge/`.
- `qml/` UI (`shell.qml`, `AppShell.qml`, `mail/ compose/ keys/ settings/ state/ views/ Ui/`); `plugin/omalook-compose/` C++ QML plugin.
- `tools/render/` WebKitGTK helper process that renders HTML mail to PNG (Qt's own renderer is not used for HTML).
- `docs/` plans and specs (KEYS.md, PLAN-*.md, TRACKER.md); `notes/` is **gitignored** personal notes.

## Build / run / test
- `./build.sh [--release] [--live] [--clean]` — builds to `/mnt/d/omalook-build` (set in `.cargo/config.toml`; the project lives on NFS, building in-tree is very slow).
- `./build.sh --live` installs to `/opt/omalook` (first run asks sudo once). The running app uses `/opt/omalook/qml`, **not** the working tree: QML-only changes need deploy/relaunch to show up.
- Dev and live copies share port **27182**, `~/.config/omalook`, and the cache — close one before starting the other.
- Tests: `cargo test --lib -- --skip test_copy_to_clipboard`; node tests: `node tests/commands.test.js`, `node tests/search.test.js`.
  `test_copy_to_clipboard` hangs on `wl-copy` (OL-005, open) — never run the full suite without the skip, never wait on it.
- Rust toolchain via `$HOME/.cargo/env` (rustup, Rust 1.98.1, Qt 6.11.x). Perf target: launch → refreshed mail < 2 s.

## Hard-won rules
- Never let a Qt `Text` fetch remote URLs or use QtWebEngine inside Quickshell: both crash the shell (OL-004). HTML goes through `qml/mail/format.js` sanitiser or the render helper.
- Keyring service name is `omalook`. Do not rename it again (the rename once hid every saved sign-in).
- Account ids are `<provider>-<suffix>`; email lives in the accounts table, not in the keyring id. `(provider, email)` is unique.
- Delta sync (Graph `/delta`) is the sync path; purge only trusts a completed first walk. Don't reintroduce full-mailbox crawls.
- Search: keyword-style patterns; Graph body search needs the date-window workaround; target p50 < 50 ms on the 60k-row cache.
- QML text uses `root.normalFontSize` (no offsets); fonts are `"monospace"`; colours come from the Omarchy theme, not hardcoded.
- Text fields own their keys; global bindings apply only when no text field has focus. Document new bindings in `docs/KEYS.md`.

## Git
- Remote `origin` = github.com/amacieli/OmarchyLook. **No feature branches needed (user decision, 2026-10-16): lump all work together on the current branch (`main`) and commit there. Push only when the user asks.** Verify upstream is current before pushing or rebasing.
- The working tree usually has the user's own uncommitted work (`build.sh`, the `omalook` symlink, `packaging/`, staged deletions of the old Python backend). **Stage selectively; never `git add -A`; never commit those unless asked.**
- Update `docs/TRACKER.md` (stable IDs, never reuse) when fixing or finding issues. Commit message states what was verified.
- Asking before restarting the running app or `--live` deploys is expected.

## Delegation
Haiku is fine for docs, renames, QML plumbing with a stated check (`cargo check`, `qmllint`, tests). Keep on Sonnet: auth/token code, async/`Send` boundaries,
cxx-qt/FFI, shared types, sync correctness, and anything where the root cause is unknown.
