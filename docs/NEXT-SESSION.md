# Next session (written 2026-11-14, main at 31abd6c)

## State
- R3 deployed. R4 done in tree except R4.6; **not deployed**. Deploy = `./build.sh --live` (restarts the app; Adam runs it or approves it).
- In tree since the last deploy: OL-090 Ctrl+V picture paste, OL-094 contact editor can always be left, OL-093 contact photo row (Ctrl-O),
  OL-092 Gmail History API sync, R4.1 dependency upgrades (keyring 3, reqwest 0.13, rusqlite 0.42, oauth2 5, zbus 5),
  R4.2 qt_bridge removed, R4.3 retention setting removed, R4.5 LIKE ESCAPE, R4.7 zero warnings, flaky tests fixed.
- Tests: `cargo test --lib` 884 pass (the `--skip test_copy_to_clipboard` is no longer needed); node `for t in tests/*.test.js; do node $t; done`.
  `cargo clippy --all-targets` 0 warnings; keep it there.

## 1. After Adam deploys: live checks (only Adam can do these on screen)
1. Contacts: `e`, Tab past Notes (must wrap to First name), Esc out; mouse Cancel/Save buttons; `Ctrl-O` photo (.jpg) on a synced contact.
2. Compose: Ctrl+V a copied picture in HTML mode (inline) and text mode (attached as a file).
3. Sign-in refresh after the oauth2/keyring upgrade: Microsoft and Google accounts still sync after a token expiry (~1 h).
   Keyring 3 was checked read-only: all 3 saved sign-ins found.
4. Gmail (History API, tested only against a fake server): new mail, read state, deletes and label moves show up.
   Watch for: multi-label messages in a different folder than before; anything missed until the every-5th-cycle reconcile.
5. SMS via KDE Connect (zbus 5): receive, send, read sync.
Logs: backend `/run/user/1000/omalook-restart.log`; UI `quickshell log /run/user/1000/quickshell/by-id/<id>/log.qslog`.

## 2. Open work, in suggested order
- **OL-095** Graph 404 (`ErrorItemNotFound`) about once a minute after account `dcb81dbd`'s mail sync. Log the route name on Graph
  errors, find the caller (likely a prefetch or a queued action for a gone item), treat 404 on a gone item as final.
- **Tab-out trap in other editors** (OL-094 follow-up): calendar event editor, settings pages, any page with `editing: true`.
  Reproduce with the harness below (11x Tab, then Esc); fix with the same pattern (wrap Tab inside, `enabled: false` behind,
  implement `escapeEditing()` so AppShell's escape hatch works).
- Status bar shows list hints (`j move`, `h back`) while the contact editor is open.
- Contact photo: no thumbnail in the editor; `Photo set` text stays until close.
- **R4.6** Unix socket instead of TCP for the UI: Adam's design decision; ask before starting.
- Deferred upgrades: ureq 3 (error-model rewrite, ~22 sites), zbus 6 (drops `zbus::blocking`), keyring 4.
- Stale cxx-qt mentions in `COMPONENT_REFERENCE.md`, `PHASE1_*.md`, older docs.
- Unverified older items: R3.5 snapshot fixture rename `graph`→`exchange` in `src/mcp/read_snapshot_tests.rs` (reason unknown).

## Tools that worked this session
- Sandbox differential: `/mnt/d/r3-sbx/` (`sbx.sh`, `probe.py`, `compare.py`, `reqs-*.json`, `seed.sql`; 9 groups, 601 requests).
  Run old vs new binary per group, `compare.py old.jsonl new.jsonl`; noise = random ids/handles, serials, binary path, timestamped file names.
- Whole-app UI harness with QtTest keys/clicks against the sandboxed backend: `/mnt/d/people-harness/` (`sbx.sh`, `qml/shell.qml` driver).
  Method in skill omalook-feature-phases, `references/qml-standalone-view-testing.md`.
- Lanes: one worktree per lane under `/mnt/d/omalook-<lane>` with its own target dir; brief `~/.hermes/cache/scratch/r4-brief.md`.
