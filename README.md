# OmaLook

A native Linux mail, calendar, contacts, tasks and SMS client, styled for Omarchy and driven from the
keyboard. It is a Rust backend daemon with a Qt/QML user interface (it runs under Quickshell).
Mail and calendar come from Microsoft Graph (Exchange / Outlook) and Gmail; the providers live in
`src/providers/`.

## Build, run, test

```bash
./build.sh               # debug build into ./target (or OMALOOK_BUILD_DIR)
./build.sh --release     # release build
./build.sh --live        # release build installed as the daily app; stops and restarts running copies
```

Tests. Never run the Rust suite without the skip: one test waits on `wl-copy` (OL-005).

```bash
cargo test --lib -- --skip test_copy_to_clipboard
for t in tests/*.test.js; do node "$t" >/dev/null || echo "FAIL $t"; done
```

Requirements: a Rust toolchain (rustup), Qt 6, and optionally `webkit2gtk-4.1` and GTK 3 headers
for the HTML mail renderer (without them the app uses Qt's rich-text renderer).

## Layout

- `src/`: the Rust backend. `main.rs` is the entry point and HTTP accept loop; `http/` holds the typed
  router, the request guard and the route groups (`http/routes/`); `email_daemon.rs` and
  `calendar_daemon.rs` do sync; `providers/` the mail and calendar APIs; `db.rs` and `db/` the SQLite
  store with FTS5; `compose.rs`, `outbox.rs`, `search.rs`, `sms_daemon.rs`; `qt_bridge/`.
- `qml/`: the UI (`shell.qml`, `AppShell.qml`, and `mail/ compose/ keys/ settings/ state/ views/ Ui/`).
- `plugin/omalook-compose/`: the C++ QML plugin for the compose editor.
- `tools/render/`: the WebKitGTK helper process that renders HTML mail to PNG.
- `docs/`: plans, specs and the tracker. `COMPONENT_REFERENCE.md` at the repo root lists dependency versions.

## Documentation

- [`docs/TRACKER.md`](docs/TRACKER.md): open and fixed issues, with stable IDs.
- [`docs/privacy-policy.md`](docs/privacy-policy.md): what is stored locally, what leaves the machine,
  and how to remove it.
- [`AGENTS.md`](AGENTS.md): working notes for developers and coding agents, including the security rules.

## License

MIT
