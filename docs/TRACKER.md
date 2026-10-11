# OmarchyLook — outstanding issues and plans

Single index of open work. Add new items at the top of the relevant section; move finished ones to **Done** with the commit hash.
IDs are stable (never reused). Status: `open` | `investigating` | `planned` | `in progress` | `done`.

## Issues

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
- **Fix:** `tools/render/omarchylook-render.c` (WebKitGTK 4.1, ~200 lines of C) renders the HTML to a PNG plus link boxes in its own process; `MessagePreview.qml` shows the PNG with a click area per link. A GTK widget cannot be embedded in a Quickshell window on Wayland, so a separate process is the embedding. Crash/hang/timeout (20 s) of the helper only means that message falls back to the Qt renderer; 3 failures disable the helper for the session.
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
| Compose attachments: in-app file browser, chips, Graph small + upload-session (Gmail refused) | `docs/PLAN-compose.md` | built 2026-10-11 (branch `feature/attachments`); live send untested |
| Received-mail attachments: list, picker (`A`), save one / all (`S`) into Downloads (Graph only) | `docs/PLAN-attachments.md` | built 2026-10-11 (branch `feature/attachments`) |

## Done
- (none yet)
