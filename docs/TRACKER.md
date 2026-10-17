# OmaLook — outstanding issues and plans

Single index of open work. Add new items at the top of the relevant section; move finished ones to **Done** with the commit hash.
IDs are stable (never reused). Status: `open` | `investigating` | `planned` | `in progress` | `done`.

## Issues

### OL-006 — Harden the UI backend port 27182: no auth, CORS `*` (planned, not started)
- **Seen:** 2026-10-16 while designing MCP. `start_http_trigger_server` (`src/main.rs`) answers every route on `127.0.0.1:27182` without authentication and with `Access-Control-Allow-Origin: *`. Any local process, or any web page open in a browser, can call `/messages/action` (archive/trash/delete), `/compose/send`, `/sms/send`, `/accounts/remove`, `/settings/*`.
- **Why it matters for MCP:** the MCP listener (port 27183, bearer token, Origin/Host checks) enforces permissions, but they are moot while this port lets anything do the same actions directly.
- **Sketch (not decided):** per-launch token handed to the QML UI and required on every route; drop the CORS header; check `Host`/`Origin`; later the single-instance guard on the port.
- **Decision (2026-10-16):** do not tackle yet. Fold into OL-007.

### OL-007 — Whole-app security analysis (planned, not started)
- **Scope (user, 2026-10-16):** a separate pass over every security issue in OmaLook, not just MCP. Starting list: OL-006; unauthenticated routes; the `/fs/list` file browser; attachment save paths; token/keyring handling; the render helper's isolation; what is written to logs and `perf.log`; the MCP surface once built.

### OL-008 — OS notification integration (planned, not started)
- **Wanted (user, 2026-10-16):** hook into the desktop notification system (freedesktop `org.freedesktop.Notifications` over D-Bus, `zbus` is already a dependency): new mail/SMS/invite alerts, and MCP "ask" approval prompts as notifications with actions when the OmaLook window is closed or the daemon runs in the background.
- **Until then:** MCP approval prompts are in-app only (keyboard-driven toast). See `notes/DESIGN-mcp-server.md` §4.3.

### OL-005 — `test_copy_to_clipboard` hangs; find the root cause (open)
- **Seen:** every full `cargo test --lib` run for weeks blocks forever at 0% CPU on `qt_bridge::auth_bridge::tests::test_copy_to_clipboard` (re-confirmed 2026-10-13); until now it was skipped with `--skip`. Predates all recent work.
- **Stopgap (2026-10-13):** the test runs the call on a thread and panics after 5 s, so the suite finishes; the test now **fails** (red) until this is fixed. It does not fix the hang.
- **Evidence:** the hung run leaves an orphaned `wl-copy` process (no xclip on this machine) waiting on stdin, consistent with the suspect below. `pkill -x wl-copy` clears them.
- **Prime suspect (unconfirmed, from reading the code):** `AuthBridge::copy_to_clipboard` (`src/qt_bridge/auth_bridge.rs`) calls `child.wait()` on `xclip` while still holding the child's `stdin` pipe, so xclip never sees EOF and never exits. Likely a real bug in the app's copy-to-clipboard too, not only the test. Same pattern may exist in the `wl-copy` branch.
- **To do:** confirm (run it under `strace -f` or run `xclip`/`wl-copy` by hand); `drop(stdin)` before `wait()`; check what `xclip` does under Wayland/XWayland; then remove the stopgap or keep it as a guard, and re-test the copy button in the UI.

### OL-001 — UI hangs at ~100% CPU on one thread (open)
- **Seen:** 2026-10-07, `quickshell -p /opt/omalook/qml`, PID 3868852. Window stopped responding; one thread pinned at ~97% CPU
  from startup onward (4:09 CPU in 4:15 elapsed); had to be killed by hand.
- **Likely trigger (user report):** rapid scrolling from one message across many others in very quick succession
  (each selection starts a body fetch + rich-text layout in the preview pane).
- **Evidence so far:** log (`/run/user/1000/quickshell/by-pid/<pid>/log.log`) shows
  `MessagePreview.qml:338 Connection refused` (the `Text.RichText` body), a `QIODevice::read (QSslSocket): device not open`
  warning, and 55x `QFont::setPixelSize: Pixel size <= 0 (0)`. No stack trace: `ptrace_scope=1` blocks `eu-stack`/`gdb` attach.
  The Rust daemon was alive when the user ran `ps` (0:01 CPU); it died from the user's `pkill`, so it is **not** the cause.
- **Hypotheses (unconfirmed):**
  1. Body requests not cancelled/debounced on fast selection change, so stale responses pile up and re-lay-out the HTML.
  2. `Text.RichText` layout loop on a particular message (large/nested table, image with 0 size, remote `<img>` fetch retries).
  3. Binding loop from the zero-pixel-size font warnings.
- **To do:** reproduce on a debug run (`QT_LOGGING_RULES`, perf timeline in `perf.rs`); capture the offending message id;
  debounce body fetch (~120 ms) and ignore stale responses by message id; run with `sudo sysctl kernel.yama.ptrace_scope=0`
  once to get a stack if it recurs. Find the source of the `Pixel size <= 0` warnings.
- **Note:** to kill it, use the PID of `quickshell -p /opt/omalook/qml`, never `pkill quickshell` (that is also the Omarchy desktop shell).

### OL-004 — quickshell SIGSEGV opening HTML mail with remote images (fixed 2026-10-10)
- **Seen:** crash report `~/.cache/quickshell/crashes/6v922t8npmt` (pid 3468927): SIGSEGV in `QUrl::resolved` <- `QTextDocument::resource` <- `QQuickText::resourceRequestFinished`, right after `Connection refused` / `QSslSocket: device not open` from the HTML `Text` in `MessagePreview.qml`.
- **Cause:** Qt's `Text.RichText` fetches remote `<img>` (and `background=`) itself; the completion callback crashes when the request fails or the text has changed meanwhile. Marketing mail with tracking pixels triggers it.
- **Fix:** `Fmt.sanitizeHtml` never passes Qt a remote URL any more: allowed remote images become `[alt]` links opened in the browser; `background`/`srcset`/`poster` attributes are stripped. data: images still render inline.

### OL-002 — HTML rendering is not correct (fixed 2026-10-10 with a WebKitGTK helper)
- **Cause:** Qt `Text.RichText` has no `border-radius`, `box-shadow`, inline-block layout or `<style>` support, so styled mail (Stripe receipts, newsletters) came out as flat boxes with collapsed columns.
- **Fix:** `tools/render/omalook-render.c` (WebKitGTK 4.1, ~200 lines of C) renders the HTML to a PNG plus link boxes in its own process; `MessagePreview.qml` shows the PNG with a click area per link. A GTK widget cannot be embedded in a Quickshell window on Wayland, so a separate process is the embedding. Crash/hang/timeout (20 s) of the helper only means that message falls back to the Qt renderer; 3 failures disable the helper for the session.
- **Isolation:** ephemeral WebKit data (no cookies/cache), page JS blocked (CSP `script-src 'none'` + only our own link query runs), navigation and popups denied, no remote loads unless `--images` (message/sender allowed).
- **Build/ship:** `build.sh` compiles it to `bin/` when `webkit2gtk-4.1`, gtk3 and gcc exist (else a warning and Qt fallback); `--live` installs it to `/opt/omalook/bin`. Runtime needs `libwebkit2gtk-4.1` (Arch: `webkit2gtk-4.1`).
- **Known limits:** no text selection in the WebKit view (use the System view to copy text); blocked remote images show as WebKit's empty image boxes; re-renders on every selection/resize (no cache yet).

### OL-003 — Make hyperlinks clickable in "system font" view (open)
- **Seen:** 2026-10-07, user request. In system-font (plain) rendering, URLs in the body are not clickable.
- **To do:** in `Fmt.textToHtml`/the system-font path, linkify `http(s)://`, `www.` and `mailto:` text (escape first, then wrap
  in `<a href>`), keep `onLinkActivated: Qt.openUrlExternally`. Only http(s) and mailto schemes; show the real target on hover.
  Open question: all links or only "some" (user said "some hyperlinks")? Clarify which (e.g. only URLs that appear as bare text,
  not link-text-differs-from-target which is a phishing signal).

## Plans

| Plan | File | Status |
|---|---|---|
| Compose: system-font / HTML modes, drafts, send | `docs/PLAN-compose.md` | planned; Phase D spike passed 2026-10-07; UI first draft + Phase A (send, outbox, undo delay) built 2026-10-08 |
| Mail list views / field chooser | `notes/PLAN-mail-list-views.md` (gitignored `notes/`) | not started |
| Keyboard model: vim nav, `g` modules, palette, mail actions | `docs/PLAN-keyboard.md` | planned (plan written 2026-10-09; decisions Q1-Q6 settled) |
| Mail details (sender name, To/Cc/Bcc, categories, sent, importance, attachments) + categories key `t` | `docs/PLAN-mail-fields.md` | built 2026-10-09 (branch `feature/mail-fields`) |
| Compose attachments: in-app file browser, chips, Graph small + upload-session (Gmail refused) | `docs/PLAN-compose.md` | built 2026-10-11; live Graph send confirmed working (small file); large-file upload session and reply/forward with attachments still unchecked |
| Search and filtering: `/` prompt, keyword patterns, quick filters, history, `fullbody:` via Graph / Gmail | `docs/PLAN-search.md` | built 2026-10-12 (branch `feature/search-filter`); Gmail `fullbody:` not yet tried live |
| Received-mail attachments: list, picker (`A`), save one / all (`S`) into Downloads (Graph only) | `docs/PLAN-attachments.md` | built 2026-10-11 (merged to main) |

## Backlog — mutt-parity gaps (compared 2026-10-11; tackle in this order)

Where OmaLook already leads mutt: native Graph + OAuth, calendar and invitations, contacts, SMS, HTML mail via the
WebKit helper, undo-send, rebindable keys with which-key and a palette. Mutt's edge is mail handling:

1. [x] **Search and filtering**: done 2026-10-12, see `docs/PLAN-search.md` (the `l` limit view was dropped on purpose).
2. [ ] **Threading**: thread view, collapse/expand, thread-wide actions.
3. [ ] **Archive / delete / move**: queued today but never checked against Graph or Gmail. Verify live, then fix what breaks.
4. [ ] **Bulk operations**: act on all marked messages, mark by pattern.
5. [ ] **Drafts** (PLAN-compose.md Phase B2): save, resume, sync to the provider, crash recovery.
6. [ ] **Gmail attachments** (send): MIME multipart in `build_mime`; the UI currently refuses them for Gmail.
7. [ ] **Compose extras**: signatures, external `$EDITOR`, forward-as-attachment, bounce, reply-to-list, aliases.
8. [ ] **Security**: GPG / S/MIME sign and encrypt.
9. [ ] **Offline**: test and harden use without a network (SQLite cache exists, untested offline).
10. [ ] **Automation**: per-account / per-folder hooks and macros.
11. [ ] Unchecked live paths for compose attachments: file of 3 MB or more (upload session), reply and forward with attachments.
12. [ ] Inline images (cid) in HTML compose.

## Done
- Backlog 1, search and filtering (2026-10-12): `9f07565` pattern engine, `99bb981` prompt and list, `78db1be` cat fix, `6a7d613` quick filters + history, `7e13369` `fullbody:` via provider. Also fixed in this branch: the project rename had changed the keyring service name, which hid every saved sign-in.
